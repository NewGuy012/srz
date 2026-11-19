mod ask;
mod diagnostics;
mod library;
mod summarize;

use crate::cache::Cache;
use crate::config::{CacheCommand, Cli, Commands, Config, ConfigSubcommand, ResolvedAction, Shell};
use crate::ui::{Ui, sanitize_terminal_text};
use anyhow::{Context, Result};
use clap::{CommandFactory, ValueEnum};
use serde::Serialize;
use std::fmt::Write as _;
use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

use ask::{AskArgs, handle_ask, handle_ask_management};
use diagnostics::{handle_doctor, handle_models, handle_recommend};
use library::{
    handle_export, handle_find, handle_history, handle_index, handle_stats, update_index_dashboard,
};
use summarize::{
    SummaryOutcome, canonical_file_url_string, fetch_url_content, file_cache_key, handle_file,
    handle_news, handle_stdin, prepare_book_query, present_summary_outcome,
    summarize_fetched_web_page, summarize_prepared_book, target_type_for_url,
};

pub(super) fn print_json<T: Serialize>(val: &T) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(val)?);
    Ok(())
}

pub(super) fn file_url(path: &Path) -> String {
    let safe = sanitize_terminal_text(&path.display().to_string());
    let normalized = safe.replace('\\', "/");
    let mut url = String::with_capacity(normalized.len() + 8);
    url.push_str(if normalized.starts_with('/') {
        "file://"
    } else {
        "file:///"
    });
    for ch in normalized.chars() {
        match ch {
            ' ' => url.push_str("%20"),
            '%' => url.push_str("%25"),
            '#' => url.push_str("%23"),
            '?' => url.push_str("%3F"),
            _ => url.push(ch),
        }
    }
    url
}

pub(super) fn init_llm(args: &Cli) -> Result<crate::llm::LlmClient> {
    crate::llm::LlmClient::from_cli(args).context("Failed to initialize LLM client")
}

