use crate::cli::NewsTimeframe;
use crate::scraper::{ScrapedPage, WebScraper, collapse_whitespace, validate_public_http_url};
use crate::system::text_util::{
    XmlEvent, XmlScanner, decode_html_entities, is_http_url, url_encode, xml_attr, xml_local_name,
};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

pub const MAX_NEWS_LIMIT: usize = 50;

#[derive(Debug, Clone, PartialEq, Eq)]
struct NewsArticleItem {
    title: String,
    google_link: String,
    source_name: Option<String>,
    pub_date: Option<String>,
    snippet: Option<String>,
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

    pub fn is_direct_feed(topic: &str) -> bool {
        is_http_url(topic.trim())
    }

    pub fn search_url_for_topic(topic: &str) -> String {
        let trimmed = topic.trim();
        if Self::is_direct_feed(trimmed) {
            trimmed.to_string()
        } else {
            format!("https://news.google.com/search?q={}", url_encode(trimmed))
        }
    }

    /// Queries Google News RSS for a topic/timeframe, or fetches a direct RSS/Atom/JSON feed URL.
    async fn fetch_feed(
        &self,
        topic: &str,
        timeframe: NewsTimeframe,
        limit: usize,
    ) -> Result<Vec<NewsArticleItem>> {
        let trimmed = topic.trim();
        let endpoint = if Self::is_direct_feed(trimmed) {
            validate_public_http_url(trimmed)?;
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
                url_encode(&query)
            )
        };

