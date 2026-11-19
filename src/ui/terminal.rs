use crate::config::PagingMode;
use std::borrow::Cow;
use std::env;
use std::fmt::Write as _;
use std::future::Future;
use std::io::{self, IsTerminal, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

struct SpinnerState {
    finished: bool,
    message: String,
}

struct SpinnerShared {
    hidden: bool,
    start_time: Mutex<Instant>,
    state: Mutex<SpinnerState>,
    wake: Condvar,
}

pub(crate) struct ProgressBar {
    shared: Arc<SpinnerShared>,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for ProgressBar {
    fn drop(&mut self) {
        self.finish_and_clear();
    }
}

impl ProgressBar {
    pub fn hidden() -> Self {
        Self::with_hidden(true)
    }

    fn with_hidden(hidden: bool) -> Self {
        Self {
            shared: Arc::new(SpinnerShared {
                hidden,
                start_time: Mutex::new(Instant::now()),
                state: Mutex::new(SpinnerState {
                    finished: false,
                    message: String::new(),
                }),
                wake: Condvar::new(),
            }),
            handle: Mutex::new(None),
        }
    }

    pub fn new_spinner() -> Self {
        if Ui::is_quiet() || !*STDERR_IS_TERMINAL {
            return Self::hidden();
        }
        let pb = Self::with_hidden(false);
        pb.start_ticker();
        pb
    }

    fn start_ticker(&self) {
        if self.shared.hidden {
            return;
        }
        self.shared.state.lock().unwrap().finished = false;
        let shared = Arc::clone(&self.shared);
        let handle = thread::spawn(move || {
            const TICKS: &[char] = &['⠋', '⠙', '⠚', '⠞', '⠖', '⠦', '⠴', '⠲', '⠳', '⠓'];
            let mut idx = 0;
            let mut guard = shared.state.lock().unwrap();
            loop {
                if guard.finished {
                    break;
                }
                if !guard.message.is_empty() {
                    let msg = guard.message.clone();
                    drop(guard);
                    let tick = TICKS[idx % TICKS.len()];
                    let display = match crossterm::terminal::size() {
                        Ok((cols, _)) => fit_spinner_message(&msg, cols as usize),
                        Err(_) => Cow::Borrowed(msg.as_str()),
                    };
                    if Ui::use_color() {
                        eprint!("\r\x1b[2K\x1b[1;36m{tick}\x1b[0m {display}");
                    } else {
                        eprint!("\r\x1b[2K{tick} {display}");
                    }
                    let _ = io::stderr().flush();
                    idx += 1;
                    guard = shared.state.lock().unwrap();
                    if guard.finished {
                        break;
                    }
                }
                let (next_guard, _) = shared
                    .wake
                    .wait_timeout(guard, Duration::from_millis(80))
                    .unwrap();
                guard = next_guard;
            }
        });
        *self.handle.lock().unwrap() = Some(handle);
    }

    pub fn set_message(&self, msg: impl Into<Cow<'static, str>>) {
        let msg = msg.into();
        self.shared.state.lock().unwrap().message = sanitize_title(msg.as_ref());
    }

    pub fn elapsed(&self) -> Duration {
        self.shared.start_time.lock().unwrap().elapsed()
    }

    pub fn finish_and_clear(&self) {
        let already_finished = {
            let mut st = self.shared.state.lock().unwrap();
            let prev = st.finished;
            st.finished = true;
            prev
        };
        if already_finished {
            return;
        }
        self.shared.wake.notify_all();
        if let Some(h) = self.handle.lock().unwrap().take() {
            let _ = h.join();
        }
        if !self.shared.hidden {
            eprint!("\r\x1b[2K");
            let _ = io::stderr().flush();
        }
    }

    pub fn is_hidden(&self) -> bool {
        self.shared.hidden
    }

    pub fn is_finished(&self) -> bool {
        self.shared.state.lock().unwrap().finished
    }

    pub fn reset(&self) {
        self.finish_and_clear();
        *self.shared.start_time.lock().unwrap() = Instant::now();
        self.shared.state.lock().unwrap().finished = false;
        if !self.shared.hidden {
            self.start_ticker();
        }
    }

    pub fn println(&self, msg: impl AsRef<str>) {
        if !self.shared.hidden {
            let safe = sanitize_terminal_text_cow(msg.as_ref());
            eprint!("\r\x1b[2K{safe}\n");
            let _ = io::stderr().flush();
        }
    }
}

pub(super) fn char_display_width(c: char) -> usize {
    const ZERO_WIDTH: &[(u32, u32)] = &[
        (0x0300, 0x036F),
        (0x1AB0, 0x1AFF),
        (0x1DC0, 0x1DFF),
        (0x200B, 0x200F),
        (0x202A, 0x202E),
        (0x2060, 0x206F),
        (0x20D0, 0x20FF),
        (0xFE00, 0xFE0F),
        (0xFE20, 0xFE2F),
    ];
    const WIDE: &[(u32, u32)] = &[
        (0x1100, 0x115F),
        (0x231A, 0x231B),
        (0x23E9, 0x23FA),
        (0x25FD, 0x25FE),
        (0x2614, 0x2615),
        (0x2648, 0x2653),
        (0x267F, 0x267F),
        (0x2693, 0x2693),
        (0x26A1, 0x26A1),
        (0x26AA, 0x26AB),
        (0x26BD, 0x26BE),
        (0x26C4, 0x26C5),
        (0x26CE, 0x26CE),
        (0x26D4, 0x26D4),
        (0x26EA, 0x26EA),
        (0x26F2, 0x26F3),
        (0x26F5, 0x26F5),
        (0x26FA, 0x26FA),
        (0x26FD, 0x26FD),
        (0x2705, 0x2705),
        (0x270A, 0x270B),
        (0x2728, 0x2728),
        (0x274C, 0x274C),
        (0x274E, 0x274E),
        (0x2753, 0x2755),
        (0x2757, 0x2757),
        (0x2795, 0x2797),
        (0x27B0, 0x27B0),
        (0x27BF, 0x27BF),
        (0x2B1B, 0x2B1C),
        (0x2B50, 0x2B50),
        (0x2B55, 0x2B55),
        (0x2E80, 0xA4CF),
        (0xAC00, 0xD7A3),
        (0xF900, 0xFAFF),
        (0xFE10, 0xFE6F),
        (0xFF00, 0xFF60),
        (0xFFE0, 0xFFE6),
        (0x1F300, 0x1FAFF),
        (0x20000, 0x3FFFD),
    ];
    if c.is_control() {
        return 0;
    }
    if c.is_ascii() {
        return 1;
    }
    let cp = c as u32;
    if ZERO_WIDTH.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp)) {
        0
    } else if WIDE.iter().any(|&(lo, hi)| (lo..=hi).contains(&cp)) {
        2
    } else {
        1
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
            return Cow::Owned(format!("{trimmed}..."));
        }
    }
    Cow::Borrowed(msg)
}

