use crate::ui::Ui;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::io::{self, Write};
use std::time::Duration;

pub const MAX_SSE_LINE_BYTES: usize = 1024 * 1024; // 1 MB per line
pub const MAX_CONTENT_BUFFER_BYTES: usize = 10 * 1024 * 1024; // 10 MB total content
pub const MAX_HTTP_RESPONSE_BYTES: usize = 10 * 1024 * 1024; // 10 MB for JSON responses

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
pub(crate) struct ChatCompletionChunk<'a> {
    #[serde(default, borrow)]
    pub(crate) choices: Vec<ChunkChoice<'a>>,
    pub(crate) usage: Option<TokenUsage>,
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
    /// Emitted by llama.cpp / Gemma 4 / DeepSeek during internal chain-of-thought
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
    #[allow(dead_code)]
    pub(crate) total_tokens: Option<u32>,
}

#[derive(Deserialize)]
pub(crate) struct ChatChoice {
    pub(crate) message: ResponseMessage,
    pub(crate) finish_reason: Option<String>,
}

#[derive(Deserialize)]
pub(crate) struct ResponseMessage {
    pub(crate) content: String,
}

/// Controls whether tokens are streamed live to stdout or buffered silently behind a spinner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StreamMode<'a> {
    /// Stream visible content tokens directly to stdout as they arrive.
    Stdout { header_title: &'a str },
    /// Buffer tokens silently behind a spinner until complete (default).
    Buffered,
    /// Execute completion completely silently with no spinner, no thinking logs, and no success checkmarks.
    Silent,
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

pub(crate) struct SseStreamState {
    pub(crate) content_buf: String,
    pub(crate) reasoning_buf: String,
    pub(crate) thinking_tokens: usize,
    pub(crate) output_tokens: usize,
    pub(crate) started_visible: bool,
    pub(crate) is_thinking: bool,
    pub(crate) streamed_thinking: bool,
    pub(crate) finished_thinking_ui: bool,
    pub(crate) in_think_tag: bool,
    pub(crate) truncated: bool,
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
            thinking_tokens: 0,
            output_tokens: 0,
            started_visible: false,
            is_thinking: false,
            streamed_thinking: false,
            finished_thinking_ui: false,
            in_think_tag: false,
            truncated: false,
            last_usage: None,
            needs_stdout_flush: false,
            stream_renderer: None,
            last_spinner_update: start_time,
            thinking_start: None,
            show_thinking_stream,
        }
    }
}

impl Default for SseStreamState {
    fn default() -> Self {
        Self::new(std::time::Instant::now(), false)
    }
}

impl SseStreamState {
    /// Processes a single SSE line. Returns `true` if the `[DONE]` sentinel was reached or limit hit.
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

        if event.usage.is_some() {
            self.last_usage = event.usage;
        }

        let Some(choice) = event.choices.first() else {
            return false;
        };
        if choice.finish_reason.as_deref() == Some("length") {
            self.truncated = true;
        }

        if let Some(reasoning) = &choice.delta.reasoning_content
            && !reasoning.is_empty()
            && !self.started_visible
        {
            self.append_reasoning_delta(reasoning, pb, mode);
        }

        if let Some(delta_text) = &choice.delta.content
            && !delta_text.is_empty()
        {
            if !self.started_visible {
                if let Some(think_start) = delta_text.find("<think>") {
                    self.in_think_tag = true;
                    let after_tag = &delta_text[think_start + 7..];
                    if let Some(think_end) = after_tag.find("</think>") {
                        let think_part = &after_tag[..think_end];
                        self.append_reasoning_delta(think_part, pb, mode);
                        self.in_think_tag = false;
                        let visible_part = &after_tag[think_end + 8..];
                        if !visible_part.is_empty() {
                            self.append_content_delta(visible_part, pb, spinner_label, mode);
                        }
                    } else {
                        self.append_reasoning_delta(after_tag, pb, mode);
                    }
                    return false;
                } else if self.in_think_tag {
                    if let Some(think_end) = delta_text.find("</think>") {
                        let think_part = &delta_text[..think_end];
                        self.append_reasoning_delta(think_part, pb, mode);
                        self.in_think_tag = false;
                        let visible_part = &delta_text[think_end + 8..];
                        if !visible_part.is_empty() {
                            self.append_content_delta(visible_part, pb, spinner_label, mode);
                        }
                    } else {
                        self.append_reasoning_delta(delta_text, pb, mode);
                    }
                    return false;
                }
            }

            if self.content_buf.len().saturating_add(delta_text.len()) > MAX_CONTENT_BUFFER_BYTES {
                self.truncated = true;
                return true;
            }
            self.append_content_delta(delta_text, pb, spinner_label, mode);
        }

