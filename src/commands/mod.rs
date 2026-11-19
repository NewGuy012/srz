mod ask;
mod diagnostics;
mod library;
mod summarize;

use crate::cache::Cache;
use crate::cli::{CacheCommand, Cli, ConfigSubcommand, ResolvedAction};
use crate::config::Config;
use crate::db::Database;
use crate::ui::Ui;
use anyhow::{Context, Result};
use clap::{CommandFactory, ValueEnum};
use std::io::{BufRead as _, Read as _};
use std::path::Path;

pub(crate) use ask::{AskArgs, handle_ask, handle_ask_management};
pub(crate) use diagnostics::handle_models;
pub use diagnostics::{handle_doctor, handle_recommend};
pub(crate) use library::{
    handle_export, handle_find, handle_history, handle_index, handle_stats, update_index_dashboard,
};
pub(crate) use summarize::{
    SummaryOutcome, fetch_or_cache_page, fetch_url_content, handle_file, handle_news, handle_stdin,
    prepare_book_query, present_summary_outcome, summarize_fetched_web_page,
    summarize_prepared_book, target_type_for_url,
};

/// Initializes an `LlmClient` from CLI arguments.
pub(crate) fn init_llm(args: &Cli) -> Result<crate::llm::LlmClient> {
    crate::llm::LlmClient::from_cli(args).context("Failed to initialize LLM client")
}

/// Reads a single UTF-8 line up to `max_bytes`, failing cleanly if exceeded.
pub(crate) fn read_line_bounded(
    reader: &mut impl std::io::BufRead,
    max_bytes: usize,
) -> std::io::Result<String> {
    let mut buf = Vec::new();
    reader
        .by_ref()
        .take((max_bytes.saturating_add(1)) as u64)
        .read_until(b'\n', &mut buf)?;
    if buf.len() > max_bytes {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Input line exceeds maximum length of {max_bytes} bytes"),
        ));
    }
    String::from_utf8(buf).map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Opens the database via `cache`, issuing a UI warning on failure.
pub(crate) fn open_db_or_warn(cache: &Cache) -> Option<Database> {
    match cache.open_database() {
        Ok(d) => Some(d),
        Err(err) => {
            Ui::warn(&format!("Could not open database: {:#}", err));
            None
        }
    }
}

/// Executes a closure with an active database connection if available.
pub(crate) fn with_db<T: Default>(
    cache: &Cache,
    f: impl FnOnce(&Database) -> Result<T>,
) -> Result<T> {
    if let Some(db) = open_db_or_warn(cache) {
        f(&db)
    } else {
        Ui::error("Database unavailable.");
        Ok(T::default())
    }
}