static QUIET: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);

static STDERR_IS_TERMINAL: LazyLock<bool> = LazyLock::new(|| io::stderr().is_terminal());
static STDOUT_IS_TERMINAL: LazyLock<bool> = LazyLock::new(|| io::stdout().is_terminal());

fn resolve_color(is_tty: bool) -> bool {
    if env::var_os("CLICOLOR_FORCE").is_some_and(|v| v != "0") {
        return true;
    }
    if env::var_os("NO_COLOR").is_some() || env::var_os("CLICOLOR").is_some_and(|v| v == "0") {
        return false;
    }
    is_tty
}

static USE_STDERR_COLOR: LazyLock<bool> = LazyLock::new(|| resolve_color(*STDERR_IS_TERMINAL));
static USE_STDOUT_COLOR: LazyLock<bool> = LazyLock::new(|| resolve_color(*STDOUT_IS_TERMINAL));

pub struct Ui;

impl Ui {
    pub fn set_quiet(quiet: bool) {
        QUIET.store(quiet, Ordering::Relaxed);
    }

    pub(crate) fn is_quiet() -> bool {
        QUIET.load(Ordering::Relaxed)
    }

    pub fn set_verbose(verbose: bool) {
        VERBOSE.store(verbose, Ordering::Relaxed);
    }

    pub(crate) fn is_verbose() -> bool {
        VERBOSE.load(Ordering::Relaxed)
    }

    /// Returns whether ANSI colors are enabled for stderr diagnostics and spinners.
    pub fn use_color() -> bool {
        *USE_STDERR_COLOR
    }

    pub(crate) fn use_stdout_color() -> bool {
        *USE_STDOUT_COLOR
    }

