use crate::scraper::{
    ScrapedPage, collapse_whitespace, extract_title_from_html, fetch_text_limited,
};
use crate::system::text_util::{XmlEvent, XmlScanner, xml_attr, xml_local_name};
use anyhow::{Context, Result};
use reqwest::{Client, Url};
use serde::Deserialize;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::time::Duration;

const ANDROID_USER_AGENT: &str = "com.google.android.youtube/20.10.38";

#[derive(Deserialize, Debug, Clone)]
struct CaptionTrack {
    #[serde(rename = "baseUrl")]
    base_url: String,
    #[serde(rename = "languageCode", default)]
    language_code: String,
    #[serde(default)]
    kind: Option<String>,
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
    #[serde(rename = "shortDescription")]
    short_description: Option<String>,
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

struct ParsedPlayerData {
    title: Option<String>,
    caption_tracks: Option<Vec<CaptionTrack>>,
    description: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VideoChapter {
    seconds: u64,
    title: String,
}

/// Checks if a URL is a YouTube video link and returns its 11-character video ID.
pub fn extract_youtube_video_id(url: &str) -> Option<&str> {
    let trimmed = url.trim();
    if !crate::system::text_util::is_http_url(trimmed) {
        return None;
    }

    let parsed = Url::parse(trimmed).ok()?;
    let host = parsed.host_str()?;
    let is_youtube_host =
        host == "youtube.com" || host.ends_with(".youtube.com") || host == "youtu.be";

    if !is_youtube_host {
        return None;
    }

    if host == "youtu.be" {
        let id = parsed
            .path()
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        return extract_slice_after_ignore_case(trimmed, "youtu.be/", id);
    }

    if let Some((_, v)) = parsed.query_pairs().find(|(k, _)| k == "v")
        && let Some(id_slice) = extract_slice_after_ignore_case(trimmed, "v=", &v)
    {
        return Some(id_slice);
    }

    let path = parsed.path();
    for prefix in &["/shorts/", "/embed/", "/v/", "/live/"] {
        if let Some(after) = path.strip_prefix(prefix) {
            let id = after.split('/').next().unwrap_or("");
            if let Some(id_slice) = extract_slice_after_ignore_case(trimmed, prefix, id) {
                return Some(id_slice);
            }
        }
    }

    None
}

fn extract_slice_after_ignore_case<'a>(
    haystack: &'a str,
    prefix: &str,
    id: &str,
) -> Option<&'a str> {
    if !is_valid_youtube_id(id) {
        return None;
    }
    let h_bytes = haystack.as_bytes();
    let p_bytes = prefix.as_bytes();
    let mut offset = 0;
    while let Some(rel) =
        crate::system::text_util::find_subslice_ignore_case(&h_bytes[offset..], p_bytes)
    {
        let start = offset + rel + p_bytes.len();
        if haystack.get(start..start + id.len()) == Some(id) {
            return Some(&haystack[start..start + id.len()]);
        }
        offset = offset + rel + 1;
    }
    None
}

