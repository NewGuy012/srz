use crate::ui::{Ui, format_duration_short};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;

pub(crate) const MAX_SSE_LINE_BYTES: usize = 1024 * 1024;
const MAX_CONTENT_BUFFER_BYTES: usize = 10 * 1024 * 1024;
pub(crate) const MAX_HTTP_RESPONSE_BYTES: usize = 10 * 1024 * 1024;

#[derive(Serialize)]
pub(crate) struct StreamOptions {
    pub(crate) include_usage: bool,
}

#[derive(Serialize)]
pub(crate) struct ChatCompletionRequest<'a> {
    pub(crate) model: &'a str,
    pub(crate) messages: &'a [RequestMessage<'a>],
    pub(crate) temperature: f32,
    pub(crate) max_tokens: u32,
    pub(crate) stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) stream_options: Option<StreamOptions>,
}

#[derive(Serialize, Clone, Debug)]
pub struct RequestMessage<'a> {
    pub role: &'static str,
    pub content: &'a str,
}

#[derive(Deserialize)]
pub(crate) struct SseErrorPayload<'a> {
    #[serde(borrow)]
    pub(crate) message: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
pub(crate) struct ChatCompletionChunk<'a> {
    #[serde(default, borrow)]
    pub(crate) choices: Vec<ChunkChoice<'a>>,
    pub(crate) usage: Option<TokenUsage>,
    #[serde(borrow)]
    pub(crate) error: Option<SseErrorPayload<'a>>,
}

#[derive(Deserialize)]
pub(crate) struct ChunkChoice<'a> {
    #[serde(borrow)]
    pub(crate) delta: ChunkDelta<'a>,
    #[serde(borrow)]
    pub(crate) finish_reason: Option<Cow<'a, str>>,
}

#[derive(Deserialize, Default)]
pub(crate) struct ChunkDelta<'a> {
    #[serde(borrow)]
    pub(crate) content: Option<Cow<'a, str>>,
    #[serde(borrow)]
    pub(crate) reasoning_content: Option<Cow<'a, str>>,
}

