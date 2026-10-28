use anyhow::{Context, Result};
use clap::{CommandFactory, FromArgMatches};
use srz::cache::{Cache, CachedSummary, SetBookSummaryParams};
use srz::cli::{CacheCommand, Cli, ConfigSubcommand, ResolvedAction};
use srz::config::Config;
use srz::db::{Database, NewSummaryRecord};
use srz::llm::{self, LlmClient, LlmOutput, StreamMode};
use srz::render;
use srz::scraper::{ScrapedPage, WebScraper};
use srz::search::{BookSearcher, SearchResult};
use srz::ui::Ui;
use serde::Serialize;
use std::fs;
use std::io::{self, IsTerminal, Write};

#[derive(Serialize)]
struct SummaryOutcome {
    title: String,
    author: Option<String>,
    url: Option<String>,
    target_type: &'static str,
    summary: String,
    word_count: usize,
    streamed_to_stdout: bool,
    prompt_tokens: Option<u32>,
    completion_tokens: Option<u32>,
    duration_ms: u64,
    tokens_per_second: Option<f64>,
    cached: bool,
}

impl SummaryOutcome {
    fn from_llm(
        target_type: &'static str,
        title: String,
        author: Option<String>,
        url: Option<String>,
        word_count: usize,
        output: LlmOutput,
    ) -> Self {
        if let Some(warn) = &output.warning {
            Ui::warn(warn);
        }
        Self {
            title,
            author,
            url,
            target_type,
            summary: output.content,
            word_count,
            streamed_to_stdout: output.streamed_to_stdout,
            prompt_tokens: output.prompt_tokens,
            completion_tokens: output.completion_tokens,
            duration_ms: output.duration_ms,
            tokens_per_second: output.tokens_per_second,
            cached: false,
        }
    }

    fn from_cached(
        target_type: &'static str,
        cached: CachedSummary,
        author: Option<String>,
        url: Option<String>,
    ) -> Self {
        Self {
            title: cached.title,
            author,
            url,
            target_type,
            summary: cached.summary,
            word_count: 0,
            streamed_to_stdout: false,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 0,
            tokens_per_second: None,
            cached: true,
        }
    }
}

