use crate::cache::Cache;
use crate::db::Database;
use crate::render;
use crate::ui::Ui;
use anyhow::Result;
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

    Ui::header(&format!(
        "Summarization History (last {} entries)",
        entries.len()
    ));
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
        let safe_created = crate::ui::sanitize_title(&entry.created_at);
        let safe_style = crate::ui::sanitize_title(&entry.style);
        println!(
            "     Date: {} | Style: {} | Model: {}\n",
            safe_created, safe_style, safe_model
        );
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

    Ui::header("Library & LLM Performance Telemetry (DuckDB)");
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
            let hours = stats.estimated_minutes_saved / 60.0;
            println!(
                "  Reading Time Saved:  ~{:.1} hours (based on 225 wpm)",
                hours
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

    // Reading Habits
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

    // Model Performance Breakdown
    if !stats.models.is_empty() {
        Ui::header("Model Throughput & Latency Breakdown");
        for m in &stats.models {
            let safe_model = crate::ui::sanitize_terminal_text(&m.model);
            println!(
                "  • {:<18} {:>3} runs | avg: {:>5.1} tok/s | p50: {:>5.1} tok/s | p95: {:>5.1} tok/s | avg time: {:>4.1}s",
                safe_model,
                m.runs,
                m.avg_tokens_per_second,
                m.p50_tokens_per_second,
                m.p95_tokens_per_second,
                m.avg_duration_ms / 1000.0
            );
        }
        println!();
    }

    // Top Domains
    if !stats.top_domains.is_empty() {
        Ui::header("Top Reading Sources");
        for d in &stats.top_domains {
            let safe_domain = crate::ui::sanitize_terminal_text(&d.domain);
            println!(
                "  • {:<24} {:>3} reads ({} words)",
                safe_domain, d.count, d.total_words
            );
        }
        println!();
    }

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
        Ui::info(&format!(
            "No summaries found in library matching '{}'.",
            query
        ));
        return Ok(());
    }

    let safe_query = crate::ui::sanitize_title(query);
    Ui::header(&format!(
        "Search Results for \"{}\" ({} match(es))",
        safe_query,
        results.len()
    ));
    for (i, res) in results.iter().enumerate() {
        let icon = render::target_icon(&res.target_type);
        let author_str = render::format_author_by(res.author.as_deref());
        let safe_title = crate::ui::sanitize_title(&res.title);
        let safe_snippet = crate::ui::sanitize_terminal_text(&res.snippet);

        println!(
            "[#{}] {} \"{}\"{} ({})",
            i + 1,
            icon,
            safe_title,
            author_str,
            res.created_at
        );
        if let Some(score) = res.score {
            println!("     Score: {:.2} (BM25) | \"{}\"", score, safe_snippet);
        } else {
            println!("     \"{}\"", safe_snippet);
        }
        let view_path = cache.html_view_path_for(&res.title, res.url.as_deref());
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

/// Formats and sanitizes a cell value for safe RFC 4180 CSV emission,
/// preventing CSV formula injection (CWE-1236) and properly escaping quotes, commas, and newlines.
pub fn format_csv_cell(cell: &str) -> String {
    // Neutralize formula injection on non-numeric strings
    let is_formula = cell.starts_with(['=', '+', '-', '@'])
        && cell.trim_start_matches(['+', '-']).parse::<f64>().is_err();

    let safe_cell = if is_formula {
        format!("'{}", cell)
    } else {
        cell.to_string()
    };

    if safe_cell.contains(',')
        || safe_cell.contains('"')
        || safe_cell.contains('\n')
        || safe_cell.contains('\r')
    {
        format!("\"{}\"", safe_cell.replace('"', "\"\""))
    } else {
        safe_cell
    }
}

/// Handles the `srz query` command to execute arbitrary SQL on DuckDB.
pub fn handle_query(db: &Database, sql: &str, csv: bool, as_json: bool) -> Result<()> {
    let res = db.query_raw(sql)?;
    if as_json {
        let objects: Vec<serde_json::Value> = res
            .rows
            .iter()
            .map(|r| {
                let mut map = serde_json::Map::new();
                for (col, val) in res.columns.iter().zip(r.iter()) {
                    map.insert(col.clone(), val.clone());
                }
                serde_json::Value::Object(map)
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&objects)?);
        return Ok(());
    }

    if csv {
        let header = res
            .columns
            .iter()
            .map(|c| format_csv_cell(c))
            .collect::<Vec<_>>()
            .join(",");
        println!("{}", header);
        for row in &res.rows {
            let row_strs: Vec<String> = row
                .iter()
                .map(|v| match v {
                    serde_json::Value::String(s) => format_csv_cell(s),
                    serde_json::Value::Null => String::new(),
                    serde_json::Value::Number(n) => n.to_string(),
                    serde_json::Value::Bool(b) => b.to_string(),
                    other => format_csv_cell(&other.to_string()),
                })
                .collect();
            println!("{}", row_strs.join(","));
        }
        return Ok(());
    }

    // Default: Formatted terminal table
    if res.columns.is_empty() {
        Ui::info("Query executed successfully (0 columns returned).");
        return Ok(());
    }

    let sanitized_columns: Vec<String> = res
        .columns
        .iter()
        .map(|c| crate::ui::terminal::sanitize_terminal_text(c))
        .collect();

    let mut col_widths: Vec<usize> = sanitized_columns
        .iter()
        .map(|c| c.chars().count().max(4))
        .collect();

    let string_rows: Vec<Vec<String>> = res
        .rows
        .iter()
        .map(|row| {
            row.iter()
                .enumerate()
                .map(|(idx, val)| {
                    let raw_str = match val {
                        serde_json::Value::String(s) => s.replace('\n', " "),
                        serde_json::Value::Null => "NULL".to_string(),
                        other => other.to_string(),
                    };
                    let s = crate::ui::terminal::sanitize_terminal_text(&raw_str);
                    if idx < col_widths.len() {
                        col_widths[idx] = col_widths[idx].max(s.chars().count()).min(50);
                    }
                    s
                })
                .collect()
        })
        .collect();

    // Print header
    let header_line = sanitized_columns
        .iter()
        .enumerate()
        .map(|(i, col)| {
            let w = col_widths[i];
            let col_len = col.chars().count();
            let pad = w.saturating_sub(col_len);
            format!("{}{}", col, " ".repeat(pad))
        })
        .collect::<Vec<_>>()
        .join(" │ ");
    println!("│ {} │", header_line);

    // Print separator
    let sep_line = col_widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┼─");
    println!("├─{}─┤", sep_line);

    // Print rows
    for row in string_rows {
        let row_line = row
            .iter()
            .enumerate()
            .map(|(i, cell)| {
                let w = col_widths
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| cell.chars().count());
                let cell_len = cell.chars().count();
                if cell_len > w {
                    let keep = w.saturating_sub(1);
                    let truncated: String = cell.chars().take(keep).collect();
                    format!("{}…", truncated)
                } else {
                    let pad = w.saturating_sub(cell_len);
                    format!("{}{}", cell, " ".repeat(pad))
                }
            })
            .collect::<Vec<_>>()
            .join(" │ ");
        println!("│ {} │", row_line);
    }

    println!("\n\x1b[2m({} row(s) returned)\x1b[0m", res.rows.len());

    Ok(())
}

