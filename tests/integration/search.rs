#[test]
fn test_shared_http_client_initialization() {
    let scraper = srz::scraper::WebScraper::new().unwrap();
    let searcher = srz::search::BookSearcher::with_client(scraper.client().clone());
    let news = srz::news::GoogleNewsClient::with_client(scraper.client().clone());

    // Verify all clients can build valid requests using the shared client configuration
    assert!(scraper.client().get("https://example.com").build().is_ok());
    assert!(searcher.client().get("https://example.com").build().is_ok());
    assert!(news.client().get("https://example.com").build().is_ok());
}
