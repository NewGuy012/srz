use crate::config::cli::PagingMode;
use std::borrow::Cow;
use std::env;
use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct ProgressBar {
    inner: Arc<SpinnerInner>,
}

pub type Spinner = ProgressBar;

struct SpinnerInner {
    hidden: bool,
    start_time: Mutex<Instant>,
    finished: AtomicBool,
    message: Mutex<String>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for SpinnerInner {
    fn drop(&mut self) {
        if !self.finished.swap(true, Ordering::SeqCst) && !self.hidden {
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
        }
    }
}

impl ProgressBar {
    pub fn hidden() -> Self {
        Self {
            inner: Arc::new(SpinnerInner {
                hidden: true,
                start_time: Mutex::new(Instant::now()),
                finished: AtomicBool::new(false),
                message: Mutex::new(String::new()),
                handle: Mutex::new(None),
            }),
        }
    }

    pub fn new_spinner() -> Self {
        if Ui::is_quiet() || !*STDERR_IS_TERMINAL {
            return Self::hidden();
        }
        let inner = Arc::new(SpinnerInner {
            hidden: false,
            start_time: Mutex::new(Instant::now()),
            finished: AtomicBool::new(false),
            message: Mutex::new(String::new()),
            handle: Mutex::new(None),
        });
        let pb = Self { inner };
        pb.start_ticker();
        pb
    }

    fn start_ticker(&self) {
        if self.inner.hidden {
            return;
        }
        self.inner.finished.store(false, Ordering::SeqCst);
        let inner_clone = Arc::clone(&self.inner);
        let handle = thread::spawn(move || {
            const TICKS: &[char] = &['⠋', '⠙', '⠚', '⠞', '⠖', '⠦', '⠴', '⠲', '⠳', '⠓'];
            let mut idx = 0;
            while !inner_clone.finished.load(Ordering::SeqCst) {
                let msg = inner_clone.message.lock().unwrap().clone();
                if !msg.is_empty() {
                    let tick = TICKS[idx % TICKS.len()];
                    if Ui::use_color() {
                        eprint!("\r\x1b[2K\x1b[1;36m{}\x1b[0m {}", tick, msg);
                    } else {
                        eprint!("\r\x1b[2K{} {}", tick, msg);
                    }
                    let _ = std::io::stderr().flush();
                }
                idx += 1;
                thread::sleep(Duration::from_millis(80));
            }
        });
        *self.inner.handle.lock().unwrap() = Some(handle);
    }

    pub fn set_message(&self, msg: impl Into<Cow<'static, str>>) {
        let s = msg.into().into_owned();
        *self.inner.message.lock().unwrap() = s;
    }

    pub fn elapsed(&self) -> Duration {
        self.inner.start_time.lock().unwrap().elapsed()
    }

    pub fn finish_and_clear(&self) {
        if self.inner.finished.swap(true, Ordering::SeqCst) {
            return;
        }
        if let Some(h) = self.inner.handle.lock().unwrap().take() {
            let _ = h.join();
        }
        if !self.inner.hidden {
            eprint!("\r\x1b[2K");
            let _ = std::io::stderr().flush();
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.inner.hidden
    }

    pub fn is_finished(&self) -> bool {
        self.inner.finished.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.finish_and_clear();
        *self.inner.start_time.lock().unwrap() = Instant::now();
        self.inner.finished.store(false, Ordering::SeqCst);
        if !self.inner.hidden {
            self.start_ticker();
        }
    }

    pub fn enable_steady_tick(&self, _d: Duration) {}

    pub fn tick(&self) {}

    pub fn println(&self, msg: impl AsRef<str>) {
        if !self.inner.hidden {
            eprint!("\r\x1b[2K{}\n", msg.as_ref());
            let _ = std::io::stderr().flush();
        }
    }
}

static QUIET: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);

static STDERR_IS_TERMINAL: LazyLock<bool> = LazyLock::new(|| std::io::stderr().is_terminal());

static USE_COLOR: LazyLock<bool> = LazyLock::new(|| {
    if env::var_os("CLICOLOR_FORCE")
        .map(|v| v != "0")
        .unwrap_or(false)
    {
        return true;
    }
    if env::var_os("NO_COLOR").is_some() {
        return false;
    }
    if let Some(val) = env::var_os("CLICOLOR")
        && val == "0"
    {
        return false;
    }
    *STDERR_IS_TERMINAL
});

pub struct Ui;

impl Ui {
    /// Sets whether quiet mode is enabled globally.
    pub fn set_quiet(quiet: bool) {
        QUIET.store(quiet, Ordering::Relaxed);
    }

