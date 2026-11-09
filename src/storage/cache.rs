use crate::cli::SummaryStyle;
use crate::engine::ServerCapabilities;
use crate::fs_util::{create_private_dir_all, non_empty_env_path, write_private_file};
use crate::scraper::ScrapedPage;
use crate::search::{BookMetadata, SearchResult};
use crate::system::sha256::Sha256;
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::env;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSummary {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub word_count: usize,
    #[serde(default)]
    pub reasoning: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SetBookSummaryParams<'a> {
    pub title: &'a str,
    pub author: Option<&'a str>,
    pub style: SummaryStyle,
    pub model: &'a str,
    pub display_title: &'a str,
    pub summary: &'a str,
    pub word_count: usize,
    pub reasoning: Option<&'a str>,
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
    /// Time-to-live for scraped news articles (default: 2 hours)
    pub news_ttl: Duration,
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
            news_ttl: Duration::from_secs(2 * 3600), // 2 hours
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
    Views,
}

impl CacheNamespace {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pages => "pages",
            Self::Search => "search",
            Self::Views => "views",
        }
    }

    pub fn all() -> &'static [Self] {
        &[Self::Pages, Self::Search, Self::Views]
    }

    pub fn ttl(&self, policy: &CachePolicy) -> Duration {
        match self {
            Self::Pages => policy.pages_ttl,
            Self::Search => policy.search_ttl,
            Self::Views => policy.views_ttl,
        }
    }

    pub fn db_table(&self) -> Option<&'static str> {
        match self {
            Self::Pages => Some("cache_pages"),
            Self::Search => Some("cache_search"),
            Self::Views => None,
        }
    }

    pub fn db_size_expr(&self) -> &'static str {
        match self {
            Self::Pages => "strlen(url) + coalesce(strlen(title), 0) + strlen(content) + 32",
            Self::Search => "strlen(data)",
            Self::Views => "0",
        }
    }
}

fn now_epoch_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
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
        .map_err(|_| format!("Invalid number in {}: '{}'", type_label, num_str))?;
    if val < 0.0 || !val.is_finite() {
        return Err(format!("{} cannot be negative or infinite", type_label));
    }
    Ok((val, unit))
}

/// Parses user-friendly duration strings like `7d`, `24h`, `30m`, `60s`, `2w`.
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

/// Parses user-friendly byte strings like `500MB`, `1GB`, `100KB`, `1048576`.
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

struct Sha256Hasher(Sha256);

impl Hasher for Sha256Hasher {
    fn finish(&self) -> u64 {
        0
    }
    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}

pub struct Cache {
    base_dir: PathBuf,
    db_path: PathBuf,
    enabled: bool,
    policy: CachePolicy,
    db_conn: Mutex<Option<Connection>>,
}

impl Cache {
    /// Creates a cache instance with storage unified into `srz.duckdb`
    /// located in `~/.local/share/srz/srz.duckdb` (or `$SRZ_DATA_DIR/srz.duckdb`).
    pub fn new(enabled: bool) -> Self {
        Self::with_paths(
            Self::default_cache_dir(),
            crate::db::Database::default_path(),
            enabled,
            CachePolicy::default(),
        )
    }

    /// Creates a cache instance rooted in a custom directory (exposed for integration testing).
    #[doc(hidden)]
    pub fn with_dir(base_dir: PathBuf, enabled: bool) -> Self {
        let db_path = base_dir.join("srz.duckdb");
        Self::with_paths(base_dir, db_path, enabled, CachePolicy::default())
    }

