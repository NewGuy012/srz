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
                <p>Rust is a blazingly fast and memory-efficient language.</p>
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