    /// Returns whether quiet mode is active.
    pub fn is_quiet() -> bool {
        QUIET.load(Ordering::Relaxed)
    }

    /// Sets whether verbose mode is enabled globally.
    pub fn set_verbose(verbose: bool) {
        VERBOSE.store(verbose, Ordering::Relaxed);
    }

    /// Returns whether verbose mode is active.
    pub fn is_verbose() -> bool {
        VERBOSE.load(Ordering::Relaxed)
    }

    /// Determines if ANSI color escapes should be emitted to stderr based on
    /// cached `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, and terminal detection.
    pub fn use_color() -> bool {
        *USE_COLOR
    }

    /// Creates a styled spinner on stderr with a status message,
    /// or a hidden progress bar if quiet mode is enabled, stderr is not a terminal,
    /// or the message is empty.
    pub fn spinner(message: impl Into<std::borrow::Cow<'static, str>>) -> ProgressBar {
        let msg = message.into();
        if Self::is_quiet() || !*STDERR_IS_TERMINAL || msg.trim().is_empty() {
            return ProgressBar::hidden();
        }
        let pb = ProgressBar::new_spinner();
        pb.set_message(msg);
        pb
    }

    /// Clears the spinner from stderr and prints a clean completion line with elapsed timing.
    pub fn finish_spinner(pb: &ProgressBar, message: &str) {
        let elapsed = pb.elapsed();
        pb.finish_and_clear();
        if !message.trim().is_empty() {
            Self::success_with_elapsed(message, elapsed);
        }
    }

    /// Prints a success message to stderr, optionally appending formatted elapsed duration.
    pub fn success_with_elapsed(message: &str, elapsed: Duration) {
        if Self::is_quiet() || message.trim().is_empty() {
            return;
        }
        let safe_msg = sanitize_terminal_text_cow(message);
        if elapsed >= Duration::from_millis(50) {
            let elapsed_str = format_duration_short(elapsed);
            if Self::use_color() {
                eprintln!(
                    "\x1b[1;32m✔\x1b[0m {} \x1b[2m({})\x1b[0m",
                    safe_msg, elapsed_str
                );
            } else {
                eprintln!("✔ {} ({})", safe_msg, elapsed_str);
            }
        } else {
            Self::success(message);
        }
    }

    /// Runs an async operation with a spinner, cleanly clearing the spinner before printing
    /// the completion message or propagating an error.
    pub async fn with_spinner<T, E, F>(
        start_message: &'static str,
        done_message: &str,
        fut: F,
    ) -> Result<T, E>
    where
        F: Future<Output = Result<T, E>>,
    {
        let pb = Self::spinner(start_message);
        match fut.await {
            Ok(val) => {
                Self::finish_spinner(&pb, done_message);
                Ok(val)
            }
            Err(err) => {
                pb.finish_and_clear();
                Err(err)
            }
        }
    }

    /// Prints a section header to stderr (keeping stdout pure for piped Markdown).
    pub fn header(title: &str) {
        if Self::is_quiet() {
            return;
        }
        let safe_title = sanitize_title(title);
        if Self::use_color() {
            eprintln!("\n\x1b[1;36m=== {} ===\x1b[0m\n", safe_title);
        } else {
            eprintln!("\n=== {} ===\n", safe_title);
        }
    }