    /// Creates a cache instance with explicit base cache directory and database file paths.
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
            db_conn: Mutex::new(None),
        }
    }

    /// Returns the active database path.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    fn get_or_init_db(&self) -> Option<std::sync::MutexGuard<'_, Option<Connection>>> {
        if !self.enabled {
            return None;
        }
        let mut guard = self.db_conn.lock().ok()?;
        if guard.is_none() {
            // Remove legacy cache.duckdb if present in base_dir
            let legacy_cache_db = self.base_dir.join("cache.duckdb");
            if legacy_cache_db.exists() && legacy_cache_db != self.db_path {
                let _ = fs::remove_file(&legacy_cache_db);
                let _ = fs::remove_file(self.base_dir.join("cache.duckdb.wal"));
            }

            if let Ok((conn, already_exists)) = crate::db::open_hardened_duckdb_file(&self.db_path)
            {
                let schema_ready =
                    already_exists && crate::storage::db::is_duckdb_schema_ready(&conn);
                if !schema_ready {
                    let _ = crate::storage::db::init_duckdb_schema(&conn);
                }
                *guard = Some(conn);
            }
        }
        Some(guard)
    }

    /// Returns the root cache directory path.
    pub fn dir(&self) -> &Path {
        &self.base_dir
    }

    /// Returns the directory where rendered `.html` and `.md` view files are saved.
    pub fn views_dir(&self) -> PathBuf {
        self.base_dir.join("views")
    }

    /// Returns the collision-resistant path to the HTML view file for a given title and optional unique key.
    pub fn html_view_path_for(&self, title: &str, unique_key: Option<&str>) -> PathBuf {
        let unique_slug = crate::render::slugify_unique(title, unique_key);
        let unique_path = self.views_dir().join(format!("{}.html", unique_slug));
        if unique_path.exists() {
            return unique_path;
        }
        // Fallback to legacy path without hash if exists
        let legacy_slug = crate::render::slugify(title);
        let legacy_path = self.views_dir().join(format!("{}.html", legacy_slug));
        if legacy_path.exists() {
            return legacy_path;
        }
        unique_path
    }

    /// Returns the active retention and storage policy.
    pub fn policy(&self) -> &CachePolicy {
        &self.policy
    }

    /// Obtains a Database handle sharing the DuckDB instance of this cache if initialized,
    /// avoiding redundant schema checks and file re-opens.
    pub fn open_database(&self) -> Option<crate::db::Database> {
        if let Some(guard) = self.get_or_init_db()
            && let Some(ref conn) = *guard
            && let Ok(cloned) = conn.try_clone()
        {
            return Some(crate::db::Database::from_connection(cloned));
        }
        crate::db::Database::open_at(&self.db_path).ok()
    }

    /// Returns a mutable reference to the active cache policy.
    /// Mutable access to cache policy (exposed for integration testing).
    #[doc(hidden)]
    pub fn policy_mut(&mut self) -> &mut CachePolicy {
        &mut self.policy
    }

    /// Overrides the active cache policy.
    pub fn set_policy(&mut self, policy: CachePolicy) {
        self.policy = policy;
    }

    fn query_table_stats(conn: &Connection, table: &str, size_expr: &str) -> (usize, u64) {
        conn.query_row(
            &format!("SELECT count(*)::BIGINT, coalesce(sum({size_expr}), 0)::BIGINT FROM {table}"),
            [],
            |r| {
                Ok((
                    r.get::<_, i64>(0)? as usize,
                    r.get::<_, i64>(1)?.max(0) as u64,
                ))
            },
        )
        .unwrap_or((0, 0))
    }

    /// Computes entry counts and total disk usage across all cache namespaces.
    pub fn stats(&self) -> CacheStats {
        let (mut pages, mut pages_bytes) = Self::namespace_stats(&self.base_dir.join("pages"));
        let (mut searches, mut search_bytes) = Self::namespace_stats(&self.base_dir.join("search"));
        let (views, views_bytes) = Self::namespace_stats(&self.views_dir());

        let mut summaries = 0;
        let mut summaries_bytes = 0;

        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let (cnt, bytes) =
                Self::query_table_stats(conn, "cache_pages", CacheNamespace::Pages.db_size_expr());
            pages += cnt;
            pages_bytes += bytes;

            let (cnt, bytes) = Self::query_table_stats(
                conn,
                "cache_search",
                CacheNamespace::Search.db_size_expr(),
            );
            searches += cnt;
            search_bytes += bytes;

            if let Ok(mut stmt) = conn.prepare(
                "SELECT count(*), coalesce(sum(strlen(summary) + coalesce(strlen(reasoning), 0)), 0)
                 FROM history h JOIN library l ON h.id = l.id
                 WHERE h.cache_key IS NOT NULL;",
            ) && let Ok(row) = stmt.query_row([], |row| {
                let cnt: i64 = row.get(0)?;
                let bytes: i64 = row.get(1)?;
                Ok((cnt.max(0) as usize, bytes.max(0) as u64))
            }) {
                summaries = row.0;
                summaries_bytes = row.1;
            }
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

    /// Removes all cached files in pages, search, and views, returning stats of what was cleared.
    pub fn clean(&self) -> std::io::Result<CacheStats> {
        let current = self.stats();
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let _ = conn.execute_batch(
                "DELETE FROM cache_pages;
                 DELETE FROM cache_search;
                 DELETE FROM cache_server_capabilities;
                 UPDATE history SET cache_key = NULL;
                 DROP TABLE IF EXISTS cache_summaries;
                 CHECKPOINT;
                 VACUUM;",
            );
        }
        for ns in CacheNamespace::all() {
            let p = self.base_dir.join(ns.as_str());
            if p.exists() {
                fs::remove_dir_all(&p)?;
            }
        }
        let legacy_summaries = self.base_dir.join("summaries");
        if legacy_summaries.exists() {
            let _ = fs::remove_dir_all(&legacy_summaries);
        }
        let marker = self.base_dir.join(".last_prune");
        if marker.exists() {
            let _ = fs::remove_file(&marker);
        }
        Ok(current)
    }

    /// Prunes expired or excess cache entries based on TTL and disk usage limits using native DuckDB window functions.
    pub fn prune(&self, options: PruneOptions) -> std::io::Result<PruneStats> {
        if !self.enabled {
            return Ok(PruneStats::default());
        }

        let initial_stats = self.stats();
        let mut files_removed = 0;

        {
            if let Some(guard) = self.get_or_init_db()
                && let Some(conn) = guard.as_ref()
            {
                let now_ms = now_epoch_ms();

                let should_prune_pages =
                    options.namespace.is_none() || options.namespace == Some(CacheNamespace::Pages);
                let should_prune_search = options.namespace.is_none()
                    || options.namespace == Some(CacheNamespace::Search);

                // Pass 1: Prune expired DuckDB cache entries by TTL in pure SQL
                if should_prune_pages {
                    let pages_ttl = match options.older_than {
                        Some(dur) => dur,
                        None => self.policy.pages_ttl,
                    };
                    if pages_ttl != Duration::MAX {
                        let cutoff_ms = now_ms.saturating_sub(pages_ttl.as_millis() as i64);
                        let sql = "DELETE FROM cache_pages WHERE epoch_ms(cached_at)::BIGINT < ?";
                        if let Ok(count) = conn.execute(sql, params![cutoff_ms]) {
                            files_removed += count;
                        }
                    }
                }

                if should_prune_search {
                    let search_ttl = match options.older_than {
                        Some(dur) => dur,
                        None => self.policy.search_ttl,
                    };
                    if search_ttl != Duration::MAX {
                        let cutoff_ms = now_ms.saturating_sub(search_ttl.as_millis() as i64);
                        let sql = "DELETE FROM cache_search WHERE epoch_ms(cached_at)::BIGINT < ?";
                        if let Ok(count) = conn.execute(sql, params![cutoff_ms]) {
                            files_removed += count;
                        }
                    }
                }

                // Pass 2: Prune excess size using pure DuckDB SQL window functions (SUM OVER ORDER BY cached_at DESC)
                let should_run_size_eviction = options.max_bytes.is_some()
                    || (options.older_than.is_none() && options.namespace.is_none());

                if should_run_size_eviction {
                    let max_budget = options.max_bytes.unwrap_or(self.policy.max_bytes);
                    let target_size = if options.max_bytes.is_some() {
                        max_budget
                    } else {
                        ((max_budget as f64) * 0.85) as u64
                    };

                    let pages_bytes = Self::query_table_stats(
                        conn,
                        "cache_pages",
                        CacheNamespace::Pages.db_size_expr(),
                    )
                    .1;
                    let search_bytes = Self::query_table_stats(
                        conn,
                        "cache_search",
                        CacheNamespace::Search.db_size_expr(),
                    )
                    .1;
                    let total_db_bytes = pages_bytes + search_bytes;

                    if total_db_bytes > target_size {
                        if should_prune_pages && pages_bytes > 0 {
                            let pages_budget = if search_bytes == 0 {
                                target_size
                            } else {
                                ((target_size as f64)
                                    * (pages_bytes as f64 / total_db_bytes as f64))
                                    as u64
                            };
                            let sql = "DELETE FROM cache_pages
                                       WHERE cache_key IN (
                                           SELECT cache_key FROM (
                                               SELECT cache_key,
                                                      SUM(strlen(url) + coalesce(strlen(title), 0) + strlen(content) + 32)
                                                          OVER (ORDER BY cached_at DESC) AS running_bytes
                                               FROM cache_pages
                                           ) sub
                                           WHERE running_bytes > ?
                                       )";
                            if let Ok(count) = conn.execute(sql, params![pages_budget as i64]) {
                                files_removed += count;
                            }
                        }

                        if should_prune_search && search_bytes > 0 {
                            let search_budget = if pages_bytes == 0 {
                                target_size
                            } else {
                                ((target_size as f64)
                                    * (search_bytes as f64 / total_db_bytes as f64))
                                    as u64
                            };
                            let sql = "DELETE FROM cache_search
                                       WHERE cache_key IN (
                                           SELECT cache_key FROM (
                                               SELECT cache_key,
                                                      SUM(strlen(data))
                                                          OVER (ORDER BY cached_at DESC) AS running_bytes
                                               FROM cache_search
                                           ) sub
                                           WHERE running_bytes > ?
                                       )";
                            if let Ok(count) = conn.execute(sql, params![search_budget as i64]) {
                                files_removed += count;
                            }
                        }

                        let _ = conn.execute_batch("CHECKPOINT; VACUUM;");
                    }
                }
            }
        }

        // Pass 3: Prune views/ files on disk
        let views_dir = self.views_dir();
        if views_dir.exists()
            && (options.namespace.is_none() || options.namespace == Some(CacheNamespace::Views))
        {
            let views_ttl = match options.older_than {
                Some(dur) => dur,
                None => self.policy.views_ttl,
            };
            if let Ok(entries) = fs::read_dir(&views_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if let Ok(meta) = entry.metadata()
                        && meta.is_file()
                        && let Ok(mtime) = meta.modified()
                        && let Ok(elapsed) = mtime.elapsed()
                        && views_ttl != Duration::MAX
                        && elapsed > views_ttl
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
        let now_secs = now_epoch_ms() / 1000;
        let _ = write_private_file(&marker, format!("{}", now_secs).as_bytes());
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
        let mut hasher = Sha256Hasher(Sha256::new());
        key.hash(&mut hasher);
        let result = hasher.0.finalize();
        crate::system::text_util::format_hex(&result)
    }

    fn read_disk_entry<T: DeserializeOwned>(
        &self,
        namespace: &str,
        key_hash: &str,
        ttl: Option<Duration>,
    ) -> Option<T> {
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

    fn write_disk_entry<T: Serialize>(&self, namespace: &str, key_hash: &str, value: &T) {
        let dir = self.base_dir.join(namespace);
        let path = dir.join(format!("{}.json", key_hash));
        if let Ok(bytes) = serde_json::to_vec(value) {
            let _ = write_private_file(&path, bytes);
        }
    }

    fn delete_expired_entry(&self, conn: &Connection, table: &str, namespace_dir: &str, key: &str) {
        let del_sql = format!("DELETE FROM {table} WHERE cache_key = ?");
        let _ = conn.execute(&del_sql, params![key]);
        let path = self
            .base_dir
            .join(namespace_dir)
            .join(format!("{key}.json"));
        let _ = fs::remove_file(path);
    }

    fn read_entry_with_ttl<T: DeserializeOwned>(
        &self,
        ns: CacheNamespace,
        key_hash: &str,
        ttl: Option<Duration>,
    ) -> Option<T> {
        if !self.enabled {
            return None;
        }

        // 1. Try DuckDB cache first
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
            && let Some(tbl) = ns.db_table()
        {
            let now_ms = now_epoch_ms();
            let sql = format!(
                "SELECT data, epoch_ms(cached_at)::BIGINT
                 FROM {tbl}
                 WHERE cache_key = ?"
            );
            if let Ok(mut stmt) = conn.prepare(&sql)
                && let Ok(mut rows) = stmt.query(params![key_hash])
                && let Some(row) = rows.next().ok().flatten()
                && let (Ok(json_data), Ok(cached_ms)) =
                    (row.get::<_, String>(0), row.get::<_, i64>(1))
            {
                if let Some(ttl_dur) = ttl
                    && ttl_dur != Duration::MAX
                    && (now_ms - cached_ms) > ttl_dur.as_millis() as i64
                {
                    self.delete_expired_entry(conn, tbl, ns.as_str(), key_hash);
                    return None;
                }
                if let Ok(val) = serde_json::from_str(&json_data) {
                    return Some(val);
                }
            }
        }

        // 2. Fallback to file on disk
        self.read_disk_entry(ns.as_str(), key_hash, ttl)
    }

    fn write_entry<T: Serialize>(&self, ns: CacheNamespace, key_hash: &str, value: &T) {
        if !self.enabled {
            return;
        }

        // Save into consolidated DuckDB cache (sole write target when available)
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
            && let Some(tbl) = ns.db_table()
            && let Ok(json_str) = serde_json::to_string(value)
        {
            let sql = format!(
                "INSERT OR REPLACE INTO {tbl} (cache_key, data, cached_at)
                 VALUES (?, ?, current_timestamp)"
            );
            if conn.execute(&sql, params![key_hash, json_str]).is_ok() {
                return;
            }
        }

        // Fallback to private disk file only if DuckDB is unavailable
        self.write_disk_entry(ns.as_str(), key_hash, value);
    }

    // --- Scraped Webpage Cache ---

    pub fn get_page(&self, url: &str) -> Option<ScrapedPage> {
        let ttl = if url.contains("news.google.com")
            || url.contains("news.ycombinator.com")
            || url.contains("/rss")
            || url.contains("/feed")
        {
            self.policy.news_ttl
        } else if url.contains("youtube.com") || url.contains("youtu.be") {
            self.policy.pages_ttl.max(Duration::from_secs(30 * 86400))
        } else {
            self.policy.pages_ttl
        };
        self.get_page_with_ttl(url, ttl)
    }

    /// Fetches a cached webpage specifically under the short news TTL (default: 2 hours).
    pub fn get_news_page(&self, url: &str) -> Option<ScrapedPage> {
        self.get_page_with_ttl(url, self.policy.news_ttl)
    }

    pub fn get_page_with_ttl(&self, url: &str, ttl: Duration) -> Option<ScrapedPage> {
        let key = Self::hash_key(&url.trim());
        if !self.enabled {
            return None;
        }

        // 1. Try DuckDB cache first
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let now_ms = now_epoch_ms();

            if let Ok(mut stmt) = conn.prepare(
                "SELECT url, title, content, word_count, epoch_ms(cached_at)::BIGINT
                 FROM cache_pages
                 WHERE cache_key = ?",
            ) && let Ok(mut rows) = stmt.query(params![&key])
                && let Some(row) = rows.next().ok().flatten()
            {
                let page_url: String = row.get(0).unwrap_or_default();
                let title: Option<String> = row.get(1).ok();
                let content: String = row.get(2).unwrap_or_default();
                let word_count: usize = row.get::<_, u32>(3).unwrap_or(0) as usize;
                let cached_ms: i64 = row.get(4).unwrap_or(0);

                if ttl != Duration::MAX && (now_ms - cached_ms) > ttl.as_millis() as i64 {
                    self.delete_expired_entry(conn, "cache_pages", "pages", &key);
                    return None;
                }

                return Some(ScrapedPage {
                    url: page_url,
                    title,
                    content,
                    word_count,
                });
            }
        }

        // 2. Fallback to file on disk
        self.read_disk_entry("pages", &key, Some(ttl))
    }

    pub fn set_page(&self, url: &str, page: &ScrapedPage) {
        let key = Self::hash_key(&url.trim());
        if !self.enabled {
            return;
        }

        // 1. Save into consolidated DuckDB cache (sole write target when available)
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
            && conn
                .execute(
                    "INSERT OR REPLACE INTO cache_pages (cache_key, url, title, content, word_count, cached_at)
                     VALUES (?, ?, ?, ?, ?, current_timestamp)",
                    params![&key, &page.url, &page.title, &page.content, page.word_count as u32],
                )
                .is_ok()
        {
            return;
        }

        // 2. Fallback to private disk file only if DuckDB is unavailable
        self.write_disk_entry("pages", &key, page);
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
        self.read_entry_with_ttl(CacheNamespace::Search, &key, Some(self.policy.search_ttl))
    }

    pub fn set_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
        metadata: &Option<BookMetadata>,
    ) {
        let key = Self::metadata_key(title, author);
        self.write_entry(CacheNamespace::Search, &key, metadata);
    }

    fn search_key(title: &str, author: Option<&str>) -> String {
        let normalized_key = (
            AsciiLower(title.trim()),
            author.map(|a| AsciiLower(a.trim())),
        );
        Self::hash_key(&normalized_key)
    }

    pub fn get_search_results(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Vec<SearchResult>> {
        let key = Self::search_key(title, author);
        self.read_entry_with_ttl(CacheNamespace::Search, &key, Some(self.policy.search_ttl))
    }

    pub fn set_search_results(&self, title: &str, author: Option<&str>, results: &[SearchResult]) {
        let key = Self::search_key(title, author);
        self.write_entry(CacheNamespace::Search, &key, &results);
    }

    // --- Unified Summary Retrieval & Storage (Single Source of Truth in srz.duckdb) ---

    fn get_summary_from_db(
        &self,
        target_type: &str,
        identifier: &str,
        style: &str,
        model: &str,
    ) -> Option<CachedSummary> {
        self.get_summary_from_db_with_ttl(
            target_type,
            identifier,
            style,
            model,
            self.policy.summaries_ttl,
        )
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
        let guard = self.get_or_init_db()?;
        let conn = guard.as_ref()?;
        let now_ms = now_epoch_ms();

        let trimmed_id = identifier.trim();
        let sql =
            "SELECT h.title, l.summary, h.word_count, l.reasoning, epoch_ms(h.created_at)::BIGINT
                   FROM history h
                   JOIN library l ON h.id = l.id
                   WHERE h.target_type = ?
                     AND h.cache_key IS NOT NULL
                     AND (h.cache_key = ? OR h.url = ? OR lower(trim(h.title)) = lower(trim(?)))
                     AND h.style = ?
                     AND h.model = ?
                   ORDER BY h.id DESC
                   LIMIT 1";

        if let Ok(mut stmt) = conn.prepare(sql)
            && let Ok(mut rows) = stmt.query(params![
                target_type,
                trimmed_id,
                trimmed_id,
                trimmed_id,
                style,
                model
            ])
            && let Some(row) = rows.next().ok().flatten()
        {
            let title: String = row.get(0).unwrap_or_default();
            let summary: String = row.get(1).unwrap_or_default();
            let word_count: usize = row.get::<_, u32>(2).unwrap_or(0) as usize;
            let reasoning: Option<String> = row.get(3).ok();
            let created_ms: i64 = row.get(4).unwrap_or(0);

            if ttl != Duration::MAX && (now_ms - created_ms) > ttl.as_millis() as i64 {
                return None;
            }

            return Some(CachedSummary {
                title,
                summary,
                word_count,
                reasoning,
            });
        }
        None
    }

    #[allow(clippy::too_many_arguments)]
    fn set_summary_in_db(
        &self,
        target_type: &str,
        cache_key: &str,
        url: Option<&str>,
        title: &str,
        summary: &str,
        word_count: usize,
        reasoning: Option<&str>,
        style: &str,
        model: &str,
    ) {
        if !self.enabled {
            return;
        }
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let record = crate::storage::db::NewSummaryRecord {
                target_type,
                title,
                author: None,
                url,
                style,
                model,
                summary,
                word_count,
                duration_ms: 0,
                prompt_tokens: None,
                completion_tokens: None,
                tokens_per_second: None,
                reasoning,
            };
            let _ = crate::storage::db::record_summary_on_conn(conn, &record, Some(cache_key));
        }
    }

    pub fn get_web_summary(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        self.get_summary_from_db("web", url, style.as_str(), model)
    }

    #[doc(hidden)]
    pub fn set_web_summary(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
        title: &str,
        summary: &str,
    ) {
        self.set_summary_in_db(
            "web",
            url,
            Some(url),
            title,
            summary,
            0,
            None,
            style.as_str(),
            model,
        );
    }

    pub fn get_file_summary(
        &self,
        file_key: &str,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        self.get_summary_from_db("file", file_key, style.as_str(), model)
    }

    /// Computes canonical cache lookup key for a book title and optional author.
    pub fn book_cache_title(title: &str, author: Option<&str>) -> String {
        match author {
            Some(a) if !a.trim().is_empty() => format!("\"{}\" by {}", title.trim(), a.trim()),
            _ => title.trim().to_string(),
        }
    }

    /// Computes canonical cache lookup key for a news topic and timeframe.
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

    pub fn get_book_summary(
        &self,
        title: &str,
        author: Option<&str>,
        style: SummaryStyle,
        model: &str,
    ) -> Option<CachedSummary> {
        let key = Self::book_cache_title(title, author);
        self.get_summary_from_db("book", &key, style.as_str(), model)
    }

    #[doc(hidden)]
    pub fn set_book_summary(&self, params: SetBookSummaryParams<'_>) {
        let key = Self::book_cache_title(params.title, params.author);
        self.set_summary_in_db(
            "book",
            &key,
            None,
            params.display_title,
            params.summary,
            params.word_count,
            params.reasoning,
            params.style.as_str(),
            params.model,
        );
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

    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn set_news_summary(
        &self,
        topic: &str,
        timeframe: crate::cli::NewsTimeframe,
        style: SummaryStyle,
        model: &str,
        title: &str,
        summary: &str,
        word_count: usize,
        reasoning: Option<&str>,
    ) {
        let search_url = crate::sources::news::GoogleNewsClient::search_url_for_topic(topic);
        let key = Self::news_cache_title(topic, timeframe);
        self.set_summary_in_db(
            "news",
            &key,
            Some(&search_url),
            title,
            summary,
            word_count,
            reasoning,
            style.as_str(),
            model,
        );
    }

    // --- Server Capabilities Cache ---

    pub fn get_server_capabilities(
        &self,
        endpoint: &str,
        current_model_id: &str,
    ) -> Option<ServerCapabilities> {
        if !self.enabled {
            return None;
        }

        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let now_ms = now_epoch_ms();
            let max_age_ms = 86_400_000; // 24 hours

            if let Ok(mut stmt) = conn.prepare(
                "SELECT endpoint, model_id, n_ctx, total_slots, supports_tokenize, tokenize_endpoint, backend_kind, cached_ms
                 FROM cache_server_capabilities
                 WHERE endpoint = ?",
            ) && let Ok(mut rows) = stmt.query(params![endpoint])
                && let Some(row) = rows.next().ok().flatten()
            {
                let ep: String = row.get(0).unwrap_or_default();
                let model: String = row.get(1).unwrap_or_default();
                let n_ctx: usize = row.get::<_, u64>(2).unwrap_or(16384) as usize;
                let total_slots: usize = row.get::<_, u32>(3).unwrap_or(1) as usize;
                let supports_tokenize: bool = row.get(4).unwrap_or(false);
                let tokenize_endpoint: Option<String> = row.get(5).ok();
                let backend_kind: String = row.get(6).unwrap_or_default();
                let cached_ms: i64 = row.get(7).unwrap_or(0);

                // Invalidation trigger: if the model changed or entry is older than 24h, invalidate
                if model != current_model_id || (now_ms - cached_ms) > max_age_ms {
                    let _ = conn.execute(
                        "DELETE FROM cache_server_capabilities WHERE endpoint = ?",
                        params![endpoint],
                    );
                    return None;
                }

                return Some(ServerCapabilities {
                    endpoint: ep,
                    model_id: model,
                    n_ctx,
                    total_slots,
                    supports_tokenize,
                    tokenize_endpoint,
                    backend_kind,
                });
            }
        }
        None
    }

    pub fn set_server_capabilities(&self, caps: &ServerCapabilities) {
        if !self.enabled {
            return;
        }

        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let now_ms = now_epoch_ms();
            let _ = conn.execute(
                "INSERT OR REPLACE INTO cache_server_capabilities
                 (endpoint, model_id, n_ctx, total_slots, supports_tokenize, tokenize_endpoint, backend_kind, cached_ms)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    &caps.endpoint,
                    &caps.model_id,
                    caps.n_ctx as u64,
                    caps.total_slots as u32,
                    caps.supports_tokenize,
                    &caps.tokenize_endpoint,
                    &caps.backend_kind,
                    now_ms,
                ],
            );
        }
    }

    #[doc(hidden)]
    pub fn update_cached_at_for_test(&self, table: &str, where_clause: &str, delta_secs: i64) {
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let col = if table == "history" {
                "created_at"
            } else {
                "cached_at"
            };
            let sql = format!(
                "UPDATE {} SET {} = {} - INTERVAL {} SECOND WHERE {}",
                table, col, col, delta_secs, where_clause
            );
            let _ = conn.execute(&sql, []);
        }
    }
}