    pub(crate) fn spinner(message: impl Into<Cow<'static, str>>) -> ProgressBar {
        let msg = message.into();
        if Self::is_quiet() || !*STDERR_IS_TERMINAL || msg.trim().is_empty() {
            return ProgressBar::hidden();
        }
        let pb = ProgressBar::new_spinner();
        pb.set_message(msg);
        pb
    }

    pub(crate) fn finish_spinner(pb: &ProgressBar, message: &str) {
        let elapsed = pb.elapsed();
        pb.finish_and_clear();
        if !message.trim().is_empty() {
            Self::success_with_elapsed(message, elapsed);
        }
    }

    pub(crate) fn success_with_elapsed(message: &str, elapsed: Duration) {
        if Self::is_quiet() || message.trim().is_empty() {
            return;
        }
        let safe_msg = sanitize_terminal_text_cow(message);
        if elapsed >= Duration::from_millis(50) {
            let elapsed_str = format_duration_short(elapsed);
            if Self::use_color() {
                eprintln!("\x1b[1;32m✔\x1b[0m {safe_msg} \x1b[2m({elapsed_str})\x1b[0m");
            } else {
                eprintln!("✔ {safe_msg} ({elapsed_str})");
            }
        } else {
            Self::success(message);
        }
    }

    pub(crate) async fn with_spinner<T, E, F>(
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

    pub(crate) fn header(title: &str) {
        if Self::is_quiet() {
            return;
        }
        let safe_title = sanitize_title(title);
        if Self::use_color() {
            eprintln!("\n\x1b[1;36m=== {safe_title} ===\x1b[0m\n");
        } else {
            eprintln!("\n=== {safe_title} ===\n");
        }
    }

    pub(crate) fn info(message: &str) {
        if !Self::is_quiet() {
            Self::print_styled("ℹ", "1;34", message);
        }
    }

    pub(crate) fn success(message: &str) {
        if !Self::is_quiet() && !message.trim().is_empty() {
            Self::print_styled("✔", "1;32", message);
        }
    }

    pub(crate) fn warn(message: &str) {
        if !Self::is_quiet() {
            Self::print_styled("⚠", "1;33", message);
        }
    }

    pub fn error(message: &str) {
        Self::print_styled("✖", "1;31", message);
    }

    pub(crate) fn verbose(message: &str) {
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
pub(super) fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}'
    )
}

/// Strips ANSI/C1/BiDi/control sequences (CWE-150/CWE-451), borrowing clean inputs zero-copy.
pub(crate) fn sanitize_terminal_text_cow<'a>(input: &'a str) -> Cow<'a, str> {
    let has_suspicious = input
        .chars()
        .any(|c| (c.is_control() && c != '\n' && c != '\t') || is_bidi_control(c));
    if !has_suspicious {
        return Cow::Borrowed(input);
    }

    let mut clean = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    let skip_csi = |iter: &mut std::iter::Peekable<std::str::Chars>| {
        // ECMA-48 CSI: optional parameter/intermediate bytes (0x20..=0x3F), capped, then final (@..=~).
        let mut count = 0usize;
        while let Some(&c) = iter.peek() {
            if ('\u{0020}'..='\u{003F}').contains(&c) && count < 256 {
                iter.next();
                count += 1;
            } else if ('@'..='~').contains(&c) {
                iter.next();
                break;
            } else {
                break;
            }
        }
    };

    let skip_string_control = |iter: &mut std::iter::Peekable<std::str::Chars>| {
        let mut prev = '\0';
        for _ in 0..4096 {
            let Some(&next) = iter.peek() else {
                break;
            };
            if next == '\n' {
                break;
            }
            iter.next();
            if next == '\x07' || next == '\u{009C}' || (prev == '\x1b' && next == '\\') {
                break;
            }
            prev = next;
        }
    };

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek().copied() {
                Some('[') => {
                    chars.next();
                    skip_csi(&mut chars);
                }
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    chars.next();
                    skip_string_control(&mut chars);
                }
                // Guard against "\x1b\x1b]52..." smuggling: never swallow an adjacent ESC or C1 opener.
                Some(next) if next == '\x1b' || ('\u{0080}'..='\u{009F}').contains(&next) => {}
                Some(next) if (' '..='/').contains(&next) => {
                    for _ in 0..16 {
                        if !chars.peek().is_some_and(|c| (' '..='/').contains(c)) {
                            break;
                        }
                        chars.next();
                    }
                    if chars.peek().is_some_and(|c| ('0'..='~').contains(c)) {
                        chars.next();
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }

        if ch == '\u{009B}' {
            skip_csi(&mut chars);
            continue;
        }

        if matches!(
            ch,
            '\u{0090}' | '\u{0098}' | '\u{009D}' | '\u{009E}' | '\u{009F}'
        ) {
            skip_string_control(&mut chars);
            continue;
        }

        if (ch.is_control() && ch != '\n' && ch != '\t') || is_bidi_control(ch) {
            continue;
        }

        clean.push(ch);
    }

    Cow::Owned(clean)
}