    /// Prints an informational message to stderr.
    pub fn info(message: &str) {
        if !Self::is_quiet() {
            Self::print_styled("ℹ", "1;34", message);
        }
    }

    /// Prints a success message to stderr.
    pub fn success(message: &str) {
        if !Self::is_quiet() && !message.trim().is_empty() {
            Self::print_styled("✔", "1;32", message);
        }
    }

    /// Prints a warning message to stderr.
    pub fn warn(message: &str) {
        if !Self::is_quiet() {
            Self::print_styled("⚠", "1;33", message);
        }
    }

    /// Prints an error message to stderr.
    pub fn error(message: &str) {
        Self::print_styled("✖", "1;31", message);
    }

    /// Prints verbose diagnostic message if verbose mode is enabled.
    pub fn verbose(message: &str) {
        if Self::is_verbose() {
            Self::print_styled("[debug]", "1;35", message);
        }
    }

    fn print_styled(symbol: &str, color_code: &str, message: &str) {
        let safe_msg = sanitize_terminal_text_cow(message);
        if Self::use_color() {
            eprintln!("\x1b[{color_code}m{symbol}\x1b[0m {safe_msg}");
        } else {
            eprintln!("{symbol} {safe_msg}");
        }
    }
}

#[inline]
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
    )
}

/// Sanitizes a string for safe terminal display by stripping ANSI escape sequences,
/// string-type controls (OSC, DCS, SOS, PM, APC), 8-bit C1 controls, BiDi overrides,
/// and unprintable ASCII control characters (CWE-150 / CWE-451).
///
/// Returns a borrowed `Cow::Borrowed` when the input contains no suspicious characters,
/// avoiding heap allocations on clean inputs.
pub fn sanitize_terminal_text_cow<'a>(input: &'a str) -> Cow<'a, str> {
    let has_suspicious = input.chars().any(|c| {
        c == '\x1b'
            || (c.is_control() && c != '\n' && c != '\t')
            || is_bidi_control(c)
            || matches!(
                c,
                '\u{0090}' | '\u{0098}' | '\u{009B}' | '\u{009D}' | '\u{009E}' | '\u{009F}'
            )
    });
    if !has_suspicious {
        return Cow::Borrowed(input);
    }

    let mut clean = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    let skip_csi = |iter: &mut std::iter::Peekable<std::str::Chars>| {
        for c in iter.by_ref() {
            if ('@'..='~').contains(&c) {
                break;
            }
        }
    };

    let skip_string_control = |iter: &mut std::iter::Peekable<std::str::Chars>| {
        let mut prev = '\0';
        for c in iter.by_ref() {
            if c == '\x07' || c == '\u{009C}' || (prev == '\x1b' && c == '\\') {
                break;
            }
            prev = c;
        }
    };

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    skip_csi(&mut chars);
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    chars.next(); // consume string sequence initiator
                    skip_string_control(&mut chars);
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }

        // 8-bit C1 CSI sequence (\u{009B})
        if ch == '\u{009B}' {
            skip_csi(&mut chars);
            continue;
        }

        // 8-bit C1 string-type sequences (OSC \u{009D}, DCS \u{0090}, SOS \u{0098}, PM \u{009E}, APC \u{009F})
        if matches!(
            ch,
            '\u{0090}' | '\u{0098}' | '\u{009D}' | '\u{009E}' | '\u{009F}'
        ) {
            skip_string_control(&mut chars);
            continue;
        }

        // Filter out control characters (except \n and \t) and Unicode BiDi overrides
        if (ch.is_control() && ch != '\n' && ch != '\t') || is_bidi_control(ch) {
            continue;
        }

        clean.push(ch);
    }

    Cow::Owned(clean)
}