#[derive(Deserialize)]
pub(crate) struct ChatCompletionResponse {
    pub(crate) choices: Vec<ChatChoice>,
    pub(crate) usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub(crate) struct TokenUsage {
    pub(crate) prompt_tokens: Option<u32>,
    pub(crate) completion_tokens: Option<u32>,
    pub(crate) total_tokens: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct ChatChoice {
    pub(crate) message: ResponseMessage,
    pub(crate) finish_reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ResponseMessage {
    #[serde(default)]
    pub(crate) content: Option<String>,
    #[serde(default)]
    pub(crate) reasoning_content: Option<String>,
}

/// Controls whether tokens are streamed live to stdout or buffered behind a spinner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamMode<'a> {
    Stdout { header_title: &'a str },
    Buffered,
}

/// Output returned by an LLM summarization call.
pub struct LlmOutput {
    pub content: String,
    pub reasoning: Option<String>,
    pub truncated: bool,
    pub warning: Option<String>,
    pub hint: Option<String>,
    pub streamed_to_stdout: bool,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThinkingState {
    Detecting,
    Thinking { in_tag: bool },
    Finished,
}

pub(crate) struct SseStreamState {
    pub(crate) content_buf: String,
    pub(crate) reasoning_buf: String,
    pub(crate) output_tokens: usize,
    pub(crate) started_visible: bool,
    pub(crate) thinking: ThinkingState,
    pub(crate) streamed_thinking: bool,
    pub(crate) pending_tag: String,
    pub(crate) truncated: bool,
    pub(crate) upstream_error: Option<String>,
    pub(crate) last_usage: Option<TokenUsage>,
    pub(crate) needs_stdout_flush: bool,
    pub(crate) stream_renderer: Option<crate::ui::StreamMarkdownRenderer>,
    pub(crate) last_spinner_update: std::time::Instant,
    pub(crate) thinking_start: Option<std::time::Instant>,
    pub(crate) show_thinking_stream: bool,
}

impl SseStreamState {
    pub(crate) fn new(start_time: std::time::Instant, show_thinking_stream: bool) -> Self {
        Self {
            content_buf: String::with_capacity(2048),
            reasoning_buf: String::with_capacity(1024),
            output_tokens: 0,
            started_visible: false,
            thinking: ThinkingState::Detecting,
            streamed_thinking: false,
            pending_tag: String::new(),
            truncated: false,
            upstream_error: None,
            last_usage: None,
            needs_stdout_flush: false,
            stream_renderer: None,
            last_spinner_update: start_time,
            thinking_start: None,
            show_thinking_stream,
        }
    }

    pub(crate) fn is_thinking(&self) -> bool {
        matches!(self.thinking, ThinkingState::Thinking { .. })
    }
}

impl Default for SseStreamState {
    fn default() -> Self {
        Self::new(std::time::Instant::now(), false)
    }
}

fn trailing_tag_prefix(haystack: &str, needle: &str) -> usize {
    let max_k = haystack.len().min(needle.len().saturating_sub(1));
    (1..=max_k)
        .rev()
        .find(|&k| haystack.ends_with(&needle[..k]))
        .unwrap_or(0)
}

impl SseStreamState {
    pub(crate) fn into_output(
        mut self,
        elapsed: Duration,
        streamed_to_stdout: bool,
        usage: Option<TokenUsage>,
        warning: Option<String>,
        fallback_chunk_tokens: bool,
    ) -> LlmOutput {
        let trimmed_len = self.content_buf.trim_end().len();
        self.content_buf.truncate(trimmed_len);
        let reasoning =
            (!self.reasoning_buf.trim().is_empty()).then(|| self.reasoning_buf.trim().to_string());
        let prompt_tokens = usage.and_then(|u| u.prompt_tokens);
        let completion_tokens = usage
            .and_then(|u| u.completion_tokens)
            .or_else(|| fallback_chunk_tokens.then_some(self.output_tokens as u32));

        LlmOutput {
            content: self.content_buf,
            reasoning,
            truncated: self.truncated,
            warning,
            hint: None,
            streamed_to_stdout,
            prompt_tokens,
            completion_tokens,
            duration_ms: elapsed.as_millis() as u64,
            tokens_per_second: compute_tokens_per_second(completion_tokens, elapsed),
        }
    }

    /// Extracts `(content, reasoning)` for non-streaming responses via the same `<think>` state machine.
    pub(crate) fn split_non_streaming(
        content: &str,
        reasoning_content: Option<&str>,
    ) -> (String, Option<String>) {
        let mut state = Self::default();
        let pb = crate::ui::ProgressBar::hidden();
        state.process_delta_parts(
            reasoning_content,
            Some(content),
            &pb,
            "",
            StreamMode::Buffered,
        );
        state.finish_stream(&pb, "", StreamMode::Buffered);
        let out = state.into_output(Duration::ZERO, false, None, None, false);
        (out.content, out.reasoning)
    }

    /// Processes a single SSE line. Returns `true` if `[DONE]`, an upstream error, or buffer cap was hit.
    pub(crate) fn process_line(
        &mut self,
        line_bytes: &[u8],
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) -> bool {
        let Ok(line_str) = std::str::from_utf8(line_bytes) else {
            return false;
        };
        let Some(data) = line_str.trim().strip_prefix("data:") else {
            return false;
        };
        let payload = data.trim();
        if payload == "[DONE]" {
            return true;
        }

        let Ok(event) = serde_json::from_str::<ChatCompletionChunk<'_>>(payload) else {
            return false;
        };

        if let Some(err) = event.error {
            let msg = err
                .message
                .as_deref()
                .map(crate::ui::sanitize_title)
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "Upstream stream error".to_string());
            self.upstream_error = Some(msg);
            return true;
        }

        if event.usage.is_some() {
            self.last_usage = event.usage;
        }

        let Some(choice) = event.choices.first() else {
            return false;
        };
        if choice.finish_reason.as_deref() == Some("length") {
            self.truncated = true;
        }

        self.process_delta_parts(
            choice.delta.reasoning_content.as_deref(),
            choice.delta.content.as_deref(),
            pb,
            spinner_label,
            mode,
        )
    }

    fn process_delta_parts(
        &mut self,
        reasoning_delta: Option<&str>,
        content_delta: Option<&str>,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) -> bool {
        if let Some(reasoning) = reasoning_delta
            && !reasoning.is_empty()
            && !self.started_visible
            && self.append_reasoning_delta(reasoning, false, pb)
        {
            return true;
        }

        if let Some(delta_text) = content_delta
            && !delta_text.is_empty()
            && self.process_content_delta(delta_text, pb, spinner_label, mode)
        {
            return true;
        }

        false
    }

    fn process_content_delta(
        &mut self,
        delta_text: &str,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) -> bool {
        if self.pending_tag.is_empty()
            && self.started_visible
            && matches!(self.thinking, ThinkingState::Finished)
        {
            return self.try_append_content_delta(delta_text, pb, spinner_label, mode);
        }

        let working: Cow<'_, str> = if self.pending_tag.is_empty() {
            Cow::Borrowed(delta_text)
        } else {
            let mut combined = std::mem::take(&mut self.pending_tag);
            combined.push_str(delta_text);
            Cow::Owned(combined)
        };

        let mut offset = 0;
        while offset < working.len() {
            let rest = &working[offset..];
            if matches!(self.thinking, ThinkingState::Thinking { in_tag: true }) {
                if let Some(end_idx) = rest.find("</think>") {
                    if !rest[..end_idx].is_empty()
                        && self.append_reasoning_delta(&rest[..end_idx], true, pb)
                    {
                        return true;
                    }
                    self.thinking = ThinkingState::Thinking { in_tag: false };
                    offset += end_idx + 8;
                    continue;
                }
                let hold = trailing_tag_prefix(rest, "</think>");
                let emit = &rest[..rest.len() - hold];
                if hold > 0 {
                    self.pending_tag.push_str(&rest[rest.len() - hold..]);
                }
                if !emit.is_empty() && self.append_reasoning_delta(emit, true, pb) {
                    return true;
                }
                break;
            }

            if !self.started_visible
                && matches!(
                    self.thinking,
                    ThinkingState::Detecting | ThinkingState::Thinking { in_tag: false }
                )
            {
                if let Some(tag_idx) = rest.find("<think>") {
                    let before = &rest[..tag_idx];
                    if !before.trim().is_empty()
                        && self.try_append_content_delta(before, pb, spinner_label, mode)
                    {
                        return true;
                    }
                    self.thinking = ThinkingState::Thinking { in_tag: true };
                    offset += tag_idx + 7;
                    continue;
                }
                let hold = trailing_tag_prefix(rest, "<think>");
                let emit = &rest[..rest.len() - hold];
                if hold > 0 {
                    self.pending_tag.push_str(&rest[rest.len() - hold..]);
                }
                if !emit.is_empty() && self.try_append_content_delta(emit, pb, spinner_label, mode)
                {
                    return true;
                }
                break;
            }

            if self.try_append_content_delta(rest, pb, spinner_label, mode) {
                return true;
            }
            break;
        }

        false
    }

    pub(crate) fn finish_stream(
        &mut self,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) {
        if !self.pending_tag.is_empty() {
            let leftover = std::mem::take(&mut self.pending_tag);
            if matches!(self.thinking, ThinkingState::Thinking { in_tag: true }) {
                let _ = self.append_reasoning_delta(&leftover, true, pb);
            } else {
                let _ = self.try_append_content_delta(&leftover, pb, spinner_label, mode);
            }
        }
        self.finish_thinking_ui(pb, spinner_label, mode);
    }

    fn try_append_content_delta(
        &mut self,
        text: &str,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) -> bool {
        if self.content_buf.len().saturating_add(text.len()) > MAX_CONTENT_BUFFER_BYTES {
            self.truncated = true;
            return true;
        }
        self.append_content_delta(text, pb, spinner_label, mode);
        false
    }

    fn append_reasoning_delta(
        &mut self,
        text: &str,
        in_tag: bool,
        pb: &crate::ui::ProgressBar,
    ) -> bool {
        if self.reasoning_buf.len().saturating_add(text.len()) > MAX_CONTENT_BUFFER_BYTES {
            self.truncated = true;
            return true;
        }
        if in_tag || self.thinking == ThinkingState::Detecting {
            self.thinking = ThinkingState::Thinking { in_tag };
        }
        let now = std::time::Instant::now();
        let is_first_thinking = self.thinking_start.is_none();
        if is_first_thinking {
            self.thinking_start = Some(now);
        }
        self.reasoning_buf.push_str(text);

        if (!pb.is_hidden() || self.show_thinking_stream) && !Ui::is_quiet() {
            if self.show_thinking_stream {
                if !self.streamed_thinking {
                    self.streamed_thinking = true;
                    pb.finish_and_clear();
                    if Ui::use_color() {
                        eprint!("\x1b[2m💭 Thinking...\n  │ ");
                    } else {
                        eprintln!("--- Thinking ---");
                    }
                }
                let safe_chunk = crate::ui::sanitize_terminal_text_cow(text);
                if Ui::use_color() {
                    eprint!("{}", safe_chunk.replace('\n', "\n  │ "));
                } else {
                    eprint!("{safe_chunk}");
                }
                let _ = io::stderr().flush();
            } else if is_first_thinking
                || now.duration_since(self.last_spinner_update) >= Duration::from_millis(100)
            {
                self.last_spinner_update = now;
                let msg = match extract_reasoning_step(&self.reasoning_buf) {
                    Some(s) => format!("💭 Thinking: {s}..."),
                    None => "💭 Thinking...".to_string(),
                };
                pb.set_message(msg);
            }
        }

        false
    }

    pub(crate) fn finish_thinking_ui(
        &mut self,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) {
        if !self.is_thinking() {
            self.thinking = ThinkingState::Finished;
            return;
        }
        self.thinking = ThinkingState::Finished;

        if Ui::is_quiet() || (pb.is_hidden() && !self.show_thinking_stream) {
            return;
        }

        if self.show_thinking_stream {
            if self.streamed_thinking {
                if Ui::use_color() {
                    eprintln!("\n\x1b[0m");
                } else {
                    eprintln!("\n----------------");
                }
            }
        } else if let Some(started) = self.thinking_start {
            let thought_line = format!(
                "💭 Thought for {}",
                format_duration_short(started.elapsed())
            );
            match mode {
                StreamMode::Buffered => pb.println(thought_line),
                StreamMode::Stdout { .. } => {
                    pb.finish_and_clear();
                    eprintln!("{thought_line}");
                }
            }
        }

        if !pb.is_hidden()
            && matches!(mode, StreamMode::Buffered)
            && !spinner_label.trim().is_empty()
        {
            if pb.is_finished() {
                pb.reset();
            }
            update_generating_spinner(pb, spinner_label);
        }
    }

    fn append_content_delta(
        &mut self,
        delta_text: &str,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) {
        let is_first = !self.started_visible;
        let text_to_emit = if is_first {
            let clean = delta_text.trim_start();
            if clean.is_empty() {
                return;
            }
            self.started_visible = true;
            clean
        } else {
            delta_text
        };

        self.finish_thinking_ui(pb, spinner_label, mode);
        self.output_tokens += 1;
        self.content_buf.push_str(text_to_emit);

        match mode {
            StreamMode::Stdout { header_title } => {
                if is_first {
                    pb.finish_and_clear();
                    let safe_title = crate::ui::sanitize_title(header_title);
                    if safe_title.starts_with("Answer:") || safe_title.starts_with("Summary:") {
                        Ui::header(&safe_title);
                    } else {
                        Ui::header(&format!("Summary: {safe_title}"));
                    }
                    self.stream_renderer = Some(crate::ui::StreamMarkdownRenderer::new());
                }
                let clean_text = crate::ui::sanitize_terminal_text_cow(text_to_emit);
                if let Some(ref mut renderer) = self.stream_renderer {
                    renderer.feed(&clean_text);
                }
                self.needs_stdout_flush = true;
            }
            StreamMode::Buffered => {
                if is_first && !spinner_label.trim().is_empty() {
                    update_generating_spinner(pb, spinner_label);
                }
            }
        }
    }
}

fn update_generating_spinner(pb: &crate::ui::ProgressBar, spinner_label: &str) {
    let clean_label = spinner_label.trim_end_matches('.');
    let gen_label = if clean_label.eq_ignore_ascii_case("thinking") {
        "Generating response"
    } else {
        clean_label
    };
    pb.set_message(format!("{gen_label}..."));
}

fn extract_reasoning_step(reasoning: &str) -> Option<String> {
    let tail_start = reasoning.floor_char_boundary(reasoning.len().saturating_sub(4096));
    let tail = &reasoning[tail_start..];
    let ends_with_nl = tail.ends_with('\n');

    for (idx, raw_line) in tail.rsplit('\n').take(20).enumerate() {
        let trimmed = raw_line.trim_end_matches(['\r', ' ', '\t']);
        if trimmed.is_empty()
            || trimmed.starts_with([' ', '\t'])
            || trimmed.starts_with("- ")
            || trimmed.starts_with("+ ")
            || (trimmed.starts_with('*') && !trimmed.starts_with("**"))
        {
            continue;
        }
        let candidate = if (idx > 0 || ends_with_nl) && trimmed.starts_with('#') {
            trimmed.trim_start_matches('#').trim()
        } else {
            let digits = trimmed.bytes().take_while(u8::is_ascii_digit).count();
            let after_num = if digits > 0
                && (trimmed[digits..].starts_with(". ") || trimmed[digits..].starts_with(") "))
            {
                trimmed[digits + 2..].trim_start()
            } else {
                trimmed
            };
            let Some(rest) = after_num.strip_prefix("**") else {
                continue;
            };
            let Some(end) = rest.find("**") else {
                continue;
            };
            rest[..end].trim()
        };
        let cleaned = candidate
            .trim_matches(|c: char| c.is_whitespace() || matches!(c, '*' | '#' | ':' | '.' | '…'))
            .trim();
        if cleaned.chars().count() >= 4 {
            return Some(cleaned.to_string());
        }
    }
    None
}

pub(crate) fn compute_tokens_per_second(
    completion_tokens: Option<u32>,
    elapsed: Duration,
) -> Option<f64> {
    completion_tokens.and_then(|c| {
        let s = elapsed.as_secs_f64();
        if s > 0.05 { Some(c as f64 / s) } else { None }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_tokens_per_second() {
        assert_eq!(
            compute_tokens_per_second(None, Duration::from_secs(1)),
            None
        );
        assert_eq!(compute_tokens_per_second(Some(10), Duration::ZERO), None);
        let rate = compute_tokens_per_second(Some(100), Duration::from_secs(2)).unwrap();
        assert!((rate - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_sse_stream_lifecycle_leading_ws_error_and_usage() {
        let mut state = SseStreamState::default();
        let pb = crate::ui::ProgressBar::hidden();

        // Leading whitespace before `<think>` must NOT disable think-tag detection
        assert!(!state.process_line(
            br#"data: {"choices":[{"delta":{"content":"\n  "}}]}"#,
            &pb,
            "T",
            StreamMode::Buffered
        ));
        assert!(!state.process_line(
            br#"data: {"choices":[{"delta":{"content":"<thi"}}]}"#,
            &pb,
            "T",
            StreamMode::Buffered
        ));
        assert!(!state.process_line(
            br#"data: {"choices":[{"delta":{"content":"nk>Step 1 & done</th"}}]}"#,
            &pb,
            "T",
            StreamMode::Buffered
        ));
        assert!(!state.process_line(
            br#"data: {"choices":[{"delta":{"content":"ink>Hello "}}]}"#,
            &pb,
            "T",
            StreamMode::Buffered
        ));
        assert!(!state.process_line(
            br#"data: {"choices":[{"delta":{"content":"World!"}}]}"#,
            &pb,
            "T",
            StreamMode::Buffered
        ));
        assert!(!state.process_line(br#"data: {"choices":[{"finish_reason":"length","delta":{}}],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#, &pb, "T", StreamMode::Buffered));
        assert!(state.process_line(b"data: [DONE]", &pb, "T", StreamMode::Buffered));
        assert!(!state.process_line(b": ping", &pb, "T", StreamMode::Buffered));
        assert!(!state.process_line(b"data: {bad json", &pb, "T", StreamMode::Buffered));

        state.finish_stream(&pb, "T", StreamMode::Buffered);
        assert_eq!(state.reasoning_buf, "Step 1 & done");
        assert_eq!(state.content_buf, "Hello World!");
        assert!(state.truncated);

        let mut err_state = SseStreamState::default();
        assert!(err_state.process_line(
            br#"data: {"error":{"message":"Context full"}}"#,
            &pb,
            "T",
            StreamMode::Buffered
        ));
        assert_eq!(err_state.upstream_error.as_deref(), Some("Context full"));

        let (c, r) =
            SseStreamState::split_non_streaming("<think>Plan</think>\nFinal answer.", None);
        assert_eq!(c, "Final answer.");
        assert_eq!(r.as_deref(), Some("Plan"));
    }

    #[test]
    fn test_extract_reasoning_step() {
        let trace = "Thinking:\n\n1.  **Analyze the Request:**\n    *   **Topic:** Nvidia\n";
        assert_eq!(
            extract_reasoning_step(trace).as_deref(),
            Some("Analyze the Request")
        );
        assert_eq!(
            extract_reasoning_step("### 2. Comparing the US and LLM Stories\nBody").as_deref(),
            Some("2. Comparing the US and LLM Stories")
        );
        assert_eq!(extract_reasoning_step("### Halfway"), None);
        assert_eq!(extract_reasoning_step("**Unclosed"), None);
        assert_eq!(extract_reasoning_step("Mentions **Nvidia** inline."), None);
    }

    #[test]
    fn test_sse_stream_state_bounds_reasoning_and_content() {
        let pb = crate::ui::ProgressBar::hidden();
        let huge = "R".repeat(MAX_CONTENT_BUFFER_BYTES + 1024);

        for (field, prefix, check_content) in [
            ("reasoning_content", "", false),
            ("content", "<think>", false),
            ("content", "<think>short</think>", true),
        ] {
            let mut state = SseStreamState::default();
            let payload =
                format!(r#"data: {{"choices":[{{"delta":{{"{field}":"{prefix}{huge}"}}}}]}}"#);
            assert!(state.process_line(payload.as_bytes(), &pb, "", StreamMode::Buffered));
            assert!(state.truncated);
            let buf_len = if check_content {
                state.content_buf.len()
            } else {
                state.reasoning_buf.len()
            };
            assert!(buf_len <= MAX_CONTENT_BUFFER_BYTES);
        }
    }
}
