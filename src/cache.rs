use crate::cli::SummaryStyle;
use crate::scraper::ScrapedPage;
use crate::search::SearchResult;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSummary {
    pub title: String,
    pub summary: String,
}

#[derive(Serialize)]
struct CachedSummaryRef<'a> {
    title: &'a str,
    summary: &'a str,
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

    pub fn set_book_summary(
        &self,
        title: &str,
        author: Option<&str>,
        style: SummaryStyle,
        model: &str,
        no_search: bool,
        display_title: &str,
        summary: &str,
    ) {
        let key = Self::summary_key("book", title, author, style, model, no_search);
        self.write_entry(
            "summaries",
            &key,
            &CachedSummaryRef {
                title: display_title,
                summary,
            },
        );
    }
}
