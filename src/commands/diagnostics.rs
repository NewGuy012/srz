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
    let mut server_caps = None;

    if let Ok(llm) = &llm_init {
        let start = std::time::Instant::now();
        match llm.list_models().await {
            Ok(models) => {
                endpoint_ok = true;
                latency_ms = Some(start.elapsed().as_millis() as u64);
                available_models = models.iter().map(|m| m.id.clone()).collect();
                model_found = available_models.contains(&args.model);
                server_caps = Some(llm.probe_server_capabilities().await);
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

    // Print Hardware Profile & Steering
    println!("Hardware Profile & Recommendations:");
    println!(
        "  • Host Device: {} ({:.1} GB RAM / {})",
        hw.chip_name,
        hw.total_memory_gb,
        hw.tier.display_name()
    );
    println!(
        "  • Recommended Family: Google Gemma 4 (High fidelity Q8_0, laptop battery & memory optimized)"
    );
    println!(
        "  • Recommended Model: {} [{} {}]",
        hw.recommended_model, hw.recommended_size, hw.recommended_quant
    );
    println!(
        "  • Recommended Context: {} ({} tokens, {})",
        hw.recommended_context_display, hw.recommended_context, hw.ram_footprint
    );
    println!(
        "  • Llama GUI Setup:     Select {} | {} | {} | {} in Llama app",
        hw.recommended_model,
        hw.recommended_size,
        hw.recommended_quant,
        hw.recommended_context_display
    );
    println!("  • llama.cpp Command:   {}", hw.recommended_launch_command);
    println!("  • Ollama Command:      {}", hw.ollama_launch_command);
    println!();

    // Print LLM status
    println!("LLM Backend:");
    let safe_base_url = crate::ui::sanitize_terminal_text(&args.base_url);
    let safe_model = crate::ui::sanitize_title(&args.model);
    if endpoint_ok {
        println!(
            "  ✓ Reachable: {} (latency: {}ms)",
            safe_base_url,
            latency_ms.unwrap_or(0)
        );
        if model_found {
            println!(
                "  ✓ Configured Model: \"{}\" is loaded and ready",
                safe_model
            );
        } else {
            println!(
                "  ! Configured Model: \"{}\" was not reported by /models endpoint.",
                safe_model
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
        if let Some(caps) = &server_caps {
            let ctx_label = crate::system::hardware::format_context_tokens(caps.n_ctx);
            println!(
                "  ✓ Context Window (n_ctx): {} tokens ({})",
                caps.n_ctx, ctx_label
            );
            println!(
                "  ✓ Server Concurrency (total_slots): {} slot(s)",
                caps.total_slots
            );
            println!(
                "  ✓ Native Tokenizer: {}",
                if caps.supports_tokenize {
                    "Supported (POST /tokenize enabled, 0% token drift)"
                } else {
                    "Fallback (tiktoken-rs)"
                }
            );
            let safe_kind = crate::ui::sanitize_title(&caps.backend_kind);
            println!("  ✓ Backend Kind: {}", safe_kind);
            if caps.n_ctx >= hw.recommended_context {
                println!(
                    "  ✓ Optimization Status: EXCELLENT (Fully optimized for single-pass summaries)"
                );
            } else {
                println!(
                    "  ! Optimization Status: SUBOPTIMAL (Server context is {} [{}] vs {} [{}] recommended for your machine; set {} in Llama GUI or run: {})",
                    caps.n_ctx,
                    ctx_label,
                    hw.recommended_context,
                    hw.recommended_context_display,
                    hw.recommended_context_display,
                    hw.recommended_launch_command
                );
            }
        }
    } else {
        println!("  ✗ Endpoint Unreachable: {}", safe_base_url);
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

/// Handles the `srz recommend` subcommand, outputting recommended model, context window,
/// and ready launch commands based on detected host memory and hardware architecture.
pub fn handle_recommend(as_json: bool) -> Result<()> {
    let hw = crate::system::hardware::HardwareProfile::detect();
    let one_pass_tokens = hw.one_pass_token_ceiling();
    let one_pass_pages = (one_pass_tokens / 250).max(1);

    if as_json {
        let report = serde_json::json!({
            "hardware": {
                "device": hw.chip_name,
                "total_memory_bytes": hw.total_memory_bytes,
                "total_memory_gb": hw.total_memory_gb,
                "tier": hw.tier.display_name(),
            },
            "recommendations": {
                "family": "Google Gemma 4",
                "model": hw.recommended_model,
                "size": hw.recommended_size,
                "quantization": hw.recommended_quant,
                "context_window": hw.recommended_context,
                "context_display": hw.recommended_context_display,
                "ram_footprint": hw.ram_footprint,
                "one_pass_token_ceiling": one_pass_tokens,
            },
            "launch_commands": {
                "llama_gui": format!("In Llama GUI, select: {}", hw.llama_gui_selection()),
                "llama_server": hw.recommended_launch_command,
                "ollama": hw.ollama_launch_command,
            }
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    Ui::header("srz Recommended Model & Context Length");

    println!("Host Machine:");
    print_host_device_info(&hw);
    println!();

    println!("Recommended Local LLM Setup (Llama GUI / llama.cpp):");
    println!("  • Model:   {}", hw.recommended_model);
    println!("  • Size:    {}", hw.recommended_size);
    println!("  • Quant:   {}", hw.recommended_quant);
    println!(
        "  • Context: {} ({} tokens, {})",
        hw.recommended_context_display, hw.recommended_context, hw.ram_footprint
    );
    println!();

    println!("How to Load in Your Runner:");
    println!("  • Llama GUI (Llama-macOS / Llama-Windows - localhost:9931):");
    println!(
        "      Open Llama GUI and select/download:\n      {}",
        hw.llama_gui_selection()
    );
    println!("  • llama.cpp CLI (headless llama-server):");
    println!("      {}", hw.recommended_launch_command);
    println!("  • Ollama:");
    println!("      {}", hw.ollama_launch_command);
    println!();

    println!("Summary Synthesis Speed:");
    println!(
        "  💡 Fast 1-Pass Summaries: Documents up to ~{}k tokens (~{} pages) synthesize in a single pass.",
        one_pass_tokens / 1000,
        one_pass_pages
    );
    println!(
        "     Larger documents automatically use parallel map-reduce chunking without OOM crashes."
    );
    println!();

    println!("Next Steps:");
    println!(
        "  1. In Llama GUI, select {} ({}), or start your local LLM using a command above.",
        hw.recommended_model, hw.recommended_context_display
    );
    println!("  2. Run `srz config init` to generate a config tailored to your machine.");
    println!("  3. Summarize any target: srz \"<URL, file, or book>\"");
    println!();

    Ok(())
}
