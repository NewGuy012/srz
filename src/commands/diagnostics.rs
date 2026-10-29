use crate::cache::Cache;
use crate::cli::Cli;
use crate::config::Config;
use crate::db::Database;
use crate::llm::LlmClient;
use crate::ui::Ui;
use anyhow::Result;
use std::path::Path;

/// Lists available models reported by the local LLM endpoint (`GET /models`).
pub async fn handle_models(args: &Cli, as_json: bool) -> Result<()> {
    let llm = crate::commands::init_llm_or_exit(args);

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
                let safe_id = crate::ui::sanitize_title(&m.id);
                let owner = m
                    .owned_by
                    .as_deref()
                    .map(|o| format!(" ({})", crate::ui::sanitize_title(o)))
                    .unwrap_or_default();
                println!("  • {}{}{}", safe_id, owner, active_badge);
            }
            println!();
            Ok(())
        }
        Err(err) => {
            let safe_err = crate::ui::sanitize_terminal_text(&format!("{:#}", err));
            Ui::error(&format!("Could not fetch models: {}", safe_err));
            Ui::info("Tip: Run `srz doctor` to diagnose server connectivity and ports.");
            std::process::exit(1);
        }
    }
}

/// Diagnoses LLM connectivity, DuckDB database status, cache storage, and configuration.
pub async fn handle_doctor(
    args: &Cli,
    config: &Config,
    config_path: Option<&Path>,
    cache: &Cache,
    as_json: bool,
) -> Result<()> {
    // 1. LLM connectivity & latency
    let llm_init = LlmClient::from_cli(args);
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
    let db_path = Database::default_path();
    let (db_ok, db_entries_count) = match Database::open_default() {
        Ok(db) => match db.get_stats() {
            Ok(stats) => (true, stats.total_summaries),
            Err(_) => (false, 0),
        },
        Err(_) => (false, 0),
    };

    // 3. Cache directory check
    let cache_dir = cache.dir().to_path_buf();
    let cache_ok = cache_dir.exists() || crate::fs_util::create_private_dir_all(&cache_dir).is_ok();
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
                "policy": {
                    "pages_ttl_days": cache.policy().pages_ttl.as_secs() / 86400,
                    "search_ttl_days": cache.policy().search_ttl.as_secs() / 86400,
                    "summaries_ttl_days": cache.policy().summaries_ttl.as_secs() / 86400,
                    "views_ttl_days": cache.policy().views_ttl.as_secs() / 86400,
                    "max_budget_bytes": cache.policy().max_bytes,
                },
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
            println!(
                "  ✓ Configured Model: \"{}\" is loaded and ready",
                args.model
            );
        } else {
            println!(
                "  ! Configured Model: \"{}\" was not reported by /models endpoint.",
                args.model
            );
            if !available_models.is_empty() {
                let safe_models: Vec<String> = available_models
                    .iter()
                    .map(|m| crate::ui::sanitize_title(m))
                    .collect();
                println!(
                    "    Available models on backend: {}",
                    safe_models.join(", ")
                );
            }
        }
    } else {
        println!("  ✗ Endpoint Unreachable: {}", args.base_url);
        if let Some(err) = endpoint_err {
            let safe_err = crate::ui::sanitize_terminal_text(&err);
            println!("    Reason: {}", safe_err);
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
        println!(
            "  ✓ Retention policy: {}d pages, {}d search, {}d summaries, {} max budget",
            cache.policy().pages_ttl.as_secs() / 86400,
            cache.policy().search_ttl.as_secs() / 86400,
            cache.policy().summaries_ttl.as_secs() / 86400,
            Cache::format_bytes(cache.policy().max_bytes)
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
