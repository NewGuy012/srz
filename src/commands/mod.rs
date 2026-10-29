pub mod ask;
pub mod diagnostics;
pub mod library;
pub mod summarize;

use anyhow::{Context, Result};
use crate::cache::Cache;
use crate::cli::{CacheCommand, Cli, ConfigSubcommand};
use crate::config::Config;
use crate::db::Database;
use crate::ui::Ui;
use std::path::Path;

pub use ask::handle_ask;
pub use diagnostics::{handle_doctor, handle_models};
pub use library::{handle_find, handle_history, handle_index, handle_stats, update_index_dashboard};
pub use summarize::{
    fetch_or_cache_page, handle_book, handle_file, handle_news, handle_stdin, handle_web,
    SummaryOutcome,
};

/// Safely opens the DuckDB database, issuing a UI warning on failure.
pub fn open_db_or_warn() -> Option<Database> {
    match Database::open_default() {
        Ok(d) => Some(d),
        Err(err) => {
            Ui::warn(&format!("Could not open DuckDB database: {}", err));
            None
        }
    }
}

/// Executes a closure with an active database connection if available.
pub fn with_db(f: impl FnOnce(&Database) -> Result<()>) -> Result<()> {
    if let Some(db) = open_db_or_warn() {
        f(&db)?;
    } else {
        Ui::error("DuckDB database unavailable.");
    }
    Ok(())
}

/// Handles cache maintenance subcommands (`dir`, `size`, `clean`).
pub fn handle_cache(cmd: CacheCommand, cache: &Cache) -> Result<()> {
    match cmd {
        CacheCommand::Dir => {
            println!("{}", cache.dir().display());
        }
        CacheCommand::Size => {
            let stats = cache.stats();
            println!(
                "{} ({} total files: {} pages, {} searches, {} summaries, {} views)",
                Cache::format_bytes(stats.total_bytes),
                stats.total_files,
                stats.pages,
                stats.searches,
                stats.summaries,
                stats.views
            );
        }
        CacheCommand::Clean => {
            let stats = cache.clean().context("Failed to clean cache directory")?;
            let mut db_reset = false;
            if let Some(database) = open_db_or_warn() {
                database.reset().context("Failed to reset DuckDB database")?;
                db_reset = true;
            }
            if stats.total_files == 0 && !db_reset {
                Ui::info("Cache is already empty.");
            } else if stats.total_files == 0 && db_reset {
                Ui::success("Cache was already empty. DuckDB database tables reset.");
            } else {
                Ui::success(&format!(
                    "Removed {} cached file(s) ({}) and reset DuckDB database tables.",
                    stats.total_files,
                    Cache::format_bytes(stats.total_bytes)
                ));
            }
        }
    }
    Ok(())
}

/// Handles configuration subcommands (`path`, `show`, `init`).
pub fn handle_config(
    subcmd: ConfigSubcommand,
    config: &Config,
    config_path: Option<&Path>,
    args: &Cli,
) -> Result<()> {
    match subcmd {
        ConfigSubcommand::Path => {
            let default_path = Config::default_config_path();
            if let Some(p) = config_path {
                println!("{} (active, exists: {})", p.display(), p.exists());
            } else {
                println!("{} (not found, using defaults)", default_path.display());
            }
        }
        ConfigSubcommand::Show => {
            println!("# Active srz Configuration");
            println!("base_url = {:?}", args.base_url);
            println!("model = {:?}", args.model);
            println!("style = {:?}", args.style.as_str());
            println!("max_tokens = {}", args.max_tokens);
            if let Some(profile_name) = &args.profile {
                println!("active_profile = {:?}", profile_name);
            }
            if !config.profiles.is_empty() {
                println!("\n# Configured profiles in config.toml:");
                for name in config.profiles.keys() {
                    println!("- {}", name);
                }
            }
        }
        ConfigSubcommand::Init => {
            let default_path = Config::default_config_path();
            if default_path.exists() {
                Ui::info(&format!(
                    "Config file already exists at {}",
                    default_path.display()
                ));
            } else {
                if let Some(parent) = default_path.parent() {
                    crate::fs_util::create_private_dir_all(parent)?;
                }
                crate::fs_util::write_private_file(&default_path, Config::default_template())?;
                Ui::success(&format!(
                    "Initialized config file template at {}",
                    default_path.display()
                ));
            }
        }
    }
    Ok(())
}
