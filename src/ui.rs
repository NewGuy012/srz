use indicatif::{ProgressBar, ProgressStyle};
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
