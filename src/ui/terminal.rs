use crate::config::PagingMode;
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
                    let display = match crossterm::terminal::size() {
                        Ok((cols, _)) => fit_spinner_message(&msg, cols as usize),
                        Err(_) => Cow::Borrowed(msg.as_str()),
                    };
                    if Ui::use_color() {
                        eprint!("\r\x1b[2K\x1b[1;36m{}\x1b[0m {}", tick, display);
                    } else {
                        eprint!("\r\x1b[2K{} {}", tick, display);
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
        let msg = msg.into();
        let s = sanitize_title(msg.as_ref());
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

    pub fn println(&self, msg: impl AsRef<str>) {
        if !self.inner.hidden {
            let safe = sanitize_terminal_text_cow(msg.as_ref());
            eprint!("\r\x1b[2K{}\n", safe);
            let _ = std::io::stderr().flush();
        }
    }
}

pub(super) fn char_display_width(c: char) -> usize {
    if c.is_ascii() {
        return 1;
    }
    match c as u32 {
        0x1100..=0x115F
        | 0x2E80..=0xA4CF
        | 0xAC00..=0xD7A3
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE6F
        | 0xFF00..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F300..=0x1FAFF => 2,
        _ => 1,
    }
}

/// Fits a single-line spinner message within `terminal_cols` so `\r\x1b[2K` redraws never wrap.
fn fit_spinner_message(msg: &str, terminal_cols: usize) -> Cow<'_, str> {
    let max_cols = terminal_cols.saturating_sub(3);
    if max_cols <= 3 {
        return Cow::Borrowed("");
    }
    let limit = max_cols - 3;
    let mut width = 0usize;
    let mut fit_limit_bytes = None;
    for (idx, c) in msg.char_indices() {
        let cw = char_display_width(c);
        if fit_limit_bytes.is_none() && width + cw > limit {
            fit_limit_bytes = Some(idx);
        }
        width += cw;
        if width > max_cols {
            let end = fit_limit_bytes.unwrap_or(idx);
            let min_cut = msg.find(": ").map_or(0, |i| i + 2);
            let cut = msg[..end]
                .rfind(' ')
                .filter(|&i| i > min_cut)
                .unwrap_or(end);
            let trimmed = msg[..cut].trim_end_matches([' ', ':', '.', '-', '…', '(']);
            return Cow::Owned(format!("{}...", trimmed));
        }
    }
    Cow::Borrowed(msg)
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

/// Strips ANSI/C1/BiDi/control sequences (CWE-150/CWE-451), borrowing clean inputs zero-copy.
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

/// Formats a duration into a compact string (`"45ms"`, `"0.4s"`, `"2.0s"`, `"1m 5s"`).
pub fn format_duration_short(d: Duration) -> String {
    let millis = d.as_millis();
    if millis < 100 {
        return format!("{millis}ms");
    }
    let tenths = (d.as_secs_f64() * 10.0).round() as u64;
    if tenths < 600 {
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else {
        let total_secs = (tenths + 5) / 10;
        format!("{}m {}s", total_secs / 60, total_secs % 60)
    }
}

pub use super::markdown::render_markdown;

/// Returns the usable terminal line width when stdout is a TTY, leaving 1 column
/// before the right margin to prevent terminal auto-wrap quirks.
pub fn terminal_width() -> Option<usize> {
    if !io::stdout().is_terminal() {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| (cols as usize).saturating_sub(1).max(20))
}

fn measure_visible_and_sgr<'a>(word: &'a str, active_sgr: &mut Option<&'a str>) -> usize {
    let bytes = word.as_bytes();
    let mut i = 0;
    let mut width = 0;
    while i < bytes.len() {
        if bytes[i] == b'\x1b' && bytes.get(i + 1) == Some(&b'[') {
            let start = i;
            i += 2;
            while i < bytes.len() && !(b'@'..=b'~').contains(&bytes[i]) {
                i += 1;
            }
            if i < bytes.len() {
                if bytes[i] == b'm' {
                    let seq = &word[start..=i];
                    *active_sgr = (seq != "\x1b[0m" && seq != "\x1b[m").then_some(seq);
                }
                i += 1;
            }
            continue;
        }
        let ch = word[i..].chars().next().unwrap_or('\0');
        width += char_display_width(ch);
        i += ch.len_utf8();
    }
    width
}

