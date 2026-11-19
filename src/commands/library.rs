use crate::cache::Cache;
use crate::commands::{file_url, print_json};
use crate::db::Database;
use crate::render;
use crate::ui::{Ui, sanitize_terminal_text, sanitize_title};
use anyhow::Result;
use std::path::PathBuf;

pub(crate) fn handle_history(db: &Database, limit: usize, as_json: bool) -> Result<()> {
    let entries = db.get_history(limit)?;
    if as_json {
        return print_json(&entries);
    }
    if entries.is_empty() {
        Ui::info("No summarization history recorded yet. Summarize a book or webpage first!");
        return Ok(());
    }

    Ui::header(&format!(
        "Summarization History (last {} entries)",
        entries.len()
    ));
    for entry in &entries {
        let icon = render::target_icon(&entry.target_type);
        let author_str = render::format_author_by(entry.author.as_deref());
        println!(
            "[#{}] {} \"{}\"{}",
            entry.id,
            icon,
            sanitize_title(&entry.title),
            author_str
        );
        println!(
            "     Words: {} | Tokens: {} prompt / {} output | Speed: {:.1} tok/s | Time: {:.2}s",
            entry.word_count,
            entry.prompt_tokens.unwrap_or(0),
            entry.completion_tokens.unwrap_or(0),
            entry.tokens_per_second.unwrap_or(0.0),
            entry.duration_ms as f64 / 1000.0
        );
        println!(
            "     Date: {} | Style: {} | Model: {}\n",
            sanitize_title(&entry.created_at),
            sanitize_title(&entry.style),
            sanitize_terminal_text(&entry.model)
        );
    }

    Ok(())
}

pub(crate) fn handle_stats(db: &Database, as_json: bool) -> Result<()> {
    let stats = db.get_stats()?;
    if as_json {
        return print_json(&stats);
    }
    if stats.total_summaries == 0 {
        Ui::info("No library statistics available yet. Run a summary to begin tracking!");
        return Ok(());
    }

    Ui::header("Library & LLM Performance Telemetry");
    println!(
        "  Total Summaries:     {} ({} books, {} web, {} videos, {} news, {} files, {} stdin)",
        stats.total_summaries,
        stats.book_summaries,
        stats.web_summaries,
        stats.video_summaries,
        stats.news_summaries,
        stats.file_summaries,
        stats.stdin_summaries
    );
    println!("  Source Words Read:   {} words", stats.total_source_words);
    if stats.estimated_minutes_saved > 0.0 {
        if stats.estimated_minutes_saved >= 60.0 {
            println!(
                "  Reading Time Saved:  ~{:.1} hours (based on 225 wpm)",
                stats.estimated_minutes_saved / 60.0
            );
        } else {
            println!(
                "  Reading Time Saved:  ~{:.0} minutes (based on 225 wpm)",
                stats.estimated_minutes_saved
            );
        }
    }
    println!(
        "  Tokens Processed:    {} prompt tokens",
        stats.total_prompt_tokens
    );
    println!(
        "  Tokens Generated:    {} completion tokens",
        stats.total_completion_tokens
    );
    println!(
        "  Generation Speed:    {:.1} tok/s avg (peak: {:.1} tok/s)",
        stats.avg_tokens_per_second, stats.max_tokens_per_second
    );
    println!(
        "  Total Compute Time:  {:.1}s\n",
        stats.total_duration_ms as f64 / 1000.0
    );

    Ui::header("Reading Habits & Activity");
    println!(
        "  Activity Window:     {} today | {} this week | {} this month",
        stats.reads_today, stats.reads_this_week, stats.reads_this_month
    );
    let streak_icon = if stats.current_streak_days > 0 {
        "🔥"
    } else {
        "⏳"
    };
    println!(
        "  Reading Streak:      {} {} day(s) (across {} active days)\n",
        streak_icon, stats.current_streak_days, stats.active_reading_days
    );

    if !stats.models.is_empty() {
        Ui::header("Model Throughput & Latency Breakdown");
        for m in &stats.models {
            println!(
                "  • {:<18} {:>3} runs | avg: {:>5.1} tok/s | p50: {:>5.1} tok/s | p95: {:>5.1} tok/s | avg time: {:>4.1}s",
                sanitize_terminal_text(&m.model),
                m.runs,
                m.avg_tokens_per_second,
                m.p50_tokens_per_second,
                m.p95_tokens_per_second,
                m.avg_duration_ms / 1000.0
            );
        }
        println!();
    }

    if !stats.top_domains.is_empty() {
        Ui::header("Top Reading Sources");
        for d in &stats.top_domains {
            println!(
                "  • {:<24} {:>3} reads ({} words)",
                sanitize_terminal_text(&d.domain),
                d.count,
                d.total_words
            );
        }
        println!();
    }

    Ok(())
}

