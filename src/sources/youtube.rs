use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use crate::scraper::ScrapedPage;

#[derive(Deserialize, Debug, Clone)]
pub struct CaptionTrack {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "languageCode", default)]
    pub language_code: String,
    #[serde(default)]
    pub name: Option<CaptionTrackName>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct CaptionTrackName {
    #[serde(rename = "simpleText", default)]
    pub simple_text: String,
}

#[derive(Deserialize, Debug)]
struct TimedTextJson {
    #[serde(default)]
    events: Vec<TimedTextEvent>,
}

#[derive(Deserialize, Debug)]
struct TimedTextEvent {
    #[serde(rename = "tStartMs", default)]
    start_ms: u64,
    #[serde(default)]
    segs: Option<Vec<TimedTextSegment>>,
}

#[derive(Deserialize, Debug)]
struct TimedTextSegment {
    #[serde(default)]
    utf8: String,
}

/// Checks if a URL is a YouTube video link and returns its 11-character video ID.
pub fn extract_youtube_video_id(url: &str) -> Option<&str> {
    let trimmed = url.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return None;
    }

    // Shortened youtu.be/ID
    if let Some(pos) = trimmed.find("youtu.be/") {
        let after = &trimmed[pos + "youtu.be/".len()..];
        let id = after.split(['?', '&', '/', '#']).next()?;
        if is_valid_youtube_id(id) {
            return Some(id);
        }
    }

    // Standard youtube.com URLs
    if trimmed.contains("youtube.com/") {
        if let Some(pos) = trimmed.find("/shorts/") {
            let after = &trimmed[pos + "/shorts/".len()..];
            let id = after.split(['?', '&', '/', '#']).next()?;
            if is_valid_youtube_id(id) {
                return Some(id);
            }
        }
        if let Some(pos) = trimmed.find("/embed/") {
            let after = &trimmed[pos + "/embed/".len()..];
            let id = after.split(['?', '&', '/', '#']).next()?;
            if is_valid_youtube_id(id) {
                return Some(id);
            }
        }
        if let Some(pos) = trimmed.find("v=") {
            let after = &trimmed[pos + 2..];
            let id = after.split(['&', '#', '/']).next()?;
            if is_valid_youtube_id(id) {
                return Some(id);
            }
        }
    }

    None
}

fn is_valid_youtube_id(id: &str) -> bool {
    id.len() == 11 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Extracts caption tracks from the YouTube watch page HTML.
pub fn extract_caption_tracks(html: &str) -> Option<Vec<CaptionTrack>> {
    let key = "\"captionTracks\":";
    let start_idx = html.find(key)? + key.len();
    let slice = &html[start_idx..];
    let array_start = slice.find('[')?;
    let mut depth = 0;
    let mut array_end = None;
    for (i, b) in slice[array_start..].bytes().enumerate() {
        if b == b'[' {
            depth += 1;
        } else if b == b']' {
            depth -= 1;
            if depth == 0 {
                array_end = Some(array_start + i + 1);
                break;
            }
        }
    }
    let array_str = &slice[array_start..array_end?];
    serde_json::from_str::<Vec<CaptionTrack>>(array_str).ok()
}

/// Extracts the video title from YouTube watch page HTML.
pub fn extract_video_title(html: &str) -> Option<String> {
    if let Some(start) = html.find("<title>") {
        let after = &html[start + "<title>".len()..];
        if let Some(end) = after.find("</title>") {
            let title = &after[..end];
            let clean = title.trim_end_matches("- YouTube").trim();
            if !clean.is_empty() {
                return Some(decode_html_entities(clean).into_owned());
            }
        }
    }
    None
}

/// Formats seconds into [MM:SS] or [HH:MM:SS].
pub fn format_timestamp(seconds: u64) -> String {
    let hrs = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hrs > 0 {
        format!("[{:02}:{:02}:{:02}]", hrs, mins, secs)
    } else {
        format!("[{:02}:{:02}]", mins, secs)
    }
}

/// Decodes common HTML entities in a single pass, returning a zero-copy `Cow::Borrowed`
/// when `s` contains no `&` character and preventing double-unescaping (e.g. `&amp;lt;`).
pub fn decode_html_entities(s: &str) -> std::borrow::Cow<'_, str> {
    if !s.contains('&') {
        return std::borrow::Cow::Borrowed(s);
    }

    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp_idx) = rest.find('&') {
        out.push_str(&rest[..amp_idx]);
        let after_amp = &rest[amp_idx..];
        let matched = if after_amp.starts_with("&quot;") {
            Some(('"', 6))
        } else if after_amp.starts_with("&apos;") {
            Some(('\'', 6))
        } else if after_amp.starts_with("&#39;") {
            Some(('\'', 5))
        } else if after_amp.starts_with("&amp;") {
            Some(('&', 5))
        } else if after_amp.starts_with("&lt;") {
            Some(('<', 4))
        } else if after_amp.starts_with("&gt;") {
            Some(('>', 4))
        } else if after_amp.starts_with("&nbsp;") {
            Some((' ', 6))
        } else {
            None
        };

        if let Some((ch, len)) = matched {
            out.push(ch);
            rest = &after_amp[len..];
        } else {
            out.push('&');
            rest = &after_amp[1..];
        }
    }
    out.push_str(rest);
    std::borrow::Cow::Owned(out)
}

