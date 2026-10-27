use crate::cli::SummaryStyle;
use crate::scraper::ScrapedPage;
use crate::search::SearchResult;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CachedSummary {
    pub title: String,
    pub summary: String,
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

    #[cfg(test)]
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
        let data = fs::read_to_string(path).ok()?;
        serde_json::from_str(&data).ok()
    }

    fn write_entry<T: Serialize>(&self, namespace: &str, key_hash: &str, value: &T) {
        let dir = self.base_dir.join(namespace);
        if fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("{}.json", key_hash));
        if let Ok(json) = serde_json::to_string_pretty(value) {
            let _ = fs::write(path, json);
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

    pub fn get_search_results(&self, title: &str, author: Option<&str>) -> Option<Vec<SearchResult>> {
        let normalized_key = (
            title.trim().to_ascii_lowercase(),
            author.map(|a| a.trim().to_ascii_lowercase()),
        );
        let key = Self::hash_key(&normalized_key);
        self.read_entry("search", &key)
    }

    pub fn set_search_results(
        &self,
        title: &str,
        author: Option<&str>,
        results: &[SearchResult],
    ) {
        let normalized_key = (
            title.trim().to_ascii_lowercase(),
            author.map(|a| a.trim().to_ascii_lowercase()),
        );
        let key = Self::hash_key(&normalized_key);
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
            target.trim().to_ascii_lowercase(),
            author.map(|a| a.trim().to_ascii_lowercase()),
            format!("{:?}", style),
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

    pub fn set_web_summary(
        &self,
        url: &str,
        style: SummaryStyle,
        model: &str,
        summary: &CachedSummary,
    ) {
        let key = Self::summary_key("web", url, None, style, model, false);
        self.write_entry("summaries", &key, summary);
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
        summary: &CachedSummary,
    ) {
        let key = Self::summary_key("book", title, author, style, model, no_search);
        self.write_entry("summaries", &key, summary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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

        let loaded = cache.get_page("https://example.com").expect("Should hit cache");
        assert_eq!(loaded.title.as_deref(), Some("Example"));
        assert_eq!(loaded.word_count, 3);

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
}