/// Wraps text to `max_width` visible columns on whitespace boundaries, preserving ANSI SGR styling across line breaks.
pub fn wrap_terminal_text(text: &str, max_width: usize) -> String {
    if max_width == 0 {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    for (line_idx, line) in text.split('\n').enumerate() {
        if line_idx > 0 {
            out.push('\n');
        }
        let mut dummy_sgr = None;
        if measure_visible_and_sgr(line, &mut dummy_sgr) <= max_width {
            out.push_str(line);
            continue;
        }
        let leading = line.bytes().take_while(|&b| b == b' ').count();
        for _ in 0..leading {
            out.push(' ');
        }
        let mut col = leading;
        let mut line_has_word = false;
        let mut active_sgr: Option<&str> = None;
        for word in line[leading..].split_whitespace() {
            let prev_sgr = active_sgr;
            let word_width = measure_visible_and_sgr(word, &mut active_sgr);
            let space = usize::from(line_has_word);
            if line_has_word && col + space + word_width > max_width {
                if prev_sgr.is_some() {
                    out.push_str("\x1b[0m");
                }
                out.push('\n');
                if let Some(sgr) = prev_sgr {
                    out.push_str(sgr);
                }
                col = 0;
            } else if line_has_word {
                out.push(' ');
                col += 1;
            }
            out.push_str(word);
            col += word_width;
            line_has_word = true;
        }
    }
    out
}

/// Wraps `text` when a terminal width is available, borrowing zero-copy when `width` is `None`.
pub fn wrap_for_terminal<'a>(text: Cow<'a, str>, width: Option<usize>) -> Cow<'a, str> {
    match width {
        Some(w) => Cow::Owned(wrap_terminal_text(text.as_ref(), w)),
        None => text,
    }
}

/// Incremental line-buffered markdown stream renderer for real-time terminal output.
pub struct StreamMarkdownRenderer {
    line_buf: String,
    fence: Option<(u8, usize)>,
    use_color: bool,
    wrap_width: Option<usize>,
}

impl Default for StreamMarkdownRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamMarkdownRenderer {
    pub fn new() -> Self {
        Self::with_options(Ui::use_color())
    }

    pub fn with_options(use_color: bool) -> Self {
        Self {
            line_buf: String::with_capacity(256),
            fence: None,
            use_color,
            wrap_width: terminal_width(),
        }
    }

    /// Maximum un-terminated line length buffered before flushing mid-line to bound memory.
    const MAX_STREAM_LINE_BYTES: usize = 16 * 1024;

    /// Feeds a chunk of text into the streaming renderer, rendering completed lines.
    pub fn feed(&mut self, chunk: &str) {
        let mut rest = chunk;
        while !rest.is_empty() {
            let space = Self::MAX_STREAM_LINE_BYTES.saturating_sub(self.line_buf.len());
            if space == 0 {
                self.flush_partial_line();
                continue;
            }
            let take = rest.floor_char_boundary(space);
            if take == 0 {
                if self.line_buf.is_empty() {
                    break;
                }
                self.flush_partial_line();
                continue;
            }
            let (head, tail) = rest.split_at(take);
            self.line_buf.push_str(head);
            rest = tail;
            self.drain_complete_lines();
        }
    }

    fn drain_complete_lines(&mut self) {
        while let Some(pos) = self.line_buf.find('\n') {
            let line: String = self.line_buf.drain(..=pos).collect();
            let line_trimmed = line.trim_end_matches(['\r', '\n']);
            let rendered = self.render_complete_line(line_trimmed);
            print!("{}", rendered);
        }
    }

    fn render_complete_line(&mut self, line_trimmed: &str) -> String {
        let sanitized = sanitize_terminal_text_cow(line_trimmed);
        let safe = sanitized.as_ref();
        let trimmed = safe.trim_start();
        let in_fence = self.fence.is_some();
        let is_fence_line = if let Some((ch, min_len)) = self.fence {
            if super::markdown::is_fence_close(trimmed, ch, min_len) {
                self.fence = None;
                true
            } else {
                false
            }
        } else if let Some((ch, len, _)) = super::markdown::parse_fence_open(trimmed) {
            self.fence = Some((ch, len));
            true
        } else {
            false
        };

        if !self.use_color {
            let width = self.wrap_width.filter(|_| !in_fence && !is_fence_line);
            let text = wrap_for_terminal(Cow::Borrowed(safe), width);
            format!("{}\n", text)
        } else if is_fence_line {
            format!("\x1b[32m{}\x1b[0m\n", safe)
        } else if in_fence {
            format!("\x1b[32m  {}\x1b[0m\n", safe)
        } else if safe.is_empty() {
            "\n".to_string()
        } else {
            let rendered = render_markdown(safe);
            let wrapped = wrap_for_terminal(Cow::Owned(rendered), self.wrap_width);
            if wrapped.ends_with('\n') {
                wrapped.into_owned()
            } else {
                format!("{}\n", wrapped)
            }
        }
    }

