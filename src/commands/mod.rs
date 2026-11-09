pub mod ask;
pub mod diagnostics;
pub mod library;
pub mod summarize;

use crate::cache::Cache;
use crate::cli::{CacheCommand, Cli, ConfigSubcommand};
use crate::config::Config;
use crate::db::Database;
use crate::ui::Ui;
use anyhow::{Context, Result};
use std::path::Path;

pub use ask::{AskArgs, handle_ask, handle_ask_management};
pub use diagnostics::{handle_doctor, handle_models, handle_recommend};
pub use library::{
    handle_export, handle_find, handle_history, handle_index, handle_query, handle_stats,
    update_index_dashboard,
};
pub use summarize::{
    PreparedBookQuery, SummaryOutcome, fetch_or_cache_page, fetch_or_cache_page_with,
    fetch_url_content, handle_file, handle_news, handle_stdin, handle_web, prepare_book_query,
    present_summary_outcome, summarize_fetched_web_page, summarize_prepared_book,
    target_type_for_url,
};

/// Initializes an `LlmClient` from CLI arguments, printing a UI error and exiting on failure.
pub fn init_llm_or_exit(args: &Cli) -> crate::llm::LlmClient {
    match crate::llm::LlmClient::from_cli(args) {
        Ok(c) => c,
        Err(err) => {
            Ui::error(&format!("Failed to initialize LLM client: {}", err));
            std::process::exit(1);
        }
    }
}

/// Initializes an `LlmClient` and discovers server capabilities (with DuckDB caching & model-change invalidation).
pub async fn init_llm_with_capabilities_or_exit(
    args: &Cli,
    cache: Option<&crate::cache::Cache>,
) -> crate::llm::LlmClient {
    let llm = init_llm_or_exit(args);
    llm.init_with_capabilities(cache).await
}

/// Safely opens the DuckDB database, issuing a UI warning on failure.
pub fn open_db_or_warn() -> Option<Database> {
    match Database::open_default() {
        Ok(d) => Some(d),
        Err(err) => {
            Ui::warn(&format!("Could not open DuckDB database: {:#}", err));
            None
        }
    }
}

/// Executes a closure with an active database connection if available.
pub fn with_db<T: Default>(f: impl FnOnce(&Database) -> Result<T>) -> Result<T> {
    if let Some(db) = open_db_or_warn() {
        f(&db)
    } else {
        Ui::error("DuckDB database unavailable.");
        Ok(T::default())
    }
}

