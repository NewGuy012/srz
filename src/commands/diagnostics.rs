use crate::cache::Cache;
use crate::commands::{init_llm, print_json};
use crate::config::{Cli, Config, SECS_PER_DAY};
use crate::llm::LlmClient;
use crate::ui::{Ui, sanitize_terminal_text, sanitize_title};
use anyhow::Result;
use std::path::Path;

pub(crate) async fn handle_models(args: &Cli, as_json: bool) -> Result<()> {
    let llm = init_llm(args)?;

    let pb = Ui::spinner("Querying LLM server for available models...");
    let models_result = llm.list_models().await;
    pb.finish_and_clear();

    match models_result {
        Ok(models) => {
            if as_json {
                return print_json(&models);
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
                let active_badge = if m.matches_model_id(&args.model) {
                    " [ACTIVE]"
                } else {
                    ""
                };
                let owner = m
                    .owned_by
                    .as_deref()
                    .map(|o| format!(" ({})", sanitize_title(o)))
                    .unwrap_or_default();
                println!("  • {}{}{}", sanitize_title(&m.id), owner, active_badge);
            }
            println!();
            Ok(())
        }
        Err(err) => {
            Ui::info("Tip: Run `srz doctor` to diagnose server connectivity and ports.");
            Err(err.context("Could not fetch models"))
        }
    }
}

pub(crate) async fn handle_doctor(
    args: &Cli,
    config: &Config,
    config_path: Option<&Path>,
    cache: &Cache,
    as_json: bool,
) -> Result<()> {
    let mut endpoint_ok = false;
    let mut latency_ms = None;
    let mut model_found = false;
    let mut available_models = Vec::new();
    let mut endpoint_err = None;
    let mut server_caps = None;

    match LlmClient::from_cli(args) {
        Ok(llm) => {
            let start = std::time::Instant::now();
            match llm.list_models().await {
                Ok(models) => {
                    endpoint_ok = true;
                    latency_ms = Some(start.elapsed().as_millis() as u64);
                    available_models = models.iter().map(|m| m.id.clone()).collect();
                    model_found = models.iter().any(|m| m.matches_model_id(&args.model));
                    let caps = llm.probe_server_capabilities().await;
                    if cache.dir().is_dir() {
                        cache.set_server_capabilities(&caps);
                    }
                    server_caps = Some(caps);
                }
                Err(e) => endpoint_err = Some(format!("{e:#}")),
            }
        }
        Err(e) => endpoint_err = Some(format!("{e:#}")),
    }

    let hw = crate::system::hardware::HardwareProfile::detect();
    let (db_path, db_ok, db_entries_count, db_err) = diagnose_database(cache);
    let cache_dir = cache.dir();
    let cache_ok = cache_dir.is_dir();
    let cache_stats = cache.stats();
    let policy = cache.policy();

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
                    "pages_ttl_days": policy.pages_ttl.as_secs() / SECS_PER_DAY,
                    "news_ttl_hours": policy.news_ttl.as_secs() / 3600,
                    "search_ttl_days": policy.search_ttl.as_secs() / SECS_PER_DAY,
                    "summaries_ttl_days": policy.summaries_ttl.as_secs() / SECS_PER_DAY,
                    "views_ttl_days": policy.views_ttl.as_secs() / SECS_PER_DAY,
                    "max_budget_bytes": policy.max_bytes,
                },
            },
            "config": {
                "path": config_path,
                "active_profile": args.profile,
                "profiles_configured": config.profiles.keys().collect::<Vec<_>>(),
            }
        });
        return print_json(&report);
    }

    Ui::header("srz System & LLM Health Diagnostics");

    println!("Hardware:");
    print_host_device_info(hw);
    println!(
        "  • Recommended: {} [{} {}] · {} ({})\n",
        hw.recommended_model,
        hw.recommended_size,
        hw.recommended_quant,
        hw.recommended_context_display,
        hw.ram_footprint
    );

    println!("LLM Backend:");
    let safe_base_url = sanitize_terminal_text(&args.base_url);
    let safe_model = sanitize_title(&args.model);
    if endpoint_ok {
        let latency = latency_ms.unwrap_or(0);
        let backend_info = if let Some(caps) = &server_caps {
            format!(
                "{latency}ms latency, {}",
                sanitize_title(&caps.backend_kind)
            )
        } else {
            format!("{latency}ms latency")
        };
        println!("  ✓ Reachable: {safe_base_url} ({backend_info})");

        if model_found {
            println!("  ✓ Model: \"{safe_model}\" loaded and ready");
        } else {
            println!("  ! Model: \"{safe_model}\" not detected on backend");
            if !available_models.is_empty() {
                let safe_models: Vec<String> = available_models
                    .iter()
                    .take(5)
                    .map(|m| sanitize_title(m))
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
            println!("  ✓ Context: {ctx_label} ({} tokens)", caps.n_ctx);
            if caps.n_ctx < hw.recommended_context {
                println!(
                    "  ! Notice: Server context ({ctx_label}) is below recommended {} for single-pass summaries",
                    hw.recommended_context_display
                );
            }
        }
    } else {
        println!("  ✗ Unreachable: {safe_base_url}");
        if let Some(err) = endpoint_err.as_deref() {
            println!("    Error: {}", sanitize_terminal_text(err));
        }
        println!("    Ensure your local LLM runner (llama.cpp, Ollama, LM Studio) is running.");
    }
    println!();

    println!("Embedded Database (redb):");
    let safe_db_path = sanitize_terminal_text(&db_path.display().to_string());
    if db_ok {
        println!("  ✓ Healthy: {safe_db_path} ({db_entries_count} summaries)");
    } else {
        println!("  ✗ Cannot open database at: {safe_db_path}");
        if let Some(err) = &db_err {
            println!("    Reason: {}", sanitize_terminal_text(err));
        }
    }
    println!();

    println!("Cache & Storage:");
    let safe_cache_dir = sanitize_terminal_text(&cache_dir.display().to_string());
    if cache_ok {
        println!(
            "  ✓ Directory: {safe_cache_dir} ({}, {} files)",
            Cache::format_bytes(cache_stats.total_bytes),
            cache_stats.total_files
        );
        println!(
            "  ✓ Retention: {}d pages · {}h news · {}d search · {}d summaries (max {})",
            policy.pages_ttl.as_secs() / SECS_PER_DAY,
            policy.news_ttl.as_secs() / 3600,
            policy.search_ttl.as_secs() / SECS_PER_DAY,
            policy.summaries_ttl.as_secs() / SECS_PER_DAY,
            Cache::format_bytes(policy.max_bytes)
        );
    } else {
        println!("  ✗ Directory unavailable or not yet initialized: {safe_cache_dir}");
    }
    println!();

    println!("Configuration:");
    if let Some(cp) = config_path {
        println!(
            "  ✓ Active config: {}",
            sanitize_terminal_text(&cp.display().to_string())
        );
    } else {
        println!(
            "  • Using defaults (no config at {})",
            sanitize_terminal_text(&Config::default_config_path().display().to_string())
        );
    }
    if let Some(p) = &args.profile {
        println!("  • Active profile: {}", sanitize_title(p));
    }
    println!();

    Ok(())
}

