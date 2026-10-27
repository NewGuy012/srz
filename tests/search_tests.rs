use srz::search::{deduplicate_candidates, extract_ddg_url, SearchResult};

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
        SearchResult {
            title: "Dune (novel) (Wikipedia)".to_string(),
            url: "https://en.wikipedia.org/wiki/Dune_%28novel%29".to_string(),
            snippet: None,
        },
        SearchResult {
            title: "Dune (novel) - Wikipedia".to_string(),
            url: "https://en.wikipedia.org/wiki/Dune_(novel)/".to_string(),
            snippet: Some("Duplicate URL from DDG".to_string()),
        },
        SearchResult {
            title: "Other Source".to_string(),
            url: "https://example.com/dune".to_string(),
            snippet: None,
        },
    ];

    let deduped = deduplicate_candidates(list);
    assert_eq!(deduped.len(), 2);
    assert_eq!(deduped[0].title, "Dune (novel) (Wikipedia)");
    assert_eq!(deduped[1].url, "https://example.com/dune");
}
