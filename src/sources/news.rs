use crate::cli::NewsTimeframe;
use crate::scraper::{ScrapedPage, WebScraper};
use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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

    /// Returns whether the provided topic is a direct HTTP/HTTPS feed URL.
    pub fn is_direct_feed(topic: &str) -> bool {
        crate::system::text_util::is_http_url(topic.trim())
    }

    /// Returns the canonical search URL for a topic or direct feed URL.
    pub fn search_url_for_topic(topic: &str) -> String {
        let trimmed = topic.trim();
        if Self::is_direct_feed(trimmed) {
            trimmed.to_string()
        } else {
            format!(
                "https://news.google.com/search?q={}",
                crate::system::text_util::url_encode(trimmed)
            )
        }
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
                crate::system::text_util::url_encode(&query)
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

        // 2. Fetch intermediate Google News article page to extract ephemeral signature & timestamp.
        // Must use /rss/articles/ because /articles/ triggers Google bot protection (HTTP 429 CAPTCHA).
        let page_url = format!("https://news.google.com/rss/articles/{}", base64_id);
        let resp = self
            .client
            .get(&page_url)
            .timeout(Duration::from_secs(4))
            .send()
            .await
            .ok()?;
        let html = crate::scraper::read_response_text_limited(resp, 64 * 1024)
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
        let form_body = format!(
            "f.req={}",
            crate::system::text_util::url_encode(&envelope.to_string())
        );

        let rpc_resp = self
            .client
            .post("https://news.google.com/_/DotsSplashUi/data/batchexecute")
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded;charset=UTF-8",
            )
            .body(form_body)
            .timeout(Duration::from_secs(4))
            .send()
            .await
            .ok()?;

        let rpc_text = crate::scraper::read_response_text_limited(rpc_resp, 128 * 1024)
            .await
            .ok()?;
        parse_batchexecute_destination_url(&rpc_text)
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
            // 1. Direct cache check for feed link with short news TTL (default: 2h)
            if let Some(c) = cache
                && let Some(page) = c.get_news_page(&item.google_link)
                && page.word_count > 60
                && !is_google_news_link(&page.url)
            {
                sources.push(NewsDigestSource {
                    title: page.title.unwrap_or(item.title),
                    source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                    url: if is_google_news_link(&item.google_link) {
                        page.url
                    } else {
                        item.google_link
                    },
                    pub_date: item.pub_date,
                    content: page.content,
                    word_count: page.word_count,
                });
                if sources.len() >= limit {
                    return Ok(sources);
                }
                continue;
            }

            // 2. Offline protobuf decode and direct URL cache check with short news TTL
            if let Some(direct) = decode_google_news_url(&item.google_link)
                && let Some(c) = cache
                && let Some(page) = c.get_news_page(&direct)
                && page.word_count > 60
                && !is_google_news_link(&page.url)
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

                // If feed already provides a rich multi-paragraph body, skip network scrape
                if let Some(snip) = &item.snippet
                    && snip.split_whitespace().count() >= 150
                {
                    let wc = snip.split_whitespace().count();
                    return Some((
                        item.google_link.clone(),
                        NewsDigestSource {
                            title: item.title,
                            source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                            url: item.google_link,
                            pub_date: item.pub_date,
                            content: snip.clone(),
                            word_count: wc,
                        },
                    ));
                }

                let is_gnews = is_google_news_link(&item.google_link);
                let resolved_url = client.resolve_article_url(&item.google_link).await;

                let direct_url = match resolved_url {
                    Some(u) => u,
                    None if !is_gnews => item.google_link.clone(),
                    None => {
                        // Could not resolve Google News redirect URL to real publisher.
                        // Fall back to feed snippet if substantial; never scrape news.google.com!
                        if let Some(snip) = &item.snippet
                            && snip.split_whitespace().count() >= 20
                        {
                            let wc = snip.split_whitespace().count();
                            return Some((
                                item.google_link.clone(),
                                NewsDigestSource {
                                    title: item.title,
                                    source_name: item
                                        .source_name
                                        .unwrap_or_else(|| "Source".to_string()),
                                    url: item.google_link,
                                    pub_date: item.pub_date,
                                    content: snip.clone(),
                                    word_count: wc,
                                },
                            ));
                        }
                        return None;
                    }
                };

                // Safety guard: never attempt to scrape Google News redirect/tracking shells
                if is_google_news_link(&direct_url) {
                    return None;
                }

                // Enforce tight 5s per-article scrape timeout to prevent slow/hanging sites from blocking the digest
                let scraped: Result<ScrapedPage> = match tokio::time::timeout(
                    Duration::from_secs(5),
                    scraper_cloned.scrape(&direct_url),
                )
                .await
                {
                    Ok(res) => res,
                    Err(_) => Err(anyhow::anyhow!("Scrape timed out after 5s")),
                };

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
                    if !is_google_news_link(&source.url) {
                        c.set_page(&source.url, &page_record);
                        if source.url != google_link {
                            c.set_page(&google_link, &page_record);
                        }
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

use crate::system::text_util::decode_html_entities;
#[cfg(test)]
use crate::system::text_util::find_ascii_case_insensitive;

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

fn day_of_week(y: i32, m: i32, d: i32) -> &'static str {
    static T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let adj_y = if m < 3 { y - 1 } else { y };
    let day = (adj_y + adj_y / 4 - adj_y / 100 + adj_y / 400 + T[(m - 1) as usize] + d) % 7;
    match day {
        0 => "Sun",
        1 => "Mon",
        2 => "Tue",
        3 => "Wed",
        4 => "Thu",
        5 => "Fri",
        6 => "Sat",
        _ => "Sun",
    }
}

