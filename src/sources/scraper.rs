use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
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
    ]
    .into_iter()
    .filter_map(|s| Selector::parse(s).ok())
    .collect()
});

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub const MAX_SCRAPE_BYTES: usize = 10 * 1024 * 1024; // 10 MB limit for web responses

/// Converts HTML to clean semantic Markdown preserving headings, lists, code blocks, and tables.
pub fn html_to_markdown(html: &str) -> String {
    let converter = htmd::HtmlToMarkdown::new();
    match converter.convert(html) {
        Ok(md) if !md.trim().is_empty() => md,
        _ => html_to_plain_text(html.as_bytes()).unwrap_or_else(|_| html.to_string()),
    }
}

/// Converts HTML bytes to plain text without table borders or footnote links.
pub fn html_to_plain_text(html_bytes: &[u8]) -> Result<String> {
    let html_str = std::str::from_utf8(html_bytes)
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|_| String::from_utf8_lossy(html_bytes));
    let fragment = scraper::Html::parse_document(&html_str);
    let mut out = String::new();
    for text in fragment.root_element().text() {
        let trimmed = text.trim();
        if !trimmed.is_empty() {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str(trimmed);
        }
    }
    Ok(out)
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

    while let Some(chunk) = response
        .chunk()
        .await
        .context("Failed to read response body chunk")?
    {
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
        headers.insert(USER_AGENT, HeaderValue::from_static(DEFAULT_USER_AGENT));
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
        let mut document: Option<Html> = None;

        // 1. Attempt Readability extraction first (dom_smoothie) to avoid full DOM construction when possible
        let (article_title, extracted_html) =
            match dom_smoothie::Readability::new(raw_html, Some(url), None) {
                Ok(mut readability) => match readability.parse() {
                    Ok(article) => {
                        let title = if !article.title.trim().is_empty() {
                            Some(article.title.trim().to_string())
                        } else {
                            None
                        };
                        (title, std::borrow::Cow::Owned(article.content.to_string()))
                    }
                    Err(_) => {
                        let doc = document.get_or_insert_with(|| Html::parse_document(raw_html));
                        (None, self.extract_main_html(doc, raw_html))
                    }
                },
                Err(_) => {
                    let doc = document.get_or_insert_with(|| Html::parse_document(raw_html));
                    (None, self.extract_main_html(doc, raw_html))
                }
            };

        // 2. Extract or fallback Title
        let title = match article_title {
            Some(t) => Some(t),
            None => {
                let doc = document.get_or_insert_with(|| Html::parse_document(raw_html));
                self.extract_title(doc)
            }
        };

        // 3. Convert HTML to clean semantic Markdown (preserving headings, lists, tables)
        let rendered_markdown = html_to_markdown(&extracted_html);

        // 4. Clean and normalize whitespace/formatting + count words in a single pass
        let (cleaned_content, word_count) = clean_text(&rendered_markdown, true, 2);

        Ok(ScrapedPage {
            url: url.to_string(),
            title,
            content: cleaned_content,
            word_count,
        })
    }

    pub fn extract_title(&self, document: &Html) -> Option<String> {
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

    fn extract_main_html<'a>(
        &self,
        document: &Html,
        raw_html: &'a str,
    ) -> std::borrow::Cow<'a, str> {
        for sel in MAIN_SELECTORS.iter() {
            if let Some(element) = document.select(sel).next() {
                let html = element.html();
                if html.len() > 200 {
                    return std::borrow::Cow::Owned(html);
                }
            }
        }

        std::borrow::Cow::Borrowed(raw_html)
    }
}

/// Helper to scan for a matching closing bracket `]`, stopping on newlines or unescaped open brackets.
fn find_closing_bracket(bytes: &[u8], start: usize) -> Option<usize> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b']' {
            return Some(i);
        }
        if bytes[i] == b'[' || bytes[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

/// Helper to scan for a matching closing parenthesis `)` handling nested parentheses.
fn find_closing_paren(bytes: &[u8], start: usize) -> Option<usize> {
    let mut depth = 1usize;
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == b'(' {
            depth += 1;
        } else if bytes[i] == b')' {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        } else if bytes[i] == b'\n' {
            return None;
        }
        i += 1;
    }
    None
}

