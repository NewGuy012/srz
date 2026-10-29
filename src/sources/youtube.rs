use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;

use crate::scraper::ScrapedPage;

const ANDROID_USER_AGENT: &str = "com.google.android.youtube/20.10.38";

#[derive(Deserialize, Debug, Clone)]
pub struct CaptionTrack {
    #[serde(rename = "baseUrl")]
    pub base_url: String,
    #[serde(rename = "languageCode", default)]
    pub language_code: String,
    #[serde(default)]
    pub kind: Option<String>,
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

#[derive(Deserialize, Debug)]
struct InnertubePlayerResponse {
    #[serde(rename = "videoDetails")]
    video_details: Option<InnertubeVideoDetails>,
    captions: Option<InnertubeCaptions>,
}

#[derive(Deserialize, Debug)]
struct InnertubeVideoDetails {
    title: Option<String>,
    #[allow(dead_code)]
    author: Option<String>,
}

#[derive(Deserialize, Debug)]
struct InnertubeCaptions {
    #[serde(rename = "playerCaptionsTracklistRenderer")]
    renderer: Option<InnertubeTracklistRenderer>,
}

#[derive(Deserialize, Debug)]
struct InnertubeTracklistRenderer {
    #[serde(rename = "captionTracks", default)]
    caption_tracks: Vec<CaptionTrack>,
}

pub struct ParsedPlayerData {
    pub title: Option<String>,
    pub caption_tracks: Option<Vec<CaptionTrack>>,
}

/// Checks if a URL is a YouTube video link and returns its 11-character video ID.
pub fn extract_youtube_video_id(url: &str) -> Option<&str> {
    let trimmed = url.trim();
    if !trimmed.starts_with("http://") && !trimmed.starts_with("https://") {
        return None;
    }

    const PATTERNS: &[&str] = &["youtu.be/", "/shorts/", "/embed/", "v="];
    for &pat in PATTERNS {
        if let Some(pos) = trimmed.find(pat) {
            let after = &trimmed[pos + pat.len()..];
            let id = after.split(['?', '&', '/', '#']).next().unwrap_or("");
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

/// Parses XML timed text format 1 `<text start="1.23" dur="4.56">...</text>` into formatted paragraphs.
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

        let seconds = crate::sources::news::extract_attr_value(tag_content, "start=")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0) as u64;

        builder.add_cue(seconds, &clean_text);
    }

    builder.finish()
}

/// Parses XML timed text format 3 (`<p t="160" d="4000"><s>word</s>...</p>`) into formatted paragraphs.
pub fn parse_xml_format3_timedtext(xml_str: &str) -> Option<String> {
    if !xml_str.contains("<p") {
        return None;
    }

    let mut builder = TimedTranscriptBuilder::new(xml_str.len() / 2);
    let mut cursor = 0;

    while let Some(pos) = xml_str[cursor..].find("<p") {
        let tag_start = cursor + pos;
        let after_p = tag_start + 2;
        let Some(next_ch) = xml_str[after_p..].chars().next() else {
            break;
        };
        if !next_ch.is_ascii_whitespace() && next_ch != '>' && next_ch != '/' {
            cursor = after_p;
            continue;
        }

        let tag_open_end = match xml_str[after_p..].find('>') {
            Some(p) => after_p + p,
            None => break,
        };

        // Skip self-closing <p ... />
        if xml_str[tag_start..tag_open_end].trim_end().ends_with('/') {
            cursor = tag_open_end + 1;
            continue;
        }

        let tag_header = &xml_str[tag_start..tag_open_end];

        let close_tag = match xml_str[tag_open_end + 1..].find("</p>") {
            Some(p) => tag_open_end + 1 + p,
            None => break,
        };

        let inner_xml = &xml_str[tag_open_end + 1..close_tag];
        cursor = close_tag + "</p>".len();

        // Extract t="..." in milliseconds
        let seconds = extract_attr_u64(tag_header, "t")
            .map(|ms| ms / 1000)
            .unwrap_or(0);

        let stripped = strip_xml_tags(inner_xml);
        let decoded = decode_html_entities(&stripped);
        let clean = decoded.trim();
        if !clean.is_empty() {
            builder.add_cue(seconds, clean);
        }
    }

    builder.finish()
}

pub fn extract_attr_u64(tag_content: &str, attr_name: &str) -> Option<u64> {
    let prefix = format!("{}=", attr_name);
    crate::sources::news::extract_attr_value(tag_content, &prefix)
        .and_then(|v| v.parse().ok())
}

pub fn strip_xml_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for c in input.chars() {
        if c == '<' {
            in_tag = true;
        } else if c == '>' {
            in_tag = false;
        } else if !in_tag {
            out.push(c);
        }
    }
    out
}

