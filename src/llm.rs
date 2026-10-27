use crate::chunker::TextChunker;
use crate::cli::SummaryStyle;
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::Duration;

#[derive(Serialize)]
struct ChatCompletionRequest<'a> {
    model: &'a str,
    messages: &'a [RequestMessage<'a>],
    temperature: f32,
    max_tokens: u32,
}

#[derive(Serialize)]
struct RequestMessage<'a> {
    role: &'static str,
    content: &'a str,
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

/// Output returned by an LLM summarization call.
pub struct LlmOutput {
    pub content: String,
    pub truncated: bool,
    pub warning: Option<String>,
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
        if !api_key.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", api_key)) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        let client = reqwest::Client::builder()
            .default_headers(headers)
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

    /// Generic completion request to OpenAI-compatible endpoint
    pub async fn complete(&self, system_prompt: &str, user_prompt: &str) -> Result<LlmOutput> {
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
        };

        let mut response = self.client.post(&self.endpoint).json(&body).send().await;

        // On macOS, localhost can resolve to IPv6 (::1). Fallback to 127.0.0.1 if needed.
        if response.is_err() {
            if let Some(alt_endpoint) = &self.fallback_endpoint {
                if let Ok(alt_resp) = self.client.post(alt_endpoint).json(&body).send().await {
                    response = Ok(alt_resp);
                }
            }
        }

        let response = response.map_err(|err| {
            anyhow::anyhow!(
                "Failed to communicate with LLM endpoint at '{}': {}. \
                 Please verify your llama.cpp app is open and listening on port 9931.",
                self.endpoint,
                err
            )
        })?;

        if !response.status().is_success() {
            let status = response.status();
            let err_text = response.text().await.unwrap_or_default();
            anyhow::bail!("LLM server returned HTTP error {}: {}", status, err_text);
        }

        let completion: ChatCompletionResponse = response
            .json()
            .await
            .context("Failed to parse LLM JSON response")?;

        let choice = completion
            .choices
            .first()
            .ok_or_else(|| anyhow::anyhow!("LLM returned empty choices"))?;

        let truncated = choice.finish_reason.as_deref() == Some("length");
        let warning = if truncated {
            let usage_detail = completion
                .usage
                .as_ref()
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
        } else {
            None
        };

        Ok(LlmOutput {
            content: choice.message.content.trim().to_string(),
            truncated,
            warning,
        })
    }

    /// Summarize webpage content (with automatic chunking for long articles)
    pub async fn summarize_webpage(
        &self,
        title: Option<&str>,
        url: &str,
        content: &str,
        style: SummaryStyle,
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
                    let part = self.complete(section_sys, &section_user).await?;
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
                return self.complete(synthesis_sys, &synthesis_user).await;
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

        self.complete(system_prompt, &user_prompt).await
    }

    /// Summarize a book using retrieved web synopsis context
    pub async fn summarize_book_with_context(
        &self,
        title: &str,
        author: Option<&str>,
        source_url: &str,
        web_context: &str,
        style: SummaryStyle,
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

        self.complete(system_prompt, &user_prompt).await
    }

    /// Summarize a book purely from the LLM's trained memory
    pub async fn summarize_book_from_memory(
        &self,
        title: &str,
        author: Option<&str>,
        style: SummaryStyle,
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

        self.complete(system_prompt, &user_prompt).await
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
