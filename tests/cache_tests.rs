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

    assert!(
        cache_disabled
            .get_book_summary("Dune", None, SummaryStyle::Detailed, "gemma-4", false)
            .is_none()
    );
    assert!(
        cache_enabled
            .get_book_summary("Dune", None, SummaryStyle::Detailed, "gemma-4", false)
            .is_some()
    );

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

#[cfg(unix)]
#[test]
fn test_write_private_file_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let dir = temp_test_dir("private_write");
    fs::create_dir_all(&dir).unwrap();
    let file_path = dir.join("test.txt");

    srz::fs_util::write_private_file(&file_path, b"test private content").unwrap();

    let meta = fs::metadata(&file_path).unwrap();
    assert_eq!(meta.permissions().mode() & 0o777, 0o600);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_parse_duration_and_bytes() {
    use srz::cache::{parse_bytes, parse_duration};
    use std::time::Duration;

    assert_eq!(parse_duration("60s").unwrap(), Duration::from_secs(60));
    assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
    assert_eq!(parse_duration("24h").unwrap(), Duration::from_secs(86400));
    assert_eq!(
        parse_duration("7d").unwrap(),
        Duration::from_secs(7 * 86400)
    );
    assert_eq!(
        parse_duration("2w").unwrap(),
        Duration::from_secs(14 * 86400)
    );
    assert_eq!(parse_duration("5").unwrap(), Duration::from_secs(5 * 86400));
    assert!(parse_duration("invalid").is_err());
    assert!(parse_duration("-5d").is_err());
    assert!(parse_duration("NaN").is_err());
    assert!(parse_duration("inf").is_err());
    assert!(parse_duration("1000000000000000000000d").is_err());

    assert_eq!(parse_bytes("500b").unwrap(), 500);
    assert_eq!(parse_bytes("10kb").unwrap(), 10 * 1024);
    assert_eq!(parse_bytes("500MB").unwrap(), 500 * 1024 * 1024);
    assert_eq!(parse_bytes("1GB").unwrap(), 1024 * 1024 * 1024);
    assert_eq!(parse_bytes("1048576").unwrap(), 1048576);
    assert!(parse_bytes("notabytes").is_err());
    assert!(parse_bytes("-500mb").is_err());
    assert!(parse_bytes("NaN").is_err());
    assert!(parse_bytes("inf").is_err());
    assert!(parse_bytes("9999999999999999999999999GB").is_err());
}

