#![forbid(unsafe_code)]

use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches};
use srz::cache::Cache;
use srz::cli::{Cli, ResolvedAction};
use srz::commands::{self, with_db};
use srz::config::Config;
use srz::db::NewSummaryRecord;
use srz::render;
use srz::scraper::WebScraper;
use srz::ui::Ui;

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
            Ui::error(msg);
            std::process::exit(1);
        }
    };

    let cache = Cache::new(!args.no_cache);
    cache.maybe_auto_prune();

    // Handle immediate utility subcommands without initializing scraping, LLM HTTP clients,
    // or opening DuckDB unless the subcommand actually requires database access.
    match action {
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
            commands::handle_doctor(&args, &config, config_path.as_deref(), &cache, args.json).await?;
            return Ok(());
        }
        ResolvedAction::Index => {
            return with_db(|db| commands::handle_index(db, &cache, args.open));
        }
        ResolvedAction::Browse => {
            return with_db(|db| srz::ui::run_tui(db, &cache));
        }
        ResolvedAction::Completions { shell } => {
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "srz",
                &mut std::io::stdout(),
            );
            return Ok(());
        }
        ResolvedAction::Upgrade {
            check,
            force,
            version,
            dry_run,
        } => {
            srz::upgrade::run_upgrade(
                check,
                force,
                version.as_deref(),
                dry_run,
                args.json,
            )
            .await?;
            return Ok(());
        }
        _ => {}
    }

    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    }

    let llm = commands::init_llm_or_exit(&args);

    let scraper = match WebScraper::new() {
        Ok(s) => s,
        Err(err) => {
            Ui::error(&format!("Failed to initialize scraper: {}", err));
            std::process::exit(1);
        }
    };

    let summary_result = match action {
        ResolvedAction::Ask {
            target,
            question,
            interactive,
        } => {
            commands::handle_ask(target, question, interactive, &scraper, &llm, &cache, &args).await?;
            return Ok(());
        }
        ResolvedAction::Web { url } => commands::handle_web(url, &scraper, &llm, &cache, &args).await,
        ResolvedAction::File { path } => commands::handle_file(&path, &scraper, &llm, &args).await,
        ResolvedAction::Stdin => commands::handle_stdin(&scraper, &llm, &args).await,
        ResolvedAction::Book {
            title,
            author,
            no_search,
        } => {
            commands::handle_book(
                title,
                author,
                no_search,
                &scraper,
                &llm,
                &cache,
                &args,
            )
            .await
        }
        ResolvedAction::News {
            topic,
            timeframe,
            limit,
        } => commands::handle_news(&topic, timeframe, limit, &scraper, &llm, &args).await,
        _ => unreachable!(),
    };

    match summary_result {
        Ok(outcome) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else if let Some(output_path) = &args.output {
                let content = if render::is_html_path(output_path) {
                    std::borrow::Cow::Owned(render::render_html_document(
                        &outcome.title,
                        &outcome.summary,
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
            } else if !outcome.streamed_to_stdout {
                srz::ui::display_summary(
                    &outcome.title,
                    &outcome.summary,
                    args.effective_paging(),
                    args.plain,
                )?;
            }

            // Always ensure clickable styled HTML and Markdown view files exist (skipping re-render on cache hit)
            if !args.json
                && let Ok(files) = render::ensure_view_files(
                    &cache.views_dir(),
                    &outcome.title,
                    &outcome.summary,
                    !outcome.cached,
                )
            {
                Ui::success(&format!(
                    "Styled HTML view: file://{}",
                    files.html_path.display()
                ));
                Ui::success(&format!(
                    "Markdown file:    file://{}",
                    files.md_path.display()
                ));

                if args.open
                    && let Err(err) = render::open_in_browser(&files.html_path)
                {
                    Ui::warn(&format!("Could not open browser: {}", err));
                }
            }

            // Record newly generated summaries in DuckDB history & library (lazily opened)
            if !outcome.cached
                && let Some(db) = commands::open_db_or_warn()
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
                };
                let _ = db.record_summary(&record);
                let _ = commands::update_index_dashboard(&db, &cache);
            }
        }
        Err(err) => {
            Ui::error(&format!("Summarization failed: {:#}", err));
            std::process::exit(1);
        }
    }

    Ok(())
}
