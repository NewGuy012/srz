#![forbid(unsafe_code)]

pub mod commands;
pub mod config;
pub mod engine;
pub mod sources;
pub mod storage;
pub mod system;
pub mod ui;

// Internal aliases for crate modules
pub(crate) use config as cli;
pub(crate) use engine::chunker;
pub(crate) use engine::llm;
pub(crate) use sources::scraper;
pub(crate) use sources::search;
pub(crate) use storage::cache;
pub(crate) use storage::db;
pub(crate) use system::fs_util;
pub(crate) use ui::render;
