use anyhow::{Context, Result};
use quick_xml::events::Event;
use quick_xml::reader::Reader;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Cursor, Read, Seek};
use std::path::Path;
use zip::ZipArchive;

use crate::render;
use crate::scraper::{ScrapedPage, WebScraper, clean_text, html_to_markdown};
use crate::system::text_util::{decode_html_entities, url_decode};

pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024; // 20 MB limit for local files

/// Maximum bytes decompressed for XML metadata files (`container.xml`, `.opf`).
pub const MAX_EPUB_XML_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

/// Maximum bytes decompressed per individual chapter HTML/XHTML entry.
pub const MAX_EPUB_CHAPTER_BYTES: u64 = 15 * 1024 * 1024; // 15 MB

/// Maximum cumulative uncompressed markdown bytes extracted from an EPUB archive.
pub const MAX_EPUB_TOTAL_BYTES: usize = 50 * 1024 * 1024; // 50 MB

/// Maximum number of chapters / spine items parsed from an EPUB.
pub const MAX_EPUB_CHAPTER_COUNT: usize = 2000;

#[derive(Debug, Clone)]
pub struct LocalDocument {
    pub title: String,
    pub content: String,
    pub word_count: usize,
}

pub struct DocumentLoader;

impl DocumentLoader {
    /// Validates that `path` exists, is a regular file, and does not exceed `MAX_FILE_BYTES`.
    pub fn validate_file(path: &Path) -> Result<()> {
        if !path.exists() {
            anyhow::bail!("File not found: {:?}", path);
        }

        let meta = fs::metadata(path)
            .with_context(|| format!("Failed to access file metadata: {:?}", path))?;
        if !meta.is_file() {
            anyhow::bail!("Path is not a regular file: {:?}", path);
        }
        if meta.len() > MAX_FILE_BYTES {
            anyhow::bail!(
                "File size ({} bytes) exceeds security limit of {} MB",
                meta.len(),
                MAX_FILE_BYTES / (1024 * 1024)
            );
        }
        Ok(())
    }

    /// Validates and loads a local document (PDF, EPUB, HTML, Markdown, or plain text).
    pub fn load_file(path: &Path, scraper: &WebScraper) -> Result<LocalDocument> {
        Self::validate_file(path)?;

        let fallback_title = || {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Local File".to_string())
        };

        let (content, word_count, title) = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            let page = PdfExtractor::extract_file(path)?;
            let t = page.title.unwrap_or_else(fallback_title);
            (page.content, page.word_count, t)
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
        {
            let page = EpubExtractor::extract_file(path)?;
            let t = page.title.unwrap_or_else(fallback_title);
            (page.content, page.word_count, t)
        } else {
            let file =
                fs::File::open(path).with_context(|| format!("Failed to open file: {:?}", path))?;
            let mut raw = String::new();
            file.take(MAX_FILE_BYTES + 1)
                .read_to_string(&mut raw)
                .with_context(|| format!("Failed to read file: {:?}", path))?;

            Self::normalize_or_plain_text(scraper, &path.display().to_string(), raw, fallback_title)
        };

        if content.trim().is_empty() {
            anyhow::bail!("File {:?} is empty.", path);
        }

        Ok(LocalDocument {
            title,
            content,
            word_count,
        })
    }

    /// Extracts clean text, word count, and title from raw string content, normalizing HTML if detected.
    pub fn normalize_or_plain_text(
        scraper: &WebScraper,
        source: &str,
        raw: String,
        fallback_title: impl FnOnce() -> String,
    ) -> (String, usize, String) {
        let looks_like_html = render::is_html_path(Path::new(source))
            || crate::system::text_util::contains_ascii_case_insensitive(&raw, "<!doctype")
            || crate::system::text_util::contains_ascii_case_insensitive(&raw, "<html")
            || crate::system::text_util::contains_ascii_case_insensitive(&raw, "<body");

        if looks_like_html && let Ok(norm) = scraper.normalize(source, &raw) {
            let t = norm.title.unwrap_or_else(fallback_title);
            (norm.content, norm.word_count, t)
        } else {
            let words = raw.split_whitespace().count();
            (raw, words, fallback_title())
        }
    }
}

// ---------------------------------------------------------------------------
// PDF Extractor
// ---------------------------------------------------------------------------

pub struct PdfExtractor;

