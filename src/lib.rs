#![forbid(unsafe_code)]

pub mod commands;
pub mod config;
pub mod engine;
pub mod sources;
pub mod storage;
pub mod system;
pub mod ui;

// Backwards-compatible aliases for crate root
pub use config::cli;
pub use engine::chunker;
pub use engine::llm;
pub use sources::document;
pub use sources::news;
pub use sources::pdf;
pub use sources::scraper;
pub use sources::search;
pub use sources::youtube;
pub use storage::cache;
pub use storage::db;
pub use system::fs_util;
pub use system::upgrade;
pub use ui::printer;
pub use ui::render;