struct TimedTranscriptBuilder {
    result: String,
    current_interval: u64,
    current_line: String,
}

impl TimedTranscriptBuilder {
    fn new(capacity: usize) -> Self {
        Self {
            result: String::with_capacity(capacity),
            current_interval: u64::MAX,
            current_line: String::new(),
        }
    }

    fn add_cue(&mut self, seconds: u64, text: &str) {
        let clean: std::borrow::Cow<'_, str> = if text.contains('\n') {
            std::borrow::Cow::Owned(text.replace('\n', " "))
        } else {
            std::borrow::Cow::Borrowed(text)
        };
        let trimmed = clean.trim();
        if trimmed.is_empty() {
            return;
        }

        let interval = seconds / 60; // Group by ~1 minute cues
        if interval != self.current_interval {
            if !self.current_line.is_empty() {
                self.result.push_str(&self.current_line);
                self.result.push_str("\n\n");
                self.current_line.clear();
            }
            self.current_interval = interval;
            self.current_line.push_str(&format_timestamp(seconds));
            self.current_line.push(' ');
        } else if !self.current_line.is_empty() && !self.current_line.ends_with(' ') {
            self.current_line.push(' ');
        }

        self.current_line.push_str(trimmed);
    }

    fn finish(mut self) -> Option<String> {
        if !self.current_line.is_empty() {
            self.result.push_str(&self.current_line);
            self.result.push('\n');
        }
        if self.result.trim().is_empty() {
            None
        } else {
            Some(self.result)
        }
    }
}

/// Parses JSON3 format timed text into formatted timestamped paragraphs.
pub fn parse_json3_timedtext(json_str: &str) -> Option<String> {
    let parsed: TimedTextJson = serde_json::from_str(json_str).ok()?;
    if parsed.events.is_empty() {
        return None;
    }

    let mut builder = TimedTranscriptBuilder::new(json_str.len() / 2);
    let mut combined_buf = String::new();

    for ev in parsed.events {
        let Some(segs) = ev.segs.as_deref() else {
            continue;
        };
        combined_buf.clear();
        for seg in segs {
            if !seg.utf8.trim().is_empty() {
                combined_buf.push_str(&seg.utf8);
            }
        }
        if !combined_buf.is_empty() {
            builder.add_cue(ev.start_ms / 1000, &combined_buf);
        }
    }

    builder.finish()
}

/// Parses XML timed text `<text start="1.23" dur="4.56">...</text>` into formatted paragraphs.
pub fn parse_xml_timedtext(xml_str: &str) -> Option<String> {
    if !xml_str.contains("<text") {
        return None;
    }

    let mut builder = TimedTranscriptBuilder::new(xml_str.len() / 2);

    let mut cursor = 0;
    while let Some(pos) = xml_str[cursor..].find("<text") {
        let tag_start = cursor + pos;
        let tag_end = match xml_str[tag_start..].find('>') {
            Some(p) => tag_start + p,
            None => break,
        };
        let tag_content = &xml_str[tag_start..tag_end];

        let close_tag = match xml_str[tag_end..].find("</text>") {
            Some(p) => tag_end + p,
            None => break,
        };
        let inner_text = &xml_str[tag_end + 1..close_tag];
        cursor = close_tag + "</text>".len();

        let clean_text = decode_html_entities(inner_text);

        let seconds = if let Some(start_pos) = tag_content.find("start=\"") {
            let after = &tag_content[start_pos + "start=\"".len()..];
            if let Some(quote_end) = after.find('"') {
                after[..quote_end].parse::<f64>().unwrap_or(0.0) as u64
            } else {
                0
            }
        } else {
            0
        };

        builder.add_cue(seconds, &clean_text);
    }

    builder.finish()
}

