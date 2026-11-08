#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches};
use srz::cache::Cache;
use srz::cli::{Cli, ResolvedAction};
use srz::commands::{self, SummaryOutcome, with_db};
use srz::config::Config;
use srz::db::NewSummaryRecord;
use srz::render;
use srz::scraper::WebScraper;
use srz::ui::Ui;
use std::io::IsTerminal;

#[tokio::main]
async fn main() -> Result<()> {
    let cmd = Cli::command();
    let matches = cmd.get_matches();
    let mut args = match Cli::from_arg_matches(&matches) {
        Ok(args) => args,
        Err(err) => err.exit(),
    };

    let (config, config_path) = match &args.config {
        Some(p) => {
            let cfg = Config::load_from_path(p)?;
            (cfg, Some(p.clone()))
        }
        None => Config::load_default(),
    };
    args.apply_config(&config, Some(&matches));

    if args.quiet || args.json {
        Ui::set_quiet(true);
    }
    if args.verbose {
        Ui::set_verbose(true);
    }

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
        ResolvedAction::Ask {
            list_chats,
            show_chat,
            delete_chat,
            ..
        } if list_chats || show_chat.is_some() || delete_chat.is_some() => {
            commands::handle_ask_management(list_chats, show_chat, delete_chat, args.json)?;
            return Ok(());
        }
        ResolvedAction::Cache(cache_cmd) => {
            commands::handle_cache(cache_cmd, &cache)?;
            return Ok(());
        }
        ResolvedAction::Config(config_cmd) => {
            commands::handle_config(config_cmd, &config, config_path.as_deref(), &args)?;
            return Ok(());
        }
        ResolvedAction::History { limit } => {
            return with_db(|db| commands::handle_history(db, limit, args.json));
        }
        ResolvedAction::Stats => {
            return with_db(|db| commands::handle_stats(db, args.json));
        }
        ResolvedAction::Find { query } => {
            return with_db(|db| commands::handle_find(db, query, &cache, args.json));
        }
        ResolvedAction::Models => {
            commands::handle_models(&args, args.json).await?;
            return Ok(());
        }
        ResolvedAction::Doctor => {
            commands::handle_doctor(&args, &config, config_path.as_deref(), &cache, args.json)
                .await?;
            return Ok(());
        }
        ResolvedAction::Recommend => {
            commands::handle_recommend(args.json)?;
            return Ok(());
        }
        ResolvedAction::Index => {
            return with_db(|db| commands::handle_index(db, &cache, args.open));
        }
        ResolvedAction::Browse => {
            loop {
                let action = with_db(|db| srz::ui::run_tui(db, &cache))?;
                match action {
                    srz::ui::TuiAction::Exit => break,
                    srz::ui::TuiAction::Ask { target } => {
                        let llm =
                            commands::init_llm_with_capabilities_or_exit(&args, Some(&cache)).await;
                        let scraper = WebScraper::new()?;
                        let _ = commands::handle_ask(
                            Some(target),
                            None,
                            true,
                            false,
                            None,
                            None,
                            None,
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
        ResolvedAction::Query { sql, csv } => {
            return with_db(|db| commands::handle_query(db, &sql, csv, args.json));
        }
        ResolvedAction::Export { format, output } => {
            return with_db(|db| commands::handle_export(db, format, &output));
        }
        ResolvedAction::Completions { shell } => {
            clap_complete::generate(shell, &mut Cli::command(), "srz", &mut std::io::stdout());
            return Ok(());
        }
        ResolvedAction::Upgrade {
            check,
            force,
            version,
        } => {
            srz::upgrade::run_upgrade(check, force, version.as_deref(), args.json).await?;
            return Ok(());
        }
        _ => {}
    }

    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    } else {
        // Sub-millisecond cache fast-path: probe cache before initializing LLM/scraper clients
        match &action {
            ResolvedAction::Web { url } if !srz::cli::is_feed_url(url) => {
                if let Some(cached) = cache.get_web_summary(url, args.style, &args.model) {
                    Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
                    let outcome = SummaryOutcome::from_cached(
                        commands::target_type_for_url(url),
                        cached,
                        None,
                        Some(url.to_string()),
                    );
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            ResolvedAction::Web { url } if srz::cli::is_feed_url(url) => {
                if let Some(cached) = cache.get_news_summary(
                    url,
                    srz::cli::NewsTimeframe::Day,
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
            ResolvedAction::Book { title } => {
                if let Some(cached) = cache.get_book_summary(title, None, args.style, &args.model) {
                    Ui::info(
                        "Loaded book summary from local cache (pass `--no-cache` to refresh).",
                    );
                    let outcome = SummaryOutcome::from_cached("book", cached, None, None);
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            ResolvedAction::File { path } => {
                if let Some(k) = commands::summarize::file_cache_key(path)
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
            ResolvedAction::News {
                topic, timeframe, ..
            } => {
                if let Some(cached) =
                    cache.get_news_summary(topic, *timeframe, args.style, &args.model)
                {
                    Ui::info(
                        "Loaded news briefing from local cache (pass `--no-cache` to refresh).",
                    );
                    let search_url = srz::news::GoogleNewsClient::search_url_for_topic(topic);
                    let outcome =
                        SummaryOutcome::from_cached("news", cached, None, Some(search_url));
                    return present_summary_outcome(outcome, &args, &cache);
                }
            }
            _ => {}
        }
    }

    let scraper = match WebScraper::new() {
        Ok(s) => s,
        Err(err) => {
            Ui::error(&format!("Failed to initialize scraper: {}", err));
            std::process::exit(1);
        }
    };

    let summary_result = match action {
        ResolvedAction::Web { url } if !srz::cli::is_feed_url(&url) => {
            let (llm, page_res) = tokio::join!(
                commands::init_llm_with_capabilities_or_exit(&args, Some(&cache)),
                commands::fetch_url_content(&url, &scraper, &cache, true)
            );
            match page_res {
                Ok(page) => commands::summarize_fetched_web_page(&url, page, &llm, &args).await,
                Err(err) => Err(err),
            }
        }
        other_action => {
            let llm = commands::init_llm_with_capabilities_or_exit(&args, Some(&cache)).await;
            match other_action {
                ResolvedAction::Ask {
                    target,
                    question,
                    interactive,
                    list_chats,
                    resume,
                    show_chat,
                    delete_chat,
                } => {
                    if let Err(err) = commands::handle_ask(
                        target,
                        question,
                        interactive,
                        list_chats,
                        resume,
                        show_chat,
                        delete_chat,
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
                ResolvedAction::Web { url } => {
                    commands::handle_web(&url, &scraper, &llm, &cache, &args).await
                }
                ResolvedAction::File { path } => {
                    commands::handle_file(&path, &scraper, &llm, &cache, &args).await
                }
                ResolvedAction::Stdin => commands::handle_stdin(&scraper, &llm, &args).await,
                ResolvedAction::Book { title } => {
                    commands::handle_book(&title, None, &scraper, &llm, &cache, &args).await
                }
                ResolvedAction::News {
                    topic,
                    timeframe,
                    limit,
                } => {
                    commands::handle_news(&topic, timeframe, limit, &scraper, &llm, &cache, &args)
                        .await
                }
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

fn present_summary_outcome(outcome: SummaryOutcome, args: &Cli, cache: &Cache) -> Result<()> {
    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else if let Some(output_path) = &args.output {
        let content = if render::is_html_path(output_path) {
            std::borrow::Cow::Owned(render::render_html_document_with_reasoning(
                &outcome.title,
                &outcome.summary,
                outcome.reasoning.as_deref(),
            ))
        } else {
            std::borrow::Cow::Borrowed(outcome.summary.as_str())
        };
        srz::fs_util::write_private_file(output_path, content.as_bytes())
            .with_context(|| format!("Failed to write output to {:?}", output_path))?;

        Ui::success(&format!(
            "Summary successfully saved to {}",
            output_path.display()
        ));
    } else {
        let is_tty = std::io::stdout().is_terminal();
        let hint_ref = if !args.plain && !args.quiet {
            outcome.hint.as_deref()
        } else {
            None
        };

        // Only generate rendered HTML and Markdown view files when the user explicitly requests opening in browser (--open)
        let view_files = if args.open {
            render::ensure_view_files_unique(
                &cache.views_dir(),
                &outcome.title,
                &outcome.summary,
                outcome.url.as_deref(),
                outcome.reasoning.as_deref(),
                !outcome.cached,
            )
            .ok()
        } else {
            None
        };

        if args.open
            && let Some(ref files) = view_files
            && let Err(err) = render::open_in_browser(&files.html_path)
        {
            Ui::warn(&format!("Could not open browser: {}", err));
        }

        if outcome.streamed_to_stdout {
            if is_tty {
                if let Some(ref files) = view_files {
                    Ui::success(&format!(
                        "Styled HTML view: file://{}",
                        files.html_path.display()
                    ));
                    Ui::success(&format!(
                        "Markdown file:    file://{}",
                        files.md_path.display()
                    ));
                }
                if let Some(hint) = hint_ref {
                    eprintln!("\n\x1b[2m{}\x1b[0m", hint);
                }
            }
        } else {
            let view_pair = view_files
                .as_ref()
                .map(|f| (f.html_path.as_path(), f.md_path.as_path()));

            srz::ui::display_summary_full(
                &outcome.title,
                &outcome.summary,
                args.effective_paging(),
                args.plain,
                if is_tty { view_pair } else { None },
                if is_tty { hint_ref } else { None },
            )?;

            // When output is piped (!is_tty), print view file notices and hints strictly to stderr
            if !is_tty {
                if let Some(ref files) = view_files {
                    Ui::success(&format!(
                        "Styled HTML view: file://{}",
                        files.html_path.display()
                    ));
                    Ui::success(&format!(
                        "Markdown file:    file://{}",
                        files.md_path.display()
                    ));
                }
                if let Some(hint) = hint_ref {
                    eprintln!("\n{}", hint);
                }
            }
        }
    }

    // Record newly generated summaries in DuckDB history & library (lazily opened)
    if !outcome.cached
        && let Some(db) = cache.open_database().or_else(commands::open_db_or_warn)
    {
        let record = NewSummaryRecord {
            target_type: outcome.target_type,
            title: &outcome.title,
            author: outcome.author.as_deref(),
            url: outcome.url.as_deref(),
            style: args.style.as_str(),
            model: &args.model,
            word_count: outcome.word_count,
            prompt_tokens: outcome.prompt_tokens,
            completion_tokens: outcome.completion_tokens,
            duration_ms: outcome.duration_ms,
            tokens_per_second: outcome.tokens_per_second,
            summary: &outcome.summary,
            reasoning: outcome.reasoning.as_deref(),
        };
        let _ = db.record_summary_with_key(&record, outcome.cache_key.as_deref());
        if args.open {
            let _ = commands::update_index_dashboard(&db, cache);
        }
    }

    // Run opportunistic maintenance off the critical path after summary presentation
    cache.maybe_auto_prune();

    Ok(())
}

fn print_execution_error(action_label: &str, err: &anyhow::Error) {
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
        || srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "model not detected")
        || (srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "model")
            && (srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "not found")
                || srz::system::text_util::contains_ascii_case_insensitive(
                    &err_msg,
                    "not loaded",
                )
                || srz::system::text_util::contains_ascii_case_insensitive(
                    &err_msg,
                    "does not exist",
                )
                || srz::system::text_util::contains_ascii_case_insensitive(
                    &err_msg,
                    "no model is loaded",
                )));

    let is_context_error =
        srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "context length")
            || srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "context window")
            || srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "maximum context")
            || srz::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "prompt is too long",
            );

    let is_oom_error =
        srz::system::text_util::contains_ascii_case_insensitive(&err_msg, "out of memory")
            || srz::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "cuda out of memory",
            )
            || srz::system::text_util::contains_ascii_case_insensitive(
                &err_msg,
                "ggml_metal_graph_compute",
            );

    if is_connection_error {
        let hw = srz::system::hardware::HardwareProfile::detect();
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
        let hw = srz::system::hardware::HardwareProfile::detect();
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