    fn render_partial_line(&self, raw: &str) -> String {
        let sanitized = sanitize_terminal_text_cow(raw);
        let safe = sanitized.as_ref();
        let rendered = if !self.use_color {
            Cow::Borrowed(safe)
        } else {
            Cow::Owned(render_markdown(safe))
        };
        let width = self.wrap_width.filter(|_| self.fence.is_none());
        wrap_for_terminal(rendered, width).into_owned()
    }

    fn flush_partial_line(&mut self) {
        if !self.line_buf.is_empty() {
            let rendered = self.render_partial_line(&self.line_buf);
            print!("{}", rendered);
            self.line_buf.clear();
            let _ = io::stdout().flush();
        }
    }

    /// Flushes any remaining partial line at stream completion.
    pub fn flush(&mut self) {
        self.drain_complete_lines();
        self.flush_partial_line();
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
    let wrap_width = terminal_width();
    let sanitized_title = sanitize_title(title);
    let sanitized_content = sanitize_terminal_text_cow(content);

    // If output is redirected/piped, or plain mode is requested, or colors are disabled:
    if !stdout_is_tty || plain || !Ui::use_color() {
        if stdout_is_tty && !plain {
            Ui::header(&format!("Summary: {}", sanitized_title));
        }
        let body = wrap_for_terminal(sanitized_content, wrap_width);
        println!("{}\n", body);
        return Ok(());
    }

    // Terminal mode: format markdown
    let header = format!(
        "\n\x1b[1;36m=== Summary: {} ===\x1b[0m\n\n",
        sanitized_title
    );
    let formatted_body =
        wrap_for_terminal(Cow::Owned(render_markdown(&sanitized_content)), wrap_width);
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
        let wrapped_hint = wrap_for_terminal(sanitize_terminal_text_cow(h), wrap_width);
        full_output.push_str(&format!("\n\x1b[2m{}\x1b[0m\n", wrapped_hint));
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

fn parse_pager_command(pager_cmd: &str) -> (&str, Vec<&str>) {
    let mut parts = pager_cmd.split_whitespace();
    let Some(bin) = parts.next() else {
        return ("less", vec!["-R", "-F", "-X"]);
    };
    if bin.is_empty() || bin.starts_with('-') || bin.chars().any(char::is_control) {
        return ("less", vec!["-R", "-F", "-X"]);
    }
    let mut args: Vec<&str> = parts.collect();
    if bin == "less" && args.is_empty() {
        args = vec!["-R", "-F", "-X"];
    }
    (bin, args)
}

/// Pipes content through the system pager ($PAGER or less -RFX).
fn run_pager(content: &str) -> io::Result<()> {
    let pager_cmd = env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let (safe_bin, args) = parse_pager_command(&pager_cmd);

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
        let rendered = render_markdown(md);
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
            let mut renderer = StreamMarkdownRenderer::with_options(use_color);
            renderer.feed("Line 1\nLine 2\nPartial line");
            assert_eq!(renderer.line_buf, "Partial line");
            renderer.flush();
            assert!(renderer.line_buf.is_empty());
        }
    }