        false
    }

    fn append_reasoning_delta(
        &mut self,
        text: &str,
        pb: &crate::ui::ProgressBar,
        mode: StreamMode<'_>,
    ) {
        self.thinking_tokens += 1;
        self.is_thinking = true;
        let now = std::time::Instant::now();
        let is_first_thinking = self.thinking_start.is_none();
        if is_first_thinking {
            self.thinking_start = Some(now);
        }
        self.reasoning_buf.push_str(text);

        if matches!(mode, StreamMode::Silent) {
            return;
        }

        if !Ui::is_quiet() {
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
                    let indented = safe_chunk.replace('\n', "\n  │ ");
                    eprint!("{}", indented);
                } else {
                    eprint!("{}", safe_chunk);
                }
                let _ = io::stderr().flush();
            } else if is_first_thinking
                || now.duration_since(self.last_spinner_update) >= Duration::from_millis(100)
            {
                self.last_spinner_update = now;
                let elapsed_s = self
                    .thinking_start
                    .map(|t| t.elapsed().as_secs_f64())
                    .unwrap_or(0.0);
                let duration_str = format_thinking_duration(elapsed_s);
                pb.set_message(format!(
                    "💭 Thinking... ({} tokens • {})",
                    self.thinking_tokens, duration_str
                ));
            }
        }
    }

    pub(crate) fn finish_thinking_ui(
        &mut self,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) {
        if self.finished_thinking_ui || !self.is_thinking {
            return;
        }
        self.finished_thinking_ui = true;
        self.is_thinking = false;

        if matches!(mode, StreamMode::Silent) || pb.is_hidden() || Ui::is_quiet() {
            return;
        }

        let total_elapsed_s = self
            .thinking_start
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or(0.0);

        if self.show_thinking_stream {
            if self.streamed_thinking {
                if Ui::use_color() {
                    eprintln!("\n\x1b[0m");
                } else {
                    eprintln!("\n----------------");
                }
            }
        } else if self.thinking_tokens > 0 {
            let total_duration_str = format_thinking_duration(total_elapsed_s);
            let thought_line = format!(
                "💭 Thought for {} ({} tokens)",
                total_duration_str, self.thinking_tokens
            );

            if matches!(mode, StreamMode::Buffered) {
                pb.println(thought_line);
            } else if matches!(mode, StreamMode::Stdout { .. }) {
                pb.finish_and_clear();
                eprintln!("{}", thought_line);
            }
        }

        if matches!(mode, StreamMode::Buffered) && !spinner_label.trim().is_empty() {
            if pb.is_finished() {
                pb.reset();
                pb.enable_steady_tick(Duration::from_millis(80));
            }
            let clean_label = spinner_label.trim_end_matches('.');
            let gen_label = if clean_label.eq_ignore_ascii_case("thinking") {
                "Generating response"
            } else {
                clean_label
            };
            pb.set_message(format!("{}...", gen_label));
        }
    }

    fn append_content_delta(
        &mut self,
        delta_text: &str,
        pb: &crate::ui::ProgressBar,
        spinner_label: &str,
        mode: StreamMode<'_>,
    ) {
        self.finish_thinking_ui(pb, spinner_label, mode);

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

        self.output_tokens += 1;
        self.content_buf.push_str(text_to_emit);

        match mode {
            StreamMode::Silent => {}
            StreamMode::Stdout { header_title } => {
                if is_first {
                    pb.finish_and_clear();
                    let safe_title = crate::ui::sanitize_title(header_title);
                    Ui::header(&format!("Summary: {}", safe_title));
                    self.stream_renderer = Some(crate::ui::StreamMarkdownRenderer::new());
                }
                let clean_text = crate::ui::sanitize_terminal_text_cow(text_to_emit);
                if let Some(ref mut renderer) = self.stream_renderer {
                    renderer.feed(&clean_text);
                } else {
                    print!("{}", clean_text);
                }
                self.needs_stdout_flush = true;
            }
            StreamMode::Buffered => {
                if is_first && !spinner_label.trim().is_empty() {
                    let clean_label = spinner_label.trim_end_matches('.');
                    let gen_label = if clean_label.eq_ignore_ascii_case("thinking") {
                        "Generating response"
                    } else {
                        clean_label
                    };
                    pb.set_message(format!("{}...", gen_label));
                }
            }
        }
    }
}

