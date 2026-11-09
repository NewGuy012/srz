use crate::cli::NewsTimeframe;
use crate::scraper::{ScrapedPage, WebScraper};
use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
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

        let target_limit = limit.max(5).clamp(1, MAX_NEWS_LIMIT);
        // Generously buffer candidates to guarantee at least target_limit scrapeable articles
        let fetch_limit = target_limit.saturating_mul(6).clamp(35, MAX_NEWS_LIMIT);
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

        let mut collected: BTreeMap<usize, NewsDigestSource> = BTreeMap::new();
        let mut completed_ranks: HashSet<usize> = HashSet::new();
        let mut uncached_items: Vec<(usize, NewsArticleItem)> = Vec::new();

        for (idx, item) in feed_items.into_iter().enumerate() {
            // 1. Direct cache check for feed link with short news TTL (default: 2h)
            if let Some(c) = cache
                && let Some(page) = c.get_news_page(&item.google_link)
                && page.word_count > 60
                && !is_google_news_link(&page.url)
            {
                collected.insert(
                    idx,
                    NewsDigestSource {
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
                    },
                );
                completed_ranks.insert(idx);
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
                collected.insert(
                    idx,
                    NewsDigestSource {
                        title: page.title.unwrap_or(item.title),
                        source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                        url: direct,
                        pub_date: item.pub_date,
                        content: page.content,
                        word_count: page.word_count,
                    },
                );
                completed_ranks.insert(idx);
                continue;
            }

            uncached_items.push((idx, item));
        }

        let mut min_pending_rank = 0;
        while completed_ranks.contains(&min_pending_rank) {
            min_pending_rank += 1;
        }

        let all_cached_satisfied = if collected.len() >= target_limit {
            let cutoff = *collected.keys().nth(target_limit - 1).unwrap();
            min_pending_rank > cutoff
        } else {
            false
        };

        if !all_cached_satisfied && !uncached_items.is_empty() {
            let semaphore =
                std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_NEWS_SCRAPES));
            let mut join_set = tokio::task::JoinSet::new();

            for (idx, item) in uncached_items {
                let client = self.clone();
                let scraper_cloned = scraper.clone();
                let sem = semaphore.clone();
                join_set.spawn(async move {
                    let Ok(_permit) = sem.acquire().await else {
                        return (idx, item.google_link, None);
                    };

                    // If feed already provides a rich multi-paragraph body, skip network scrape
                    if let Some(snip) = &item.snippet
                        && snip.split_whitespace().count() >= 150
                    {
                        let wc = snip.split_whitespace().count();
                        return (
                            idx,
                            item.google_link.clone(),
                            Some(NewsDigestSource {
                                title: item.title,
                                source_name: item
                                    .source_name
                                    .unwrap_or_else(|| "Source".to_string()),
                                url: item.google_link,
                                pub_date: item.pub_date,
                                content: snip.clone(),
                                word_count: wc,
                            }),
                        );
                    }

                    let is_gnews = is_google_news_link(&item.google_link);
                    let resolved_url = client.resolve_article_url(&item.google_link).await;

                    let direct_url = match resolved_url {
                        Some(u) => u,
                        None if !is_gnews => item.google_link.clone(),
                        None => {
                            if let Some(snip) = &item.snippet
                                && snip.split_whitespace().count() >= 20
                            {
                                let wc = snip.split_whitespace().count();
                                return (
                                    idx,
                                    item.google_link.clone(),
                                    Some(NewsDigestSource {
                                        title: item.title,
                                        source_name: item
                                            .source_name
                                            .unwrap_or_else(|| "Source".to_string()),
                                        url: item.google_link,
                                        pub_date: item.pub_date,
                                        content: snip.clone(),
                                        word_count: wc,
                                    }),
                                );
                            }
                            return (idx, item.google_link, None);
                        }
                    };

                    if is_google_news_link(&direct_url) {
                        return (idx, item.google_link, None);
                    }

                    let scraped: Result<ScrapedPage> = match tokio::time::timeout(
                        Duration::from_secs(4),
                        scraper_cloned.scrape(&direct_url),
                    )
                    .await
                    {
                        Ok(res) => res,
                        Err(_) => Err(anyhow::anyhow!("Scrape timed out after 4s")),
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
                        let wc = snip.split_whitespace().count();
                        (snip.clone(), wc, item.title)
                    } else {
                        return (idx, item.google_link, None);
                    };

                    (
                        idx,
                        item.google_link,
                        Some(NewsDigestSource {
                            title: resolved_title,
                            source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                            url: direct_url,
                            pub_date: item.pub_date,
                            content,
                            word_count,
                        }),
                    )
                });
            }

            while let Some(res) = join_set.join_next().await {
                if let Ok((idx, google_link, maybe_source)) = res {
                    completed_ranks.insert(idx);
                    while completed_ranks.contains(&min_pending_rank) {
                        min_pending_rank += 1;
                    }

                    if let Some(source) = maybe_source {
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
                        collected.insert(idx, source);
                    }

                    if collected.len() >= target_limit {
                        let cutoff = *collected.keys().nth(target_limit - 1).unwrap();
                        if min_pending_rank > cutoff {
                            join_set.abort_all();
                            break;
                        }
                    }
                }
            }
        }

        if collected.is_empty() {
            anyhow::bail!(
                "Found headlines for '{}', but none of the article pages could be scraped.",
                topic
            );
        }

        // Rank and sort collected articles balancing recency (freshness) and relevance (feed position)
        let mut ranked: Vec<(usize, Option<i64>, NewsDigestSource)> = collected
            .into_iter()
            .map(|(rank, src)| {
                let ts = src
                    .pub_date
                    .as_deref()
                    .and_then(parse_feed_date_to_timestamp);
                (rank, ts, src)
            })
            .collect();

        sort_news_by_recency_and_relevance(&mut ranked);

        let final_sources: Vec<NewsDigestSource> = ranked
            .into_iter()
            .take(target_limit)
            .map(|(_, _, src)| src)
            .collect();

        Ok(final_sources)
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