impl PdfExtractor {
    /// Extracts text from a PDF file path.
    pub fn extract_file(path: &Path) -> Result<ScrapedPage> {
        DocumentLoader::validate_file(path)?;
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read PDF file at {:?}", path))?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    /// Extracts text from in-memory PDF bytes, parsing the PDF document structure only once.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        let doc_res = pdf_extract::Document::load_mem(bytes);
        let mut doc = match doc_res {
            Ok(d) => d,
            Err(_) if bytes.contains(&b'\r') => {
                // Defensive recovery: if the PDF was corrupted by CRLF line-ending translation (e.g. Git on Windows),
                // strip carriage returns to restore the original xref byte offsets.
                let stripped: Vec<u8> = bytes.iter().copied().filter(|&b| b != b'\r').collect();
                pdf_extract::Document::load_mem(&stripped)
                    .context("Failed to parse PDF document")?
            }
            Err(e) => return Err(e).context("Failed to parse PDF document"),
        };
        if doc.is_encrypted() {
            let _ = doc.decrypt("");
        }
        let mut raw_text = String::new();
        pdf_extract::output_doc(&doc, &mut pdf_extract::PlainTextOutput::new(&mut raw_text))
            .context("Failed to extract text layer from PDF document")?;

        let (title, _author) = Self::extract_metadata_from_doc(&doc, path);
        let processed = Self::post_process_text(&raw_text);
        let (content, word_count) = clean_text(&processed, true, 2);

        if word_count < 5 {
            let path_str = path
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "provided document".to_string());
            anyhow::bail!(
                "No readable text found in PDF ({}). This appears to be a scanned image-only PDF without an embedded text layer.",
                path_str
            );
        }

        let url = path
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_else(|| "file://document.pdf".to_string());