async fn fetch_or_cache_page(
    url: &str,
    scraper: &WebScraper,
    cache: &Cache,
    cache_hit_msg: &str,
    spinner_msg: &'static str,
    done_msg: &str,
) -> Result<ScrapedPage> {
    if let Some(cached_page) = cache.get_page(url) {
        Ui::info(cache_hit_msg);
        return Ok(cached_page);
    }
    let page = Ui::with_spinner(spinner_msg, done_msg, scraper.scrape(url)).await?;
    cache.set_page(url, &page);
    Ok(page)
}

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

    // Handle immediate utility subcommands without initializing scraping, LLM HTTP clients,
    // or opening DuckDB unless the subcommand actually requires database access.
    match action {
        ResolvedAction::Cache(cache_cmd) => {
            handle_cache(cache_cmd, &cache)?;
            return Ok(());
        }
        ResolvedAction::Config(config_cmd) => {
            handle_config(config_cmd, &config, config_path.as_deref(), &args)?;
            return Ok(());
        }
        ResolvedAction::History { limit } => {
            if let Some(db) = open_db_or_warn() {
                handle_history(&db, limit, args.json)?;
            } else {
                Ui::error("DuckDB database unavailable.");
            }
            return Ok(());
        }
        ResolvedAction::Stats => {
            if let Some(db) = open_db_or_warn() {
                handle_stats(&db, args.json)?;
            } else {
                Ui::error("DuckDB database unavailable.");
            }
            return Ok(());
        }
        ResolvedAction::Find { query } => {
            if let Some(db) = open_db_or_warn() {
                handle_find(&db, query, &cache, args.json)?;
            } else {
                Ui::error("DuckDB database unavailable.");
            }
            return Ok(());
        }
        ResolvedAction::Models => {
            handle_models(&args, args.json).await?;
            return Ok(());
        }
        ResolvedAction::Doctor => {
            handle_doctor(&args, &config, config_path.as_deref(), &cache, args.json).await?;
            return Ok(());
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
        _ => {}
    }

    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    }

    let llm = match LlmClient::new(&args.base_url, &args.model, &args.api_key, args.max_tokens) {
        Ok(client) => client,
        Err(err) => {
            Ui::error(&format!("Failed to initialize LLM client: {}", err));
            std::process::exit(1);
        }
    };

    let scraper = match WebScraper::new() {
        Ok(s) => s,
        Err(err) => {
            Ui::error(&format!("Failed to initialize scraper: {}", err));
            std::process::exit(1);
        }
    };

    let summary_result = match action {
        ResolvedAction::Web { url } => handle_web(url, &scraper, &llm, &cache, &args).await,
        ResolvedAction::File { path } => handle_file(&path, &scraper, &llm, &cache, &args).await,
        ResolvedAction::Stdin => handle_stdin(&scraper, &llm, &args).await,
        ResolvedAction::Book {
            title,
            author,
            no_search,
        } => {
            handle_book(
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
        _ => unreachable!(),
    };

    match summary_result {
        Ok(outcome) => {
            if args.json {
                println!("{}", serde_json::to_string_pretty(&outcome)?);
            } else if let Some(output_path) = &args.output {
                if render::is_html_path(output_path) {
                    let html_doc = render::render_html_document(&outcome.title, &outcome.summary);
                    fs::write(output_path, html_doc)
                        .with_context(|| format!("Failed to write output to {:?}", output_path))?;
                } else {
                    fs::write(output_path, &outcome.summary)
                        .with_context(|| format!("Failed to write output to {:?}", output_path))?;
                }

                Ui::success(&format!(
                    "Summary successfully saved to {}",
                    output_path.display()
                ));
            } else if !outcome.streamed_to_stdout {
                Ui::header(&format!("Summary: {}", outcome.title));
                println!("{}\n", outcome.summary);
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
            }

            // Record newly generated summaries in DuckDB history & library (lazily opened)
            if !outcome.cached
                && let Some(db) = open_db_or_warn()
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
            }
        }
        Err(err) => {
            Ui::error(&format!("Summarization failed: {:#}", err));
            std::process::exit(1);
        }
    }

    Ok(())
}

fn open_db_or_warn() -> Option<Database> {
    match Database::open_default() {
        Ok(d) => Some(d),
        Err(err) => {
            Ui::warn(&format!("Could not open DuckDB database: {}", err));
            None
        }
    }
}

fn target_icon(target_type: &str) -> &'static str {
    match target_type {
        "book" => "📚",
        "file" => "📄",
        "stdin" => "⌨️",
        _ => "🌐",
    }
}

fn handle_history(db: &Database, limit: usize, as_json: bool) -> Result<()> {
    let entries = db.get_history(limit)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
        return Ok(());
    }
    if entries.is_empty() {
        Ui::info("No summarization history recorded yet. Summarize a book or webpage first!");
        return Ok(());
    }

    Ui::header(&format!("Summarization History (last {} entries)", entries.len()));
    for entry in &entries {
        let icon = target_icon(&entry.target_type);
        let author_str = entry
            .author
            .as_ref()
            .map(|a| format!(" by {}", a))
            .unwrap_or_default();

        println!("[#{}] {} \"{}\"{}", entry.id, icon, entry.title, author_str);
        println!(
            "     Words: {} | Tokens: {} prompt / {} output | Speed: {:.1} tok/s | Time: {:.2}s",
            entry.word_count,
            entry.prompt_tokens.unwrap_or(0),
            entry.completion_tokens.unwrap_or(0),
            entry.tokens_per_second.unwrap_or(0.0),
            entry.duration_ms as f64 / 1000.0
        );
        println!("     Date: {} | Style: {} | Model: {}\n", entry.created_at, entry.style, entry.model);
    }

    Ok(())
}

