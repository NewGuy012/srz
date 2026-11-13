use crate::system::text_util::{contains_ascii_case_insensitive, url_decode, url_encode};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashSet;
use std::time::Duration;

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
    #[serde(default)]
    pub work_key: Option<String>,
}

#[derive(Clone)]
pub struct BookSearcher {
    client: reqwest::Client,
}

impl BookSearcher {
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    async fn get_with_timeout(&self, url: &str, timeout_secs: u64) -> Option<reqwest::Response> {
        let fetch_fut = self.client.get(url).send();
        match tokio::time::timeout(Duration::from_secs(timeout_secs), fetch_fut).await {
            Ok(Ok(r)) if r.status().is_success() => Some(r),
            _ => None,
        }
    }

    /// Queries Open Library API to resolve canonical book title, author, and first publication year.
    /// Returns None if the query times out (3s), fails, or yields no close match.
    pub async fn resolve_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<BookMetadata> {
        let mut endpoint = format!(
            "https://openlibrary.org/search.json?title={}&limit=3&fields=title,author_name,first_publish_year,key",
            url_encode(title)
        );
        if let Some(auth) = author {
            endpoint.push_str("&author=");
            endpoint.push_str(&url_encode(auth));
        }

        let resp = self.get_with_timeout(&endpoint, 3).await?;
        let bytes = crate::scraper::read_response_bytes_limited(resp, 5 * 1024 * 1024)
            .await
            .ok()?;
        let json: Value = serde_json::from_slice(&bytes).ok()?;
        let docs = json.get("docs")?.as_array()?;

        let norm_req = normalize_title_for_comparison(title);

        for doc in docs {
            let doc_title = doc.get("title")?.as_str()?;
            let norm_doc = normalize_title_for_comparison(doc_title);

            // Require close title match
            if norm_doc == norm_req || norm_doc.contains(&norm_req) || norm_req.contains(&norm_doc)
            {
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

                let work_key = doc
                    .get("key")
                    .and_then(|k| k.as_str())
                    .map(|s| s.to_string());

                return Some(BookMetadata {
                    canonical_title: doc_title.to_string(),
                    author: author_name,
                    first_publish_year,
                    work_key,
                });
            }
        }

        None
    }

    /// Fetches the editorial description of an Open Library work (e.g. `/works/OL12345W`),
    /// providing a grounded synopsis fallback when web scraping fails.
    pub async fn fetch_work_description(&self, work_key: &str) -> Option<String> {
        let clean_key = work_key.trim().trim_start_matches('/');
        if !is_valid_work_key(clean_key) {
            return None;
        }
        let endpoint = format!("https://openlibrary.org/{}.json", clean_key);

        let resp = self.get_with_timeout(&endpoint, 3).await?;
        let bytes = crate::scraper::read_response_bytes_limited(resp, 2 * 1024 * 1024)
            .await
            .ok()?;
        parse_work_description(&bytes)
    }

    /// Searches for candidate book synopses concurrently across Wikipedia (OpenSearch + fulltext) and DuckDuckGo,
    /// returning a deduplicated and scored/ranked list of candidates.
    pub async fn search_candidates(&self, title: &str, author: Option<&str>) -> Vec<SearchResult> {
        let mut unique = self.search_raw_candidates(title, author).await;
        rank_candidates(title, author, &mut unique);
        unique
    }

