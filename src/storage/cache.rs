use crate::config::SummaryStyle;
use crate::engine::ServerCapabilities;
use crate::fs_util;
use crate::scraper::ScrapedPage;
use crate::sources::search::{BookMetadata, SearchResult};
use crate::storage::db::{CacheTable, Database};
use crate::system::sha256::sha256_hex;
use crate::system::text_util::{slugify, slugify_unique};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

const SECS_PER_DAY: u64 = 86_400;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedSummary {
    pub title: String,
    #[serde(default)]
    pub author: Option<String>,
    pub summary: String,
    #[serde(default)]
    pub word_count: usize,
    #[serde(default)]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub pages: usize,
    pub searches: usize,
    pub summaries: usize,
    pub views: usize,
    pub total_files: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    pub pages_ttl: Duration,
    pub news_ttl: Duration,
    pub search_ttl: Duration,
    pub summaries_ttl: Duration,
    pub views_ttl: Duration,
    pub caps_ttl: Duration,
    pub max_bytes: u64,
    pub auto_prune_interval: Duration,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            pages_ttl: Duration::from_secs(7 * SECS_PER_DAY),
            news_ttl: Duration::from_secs(2 * 3600),
            search_ttl: Duration::from_secs(14 * SECS_PER_DAY),
            summaries_ttl: Duration::from_secs(90 * SECS_PER_DAY),
            views_ttl: Duration::from_secs(14 * SECS_PER_DAY),
            caps_ttl: Duration::from_secs(7 * SECS_PER_DAY),
            max_bytes: 500 * 1024 * 1024,
            auto_prune_interval: Duration::from_secs(SECS_PER_DAY),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneOptions {
    pub older_than: Option<Duration>,
    pub max_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub files_removed: usize,
    pub bytes_reclaimed: u64,
}

fn parse_number_and_unit(
    s: &str,
    default_unit: &str,
    type_label: &str,
) -> Result<(f64, String), String> {
    let s = s.trim();
    if s.is_empty() {
        return Err(format!("Empty {} string", type_label));
    }
    let (num_str, unit) = match s.find(|c: char| !c.is_ascii_digit() && c != '.') {
        Some(idx) => (&s[..idx], s[idx..].trim().to_ascii_lowercase()),
        None => (s, default_unit.to_string()),
    };

    let val: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid number in {} string: '{}'", type_label, s))?;

    Ok((val, unit))
}

pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let (val, unit) = parse_number_and_unit(s, "d", "duration")?;
    let secs = match unit.as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => val,
        "m" | "min" | "mins" | "minute" | "minutes" => val * 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => val * 3600.0,
        "d" | "day" | "days" => val * 86400.0,
        "w" | "week" | "weeks" => val * 7.0 * 86400.0,
        other => {
            return Err(format!(
                "Unknown duration unit: '{}' (use s, m, h, d, w)",
                other
            ));
        }
    };

    if !secs.is_finite() || secs < 0.0 {
        return Err(format!("Duration value out of range: '{}'", s));
    }

    Duration::try_from_secs_f64(secs)
        .map_err(|_| format!("Duration value exceeds maximum representable time: '{}'", s))
}

pub fn parse_bytes(s: &str) -> Result<u64, String> {
    let (val, unit) = parse_number_and_unit(s, "b", "byte size")?;
    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes = match unit.as_str() {
        "b" | "bytes" => val,
        "k" | "kb" | "kib" => val * KB,
        "m" | "mb" | "mib" => val * MB,
        "g" | "gb" | "gib" => val * GB,
        other => {
            return Err(format!(
                "Unknown byte unit: '{}' (use B, KB, MB, GB)",
                other
            ));
        }
    };

    if !bytes.is_finite() || bytes < 0.0 || bytes > u64::MAX as f64 {
        return Err(format!("Byte size out of range: '{}'", s));
    }

    Ok(bytes as u64)
}

#[derive(Clone)]
pub struct Cache {
    base_dir: PathBuf,
    db_path: PathBuf,
    enabled: bool,
    policy: CachePolicy,
    db: Arc<Mutex<Option<Database>>>,
}