/// Parses RFC 2822 or normalized feed date strings into a UTC Unix epoch timestamp in seconds.
pub fn parse_feed_date_to_timestamp(date_str: &str) -> Option<i64> {
    let trimmed = date_str.trim();
    if trimmed.is_empty() {
        return None;
    }

    let mut parts: Vec<&str> = trimmed.split_whitespace().collect();
    if parts.is_empty() {
        return None;
    }

    // Skip optional day of week (e.g. "Fri," or "Fri")
    if (parts[0].ends_with(',')
        || (parts[0].len() == 3 && parts[0].chars().all(|c| c.is_ascii_alphabetic())))
        && parts.len() >= 5
    {
        parts.remove(0);
    }

    if parts.len() < 4 {
        return None;
    }

    let day: u32 = parts[0].parse().ok()?;
    let month = match parts[1].to_ascii_lowercase().as_str() {
        "jan" => 1,
        "feb" => 2,
        "mar" => 3,
        "apr" => 4,
        "may" => 5,
        "jun" => 6,
        "jul" => 7,
        "aug" => 8,
        "sep" => 9,
        "oct" => 10,
        "nov" => 11,
        "dec" => 12,
        _ => return None,
    };
    let year: i32 = parts[2].parse().ok()?;

    let time_parts: Vec<&str> = parts[3].split(':').collect();
    if time_parts.len() < 2 {
        return None;
    }
    let hour: u32 = time_parts[0].parse().ok()?;
    let min: u32 = time_parts[1].parse().ok()?;
    let sec: u32 = if time_parts.len() >= 3 {
        time_parts[2].parse().unwrap_or(0)
    } else {
        0
    };

    let offset_secs = if parts.len() >= 5 {
        parse_timezone_offset_seconds(parts[4])
    } else {
        0
    };

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 60 {
        return None;
    }

    let days = days_from_civil(year, month, day);
    let total_secs = days
        .checked_mul(86400)?
        .checked_add((hour as i64) * 3600 + (min as i64) * 60 + (sec as i64))?
        .checked_sub(offset_secs)?;

    Some(total_secs)
}

