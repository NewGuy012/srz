use crate::system::text_util::{XmlEvent, XmlScanner, xml_local_name};
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::time::Duration;

pub const DEFAULT_USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36";

pub const MAX_SCRAPE_BYTES: usize = 10 * 1024 * 1024; // 10 MB limit for web responses

fn starts_with_tag(slice: &[u8], prefix: &[u8]) -> bool {
    slice.len() >= prefix.len()
        && slice[..prefix.len()].eq_ignore_ascii_case(prefix)
        && slice
            .get(prefix.len())
            .is_none_or(|&b| b.is_ascii_whitespace() || b == b'>' || b == b'/')
}

fn find_open_tag_end(bytes: &[u8]) -> Option<usize> {
    let mut i = 1;
    let mut quote = 0u8;
    while i < bytes.len() {
        let b = bytes[i];
        if quote != 0 {
            if b == quote {
                quote = 0;
            }
        } else if b == b'"' || b == b'\'' {
            quote = b;
        } else if b == b'>' {
            return Some(i);
        } else if b == b'<' {
            return None;
        }
        i += 1;
    }
    None
}

fn find_closing_tag_end(bytes: &[u8], close_prefix: &[u8]) -> Option<usize> {
    let mut offset = 0;
    while let Some(pos) =
        crate::system::text_util::find_subslice_ignore_case(&bytes[offset..], close_prefix)
    {
        let abs = offset + pos + close_prefix.len();
        let mut i = abs;
        while bytes.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        if bytes.get(i) == Some(&b'>') {
            return Some(i + 1);
        }
        offset = abs;
    }
    None
}

fn strip_boilerplate_html_tags(html: &str, strip_head: bool) -> Cow<'_, str> {
    const TAGS: [(&[u8], &[u8]); 5] = [
        (b"<script", b"</script"),
        (b"<style", b"</style"),
        (b"<svg", b"</svg"),
        (b"<noscript", b"</noscript"),
        (b"<head", b"</head"),
    ];
    let active_tags = if strip_head { &TAGS[..] } else { &TAGS[..4] };
    let bytes = html.as_bytes();
    let has_comments = bytes.windows(4).any(|w| w == b"<!--");
    if !has_comments
        && !active_tags.iter().any(|(open, _)| {
            crate::system::text_util::find_subslice_ignore_case(bytes, open).is_some()
        })
    {
        return Cow::Borrowed(html);
    }

    let mut out = String::with_capacity(html.len());
    let mut cursor = 0;
    let mut missing_closing_mask = 0u8;

    while cursor < bytes.len() {
        if bytes[cursor..].starts_with(b"<!--") {
            cursor = bytes[cursor + 4..]
                .windows(3)
                .position(|w| w == b"-->")
                .map_or(bytes.len(), |p| cursor + 4 + p + 3);
            continue;
        }
        if bytes[cursor] == b'<'
            && let Some((idx, (open_b, close_prefix))) = active_tags
                .iter()
                .enumerate()
                .find(|(_, (open_b, _))| starts_with_tag(&bytes[cursor..], open_b))
        {
            if let Some(tag_end) = find_open_tag_end(&bytes[cursor..])
                && tag_end > open_b.len()
                && bytes[cursor + tag_end - 1] == b'/'
            {
                cursor += tag_end + 1;
                continue;
            }
            if (missing_closing_mask & (1 << idx)) == 0 {
                if let Some(close_end) = find_closing_tag_end(&bytes[cursor..], close_prefix) {
                    cursor += close_end;
                    continue;
                }
                missing_closing_mask |= 1 << idx;
            }
        }
        let next_tag = bytes[cursor + 1..]
            .iter()
            .position(|&b| b == b'<')
            .map_or(bytes.len(), |pos| cursor + 1 + pos);
        out.push_str(&html[cursor..next_tag]);
        cursor = next_tag;
    }

    Cow::Owned(out)
}

fn truncate_wikipedia_html_before_references(html: &str) -> Cow<'_, str> {
    const WIKI_REFERENCE_MARKERS: &[&[u8]] = &[
        b"id=\"references\"",
        b"id=\"bibliography\"",
        b"id=\"works_cited\"",
        b"class=\"reflist",
        b"class=\"references",
    ];

    let bytes = html.as_bytes();
    let mut cut_idx = None;

    for marker in WIKI_REFERENCE_MARKERS {
        if let Some(pos) = crate::system::text_util::find_subslice_ignore_case(bytes, marker) {
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

fn ascii_lower_tag<'a>(s: &str, buf: &'a mut [u8; 16]) -> Option<&'a str> {
    let b = s.as_bytes();
    if b.len() > buf.len() {
        return None;
    }
    for (dst, &src) in buf.iter_mut().zip(b.iter()) {
        *dst = src.to_ascii_lowercase();
    }
    std::str::from_utf8(&buf[..b.len()]).ok()
}