impl Cache {
    pub fn new(enabled: bool) -> Self {
        Self::with_paths(
            crate::fs_util::default_cache_dir(),
            Database::default_path(),
            enabled,
            CachePolicy::default(),
        )
    }

    #[doc(hidden)]
    pub fn with_dir(base_dir: PathBuf, enabled: bool) -> Self {
        let db_path = base_dir.join("srz.db");
        Self::with_paths(base_dir, db_path, enabled, CachePolicy::default())
    }

    fn with_paths(base_dir: PathBuf, db_path: PathBuf, enabled: bool, policy: CachePolicy) -> Self {
        Self {
            base_dir,
            db_path,
            enabled,
            policy,
            db: Arc::new(Mutex::new(None)),
        }
    }

    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    pub fn dir(&self) -> &Path {
        &self.base_dir
    }

    pub fn views_dir(&self) -> PathBuf {
        self.base_dir.join("views")
    }

    pub fn html_view_path_for(&self, title: &str, unique_key: Option<&str>) -> PathBuf {
        let unique_path = self
            .views_dir()
            .join(format!("{}.html", slugify_unique(title, unique_key)));
        if unique_path.exists() {
            return unique_path;
        }
        let legacy_path = self.views_dir().join(format!("{}.html", slugify(title)));
        if legacy_path.exists() {
            return legacy_path;
        }
        unique_path
    }

    pub fn policy(&self) -> &CachePolicy {
        &self.policy
    }

    pub fn set_policy(&mut self, policy: CachePolicy) {
        self.policy = policy;
    }

    /// Opens or reuses the embedded database handle regardless of whether `--no-cache` is set.
    pub fn open_database(&self) -> anyhow::Result<Database> {
        let mut guard = self
            .db
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock poisoned"))?;
        if let Some(db) = guard.as_ref() {
            return Ok(db.clone());
        }
        let database = Database::open_at(&self.db_path)?;
        *guard = Some(database.clone());
        Ok(database)
    }

    fn existing_database(&self) -> Option<anyhow::Result<Database>> {
        let has_handle = self.db.lock().is_ok_and(|g| g.is_some());
        (has_handle || self.db_path.exists()).then(|| self.open_database())
    }

    fn cached_db(&self) -> Option<Database> {
        self.enabled.then(|| self.open_database().ok()).flatten()
    }

    fn get_json<T: DeserializeOwned>(
        &self,
        table: CacheTable,
        key: &str,
        ttl: Duration,
    ) -> Option<T> {
        self.cached_db()?.get_cached_json(table, key, ttl)
    }

    fn set_json<T: Serialize + ?Sized>(&self, table: CacheTable, key: &str, value: &T) {
        if let Some(db) = self.cached_db() {
            let _ = db.set_cached_json(table, key, value);
        }
    }

    pub fn stats(&self) -> CacheStats {
        let (views, views_bytes) = Self::dir_stats(&self.views_dir());
        let (pages, pages_bytes, searches, search_bytes, summaries, summaries_bytes) = self
            .existing_database()
            .and_then(|res| res.ok())
            .map(|db| db.cache_stats_breakdown())
            .unwrap_or_default();

        CacheStats {
            pages,
            searches,
            summaries,
            views,
            total_files: pages
                .saturating_add(searches)
                .saturating_add(summaries)
                .saturating_add(views),
            total_bytes: pages_bytes
                .saturating_add(search_bytes)
                .saturating_add(summaries_bytes)
                .saturating_add(views_bytes),
        }
    }

    pub fn clean(&self) -> std::io::Result<CacheStats> {
        let before = self.stats();
        if let Some(db_res) = self.existing_database() {
            let db = db_res.map_err(std::io::Error::other)?;
            db.clear_cache_tables().map_err(std::io::Error::other)?;
        }
        let views = self.views_dir();
        if views.exists() {
            fs::remove_dir_all(views)?;
        }
        Ok(before)
    }