/// Sanitizes a string for safe terminal display (stripping ANSI/C1/BiDi/control sequences).
pub(crate) fn sanitize_terminal_text(input: &str) -> String {
    sanitize_terminal_text_cow(input).into_owned()
}

/// Sanitizes a title string for terminal presentation by removing escape sequences and collapsing whitespace.
pub(crate) fn sanitize_title(title: &str) -> String {
    sanitize_terminal_text_cow(title)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Formats a duration into a compact string (`"45ms"`, `"0.4s"`, `"2.0s"`, `"1m 5s"`).
pub(crate) fn format_duration_short(d: Duration) -> String {
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

pub(crate) use super::markdown::render_markdown;

/// Returns the usable terminal line width when stdout is a TTY, leaving 1 column
/// before the right margin to prevent terminal auto-wrap quirks.
pub(crate) fn terminal_width() -> Option<usize> {
    if !*STDOUT_IS_TERMINAL {
        return None;
    }
    crossterm::terminal::size()
        .ok()
        .map(|(cols, _)| (cols as usize).saturating_sub(1).max(20))
}

/// Parses a leading 7-bit CSI sequence (`\x1b[...<final>`) returning `(byte_len, is_sgr)`.
pub(super) fn parse_csi(s: &str) -> Option<(usize, bool)> {
    let bytes = s.as_bytes();
    if !bytes.starts_with(b"\x1b[") {
        return None;
    }
    let mut i = 2;
    while i < bytes.len() && (0x20..=0x3F).contains(&bytes[i]) {
        i += 1;
    }
    if i < bytes.len() && (b'@'..=b'~').contains(&bytes[i]) {
        Some((i + 1, bytes[i] == b'm'))
    } else {
        None
    }
}

fn measure_visible_and_sgr<'a>(word: &'a str, active_sgr: &mut Option<&'a str>) -> usize {
    let mut i = 0;
    let mut width = 0;
    while i < word.len() {
        if let Some((len, is_sgr)) = parse_csi(&word[i..]) {
            if is_sgr {
                let seq = &word[i..i + len];
                *active_sgr = (seq != "\x1b[0m" && seq != "\x1b[m").then_some(seq);
            }
            i += len;
            continue;
        }
        let ch = word[i..].chars().next().unwrap_or('\0');
        width += char_display_width(ch);
        i += ch.len_utf8();
    }
    width
}

/// Wraps text to `max_width` visible columns on whitespace boundaries, preserving ANSI SGR styling across line breaks.
pub(crate) fn wrap_terminal_text(text: &str, max_width: usize) -> String {
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
pub(crate) fn wrap_for_terminal<'a>(text: Cow<'a, str>, width: Option<usize>) -> Cow<'a, str> {
    match width {
        Some(w) => Cow::Owned(wrap_terminal_text(text.as_ref(), w)),
        None => text,
    }
}