/// Converts HTML to Markdown preserving headings, lists, tables, code blocks, and formatting.
pub fn html_to_markdown(html: &str) -> String {
    let sanitized = strip_boilerplate_html_tags(html, true);
    let mut scanner = XmlScanner::new(sanitized.as_ref());
    let mut out = String::with_capacity(sanitized.len());
    let mut list_depth: usize = 0;
    let mut in_pre: usize = 0;
    let mut row_has_cell = false;
    let mut tag_buf = [0u8; 16];

    while let Some(event) = scanner.next_event() {
        match event {
            XmlEvent::Start { name, is_empty, .. } => {
                let Some(tag) = ascii_lower_tag(xml_local_name(name), &mut tag_buf) else {
                    continue;
                };
                match tag {
                    "br" => out.push('\n'),
                    "hr" => out.push_str("\n\n---\n\n"),
                    _ if is_empty => {}
                    "h1" => out.push_str("\n\n# "),
                    "h2" => out.push_str("\n\n## "),
                    "h3" => out.push_str("\n\n### "),
                    "h4" => out.push_str("\n\n#### "),
                    "h5" => out.push_str("\n\n##### "),
                    "h6" => out.push_str("\n\n###### "),
                    "p" | "div" | "article" | "section" | "table" => {
                        if !out.ends_with("\n\n") && !out.is_empty() {
                            out.push_str("\n\n");
                        }
                    }
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
                    "tr" => {
                        if !out.ends_with('\n') && !out.is_empty() {
                            out.push('\n');
                        }
                        row_has_cell = false;
                    }
                    "td" | "th" => {
                        if row_has_cell {
                            out.push_str(" | ");
                        } else {
                            row_has_cell = true;
                        }
                    }
                    "pre" => {
                        in_pre = in_pre.saturating_add(1);
                        out.push_str("\n```\n");
                    }
                    "code" if in_pre == 0 => out.push('`'),
                    "strong" | "b" => out.push_str("**"),
                    "em" | "i" => out.push('*'),
                    "blockquote" => out.push_str("\n> "),
                    _ => {}
                }
            }
            XmlEvent::End { name } => {
                let Some(tag) = ascii_lower_tag(xml_local_name(name), &mut tag_buf) else {
                    continue;
                };
                match tag {
                    "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "p" | "div" | "table" => {
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
                    "tr" => {
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                        row_has_cell = false;
                    }
                    "pre" => {
                        in_pre = in_pre.saturating_sub(1);
                        out.push_str("\n```\n\n");
                    }
                    "code" if in_pre == 0 => out.push('`'),
                    "strong" | "b" => out.push_str("**"),
                    "em" | "i" => out.push('*'),
                    _ => {}
                }
            }
            XmlEvent::Text(text) => {
                out.push_str(&crate::system::text_util::decode_html_entities(text));
            }
            XmlEvent::CData(cdata) => out.push_str(cdata),
        }
    }

    if out.trim().is_empty() {
        crate::system::text_util::strip_html_tags(sanitized.as_ref())
    } else {
        out
    }
}

fn format_byte_limit(max_bytes: usize) -> String {
    if max_bytes >= 1024 * 1024 {
        format!("{} MB", max_bytes / (1024 * 1024))
    } else if max_bytes >= 1024 {
        format!("{} KB", max_bytes / 1024)
    } else {
        format!("{max_bytes} bytes")
    }
}

pub(super) fn bytes_to_lossy_utf8(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        Err(e) => String::from_utf8_lossy(e.as_bytes()).into_owned(),
    }
}

pub(super) async fn send_checked(
    request: reqwest::RequestBuilder,
    label: &str,
) -> Result<reqwest::Response> {
    let response = request
        .send()
        .await
        .with_context(|| format!("Failed to fetch {label}"))?;
    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to fetch {label} (HTTP status: {})",
            response.status()
        );
    }
    Ok(response)
}

