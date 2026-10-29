use indicatif::{ProgressBar, ProgressStyle};
use std::env;
use std::future::Future;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::LazyLock;
use std::time::Duration;

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

static SPINNER_STYLE: LazyLock<ProgressStyle> = LazyLock::new(|| {
    ProgressStyle::default_spinner()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏")
        .template("{spinner:.green} {msg}")
        .expect("Valid spinner template")
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
    /// or a hidden progress bar if quiet mode is enabled or stderr is not a terminal.
    pub fn spinner(message: impl Into<std::borrow::Cow<'static, str>>) -> ProgressBar {
        if Self::is_quiet() || !*STDERR_IS_TERMINAL {
            return ProgressBar::hidden();
        }
        let pb = ProgressBar::new_spinner();
        pb.set_style(SPINNER_STYLE.clone());
        pb.set_message(message);
        pb.enable_steady_tick(Duration::from_millis(80));
        pb
    }

    /// Clears the spinner from stderr and prints a clean completion line if not in quiet mode.
    pub fn finish_spinner(pb: &ProgressBar, message: &str) {
        pb.finish_and_clear();
        Self::success(message);
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
        if !Self::is_quiet() {
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
        let safe_msg = sanitize_terminal_text(message);
        if Self::use_color() {
            eprintln!("\x1b[{color_code}m{symbol}\x1b[0m {safe_msg}");
        } else {
            eprintln!("{symbol} {safe_msg}");
        }
    }
}

/// Sanitizes a string for safe terminal display by stripping ANSI escape sequences
/// and unprintable ASCII control characters (CWE-150 / Terminal Escape Injection).
pub fn sanitize_terminal_text(input: &str) -> String {
    let mut clean = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            match chars.peek() {
                Some('[') => {
                    chars.next(); // consume '['
                    // CSI sequence ends at 0x40..=0x7E (@..=~)
                    for c in chars.by_ref() {
                        if ('@'..='~').contains(&c) {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next(); // consume ']'
                    // OSC sequence ends at BEL (\x07) or ST (\x1b\)
                    let mut prev = '\0';
                    for c in chars.by_ref() {
                        if c == '\x07' || (prev == '\x1b' && c == '\\') {
                            break;
                        }
                        prev = c;
                    }
                }
                Some(_) => {
                    chars.next();
                }
                None => {}
            }
            continue;
        }

        // Filter out control characters except newline and tab
        if ch.is_ascii_control() && ch != '\n' && ch != '\t' {
            continue;
        }

        clean.push(ch);
    }

    clean
}

/// Sanitizes a title string for terminal presentation by removing escape sequences and collapsing whitespace.
pub fn sanitize_title(title: &str) -> String {
    let sanitized = sanitize_terminal_text(title);
    let mut out = String::with_capacity(sanitized.len());
    for word in sanitized.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_terminal_text_strips_ansi_and_osc() {
        let csi_injected = "Hello \x1b[31;1mRed Alert\x1b[0m World";
        assert_eq!(sanitize_terminal_text(csi_injected), "Hello Red Alert World");

        let osc_injected = "Normal \x1b]0;Evil Title\x07Text";
        assert_eq!(sanitize_terminal_text(osc_injected), "Normal Text");

        let control_chars = "Line\x00with\x08bell\x07and\x1bsome chars";
        assert_eq!(sanitize_terminal_text(control_chars), "Linewithbellandome chars");
    }

    #[test]
    fn test_sanitize_title_collapses_newlines() {
        let raw_title = "\x1b[1mImportant\x1b[0m\nDocument\r\nTitle";
        assert_eq!(sanitize_title(raw_title), "Important Document Title");
    }
}
