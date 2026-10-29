use crate::cli::SummaryStyle;
use crate::fs_util::{create_private_dir_all, non_empty_env_path, open_private_file_for_write};
use crate::scraper::ScrapedPage;
use crate::search::{BookMetadata, SearchResult};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSummary {
    pub title: String,
    pub summary: String,
}

#[derive(Debug, Clone)]
pub struct SetBookSummaryParams<'a> {
    pub title: &'a str,
    pub author: Option<&'a str>,
    pub style: SummaryStyle,
    pub model: &'a str,
    pub no_search: bool,
    pub display_title: &'a str,
    pub summary: &'a str,
}

#[derive(Serialize)]
struct CachedSummaryRef<'a> {
    title: &'a str,
    summary: &'a str,
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

/// Retention policies and storage limits across cache namespaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    /// Time-to-live for scraped webpage DOM and text (default: 7 days)
    pub pages_ttl: Duration,
    /// Time-to-live for book metadata and search engine candidates (default: 14 days)
    pub search_ttl: Duration,
    /// Time-to-live for generated LLM summaries (default: 90 days)
    pub summaries_ttl: Duration,
    /// Time-to-live for rendered HTML and Markdown views (default: 14 days)
    pub views_ttl: Duration,
    /// Maximum disk budget before oldest entries are pruned (default: 500 MB)
    pub max_bytes: u64,
    /// Minimum interval between automatic opportunistic pruning passes (default: 24 hours)
    pub auto_prune_interval: Duration,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self {
            pages_ttl: Duration::from_secs(7 * 86400),
            search_ttl: Duration::from_secs(14 * 86400),
            summaries_ttl: Duration::from_secs(90 * 86400),
            views_ttl: Duration::from_secs(14 * 86400),
            max_bytes: 500 * 1024 * 1024,                    // 500 MB
            auto_prune_interval: Duration::from_secs(86400), // 24 hours
        }
    }
}

/// Individual cache namespaces within `~/.cache/srz/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheNamespace {
    Pages,
    Search,
    Summaries,
    Views,
}

impl CacheNamespace {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pages => "pages",
            Self::Search => "search",
            Self::Summaries => "summaries",
            Self::Views => "views",
        }
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "pages" | "page" | "web" => Some(Self::Pages),
            "search" | "searches" | "meta" => Some(Self::Search),
            "summaries" | "summary" => Some(Self::Summaries),
            "views" | "view" | "html" => Some(Self::Views),
            _ => None,
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Pages, Self::Search, Self::Summaries, Self::Views]
    }
}

/// Options controlling targeted or automatic cache pruning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneOptions {
    pub older_than: Option<Duration>,
    pub max_bytes: Option<u64>,
    pub namespace: Option<CacheNamespace>,
}

/// Telemetry returned by a cache pruning run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneStats {
    pub files_removed: usize,
    pub bytes_reclaimed: u64,
}

/// Parses user-friendly duration strings like `7d`, `24h`, `30m`, `60s`, `2w`.
pub fn parse_duration(s: &str) -> Result<Duration, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty duration string".to_string());
    }
    let (num_str, unit) = match s.find(|c: char| !c.is_ascii_digit() && c != '.') {
        Some(idx) => (&s[..idx], s[idx..].trim().to_ascii_lowercase()),
        None => (s, "d".to_string()),
    };

    let val: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid number in duration: '{}'", num_str))?;
    if val < 0.0 {
        return Err("Duration cannot be negative".to_string());
    }

    let secs = match unit.as_str() {
        "s" | "sec" | "secs" | "second" | "seconds" => val,
        "m" | "min" | "mins" | "minute" | "minutes" => val * 60.0,
        "h" | "hr" | "hrs" | "hour" | "hours" => val * 3600.0,
        "d" | "day" | "days" => val * 86400.0,
        "w" | "week" | "weeks" => val * 7.0 * 86400.0,
        other => return Err(format!("Unknown duration unit: '{}' (use s, m, h, d, w)", other)),
    };

    Ok(Duration::from_secs_f64(secs))
}

/// Parses user-friendly byte strings like `500MB`, `1GB`, `100KB`, `1048576`.
pub fn parse_bytes(s: &str) -> Result<u64, String> {
    let s = s.trim();
    if s.is_empty() {
        return Err("Empty byte size string".to_string());
    }
    let (num_str, unit) = match s.find(|c: char| !c.is_ascii_digit() && c != '.') {
        Some(idx) => (&s[..idx], s[idx..].trim().to_ascii_lowercase()),
        None => (s, "b".to_string()),
    };

    let val: f64 = num_str
        .parse()
        .map_err(|_| format!("Invalid number in byte size: '{}'", num_str))?;
    if val < 0.0 {
        return Err("Byte size cannot be negative".to_string());
    }

    const KB: f64 = 1024.0;
    const MB: f64 = KB * 1024.0;
    const GB: f64 = MB * 1024.0;

    let bytes = match unit.as_str() {
        "b" | "bytes" | "" => val,
        "k" | "kb" | "kib" => val * KB,
        "m" | "mb" | "mib" => val * MB,
        "g" | "gb" | "gib" => val * GB,
        other => return Err(format!("Unknown byte unit: '{}' (use B, KB, MB, GB)", other)),
    };

    Ok(bytes as u64)
}