pub(crate) fn handle_find(db: &Database, query: &str, cache: &Cache, as_json: bool) -> Result<()> {
    let results = db.search(query)?;
    if as_json {
        return print_json(&results);
    }
    let safe_query = sanitize_title(query);
    if results.is_empty() {
        Ui::info(&format!(
            "No summaries found in library matching '{safe_query}'."
        ));
        return Ok(());
    }

    Ui::header(&format!(
        "Search Results for \"{safe_query}\" ({} match(es))",
        results.len()
    ));
    for (i, res) in results.iter().enumerate() {
        let icon = render::target_icon(&res.target_type);
        let author_str = render::format_author_by(res.author.as_deref());
        println!(
            "[#{}] {} \"{}\"{} ({})",
            i + 1,
            icon,
            sanitize_title(&res.title),
            author_str,
            sanitize_title(&res.created_at)
        );
        let safe_snippet = sanitize_terminal_text(&res.snippet);
        if let Some(score) = res.score {
            println!("     Score: {score:.2} | \"{safe_snippet}\"");
        } else {
            println!("     \"{safe_snippet}\"");
        }
        let view_path = cache.html_view_path_for(&res.title, res.url.as_deref());
        if view_path.exists() {
            println!("     View: {}", file_url(&view_path));
        }
        println!();
    }

    Ok(())
}

pub(crate) fn update_index_dashboard(db: &Database, cache: &Cache) -> Result<PathBuf> {
    let stats = db.get_stats()?;
    let entries = db.get_history(usize::MAX)?;
    let index_path = render::ensure_index_file(&cache.views_dir(), &stats, &entries)?;
    Ok(index_path)
}

pub(crate) fn handle_index(db: &Database, cache: &Cache, open_browser: bool) -> Result<()> {
    let index_path = update_index_dashboard(db, cache)?;
    Ui::success(&format!(
        "Library dashboard generated: {}",
        file_url(&index_path)
    ));
    if open_browser {
        if let Err(err) = render::open_in_browser(&index_path) {
            Ui::warn(&format!("Could not open browser: {err}"));
        } else {
            Ui::info("Opened library dashboard in browser.");
        }
    }
    Ok(())
}

pub(crate) fn handle_export(
    db: &Database,
    format: crate::config::ExportFormat,
    output: &std::path::Path,
) -> Result<()> {
    let safe_out = sanitize_terminal_text(&output.display().to_string());
    match format {
        crate::config::ExportFormat::Markdown => {
            let count = db.export_markdown_vault(output)?;
            Ui::success(&format!(
                "Exported {count} markdown summary files to {safe_out}"
            ));
        }
        crate::config::ExportFormat::Json => {
            db.export_json(output)?;
            Ui::success(&format!("Exported library JSON to {safe_out}"));
        }
        crate::config::ExportFormat::Csv => {
            db.export_csv(output)?;
            Ui::success(&format!("Exported library CSV to {safe_out}"));
        }
    }
    Ok(())
}
