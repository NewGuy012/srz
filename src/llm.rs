use crate::chunker::TextChunker;
use crate::cli::SummaryStyle;
use crate::ui::Ui;
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [RequestMessage<'a>],
    temperature: f32,
    max_tokens: u32,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    stream_options: Option<StreamOptions>,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatCompletionChunk {
    #[serde(default)]
    choices: Vec<ChunkChoice>,
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct ChunkChoice {
    delta: ChunkDelta,
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct ChunkDelta {
    content: Option<String>,
    /// Emitted by llama.cpp / Gemma 4 / DeepSeek during internal chain-of-thought
    reasoning_content: Option<String>,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
    usage: Option<TokenUsage>,
}

#[derive(Deserialize)]
struct TokenUsage {
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    total_tokens: Option<u32>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ResponseMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct ResponseMessage {
    content: String,
}

/// Controls whether tokens are streamed live to stdout or buffered silently behind a spinner.
#[derive(Clone, Copy)]
pub enum StreamMode<'a> {
    /// Stream visible content tokens directly to stdout as they arrive.
    Stdout { header_title: &'a str },
    /// Buffer tokens silently while updating the spinner token count (e.g. for `-o <FILE>` or map-reduce sections).
    Buffered,
}

/// Output returned by an LLM summarization call.
pub struct LlmOutput {
    pub content: String,
    pub truncated: bool,
    pub warning: Option<String>,
    pub streamed_to_stdout: bool,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
}

/// Maximum words of grounded reference context for a 32k context window (~16k-18k tokens),
/// leaving ample headroom for up to 8,192 completion tokens (thinking + summary).
pub const MAX_CONTEXT_WORDS: usize = 12_000;

/// Maximum words per chunk when map-reducing extra-long articles (> 12,000 words).
const CHUNK_MAX_WORDS: usize = 10_000;
const CHUNK_OVERLAP_WORDS: usize = 500;

pub struct LlmClient {
    client: reqwest::Client,
    endpoint: String,
    fallback_endpoint: Option<String>,
    model: String,
    max_tokens: u32,
}

impl LlmClient {
    pub fn new(base_url: &str, model: &str, api_key: &str, max_tokens: u32) -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if !api_key.is_empty()
            && let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", api_key))
        {
            headers.insert(AUTHORIZATION, val);
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(300))
            .build()
            .context("Failed to build LLM HTTP client")?;

        let normalized_base = base_url.trim_end_matches('/');
        let endpoint = format!("{}/chat/completions", normalized_base);
        let fallback_endpoint = if endpoint.contains("localhost") {
            Some(endpoint.replace("localhost", "127.0.0.1"))
        } else {
            None
        };

        Ok(Self {
            client,
            endpoint,
            fallback_endpoint,
            model: model.to_string(),
            max_tokens,
        })
    }

    /// Streams completion tokens via SSE (`stream: true`) with live thinking token feedback on stderr.
    pub async fn complete_streaming(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        spinner_label: &'static str,
        done_label: &str,
        mode: StreamMode<'_>,
    ) -> Result<LlmOutput> {
        let start_time = std::time::Instant::now();
        let pb = Ui::spinner(spinner_label);

        let combined_user_content: Cow<'_, str> = if system_prompt.is_empty() {
            Cow::Borrowed(user_prompt)
        } else {
            Cow::Owned(format!("Instructions:\n{}\n\n{}", system_prompt, user_prompt))
        };

        let messages = [RequestMessage {
            role: "user",
            content: &combined_user_content,
        }];

        let body = ChatCompletionRequest {
            model: &self.model,
            messages: &messages,
            temperature: 0.3,
            max_tokens: self.max_tokens,
            stream: true,
            stream_options: Some(StreamOptions { include_usage: true }),
        };

        let mut response = self.client.post(&self.endpoint).json(&body).send().await;

        // On macOS, localhost can resolve to IPv6 (::1). Fallback to 127.0.0.1 if needed.
        if response.is_err()
            && let Some(alt_endpoint) = &self.fallback_endpoint
            && let Ok(alt_resp) = self.client.post(alt_endpoint).json(&body).send().await
        {
            response = Ok(alt_resp);
        }

        let mut response = match response {
            Ok(resp) => resp,
            Err(err) => {
                pb.finish_and_clear();
                anyhow::bail!(
                    "Failed to communicate with LLM endpoint at '{}': {}. \
                     Please verify your llama.cpp app is open and listening on port 9931.",
                    self.endpoint,
                    err
                );
            }
        };

        if !response.status().is_success() {
            pb.finish_and_clear();
            let status = response.status();
            let err_text = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM server returned HTTP error {}: {}", status, err_text);
        }

        // Fallback if server responds with a non-streaming JSON body instead of text/event-stream
        let is_event_stream = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|ct| ct.contains("text/event-stream"))
            .unwrap_or(true);

        if !is_event_stream {
            let completion: ChatCompletionResponse = match response.json().await {
                Ok(c) => c,
                Err(e) => {
                    pb.finish_and_clear();
                    return Err(e).context("Failed to parse LLM JSON response");
                }
            };
            Ui::finish_spinner(&pb, done_label);

            let choice = completion
                .choices
                .first()
                .ok_or_else(|| anyhow::anyhow!("LLM returned empty choices"))?;
            let truncated = choice.finish_reason.as_deref() == Some("length");
            let warning = Self::build_truncation_warning(truncated, completion.usage.as_ref());

            let elapsed = start_time.elapsed();
            let duration_ms = elapsed.as_millis() as u64;
            let prompt_tokens = completion.usage.as_ref().and_then(|u| u.prompt_tokens);
            let completion_tokens = completion.usage.as_ref().and_then(|u| u.completion_tokens);
            let tokens_per_second = completion_tokens.and_then(|c| {
                let s = elapsed.as_secs_f64();
                if s > 0.05 {
                    Some(c as f64 / s)
                } else {
                    None
                }
            });

            return Ok(LlmOutput {
                content: choice.message.content.trim().to_string(),
                truncated,
                warning,
                streamed_to_stdout: false,
                prompt_tokens,
                completion_tokens,
                duration_ms,
                tokens_per_second,
            });
        }

        let mut line_buf: Vec<u8> = Vec::with_capacity(512);
        let mut content_buf = String::with_capacity(2048);
        let mut thinking_tokens: usize = 0;
        let mut output_tokens: usize = 0;
        let mut started_visible = false;
        let mut truncated = false;
        let mut last_usage: Option<TokenUsage> = None;

        while let Some(chunk) = response
            .chunk()
            .await
            .context("Error reading LLM stream")?
        {
            line_buf.extend_from_slice(&chunk);

            while let Some(newline_pos) = line_buf.iter().position(|&b| b == b'\n') {
                let line_bytes = &line_buf[..newline_pos];
                if let Ok(line_str) = std::str::from_utf8(line_bytes) {
                    let trimmed = line_str.trim();
                    if let Some(data) = trimmed.strip_prefix("data:") {
                        let payload = data.trim();
                        if payload == "[DONE]" {
                            line_buf.drain(..=newline_pos);
                            break;
                        }

                        if let Ok(event) = serde_json::from_str::<ChatCompletionChunk>(payload) {
                            if event.usage.is_some() {
                                last_usage = event.usage;
                            }

                            if let Some(choice) = event.choices.first() {
                                if choice.finish_reason.as_deref() == Some("length") {
                                    truncated = true;
                                }

                                if let Some(reasoning) = &choice.delta.reasoning_content
                                    && !reasoning.is_empty()
                                    && !started_visible
                                {
                                    thinking_tokens += 1;
                                    pb.set_message(format!(
                                        "{} (thinking: {} tokens)",
                                        spinner_label, thinking_tokens
                                    ));
                                }

                                if let Some(delta_text) = &choice.delta.content
                                    && !delta_text.is_empty()
                                {
                                        if !started_visible {
                                            let clean_start = delta_text.trim_start();
                                            if !clean_start.is_empty() {
                                                started_visible = true;
                                                output_tokens += 1;
                                                content_buf.push_str(clean_start);

                                                match mode {
                                                    StreamMode::Stdout { header_title } => {
                                                        pb.finish_and_clear();
                                                        Ui::header(&format!(
                                                            "Summary: {}",
                                                            header_title
                                                        ));
                                                        print!("{}", clean_start);
                                                        let _ = io::stdout().flush();
                                                    }
                                                    StreamMode::Buffered => {
                                                        pb.set_message(format!(
                                                            "{} ({} tokens)",
                                                            spinner_label, output_tokens
                                                        ));
                                                    }
                                                }
                                            }
                                        } else {
                                            output_tokens += 1;
                                            content_buf.push_str(delta_text);

                                            match mode {
                                                StreamMode::Stdout { .. } => {
                                                    print!("{}", delta_text);
                                                    let _ = io::stdout().flush();
                                                }
                                                StreamMode::Buffered => {
                                                    pb.set_message(format!(
                                                        "{} ({} tokens)",
                                                        spinner_label, output_tokens
                                                    ));
                                                }
                                            }
                                        }
                                    }
                            }
                        }
                    }
                }
                line_buf.drain(..=newline_pos);
            }
        }

        let streamed_to_stdout = matches!(mode, StreamMode::Stdout { .. }) && started_visible;
        if streamed_to_stdout {
            println!("\n");
            Ui::success(done_label);
        } else {
            Ui::finish_spinner(&pb, done_label);
        }

        let elapsed = start_time.elapsed();
        let duration_ms = elapsed.as_millis() as u64;
        let prompt_tokens = last_usage.as_ref().and_then(|u| u.prompt_tokens);
        let completion_tokens = last_usage
            .as_ref()
            .and_then(|u| u.completion_tokens)
            .or(Some(output_tokens as u32));
        let tokens_per_second = completion_tokens.and_then(|c| {
            let s = elapsed.as_secs_f64();
            if s > 0.05 {
                Some(c as f64 / s)
            } else {
                None
            }
        });

        let warning = Self::build_truncation_warning(truncated, last_usage.as_ref());

        Ok(LlmOutput {
            content: content_buf.trim().to_string(),
            truncated,
            warning,
            streamed_to_stdout,
            prompt_tokens,
            completion_tokens,
            duration_ms,
            tokens_per_second,
        })
    }

    fn build_truncation_warning(truncated: bool, usage: Option<&TokenUsage>) -> Option<String> {
        if !truncated {
            return None;
        }
        let usage_detail = usage
            .and_then(|u| {
                Some(format!(
                    " (prompt: {} tokens, generated: {} tokens, total: {} tokens)",
                    u.prompt_tokens?, u.completion_tokens?, u.total_tokens?
                ))
            })
            .unwrap_or_default();
        Some(format!(
            "Output reached the token limit{}. You can increase it with `--max-tokens <NUM>` or increase your model's context length.",
            usage_detail
        ))
    }

    /// Summarize webpage content (with automatic chunking for long articles)
    pub async fn summarize_webpage(
        &self,
        title: Option<&str>,
        url: &str,
        content: &str,
        style: SummaryStyle,
        mode: StreamMode<'_>,
    ) -> Result<LlmOutput> {
        let title_str = title.unwrap_or("Untitled Webpage");
        let word_count = content.split_whitespace().count();

        // If the webpage exceeds MAX_CONTEXT_WORDS (12,000 words), chunk and map-reduce
        if word_count > MAX_CONTEXT_WORDS {
            let chunks = TextChunker::chunk(content, CHUNK_MAX_WORDS, CHUNK_OVERLAP_WORDS);
            if chunks.len() > 1 {
                let mut section_summaries = Vec::with_capacity(chunks.len());
                for (i, chunk) in chunks.iter().enumerate() {
                    let section_sys = "You are an assistant summarizing a section of a long article. Be factual and concise.";
                    let section_user = format!(
                        "Article: '{}'\nSection {} of {}:\n\n{}",
                        title_str,
                        i + 1,
                        chunks.len(),
                        chunk
                    );
                    let part = self
                        .complete_streaming(
                            section_sys,
                            &section_user,
                            "Summarizing article chunk...",
                            &format!("Chunk {}/{} summarized.", i + 1, chunks.len()),
                            StreamMode::Buffered,
                        )
                        .await?;
                    section_summaries.push(part.content);
                }

                let combined_sections = section_summaries.join("\n\n---\n\n");
                let synthesis_sys = "You are an expert analyst. Synthesize the provided section summaries into a single cohesive Markdown summary.";
                let synthesis_user = format!(
                    "Article: '{}'\nURL: {}\nStyle: {}\n\nSection Summaries:\n{}",
                    title_str,
                    url,
                    style.prompt_guidance(),
                    combined_sections
                );
                return self
                    .complete_streaming(
                        synthesis_sys,
                        &synthesis_user,
                        "Synthesizing final webpage summary...",
                        "Summary generated.",
                        mode,
                    )
                    .await;
            }
        }

        // Direct single-pass summarization
        let system_prompt = "You are an expert analyst and summarizer. \
            Your goal is to extract the core insights, arguments, data, and conclusions from the provided text. \
            Write in clear, clean Markdown.";

        let user_prompt = format!(
            "Webpage URL: {}\nArticle Title: {}\nStyle instructions: {}\n\nContent:\n---\n{}\n---",
            url,
            title_str,
            style.prompt_guidance(),
            content
        );

        self.complete_streaming(
            system_prompt,
            &user_prompt,
            "Generating summary with local LLM...",
            "Summary generated.",
            mode,
        )
        .await
    }

    /// Summarize a book using retrieved web synopsis context
    pub async fn summarize_book_with_context(
        &self,
        title: &str,
        author: Option<&str>,
        source_url: &str,
        web_context: &str,
        style: SummaryStyle,
        mode: StreamMode<'_>,
    ) -> Result<LlmOutput> {
        let author_line: Cow<'_, str> = match author {
            Some(a) => Cow::Owned(format!("Author: {}\n", a)),
            None => Cow::Borrowed(""),
        };

        // Zero-copy slice when web_context fits within MAX_CONTEXT_WORDS (12,000 words)
        let bounded_context = truncate_words_preserving_lines(web_context, MAX_CONTEXT_WORDS);

        let system_prompt = "You are a literary analyst and scholar. \
            You provide thorough, objective, and deeply insightful book summaries grounded in the provided reference materials. \
            Use clean Markdown formatting with clear section headers.";

        let user_prompt = format!(
            "Book Title: {}\n{}Reference Source: {}\nStyle instructions: {}\n\n\
            Retrieved Context:\n---\n{}\n---\n\n\
            Please provide a structured summary covering:\n\
            1. Overview & Context (Author, background, genre, core thesis)\n\
            2. Main Themes & Key Ideas\n\
            3. Summary / Key Takeaways\n\
            4. Notable Impact or Significance",
            title,
            author_line,
            source_url,
            style.prompt_guidance(),
            bounded_context
        );

        self.complete_streaming(
            system_prompt,
            &user_prompt,
            "Synthesizing grounded book summary with local LLM...",
            "Grounded summary complete.",
            mode,
        )
        .await
    }

    /// Summarize a book purely from the LLM's trained memory
    pub async fn summarize_book_from_memory(
        &self,
        title: &str,
        author: Option<&str>,
        style: SummaryStyle,
        mode: StreamMode<'_>,
    ) -> Result<LlmOutput> {
        let author_line: Cow<'_, str> = match author {
            Some(a) => Cow::Owned(format!("Author: {}\n", a)),
            None => Cow::Borrowed(""),
        };

        let system_prompt = "You are a literary analyst and scholar. \
            Summarize the requested book using your trained knowledge base. \
            If you are genuinely unfamiliar with this specific book or title, state clearly that the book was not recognized. \
            Use clean Markdown formatting with clear section headers.";

        let user_prompt = format!(
            "Book Title: {}\n{}Style instructions: {}\n\n\
            Please provide a structured summary covering:\n\
            1. Overview & Context (Author, background, genre, core premise)\n\
            2. Core Themes & Arguments / Narrative Arc\n\
            3. Key Takeaways & Lessons\n\
            4. Critical Reception & Significance",
            title,
            author_line,
            style.prompt_guidance()
        );

        self.complete_streaming(
            system_prompt,
            &user_prompt,
            "Generating book summary from LLM trained knowledge...",
            "Summary generated from LLM memory.",
            mode,
        )
        .await
    }
}

/// Truncates `text` to at most `max_words` words while preserving original line and paragraph breaks.
/// Returns `Cow::Borrowed(text)` with zero allocations when `text` has `<= max_words` words.
pub fn truncate_words_preserving_lines(text: &str, max_words: usize) -> Cow<'_, str> {
    if max_words == 0 {
        return Cow::Borrowed("[Context trimmed for token budget]");
    }

    let mut words = text.split_whitespace();
    let Some(nth_word) = words.nth(max_words - 1) else {
        return Cow::Borrowed(text);
    };

    // If there are no further words after `max_words`, return the original slice zero-copy
    if words.next().is_none() {
        return Cow::Borrowed(text);
    }

    // Compute byte offset of the end of the `max_words`-th word within `text`
    let end_offset = (nth_word.as_ptr() as usize) - (text.as_ptr() as usize) + nth_word.len();
    let prefix = text[..end_offset].trim_end();
    Cow::Owned(format!("{}\n\n[Context trimmed for token budget]", prefix))
}
