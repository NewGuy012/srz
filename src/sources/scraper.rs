use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::sync::LazyLock;
use std::time::Duration;

static HTMD_CONVERTER: LazyLock<htmd::HtmlToMarkdown> = LazyLock::new(htmd::HtmlToMarkdown::new);

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub const MAX_SCRAPE_BYTES: usize = 10 * 1024 * 1024; // 10 MB limit for web responses

/// Helper to check if a byte slice starts with a given tag name case-insensitively,
/// ensuring a tag delimiter (whitespace, '>', or '/') follows immediately.
fn starts_with_tag(slice: &[u8], prefix: &[u8]) -> bool {
    if slice.len() < prefix.len() {
        return false;
    }
    for (a, b) in slice[..prefix.len()].iter().zip(prefix.iter()) {
        if !a.eq_ignore_ascii_case(b) {
            return false;
        }
    }
    if slice.len() > prefix.len() {
        let next = slice[prefix.len()];
        next.is_ascii_whitespace() || next == b'>' || next == b'/'
    } else {
        true
    }
}

/// Finds the index of `needle` in `haystack` case-insensitively.
fn find_subslice_ignore_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|window| {
        window
            .iter()
            .zip(needle.iter())
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

/// Quickly strips `<script>...</script>`, `<style>...</style>`, `<svg>...</svg>`,
/// and `<noscript>...</noscript>` tags from raw HTML to prevent massive DOM bloat in Readability.
pub fn strip_boilerplate_html_tags(html: &str) -> Cow<'_, str> {
    let has_script = crate::system::text_util::contains_ascii_case_insensitive(html, "<script");
    let has_style = crate::system::text_util::contains_ascii_case_insensitive(html, "<style");
    let has_svg = crate::system::text_util::contains_ascii_case_insensitive(html, "<svg");
    let has_noscript = crate::system::text_util::contains_ascii_case_insensitive(html, "<noscript");

    if !has_script && !has_style && !has_svg && !has_noscript {
        return Cow::Borrowed(html);
    }

    let mut out = String::with_capacity(html.len());
    let bytes = html.as_bytes();
    let mut cursor = 0;
    let len = bytes.len();

    while cursor < len {
        if bytes[cursor] == b'<' {
            let rest = &bytes[cursor..];
            let closing_tag: Option<&'static [u8]> = if starts_with_tag(rest, b"<script") {
                Some(b"</script>")
            } else if starts_with_tag(rest, b"<style") {
                Some(b"</style>")
            } else if starts_with_tag(rest, b"<svg") {
                Some(b"</svg>")
            } else if starts_with_tag(rest, b"<noscript") {
                Some(b"</noscript>")
            } else {
                None
            };

            if let Some(closing) = closing_tag
                && let Some(close_offset) = find_subslice_ignore_case(&bytes[cursor..], closing)
            {
                cursor += close_offset + closing.len();
                continue;
            }
        }

        let next_tag = bytes[cursor + 1..]
            .iter()
            .position(|&b| b == b'<')
            .map(|pos| cursor + 1 + pos)
            .unwrap_or(len);

        out.push_str(&html[cursor..next_tag]);
        cursor = next_tag;
    }

    Cow::Owned(out)
}

/// Truncates Wikipedia HTML before the references, bibliography, and navigation footer,
/// avoiding expensive DOM parsing and Markdown conversion of thousands of citation links.
pub fn truncate_wikipedia_html_before_references(html: &str) -> Cow<'_, str> {
    const WIKI_REFERENCE_MARKERS: &[&str] = &[
        "id=\"references\"",
        "id=\"bibliography\"",
        "id=\"works_cited\"",
        "class=\"reflist",
        "class=\"references",
    ];

    let lower = html.to_ascii_lowercase();
    let mut cut_idx = None;

    for marker in WIKI_REFERENCE_MARKERS {
        if let Some(pos) = lower.find(marker) {
            let tag_start = html[..pos].rfind('<').unwrap_or(pos);
            cut_idx = Some(match cut_idx {
                Some(prev) => std::cmp::min(prev, tag_start),
                None => tag_start,
            });
        }
    }

    if let Some(idx) = cut_idx
        && idx > 1000
    {
        let mut truncated = String::with_capacity(idx + 32);
        truncated.push_str(&html[..idx]);
        truncated.push_str("</div></body></html>");
        return Cow::Owned(truncated);
    }

    Cow::Borrowed(html)
}

/// Converts HTML to clean semantic Markdown preserving headings, lists, code blocks, and tables.
pub fn html_to_markdown(html: &str) -> String {
    match HTMD_CONVERTER.convert(html) {
        Ok(md) if !md.trim().is_empty() => md,
        _ => html_to_plain_text(html.as_bytes()).unwrap_or_else(|_| html.to_string()),
    }
}

