use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use std::time::Duration;

static OG_TITLE_SEL: LazyLock<Selector> =
    LazyLock::new(|| Selector::parse("meta[property='og:title']").unwrap());
static TITLE_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("title").unwrap());
static H1_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("h1").unwrap());
static MAIN_SELECTORS: LazyLock<Vec<Selector>> = LazyLock::new(|| {
    [
        "article",
        "main",
        "[role='main']",
        ".mw-parser-output",
        "#mw-content-text",
        "#content",
        ".post-content",
        ".article-content",
        ".entry-content",
        "body",
    ]
    .into_iter()
    .filter_map(|s| Selector::parse(s).ok())
    .collect()
});

const REFERENCE_HEADINGS: &[&str] = &[
    "references",
    "## references",
    "=== references ===",
    "== references ==",
    "external links",
    "## external links",
    "=== external links ===",
    "notes and references",
    "## notes",
    "## bibliography",
    "see also",
    "## see also",
];

pub const DEFAULT_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub const MAX_SCRAPE_BYTES: usize = 10 * 1024 * 1024; // 10 MB limit for web responses

/// Converts HTML bytes to plain text without table borders or footnote links.
pub fn html_to_plain_text(html_bytes: &[u8]) -> Result<String> {
    html2text::config::plain()
        .link_footnotes(false)
        .no_table_borders()
        .string_from_read(html_bytes, 10_000)
        .context("Failed to convert HTML to text")
}

/// Reads response body bytes up to `max_bytes`, erroring if the content length or streamed bytes exceed `max_bytes`.
pub async fn read_response_bytes_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    if let Some(content_len) = response.content_length()
        && content_len as usize > max_bytes
    {
        anyhow::bail!(
            "Response content length ({} bytes) exceeds limit of {} MB",
            content_len,
            max_bytes / (1024 * 1024)
        );
    }

    let mut bytes = if let Some(content_len) = response.content_length() {
        Vec::with_capacity((content_len as usize).min(max_bytes))
    } else {
        Vec::new()
    };

    while let Some(chunk) = response.chunk().await.context("Failed to read response body chunk")? {
        if bytes.len() + chunk.len() > max_bytes {
            anyhow::bail!(
                "Response body exceeded limit of {} MB",
                max_bytes / (1024 * 1024)
            );
        }
        bytes.extend_from_slice(&chunk);
    }

    Ok(bytes)
}

/// Reads response body text up to `max_bytes`, decoding with lossy UTF-8 fallback.
pub async fn read_response_text_limited(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<String> {
    let bytes = read_response_bytes_limited(response, max_bytes).await?;
    match String::from_utf8(bytes) {
        Ok(s) => Ok(s),
        Err(e) => Ok(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScrapedPage {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    pub word_count: usize,
}

#[derive(Clone)]
pub struct WebScraper {
    client: reqwest::Client,
}

impl WebScraper {
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(DEFAULT_USER_AGENT),
        );
        headers.insert(
            reqwest::header::ACCEPT,
            HeaderValue::from_static(
                "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
            ),
        );
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-ch-ua"),
            HeaderValue::from_static(
                "\"Chromium\";v=\"124\", \"Google Chrome\";v=\"124\", \"Not-A.Brand\";v=\"99\"",
            ),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-ch-ua-mobile"),
            HeaderValue::from_static("?0"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-ch-ua-platform"),
            HeaderValue::from_static("\"macOS\""),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-fetch-dest"),
            HeaderValue::from_static("document"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-fetch-mode"),
            HeaderValue::from_static("navigate"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-fetch-site"),
            HeaderValue::from_static("none"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("sec-fetch-user"),
            HeaderValue::from_static("?1"),
        );
        headers.insert(
            reqwest::header::HeaderName::from_static("upgrade-insecure-requests"),
            HeaderValue::from_static("1"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("Failed to initialize HTTP client")?;

        Ok(Self { client })
    }

    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetches and normalizes the text from the given URL with a strict maximum response limit (10 MB).
    pub async fn scrape(&self, url: &str) -> Result<ScrapedPage> {
        let response = self
            .client
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch URL: {}", url))?;

        if !response.status().is_success() {
            anyhow::bail!(
                "Failed to fetch webpage (HTTP status: {})",
                response.status()
            );
        }

        let html = read_response_text_limited(response, MAX_SCRAPE_BYTES).await?;
        self.normalize(url, &html)
    }

    /// Normalizes raw HTML into clean, readable text.
    pub fn normalize(&self, url: &str, raw_html: &str) -> Result<ScrapedPage> {
        let document = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = self.extract_title(&document);

        // 2. Select main content container or fallback to body
        let target_html = self.extract_main_html(&document);

        // 3. Convert HTML to clean plain text without link footnote bloat or 100-col artificial wraps
        let rendered_text = html_to_plain_text(target_html.as_bytes())?;

        // 4. Clean and normalize whitespace/formatting + count words in a single pass
        let (cleaned_content, word_count) = clean_text(&rendered_text, true, 2);

        Ok(ScrapedPage {
            url: url.to_string(),
            title,
            content: cleaned_content,
            word_count,
        })
    }

    fn extract_title(&self, document: &Html) -> Option<String> {
        // Try og:title first (zero-copy attribute slice until final .to_string())
        if let Some(elem) = document.select(&OG_TITLE_SEL).next()
            && let Some(content) = elem.value().attr("content")
        {
            let trimmed = content.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        // Try <title> tag, then <h1> tag
        for sel in [&*TITLE_SEL, &*H1_SEL] {
            if let Some(elem) = document.select(sel).next() {
                let text: String = elem.text().collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        None
    }

    fn extract_main_html(&self, document: &Html) -> String {
        for sel in MAIN_SELECTORS.iter() {
            if let Some(element) = document.select(sel).next() {
                let html = element.html();
                if html.len() > 200 {
                    return html;
                }
            }
        }

        document.html()
    }
}

/// Checks if a line is a reference/bibliography heading in-place without allocating a lowercase `String`.
fn is_reference_heading(line: &str) -> bool {
    REFERENCE_HEADINGS
        .iter()
        .any(|heading| line.eq_ignore_ascii_case(heading))
}

/// Cleans lines and counts words in a single pass.
pub fn clean_raw_text(text: &str) -> (String, usize) {
    clean_text(text, false, 1)
}

/// Cleans lines, optionally strips trailing reference sections using zero-allocation ASCII comparison,
/// and computes total word count in a single pass into one pre-sized `String`.
fn clean_text(text: &str, strip_references: bool, max_empty_lines: usize) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut word_count = 0;
    let mut empty_line_count = 0;

    for raw_line in text.lines() {
        let line = raw_line.trim();

        if strip_references && is_reference_heading(line) {
            break;
        }

        if line.is_empty() {
            empty_line_count += 1;
            if empty_line_count <= max_empty_lines && !out.is_empty() {
                out.push('\n');
            }
        } else {
            empty_line_count = 0;
            word_count += line.split_whitespace().count();
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(line);
        }
    }

    let trimmed_len = out.trim_end().len();
    out.truncate(trimmed_len);
    (out, word_count)
}