/// Incremental line-buffered markdown stream renderer for real-time terminal output.
pub(crate) struct StreamMarkdownRenderer {
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
        Self::with_options(Ui::use_stdout_color())
    }

    fn with_options(use_color: bool) -> Self {
        Self {
            line_buf: String::with_capacity(256),
            fence: None,
            use_color,
            wrap_width: terminal_width(),
        }
    }

    const MAX_STREAM_LINE_BYTES: usize = 16 * 1024;

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
            print!("{}", self.render_complete_line(line_trimmed));
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
            format!("{}\n", wrap_for_terminal(Cow::Borrowed(safe), width))
        } else if is_fence_line {
            format!("\x1b[32m{safe}\x1b[0m\n")
        } else if in_fence {
            format!("\x1b[32m  {safe}\x1b[0m\n")
        } else if safe.is_empty() {
            "\n".to_string()
        } else {
            let rendered = super::markdown::render_markdown_presanitized(safe);
            let wrapped = wrap_for_terminal(Cow::Owned(rendered), self.wrap_width);
            if wrapped.ends_with('\n') {
                wrapped.into_owned()
            } else {
                format!("{wrapped}\n")
            }
        }
    }

    fn render_partial_line(&self, raw: &str) -> String {
        let sanitized = sanitize_terminal_text_cow(raw);
        let safe = sanitized.as_ref();
        let rendered = if !self.use_color {
            Cow::Borrowed(safe)
        } else if self.fence.is_some() {
            Cow::Owned(format!("\x1b[32m  {safe}\x1b[0m"))
        } else {
            Cow::Owned(super::markdown::render_markdown_presanitized(safe))
        };
        let width = self.wrap_width.filter(|_| self.fence.is_none());
        wrap_for_terminal(rendered, width).into_owned()
    }

    fn flush_partial_line(&mut self) {
        if !self.line_buf.is_empty() {
            print!("{}", self.render_partial_line(&self.line_buf));
            self.line_buf.clear();
            let _ = io::stdout().flush();
        }
    }

    pub fn flush(&mut self) {
        self.drain_complete_lines();
        self.flush_partial_line();
    }
}

fn build_summary_output(
    title: &str,
    content: &str,
    use_color: bool,
    plain: bool,
    wrap_width: Option<usize>,
    view_files: Option<(&Path, &Path)>,
    hint: Option<&str>,
) -> String {
    let sanitized_title = sanitize_title(title);
    let sanitized_content = sanitize_terminal_text_cow(content);
    let color = !plain && use_color;

    let mut out = String::new();
    if !plain {
        let _ = write!(
            out,
            "\n{}=== Summary: {sanitized_title} ==={}\n\n",
            if color { "\x1b[1;36m" } else { "" },
            if color { "\x1b[0m" } else { "" }
        );
    }

    let body = if color {
        wrap_for_terminal(
            Cow::Owned(super::markdown::render_markdown_presanitized(
                &sanitized_content,
            )),
            wrap_width,
        )
    } else {
        wrap_for_terminal(sanitized_content, wrap_width)
    };
    out.push_str(&body);
    out.push('\n');

    if let Some((html_path, md_path)) = view_files {
        let safe_html = sanitize_terminal_text(&html_path.display().to_string());
        let safe_md = sanitize_terminal_text(&md_path.display().to_string());
        if color {
            let _ = write!(
                out,
                "\n\x1b[32m✔\x1b[0m \x1b[2mStyled HTML view:\x1b[0m file://{safe_html}\n\x1b[32m✔\x1b[0m \x1b[2mMarkdown file:   \x1b[0m file://{safe_md}\n"
            );
        } else {
            let _ = write!(
                out,
                "\n✔ Styled HTML view: file://{safe_html}\n✔ Markdown file:    file://{safe_md}\n"
            );
        }
    }

    if let Some(h) = hint {
        let wrapped_hint = wrap_for_terminal(sanitize_terminal_text_cow(h), wrap_width);
        let _ = write!(
            out,
            "\n{}{wrapped_hint}{}\n",
            if color { "\x1b[2m" } else { "" },
            if color { "\x1b[0m" } else { "" }
        );
    }

    out
}

