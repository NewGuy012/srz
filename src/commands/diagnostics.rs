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
                Ui::info(
                    "Tip: Load or download a model in your runner (e.g. Llama GUI, Ollama, or llama-server).",
                );
                return Ok(());
            }

            Ui::header(&format!(
                "Available Models at {} ({} found)",
                llm.base_url(),
                models.len()
            ));
            for m in &models {
                let is_active = m.matches_model_id(&args.model);
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
    let mut server_caps = None;

    if let Ok(llm) = &llm_init {
        let start = std::time::Instant::now();
        match llm.list_models().await {
            Ok(models) => {
                endpoint_ok = true;
                latency_ms = Some(start.elapsed().as_millis() as u64);
                available_models = models.iter().map(|m| m.id.clone()).collect();
                model_found = models.iter().any(|m| m.matches_model_id(&args.model));
                let caps = llm.probe_server_capabilities().await;
                cache.set_server_capabilities(&caps);
                server_caps = Some(caps);
            }
            Err(e) => {
                endpoint_err = Some(e.to_string());
            }
        }
    }

    // 2. Hardware profile detection
    let hw = crate::system::hardware::HardwareProfile::detect();

    // 2. DuckDB database check
    let db_path = Database::default_path();
    let (db_ok, db_entries_count, db_err) = match Database::open_default() {
        Ok(db) => match db.get_stats() {
            Ok(stats) => (true, stats.total_summaries, None),
            Err(e) => (false, 0, Some(format!("Failed to retrieve stats: {:#}", e))),
        },
        Err(e) => (false, 0, Some(format!("{:#}", e))),
    };

    // 3. Cache directory check
    let cache_dir = cache.dir().to_path_buf();
    let cache_ok = cache_dir.exists() || crate::fs_util::create_private_dir_all(&cache_dir).is_ok();
    let cache_stats = cache.stats();

    if as_json {
        let report = serde_json::json!({
            "hardware": hw.to_json_value(),
            "llm": {
                "endpoint": args.base_url,
                "reachable": endpoint_ok,
                "latency_ms": latency_ms,
                "configured_model": args.model,
                "model_available": model_found,
                "available_models": available_models,
                "capabilities": server_caps,
                "error": endpoint_err,
            },
            "database": {
                "path": db_path,
                "healthy": db_ok,
                "total_summaries": db_entries_count,
                "error": db_err,
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

    // Print Hardware Profile
    println!("Hardware:");
    println!(
        "  • Device: {} ({:.1} GB RAM, {})",
        hw.chip_name,
        hw.total_memory_gb,
        hw.tier.display_name()
    );
    println!(
        "  • Recommended: {} [{} {}] · {} ({})",
        hw.recommended_model,
        hw.recommended_size,
        hw.recommended_quant,
        hw.recommended_context_display,
        hw.ram_footprint
    );
    println!();

    // Print LLM status
    println!("LLM Backend:");
    let safe_base_url = crate::ui::sanitize_terminal_text(&args.base_url);
    let safe_model = crate::ui::sanitize_title(&args.model);
    if endpoint_ok {
        let latency = latency_ms.unwrap_or(0);
        let backend_info = if let Some(caps) = &server_caps {
            let safe_kind = crate::ui::sanitize_title(&caps.backend_kind);
            format!("{}ms latency, {}", latency, safe_kind)
        } else {
            format!("{}ms latency", latency)
        };
        println!("  ✓ Reachable: {} ({})", safe_base_url, backend_info);

        if model_found {
            println!("  ✓ Model: \"{}\" loaded and ready", safe_model);
        } else {
            println!("  ! Model: \"{}\" not detected on backend", safe_model);
            if !available_models.is_empty() {
                let safe_models: Vec<String> = available_models
                    .iter()
                    .take(5)
                    .map(|m| crate::ui::sanitize_title(m))
                    .collect();
                let suffix = if available_models.len() > 5 {
                    format!(" (+{} more)", available_models.len() - 5)
                } else {
                    String::new()
                };
                println!("    Available: {}{}", safe_models.join(", "), suffix);
            } else {
                println!("    No models are currently loaded on this backend.");
            }
        }

        if let Some(caps) = &server_caps {
            let ctx_label = crate::system::hardware::format_context_tokens(caps.n_ctx);
            let tok_label = if caps.supports_tokenize {
                "native tokenizer"
            } else {
                "fallback tokenizer"
            };
            println!(
                "  ✓ Context: {} ({} tokens) · {} slot(s) · {}",
                ctx_label, caps.n_ctx, caps.total_slots, tok_label
            );

            if caps.n_ctx < hw.recommended_context {
                println!(
                    "  ! Notice: Server context ({}) is below recommended {} for single-pass summaries",
                    ctx_label, hw.recommended_context_display
                );
            }
        }
    } else {
        println!("  ✗ Unreachable: {}", safe_base_url);
        if let Some(err) = &endpoint_err {
            let safe_err = crate::ui::sanitize_terminal_text(err);
            if safe_err.contains("Server returned HTTP") {
                println!("    Error: {}", safe_err);
            } else {
                println!(
                    "    Ensure your local LLM runner (llama.cpp, Ollama, LM Studio) is running."
                );
            }
        } else {
            println!("    Ensure your local LLM runner (llama.cpp, Ollama, LM Studio) is running.");
        }
    }
    println!();

    // Print DuckDB status
    println!("DuckDB Database:");
    if db_ok {
        println!(
            "  ✓ Healthy: {} ({} summaries)",
            db_path.display(),
            db_entries_count
        );
    } else {
        println!("  ✗ Cannot open database at: {}", db_path.display());
        if let Some(err) = &db_err {
            println!("    Reason: {}", err);
        }
    }
    println!();

    // Print Cache status
    println!("Cache & Storage:");
    if cache_ok {
        println!(
            "  ✓ Directory: {} ({}, {} files)",
            cache_dir.display(),
            Cache::format_bytes(cache_stats.total_bytes),
            cache_stats.total_files
        );
        println!(
            "  ✓ Retention: {}d pages · {}d search · {}d summaries (max {})",
            cache.policy().pages_ttl.as_secs() / 86400,
            cache.policy().search_ttl.as_secs() / 86400,
            cache.policy().summaries_ttl.as_secs() / 86400,
            Cache::format_bytes(cache.policy().max_bytes)
        );
    } else {
        println!("  ✗ Inaccessible directory: {}", cache_dir.display());
    }
    println!();

    // Print Config status
    println!("Configuration:");
    if let Some(cp) = config_path {
        println!("  ✓ Active config: {}", cp.display());
    } else {
        println!(
            "  • Using defaults (no config at {})",
            Config::default_config_path().display()
        );
    }
    if let Some(p) = &args.profile {
        println!("  • Active profile: {}", crate::ui::sanitize_title(p));
    }
    println!();

    Ok(())
}

fn print_host_device_info(hw: &crate::system::hardware::HardwareProfile) {
    println!("  • Device: {}", hw.chip_name);
    println!(
        "  • Memory: {:.1} GB RAM ({})",
        hw.total_memory_gb,
        hw.tier.display_name()
    );
}

/// Handles the `srz recommend` subcommand, outputting recommended model and context window
/// based on detected host memory and hardware architecture.
pub fn handle_recommend(as_json: bool) -> Result<()> {
    let hw = crate::system::hardware::HardwareProfile::detect();

    if as_json {
        let report = hw.to_recommend_report_json();
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    Ui::header("srz Recommended Model & Context Length");

    println!("Host Machine:");
    print_host_device_info(&hw);
    println!();

    println!("Recommended Local LLM Setup (Llama GUI / llama.cpp):");
    println!("  • Model:         {}", hw.concise_model);
    println!("  • Size:          {}", hw.recommended_size);
    println!("  • Quant:         {}", hw.recommended_quant);
    println!("  • Disk Size:     {}", hw.disk_size);
    println!("  • Context:       {}", hw.recommended_context_display);
    println!("  • Estimated RAM: {}", hw.ram_footprint);
    println!();

    Ok(())
}