fn handle_stats(db: &Database, as_json: bool) -> Result<()> {
    let stats = db.get_stats()?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
        return Ok(());
    }
    if stats.total_summaries == 0 {
        Ui::info("No library statistics available yet. Run a summary to begin tracking!");
        return Ok(());
    }

    Ui::header("Library & LLM Performance Statistics (DuckDB)");
    println!(
        "  Total Summaries:     {} ({} books, {} web articles, {} files, {} stdin)",
        stats.total_summaries,
        stats.book_summaries,
        stats.web_summaries,
        stats.file_summaries,
        stats.stdin_summaries
    );
    println!("  Source Words Read:   {} words", stats.total_source_words);
    println!("  Tokens Processed:    {} prompt tokens", stats.total_prompt_tokens);
    println!("  Tokens Generated:    {} completion tokens", stats.total_completion_tokens);
    println!(
        "  Average Speed:       {:.1} tok/s (peak: {:.1} tok/s)",
        stats.avg_tokens_per_second, stats.max_tokens_per_second
    );
    println!("  Total Compute Time:  {:.1}s\n", stats.total_duration_ms as f64 / 1000.0);

    Ok(())
}

fn handle_find(db: &Database, query: &str, cache: &Cache, as_json: bool) -> Result<()> {
    let results = db.search(query)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    if results.is_empty() {
        Ui::info(&format!("No summaries found in library matching '{}'.", query));
        return Ok(());
    }

    Ui::header(&format!("Search Results for \"{}\" ({} match(es))", query, results.len()));
    for (i, res) in results.iter().enumerate() {
        let icon = target_icon(&res.target_type);
        let author_str = res
            .author
            .as_ref()
            .map(|a| format!(" by {}", a))
            .unwrap_or_default();

        println!("[#{}] {} \"{}\"{} ({})", i + 1, icon, res.title, author_str, res.created_at);
        println!("     \"{}\"", res.snippet);
        let slug = render::slugify(&res.title);
        let view_path = cache.views_dir().join(format!("{}.html", slug));
        if view_path.exists() {
            println!("     View: file://{}", view_path.display());
        }
        println!();
    }

    Ok(())
}

fn handle_cache(cmd: CacheCommand, cache: &Cache) -> Result<()> {
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

fn handle_config(
    subcmd: ConfigSubcommand,
    config: &Config,
    config_path: Option<&std::path::Path>,
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
                    fs::create_dir_all(parent)?;
                }
                fs::write(&default_path, Config::default_template())?;
                Ui::success(&format!(
                    "Initialized config file template at {}",
                    default_path.display()
                ));
            }
        }
    }
    Ok(())
}

async fn handle_models(args: &Cli, as_json: bool) -> Result<()> {
    let llm = match LlmClient::new(&args.base_url, &args.model, &args.api_key, args.max_tokens) {
        Ok(c) => c,
        Err(err) => {
            Ui::error(&format!("Failed to initialize LLM client: {}", err));
            std::process::exit(1);
        }
    };

    let pb = Ui::spinner("Querying LLM server for available models...");
    let models_result = llm.list_models().await;
    pb.finish_and_clear();

    match models_result {
        Ok(models) => {
            if as_json {
                println!("{}", serde_json::to_string_pretty(&models)?);
                return Ok(());
            }
            if models.is_empty() {
                Ui::info(&format!(
                    "Backend at {} reported 0 loaded models.",
                    llm.base_url()
                ));
                return Ok(());
            }

            Ui::header(&format!(
                "Available Models at {} ({} found)",
                llm.base_url(),
                models.len()
            ));
            for m in &models {
                let is_active = m.id == args.model;
                let active_badge = if is_active { " [ACTIVE]" } else { "" };
                let owner = m
                    .owned_by
                    .as_deref()
                    .map(|o| format!(" ({})", o))
                    .unwrap_or_default();
                println!("  • {}{}{}", m.id, owner, active_badge);
            }
            println!();
            Ok(())
        }
        Err(err) => {
            Ui::error(&format!("Could not fetch models: {:#}", err));
            Ui::info("Tip: Run `srz doctor` to diagnose server connectivity and ports.");
            std::process::exit(1);
        }
    }
}