    fn prune_views_older_than(&self, ttl: Duration) -> (usize, u64) {
        let views = self.views_dir();
        if !views.exists() {
            return (0, 0);
        }
        let now = SystemTime::now();
        let mut files_removed = 0;
        let mut bytes_reclaimed = 0u64;
        if let Ok(entries) = fs::read_dir(views) {
            for entry in entries.flatten() {
                if entry.file_type().is_ok_and(|t| t.is_file())
                    && let Ok(meta) = entry.metadata()
                    && let Ok(modified) = meta.modified()
                    && let Ok(age) = now.duration_since(modified)
                    && age > ttl
                    && fs::remove_file(entry.path()).is_ok()
                {
                    files_removed += 1;
                    bytes_reclaimed = bytes_reclaimed.saturating_add(meta.len());
                }
            }
        }
        (files_removed, bytes_reclaimed)
    }

    fn run_prune(
        &self,
        options: PruneOptions,
        use_policy_ttls: bool,
    ) -> std::io::Result<PruneStats> {
        let mut files_removed = 0usize;
        let mut bytes_reclaimed = 0u64;

        let db = self.open_database().map_err(std::io::Error::other)?;
        if let Some(max_age) = options.older_than {
            let (cnt, bytes) = db
                .prune_cache_by_age(max_age)
                .map_err(std::io::Error::other)?;
            files_removed += cnt;
            bytes_reclaimed = bytes_reclaimed.saturating_add(bytes);
            let (vcnt, vbytes) = self.prune_views_older_than(max_age);
            files_removed += vcnt;
            bytes_reclaimed = bytes_reclaimed.saturating_add(vbytes);
        } else if use_policy_ttls || options.max_bytes.is_none() {
            let (cnt, bytes) = db
                .prune_cache_with_ttls(
                    self.policy.pages_ttl,
                    self.policy.search_ttl,
                    self.policy.caps_ttl,
                    Some(self.policy.summaries_ttl),
                )
                .map_err(std::io::Error::other)?;
            files_removed += cnt;
            bytes_reclaimed = bytes_reclaimed.saturating_add(bytes);
            let (vcnt, vbytes) = self.prune_views_older_than(self.policy.views_ttl);
            files_removed += vcnt;
            bytes_reclaimed = bytes_reclaimed.saturating_add(vbytes);
        }

        if let Some(max_bytes) = options.max_bytes {
            let (cnt, bytes) = db
                .prune_cache_lru(max_bytes)
                .map_err(std::io::Error::other)?;
            files_removed += cnt;
            bytes_reclaimed = bytes_reclaimed.saturating_add(bytes);
        }

        Ok(PruneStats {
            files_removed,
            bytes_reclaimed,
        })
    }

    pub fn prune(&self, options: PruneOptions) -> std::io::Result<PruneStats> {
        self.run_prune(options, false)
    }

    pub fn maybe_auto_prune(&self) -> Option<PruneStats> {
        if !self.enabled {
            return None;
        }
        let marker = self.base_dir.join(".last_prune");
        let now = SystemTime::now();

        if let Ok(meta) = fs::metadata(&marker)
            && let Ok(mod_time) = meta.modified()
            && let Ok(elapsed) = now.duration_since(mod_time)
            && elapsed < self.policy.auto_prune_interval
        {
            return None;
        }

        let stats = self
            .run_prune(
                PruneOptions {
                    older_than: None,
                    max_bytes: Some(self.policy.max_bytes),
                },
                true,
            )
            .ok()?;
        let _ = fs_util::create_private_dir_all(&self.base_dir);
        let _ = fs_util::write_private_file(&marker, b"");
        Some(stats)
    }

    pub fn format_bytes(bytes: u64) -> String {
        const KB: u64 = 1024;
        const MB: u64 = KB * 1024;
        const GB: u64 = MB * 1024;

        if bytes >= GB {
            format!("{:.1} GB", bytes as f64 / GB as f64)
        } else if bytes >= MB {
            format!("{:.1} MB", bytes as f64 / MB as f64)
        } else if bytes >= KB {
            format!("{:.1} KB", bytes as f64 / KB as f64)
        } else {
            format!("{} B", bytes)
        }
    }