/// Identifies whether a bracketed anchor represents a footnote citation, Wikipedia edit tag, or metadata reference.
pub fn is_citation_or_footnote(anchor: &str) -> bool {
    let s = anchor.trim();
    if s.is_empty() {
        return true;
    }
    let has_digit = s.chars().any(|c| c.is_ascii_digit());
    let all_citation_chars = s.chars().all(|c| {
        c.is_ascii_digit()
            || c == ','
            || c == '-'
            || c == '–'
            || c == ';'
            || c.is_ascii_whitespace()
    });
    if has_digit && all_citation_chars {
        return true;
    }

    let lower = s.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "edit"
            | "edit source"
            | "citation needed"
            | "clarification needed"
            | "source"
            | "when?"
            | "who?"
            | "where?"
            | "why?"
            | "how?"
            | "dead link"
            | "verification needed"
            | "failed verification"
            | "original research?"
            | "dubious"
            | "disputed"
    ) || lower.starts_with("note ")
        || lower.starts_with("nb ")
        || lower.starts_with("cite_note")
        || lower.starts_with("lower-alpha")
        || lower.starts_with("lower-roman")
}

fn skip_citation_whitespace(out: &mut String, bytes: &[u8], next_idx: usize, i: &mut usize) {
    let len = bytes.len();
    let next_is_space = next_idx < len && bytes[next_idx] == b' ';
    let next_is_punct = next_idx < len
        && matches!(
            bytes[next_idx],
            b'.' | b',' | b';' | b':' | b')' | b'!' | b'?'
        );

    if out.ends_with(' ') {
        if next_is_space {
            *i = next_idx + 1;
        } else if next_is_punct {
            out.pop();
            *i = next_idx;
        } else {
            *i = next_idx;
        }
    } else {
        *i = next_idx;
    }
}

/// Cleans markdown links and citations from a single line of text:
/// - Strips destination URLs from `[anchor](url)` to retain clean prose without URL token bloat.
/// - Drops footnote citations (`[1]`, `[1, 2]`, `[citation needed]`, `[edit]`).
/// - Drops image markdown (`![alt](url)`).
/// - Preserves inline code (backticks) and code syntax like `arr[0]`.
pub fn clean_markdown_line(line: &str) -> Cow<'_, str> {
    if !line.contains('[') && !line.contains('!') {
        return Cow::Borrowed(line);
    }

    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut out = String::with_capacity(len);
    let mut i = 0;
    let mut in_inline_code = false;

    while i < len {
        if bytes[i] == b'`' {
            in_inline_code = !in_inline_code;
            out.push('`');
            i += 1;
            continue;
        }

        if in_inline_code {
            let start = i;
            while i < len && bytes[i] != b'`' {
                i += 1;
            }
            out.push_str(&line[start..i]);
            continue;
        }

        // Markdown image: ![alt](url)
        if bytes[i] == b'!'
            && i + 1 < len
            && bytes[i + 1] == b'['
            && let Some(close_bracket) = find_closing_bracket(bytes, i + 2)
            && close_bracket + 1 < len
            && bytes[close_bracket + 1] == b'('
            && let Some(close_paren) = find_closing_paren(bytes, close_bracket + 2)
        {
            i = close_paren + 1;
            continue;
        }

        // Markdown link or bracket: [anchor]...
        if bytes[i] == b'['
            && let Some(close_bracket) = find_closing_bracket(bytes, i + 1)
        {
            let anchor = &line[i + 1..close_bracket];
            if close_bracket + 1 < len && bytes[close_bracket + 1] == b'(' {
                if let Some(close_paren) = find_closing_paren(bytes, close_bracket + 2) {
                    let next_idx = close_paren + 1;
                    if is_citation_or_footnote(anchor) {
                        skip_citation_whitespace(&mut out, bytes, next_idx, &mut i);
                        continue;
                    }

                    // Emit anchor text (preserving clean prose without URL overhead)
                    out.push_str(anchor);
                    i = next_idx;
                    continue;
                }
            } else if is_citation_or_footnote(anchor) {
                let next_idx = close_bracket + 1;
                skip_citation_whitespace(&mut out, bytes, next_idx, &mut i);
                continue;
            }
        }

        let ch = line[i..].chars().next().unwrap_or(' ');
        out.push(ch);
        i += ch.len_utf8();
    }

    Cow::Owned(out)
}

