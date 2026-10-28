use crate::cli::SummaryStyle;
use crate::scraper::ScrapedPage;
use crate::search::SearchResult;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter};
use std::path::{Path, PathBuf};

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
}

impl Cache {
    /// Creates a cache instance rooted in `~/.cache/srz` (or `$XDG_CACHE_HOME/srz`),
    /// falling back to the system temp directory if home is unavailable.
    pub fn new(enabled: bool) -> Self {
        let base_dir = Self::default_cache_dir();
        Self { base_dir, enabled }
    }

    /// Creates a cache instance rooted in a custom directory (useful for testing).
    pub fn with_dir(base_dir: PathBuf, enabled: bool) -> Self {
        Self { base_dir, enabled }
    }

    /// Returns the root cache directory path.
    pub fn dir(&self) -> &Path {
        &self.base_dir
    }

    /// Returns the directory where rendered `.html` and `.md` view files are saved.
    pub fn views_dir(&self) -> PathBuf {
        self.base_dir.join("views")
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
        Ok(current)
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

    fn default_cache_dir() -> PathBuf {
        if let Ok(xdg) = env::var("XDG_CACHE_HOME") {
            let trimmed = xdg.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join("srz");
            }
        }

        if let Ok(home) = env::var("HOME") {
            let trimmed = home.trim();
            if !trimmed.is_empty() {
                return PathBuf::from(trimmed).join(".cache").join("srz");
            }
        }

        env::temp_dir().join("srz-cache")
    }

    fn hash_key<H: Hash>(key: &H) -> String {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    fn read_entry<T: DeserializeOwned>(&self, namespace: &str, key_hash: &str) -> Option<T> {
        if !self.enabled {
            return None;
        }
        let path = self
            .base_dir
            .join(namespace)
            .join(format!("{}.json", key_hash));
        let file = File::open(path).ok()?;
        serde_json::from_reader(BufReader::new(file)).ok()
    }

    fn write_entry<T: Serialize>(&self, namespace: &str, key_hash: &str, value: &T) {
        let dir = self.base_dir.join(namespace);
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.json", key_hash));
        if let Ok(file) = File::create(path) {
            let _ = serde_json::to_writer_pretty(BufWriter::new(file), value);
        }
    }

    // --- Scraped Webpage Cache ---

    pub fn get_page(&self, url: &str) -> Option<ScrapedPage> {
        let key = Self::hash_key(&url.trim());
        self.read_entry("pages", &key)
    }

    pub fn set_page(&self, url: &str, page: &ScrapedPage) {
        let key = Self::hash_key(&url.trim());
        self.write_entry("pages", &key, page);
    }

    // --- Book Search Candidates Cache ---

    fn search_key(title: &str, author: Option<&str>) -> String {
        let normalized_key = (
            AsciiLower(title.trim()),
            author.map(|a| AsciiLower(a.trim())),
        );
        Self::hash_key(&normalized_key)
    }

    pub fn get_search_results(&self, title: &str, author: Option<&str>) -> Option<Vec<SearchResult>> {
        let key = Self::search_key(title, author);
        self.read_entry("search", &key)
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
        self.read_entry("summaries", &key)
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
        self.read_entry("summaries", &key)
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