/// Handles the `srz export` command.
pub fn handle_export(
    db: &Database,
    format: crate::config::cli::ExportFormat,
    output: &std::path::Path,
) -> Result<()> {
    match format {
        crate::config::cli::ExportFormat::Markdown => {
            let count = db.export_markdown_vault(output)?;
            Ui::success(&format!(
                "Exported {} markdown summary files to {:?}",
                count, output
            ));
        }
        crate::config::cli::ExportFormat::Json => {
            db.export_json(output)?;
            Ui::success(&format!("Exported library JSON to {:?}", output));
        }
        crate::config::cli::ExportFormat::Csv => {
            db.export_csv(output)?;
            Ui::success(&format!("Exported library CSV to {:?}", output));
        }
        crate::config::cli::ExportFormat::Parquet => {
            db.export_parquet(output)?;
            Ui::success(&format!("Exported library Parquet to {:?}", output));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_csv_cell_escaping_and_injection() {
        // Plain text
        assert_eq!(format_csv_cell("simple"), "simple");

        // Quotes and commas
        assert_eq!(format_csv_cell("hello, world"), "\"hello, world\"");
        assert_eq!(format_csv_cell("he said \"hi\""), "\"he said \"\"hi\"\"\"");
        assert_eq!(format_csv_cell("line1\nline2"), "\"line1\nline2\"");
        assert_eq!(format_csv_cell("line1\r\nline2"), "\"line1\r\nline2\"");

        // Formula injection prefixes
        assert_eq!(format_csv_cell("=1+1"), "'=1+1");
        assert_eq!(format_csv_cell("@SUM(A1:A10)"), "'@SUM(A1:A10)");
        assert_eq!(format_csv_cell("+cmd|' /C calc'!A0"), "'+cmd|' /C calc'!A0");
        assert_eq!(format_csv_cell("-2+3*4"), "'-2+3*4");
        assert_eq!(format_csv_cell("=SUM(A1, B1)"), "\"'=SUM(A1, B1)\"");

        // Normal numbers should NOT be escaped as formula injection
        assert_eq!(format_csv_cell("-42"), "-42");
        assert_eq!(format_csv_cell("+3.14"), "+3.14");
    }
}