/// Sanitizes a string for safe terminal display by stripping ANSI escape sequences,
/// string-type controls (OSC, DCS, SOS, PM, APC), 8-bit C1 controls, BiDi overrides,
/// and unprintable ASCII control characters (CWE-150 / CWE-451).
pub fn sanitize_terminal_text(input: &str) -> String {
    sanitize_terminal_text_cow(input).into_owned()
}

/// Sanitizes a title string for terminal presentation by removing escape sequences and collapsing whitespace.
pub fn sanitize_title(title: &str) -> String {
    sanitize_terminal_text_cow(title)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Formats a duration into a human-readable compact string (e.g., `80ms`, `1.4s`, `2m 15s`).
pub fn format_duration_short(d: Duration) -> String {
    let secs = d.as_secs_f64();
    if secs < 0.1 {
        format!("{:.0}ms", d.as_millis())
    } else if secs < 60.0 {
        format!("{:.1}s", secs)
    } else {
        let mins = (secs / 60.0).floor() as u64;
        let rem_secs = secs - (mins as f64 * 60.0);
        format!("{}m {:.0}s", mins, rem_secs)
    }
}

/// Renders markdown string into ANSI formatted terminal text using pulldown-cmark.
pub fn render_markdown(markdown: &str, _width: Option<usize>) -> String {
    use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};

    let mut out = String::with_capacity(markdown.len() * 2);
    let mut in_code_block = false;
    let mut list_depth: usize = 0;

    for event in Parser::new(markdown) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => match level {
                HeadingLevel::H1 => out.push_str("\x1b[1;36;4m"),
                HeadingLevel::H2 => out.push_str("\x1b[1;36m"),
                _ => out.push_str("\x1b[1;33m"),
            },
            Event::End(TagEnd::Heading(_)) => {
                out.push_str("\x1b[0m\n\n");
            }
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => {
                out.push_str("\n\n");
            }
            Event::Start(Tag::List(_)) => {
                list_depth += 1;
            }
            Event::End(TagEnd::List(_)) => {
                list_depth = list_depth.saturating_sub(1);
                if list_depth == 0 {
                    out.push('\n');
                }
            }
            Event::Start(Tag::Item) => {
                let indent = "  ".repeat(list_depth.saturating_sub(1));
                out.push_str(&indent);
                out.push_str("\x1b[36m•\x1b[0m ");
            }
            Event::End(TagEnd::Item) => {
                out.push('\n');
            }
            Event::Start(Tag::Strong) => out.push_str("\x1b[1;37m"),
            Event::End(TagEnd::Strong) => out.push_str("\x1b[0m"),
            Event::Start(Tag::Emphasis) => out.push_str("\x1b[35m"),
            Event::End(TagEnd::Emphasis) => out.push_str("\x1b[0m"),
            Event::Start(Tag::CodeBlock(_)) => {
                in_code_block = true;
                out.push_str("\x1b[32m");
            }
            Event::End(TagEnd::CodeBlock) => {
                in_code_block = false;
                out.push_str("\x1b[0m\n\n");
            }
            Event::Code(code) => {
                out.push_str("\x1b[32m");
                out.push_str(&code);
                out.push_str("\x1b[0m");
            }
            Event::Text(text) => {
                out.push_str(&text);
            }
            Event::SoftBreak => {
                if in_code_block {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
            Event::HardBreak => out.push('\n'),
            Event::Rule => out.push_str("\x1b[2m──────────\x1b[0m\n\n"),
            _ => {}
        }
    }

    out.trim_end().to_string()
}

/// Incremental line-buffered markdown stream renderer for real-time terminal output.
/// Buffers streaming token chunks until line boundaries are reached, then applies Termimad ANSI styling.
pub struct StreamMarkdownRenderer {
    line_buf: String,
    in_code_block: bool,
    use_color: bool,
    width: usize,
}

impl Default for StreamMarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamMarkdownRenderer {
    pub fn new() -> Self {
        let use_color = Ui::use_color();
        let width = crossterm::terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or(80);
        Self::with_options(use_color, width)
    }

