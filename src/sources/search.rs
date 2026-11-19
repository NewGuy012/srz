use crate::scraper::{
    collapse_whitespace, fetch_bytes_limited, fetch_text_limited, validate_public_http_url,
};
use crate::system::text_util::{
    XmlEvent, XmlScanner, decode_html_entities, find_ascii_case_insensitive, strip_html_tags,
    url_decode, url_encode, xml_attr,
};
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

    async fn fetch_bytes(&self, url: &str, timeout_secs: u64, max_bytes: usize) -> Result<Vec<u8>> {
        fetch_bytes_limited(
            self.client
                .get(url)
                .timeout(Duration::from_secs(timeout_secs)),
            max_bytes,
            url,
        )
        .await
    }

    /// Queries Open Library to resolve canonical book title, author, and first publication year.
    pub async fn resolve_book_metadata(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Option<BookMetadata> {
        let norm_req = normalize_title_for_comparison(title);
        if norm_req.is_empty() {
            return None;
        }
        let mut endpoint = format!(
            "https://openlibrary.org/search.json?title={}&limit=3&fields=title,author_name,first_publish_year,key",
            url_encode(title)
        );
        if let Some(auth) = author {
            endpoint.push_str("&author=");
            endpoint.push_str(&url_encode(auth));
        }

        let bytes = self.fetch_bytes(&endpoint, 3, 5 * 1024 * 1024).await.ok()?;
        let json: Value = serde_json::from_slice(&bytes).ok()?;
        let docs = json.get("docs")?.as_array()?;

        for doc in docs {
            let doc_title = doc.get("title")?.as_str()?;
            let norm_doc = normalize_title_for_comparison(doc_title);

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

    /// Fetches the editorial description of an Open Library work (e.g. `/works/OL12345W`).
    pub async fn fetch_work_description(&self, work_key: &str) -> Option<String> {
        let clean_key = work_key.trim().trim_start_matches('/');
        if !is_valid_work_key(clean_key) {
            return None;
        }
        let endpoint = format!("https://openlibrary.org/{clean_key}.json");
        let bytes = self.fetch_bytes(&endpoint, 3, 2 * 1024 * 1024).await.ok()?;
        parse_work_description(&bytes)
    }

    async fn fetch_candidates(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> (Vec<SearchResult>, PreparedQuery) {
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
        let prepared = PreparedQuery::new(title, author);
        let has_high_confidence_wiki = unique.iter().any(|c| prepared.score(c) >= 85);

        if !has_high_confidence_wiki {
            let ddg_results = self.search_duckduckgo(title, author).await;
            if !ddg_results.is_empty() {
                unique.extend(ddg_results);
                unique = deduplicate_candidates(unique);
            }
        }

        (unique, prepared)
    }

    /// Searches Wikipedia and DuckDuckGo concurrently without final sorting.
    pub async fn search_raw_candidates(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Vec<SearchResult> {
        self.fetch_candidates(title, author).await.0
    }

    /// Searches Wikipedia and DuckDuckGo concurrently and returns ranked, deduplicated candidates.
    pub async fn search_candidates(&self, title: &str, author: Option<&str>) -> Vec<SearchResult> {
        let (mut unique, prepared) = self.fetch_candidates(title, author).await;
        score_with_prepared(&prepared, &mut unique);
        unique
    }

    async fn search_wikipedia_opensearch(&self, title: &str) -> Result<Vec<SearchResult>> {
        let endpoint = format!(
            "https://en.wikipedia.org/w/api.php?action=opensearch&search={}&limit=5&namespace=0&format=json",
            url_encode(title)
        );

        let bytes = self.fetch_bytes(&endpoint, 4, 5 * 1024 * 1024).await?;
        let json: Value = serde_json::from_slice(&bytes)?;
        let mut results = Vec::new();

        let descriptions = json.get(2).and_then(|v| v.as_array());
        if let (Some(titles), Some(urls)) = (
            json.get(1).and_then(|v| v.as_array()),
            json.get(3).and_then(|v| v.as_array()),
        ) {
            for (idx, (t_val, u_val)) in titles.iter().zip(urls.iter()).enumerate() {
                if let (Some(page_title), Some(page_url)) = (t_val.as_str(), u_val.as_str())
                    && validate_public_http_url(page_url).is_ok()
                {
                    let snippet = descriptions
                        .and_then(|d| d.get(idx))
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string());
                    results.push(SearchResult::new(
                        format!("{page_title} (Wikipedia)"),
                        page_url,
                        snippet,
                    ));
                }
            }
        }

        Ok(results)
    }

    async fn search_wikipedia_fulltext(
        &self,
        title: &str,
        author: Option<&str>,
    ) -> Result<Vec<SearchResult>> {
        let query = match author {
            Some(auth) => format!("{title} {auth} novel OR book"),
            None => format!("{title} novel OR book"),
        };

        let endpoint = format!(
            "https://en.wikipedia.org/w/api.php?action=query&list=search&srsearch={}&format=json&utf8=",
            url_encode(&query)
        );

        let bytes = self.fetch_bytes(&endpoint, 4, 5 * 1024 * 1024).await?;
        let json: Value = serde_json::from_slice(&bytes)?;
        let mut results = Vec::new();

        if let Some(items) = json.pointer("/query/search").and_then(|v| v.as_array()) {
            for item in items.iter().take(3) {
                if let Some(page_title) = item.get("title").and_then(|t| t.as_str()) {
                    let snippet = item.get("snippet").and_then(|s| s.as_str()).map(|raw| {
                        let stripped = strip_html_tags(raw);
                        decode_html_entities(stripped.trim()).into_owned()
                    });

                    let url_title = page_title.replace(' ', "_");
                    let url = format!("https://en.wikipedia.org/wiki/{}", url_encode(&url_title));

                    results.push(SearchResult::new(
                        format!("{page_title} (Wikipedia)"),
                        url,
                        snippet,
                    ));
                }
            }
        }

        Ok(results)
    }

    async fn search_duckduckgo(&self, title: &str, author: Option<&str>) -> Vec<SearchResult> {
        let safe_title = title.replace('"', " ");
        let query = match author {
            Some(auth) => format!(
                "\"{}\" \"{}\" book synopsis summary",
                safe_title,
                auth.replace('"', " ")
            ),
            None => format!("\"{}\" book synopsis summary", safe_title),
        };

        let endpoint = format!("https://html.duckduckgo.com/html/?q={}", url_encode(&query));
        let Ok(html) = fetch_text_limited(
            self.client.get(&endpoint).timeout(Duration::from_secs(3)),
            crate::scraper::MAX_SCRAPE_BYTES,
            &endpoint,
        )
        .await
        else {
            return Vec::new();
        };
        parse_ddg_html_results(&html)
    }
}

fn parse_ddg_html_results(html: &str) -> Vec<SearchResult> {
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
        let mut href: Option<String> = None;
        let mut title = String::new();
        let mut snippet: Option<String> = None;

        let mut scanner = XmlScanner::new(&block[title_pos..]);
        while let Some(ev) = scanner.next_event() {
            let XmlEvent::Start {
                name,
                attrs,
                is_empty,
            } = ev
            else {
                continue;
            };
            if href.is_none()
                && name.eq_ignore_ascii_case("a")
                && let Some(raw_href) = xml_attr(attrs, "href")
            {
                href = Some(raw_href.into_owned());
                if !is_empty {
                    title = collapse_whitespace(&scanner.read_text(name));
                }
            } else if snippet.is_none()
                && xml_attr(attrs, "class")
                    .is_some_and(|c| c.split_ascii_whitespace().any(|t| t == "result__snippet"))
                && !is_empty
            {
                let cleaned = collapse_whitespace(&scanner.read_text(name));
                if !cleaned.is_empty() {
                    snippet = Some(cleaned);
                }
            }
            if href.is_some() && snippet.is_some() {
                break;
            }
        }

        let Some(raw_href) = href else {
            continue;
        };
        let actual_url = extract_ddg_url(&raw_href);
        if validate_public_http_url(&actual_url).is_err() {
            continue;
        }

        results.push(SearchResult::new(title, actual_url.into_owned(), snippet));
        if results.len() >= 3 {
            break;
        }
    }

    results
}

fn extract_ddg_url(raw: &str) -> Cow<'_, str> {
    let query_pos = raw.find("?uddg=").or_else(|| raw.find("&uddg="));
    if let Some(pos) = query_pos {
        let remainder = &raw[pos + 6..];
        let encoded_url = remainder.split('&').next().unwrap_or(remainder);
        if let Ok(decoded) = url_decode(encoded_url) {
            return decoded;
        }
    }
    Cow::Borrowed(raw)
}

fn is_valid_work_key(work_key: &str) -> bool {
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

fn parse_work_description(json_bytes: &[u8]) -> Option<String> {
    let json: Value = serde_json::from_slice(json_bytes).ok()?;
    let desc_val = json.get("description")?;
    let s = desc_val
        .as_str()
        .or_else(|| desc_val.get("value").and_then(|v| v.as_str()))?;
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn deduplicate_candidates(candidates: Vec<SearchResult>) -> Vec<SearchResult> {
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
    "companion to",
    "study of",
    "annotated",
    "the art of",
    "the philosophy of",
    "film series",
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
        let novel_variant = format!("{norm_req} (novel)");
        let book_variant = format!("{norm_req} (book)");
        let norm_author = author_query
            .map(|a| a.trim().to_lowercase())
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

        if norm_cand == self.norm_req {
            score += 100;
        } else if norm_cand == self.novel_variant || norm_cand == self.book_variant {
            score += 95;
        } else if let Some(stripped) = norm_cand.strip_prefix(&self.norm_req) {
            let remainder = stripped.trim();
            if remainder.starts_with('(') && remainder.ends_with(')') {
                score += 85;
            } else {
                score += 40;
            }
        } else if norm_cand.ends_with(&self.norm_req) {
            score += 30;
        } else if norm_cand.contains(&self.norm_req) {
            score += 20;
        }

        for marker in DERIVATIVE_MARKERS {
            if norm_cand.contains(marker) && !self.norm_req.contains(marker) {
                score -= 60;
                break;
            }
        }

        if let Some(norm_author) = &self.norm_author {
            let last_name = norm_author.split_whitespace().last().unwrap_or(norm_author);
            let snippet_str = candidate.snippet.as_deref().unwrap_or("");

            if norm_cand.contains(norm_author.as_str())
                || find_ascii_case_insensitive(snippet_str, norm_author).is_some()
            {
                score += 50;
            } else if last_name.len() > 2
                && (norm_cand.contains(last_name)
                    || find_ascii_case_insensitive(snippet_str, last_name).is_some())
            {
                score += 30;
            }
        }

        if candidate.url.contains("wikipedia.org/wiki/") {
            score += 15;
        }

        score
    }
}

fn score_with_prepared(prepared: &PreparedQuery, candidates: &mut [SearchResult]) {
    for c in candidates.iter_mut() {
        c.score = prepared.score(c);
    }
    candidates.sort_by_key(|a| std::cmp::Reverse(a.score));
}

/// Scores and sorts candidates descending against the requested book title and optional author.
pub fn rank_candidates(
    title_query: &str,
    author_query: Option<&str>,
    candidates: &mut [SearchResult],
) {
    let prepared = PreparedQuery::new(title_query, author_query);
    score_with_prepared(&prepared, candidates);
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
            for lower in ch.to_lowercase() {
                out.push(lower);
            }
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

    let lead_end = markdown.find("\n## ").unwrap_or(markdown.len());
    let lead = markdown[..lead_end].trim();

    let mut primary_start = None;
    let mut secondary_start = None;
    let mut offset = 0;

    for line in markdown.split_inclusive('\n') {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            if PRIMARY_HEADINGS
                .iter()
                .any(|h| trimmed.eq_ignore_ascii_case(h))
            {
                primary_start = Some(offset);
                break;
            }
            if secondary_start.is_none()
                && NON_FICTION_HEADINGS
                    .iter()
                    .any(|h| trimmed.eq_ignore_ascii_case(h))
            {
                secondary_start = Some(offset);
            }
        }
        offset += line.len();
    }

    let start_idx = primary_start.or(secondary_start)?;
    let section_body = &markdown[start_idx..];
    let body_start = section_body.find('\n').unwrap_or(0);
    let end_idx = section_body[body_start..]
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
    fn test_work_key_validation_and_description() {
        for valid in ["/works/OL12345W", "works/OL12345-M", "works/OL123_45"] {
            assert!(is_valid_work_key(valid), "expected valid: {valid}");
        }
        for invalid in [
            "/works/../admin",
            "works/OL123?param=val",
            "works/OL123#frag",
            "authors/OL123A",
            "",
        ] {
            assert!(!is_valid_work_key(invalid), "expected invalid: {invalid}");
        }
        assert_eq!(
            parse_work_description(br#"{"description": "Dune is a sci-fi novel."}"#).as_deref(),
            Some("Dune is a sci-fi novel.")
        );
        assert_eq!(
            parse_work_description(br#"{"description": {"value": "Orwell classic."}}"#).as_deref(),
            Some("Orwell classic.")
        );
        assert_eq!(parse_work_description(br#"{"description": ""}"#), None);
    }

    #[test]
    fn test_deduplicate_and_rank_candidates() {
        let mut list = deduplicate_candidates(vec![
            SearchResult::new(
                "Dune (novel) (Wikipedia)",
                "https://en.wikipedia.org/wiki/Dune_%28novel%29",
                None,
            ),
            SearchResult::new(
                "Dune - Wikipedia",
                "https://en.wikipedia.org/wiki/Dune_(novel)/",
                Some("dup".to_string()),
            ),
            SearchResult::new(
                "The History of Dune (Wikipedia)",
                "https://en.wikipedia.org/wiki/History_of_Dune",
                Some("Study".to_string()),
            ),
            SearchResult::new(
                "Dune (film series) (Wikipedia)",
                "https://en.wikipedia.org/wiki/Dune_(film_series)",
                Some("Film".to_string()),
            ),
        ]);
        assert_eq!(list.len(), 3);

        rank_candidates("Dune", Some("Frank Herbert"), &mut list);
        assert_eq!(list[0].title, "Dune (novel) (Wikipedia)");
        assert!(list[0].score >= 85);
        assert_eq!(
            normalize_title_for_comparison("Étude — A Study"),
            "étude a study"
        );
    }

    #[test]
    fn test_extract_wikipedia_synopsis() {
        let forty_words =
            "Arrakis supplies spice extending human life across interstellar empires. ".repeat(5);
        let wiki = format!(
            "# Dune\n\nLead paragraph.\n\n## Plot holes\nUnrelated tiny section.\n\n## Plot\n{forty_words}\n\n## References\n[1] Herbert."
        );
        let text = extract_wikipedia_synopsis(&wiki).expect("expected synopsis");
        assert!(text.contains("Lead paragraph."));
        assert!(text.contains("## Plot\nArrakis"));
        assert!(!text.contains("Plot holes"));
        assert!(!text.contains("References"));

        let nonfiction = format!("# Thinking\n\nIntro.\n\n## Overview\n{forty_words}");
        assert!(
            extract_wikipedia_synopsis(&nonfiction)
                .unwrap()
                .contains("## Overview")
        );
    }

    #[test]
    fn test_duckduckgo_parsing_and_ssrf_filtering() {
        assert_eq!(
            extract_ddg_url(
                "/l/?notuddg=https%3A%2F%2Fevil.com&uddg=https%3A%2F%2Fexample.com%2Fa"
            ),
            "https://example.com/a"
        );
        let html = r##"
            <div class="result__body"><h2 class="result__title"><a href="/l/?uddg=http%3A%2F%2F127.0.0.1%2Fadmin">Evil</a></h2></div>
            <div class="result__body"><h2 class="result__title"><a href="javascript:alert(1)">XSS</a></h2></div>
            <div class="result__body"><h2 class="result__title"><a class="result__a" href="/l/?uddg=https%3A%2F%2Fexample.com%2Fdune"><b>Dune</b> (novel)</a></h2><a class="result__snippet" href="#"><b>Dune</b> is classic &amp; iconic.</a></div>
        "##;
        let parsed = parse_ddg_html_results(html);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Dune (novel)");
        assert_eq!(parsed[0].url, "https://example.com/dune");
        assert_eq!(
            parsed[0].snippet.as_deref(),
            Some("Dune is classic & iconic.")
        );
    }
}