#[test]
fn test_cache_ttl_expiration() {
    use std::thread;
    use std::time::Duration;

    let dir = temp_test_dir("ttl");
    let mut cache = Cache::with_dir(dir.clone(), true);
    cache.policy_mut().pages_ttl = Duration::from_millis(40);

    let page = ScrapedPage {
        url: "https://expiring.com".to_string(),
        title: Some("Expiring".to_string()),
        content: "Temporary content".to_string(),
        word_count: 2,
    };
    cache.set_page("https://expiring.com", &page);

    // Immediate read should hit
    assert!(cache.get_page("https://expiring.com").is_some());

    // Wait past expiration
    thread::sleep(Duration::from_millis(60));

    // Entry should have expired and returned None
    assert!(cache.get_page("https://expiring.com").is_none());

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cache_prune_by_age() {
    use srz::cache::{CacheNamespace, PruneOptions};
    use std::thread;
    use std::time::Duration;

    let dir = temp_test_dir("prune_age");
    let cache = Cache::with_dir(dir.clone(), true);

    let page = ScrapedPage {
        url: "https://prune-test.com".to_string(),
        title: Some("Prune Test".to_string()),
        content: "To be pruned".to_string(),
        word_count: 3,
    };
    cache.set_page("https://prune-test.com", &page);
    assert_eq!(cache.stats().pages, 1);

    thread::sleep(Duration::from_millis(40));

    // Pruning with older_than 20ms should prune the entry
    let stats = cache
        .prune(PruneOptions {
            older_than: Some(Duration::from_millis(20)),
            max_bytes: None,
            namespace: Some(CacheNamespace::Pages),
        })
        .unwrap();

    assert_eq!(stats.files_removed, 1);
    assert!(stats.bytes_reclaimed > 0);
    assert_eq!(cache.stats().pages, 0);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cache_prune_by_max_size_lru() {
    use srz::cache::PruneOptions;
    use std::thread;
    use std::time::Duration;

    let dir = temp_test_dir("prune_size");
    let cache = Cache::with_dir(dir.clone(), true);

    for i in 0..5 {
        let page = ScrapedPage {
            url: format!("https://example.com/{}", i),
            title: Some(format!("Page {}", i)),
            content: "A".repeat(1000),
            word_count: 100,
        };
        cache.set_page(&format!("https://example.com/{}", i), &page);
        thread::sleep(Duration::from_millis(15));
    }

    assert_eq!(cache.stats().pages, 5);
    let original_bytes = cache.stats().total_bytes;

    // Prune so total cache doesn't exceed 2500 bytes (should evict oldest)
    let prune_stats = cache
        .prune(PruneOptions {
            older_than: None,
            max_bytes: Some(2500),
            namespace: None,
        })
        .unwrap();

    assert!(prune_stats.files_removed > 0);
    assert!(cache.stats().total_bytes <= 2500);
    assert!(cache.stats().total_bytes < original_bytes);

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cache_opportunistic_auto_prune() {
    use std::time::Duration;

    let dir = temp_test_dir("auto_prune");
    let mut cache = Cache::with_dir(dir.clone(), true);
    cache.policy_mut().auto_prune_interval = Duration::from_millis(10);

    let page = ScrapedPage {
        url: "https://auto.example.com".to_string(),
        title: Some("Auto".to_string()),
        content: "Auto prune content".to_string(),
        word_count: 3,
    };
    cache.set_page("https://auto.example.com", &page);

    // First auto-prune creates marker file
    let stats = cache.maybe_auto_prune();
    assert!(stats.is_some());
    assert!(dir.join(".last_prune").exists());

    // Immediate second call should skip because interval hasn't elapsed
    let skipped = cache.maybe_auto_prune();
    assert!(skipped.is_none());

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cache_sha256_hash_keys() {
    let dir = temp_test_dir("sha256");
    let cache = Cache::with_dir(dir.clone(), true);

    let page = ScrapedPage {
        url: "https://secure-hash.example.com".to_string(),
        title: Some("Secure".to_string()),
        content: "Testing SHA-256 hash length".to_string(),
        word_count: 4,
    };
    cache.set_page("https://secure-hash.example.com", &page);

    // Read pages directory and verify the key filename is 64 hex characters (SHA-256)
    let pages_dir = dir.join("pages");
    let mut files = fs::read_dir(pages_dir).unwrap();
    let entry = files.next().unwrap().unwrap();
    let stem = entry
        .path()
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(stem.len(), 64);
    assert!(stem.chars().all(|c| c.is_ascii_hexdigit()));

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[test]
fn test_fs_util_rejects_symlinks() {
    use std::os::unix::fs::symlink;

    let dir = temp_test_dir("symlink_test");
    fs::create_dir_all(&dir).unwrap();

    let target = dir.join("real_target.txt");
    fs::write(&target, b"important data").unwrap();

    let link = dir.join("symlink_file.txt");
    symlink(&target, &link).unwrap();

    // Opening symlink for private write must be rejected with InvalidInput
    let write_res = srz::fs_util::open_private_file_for_write(&link);
    assert!(write_res.is_err());
    assert_eq!(
        write_res.err().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );

    // Creating private dir at symlink must be rejected
    let link_dir = dir.join("symlink_dir");
    symlink(&target, &link_dir).unwrap();
    let dir_res = srz::fs_util::create_private_dir_all(&link_dir);
    assert!(dir_res.is_err());
    assert_eq!(
        dir_res.err().unwrap().kind(),
        std::io::ErrorKind::InvalidInput
    );

    let _ = fs::remove_dir_all(dir);
}

#[test]
fn test_cache_duckdb_consolidation() {
    let dir = temp_test_dir("duckdb_cache");
    let cache = Cache::with_dir(dir.clone(), true);

    let page = ScrapedPage {
        url: "https://duckdb-cached.com".to_string(),
        title: Some("DuckDB Cache".to_string()),
        content: "Testing DuckDB cache consolidation".to_string(),
        word_count: 4,
    };
    cache.set_page("https://duckdb-cached.com", &page);

    cache.set_web_summary(
        "https://duckdb-cached.com",
        SummaryStyle::Concise,
        "gemma-4",
        "DuckDB Cache",
        "Consolidated cache in DuckDB",
    );

    let db_path = dir.join("cache.duckdb");
    assert!(db_path.exists(), "cache.duckdb database file must exist");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = fs::metadata(&db_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    let loaded_page = cache.get_page("https://duckdb-cached.com");
    assert!(loaded_page.is_some());
    assert_eq!(loaded_page.unwrap().title.as_deref(), Some("DuckDB Cache"));

    let loaded_summary = cache.get_web_summary(
        "https://duckdb-cached.com",
        SummaryStyle::Concise,
        "gemma-4",
    );
    assert!(loaded_summary.is_some());
    assert_eq!(
        loaded_summary.unwrap().summary,
        "Consolidated cache in DuckDB"
    );

    // Clean should clear the cache
    cache.clean().unwrap();
    assert!(cache.get_page("https://duckdb-cached.com").is_none());

    let _ = fs::remove_dir_all(dir);
}