fn parse_timezone_offset_seconds(tz: &str) -> i64 {
    let clean = tz.trim();
    if clean.eq_ignore_ascii_case("gmt") || clean.eq_ignore_ascii_case("utc") || clean == "Z" {
        return 0;
    }
    if (clean.starts_with('+') || clean.starts_with('-')) && clean.len() >= 5 {
        let sign = if clean.starts_with('-') { -1 } else { 1 };
        let hh: i64 = clean[1..3].parse().unwrap_or(0);
        let mm: i64 = clean[3..5].parse().unwrap_or(0);
        return sign * (hh * 3600 + mm * 60);
    }
    0
}

/// Howard Hinnant's civil day count algorithm from 1970-01-01
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = y - if m <= 2 { 1 } else { 0 };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = (y - era * 400) as u32;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era as i64 * 146097 + doe as i64 - 719468
}

/// Ranks and sorts news sources balancing recency (freshness) and relevance (feed position).
pub fn sort_news_by_recency_and_relevance(sources: &mut [(usize, Option<i64>, NewsDigestSource)]) {
    const RECENCY_BUCKET_SECS: i64 = 18 * 3600; // 18-hour window for breaking news parity

    sources.sort_by(|a, b| {
        match (a.1, b.1) {
            (Some(ts_a), Some(ts_b)) => {
                let diff = ts_a - ts_b;
                // If publication dates are substantially different (outside the 18h cycle),
                // the more recent news strictly takes priority.
                if diff.abs() > RECENCY_BUCKET_SECS {
                    ts_b.cmp(&ts_a) // Newest timestamp first
                } else {
                    // Within the same news cycle/day, Google's relevance ranking breaks the tie!
                    a.0.cmp(&b.0)
                }
            }
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (None, None) => a.0.cmp(&b.0),
        }
    });
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

    #[test]
    fn test_parse_feed_date_to_timestamp() {
        // RFC 2822 with GMT
        let ts1 = parse_feed_date_to_timestamp("Thu, 22 Oct 2026 08:30:00 GMT");
        assert!(ts1.is_some());

        // RFC 2822 with +0000
        let ts2 = parse_feed_date_to_timestamp("Fri, 23 Oct 2026 10:00:00 +0000");
        assert!(ts2.is_some());
        assert!(ts2.unwrap() > ts1.unwrap()); // Next day is larger

        // RFC 2822 without weekday
        let ts3 = parse_feed_date_to_timestamp("23 Oct 2026 10:00:00 +0000");
        assert_eq!(ts3, ts2);

        // Negative timezone offset (-0500)
        let ts4 = parse_feed_date_to_timestamp("23 Oct 2026 10:00:00 -0500");
        assert_eq!(ts4.unwrap(), ts2.unwrap() + 5 * 3600);

        // Invalid dates
        assert_eq!(parse_feed_date_to_timestamp("not a date"), None);
        assert_eq!(parse_feed_date_to_timestamp(""), None);
    }

    #[test]
    fn test_sort_news_by_recency_and_relevance() {
        let make_src = |title: &str| NewsDigestSource {
            title: title.to_string(),
            source_name: "Source".to_string(),
            url: format!("https://example.com/{}", title),
            pub_date: None,
            content: "content".to_string(),
            word_count: 100,
        };

        let base_ts = 1_700_000_000i64;

        // Article A: 2h ago, original rank 2 (within 18h window of B & C)
        // Article B: 4h ago, original rank 0
        // Article C: 6h ago, original rank 1
        // Article D: 3 days ago, original rank 3 (outside 18h window)
        // Article E: No date, original rank 4
        let mut items = vec![
            (2, Some(base_ts + 4 * 3600), make_src("A")),  // 2h ago
            (0, Some(base_ts + 2 * 3600), make_src("B")),  // 4h ago
            (1, Some(base_ts), make_src("C")),             // 6h ago
            (3, Some(base_ts - 72 * 3600), make_src("D")), // 3 days ago
            (4, None, make_src("E")),                      // No date
        ];

        sort_news_by_recency_and_relevance(&mut items);

        // Within the fresh 18-hour cycle (A, B, C), original Google relevance rank wins:
        // Rank 0 (B) -> Rank 1 (C) -> Rank 2 (A)
        // Then D (older date)
        // Then E (no date)
        assert_eq!(items[0].2.title, "B");
        assert_eq!(items[1].2.title, "C");
        assert_eq!(items[2].2.title, "A");
        assert_eq!(items[3].2.title, "D");
        assert_eq!(items[4].2.title, "E");
    }
}
