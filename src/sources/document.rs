use crate::fs_util::read_file_bytes_limited;
use crate::system::text_util::{XmlEvent, XmlScanner, xml_attr, xml_local_name};
use crate::system::zip::ZipReader;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::render;
use crate::scraper::{ScrapedPage, WebScraper, clean_text, html_to_markdown};
use crate::system::text_util::url_decode;

pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024; // 20 MB limit for local files
pub const MAX_PDF_TEXT_BYTES: usize = 50 * 1024 * 1024; // 50 MB limit for extracted PDF text

#[derive(Default)]
struct BoundedIoWriter {
    buf: Vec<u8>,
    max_bytes: usize,
    exceeded: bool,
}

impl std::io::Write for BoundedIoWriter {
    fn write(&mut self, data: &[u8]) -> std::io::Result<usize> {
        if self.buf.len().saturating_add(data.len()) > self.max_bytes {
            self.exceeded = true;
            return Err(std::io::Error::other("PDF text limit exceeded"));
        }
        self.buf.extend_from_slice(data);
        Ok(data.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Maximum bytes decompressed for XML metadata files (`container.xml`, `.opf`).
pub const MAX_EPUB_XML_BYTES: u64 = 5 * 1024 * 1024; // 5 MB

/// Maximum bytes decompressed per individual chapter HTML/XHTML entry.
pub const MAX_EPUB_CHAPTER_BYTES: u64 = 15 * 1024 * 1024; // 15 MB

/// Maximum cumulative uncompressed markdown bytes extracted from an EPUB archive.
pub const MAX_EPUB_TOTAL_BYTES: usize = 50 * 1024 * 1024; // 50 MB

/// Maximum number of chapters / spine items parsed from an EPUB.
pub const MAX_EPUB_CHAPTER_COUNT: usize = 2000;

pub struct DocumentLoader;

impl DocumentLoader {
    /// Validates and loads a local document (PDF, EPUB, HTML, Markdown, or plain text).
    pub fn load_file(path: &Path, scraper: &WebScraper) -> Result<ScrapedPage> {
        let fallback_title = || {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Local File".to_string())
        };

        let mut page = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            PdfExtractor::extract_file(path)?
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("epub"))
        {
            EpubExtractor::extract_file(path)?
        } else {
            let bytes = read_file_bytes_limited(path, MAX_FILE_BYTES, false, "Document")?;
            let raw = String::from_utf8(bytes)
                .with_context(|| format!("Failed to read file: {:?}", path))?;

            let (content, word_count, title) = Self::normalize_or_plain_text(
                scraper,
                &path.display().to_string(),
                raw,
                fallback_title,
            );
            ScrapedPage {
                url: format!("file://{}", path.display()),
                title: Some(title),
                content,
                word_count,
            }
        };

        if page.title.as_deref().is_none_or(|t| t.trim().is_empty()) {
            page.title = Some(fallback_title());
        }
        if page.content.trim().is_empty() {
            anyhow::bail!("File {:?} is empty.", path);
        }

        Ok(page)
    }

    /// Extracts clean text, word count, and title from raw string content, normalizing HTML if detected.
    pub fn normalize_or_plain_text(
        scraper: &WebScraper,
        source: &str,
        raw: String,
        fallback_title: impl FnOnce() -> String,
    ) -> (String, usize, String) {
        let looks_like_html = render::is_html_path(Path::new(source))
            || crate::system::text_util::find_ascii_case_insensitive(&raw, "<!doctype").is_some()
            || crate::system::text_util::find_ascii_case_insensitive(&raw, "<html").is_some()
            || crate::system::text_util::find_ascii_case_insensitive(&raw, "<body").is_some();

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
        let bytes = read_file_bytes_limited(path, MAX_FILE_BYTES, false, "PDF")?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    /// Pre-flights non-image FlateDecode PDF streams into `io::sink()` to block zlib bombs before `pdf_extract` allocates.
    pub(super) fn verify_pdf_stream_budgets(
        doc: &pdf_extract::Document,
        max_bytes: usize,
    ) -> Result<()> {
        use std::io::Read;
        let mut remaining = max_bytes as u64;

        for obj in doc.objects.values() {
            let pdf_extract::Object::Stream(stream) = obj else {
                continue;
            };
            if stream
                .dict
                .get(b"Subtype")
                .and_then(|o| o.as_name())
                .is_ok_and(|n| n == b"Image")
            {
                continue;
            }
            let is_flate = match stream.dict.get(b"Filter") {
                Ok(pdf_extract::Object::Name(n)) => n == b"FlateDecode",
                Ok(pdf_extract::Object::Array(arr)) => {
                    matches!(arr.first(), Some(pdf_extract::Object::Name(n)) if n == b"FlateDecode")
                }
                _ => false,
            };
            if !is_flate {
                continue;
            }

            let mut decoder =
                flate2::read::ZlibDecoder::new(stream.content.as_slice()).take(remaining + 1);
            if let Ok(written) = std::io::copy(&mut decoder, &mut std::io::sink()) {
                if written > remaining {
                    anyhow::bail!(
                        "Decompressed PDF stream data exceeds maximum allowed size of {} bytes",
                        max_bytes
                    );
                }
                remaining = remaining.saturating_sub(written);
            }
        }
        Ok(())
    }

    pub(super) fn extract_pdf_text_bounded(
        doc: &pdf_extract::Document,
        max_text_bytes: usize,
    ) -> Result<String> {
        Self::verify_pdf_stream_budgets(doc, max_text_bytes)?;
        let mut writer = BoundedIoWriter {
            max_bytes: max_text_bytes,
            ..Default::default()
        };
        let extract_res = {
            let writer_ref: &mut dyn std::io::Write = &mut writer;
            let mut output = pdf_extract::PlainTextOutput::new(writer_ref);
            pdf_extract::output_doc(doc, &mut output)
        };
        if writer.exceeded {
            anyhow::bail!(
                "Extracted PDF text exceeds maximum allowed size of {} bytes",
                max_text_bytes
            );
        }
        extract_res.context("Failed to extract text layer from PDF document")?;
        String::from_utf8(writer.buf).context("Extracted PDF text was not valid UTF-8")
    }

    /// Extracts text from in-memory PDF bytes, parsing the PDF document structure only once.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        let doc_res = pdf_extract::Document::load_mem(bytes);
        let mut doc = match doc_res {
            Ok(d) => d,
            Err(_) if bytes.windows(2).any(|w| w == b"\r\n") => {
                // Recover only `\r\n` CRLF translation without corrupting standalone `\r` inside binary streams.
                let mut normalized = Vec::with_capacity(bytes.len());
                for (i, &b) in bytes.iter().enumerate() {
                    if b == b'\r' && bytes.get(i + 1) == Some(&b'\n') {
                        continue;
                    }
                    normalized.push(b);
                }
                pdf_extract::Document::load_mem(&normalized)
                    .context("Failed to parse PDF document")?
            }
            Err(e) => return Err(e).context("Failed to parse PDF document"),
        };
        if doc.is_encrypted() {
            let _ = doc.decrypt("");
        }
        let raw_text = Self::extract_pdf_text_bounded(&doc, MAX_PDF_TEXT_BYTES)?;

        let title = Self::extract_title_from_doc(&doc, path);
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

    /// Attempts to extract Title from the parsed PDF trailer Info dictionary.
    fn extract_title_from_doc(doc: &pdf_extract::Document, path: Option<&Path>) -> String {
        let mut title = None;

        if let Ok(info) = doc
            .trailer
            .get(b"Info")
            .and_then(|obj| doc.dereference(obj))
            && let Ok(dict) = info.1.as_dict()
            && let Ok(t) = dict.get(b"Title").and_then(|o| o.as_str())
        {
            let decoded = String::from_utf8_lossy(t).trim().to_string();
            if !decoded.is_empty() {
                title = Some(decoded);
            }
        }

        let fallback_title = path
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "PDF Document".to_string());

        title.unwrap_or(fallback_title)
    }

    /// Reconstructs natural paragraphs, removes carriage returns, and rejoins hyphenated line breaks.
    pub fn post_process_text(raw: &str) -> String {
        let mut cleaned = String::with_capacity(raw.len());
        let mut lines = raw.lines().peekable();

        while let Some(line) = lines.next() {
            let is_page_break = line.contains('\x0c');
            let trimmed = line.trim_matches(|c: char| c.is_whitespace() || c == '\x0c');
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
                let next_is_heading_or_start = lines.peek().is_some_and(|n| {
                    let nt = n.trim();
                    nt.starts_with(['#', '•'])
                        || nt.starts_with("- ")
                        || nt.starts_with("* ")
                        || (nt.len() < 80
                            && nt.bytes().next().is_some_and(|b| b.is_ascii_digit())
                            && nt.contains(". "))
                });

                if is_page_break {
                    if !cleaned.ends_with("\n\n") {
                        cleaned.push_str("\n\n");
                    }
                } else if trimmed.ends_with(['.', ':', '?', '!']) && next_is_heading_or_start {
                    cleaned.push_str("\n\n");
                } else {
                    cleaned.push(' ');
                }
            }
        }

        cleaned.trim().to_string()
    }
}

// ---------------------------------------------------------------------------
// EPUB Extractor
// ---------------------------------------------------------------------------

pub struct EpubExtractor;

impl EpubExtractor {
    /// Extracts text and metadata from an EPUB file path.
    pub fn extract_file(path: &Path) -> Result<ScrapedPage> {
        let bytes = read_file_bytes_limited(path, MAX_FILE_BYTES, false, "EPUB")?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    /// Extracts text and metadata from in-memory EPUB bytes.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        let archive = ZipReader::new(bytes).context("Failed to open EPUB as zip archive")?;

        let opf_path = find_opf_path(&archive)?;
        let (title, chapters) = parse_opf(&archive, &opf_path)?;

        let mut book_markdown = String::new();
        let mut total_bytes: usize = 0;
        let mut total_decompressed: usize = 0;
        let mut seen_chapters = HashSet::new();

        for chapter_path in chapters {
            if !seen_chapters.insert(chapter_path.clone()) {
                continue;
            }
            let raw_cap = MAX_EPUB_TOTAL_BYTES
                .saturating_sub(total_decompressed)
                .min(MAX_EPUB_CHAPTER_BYTES as usize);
            if raw_cap == 0 {
                break;
            }
            if let Ok(file_bytes) = archive.read_file(&chapter_path, raw_cap) {
                total_decompressed = total_decompressed.saturating_add(file_bytes.len());
                if let Ok(html) = std::str::from_utf8(&file_bytes) {
                    let md = html_to_markdown(html);
                    let trimmed = md.trim();
                    let remaining = MAX_EPUB_TOTAL_BYTES.saturating_sub(total_bytes);
                    let slice =
                        &trimmed[..trimmed.floor_char_boundary(remaining.min(trimmed.len()))];
                    if !slice.is_empty() {
                        if !book_markdown.is_empty() {
                            book_markdown.push_str("\n\n");
                        }
                        book_markdown.push_str(slice);
                        total_bytes = total_bytes.saturating_add(slice.len());
                    }
                    if total_bytes >= MAX_EPUB_TOTAL_BYTES {
                        break;
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
            .map(|p| format!("file://{}", p.display()))
            .unwrap_or_else(|| "file://document.epub".to_string());

        Ok(ScrapedPage {
            url,
            title: resolved_title,
            content: cleaned_content,
            word_count,
        })
    }
}

/// Locates the package OPF file path within the EPUB archive.
fn find_opf_path(archive: &ZipReader) -> Result<String> {
    // 1. Standard EPUB path: check META-INF/container.xml
    if let Ok(xml_bytes) = archive.read_file("META-INF/container.xml", MAX_EPUB_XML_BYTES as usize)
        && let Ok(xml) = std::str::from_utf8(&xml_bytes)
        && let Some(path) = parse_container_xml(xml)
    {
        return Ok(path);
    }

    // 2. Fallback: locate any entry ending with .opf from the in-memory central directory
    for name in archive.file_names() {
        if crate::system::text_util::ends_with_ascii_case_insensitive(name, ".opf") {
            return Ok(name.to_string());
        }
    }

    anyhow::bail!("Invalid EPUB: no OPF package file found in archive")
}

/// Parses META-INF/container.xml to find the rootfile full-path.
fn parse_container_xml(xml: &str) -> Option<String> {
    let mut scanner = XmlScanner::new(xml);
    while let Some(ev) = scanner.next_event() {
        if let XmlEvent::Start { name, attrs, .. } = ev
            && xml_local_name(name).eq_ignore_ascii_case("rootfile")
            && let Some(val) = xml_attr(attrs, "full-path")
        {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Parses the OPF package document to extract book title and ordered chapter file paths.
fn parse_opf(archive: &ZipReader, opf_path: &str) -> Result<(Option<String>, Vec<String>)> {
    let opf_bytes = archive
        .read_file(opf_path, MAX_EPUB_XML_BYTES as usize)
        .with_context(|| format!("Failed to read OPF file at {}", opf_path))?;
    let opf_xml = std::str::from_utf8(&opf_bytes).context("OPF XML is not valid UTF-8")?;

    let opf_dir = match opf_path.rfind('/') {
        Some(idx) => &opf_path[..=idx],
        None => "",
    };

    const MAX_EPUB_MANIFEST_ITEMS: usize = 16_384;
    let mut scanner = XmlScanner::new(opf_xml);
    let mut title: Option<String> = None;
    let mut manifest: HashMap<String, (String, String)> = HashMap::new(); // id -> (href, media_type)
    let mut spine: Vec<String> = Vec::new(); // list of idref

    while let Some(ev) = scanner.next_event() {
        if let XmlEvent::Start {
            name,
            attrs,
            is_empty,
        } = ev
        {
            let local = xml_local_name(name);
            if local.eq_ignore_ascii_case("title") && title.is_none() && !is_empty {
                let text = scanner.read_text(name);
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    title = Some(trimmed.to_string());
                }
            } else if local.eq_ignore_ascii_case("item") {
                if manifest.len() < MAX_EPUB_MANIFEST_ITEMS
                    && let (Some(id), Some(href)) = (xml_attr(attrs, "id"), xml_attr(attrs, "href"))
                {
                    let (id, href) = (id.trim(), href.trim());
                    if !id.is_empty() && !href.is_empty() {
                        let mt = xml_attr(attrs, "media-type")
                            .map(|s| s.trim().to_string())
                            .unwrap_or_default();
                        manifest.insert(id.to_string(), (href.to_string(), mt));
                    }
                }
            } else if local.eq_ignore_ascii_case("itemref")
                && spine.len() < MAX_EPUB_CHAPTER_COUNT
                && let Some(idref) = xml_attr(attrs, "idref")
                    .map(|v| v.trim().to_string())
                    .filter(|s| !s.is_empty())
            {
                spine.push(idref);
            }
        }
    }

    let mut chapter_paths = Vec::new();

    // 1. Follow spine reading order if present
    for idref in &spine {
        if let Some((href, _)) = manifest.get(idref) {
            chapter_paths.push(resolve_epub_path(opf_dir, href));
            if chapter_paths.len() >= MAX_EPUB_CHAPTER_COUNT {
                break;
            }
        }
    }

    // 2. Fallback to manifest items if spine is empty
    if chapter_paths.is_empty() {
        for (href, media_type) in manifest.values() {
            let is_html = ["html", "xml"].iter().any(|m| {
                crate::system::text_util::find_ascii_case_insensitive(media_type, m).is_some()
            }) || [".xhtml", ".html", ".htm"]
                .iter()
                .any(|e| crate::system::text_util::ends_with_ascii_case_insensitive(href, e));
            if is_html {
                chapter_paths.push(resolve_epub_path(opf_dir, href));
                if chapter_paths.len() >= MAX_EPUB_CHAPTER_COUNT {
                    break;
                }
            }
        }
    }

    Ok((title, chapter_paths))
}

/// Resolves relative EPUB href path against base OPF directory, normalizing `/` and `\` segments.
fn resolve_epub_path(base_dir: &str, href: &str) -> String {
    let clean_href = href.split(['?', '#']).next().unwrap_or(href);
    let decoded = url_decode(clean_href).unwrap_or(std::borrow::Cow::Borrowed(clean_href));

    let full = if base_dir.is_empty() {
        decoded.into_owned()
    } else {
        format!("{}/{}", base_dir.trim_end_matches(['/', '\\']), decoded)
    };

    let mut parts: Vec<&str> = Vec::new();
    for part in full.split(['/', '\\']) {
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

    fn build_pdf_with_stream_content(content: &[u8]) -> Vec<u8> {
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

    #[test]
    fn test_extract_pdf_from_mem() {
        let content = b"BT /F1 12 Tf 100 700 Td (Hello from Rust PDF extractor with enough words to pass the minimum text threshold for testing purposes) Tj ET";
        let buffer = build_pdf_with_stream_content(content);
        let page = PdfExtractor::extract_from_bytes(&buffer, None).unwrap();
        assert!(page.content.contains("Hello from Rust PDF extractor"));
        assert_eq!(page.title.as_deref(), Some("PDF Document"));

        let sample_content = b"BT /F1 12 Tf 50 720 Td (srz - High-Performance Modular Rust CLI for Summarization) Tj /F1 10 Tf 0 -30 Td (srz extracts and synthesizes structured summaries using local OpenAI-compliant LLMs.) Tj 0 -20 Td (Key Features: Universal Ingestion for Web, YouTube captions, local PDFs, and live news.) Tj 0 -20 Td (Private and Secure: 100 percent offline local inference with strict POSIX permissions.) Tj 0 -20 Td (Offline Library: Embedded redb database stores every summary for instant full-text search.) Tj 0 -20 Td (Terminal Paging: Syntax-highlighted output with automatic terminal paging via less.) Tj 0 -20 Td (Interactive Grounded Q and A: Ask questions grounded on full source documents or past reads.) Tj ET";
        let sample_page =
            PdfExtractor::extract_from_bytes(&build_pdf_with_stream_content(sample_content), None)
                .unwrap();
        assert!(
            sample_page
                .content
                .contains("srz - High-Performance Modular Rust CLI")
        );
        assert!(sample_page.content.contains("Universal Ingestion"));
        assert!(sample_page.word_count >= 50);
    }

    #[test]
    fn test_pdf_extractor_bounds_type_checks_and_crlf_recovery() {
        assert!(PdfExtractor::extract_file(Path::new("/tmp/does_not_exist_srz_test.pdf")).is_err());
        assert!(PdfExtractor::extract_file(&std::env::temp_dir()).is_err());

        let original_bytes =
            std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/document.pdf")).unwrap();
        let mut crlf_bytes = Vec::with_capacity(original_bytes.len() + 256);
        for &b in &original_bytes {
            if b == b'\n' {
                crlf_bytes.push(b'\r');
            }
            crlf_bytes.push(b);
        }
        let page = PdfExtractor::extract_from_bytes(&crlf_bytes, None)
            .expect("PdfExtractor should recover from CRLF line ending corruption");
        assert!(page.content.contains("THE HISTORY OF RUST"));
        assert!(page.word_count >= 100);
    }

    #[test]
    fn test_epub_extractor_and_document_loader() {
        let container_xml = br#"<?xml version="1.0"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;
        let opf_xml = br#"<?xml version="1.0" encoding="utf-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0" unique-identifier="pub-id">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>The Great Gatsby</dc:title>
    <dc:creator>F. Scott Fitzgerald</dc:creator>
  </metadata>
  <manifest>
    <item id="c1" href="chapters/ch1.xhtml" media-type="application/xhtml+xml"/>
    <item id="c2" href="chapters/ch2.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c2"/>
  </spine>
</package>"#;
        let ch1_xhtml = br#"<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 1</title></head>
<body>
  <h1>Chapter 1</h1>
  <p>In my younger and more vulnerable years my father gave me some advice that I've been turning over in my mind ever since.</p>
</body>
</html>"#;
        let ch2_xhtml = br#"<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>Chapter 2</title></head>
<body>
  <h1>Chapter 2</h1>
  <p>About half way between West Egg and New York the motor road hastily joins the railroad.</p>
</body>
</html>"#;

        let epub_bytes = crate::system::zip::create_test_zip(&[
            ("META-INF/container.xml", &container_xml[..]),
            ("OEBPS/content.opf", &opf_xml[..]),
            ("OEBPS/chapters/ch1.xhtml", &ch1_xhtml[..]),
            ("OEBPS/chapters/ch2.xhtml", &ch2_xhtml[..]),
        ]);

        let page = EpubExtractor::extract_from_bytes(&epub_bytes, None).unwrap();
        assert_eq!(page.title.as_deref(), Some("The Great Gatsby"));
        assert!(
            page.content
                .contains("In my younger and more vulnerable years")
        );
        assert!(page.content.contains("About half way between West Egg"));
        assert!(page.word_count >= 20);

        let temp_dir = crate::fs_util::tempdir().unwrap();
        let epub_file = temp_dir.path().join("gatsby.epub");
        std::fs::write(&epub_file, &epub_bytes).unwrap();

        let scraper = WebScraper::new().unwrap();
        let doc = DocumentLoader::load_file(&epub_file, &scraper).unwrap();
        assert_eq!(doc.title.as_deref(), Some("The Great Gatsby"));
        assert!(doc.url.starts_with("file://"));
        assert!(
            doc.content
                .contains("In my younger and more vulnerable years")
        );
        assert_eq!(doc.word_count, page.word_count);
    }

    #[test]
    fn test_epub_deduplication_and_spine_limit() {
        let container_xml =
            br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles><rootfile full-path="content.opf"/></rootfiles>
</container>"#;
        let opf_xml = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Bomb Test</dc:title></metadata>
  <manifest>
    <item id="c1" href="ch1.html" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
    <itemref idref="c1"/>
    <itemref idref="c1"/>
  </spine>
</package>"#;
        let ch1_html = b"<html><body><p>Unique single chapter text content here.</p></body></html>";

        let bytes = crate::system::zip::create_test_zip(&[
            ("META-INF/container.xml", container_xml),
            ("content.opf", opf_xml),
            ("ch1.html", ch1_html),
        ]);
        let page = EpubExtractor::extract_from_bytes(&bytes, None).unwrap();
        let occurrences = page.content.matches("Unique single chapter text").count();
        assert_eq!(occurrences, 1);

        assert_eq!(
            resolve_epub_path("OEBPS%25/", "chapters/ch1.xhtml#p1"),
            "OEBPS%25/chapters/ch1.xhtml"
        );
        assert_eq!(
            resolve_epub_path("OEBPS/text/", "..\\..\\secret%2ffile.xhtml?v=1#top"),
            "secret/file.xhtml"
        );
        assert_eq!(
            resolve_epub_path("OEBPS/text/", "../images%5Cfig1.xhtml"),
            "OEBPS/images/fig1.xhtml"
        );
    }

    #[test]
    fn test_pdf_extract_text_bounded_enforces_limit() {
        let pdf_bytes =
            std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/document.pdf")).unwrap();
        let doc = pdf_extract::Document::load_mem(&pdf_bytes).unwrap();

        assert!(
            !PdfExtractor::extract_pdf_text_bounded(&doc, MAX_PDF_TEXT_BYTES)
                .unwrap()
                .trim()
                .is_empty()
        );
        assert!(
            PdfExtractor::extract_pdf_text_bounded(&doc, 16)
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum allowed size")
        );

        let mut flate_doc = pdf_extract::Document::with_version("1.5");
        let mut dict = pdf_extract::Dictionary::new();
        dict.set("Filter", pdf_extract::Object::Name(b"FlateDecode".to_vec()));
        let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut encoder, &vec![b'A'; 64 * 1024]).unwrap();
        let compressed = encoder.finish().unwrap();
        flate_doc.add_object(pdf_extract::Stream::new(dict, compressed));
        assert!(
            PdfExtractor::verify_pdf_stream_budgets(&flate_doc, 1024)
                .unwrap_err()
                .to_string()
                .contains("Decompressed PDF stream data exceeds")
        );
    }
}