/// Dispatches timed text payloads to the appropriate parser based on payload structure.
pub fn parse_timedtext(body: &str) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('{')
        && let Some(res) = parse_json3_timedtext(trimmed)
    {
        return Some(res);
    }
    if trimmed.contains("<p")
        && let Some(res) = parse_xml_format3_timedtext(trimmed)
    {
        return Some(res);
    }
    if trimmed.contains("<text")
        && let Some(res) = parse_xml_timedtext(trimmed)
    {
        return Some(res);
    }
    None
}

/// Queries YouTube's internal Innertube Player API for video metadata and caption tracks.
pub async fn fetch_innertube_player(
    client: &Client,
    video_id: &str,
    client_name: &str,
    client_version: &str,
) -> Result<ParsedPlayerData> {
    let endpoint = "https://www.youtube.com/youtubei/v1/player";
    let body = serde_json::json!({
        "context": {
            "client": {
                "clientName": client_name,
                "clientVersion": client_version,
                "hl": "en",
                "gl": "US"
            }
        },
        "videoId": video_id
    });

    let resp = client
        .post(endpoint)
        .header("Content-Type", "application/json")
        .header(
            "User-Agent",
            format!("com.google.android.youtube/{}", client_version),
        )
        .json(&body)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .context("Failed to send player request")?;

    if !resp.status().is_success() {
        anyhow::bail!("Player API returned status {}", resp.status());
    }

    let text = crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES).await?;
    let player: InnertubePlayerResponse =
        serde_json::from_str(&text).context("Failed to parse player JSON")?;

    let title = player.video_details.and_then(|vd| vd.title);
    let tracks = player
        .captions
        .and_then(|c| c.renderer.map(|r| r.caption_tracks));

    Ok(ParsedPlayerData {
        title,
        caption_tracks: tracks,
    })
}

async fn download_and_parse_captions(client: &Client, url: &str) -> Option<String> {
    let mut candidate_urls = vec![url.to_string()];
    if !url.contains("fmt=") {
        candidate_urls.push(format!("{}&fmt=json3", url));
    }

    let uas = [ANDROID_USER_AGENT, crate::scraper::DEFAULT_USER_AGENT];
    for u in &candidate_urls {
        for ua in uas {
            if let Some(parsed) = fetch_single_caption_url(client, u, ua).await {
                return Some(parsed);
            }
        }
    }

    None
}

async fn fetch_single_caption_url(client: &Client, url: &str, user_agent: &str) -> Option<String> {
    let resp = client
        .get(url)
        .header("User-Agent", user_agent)
        .header("Accept-Language", "en-US,en;q=0.9")
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

    if body.trim().is_empty() {
        return None;
    }

    parse_timedtext(&body)
}

