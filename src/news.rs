use crate::cli::NewsTimeframe;
use crate::scraper::{ScrapedPage, WebScraper};
use ::scraper::{Html, Selector};
use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use serde::{Deserialize, Serialize};
use std::sync::LazyLock;
use std::time::Duration;

static ITEM_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("item").unwrap());
static TITLE_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("title").unwrap());
static LINK_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("link").unwrap());
static PUBDATE_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("pubDate").unwrap());
static SOURCE_SEL: LazyLock<Selector> = LazyLock::new(|| Selector::parse("source").unwrap());

#[derive(Debug, Clone, Serialize, Deserialize)]
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
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
        );

        let client = reqwest::Client::builder()
            .default_headers(headers)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()
            .context("Failed to build Google News HTTP client")?;

        Ok(Self { client })
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

        let xml = resp.text().await.context("Failed to read RSS body")?;
        let doc = Html::parse_document(&xml);

        let mut items = Vec::new();
        for item_elem in doc.select(&ITEM_SEL).take(limit) {
            let title = item_elem
                .select(&TITLE_SEL)
                .next()
                .map(|t| t.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            // NextSibling or inner text for <link> in RSS
            let link = item_elem
                .select(&LINK_SEL)
                .next()
                .map(|l| l.text().collect::<String>().trim().to_string())
                .unwrap_or_default();

            let pub_date = item_elem
                .select(&PUBDATE_SEL)
                .next()
                .map(|p| p.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());

            let source_name = item_elem
                .select(&SOURCE_SEL)
                .next()
                .map(|s| s.text().collect::<String>().trim().to_string())
                .filter(|s| !s.is_empty());

            if !title.is_empty() && !link.is_empty() {
                items.push(NewsArticleItem {
                    title,
                    google_link: link,
                    source_name,
                    pub_date,
                });
            }
        }

        Ok(items)
    }

    /// Resolves an opaque Google News `/rss/articles/CBMi...` redirect link to the real publisher destination URL.
    pub async fn resolve_article_url(&self, google_url: &str) -> Option<String> {
        let base64_id = extract_article_id(google_url)?;

        // 1. Fetch intermediate Google News article page to extract ephemeral signature & timestamp
        let page_url = format!("https://news.google.com/articles/{}", base64_id);
        let resp = self.client.get(&page_url).send().await.ok()?;
        let html = resp.text().await.ok()?;

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

        let rpc_text = rpc_resp.text().await.ok()?;
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
        let feed_items = self.fetch_feed(topic, timeframe, limit).await?;
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
    let inner_array = parsed.as_array()?;
    let sub = inner_array.first()?.as_array()?;
    let json_str = sub.get(2)?.as_str()?;
    let inner_json: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let dest_url = inner_json.get(1)?.as_str()?;
    Some(dest_url.to_string())
}