/// Converts HTML bytes to plain text without table borders or footnote links.
pub fn html_to_plain_text(html_bytes: &[u8]) -> Result<String> {
    let html_str = std::str::from_utf8(html_bytes)
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|_| String::from_utf8_lossy(html_bytes));

    if let Ok(md) = HTMD_CONVERTER.convert(&html_str) {
        if !md.trim().is_empty() {
            let (cleaned, _) = clean_text(&md, false, 1);
            return Ok(cleaned);
        }
    }

    let mut out = String::with_capacity(html_str.len());
    let mut in_tag = false;
    for c in html_str.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
            out.push(' ');
        } else if !in_tag {
            out.push(c);
        }
    }
    let (cleaned, _) = clean_text(&out, false, 1);
    Ok(cleaned)
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

    /// Normalizes raw HTML into clean, readable text using Readability and Markdown conversion.
    pub fn normalize(&self, url: &str, raw_html: &str) -> Result<ScrapedPage> {
        let pre_cleaned = if url.contains("wikipedia.org") {
            truncate_wikipedia_html_before_references(raw_html)
        } else {
            Cow::Borrowed(raw_html)
        };
        let sanitized = strip_boilerplate_html_tags(pre_cleaned.as_ref());

        // 1. Attempt Readability extraction first (dom_smoothie)
        let (article_title, extracted_html) =
            match dom_smoothie::Readability::new(sanitized.as_ref(), Some(url), None) {
                Ok(mut readability) => match readability.parse() {
                    Ok(article) => {
                        let title = if !article.title.trim().is_empty() {
                            Some(article.title.trim().to_string())
                        } else {
                            None
                        };
                        (title, std::borrow::Cow::Owned(article.content.to_string()))
                    }
                    Err(_) => (None, Cow::Borrowed(raw_html)),
                },
                Err(_) => (None, Cow::Borrowed(raw_html)),
            };

        // 2. Extract or fallback Title
        let title = match article_title {
            Some(t) => Some(t),
            None => extract_title_from_html(raw_html),
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
}

/// Extracts title from raw HTML by inspecting `<meta property="og:title">`, `<title>`, or `<h1>`.
pub fn extract_title_from_html(html: &str) -> Option<String> {
    // 1. Try og:title
    if let Some(pos) =
        crate::system::text_util::find_ascii_case_insensitive(html, "property=\"og:title\"")
            .or_else(|| {
                crate::system::text_util::find_ascii_case_insensitive(html, "property='og:title'")
            })
    {
        let tag_start = html[..pos].rfind('<').unwrap_or(pos);
        let tag_end = html[pos..].find('>').map(|p| pos + p).unwrap_or(html.len());
        let tag = &html[tag_start..tag_end];
        if let Some(c_pos) = tag.find("content=\"") {
            let val = &tag[c_pos + 9..];
            if let Some(end) = val.find('\"') {
                let trimmed = val[..end].trim();
                if !trimmed.is_empty() {
                    return Some(
                        crate::system::text_util::decode_html_entities(trimmed).into_owned(),
                    );
                }
            }
        } else if let Some(c_pos) = tag.find("content='") {
            let val = &tag[c_pos + 9..];
            if let Some(end) = val.find('\'') {
                let trimmed = val[..end].trim();
                if !trimmed.is_empty() {
                    return Some(
                        crate::system::text_util::decode_html_entities(trimmed).into_owned(),
                    );
                }
            }
        }
    }

    // 2. Try <title>...</title>
    if let Some(pos) = crate::system::text_util::find_ascii_case_insensitive(html, "<title") {
        let rest = &html[pos + 6..];
        if let Some(tag_end) = rest.find('>') {
            let content = &rest[tag_end + 1..];
            if let Some(end) =
                crate::system::text_util::find_ascii_case_insensitive(content, "</title>")
            {
                let trimmed = content[..end].trim();
                if !trimmed.is_empty() {
                    return Some(
                        crate::system::text_util::decode_html_entities(trimmed).into_owned(),
                    );
                }
            }
        }
    }

    // 3. Try <h1>...</h1>
    if let Some(pos) = crate::system::text_util::find_ascii_case_insensitive(html, "<h1") {
        let rest = &html[pos + 3..];
        if let Some(tag_end) = rest.find('>') {
            let content = &rest[tag_end + 1..];
            if let Some(end) =
                crate::system::text_util::find_ascii_case_insensitive(content, "</h1>")
            {
                let mut stripped = String::new();
                let mut in_tag = false;
                for c in content[..end].chars() {
                    if c == '<' {
                        in_tag = true;
                    } else if c == '>' {
                        in_tag = false;
                    } else if !in_tag {
                        stripped.push(c);
                    }
                }
                let trimmed = stripped.trim();
                if !trimmed.is_empty() {
                    return Some(
                        crate::system::text_util::decode_html_entities(trimmed).into_owned(),
                    );
                }
            }
        }
    }

    None
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

/// Cleans lines and counts words in a single pass without stripping reference sections (exposed for integration testing).
#[doc(hidden)]
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
    fn test_strip_boilerplate_html_tags() {
        let html = r#"<html><head><script type="application/json">{"data": 123}</script><style>body { color: red; }</style><title>Test</title></head><body><svg><path d="M0 0"/></svg><noscript>Enable JS</noscript><p>Actual article content here.</p></body></html>"#;
        let stripped = strip_boilerplate_html_tags(html);
        assert!(!stripped.contains("application/json"));
        assert!(!stripped.contains("color: red"));
        assert!(!stripped.contains("<svg"));
        assert!(!stripped.contains("Enable JS"));
        assert!(stripped.contains("<title>Test</title>"));
        assert!(stripped.contains("<p>Actual article content here.</p>"));
    }

    #[test]
    fn test_truncate_wikipedia_html_before_references() {
        let padding = "A".repeat(1200);
        let html = format!(
            "<html><body><p>Article introduction and body: {}</p><h2 id=\"References\">References</h2><div class=\"reflist\"><ul><li>Citation 1</li></ul></div></body></html>",
            padding
        );

        let truncated = truncate_wikipedia_html_before_references(&html);
        assert!(truncated.contains("Article introduction and body"));
        assert!(!truncated.contains("<h2 id=\"References\">"));
        assert!(!truncated.contains("reflist"));
        assert!(!truncated.contains("Citation 1"));
        assert!(truncated.ends_with("</div></body></html>"));
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
