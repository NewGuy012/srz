use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::path::Path;
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
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
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

    /// Fetches and normalizes the text from the given URL.
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

        let html = response
            .text()
            .await
            .context("Failed to read response body")?;

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
        let rendered_text = html2text::config::plain()
            .link_footnotes(false)
            .no_table_borders()
            .string_from_read(target_html.as_bytes(), 10_000)
            .context("Failed to convert HTML to text")?;

        // 4. Clean and normalize whitespace/formatting + count words in a single pass
        let (cleaned_content, word_count) = self.clean_text_and_count_words(&rendered_text);

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

        // Try <title> tag
        if let Some(elem) = document.select(&TITLE_SEL).next() {
            let text: String = elem.text().collect();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        // Try <h1> tag
        if let Some(elem) = document.select(&H1_SEL).next() {
            let text: String = elem.text().collect();
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
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

    /// Cleans lines, strips trailing reference sections using zero-allocation ASCII comparison,
    /// and computes total word count in a single pass into one pre-sized `String`.
    fn clean_text_and_count_words(&self, text: &str) -> (String, usize) {
        let mut out = String::with_capacity(text.len());
        let mut word_count = 0;
        let mut empty_line_count = 0;

        for raw_line in text.lines() {
            let line = raw_line.trim();

            if is_reference_heading(line) {
                break;
            }

            if line.is_empty() {
                empty_line_count += 1;
                if empty_line_count <= 2 && !out.is_empty() {
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
}

/// Checks if a line is a reference/bibliography heading in-place without allocating a lowercase `String`.
fn is_reference_heading(line: &str) -> bool {
    REFERENCE_HEADINGS
        .iter()
        .any(|heading| line.eq_ignore_ascii_case(heading))
}

/// Returns `true` if `path` has a `.pdf` extension (case-insensitive).
pub fn is_pdf_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// Returns `true` if `path` has an `.epub` extension (case-insensitive).
pub fn is_epub_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("epub"))
        .unwrap_or(false)
}

/// Extracted text content and metadata from a local file.
#[derive(Debug, Clone)]
pub struct ExtractedDocument {
    pub title: String,
    pub author: Option<String>,
    pub content: String,
    pub word_count: usize,
}

/// Extracts text and word count from a PDF document.
pub fn extract_pdf(path: &Path) -> Result<ExtractedDocument> {
    let text = pdf_extract::extract_text(path)
        .with_context(|| format!("Failed to extract text from PDF file: {:?}", path))?;

    let title = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "PDF Document".to_string());

    let (content, word_count) = clean_raw_text(&text);
    if content.trim().is_empty() {
        anyhow::bail!(
            "PDF document {:?} contained no extractable text (it may be scanned/image-only).",
            path
        );
    }

    Ok(ExtractedDocument {
        title,
        author: None,
        content,
        word_count,
    })
}

/// Extracts text, chapter contents, and metadata from an EPUB document.
pub fn extract_epub(path: &Path) -> Result<ExtractedDocument> {
    let mut doc = epub::doc::EpubDoc::new(path)
        .map_err(|e| anyhow::anyhow!("Failed to open EPUB file: {:?}", e))?;

    let title = doc
        .mdata("title")
        .map(|v| v.value.clone())
        .or_else(|| {
            path.file_stem()
                .map(|s| s.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "EPUB Book".to_string());

    let author = doc.mdata("creator").map(|v| v.value.clone());

    let mut full_text = String::new();
    let num_chapters = doc.get_num_chapters();
    for _ in 0..num_chapters {
        if let Some((page_html, _mime)) = doc.get_current_str() {
            let rendered = html2text::config::plain()
                .link_footnotes(false)
                .no_table_borders()
                .string_from_read(page_html.as_bytes(), 10_000)
                .unwrap_or_default();
            if !rendered.trim().is_empty() {
                if !full_text.is_empty() {
                    full_text.push_str("\n\n");
                }
                full_text.push_str(rendered.trim());
            }
        }
        if !doc.go_next() {
            break;
        }
    }

    let (content, word_count) = clean_raw_text(&full_text);
    if content.trim().is_empty() {
        anyhow::bail!("EPUB file {:?} contained no extractable text.", path);
    }

    Ok(ExtractedDocument {
        title,
        author,
        content,
        word_count,
    })
}

/// Cleans lines and counts words in a single pass.
pub fn clean_raw_text(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut word_count = 0;
    let mut empty_line_count = 0;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            empty_line_count += 1;
            if empty_line_count == 1 && !out.is_empty() {
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