async fn handle_doctor(
    args: &Cli,
    config: &Config,
    config_path: Option<&std::path::Path>,
    cache: &Cache,
    as_json: bool,
) -> Result<()> {
    // 1. LLM connectivity & latency
    let llm_init = LlmClient::new(&args.base_url, &args.model, &args.api_key, args.max_tokens);
    let mut endpoint_ok = false;
    let mut latency_ms = None;
    let mut model_found = false;
    let mut available_models = Vec::new();
    let mut endpoint_err = None;

    if let Ok(llm) = &llm_init {
        let start = std::time::Instant::now();
        match llm.list_models().await {
            Ok(models) => {
                endpoint_ok = true;
                latency_ms = Some(start.elapsed().as_millis() as u64);
                available_models = models.iter().map(|m| m.id.clone()).collect();
                model_found = available_models.contains(&args.model);
            }
            Err(e) => {
                endpoint_err = Some(e.to_string());
            }
        }
    }

    // 2. DuckDB database check
    let db_path = Cache::default_cache_dir().join("srz.duckdb");
    let (db_ok, db_entries_count) = match Database::open_default() {
        Ok(db) => match db.get_stats() {
            Ok(stats) => (true, stats.total_summaries),
            Err(_) => (false, 0),
        },
        Err(_) => (false, 0),
    };

    // 3. Cache directory check
    let cache_dir = cache.dir().to_path_buf();
    let cache_ok = cache_dir.exists() || std::fs::create_dir_all(&cache_dir).is_ok();
    let cache_stats = cache.stats();

    if as_json {
        let report = serde_json::json!({
            "llm": {
                "endpoint": args.base_url,
                "reachable": endpoint_ok,
                "latency_ms": latency_ms,
                "configured_model": args.model,
                "model_available": model_found,
                "available_models": available_models,
                "error": endpoint_err,
            },
            "database": {
                "path": db_path,
                "healthy": db_ok,
                "total_summaries": db_entries_count,
            },
            "cache": {
                "dir": cache_dir,
                "healthy": cache_ok,
                "files_count": cache_stats.total_files,
                "size_bytes": cache_stats.total_bytes,
            },
            "config": {
                "path": config_path,
                "active_profile": args.profile,
                "profiles_configured": config.profiles.keys().collect::<Vec<_>>(),
            }
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    Ui::header("srz System & LLM Health Diagnostics");

    // Print LLM status
    println!("LLM Backend:");
    if endpoint_ok {
        println!(
            "  ✓ Reachable: {} (latency: {}ms)",
            args.base_url,
            latency_ms.unwrap_or(0)
        );
        if model_found {
            println!("  ✓ Configured Model: \"{}\" is loaded and ready", args.model);
        } else {
            println!(
                "  ! Configured Model: \"{}\" was not reported by /models endpoint.",
                args.model
            );
            if !available_models.is_empty() {
                println!("    Available models on backend: {}", available_models.join(", "));
            }
        }
    } else {
        println!("  ✗ Endpoint Unreachable: {}", args.base_url);
        if let Some(err) = endpoint_err {
            println!("    Reason: {}", err);
        }
        println!("    Check that llama.cpp / Ollama / LM Studio is running and listening.");
    }
    println!();

    // Print DuckDB status
    println!("DuckDB Embedded Database:");
    if db_ok {
        println!(
            "  ✓ Healthy at: {} (tracking {} past summaries)",
            db_path.display(),
            db_entries_count
        );
    } else {
        println!("  ✗ Could not open database at: {}", db_path.display());
    }
    println!();

    // Print Cache status
    println!("Cache & Storage:");
    if cache_ok {
        println!(
            "  ✓ Storage directory: {} ({} in {} files)",
            cache_dir.display(),
            Cache::format_bytes(cache_stats.total_bytes),
            cache_stats.total_files
        );
    } else {
        println!(
            "  ✗ Unable to access or create cache directory at: {}",
            cache_dir.display()
        );
    }
    println!();

    // Print Config status
    println!("Configuration:");
    if let Some(cp) = config_path {
        println!("  ✓ Active config file: {}", cp.display());
    } else {
        println!(
            "  • No config file found (using defaults at {})",
            Config::default_config_path().display()
        );
    }
    if let Some(p) = &args.profile {
        println!("  • Active profile: {}", p);
    }
    println!();

    Ok(())
}

async fn handle_file(
    path: &std::path::Path,
    scraper: &WebScraper,
    llm: &LlmClient,
    _cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    if !path.exists() {
        anyhow::bail!("File not found: {:?}", path);
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("Failed to read file: {:?}", path))?;

    let (content, word_count, title) = if render::is_html_path(path) {
        let norm = scraper.normalize(&path.display().to_string(), &raw)?;
        let t = norm.title.unwrap_or_else(|| {
            path.file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "Local File".to_string())
        });
        (norm.content, norm.word_count, t)
    } else {
        let words = raw.split_whitespace().count();
        let t = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "Local File".to_string());
        (raw, words, t)
    };

    if content.trim().is_empty() {
        anyhow::bail!("File {:?} is empty.", path);
    }

    Ui::info(&format!(
        "Loaded {} words from file '{}'",
        word_count,
        path.display()
    ));

    let mode = args.stream_mode(&title);
    let path_str = path.display().to_string();
    let output = llm
        .summarize_document_counted(
            &title,
            Some(&path_str),
            &content,
            Some(word_count),
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        "file",
        title,
        None,
        Some(path_str),
        word_count,
        output,
    ))
}

