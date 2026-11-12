use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::Duration;

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
                && let Some(close_offset) =
                    crate::system::text_util::find_subslice_ignore_case(&bytes[cursor..], closing)
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

fn lower_ascii_tag<'a>(name: &str, buf: &'a mut [u8; 16]) -> &'a str {
    let local = crate::system::text_util::xml_local_name(name);
    let len = local.len();
    if len > buf.len() || !local.is_ascii() {
        return "";
    }
    for (dst, &src) in buf[..len].iter_mut().zip(local.as_bytes()) {
        *dst = src.to_ascii_lowercase();
    }
    std::str::from_utf8(&buf[..len]).unwrap_or("")
}

/// Converts HTML to clean semantic Markdown preserving headings, lists, code blocks, and formatting.
pub fn html_to_markdown(html: &str) -> String {
    use crate::system::text_util::{XmlEvent, XmlScanner, decode_html_entities};

    let mut scanner = XmlScanner::new(html);
    let mut out = String::with_capacity(html.len());
    let mut list_depth: usize = 0;
    let mut buf = [0u8; 16];

    while let Some(event) = scanner.next_event() {
        match event {
            XmlEvent::Start { name, .. } => match lower_ascii_tag(name, &mut buf) {
                "h1" => out.push_str("\n\n# "),
                "h2" => out.push_str("\n\n## "),
                "h3" => out.push_str("\n\n### "),
                "h4" => out.push_str("\n\n#### "),
                "h5" => out.push_str("\n\n##### "),
                "h6" => out.push_str("\n\n###### "),
                "p" | "div" | "article" | "section" => {
                    if !out.ends_with("\n\n") && !out.is_empty() {
                        out.push_str("\n\n");
                    }
                }
                "br" => out.push('\n'),
                "hr" => out.push_str("\n\n---\n\n"),
                "ul" | "ol" => {
                    list_depth = list_depth.saturating_add(1);
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                "li" => {
                    out.push('\n');
                    for _ in 0..list_depth.saturating_sub(1) {
                        out.push_str("  ");
                    }
                    out.push_str("* ");
                }
                "pre" => out.push_str("\n```\n"),
                "code" => out.push('`'),
                "strong" | "b" => out.push_str("**"),
                "em" | "i" => out.push('*'),
                "blockquote" => out.push_str("\n> "),
                _ => {}
            },
            XmlEvent::End { name } => match lower_ascii_tag(name, &mut buf) {
                "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" => {
                    if !out.ends_with("\n\n") {
                        out.push_str("\n\n");
                    }
                }
                "ul" | "ol" => {
                    list_depth = list_depth.saturating_sub(1);
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                }
                "pre" => out.push_str("\n```\n\n"),
                "code" => out.push('`'),
                "strong" | "b" => out.push_str("**"),
                "em" | "i" => out.push('*'),
                _ => {}
            },
            XmlEvent::Text(text) => out.push_str(&decode_html_entities(text)),
            XmlEvent::CData(cdata) => out.push_str(cdata),
        }
    }

    if out.trim().is_empty() {
        crate::system::text_util::strip_html_tags(html)
    } else {
        out
    }
}

/// Converts HTML bytes to plain text without markup.
pub fn html_to_plain_text(html_bytes: &[u8]) -> Result<String> {
    let html_str = std::str::from_utf8(html_bytes)
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|_| String::from_utf8_lossy(html_bytes));

    let md = html_to_markdown(&html_str);
    let (cleaned, _) = clean_text(&md, false, 1);
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

/// Returns true if the given IPv4 address belongs to a private, loopback, link-local,
/// or non-routable subnet (RFC 1918, RFC 3927, RFC 6598, etc.).
pub fn is_private_ipv4(ipv4: std::net::Ipv4Addr) -> bool {
    let o = ipv4.octets();
    o[0] == 0
        || o[0] == 127
        || o[0] == 10
        || (o[0] == 172 && (16..=31).contains(&o[1]))
        || (o[0] == 192 && o[1] == 168)
        || (o[0] == 169 && o[1] == 254)
        || (o[0] == 100 && (64..=127).contains(&o[1]))
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 192 && o[1] == 0 && o[2] == 2)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 224
}

/// Returns true if the given IPv6 address belongs to a loopback, unique-local, link-local,
/// multicast, transitional, documentation, or IPv4-mapped private address.
pub fn is_private_ipv6(ipv6: std::net::Ipv6Addr) -> bool {
    let s = ipv6.segments();
    if ipv6.is_loopback() || ipv6.is_unspecified() || ipv6.is_multicast() {
        return true;
    }
    // IPv4-compatible (::/96) and IPv4-mapped (::ffff:0:0/96)
    if s[0] == 0
        && s[1] == 0
        && s[2] == 0
        && s[3] == 0
        && s[4] == 0
        && (s[5] == 0 || s[5] == 0xffff)
    {
        let ip4 = std::net::Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        );
        return is_private_ipv4(ip4);
    }
    // NAT64 (64:ff9b::/96, 64:ff9b:1::/48)
    if s[0] == 0x0064 && s[1] == 0xff9b && ((s[2..=5] == [0, 0, 0, 0]) || s[2] == 0x0001) {
        return true;
    }
    // Discard-Only (100::/64)
    if s[0] == 0x0100 && s[1] == 0 && s[2] == 0 && s[3] == 0 {
        return true;
    }
    // Teredo (2001::/32), Benchmarking (2001:2::/48), Documentation (2001:db8::/32), 6to4 (2002::/16)
    if (s[0] == 0x2001 && (s[1] == 0 || (s[1] == 0x0002 && s[2] == 0) || s[1] == 0x0db8))
        || s[0] == 0x2002
    {
        return true;
    }
    let o = ipv6.octets();
    // Unique local (fc00::/7) and link-local unicast (fe80::/10)
    (o[0] & 0xfe) == 0xfc || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80)
}

