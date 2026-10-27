use srz::cache::{Cache, CachedSummary};
use srz::cli::SummaryStyle;
use srz::scraper::ScrapedPage;
use srz::search::SearchResult;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_test_dir(label: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    env::temp_dir().join(format!("srz_test_{}_{}", label, nanos))
}

#[test]
fn test_page_cache_roundtrip() {
    let dir = temp_test_dir("page");
    let cache = Cache::with_dir(dir.clone(), true);

    let page = ScrapedPage {
        url: "https://example.com".to_string(),
        title: Some("Example".to_string()),
        content: "Hello world content".to_string(),
        word_count: 3,
    };

    assert!(cache.get_page("https://example.com").is_none());
    cache.set_page("https://example.com", &page);

    let loaded = cache
        .get_page("https://example.com")
        .expect("Should hit cache");
    assert_eq!(loaded.title.as_deref(), Some("Example"));
    assert_eq!(loaded.word_count, 3);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_search_results_cache_is_case_insensitive() {
    let dir = temp_test_dir("search");
    let cache = Cache::with_dir(dir.clone(), true);

    let results = vec![SearchResult {
        title: "Dune (Wikipedia)".to_string(),
        url: "https://en.wikipedia.org/wiki/Dune_(novel)".to_string(),
        snippet: Some("Sci-fi novel".to_string()),
    }];

    cache.set_search_results("Dune", Some("Frank Herbert"), &results);

    let loaded = cache
        .get_search_results("  dune ", Some("FRANK HERBERT"))
        .expect("Should hit case-insensitive search cache");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].title, "Dune (Wikipedia)");

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_disabled_cache_bypasses_reads() {
    let dir = temp_test_dir("disabled");
    let cache_enabled = Cache::with_dir(dir.clone(), true);
    let cache_disabled = Cache::with_dir(dir.clone(), false);

    let summary = CachedSummary {
        title: "\"Dune\"".to_string(),
        summary: "Great sci-fi novel.".to_string(),
    };

    cache_enabled.set_book_summary(
        "Dune",
        None,
        SummaryStyle::Detailed,
        "gemma-4",
        false,
        &summary,
    );

    assert!(cache_disabled
        .get_book_summary("Dune", None, SummaryStyle::Detailed, "gemma-4", false)
        .is_none());
    assert!(cache_enabled
        .get_book_summary("Dune", None, SummaryStyle::Detailed, "gemma-4", false)
        .is_some());

    let _ = fs::remove_dir_all(dir);
}
