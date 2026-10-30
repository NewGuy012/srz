use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use reqwest::{Client, Url};
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

    let parsed = Url::parse(trimmed).ok()?;
    let host = parsed.host_str()?;
    let is_youtube_host =
        host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be";

    if !is_youtube_host {
        return None;
    }

    // For youtu.be/<id>
    if host == "youtu.be" {
        let path = parsed.path().trim_start_matches('/');
        let id = path.split('/').next().unwrap_or("");
        return extract_slice_after(trimmed, "youtu.be/", id);
    }

    // For youtube.com / *.youtube.com:
    // 1. Check query parameter v=<id>
    if let Some((_, v)) = parsed.query_pairs().find(|(k, _)| k == "v")
        && let Some(id_slice) = extract_slice_after(trimmed, "v=", &v)
    {
        return Some(id_slice);
    }

    // 2. Check path prefixes: /shorts/<id>, /embed/<id>, /v/<id>
    let path = parsed.path();
    for prefix in &["/shorts/", "/embed/", "/v/"] {
        if let Some(after) = path.strip_prefix(prefix) {
            let id = after.split('/').next().unwrap_or("");
            if let Some(id_slice) = extract_slice_after(trimmed, prefix, id) {
                return Some(id_slice);
            }
        }
    }

    None
}

fn extract_slice_after<'a>(haystack: &'a str, prefix: &str, id: &str) -> Option<&'a str> {
    if !is_valid_youtube_id(id) {
        return None;
    }
    for (pos, _) in haystack.match_indices(prefix) {
        let start = pos + prefix.len();
        if haystack.get(start..start + id.len()) == Some(id) {
            return Some(&haystack[start..start + id.len()]);
        }
    }
    None
}

fn is_valid_youtube_id(id: &str) -> bool {
    id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
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

/// Formats seconds into `[MM:SS]` or `[HH:MM:SS]`.
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

pub use crate::system::text_util::decode_html_entities;

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

    let mut reader = Reader::from_str(xml_str);

    let mut builder = TimedTranscriptBuilder::new(xml_str.len() / 2);
    let mut current_start = 0.0;

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == "text" => {
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == "start" {
                        current_start = attr.value.parse::<f64>().unwrap_or(0.0);
                    }
                }
                if let Ok(text) = reader.read_text(e.name()) {
                    let decoded = decode_html_entities(text.as_ref());
                    let trimmed = decoded.trim();
                    if !trimmed.is_empty() {
                        builder.add_cue(current_start as u64, trimmed);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
    }

    builder.finish()
}

/// Parses XML timed text format 3 (`<p t="160" d="4000"><s>word</s>...</p>`) into formatted paragraphs.
pub fn parse_xml_format3_timedtext(xml_str: &str) -> Option<String> {
    if !xml_str.contains("<p") {
        return None;
    }

    let mut reader = Reader::from_str(xml_str);

    let mut builder = TimedTranscriptBuilder::new(xml_str.len() / 2);
    let mut current_sec = 0;
    let mut in_p = false;
    let mut p_text = String::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(e)) if e.name().as_ref() == "p" => {
                in_p = true;
                p_text.clear();
                for attr in e.attributes().flatten() {
                    if attr.key.as_ref() == "t" {
                        let ms = attr.value.parse::<u64>().unwrap_or(0);
                        current_sec = ms / 1000;
                    }
                }
            }
            Ok(Event::Text(e)) if in_p => {
                let decoded = decode_html_entities(e.as_ref());
                p_text.push_str(&decoded);
            }
            Ok(Event::End(e)) if e.name().as_ref() == "p" => {
                in_p = false;
                let trimmed = p_text.trim();
                if !trimmed.is_empty() {
                    builder.add_cue(current_sec, trimmed);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => return None,
            _ => {}
        }
    }

    builder.finish()
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

    let text =
        crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES).await?;
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

/// Validates that a caption track URL belongs to the official YouTube timedtext API to prevent SSRF.
pub fn is_safe_caption_url(url_str: &str) -> bool {
    let Ok(parsed) = Url::parse(url_str) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    let is_youtube = host == "youtube.com" || host.ends_with(".youtube.com");
    is_youtube && parsed.path().starts_with("/api/timedtext")
}

