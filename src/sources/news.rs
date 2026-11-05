use crate::cli::NewsTimeframe;
use crate::scraper::{ScrapedPage, WebScraper};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Maximum allowable limit of news articles to process in a single digest run.
pub const MAX_NEWS_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsArticleItem {
    pub title: String,
    pub google_link: String,
    pub source_name: Option<String>,
    pub pub_date: Option<String>,
    #[serde(default)]
    pub snippet: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewsDigestSource {
    pub title: String,
    pub source_name: String,
    pub url: String,
    pub pub_date: Option<String>,
    pub content: String,
    pub word_count: usize,
}

#[derive(Clone)]
pub struct GoogleNewsClient {
    client: reqwest::Client,
}

impl GoogleNewsClient {
    pub fn with_client(client: reqwest::Client) -> Self {
        Self { client }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// Queries Google News RSS for a topic/timeframe, or fetches a direct RSS/Atom/JSON feed URL.
    pub async fn fetch_feed(
        &self,
        topic: &str,
        timeframe: NewsTimeframe,
        limit: usize,
    ) -> Result<Vec<NewsArticleItem>> {
        let trimmed = topic.trim();
        let endpoint = if crate::system::text_util::is_http_url(trimmed) {
            trimmed.to_string()
        } else {
            let suffix = timeframe.as_google_query_suffix();
            let query = if suffix.is_empty() || trimmed.contains("when:") {
                trimmed.to_string()
            } else {
                format!("{} {}", trimmed, suffix)
            };

            format!(
                "https://news.google.com/rss/search?q={}&hl=en-US&gl=US&ceid=US:en",
                urlencoding::encode(&query)
            )
        };

        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .with_context(|| format!("Failed to fetch syndication feed from {}", endpoint))?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Syndication feed returned HTTP status {}: {}",
                resp.status(),
                endpoint
            );
        }

        let xml =
            crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES)
                .await?;
        let capped_limit = limit.clamp(1, MAX_NEWS_LIMIT.saturating_mul(2).saturating_add(2));
        let items = parse_rss_items(&xml, capped_limit);

        Ok(items)
    }

    /// Resolves an opaque Google News `/rss/articles/CBMi...` redirect link to the real publisher destination URL.
    pub async fn resolve_article_url(&self, google_url: &str) -> Option<String> {
        // 1. Fast-path: attempt zero-network offline protobuf decoding for CBMi IDs
        if let Some(direct) = decode_google_news_url(google_url) {
            return Some(direct);
        }

        let base64_id = extract_article_id(google_url)?;

        // 2. Fetch intermediate Google News article page to extract ephemeral signature & timestamp
        let page_url = format!("https://news.google.com/articles/{}", base64_id);
        let resp = self.client.get(&page_url).send().await.ok()?;
        let html =
            crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES)
                .await
                .ok()?;

        let sig = extract_attr_value(&html, "data-n-a-sg=")?;
        if sig.is_empty()
            || sig.len() > 512
            || sig.chars().any(|c| c.is_control() || c == '"' || c == '\\')
        {
            return None;
        }
        let ts_str = extract_attr_value(&html, "data-n-a-ts=")?;
        let ts: i64 = ts_str.parse().ok()?;

        // 2. Submit batchexecute RPC request to resolve destination URL safely via structured JSON
        let payload_json = serde_json::json!([
            "garturlreq",
            [
                [
                    "X",
                    "X",
                    ["X", "X"],
                    null,
                    null,
                    1,
                    1,
                    "US:en",
                    null,
                    1,
                    null,
                    null,
                    null,
                    null,
                    null,
                    0,
                    1
                ],
                "X",
                "X",
                1,
                [1, 1, 1],
                1,
                1,
                null,
                0,
                0,
                null,
                0
            ],
            base64_id,
            ts,
            sig
        ]);
        let payload_str = payload_json.to_string();
        let envelope = serde_json::json!([[["Fbv4je", payload_str, null, "generic"]]]);
        let form_body = format!("f.req={}", urlencoding::encode(&envelope.to_string()));

        let rpc_resp = self
            .client
            .post("https://news.google.com/_/DotsSplashUi/data/batchexecute")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .body(form_body)
            .send()
            .await
            .ok()?;

        let rpc_text =
            crate::scraper::read_response_text_limited(rpc_resp, crate::scraper::MAX_SCRAPE_BYTES)
                .await
                .ok()?;
        parse_batchexecute_destination_url(&rpc_text)
    }

    /// Fetches and normalizes top news articles concurrently with bounded concurrency.
    pub async fn collect_news_articles(
        &self,
        topic: &str,
        timeframe: NewsTimeframe,
        limit: usize,
        scraper: &WebScraper,
    ) -> Result<Vec<NewsDigestSource>> {
        self.collect_news_articles_with_cache(topic, timeframe, limit, scraper, None)
            .await
    }

    /// Fetches and normalizes top news articles concurrently, consulting and populating `cache` when provided.
    pub async fn collect_news_articles_with_cache(
        &self,
        topic: &str,
        timeframe: NewsTimeframe,
        limit: usize,
        scraper: &WebScraper,
        cache: Option<&crate::cache::Cache>,
    ) -> Result<Vec<NewsDigestSource>> {
        const MAX_CONCURRENT_NEWS_SCRAPES: usize = 6;

        let limit = limit.clamp(1, MAX_NEWS_LIMIT);
        // Fetch up to 2x limit (or at least limit + 2) candidates to buffer against paywalls/dead links
        let fetch_limit = limit.saturating_mul(2).max(limit.saturating_add(2));
        let feed_items = self.fetch_feed(topic, timeframe, fetch_limit).await?;
        if feed_items.is_empty() {
            let tip = match timeframe {
                NewsTimeframe::Day => {
                    " Try expanding the recency timeframe with `-t week`, `-t month`, or `-t all`, or broadening the topic search terms."
                }
                NewsTimeframe::Week => {
                    " Try expanding the recency timeframe with `-t month` or `-t all`, or broadening the topic search terms."
                }
                NewsTimeframe::Month => {
                    " Try expanding the recency timeframe with `-t all`, or broadening the topic search terms."
                }
                NewsTimeframe::All => {
                    " Try broadening the topic search terms, or verify the syndication feed URL."
                }
            };
            anyhow::bail!(
                "No recent articles found in feed for '{}' ({}){}",
                topic,
                timeframe.display_label(),
                if crate::system::text_util::is_http_url(topic.trim()) {
                    ""
                } else {
                    tip
                }
            );
        }

        let mut sources = Vec::new();
        let mut uncached_items = Vec::new();

        for item in feed_items {
            // 1. Direct cache check for feed link
            if let Some(c) = cache
                && let Some(page) = c.get_page(&item.google_link)
                && page.word_count > 60
            {
                sources.push(NewsDigestSource {
                    title: page.title.unwrap_or(item.title),
                    source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                    url: item.google_link,
                    pub_date: item.pub_date,
                    content: page.content,
                    word_count: page.word_count,
                });
                if sources.len() >= limit {
                    return Ok(sources);
                }
                continue;
            }

            // 2. Offline protobuf decode and direct URL cache check
            if let Some(direct) = decode_google_news_url(&item.google_link) {
                if let Some(c) = cache
                    && let Some(page) = c.get_page(&direct)
                    && page.word_count > 60
                {
                    // Cache under google_link as well for sub-millisecond future hits
                    c.set_page(&item.google_link, &page);
                    sources.push(NewsDigestSource {
                        title: page.title.unwrap_or(item.title),
                        source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                        url: direct,
                        pub_date: item.pub_date,
                        content: page.content,
                        word_count: page.word_count,
                    });
                    if sources.len() >= limit {
                        return Ok(sources);
                    }
                    continue;
                }
            }

            uncached_items.push(item);
        }

        let semaphore =
            std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_NEWS_SCRAPES));
        let mut join_set = tokio::task::JoinSet::new();

        // Resolve destination URLs and scrape articles with bounded concurrency
        for item in uncached_items {
            let client = self.clone();
            let scraper_cloned = scraper.clone();
            let sem = semaphore.clone();
            join_set.spawn(async move {
                let _permit = sem.acquire().await.ok()?;
                let direct_url = match client.resolve_article_url(&item.google_link).await {
                    Some(u) => u,
                    None => item.google_link.clone(),
                };

                let scraped: Result<ScrapedPage> = scraper_cloned.scrape(&direct_url).await;
                let (content, word_count, resolved_title) = if let Ok(page) = scraped
                    && page.word_count > 60
                {
                    (
                        page.content,
                        page.word_count,
                        page.title.unwrap_or(item.title),
                    )
                } else if let Some(snip) = &item.snippet
                    && snip.split_whitespace().count() >= 20
                {
                    // Fall back to syndication feed description/content when scraping hits paywalls/403
                    let wc = snip.split_whitespace().count();
                    (snip.clone(), wc, item.title)
                } else {
                    return None;
                };

                Some((
                    item.google_link,
                    NewsDigestSource {
                        title: resolved_title,
                        source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                        url: direct_url,
                        pub_date: item.pub_date,
                        content,
                        word_count,
                    },
                ))
            });
        }

        while let Some(res) = join_set.join_next().await {
            if let Ok(Some((google_link, source))) = res {
                if let Some(c) = cache {
                    let page_record = ScrapedPage {
                        url: source.url.clone(),
                        title: Some(source.title.clone()),
                        content: source.content.clone(),
                        word_count: source.word_count,
                    };
                    c.set_page(&source.url, &page_record);
                    if source.url != google_link {
                        c.set_page(&google_link, &page_record);
                    }
                }
                sources.push(source);
                if sources.len() >= limit {
                    join_set.abort_all();
                    break;
                }
            }
        }

        if sources.is_empty() {
            anyhow::bail!(
                "Found headlines for '{}', but none of the article pages could be scraped.",
                topic
            );
        }

        Ok(sources)
    }
}