/// Formats an elapsed thinking duration into a clean Antigravity/Gemini style string (e.g., "4s", "19s", "35s", "0.4s").
pub fn format_thinking_duration(secs: f64) -> String {
    if secs < 1.0 {
        format!("{:.1}s", secs)
    } else if secs < 10.0 {
        let rounded_10 = (secs * 10.0).round();
        if rounded_10 % 10.0 == 0.0 {
            format!("{}s", (rounded_10 / 10.0) as u64)
        } else {
            format!("{:.1}s", secs)
        }
    } else {
        format!("{}s", secs.round() as u64)
    }
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
    fn test_sse_stream_state_process_line() {
        let mut state = SseStreamState::default();
        let pb = crate::ui::ProgressBar::hidden();

        // 1. Process reasoning tokens (thinking occurs before visible content)
        let reason_chunk = br#"data: {"choices":[{"delta":{"reasoning_content":"Step 1"}}]}"#;
        assert!(!state.process_line(reason_chunk, &pb, "Testing", StreamMode::Buffered));
        assert_eq!(state.thinking_tokens, 1);
        assert!(state.is_thinking);
        assert_eq!(state.reasoning_buf, "Step 1");
        assert_eq!(state.content_buf, "");

        // 2. Process regular delta content chunks
        let chunk1 = br#"data: {"choices":[{"delta":{"content":"Hello "}}]}"#;
        assert!(!state.process_line(chunk1, &pb, "Testing", StreamMode::Buffered));
        assert_eq!(state.content_buf, "Hello ");
        assert_eq!(state.output_tokens, 1);
        assert!(state.started_visible);

        let chunk2 = br#"data: {"choices":[{"delta":{"content":"World!"}}]}"#;
        assert!(!state.process_line(chunk2, &pb, "Testing", StreamMode::Buffered));
        assert_eq!(state.content_buf, "Hello World!");
        assert_eq!(state.output_tokens, 2);

        // 3. Process usage chunk
        let usage_chunk = br#"data: {"choices":[],"usage":{"prompt_tokens":10,"completion_tokens":20,"total_tokens":30}}"#;
        assert!(!state.process_line(usage_chunk, &pb, "Testing", StreamMode::Buffered));
        assert_eq!(
            state.last_usage,
            Some(TokenUsage {
                prompt_tokens: Some(10),
                completion_tokens: Some(20),
                total_tokens: Some(30),
            })
        );

        // 4. Process length finish reason
        let length_chunk = br#"data: {"choices":[{"finish_reason":"length","delta":{}}]}"#;
        assert!(!state.process_line(length_chunk, &pb, "Testing", StreamMode::Buffered));
        assert!(state.truncated);

        // 5. Process [DONE] sentinel
        let done_chunk = b"data: [DONE]";
        assert!(state.process_line(done_chunk, &pb, "Testing", StreamMode::Buffered));

        // 6. Non-data / malformed lines return false gracefully
        assert!(!state.process_line(b": ping", &pb, "Testing", StreamMode::Buffered));
        assert!(!state.process_line(b"data: {bad json", &pb, "Testing", StreamMode::Buffered));
    }

    #[test]
    fn test_format_thinking_duration() {
        assert_eq!(format_thinking_duration(0.4), "0.4s");
        assert_eq!(format_thinking_duration(4.0), "4s");
        assert_eq!(format_thinking_duration(4.2), "4.2s");
        assert_eq!(format_thinking_duration(19.0), "19s");
        assert_eq!(format_thinking_duration(19.8), "20s");
        assert_eq!(format_thinking_duration(35.2), "35s");
    }

    #[test]
    fn test_sse_stream_state_clean_spinner_lifecycle() {
        let pb = crate::ui::ProgressBar::hidden();
        let mut state = SseStreamState::new(std::time::Instant::now(), false);

        // Step 1: Thinking chunk
        let reason_1 = br#"data: {"choices":[{"delta":{"reasoning_content":"Reading articles"}}]}"#;
        assert!(!state.process_line(
            reason_1,
            &pb,
            "Synthesizing news briefing...",
            StreamMode::Buffered
        ));
        assert_eq!(state.thinking_tokens, 1);
        assert!(state.is_thinking);
        assert!(!state.finished_thinking_ui);

        // Step 2: Content generation starts: finishes thinking UI, keeps pb alive and transitions to loading status
        let content_chunk =
            b"data: {\"choices\":[{\"delta\":{\"content\":\"# Executive News Briefing\\n\"}}]}\n";
        assert!(!state.process_line(
            content_chunk,
            &pb,
            "Synthesizing news briefing...",
            StreamMode::Buffered
        ));
        assert!(state.finished_thinking_ui);
        assert!(!state.is_thinking);
        assert!(
            !pb.is_finished(),
            "ProgressBar must remain active during buffered content generation"
        );
        assert_eq!(state.output_tokens, 1);
        assert!(state.content_buf.contains("Executive News Briefing"));

        // Subsequent content chunk updates spinner progress
        let content_chunk_2 = b"data: {\"choices\":[{\"delta\":{\"content\":\"Key developments in Python 3.13.\"}}]}\n";
        assert!(!state.process_line(
            content_chunk_2,
            &pb,
            "Synthesizing news briefing...",
            StreamMode::Buffered
        ));
        assert_eq!(state.output_tokens, 2);
        assert!(!pb.is_finished());
    }

    #[test]
    fn test_pb_reset_resumes_spinner() {
        let pb = crate::ui::ProgressBar::new_spinner();
        pb.finish_and_clear();
        assert!(pb.is_finished());
        pb.reset();
        assert!(!pb.is_finished());
    }

    #[test]
    fn test_sse_stream_state_compact_vs_verbose_thinking() {
        let pb = crate::ui::ProgressBar::hidden();

        // Compact mode (default)
        let mut state_compact = SseStreamState::new(std::time::Instant::now(), false);
        let reason_chunk =
            br#"data: {"choices":[{"delta":{"reasoning_content":"Checking parameters"}}]}"#;
        assert!(!state_compact.process_line(reason_chunk, &pb, "Working", StreamMode::Buffered));
        assert_eq!(state_compact.thinking_tokens, 1);
        assert!(state_compact.is_thinking);
        assert!(!state_compact.streamed_thinking);

        // When content arrives, finish_thinking_ui runs and settles thinking
        let content_chunk = br#"data: {"choices":[{"delta":{"content":"Summary result"}}]}"#;
        assert!(!state_compact.process_line(content_chunk, &pb, "Working", StreamMode::Buffered));
        assert!(!state_compact.is_thinking);
        assert!(state_compact.finished_thinking_ui);
        assert_eq!(state_compact.content_buf, "Summary result");

        // Verbose / show_thinking_stream mode
        let mut state_verbose = SseStreamState::new(std::time::Instant::now(), true);
        assert!(!state_verbose.process_line(reason_chunk, &pb, "Working", StreamMode::Buffered));
        assert_eq!(state_verbose.thinking_tokens, 1);
        assert!(state_verbose.is_thinking);
        assert!(state_verbose.streamed_thinking);
    }

    #[test]
    fn test_sse_stream_state_silent_mode() {
        let pb = crate::ui::ProgressBar::hidden();
        let mut state = SseStreamState::new(std::time::Instant::now(), false);

        let reason_chunk =
            br#"data: {"choices":[{"delta":{"reasoning_content":"Silent reasoning"}}]}"#;
        assert!(!state.process_line(reason_chunk, &pb, "", StreamMode::Silent));
        assert_eq!(state.thinking_tokens, 1);
        assert!(state.is_thinking);
        assert!(!state.streamed_thinking);
        assert!(state.reasoning_buf.contains("Silent reasoning"));

        let content_chunk =
            br#"data: {"choices":[{"delta":{"content":"Silent completion output"}}]}"#;
        assert!(!state.process_line(content_chunk, &pb, "", StreamMode::Silent));
        assert_eq!(state.output_tokens, 1);
        assert!(!state.is_thinking);
        assert!(state.finished_thinking_ui);
        assert_eq!(state.content_buf, "Silent completion output");
    }
}