    fn dir_stats(dir: &Path) -> (usize, u64) {
        if !dir.exists() {
            return (0, 0);
        }
        let mut count = 0;
        let mut bytes = 0u64;
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    count += 1;
                    bytes = bytes.saturating_add(meta.len());
                }
            }
        }
        (count, bytes)
    }

    pub fn get_page(&self, url: &str) -> Option<ScrapedPage> {
        self.get_page_with_ttl(url, self.policy.pages_ttl)
    }

    pub fn get_news_page(&self, url: &str) -> Option<ScrapedPage> {
        self.get_page_with_ttl(url, self.policy.news_ttl)
    }

    fn get_page_with_ttl(&self, url: &str, ttl: Duration) -> Option<ScrapedPage> {
        self.get_json(CacheTable::Pages, &sha256_hex(url.as_bytes()), ttl)
    }

    pub fn set_page(&self, url: &str, page: &ScrapedPage) {
        self.set_json(CacheTable::Pages, &sha256_hex(url.as_bytes()), page);
    }

    fn search_key(title: &str, author: Option<&str>) -> String {
        let t = title.trim().to_ascii_lowercase();
        let a = author
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        format!("{}:{}", t, a)
    }

    pub fn get_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Option<BookMetadata>> {
        let key = format!("meta:{}", Self::search_key(title, author));
        self.get_json(CacheTable::Search, &key, self.policy.search_ttl)
    }

    pub fn set_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
        metadata: &Option<BookMetadata>,
    ) {
        let key = format!("meta:{}", Self::search_key(title, author));
        self.set_json(CacheTable::Search, &key, metadata);
    }

    pub fn get_search_results(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Vec<SearchResult>> {
        let key = Self::search_key(title, author);
        self.get_json(CacheTable::Search, &key, self.policy.search_ttl)
    }

    pub fn set_search_results(&self, title: &str, author: Option<&str>, results: &[SearchResult]) {
        let key = Self::search_key(title, author);
        self.set_json(CacheTable::Search, &key, results);
    }

    pub fn book_cache_title(title: &str, author: Option<&str>) -> String {
        match author {
            Some(a) if !a.trim().is_empty() => format!("\"{}\" by {}", title.trim(), a.trim()),
            _ => title.trim().to_string(),
        }
    }

    pub fn news_cache_title(topic: &str, timeframe: crate::cli::NewsTimeframe) -> String {
        if crate::sources::news::GoogleNewsClient::is_direct_feed(topic) {
            format!("News: {}", topic.trim())
        } else {
            match timeframe {
                crate::cli::NewsTimeframe::All => format!("News: {}", topic.trim()),
                tf => format!("News: {} ({})", topic.trim(), tf.display_label()),
            }
        }
    }

    pub fn get_summary(
        &self,
        target_type: &str,
        identifier: &str,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        let ttl = if target_type == "news" {
            self.policy.news_ttl
        } else {
            self.policy.summaries_ttl
        };
        self.cached_db()?
            .get_cached_summary(target_type, identifier, style.as_str(), model, ttl)
    }

    pub fn get_server_capabilities(
        &self,
        endpoint: &str,
        model: &str,
    ) -> Option<ServerCapabilities> {
        let db = self.cached_db()?;
        let caps: ServerCapabilities =
            db.get_cached_json(CacheTable::Caps, endpoint, self.policy.caps_ttl)?;
        if caps.model_id == model {
            Some(caps)
        } else {
            let _ = db.remove_cached_entry(CacheTable::Caps, endpoint);
            None
        }
    }

    pub fn set_server_capabilities(&self, caps: &ServerCapabilities) {
        self.set_json(CacheTable::Caps, &caps.endpoint, caps);
    }
}

#[cfg(test)]
mod tests {
    use super::super::make_summary;
    use super::*;
    use crate::cli::NewsTimeframe;
    use crate::storage::db::NewSummaryRecord;

    fn test_cache() -> (crate::fs_util::TempDir, Cache) {
        let tmp = crate::fs_util::tempdir().expect("Failed to create temporary directory");
        let cache = Cache::with_dir(tmp.path().to_path_buf(), true);
        (tmp, cache)
    }