    /// Fetches candidates across search backends, prioritizing fast Wikipedia JSON APIs
    /// and short-circuiting to bypass slow DuckDuckGo HTML scraping when a confident match is found.
    pub async fn search_raw_candidates(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Vec<SearchResult> {
        let (opensearch_res, fulltext_res) = tokio::join!(
            self.search_wikipedia_opensearch(title),
            self.search_wikipedia_fulltext(title, author)
        );

        let mut raw_candidates = Vec::new();
        if let Ok(results) = opensearch_res {
            raw_candidates.extend(results);
        }
        if let Ok(results) = fulltext_res {
            raw_candidates.extend(results);
        }

        let mut unique = deduplicate_candidates(raw_candidates);

        // If Wikipedia yielded a high-confidence match (score >= 85), return immediately
        // to bypass the 1.5-3.5s DuckDuckGo HTML scraping latency.
        let prepared = PreparedQuery::new(title, author);
        let has_high_confidence_wiki = unique.iter().any(|c| prepared.score(c) >= 85);

        if !has_high_confidence_wiki
            && let Ok(ddg_results) = self.search_duckduckgo(title, author).await
        {
            unique.extend(ddg_results);
            unique = deduplicate_candidates(unique);
        }

        unique
    }

    /// Queries Wikipedia OpenSearch API (canonical exact/prefix title index)
    async fn search_wikipedia_opensearch(&self, title: &str) -> Result<Vec<SearchResult>> {
        let endpoint = format!(
            "https://en.wikipedia.org/w/api.php?action=opensearch&search={}&limit=5&namespace=0&format=json",
            url_encode(title)
        );

        let Some(resp) = self.get_with_timeout(&endpoint, 4).await else {
            return Ok(Vec::new());
        };

        let bytes = crate::scraper::read_response_bytes_limited(resp, 5 * 1024 * 1024).await?;
        let json: Value = serde_json::from_slice(&bytes)?;
        let mut results = Vec::new();

        // OpenSearch returns: [ search_term, [titles], [descriptions], [urls] ]
        let descriptions = json.get(2).and_then(|v| v.as_array());
        if let (Some(titles), Some(urls)) = (
            json.get(1).and_then(|v| v.as_array()),
            json.get(3).and_then(|v| v.as_array()),
        ) {
            for (idx, (t_val, u_val)) in titles.iter().zip(urls.iter()).enumerate() {
                if let (Some(page_title), Some(page_url)) = (t_val.as_str(), u_val.as_str()) {
                    let snippet = descriptions
                        .and_then(|d| d.get(idx))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    results.push(SearchResult::new(
                        format!("{} (Wikipedia)", page_title),
                        page_url,
                        snippet,
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
            url_encode(&query)
        );

        let Some(resp) = self.get_with_timeout(&endpoint, 4).await else {
            return Ok(Vec::new());
        };

        let bytes = crate::scraper::read_response_bytes_limited(resp, 5 * 1024 * 1024).await?;
        let json: Value = serde_json::from_slice(&bytes)?;
        let mut results = Vec::new();

        if let Some(items) = json.pointer("/query/search").and_then(|v| v.as_array()) {
            for item in items.iter().take(3) {
                if let Some(page_title) = item.get("title").and_then(|t| t.as_str()) {
                    let snippet = item
                        .get("snippet")
                        .and_then(|s| s.as_str())
                        .map(|raw_snippet| {
                            let stripped = crate::system::text_util::strip_html_tags(raw_snippet);
                            crate::system::text_util::decode_html_entities(stripped.trim())
                                .into_owned()
                        });

                    let url_title = page_title.replace(' ', "_");
                    let url = format!("https://en.wikipedia.org/wiki/{}", url_encode(&url_title));

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

        let endpoint = format!("https://html.duckduckgo.com/html/?q={}", url_encode(&query));

        let Some(resp) = self.get_with_timeout(&endpoint, 3).await else {
            return Ok(Vec::new());
        };

        let html = match crate::scraper::read_response_text_limited(
            resp,
            crate::scraper::MAX_SCRAPE_BYTES,
        )
        .await
        {
            Ok(h) => h,
            Err(_) => return Ok(Vec::new()),
        };
        let mut results = Vec::new();
        let mut cursor = 0;

        while let Some(pos) = html[cursor..].find("result__body") {
            let start = cursor + pos;
            let next = html[start + 12..]
                .find("result__body")
                .map(|p| start + 12 + p)
                .unwrap_or(html.len());
            let block = &html[start..next];
            cursor = next;

            let Some(title_pos) = block.find("result__title") else {
                continue;
            };
            let title_rest = &block[title_pos..];
            let href = crate::system::text_util::extract_attr_value(title_rest, "href=");

            let Some(raw_href) = href else {
                continue;
            };
            let actual_url = extract_ddg_url(raw_href);
            if !actual_url.starts_with("http") {
                continue;
            }

            let title = if let Some(a_start) = title_rest.find('>') {
                let after_a = &title_rest[a_start + 1..];
                if let Some(a_end) = after_a.find("</a>") {
                    let stripped = crate::system::text_util::strip_html_tags(&after_a[..a_end]);
                    crate::system::text_util::decode_html_entities(stripped.trim()).into_owned()
                } else {
                    String::new()
                }
            } else {
                String::new()
            };

            let snippet = if let Some(snip_pos) = block.find("result__snippet") {
                let snip_rest = &block[snip_pos..];
                if let Some(tag_end) = snip_rest.find('>') {
                    let content = &snip_rest[tag_end + 1..];
                    if let Some(close_tag) = content.find('<') {
                        let inner = content[..close_tag].trim();
                        if !inner.is_empty() {
                            Some(crate::system::text_util::decode_html_entities(inner).into_owned())
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            results.push(SearchResult::new(
                title.trim(),
                actual_url.into_owned(),
                snippet,
            ));
            if results.len() >= 3 {
                break;
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
        if let Ok(decoded) = url_decode(encoded_url) {
            return decoded;
        }
    }
    Cow::Borrowed(raw)
}

/// Validates an Open Library work key (e.g. `/works/OL12345W` or `works/OL12345W`),
/// ensuring it strictly starts with `works/OL` and contains only safe identifier characters (CWE-22 / CWE-918).
pub fn is_valid_work_key(work_key: &str) -> bool {
    let clean = work_key.trim().trim_start_matches('/');
    if !clean.starts_with("works/OL") {
        return false;
    }
    let id_part = &clean["works/".len()..];
    !id_part.is_empty()
        && id_part
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Parses an Open Library work description JSON response, handling both plain string descriptions
/// and typed objects `{"type": "/type/text", "value": "..."}`.
pub fn parse_work_description(json_bytes: &[u8]) -> Option<String> {
    let json: Value = serde_json::from_slice(json_bytes).ok()?;
    if let Some(desc_val) = json.get("description") {
        if let Some(desc_str) = desc_val.as_str() {
            let trimmed = desc_str.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        } else if let Some(val_str) = desc_val.get("value").and_then(|v| v.as_str()) {
            let trimmed = val_str.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Deduplicates search results by normalized (percent-decoded, trailing-slash-trimmed) URL.
pub fn deduplicate_candidates(candidates: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::with_capacity(candidates.len());
    let mut unique = Vec::with_capacity(candidates.len());

    for item in candidates {
        let decoded = url_decode(&item.url).unwrap_or(Cow::Borrowed(&item.url));
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

struct PreparedQuery {
    norm_req: String,
    novel_variant: String,
    book_variant: String,
    norm_author: Option<String>,
}

impl PreparedQuery {
    fn new(title_query: &str, author_query: Option<&str>) -> Self {
        let norm_req = normalize_title_for_comparison(title_query);
        let novel_variant = format!("{} (novel)", norm_req);
        let book_variant = format!("{} (book)", norm_req);
        let norm_author = author_query
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty());

        Self {
            norm_req,
            novel_variant,
            book_variant,
            norm_author,
        }
    }

    fn score(&self, candidate: &SearchResult) -> i32 {
        let mut score = 0;
        let norm_cand = normalize_title_for_comparison(&candidate.title);

        // 1. Exact match bonus
        if norm_cand == self.norm_req {
            score += 100;
        } else if norm_cand == self.novel_variant || norm_cand == self.book_variant {
            score += 95;
        } else if let Some(stripped) = norm_cand.strip_prefix(&self.norm_req) {
            let remainder = stripped.trim();
            if remainder.starts_with('(') && remainder.ends_with(')') {
                // Parenthetical qualifier like "(novel)" or "(1937)"
                score += 85;
            } else {
                // Subtitle like ": an unexpected journey"
                score += 40;
            }
        } else if norm_cand.ends_with(&self.norm_req) {
            score += 30;
        } else if norm_cand.contains(&self.norm_req) {
            score += 20;
        }

        // 2. Heavy penalty for derivative / commentary / adaptation keywords
        for marker in DERIVATIVE_MARKERS {
            if norm_cand.contains(marker) && !self.norm_req.contains(marker) {
                score -= 60;
                break;
            }
        }

        // 3. Author matching bonus (without concatenating title + snippet into a new String)
        if let Some(norm_author) = &self.norm_author {
            let last_name = norm_author.split_whitespace().last().unwrap_or(norm_author);
            let snippet_str = candidate.snippet.as_deref().unwrap_or("");

            if norm_cand.contains(norm_author.as_str())
                || contains_ascii_case_insensitive(snippet_str, norm_author)
            {
                score += 50;
            } else if last_name.len() > 2
                && (norm_cand.contains(last_name)
                    || contains_ascii_case_insensitive(snippet_str, last_name))
            {
                score += 30;
            }
        }

        // 4. Source authority bonus for Wikipedia novel/canonical pages
        if candidate.url.contains("wikipedia.org/wiki/") {
            score += 15;
        }

        score
    }
}

/// Scores each candidate against the requested book title and optional author,
/// sorting candidates descending by relevance score.
pub fn rank_candidates(
    title_query: &str,
    author_query: Option<&str>,
    candidates: &mut [SearchResult],
) {
    let prepared = PreparedQuery::new(title_query, author_query);
    for c in candidates.iter_mut() {
        c.score = prepared.score(c);
    }
    candidates.sort_by_key(|a| std::cmp::Reverse(a.score));
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
    if out.ends_with(' ') {
        out.pop();
    }
    out
}

/// Extracts the lead section and main narrative/summary section from a Wikipedia page.
pub fn extract_wikipedia_synopsis(markdown: &str) -> Option<String> {
    // 1. Identify lead section (text before the first `## ` heading)
    let lead_end = markdown.find("\n## ").unwrap_or(markdown.len());
    let lead = markdown[..lead_end].trim();

    // 2. Search for plot/synopsis/overview section
    const PRIMARY_HEADINGS: &[&str] = &[
        "## plot",
        "## synopsis",
        "## plot summary",
        "## summary",
        "## premise",
        "## narrative",
    ];

    const NON_FICTION_HEADINGS: &[&str] = &[
        "## overview",
        "## content",
        "## contents",
        "## core concepts",
        "## key themes",
        "## themes",
        "## thesis",
        "## background and overview",
        "## structure",
    ];

    let lower = markdown.to_ascii_lowercase();
    let mut plot_start = None;

    // Check primary narrative/fiction headings first
    for heading in PRIMARY_HEADINGS {
        if let Some(pos) = lower.find(heading) {
            plot_start = Some(pos);
            break;
        }
    }

    // If no primary fiction heading found, check non-fiction content/overview headings
    if plot_start.is_none() {
        for heading in NON_FICTION_HEADINGS {
            if let Some(pos) = lower.find(heading) {
                plot_start = Some(pos);
                break;
            }
        }
    }

    let start_idx = plot_start?;

    // Find where the plot/content section ends: the next `## ` heading after `start_idx`
    let section_body = &markdown[start_idx..];
    let body_start = section_body.find('\n').unwrap_or(0);
    let after_heading = &section_body[body_start..];
    let end_idx = after_heading
        .find("\n## ")
        .map(|p| start_idx + body_start + p)
        .unwrap_or(markdown.len());
    let plot_content = markdown[start_idx..end_idx].trim();

    if plot_content.split_whitespace().count() < 40 {
        return None;
    }

    let mut result = String::with_capacity(lead.len() + plot_content.len() + 16);
    if !lead.is_empty() {
        result.push_str(lead);
        result.push_str("\n\n---\n\n");
    }
    result.push_str(plot_content);

    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_work_key() {
        assert!(is_valid_work_key("/works/OL12345W"));
        assert!(is_valid_work_key("works/OL12345W"));
        assert!(is_valid_work_key("works/OL12345-M"));
        assert!(is_valid_work_key("works/OL123_45"));

        // Path traversal / SSRF rejections
        assert!(!is_valid_work_key("/works/../admin"));
        assert!(!is_valid_work_key("works/OL123?param=val"));
        assert!(!is_valid_work_key("works/OL123#frag"));
        assert!(!is_valid_work_key("works/OL123/sub"));
        assert!(!is_valid_work_key("@attacker.com/works/OL123"));
        assert!(!is_valid_work_key("authors/OL123A"));
        assert!(!is_valid_work_key("works/"));
        assert!(!is_valid_work_key(""));
    }
}