/// Zero-allocation wrapper that hashes a `&str` case-insensitively (ASCII) without allocating a `String`.
struct AsciiLower<'a>(&'a str);

impl Hash for AsciiLower<'_> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        for b in self.0.bytes() {
            state.write_u8(b.to_ascii_lowercase());
        }
        state.write_u8(0xff);
    }
}

pub struct Cache {
    base_dir: PathBuf,
    enabled: bool,
    policy: CachePolicy,
}

impl Cache {
    /// Creates a cache instance rooted in `~/.cache/srz` (or `$XDG_CACHE_HOME/srz`),
    /// falling back to the system temp directory if home is unavailable.
    pub fn new(enabled: bool) -> Self {
        Self::with_policy(Self::default_cache_dir(), enabled, CachePolicy::default())
    }

    /// Creates a cache instance rooted in a custom directory (useful for testing).
    pub fn with_dir(base_dir: PathBuf, enabled: bool) -> Self {
        Self::with_policy(base_dir, enabled, CachePolicy::default())
    }

    /// Creates a cache instance with a custom directory and retention policy.
    pub fn with_policy(base_dir: PathBuf, enabled: bool, policy: CachePolicy) -> Self {
        Self {
            base_dir,
            enabled,
            policy,
        }
    }

    /// Returns the root cache directory path.
    pub fn dir(&self) -> &Path {
        &self.base_dir
    }

    /// Returns the directory where rendered `.html` and `.md` view files are saved.
    pub fn views_dir(&self) -> PathBuf {
        self.base_dir.join("views")
    }

    /// Returns the active retention and storage policy.
    pub fn policy(&self) -> &CachePolicy {
        &self.policy
    }

    /// Returns a mutable reference to the active cache policy.
    pub fn policy_mut(&mut self) -> &mut CachePolicy {
        &mut self.policy
    }

    /// Overrides the active cache policy.
    pub fn set_policy(&mut self, policy: CachePolicy) {
        self.policy = policy;
    }

    /// Computes entry counts and total disk usage across all cache namespaces.
    pub fn stats(&self) -> CacheStats {
        let (pages, pages_bytes) = Self::namespace_stats(&self.base_dir.join("pages"));
        let (searches, search_bytes) = Self::namespace_stats(&self.base_dir.join("search"));
        let (summaries, summary_bytes) = Self::namespace_stats(&self.base_dir.join("summaries"));
        let (views, views_bytes) = Self::namespace_stats(&self.base_dir.join("views"));

        CacheStats {
            pages,
            searches,
            summaries,
            views,
            total_files: pages + searches + summaries + views,
            total_bytes: pages_bytes + search_bytes + summary_bytes + views_bytes,
        }
    }

    /// Removes all cached files in pages, search, summaries, and views, returning stats of what was cleared.
    pub fn clean(&self) -> std::io::Result<CacheStats> {
        let current = self.stats();
        for dir_name in &["pages", "search", "summaries", "views"] {
            let p = self.base_dir.join(dir_name);
            if p.exists() {
                fs::remove_dir_all(&p)?;
            }
        }
        let marker = self.base_dir.join(".last_prune");
        if marker.exists() {
            let _ = fs::remove_file(&marker);
        }
        Ok(current)
    }

    /// Prunes expired or excess cache entries based on TTL and disk usage limits.
    pub fn prune(&self, options: PruneOptions) -> std::io::Result<PruneStats> {
        if !self.enabled {
            return Ok(PruneStats::default());
        }

        let namespaces = match options.namespace {
            Some(ns) => vec![ns],
            None => CacheNamespace::all().to_vec(),
        };

        let mut files_removed = 0;
        let mut bytes_reclaimed = 0;

        // Pass 1: Prune by expiration / TTL
        for ns in &namespaces {
            let dir = self.base_dir.join(ns.as_str());
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            let ttl = match options.older_than {
                Some(dur) => dur,
                None => match ns {
                    CacheNamespace::Pages => self.policy.pages_ttl,
                    CacheNamespace::Search => self.policy.search_ttl,
                    CacheNamespace::Summaries => self.policy.summaries_ttl,
                    CacheNamespace::Views => self.policy.views_ttl,
                },
            };

            if ttl != Duration::MAX {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Ok(meta) = entry.metadata()
                        && meta.is_file()
                        && let Ok(mtime) = meta.modified()
                        && let Ok(elapsed) = mtime.elapsed()
                        && elapsed > ttl
                    {
                        let len = meta.len();
                        if fs::remove_file(&path).is_ok() {
                            files_removed += 1;
                            bytes_reclaimed += len;
                        }
                    }
                }
            }
        }