    fn make_page(url: &str, title: &str, content: &str, word_count: usize) -> ScrapedPage {
        ScrapedPage {
            url: url.to_string(),
            title: Some(title.to_string()),
            content: content.to_string(),
            word_count,
        }
    }

    #[test]
    fn test_page_search_and_caps_cache_roundtrip() {
        let (_tmp, cache) = test_cache();
        let url = "https://example.com";
        assert!(cache.get_page(url).is_none());
        cache.set_page(url, &make_page(url, "Example", "Hello world content", 3));
        assert_eq!(
            cache.get_page(url).unwrap().title.as_deref(),
            Some("Example")
        );

        let results = vec![SearchResult::new(
            "Dune (Wikipedia)",
            "https://en.wikipedia.org/wiki/Dune_(novel)",
            Some("Sci-fi novel".to_string()),
        )];
        cache.set_search_results("Dune", Some("Frank Herbert"), &results);
        assert_eq!(
            cache
                .get_search_results("  dune ", Some("FRANK HERBERT"))
                .unwrap()[0]
                .title,
            "Dune (Wikipedia)"
        );

        let meta = Some(BookMetadata {
            canonical_title: "The Hobbit".to_string(),
            author: Some("J. R. R. Tolkien".to_string()),
            first_publish_year: Some(1937),
            work_key: Some("/works/OL262758W".to_string()),
        });
        cache.set_book_metadata("The Hobbit", None, &meta);
        assert_eq!(
            cache.get_book_metadata("  the hobbit ", None).unwrap(),
            meta
        );

        let caps = ServerCapabilities {
            endpoint: "http://localhost:9931".to_string(),
            model_id: "gemma-4".to_string(),
            n_ctx: 32768,
            backend_kind: "llama.cpp".to_string(),
        };
        cache.set_server_capabilities(&caps);
        assert_eq!(
            cache
                .get_server_capabilities("http://localhost:9931", "gemma-4")
                .unwrap()
                .n_ctx,
            32768
        );
        assert!(
            cache
                .get_server_capabilities("http://localhost:9931", "other")
                .is_none()
        );
        assert!(
            cache
                .get_server_capabilities("http://localhost:9931", "gemma-4")
                .is_none()
        );
    }

    #[test]
    fn test_disabled_cache_and_maintenance_ops() {
        let (tmp, cache_enabled) = test_cache();
        cache_enabled.set_page(
            "https://cached.com",
            &make_page("https://cached.com", "T", "Body", 1),
        );
        drop(cache_enabled);

        let cache_disabled = Cache::with_dir(tmp.path().to_path_buf(), false);
        let db = cache_disabled.open_database().unwrap();
        let record = NewSummaryRecord {
            style: SummaryStyle::Detailed.as_str(),
            model: "gemma-4",
            word_count: 420,
            reasoning: Some("Thought process"),
            ..make_summary(
                "book",
                "\"Dune\"",
                Some("Frank Herbert"),
                None,
                "Great novel.",
            )
        };
        db.record_summary(&record, Some(&Cache::book_cache_title("Dune", None)))
            .unwrap();
        assert!(
            cache_disabled
                .get_summary(
                    "book",
                    &Cache::book_cache_title("Dune", None),
                    SummaryStyle::Detailed,
                    "gemma-4"
                )
                .is_none()
        );
        assert_eq!(cache_disabled.stats().pages, 1);
        drop(db);
        drop(cache_disabled);

        let cache_reopened = Cache::with_dir(tmp.path().to_path_buf(), true);
        let hit = cache_reopened
            .get_summary(
                "book",
                &Cache::book_cache_title("Dune", None),
                SummaryStyle::Detailed,
                "gemma-4",
            )
            .expect("Should hit cache");
        assert_eq!(
            (hit.word_count, hit.author.as_deref()),
            (420, Some("Frank Herbert"))
        );
        drop(cache_reopened);

        let disabled = Cache::with_dir(tmp.path().to_path_buf(), false);
        let removed = disabled.clean().unwrap();
        assert!(removed.total_files >= 2);
        assert_eq!(disabled.stats().total_files, 0);
    }