        Ok(ScrapedPage {
            url,
            title: Some(title),
            content,
            word_count,
        })
    }

    /// Attempts to extract Title and Author from the parsed PDF trailer Info dictionary.
    fn extract_metadata_from_doc(
        doc: &pdf_extract::Document,
        path: Option<&Path>,
    ) -> (String, Option<String>) {
        let mut title = None;
        let mut author = None;

        if let Ok(info) = doc
            .trailer
            .get(b"Info")
            .and_then(|obj| doc.dereference(obj))
            && let Ok(dict) = info.1.as_dict()
        {
            if let Ok(t) = dict.get(b"Title").and_then(|o| o.as_str()) {
                let decoded = String::from_utf8_lossy(t).trim().to_string();
                if !decoded.is_empty() {
                    title = Some(decoded);
                }
            }
            if let Ok(a) = dict.get(b"Author").and_then(|o| o.as_str()) {
                let decoded = String::from_utf8_lossy(a).trim().to_string();
                if !decoded.is_empty() {
                    author = Some(decoded);
                }
            }
        }

        let fallback_title = path
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF Document".to_string());

        (title.unwrap_or(fallback_title), author)
    }

    /// Reconstructs natural paragraphs, removes carriage returns, and rejoins hyphenated line breaks.
    pub fn post_process_text(raw: &str) -> String {
        let mut cleaned = String::with_capacity(raw.len());
        let mut lines = raw.lines().peekable();

        while let Some(line) = lines.next() {
            let mut trimmed = line.trim();
            if trimmed.is_empty() {
                if !cleaned.ends_with("\n\n") && !cleaned.is_empty() {
                    cleaned.push_str("\n\n");
                }
                continue;
            }

            let has_form_feed = trimmed.contains('\x0c');
            let is_page_break = if has_form_feed {
                trimmed = trimmed.trim_matches('\x0c').trim();
                true
            } else {
                false
            };

            if trimmed.is_empty() {
                if !cleaned.ends_with("\n\n") && !cleaned.is_empty() {
                    cleaned.push_str("\n\n");
                }
                continue;
            }

            // Rejoin words split across line endings: "algo-\nrithm" -> "algorithm"
            if trimmed.ends_with('-') && lines.peek().is_some() {
                cleaned.push_str(&trimmed[..trimmed.len() - 1]);
            } else {
                cleaned.push_str(trimmed);
                if is_page_break {
                    if !cleaned.ends_with("\n\n") {
                        cleaned.push_str("\n\n");
                    }
                } else {
                    let next_is_heading_or_start = lines
                        .peek()
                        .map(|n| {
                            let nt = n.trim();
                            nt.starts_with('#')
                                || nt.starts_with("• ")
                                || nt.starts_with("- ")
                                || nt.starts_with("* ")
                                || (nt.len() < 80
                                    && nt
                                        .chars()
                                        .next()
                                        .map(|c| c.is_ascii_digit())
                                        .unwrap_or(false)
                                    && nt.contains(". "))
                        })
                        .unwrap_or(false);

                    let ends_sentence = trimmed.ends_with('.')
                        || trimmed.ends_with(':')
                        || trimmed.ends_with('?')
                        || trimmed.ends_with('!');

                    if ends_sentence && next_is_heading_or_start {
                        cleaned.push_str("\n\n");
                    } else {
                        cleaned.push(' ');
                    }
                }
            }
        }

        cleaned.trim().to_string()
    }

    /// Generates a valid in-memory PDF containing the given raw content stream.
    pub fn build_pdf_with_stream_content(content: &[u8]) -> Vec<u8> {
        use pdf_extract::{Dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();

        let mut font = Dictionary::new();
        font.set("Type", "Font");
        font.set("Subtype", "Type1");
        font.set("BaseFont", "Helvetica");
        let font_id = doc.add_object(font);

        let mut fonts_dict = Dictionary::new();
        fonts_dict.set("F1", font_id);
        let mut resources = Dictionary::new();
        resources.set("Font", fonts_dict);
        let resources_id = doc.add_object(resources);

        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.to_vec()));

        let mut page = Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set("Contents", content_id);
        page.set("Resources", resources_id);
        page.set("MediaBox", vec![0.into(), 0.into(), 612.into(), 792.into()]);
        let page_id = doc.add_object(page);

        let mut pages = Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Kids", vec![page_id.into()]);
        pages.set("Count", 1);
        doc.objects.insert(pages_id, Object::Dictionary(pages));

        let mut catalog = Dictionary::new();
        catalog.set("Type", "Catalog");
        catalog.set("Pages", pages_id);
        let catalog_id = doc.add_object(catalog);
        doc.trailer.set("Root", catalog_id);

        let mut buffer = Vec::new();
        doc.save_to(&mut buffer)
            .expect("Failed to serialize sample PDF");
        buffer
    }

    /// Generates a valid in-memory PDF containing structured sample text for tests and usage examples.
    pub fn build_sample_pdf_bytes() -> Vec<u8> {
        let content = b"BT /F1 12 Tf 50 720 Td (srz - High-Performance Modular Rust CLI for Summarization) Tj /F1 10 Tf 0 -30 Td (srz extracts and synthesizes structured summaries using local OpenAI-compliant LLMs.) Tj 0 -20 Td (Key Features: Universal Ingestion for Web, YouTube captions, local PDFs, and live news.) Tj 0 -20 Td (Private and Secure: 100 percent offline local inference with strict POSIX permissions.) Tj 0 -20 Td (Offline Library: Embedded DuckDB indexes every summary for instant full-text search.) Tj 0 -20 Td (Terminal Paging: Syntax-highlighted output with automatic terminal paging via less.) Tj 0 -20 Td (Interactive Grounded Q and A: Ask questions grounded on full source documents or past reads.) Tj ET";
        Self::build_pdf_with_stream_content(content)
    }
}

// ---------------------------------------------------------------------------
// EPUB Extractor
// ---------------------------------------------------------------------------

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
    fn test_post_process_text_hyphenation() {
        let raw = "This is an implemen-\ntation of modern systems.\n\nNew paragraph here.";
        let processed = PdfExtractor::post_process_text(raw);
        assert!(processed.contains("implementation of modern systems."));
        assert!(processed.contains("New paragraph here."));
    }

    #[test]
    fn test_empty_pdf_detection() {
        let empty_text = "     \n\n  ";
        let processed = PdfExtractor::post_process_text(empty_text);
        assert_eq!(processed, "");
    }

    #[test]
    fn test_extract_pdf_from_mem() {
        let content = b"BT /F1 12 Tf 100 700 Td (Hello from Rust PDF extractor with enough words to pass the minimum text threshold for testing purposes) Tj ET";
        let buffer = PdfExtractor::build_pdf_with_stream_content(content);
        let page = PdfExtractor::extract_from_bytes(&buffer, None).unwrap();
        assert!(page.content.contains("Hello from Rust PDF extractor"));
        assert_eq!(page.title.as_deref(), Some("PDF Document"));
    }

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
        let occurrences = page.content.matches("Unique single chapter text").count();
        assert_eq!(occurrences, 1);
    }
}