/// Handles cache maintenance subcommands (`dir`, `size`, `clean`, `prune`).
pub(crate) fn handle_cache(cmd: CacheCommand, cache: &Cache) -> Result<()> {
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
            if reset_db && let Some(database) = open_db_or_warn(cache) {
                database.reset().context("Failed to reset database")?;
                db_reset = true;
            }
            if stats.total_files == 0 && !db_reset {
                Ui::info("Cache is already empty.");
            } else if stats.total_files == 0 && db_reset {
                Ui::success("Cache was already empty. Database tables reset.");
            } else if db_reset {
                Ui::success(&format!(
                    "Removed {} cached file(s) ({}) and reset database tables.",
                    stats.total_files,
                    Cache::format_bytes(stats.total_bytes)
                ));
            } else {
                Ui::success(&format!(
                    "Removed {} cached file(s) ({}). Database preserved.",
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
pub(crate) fn handle_config(
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
            println!("max_tokens = {}", args.effective_max_tokens());
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
                let template = Config::template_for_hardware(hw);
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

fn build_cache(args: &Cli, config: &Config) -> Cache {
    let mut cache = Cache::new(!args.no_cache);
    if let Some(ref cache_cfg) = config.cache {
        cache.set_policy(cache_cfg.to_policy());
    }
    cache
}

fn try_cached_summary(action: &ResolvedAction<'_>, args: &Cli, cache: &Cache) -> Result<bool> {
    if args.no_cache {
        return Ok(false);
    }

    let cache_probe: Option<(&'static str, String, Option<String>, &str)> = match action {
        ResolvedAction::Web { url } => Some((
            target_type_for_url(url),
            (*url).to_string(),
            Some((*url).to_string()),
            "Loaded summary from local cache (pass `--no-cache` to refresh).",
        )),
        ResolvedAction::Book { title } => Some((
            "book",
            Cache::book_cache_title(title, None),
            None,
            "Loaded book summary from local cache (pass `--no-cache` to refresh).",
        )),
        ResolvedAction::File { path } => {
            crate::commands::summarize::file_cache_key(path).map(|k| {
                (
                    "file",
                    k,
                    Some(path.display().to_string()),
                    "Loaded summary from local cache (pass `--no-cache` to refresh).",
                )
            })
        }
        ResolvedAction::News {
            topic, timeframe, ..
        } => Some((
            "news",
            Cache::news_cache_title(topic, *timeframe),
            Some(crate::sources::news::GoogleNewsClient::search_url_for_topic(topic)),
            "Loaded news briefing from local cache (pass `--no-cache` to refresh).",
        )),
        _ => None,
    };

    if let Some((target_type, id, url, msg)) = cache_probe
        && let Some(cached) = cache.get_summary(target_type, &id, args.style, &args.model)
    {
        Ui::info(msg);
        let outcome = SummaryOutcome::from_cached(target_type, cached, url);
        present_summary_outcome(outcome, args, cache)?;
        return Ok(true);
    }

    Ok(false)
}

/// Runs synchronous offline subcommands and cache-hit fast paths without starting Tokio.
/// Returns `Ok(true)` if handled completely, or `Ok(false)` when async execution is required.
pub fn dispatch_sync(args: &Cli, config: &Config, config_path: Option<&Path>) -> Result<bool> {
    let action = args.resolve_action().map_err(|msg| anyhow::anyhow!(msg))?;
    let cache = build_cache(args, config);

    match action {
        ResolvedAction::Ask {
            list_chats,
            show_chat,
            delete_chat,
            ..
        } if list_chats || show_chat.is_some() || delete_chat.is_some() => {
            handle_ask_management(list_chats, show_chat, delete_chat, &cache, args.json)?;
            return Ok(true);
        }
        ResolvedAction::Cache(cache_cmd) => {
            handle_cache(cache_cmd, &cache)?;
            return Ok(true);
        }
        ResolvedAction::Config(config_cmd) => {
            handle_config(config_cmd, config, config_path, args)?;
            return Ok(true);
        }
        ResolvedAction::History { limit } => {
            with_db(&cache, |db| handle_history(db, limit, args.json))?;
            return Ok(true);
        }
        ResolvedAction::Stats => {
            with_db(&cache, |db| handle_stats(db, args.json))?;
            return Ok(true);
        }
        ResolvedAction::Find { query } => {
            with_db(&cache, |db| handle_find(db, query, &cache, args.json))?;
            return Ok(true);
        }
        ResolvedAction::Recommend => {
            handle_recommend(args.json)?;
            return Ok(true);
        }
        ResolvedAction::Index => {
            with_db(&cache, |db| handle_index(db, &cache, args.open))?;
            return Ok(true);
        }
        ResolvedAction::Export { format, output } => {
            with_db(&cache, |db| handle_export(db, format, &output))?;
            return Ok(true);
        }
        ResolvedAction::Completions { shell } => {
            print!("{}", generate_completions_script(shell));
            return Ok(true);
        }
        ref other => {
            if try_cached_summary(other, args, &cache)? {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

/// Dispatches async CLI execution (after `dispatch_sync` returns `Ok(false)`).
pub async fn dispatch_async(
    args: Cli,
    config: Config,
    config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    let action = args.resolve_action().map_err(|msg| anyhow::anyhow!(msg))?;
    let cache = build_cache(&args, &config);

    match action {
        ResolvedAction::Models => return handle_models(&args, args.json).await,
        ResolvedAction::Doctor => {
            return handle_doctor(&args, &config, config_path.as_deref(), &cache, args.json).await;
        }
        ResolvedAction::Upgrade {
            check,
            force,
            version,
        } => {
            return crate::system::upgrade::run_upgrade(
                check,
                force,
                version.as_deref(),
                args.json,
            )
            .await;
        }
        ResolvedAction::Browse => {
            loop {
                let tui_action = with_db(&cache, |db| crate::ui::run_tui(db, &cache))?;
                match tui_action {
                    crate::ui::TuiAction::Exit => break,
                    crate::ui::TuiAction::Ask { target } => {
                        let scraper = crate::scraper::WebScraper::new()?;
                        match init_llm(&args) {
                            Ok(llm) => {
                                let llm = llm.init_with_capabilities(Some(&cache)).await;
                                if let Err(err) = handle_ask(
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
                                .await
                                {
                                    print_execution_error("Ask failed", &err);
                                }
                            }
                            Err(err) => print_execution_error("Ask failed", &err),
                        }
                        println!("\n\x1b[2mPress Enter to return to library...\x1b[0m");
                        let mut stdin_lock = std::io::stdin().lock();
                        let _ = read_line_bounded(&mut stdin_lock, 256);
                    }
                }
            }
            return Ok(());
        }
        _ => {}
    }

    let scraper = crate::scraper::WebScraper::new()?;
    let llm = init_llm(&args)?;
    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    }

    let summary_result = match action {
        ResolvedAction::Web { url } => {
            let (llm, page_res) = tokio::join!(
                llm.init_with_capabilities(Some(&cache)),
                fetch_url_content(url, &scraper, &cache, true)
            );
            match page_res {
                Ok(page) => summarize_fetched_web_page(url, page, &llm, &args).await,
                Err(err) => Err(err),
            }
        }
        ResolvedAction::Book { title } => {
            let (llm, prep_res) = tokio::join!(
                llm.init_with_capabilities(Some(&cache)),
                prepare_book_query(title, None, &scraper, &cache)
            );
            match prep_res {
                Ok(prep) => summarize_prepared_book(prep, &llm, &scraper, &cache, &args).await,
                Err(err) => Err(err),
            }
        }
        other_action => {
            let llm = llm.init_with_capabilities(Some(&cache)).await;
            match other_action {
                ResolvedAction::Ask {
                    target,
                    question,
                    interactive,
                    resume,
                    ..
                } => {
                    return handle_ask(
                        AskArgs {
                            target,
                            question,
                            interactive,
                            resume,
                        },
                        &scraper,
                        &llm,
                        &cache,
                        &args,
                    )
                    .await
                    .map_err(|e| e.context("Ask failed"));
                }
                ResolvedAction::File { path } => {
                    handle_file(&path, &scraper, &llm, &cache, &args).await
                }
                ResolvedAction::Stdin => handle_stdin(&scraper, &llm, &args).await,
                ResolvedAction::News {
                    topic,
                    timeframe,
                    limit,
                } => handle_news(&topic, timeframe, limit, &scraper, &llm, &cache, &args).await,
                _ => unreachable!(),
            }
        }
    };

    let outcome = summary_result.map_err(|e| e.context("Summarization failed"))?;
    present_summary_outcome(outcome, &args, &cache)
}

/// Dispatches execution of the resolved CLI action.
pub async fn dispatch_cli(
    args: Cli,
    config: Config,
    config_path: Option<std::path::PathBuf>,
) -> Result<()> {
    if dispatch_sync(&args, &config, config_path.as_deref())? {
        return Ok(());
    }
    dispatch_async(args, config, config_path).await
}

pub fn print_execution_error(action_label: &str, err: &anyhow::Error) {
    let err_msg = format!("{:#}", err);
    if err_msg.starts_with(action_label)
        || err_msg.starts_with("Ask failed:")
        || err_msg.starts_with("Summarization failed:")
        || err_msg.starts_with("Failed to ")
    {
        Ui::error(&err_msg);
    } else {
        Ui::error(&format!("{}: {}", action_label, err_msg));
    }

    let has_any = |needles: &[&str]| {
        needles
            .iter()
            .any(|n| crate::system::text_util::find_ascii_case_insensitive(&err_msg, n).is_some())
    };
    let is_connection_error = has_any(&[
        "failed to communicate with llm endpoint",
        "failed to reach llm endpoint",
        "connection closed",
        "connection refused",
        "connection error",
        "error trying to connect",
        "is listening on port 9931",
    ]);
    let is_model_not_found = has_any(&["local model not detected", "model not detected"])
        || (has_any(&["model"])
            && has_any(&[
                "not found",
                "not loaded",
                "does not exist",
                "no model is loaded",
            ]));
    let is_context_error = has_any(&[
        "context length",
        "context window",
        "maximum context",
        "prompt is too long",
    ]);
    let is_oom_error = has_any(&[
        "out of memory",
        "cuda out of memory",
        "ggml_metal_graph_compute",
    ]);

    if is_connection_error {
        println!();
        Ui::info("💡 Quick Start: Your local LLM backend is not reachable.");
        Ui::info("How to resolve:");
        Ui::info("  • Ensure your LLM server (llama-server, Ollama, etc.) is running.");
        Ui::info("  • Run `srz doctor` to diagnose backend connectivity.");
        Ui::info("  • Run `srz recommend` to view recommended models and launch commands.\n");
    } else if is_model_not_found {
        println!();
        Ui::info("💡 Quick Start: Local model not detected on your LLM backend.");
        Ui::info("Your LLM server is active, but could not find or load the configured model.");
        Ui::info("How to resolve:");
        Ui::info("  • Check models currently reported by your backend:");
        println!("    \x1b[1;32msrz models\x1b[0m");
        Ui::info("  • Run `srz recommend` for model suggestions for your machine:");
        println!("    \x1b[1;32msrz recommend\x1b[0m");
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

fn enum_values<T: ValueEnum>() -> String {
    T::value_variants()
        .iter()
        .filter_map(|v| v.to_possible_value().map(|pv| pv.get_name().to_string()))
        .collect::<Vec<_>>()
        .join(" ")
}

fn subcommand_names(cmd: &clap::Command, parent: &str) -> String {
    cmd.find_subcommand(parent)
        .map(|sub| {
            sub.get_subcommands()
                .filter(|s| !s.is_hide_set() && s.get_name() != "help")
                .map(|s| s.get_name())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn first_line(text: Option<&clap::builder::StyledStr>) -> String {
    text.and_then(|s| s.to_string().lines().next().map(str::to_string))
        .unwrap_or_default()
}

fn zsh_escape_single(s: &str) -> String {
    s.replace('\'', "'\\''")
}

fn zsh_escape_desc(s: &str) -> String {
    zsh_escape_single(s).replace('[', "\\[").replace(']', "\\]")
}

fn fish_escape_dq(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
}

fn ps_array(words: &str) -> String {
    words
        .split_whitespace()
        .map(|w| format!("'{}'", w.replace('\'', "''")))
        .collect::<Vec<_>>()
        .join(", ")
}

fn possible_value_names(arg: &clap::Arg) -> String {
    arg.get_possible_values()
        .into_iter()
        .filter(|pv| !pv.is_hide_set())
        .map(|pv| pv.get_name().to_string())
        .collect::<Vec<_>>()
        .join(" ")
}

fn zsh_argument_lines(cmd: &clap::Command) -> String {
    let mut lines = Vec::new();
    for arg in cmd.get_arguments() {
        if arg.is_positional() || arg.is_hide_set() {
            continue;
        }
        let mut names = Vec::new();
        if let Some(s) = arg.get_short() {
            names.push(format!("-{s}"));
        }
        if let Some(l) = arg.get_long() {
            names.push(format!("--{l}"));
        }
        for alias in arg.get_all_aliases().into_iter().flatten() {
            names.push(format!("--{alias}"));
        }
        if names.is_empty() {
            continue;
        }

        let desc = zsh_escape_desc(&first_line(arg.get_help()));
        let takes_val = arg.get_action().takes_values();
        let val_suffix = if !takes_val {
            String::new()
        } else {
            let pvs = possible_value_names(arg);
            if !pvs.is_empty() {
                format!(":VALUE:({pvs})")
            } else if matches!(arg.get_id().as_str(), "config" | "output") {
                ":FILE:_files".to_string()
            } else if arg.get_id().as_str() == "base_url" {
                ":URL:_urls".to_string()
            } else {
                let vname = arg
                    .get_value_names()
                    .and_then(|v| v.first())
                    .map(|s| s.as_str())
                    .unwrap_or("VALUE");
                format!(":{vname}:")
            }
        };

        let spec = if names.len() == 1 {
            let eq = if takes_val { "=" } else { "" };
            format!("        '{}{eq}[{desc}]{val_suffix}'", names[0])
        } else {
            let joined = names.join(" ");
            let brace = names.join(",");
            format!("        '({joined})'{{{brace}}}'[{desc}]{val_suffix}'")
        };
        lines.push(spec);
    }
    lines.join(" \\\n")
}

fn fish_flag_lines(cmd: &clap::Command) -> String {
    let mut out = String::new();
    for arg in cmd.get_arguments() {
        if arg.is_positional() || arg.is_hide_set() {
            continue;
        }
        let desc = fish_escape_dq(&first_line(arg.get_help()));
        let mut line = String::from("complete -c srz");
        if let Some(s) = arg.get_short() {
            line.push_str(&format!(" -s {s}"));
        }
        if let Some(l) = arg.get_long() {
            line.push_str(&format!(" -l {l}"));
        }
        if matches!(arg.get_id().as_str(), "config" | "output") {
            line.push_str(" -F");
        }
        let pvs = possible_value_names(arg);
        if !pvs.is_empty() {
            line.push_str(&format!(" -a \"{pvs}\""));
        }
        line.push_str(&format!(" -d \"{desc}\"\n"));
        out.push_str(&line);

        if let Some(long) = arg.get_long() {
            for alias in arg.get_all_aliases().into_iter().flatten() {
                out.push_str(&format!(
                    "complete -c srz -l {alias} -d \"Alias for --{long}\"\n"
                ));
            }
        }
    }
    out
}

/// Generates a shell completion script matching the active `srz` CLI definition.
pub fn generate_completions_script(shell: crate::cli::Shell) -> String {
    let mut cmd = Cli::command();
    cmd.build();

    let commands: Vec<(String, String)> = cmd
        .get_subcommands()
        .filter(|s| !s.is_hide_set())
        .map(|s| (s.get_name().to_string(), first_line(s.get_about())))
        .collect();
    let cmds_space = commands
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let mut flags = Vec::new();
    for arg in cmd.get_arguments() {
        if arg.is_positional() || arg.is_hide_set() {
            continue;
        }
        if let Some(s) = arg.get_short() {
            flags.push(format!("-{s}"));
        }
        if let Some(l) = arg.get_long() {
            flags.push(format!("--{l}"));
        }
        for alias in arg.get_all_aliases().into_iter().flatten() {
            flags.push(format!("--{alias}"));
        }
    }
    let flags_space = flags.join(" ");

    let styles = enum_values::<crate::config::SummaryStyle>();
    let paging = enum_values::<crate::config::PagingMode>();
    let shells = enum_values::<crate::config::Shell>();
    let formats = enum_values::<crate::config::ExportFormat>();
    let timeframes = enum_values::<crate::config::NewsTimeframe>();
    let cache_cmds = subcommand_names(&cmd, "cache");
    let config_cmds = subcommand_names(&cmd, "config");

    match shell {
        crate::cli::Shell::Bash => format!(
            r#"_srz() {{
    local cur prev opts cmds
    COMPREPLY=()
    cur="${{COMP_WORDS[COMP_CWORD]}}"
    prev="${{COMP_WORDS[COMP_CWORD-1]}}"
    cmds="{cmds_space}"
    opts="{flags_space}"

    if [[ ${{COMP_CWORD}} -eq 1 ]] ; then
        COMPREPLY=( $(compgen -W "${{cmds}} ${{opts}}" -- ${{cur}}) )
        return 0
    fi

    case "${{prev}}" in
        --style) COMPREPLY=( $(compgen -W "{styles}" -- ${{cur}}) ); return 0 ;;
        --paging) COMPREPLY=( $(compgen -W "{paging}" -- ${{cur}}) ); return 0 ;;
        -t|--timeframe) COMPREPLY=( $(compgen -W "{timeframes}" -- ${{cur}}) ); return 0 ;;
        -f|--format|export) COMPREPLY=( $(compgen -W "{formats}" -- ${{cur}}) ); return 0 ;;
        completions) COMPREPLY=( $(compgen -W "{shells}" -- ${{cur}}) ); return 0 ;;
        cache) COMPREPLY=( $(compgen -W "{cache_cmds}" -- ${{cur}}) ); return 0 ;;
        config) COMPREPLY=( $(compgen -W "{config_cmds}" -- ${{cur}}) ); return 0 ;;
    esac

    COMPREPLY=( $(compgen -W "${{opts}}" -- ${{cur}}) )
    return 0
}}
complete -F _srz -o bashdefault -o default srz
"#
        ),
        crate::cli::Shell::Zsh => {
            let cmd_entries = commands
                .iter()
                .map(|(name, desc)| {
                    format!(
                        "        '{}:{}'",
                        zsh_escape_single(name),
                        zsh_escape_single(desc)
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let arg_entries = zsh_argument_lines(&cmd);
            format!(
                r#"#compdef srz

_srz() {{
    local -a commands
    commands=(
{cmd_entries}
    )

    _arguments -C \
{arg_entries} \
        '1: :->command' \
        '*:: :->args'

    case $state in
        command) _describe -t commands 'srz commands' commands ;;
        args)
            case $line[1] in
                cache) _values 'cache command' {cache_cmds} ;;
                config) _values 'config command' {config_cmds} ;;
                completions) _values 'shell' {shells} ;;
                export) _values 'format' {formats} ;;
                news) _arguments '(-t --timeframe)'{{-t,--timeframe}}'[Timeframe]:TIMEFRAME:({timeframes})' ;;
                *) _files ;;
            esac
            ;;
    esac
}}

_srz "$@"
"#
            )
        }
        crate::cli::Shell::Fish => {
            let mut out = String::from("complete -c srz -f\n");
            for (name, desc) in &commands {
                if name != "help" {
                    out.push_str(&format!(
                        "complete -c srz -n \"__fish_use_subcommand\" -a \"{name}\" -d \"{}\"\n",
                        fish_escape_dq(desc)
                    ));
                }
            }
            out.push_str(&format!(
                "complete -c srz -n \"__fish_seen_subcommand_from cache\" -a \"{cache_cmds}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from config\" -a \"{config_cmds}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from completions\" -a \"{shells}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from export\" -a \"{formats}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from news\" -s t -l timeframe -a \"{timeframes}\" -d \"Timeframe for news recency\"\n"
            ));
            out.push_str(&fish_flag_lines(&cmd));
            out
        }
        crate::cli::Shell::PowerShell => format!(
            r#"Register-ArgumentCompleter -Native -CommandName 'srz' -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @({cmds})
    $flags = @({flags})
    $elements = $commandAst.CommandElements
    $vals = if ($elements.Count -le 2) {{
        $commands + $flags
    }} else {{
        switch ($elements[-2].Extent.Text) {{
            'cache'       {{ @({cache_cmds}) }}
            'config'      {{ @({config_cmds}) }}
            'completions' {{ @({shells}) }}
            'export'      {{ @({formats}) }}
            '--style'     {{ @({styles}) }}
            '--paging'    {{ @({paging}) }}
            '-t'          {{ @({timeframes}) }}
            '--timeframe' {{ @({timeframes}) }}
            default       {{ $flags }}
        }}
    }}
    $vals | Where-Object {{ $_ -like "$wordToComplete*" }} | ForEach-Object {{
        [System.Management.Automation.CompletionResult]::new($_, $_, 'ParameterValue', $_)
    }}
}}
"#,
            cmds = ps_array(&cmds_space),
            flags = ps_array(&flags_space),
            cache_cmds = ps_array(&cache_cmds),
            config_cmds = ps_array(&config_cmds),
            shells = ps_array(&shells),
            formats = ps_array(&formats),
            styles = ps_array(&styles),
            paging = ps_array(&paging),
            timeframes = ps_array(&timeframes),
        ),
        crate::cli::Shell::Elvish => format!(
            r#"set edit:completion:arg-completer[srz] = {{|@words|
    var num-words = (count $words)
    if (== $num-words 2) {{
        put {cmds_space} {flags_space}
    }} elif (== $num-words 3) {{
        var prev = $words[1]
        if (== $prev cache) {{ put {cache_cmds}
        }} elif (== $prev config) {{ put {config_cmds}
        }} elif (== $prev completions) {{ put {shells}
        }} elif (== $prev export) {{ put {formats}
        }} elif (== $prev --style) {{ put {styles}
        }} elif (== $prev --paging) {{ put {paging}
        }} elif (or (== $prev -t) (== $prev --timeframe)) {{ put {timeframes} }}
    }}
}}
"#
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_completions_match_cli_surface() {
        for shell in [
            crate::cli::Shell::Bash,
            crate::cli::Shell::Zsh,
            crate::cli::Shell::Fish,
            crate::cli::Shell::PowerShell,
            crate::cli::Shell::Elvish,
        ] {
            let script = generate_completions_script(shell);
            for required in ["cache", "browse", "recommend", "bullets"] {
                assert!(
                    script.contains(required),
                    "shell {:?} missing '{required}'",
                    shell
                );
            }
            for flag in ["paging", "plain"] {
                let expected = match shell {
                    crate::cli::Shell::Fish => format!("-l {flag}"),
                    _ => format!("--{flag}"),
                };
                assert!(
                    script.contains(&expected),
                    "shell {:?} missing '{expected}'",
                    shell
                );
            }
            for legacy in [
                "--no-stream",
                "--page",
                "bullet action technical",
                "web book",
            ] {
                assert!(
                    !script.contains(legacy),
                    "shell {:?} still has stale '{legacy}'",
                    shell
                );
            }
        }
    }

    #[test]
    fn test_cache_clean_reset_db_reuses_open_cache_handle() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let cache = Cache::with_dir(tmp.path().to_path_buf(), true);
        let db = cache.open_database().unwrap();
        db.record_summary(
            &crate::db::NewSummaryRecord {
                target_type: "web",
                title: "Lock Safety",
                author: None,
                url: Some("https://example.com"),
                style: "concise",
                model: "gemma-4",
                word_count: 10,
                prompt_tokens: None,
                completion_tokens: None,
                duration_ms: 0,
                tokens_per_second: None,
                summary: "Summary text",
                reasoning: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(db.get_stats().unwrap().total_summaries, 1);

        handle_cache(CacheCommand::Clean { reset_db: true }, &cache).unwrap();
        assert_eq!(db.get_stats().unwrap().total_summaries, 0);
    }

    #[test]
    fn test_read_line_bounded_enforces_limit() {
        let mut ok_reader = std::io::Cursor::new(b"hello\n");
        assert_eq!(read_line_bounded(&mut ok_reader, 6).unwrap(), "hello\n");

        let mut eof_reader = std::io::Cursor::new(b"");
        assert_eq!(read_line_bounded(&mut eof_reader, 6).unwrap(), "");

        let mut long_reader = std::io::Cursor::new(b"1234567\n");
        assert!(read_line_bounded(&mut long_reader, 6).is_err());
    }
}