/// Handles cache maintenance subcommands (`dir`, `size`, `clean`, `prune`).
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
        CacheCommand::Clean { reset_db } => {
            let stats = cache.clean().context("Failed to clean cache directory")?;
            let mut db_reset = false;
            if reset_db && let Some(database) = open_db_or_warn() {
                database
                    .reset()
                    .context("Failed to reset DuckDB database")?;
                db_reset = true;
            }
            if stats.total_files == 0 && !db_reset {
                Ui::info("Cache is already empty.");
            } else if stats.total_files == 0 && db_reset {
                Ui::success("Cache was already empty. DuckDB database tables reset.");
            } else if db_reset {
                Ui::success(&format!(
                    "Removed {} cached file(s) ({}) and reset DuckDB database tables.",
                    stats.total_files,
                    Cache::format_bytes(stats.total_bytes)
                ));
            } else {
                Ui::success(&format!(
                    "Removed {} cached file(s) ({}). DuckDB database preserved.",
                    stats.total_files,
                    Cache::format_bytes(stats.total_bytes)
                ));
            }
        }
        CacheCommand::Prune {
            older_than,
            max_size,
        } => {
            let parsed_older_than = match older_than.as_deref() {
                Some(s) => Some(crate::cache::parse_duration(s).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };
            let parsed_max_size = match max_size.as_deref() {
                Some(s) => Some(crate::cache::parse_bytes(s).map_err(|e| anyhow::anyhow!(e))?),
                None => None,
            };

            let options = crate::cache::PruneOptions {
                older_than: parsed_older_than,
                max_bytes: parsed_max_size,
                namespace: None,
            };

            let prune_stats = cache.prune(options).context("Failed to prune cache")?;
            if prune_stats.files_removed == 0 {
                Ui::info("No expired or excess cache entries to prune.");
            } else {
                Ui::success(&format!(
                    "Pruned {} cached file(s), reclaimed {}.",
                    prune_stats.files_removed,
                    Cache::format_bytes(prune_stats.bytes_reclaimed)
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
                    println!("- {}", crate::ui::sanitize_title(name));
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
                Ui::info(
                    "Tip: Run `srz recommend` to view the optimal model and context for your machine.",
                );
            } else {
                if let Some(parent) = default_path.parent() {
                    crate::fs_util::create_private_dir_all(parent)?;
                }
                let hw = crate::system::hardware::HardwareProfile::detect();
                let template = Config::template_for_hardware(&hw);
                crate::fs_util::write_private_file(&default_path, template.as_bytes())?;
                Ui::success(&format!(
                    "Initialized config file tailored for your hardware ({:.1} GB RAM, {}) at {}",
                    hw.total_memory_gb,
                    hw.recommended_context_display,
                    default_path.display()
                ));
                Ui::info(&format!(
                    "Llama GUI: select {} ({}) | CLI: {}",
                    hw.recommended_model,
                    hw.recommended_context_display,
                    hw.recommended_launch_command
                ));
            }
        }
    }
    Ok(())
}

/// Dispatches execution of the resolved CLI action.
pub async fn dispatch_cli(
    args: Cli,
    config: Config,
    config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    let action = match args.resolve_action() {
        Ok(action) => action,
        Err(msg) => {
            Ui::error(&msg);
            std::process::exit(1);
        }
    };

    let mut cache = Cache::new(!args.no_cache);
    if let Some(ref cache_cfg) = config.cache {
        cache.set_policy(cache_cfg.to_policy());
    }

    // Handle immediate utility subcommands without initializing scraping, LLM HTTP clients,
    // or opening DuckDB unless the subcommand actually requires database access.
    match action {
        crate::cli::ResolvedAction::Ask {
            list_chats,
            show_chat,
            delete_chat,
            ..
        } if list_chats || show_chat.is_some() || delete_chat.is_some() => {
            handle_ask_management(list_chats, show_chat, delete_chat, args.json)?;
            return Ok(());
        }
        crate::cli::ResolvedAction::Cache(cache_cmd) => {
            handle_cache(cache_cmd, &cache)?;
            return Ok(());
        }
        crate::cli::ResolvedAction::Config(config_cmd) => {
            handle_config(config_cmd, &config, config_path.as_deref(), &args)?;
            return Ok(());
        }
        crate::cli::ResolvedAction::History { limit } => {
            return with_db(|db| handle_history(db, limit, args.json));
        }
        crate::cli::ResolvedAction::Stats => {
            return with_db(|db| handle_stats(db, args.json));
        }
        crate::cli::ResolvedAction::Find { query } => {
            return with_db(|db| handle_find(db, query, &cache, args.json));
        }
        crate::cli::ResolvedAction::Models => {
            handle_models(&args, args.json).await?;
            return Ok(());
        }
        crate::cli::ResolvedAction::Doctor => {
            handle_doctor(&args, &config, config_path.as_deref(), &cache, args.json).await?;
            return Ok(());
        }
        crate::cli::ResolvedAction::Recommend => {
            handle_recommend(args.json)?;
            return Ok(());
        }
        crate::cli::ResolvedAction::Index => {
            return with_db(|db| handle_index(db, &cache, args.open));
        }
        crate::cli::ResolvedAction::Browse => {
            loop {
                let action = with_db(|db| crate::ui::run_tui(db, &cache))?;
                match action {
                    crate::ui::TuiAction::Exit => break,
                    crate::ui::TuiAction::Ask { target } => {
                        let llm = init_llm_with_capabilities_or_exit(&args, Some(&cache)).await;
                        let scraper = crate::scraper::WebScraper::new()?;
                        let _ = handle_ask(
                            AskArgs {
                                target: Some(target),
                                interactive: true,
                                ..Default::default()
                            },
                            &scraper,
                            &llm,
                            &cache,
                            &args,
                        )
                        .await;
                        println!("\n\x1b[2mPress Enter to return to library...\x1b[0m");
                        let mut line = String::new();
                        let _ = std::io::stdin().read_line(&mut line);
                    }
                }
            }
            return Ok(());
        }
        crate::cli::ResolvedAction::Query { sql, csv } => {
            return with_db(|db| handle_query(db, &sql, csv, args.json));
        }
        crate::cli::ResolvedAction::Export { format, output } => {
            return with_db(|db| handle_export(db, format, &output));
        }
        crate::cli::ResolvedAction::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut <Cli as clap::CommandFactory>::command(),
                "srz",
                &mut std::io::stdout(),
            );
            return Ok(());
        }
        crate::cli::ResolvedAction::Upgrade {
            check,
            force,
            version,
        } => {
            crate::system::upgrade::run_upgrade(check, force, version.as_deref(), args.json)
                .await?;
            return Ok(());
        }
        _ => {}
    }

    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    } else {
        // Sub-millisecond cache fast-path: probe cache before initializing LLM/scraper clients
        match &action {
            crate::cli::ResolvedAction::Web { url } if !crate::cli::is_feed_url(url) => {
                if let Some(cached) = cache.get_web_summary(url, args.style, &args.model) {
                    Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
                    let outcome = SummaryOutcome::from_cached(
                        target_type_for_url(url),
                        cached,
                        None,
                        Some(url.to_string()),
                    );
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            crate::cli::ResolvedAction::Web { url } if crate::cli::is_feed_url(url) => {
                if let Some(cached) = cache.get_news_summary(
                    url,
                    crate::cli::NewsTimeframe::All,
                    args.style,
                    &args.model,
                ) {
                    Ui::info(
                        "Loaded news briefing from local cache (pass `--no-cache` to refresh).",
                    );
                    let outcome =
                        SummaryOutcome::from_cached("news", cached, None, Some(url.to_string()));
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            crate::cli::ResolvedAction::Book { title } => {
                if let Some(cached) = cache.get_book_summary(title, None, args.style, &args.model) {
                    Ui::info(
                        "Loaded book summary from local cache (pass `--no-cache` to refresh).",
                    );
                    let outcome = SummaryOutcome::from_cached("book", cached, None, None);
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            crate::cli::ResolvedAction::File { path } => {
                if let Some(k) = crate::commands::summarize::file_cache_key(path)
                    && let Some(cached) = cache.get_file_summary(&k, args.style, &args.model)
                {
                    Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
                    let outcome = SummaryOutcome::from_cached(
                        "file",
                        cached,
                        None,
                        Some(path.display().to_string()),
                    );
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            crate::cli::ResolvedAction::News {
                topic, timeframe, ..
            } => {
                if let Some(cached) =
                    cache.get_news_summary(topic, *timeframe, args.style, &args.model)
                {
                    Ui::info(
                        "Loaded news briefing from local cache (pass `--no-cache` to refresh).",
                    );
                    let search_url =
                        crate::sources::news::GoogleNewsClient::search_url_for_topic(topic);
                    let outcome =
                        SummaryOutcome::from_cached("news", cached, None, Some(search_url));
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            _ => {}
        }
    }

    let scraper = match crate::scraper::WebScraper::new() {
        Ok(s) => s,
        Err(err) => {
            Ui::error(&format!("Failed to initialize scraper: {}", err));
            std::process::exit(1);
        }
    };

    let summary_result = match action {
        crate::cli::ResolvedAction::Web { url } if !crate::cli::is_feed_url(url) => {
            let (llm, page_res) = tokio::join!(
                init_llm_with_capabilities_or_exit(&args, Some(&cache)),
                fetch_url_content(url, &scraper, &cache, true)
            );
            match page_res {
                Ok(page) => summarize_fetched_web_page(url, page, &llm, &args).await,
                Err(err) => Err(err),
            }
        }
        crate::cli::ResolvedAction::Book { title } => {
            let (llm, prep_res) = tokio::join!(
                init_llm_with_capabilities_or_exit(&args, Some(&cache)),
                prepare_book_query(title, None, &scraper, &cache, &args)
            );
            match prep_res {
                Ok(prep) => summarize_prepared_book(prep, &llm, &scraper, &cache, &args).await,
                Err(err) => Err(err),
            }
        }
        other_action => {
            let llm = init_llm_with_capabilities_or_exit(&args, Some(&cache)).await;
            match other_action {
                crate::cli::ResolvedAction::Ask {
                    target,
                    question,
                    interactive,
                    list_chats,
                    resume,
                    show_chat,
                    delete_chat,
                } => {
                    if let Err(err) = handle_ask(
                        AskArgs {
                            target,
                            question,
                            interactive,
                            list_chats,
                            resume,
                            show_chat,
                            delete_chat,
                        },
                        &scraper,
                        &llm,
                        &cache,
                        &args,
                    )
                    .await
                    {
                        print_execution_error("Ask failed", &err);
                        std::process::exit(1);
                    }
                    return Ok(());
                }
                crate::cli::ResolvedAction::Web { url } => {
                    handle_web(url, &scraper, &llm, &cache, &args).await
                }
                crate::cli::ResolvedAction::File { path } => {
                    handle_file(&path, &scraper, &llm, &cache, &args).await
                }
                crate::cli::ResolvedAction::Stdin => handle_stdin(&scraper, &llm, &args).await,
                crate::cli::ResolvedAction::News {
                    topic,
                    timeframe,
                    limit,
                } => handle_news(&topic, timeframe, limit, &scraper, &llm, &cache, &args).await,
                _ => unreachable!(),
            }
        }
    };

    match summary_result {
        Ok(outcome) => present_summary_outcome(outcome, &args, &cache)?,
        Err(err) => {
            print_execution_error("Summarization failed", &err);
            std::process::exit(1);
        }
    }

    Ok(())
}

pub fn print_execution_error(action_label: &str, err: &anyhow::Error) {
    let err_msg = format!("{:#}", err);
    if err_msg.starts_with(action_label) {
        Ui::error(&err_msg);
    } else {
        Ui::error(&format!("{}: {}", action_label, err_msg));
    }

    let is_connection_error = err_msg.contains("Failed to communicate with LLM endpoint")
        || err_msg.contains("Failed to reach LLM endpoint")
        || err_msg.contains("connection closed")
        || err_msg.contains("Connection refused")
        || err_msg.contains("connection error")
        || err_msg.contains("error trying to connect")
        || err_msg.contains("is listening on port 9931");

    let is_model_not_found = err_msg.contains("Local model not detected")
        || crate::system::text_util::contains_ascii_case_insensitive(
            &err_msg,
            "model not detected",
        )
        || (crate::system::text_util::contains_ascii_case_insensitive(&err_msg, "model")
            && (crate::system::text_util::contains_ascii_case_insensitive(&err_msg, "not found")
                || crate::system::text_util::contains_ascii_case_insensitive(
                    &err_msg,
                    "not loaded",
                )
                || crate::system::text_util::contains_ascii_case_insensitive(
                    &err_msg,
                    "does not exist",
                )
                || crate::system::text_util::contains_ascii_case_insensitive(
                    &err_msg,
                    "no model is loaded",
                )));

    let is_context_error =
        crate::system::text_util::contains_ascii_case_insensitive(&err_msg, "context length")
            || crate::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "context window",
            )
            || crate::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "maximum context",
            )
            || crate::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "prompt is too long",
            );

    let is_oom_error =
        crate::system::text_util::contains_ascii_case_insensitive(&err_msg, "out of memory")
            || crate::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "cuda out of memory",
            )
            || crate::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "ggml_metal_graph_compute",
            );

    if is_connection_error {
        let hw = crate::system::hardware::HardwareProfile::detect();
        println!();
        Ui::info("💡 Quick Start: Your local LLM backend is not reachable.");
        Ui::info(&format!(
            "Based on your machine ({:.1} GB RAM detected):",
            hw.total_memory_gb
        ));
        Ui::info(&format!(
            "  • In Llama GUI (localhost:9931): Load {} ({})",
            hw.recommended_model, hw.recommended_context_display
        ));
        Ui::info("  • Or start llama-server CLI:");
        println!("    \x1b[1;32m{}\x1b[0m", hw.recommended_launch_command);
        Ui::info(
            "Or run `srz recommend` to view alternative models, Ollama commands, and profiles.\n",
        );
    } else if is_model_not_found {
        let hw = crate::system::hardware::HardwareProfile::detect();
        println!();
        Ui::info("💡 Quick Start: Local model not detected on your LLM backend.");
        Ui::info("Your LLM server is active, but could not find or load the configured model.");
        Ui::info("How to resolve:");
        Ui::info(&format!(
            "  • In Llama GUI (localhost:9931): Download or select {} ({})",
            hw.concise_model, hw.recommended_context_display
        ));
        Ui::info("  • If using Ollama, pull or run the model:");
        println!("    \x1b[1;32m{}\x1b[0m", hw.ollama_launch_command);
        Ui::info("  • If using llama.cpp CLI (llama-server):");
        println!("    \x1b[1;32m{}\x1b[0m", hw.recommended_launch_command);
        Ui::info("  • Check models currently reported by your backend:");
        println!("    \x1b[1;32msrz models\x1b[0m");
        Ui::info("  • Diagnose backend connection & model status:");
        println!("    \x1b[1;32msrz doctor\x1b[0m\n");
    } else if is_context_error {
        println!();
        Ui::info("💡 Tip: The input exceeded the local LLM's context window.");
        Ui::info(
            "  • Increase the context window in your runner (e.g. `--ctx-size 65536` or in Llama GUI settings).",
        );
        Ui::info("  • Or pass `--context-window <TOKENS>` to configure srz to fit your runner.");
        Ui::info(
            "  • Run `srz recommend` to see optimal context recommendations for your machine.\n",
        );
    } else if is_oom_error {
        println!();
        Ui::info("💡 Tip: Your LLM backend ran out of memory (VRAM/RAM).");
        Ui::info("  • Try a model with smaller quantization (e.g. Q4_K_M instead of Q8_0).");
        Ui::info("  • Or lower the context length in your runner.");
        Ui::info("  • Run `srz recommend` for hardware-optimized recommendations.\n");
    }
}
