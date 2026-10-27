use indicatif::{ProgressBar, ProgressStyle};
use std::future::Future;
use std::time::Duration;

pub struct Ui;

impl Ui {
    /// Creates a styled spinner with a status message.
    pub fn spinner(message: &'static str) -> ProgressBar {
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

    /// Clears the spinner from stderr and prints a clean, newline-terminated completion line.
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

    /// Prints a section header.
    pub fn header(title: &str) {
        println!("\n\x1b[1;36m=== {} ===\x1b[0m\n", title);
    }

    /// Prints an informational message.
    pub fn info(message: &str) {
        println!("\x1b[1;34mℹ\x1b[0m {}", message);
    }

    /// Prints a success message.
    pub fn success(message: &str) {
        println!("\x1b[1;32m✔\x1b[0m {}", message);
    }

    /// Prints a warning message.
    pub fn warn(message: &str) {
        println!("\x1b[1;33m⚠\x1b[0m {}", message);
    }

    /// Prints an error message.
    pub fn error(message: &str) {
        eprintln!("\x1b[1;31m✖\x1b[0m {}", message);
    }
}