    pub fn with_options(use_color: bool, width: usize) -> Self {
        Self {
            line_buf: String::with_capacity(256),
            in_code_block: false,
            use_color,
            width,
        }
    }

    /// Feeds a chunk of text into the streaming renderer, rendering completed lines.
    pub fn feed(&mut self, chunk: &str) {
        self.line_buf.push_str(chunk);
        while let Some(pos) = self.line_buf.find('\n') {
            let line: String = self.line_buf.drain(..=pos).collect();
            let line_trimmed = line.trim_end_matches(['\r', '\n']);

            if !self.use_color {
                println!("{}", line_trimmed);
            } else if line_trimmed.trim_start().starts_with("```") {
                self.in_code_block = !self.in_code_block;
                println!("\x1b[32m{}\x1b[0m", line_trimmed);
            } else if self.in_code_block {
                println!("\x1b[32m  {}\x1b[0m", line_trimmed);
            } else if line_trimmed.is_empty() {
                println!();
            } else {
                let rendered = render_markdown(line_trimmed, Some(self.width));
                print!("{}", rendered);
                if !rendered.ends_with('\n') {
                    println!();
                }
            }
        }
    }

    /// Flushes any remaining partial line at stream completion.
    pub fn flush(&mut self) {
        if !self.line_buf.is_empty() {
            if !self.use_color {
                print!("{}", self.line_buf);
            } else {
                let rendered = render_markdown(&self.line_buf, Some(self.width));
                print!("{}", rendered);
            }
            self.line_buf.clear();
            let _ = io::stdout().flush();
        }
    }
}

/// Prints a unified terminal summary document (including optional view file links and context hints),
/// launching a pager if the combined document exceeds terminal height.
pub fn display_summary_full(
    title: &str,
    content: &str,
    paging_mode: PagingMode,
    plain: bool,
    view_files: Option<(&Path, &Path)>,
    hint: Option<&str>,
) -> io::Result<()> {
    let stdout_is_tty = io::stdout().is_terminal();
    let sanitized_title = sanitize_title(title);
    let sanitized_content = sanitize_terminal_text_cow(content);

    // If output is redirected/piped, or plain mode is requested, or colors are disabled:
    if !stdout_is_tty || plain || !Ui::use_color() {
        if stdout_is_tty && !plain {
            Ui::header(&format!("Summary: {}", sanitized_title));
        }
        println!("{}\n", sanitized_content);
        return Ok(());
    }

    // Terminal mode: format markdown
    let header = format!(
        "\n\x1b[1;36m=== Summary: {} ===\x1b[0m\n\n",
        sanitized_title
    );
    let formatted_body = render_markdown(&sanitized_content, None);
    let mut full_output = format!("{}{}\n", header, formatted_body);

    // Append view file links if available
    if let Some((html_path, md_path)) = view_files {
        full_output.push_str(&format!(
            "\n\x1b[32m✔\x1b[0m \x1b[2mStyled HTML view:\x1b[0m file://{}\n\x1b[32m✔\x1b[0m \x1b[2mMarkdown file:   \x1b[0m file://{}\n",
            html_path.display(),
            md_path.display()
        ));
    }

    // Append hint if available
    if let Some(h) = hint {
        full_output.push_str(&format!("\n\x1b[2m{}\x1b[0m\n", h));
    }

    let (_, term_height) = crossterm::terminal::size().unwrap_or((80, 24));
    let line_count = full_output.lines().count();

    let should_page = match paging_mode {
        PagingMode::Always => true,
        PagingMode::Never => false,
        PagingMode::Auto => line_count > term_height.saturating_sub(4) as usize,
    };

    if should_page {
        if let Err(err) = run_pager(&full_output) {
            Ui::verbose(&format!("Pager error, falling back to direct print: {err}"));
            print!("{}", full_output);
            io::stdout().flush()?;
        }
    } else {
        print!("{}", full_output);
        io::stdout().flush()?;
    }

    Ok(())
}

