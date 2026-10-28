use indicatif::{ProgressBar, ProgressStyle};
use std::env;
use std::future::Future;
use std::io::IsTerminal;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static QUIET: AtomicBool = AtomicBool::new(false);
static VERBOSE: AtomicBool = AtomicBool::new(false);

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
    /// standard `NO_COLOR`, `CLICOLOR`, `CLICOLOR_FORCE`, and terminal detection.
    pub fn use_color() -> bool {
        if env::var_os("CLICOLOR_FORCE").map(|v| v != "0").unwrap_or(false) {
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
        std::io::stderr().is_terminal()
    }

    /// Creates a styled spinner on stderr with a status message,
    /// or a hidden progress bar if quiet mode is enabled or stderr is not a terminal.
    pub fn spinner(message: impl Into<std::borrow::Cow<'static, str>>) -> ProgressBar {
        if Self::is_quiet() || !std::io::stderr().is_terminal() {
            return ProgressBar::hidden();
        }
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏ ")
                .template("{spinner:.green} {msg}")
                .expect("Valid spinner template"),
        );
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
        if Self::use_color() {
            eprintln!("\n\x1b[1;36m=== {} ===\x1b[0m\n", title);
        } else {
            eprintln!("\n=== {} ===\n", title);
        }
    }

    /// Prints an informational message to stderr.
    pub fn info(message: &str) {
        if Self::is_quiet() {
            return;
        }
        if Self::use_color() {
            eprintln!("\x1b[1;34mℹ\x1b[0m {}", message);
        } else {
            eprintln!("ℹ {}", message);
        }
    }

    /// Prints a success message to stderr.
    pub fn success(message: &str) {
        if Self::is_quiet() {
            return;
        }
        if Self::use_color() {
            eprintln!("\x1b[1;32m✔\x1b[0m {}", message);
        } else {
            eprintln!("✔ {}", message);
        }
    }

    /// Prints a warning message to stderr.
    pub fn warn(message: &str) {
        if Self::is_quiet() {
            return;
        }
        if Self::use_color() {
            eprintln!("\x1b[1;33m⚠\x1b[0m {}", message);
        } else {
            eprintln!("⚠ {}", message);
        }
    }

    /// Prints an error message to stderr.
    pub fn error(message: &str) {
        if Self::use_color() {
            eprintln!("\x1b[1;31m✖\x1b[0m {}", message);
        } else {
            eprintln!("✖ {}", message);
        }
    }

    /// Prints verbose diagnostic message if verbose mode is enabled.
    pub fn verbose(message: &str) {
        if !Self::is_verbose() {
            return;
        }
        if Self::use_color() {
            eprintln!("\x1b[1;35m[debug]\x1b[0m {}", message);
        } else {
            eprintln!("[debug] {}", message);
        }
    }
}