/// Checks if a line is a reference, bibliography, or appendix heading (including Wikipedia `[edit]` variants).
pub fn is_reference_heading(line: &str) -> bool {
    let mut s = line.trim();
    if s.is_empty() {
        return false;
    }
    // Strip leading Markdown heading markers (#)
    s = s.trim_start_matches('#').trim();
    // Strip leading/trailing MediaWiki heading markers (=)
    s = s.trim_matches('=').trim();

    // Strip trailing [edit], [edit source], (edit), etc.
    if let Some(idx) = s.rfind('[') {
        let suffix = s[idx..].trim();
        if suffix.eq_ignore_ascii_case("[edit]")
            || suffix.eq_ignore_ascii_case("[edit source]")
            || suffix.eq_ignore_ascii_case("[ edit ]")
        {
            s = s[..idx].trim();
        }
    } else if let Some(idx) = s.rfind('(') {
        let suffix = s[idx..].trim();
        if suffix.eq_ignore_ascii_case("(edit)") {
            s = s[..idx].trim();
        }
    }

    matches!(
        s.to_ascii_lowercase().as_str(),
        "references"
            | "external links"
            | "further reading"
            | "notes"
            | "footnotes"
            | "notes and references"
            | "references and notes"
            | "bibliography"
            | "works cited"
            | "sources"
            | "citations"
            | "see also"
    )
}

/// Cleans lines and counts words in a single pass without stripping reference sections.
pub fn clean_raw_text(text: &str) -> (String, usize) {
    clean_text(text, false, 1)
}