/// Reads response body bytes up to `max_bytes`, failing if content length or streamed bytes exceed `max_bytes`.
pub async fn read_response_bytes_limited(
    mut response: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    let initial_cap = if let Some(content_len) = response.content_length() {
        if content_len > max_bytes as u64 {
            anyhow::bail!(
                "Response content length ({} bytes) exceeds limit of {}",
                content_len,
                format_byte_limit(max_bytes)
            );
        }
        usize::try_from(content_len)
            .unwrap_or(max_bytes)
            .min(max_bytes)
            .min(64 * 1024)
    } else {
        0
    };

    let mut bytes = Vec::with_capacity(initial_cap);
    while let Some(chunk) = response
        .chunk()
        .await
        .context("Failed to read response body chunk")?
    {
        if chunk.len() > max_bytes.saturating_sub(bytes.len()) {
            anyhow::bail!(
                "Response body exceeded limit of {}",
                format_byte_limit(max_bytes)
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
    Ok(bytes_to_lossy_utf8(
        read_response_bytes_limited(response, max_bytes).await?,
    ))
}

/// Sends an HTTP request, validates a 2xx status, and reads up to `max_bytes` bytes.
pub async fn fetch_bytes_limited(
    request: reqwest::RequestBuilder,
    max_bytes: usize,
    label: &str,
) -> Result<Vec<u8>> {
    let response = send_checked(request, label).await?;
    read_response_bytes_limited(response, max_bytes).await
}

/// Sends an HTTP request, validates a 2xx status, and reads up to `max_bytes` UTF-8 text.
pub async fn fetch_text_limited(
    request: reqwest::RequestBuilder,
    max_bytes: usize,
    label: &str,
) -> Result<String> {
    let response = send_checked(request, label).await?;
    read_response_text_limited(response, max_bytes).await
}

fn is_private_ipv4(ipv4: std::net::Ipv4Addr) -> bool {
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
        || (o[0] == 192 && o[1] == 88 && o[2] == 99)
        || (o[0] == 198 && (o[1] & 0xfe) == 18)
        || (o[0] == 198 && o[1] == 51 && o[2] == 100)
        || (o[0] == 203 && o[1] == 0 && o[2] == 113)
        || o[0] >= 224
}

fn is_private_ipv6(ipv6: std::net::Ipv6Addr) -> bool {
    let s = ipv6.segments();
    // IPv4-mapped (::ffff:0:0/96): allow only when embedded IPv4 is public
    if s[0..5] == [0, 0, 0, 0, 0] && s[5] == 0xffff {
        let ip4 = std::net::Ipv4Addr::new(
            (s[6] >> 8) as u8,
            (s[6] & 0xff) as u8,
            (s[7] >> 8) as u8,
            (s[7] & 0xff) as u8,
        );
        return is_private_ipv4(ip4);
    }
    // Default-deny outside Global Unicast (2000::/3) and block special-purpose ranges within it
    if (s[0] & 0xe000) != 0x2000 {
        return true;
    }
    (s[0] == 0x2001
        && (s[1] == 0
            || (s[1] == 0x0002 && s[2] == 0)
            || (s[1] & 0xfff0) == 0x0010
            || (s[1] & 0xfff0) == 0x0020
            || s[1] == 0x0db8))
        || s[0] == 0x2002
        || (s[0] == 0x3fff && (s[1] & 0xf000) == 0)
}

fn is_private_ip(ip: std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => is_private_ipv4(v4),
        std::net::IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_or_local_host(host: &str) -> bool {
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
    unbracketed
        .parse::<std::net::IpAddr>()
        .is_ok_and(is_private_ip)
}

fn filter_public_addrs(
    host: &str,
    addrs: impl IntoIterator<Item = std::net::SocketAddr>,
) -> std::result::Result<Vec<std::net::SocketAddr>, std::io::Error> {
    let resolved: Vec<std::net::SocketAddr> = addrs.into_iter().collect();
    if resolved.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("DNS resolution returned no addresses for '{}'", host),
        ));
    }
    if let Some(private) = resolved.iter().find(|addr| is_private_ip(addr.ip())) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!(
                "Blocked DNS resolution of '{}' to private or local IP {} (SSRF protection)",
                host,
                private.ip()
            ),
        ));
    }
    Ok(resolved)
}

/// DNS resolver that enforces SSRF IP validation on resolved addresses to block DNS rebinding.
pub(crate) struct PublicDnsResolver;

impl reqwest::dns::Resolve for PublicDnsResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str();
            if is_private_or_local_host(host) {
                return Err(Box::new(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!(
                        "Blocked request to private or local address '{}' (SSRF protection)",
                        host
                    ),
                ))
                    as Box<dyn std::error::Error + Send + Sync>);
            }
            let addrs = tokio::net::lookup_host((host, 0)).await?;
            let public = filter_public_addrs(host, addrs)?;
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
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

