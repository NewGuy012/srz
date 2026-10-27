use crate::chunker::TextChunker;
use crate::cli::SummaryStyle;
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Serialize, Deserialize, Clone)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatMessage,
}

pub struct LlmClient {
    client: reqwest::Client,
    base_url: String,
    model: String,
    api_key: String,
}

impl LlmClient {
    pub fn new(base_url: &str, model: &str, api_key: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(120))
            .build()
            .context("Failed to build LLM HTTP client")?;

        let normalized_base = base_url.trim_end_matches('/').to_string();

        Ok(Self {
            client,
            base_url: normalized_base,
            model: model.to_string(),
            api_key: api_key.to_string(),
        })
    }

    /// Generic completion request to OpenAI-compatible endpoint
    pub async fn complete(&self, system_prompt: &str, user_prompt: &str) -> Result<String> {
        let endpoint = format!("{}/chat/completions", self.base_url);

        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        if !self.api_key.is_empty() {
            if let Ok(val) = HeaderValue::from_str(&format!("Bearer {}", self.api_key)) {
                headers.insert(AUTHORIZATION, val);
            }
        }

        let combined_user_content = if system_prompt.is_empty() {
            user_prompt.to_string()
        } else {
            format!("Instructions:\n{}\n\n{}", system_prompt, user_prompt)
        };

        let body = ChatCompletionRequest {
            model: self.model.clone(),
            messages: vec![ChatMessage {
                role: "user".to_string(),
                content: combined_user_content,
            }],
            temperature: 0.3,
        };

        let mut response = self
            .client
            .post(&endpoint)
            .headers(headers.clone())
            .json(&body)
            .send()
            .await;

        // On macOS, localhost can resolve to IPv6 (::1). Fallback to 127.0.0.1 if needed.
        if response.is_err() && endpoint.contains("localhost") {
            let alt_endpoint = endpoint.replace("localhost", "127.0.0.1");
            if let Ok(alt_resp) = self
                .client
                .post(&alt_endpoint)
                .headers(headers)
                .json(&body)
                .send()
                .await
            {
                response = Ok(alt_resp);
            }
        }

        let response = response.map_err(|err| {
            anyhow::anyhow!(
                "Failed to communicate with LLM endpoint at '{}': {}. \
                 Please verify your llama.cpp app is open and listening on port 9931.",
                endpoint,
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

        Ok(choice.message.content.trim().to_string())
    }

    /// Summarize webpage content (with automatic chunking for long articles)
    pub async fn summarize_webpage(
        &self,
        title: Option<&str>,
        url: &str,
        content: &str,
        style: SummaryStyle,
    ) -> Result<String> {
        let title_str = title.unwrap_or("Untitled Webpage");
        let word_count = content.split_whitespace().count();

        // If the webpage exceeds 4000 words, chunk and map-reduce
        if word_count > 4000 {
            let chunks = TextChunker::chunk(content, 3000, 300);
            if chunks.len() > 1 {
                let mut section_summaries = Vec::new();
                for (i, chunk) in chunks.iter().enumerate() {
                    let section_sys = "You are an assistant summarizing a section of a long article. Be factual and concise.";
                    let section_user = format!(
                        "Article: '{}'\nSection {} of {}:\n\n{}",
                        title_str,
                        i + 1,
                        chunks.len(),
                        chunk
                    );
                    let part_summary = self.complete(section_sys, &section_user).await?;
                    section_summaries.push(part_summary);
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
    ) -> Result<String> {
        let author_line = author
            .map(|a| format!("Author: {}\n", a))
            .unwrap_or_default();

        // Budget context to ~3,500 words (~4,500 tokens) to safely fit within local LLM context windows
        let bounded_context = if web_context.split_whitespace().count() > 3500 {
            let words: Vec<&str> = web_context.split_whitespace().take(3500).collect();
            format!("{}\n\n[Context trimmed for token budget]", words.join(" "))
        } else {
            web_context.to_string()
        };

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
    ) -> Result<String> {
        let author_line = author
            .map(|a| format!("Author: {}\n", a))
            .unwrap_or_default();

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