    #[test]
    fn test_spinner_lifecycle() {
        let active = ProgressBar::new_spinner();
        active.set_message("Testing spinner");
        let hidden = ProgressBar::hidden();
        assert!(hidden.is_hidden());

        for pb in [active, hidden] {
            assert!(!pb.is_finished());
            pb.finish_and_clear();
            assert!(pb.is_finished());
            pb.reset();
            assert!(!pb.is_finished());
            pb.finish_and_clear();
            assert!(pb.is_finished());
        }
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

    #[test]
    fn test_stream_markdown_renderer_strips_terminal_escapes() {
        let mut plain = StreamMarkdownRenderer::with_options(false);
        let complete = plain.render_complete_line("\x1b]2;pwned\x07hello\x1b[31m!\r");
        assert_eq!(complete, "hello!\n");
        let partial = plain.render_partial_line("\x1b[2Jpartial\x07\r");
        assert_eq!(partial, "partial");

        let mut colored = StreamMarkdownRenderer::with_options(true);
        let fence = colored.render_complete_line("```\x1b]0;title\x07");
        assert!(!fence.contains("\x1b]0;"));
        assert!(!fence.contains('\x07'));
        let code_line = colored.render_complete_line("let x = \x1b[31m1\x1b[0m;\r");
        assert!(!code_line.contains("\x1b[31m"));
        assert!(!code_line.contains('\r'));
        assert!(code_line.contains("let x = 1;"));
        let colored_partial = colored.render_partial_line("tail\x1b[2J\x07");
        assert!(!colored_partial.contains("\x1b[2J"));
        assert!(!colored_partial.contains('\x07'));

        let rendered_md = render_markdown("\x1b]2;pwned\x07**bold**\x1b[31m");
        assert!(!rendered_md.contains("\x1b]2;"));
        assert!(!rendered_md.contains("\x1b[31m"));
        assert!(!rendered_md.contains('\x07'));
        assert!(rendered_md.contains("bold"));
    }

    #[test]
    fn test_stream_markdown_renderer_bounds_unterminated_line() {
        let mut renderer = StreamMarkdownRenderer::with_options(false);
        renderer.feed(&"A".repeat(40_000));
        assert!(renderer.line_buf.len() <= StreamMarkdownRenderer::MAX_STREAM_LINE_BYTES);

        // Multi-byte UTF-8 input across buffer limit boundaries must not panic
        renderer.feed(&"🦀".repeat(10_000));
        assert!(renderer.line_buf.len() <= StreamMarkdownRenderer::MAX_STREAM_LINE_BYTES);
        renderer.flush();
        assert!(renderer.line_buf.is_empty());
    }

    #[test]
    fn test_progress_bar_set_message_sanitizes_control_sequences() {
        let pb = ProgressBar::hidden();
        pb.set_message("Load \x1b]2;evil\x07\x1b[31mfile\r\nname");
        let stored = pb.inner.message.lock().unwrap().clone();
        assert!(!stored.contains('\x1b'));
        assert!(!stored.contains('\x07'));
        assert!(!stored.contains('\r'));
        assert!(!stored.contains('\n'));
        assert_eq!(stored, "Load file name");
    }

    #[test]
    fn test_fit_spinner_message_adapts_to_terminal_width() {
        let msg = "💭 Thinking: Draft paragraph 2 - technical depth and complexity overview...";
        assert!(matches!(fit_spinner_message(msg, 120), Cow::Borrowed(_)));
        assert_eq!(
            fit_spinner_message(msg, 55),
            "💭 Thinking: Draft paragraph 2 - technical depth..."
        );
        assert_eq!(fit_spinner_message(msg, 6), "");
    }

    #[test]
    fn test_wrap_terminal_text_and_ansi_sgr() {
        // Basic word wrapping without cutting words mid-word
        let wrapped =
            wrap_terminal_text("Nvidia crisis triggered massive financial uncertainty.", 24);
        assert_eq!(
            wrapped,
            "Nvidia crisis triggered\nmassive financial\nuncertainty."
        );

        // ANSI codes count as zero width and active style carries across wraps
        let styled = "\x1b[1;37mCEO Resignation and Collapse\x1b[0m follows.";
        let wrapped_styled = wrap_terminal_text(styled, 18);
        assert_eq!(
            wrapped_styled,
            "\x1b[1;37mCEO Resignation\x1b[0m\n\x1b[1;37mand Collapse\x1b[0m\nfollows."
        );

        // Indented list item and unbreakable long URL
        let indented = "  \x1b[36m•\x1b[0m Read https://example.com/very/long/path/here now";
        let wrapped_url = wrap_terminal_text(indented, 16);
        assert_eq!(
            wrapped_url,
            "  \x1b[36m•\x1b[0m Read\nhttps://example.com/very/long/path/here\nnow"
        );

        // None width borrows without allocating
        let input = "Unchanged paragraph";
        assert!(matches!(
            wrap_for_terminal(Cow::Borrowed(input), None),
            Cow::Borrowed("Unchanged paragraph")
        ));

        assert_eq!(
            parse_pager_command("--evil foo"),
            ("less", vec!["-R", "-F", "-X"])
        );
        assert_eq!(parse_pager_command("most -s"), ("most", vec!["-s"]));
    }
}