/// Validates that `url_str` uses HTTP or HTTPS, has no control chars/userinfo, and targets public hosts.
pub fn validate_public_http_url(url_str: &str) -> Result<()> {
    if url_str.chars().any(|c| c.is_control()) {
        anyhow::bail!("Invalid URL: control characters are not permitted");
    }
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
        headers.insert(
            reqwest::header::USER_AGENT,
            HeaderValue::from_static(DEFAULT_USER_AGENT),
        );
        headers.insert(
            reqwest::header::ACCEPT_LANGUAGE,
            HeaderValue::from_static("en-US,en;q=0.9"),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20))
            .dns_resolver(std::sync::Arc::new(PublicDnsResolver))
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

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Fetches and normalizes text from `url` with a strict 10 MB response cap.
    pub async fn scrape(&self, url: &str) -> Result<ScrapedPage> {
        validate_public_http_url(url)?;
        let req = self.client.get(url).header(
            reqwest::header::ACCEPT,
            "text/html,application/xhtml+xml,application/xml;q=0.9,*/*;q=0.8",
        );
        let response = send_checked(req, &format!("URL: {url}")).await?;
        let final_url = response.url().to_string();
        let html = read_response_text_limited(response, MAX_SCRAPE_BYTES).await?;
        self.normalize(&final_url, &html)
    }

    /// Normalizes raw HTML into clean Markdown and counts words.
    pub fn normalize(&self, url: &str, raw_html: &str) -> Result<ScrapedPage> {
        let is_wikipedia = reqwest::Url::parse(url)
            .ok()
            .and_then(|u| {
                u.host_str()
                    .map(|h| h == "wikipedia.org" || h.ends_with(".wikipedia.org"))
            })
            .unwrap_or(false);
        let pre_cleaned = if is_wikipedia {
            truncate_wikipedia_html_before_references(raw_html)
        } else {
            Cow::Borrowed(raw_html)
        };

        let doc_url = crate::system::text_util::is_http_url(url).then_some(url);
        let cfg = dom_smoothie::Config {
            max_elements_to_parse: 50_000,
            ..Default::default()
        };
        let (article_title, extracted_html) =
            match dom_smoothie::Readability::new(pre_cleaned.as_ref(), doc_url, Some(cfg))
                .and_then(|mut r| r.parse())
            {
                Ok(article) => {
                    let t = article.title.trim();
                    (
                        (!t.is_empty()).then(|| t.to_string()),
                        Cow::Owned(article.content.to_string()),
                    )
                }
                Err(_) => (None, Cow::Borrowed(pre_cleaned.as_ref())),
            };

        let title = article_title.or_else(|| extract_title_from_html(pre_cleaned.as_ref()));
        let rendered_markdown = html_to_markdown(&extracted_html);
        let (cleaned_content, word_count) = clean_text(&rendered_markdown, true, 2);

        Ok(ScrapedPage {
            url: url.to_string(),
            title,
            content: cleaned_content,
            word_count,
        })
    }
}

