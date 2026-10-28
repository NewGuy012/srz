use srz::chunker::TextChunker;
use srz::llm::truncate_words_preserving_lines;
use srz::render::{ensure_view_files, markdown_to_html, render_html_document, slugify};
use srz::scraper::WebScraper;
use std::borrow::Cow;
use std::fs;

#[test]
fn test_normalize_extracts_title_and_clean_text() {
    let scraper = WebScraper::new().unwrap();
    let html = r#"
        <!DOCTYPE html>
        <html>
        <head>
            <title>Rust Programming Language</title>
            <meta property="og:title" content="Rust: Fast and Reliable">
        </head>
        <body>
            <header><nav><a href="/">Home</a></nav></header>
            <main>
                <h1>Rust Overview</h1>
                <p>Rust is a <a href="https://rust-lang.org">blazingly fast</a> and memory-efficient language.</p>
                <p>It empowers everyone to build reliable and efficient software.</p>
                <h2>References</h2>
                <p>Citation 1 that should be stripped.</p>
            </main>
            <footer><p>© 2026 Rust</p></footer>
        </body>
        </html>
    "#;

    let page = scraper.normalize("https://rust-lang.org", html).unwrap();
    assert_eq!(page.title.as_deref(), Some("Rust: Fast and Reliable"));
    assert!(page.content.contains("blazingly fast"));
    assert!(!page.content.contains("[1]"));
    assert!(!page.content.contains("Citation 1"));
    assert!(page.word_count > 10);
}

#[test]
fn test_short_text_single_chunk_is_zero_copy() {
    let text = "Paragraph one with some text.\n\nParagraph two with more text.";
    let chunks = TextChunker::chunk(text, 100, 10);
    assert_eq!(chunks.len(), 1);
    assert!(matches!(chunks[0], Cow::Borrowed(_)));
    assert_eq!(chunks[0], text);
}

#[test]
fn test_chunking_splits_on_word_limit() {
    let p1 = "word ".repeat(50);
    let p2 = "test ".repeat(50);
    let text = format!("{}\n\n{}", p1.trim(), p2.trim());

    let chunks = TextChunker::chunk(&text, 60, 10);
    assert_eq!(chunks.len(), 2);
}

#[test]
fn test_truncate_words_preserves_newlines_and_borrows_when_under_limit() {
    let input =
        "# Heading 1\n\nParagraph one with five words.\n\n## Heading 2\n\nParagraph two with extra words.";

    // Under budget -> zero-copy Cow::Borrowed
    let untouched = truncate_words_preserving_lines(input, 100);
    assert!(matches!(untouched, Cow::Borrowed(_)));
    assert_eq!(untouched, input);

    // Over budget -> slices at exact word boundary preserving newlines
    let out = truncate_words_preserving_lines(input, 8);
    assert!(out.contains("# Heading 1\n\nParagraph one with five words."));
    assert!(out.ends_with("[Context trimmed for token budget]"));
    assert!(!out.contains("Heading 2"));
}

#[test]
fn test_markdown_to_styled_html_rendering() {
    assert_eq!(slugify("\"Dune\" by Frank Herbert"), "dune-by-frank-herbert");

    let md = "# Literary Analysis: Dune\n\n***\n\n## 1. Overview\n\n**Author:** Frank Herbert\n**Genre:** *Science Fiction*\n\n- Key item 1\n- Key item 2";
    let html_body = markdown_to_html(md);
    assert!(html_body.contains("<h1>Literary Analysis: Dune</h1>"));
    assert!(html_body.contains("<hr>"));
    assert!(html_body.contains("<h2>1. Overview</h2>"));
    assert!(html_body.contains("<strong>Author:</strong> Frank Herbert"));
    assert!(html_body.contains("<em>Science Fiction</em>"));
    assert!(html_body.contains("<ul>"));
    assert!(html_body.contains("<li>Key item 1</li>"));

    let full_doc = render_html_document("\"Dune\"", md);
    assert!(full_doc.contains("<!DOCTYPE html>"));
    assert!(full_doc.contains("Summary: &quot;Dune&quot;"));

    // Verify ensure_view_files skips rewrite when force_write is false and files exist
    let tmp_dir = std::env::temp_dir().join("srz_test_views_reuse");
    let _ = fs::remove_dir_all(&tmp_dir);
    let rendered = ensure_view_files(&tmp_dir, "Dune", "Initial summary", true).unwrap();
    assert_eq!(fs::read_to_string(&rendered.md_path).unwrap(), "Initial summary");

    // With force_write = false, existing files are reused untouched
    let reused = ensure_view_files(&tmp_dir, "Dune", "Updated summary", false).unwrap();
    assert_eq!(fs::read_to_string(&reused.md_path).unwrap(), "Initial summary");

    // With force_write = true, files are overwritten
    let overwritten = ensure_view_files(&tmp_dir, "Dune", "Updated summary", true).unwrap();
    assert_eq!(fs::read_to_string(&overwritten.md_path).unwrap(), "Updated summary");
    let _ = fs::remove_dir_all(&tmp_dir);
}

#[test]
fn test_render_index_html() {
    use srz::db::{HistoryEntry, LibraryStats};
    use srz::render::{ensure_index_file, render_index_html};

    let stats = LibraryStats {
        total_summaries: 2,
        book_summaries: 1,
        web_summaries: 1,
        file_summaries: 0,
        stdin_summaries: 0,
        total_source_words: 5000,
        total_prompt_tokens: 3500,
        total_completion_tokens: 800,
        avg_tokens_per_second: 42.0,
        max_tokens_per_second: 55.0,
        total_duration_ms: 12000,
    };

    let entries = vec![
        HistoryEntry {
            id: 1,
            target_type: "book".to_string(),
            title: "Dune".to_string(),
            author: Some("Frank Herbert".to_string()),
            url: None,
            style: "concise".to_string(),
            model: "gemma-4".to_string(),
            word_count: 3000,
            prompt_tokens: Some(2000),
            completion_tokens: Some(400),
            duration_ms: 6000,
            tokens_per_second: Some(45.0),
            created_at: "2026-10-28 10:00".to_string(),
        },
        HistoryEntry {
            id: 2,
            target_type: "web".to_string(),
            title: "Rust 2026 Edition".to_string(),
            author: None,
            url: Some("https://rust-lang.org".to_string()),
            style: "bullets".to_string(),
            model: "gemma-4".to_string(),
            word_count: 2000,
            prompt_tokens: Some(1500),
            completion_tokens: Some(400),
            duration_ms: 6000,
            tokens_per_second: Some(40.0),
            created_at: "2026-10-28 10:15".to_string(),
        },
    ];

    let html = render_index_html(&stats, &entries);
    assert!(html.contains("srz &bull; Summary Library"));
    assert!(html.contains("Dune"));
    assert!(html.contains("Frank Herbert"));
    assert!(html.contains("Rust 2026 Edition"));
    assert!(html.contains("data-type=\"book\""));
    assert!(html.contains("data-type=\"web\""));

    let tmp_dir = std::env::temp_dir().join("srz_test_index_dashboard");
    let _ = fs::remove_dir_all(&tmp_dir);
    let index_file = ensure_index_file(&tmp_dir, &stats, &entries).unwrap();
    assert!(index_file.exists());
    let written = fs::read_to_string(&index_file).unwrap();
    assert_eq!(written, html);
    let _ = fs::remove_dir_all(&tmp_dir);
}