    #[test]
    fn test_parse_duration_and_bytes() {
        for (input, secs) in [
            ("60s", 60),
            ("30m", 1800),
            ("24h", 86400),
            ("2w", 14 * 86400),
            ("5", 5 * 86400),
        ] {
            assert_eq!(parse_duration(input).unwrap(), Duration::from_secs(secs));
        }
        for invalid in ["invalid", "-5d", "NaN", "inf", "1000000000000000000000d"] {
            assert!(parse_duration(invalid).is_err(), "{invalid}");
        }
        for (input, bytes) in [
            ("500b", 500),
            ("10kb", 10 * 1024),
            ("500MB", 500 * 1024 * 1024),
            ("1048576", 1048576),
        ] {
            assert_eq!(parse_bytes(input).unwrap(), bytes);
        }
        for invalid in ["notabytes", "-500mb", "NaN", "9999999999999999999999999GB"] {
            assert!(parse_bytes(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn test_cache_ttl_and_timeframe_discrimination() {
        let (_tmp, mut cache) = test_cache();
        cache.policy.pages_ttl = Duration::from_secs(60);

        let url = "https://expiring.com";
        cache.set_page(url, &make_page(url, "Expiring", "Temporary content", 2));
        assert!(cache.get_page(url).is_some());

        let db = cache.open_database().unwrap();
        db.backdate_for_test(url, 120);
        assert!(cache.get_page(url).is_none());
        db.backdate_for_test(url, 10800);
        assert!(cache.get_news_page(url).is_none());

        let news_key = Cache::news_cache_title("python language news", NewsTimeframe::Week);
        db.record_summary(
            &NewSummaryRecord {
                model: "default",
                ..make_summary(
                    "news",
                    "News: python language news",
                    None,
                    None,
                    "Python 3.13.",
                )
            },
            Some(&news_key),
        )
        .unwrap();
        assert!(
            cache
                .get_summary("news", &news_key, SummaryStyle::Concise, "default")
                .is_some()
        );
        assert!(
            cache
                .get_summary(
                    "news",
                    &Cache::news_cache_title("python language news", NewsTimeframe::Day),
                    SummaryStyle::Concise,
                    "default"
                )
                .is_none()
        );
        db.backdate_for_test("News: python language news", 10800);
        assert!(
            cache
                .get_summary("news", &news_key, SummaryStyle::Concise, "default")
                .is_none()
        );
    }

    #[test]
    fn test_cache_prune_age_lru_and_auto_prune() {
        let (tmp, mut cache) = test_cache();
        let body = "A".repeat(1000);
        for i in 0..5 {
            let url = format!("https://example.com/{i}");
            cache.set_page(&url, &make_page(&url, &format!("Page {i}"), &body, 100));
        }
        cache.set_book_metadata("Dune", None, &None);
        let db = cache.open_database().unwrap();
        for i in 0..5 {
            db.backdate_for_test(&format!("https://example.com/{i}"), (4 - i) * 600);
        }

        let lru = cache
            .prune(PruneOptions {
                older_than: None,
                max_bytes: Some(2500),
            })
            .unwrap();
        assert!(lru.files_removed > 0 && lru.bytes_reclaimed > 0);
        assert!(cache.stats().total_bytes <= 2500);
        assert!(cache.get_page("https://example.com/0").is_none());
        assert!(cache.get_page("https://example.com/4").is_some());

        let age = cache
            .prune(PruneOptions {
                older_than: Some(Duration::from_secs(300)),
                max_bytes: None,
            })
            .unwrap();
        assert!(age.files_removed > 0);

        // Auto-prune respects per-table TTLs (14d search entry survives 7d pages_ttl).
        cache.policy.auto_prune_interval = Duration::from_secs(60);
        assert!(cache.maybe_auto_prune().is_some());
        assert!(cache.get_book_metadata("Dune", None).is_some());
        assert!(tmp.path().join(".last_prune").exists());
        assert!(cache.maybe_auto_prune().is_none());
    }
}
