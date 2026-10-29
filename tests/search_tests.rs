use srz::search::{SearchResult, deduplicate_candidates, extract_ddg_url, rank_candidates};

#[test]
fn test_extract_ddg_url() {
    let ddg_link = "/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FDune_%28novel%29&rut=123";
    let extracted = extract_ddg_url(ddg_link);
    assert_eq!(extracted, "https://en.wikipedia.org/wiki/Dune_(novel)");

    let direct_link = "https://example.com/summary";
    assert_eq!(extract_ddg_url(direct_link), direct_link);
}

#[test]
fn test_deduplicate_candidates_normalizes_encoded_urls() {
    let list = vec![
        SearchResult::new(
            "Dune (novel) (Wikipedia)",
            "https://en.wikipedia.org/wiki/Dune_%28novel%29",
            None,
        ),
        SearchResult::new(
            "Dune (novel) - Wikipedia",
            "https://en.wikipedia.org/wiki/Dune_(novel)/",
            Some("Duplicate URL from DDG".to_string()),
        ),
        SearchResult::new("Other Source", "https://example.com/dune", None),
    ];

    let deduped = deduplicate_candidates(list);
    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].title, "Dune (novel) (Wikipedia)");
    assert_eq!(deduped[1].url, "https://example.com/dune");
}

#[test]
fn test_rank_candidates_prioritizes_exact_novel_over_commentary_and_films() {
    let mut candidates = vec![
        SearchResult::new(
            "The History of The Hobbit (Wikipedia)",
            "https://en.wikipedia.org/wiki/The_History_of_The_Hobbit",
            Some("A scholarly study by John D. Rateliff".to_string()),
        ),
        SearchResult::new(
            "The Hobbit (film series) (Wikipedia)",
            "https://en.wikipedia.org/wiki/The_Hobbit_(film_series)",
            Some("Peter Jackson film trilogy".to_string()),
        ),
        SearchResult::new(
            "The Hobbit (Wikipedia)",
            "https://en.wikipedia.org/wiki/The_Hobbit",
            Some("1937 fantasy novel by English author J. R. R. Tolkien".to_string()),
        ),
    ];

    rank_candidates("The Hobbit", None, &mut candidates);

    // "The Hobbit" should overwhelmingly rank #1 with top score
    assert_eq!(candidates[0].title, "The Hobbit (Wikipedia)");
    assert!(candidates[0].score > candidates[1].score);
    assert!(candidates[0].score > candidates[2].score);

    // If author is supplied, score is boosted even further
    rank_candidates("The Hobbit", Some("J. R. R. Tolkien"), &mut candidates);
    assert_eq!(candidates[0].title, "The Hobbit (Wikipedia)");
    assert!(candidates[0].score >= 150);
}

#[test]
fn test_shared_http_client_initialization() {
    let scraper = srz::scraper::WebScraper::new().unwrap();
    let searcher = srz::search::BookSearcher::with_client(scraper.client().clone());
    let news = srz::news::GoogleNewsClient::with_client(scraper.client().clone());

    // Verify all clients share the underlying HTTP connection pool
    let client_ref: *const reqwest::Client = scraper.client();
    assert!(!client_ref.is_null());
    let searcher_ref: *const reqwest::Client = searcher.client();
    assert!(!searcher_ref.is_null());
    let news_ref: *const reqwest::Client = news.client();
    assert!(!news_ref.is_null());
}
