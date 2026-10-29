use anyhow::Result;
use crate::cache::Cache;
use crate::cli::Cli;
use crate::config::Config;
use crate::db::Database;
use crate::llm::LlmClient;
use crate::ui::Ui;
use std::path::Path;

/// Lists available models reported by the local LLM endpoint (`GET /models`).
pub async fn handle_models(args: &Cli, as_json: bool) -> Result<()> {
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

/// Diagnoses LLM connectivity, DuckDB database status, cache storage, and configuration.
pub async fn handle_doctor(
    args: &Cli,
    config: &Config,
    config_path: Option<&Path>,
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