        // Pass 2: Prune by total size (LRU eviction based on mtime)
        let should_run_size_eviction = options.max_bytes.is_some()
            || (options.older_than.is_none() && options.namespace.is_none());

        if should_run_size_eviction {
            let max_budget = options.max_bytes.unwrap_or(self.policy.max_bytes);
            if max_budget < u64::MAX {
                struct FileInfo {
                    path: PathBuf,
                    len: u64,
                    mtime: SystemTime,
                }

                let mut all_files = Vec::new();
                let mut total_size = 0u64;

                for ns in &namespaces {
                    let dir = self.base_dir.join(ns.as_str());
                    let Ok(entries) = fs::read_dir(&dir) else {
                        continue;
                    };
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if let Ok(meta) = entry.metadata()
                            && meta.is_file()
                        {
                            let len = meta.len();
                            total_size += len;
                            let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                            all_files.push(FileInfo { path, len, mtime });
                        }
                    }
                }

                if total_size > max_budget {
                    all_files.sort_by_key(|f| f.mtime);

                    let target_size = if options.max_bytes.is_some() {
                        max_budget
                    } else {
                        ((max_budget as f64) * 0.85) as u64
                    };

                    for file in all_files {
                        if total_size <= target_size {
                            break;
                        }
                        if fs::remove_file(&file.path).is_ok() {
                            files_removed += 1;
                            bytes_reclaimed += file.len;
                            total_size = total_size.saturating_sub(file.len);
                        }
                    }
                }
            }
        }

        Ok(PruneStats {
            files_removed,
            bytes_reclaimed,
        })
    }

    /// Checks if the opportunistic pruning interval has elapsed (default: 24h),
    /// runs pruning in the background if needed, and updates the timestamp marker.
    pub fn maybe_auto_prune(&self) -> Option<PruneStats> {
        if !self.enabled || self.policy.auto_prune_interval == Duration::MAX {
            return None;
        }

        let marker = self.base_dir.join(".last_prune");
        if let Ok(meta) = fs::metadata(&marker)
            && let Ok(mtime) = meta.modified()
            && let Ok(elapsed) = mtime.elapsed()
            && elapsed < self.policy.auto_prune_interval
        {
            return None;
        }

        let stats = self.prune(PruneOptions::default()).ok()?;
        let _ = create_private_dir_all(&self.base_dir);
        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = fs::write(&marker, format!("{}", now_secs));
        Some(stats)
    }

    /// Formats a byte count into a human-readable string (e.g., `14.2 KB`, `1.8 MB`).
    pub fn format_bytes(bytes: u64) -> String {
        const KB: f64 = 1024.0;
        const MB: f64 = KB * 1024.0;
        const GB: f64 = MB * 1024.0;

        let b = bytes as f64;
        if bytes >= GB as u64 {
            format!("{:.2} GB", b / GB)
        } else if bytes >= MB as u64 {
            format!("{:.2} MB", b / MB)
        } else if bytes >= KB as u64 {
            format!("{:.1} KB", b / KB)
        } else {
            format!("{} B", bytes)
        }
    }

    fn namespace_stats(dir: &Path) -> (usize, u64) {
        let Ok(entries) = fs::read_dir(dir) else {
            return (0, 0);
        };
        let mut count = 0;
        let mut bytes = 0;
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata()
                && meta.is_file()
            {
                count += 1;
                bytes += meta.len();
            }
        }
        (count, bytes)
    }

    /// Returns the default root directory (`$XDG_CACHE_HOME/srz`, `~/.cache/srz`, or `$TMPDIR/srz-cache`).
    pub fn default_cache_dir() -> PathBuf {
        if let Some(xdg) = non_empty_env_path("XDG_CACHE_HOME") {
            return xdg.join("srz");
        }
        if let Some(home) = non_empty_env_path("HOME") {
            return home.join(".cache").join("srz");
        }
        env::temp_dir().join("srz-cache")
    }

    fn hash_key<H: Hash>(key: &H) -> String {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn read_entry_with_ttl<T: DeserializeOwned>(
        &self,
        namespace: &str,
        key_hash: &str,
        ttl: Option<Duration>,
    ) -> Option<T> {
        if !self.enabled {
            return None;
        }
        let path = self
            .base_dir
            .join(namespace)
            .join(format!("{}.json", key_hash));
        let file = File::open(&path).ok()?;

        if let Some(ttl) = ttl
            && ttl != Duration::MAX
            && let Ok(meta) = file.metadata()
            && let Ok(mtime) = meta.modified()
            && let Ok(elapsed) = mtime.elapsed()
            && elapsed > ttl
        {
            let _ = fs::remove_file(&path);
            return None;
        }

        serde_json::from_reader(BufReader::new(file)).ok()
    }

    fn write_entry<T: Serialize>(&self, namespace: &str, key_hash: &str, value: &T) {
        let dir = self.base_dir.join(namespace);
        if create_private_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.json", key_hash));
        if let Ok(file) = open_private_file_for_write(&path) {
            let _ = serde_json::to_writer(BufWriter::new(file), value);
        }
    }

    // --- Scraped Webpage Cache ---

    pub fn get_page(&self, url: &str) -> Option<ScrapedPage> {
        let ttl = if url.contains("youtube.com") || url.contains("youtu.be") {
            self.policy.pages_ttl.max(Duration::from_secs(30 * 86400))
        } else {
            self.policy.pages_ttl
        };
        self.get_page_with_ttl(url, ttl)
    }

    pub fn get_page_with_ttl(&self, url: &str, ttl: Duration) -> Option<ScrapedPage> {
        let key = Self::hash_key(&url.trim());
        self.read_entry_with_ttl("pages", &key, Some(ttl))
    }

    pub fn set_page(&self, url: &str, page: &ScrapedPage) {
        let key = Self::hash_key(&url.trim());
        self.write_entry("pages", &key, page);
    }

    // --- Book Metadata & Search Candidates Cache ---

    fn metadata_key(title: &str, author: Option<&str>) -> String {
        let normalized_key = (
            "meta",
            AsciiLower(title.trim()),
            author.map(|a| AsciiLower(a.trim())),
        );
        Self::hash_key(&normalized_key)
    }

    pub fn get_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Option<BookMetadata>> {
        let key = Self::metadata_key(title, author);
        self.read_entry_with_ttl("search", &key, Some(self.policy.search_ttl))
    }

    pub fn set_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
        metadata: &Option<BookMetadata>,
    ) {
        let key = Self::metadata_key(title, author);
        self.write_entry("search", &key, metadata);
    }

    fn search_key(title: &str, author: Option<&str>) -> String {
        let normalized_key = (
            AsciiLower(title.trim()),
            author.map(|a| AsciiLower(a.trim())),
        );
        Self::hash_key(&normalized_key)
    }

    pub fn get_search_results(&self, title: &str, author: Option<&str>) -> Option<Vec<SearchResult>> {
        let key = Self::search_key(title, author);
        self.read_entry_with_ttl("search", &key, Some(self.policy.search_ttl))
    }

    pub fn set_search_results(
        &self,
        title: &str,
        author: Option<&str>,
        results: &[SearchResult],
    ) {
        let key = Self::search_key(title, author);
        self.write_entry("search", &key, &results);
    }

    // --- Generated Summary Cache ---

    fn summary_key(
        kind: &str,
        target: &str,
        author: Option<&str>,
        style: SummaryStyle,
        model: &str,
        no_search: bool,
    ) -> String {
        let tuple = (
            kind,
            AsciiLower(target.trim()),
            author.map(|a| AsciiLower(a.trim())),
            style,
            model.trim(),
            no_search,
        );
        Self::hash_key(&tuple)
    }

    pub fn get_web_summary(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        let key = Self::summary_key("web", url, None, style, model, false);
        self.read_entry_with_ttl("summaries", &key, Some(self.policy.summaries_ttl))
    }

    pub fn set_web_summary(&self, url: &str, style: SummaryStyle, model: &str, title: &str, summary: &str) {
        let key = Self::summary_key("web", url, None, style, model, false);
        self.write_entry("summaries", &key, &CachedSummaryRef { title, summary });
    }

    pub fn get_book_summary(
        &self,
        title: &str,
        author: Option<&str>,
        style: SummaryStyle,
        model: &str,
        no_search: bool,
    ) -> Option<CachedSummary> {
        let key = Self::summary_key("book", title, author, style, model, no_search);
        self.read_entry_with_ttl("summaries", &key, Some(self.policy.summaries_ttl))
    }

    pub fn set_book_summary(&self, params: SetBookSummaryParams<'_>) {
        let key = Self::summary_key(
            "book",
            params.title,
            params.author,
            params.style,
            params.model,
            params.no_search,
        );
        self.write_entry(
            "summaries",
            &key,
            &CachedSummaryRef {
                title: params.display_title,
                summary: params.summary,
            },
        );
    }
}
