use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read, Seek};
use std::path::Path;

use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use zip::ZipArchive;

use crate::scraper::{ScrapedPage, clean_text, html_to_markdown};
use crate::sources::document::DocumentLoader;
use crate::system::text_util::{decode_html_entities, url_decode};

/// Maximum bytes decompressed for XML metadata files (`container.xml`, `.opf`).
pub const MAX_EPUB_XML_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

/// Maximum bytes decompressed per individual chapter HTML/XHTML entry.
pub const MAX_EPUB_CHAPTER_BYTES: u64 = 15 * 1024 * 1024; // 15 MB

/// Maximum cumulative uncompressed markdown bytes extracted from an EPUB archive.
pub const MAX_EPUB_TOTAL_BYTES: usize = 50 * 1024 * 1024; // 50 MB

/// Maximum number of chapters / spine items parsed from an EPUB.
pub const MAX_EPUB_CHAPTER_COUNT: usize = 2000;

pub struct EpubExtractor;

impl EpubExtractor {
    /// Extracts text and metadata from an EPUB file path.
    pub fn extract_file(path: &Path) -> Result<ScrapedPage> {
        DocumentLoader::validate_file(path)?;
        let file =
            File::open(path).with_context(|| format!("Failed to open EPUB file at {:?}", path))?;
        Self::extract_from_reader(file, Some(path))
    }

    /// Extracts text and metadata from in-memory EPUB bytes.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        let cursor = Cursor::new(bytes);
        Self::extract_from_reader(cursor, path)
    }

    /// Core extractor reading from any seekable reader.
    pub fn extract_from_reader<R: Read + Seek>(
        reader: R,
        path: Option<&Path>,
    ) -> Result<ScrapedPage> {
        let mut archive = ZipArchive::new(reader).context("Failed to open EPUB as zip archive")?;

        let opf_path = find_opf_path(&mut archive)?;
        let (title, chapters) = parse_opf(&mut archive, &opf_path)?;

        let mut book_markdown = String::new();
        let mut total_bytes: usize = 0;
        let mut seen_chapters = HashSet::new();

        for chapter_path in chapters {
            if !seen_chapters.insert(chapter_path.clone()) {
                continue;
            }
            if let Ok(mut file) = archive.by_name(&chapter_path) {
                let mut html = String::new();
                if file
                    .by_ref()
                    .take(MAX_EPUB_CHAPTER_BYTES)
                    .read_to_string(&mut html)
                    .is_ok()
                {
                    let md = html_to_markdown(&html);
                    let trimmed = md.trim();
                    if !trimmed.is_empty() {
                        if total_bytes.saturating_add(trimmed.len()) > MAX_EPUB_TOTAL_BYTES {
                            let remaining = MAX_EPUB_TOTAL_BYTES.saturating_sub(total_bytes);
                            if remaining > 0 {
                                let bound = trimmed.floor_char_boundary(remaining);
                                let slice = &trimmed[..bound];
                                if !slice.is_empty() {
                                    if !book_markdown.is_empty() {
                                        book_markdown.push_str("\n\n");
                                    }
                                    book_markdown.push_str(slice);
                                }
                            }
                            break;
                        }
                        if !book_markdown.is_empty() {
                            book_markdown.push_str("\n\n");
                        }
                        book_markdown.push_str(trimmed);
                        total_bytes = total_bytes.saturating_add(trimmed.len());
                    }
                }
            }
        }

        let (cleaned_content, word_count) = clean_text(&book_markdown, true, 2);

        if word_count < 5 {
            let path_str = path
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "provided EPUB document".to_string());
            anyhow::bail!(
                "No readable text found in EPUB ({}). The file may be empty, image-only, or encrypted with DRM.",
                path_str
            );
        }

        let resolved_title = title.or_else(|| {
            path.and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
        });

        let url = path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "epub://document".to_string());

        Ok(ScrapedPage {
            url,
            title: resolved_title,
            content: cleaned_content,
            word_count,
        })
    }
}