/// Checks if a host name or IP address resolves to a local or private network destination.
pub fn is_private_or_local_host(host: &str) -> bool {
    let unbracketed = host.trim_start_matches('[').trim_end_matches(']');
    if unbracketed.contains('%') {
        return true;
    }
    let lower = unbracketed.to_ascii_lowercase();
    let lower_trim = lower.trim_end_matches('.');
    if lower_trim == "localhost"
        || lower_trim.ends_with(".localhost")
        || lower_trim.ends_with(".local")
        || lower_trim.ends_with(".internal")
        || lower_trim.ends_with(".lan")
    {
        return true;
    }
    if let Ok(ipv4) = unbracketed.parse::<std::net::Ipv4Addr>() {
        return is_private_ipv4(ipv4);
    }
    if let Ok(ipv6) = unbracketed.parse::<std::net::Ipv6Addr>() {
        return is_private_ipv6(ipv6);
    }
    false
}

/// Validates a parsed URL for SSRF safety (HTTP/HTTPS only, no userinfo, no private/local hosts).
pub fn validate_parsed_public_http_url(parsed: &reqwest::Url) -> Result<()> {
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        anyhow::bail!(
            "Unsupported URL scheme '{}': only http and https are permitted",
            scheme
        );
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        anyhow::bail!("URLs containing embedded credentials are not permitted");
    }
    let host = parsed.host_str().context("URL is missing a host")?;
    if is_private_or_local_host(host) {
        anyhow::bail!(
            "Blocked request to private or local network address '{}' (SSRF protection)",
            host
        );
    }
    Ok(())
}

/// Validates that `url_str` uses HTTP or HTTPS and does not target private or local network hosts (SSRF mitigation).
pub fn validate_public_http_url(url_str: &str) -> Result<()> {
    let parsed =
        reqwest::Url::parse(url_str).with_context(|| format!("Invalid URL: {}", url_str))?;
    validate_parsed_public_http_url(&parsed)
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
            .redirect(reqwest::redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= 10 {
                    return attempt.error("Too many redirects (max 10)");
                }
                if let Err(err) = validate_parsed_public_http_url(attempt.url()) {
                    return attempt.error(err.to_string());
                }
                attempt.follow()
            }))
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
        validate_public_http_url(url)?;

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
                let stripped = crate::system::text_util::strip_html_tags(&content[..end]);
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

    #[test]
    fn test_ssrf_validation_blocks_private_and_local_targets() {
        // Blocked: localhost & local domains
        assert!(validate_public_http_url("http://localhost/").is_err());
        assert!(validate_public_http_url("http://localhost:8080/admin").is_err());
        assert!(validate_public_http_url("http://server.local/").is_err());
        assert!(validate_public_http_url("http://service.internal/").is_err());

        // Blocked: IPv4 loopback & RFC1918 & link-local
        assert!(validate_public_http_url("http://127.0.0.1/").is_err());
        assert!(validate_public_http_url("http://127.0.0.2:9000/").is_err());
        assert!(validate_public_http_url("http://10.0.0.1/status").is_err());
        assert!(validate_public_http_url("http://172.16.0.1/").is_err());
        assert!(validate_public_http_url("http://172.31.255.254/").is_err());
        assert!(validate_public_http_url("http://192.168.1.1/router").is_err());
        assert!(validate_public_http_url("http://169.254.169.254/latest/meta-data").is_err());
        assert!(validate_public_http_url("http://0.0.0.0/").is_err());

        // Blocked: IPv6 loopback, link-local, special/transitional, and zone-scoped targets
        assert!(validate_public_http_url("http://[::1]/").is_err());
        assert!(validate_public_http_url("http://[fe80::1]/").is_err());
        assert!(validate_public_http_url("http://[fe80::1%25eth0]/").is_err());
        assert!(validate_public_http_url("http://[::ffff:127.0.0.1]/").is_err());
        assert!(validate_public_http_url("http://[::ffff:169.254.169.254]/").is_err());
        assert!(validate_public_http_url("http://[64:ff9b::7f00:1]/").is_err());
        assert!(validate_public_http_url("http://[2002:7f00:1::]/").is_err());
        assert!(validate_public_http_url("http://[2001::1]/").is_err());
        assert!(validate_public_http_url("http://[2001:db8::1]/").is_err());
        assert!(validate_public_http_url("http://[ff02::1]/").is_err());

        // Blocked: embedded userinfo credentials
        assert!(validate_public_http_url("http://user:pass@example.com/").is_err());
        assert!(validate_public_http_url("https://token@example.com/").is_err());

        // Blocked: non-HTTP schemes
        assert!(validate_public_http_url("ftp://example.com/file").is_err());
        assert!(validate_public_http_url("file:///etc/passwd").is_err());
        assert!(validate_public_http_url("gopher://example.com/").is_err());

        // Allowed: public HTTP/HTTPS URLs
        assert!(validate_public_http_url("https://example.com").is_ok());
        assert!(validate_public_http_url("https://en.wikipedia.org/wiki/Rust").is_ok());
        assert!(validate_public_http_url("http://93.184.216.34/").is_ok());
    }
}
