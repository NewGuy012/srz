use srz::chunker::TextChunker;
use srz::llm::truncate_words_preserving_lines;
use srz::scraper::WebScraper;

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
fn test_short_text_single_chunk() {
    let text = "Paragraph one with some text.\n\nParagraph two with more text.";
    let chunks = TextChunker::chunk(text, 100, 10);
    assert_eq!(chunks.len(), 1);
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
fn test_truncate_words_preserves_newlines() {
    let input =
        "# Heading 1\n\nParagraph one with five words.\n\n## Heading 2\n\nParagraph two with extra words.";
    let out = truncate_words_preserving_lines(input, 8);
    assert!(out.contains("# Heading 1\n\nParagraph one with five words."));
    assert!(out.ends_with("[Context trimmed for token budget]"));
    assert!(!out.contains("Heading 2"));
}