fn format_iso8601_to_rfc2822(s: &str) -> Option<String> {
    let s = s.trim();
    if s.len() < 19 {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: usize = s[5..7].parse().ok()?;
    let day: i32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let time_part = &s[11..19];
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let month_name = MONTHS[month - 1];
    let dow = day_of_week(year, month as i32, day);

    Some(format!(
        "{}, {:02} {} {} {} +0000",
        dow, day, month_name, year, time_part
    ))
}

fn normalize_feed_date(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some(formatted) = format_iso8601_to_rfc2822(trimmed) {
        return Some(formatted);
    }
    if let Some(stripped) = trimmed.strip_suffix(" GMT") {
        return Some(format!("{} +0000", stripped));
    }
    if let Some(stripped) = trimmed.strip_suffix(" UTC") {
        return Some(format!("{} +0000", stripped));
    }
    Some(trimmed.to_string())
}

/// Parses RSS 0.9/1.0/2.0 or Atom feed items into a list of [`NewsArticleItem`].
pub fn parse_rss_items(xml: &str, limit: usize) -> Vec<NewsArticleItem> {
    if limit == 0 {
        return Vec::new();
    }

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut items = Vec::new();
    let mut feed_source: Option<String> = None;

    let mut in_entry = false;
    let mut cur_title = String::new();
    let mut cur_link = String::new();
    let mut cur_id = String::new();
    let mut cur_pub_date = None;
    let mut cur_source = None;
    let mut cur_author = None;
    let mut cur_snippet: Option<String> = None;

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let name = e.local_name().into_inner().to_ascii_lowercase();

                if name == "item" || name == "entry" {
                    in_entry = true;
                    cur_title.clear();
                    cur_link.clear();
                    cur_id.clear();
                    cur_pub_date = None;
                    cur_source = None;
                    cur_author = None;
                    cur_snippet = None;
                } else if in_entry {
                    match name.as_str() {
                        "title" => {
                            if let Ok(text) = reader.read_text(e.name())
                                && cur_title.is_empty()
                            {
                                cur_title = clean_xml_text(text.as_ref());
                            }
                        }
                        "link" => {
                            for attr in e.attributes().flatten() {
                                if attr
                                    .key
                                    .local_name()
                                    .into_inner()
                                    .eq_ignore_ascii_case("href")
                                {
                                    let trimmed = attr.value.trim();
                                    if crate::system::text_util::is_http_url(trimmed) {
                                        cur_link = trimmed.to_string();
                                    }
                                }
                            }
                            if let Ok(text) = reader.read_text(e.name()) {
                                let trimmed = text.trim();
                                if cur_link.is_empty()
                                    && crate::system::text_util::is_http_url(trimmed)
                                {
                                    cur_link = trimmed.to_string();
                                }
                            }
                        }
                        "guid" | "id" => {
                            if let Ok(text) = reader.read_text(e.name())
                                && cur_id.is_empty()
                            {
                                cur_id = clean_xml_text(text.as_ref());
                            }
                        }
                        "pubdate" | "published" | "updated" => {
                            if let Ok(text) = reader.read_text(e.name())
                                && cur_pub_date.is_none()
                            {
                                cur_pub_date = normalize_feed_date(text.as_ref());
                            }
                        }
                        "source" => {
                            if let Ok(text) = reader.read_text(e.name())
                                && cur_source.is_none()
                            {
                                let cleaned = clean_xml_text(text.as_ref());
                                if !cleaned.is_empty() {
                                    cur_source = Some(cleaned);
                                }
                            }
                        }
                        "author" | "name" => {
                            if let Ok(text) = reader.read_text(e.name())
                                && cur_author.is_none()
                            {
                                let inner = if let Some(start) = text.find("<name>") {
                                    if let Some(end) = text[start + 6..].find("</name>") {
                                        &text[start + 6..start + 6 + end]
                                    } else {
                                        text.as_ref()
                                    }
                                } else {
                                    text.as_ref()
                                };
                                let cleaned = clean_xml_text(inner);
                                if !cleaned.is_empty() {
                                    cur_author = Some(cleaned);
                                }
                            }
                        }
                        "description" | "summary" | "content" | "encoded" => {
                            if let Ok(text) = reader.read_text(e.name()) {
                                let cleaned = clean_xml_text(text.as_ref());
                                if !cleaned.is_empty() {
                                    if let Some(existing) = &cur_snippet {
                                        if cleaned.len() > existing.len() {
                                            cur_snippet = Some(cleaned);
                                        }
                                    } else {
                                        cur_snippet = Some(cleaned);
                                    }
                                }
                            }
                        }
                        _ => {}
                    }
                } else if name == "title"
                    && feed_source.is_none()
                    && let Ok(text) = reader.read_text(e.name())
                {
                    feed_source = Some(clean_xml_text(text.as_ref()));
                }
            }
            Ok(Event::Empty(ref e)) => {
                let name = e.local_name().into_inner().to_ascii_lowercase();
                if in_entry && name == "link" && cur_link.is_empty() {
                    for attr in e.attributes().flatten() {
                        if attr
                            .key
                            .local_name()
                            .into_inner()
                            .eq_ignore_ascii_case("href")
                        {
                            let trimmed = attr.value.trim();
                            if crate::system::text_util::is_http_url(trimmed) {
                                cur_link = trimmed.to_string();
                            }
                        }
                    }
                }
            }
            Ok(Event::End(ref e)) => {
                let name = e.local_name().into_inner().to_ascii_lowercase();
                if name == "item" || name == "entry" {
                    in_entry = false;
                    let link = if !cur_link.is_empty() {
                        cur_link.clone()
                    } else if crate::system::text_util::is_http_url(cur_id.trim()) {
                        cur_id.trim().to_string()
                    } else {
                        String::new()
                    };

                    let source_name = cur_source
                        .take()
                        .or(cur_author.take())
                        .or_else(|| {
                            cur_title
                                .rsplit_once(" - ")
                                .map(|(_, s)| s.trim().to_string())
                        })
                        .or_else(|| feed_source.clone());

                    if !cur_title.is_empty() && !link.is_empty() {
                        items.push(NewsArticleItem {
                            title: cur_title.clone(),
                            google_link: link,
                            source_name,
                            pub_date: cur_pub_date.take(),
                            snippet: cur_snippet.take(),
                        });
                        if items.len() >= limit {
                            break;
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }

    items
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
                    if let Ok(s) = std::str::from_utf8(slice)
                        && !s
                            .chars()
                            .any(|c| c.is_control() || c == ' ' || c == '"' || c == '<' || c == '>')
                        && crate::system::text_util::is_http_url(s)
                    {
                        return Some(s.to_string());
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
            if let Ok(s) = std::str::from_utf8(slice)
                && crate::system::text_util::is_http_url(s)
            {
                return Some(s.to_string());
            }
        }
    }
    None
}

/// Returns whether the provided URL points to Google News redirect/syndication endpoints.
pub fn is_google_news_link(url: &str) -> bool {
    let clean = url.trim();
    clean.starts_with("https://news.google.com/") || clean.starts_with("http://news.google.com/")
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
    let clean = text.trim();
    let json_text = clean.strip_prefix(")]}'").unwrap_or(clean).trim();

    // The response may be chunk-delimited by double newlines
    let candidate = json_text
        .split("\n\n")
        .find(|chunk| chunk.trim().starts_with('['))
        .unwrap_or(json_text);

    let parsed: serde_json::Value = serde_json::from_str(candidate.trim()).ok()?;

    // Search for the "Fbv4je" envelope
    if let Some(arr) = parsed.as_array() {
        for entry in arr {
            if let Some(entry_arr) = entry.as_array()
                && entry_arr.get(1).and_then(|v| v.as_str()) == Some("Fbv4je")
                && let Some(payload_str) = entry_arr.get(2).and_then(|v| v.as_str())
                && let Ok(inner) = serde_json::from_str::<serde_json::Value>(payload_str)
                && let Some(dest) = inner.pointer("/1").and_then(|v| v.as_str())
            {
                let trimmed = dest.trim();
                if crate::system::text_util::is_http_url(trimmed)
                    && !trimmed.chars().any(|c| c.is_control())
                {
                    return Some(trimmed.to_string());
                }
            }
        }
    }

    // Fallback to legacy fixed pointer /0/2 -> /1
    if let Some(json_str) = parsed.pointer("/0/2").and_then(|v| v.as_str())
        && let Ok(inner) = serde_json::from_str::<serde_json::Value>(json_str)
        && let Some(dest) = inner.pointer("/1").and_then(|v| v.as_str())
    {
        let trimmed = dest.trim();
        if crate::system::text_util::is_http_url(trimmed)
            && !trimmed.chars().any(|c| c.is_control())
        {
            return Some(trimmed.to_string());
        }
    }

    None
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
    fn test_is_google_news_link() {
        assert!(is_google_news_link(
            "https://news.google.com/rss/articles/CBMi123"
        ));
        assert!(is_google_news_link(
            "http://news.google.com/articles/CBMi123"
        ));
        assert!(!is_google_news_link(
            "https://www.infoworld.com/article/123"
        ));
        assert!(!is_google_news_link("https://example.com/feed"));
    }

    #[test]
    fn test_parse_batchexecute_destination_url_live_format() {
        let live_rpc = ")]}'\n\n[[\"wrb.fr\",\"Fbv4je\",\"[\\\"garturlres\\\",\\\"https://www.infoworld.com/article/4217735/rustup-1-29-1-brings-concurrency-improvements.html\\\",1]\",null,null,null,\"generic\"],[\"di\",16],[\"af.httprm\",16,\"-7664160711534798251\",10]]";
        assert_eq!(
            parse_batchexecute_destination_url(live_rpc),
            Some("https://www.infoworld.com/article/4217735/rustup-1-29-1-brings-concurrency-improvements.html".to_string())
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
