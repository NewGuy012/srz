use anyhow::Result;
use crate::cache::Cache;
use crate::db::Database;
use crate::render;
use crate::ui::Ui;
use std::path::PathBuf;

/// Displays recent summarization history.
pub fn handle_history(db: &Database, limit: usize, as_json: bool) -> Result<()> {
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
        let icon = render::target_icon(&entry.target_type);
        let author_str = render::format_author_by(entry.author.as_deref());
        let safe_title = crate::ui::sanitize_title(&entry.title);
        let safe_model = crate::ui::sanitize_terminal_text(&entry.model);

        println!("[#{}] {} \"{}\"{}", entry.id, icon, safe_title, author_str);
        println!(
            "     Words: {} | Tokens: {} prompt / {} output | Speed: {:.1} tok/s | Time: {:.2}s",
            entry.word_count,
            entry.prompt_tokens.unwrap_or(0),
            entry.completion_tokens.unwrap_or(0),
            entry.tokens_per_second.unwrap_or(0.0),
            entry.duration_ms as f64 / 1000.0
        );
        println!("     Date: {} | Style: {} | Model: {}\n", entry.created_at, entry.style, safe_model);
    }

    Ok(())
}

/// Displays aggregate library and performance analytics.
pub fn handle_stats(db: &Database, as_json: bool) -> Result<()> {
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
        "  Total Summaries:     {} ({} books, {} web articles, {} videos, {} news digests, {} files, {} stdin)",
        stats.total_summaries,
        stats.book_summaries,
        stats.web_summaries,
        stats.video_summaries,
        stats.news_summaries,
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

/// Searches the summarized library for matching queries.
pub fn handle_find(db: &Database, query: &str, cache: &Cache, as_json: bool) -> Result<()> {
    let results = db.search(query)?;
    if as_json {
        println!("{}", serde_json::to_string_pretty(&results)?);
        return Ok(());
    }
    if results.is_empty() {
        Ui::info(&format!("No summaries found in library matching '{}'.", query));
        return Ok(());
    }

    let safe_query = crate::ui::sanitize_title(query);
    Ui::header(&format!("Search Results for \"{}\" ({} match(es))", safe_query, results.len()));
    for (i, res) in results.iter().enumerate() {
        let icon = render::target_icon(&res.target_type);
        let author_str = render::format_author_by(res.author.as_deref());
        let safe_title = crate::ui::sanitize_title(&res.title);
        let safe_snippet = crate::ui::sanitize_terminal_text(&res.snippet);

        println!("[#{}] {} \"{}\"{} ({})", i + 1, icon, safe_title, author_str, res.created_at);
        if let Some(score) = res.score {
            println!("     Score: {:.2} (BM25) | \"{}\"", score, safe_snippet);
        } else {
            println!("     \"{}\"", safe_snippet);
        }
        let view_path = cache.html_view_path(&res.title);
        if view_path.exists() {
            println!("     View: file://{}", view_path.display());
        }
        println!();
    }

    Ok(())
}

/// Generates or updates the HTML library dashboard in `views_dir`.
pub fn update_index_dashboard(db: &Database, cache: &Cache) -> Result<PathBuf> {
    let stats = db.get_stats()?;
    let entries = db.get_all_for_index()?;
    let index_path = render::ensure_index_file(&cache.views_dir(), &stats, &entries)?;
    Ok(index_path)
}

/// Generates and optionally opens the HTML dashboard in the default browser.
pub fn handle_index(db: &Database, cache: &Cache, open_browser: bool) -> Result<()> {
    let index_path = update_index_dashboard(db, cache)?;
    Ui::success(&format!(
        "Library dashboard generated: file://{}",
        index_path.display()
    ));
    if open_browser {
        if let Err(err) = render::open_in_browser(&index_path) {
            Ui::warn(&format!("Could not open browser: {}", err));
        } else {
            Ui::info("Opened library dashboard in browser.");
        }
    }
    Ok(())
}