fn diagnose_database(cache: &Cache) -> (std::path::PathBuf, bool, i64, Option<String>) {
    let db_path = cache.db_path().to_path_buf();
    let (db_ok, db_entries_count, db_err) = match cache.open_database() {
        Ok(db) => match db.get_stats() {
            Ok(stats) => (true, stats.total_summaries, None),
            Err(e) => (false, 0, Some(format!("Failed to retrieve stats: {e:#}"))),
        },
        Err(e) => (false, 0, Some(format!("{e:#}"))),
    };
    (db_path, db_ok, db_entries_count, db_err)
}

fn print_host_device_info(hw: &crate::system::hardware::HardwareProfile) {
    println!("  • Device: {}", sanitize_title(&hw.chip_name));
    println!(
        "  • Memory: {:.1} GB RAM ({})",
        hw.total_memory_gb,
        hw.tier.display_name()
    );
}

pub(crate) fn handle_recommend(as_json: bool) -> Result<()> {
    let hw = crate::system::hardware::HardwareProfile::detect();

    if as_json {
        return print_json(&hw.to_recommend_report_json());
    }

    Ui::header("srz Recommended Model & Context Length");

    println!("Host Machine:");
    print_host_device_info(hw);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::ServerCapabilities;

    #[test]
    fn test_diagnose_database_reuses_handle_and_works_without_cache() {
        for cache_enabled in [true, false] {
            let tmp = crate::fs_util::tempdir().unwrap();
            let cache = Cache::with_dir(tmp.path().to_path_buf(), cache_enabled);
            if cache_enabled {
                cache.set_server_capabilities(&ServerCapabilities {
                    endpoint: "http://localhost:9931/v1".into(),
                    model_id: "gemma-4".into(),
                    n_ctx: 65536,
                    backend_kind: "llama.cpp".into(),
                });
            }
            let (db_path, db_ok, count, err) = diagnose_database(&cache);
            assert_eq!(db_path, cache.db_path());
            assert!(db_ok, "cache_enabled={cache_enabled}, err={err:?}");
            assert_eq!(count, 0);
        }
    }

    #[test]
    fn test_diagnose_database_reports_external_lock_conflict() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let cache = Cache::with_dir(tmp.path().to_path_buf(), true);
        let _external_hold = crate::db::Database::open_at(cache.db_path()).unwrap();

        let (_db_path, db_ok, _count, err) = diagnose_database(&cache);
        assert!(!db_ok);
        let err_msg = err.expect("expected lock error message");
        assert!(err_msg.contains("DatabaseAlreadyOpen"), "{err_msg}");
    }
}
