use crate::config::cli::SummaryStyle;
use crate::engine::ServerCapabilities;
use crate::fs_util;
use crate::scraper::ScrapedPage;
use crate::sources::search::{BookMetadata, SearchResult};
use crate::storage::db::{Database, TABLE_CACHE_CAPS, TABLE_CACHE_PAGES, TABLE_CACHE_SEARCH};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CachedSummary {
    pub title: String,
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
            pages_ttl: Duration::from_secs(7 * 86400),
            news_ttl: Duration::from_secs(2 * 3600), // 2 hours
            search_ttl: Duration::from_secs(14 * 86400),
            summaries_ttl: Duration::from_secs(90 * 86400),
            views_ttl: Duration::from_secs(14 * 86400),
            caps_ttl: Duration::from_secs(7 * 86400),
            max_bytes: 500 * 1024 * 1024,                    // 500 MB
            auto_prune_interval: Duration::from_secs(86400), // 24 hours
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
        "b" | "bytes" | "" => val,
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

fn sha256_hex(data: &str) -> String {
    crate::system::sha256::Sha256::digest_hex(data.as_bytes())
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
            Self::default_cache_dir(),
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

    pub fn with_paths(
        base_dir: PathBuf,
        db_path: PathBuf,
        enabled: bool,
        policy: CachePolicy,
    ) -> Self {
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
        let unique_slug = crate::render::slugify_unique(title, unique_key);
        let unique_path = self.views_dir().join(format!("{}.html", unique_slug));
        if unique_path.exists() {
            return unique_path;
        }
        let legacy_slug = crate::render::slugify(title);
        let legacy_path = self.views_dir().join(format!("{}.html", legacy_slug));
        if legacy_path.exists() {
            return legacy_path;
        }
        unique_path
    }

    pub fn policy(&self) -> &CachePolicy {
        &self.policy
    }

    pub fn policy_mut(&mut self) -> &mut CachePolicy {
        &mut self.policy
    }

    pub fn set_policy(&mut self, policy: CachePolicy) {
        self.policy = policy;
    }

    fn get_or_init_db(&self) -> Option<Database> {
        if !self.enabled {
            return None;
        }
        let mut guard = self.db.lock().ok()?;
        if guard.is_none()
            && let Ok(database) = Database::open_at(&self.db_path)
        {
            *guard = Some(database);
        }
        guard.clone()
    }

    pub fn open_database(&self) -> Option<Database> {
        self.get_or_init_db()
    }

    pub fn default_cache_dir() -> PathBuf {
        crate::fs_util::default_cache_dir()
    }

    pub fn stats(&self) -> CacheStats {
        let (views, views_bytes) = Self::dir_stats(&self.views_dir());

        let mut pages = 0;
        let mut searches = 0;
        let mut summaries = 0;
        let mut pages_bytes = 0;
        let mut search_bytes = 0;
        let mut summaries_bytes = 0;

        if let Some(db) = self.get_or_init_db() {
            let (p_cnt, p_bytes, s_cnt, s_bytes, sum_cnt, sum_bytes) = db.cache_stats_breakdown();
            pages = p_cnt;
            pages_bytes = p_bytes;
            searches = s_cnt;
            search_bytes = s_bytes;
            summaries = sum_cnt;
            summaries_bytes = sum_bytes;
        }

        CacheStats {
            pages,
            searches,
            summaries,
            views,
            total_files: pages + searches + summaries + views,
            total_bytes: pages_bytes + search_bytes + summaries_bytes + views_bytes,
        }
    }

    pub fn clean(&self) -> std::io::Result<CacheStats> {
        let before = self.stats();
        if let Some(db) = self.get_or_init_db() {
            let _ = db.clear_cache_tables();
        }
        if self.views_dir().exists() {
            let _ = fs::remove_dir_all(self.views_dir());
        }
        Ok(before)
    }

    pub fn prune(&self, options: PruneOptions) -> std::io::Result<PruneStats> {
        let initial_stats = self.stats();
        let mut files_removed = 0;

        if let Some(db) = self.get_or_init_db() {
            if let Some(max_age) = options.older_than
                && let Ok(count) = db.prune_cache_by_age(max_age)
            {
                files_removed += count;
            }
            if let Some(max_bytes) = options.max_bytes
                && let Ok(count) = db.prune_cache_lru(max_bytes as usize)
            {
                files_removed += count;
            }
        }

        // Prune views
        if self.views_dir().exists() {
            let now = SystemTime::now();
            let ttl = options.older_than.unwrap_or(self.policy.views_ttl);
            if let Ok(entries) = fs::read_dir(self.views_dir()) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Ok(meta) = entry.metadata()
                        && let Ok(modified) = meta.modified()
                        && let Ok(age) = now.duration_since(modified)
                        && age > ttl
                        && fs::remove_file(&path).is_ok()
                    {
                        files_removed += 1;
                    }
                }
            }
        }

        let final_stats = self.stats();
        let bytes_reclaimed = initial_stats
            .total_bytes
            .saturating_sub(final_stats.total_bytes);

        Ok(PruneStats {
            files_removed,
            bytes_reclaimed,
        })
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

        let _ = fs_util::create_private_dir_all(&self.base_dir);
        let _ = fs_util::write_private_file(&marker, b"");

        self.prune(PruneOptions {
            older_than: Some(self.policy.pages_ttl),
            max_bytes: Some(self.policy.max_bytes),
        })
        .ok()
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
        let mut bytes = 0;
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    count += 1;
                    bytes += meta.len();
                }
            }
        }
        (count, bytes)
    }

    // Page cache
    pub fn get_page(&self, url: &str) -> Option<ScrapedPage> {
        self.get_page_with_ttl(url, self.policy.pages_ttl)
    }

    pub fn get_news_page(&self, url: &str) -> Option<ScrapedPage> {
        self.get_page_with_ttl(url, self.policy.news_ttl)
    }

    pub fn get_page_with_ttl(&self, url: &str, ttl: Duration) -> Option<ScrapedPage> {
        if !self.enabled {
            return None;
        }
        let db = self.get_or_init_db()?;
        let hash = sha256_hex(url);
        db.get_cached_json(TABLE_CACHE_PAGES, &hash, ttl)
    }

    pub fn set_page(&self, url: &str, page: &ScrapedPage) {
        if !self.enabled {
            return;
        }
        if let Some(db) = self.get_or_init_db() {
            let hash = sha256_hex(url);
            let _ = db.set_cached_json(TABLE_CACHE_PAGES, &hash, page);
        }
    }

    // Search cache
    fn search_key(title: &str, author: Option<&str>) -> String {
        let t = title.trim().to_ascii_lowercase();
        let a = author
            .map(|s| s.trim().to_ascii_lowercase())
            .unwrap_or_default();
        format!("{}:{}", t, a)
    }

    fn metadata_key(title: &str, author: Option<&str>) -> String {
        format!("meta:{}", Self::search_key(title, author))
    }

    pub fn get_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Option<BookMetadata>> {
        if !self.enabled {
            return None;
        }
        let db = self.get_or_init_db()?;
        let key = Self::metadata_key(title, author);
        db.get_cached_json(TABLE_CACHE_SEARCH, &key, self.policy.search_ttl)
    }

    pub fn set_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
        metadata: &Option<BookMetadata>,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(db) = self.get_or_init_db() {
            let key = Self::metadata_key(title, author);
            let _ = db.set_cached_json(TABLE_CACHE_SEARCH, &key, metadata);
        }
    }

    pub fn get_search_results(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Vec<SearchResult>> {
        if !self.enabled {
            return None;
        }
        let db = self.get_or_init_db()?;
        let key = Self::search_key(title, author);
        db.get_cached_json(TABLE_CACHE_SEARCH, &key, self.policy.search_ttl)
    }

    pub fn set_search_results(&self, title: &str, author: Option<&str>, results: &[SearchResult]) {
        if !self.enabled {
            return;
        }
        if let Some(db) = self.get_or_init_db() {
            let key = Self::search_key(title, author);
            let _ = db.set_cached_json(TABLE_CACHE_SEARCH, &key, results);
        }
    }

    // Summary cache
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

    pub fn get_web_summary(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        self.get_summary_from_db_with_ttl(
            "web",
            url,
            style.as_str(),
            model,
            self.policy.summaries_ttl,
        )
    }

    pub fn get_file_summary(
        &self,
        file_key: &str,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        self.get_summary_from_db_with_ttl(
            "file",
            file_key,
            style.as_str(),
            model,
            self.policy.summaries_ttl,
        )
    }

    pub fn get_book_summary(
        &self,
        title: &str,
        author: Option<&str>,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        let key = Self::book_cache_title(title, author);
        self.get_summary_from_db_with_ttl(
            "book",
            &key,
            style.as_str(),
            model,
            self.policy.summaries_ttl,
        )
    }

    pub fn get_news_summary(
        &self,
        topic: &str,
        timeframe: crate::cli::NewsTimeframe,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        let key = Self::news_cache_title(topic, timeframe);
        self.get_summary_from_db_with_ttl("news", &key, style.as_str(), model, self.policy.news_ttl)
    }

    fn get_summary_from_db_with_ttl(
        &self,
        target_type: &str,
        identifier: &str,
        style: &str,
        model: &str,
        ttl: Duration,
    ) -> Option<CachedSummary> {
        if !self.enabled {
            return None;
        }
        let db = self.get_or_init_db()?;
        db.get_cached_summary(target_type, identifier, style, model, ttl)
    }

    // Caps cache
    pub fn get_server_capabilities(
        &self,
        endpoint: &str,
        model: &str,
    ) -> Option<ServerCapabilities> {
        if !self.enabled {
            return None;
        }
        let db = self.get_or_init_db()?;
        let caps: ServerCapabilities =
            db.get_cached_json(TABLE_CACHE_CAPS, endpoint, self.policy.caps_ttl)?;
        if caps.model_id == model {
            Some(caps)
        } else {
            let _ = db.remove_cached_entry(TABLE_CACHE_CAPS, endpoint);
            None
        }
    }

    pub fn set_server_capabilities(&self, caps: &ServerCapabilities) {
        if !self.enabled {
            return;
        }
        if let Some(db) = self.get_or_init_db() {
            let _ = db.set_cached_json(TABLE_CACHE_CAPS, &caps.endpoint, caps);
        }
    }

    #[doc(hidden)]
    pub fn backdate_page_cache_for_test(&self, url: &str, delta_secs: u64) {
        if let Some(db) = self.get_or_init_db() {
            db.backdate_page_cache_for_test(url, delta_secs);
        }
    }

    #[doc(hidden)]
    pub fn backdate_history_for_test(&self, title: &str, delta_secs: u64) {
        if let Some(db) = self.get_or_init_db() {
            db.backdate_history_for_test(title, delta_secs);
        }
    }
}
