use srz::cache::{Cache, SetBookSummaryParams};
use srz::cli::SummaryStyle;
use srz::scraper::ScrapedPage;
use srz::search::{BookMetadata, SearchResult};
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

    let results = vec![SearchResult::new(
        "Dune (Wikipedia)",
        "https://en.wikipedia.org/wiki/Dune_(novel)",
        Some("Sci-fi novel".to_string()),
    )];

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

    cache_enabled.set_book_summary(SetBookSummaryParams {
        title: "Dune",
        author: None,
        style: SummaryStyle::Detailed,
        model: "gemma-4",
        no_search: false,
        display_title: "\"Dune\"",
        summary: "Great sci-fi novel.",
    });

    assert!(cache_disabled
        .get_book_summary("Dune", None, SummaryStyle::Detailed, "gemma-4", false)
        .is_none());
    assert!(cache_enabled
        .get_book_summary("Dune", None, SummaryStyle::Detailed, "gemma-4", false)
        .is_some());

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cache_stats_and_clean() {
    let dir = temp_test_dir("stats_clean");
    let cache = Cache::with_dir(dir.clone(), true);

    assert_eq!(cache.stats().total_files, 0);

    let page = ScrapedPage {
        url: "https://example.com".to_string(),
        title: Some("Example".to_string()),
        content: "Some article body".to_string(),
        word_count: 3,
    };
    cache.set_page("https://example.com", &page);
    cache.set_book_summary(SetBookSummaryParams {
        title: "Dune",
        author: None,
        style: SummaryStyle::Concise,
        model: "gemma-4",
        no_search: false,
        display_title: "\"Dune\"",
        summary: "Summary text",
    });

    let stats = cache.stats();
    assert_eq!(stats.pages, 1);
    assert_eq!(stats.summaries, 1);
    assert_eq!(stats.total_files, 2);
    assert!(stats.total_bytes > 0);

    let removed = cache.clean().unwrap();
    assert_eq!(removed.total_files, 2);
    assert_eq!(cache.stats().total_files, 0);
}

#[test]
fn test_book_metadata_cache_roundtrip() {
    let dir = temp_test_dir("metadata");
    let cache = Cache::with_dir(dir.clone(), true);

    assert!(cache.get_book_metadata("The Hobbit", None).is_none());

    let meta = Some(BookMetadata {
        canonical_title: "The Hobbit".to_string(),
        author: Some("J. R. R. Tolkien".to_string()),
        first_publish_year: Some(1937),
    });
    cache.set_book_metadata("The Hobbit", None, &meta);

    let loaded = cache
        .get_book_metadata("  the hobbit ", None)
        .expect("Should hit metadata cache");
    assert_eq!(loaded, meta);

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn test_cache_file_permissions() {
    use std::os::unix::fs::PermissionsExt;

    let dir = temp_test_dir("perms");
    let cache = Cache::with_dir(dir.clone(), true);

    let page = ScrapedPage {
        url: "https://secure.example.com".to_string(),
        title: Some("Secure".to_string()),
        content: "Secret content".to_string(),
        word_count: 2,
    };
    cache.set_page("https://secure.example.com", &page);

    // Verify directory has 0700 permissions
    let pages_dir = dir.join("pages");
    let dir_meta = fs::metadata(&pages_dir).unwrap();
    assert_eq!(dir_meta.permissions().mode() & 0o777, 0o700);

    // Verify cache entry file has 0600 permissions
    let entries = fs::read_dir(&pages_dir).unwrap().collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    let file_meta = entries[0].as_ref().unwrap().metadata().unwrap();
    assert_eq!(file_meta.permissions().mode() & 0o777, 0o600);

    let _ = fs::remove_dir_all(dir);
}