use crate::system::text_util::{decode_html_entities, find_ascii_case_insensitive};

/// Cleans CDATA wrappers and decodes HTML entities from an XML element's inner text.
fn clean_xml_text(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(cdata_start) = trimmed.find("<![CDATA[") {
        let after_start = &trimmed[cdata_start + "<![CDATA[".len()..];
        if let Some(cdata_end) = after_start.find("]]>") {
            return after_start[..cdata_end].trim().to_string();
        }
    }
    decode_html_entities(trimmed).into_owned()
}

/// Extracts `<source>` values from `<item>` elements if present, bounded by `limit`.
fn extract_source_tags(xml: &str, limit: usize) -> Vec<String> {
    if limit == 0 {
        return Vec::new();
    }
    let mut sources = Vec::new();
    let mut cursor = 0;
    while let Some(rel_start) = find_ascii_case_insensitive(&xml[cursor..], "<source") {
        let tag_start = cursor + rel_start;
        if let Some(tag_close) = xml[tag_start..].find('>') {
            let inner_start = tag_start + tag_close + 1;
            if let Some(end_rel) = find_ascii_case_insensitive(&xml[inner_start..], "</source>") {
                let inner = xml[inner_start..inner_start + end_rel].trim();
                if !inner.is_empty() {
                    sources.push(clean_xml_text(inner));
                    if sources.len() >= limit {
                        break;
                    }
                }
                cursor = inner_start + end_rel + "</source>".len();
                continue;
            }
        }
        cursor = tag_start + 7;
    }
    sources
}

