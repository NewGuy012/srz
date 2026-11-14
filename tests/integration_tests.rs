#[path = "integration/cli.rs"]
mod cli;
#[path = "integration/config.rs"]
mod config;
#[path = "integration/search.rs"]
mod search;

#[test]
fn test_assets_document_pdf_cli_access_and_extraction() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/document.pdf");
    assert!(
        path.exists(),
        "assets/document.pdf must exist for CLI access and tests"
    );

    let page = srz::sources::document::PdfExtractor::extract_file(&path)
        .expect("Failed to extract assets/document.pdf");
    for expected in [
        "THE HISTORY OF RUST",
        "Ownership & Borrowing",
        "Graydon Hoare",
        "The Rust Foundation",
    ] {
        assert!(page.content.contains(expected));
    }
    assert!(page.word_count >= 100);

    let scraper = srz::scraper::WebScraper::new().unwrap();
    let doc = srz::sources::DocumentLoader::load_file(&path, &scraper)
        .expect("CLI DocumentLoader failed to access assets/document.pdf");
    assert!(doc.word_count >= 100);
    assert!(doc.content.contains("THE HISTORY OF RUST"));
    assert!(doc.content.contains("CHRONOLOGICAL MILESTONE TIMELINE"));
}