/// Locates the package OPF file path within the EPUB archive.
fn find_opf_path<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<String> {
    // 1. Standard EPUB path: check META-INF/container.xml
    if let Ok(mut container_file) = archive.by_name("META-INF/container.xml") {
        let mut xml = String::new();
        if container_file
            .by_ref()
            .take(MAX_EPUB_XML_BYTES)
            .read_to_string(&mut xml)
            .is_ok()
            && let Some(path) = parse_container_xml(&xml)
        {
            return Ok(path);
        }
    }

    // 2. Fallback: locate any entry ending with .opf from the in-memory central directory
    for name_res in archive.file_names() {
        if let Ok(name) = name_res
            && crate::system::text_util::ends_with_ascii_case_insensitive(&name, ".opf")
        {
            return Ok(name.into_owned());
        }
    }

    anyhow::bail!("Invalid EPUB: no OPF package file found in archive")
}

/// Parses META-INF/container.xml to find the rootfile full-path.
fn parse_container_xml(xml: &str) -> Option<String> {
    let mut reader = Reader::from_str(xml);
    loop {
        match reader.read_event() {
            Ok(Event::Start(e) | Event::Empty(e)) => {
                let local = e.local_name();
                if local.as_ref().eq_ignore_ascii_case("rootfile") {
                    for attr in e.attributes().flatten() {
                        if attr
                            .key
                            .local_name()
                            .as_ref()
                            .eq_ignore_ascii_case("full-path")
                        {
                            let val = attr.value.trim().to_string();
                            if !val.is_empty() {
                                return Some(val);
                            }
                        }
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
    }
    None
}

/// Parses the OPF package document to extract book title and ordered chapter file paths.
fn parse_opf<R: Read + Seek>(
    archive: &mut ZipArchive<R>,
    opf_path: &str,
) -> Result<(Option<String>, Vec<String>)> {
    let mut opf_file = archive
        .by_name(opf_path)
        .with_context(|| format!("Failed to read OPF file at {}", opf_path))?;
    let mut opf_xml = String::new();
    opf_file
        .by_ref()
        .take(MAX_EPUB_XML_BYTES)
        .read_to_string(&mut opf_xml)
        .context("Failed to read OPF content")?;

    let opf_dir = match opf_path.rfind('/') {
        Some(idx) => &opf_path[..=idx],
        None => "",
    };

    let mut reader = Reader::from_str(&opf_xml);
    let mut title: Option<String> = None;
    let mut manifest: HashMap<String, (String, String)> = HashMap::new(); // id -> (href, media_type)
    let mut spine: Vec<String> = Vec::new(); // list of idref

    loop {
        match reader.read_event() {
            Ok(Event::Start(ref e)) => {
                let local = e.local_name();
                if local.as_ref().eq_ignore_ascii_case("title") && title.is_none() {
                    if let Ok(t) = reader.read_text(e.name()) {
                        let decoded = decode_html_entities(t.as_ref());
                        let trimmed = decoded.trim();
                        if !trimmed.is_empty() {
                            title = Some(trimmed.to_string());
                        }
                    }
                } else if local.as_ref().eq_ignore_ascii_case("item") {
                    parse_item_attr(e, &mut manifest);
                } else if local.as_ref().eq_ignore_ascii_case("itemref") {
                    parse_itemref_attr(e, &mut spine);
                }
            }
            Ok(Event::Empty(ref e)) => {
                let local = e.local_name();
                if local.as_ref().eq_ignore_ascii_case("item") {
                    parse_item_attr(e, &mut manifest);
                } else if local.as_ref().eq_ignore_ascii_case("itemref") {
                    parse_itemref_attr(e, &mut spine);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(anyhow::anyhow!("XML parse error in OPF: {}", e));
            }
            _ => {}
        }
    }

    let mut chapter_paths = Vec::new();

    // 1. Follow spine reading order if present
    for idref in &spine {
        if let Some((href, _)) = manifest.get(idref) {
            let resolved = resolve_epub_path(opf_dir, href);
            chapter_paths.push(resolved);
            if chapter_paths.len() >= MAX_EPUB_CHAPTER_COUNT {
                break;
            }
        }
    }

    // 2. Fallback to manifest items if spine is empty
    if chapter_paths.is_empty() {
        for (href, media_type) in manifest.values() {
            let is_html_media =
                crate::system::text_util::contains_ascii_case_insensitive(media_type, "html")
                    || crate::system::text_util::contains_ascii_case_insensitive(media_type, "xml");
            let is_html_ext =
                crate::system::text_util::ends_with_ascii_case_insensitive(href, ".xhtml")
                    || crate::system::text_util::ends_with_ascii_case_insensitive(href, ".html")
                    || crate::system::text_util::ends_with_ascii_case_insensitive(href, ".htm");
            if is_html_media || is_html_ext {
                chapter_paths.push(resolve_epub_path(opf_dir, href));
                if chapter_paths.len() >= MAX_EPUB_CHAPTER_COUNT {
                    break;
                }
            }
        }
    }

    Ok((title, chapter_paths))
}

fn parse_item_attr(
    e: &quick_xml::events::BytesStart<'_>,
    manifest: &mut HashMap<String, (String, String)>,
) {
    let mut id = String::new();
    let mut href = String::new();
    let mut media_type = String::new();

    for attr in e.attributes().flatten() {
        let key_local = attr.key.local_name();
        if key_local.as_ref().eq_ignore_ascii_case("id") {
            id = attr.value.trim().to_string();
        } else if key_local.as_ref().eq_ignore_ascii_case("href") {
            href = attr.value.trim().to_string();
        } else if key_local.as_ref().eq_ignore_ascii_case("media-type") {
            media_type = attr.value.trim().to_string();
        }
    }

    if !id.is_empty() && !href.is_empty() {
        manifest.insert(id, (href, media_type));
    }
}

fn parse_itemref_attr(e: &quick_xml::events::BytesStart<'_>, spine: &mut Vec<String>) {
    if spine.len() >= MAX_EPUB_CHAPTER_COUNT {
        return;
    }
    for attr in e.attributes().flatten() {
        if attr.key.local_name().as_ref().eq_ignore_ascii_case("idref") {
            let idref = attr.value.trim().to_string();
            if !idref.is_empty() {
                spine.push(idref);
            }
        }
    }
}

/// Resolves relative EPUB href path against base OPF directory and cleans ./ and ../ segments.
fn resolve_epub_path(base_dir: &str, href: &str) -> String {
    let clean_href = href.split('#').next().unwrap_or(href);
    let decoded = url_decode(clean_href).unwrap_or(std::borrow::Cow::Borrowed(clean_href));

    let full = if base_dir.is_empty() {
        decoded.to_string()
    } else {
        format!("{}{}", base_dir, decoded)
    };

    let mut parts: Vec<&str> = Vec::new();
    for part in full.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            parts.pop();
        } else {
            parts.push(part);
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    #[test]
    fn test_epub_deduplication_and_spine_limit() {
        let mut zip_buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut zip_buf);
            let opts = SimpleFileOptions::default();

            writer.start_file("META-INF/container.xml", opts).unwrap();
            writer
                .write_all(
                    br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf"/></rootfiles>
</container>"#,
                )
                .unwrap();

            writer.start_file("content.opf", opts).unwrap();
            // Create duplicate itemrefs in spine
            writer
                .write_all(
                    br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Bomb Test</dc:title></metadata>
  <manifest>
    <item id="c1" href="ch1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c1"/>
    <itemref idref="c1"/>
  </spine>
</package>"#,
                )
                .unwrap();

            writer.start_file("ch1.html", opts).unwrap();
            writer
                .write_all(
                    b"<html><body><p>Unique single chapter text content here.</p></body></html>",
                )
                .unwrap();

            writer.finish().unwrap();
        }

        let bytes = zip_buf.into_inner();
        let page = EpubExtractor::extract_from_bytes(&bytes, None).unwrap();
        // The chapter text should appear exactly once due to deduplication
        let occurrences = page.content.matches("Unique single chapter text").count();
        assert_eq!(occurrences, 1);
    }
}