/// Pipes content through the system pager ($PAGER or less -RFX).
fn run_pager(content: &str) -> io::Result<()> {
    let pager_cmd = env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let mut parts = pager_cmd.split_whitespace();
    let bin = parts.next().unwrap_or("less");
    let safe_bin = if bin.is_empty() || bin.starts_with('-') || bin.chars().any(|c| c.is_control())
    {
        "less"
    } else {
        bin
    };
    let mut args: Vec<&str> = parts.collect();

    // If using default `less`, add flags to display colors and quit if fits on one screen
    if safe_bin == "less" && args.is_empty() {
        args = vec!["-R", "-F", "-X"];
    }

    let mut child = Command::new(safe_bin)
        .args(&args)
        .env("LESSSECURE", "1")
        .stdin(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe error when user exits pager early (e.g. presses 'q')
        if let Err(err) = stdin.write_all(content.as_bytes())
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(err);
        }
    }

    let _ = child.wait();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_short() {
        assert_eq!(format_duration_short(Duration::from_millis(45)), "45ms");
        assert_eq!(format_duration_short(Duration::from_millis(1400)), "1.4s");
        assert_eq!(format_duration_short(Duration::from_secs(65)), "1m 5s");
        assert_eq!(format_duration_short(Duration::from_secs(120)), "2m 0s");
    }

    #[test]
    fn test_sanitize_terminal_text_strips_ansi_and_osc() {
        let csi_injected = "Hello \x1b[31;1mRed Alert\x1b[0m World";
        assert_eq!(
            sanitize_terminal_text(csi_injected),
            "Hello Red Alert World"
        );

        let osc_injected = "Normal \x1b]0;Evil Title\x07Text";
        assert_eq!(sanitize_terminal_text(osc_injected), "Normal Text");

        let control_chars = "Line\x00with\x08bell\x07and\x1bsome chars";
        assert_eq!(
            sanitize_terminal_text(control_chars),
            "Linewithbellandome chars"
        );
    }

    #[test]
    fn test_sanitize_terminal_text_strips_c1_bidi_and_string_controls() {
        // C1 8-bit CSI
        let c1_csi = "Hello \u{009B}31mC1 CSI\u{009B}0m World";
        assert_eq!(sanitize_terminal_text(c1_csi), "Hello C1 CSI World");

        // C1 8-bit OSC with ST terminator (\u{009C})
        let c1_osc = "Start \u{009D}0;Evil Title\u{009C} End";
        assert_eq!(sanitize_terminal_text(c1_osc), "Start  End");

        // 7-bit DCS, PM, APC
        let dcs_seq = "DCS \x1bP123;cmd\x1b\\Payload";
        assert_eq!(sanitize_terminal_text(dcs_seq), "DCS Payload");
        let apc_seq = "APC \x1b_secret\x07Data";
        assert_eq!(sanitize_terminal_text(apc_seq), "APC Data");

        // Unicode BiDi override injection (Trojan Source / CWE-451)
        let bidi_text = "Benign \u{202E}gnineB\u{202C} Text \u{2067}RLI\u{2069}";
        assert_eq!(sanitize_terminal_text(bidi_text), "Benign gnineB Text RLI");
    }

    #[test]
    fn test_sanitize_title_collapses_newlines() {
        let raw_title = "\x1b[1mImportant\x1b[0m\nDocument\r\nTitle";
        assert_eq!(sanitize_title(raw_title), "Important Document Title");

        let bidi_title = "Title \u{202E}with \n \t BiDi\u{202C}";
        assert_eq!(sanitize_title(bidi_title), "Title with BiDi");
    }

    #[test]
    fn test_sanitize_terminal_text_cow_zero_alloc() {
        let clean = "This is a completely clean string!";
        match sanitize_terminal_text_cow(clean) {
            std::borrow::Cow::Borrowed(borrowed) => {
                assert_eq!(borrowed, clean);
            }
            std::borrow::Cow::Owned(_) => panic!("expected clean string to be borrowed"),
        }

        let dirty = "This has \x1b[31mcolor\x1b[0m";
        match sanitize_terminal_text_cow(dirty) {
            std::borrow::Cow::Borrowed(_) => panic!("expected dirty string to be owned"),
            std::borrow::Cow::Owned(owned) => {
                assert_eq!(owned, "This has color");
            }
        }
    }

    #[test]
    fn test_render_markdown_basic() {
        let md =
            "# Title\n\nThis is **bold** and *italic* text with `code`.\n\n* Bullet 1\n* Bullet 2";
        let rendered = render_markdown(md, Some(80));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("bold"));
        assert!(rendered.contains("Bullet"));
        assert!(rendered.contains("\x1b[1;36;4m")); // H1 cyan underline
        assert!(rendered.contains("\x1b[1;37m")); // Bold white
        assert!(rendered.contains("\x1b[36m•\x1b[0m")); // Bullet
    }

    #[test]
    fn test_display_summary_plain_and_full() {
        // Plain mode should succeed without error
        let res = display_summary_full(
            "Test Title",
            "Test Body Content",
            PagingMode::Never,
            true,
            Some((Path::new("/tmp/test.html"), Path::new("/tmp/test.md"))),
            Some("💡 Context hint"),
        );
        assert!(res.is_ok());
    }

    #[test]
    fn test_stream_markdown_renderer() {
        let mut renderer = StreamMarkdownRenderer::new();
        renderer.feed("# Heading 1\n");
        renderer.feed("This is a paragraph with **bold** text.\n");
        renderer.feed("```rust\n");
        renderer.feed("let a = 42;\n");
        renderer.feed("```\n");
        renderer.feed("Trailing unclosed line without newline");
        assert!(!renderer.line_buf.is_empty());
        renderer.flush();
        assert!(renderer.line_buf.is_empty());
    }

    #[test]
    fn test_stream_markdown_renderer_options() {
        for use_color in [true, false] {
            let mut renderer = StreamMarkdownRenderer::with_options(use_color, 80);
            renderer.feed("Line 1\nLine 2\nPartial line");
            assert_eq!(renderer.line_buf, "Partial line");
            renderer.flush();
            assert!(renderer.line_buf.is_empty());
        }
    }

    #[test]
    fn test_spinner_lifecycle() {
        let pb = ProgressBar::new_spinner();
        pb.set_message("Testing spinner");
        assert!(!pb.is_finished());
        pb.finish_and_clear();
        assert!(pb.is_finished());
        pb.reset();
        assert!(!pb.is_finished());
        pb.finish_and_clear();
    }

    #[test]
    fn test_hidden_spinner_lifecycle() {
        let pb = ProgressBar::hidden();
        assert!(pb.is_hidden());
        assert!(!pb.is_finished());
        pb.finish_and_clear();
        assert!(pb.is_finished());
        pb.reset();
        assert!(!pb.is_finished());
        pb.finish_and_clear();
        assert!(pb.is_finished());
    }

    #[test]
    fn test_spinner_empty_message_returns_hidden() {
        assert!(Ui::spinner("").is_hidden());
        assert!(Ui::spinner("   ").is_hidden());
        assert!(Ui::spinner("\n\t").is_hidden());
    }

    #[test]
    fn test_success_and_elapsed_empty_message_ignored() {
        // Smoke test: verify that empty/blank strings do not panic or emit corrupt escape sequences
        Ui::success("");
        Ui::success("   ");
        Ui::success_with_elapsed("", Duration::from_secs(5));
        Ui::success_with_elapsed("   ", Duration::from_secs(5));
        let pb = ProgressBar::hidden();
        Ui::finish_spinner(&pb, "");
        Ui::finish_spinner(&pb, "   ");
        assert!(pb.is_hidden());
    }
}
