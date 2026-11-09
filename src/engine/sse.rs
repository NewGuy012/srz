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
    pub(crate) last_thought_headline: Option<String>,
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
            last_thought_headline: None,
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
        pb: &indicatif::ProgressBar,
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
        pb: &indicatif::ProgressBar,
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

                if let Some(headline) = extract_thought_headline(&self.reasoning_buf) {
                    self.handle_thought_headline_update(&headline, pb);
                }

                let msg = match &self.last_thought_headline {
                    Some(step) if !step.is_empty() => {
                        let clean_step = step.trim_end_matches(['.', '…']);
                        format!("💭 Thinking: {}...", to_sentence_case(clean_step))
                    }
                    _ => "💭 Thinking...".to_string(),
                };
                pb.set_message(msg);
            }
        }
    }

    pub(crate) fn handle_thought_headline_update(
        &mut self,
        new_headline: &str,
        _pb: &indicatif::ProgressBar,
    ) {
        let sentence_cased = to_sentence_case(new_headline);
        match &self.last_thought_headline {
            None => {
                self.last_thought_headline = Some(sentence_cased);
            }
            Some(old_headline) => {
                if is_same_or_growing_headline(old_headline, &sentence_cased) {
                    if sentence_cased.len() > old_headline.len() {
                        self.last_thought_headline = Some(sentence_cased);
                    }
                    return;
                }

                // Update active step headline in-place for live summary without dumping noisy lines to terminal
                self.last_thought_headline = Some(sentence_cased);
            }
        }
    }

    pub(crate) fn finish_thinking_ui(
        &mut self,
        pb: &indicatif::ProgressBar,
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
            // Settle thinking into Antigravity/Gemini dropdown indicator
            let total_duration_str = format_thinking_duration(total_elapsed_s);
            let thought_line = if Ui::use_color() {
                format!("  \x1b[2m💭 Thought for {} ›\x1b[0m", total_duration_str)
            } else {
                format!("  Thought for {} >", total_duration_str)
            };

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
        pb: &indicatif::ProgressBar,
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

/// Checks if two thought headlines are essentially identical or if one is an incremental continuation
/// of the other as tokens stream in, preventing accidental premature step splitting.
pub fn is_same_or_growing_headline(old_h: &str, new_h: &str) -> bool {
    let old_norm = old_h.trim().to_ascii_lowercase();
    let new_norm = new_h.trim().to_ascii_lowercase();
    old_norm == new_norm || old_norm.starts_with(&new_norm) || new_norm.starts_with(&old_norm)
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

/// Truncates a headline to `max_len` characters at a clean word boundary with an ellipsis.
fn truncate_headline(s: &str, max_len: usize) -> String {
    let trimmed = s.trim();
    if trimmed.chars().count() <= max_len {
        return trimmed.to_string();
    }
    let boundary = trimmed
        .char_indices()
        .take_while(|&(idx, _)| idx < max_len)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(max_len);
    let cut = match trimmed[..boundary].rfind(char::is_whitespace) {
        Some(space) if space > 10 => &trimmed[..space],
        _ => &trimmed[..boundary],
    };
    format!("{}…", cut.trim_end_matches([' ', ':', '.', '-']))
}

/// Strips markdown formatting characters (`*`, `_`, `#`, `` ` ``) and collapses whitespace in a single pass.
pub fn strip_markdown_decorations(s: &str) -> String {
    s.chars()
        .filter(|&c| c != '*' && c != '_' && c != '#' && c != '`')
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Checks if a word is an uppercase acronym/initialism (e.g. "API", "LLM", "USA").
fn is_acronym(word: &str) -> bool {
    let alpha: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    alpha.len() >= 2 && alpha.iter().all(|c| c.is_uppercase())
}

/// Capitalizes the first alphabetic character in a word, lowercasing remaining characters
/// unless the word is an acronym.
fn capitalize_word(word: &str, is_all_caps: bool) -> String {
    if !is_all_caps && is_acronym(word) {
        return word.to_string();
    }
    let mut out = String::with_capacity(word.len());
    let mut capitalized = false;
    for c in word.chars() {
        if !capitalized && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalized = true;
        } else {
            out.extend(c.to_lowercase());
        }
    }
    out
}

/// Converts a single phrase segment to sentence case.
fn format_segment(segment: &str) -> String {
    let is_all_caps = segment.chars().filter(|c| c.is_alphabetic()).count() > 1
        && segment
            .chars()
            .filter(|c| c.is_alphabetic())
            .all(|c| c.is_uppercase());

    let mut words = segment.split_whitespace();
    let Some(first) = words.next() else {
        return String::new();
    };

    let mut out = capitalize_word(first, is_all_caps);
    for word in words {
        out.push(' ');
        if !is_all_caps && is_acronym(word) {
            out.push_str(word);
        } else {
            out.extend(word.chars().flat_map(|c| c.to_lowercase()));
        }
    }
    out
}

/// Converts a thought headline string to sentence case.
/// Anything after a colon (e.g. `Step 2: Processing content`) starts with a capital letter,
/// while subsequent words are lowercased (preserving uppercase acronyms like API/LLM).
pub fn to_sentence_case(s: &str) -> String {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    trimmed
        .split(':')
        .map(|segment| format_segment(segment.trim()))
        .filter(|seg| !seg.is_empty())
        .collect::<Vec<_>>()
        .join(": ")
}

/// Checks whether a candidate headline is substantive and not generic thinking filler.
fn is_meaningful_headline(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let trimmed = lower.trim();

    if trimmed.len() < 3 {
        return false;
    }

    // Reject generic prompt prefixes / meta phrases
    if trimmed.starts_with("here's a thinking process")
        || trimmed.starts_with("thinking process")
        || trimmed.starts_with("thought process")
        || trimmed.starts_with("let's think")
        || trimmed.starts_with("let's see")
        || trimmed == "okay"
        || trimmed == "understood"
        || trimmed == "sure"
        || trimmed == "notes"
        || trimmed == "constraints"
        || trimmed == "constraint"
        || trimmed == "the 'what'"
        || trimmed == "the 'why'"
        || trimmed == "the 'how'"
        || trimmed == "core result"
        || trimmed == "key drivers"
        || trimmed == "role"
        || trimmed == "output style"
        || trimmed == "structure required"
        || trimmed == "timeframe"
        || trimmed == "topic"
    {
        return false;
    }

    true
}

/// Checks if a string starts with a common analytical action verb.
fn is_action_verb_sentence(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    const VERB_PREFIXES: &[&str] = &[
        "analyz",
        "review",
        "synthesiz",
        "draft",
        "structur",
        "evaluat",
        "check",
        "extract",
        "search",
        "identif",
        "summariz",
        "process",
        "understand",
        "compar",
        "read",
        "investigat",
        "gather",
        "determin",
        "verif",
        "plan",
        "outlin",
        "examin",
    ];

    let first_word = lower.split_whitespace().next().unwrap_or("");
    VERB_PREFIXES.iter().any(|v| first_word.starts_with(v))
}

fn strip_parenthetical_and_trim(s: &str) -> &str {
    let cleaned = s.trim_matches(|c: char| c.is_whitespace() || c == ':' || c == '.' || c == '-');
    if let Some(paren_pos) = cleaned.find('(') {
        let before = cleaned[..paren_pos].trim();
        if before.len() >= 10 { before } else { cleaned }
    } else {
        cleaned
    }
}

/// Cleans a candidate heading line and extracts the core step title.
fn clean_heading_candidate(line: &str) -> Option<String> {
    let trimmed = line.trim();

    // 1. Bold title within line: `**Title:**` or `1. **Title:**`
    if let Some(start_bold) = trimmed.find("**") {
        let after_start = &trimmed[start_bold + 2..];
        if let Some(end_bold) = after_start.find("**") {
            let bold_text = &after_start[..end_bold];
            let without_paren = strip_parenthetical_and_trim(bold_text);
            if !without_paren.is_empty() {
                return Some(truncate_headline(without_paren, 45));
            }
        }
    }

    // 2. Markdown heading `# Title`, numbered step `1. Title`, or `Step 1: Title`
    let is_heading = trimmed.starts_with('#');
    let is_numbered = trimmed.starts_with(|c: char| c.is_ascii_digit())
        && (trimmed.contains(". ") || trimmed.contains(") ") || trimmed.contains(':'));
    let lower = trimmed.to_ascii_lowercase();
    let is_step_prefix = lower.starts_with("step ")
        || lower.starts_with("phase ")
        || lower.starts_with("stage ")
        || lower.starts_with("part ");

    if is_heading || is_numbered || is_step_prefix {
        let stripped = strip_markdown_decorations(trimmed.trim_start_matches('#'));
        let after_num = if is_heading || is_numbered {
            stripped.trim_start_matches(|c: char| {
                c.is_ascii_digit() || c == '.' || c == ')' || c == ' ' || c == '\t'
            })
        } else {
            &stripped
        };
        let without_paren = strip_parenthetical_and_trim(after_num);
        if !without_paren.is_empty() {
            return Some(truncate_headline(without_paren, 45));
        }
    }

    None
}

/// Extracts a concise, clean one-liner headline summarizing the active thought step or action
/// from the accumulated reasoning buffer.
pub fn extract_thought_headline(reasoning_text: &str) -> Option<String> {
    if reasoning_text.trim().is_empty() {
        return None;
    }

    let lines: Vec<&str> = reasoning_text
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect();

    if lines.is_empty() {
        return None;
    }

    // Priority 1: Scan backwards for structured section/step headings:
    for line in lines.iter().rev() {
        if let Some(candidate) = clean_heading_candidate(line)
            && is_meaningful_headline(&candidate)
        {
            return Some(to_sentence_case(&candidate));
        }
    }

    // Priority 2: Scan backwards for action-oriented lines starting with active verbs:
    for line in lines.iter().rev().take(15) {
        let line_clean = strip_markdown_decorations(line);
        let trimmed = line_clean
            .trim_matches(|c: char| c.is_whitespace() || c == ':' || c == '.' || c == '-');
        if is_action_verb_sentence(trimmed) && is_meaningful_headline(trimmed) {
            return Some(to_sentence_case(&truncate_headline(trimmed, 45)));
        }
    }

    // Priority 3: Fallback to the latest substantive non-bullet line if clean
    for line in lines.iter().rev().take(10) {
        if line.starts_with('*') || line.starts_with('-') || line.starts_with('+') {
            continue;
        }
        let line_clean = strip_markdown_decorations(line);
        let trimmed = line_clean
            .trim_matches(|c: char| c.is_whitespace() || c == ':' || c == '.' || c == '-');
        if trimmed.len() >= 8 && is_meaningful_headline(trimmed) {
            return Some(to_sentence_case(&truncate_headline(trimmed, 45)));
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
    fn test_sse_stream_state_process_line() {
        let mut state = SseStreamState::default();
        let pb = indicatif::ProgressBar::hidden();

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
    fn test_extract_thought_headline_variations() {
        // User's exact scenario from election news briefing
        let step1 = "Here's a thinking process to construct the news briefing:\n\n1.  **Analyze the Request:**\n    *   **Topic:** USA election news (Midterms, Nov 2026).\n    *   **Timeframe:** Past Week";
        assert_eq!(
            extract_thought_headline(step1),
            Some("Analyze the request".to_string())
        );

        let step2 = format!(
            "{}\n\n2.  **Source Material Review & Extraction (Thematic Grouping):**\n    *   **Core Result:** Democrats won control",
            step1
        );
        assert_eq!(
            extract_thought_headline(&step2),
            Some("Source material review & extraction".to_string())
        );

        let step3 = format!(
            "{}\n\n3.  **Structuring and Drafting the Briefing (Applying Professional Tone):**\n    *   Drafting 1. Executive Overview",
            step2
        );
        assert_eq!(
            extract_thought_headline(&step3),
            Some("Structuring and drafting the briefing".to_string())
        );

        let step4 = format!(
            "{}\n\n4.  **Final Review:**\n    *   Check constraints",
            step3
        );
        assert_eq!(
            extract_thought_headline(&step4),
            Some("Final review".to_string())
        );

        // Markdown headings
        let md_heading =
            "### 1. Ownership and Borrowing\nExplaining how Rust guarantees memory safety.";
        assert_eq!(
            extract_thought_headline(md_heading),
            Some("Ownership and borrowing".to_string())
        );

        // Action verb sentence
        let action_sentence = "Reviewing the 5 news articles.";
        assert_eq!(
            extract_thought_headline(action_sentence),
            Some("Reviewing the 5 news articles".to_string())
        );

        // Long sentence truncated at 45 chars with ellipsis
        let long_sentence =
            "Reviewing the 5 articles to synthesize briefing and extract core developments.";
        assert_eq!(
            extract_thought_headline(long_sentence),
            Some("Reviewing the 5 articles to synthesize…".to_string())
        );

        // Standalone bold
        let standalone_bold = "**Synthesizing Main Takeaway**\nThe candidate won.";
        assert_eq!(
            extract_thought_headline(standalone_bold),
            Some("Synthesizing main takeaway".to_string())
        );

        // Empty and generic filler
        assert_eq!(extract_thought_headline(""), None);
        assert_eq!(extract_thought_headline("   \n  \t  "), None);
        assert_eq!(
            extract_thought_headline("Here's a thinking process:\nOkay\nUnderstood"),
            None
        );
    }

    #[test]
    fn test_is_same_or_growing_headline() {
        assert!(is_same_or_growing_headline(
            "Analyze the Request",
            "Analyze the Request"
        ));
        assert!(is_same_or_growing_headline(
            "Analyze",
            "Analyze the Request"
        ));
        assert!(is_same_or_growing_headline(
            "Analyze the Request",
            "Analyze"
        ));
        assert!(is_same_or_growing_headline(
            "Source Material Review",
            "Source Material Review & Extraction"
        ));
        assert!(!is_same_or_growing_headline(
            "Analyze the Request",
            "Source Material Review & Extraction"
        ));
        assert!(!is_same_or_growing_headline(
            "Structuring and Drafting the Briefing",
            "Final Review"
        ));
    }

    #[test]
    fn test_sse_stream_state_multi_step_thinking_and_loading_spinner() {
        let pb = indicatif::ProgressBar::hidden();
        let mut state = SseStreamState::new(std::time::Instant::now(), false);

        // Step 1: Analyze
        let reason_1 = br#"data: {"choices":[{"delta":{"reasoning_content":"1. **Analyze Source Material:**\nReading articles"}}]}"#;
        assert!(!state.process_line(
            reason_1,
            &pb,
            "Synthesizing news briefing...",
            StreamMode::Buffered
        ));
        assert_eq!(
            state.last_thought_headline.as_deref(),
            Some("Analyze source material")
        );
        assert!(state.is_thinking);
        assert!(!state.finished_thinking_ui);

        // Step 2: Synthesize - triggers headline update and step 1 completion
        state.last_spinner_update -= std::time::Duration::from_millis(150);
        let reason_2 = br#"data: {"choices":[{"delta":{"reasoning_content":"\n\n2. **Synthesize Key Developments:**\nGrouping stories"}}]}"#;
        assert!(!state.process_line(
            reason_2,
            &pb,
            "Synthesizing news briefing...",
            StreamMode::Buffered
        ));
        assert_eq!(
            state.last_thought_headline.as_deref(),
            Some("Synthesize key developments")
        );
        assert!(state.is_thinking);

        // Content generation starts: finishes thinking UI, keeps pb alive and transitions to loading status
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
        let pb = indicatif::ProgressBar::new_spinner();
        pb.finish_and_clear();
        assert!(pb.is_finished());
        pb.reset();
        assert!(!pb.is_finished());
    }

    #[test]
    fn test_sse_stream_state_compact_vs_verbose_thinking() {
        let pb = indicatif::ProgressBar::hidden();

        // Compact mode (default)
        let mut state_compact = SseStreamState::new(std::time::Instant::now(), false);
        let reason_chunk = br#"data: {"choices":[{"delta":{"reasoning_content":"1. **Analyze the Request:**\nChecking parameters"}}]}"#;
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
        assert!(state_compact.reasoning_buf.contains("Analyze the Request"));

        // Verbose / show_thinking_stream mode
        let mut state_verbose = SseStreamState::new(std::time::Instant::now(), true);
        assert!(!state_verbose.process_line(reason_chunk, &pb, "Working", StreamMode::Buffered));
        assert_eq!(state_verbose.thinking_tokens, 1);
        assert!(state_verbose.is_thinking);
        assert!(state_verbose.streamed_thinking);
    }

    #[test]
    fn test_sse_stream_state_silent_mode() {
        let pb = indicatif::ProgressBar::hidden();
        let mut state = SseStreamState::new(std::time::Instant::now(), false);

        let reason_chunk = br#"data: {"choices":[{"delta":{"reasoning_content":"1. **Analyze:**\nSilent reasoning"}}]}"#;
        assert!(!state.process_line(reason_chunk, &pb, "", StreamMode::Silent));
        assert_eq!(state.thinking_tokens, 1);
        assert!(state.is_thinking);
        assert!(state.last_thought_headline.is_none());
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

    #[test]
    fn test_handle_thought_headline_update_in_place_tracking() {
        let pb = indicatif::ProgressBar::hidden();
        let mut state = SseStreamState::new(std::time::Instant::now(), false);

        // First headline
        state.handle_thought_headline_update("Step 1: Ingesting Data", &pb);
        assert_eq!(
            state.last_thought_headline.as_deref(),
            Some("Step 1: Ingesting data")
        );

        // Transition: updates active headline in-place without terminal noise
        state.handle_thought_headline_update("Step 2: Processing Content", &pb);
        assert_eq!(
            state.last_thought_headline.as_deref(),
            Some("Step 2: Processing content")
        );

        state.handle_thought_headline_update("Step 3: Generating Output", &pb);
        assert_eq!(
            state.last_thought_headline.as_deref(),
            Some("Step 3: Generating output")
        );
    }

    #[test]
    fn test_sse_stream_state_clean_terminal_spinner_without_telemetry() {
        let pb =
            indicatif::ProgressBar::with_draw_target(None, indicatif::ProgressDrawTarget::stderr());
        let mut state = SseStreamState::new(std::time::Instant::now(), false);

        // Initial thinking with whitespace / punctuation before headline emerges
        let reason_initial = br#"data: {"choices":[{"delta":{"reasoning_content":"\n\n"}}]}"#;
        assert!(!state.process_line(
            reason_initial,
            &pb,
            "Generating summary with local LLM...",
            StreamMode::Buffered
        ));
        assert_eq!(pb.message(), "💭 Thinking...");
        assert!(!pb.message().contains('('));
        assert!(!pb.message().contains('s'));

        // Thinking with a headline extracted
        state.last_spinner_update -= std::time::Duration::from_millis(150);
        let reason_headline = br#"data: {"choices":[{"delta":{"reasoning_content":"1. **Identify Core Themes:**\nExtracting key points."}}]}"#;
        assert!(!state.process_line(
            reason_headline,
            &pb,
            "Generating summary with local LLM...",
            StreamMode::Buffered
        ));
        assert_eq!(pb.message(), "💭 Thinking: Identify core themes...");
        assert!(!pb.message().contains('('));
        assert!(!pb.message().contains("tokens"));
        assert!(!pb.message().contains("t/s"));

        // Content generation starts
        let content_chunk =
            br#"data: {"choices":[{"delta":{"content":"This is the executive summary."}}]}"#;
        assert!(!state.process_line(
            content_chunk,
            &pb,
            "Generating summary with local LLM...",
            StreamMode::Buffered
        ));
        assert_eq!(pb.message(), "Generating summary with local LLM...");
        assert!(!pb.message().contains("tokens"));
        assert!(!pb.message().contains("t/s"));
        assert!(!pb.message().contains('['));

        // Subsequent content chunks preserve clean label without live tokens or t/s telemetry
        let content_chunk_2 =
            br#"data: {"choices":[{"delta":{"content":" Additional details follow."}}]}"#;
        assert!(!state.process_line(
            content_chunk_2,
            &pb,
            "Generating summary with local LLM...",
            StreamMode::Buffered
        ));
        assert_eq!(pb.message(), "Generating summary with local LLM...");
        assert!(!pb.message().contains("tokens"));
        assert!(!pb.message().contains("t/s"));
    }

    #[test]
    fn test_to_sentence_case() {
        assert_eq!(to_sentence_case(""), "");
        assert_eq!(to_sentence_case("   "), "");
        assert_eq!(to_sentence_case("Draft the Summary"), "Draft the summary");
        assert_eq!(to_sentence_case("draft the summary"), "Draft the summary");
        assert_eq!(
            to_sentence_case("Identify Core Themes"),
            "Identify core themes"
        );
        assert_eq!(
            to_sentence_case("Source Material Review & Extraction"),
            "Source material review & extraction"
        );
        assert_eq!(
            to_sentence_case("ANALYZE THE REQUEST"),
            "Analyze the request"
        );
        assert_eq!(
            to_sentence_case("Querying the LLM API"),
            "Querying the LLM API"
        );
        assert_eq!(
            to_sentence_case("Reviewing 5 articles to synthesize…"),
            "Reviewing 5 articles to synthesize…"
        );
        assert_eq!(
            to_sentence_case("Step 1: Ingesting Data"),
            "Step 1: Ingesting data"
        );
        assert_eq!(
            to_sentence_case("Step 2: Processing Content"),
            "Step 2: Processing content"
        );
    }

    #[test]
    fn test_strip_markdown_decorations() {
        assert_eq!(strip_markdown_decorations(""), "");
        assert_eq!(strip_markdown_decorations("   "), "");
        assert_eq!(strip_markdown_decorations("**_#`"), "");
        assert_eq!(
            strip_markdown_decorations("## 1. **Identify** _core_ `themes`\n"),
            "1. Identify core themes"
        );
        assert_eq!(
            strip_markdown_decorations("   **Step 1:**   Formulate plan   "),
            "Step 1: Formulate plan"
        );
    }
}