async fn handle_stdin(
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<SummaryOutcome> {
    use std::io::Read;
    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .context("Failed to read from standard input")?;

    if raw.trim().is_empty() {
        anyhow::bail!("Standard input was empty.");
    }

    let is_html = raw.contains("<!DOCTYPE") || raw.contains("<html") || raw.contains("<body");
    let (content, word_count, title) = if is_html {
        if let Ok(norm) = scraper.normalize("stdin", &raw) {
            let t = norm.title.unwrap_or_else(|| "Standard Input".to_string());
            (norm.content, norm.word_count, t)
        } else {
            let words = raw.split_whitespace().count();
            (raw, words, "Standard Input".to_string())
        }
    } else {
        let words = raw.split_whitespace().count();
        (raw, words, "Standard Input".to_string())
    };

    Ui::info(&format!("Read {} words from standard input", word_count));

    let mode = args.stream_mode(&title);
    let output = llm
        .summarize_document_counted(
            &title,
            Some("stdin"),
            &content,
            Some(word_count),
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        "stdin",
        title,
        None,
        None,
        word_count,
        output,
    ))
}

async fn handle_web(
    url: &str,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    // 1. Check for a cached summary matching URL + style + model
    if let Some(cached) = cache.get_web_summary(url, args.style, &args.model) {
        Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            "web",
            cached,
            None,
            Some(url.to_string()),
        ));
    }

    // 2. Check for a cached scraped page to avoid redundant network requests
    let page = fetch_or_cache_page(
        url,
        scraper,
        cache,
        "Loaded webpage content from local cache.",
        "Scraping and normalizing webpage content...",
        "Webpage content scraped and normalized.",
    )
    .await?;

    let title_display = page.title.clone().unwrap_or_else(|| url.to_string());

    Ui::info(&format!(
        "Extracted {} words from '{}' ({})",
        page.word_count, title_display, page.url
    ));

    let mode = args.stream_mode(&title_display);
    let output = llm
        .summarize_webpage(
            page.title.as_deref(),
            url,
            &page.content,
            page.word_count,
            args.style,
            mode,
        )
        .await?;

    if !output.truncated {
        cache.set_web_summary(
            url,
            args.style,
            &args.model,
            &title_display,
            &output.content,
        );
    }

    Ok(SummaryOutcome::from_llm(
        "web",
        title_display,
        None,
        Some(url.to_string()),
        page.word_count,
        output,
    ))
}

