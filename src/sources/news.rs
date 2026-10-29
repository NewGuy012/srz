use crate::cli::NewsTimeframe;
use crate::scraper::{ScrapedPage, WebScraper};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsArticleItem {
    pub title: String,
    pub google_link: String,
    pub source_name: Option<String>,
    pub pub_date: Option<String>,
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

    /// Queries Google News RSS for the given topic and recency timeframe.
    pub async fn fetch_feed(
        &self,
        topic: &str,
        timeframe: NewsTimeframe,
        limit: usize,
    ) -> Result<Vec<NewsArticleItem>> {
        let suffix = timeframe.as_google_query_suffix();
        let query = if suffix.is_empty() || topic.contains("when:") {
            topic.trim().to_string()
        } else {
            format!("{} {}", topic.trim(), suffix)
        };

        let endpoint = format!(
            "https://news.google.com/rss/search?q={}&hl=en-US&gl=US&ceid=US:en",
            urlencoding::encode(&query)
        );

        let resp = self
            .client
            .get(&endpoint)
            .send()
            .await
            .with_context(|| format!("Failed to fetch Google News RSS feed from {}", endpoint))?;

        if !resp.status().is_success() {
            anyhow::bail!(
                "Google News RSS returned HTTP status {}: {}",
                resp.status(),
                endpoint
            );
        }

        let xml = crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES).await?;
        let items = parse_rss_items(&xml, limit);

        Ok(items)
    }

    /// Resolves an opaque Google News `/rss/articles/CBMi...` redirect link to the real publisher destination URL.
    pub async fn resolve_article_url(&self, google_url: &str) -> Option<String> {
        let base64_id = extract_article_id(google_url)?;

        // 1. Fetch intermediate Google News article page to extract ephemeral signature & timestamp
        let page_url = format!("https://news.google.com/articles/{}", base64_id);
        let resp = self.client.get(&page_url).send().await.ok()?;
        let html =
            crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES)
                .await
                .ok()?;

        let sig = extract_attr_value(&html, "data-n-a-sg=")?;
        let ts_str = extract_attr_value(&html, "data-n-a-ts=")?;
        let ts: i64 = ts_str.parse().ok()?;

        // 2. Submit batchexecute RPC request to resolve destination URL
        let payload_str = format!(
            r#"["garturlreq",[["X","X",["X","X"],null,null,1,1,"US:en",null,1,null,null,null,null,null,0,1],"X","X",1,[1,1,1],1,1,null,0,0,null,0],"{}",{},"{}"]"#,
            base64_id, ts, sig
        );
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

    /// Fetches and normalizes top news articles concurrently.
    pub async fn collect_news_articles(
        &self,
        topic: &str,
        timeframe: NewsTimeframe,
        limit: usize,
        scraper: &WebScraper,
    ) -> Result<Vec<NewsDigestSource>> {
        // Fetch up to 2x limit (or at least limit + 2) candidates to buffer against paywalls/dead links
        let fetch_limit = (limit * 2).max(limit + 2);
        let feed_items = self.fetch_feed(topic, timeframe, fetch_limit).await?;
        if feed_items.is_empty() {
            anyhow::bail!(
                "No recent news found on Google News for topic '{}' ({}).",
                topic,
                timeframe.display_label()
            );
        }

        let mut handles = Vec::with_capacity(feed_items.len());

        // Resolve destination URLs and scrape articles concurrently
        for item in feed_items {
            let client = self.clone();
            let scraper_cloned = scraper.clone();
            handles.push(tokio::spawn(async move {
                let direct_url = match client.resolve_article_url(&item.google_link).await {
                    Some(u) => u,
                    None => item.google_link.clone(),
                };

                let scraped: Result<ScrapedPage> = scraper_cloned.scrape(&direct_url).await;
                if let Ok(page) = scraped
                    && page.word_count > 60
                {
                    Some(NewsDigestSource {
                        title: page.title.unwrap_or(item.title),
                        source_name: item.source_name.unwrap_or_else(|| "Source".to_string()),
                        url: direct_url,
                        pub_date: item.pub_date,
                        content: page.content,
                        word_count: page.word_count,
                    })
                } else {
                    None
                }
            }));
        }

        let mut sources = Vec::new();
        for handle in handles {
            if let Ok(Some(source)) = handle.await {
                sources.push(source);
                if sources.len() >= limit {
                    break;
                }
            }
        }

        if sources.is_empty() {
            anyhow::bail!(
                "Found news headlines for '{}', but none of the article pages could be scraped.",
                topic
            );
        }

        Ok(sources)
    }
}

