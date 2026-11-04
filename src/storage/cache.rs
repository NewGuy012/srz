use crate::cli::SummaryStyle;
use crate::engine::ServerCapabilities;
use crate::fs_util::{
    create_private_dir_all, non_empty_env_path, open_private_file_for_write, write_private_file,
};
use crate::scraper::ScrapedPage;
use crate::search::{BookMetadata, SearchResult};
use duckdb::{Connection, params};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSummary {
    pub title: String,
    pub summary: String,
    #[serde(default)]
    pub word_count: usize,
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
    pub word_count: usize,
}

#[derive(Serialize)]
struct CachedSummaryRef<'a> {
    title: &'a str,
    summary: &'a str,
    word_count: usize,
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

    pub fn ttl(&self, policy: &CachePolicy) -> Duration {
        match self {
            Self::Pages => policy.pages_ttl,
            Self::Search => policy.search_ttl,
            Self::Summaries => policy.summaries_ttl,
            Self::Views => policy.views_ttl,
        }
    }

    pub fn db_table(&self) -> Option<&'static str> {
        match self {
            Self::Pages => Some("cache_pages"),
            Self::Search => Some("cache_search"),
            Self::Summaries => Some("cache_summaries"),
            Self::Views => None,
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
    enabled: bool,
    policy: CachePolicy,
    db_conn: Mutex<Option<Connection>>,
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
            db_conn: Mutex::new(None),
        }
    }

    fn get_or_init_db(&self) -> Option<std::sync::MutexGuard<'_, Option<Connection>>> {
        if !self.enabled {
            return None;
        }
        let mut guard = self.db_conn.lock().ok()?;
        if guard.is_none() {
            let db_path = self.base_dir.join("cache.duckdb");
            if let Ok(meta) = fs::symlink_metadata(&db_path)
                && meta.file_type().is_symlink()
            {
                return None;
            }
            let already_exists =
                db_path.exists() && fs::metadata(&db_path).map(|m| m.len() > 0).unwrap_or(false);
            if let Some(parent) = db_path.parent() {
                let _ = create_private_dir_all(parent);
            }
            if let Ok(conn) = Connection::open(&db_path) {
                if db_path.exists() {
                    crate::fs_util::set_private_file_perms(&db_path);
                }
                let _ = conn.execute_batch(crate::db::DUCKDB_SECURITY_LOCKDOWN);
                let schema_ready = already_exists
                    && conn
                        .execute("SELECT 1 FROM cache_pages LIMIT 0", [])
                        .is_ok();
                if !schema_ready {
                    let _ = conn.execute_batch(
                        "CREATE TABLE IF NOT EXISTS cache_pages (
                             cache_key VARCHAR PRIMARY KEY,
                             url VARCHAR NOT NULL,
                             title VARCHAR,
                             content TEXT NOT NULL,
                             word_count UINTEGER NOT NULL,
                             cached_at TIMESTAMP NOT NULL DEFAULT current_timestamp
                         );
                         CREATE TABLE IF NOT EXISTS cache_search (
                             cache_key VARCHAR PRIMARY KEY,
                             data TEXT NOT NULL,
                             cached_at TIMESTAMP NOT NULL DEFAULT current_timestamp
                         );
                         CREATE TABLE IF NOT EXISTS cache_summaries (
                             cache_key VARCHAR PRIMARY KEY,
                             data TEXT NOT NULL,
                             cached_at TIMESTAMP NOT NULL DEFAULT current_timestamp
                         );
                         ALTER TABLE cache_summaries ADD COLUMN IF NOT EXISTS data TEXT;",
                    );
                }
                let _ = conn.execute_batch(
                    "CREATE TABLE IF NOT EXISTS cache_server_capabilities (
                         endpoint VARCHAR PRIMARY KEY,
                         model_id VARCHAR NOT NULL,
                         n_ctx UBIGINT NOT NULL,
                         total_slots UINTEGER NOT NULL DEFAULT 1,
                         supports_tokenize BOOLEAN NOT NULL DEFAULT FALSE,
                         tokenize_endpoint VARCHAR,
                         backend_kind VARCHAR NOT NULL,
                         cached_ms BIGINT NOT NULL DEFAULT 0
                     );",
                );
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

    /// Returns the path to the HTML view file for a given title.
    pub fn html_view_path(&self, title: &str) -> PathBuf {
        self.views_dir()
            .join(format!("{}.html", crate::render::slugify(title)))
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
        let (mut pages, mut pages_bytes) = Self::namespace_stats(&self.base_dir.join("pages"));
        let (mut searches, mut search_bytes) = Self::namespace_stats(&self.base_dir.join("search"));
        let (mut summaries, mut summary_bytes) =
            Self::namespace_stats(&self.base_dir.join("summaries"));
        let (views, views_bytes) = Self::namespace_stats(&self.base_dir.join("views"));

        if self.base_dir.join("cache.duckdb").exists()
            && let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            if let Ok((cnt, bytes)) = conn.query_row(
                "SELECT count(*)::BIGINT, coalesce(sum(strlen(url) + coalesce(strlen(title), 0) + strlen(content) + 32), 0)::BIGINT FROM cache_pages",
                [],
                |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)?.max(0) as u64)),
            ) {
                pages += cnt;
                pages_bytes += bytes;
            }
            if let Ok((cnt, bytes)) = conn.query_row(
                "SELECT count(*)::BIGINT, coalesce(sum(strlen(data)), 0)::BIGINT FROM cache_search",
                [],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)? as usize,
                        r.get::<_, i64>(1)?.max(0) as u64,
                    ))
                },
            ) {
                searches += cnt;
                search_bytes += bytes;
            }
            if let Ok((cnt, bytes)) = conn.query_row(
                "SELECT count(*)::BIGINT, coalesce(sum(strlen(data)), 0)::BIGINT FROM cache_summaries",
                [],
                |r| Ok((r.get::<_, i64>(0)? as usize, r.get::<_, i64>(1)?.max(0) as u64)),
            ) {
                summaries += cnt;
                summary_bytes += bytes;
            }
        }

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
        if self.base_dir.join("cache.duckdb").exists()
            && let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let _ = conn.execute_batch(
                "DELETE FROM cache_pages;
                 DELETE FROM cache_search;
                 DELETE FROM cache_summaries;",
            );
        }
        for ns in CacheNamespace::all() {
            let p = self.base_dir.join(ns.as_str());
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
        let mut bytes_reclaimed = 0u64;

        let should_run_size_eviction = options.max_bytes.is_some()
            || (options.older_than.is_none() && options.namespace.is_none());

        enum EvictTarget {
            DbRow {
                table: &'static str,
                cache_key: String,
            },
            DiskFile(PathBuf),
        }

        struct SurvivingEntry {
            target: EvictTarget,
            len: u64,
            timestamp_ms: i64,
        }

        let mut surviving_entries = Vec::new();
        let mut total_size = 0u64;

        // Pass 1a: Prune DuckDB cache entries by TTL & collect surviving DB rows for LRU size eviction
        if self.base_dir.join("cache.duckdb").exists()
            && let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let now_ms = now_epoch_ms();

            for ns in &namespaces {
                let Some(table) = ns.db_table() else { continue };
                let size_expr = if table == "cache_pages" {
                    "strlen(url) + coalesce(strlen(title), 0) + strlen(content) + 32"
                } else {
                    "strlen(data)"
                };

                let ttl = match options.older_than {
                    Some(dur) => dur,
                    None => ns.ttl(&self.policy),
                };
                if ttl != Duration::MAX {
                    let cutoff_ms = now_ms.saturating_sub(ttl.as_millis() as i64);
                    let stats_sql = format!(
                        "SELECT count(*)::BIGINT, coalesce(sum({size_expr}), 0)::BIGINT
                         FROM {table}
                         WHERE epoch_ms(cached_at)::BIGINT < ?"
                    );
                    if let Ok((expired_cnt, expired_bytes)) =
                        conn.query_row(&stats_sql, params![cutoff_ms], |r| {
                            Ok((
                                r.get::<_, i64>(0)? as usize,
                                r.get::<_, i64>(1)?.max(0) as u64,
                            ))
                        })
                        && expired_cnt > 0
                    {
                        let del_sql =
                            format!("DELETE FROM {table} WHERE epoch_ms(cached_at)::BIGINT < ?");
                        if conn.execute(&del_sql, params![cutoff_ms]).is_ok() {
                            files_removed += expired_cnt;
                            bytes_reclaimed += expired_bytes;
                        }
                    }
                }

                if should_run_size_eviction {
                    let list_sql = format!(
                        "SELECT cache_key, ({size_expr})::BIGINT, epoch_ms(cached_at)::BIGINT
                         FROM {table}"
                    );
                    if let Ok(mut stmt) = conn.prepare(&list_sql)
                        && let Ok(rows) = stmt.query_map([], |r| {
                            Ok((
                                r.get::<_, String>(0)?,
                                r.get::<_, i64>(1)?.max(0) as u64,
                                r.get::<_, i64>(2)?,
                            ))
                        })
                    {
                        for (cache_key, len, timestamp_ms) in rows.flatten() {
                            total_size += len;
                            surviving_entries.push(SurvivingEntry {
                                target: EvictTarget::DbRow { table, cache_key },
                                len,
                                timestamp_ms,
                            });
                        }
                    }
                }
            }
        }

        // Pass 1b: Prune filesystem files (views/ and any legacy JSON files) by TTL
        for ns in &namespaces {
            let dir = self.base_dir.join(ns.as_str());
            let Ok(entries) = fs::read_dir(&dir) else {
                continue;
            };

            let ttl = match options.older_than {
                Some(dur) => dur,
                None => ns.ttl(&self.policy),
            };

            for entry in entries.flatten() {
                let path = entry.path();
                if let Ok(meta) = entry.metadata()
                    && meta.is_file()
                {
                    let len = meta.len();
                    let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    let is_expired = if ttl != Duration::MAX {
                        mtime.elapsed().map(|el| el > ttl).unwrap_or(false)
                    } else {
                        false
                    };

                    if is_expired {
                        if fs::remove_file(&path).is_ok() {
                            files_removed += 1;
                            bytes_reclaimed += len;
                        }
                    } else if should_run_size_eviction {
                        let timestamp_ms = mtime
                            .duration_since(UNIX_EPOCH)
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        total_size += len;
                        surviving_entries.push(SurvivingEntry {
                            target: EvictTarget::DiskFile(path),
                            len,
                            timestamp_ms,
                        });
                    }
                }
            }
        }

        // Pass 2: Prune by total size (LRU eviction based on timestamp) across DB & disk
        if should_run_size_eviction {
            let max_budget = options.max_bytes.unwrap_or(self.policy.max_bytes);
            if max_budget < u64::MAX && total_size > max_budget {
                surviving_entries.sort_by_key(|e| e.timestamp_ms);

                let target_size = if options.max_bytes.is_some() {
                    max_budget
                } else {
                    ((max_budget as f64) * 0.85) as u64
                };

                let db_guard = self.get_or_init_db();
                let db_conn = db_guard.as_ref().and_then(|g| g.as_ref());

                for entry in surviving_entries {
                    if total_size <= target_size {
                        break;
                    }
                    let removed = match entry.target {
                        EvictTarget::DbRow { table, cache_key } => {
                            if let Some(conn) = db_conn {
                                let sql = format!("DELETE FROM {table} WHERE cache_key = ?");
                                conn.execute(&sql, params![cache_key]).is_ok()
                            } else {
                                false
                            }
                        }
                        EvictTarget::DiskFile(path) => fs::remove_file(&path).is_ok(),
                    };
                    if removed {
                        files_removed += 1;
                        bytes_reclaimed += entry.len;
                        total_size = total_size.saturating_sub(entry.len);
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
        if create_private_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.json", key_hash));
        if let Ok(file) = open_private_file_for_write(&path) {
            let _ = serde_json::to_writer(BufWriter::new(file), value);
        }
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

        // 1. Try DuckDB cache first
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let now_ms = now_epoch_ms();

            let table = match namespace {
                "search" => Some("cache_search"),
                "summaries" => Some("cache_summaries"),
                _ => None,
            };

            if let Some(tbl) = table {
                let sql = format!(
                    "SELECT data, epoch_ms(cached_at)::BIGINT
                     FROM {}
                     WHERE cache_key = ?",
                    tbl
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
                        let del_sql = format!("DELETE FROM {} WHERE cache_key = ?", tbl);
                        let _ = conn.execute(&del_sql, params![key_hash]);
                        let path = self
                            .base_dir
                            .join(namespace)
                            .join(format!("{}.json", key_hash));
                        let _ = fs::remove_file(path);
                        return None;
                    }
                    if let Ok(val) = serde_json::from_str(&json_data) {
                        return Some(val);
                    }
                }
            }
        }

        // 2. Fallback to file on disk
        self.read_disk_entry(namespace, key_hash, ttl)
    }

    fn write_entry<T: Serialize>(&self, namespace: &str, key_hash: &str, value: &T) {
        if !self.enabled {
            return;
        }

        // Save into consolidated DuckDB cache (sole write target when available)
        if let Some(guard) = self.get_or_init_db()
            && let Some(conn) = guard.as_ref()
        {
            let table = match namespace {
                "search" => Some("cache_search"),
                "summaries" => Some("cache_summaries"),
                _ => None,
            };
            if let Some(tbl) = table
                && let Ok(json_str) = serde_json::to_string(value)
            {
                let sql = format!(
                    "INSERT OR REPLACE INTO {} (cache_key, data, cached_at)
                     VALUES (?, ?, current_timestamp)",
                    tbl
                );
                if conn.execute(&sql, params![key_hash, json_str]).is_ok() {
                    return;
                }
            }
        }

        // Fallback to private disk file only if DuckDB is unavailable
        self.write_disk_entry(namespace, key_hash, value);
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
                    let _ =
                        conn.execute("DELETE FROM cache_pages WHERE cache_key = ?", params![&key]);
                    let path = self.base_dir.join("pages").join(format!("{}.json", key));
                    let _ = fs::remove_file(path);
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

    pub fn get_search_results(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<Vec<SearchResult>> {
        let key = Self::search_key(title, author);
        self.read_entry_with_ttl("search", &key, Some(self.policy.search_ttl))
    }

    pub fn set_search_results(&self, title: &str, author: Option<&str>, results: &[SearchResult]) {
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

    pub fn set_web_summary(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
        title: &str,
        summary: &str,
    ) {
        self.set_web_summary_with_words(url, style, model, title, summary, 0);
    }

    pub fn set_web_summary_with_words(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
        title: &str,
        summary: &str,
        word_count: usize,
    ) {
        let key = Self::summary_key("web", url, None, style, model, false);
        self.write_entry(
            "summaries",
            &key,
            &CachedSummaryRef {
                title,
                summary,
                word_count,
            },
        );
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
                word_count: params.word_count,
            },
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
            let sql = format!(
                "UPDATE {} SET cached_at = cached_at - INTERVAL {} SECOND WHERE {}",
                table, delta_secs, where_clause
            );
            let _ = conn.execute(&sql, []);
        }
    }
}