async fn download_and_parse_captions(client: &Client, url: &str) -> Option<String> {
    if !is_safe_caption_url(url) {
        return None;
    }

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

    // 1. Attempt to fetch video details and caption tracks via Innertube Player APIs (Android, then Web)
    let mut title: Option<String> = None;
    let mut tracks: Option<Vec<CaptionTrack>> = None;

    const INNERTUBE_CLIENTS: [(&str, &str); 2] =
        [("ANDROID", "20.10.38"), ("WEB", "2.20240101.00.00")];

    for (client_name, client_version) in INNERTUBE_CLIENTS {
        if tracks.is_some() && title.is_some() {
            break;
        }
        if let Ok(player_data) =
            fetch_innertube_player(client, video_id, client_name, client_version).await
        {
            if title.is_none() && player_data.title.is_some() {
                title = player_data.title;
            }
            if tracks.is_none()
                && let Some(t_list) = player_data.caption_tracks
                && !t_list.is_empty()
            {
                tracks = Some(t_list);
            }
        }
    }

    // 2. Fallback to watch page HTML if tracks or title are still missing
    if (tracks.is_none() || title.is_none())
        && let Ok(resp) = client
            .get(&watch_url)
            .header("User-Agent", crate::scraper::DEFAULT_USER_AGENT)
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(Duration::from_secs(10))
            .send()
            .await
        && resp.status().is_success()
        && let Ok(html) =
            crate::scraper::read_response_text_limited(resp, crate::scraper::MAX_SCRAPE_BYTES).await
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
        assert_eq!(
            extract_youtube_video_id("https://example.com/article"),
            None
        );
        assert_eq!(extract_youtube_video_id("Dune"), None);

        // Security regression: ensure non-YouTube hosts with YouTube paths/params are rejected
        assert_eq!(
            extract_youtube_video_id("https://evil.com/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            extract_youtube_video_id("https://evil.com/youtu.be/dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            extract_youtube_video_id("https://attacker.com/shorts/dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            extract_youtube_video_id("https://notyoutube.com/watch?v=dQw4w9WgXcQ"),
            None
        );
        assert_eq!(
            extract_youtube_video_id("https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ"),
            None
        );
    }

    #[test]
    fn test_is_safe_caption_url() {
        assert!(is_safe_caption_url(
            "https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ&lang=en"
        ));
        assert!(is_safe_caption_url(
            "https://youtube.com/api/timedtext?v=dQw4w9WgXcQ"
        ));
        assert!(!is_safe_caption_url(
            "http://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ"
        )); // insecure HTTP
        assert!(!is_safe_caption_url(
            "https://evil.com/api/timedtext?v=dQw4w9WgXcQ"
        )); // evil domain
        assert!(!is_safe_caption_url(
            "http://169.254.169.254/latest/meta-data"
        )); // cloud metadata SSRF
        assert!(!is_safe_caption_url(
            "https://www.youtube.com/account_settings"
        )); // non-caption path
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
        assert!(
            parse_timedtext(json)
                .unwrap()
                .contains("[00:02] json format")
        );

        let format1 = r#"<transcript><text start="3.0">format 1</text></transcript>"#;
        assert!(
            parse_timedtext(format1)
                .unwrap()
                .contains("[00:03] format 1")
        );

        let format3 =
            r#"<timedtext format="3"><body><p t="4000"><s>format 3</s></p></body></timedtext>"#;
        assert!(
            parse_timedtext(format3)
                .unwrap()
                .contains("[00:04] format 3")
        );
    }

    #[test]
    fn test_extract_video_title() {
        let html = "<html><head><title>Rust in 100 Seconds - YouTube</title></head></html>";
        assert_eq!(
            extract_video_title(html),
            Some("Rust in 100 Seconds".to_string())
        );
    }

    #[test]
    fn test_decode_html_entities_single_pass_and_zero_copy() {
        let plain = "Plain subtitle cue without entities";
        assert!(matches!(
            decode_html_entities(plain),
            std::borrow::Cow::Borrowed(_)
        ));
        assert_eq!(
            decode_html_entities("A &amp;lt; B &quot;ok&quot;"),
            "A &lt; B \"ok\""
        );
    }
}