pub(super) fn read_line_bounded(
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

pub(super) fn open_db_or_warn(cache: &Cache) -> Option<crate::db::Database> {
    match cache.open_database() {
        Ok(d) => Some(d),
        Err(err) => {
            Ui::warn(&format!("Could not open database: {err:#}"));
            None
        }
    }
}

fn handle_cache(cmd: &CacheCommand, cache: &Cache) -> Result<()> {
    match cmd {
        CacheCommand::Dir => {
            println!(
                "{}",
                sanitize_terminal_text(&cache.dir().display().to_string())
            );
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
            if *reset_db {
                cache
                    .open_database()?
                    .reset()
                    .context("Failed to reset database")?;
            }
            if stats.total_files == 0 && !reset_db {
                Ui::info("Cache is already empty.");
            } else if stats.total_files == 0 && *reset_db {
                Ui::success("Cache was already empty. Database tables reset.");
            } else if *reset_db {
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
            let parsed_older_than = older_than
                .as_deref()
                .map(|s| crate::cache::parse_duration(s).map_err(|e| anyhow::anyhow!(e)))
                .transpose()?;
            let parsed_max_size = max_size
                .as_deref()
                .map(|s| crate::cache::parse_bytes(s).map_err(|e| anyhow::anyhow!(e)))
                .transpose()?
                .or_else(|| {
                    parsed_older_than
                        .is_none()
                        .then_some(cache.policy().max_bytes)
                });

            let prune_stats = cache
                .prune(crate::cache::PruneOptions {
                    older_than: parsed_older_than,
                    max_bytes: parsed_max_size,
                })
                .context("Failed to prune cache")?;
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

fn handle_config(
    subcmd: ConfigSubcommand,
    config: &Config,
    config_path: Option<&Path>,
    args: &Cli,
) -> Result<()> {
    match subcmd {
        ConfigSubcommand::Path => {
            if let Some(p) = config_path {
                println!(
                    "{} (active, exists: {})",
                    sanitize_terminal_text(&p.display().to_string()),
                    p.exists()
                );
            } else {
                println!(
                    "{} (not found, using defaults)",
                    sanitize_terminal_text(&Config::default_config_path().display().to_string())
                );
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
            let safe_path = sanitize_terminal_text(&default_path.display().to_string());
            let hw = crate::system::hardware::HardwareProfile::detect();
            let template = Config::template_for_hardware(hw);
            if !crate::fs_util::write_private_file_exclusive(&default_path, template.as_bytes())? {
                Ui::info(&format!("Config file already exists at {safe_path}"));
                Ui::info(
                    "Tip: Run `srz recommend` to view the optimal model and context for your machine.",
                );
            } else {
                Ui::success(&format!(
                    "Initialized config file tailored for your hardware ({:.1} GB RAM, {}) at {safe_path}",
                    hw.total_memory_gb, hw.recommended_context_display
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
            "summary",
        )),
        ResolvedAction::Book { title } => Some((
            "book",
            Cache::book_cache_title(title, None),
            None,
            "book summary",
        )),
        ResolvedAction::File { path } => file_cache_key(path)
            .map(|k| ("file", k, Some(canonical_file_url_string(path)), "summary")),
        ResolvedAction::News {
            topic, timeframe, ..
        } => Some((
            "news",
            Cache::news_cache_title(topic, *timeframe),
            Some(crate::sources::news::GoogleNewsClient::search_url_for_topic(topic)),
            "news briefing",
        )),
        _ => None,
    };

    if let Some((target_type, id, fallback_url, label)) = cache_probe
        && let Some(cached) = cache.get_summary(target_type, &id, args.style, &args.model)
    {
        Ui::info(&format!(
            "Loaded {label} from local cache (pass `--no-cache` to refresh)."
        ));
        present_summary_outcome(
            SummaryOutcome::from_cached(target_type, cached, fallback_url),
            args,
            cache,
        )?;
        return Ok(true);
    }

    Ok(false)
}

fn dispatch_sync(
    action: &ResolvedAction<'_>,
    args: &Cli,
    config: &Config,
    config_path: Option<&Path>,
    cache: &Cache,
) -> Result<bool> {
    match action {
        ResolvedAction::Ask {
            list_chats,
            show_chat,
            delete_chat,
            ..
        } if *list_chats || show_chat.is_some() || delete_chat.is_some() => {
            handle_ask_management(*list_chats, *show_chat, *delete_chat, cache, args.json)?;
            return Ok(true);
        }
        ResolvedAction::Subcommand(cmd) => match cmd {
            Commands::Cache { action } => {
                handle_cache(action, cache)?;
                return Ok(true);
            }
            Commands::Config { action } => {
                handle_config(*action, config, config_path, args)?;
                return Ok(true);
            }
            Commands::History { limit } => {
                handle_history(&cache.open_database()?, *limit, args.json)?;
                return Ok(true);
            }
            Commands::Stats => {
                handle_stats(&cache.open_database()?, args.json)?;
                return Ok(true);
            }
            Commands::Find { query } => {
                handle_find(&cache.open_database()?, query.trim(), cache, args.json)?;
                return Ok(true);
            }
            Commands::Recommend => {
                handle_recommend(args.json)?;
                return Ok(true);
            }
            Commands::Index => {
                handle_index(&cache.open_database()?, cache, args.open)?;
                return Ok(true);
            }
            Commands::Export { format, output } => {
                handle_export(&cache.open_database()?, *format, output)?;
                return Ok(true);
            }
            Commands::Completions { shell } => {
                print!("{}", generate_completions_script(*shell));
                return Ok(true);
            }
            _ => {}
        },
        _ => {
            if try_cached_summary(action, args, cache)? {
                return Ok(true);
            }
        }
    }

    Ok(false)
}

async fn dispatch_async(
    action: &ResolvedAction<'_>,
    args: &Cli,
    config: &Config,
    config_path: Option<&Path>,
    cache: &Cache,
) -> Result<()> {
    if let ResolvedAction::Subcommand(cmd) = action {
        match cmd {
            Commands::Models => return handle_models(args, args.json).await,
            Commands::Doctor => {
                return handle_doctor(args, config, config_path, cache, args.json).await;
            }
            Commands::Upgrade {
                check,
                force,
                version,
            } => {
                return crate::system::upgrade::run_upgrade(
                    *check,
                    *force,
                    version.as_deref(),
                    args.json,
                )
                .await;
            }
            Commands::Browse => {
                loop {
                    let tui_action = {
                        let db = cache.open_database()?;
                        crate::ui::run_tui(&db, cache)?
                    };
                    match tui_action {
                        crate::ui::TuiAction::Exit => break,
                        crate::ui::TuiAction::Ask { target } => {
                            let scraper = crate::scraper::WebScraper::new()?;
                            match init_llm(args) {
                                Ok(llm) => {
                                    let llm = llm.init_with_capabilities(Some(cache)).await;
                                    if let Err(err) = handle_ask(
                                        AskArgs {
                                            target: Some(target),
                                            interactive: true,
                                            ..Default::default()
                                        },
                                        &scraper,
                                        &llm,
                                        cache,
                                        args,
                                    )
                                    .await
                                    {
                                        print_execution_error("Ask failed", &err);
                                    }
                                }
                                Err(err) => print_execution_error("Ask failed", &err),
                            }
                            if Ui::use_stdout_color() {
                                println!("\n\x1b[2mPress Enter to return to library...\x1b[0m");
                            } else {
                                println!("\nPress Enter to return to library...");
                            }
                            let mut stdin_lock = std::io::stdin().lock();
                            let _ = read_line_bounded(&mut stdin_lock, 256);
                        }
                    }
                }
                return Ok(());
            }
            _ => unreachable!(),
        }
    }

    let scraper = crate::scraper::WebScraper::new()?;
    let llm = init_llm(args)?;
    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    }

    let summary_result = match action {
        ResolvedAction::Web { url } => {
            let (llm, page_res) = tokio::join!(
                llm.init_with_capabilities(Some(cache)),
                fetch_url_content(url, &scraper, cache, true)
            );
            match page_res {
                Ok(page) => summarize_fetched_web_page(url, page, &llm, args).await,
                Err(err) => Err(err),
            }
        }
        ResolvedAction::Book { title } => {
            let (llm, prep) = tokio::join!(
                llm.init_with_capabilities(Some(cache)),
                prepare_book_query(title, &scraper, cache)
            );
            summarize_prepared_book(prep, &llm, &scraper, cache, args).await
        }
        other => {
            let llm = llm.init_with_capabilities(Some(cache)).await;
            match other {
                ResolvedAction::Ask {
                    target,
                    question,
                    interactive,
                    resume,
                    ..
                } => {
                    return handle_ask(
                        AskArgs {
                            target: target.clone(),
                            question: question.clone(),
                            interactive: *interactive,
                            resume: *resume,
                        },
                        &scraper,
                        &llm,
                        cache,
                        args,
                    )
                    .await
                    .map_err(|e| e.context("Ask failed"));
                }
                ResolvedAction::File { path } => {
                    handle_file(path, &scraper, &llm, cache, args).await
                }
                ResolvedAction::Stdin => handle_stdin(&scraper, &llm, args).await,
                ResolvedAction::News {
                    topic,
                    timeframe,
                    limit,
                } => handle_news(topic, *timeframe, *limit, &scraper, &llm, cache, args).await,
                _ => unreachable!(),
            }
        }
    };

    let outcome = summary_result.map_err(|e| e.context("Summarization failed"))?;
    present_summary_outcome(outcome, args, cache)
}

/// Resolves the CLI action and cache once, runs fast sync paths without Tokio, or executes async commands.
pub fn execute(args: Cli, config: Config, config_path: Option<PathBuf>) -> Result<()> {
    let action = args.resolve_action()?;
    let cache = build_cache(&args, &config);

    if dispatch_sync(&action, &args, &config, config_path.as_deref(), &cache)? {
        return Ok(());
    }

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(dispatch_async(
            &action,
            &args,
            &config,
            config_path.as_deref(),
            &cache,
        ))
}

pub fn print_execution_error(action_label: &str, err: &anyhow::Error) {
    let err_msg = format!("{err:#}");
    if err_msg.starts_with(action_label)
        || err_msg.starts_with("Ask failed:")
        || err_msg.starts_with("Summarization failed:")
        || err_msg.starts_with("Failed to ")
    {
        Ui::error(&err_msg);
    } else {
        Ui::error(&format!("{action_label}: {err_msg}"));
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
        "listening on port",
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
        Ui::info("");
        Ui::info("💡 Quick Start: Your local LLM backend is not reachable.");
        Ui::info("How to resolve:");
        Ui::info("  • Ensure your LLM server (llama-server, Ollama, etc.) is running.");
        Ui::info("  • Run `srz doctor` to diagnose backend connectivity.");
        Ui::info("  • Run `srz recommend` to view recommended models and launch commands.");
    } else if is_model_not_found {
        Ui::info("");
        Ui::info("💡 Quick Start: Local model not detected on your LLM backend.");
        Ui::info("How to resolve:");
        Ui::info("  • Check available models:  srz models");
        Ui::info("  • View recommendations:    srz recommend");
        Ui::info("  • Diagnose configuration:  srz doctor");
    } else if is_context_error {
        Ui::info("");
        Ui::info("💡 Tip: The input exceeded the LLM's context window.");
        Ui::info("  • Increase `--ctx-size` in your runner or pass `--context-window <TOKENS>`.");
        Ui::info("  • Run `srz recommend` for optimal settings on your machine.");
    } else if is_oom_error {
        Ui::info("");
        Ui::info("💡 Tip: Your LLM backend ran out of memory (VRAM/RAM).");
        Ui::info("  • Try a smaller quantization (e.g. Q4_K_M) or lower context length.");
        Ui::info("  • Run `srz recommend` for hardware-optimized settings.");
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
    zsh_escape_single(s)
        .replace(':', "\\:")
        .replace('[', "\\[")
        .replace(']', "\\]")
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

struct FlagSpec {
    short: Option<char>,
    long: Option<String>,
    aliases: Vec<String>,
    help: String,
    takes_val: bool,
    possible_vals: String,
    is_file: bool,
    is_url: bool,
    val_name: String,
}

fn collect_visible_flags(cmd: &clap::Command) -> Vec<FlagSpec> {
    cmd.get_arguments()
        .filter(|arg| !arg.is_positional() && !arg.is_hide_set())
        .map(|arg| FlagSpec {
            short: arg.get_short(),
            long: arg.get_long().map(str::to_string),
            aliases: arg
                .get_all_aliases()
                .into_iter()
                .flatten()
                .map(str::to_string)
                .collect(),
            help: first_line(arg.get_help()),
            takes_val: arg.get_action().takes_values(),
            possible_vals: arg
                .get_possible_values()
                .into_iter()
                .filter(|pv| !pv.is_hide_set())
                .map(|pv| pv.get_name().to_string())
                .collect::<Vec<_>>()
                .join(" "),
            is_file: matches!(arg.get_id().as_str(), "config" | "output"),
            is_url: arg.get_id().as_str() == "base_url",
            val_name: arg
                .get_value_names()
                .and_then(|v| v.first())
                .map(|s| s.as_str())
                .unwrap_or("VALUE")
                .to_string(),
        })
        .collect()
}

fn zsh_argument_lines(specs: &[FlagSpec]) -> String {
    let mut lines = Vec::new();
    for spec in specs {
        let mut names = Vec::new();
        if let Some(s) = spec.short {
            names.push(format!("-{s}"));
        }
        if let Some(ref l) = spec.long {
            names.push(format!("--{l}"));
        }
        for alias in &spec.aliases {
            names.push(format!("--{alias}"));
        }
        if names.is_empty() {
            continue;
        }

        let desc = zsh_escape_desc(&spec.help);
        let val_suffix = if !spec.takes_val {
            String::new()
        } else if !spec.possible_vals.is_empty() {
            format!(":VALUE:({})", spec.possible_vals)
        } else if spec.is_file {
            ":FILE:_files".to_string()
        } else if spec.is_url {
            ":URL:_urls".to_string()
        } else {
            format!(":{}:", spec.val_name)
        };

        let line = if names.len() == 1 {
            let eq = if spec.takes_val && names[0].starts_with("--") {
                "="
            } else {
                ""
            };
            format!("        '{}{eq}[{desc}]{val_suffix}'", names[0])
        } else {
            let joined = names.join(" ");
            let brace_items: Vec<String> = names
                .iter()
                .map(|n| {
                    if spec.takes_val && n.starts_with("--") {
                        format!("{n}=")
                    } else {
                        n.clone()
                    }
                })
                .collect();
            let brace = brace_items.join(",");
            format!("        '({joined})'{{{brace}}}'[{desc}]{val_suffix}'")
        };
        lines.push(line);
    }
    lines.join(" \\\n")
}

fn fish_flag_lines(specs: &[FlagSpec]) -> String {
    let mut out = String::new();
    for spec in specs {
        let desc = fish_escape_dq(&spec.help);
        out.push_str("complete -c srz");
        if let Some(s) = spec.short {
            let _ = write!(out, " -s {s}");
        }
        if let Some(ref l) = spec.long {
            let _ = write!(out, " -l {l}");
        }
        if spec.is_file {
            out.push_str(" -F");
        }
        if !spec.possible_vals.is_empty() {
            let _ = write!(out, " -a \"{}\"", spec.possible_vals);
        }
        let _ = writeln!(out, " -d \"{desc}\"");
        if let Some(ref long) = spec.long {
            for alias in &spec.aliases {
                let _ = writeln!(out, "complete -c srz -l {alias} -d \"Alias for --{long}\"");
            }
        }
    }
    out
}

fn generate_completions_script(shell: Shell) -> String {
    let mut cmd = Cli::command();
    cmd.build();

    let commands: Vec<(String, String)> = cmd
        .get_subcommands()
        .filter(|s| !s.is_hide_set() && s.get_name() != "help")
        .map(|s| (s.get_name().to_string(), first_line(s.get_about())))
        .collect();
    let cmds_space = commands
        .iter()
        .map(|(n, _)| n.as_str())
        .collect::<Vec<_>>()
        .join(" ");

    let specs = collect_visible_flags(&cmd);
    let mut flag_names = Vec::new();
    for s in &specs {
        if let Some(c) = s.short {
            flag_names.push(format!("-{c}"));
        }
        if let Some(ref l) = s.long {
            flag_names.push(format!("--{l}"));
        }
        for a in &s.aliases {
            flag_names.push(format!("--{a}"));
        }
    }
    let flags_space = flag_names.join(" ");

    let styles = enum_values::<crate::config::SummaryStyle>();
    let paging = enum_values::<crate::config::PagingMode>();
    let shells = enum_values::<crate::config::Shell>();
    let formats = enum_values::<crate::config::ExportFormat>();
    let timeframes = enum_values::<crate::config::NewsTimeframe>();
    let cache_cmds = subcommand_names(&cmd, "cache");
    let config_cmds = subcommand_names(&cmd, "config");

    match shell {
        Shell::Bash => format!(
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
        Shell::Zsh => {
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
            let arg_entries = zsh_argument_lines(&specs);
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
        Shell::Fish => {
            let mut out = String::from("complete -c srz -f\n");
            for (name, desc) in &commands {
                let _ = writeln!(
                    out,
                    "complete -c srz -n \"__fish_use_subcommand\" -a \"{name}\" -d \"{}\"",
                    fish_escape_dq(desc)
                );
            }
            let _ = write!(
                out,
                "complete -c srz -n \"__fish_seen_subcommand_from cache\" -a \"{cache_cmds}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from config\" -a \"{config_cmds}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from completions\" -a \"{shells}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from export\" -a \"{formats}\"\n\
                 complete -c srz -n \"__fish_seen_subcommand_from news\" -s t -l timeframe -a \"{timeframes}\" -d \"Timeframe for news recency\"\n"
            );
            out.push_str(&fish_flag_lines(&specs));
            out
        }
        Shell::PowerShell => format!(
            r#"Register-ArgumentCompleter -Native -CommandName 'srz' -ScriptBlock {{
    param($wordToComplete, $commandAst, $cursorPosition)
    $commands = @({cmds})
    $flags = @({flags})
    $elements = $commandAst.CommandElements
    $prevIndex = if ([string]::IsNullOrEmpty($wordToComplete)) {{ $elements.Count - 1 }} else {{ $elements.Count - 2 }}
    $vals = if ($prevIndex -lt 1) {{
        $commands + $flags
    }} else {{
        switch ($elements[$prevIndex].Extent.Text) {{
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
        Shell::Elvish => format!(
            r#"set edit:completion:arg-completer[srz] = {{|@words|
    var num-words = (count $words)
    if (== $num-words 2) {{
        put {cmds_space} {flags_space}
    }} elif (>= $num-words 3) {{
        var prev = $words[-2]
        if (== $prev cache) {{ put {cache_cmds}
        }} elif (== $prev config) {{ put {config_cmds}
        }} elif (== $prev completions) {{ put {shells}
        }} elif (== $prev export) {{ put {formats}
        }} elif (== $prev --style) {{ put {styles}
        }} elif (== $prev --paging) {{ put {paging}
        }} elif (or (== $prev -t) (== $prev --timeframe)) {{ put {timeframes}
        }} else {{ put {flags_space} }}
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
            Shell::Bash,
            Shell::Zsh,
            Shell::Fish,
            Shell::PowerShell,
            Shell::Elvish,
        ] {
            let script = generate_completions_script(shell);
            for required in ["cache", "browse", "recommend", "bullets"] {
                assert!(
                    script.contains(required),
                    "shell {shell:?} missing '{required}'"
                );
            }
            for flag in ["paging", "plain"] {
                let expected = if matches!(shell, Shell::Fish) {
                    format!("-l {flag}")
                } else {
                    format!("--{flag}")
                };
                assert!(
                    script.contains(&expected),
                    "shell {shell:?} missing '{expected}'"
                );
            }
        }
        assert!(generate_completions_script(Shell::PowerShell).contains("$prevIndex -lt 1"));
        assert!(generate_completions_script(Shell::Elvish).contains(">= $num-words 3"));
    }

    #[test]
    fn test_cache_clean_reset_db_and_file_url() {
        assert!(file_url(Path::new("/tmp/a b#1.html")).ends_with("file:///tmp/a%20b%231.html"));

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

        handle_cache(&CacheCommand::Clean { reset_db: true }, &cache).unwrap();
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