/// Case-insensitive search for an ASCII substring within a UTF-8 string slice.
pub fn find_ascii_case_insensitive(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let needle_bytes = needle.as_bytes();
    let haystack_bytes = haystack.as_bytes();
    if needle_bytes.len() > haystack_bytes.len() {
        return None;
    }

    haystack_bytes
        .windows(needle_bytes.len())
        .position(|window| {
            window
                .iter()
                .zip(needle_bytes)
                .all(|(h, n)| h.eq_ignore_ascii_case(n))
        })
}

/// Cleans CDATA wrappers and decodes HTML entities from an XML element's inner text.
fn clean_xml_text(raw: &str) -> String {
    let trimmed = raw.trim();
    if let Some(cdata_start) = trimmed.find("<![CDATA[") {
        let after_start = &trimmed[cdata_start + "<![CDATA[".len()..];
        if let Some(cdata_end) = after_start.find("]]>") {
            return after_start[..cdata_end].trim().to_string();
        }
    }
    crate::sources::youtube::decode_html_entities(trimmed).into_owned()
}

/// Extracts the inner content of a named XML tag (e.g. `<title>`, `<link>`, `<source ...>`).
pub fn extract_tag_value(xml: &str, tag_name: &str) -> Option<String> {
    let open_prefix = format!("<{}", tag_name);
    let mut search_pos = 0;

    while let Some(rel_start) = find_ascii_case_insensitive(&xml[search_pos..], &open_prefix) {
        let tag_start = search_pos + rel_start;
        let after_prefix = tag_start + open_prefix.len();
        if after_prefix >= xml.len() {
            return None;
        }

        // Verify that tag name boundary is respected: next char must be '>', whitespace, or '/'
        let next_char = xml[after_prefix..].chars().next()?;
        if next_char != '>' && !next_char.is_ascii_whitespace() && next_char != '/' {
            search_pos = after_prefix;
            continue;
        }

        let p = xml[after_prefix..].find('>')?;
        let tag_open_end = after_prefix + p;

        // If self-closing (e.g. `<link ... />`), it has no inner text
        if xml[tag_start..tag_open_end].trim_end().ends_with('/') {
            return None;
        }

        let close_tag = format!("</{}>", tag_name);
        let p = find_ascii_case_insensitive(&xml[tag_open_end + 1..], &close_tag)?;
        let tag_close_start = tag_open_end + 1 + p;

        let inner = &xml[tag_open_end + 1..tag_close_start];
        let cleaned = clean_xml_text(inner);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }

        search_pos = tag_close_start + close_tag.len();
    }

    None
}

/// Parses `<item>` elements from an RSS XML string into a list of [`NewsArticleItem`].
pub fn parse_rss_items(xml: &str, limit: usize) -> Vec<NewsArticleItem> {
    if limit == 0 {
        return Vec::new();
    }

    let mut items = Vec::new();
    let mut cursor = 0;

    while let Some(rel_start) = find_ascii_case_insensitive(&xml[cursor..], "<item") {
        let item_start = cursor + rel_start;
        let after_item_tag = match xml[item_start..].find('>') {
            Some(p) => item_start + p + 1,
            None => break,
        };

        let item_end = match find_ascii_case_insensitive(&xml[after_item_tag..], "</item>") {
            Some(p) => after_item_tag + p,
            None => break,
        };

        let item_xml = &xml[after_item_tag..item_end];
        cursor = item_end + "</item>".len();

        let title = extract_tag_value(item_xml, "title").unwrap_or_default();
        let link = extract_tag_value(item_xml, "link")
            .or_else(|| {
                extract_tag_value(item_xml, "guid")
                    .filter(|g| g.starts_with("http://") || g.starts_with("https://"))
            })
            .unwrap_or_default();

        let pub_date = extract_tag_value(item_xml, "pubDate");
        let source_name = extract_tag_value(item_xml, "source");

        if !title.is_empty() && !link.is_empty() {
            items.push(NewsArticleItem {
                title,
                google_link: link,
                source_name,
                pub_date,
            });
            if items.len() >= limit {
                break;
            }
        }
    }

    items
}

pub fn extract_article_id(url: &str) -> Option<&str> {
    let clean = url.trim();
    if let Some(pos) = clean.find("/articles/") {
        let after = &clean[pos + 10..];
        let id = after.split('?').next().unwrap_or(after);
        if !id.is_empty() {
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
    inner_json.pointer("/1")?.as_str().map(String::from)
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
        assert_eq!(items[0].pub_date.as_deref(), Some("Fri, 23 Oct 2026 10:00:00 GMT"));
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
}
