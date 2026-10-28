use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::sync::LazyLock;
use std::time::Duration;

static DDG_RESULT_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result__body").unwrap());
static DDG_TITLE_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result__title a").unwrap());
static DDG_SNIPPET_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse(".result__snippet").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub snippet: Option<String>,
    #[serde(default)]
    pub score: i32,
}

impl SearchResult {
    pub fn new(title: impl Into<String>, url: impl Into<String>, snippet: Option<String>) -> Self {
        Self {
            title: title.into(),
            url: url.into(),
            snippet,
            score: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookMetadata {
    pub canonical_title: String,
    pub author: Option<String>,
    pub first_publish_year: Option<u32>,
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
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("Failed to initialize search HTTP client")?;

        Ok(Self { client })
    }

    /// Queries Open Library API to resolve canonical book title, author, and first publication year.
    /// Returns None if the query times out (3s), fails, or yields no close match.
    pub async fn resolve_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<BookMetadata> {
        let mut endpoint = format!(
            "https://openlibrary.org/search.json?title={}&limit=3&fields=title,author_name,first_publish_year",
            urlencoding::encode(title)
        );
        if let Some(auth) = author {
            endpoint.push_str("&author=");
            endpoint.push_str(&urlencoding::encode(auth));
        }

        let fetch_fut = self.client.get(&endpoint).send();
        let resp = match tokio::time::timeout(Duration::from_secs(3), fetch_fut).await {
            Ok(Ok(r)) if r.status().is_success() => r,
            _ => return None,
        };

        let json: Value = resp.json().await.ok()?;
        let docs = json.get("docs")?.as_array()?;

        let norm_req = normalize_title_for_comparison(title);

        for doc in docs {
            let doc_title = doc.get("title")?.as_str()?;
            let norm_doc = normalize_title_for_comparison(doc_title);

            // Require close title match
            if norm_doc == norm_req || norm_doc.contains(&norm_req) || norm_req.contains(&norm_doc) {
                let author_name = doc
                    .get("author_name")
                    .and_then(|a| a.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                let first_publish_year = doc
                    .get("first_publish_year")
                    .and_then(|y| y.as_u64())
                    .map(|y| y as u32);

                return Some(BookMetadata {
                    canonical_title: doc_title.to_string(),
                    author: author_name,
                    first_publish_year,
                });
            }
        }

        None
    }

    /// Searches for candidate book synopses concurrently across Wikipedia (OpenSearch + fulltext) and DuckDuckGo,
    /// returning a deduplicated and scored/ranked list of candidates.
    pub async fn search_candidates(&self, title: &str, author: Option<&str>) -> Vec<SearchResult> {
        let (opensearch_res, fulltext_res, ddg_res) = tokio::join!(
            self.search_wikipedia_opensearch(title),
            self.search_wikipedia_fulltext(title, author),
            self.search_duckduckgo(title, author)
        );

        let mut raw_candidates = Vec::new();
        if let Ok(results) = opensearch_res {
            raw_candidates.extend(results);
        }
        if let Ok(results) = fulltext_res {
            raw_candidates.extend(results);
        }
        if let Ok(results) = ddg_res {
            raw_candidates.extend(results);
        }

        let mut unique = deduplicate_candidates(raw_candidates);
        rank_candidates(title, author, &mut unique);
        unique
    }

    /// Queries Wikipedia OpenSearch API (canonical exact/prefix title index)
    async fn search_wikipedia_opensearch(&self, title: &str) -> Result<Vec<SearchResult>> {
        let endpoint = format!(
            "https://en.wikipedia.org/w/api.php?action=opensearch&search={}&limit=5&namespace=0&format=json",
            urlencoding::encode(title)
        );

        let fetch_fut = self.client.get(&endpoint).send();
        let resp = match tokio::time::timeout(Duration::from_secs(4), fetch_fut).await {
            Ok(Ok(r)) if r.status().is_success() => r,
            _ => return Ok(Vec::new()),
        };

        let json: Value = resp.json().await?;
        let mut results = Vec::new();

        // OpenSearch returns: [ search_term, [titles], [descriptions], [urls] ]
        if let (Some(titles), Some(urls)) = (
            json.get(1).and_then(|v| v.as_array()),
            json.get(3).and_then(|v| v.as_array()),
        ) {
            for (t_val, u_val) in titles.iter().zip(urls.iter()) {
                if let (Some(page_title), Some(page_url)) = (t_val.as_str(), u_val.as_str()) {
                    results.push(SearchResult::new(
                        format!("{} (Wikipedia)", page_title),
                        page_url,
                        None,
                    ));
                }
            }
        }

        Ok(results)
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

        let fetch_fut = self.client.get(&endpoint).send();
        let resp = match tokio::time::timeout(Duration::from_secs(4), fetch_fut).await {
            Ok(Ok(r)) if r.status().is_success() => r,
            _ => return Ok(Vec::new()),
        };

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

                    results.push(SearchResult::new(
                        format!("{} (Wikipedia)", page_title),
                        url,
                        snippet,
                    ));
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

        let fetch_fut = self.client.get(&endpoint).send();
        let resp = match tokio::time::timeout(Duration::from_secs(4), fetch_fut).await {
            Ok(Ok(r)) if r.status().is_success() => r,
            _ => return Ok(Vec::new()),
        };

        let html = resp.text().await?;
        let document = Html::parse_document(&html);

        let mut results = Vec::new();

        for element in document.select(&DDG_RESULT_SEL).take(3) {
            if let Some(link_elem) = element.select(&DDG_TITLE_SEL).next()
                && let Some(raw_href) = link_elem.value().attr("href")
            {
                let actual_url = extract_ddg_url(raw_href);
                if actual_url.starts_with("http") {
                        let title_text: String = link_elem.text().collect();
                        let snippet: Option<String> = element
                            .select(&DDG_SNIPPET_SEL)
                            .next()
                            .map(|s| s.text().collect::<String>().trim().to_string());

                        results.push(SearchResult::new(
                            title_text.trim(),
                            actual_url.into_owned(),
                            snippet,
                        ));
                    }
                }
            }

        Ok(results)
    }
}

/// Extracts target destination URL from DuckDuckGo redirect link, returning a zero-copy
/// `Cow::Borrowed` slice whenever no percent-decoding is needed.
pub fn extract_ddg_url(raw: &str) -> Cow<'_, str> {
    if let Some(pos) = raw.find("uddg=") {
        let remainder = &raw[pos + 5..];
        let encoded_url = remainder.split('&').next().unwrap_or(remainder);
        if let Ok(decoded) = urlencoding::decode(encoded_url) {
            return decoded;
        }
    }
    Cow::Borrowed(raw)
}

/// Deduplicates search results by normalized (percent-decoded, trailing-slash-trimmed) URL.
pub fn deduplicate_candidates(candidates: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::with_capacity(candidates.len());
    let mut unique = Vec::with_capacity(candidates.len());

    for item in candidates {
        let decoded = urlencoding::decode(&item.url).unwrap_or(Cow::Borrowed(&item.url));
        let normalized = decoded.trim_end_matches('/').to_ascii_lowercase();
        if seen.insert(normalized) {
            unique.push(item);
        }
    }

    unique
}

const DERIVATIVE_MARKERS: &[&str] = &[
    "history of",
    "the history of",
    "companion to",
    "study of",
    "annotated",
    "the art of",
    "the philosophy of",
    "film series",
    "(film series)",
    "(film)",
    "(franchise)",
    "soundtrack",
    "adaptations of",
    "characters of",
    "list of",
    "critical reception of",
    "themes of",
    "sparknotes",
    "cliffsnotes",
];

/// Scores each candidate against the requested book title and optional author,
/// sorting candidates descending by relevance score.
pub fn rank_candidates(title_query: &str, author_query: Option<&str>, candidates: &mut [SearchResult]) {
    for c in candidates.iter_mut() {
        c.score = score_candidate(title_query, author_query, c);
    }
    candidates.sort_by_key(|a| std::cmp::Reverse(a.score));
}

/// Evaluates relevance of a candidate search result to the user's book query.
pub fn score_candidate(title_query: &str, author_query: Option<&str>, candidate: &SearchResult) -> i32 {
    let mut score = 0;

    let norm_req = normalize_title_for_comparison(title_query);
    let norm_cand = normalize_title_for_comparison(&candidate.title);

    // 1. Exact match bonus
    if norm_cand == norm_req {
        score += 100;
    } else if norm_cand == format!("{} (novel)", norm_req)
        || norm_cand == format!("{} (book)", norm_req)
    {
        score += 95;
    } else if let Some(stripped) = norm_cand.strip_prefix(&norm_req) {
        let remainder = stripped.trim();
        if remainder.starts_with('(') && remainder.ends_with(')') {
            // Parenthetical qualifier like "(novel)" or "(1937)"
            score += 85;
        } else {
            // Subtitle like ": an unexpected journey"
            score += 40;
        }
    } else if norm_cand.ends_with(&norm_req) {
        score += 30;
    } else if norm_cand.contains(&norm_req) {
        score += 20;
    }

    // 2. Heavy penalty for derivative / commentary / adaptation keywords
    for marker in DERIVATIVE_MARKERS {
        if norm_cand.contains(marker) && !norm_req.contains(marker) {
            score -= 60;
            break;
        }
    }

    // 3. Author matching bonus
    if let Some(author) = author_query {
        let norm_author = author.trim().to_ascii_lowercase();
        let last_name = norm_author.split_whitespace().last().unwrap_or(&norm_author);

        let candidate_text = format!(
            "{} {}",
            norm_cand,
            candidate.snippet.as_deref().unwrap_or("").to_ascii_lowercase()
        );

        if candidate_text.contains(&norm_author) {
            score += 50;
        } else if candidate_text.contains(last_name) && last_name.len() > 2 {
            score += 30;
        }
    }

    // 4. Source authority bonus for Wikipedia novel/canonical pages
    if candidate.url.contains("wikipedia.org/wiki/") {
        score += 15;
    }

    score
}

fn normalize_title_for_comparison(title: &str) -> String {
    let stripped = title
        .strip_suffix(" (Wikipedia)")
        .or_else(|| title.strip_suffix(" - Wikipedia"))
        .unwrap_or(title);

    let mut out = String::with_capacity(stripped.len());
    let mut prev_space = false;
    for ch in stripped.chars() {
        if ch.is_alphanumeric() || ch == '(' || ch == ')' || ch == ':' {
            out.push(ch.to_ascii_lowercase());
            prev_space = false;
        } else if !prev_space && !out.is_empty() {
            out.push(' ');
            prev_space = true;
        }
    }
    out.trim().to_string()
}