/// Parses RSS 0.9/1.0/2.0, Atom, or JSON Feed items into a list of [`NewsArticleItem`].
pub fn parse_rss_items(xml: &str, limit: usize) -> Vec<NewsArticleItem> {
    if limit == 0 {
        return Vec::new();
    }

    let normalized_xml;
    let xml_to_parse = if let Some(rss_pos) = find_ascii_case_insensitive(xml, "<rss") {
        let tag_close = xml[rss_pos..].find('>').map(|i| rss_pos + i);
        let rss_tag_has_version = tag_close.is_some_and(|end| {
            crate::system::text_util::contains_ascii_case_insensitive(
                &xml[rss_pos..end],
                "version=",
            )
        });
        if !rss_tag_has_version {
            normalized_xml = xml.replacen("<rss>", "<rss version=\"2.0\">", 1).replacen(
                "<rss ",
                "<rss version=\"2.0\" ",
                1,
            );
            normalized_xml.as_str()
        } else {
            xml
        }
    } else {
        xml
    };

    let parsed_feed = feed_rs::parser::parse(xml_to_parse.as_bytes()).ok();
    let source_tags = extract_source_tags(xml, limit);

    if let Some(feed) = parsed_feed {
        let mut items = Vec::new();
        let feed_source = feed.title.as_ref().map(|t| t.content.clone());

        for (idx, entry) in feed.entries.into_iter().take(limit).enumerate() {
            let title = entry.title.map(|t| t.content).unwrap_or_default();
            let link = entry
                .links
                .into_iter()
                .map(|l| l.href.trim().to_string())
                .find(|href| crate::system::text_util::is_http_url(href))
                .unwrap_or_else(|| {
                    let id_trimmed = entry.id.trim();
                    if crate::system::text_util::is_http_url(id_trimmed) {
                        id_trimmed.to_string()
                    } else {
                        String::new()
                    }
                });

            let pub_date = entry.published.or(entry.updated).map(|dt| dt.to_rfc2822());

            let source_name = source_tags
                .get(idx)
                .cloned()
                .or(entry.source)
                .or_else(|| entry.authors.first().and_then(|a| a.name.clone()))
                .or_else(|| title.rsplit_once(" - ").map(|(_, s)| s.trim().to_string()))
                .or_else(|| feed_source.clone());

            let snippet = entry
                .summary
                .map(|s| clean_xml_text(&s.content))
                .or_else(|| {
                    entry
                        .content
                        .and_then(|c| c.body.map(|b| clean_xml_text(&b)))
                })
                .filter(|s| !s.is_empty());

            if !title.is_empty() && !link.is_empty() {
                items.push(NewsArticleItem {
                    title,
                    google_link: link,
                    source_name,
                    pub_date,
                    snippet,
                });
            }
        }

        if !items.is_empty() {
            return items;
        }
    }

    Vec::new()
}