async fn download_and_parse_captions(client: &Client, url: &str) -> Option<String> {
    let resp = client
        .get(url)
        .header("User-Agent", "Mozilla/5.0")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let body = crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES)
        .await
        .ok()?;
    parse_json3_timedtext(&body).or_else(|| parse_xml_timedtext(&body))
}

/// Fetches and extracts transcript and metadata for a YouTube video.
pub async fn fetch_youtube_transcript(client: &Client, video_id: &str) -> Result<ScrapedPage> {
    let watch_url = format!("https://www.youtube.com/watch?v={}", video_id);

    // 1. Fetch watch page HTML
    let resp = client
        .get(&watch_url)
        .header(
            "User-Agent",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36",
        )
        .header("Accept-Language", "en-US,en;q=0.9")
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .with_context(|| format!("Failed to reach YouTube for video ID: {}", video_id))?;

    if !resp.status().is_success() {
        anyhow::bail!("YouTube watch page returned HTTP status: {}", resp.status());
    }

    let html = crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES)
        .await
        .unwrap_or_default();
    let title = extract_video_title(&html).unwrap_or_else(|| format!("YouTube Video ({})", video_id));

    // 2. Extract caption tracks from watch page
    let tracks = extract_caption_tracks(&html);

    let chosen_url = if let Some(ref list) = tracks {
        // Prefer English track if available, else first track
        let english_track = list
            .iter()
            .find(|t| t.language_code.starts_with("en"))
            .or_else(|| list.first());
        english_track.map(|t| t.base_url.clone())
    } else {
        None
    };

    // 3. Download and parse captions
    let transcript = if let Some(base_url) = chosen_url {
        let json_url = if base_url.contains("fmt=") {
            base_url
        } else {
            format!("{}&fmt=json3", base_url)
        };
        download_and_parse_captions(client, &json_url).await
    } else {
        let fallback_url = format!(
            "https://www.youtube.com/api/timedtext?v={}&lang=en&fmt=json3",
            video_id
        );
        download_and_parse_captions(client, &fallback_url).await
    };

    let content = transcript.context(format!(
        "No English closed captions or transcript found for YouTube video '{}' (ID: {}). The video creator may have disabled subtitles.",
        title, video_id
    ))?;

    let word_count = content.split_whitespace().count();

    Ok(ScrapedPage {
        url: watch_url,
        title: Some(title),
        content,
        word_count,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_youtube_video_id() {
        assert_eq!(
            extract_youtube_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_youtube_video_id("https://youtube.com/watch?v=dQw4w9WgXcQ&t=42s"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_youtube_video_id("https://youtu.be/dQw4w9WgXcQ?si=12345"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_youtube_video_id("https://m.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_youtube_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(
            extract_youtube_video_id("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            Some("dQw4w9WgXcQ")
        );
        assert_eq!(extract_youtube_video_id("https://example.com/article"), None);
        assert_eq!(extract_youtube_video_id("Dune"), None);
    }

    #[test]
    fn test_parse_json3_timedtext() {
        let json = r#"{
            "events": [
                {
                    "tStartMs": 1000,
                    "segs": [{"utf8": "Hello "}, {"utf8": "world"}]
                },
                {
                    "tStartMs": 65000,
                    "segs": [{"utf8": "Next minute topic"}]
                }
            ]
        }"#;

        let parsed = parse_json3_timedtext(json).unwrap();
        assert!(parsed.contains("[00:01] Hello world"));
        assert!(parsed.contains("[01:05] Next minute topic"));
    }

    #[test]
    fn test_parse_xml_timedtext() {
        let xml = r#"<transcript>
            <text start="2.5" dur="3.0">Welcome to &amp; the course</text>
            <text start="62.0" dur="4.0">Second minute begins</text>
        </transcript>"#;

        let parsed = parse_xml_timedtext(xml).unwrap();
        assert!(parsed.contains("[00:02] Welcome to & the course"));
        assert!(parsed.contains("[01:02] Second minute begins"));
    }

    #[test]
    fn test_extract_video_title() {
        let html = "<html><head><title>Rust in 100 Seconds - YouTube</title></head></html>";
        assert_eq!(extract_video_title(html), Some("Rust in 100 Seconds".to_string()));
    }

    #[test]
    fn test_decode_html_entities_single_pass_and_zero_copy() {
        let plain = "Plain subtitle cue without entities";
        assert!(matches!(
            decode_html_entities(plain),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(decode_html_entities("A &amp;lt; B &quot;ok&quot;"), "A &lt; B \"ok\"");
    }
}
