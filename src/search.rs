use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
}

pub struct BookSearcher {
    client: reqwest::Client,
}

impl BookSearcher {
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("Failed to initialize search HTTP client")?;

        Ok(Self { client })
    }

    /// Searches for candidate book synopses concurrently across Wikipedia and DuckDuckGo,
    /// returning a deduplicated list of candidates.
    pub async fn search_candidates(&self, title: &str, author: Option<&str>) -> Vec<SearchResult> {
        let (wiki_res, ddg_res) = tokio::join!(
            self.search_wikipedia_fulltext(title, author),
            self.search_duckduckgo(title, author)
        );

        let mut raw_candidates = Vec::new();
        if let Ok(wiki_results) = wiki_res {
            raw_candidates.extend(wiki_results);
        }
        if let Ok(ddg_results) = ddg_res {
            raw_candidates.extend(ddg_results);
        }

        deduplicate_candidates(raw_candidates)
    }

    /// Queries Wikipedia full-text search API
    async fn search_wikipedia_fulltext(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let query = match author {
            Some(auth) => format!("{} {} novel OR book", title, auth),
            None => format!("{} novel OR book", title),
        };

        let endpoint = format!(
            "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&format=json&utf8=",
            urlencoding::encode(&query)
        );

        let resp = self.client.get(&endpoint).send().await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }

        let json: Value = resp.json().await?;
        let mut results = Vec::new();

        if let Some(items) = json.pointer("/query/search").and_then(|v| v.as_array()) {
            for item in items.iter().take(3) {
                if let Some(page_title) = item.get("title").and_then(|t| t.as_str()) {
                    let snippet = item.get("snippet").and_then(|s| s.as_str()).map(|raw_snippet| {
                        // Strip HTML tags from snippet
                        let fragment = Html::parse_fragment(raw_snippet);
                        fragment.root_element().text().collect::<String>()
                    });

                    let url_title = page_title.replace(' ', "_");
                    let url = format!(
                        "https://en.wikipedia.org/wiki/{}",
                        urlencoding::encode(&url_title)
                    );

                    results.push(SearchResult {
                        title: format!("{} (Wikipedia)", page_title),
                        url,
                        snippet,
                    });
                }
            }
        }

        Ok(results)
    }

    /// Queries DuckDuckGo HTML search for book synopses
    async fn search_duckduckgo(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let query = match author {
            Some(auth) => format!("\"{}\" \"{}\" book synopsis summary", title, auth),
            None => format!("\"{}\" book synopsis summary", title),
        };

        let endpoint = format!(
            "https://html.duckduckgo.com/html/?q={}",
            urlencoding::encode(&query)
        );

        let resp = self.client.get(&endpoint).send().await?;
        if !resp.status().is_success() {
            return Ok(Vec::new());
        }

        let html = resp.text().await?;
        let document = Html::parse_document(&html);

        let result_sel = Selector::parse(".result__body").unwrap();
        let title_sel = Selector::parse(".result__title a").unwrap();
        let snippet_sel = Selector::parse(".result__snippet").unwrap();

        let mut results = Vec::new();

        for element in document.select(&result_sel).take(3) {
            if let Some(link_elem) = element.select(&title_sel).next() {
                if let Some(raw_href) = link_elem.value().attr("href") {
                    let title_text: String = link_elem.text().collect();
                    let snippet: Option<String> = element
                        .select(&snippet_sel)
                        .next()
                        .map(|s| s.text().collect::<String>().trim().to_string());

                    let actual_url = extract_ddg_url(raw_href);
                    if !actual_url.is_empty() && actual_url.starts_with("http") {
                        results.push(SearchResult {
                            title: title_text.trim().to_string(),
                            url: actual_url,
                            snippet,
                        });
                    }
                }
            }
        }

        Ok(results)
    }
}

/// Extracts target destination URL from DuckDuckGo redirect link
pub fn extract_ddg_url(raw: &str) -> String {
    if let Some(pos) = raw.find("uddg=") {
        let remainder = &raw[pos + 5..];
        let encoded_url = remainder.split('&').next().unwrap_or(remainder);
        if let Ok(decoded) = urlencoding::decode(encoded_url) {
            return decoded.into_owned();
        }
    }
    raw.to_string()
}

/// Deduplicates search results by normalized (percent-decoded, trailing-slash-trimmed) URL.
fn deduplicate_candidates(candidates: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut unique = Vec::with_capacity(candidates.len());

    for item in candidates {
        let decoded = urlencoding::decode(&item.url)
            .map(|c| c.into_owned())
            .unwrap_or_else(|_| item.url.clone());
        let normalized = decoded.trim_end_matches('/').to_ascii_lowercase();
        if seen.insert(normalized) {
            unique.push(item);
        }
    }

    unique
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_ddg_url() {
        let ddg_link = "/l/?uddg=https%3A%2F%2Fen.wikipedia.org%2Fwiki%2FDune_%28novel%29&rut=123";
        let extracted = extract_ddg_url(ddg_link);
        assert_eq!(extracted, "https://en.wikipedia.org/wiki/Dune_(novel)");

        let direct_link = "https://example.com/summary";
        assert_eq!(extract_ddg_url(direct_link), direct_link);
    }

    #[test]
    fn test_deduplicate_candidates_normalizes_encoded_urls() {
        let list = vec![
            SearchResult {
                title: "Dune (novel) (Wikipedia)".to_string(),
                url: "https://en.wikipedia.org/wiki/Dune_%28novel%29".to_string(),
                snippet: None,
            },
            SearchResult {
                title: "Dune (novel) - Wikipedia".to_string(),
                url: "https://en.wikipedia.org/wiki/Dune_(novel)/".to_string(),
                snippet: Some("Duplicate URL from DDG".to_string()),
            },
            SearchResult {
                title: "Other Source".to_string(),
                url: "https://example.com/dune".to_string(),
                snippet: None,
            },
        ];

        let deduped = deduplicate_candidates(list);
        assert_eq!(deduped.len(), 2);
        assert_eq!(deduped[0].title, "Dune (novel) (Wikipedia)");
        assert_eq!(deduped[1].url, "https://example.com/dune");
    }
}