/// Attempts zero-network offline decoding of an obfuscated Google News URL (`CBMi...`).
///
/// Google News article IDs starting with `CBMi` are URL-safe base64-encoded protobuf messages
/// that contain the canonical destination URL in cleartext. Decoding them offline eliminates
/// up to two HTTP roundtrips (page fetch + `batchexecute` RPC).
pub fn decode_google_news_url(url: &str) -> Option<String> {
    let id = extract_article_id(url)?;
    decode_cbmi_id(id)
}

/// Decodes an obfuscated Google News article ID starting with `CBMi` into its target URL.
pub fn decode_cbmi_id(id: &str) -> Option<String> {
    if !id.starts_with("CBMi") {
        return None;
    }
    let raw = decode_base64_url_safe(id)?;
    extract_url_from_protobuf(&raw)
}

/// Decodes standard or URL-safe base64 strings into raw bytes without external crate dependencies.
fn decode_base64_url_safe(input: &str) -> Option<Vec<u8>> {
    let clean = input.trim_end_matches('=');
    let mut out = Vec::with_capacity((clean.len() * 3) / 4 + 4);
    let mut buf = 0u32;
    let mut bits = 0;

    for b in clean.bytes() {
        let val = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' | b'+' => 62,
            b'_' | b'/' => 63,
            b'=' => continue,
            _ => return None,
        } as u32;

        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Scans decoded protobuf byte slices for an embedded `http://` or `https://` payload.
fn extract_url_from_protobuf(raw: &[u8]) -> Option<String> {
    const SCHEMES: &[&[u8]] = &[b"https://", b"http://"];
    for scheme in SCHEMES {
        if let Some(pos) = raw.windows(scheme.len()).position(|w| w == *scheme) {
            // First check if a length byte immediately precedes the scheme
            if pos > 0 {
                let len = raw[pos - 1] as usize;
                if len >= scheme.len() && pos + len <= raw.len() {
                    let slice = &raw[pos..pos + len];
                    if let Ok(s) = std::str::from_utf8(slice) {
                        if !s
                            .chars()
                            .any(|c| c.is_control() || c == ' ' || c == '"' || c == '<' || c == '>')
                            && crate::system::text_util::is_http_url(s)
                        {
                            return Some(s.to_string());
                        }
                    }
                }
            }
            // Fallback: scan forward while encountering valid printable ASCII URL characters
            let mut end = pos;
            while end < raw.len() {
                let b = raw[end];
                if (33..=126).contains(&b) && b != b'"' && b != b'\'' && b != b'<' && b != b'>' {
                    end += 1;
                } else {
                    break;
                }
            }
            let slice = &raw[pos..end];
            if let Ok(s) = std::str::from_utf8(slice) {
                if crate::system::text_util::is_http_url(s) {
                    return Some(s.to_string());
                }
            }
        }
    }
    None
}

pub fn extract_article_id(url: &str) -> Option<&str> {
    let clean = url.trim();
    if !clean.starts_with("https://news.google.com/")
        && !clean.starts_with("http://news.google.com/")
    {
        return None;
    }
    if let Some(pos) = clean.find("/articles/") {
        let after = &clean[pos + 10..];
        let id = after.split(['?', '#', '/']).next().unwrap_or(after);
        if !id.is_empty()
            && id.len() <= 2048
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'=')
        {
            return Some(id);
        }
    }
    None
}