/// Fetches and extracts transcript and metadata for a YouTube video.
pub async fn fetch_youtube_transcript(client: &Client, video_id: &str) -> Result<ScrapedPage> {
    let watch_url = format!("https://www.youtube.com/watch?v={}", video_id);

    // 1. First, attempt to fetch video details and caption tracks via Innertube Android Player API
    let mut title: Option<String> = None;
    let mut tracks: Option<Vec<CaptionTrack>> = None;

    if let Ok(player_data) = fetch_innertube_player(client, video_id, "ANDROID", "20.10.38").await {
        if let Some(t) = player_data.title {
            title = Some(t);
        }
        if let Some(t_list) = player_data.caption_tracks
            && !t_list.is_empty()
        {
            tracks = Some(t_list);
        }
    }

    // 2. Fallback to Innertube WEB Player API if tracks were not found
    if tracks.is_none()
        && let Ok(player_data) = fetch_innertube_player(client, video_id, "WEB", "2.20240101.00.00").await
    {
        if title.is_none() && player_data.title.is_some() {
            title = player_data.title;
        }
        if let Some(t_list) = player_data.caption_tracks
            && !t_list.is_empty()
        {
            tracks = Some(t_list);
        }
    }

    // 3. Fallback to watch page HTML if tracks or title are still missing
    if (tracks.is_none() || title.is_none())
        && let Ok(resp) = client
            .get(&watch_url)
            .header(
                "User-Agent",
                crate::scraper::DEFAULT_USER_AGENT,
            )
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(Duration::from_secs(10))
            .send()
            .await
        && resp.status().is_success()
        && let Ok(html) = crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES).await
    {
        if title.is_none() {
            title = extract_video_title(&html);
        }
        if tracks.is_none() {
            tracks = extract_caption_tracks(&html);
        }
    }

    let resolved_title = title.unwrap_or_else(|| format!("YouTube Video ({})", video_id));

    // 4. Select the best caption track (preferring English, human-authored over ASR)
    let chosen_url = if let Some(ref list) = tracks {
        let best_track = list
            .iter()
            .find(|t| t.language_code.starts_with("en") && t.kind.as_deref() != Some("asr"))
            .or_else(|| list.iter().find(|t| t.language_code.starts_with("en")))
            .or_else(|| list.first());
        best_track.map(|t| t.base_url.clone())
    } else {
        None
    };

    // 5. Download and parse captions
    let transcript = if let Some(base_url) = chosen_url {
        download_and_parse_captions(client, &base_url).await
    } else {
        let fallback_url = format!(
            "https://www.youtube.com/api/timedtext?v={}&lang=en&fmt=json3",
            video_id
        );
        download_and_parse_captions(client, &fallback_url).await
    };

    let content = transcript.context(format!(
        "No English closed captions or transcript found for YouTube video '{}' (ID: {}). The video creator may have disabled subtitles.",
        resolved_title, video_id
    ))?;

    let word_count = content.split_whitespace().count();

    Ok(ScrapedPage {
        url: watch_url,
        title: Some(resolved_title),
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
    fn test_parse_xml_format3_timedtext() {
        let xml = r#"<?xml version="1.0" encoding="utf-8" ?><timedtext format="3">
        <head></head>
        <body>
            <p t="160" d="4000"><s ac="0">rust</s><s t="680" ac="0"> a</s><s t="840" ac="0"> memory</s><s t="1279" ac="0"> safe</s></p>
            <p t="65000" d="3000"><s ac="0">second</s><s t="500"> minute</s></p>
        </body>
        </timedtext>"#;

        let parsed = parse_xml_format3_timedtext(xml).unwrap();
        assert!(parsed.contains("[00:00] rust a memory safe"));
        assert!(parsed.contains("[01:05] second minute"));
    }

    #[test]
    fn test_parse_timedtext_dispatch() {
        let json = r#"{"events": [{"tStartMs": 2000, "segs": [{"utf8": "json format"}]}]}"#;
        assert!(parse_timedtext(json).unwrap().contains("[00:02] json format"));

        let format1 = r#"<transcript><text start="3.0">format 1</text></transcript>"#;
        assert!(parse_timedtext(format1).unwrap().contains("[00:03] format 1"));

        let format3 = r#"<timedtext format="3"><body><p t="4000"><s>format 3</s></p></body></timedtext>"#;
        assert!(parse_timedtext(format3).unwrap().contains("[00:04] format 3"));
    }

    #[test]
    fn test_strip_xml_tags() {
        let s = "<s ac=\"0\">rust</s><s t=\"680\"> a</s> &amp; <b>code</b>";
        assert_eq!(strip_xml_tags(s), "rust a &amp; code");
    }

    #[test]
    fn test_extract_attr_u64() {
        assert_eq!(extract_attr_u64(r#"<p t="160" d="4000">"#, "t"), Some(160));
        assert_eq!(extract_attr_u64(r#"<p d="4000" t='65000'>"#, "t"), Some(65000));
        assert_eq!(extract_attr_u64(r#"<p>"#, "t"), None);
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