fn is_valid_youtube_id(id: &str) -> bool {
    id.len() == 11
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn extract_json_after_key<T: serde::de::DeserializeOwned>(
    html: &str,
    key: &str,
    expected_prefix: char,
) -> Option<T> {
    for (pos, _) in html.match_indices(key) {
        let after = html[pos + key.len()..].trim_start();
        if after.starts_with(expected_prefix)
            && let Some(res) = serde_json::Deserializer::from_str(after)
                .into_iter::<T>()
                .next()
            && let Ok(val) = res
        {
            return Some(val);
        }
    }
    None
}

fn extract_caption_tracks(html: &str) -> Option<Vec<CaptionTrack>> {
    extract_json_after_key(html, "\"captionTracks\":", '[')
}

fn extract_video_title(html: &str) -> Option<String> {
    extract_title_from_html(html).and_then(|t| {
        let clean = t
            .strip_suffix(" - YouTube")
            .unwrap_or(&t)
            .trim()
            .to_string();
        (!clean.is_empty()).then_some(clean)
    })
}

fn extract_video_description(html: &str) -> Option<String> {
    extract_json_after_key(html, "\"shortDescription\":", '"')
}

fn format_timestamp(seconds: u64) -> String {
    let hrs = seconds / 3600;
    let mins = (seconds % 3600) / 60;
    let secs = seconds % 60;
    if hrs > 0 {
        format!("[{hrs:02}:{mins:02}:{secs:02}]")
    } else {
        format!("[{mins:02}:{secs:02}]")
    }
}

fn parse_chapters_from_description(desc: &str) -> Vec<VideoChapter> {
    let mut chapters = Vec::new();

    for line in desc.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some((secs, title)) =
            parse_chapter_line(trimmed, true).or_else(|| parse_chapter_line(trimmed, false))
        {
            chapters.push(VideoChapter {
                seconds: secs,
                title,
            });
        }
    }

    if chapters.len() < 2 {
        return Vec::new();
    }

    chapters.sort_by_key(|c| c.seconds);
    chapters.dedup_by_key(|c| c.seconds);

    if chapters.first().is_some_and(|c| c.seconds <= 10) {
        chapters
    } else {
        Vec::new()
    }
}

fn is_chapter_sep(c: char) -> bool {
    matches!(c, '-' | '–' | '—' | ':' | '.' | ' ')
}

fn parse_chapter_line(line: &str, at_start: bool) -> Option<(u64, String)> {
    let token = if at_start {
        line.split_whitespace().next()?
    } else {
        line.split_whitespace().last()?
    };
    let secs = parse_timestamp_token(token)?;
    let remainder = if at_start {
        line[token.len()..]
            .trim()
            .trim_start_matches(is_chapter_sep)
    } else {
        line[..line.len() - token.len()]
            .trim()
            .trim_end_matches(is_chapter_sep)
    };
    let clean_title = remainder.trim();
    (!clean_title.is_empty()).then(|| (secs, clean_title.to_string()))
}

fn parse_timestamp_token(token: &str) -> Option<u64> {
    let clean = token.trim_matches(|c: char| matches!(c, '[' | ']' | '(' | ')' | '{' | '}'));
    let mut parts = clean.split(':');
    let first = parts.next()?;
    let second = parts.next()?;
    let third = parts.next();
    if parts.next().is_some() {
        return None;
    }

    match third {
        None => {
            if second.len() != 2 {
                return None;
            }
            let m: u64 = first.parse().ok()?;
            let s: u64 = second.parse().ok()?;
            (s < 60)
                .then(|| m.checked_mul(60)?.checked_add(s))
                .flatten()
        }
        Some(sec_str) => {
            if second.len() != 2 || sec_str.len() != 2 {
                return None;
            }
            let h: u64 = first.parse().ok()?;
            let m: u64 = second.parse().ok()?;
            let s: u64 = sec_str.parse().ok()?;
            (m < 60 && s < 60)
                .then(|| h.checked_mul(3600)?.checked_add(m * 60)?.checked_add(s))
                .flatten()
        }
    }
}

struct TimedTranscriptBuilder<'a> {
    result: String,
    current_interval: u64,
    current_line: String,
    chapters: &'a [VideoChapter],
    current_chapter_idx: usize,
}

impl<'a> TimedTranscriptBuilder<'a> {
    fn with_chapters(capacity: usize, chapters: &'a [VideoChapter]) -> Self {
        Self {
            result: String::with_capacity(capacity),
            current_interval: u64::MAX,
            current_line: String::new(),
            chapters,
            current_chapter_idx: usize::MAX,
        }
    }

    fn flush_line(&mut self) {
        if !self.current_line.is_empty() {
            self.result.push_str(&self.current_line);
            self.result.push_str("\n\n");
            self.current_line.clear();
        }
    }

