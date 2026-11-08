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

    // Verify high-confidence short-circuit threshold logic (score >= 85)
    assert!(candidates[0].score >= 85);
}

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

#[test]
fn test_extract_wikipedia_synopsis() {
    let wiki_markdown = r#"# Dune (novel)

Dune is a 1965 epic science fiction novel by American author Frank Herbert. Set in the distant future.

## Background
Frank Herbert began researching in Oregon in 1957.

## Plot
Set in the distant future amidst a feudal interstellar society, Dune tells the story of young Paul Atreides, whose family accepts the stewardship of the desert planet Arrakis.

### Arrakis
Arrakis is the only source of the spice melange, the most valuable substance in the universe. The spice extends human life and provides higher levels of thought. Paul must navigate treacherous political alliances and survive the harsh desert sands.

## Themes
The novel explores the interplay between religion, politics, and ecology.

## Adaptations
A film adaptation was directed by David Lynch in 1984.

## References
[1] Herbert, Frank. Dune (1965).
"#;

    let extracted = srz::search::extract_wikipedia_synopsis(wiki_markdown);
    assert!(extracted.is_some());
    let text = extracted.unwrap();

    // Verify lead overview is preserved
    assert!(text.contains("Dune is a 1965 epic science fiction novel"));

    // Verify plot and subsection are extracted
    assert!(text.contains("## Plot"));
    assert!(text.contains("Paul Atreides"));
    assert!(text.contains("Arrakis is the only source of the spice"));

    // Verify bloat sections (Background, Adaptations, References) are cleanly stripped
    assert!(!text.contains("## Background"));
    assert!(!text.contains("Frank Herbert began researching"));
    assert!(!text.contains("## Adaptations"));
    assert!(!text.contains("David Lynch"));
    assert!(!text.contains("## References"));
}

#[test]
fn test_extract_wikipedia_synopsis_non_fiction() {
    let wiki_markdown = r#"# Thinking, Fast and Slow

Thinking, Fast and Slow is a 2011 best-selling book by Nobel Memorial Prize in Economic Sciences laureate Daniel Kahneman.

## Overview
The book summarizes research that Kahneman conducted over decades, often in collaboration with Amos Tversky. It covers all three phases of his career: his early days working on cognitive biases, his work on prospect theory, and his later work on happiness. The central thesis is a dichotomy between two modes of thought: System 1 is fast, instinctive and emotional; System 2 is slower, more deliberative, and more logical.

## Key themes
The dichotomy between two modes of thought, heuristics and biases, and prospect theory.

## References
[1] Kahneman, Daniel. Thinking, Fast and Slow (2011). Farrar, Straus and Giroux.
"#;

    let extracted = srz::search::extract_wikipedia_synopsis(wiki_markdown);
    assert!(extracted.is_some());
    let text = extracted.unwrap();

    // Verify lead overview is preserved
    assert!(text.contains("Thinking, Fast and Slow is a 2011 best-selling book"));

    // Verify non-fiction Overview section is extracted
    assert!(text.contains("## Overview"));
    assert!(text.contains("System 1 is fast, instinctive and emotional"));

    // Verify references are excluded
    assert!(!text.contains("## References"));
    assert!(!text.contains("Farrar, Straus and Giroux"));
}

#[test]
fn test_parse_work_description() {
    // 1. Plain string description format
    let json_str =
        r#"{"title": "Dune", "description": "Dune is a science fiction novel by Frank Herbert."}"#;
    let parsed = srz::search::parse_work_description(json_str.as_bytes());
    assert_eq!(
        parsed.as_deref(),
        Some("Dune is a science fiction novel by Frank Herbert.")
    );

    // 2. Typed object description format: {"type": "/type/text", "value": "..."}
    let json_obj = r#"{"title": "1984", "description": {"type": "/type/text", "value": "Nineteen Eighty-Four is a dystopian social science fiction novel by George Orwell."}}"#;
    let parsed_obj = srz::search::parse_work_description(json_obj.as_bytes());
    assert_eq!(
        parsed_obj.as_deref(),
        Some("Nineteen Eighty-Four is a dystopian social science fiction novel by George Orwell.")
    );

    // 3. Missing description or empty string
    let json_empty = r#"{"title": "Unknown Book", "description": ""}"#;
    assert!(srz::search::parse_work_description(json_empty.as_bytes()).is_none());

    let json_none = r#"{"title": "No Description"}"#;
    assert!(srz::search::parse_work_description(json_none.as_bytes()).is_none());
}

#[test]
fn test_book_metadata_serde_backward_compatibility() {
    // Old cached JSON without work_key field
    let old_json =
        r#"{"canonical_title":"The Hobbit","author":"J. R. R. Tolkien","first_publish_year":1937}"#;
    let meta: srz::search::BookMetadata = serde_json::from_str(old_json).unwrap();
    assert_eq!(meta.canonical_title, "The Hobbit");
    assert_eq!(meta.author.as_deref(), Some("J. R. R. Tolkien"));
    assert_eq!(meta.first_publish_year, Some(1937));
    assert_eq!(meta.work_key, None);

    // New JSON with work_key
    let new_json = r#"{"canonical_title":"The Hobbit","author":"J. R. R. Tolkien","first_publish_year":1937,"work_key":"/works/OL262758W"}"#;
    let meta_new: srz::search::BookMetadata = serde_json::from_str(new_json).unwrap();
    assert_eq!(meta_new.work_key.as_deref(), Some("/works/OL262758W"));
}