async fn handle_book(
    title: &str,
    author: Option<&str>,
    no_search: bool,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let book_display = match author {
        Some(auth) => format!("\"{}\" by {}", title, auth),
        None => format!("\"{}\"", title),
    };

    // 1. Check for a cached summary matching title + author + style + model + search mode
    if let Some(cached) =
        cache.get_book_summary(title, author, args.style, &args.model, no_search)
    {
        Ui::info("Loaded book summary from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            "book",
            cached,
            author.map(|a| a.to_string()),
            None,
        ));
    }

    let mode = args.stream_mode(&book_display);
    let query = BookQuery {
        title,
        author,
        no_search,
    };

    // 2. Generate summary (via grounded web search or LLM memory fallback)
    let (output, word_count, source_url) =
        generate_book_summary(&query, scraper, llm, cache, args, mode).await?;

    // 3. Cache only complete, non-truncated summaries (zero-copy borrow of title & summary)
    if !output.truncated {
        cache.set_book_summary(SetBookSummaryParams {
            title,
            author,
            style: args.style,
            model: &args.model,
            no_search,
            display_title: &book_display,
            summary: &output.content,
        });
    }

    Ok(SummaryOutcome::from_llm(
        "book",
        book_display,
        author.map(|a| a.to_string()),
        source_url,
        word_count,
        output,
    ))
}

struct BookQuery<'a> {
    title: &'a str,
    author: Option<&'a str>,
    no_search: bool,
}

async fn resolve_book_identity(
    title: &str,
    author: Option<&str>,
    cache: &Cache,
    searcher: &mut Option<BookSearcher>,
) -> (String, Option<String>) {
    if let Some(auth) = author {
        return (title.to_string(), Some(auth.to_string()));
    }

    let meta_opt = if let Some(cached_meta) = cache.get_book_metadata(title, None) {
        cached_meta
    } else if let Ok(s) = BookSearcher::new() {
        let fetched = s.resolve_book_metadata(title, None).await;
        cache.set_book_metadata(title, None, &fetched);
        *searcher = Some(s);
        fetched
    } else {
        None
    };

    if let Some(meta) = meta_opt {
        let year_str = meta
            .first_publish_year
            .map(|y| format!(" ({})", y))
            .unwrap_or_default();
        if let Some(ref a) = meta.author {
            Ui::info(&format!(
                "Identified book: \"{}\" by {}{}",
                meta.canonical_title, a, year_str
            ));
        }
        (meta.canonical_title, meta.author)
    } else {
        (title.to_string(), None)
    }
}

async fn fetch_or_cache_candidates(
    effective_title: &str,
    effective_author: Option<&str>,
    cache: &Cache,
    searcher: &mut Option<BookSearcher>,
) -> Result<Vec<SearchResult>> {
    if let Some(cached_candidates) = cache.get_search_results(effective_title, effective_author) {
        Ui::info(&format!(
            "Loaded {} search candidate(s) from local cache.",
            cached_candidates.len()
        ));
        return Ok(cached_candidates);
    }

    let spinner = Ui::spinner("Searching web for book synopsis (Wikipedia / DuckDuckGo)...");
    let active_searcher = match searcher {
        Some(s) => s,
        None => searcher.insert(BookSearcher::new()?),
    };
    let results = active_searcher
        .search_candidates(effective_title, effective_author)
        .await;
    if !results.is_empty() {
        Ui::finish_spinner(
            &spinner,
            &format!("Found {} search candidate(s) online.", results.len()),
        );
        cache.set_search_results(effective_title, effective_author, &results);
    } else {
        Ui::finish_spinner(&spinner, "No web results found.");
    }
    Ok(results)
}