    fn add_cue(&mut self, seconds: u64, text: &str) {
        let clean: Cow<'_, str> = if text.contains('\n') {
            Cow::Owned(text.replace('\n', " "))
        } else {
            Cow::Borrowed(text)
        };
        let trimmed = clean.trim();
        if trimmed.is_empty()
            || ["[music]", "[applause]", "[laughter]", "[cheering]"]
                .iter()
                .any(|tag| trimmed.eq_ignore_ascii_case(tag))
        {
            return;
        }

        if !self.chapters.is_empty() {
            let mut active_idx = if self.current_chapter_idx != usize::MAX {
                self.current_chapter_idx
            } else {
                0
            };
            while active_idx + 1 < self.chapters.len()
                && seconds >= self.chapters[active_idx + 1].seconds
            {
                active_idx += 1;
            }

            if active_idx != self.current_chapter_idx {
                self.flush_line();
                self.current_chapter_idx = active_idx;
                let ch = &self.chapters[active_idx];
                let _ = writeln!(
                    self.result,
                    "{} ## {}",
                    format_timestamp(ch.seconds),
                    ch.title
                );
            }
        } else {
            let interval = seconds / 60;
            if interval != self.current_interval {
                self.flush_line();
                self.current_interval = interval;
                self.current_line.push_str(&format_timestamp(seconds));
            }
        }
        if !self.current_line.is_empty() && !self.current_line.ends_with(' ') {
            self.current_line.push(' ');
        }
        self.current_line.push_str(trimmed);
    }

    fn finish(mut self) -> Option<String> {
        if !self.current_line.is_empty() {
            self.result.push_str(&self.current_line);
            self.result.push('\n');
        }
        (!self.result.trim().is_empty()).then_some(self.result)
    }
}

fn parse_json3_timedtext(json_str: &str, chapters: &[VideoChapter]) -> Option<String> {
    let parsed: TimedTextJson = serde_json::from_str(json_str).ok()?;
    if parsed.events.is_empty() {
        return None;
    }

    let mut builder = TimedTranscriptBuilder::with_chapters(json_str.len() / 2, chapters);
    let mut combined_buf = String::new();

    for ev in parsed.events {
        let Some(segs) = ev.segs.as_deref() else {
            continue;
        };
        combined_buf.clear();
        for seg in segs {
            combined_buf.push_str(&seg.utf8);
        }
        if !combined_buf.trim().is_empty() {
            builder.add_cue(ev.start_ms / 1000, &combined_buf);
        }
    }

    builder.finish()
}

fn parse_xml_cues(
    xml_str: &str,
    target_tag: &str,
    attr_name: &str,
    in_millis: bool,
    chapters: &[VideoChapter],
) -> Option<String> {
    let mut scanner = XmlScanner::new(xml_str);
    let mut builder = TimedTranscriptBuilder::with_chapters(xml_str.len() / 2, chapters);
    while let Some(ev) = scanner.next_event() {
        if let XmlEvent::Start {
            name,
            attrs,
            is_empty: false,
        } = ev
            && xml_local_name(name).eq_ignore_ascii_case(target_tag)
        {
            let Some(secs) = xml_attr(attrs, attr_name)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
                .map(|v| (if in_millis { v / 1000.0 } else { v }) as u64)
            else {
                let _ = scanner.read_text(name);
                continue;
            };
            let cleaned = collapse_whitespace(&scanner.read_text(name));
            builder.add_cue(secs, &cleaned);
        }
    }
    builder.finish()
}

fn parse_timedtext(body: &str, chapters: &[VideoChapter]) -> Option<String> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('{')
        && let Some(res) = parse_json3_timedtext(trimmed, chapters)
    {
        return Some(res);
    }
    if trimmed.contains("<p")
        && let Some(res) = parse_xml_cues(trimmed, "p", "t", true, chapters)
    {
        return Some(res);
    }
    if trimmed.contains("<text")
        && let Some(res) = parse_xml_cues(trimmed, "text", "start", false, chapters)
    {
        return Some(res);
    }
    None
}