/// Cleans lines, strips markdown link URLs and inline citations,
/// optionally strips trailing reference/appendix sections,
/// and computes total word count in a single pass into one pre-sized `String`.
pub fn clean_text(text: &str, strip_references: bool, max_empty_lines: usize) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut word_count = 0;
    let mut empty_line_count = 0;
    let mut in_code_block = false;

    for raw_line in text.lines() {
        let trimmed_line = raw_line.trim();

        if trimmed_line.starts_with("```") || trimmed_line.starts_with("~~~") {
            in_code_block = !in_code_block;
        }

        if !in_code_block && strip_references && is_reference_heading(trimmed_line) {
            break;
        }

        let cleaned_line = if in_code_block {
            Cow::Borrowed(trimmed_line)
        } else {
            clean_markdown_line(trimmed_line)
        };
        let line = cleaned_line.trim();

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scraper_normalize_with_readability() {
        let scraper = WebScraper::new().unwrap();
        let html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <title>Article Page</title>
                <meta property="og:title" content="Extracted Article Title">
            </head>
            <body>
                <header><nav><a href="/">Home</a><a href="/about">About</a></nav></header>
                <aside class="sidebar"><p>Ad sidebar with lots of links</p></aside>
                <article>
                    <h1>Article Main Heading</h1>
                    <p>This is the first paragraph of the core article text with meaningful content.</p>
                    <p>This is the second paragraph providing in-depth analysis on the topic.</p>
                </article>
                <footer><p>© 2026 Company Inc. All rights reserved.</p></footer>
            </body>
            </html>
        "#;

        let page = scraper
            .normalize("https://example.com/article", html)
            .unwrap();
        assert_eq!(page.title.as_deref(), Some("Extracted Article Title"));
        assert!(
            page.content
                .contains("first paragraph of the core article text")
        );
        assert!(
            page.content
                .contains("second paragraph providing in-depth analysis")
        );
        assert!(!page.content.contains("Ad sidebar with lots of links"));
        assert!(!page.content.contains("All rights reserved"));
    }

    #[test]
    fn test_html_to_markdown_preserves_semantics() {
        let html = "<h2>Overview</h2><p>Here is a list:</p><ul><li>First item</li><li>Second item</li></ul>";
        let md = html_to_markdown(html);
        assert!(md.contains("## Overview") || md.contains("Overview"));
        assert!(md.contains("First item"));
        assert!(md.contains("Second item"));
    }

    #[test]
    fn test_clean_markdown_line_strips_link_urls() {
        let line = "Rust is a [systems programming language](https://en.wikipedia.org/wiki/Systems_programming_language) created by [Graydon Hoare](https://en.wikipedia.org/wiki/Graydon_Hoare).";
        let cleaned = clean_markdown_line(line);
        assert_eq!(
            cleaned,
            "Rust is a systems programming language created by Graydon Hoare."
        );
    }

    #[test]
    fn test_clean_markdown_line_nested_parentheses_url() {
        let line =
            "Learn about [Rust](https://en.wikipedia.org/wiki/Rust_(programming_language)) today.";
        let cleaned = clean_markdown_line(line);
        assert_eq!(cleaned, "Learn about Rust today.");
    }

    #[test]
    fn test_clean_markdown_line_strips_citations_and_edit_tags() {
        let line =
            "Rust was announced in 2010[12][13] by Mozilla Research[14], winning awards [15].";
        let cleaned = clean_markdown_line(line);
        assert_eq!(
            cleaned,
            "Rust was announced in 2010 by Mozilla Research, winning awards."
        );

        let line2 = "It is considered safe[citation needed] and efficient [note 1].";
        let cleaned2 = clean_markdown_line(line2);
        assert_eq!(cleaned2, "It is considered safe and efficient.");
    }

    #[test]
    fn test_clean_markdown_line_preserves_inline_code_and_brackets() {
        let line = "Access element with `arr[0]` or check [a-z] regex.";
        let cleaned = clean_markdown_line(line);
        assert_eq!(
            cleaned,
            "Access element with `arr[0]` or check [a-z] regex."
        );
    }

    #[test]
    fn test_clean_text_preserves_code_blocks() {
        let md = "Some intro text.\n\n```rust\nfn main() {\n    let arr = [1, 2, 3];\n    println!(\"{}\", arr[0]);\n}\n```\n\nOutro text.";
        let (cleaned, _) = clean_text(md, true, 2);
        assert!(cleaned.contains("let arr = [1, 2, 3];"));
        assert!(cleaned.contains("println!(\"{}\", arr[0]);"));
    }

    #[test]
    fn test_is_reference_heading_wikipedia_variants() {
        assert!(is_reference_heading("## References"));
        assert!(is_reference_heading("## References [edit]"));
        assert!(is_reference_heading("## References [edit source]"));
        assert!(is_reference_heading("### External links [edit]"));
        assert!(is_reference_heading("== References =="));
        assert!(is_reference_heading("=== Further reading ==="));
        assert!(is_reference_heading("## Notes and references"));
        assert!(is_reference_heading("## See also"));
        assert!(!is_reference_heading("## Language features"));
        assert!(!is_reference_heading("## History of Rust"));
    }

    #[test]
    fn test_clean_text_truncates_at_reference_heading() {
        let doc =
            "Core article content.\n\nMore details.\n\n## References [edit]\n\n1. Citation link";
        let (cleaned, _) = clean_text(doc, true, 2);
        assert!(cleaned.contains("Core article content."));
        assert!(cleaned.contains("More details."));
        assert!(!cleaned.contains("References"));
        assert!(!cleaned.contains("Citation link"));
    }
}