fn disambiguate_candidates_if_needed(
    effective_title: &str,
    ordered_candidates: &mut Vec<SearchResult>,
    args: &Cli,
) {
    if ordered_candidates.len() <= 1
        || args.yes
        || args.quiet
        || args.json
        || !io::stdin().is_terminal()
    {
        return;
    }

    let top_score = ordered_candidates[0].score;
    let second_score = ordered_candidates[1].score;
    let is_high_confidence = top_score >= 90 && (top_score - second_score) >= 30;
    if is_high_confidence {
        return;
    }

    Ui::header(&format!("Reference Disambiguation for \"{}\"", effective_title));
    eprintln!("Multiple references found online:");
    let display_limit = ordered_candidates.len().min(4);
    for (i, cand) in ordered_candidates[..display_limit].iter().enumerate() {
        let tag = if i == 0 { " [Recommended]" } else { "" };
        eprintln!("  [{}] {}{}", i + 1, cand.title, tag);
        if let Some(snippet) = &cand.snippet {
            eprintln!("      \"{}\"", snippet);
        }
    }
    eprint!("\nSelect reference [1-{}, Enter for 1]: ", display_limit);
    let _ = io::stderr().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok()
        && let Ok(choice) = input.trim().parse::<usize>()
        && (1..=display_limit).contains(&choice)
    {
        let selected = ordered_candidates.remove(choice - 1);
        ordered_candidates.insert(0, selected);
    }
    eprintln!();
}

async fn generate_book_summary(
    query: &BookQuery<'_>,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
    mode: StreamMode<'_>,
) -> Result<(LlmOutput, usize, Option<String>)> {
    if !query.no_search {
        let mut searcher: Option<BookSearcher> = None;
        let (resolved_title, resolved_author) =
            resolve_book_identity(query.title, query.author, cache, &mut searcher).await;

        let effective_title = resolved_title.as_str();
        let effective_author = resolved_author.as_deref().or(query.author);

        let mut candidates = fetch_or_cache_candidates(
            effective_title,
            effective_author,
            cache,
            &mut searcher,
        )
        .await?;

        if !candidates.is_empty() {
            disambiguate_candidates_if_needed(effective_title, &mut candidates, args);

            let mut fallback_snippet = None;
            let mut fallback_url = None;

            for res in &candidates {
                Ui::info(&format!("Attempting reference: {} ({})", res.title, res.url));

                let page_result = fetch_or_cache_page(
                    &res.url,
                    scraper,
                    cache,
                    "Loaded reference page from local cache.",
                    "Scraping reference page...",
                    "Reference page scraped.",
                )
                .await;

                match page_result {
                    Ok(page) => {
                        Ui::info(&format!(
                            "Grounded reference text: {} words (budgeted up to {} words for 32k LLM context)",
                            page.word_count,
                            llm::MAX_CONTEXT_WORDS
                        ));

                        let output = llm
                            .summarize_book_with_context(
                                effective_title,
                                effective_author,
                                &res.url,
                                &page.content,
                                args.style,
                                mode,
                            )
                            .await?;
                        return Ok((output, page.word_count, Some(res.url.clone())));
                    }
                    Err(err) => {
                        Ui::warn(&format!(
                            "Could not scrape {}: {}. Trying next reference...",
                            res.url, err
                        ));
                        if fallback_snippet.is_none() && res.snippet.is_some() {
                            fallback_snippet = res.snippet.clone();
                            fallback_url = Some(res.url.clone());
                        }
                    }
                }
            }

            // If scraping all candidate pages failed, try using search snippet
            if let (Some(snippet), Some(url)) = (fallback_snippet, fallback_url) {
                Ui::info("Using search snippet as grounded context...");
                let word_count = snippet.split_whitespace().count();
                if let Ok(output) = llm
                    .summarize_book_with_context(
                        effective_title,
                        effective_author,
                        &url,
                        &snippet,
                        args.style,
                        mode,
                    )
                    .await
                {
                    return Ok((output, word_count, Some(url)));
                }
            }

            Ui::warn("All online references failed to scrape. Falling back to LLM internal memory.");
        } else {
            Ui::warn("No web synopsis found online. Falling back to LLM trained memory.");
        }
    } else {
        Ui::info("Web search disabled. Using local LLM memory directly.");
    }

    let output = llm
        .summarize_book_from_memory(query.title, query.author, args.style, mode)
        .await?;
    Ok((output, 0, None))
}