/// Prints a unified terminal summary document (including optional view file links and context hints),
/// launching a pager when the combined document exceeds terminal height.
pub(crate) fn display_summary_full(
    title: &str,
    content: &str,
    paging_mode: PagingMode,
    plain: bool,
    view_files: Option<(&Path, &Path)>,
    hint: Option<&str>,
) -> io::Result<()> {
    let wrap_width = terminal_width();
    if !*STDOUT_IS_TERMINAL {
        let body = wrap_for_terminal(sanitize_terminal_text_cow(content), wrap_width);
        println!("{body}\n");
        return Ok(());
    }

    let full_output = build_summary_output(
        title,
        content,
        Ui::use_stdout_color(),
        plain,
        wrap_width,
        view_files,
        hint,
    );

    let (_, term_height) = crossterm::terminal::size().unwrap_or((80, 24));
    let line_count = full_output.lines().count();
    let should_page = !plain
        && match paging_mode {
            PagingMode::Always => true,
            PagingMode::Never => false,
            PagingMode::Auto => line_count > term_height.saturating_sub(4) as usize,
        };

    if should_page {
        if let Err(err) = run_pager(&full_output) {
            Ui::verbose(&format!("Pager error, falling back to direct print: {err}"));
            print!("{full_output}");
            io::stdout().flush()?;
        }
    } else {
        print!("{full_output}");
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
    let is_less = Path::new(bin).file_name().and_then(|n| n.to_str()) == Some("less");
    if is_less && args.is_empty() {
        args = vec!["-R", "-F", "-X"];
    }
    (bin, args)
}

/// Spawns `cmd` with `args`, writes `input` to stdin, and always reaps the child process.
pub(super) fn pipe_to_command(
    cmd: &str,
    args: &[&str],
    extra_env: Option<(&str, &str)>,
    input: &str,
    ignore_broken_pipe: bool,
    quiet_stdio: bool,
    check_exit_status: bool,
) -> io::Result<()> {
    let mut command = Command::new(cmd);
    command.args(args).stdin(Stdio::piped());
    if let Some((k, v)) = extra_env {
        command.env(k, v);
    }
    if quiet_stdio {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }
    let mut child = command.spawn()?;
    let mut write_err = None;
    if let Some(mut stdin) = child.stdin.take()
        && let Err(err) = stdin.write_all(input.as_bytes())
        && (!ignore_broken_pipe || err.kind() != io::ErrorKind::BrokenPipe)
    {
        write_err = Some(err);
    }
    let status = child.wait();
    if let Some(err) = write_err {
        return Err(err);
    }
    let status = status?;
    if check_exit_status && !status.success() {
        return Err(io::Error::other(format!("{cmd} exited with error")));
    }
    Ok(())
}

fn run_pager(content: &str) -> io::Result<()> {
    let pager_cmd = env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let (safe_bin, args) = parse_pager_command(&pager_cmd);
    pipe_to_command(
        safe_bin,
        &args,
        Some(("LESSSECURE", "1")),
        content,
        true,
        false,
        false,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration_short() {
        assert_eq!(format_duration_short(Duration::from_millis(45)), "45ms");
        assert_eq!(format_duration_short(Duration::from_millis(1400)), "1.4s");
        assert_eq!(
            format_duration_short(Duration::from_millis(59_960)),
            "1m 0s"
        );
        assert_eq!(format_duration_short(Duration::from_secs(120)), "2m 0s");
    }

    #[test]
    fn test_sanitize_terminal_text_security_and_zero_alloc() {
        assert!(matches!(
            sanitize_terminal_text_cow("Clean string!"),
            Cow::Borrowed("Clean string!")
        ));
        let dirty = "\x1b[31;1mRed\x1b[0m \x1b]0;Evil\x07 \u{009B}31mC1\u{009B}0m \u{009D}osc\u{009C} \x1bPdcs\x1b\\ \u{202E}bidi\u{202C}\x00\x07";
        assert_eq!(sanitize_terminal_text(dirty), "Red  C1   bidi");

        // Guard nested ESC smuggling ("\x1b\x1b]52..."), bounded CSI, and unterminated OSC stopping at '\n'.
        assert_eq!(sanitize_terminal_text("\x1b\x1b]52;c;evil\x07ok"), "ok");
        assert_eq!(sanitize_terminal_text("a\x1b[31\nkept"), "a\nkept");
        assert_eq!(
            sanitize_terminal_text("head\x1b]0;unterminated\nvisible"),
            "head\nvisible"
        );
        assert_eq!(
            sanitize_title("\x1b[1mTitle\x1b[0m\nNext\r\n\u{202E}Line"),
            "Title Next Line"
        );
    }

    #[test]
    fn test_markdown_rendering_nesting_and_safety() {
        let md = "# H1 `code` tail\n\n**bold *it* end** ***bi*** ~~del~~";
        let rendered = render_markdown(md);
        assert!(rendered.contains("\x1b[1;36;4mH1 \x1b[32mcode\x1b[0m\x1b[1;36;4m tail\x1b[0m"));
        assert!(rendered.contains("\x1b[1;37mbold \x1b[35mit\x1b[0m\x1b[1;37m end\x1b[0m"));
        assert!(rendered.contains("\x1b[1;3;35mbi\x1b[0m"));

        assert!(!super::super::markdown::is_safe_markdown_url(
            "\\\\server\\share"
        ));
        assert!(!super::super::markdown::is_safe_markdown_url(
            "javascript:alert(1)"
        ));
        assert!(super::super::markdown::is_safe_markdown_url(
            "https://example.com"
        ));

        // Deeply nested quotes must terminate cleanly within MAX_DEPTH instead of overflowing stack.
        let deep_quotes = format!("{}leaf", "> ".repeat(64));
        assert!(super::super::markdown::markdown_to_html(&deep_quotes).contains("leaf"));
    }

    #[test]
    fn test_stream_renderer_sanitizes_fences_and_bounds() {
        let mut plain = StreamMarkdownRenderer::with_options(false);
        assert_eq!(
            plain.render_complete_line("\x1b]2;pwned\x07hello\x1b[31m!\r"),
            "hello!\n"
        );
        plain.feed(&"🦀".repeat(10_000));
        assert!(plain.line_buf.len() <= StreamMarkdownRenderer::MAX_STREAM_LINE_BYTES);
        plain.flush();
        assert!(plain.line_buf.is_empty());

        let mut colored = StreamMarkdownRenderer::with_options(true);
        let fence = colored.render_complete_line("```\x1b]0;title\x07");
        assert!(!fence.contains("\x1b]0;") && !fence.contains('\x07'));
        let partial_code = colored.render_partial_line("let x = *raw*;");
        assert!(partial_code.starts_with("\x1b[32m") && partial_code.contains("*raw*"));
    }

    #[test]
    fn test_summary_output_nocolor_and_path_sanitization() {
        let out = build_summary_output(
            "Title",
            "Body",
            false,
            false,
            Some(80),
            Some((Path::new("/tmp/a\x1b]0;x\x07.html"), Path::new("/tmp/b.md"))),
            Some("Hint text"),
        );
        assert!(out.contains("=== Summary: Title ==="));
        assert!(out.contains("file:///tmp/a.html"));
        assert!(out.contains("Hint text"));
        assert!(!out.contains('\x1b'));
    }

    #[test]
    fn test_progress_bar_drop_spinner_fit_and_display_width() {
        assert_eq!(char_display_width('\u{0301}'), 0); // combining acute accent
        assert_eq!(char_display_width('\u{FE0F}'), 0); // VS16
        assert_eq!(char_display_width('\u{200D}'), 0); // ZWJ
        assert_eq!(char_display_width('𠮷'), 2); // CJK Ext B

        let pb = ProgressBar::hidden();
        pb.set_message("Load \x1b]2;evil\x07\x1b[31mfile\r\nname");
        assert_eq!(pb.shared.state.lock().unwrap().message, "Load file name");
        let shared = Arc::clone(&pb.shared);
        assert!(!pb.is_finished());
        pb.reset();
        assert!(!pb.is_finished());
        drop(pb);
        assert!(shared.state.lock().unwrap().finished);

        let msg = "💭 Thinking: Draft paragraph 2 - technical depth and complexity overview...";
        assert!(matches!(fit_spinner_message(msg, 120), Cow::Borrowed(_)));
        assert_eq!(
            fit_spinner_message(msg, 55),
            "💭 Thinking: Draft paragraph 2 - technical depth..."
        );
        assert_eq!(fit_spinner_message(msg, 6), "");
    }

    #[test]
    fn test_wrap_terminal_text_and_pager_parse() {
        let styled = "\x1b[1;37mCEO Resignation and Collapse\x1b[0m follows.";
        assert_eq!(
            wrap_terminal_text(styled, 18),
            "\x1b[1;37mCEO Resignation\x1b[0m\n\x1b[1;37mand Collapse\x1b[0m\nfollows."
        );
        assert!(matches!(
            wrap_for_terminal(Cow::Borrowed("Unchanged"), None),
            Cow::Borrowed("Unchanged")
        ));
        assert_eq!(
            parse_pager_command("--evil foo"),
            ("less", vec!["-R", "-F", "-X"])
        );
        assert_eq!(
            parse_pager_command("/usr/bin/less"),
            ("/usr/bin/less", vec!["-R", "-F", "-X"])
        );
        assert_eq!(parse_pager_command("most -s"), ("most", vec!["-s"]));
    }
}