pub fn extract_attr_value<'a>(html: &'a str, attr_prefix: &str) -> Option<&'a str> {
    let start_pos = html.find(attr_prefix)?;
    let remainder = &html[start_pos + attr_prefix.len()..];
    let quote = remainder.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let rest = &remainder[1..];
    let end_pos = rest.find(quote)?;
    Some(&rest[..end_pos])
}

pub fn parse_batchexecute_destination_url(text: &str) -> Option<String> {
    let split_chunk = text.split("\n\n").nth(1)?;
    let parsed: serde_json::Value = serde_json::from_str(split_chunk).ok()?;
    let json_str = parsed.pointer("/0/2")?.as_str()?;
    let inner_json: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let dest = inner_json.pointer("/1")?.as_str()?.trim();
    if crate::system::text_util::is_http_url(dest) && !dest.chars().any(|c| c.is_control()) {
        Some(dest.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_rss_items_standard() {
        let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<rss version="2.0">
  <channel>
    <title>Google News</title>
    <item>
      <title>Pens Points: Penguins give updates on Kindel, Rust - PensBurgh</title>
      <link>https://news.google.com/rss/articles/CBMi12345?oc=5</link>
      <pubDate>Fri, 23 Oct 2026 10:00:00 GMT</pubDate>
      <source url="https://www.pensburgh.com">PensBurgh</source>
    </item>
    <item>
      <title>Rust 1.85 Released with Async Closures &amp; Editions</title>
      <link>https://news.google.com/rss/articles/CBMi67890?oc=5</link>
      <pubDate>Thu, 22 Oct 2026 08:30:00 GMT</pubDate>
      <source url="https://blog.rust-lang.org">Rust Blog</source>
    </item>
  </channel>
</rss>"#;

        let items = parse_rss_items(xml, 10);
        assert_eq!(items.len(), 2);

        assert_eq!(
            items[0].title,
            "Pens Points: Penguins give updates on Kindel, Rust - PensBurgh"
        );
        assert_eq!(
            items[0].google_link,
            "https://news.google.com/rss/articles/CBMi12345?oc=5"
        );
        assert_eq!(
            items[0].pub_date.as_deref(),
            Some("Fri, 23 Oct 2026 10:00:00 +0000")
        );
        assert_eq!(items[0].source_name.as_deref(), Some("PensBurgh"));

        assert_eq!(
            items[1].title,
            "Rust 1.85 Released with Async Closures & Editions"
        );
        assert_eq!(
            items[1].google_link,
            "https://news.google.com/rss/articles/CBMi67890?oc=5"
        );
        assert_eq!(items[1].source_name.as_deref(), Some("Rust Blog"));
    }

    #[test]
    fn test_parse_atom_feed() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Feed</title>
  <entry>
    <title>Atom Entry Title</title>
    <link href="https://example.com/atom-1"/>
    <id>urn:uuid:1225c695-cfb8-4ebb-aaaa-80da344efa6a</id>
    <updated>2026-10-23T18:30:02Z</updated>
    <author>
      <name>John Doe</name>
    </author>
  </entry>
</feed>"#;

        let items = parse_rss_items(xml, 5);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Atom Entry Title");
        assert_eq!(items[0].google_link, "https://example.com/atom-1");
        assert_eq!(items[0].source_name.as_deref(), Some("John Doe"));
        assert_eq!(
            items[0].pub_date.as_deref(),
            Some("Fri, 23 Oct 2026 18:30:02 +0000")
        );
    }

    #[test]
    fn test_parse_rss_items_limit() {
        let xml = r#"<rss version="2.0"><channel>
            <item><title>Item 1</title><link>https://example.com/1</link></item>
            <item><title>Item 2</title><link>https://example.com/2</link></item>
            <item><title>Item 3</title><link>https://example.com/3</link></item>
        </channel></rss>"#;

        let items = parse_rss_items(xml, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Item 1");
        assert_eq!(items[1].title, "Item 2");
    }

    #[test]
    fn test_parse_rss_items_cdata_and_entities() {
        let xml = r#"<rss><channel>
            <item>
                <title><![CDATA[Rust &amp; WebAssembly in 2026]]></title>
                <link>https://example.com/cdata-link</link>
                <source url="https://example.com"><![CDATA[Tech Daily]]></source>
            </item>
            <item>
                <title>Special &quot;Quotes&quot; &amp; &apos;Apostrophes&apos;</title>
                <link>https://example.com/quotes</link>
            </item>
        </channel></rss>"#;

        let items = parse_rss_items(xml, 5);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Rust &amp; WebAssembly in 2026");
        assert_eq!(items[0].source_name.as_deref(), Some("Tech Daily"));
        assert_eq!(items[1].title, "Special \"Quotes\" & 'Apostrophes'");
    }

    #[test]
    fn test_parse_rss_items_guid_fallback() {
        let xml = r#"<rss><channel>
            <item>
                <title>No Link Tag</title>
                <guid>https://example.com/guid-link</guid>
            </item>
        </channel></rss>"#;

        let items = parse_rss_items(xml, 5);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].google_link, "https://example.com/guid-link");
    }

    #[test]
    fn test_find_ascii_case_insensitive() {
        assert_eq!(find_ascii_case_insensitive("Hello World", "world"), Some(6));
        assert_eq!(find_ascii_case_insensitive("<ITEM>", "<item>"), Some(0));
        assert_eq!(find_ascii_case_insensitive("Short", "LongerPattern"), None);
        assert_eq!(find_ascii_case_insensitive("abc", ""), Some(0));
    }

    #[test]
    fn test_google_news_url_extraction() {
        let url = "https://news.google.com/rss/articles/CBMiWmh0dHBzOi8vd3d3LnNpdGVwb2ludC5jb20vamF2YXNjcmlwdC1uZXdzLw?oc=5";
        assert_eq!(
            extract_article_id(url),
            Some("CBMiWmh0dHBzOi8vd3d3LnNpdGVwb2ludC5jb20vamF2YXNjcmlwdC1uZXdzLw")
        );

        let html = r#"<div data-n-a-sg="ABC123xyz" data-n-a-ts="1714200000"></div>"#;
        assert_eq!(extract_attr_value(html, "data-n-a-sg="), Some("ABC123xyz"));
        assert_eq!(extract_attr_value(html, "data-n-a-ts="), Some("1714200000"));

        let rpc_text = ")]}'\n\n[[\"wrb.fr\",\"Fbv4je\",\"[\\\"garturlreq\\\",\\\"https://www.sitepoint.com/javascript-news/\\\"]\"]]";
        assert_eq!(
            parse_batchexecute_destination_url(rpc_text),
            Some("https://www.sitepoint.com/javascript-news/".to_string())
        );
    }

    #[test]
    fn test_google_news_url_extraction_security() {
        // Must reject foreign origins
        assert_eq!(
            extract_article_id("https://attacker.com/articles/CBMi12345"),
            None
        );
        assert_eq!(
            extract_article_id("http://evil.news.google.com.attacker.com/articles/CBMi12345"),
            None
        );

        // Must reject non-base64 characters / path traversal
        assert_eq!(
            extract_article_id("https://news.google.com/articles/../../etc/passwd"),
            None
        );
        assert_eq!(
            extract_article_id("https://news.google.com/articles/<script>"),
            None
        );

        // Valid Google News article URL
        assert_eq!(
            extract_article_id("https://news.google.com/articles/CBMi-Valid_Id=123?oc=5"),
            Some("CBMi-Valid_Id=123")
        );

        // Reject dangerous destination schemes from RPC
        let xss_rpc = ")]}'\n\n[[\"wrb.fr\",\"Fbv4je\",\"[\\\"garturlreq\\\",\\\"javascript:alert(1)\\\"]\"]]";
        assert_eq!(parse_batchexecute_destination_url(xss_rpc), None);

        let file_rpc =
            ")]}'\n\n[[\"wrb.fr\",\"Fbv4je\",\"[\\\"garturlreq\\\",\\\"file:///etc/passwd\\\"]\"]]";
        assert_eq!(parse_batchexecute_destination_url(file_rpc), None);

        // Reject non-http links in RSS items
        let bad_rss = r#"<rss version="2.0"><channel>
            <item><title>XSS Link</title><link>javascript:alert(1)</link></item>
            <item><title>File Link</title><link>file:///etc/passwd</link></item>
        </channel></rss>"#;
        assert_eq!(parse_rss_items(bad_rss, 5).len(), 0);
    }

    #[test]
    fn test_decode_google_news_url_offline() {
        let url = "https://news.google.com/rss/articles/CBMiWmh0dHBzOi8vd3d3LnNpdGVwb2ludC5jb20vamF2YXNjcmlwdC1uZXdzLw?oc=5";
        assert_eq!(
            decode_google_news_url(url),
            Some("https://www.sitepoint.com/javascript-news/".to_string())
        );

        // Non-CBMi IDs return None to fall back to RPC
        assert_eq!(
            decode_google_news_url("https://news.google.com/articles/CBIi98765"),
            None
        );
    }

    #[test]
    fn test_parse_rss_items_extracts_snippet() {
        let xml = r#"<rss version="2.0"><channel>
            <item>
                <title>Article with Summary</title>
                <link>https://example.com/article</link>
                <description><![CDATA[This is a descriptive summary of the breaking story.]]></description>
            </item>
        </channel></rss>"#;

        let items = parse_rss_items(xml, 5);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].snippet.as_deref(),
            Some("This is a descriptive summary of the breaking story.")
        );
    }
}