pub(super) fn collapse_whitespace(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_whitespace() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

pub(super) fn extract_title_from_html(html: &str) -> Option<String> {
    let clean = strip_boilerplate_html_tags(html, false);
    let mut scanner = XmlScanner::new(clean.as_ref());
    let mut title_fallback: Option<String> = None;
    let mut h1_fallback: Option<String> = None;

    while let Some(XmlEvent::Start {
        name,
        attrs,
        is_empty,
    }) = scanner.next_event()
    {
        let local = xml_local_name(name);
        if local.eq_ignore_ascii_case("meta") {
            if crate::system::text_util::xml_attr(attrs, "property")
                .is_some_and(|p| p.trim().eq_ignore_ascii_case("og:title"))
                && let Some(content) = crate::system::text_util::xml_attr(attrs, "content")
            {
                let trimmed = content.trim();
                if !trimmed.is_empty() {
                    return Some(
                        crate::system::text_util::decode_html_entities(trimmed).into_owned(),
                    );
                }
            }
        } else if !is_empty && local.eq_ignore_ascii_case("title") && title_fallback.is_none() {
            let text = collapse_whitespace(&scanner.read_text(name));
            if !text.is_empty() {
                title_fallback = Some(text);
            }
        } else if !is_empty && local.eq_ignore_ascii_case("h1") && h1_fallback.is_none() {
            let text = collapse_whitespace(&scanner.read_text(name));
            if !text.is_empty() {
                h1_fallback = Some(text);
            }
        }
    }

    title_fallback.or(h1_fallback)
}

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

fn is_citation_or_footnote(anchor: &str) -> bool {
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
    *i = next_idx;
    if out.ends_with(' ')
        && let Some(&next_b) = bytes.get(next_idx)
    {
        if next_b == b' ' {
            *i = next_idx + 1;
        } else if matches!(next_b, b'.' | b',' | b';' | b':' | b')' | b'!' | b'?') {
            out.pop();
        }
    }
}

fn clean_markdown_line(line: &str) -> Cow<'_, str> {
    if !line.contains('[') {
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

fn is_reference_heading(line: &str) -> bool {
    let mut s = line.trim();
    if s.is_empty() {
        return false;
    }
    let has_hash = s.starts_with('#');
    let has_wiki_eq = s.len() >= 2 && s.starts_with('=') && s.ends_with('=');
    s = s.trim_start_matches('#').trim().trim_matches('=').trim();

    let mut has_edit = false;
    if let Some(idx) = s.rfind('[') {
        let suffix = s[idx..].trim();
        if suffix.eq_ignore_ascii_case("[edit]")
            || suffix.eq_ignore_ascii_case("[edit source]")
            || suffix.eq_ignore_ascii_case("[ edit ]")
        {
            s = s[..idx].trim();
            has_edit = true;
        }
    } else if let Some(idx) = s.rfind('(')
        && s[idx..].trim().eq_ignore_ascii_case("(edit)")
    {
        s = s[..idx].trim();
        has_edit = true;
    }

    if !(has_hash || has_wiki_eq || has_edit) {
        return false;
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

/// Strips markdown links/citations, optionally drops trailing reference headings, and counts words.
pub fn clean_text(text: &str, strip_references: bool, max_empty_lines: usize) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut word_count = 0;
    let mut empty_line_count = 0;
    let mut in_code_block = false;

    for raw_line in text.lines() {
        let trimmed = raw_line.trim();
        let is_fence = trimmed.starts_with("```") || trimmed.starts_with("~~~");
        if is_fence {
            in_code_block = !in_code_block;
        }

        if !in_code_block && strip_references && is_reference_heading(trimmed) {
            break;
        }

        let cleaned_line = if is_fence {
            Cow::Borrowed(trimmed)
        } else if in_code_block {
            Cow::Borrowed(raw_line.trim_end())
        } else {
            match clean_markdown_line(trimmed) {
                Cow::Borrowed(b) => Cow::Borrowed(b.trim()),
                Cow::Owned(o) => Cow::Owned(o.trim().to_string()),
            }
        };
        let line = cleaned_line.as_ref();

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
    fn test_scraper_normalize_web_and_local_html() {
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
                    <p>This is the first paragraph of the core article text with meaningful content [1].</p>
                    <p>This is the second paragraph providing in-depth analysis on the topic.</p>
                    <h2>References</h2>
                    <p>Citation 1 that should be stripped.</p>
                </article>
                <footer><p>© 2026 Company Inc. All rights reserved.</p></footer>
            </body>
            </html>
        "#;

        for source in ["https://example.com/article", "/tmp/local.html", "stdin"] {
            let page = scraper.normalize(source, html).unwrap();
            assert_eq!(page.title.as_deref(), Some("Extracted Article Title"));
            assert!(
                page.content
                    .contains("first paragraph of the core article text")
            );
            assert!(!page.content.contains("Ad sidebar with lots of links"));
            assert!(!page.content.contains("[1]"));
            assert!(!page.content.contains("Citation 1"));
            assert!(page.word_count > 10);
        }

        // Fallback HTML (too short for Readability) still strips scripts/styles/head
        let local_small = scraper
            .normalize(
                "/tmp/small.html",
                "<html><head><script>window.evil=1;</script><title>Small</title></head><body><svg><title>Icon</title></svg><p>Body text</p></body></html>",
            )
            .unwrap();
        assert_eq!(local_small.title.as_deref(), Some("Small"));
        assert!(!local_small.content.contains("evil"));
        assert!(!local_small.content.contains("Icon"));
        assert!(local_small.content.contains("Body text"));
    }

    #[test]
    fn test_html_to_markdown_semantics_tables_and_boilerplate() {
        let html = r#"<html><head><title>T</title><style>body{}</style><script>var a=1<2;</script></head><body><!-- <script> --><h2>Overview</h2><pre/><ul><li>Item 1</li><li>Item 2</li></ul><pre><code>let x = 1;</code></pre><table><tr><th>ColA</th><th>ColB</th></tr><tr><td>Val1</td><td>Val2</td></tr></table><svg width="24"/><svg><path/></svg><noscript>Enable JS</noscript></body></html>"#;
        let md = html_to_markdown(html);
        assert!(md.contains("## Overview"));
        assert!(md.contains("* Item 1"));
        assert!(md.contains("ColA | ColB"));
        assert!(md.contains("Val1 | Val2"));
        assert!(!md.contains("var a=1"));
        assert!(!md.contains("Enable JS"));
        assert!(md.contains("```\nlet x = 1;\n```"));
        assert!(!md.contains("```\n`"));

        let unclosed = format!("{}<p>ok</p>", "<script>".repeat(5_000));
        assert!(html_to_markdown(&unclosed).contains("ok"));
    }

    #[test]
    fn test_truncate_wikipedia_html_before_references() {
        let padding = "A".repeat(1200);
        let html = format!(
            "<html><body><p>Body: {padding}</p><h2 id=\"References\">Refs</h2><div class=\"reflist\">Citation 1</div></body></html>"
        );
        let truncated = truncate_wikipedia_html_before_references(&html);
        assert!(truncated.contains("Body:"));
        assert!(!truncated.contains("Citation 1"));
        assert!(truncated.ends_with("</div></body></html>"));
    }

    #[test]
    fn test_clean_markdown_line_and_clean_text_invariants() {
        assert_eq!(
            clean_markdown_line(
                "Learn [Rust](https://en.wikipedia.org/wiki/Rust_(programming_language))[12], check `arr[0]` & alerts!"
            ),
            "Learn Rust, check `arr[0]` & alerts!"
        );

        let md = "Intro.\n\nNotes\nBody after bare Notes line!\n\n```rust\nfn main() {\n    let arr = [1, 2];\n}\n```\n\n## References [edit]\n\n1. Citation link";
        let (cleaned, _) = clean_text(md, true, 2);
        assert!(cleaned.contains("Body after bare Notes line!"));
        assert!(cleaned.contains("\n    let arr = [1, 2];\n"));
        assert!(!cleaned.contains("Citation link"));
        assert!(is_reference_heading("== Further reading =="));
        assert!(!is_reference_heading("Notes"));
        assert!(!is_reference_heading("## History of Rust"));
    }

    #[test]
    fn test_ssrf_validation_blocks_private_and_local_targets() {
        for blocked in [
            "http://localhost/",
            "http://server.local/",
            "http://service.internal/",
            "http://127.0.0.1/",
            "http://10.0.0.1/status",
            "http://172.16.0.1/",
            "http://192.168.1.1/",
            "http://169.254.169.254/latest/meta-data",
            "http://198.18.0.1/",
            "http://192.88.99.1/",
            "http://0.0.0.0/",
            "http://[::1]/",
            "http://[fe80::1]/",
            "http://[fe80::1%25eth0]/",
            "http://[fec0::1]/",
            "http://[::ffff:127.0.0.1]/",
            "http://[::ffff:0:127.0.0.1]/",
            "http://[64:ff9b::7f00:1]/",
            "http://[2002:7f00:1::]/",
            "http://[2001::1]/",
            "http://[2001:db8::1]/",
            "http://[3fff::1]/",
            "http://[5f00::1]/",
            "http://[ff02::1]/",
            "http://user:pass@example.com/",
            "https://example.com/\nHost: evil",
            "ftp://example.com/file",
            "file:///etc/passwd",
        ] {
            assert!(
                validate_public_http_url(blocked).is_err(),
                "expected blocked: {blocked}"
            );
        }

        for allowed in [
            "https://example.com",
            "http://93.184.216.34/",
            "https://[2606:4700:4700::1111]/",
        ] {
            assert!(
                validate_public_http_url(allowed).is_ok(),
                "expected allowed: {allowed}"
            );
        }

        let public_addr: std::net::SocketAddr = "93.184.216.34:443".parse().unwrap();
        let private_addr: std::net::SocketAddr = "127.0.0.1:443".parse().unwrap();
        assert!(filter_public_addrs("example.com", [public_addr]).is_ok());
        assert!(filter_public_addrs("rebind.example", [public_addr, private_addr]).is_err());
        assert!(filter_public_addrs("empty.example", []).is_err());
    }
}