        let body = crate::scraper::fetch_text_limited(
            self.client.get(&endpoint),
            crate::scraper::MAX_SCRAPE_BYTES,
            &format!("syndication feed from {endpoint}"),
        )
        .await?;
        Ok(parse_feed_items(&body, limit.clamp(1, MAX_NEWS_LIMIT)))
    }

    async fn resolve_article_url(&self, google_url: &str) -> Option<String> {
        if let Some(direct) = decode_google_news_url(google_url) {
            return Some(direct);
        }

        let base64_id = extract_article_id(google_url)?;

        // Must use /rss/articles/ because /articles/ triggers Google HTTP 429 protection.
        let page_url = format!("https://news.google.com/rss/articles/{base64_id}");
        let html = crate::scraper::fetch_text_limited(
            self.client.get(&page_url).timeout(Duration::from_secs(4)),
            256 * 1024,
            "Google News article page",
        )
        .await
        .ok()?;
        let (sig, ts) = extract_gnews_sig_ts(&html)?;

        let payload_str = serde_json::json!([
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
        ])
        .to_string();
        let envelope = serde_json::json!([[["Fbv4je", payload_str, null, "generic"]]]);
        let form_body = format!("f.req={}", url_encode(&envelope.to_string()));

        let rpc_text = crate::scraper::fetch_text_limited(
            self.client
                .post("https://news.google.com/_/DotsSplashUi/data/batchexecute")
                .header(
                    reqwest::header::CONTENT_TYPE,
                    "application/x-www-form-urlencoded;charset=UTF-8",
                )
                .body(form_body)
                .timeout(Duration::from_secs(4)),
            128 * 1024,
            "Google News batchexecute",
        )
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

        let target_limit = limit.clamp(1, MAX_NEWS_LIMIT);
        let fetch_limit = target_limit.saturating_mul(5).clamp(15, MAX_NEWS_LIMIT);
        let feed_items = self.fetch_feed(topic, timeframe, fetch_limit).await?;
        if feed_items.is_empty() {
            let tip = if Self::is_direct_feed(topic) {
                String::new()
            } else {
                let extra = match timeframe {
                    NewsTimeframe::Day => "`-t week`, `-t month`, or `-t all`, or ",
                    NewsTimeframe::Week => "`-t month` or `-t all`, or ",
                    NewsTimeframe::Month => "`-t all`, or ",
                    NewsTimeframe::All => "",
                };
                if extra.is_empty() {
                    " Try broadening the topic search terms.".to_string()
                } else {
                    format!(
                        " Try expanding the recency timeframe with {extra}broadening the topic search terms."
                    )
                }
            };
            anyhow::bail!(
                "No recent articles found in feed for '{}' ({}){}",
                topic,
                timeframe.display_label(),
                tip
            );
        }

        let mut collected: BTreeMap<usize, NewsDigestSource> = BTreeMap::new();
        let mut uncached_items: Vec<(usize, NewsArticleItem)> = Vec::new();

        for (idx, item) in feed_items.into_iter().enumerate() {
            let cached_page = cache.and_then(|c| {
                c.get_news_page(&item.google_link).or_else(|| {
                    decode_google_news_url(&item.google_link).and_then(|u| c.get_news_page(&u))
                })
            });

            if let Some(page) = cached_page
                && page.word_count > 60
                && !is_google_news_link(&page.url)
            {
                let url = if is_google_news_link(&item.google_link) {
                    page.url
                } else {
                    item.google_link.clone()
                };
                collected.insert(
                    idx,
                    make_digest_source(
                        &item,
                        url,
                        page.content,
                        page.word_count,
                        page.title.as_deref(),
                    ),
                );
            } else {
                uncached_items.push((idx, item));
            }
        }

        let have_top_k = |map: &BTreeMap<usize, NewsDigestSource>, pending: &BTreeSet<usize>| {
            map.keys()
                .nth(target_limit.saturating_sub(1))
                .is_some_and(|&cutoff| {
                    map.len() >= target_limit && pending.first().is_none_or(|&min_p| min_p > cutoff)
                })
        };

        if !have_top_k(&collected, &BTreeSet::new()) && !uncached_items.is_empty() {
            let semaphore =
                std::sync::Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_NEWS_SCRAPES));
            let mut join_set = tokio::task::JoinSet::new();
            let mut pending_indices = BTreeSet::new();

            for (idx, item) in uncached_items {
                pending_indices.insert(idx);
                let client = self.clone();
                let scraper_cloned = scraper.clone();
                let sem = semaphore.clone();
                join_set.spawn(async move {
                    let Ok(_permit) = sem.acquire().await else {
                        return (idx, item.google_link, None);
                    };

                    if is_safe_article_url(&item.google_link)
                        && let Some(src) = snippet_digest_source(&item, 150)
                    {
                        return (idx, item.google_link, Some(src));
                    }

                    let is_gnews = is_google_news_link(&item.google_link);
                    let resolved_url = client.resolve_article_url(&item.google_link).await;

                    let direct_url = match resolved_url {
                        Some(u) => u,
                        None if !is_gnews => item.google_link.clone(),
                        None => {
                            let src = is_safe_article_url(&item.google_link)
                                .then(|| snippet_digest_source(&item, 20))
                                .flatten();
                            return (idx, item.google_link, src);
                        }
                    };

                    if is_google_news_link(&direct_url) || !is_safe_article_url(&direct_url) {
                        return (idx, item.google_link, None);
                    }

                    let scraped = tokio::time::timeout(
                        Duration::from_secs(4),
                        scraper_cloned.scrape(&direct_url),
                    )
                    .await;

                    let src = if let Ok(Ok(page)) = scraped
                        && page.word_count > 60
                    {
                        Some(make_digest_source(
                            &item,
                            direct_url,
                            page.content,
                            page.word_count,
                            page.title.as_deref(),
                        ))
                    } else {
                        snippet_digest_source(&item, 20).map(|mut s| {
                            s.url = direct_url;
                            s
                        })
                    };
                    (idx, item.google_link, src)
                });
            }

            while let Some(res) = join_set.join_next().await {
                if let Ok((idx, google_link, maybe_source)) = res {
                    pending_indices.remove(&idx);
                    if let Some(source) = maybe_source {
                        if let Some(c) = cache
                            && source.word_count > 60
                            && !is_google_news_link(&source.url)
                            && is_safe_article_url(&source.url)
                        {
                            let page_record = ScrapedPage {
                                url: source.url.clone(),
                                title: Some(source.title.clone()),
                                content: source.content.clone(),
                                word_count: source.word_count,
                            };
                            c.set_page(&source.url, &page_record);
                            if source.url != google_link && is_safe_article_url(&google_link) {
                                c.set_page(&google_link, &page_record);
                            }
                        }
                        collected.insert(idx, source);
                    }

                    if have_top_k(&collected, &pending_indices) {
                        join_set.abort_all();
                        break;
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

        Ok(collected.into_values().take(target_limit).collect())
    }
}

fn make_digest_source(
    item: &NewsArticleItem,
    url: String,
    content: String,
    word_count: usize,
    title_override: Option<&str>,
) -> NewsDigestSource {
    NewsDigestSource {
        title: title_override
            .map(str::to_string)
            .unwrap_or_else(|| item.title.clone()),
        source_name: item
            .source_name
            .clone()
            .unwrap_or_else(|| "Source".to_string()),
        url,
        pub_date: item.pub_date.clone(),
        content,
        word_count,
    }
}

fn snippet_digest_source(item: &NewsArticleItem, min_words: usize) -> Option<NewsDigestSource> {
    let snip = item.snippet.as_ref()?;
    let wc = snip.split_whitespace().count();
    (wc >= min_words)
        .then(|| make_digest_source(item, item.google_link.clone(), snip.clone(), wc, None))
}

fn extract_gnews_sig_ts(html: &str) -> Option<(String, i64)> {
    let mut sig = None;
    let mut ts = None;
    let mut scanner = XmlScanner::new(html);
    while let Some(ev) = scanner.next_event() {
        if let XmlEvent::Start { attrs, .. } = ev {
            if sig.is_none()
                && let Some(val) = xml_attr(attrs, "data-n-a-sg")
            {
                let s = val.trim();
                if s.is_empty()
                    || s.len() > 512
                    || s.chars().any(|c| c.is_control() || c == '"' || c == '\\')
                {
                    return None;
                }
                sig = Some(s.to_string());
            }
            if ts.is_none()
                && let Some(val) = xml_attr(attrs, "data-n-a-ts")
            {
                ts = val.trim().parse::<i64>().ok();
            }
            if let (Some(s), Some(t)) = (&sig, ts) {
                return Some((s.clone(), t));
            }
        }
    }
    None
}

fn clean_feed_snippet(raw: &str) -> String {
    let trimmed = raw.trim();
    if !(trimmed.contains('<') && trimmed.contains('>')) {
        return collapse_whitespace(decode_html_entities(trimmed).as_ref());
    }
    let mut out = String::with_capacity(trimmed.len());
    let mut scanner = XmlScanner::new(trimmed);
    while let Some(ev) = scanner.next_event() {
        match ev {
            XmlEvent::Start { name, .. } | XmlEvent::End { name } => {
                let local = xml_local_name(name);
                if matches!(
                    local.to_ascii_lowercase().as_str(),
                    "p" | "div"
                        | "br"
                        | "li"
                        | "tr"
                        | "h1"
                        | "h2"
                        | "h3"
                        | "h4"
                        | "h5"
                        | "h6"
                        | "blockquote"
                ) && !out.ends_with(' ')
                    && !out.is_empty()
                {
                    out.push(' ');
                }
            }
            XmlEvent::Text(t) => out.push_str(decode_html_entities(t).as_ref()),
            XmlEvent::CData(c) => out.push_str(decode_html_entities(c).as_ref()),
        }
    }
    collapse_whitespace(&out)
}

fn parse_feed_items(body: &str, limit: usize) -> Vec<NewsArticleItem> {
    if limit == 0 {
        return Vec::new();
    }
    if body.trim_start().starts_with('{') {
        return parse_json_feed(body, limit);
    }
    parse_rss_items(body, limit)
}

fn parse_json_feed(json_str: &str, limit: usize) -> Vec<NewsArticleItem> {
    let Ok(val) = serde_json::from_str::<serde_json::Value>(json_str) else {
        return Vec::new();
    };
    let feed_title = val
        .get("title")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let Some(entries) = val.get("items").and_then(|v| v.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries {
        let Some(title) = entry
            .get("title")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        let Some(link) = entry
            .get("url")
            .or_else(|| entry.get("external_url"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|u| is_http_url(u))
        else {
            continue;
        };
        let snippet = entry
            .get("content_text")
            .and_then(|v| v.as_str())
            .map(collapse_whitespace)
            .or_else(|| {
                entry
                    .get("content_html")
                    .or_else(|| entry.get("summary"))
                    .and_then(|v| v.as_str())
                    .map(clean_feed_snippet)
            })
            .filter(|s| !s.is_empty());
        let source_name = entry
            .get("authors")
            .and_then(|a| a.as_array())
            .and_then(|arr| arr.first())
            .or_else(|| entry.get("author"))
            .and_then(|a| a.get("name"))
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .or_else(|| feed_title.clone());
        let pub_date = entry
            .get("date_published")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        out.push(NewsArticleItem {
            title: title.to_string(),
            google_link: link.to_string(),
            source_name,
            pub_date,
            snippet,
        });
        if out.len() >= limit {
            break;
        }
    }
    out
}

fn parse_rss_items(xml: &str, limit: usize) -> Vec<NewsArticleItem> {
    if limit == 0 {
        return Vec::new();
    }

    let mut scanner = XmlScanner::new(xml);
    let mut items = Vec::new();
    let mut feed_source: Option<String> = None;

    let mut in_entry = false;
    let mut in_author = false;
    let mut cur_title = String::new();
    let mut cur_link = String::new();
    let mut cur_id = String::new();
    let mut cur_pub_date = None;
    let mut cur_source = None;
    let mut cur_author: Option<String> = None;
    let mut cur_author_plain = String::new();
    let mut cur_snippet: Option<String> = None;

    while let Some(ev) = scanner.next_event() {
        match ev {
            XmlEvent::Start {
                name,
                attrs,
                is_empty,
            } => {
                let local = xml_local_name(name);
                if (local.eq_ignore_ascii_case("item") || local.eq_ignore_ascii_case("entry"))
                    && !is_empty
                {
                    in_entry = true;
                    in_author = false;
                    cur_title.clear();
                    cur_link.clear();
                    cur_id.clear();
                    cur_pub_date = None;
                    cur_source = None;
                    cur_author = None;
                    cur_author_plain.clear();
                    cur_snippet = None;
                } else if in_entry {
                    if in_author {
                        if local.eq_ignore_ascii_case("name") && !is_empty {
                            let n = scanner.read_text(name).trim().to_string();
                            if !n.is_empty() {
                                cur_author = Some(n);
                            }
                        } else if !is_empty {
                            let _ = scanner.read_text(name);
                        }
                        continue;
                    }
                    if local.eq_ignore_ascii_case("title") && !is_empty {
                        let text = scanner.read_text(name);
                        if cur_title.is_empty() {
                            cur_title = text.trim().to_string();
                        }
                    } else if local.eq_ignore_ascii_case("link") {
                        let is_alt = xml_attr(attrs, "rel")
                            .is_none_or(|r| r.trim().eq_ignore_ascii_case("alternate"));
                        if cur_link.is_empty()
                            && is_alt
                            && let Some(href) = xml_attr(attrs, "href")
                        {
                            let trimmed = href.trim();
                            if is_http_url(trimmed) {
                                cur_link = trimmed.to_string();
                            }
                        }
                        if !is_empty {
                            let text = scanner.read_text(name);
                            let trimmed = text.trim();
                            if cur_link.is_empty() && is_http_url(trimmed) {
                                cur_link = trimmed.to_string();
                            }
                        }
                    } else if (local.eq_ignore_ascii_case("guid")
                        || local.eq_ignore_ascii_case("id"))
                        && !is_empty
                    {
                        let text = scanner.read_text(name);
                        if cur_id.is_empty() {
                            cur_id = text.trim().to_string();
                        }
                    } else if (local.eq_ignore_ascii_case("pubdate")
                        || local.eq_ignore_ascii_case("published")
                        || local.eq_ignore_ascii_case("updated"))
                        && !is_empty
                    {
                        let text = scanner.read_text(name);
                        let trimmed = text.trim();
                        if cur_pub_date.is_none() && !trimmed.is_empty() {
                            cur_pub_date = Some(trimmed.to_string());
                        }
                    } else if local.eq_ignore_ascii_case("source") && !is_empty {
                        let cleaned = scanner.read_text(name).trim().to_string();
                        if cur_source.is_none() && !cleaned.is_empty() {
                            cur_source = Some(cleaned);
                        }
                    } else if local.eq_ignore_ascii_case("author") && !is_empty {
                        in_author = true;
                        cur_author_plain.clear();
                    } else if (local.eq_ignore_ascii_case("description")
                        || local.eq_ignore_ascii_case("summary")
                        || local.eq_ignore_ascii_case("content")
                        || local.eq_ignore_ascii_case("encoded"))
                        && !is_empty
                    {
                        let text = scanner.read_text(name);
                        let cleaned = clean_feed_snippet(&text);
                        if !cleaned.is_empty()
                            && cur_snippet
                                .as_ref()
                                .is_none_or(|existing| cleaned.len() > existing.len())
                        {
                            cur_snippet = Some(cleaned);
                        }
                    }
                } else if local.eq_ignore_ascii_case("title") && feed_source.is_none() && !is_empty
                {
                    let text = scanner.read_text(name);
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        feed_source = Some(trimmed.to_string());
                    }
                }
            }
            XmlEvent::Text(t) if in_author => {
                cur_author_plain.push_str(decode_html_entities(t).as_ref())
            }
            XmlEvent::CData(c) if in_author => cur_author_plain.push_str(c),
            XmlEvent::End { name } => {
                let local = xml_local_name(name);
                if local.eq_ignore_ascii_case("author") && in_author {
                    in_author = false;
                    if cur_author.is_none() {
                        let plain = cur_author_plain.trim();
                        if !plain.is_empty() {
                            cur_author = Some(plain.to_string());
                        }
                    }
                } else if local.eq_ignore_ascii_case("item") || local.eq_ignore_ascii_case("entry")
                {
                    in_entry = false;
                    let link = if !cur_link.is_empty() {
                        cur_link.clone()
                    } else if is_http_url(cur_id.trim()) {
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
            _ => {}
        }
    }

    items
}

fn decode_google_news_url(url: &str) -> Option<String> {
    let id = extract_article_id(url)?;
    if !id.starts_with("CBMi") {
        return None;
    }
    let raw = decode_base64_url_safe(id)?;
    extract_url_from_protobuf(&raw)
}

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

fn is_safe_article_url(url: &str) -> bool {
    validate_public_http_url(url.trim()).is_ok()
}

fn read_preceding_varint_len(raw: &[u8], pos: usize) -> Option<usize> {
    if pos >= 2 && (raw[pos - 2] & 0x80) != 0 && (raw[pos - 1] & 0x80) == 0 {
        let len = ((raw[pos - 2] & 0x7f) as usize) | (((raw[pos - 1] & 0x7f) as usize) << 7);
        return (len >= 128 && pos + len <= raw.len()).then_some(len);
    }
    if pos >= 1 && (raw[pos - 1] & 0x80) == 0 {
        let len = raw[pos - 1] as usize;
        return (len < 128 && pos + len <= raw.len()).then_some(len);
    }
    None
}

fn extract_url_from_protobuf(raw: &[u8]) -> Option<String> {
    const SCHEMES: &[&[u8]] = &[b"https://", b"http://"];
    for scheme in SCHEMES {
        if let Some(pos) = raw.windows(scheme.len()).position(|w| w == *scheme) {
            if let Some(len) = read_preceding_varint_len(raw, pos)
                && len >= scheme.len()
                && let Ok(s) = std::str::from_utf8(&raw[pos..pos + len])
                && !s
                    .chars()
                    .any(|c| c == ' ' || c == '"' || c == '<' || c == '>')
                && is_safe_article_url(s)
            {
                return Some(s.to_string());
            }

            let mut end = pos;
            while end < raw.len() {
                let b = raw[end];
                if (33..=126).contains(&b) && !matches!(b, b'"' | b'\'' | b'<' | b'>' | b'*') {
                    end += 1;
                } else {
                    break;
                }
            }
            if let Ok(s) = std::str::from_utf8(&raw[pos..end])
                && is_safe_article_url(s)
            {
                return Some(s.to_string());
            }
        }
    }
    None
}

fn is_google_news_link(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.host_str() == Some("news.google.com")
    })
}

fn extract_article_id(url: &str) -> Option<&str> {
    if !is_google_news_link(url) {
        return None;
    }
    let clean = url.trim();
    let without_scheme = clean.split_once("://").map(|(_, r)| r).unwrap_or(clean);
    let path_and_query = without_scheme.split_once('/').map(|(_, p)| p).unwrap_or("");
    let path = path_and_query.split(['?', '#']).next().unwrap_or("");
    let rest = path
        .strip_prefix("rss/articles/")
        .or_else(|| path.strip_prefix("articles/"))?;
    let id = rest.split('/').next()?;
    (!id.is_empty()
        && id.len() <= 2048
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'=')))
    .then_some(id)
}

fn parse_batchexecute_destination_url(text: &str) -> Option<String> {
    let clean = text.trim();
    let json_text = clean.strip_prefix(")]}'").unwrap_or(clean).trim();
    let candidate = json_text
        .lines()
        .find(|line| line.trim_start().starts_with('['))
        .unwrap_or(json_text);
    let parsed: serde_json::Value = serde_json::from_str(candidate.trim()).ok()?;

    let payload_str = parsed
        .as_array()
        .and_then(|arr| {
            arr.iter().find_map(|entry| {
                let entry_arr = entry.as_array()?;
                (entry_arr.get(1)?.as_str() == Some("Fbv4je"))
                    .then(|| entry_arr.get(2)?.as_str())
                    .flatten()
            })
        })
        .or_else(|| parsed.pointer("/0/2")?.as_str())?;

    let inner: serde_json::Value = serde_json::from_str(payload_str).ok()?;
    let trimmed = inner.pointer("/1")?.as_str()?.trim();
    is_safe_article_url(trimmed).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_feed_rss_atom_json_and_edge_cases() {
        let rss = r#"<?xml version="1.0"?><rss version="2.0"><channel><title>Feed</title>
            <item>
                <title>Pens Points - PensBurgh</title>
                <link>https://news.google.com/rss/articles/CBMi12345?oc=5</link>
                <pubDate>Fri, 23 Oct 2026 10:00:00 GMT</pubDate>
                <source url="https://pensburgh.com">PensBurgh</source>
                <description><![CDATA[<p>a<b>b</b>! &amp; 3 < 5.</p>]]></description>
            </item>
            <item>
                <title>Special &quot;Q&quot; &amp; &apos;A&apos;</title>
                <guid>https://example.com/guid-link</guid>
                <description>Plain &amp; entity</description>
            </item>
            <item><title>XSS</title><link>javascript:alert(1)</link></item>
        </channel></rss>"#;
        let items = parse_feed_items(rss, 2);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].source_name.as_deref(), Some("PensBurgh"));
        assert_eq!(items[0].snippet.as_deref(), Some("ab! & 3 < 5."));
        assert_eq!(items[1].title, "Special \"Q\" & 'A'");
        assert_eq!(items[1].google_link, "https://example.com/guid-link");
        assert_eq!(items[1].snippet.as_deref(), Some("Plain & entity"));

        let atom = r#"<feed xmlns="http://www.w3.org/2005/Atom"><entry>
            <title>Atom Title</title>
            <link rel="enclosure" href="https://example.com/audio.mp3"/>
            <link rel="alternate" href="https://example.com/atom-1"/>
            <updated>2026-10-23T18:30:02Z</updated>
            <author><name>John Doe</name><email>john@example.com</email></author>
        </entry></feed>"#;
        let atom_items = parse_feed_items(atom, 5);
        assert_eq!(atom_items.len(), 1);
        assert_eq!(atom_items[0].google_link, "https://example.com/atom-1");
        assert_eq!(atom_items[0].source_name.as_deref(), Some("John Doe"));

        let json_feed = r#"{"version":"https://jsonfeed.org/version/1.1","title":"JF","items":[{"title":"JSON Item","url":"https://example.com/j1","content_text":"Hello world"}]}"#;
        let jf_items = parse_feed_items(json_feed, 5);
        assert_eq!(jf_items.len(), 1);
        assert_eq!(jf_items[0].google_link, "https://example.com/j1");
        assert_eq!(jf_items[0].source_name.as_deref(), Some("JF"));
    }

    fn encode_cbmi_token(target_url: &str) -> String {
        const TABLE: &[u8; 64] =
            b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let mut raw = vec![0x08, 0x13, 0x22];
        let mut len = target_url.len();
        while len >= 0x80 {
            raw.push((len as u8 & 0x7f) | 0x80);
            len >>= 7;
        }
        raw.push(len as u8);
        raw.extend_from_slice(target_url.as_bytes());
        raw.extend_from_slice(&[0x2a, 0x03, b'f', b'o', b'o']);
        let mut out = String::new();
        for chunk in raw.chunks(3) {
            let b0 = chunk[0] as usize;
            let b1 = *chunk.get(1).unwrap_or(&0) as usize;
            let b2 = *chunk.get(2).unwrap_or(&0) as usize;
            out.push(TABLE[b0 >> 2] as char);
            out.push(TABLE[((b0 & 0x03) << 4) | (b1 >> 4)] as char);
            if chunk.len() > 1 {
                out.push(TABLE[((b1 & 0x0f) << 2) | (b2 >> 6)] as char);
            }
            if chunk.len() > 2 {
                out.push(TABLE[b2 & 0x3f] as char);
            }
        }
        out
    }

    #[test]
    fn test_google_news_decoding_validation_and_rpc() {
        let long_url = format!("https://example.com/{}/story.html", "a".repeat(130));
        let long_link = format!(
            "https://NEWS.GOOGLE.COM/rss/articles/{}",
            encode_cbmi_token(&long_url)
        );
        assert_eq!(
            decode_google_news_url(&long_link).as_deref(),
            Some(long_url.as_str())
        );
        assert_eq!(
            decode_google_news_url("https://news.google.com/articles/CBIi98765"),
            None
        );

        for bad in [
            "https://attacker.com/articles/CBMi12345",
            "https://example.com/?q=/articles/CBMi12345",
            "https://news.google.com/articles/../../etc/passwd",
            &format!(
                "https://news.google.com/rss/articles/{}",
                encode_cbmi_token("http://127.0.0.1/admin")
            ),
        ] {
            assert_eq!(decode_google_news_url(bad), None, "should reject {bad}");
        }

        let sig_html = r#"<div data-n-a-sg="ABC123xyz" data-n-a-ts="1714200000"></div>"#;
        assert_eq!(
            extract_gnews_sig_ts(sig_html),
            Some(("ABC123xyz".to_string(), 1714200000))
        );

        let rpc_framed = ")]}'\n\n123\n[[\"wrb.fr\",\"Fbv4je\",\"[\\\"garturlres\\\",\\\"https://www.infoworld.com/article/4217735/rustup.html\\\",1]\",null,null,null,\"generic\"]]";
        assert_eq!(
            parse_batchexecute_destination_url(rpc_framed).as_deref(),
            Some("https://www.infoworld.com/article/4217735/rustup.html")
        );

        for blocked in [
            "javascript:alert(1)",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            let rpc = serde_json::json!([[
                "wrb.fr",
                "Fbv4je",
                serde_json::json!(["garturlres", blocked]).to_string()
            ]])
            .to_string();
            assert_eq!(parse_batchexecute_destination_url(&rpc), None);
        }
    }

    #[tokio::test]
    async fn test_fetch_feed_blocks_private_direct_url() {
        let client = GoogleNewsClient::with_client(reqwest::Client::new());
        let err = client
            .fetch_feed("http://127.0.0.1:8080/feed.xml", NewsTimeframe::Day, 5)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SSRF protection"));
    }
}
