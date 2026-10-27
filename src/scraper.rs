use anyhow::{Context, Result};
use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use scraper::{Html, Selector};
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ScrapedPage {
    pub url: String,
    pub title: Option<String>,
    pub content: String,
    pub word_count: usize,
}

pub struct WebScraper {
    client: reqwest::Client,
}

impl WebScraper {
    pub fn new() -> Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            USER_AGENT,
            HeaderValue::from_static(
                "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                 AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36",
            ),
        );
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
            .timeout(Duration::from_secs(20))
            .redirect(reqwest::redirect::Policy::limited(10))
            .build()
            .context("Failed to initialize HTTP client")?;

        Ok(Self { client })
    }

    /// Fetches and normalizes the text from the given URL.
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

        let html = response
            .text()
            .await
            .context("Failed to read response body")?;

        self.normalize(url, &html)
    }

    /// Normalizes raw HTML into clean, readable text.
    pub fn normalize(&self, url: &str, raw_html: &str) -> Result<ScrapedPage> {
        let document = Html::parse_document(raw_html);

        // 1. Extract Title
        let title = self.extract_title(&document);

        // 2. Select main content container or fallback to body
        let target_html = self.extract_main_html(&document);

        // 3. Convert HTML to clean plain text
        let rendered_text = html2text::from_read(target_html.as_bytes(), 100)
            .context("Failed to convert HTML to text")?;

        // 4. Clean and normalize whitespace/formatting
        let cleaned_content = self.clean_text(&rendered_text);
        let word_count = cleaned_content.split_whitespace().count();

        Ok(ScrapedPage {
            url: url.to_string(),
            title,
            content: cleaned_content,
            word_count,
        })
    }

    fn extract_title(&self, document: &Html) -> Option<String> {
        // Try og:title first
        if let Ok(og_sel) = Selector::parse("meta[property='og:title']") {
            if let Some(elem) = document.select(&og_sel).next() {
                if let Some(content) = elem.value().attr("content") {
                    let trimmed = content.trim();
                    if !trimmed.is_empty() {
                        return Some(trimmed.to_string());
                    }
                }
            }
        }

        // Try <title> tag
        if let Ok(title_sel) = Selector::parse("title") {
            if let Some(elem) = document.select(&title_sel).next() {
                let text: String = elem.text().collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        // Try <h1> tag
        if let Ok(h1_sel) = Selector::parse("h1") {
            if let Some(elem) = document.select(&h1_sel).next() {
                let text: String = elem.text().collect();
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    return Some(trimmed.to_string());
                }
            }
        }

        None
    }

    fn extract_main_html(&self, document: &Html) -> String {
        // Try prioritizing primary content selectors
        let candidate_selectors = [
            "article",
            "main",
            "[role='main']",
            "#content",
            ".post-content",
            ".article-content",
            ".entry-content",
            "body",
        ];

        for sel_str in candidate_selectors {
            if let Ok(sel) = Selector::parse(sel_str) {
                if let Some(element) = document.select(&sel).next() {
                    let html = element.html();
                    // Basic sanity check: content should have reasonable length
                    if html.len() > 200 {
                        return html;
                    }
                }
            }
        }

        document.html()
    }

    fn clean_text(&self, text: &str) -> String {
        let mut lines = Vec::new();
        let mut empty_line_count = 0;

        for raw_line in text.lines() {
            let line = raw_line.trim();

            // Truncate citation, reference, and external link sections common in wikis
            let lower = line.to_ascii_lowercase();
            if lower == "references"
                || lower == "## references"
                || lower == "=== references ==="
                || lower == "== references =="
                || lower == "external links"
                || lower == "## external links"
                || lower == "=== external links ==="
                || lower == "notes and references"
                || lower == "## notes"
                || lower == "## bibliography"
            {
                break;
            }

            if line.is_empty() {
                empty_line_count += 1;
                if empty_line_count <= 2 {
                    lines.push("");
                }
            } else {
                empty_line_count = 0;
                lines.push(line);
            }
        }

        lines.join("\n").trim().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_extracts_title_and_clean_text() {
        let scraper = WebScraper::new().unwrap();
        let html = r#"
            <!DOCTYPE html>
            <html>
            <head>
                <title>Rust Programming Language</title>
                <meta property="og:title" content="Rust: Fast and Reliable">
            </head>
            <body>
                <header><nav><a href="/">Home</a></nav></header>
                <main>
                    <h1>Rust Overview</h1>
                    <p>Rust is a blazingly fast and memory-efficient language.</p>
                    <p>It empowers everyone to build reliable and efficient software.</p>
                </main>
                <footer><p>© 2026 Rust</p></footer>
            </body>
            </html>
        "#;

        let page = scraper.normalize("https://rust-lang.org", html).unwrap();
        assert_eq!(page.title.as_deref(), Some("Rust: Fast and Reliable"));
        assert!(page.content.contains("blazingly fast"));
        assert!(page.word_count > 10);
    }
}