async fn fetch_innertube_player(
    client: &Client,
    video_id: &str,
    client_name: &str,
    client_version: &str,
) -> Result<ParsedPlayerData> {
    if !is_valid_youtube_id(video_id) {
        anyhow::bail!("Invalid YouTube video ID");
    }
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
    let ua = if client_name == "ANDROID" {
        Cow::Owned(format!("com.google.android.youtube/{client_version}"))
    } else {
        Cow::Borrowed(crate::scraper::DEFAULT_USER_AGENT)
    };

    let text = fetch_text_limited(
        client
            .post(endpoint)
            .header("Content-Type", "application/json")
            .header("User-Agent", ua.as_ref())
            .body(serde_json::to_vec(&body).context("Failed to serialize player request")?)
            .timeout(Duration::from_secs(10)),
        crate::scraper::MAX_SCRAPE_BYTES,
        "YouTube Player API",
    )
    .await?;
    let player: InnertubePlayerResponse =
        serde_json::from_str(&text).context("Failed to parse player JSON")?;

    let title = player
        .video_details
        .as_ref()
        .and_then(|vd| vd.title.clone());
    let description = player
        .video_details
        .as_ref()
        .and_then(|vd| vd.short_description.clone());
    let tracks = player
        .captions
        .and_then(|c| c.renderer.map(|r| r.caption_tracks));

    Ok(ParsedPlayerData {
        title,
        caption_tracks: tracks,
        description,
    })
}

fn is_safe_caption_url(url_str: &str) -> bool {
    let Ok(parsed) = Url::parse(url_str) else {
        return false;
    };
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.port().is_some()
    {
        return false;
    }
    let Some(host) = parsed.host_str() else {
        return false;
    };
    (host == "youtube.com" || host.ends_with(".youtube.com")) && parsed.path() == "/api/timedtext"
}

fn append_query_param(url: &str, key: &str, value: &str) -> String {
    let Ok(mut parsed) = Url::parse(url) else {
        return url.to_string();
    };
    if !parsed.query_pairs().any(|(k, _)| k == key) {
        parsed.query_pairs_mut().append_pair(key, value);
    }
    parsed.into()
}

async fn download_and_parse_captions(
    client: &Client,
    url: &str,
    chapters: &[VideoChapter],
) -> Option<String> {
    if !is_safe_caption_url(url) {
        return None;
    }

    let json3_url = append_query_param(url, "fmt", "json3");
    let mut attempts: Vec<(&str, &str)> = vec![
        (&json3_url, ANDROID_USER_AGENT),
        (&json3_url, crate::scraper::DEFAULT_USER_AGENT),
    ];
    if json3_url != url {
        attempts.push((url, crate::scraper::DEFAULT_USER_AGENT));
    }

    for (u, ua) in attempts {
        if let Some(parsed) = fetch_single_caption_url(client, u, ua, chapters).await {
            return Some(parsed);
        }
    }

    None
}

async fn fetch_single_caption_url(
    client: &Client,
    url: &str,
    user_agent: &str,
    chapters: &[VideoChapter],
) -> Option<String> {
    let body = fetch_text_limited(
        client
            .get(url)
            .header("User-Agent", user_agent)
            .header("Accept-Language", "en-US,en;q=0.9")
            .timeout(Duration::from_secs(10)),
        crate::scraper::MAX_SCRAPE_BYTES,
        "YouTube captions",
    )
    .await
    .ok()?;

    parse_timedtext(&body, chapters)
}

/// Fetches and extracts transcript and metadata for a YouTube video.
pub async fn fetch_youtube_transcript(client: &Client, video_id: &str) -> Result<ScrapedPage> {
    if !is_valid_youtube_id(video_id) {
        anyhow::bail!("Invalid YouTube video ID '{}'", video_id);
    }
    let watch_url = format!("https://www.youtube.com/watch?v={video_id}");

    let mut title: Option<String> = None;
    let mut tracks: Option<Vec<CaptionTrack>> = None;
    let mut description: Option<String> = None;

    const INNERTUBE_CLIENTS: [(&str, &str); 2] =
        [("ANDROID", "20.10.38"), ("WEB", "2.20240101.00.00")];

    for (client_name, client_version) in INNERTUBE_CLIENTS {
        if tracks.is_some() && title.is_some() {
            break;
        }
        if let Ok(player_data) =
            fetch_innertube_player(client, video_id, client_name, client_version).await
        {
            if title.is_none() {
                title = player_data.title;
            }
            if description.is_none() {
                description = player_data.description;
            }
            if tracks.is_none()
                && let Some(t_list) = player_data.caption_tracks
                && !t_list.is_empty()
            {
                tracks = Some(t_list);
            }
        }
    }

    if (tracks.is_none() || title.is_none())
        && let Ok(html) = fetch_text_limited(
            client
                .get(&watch_url)
                .header("User-Agent", crate::scraper::DEFAULT_USER_AGENT)
                .header("Accept-Language", "en-US,en;q=0.9")
                .timeout(Duration::from_secs(10)),
            crate::scraper::MAX_SCRAPE_BYTES,
            "YouTube watch page",
        )
        .await
    {
        if title.is_none() {
            title = extract_video_title(&html);
        }
        if tracks.is_none() {
            tracks = extract_caption_tracks(&html);
        }
        if description.is_none() {
            description = extract_video_description(&html);
        }
    }

    let resolved_title = title.unwrap_or_else(|| format!("YouTube Video ({video_id})"));
    let chapters = description
        .as_deref()
        .map(parse_chapters_from_description)
        .unwrap_or_default();

    let (chosen_url, fallback_native_url): (Option<String>, Option<String>) =
        if let Some(ref list) = tracks {
            if let Some(en_human) = list
                .iter()
                .find(|t| t.language_code.starts_with("en") && t.kind.as_deref() != Some("asr"))
            {
                (Some(en_human.base_url.clone()), None)
            } else if let Some(en_asr) = list.iter().find(|t| t.language_code.starts_with("en")) {
                (Some(en_asr.base_url.clone()), None)
            } else if let Some(foreign) = list.first() {
                (
                    Some(append_query_param(&foreign.base_url, "tlang", "en")),
                    Some(foreign.base_url.clone()),
                )
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

    let transcript = if let Some(ref base_url) = chosen_url {
        let res = download_and_parse_captions(client, base_url, &chapters).await;
        if res.is_none()
            && let Some(ref native) = fallback_native_url
        {
            download_and_parse_captions(client, native, &chapters).await
        } else {
            res
        }
    } else {
        let fallback_url =
            format!("https://www.youtube.com/api/timedtext?v={video_id}&lang=en&fmt=json3");
        download_and_parse_captions(client, &fallback_url, &chapters).await
    };

    let content = transcript.context(format!(
        "No closed captions or transcript found for YouTube video '{resolved_title}' (ID: {video_id}). The video creator may have disabled subtitles."
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
    fn test_extract_youtube_video_id_and_caption_url_ssrf() {
        for url in [
            "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://youtu.be/dQw4w9WgXcQ?si=12345",
            "https://YOUTU.BE/dQw4w9WgXcQ",
            "https://m.youtube.com/watch?v=dQw4w9WgXcQ",
            "https://www.youtube.com/shorts/dQw4w9WgXcQ",
            "https://www.youtube.com/embed/dQw4w9WgXcQ",
            "https://www.youtube.com/live/dQw4w9WgXcQ",
        ] {
            assert_eq!(
                extract_youtube_video_id(url),
                Some("dQw4w9WgXcQ"),
                "url={url}"
            );
        }
        for url in [
            "https://example.com/article",
            "https://evil.com/youtu.be/dQw4w9WgXcQ",
            "https://youtube.com.evil.com/watch?v=dQw4w9WgXcQ",
        ] {
            assert_eq!(extract_youtube_video_id(url), None, "url={url}");
        }

        assert!(is_safe_caption_url(
            "https://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ"
        ));
        for bad in [
            "http://www.youtube.com/api/timedtext?v=dQw4w9WgXcQ",
            "https://user:pass@www.youtube.com/api/timedtext?v=dQw4w9WgXcQ",
            "https://www.youtube.com:8443/api/timedtext?v=dQw4w9WgXcQ",
            "https://www.youtube.com/api/timedtext_evil?v=dQw4w9WgXcQ",
            "https://evil.com/api/timedtext?v=dQw4w9WgXcQ",
        ] {
            assert!(!is_safe_caption_url(bad), "expected blocked: {bad}");
        }
    }

    #[test]
    fn test_parse_timedtext_formats_chapters_and_consent_rejection() {
        let json3 = r#"{"events":[{"tStartMs":2000,"segs":[{"utf8":"Hello"},{"utf8":" "},{"utf8":"world"}]},{"tStartMs":110000,"segs":[{"utf8":"Next chapter"}]}]}"#;
        let chapters = vec![
            VideoChapter {
                seconds: 0,
                title: "Intro".to_string(),
            },
            VideoChapter {
                seconds: 105,
                title: "Body".to_string(),
            },
        ];
        let with_ch = parse_timedtext(json3, &chapters).unwrap();
        assert!(with_ch.contains("[00:00] ## Intro\nHello world"));
        assert!(with_ch.contains("[01:45] ## Body\nNext chapter"));

        let f1 = r#"<transcript><text start="2.5">Welcome &amp; enjoy</text><text start="62.0">Minute two</text></transcript>"#;
        assert!(
            parse_timedtext(f1, &[])
                .unwrap()
                .contains("[00:02] Welcome & enjoy")
        );
        assert!(
            parse_timedtext(f1, &[])
                .unwrap()
                .contains("[01:02] Minute two")
        );

        let f3 = r#"<timedtext format="3"><body><p t="160"><s>rust</s><s t="500"> safe</s></p><p t="65000"><s>next</s></p></body></timedtext>"#;
        let f3_parsed = parse_timedtext(f3, &[]).unwrap();
        assert!(f3_parsed.contains("[00:00] rust safe"));
        assert!(f3_parsed.contains("[01:05] next"));

        // HTML error/consent pages without timedtext timestamps must not parse as [00:00]
        assert_eq!(
            parse_timedtext(
                "<html><body><p>Before you continue...</p></body></html>",
                &[]
            ),
            None
        );
    }

    #[test]
    fn test_html_metadata_chapters_and_timestamp_overflow() {
        let html = r#"<html><head><title>Rust in 100 Seconds - YouTube</title></head><script>{"captionTracks":null,"shortDescription":"Line 1\nLine 2 \"quotes\", and \",\"\u00a0ok","other":[1]}</script></html>"#;
        assert_eq!(
            extract_video_title(html).as_deref(),
            Some("Rust in 100 Seconds")
        );
        assert!(extract_caption_tracks(html).is_none());
        assert_eq!(
            extract_video_description(html).as_deref(),
            Some("Line 1\nLine 2 \"quotes\", and \",\"\u{00a0}ok")
        );

        let desc = "\n00:00 - Intro\n01:45 Body\n01:15:30 End\n999999999999999999:00 Overflow\n9999999999999999:00:00 Overflow2\n3:5 Ratio";
        let chapters = parse_chapters_from_description(desc);
        assert_eq!(chapters.len(), 3);
        assert_eq!(chapters[2].seconds, 4530);
    }
}
