use crate::fs_util::read_file_bytes_limited;
use crate::render;
use crate::scraper::{ScrapedPage, WebScraper, bytes_to_lossy_utf8, clean_text, html_to_markdown};
use crate::system::text_util::{
    XmlEvent, XmlScanner, find_ascii_case_insensitive, url_decode, xml_attr, xml_local_name,
};
use crate::system::zip::ZipReader;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::Path;

pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024;
const MAX_PDF_TEXT_BYTES: usize = 50 * 1024 * 1024;
const MAX_EPUB_XML_BYTES: usize = 5 * 1024 * 1024;
const MAX_EPUB_CHAPTER_BYTES: usize = 15 * 1024 * 1024;
const MAX_EPUB_TOTAL_BYTES: usize = 50 * 1024 * 1024;
const MAX_EPUB_CHAPTER_COUNT: usize = 2000;

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

struct CountingSink(u64);

impl std::io::Write for CountingSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(buf.len() as u64);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn finalize_document_page(
    path: Option<&Path>,
    default_filename: &str,
    default_title: &str,
    explicit_title: Option<String>,
    raw_text: &str,
    empty_msg: &str,
) -> Result<ScrapedPage> {
    let (content, word_count) = clean_text(raw_text, true, 2);
    if word_count < 5 {
        let path_str = path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "provided document".to_string());
        anyhow::bail!("{empty_msg} ({path_str})");
    }
    let title = explicit_title
        .filter(|t| !t.trim().is_empty())
        .or_else(|| {
            path.and_then(|p| p.file_stem())
                .map(|s| s.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| default_title.to_string());
    let url = path
        .map(|p| format!("file://{}", p.display()))
        .unwrap_or_else(|| format!("file://{default_filename}"));

    Ok(ScrapedPage {
        url,
        title: Some(title),
        content,
        word_count,
    })
}

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
            let raw = bytes_to_lossy_utf8(bytes);
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
        let path = Path::new(source);
        let is_markdown = path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("md") || e.eq_ignore_ascii_case("markdown"));
        let prefix = &raw[..raw.floor_char_boundary(raw.len().min(2048))];
        let trimmed_prefix = prefix.trim_start();
        let looks_like_html = render::is_html_path(path)
            || (!is_markdown
                && (find_ascii_case_insensitive(trimmed_prefix, "<!doctype").is_some()
                    || find_ascii_case_insensitive(trimmed_prefix, "<html").is_some()
                    || find_ascii_case_insensitive(trimmed_prefix, "<body").is_some()));

        if looks_like_html && let Ok(norm) = scraper.normalize(source, &raw) {
            let t = norm.title.unwrap_or_else(fallback_title);
            (norm.content, norm.word_count, t)
        } else {
            let words = raw.split_whitespace().count();
            (raw, words, fallback_title())
        }
    }
}

pub struct PdfExtractor;

impl PdfExtractor {
    /// Extracts text from a PDF file path.
    pub fn extract_file(path: &Path) -> Result<ScrapedPage> {
        let bytes = read_file_bytes_limited(path, MAX_FILE_BYTES, false, "PDF")?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    fn count_flate_decompressed(data: &[u8], remaining: u64) -> Result<u64> {
        use std::io::Read;
        let mut sink = CountingSink(0);
        let mut zdec = flate2::read::ZlibDecoder::new(data).take(remaining + 1);
        let zres = std::io::copy(&mut zdec, &mut sink);
        if zres.is_err() && sink.0 == 0 && data.len() > 2 {
            let mut rdec = flate2::read::DeflateDecoder::new(&data[2..]).take(remaining + 1);
            let _ = std::io::copy(&mut rdec, &mut sink);
        }
        if sink.0 > remaining {
            anyhow::bail!(
                "Decompressed PDF stream data exceeds maximum allowed size of {} bytes",
                MAX_PDF_TEXT_BYTES
            );
        }
        Ok(sink.0)
    }

    /// Pre-flights raw `/ObjStm` and `/XRef` Flate streams before `Document::load_mem` parses them eagerly.
    fn preflight_raw_object_streams(bytes: &[u8]) -> Result<()> {
        let mut remaining = MAX_PDF_TEXT_BYTES as u64;
        for marker in [b"/ObjStm".as_slice(), b"/XRef".as_slice()] {
            let mut cursor = 0;
            while let Some(rel) = bytes[cursor..]
                .windows(marker.len())
                .position(|w| w == marker)
            {
                let start_search = cursor + rel + marker.len();
                cursor = start_search;
                let window_end = bytes.len().min(start_search + 4096);
                if let Some(st_rel) = bytes[start_search..window_end]
                    .windows(6)
                    .position(|w| w == b"stream")
                {
                    let between = &bytes[start_search..start_search + st_rel];
                    if !between.trim_ascii_end().ends_with(b">>")
                        || between.windows(6).any(|w| w == b"endobj")
                    {
                        continue;
                    }
                    let mut stream_start = start_search + st_rel + 6;
                    if bytes.get(stream_start) == Some(&b'\r') {
                        stream_start += 1;
                    }
                    if bytes.get(stream_start) == Some(&b'\n') {
                        stream_start += 1;
                    }
                    if let Some(end_rel) = bytes[stream_start..]
                        .windows(9)
                        .position(|w| w == b"endstream")
                    {
                        let slice = &bytes[stream_start..stream_start + end_rel];
                        let decoded = Self::count_flate_decompressed(slice, remaining)?;
                        remaining = remaining.saturating_sub(decoded);
                    }
                }
            }
        }
        Ok(())
    }

    fn verify_pdf_stream_budgets(doc: &pdf_extract::Document, max_bytes: usize) -> Result<()> {
        let mut remaining = max_bytes as u64;
        let mut protected_ids = HashSet::new();

        for obj in doc.objects.values() {
            let dict = match obj {
                pdf_extract::Object::Dictionary(d) => d,
                pdf_extract::Object::Stream(s) => &s.dict,
                _ => continue,
            };
            if let Ok(contents) = dict.get(b"Contents") {
                match contents {
                    pdf_extract::Object::Reference(id) => {
                        protected_ids.insert(*id);
                    }
                    pdf_extract::Object::Array(arr) => {
                        for item in arr {
                            if let Ok(id) = item.as_reference() {
                                protected_ids.insert(id);
                            }
                        }
                    }
                    _ => {}
                }
            }
            if let Ok(id) = dict.get(b"ToUnicode").and_then(|o| o.as_reference()) {
                protected_ids.insert(id);
            }
        }

        for (&obj_id, obj) in &doc.objects {
            let pdf_extract::Object::Stream(stream) = obj else {
                continue;
            };
            let is_unreferenced_image = !protected_ids.contains(&obj_id)
                && stream
                    .dict
                    .get(b"Subtype")
                    .and_then(|o| o.as_name())
                    .is_ok_and(|n| n == b"Image");
            if is_unreferenced_image {
                continue;
            }

            let Ok(filters) = stream.filters() else {
                continue;
            };
            if filters.len() > 1 {
                anyhow::bail!(
                    "Unsupported chained PDF stream filters in preflight ({} stages)",
                    filters.len()
                );
            }
            match filters.first().copied() {
                Some(b"FlateDecode") => {
                    let decoded = Self::count_flate_decompressed(&stream.content, remaining)?;
                    remaining = remaining.saturating_sub(decoded);
                }
                Some(b"LZWDecode") => {
                    anyhow::bail!("Unsupported unbounded LZWDecode PDF stream in preflight");
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn extract_pdf_text_bounded(
        doc: &pdf_extract::Document,
        max_text_bytes: usize,
    ) -> Result<String> {
        Self::verify_pdf_stream_budgets(doc, max_text_bytes)?;
        let mut writer = BoundedIoWriter {
            buf: Vec::new(),
            max_bytes: max_text_bytes,
            exceeded: false,
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

    /// Extracts text from in-memory PDF bytes.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        Self::preflight_raw_object_streams(bytes)?;
        let doc_res = pdf_extract::Document::load_mem(bytes);
        let mut doc = match doc_res {
            Ok(d) => d,
            Err(_)
                if bytes[..bytes.len().min(1024)]
                    .windows(5)
                    .any(|w| w == b"%PDF-")
                    && bytes.windows(2).any(|w| w == b"\r\n") =>
            {
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
        if doc.is_encrypted()
            && let Err(err) = doc.decrypt("")
        {
            anyhow::bail!("PDF is password-protected or encrypted ({err})");
        }
        let raw_text = Self::extract_pdf_text_bounded(&doc, MAX_PDF_TEXT_BYTES)?;
        let title = Self::extract_title_from_doc(&doc);
        let processed = Self::post_process_text(&raw_text);
        finalize_document_page(
            path,
            "document.pdf",
            "PDF Document",
            title,
            &processed,
            "No readable text found in PDF. This appears to be a scanned image-only PDF without an embedded text layer.",
        )
    }

    fn decode_pdf_text_string(raw: &[u8]) -> String {
        if let Some(rest) = raw.strip_prefix(&[0xFE, 0xFF]) {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_be_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else if let Some(rest) = raw.strip_prefix(&[0xFF, 0xFE]) {
            let units: Vec<u16> = rest
                .as_chunks::<2>()
                .0
                .iter()
                .map(|c| u16::from_le_bytes(*c))
                .collect();
            String::from_utf16_lossy(&units)
        } else {
            String::from_utf8_lossy(raw).into_owned()
        }
    }

    fn extract_title_from_doc(doc: &pdf_extract::Document) -> Option<String> {
        let info = doc
            .trailer
            .get(b"Info")
            .and_then(|obj| doc.dereference(obj))
            .ok()?;
        let dict = info.1.as_dict().ok()?;
        let raw = dict.get(b"Title").and_then(|o| o.as_str()).ok()?;
        let decoded = Self::decode_pdf_text_string(raw).trim().to_string();
        (!decoded.is_empty()).then_some(decoded)
    }

    fn looks_like_numbered_heading(nt: &str) -> bool {
        let digits = nt.bytes().take_while(|b| b.is_ascii_digit()).count();
        if !(1..=3).contains(&digits) || nt.len() >= 80 {
            return false;
        }
        let rest = &nt[digits..];
        rest.starts_with(". ") && rest[2..].chars().next().is_some_and(|c| c.is_uppercase())
    }

    fn post_process_text(raw: &str) -> String {
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

            if trimmed.len() > 1
                && trimmed.ends_with('-')
                && !trimmed[..trimmed.len() - 1].ends_with(char::is_whitespace)
                && lines.peek().is_some_and(|n| !n.trim().is_empty())
            {
                cleaned.push_str(&trimmed[..trimmed.len() - 1]);
            } else {
                cleaned.push_str(trimmed);
                let next_is_heading_or_start = lines.peek().is_some_and(|n| {
                    let nt = n.trim();
                    nt.starts_with(['#', '•'])
                        || nt.starts_with("- ")
                        || nt.starts_with("* ")
                        || Self::looks_like_numbered_heading(nt)
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

struct EpubExtractor;

impl EpubExtractor {
    fn extract_file(path: &Path) -> Result<ScrapedPage> {
        let bytes = read_file_bytes_limited(path, MAX_FILE_BYTES, false, "EPUB")?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
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
                .min(MAX_EPUB_CHAPTER_BYTES);
            if raw_cap == 0 {
                break;
            }
            let file_bytes = match archive.read_file(&chapter_path, raw_cap) {
                Ok(b) => b,
                Err(e) => {
                    if archive.file_names().any(|n| n == chapter_path) {
                        return Err(e).with_context(|| {
                            format!("Failed to decompress EPUB chapter '{chapter_path}'")
                        });
                    }
                    continue;
                }
            };
            total_decompressed = total_decompressed.saturating_add(file_bytes.len());
            let html = String::from_utf8_lossy(&file_bytes);
            let md = html_to_markdown(&html);
            let trimmed = md.trim();
            let remaining = MAX_EPUB_TOTAL_BYTES.saturating_sub(total_bytes);
            let slice = &trimmed[..trimmed.floor_char_boundary(remaining.min(trimmed.len()))];
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

        finalize_document_page(
            path,
            "document.epub",
            "EPUB Document",
            title,
            &book_markdown,
            "No readable text found in EPUB. The file may be empty, image-only, or encrypted with DRM.",
        )
    }
}

fn find_opf_path(archive: &ZipReader) -> Result<String> {
    if let Ok(xml_bytes) = archive.read_file("META-INF/container.xml", MAX_EPUB_XML_BYTES)
        && let Ok(xml) = std::str::from_utf8(&xml_bytes)
        && let Some(path) = parse_container_xml(xml)
    {
        return Ok(path);
    }

    for name in archive.file_names() {
        if crate::system::text_util::ends_with_ascii_case_insensitive(name, ".opf") {
            return Ok(name.to_string());
        }
    }

    anyhow::bail!("Invalid EPUB: no OPF package file found in archive")
}

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

fn parse_opf(archive: &ZipReader, opf_path: &str) -> Result<(Option<String>, Vec<String>)> {
    let opf_bytes = archive
        .read_file(opf_path, MAX_EPUB_XML_BYTES)
        .with_context(|| format!("Failed to read OPF file at {}", opf_path))?;
    let opf_xml = std::str::from_utf8(&opf_bytes).context("OPF XML is not valid UTF-8")?;

    let opf_dir = match opf_path.rfind('/') {
        Some(idx) => &opf_path[..=idx],
        None => "",
    };

    const MAX_EPUB_MANIFEST_ITEMS: usize = 16_384;
    let mut scanner = XmlScanner::new(opf_xml);
    let mut title: Option<String> = None;
    let mut manifest: HashMap<String, String> = HashMap::new();
    let mut manifest_order: Vec<(String, String)> = Vec::new();
    let mut spine: Vec<String> = Vec::new();

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
                        manifest.insert(id.to_string(), href.to_string());
                        manifest_order.push((href.to_string(), mt));
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
    for idref in &spine {
        if let Some(href) = manifest.get(idref) {
            chapter_paths.push(resolve_epub_path(opf_dir, href));
            if chapter_paths.len() >= MAX_EPUB_CHAPTER_COUNT {
                break;
            }
        }
    }

    if chapter_paths.is_empty() {
        for (href, media_type) in manifest_order {
            let is_html = ["html", "xml"]
                .iter()
                .any(|m| find_ascii_case_insensitive(&media_type, m).is_some())
                || [".xhtml", ".html", ".htm"]
                    .iter()
                    .any(|e| crate::system::text_util::ends_with_ascii_case_insensitive(&href, e));
            if is_html {
                chapter_paths.push(resolve_epub_path(opf_dir, &href));
                if chapter_paths.len() >= MAX_EPUB_CHAPTER_COUNT {
                    break;
                }
            }
        }
    }

    Ok((title, chapter_paths))
}

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
    fn test_post_process_text_and_html_detection() {
        let raw = "Implemen-\ntation done.\n2024 was good. Wait\nDash - keep\n\n";
        let processed = PdfExtractor::post_process_text(raw);
        assert!(processed.contains("Implementation done."));
        assert!(processed.contains("2024 was good. Wait"));
        assert!(processed.contains("Dash - keep"));
        assert_eq!(PdfExtractor::post_process_text("   \n\n "), "");

        let scraper = WebScraper::new().unwrap();
        let md_with_html = "Tutorial mentions <html> and <body> tags.\n# Real Title";
        let (out, _, _) = DocumentLoader::normalize_or_plain_text(
            &scraper,
            "/tmp/guide.md",
            md_with_html.to_string(),
            || "guide".to_string(),
        );
        assert_eq!(out, md_with_html);
    }

    #[test]
    fn test_pdf_bounds_recovery_utf16_and_preflight_bypasses() {
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
        let page = PdfExtractor::extract_from_bytes(&crlf_bytes, None).unwrap();
        assert!(page.content.contains("THE HISTORY OF RUST"));

        let doc = pdf_extract::Document::load_mem(&original_bytes).unwrap();
        assert!(
            PdfExtractor::extract_pdf_text_bounded(&doc, 16)
                .unwrap_err()
                .to_string()
                .contains("exceeds maximum allowed size")
        );

        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        std::io::Write::write_all(&mut enc, &vec![b'A'; 64 * 1024]).unwrap();
        let mut bad_adler = enc.finish().unwrap();
        let last = bad_adler.len() - 1;
        bad_adler[last] ^= 0xff; // Corrupt Adler-32 trailer (Bypass #6)

        // Spoofed /Subtype /Image on a page's /Contents stream must NOT skip preflight (Bypass #4)
        let mut bypass_doc = pdf_extract::Document::with_version("1.5");
        let mut sdict = pdf_extract::Dictionary::new();
        sdict.set("Subtype", "Image");
        sdict.set("Filter", "FlateDecode");
        let sid = bypass_doc.add_object(pdf_extract::Stream::new(sdict, bad_adler));
        let mut pdict = pdf_extract::Dictionary::new();
        pdict.set("Contents", pdf_extract::Object::Reference(sid));
        bypass_doc.add_object(pdict);
        assert!(PdfExtractor::verify_pdf_stream_budgets(&bypass_doc, 1024).is_err());

        // Chained filters & LZWDecode must be rejected (Bypasses #2 & #3)
        let mut chain_doc = pdf_extract::Document::with_version("1.5");
        let mut cdict = pdf_extract::Dictionary::new();
        cdict.set("Filter", vec!["FlateDecode".into(), "FlateDecode".into()]);
        chain_doc.add_object(pdf_extract::Stream::new(cdict, b"x".to_vec()));
        assert!(PdfExtractor::verify_pdf_stream_budgets(&chain_doc, 1024).is_err());

        let mut lzw_doc = pdf_extract::Document::with_version("1.5");
        let mut ldict = pdf_extract::Dictionary::new();
        ldict.set("Filter", "LZWDecode");
        lzw_doc.add_object(pdf_extract::Stream::new(ldict, b"x".to_vec()));
        assert!(PdfExtractor::verify_pdf_stream_budgets(&lzw_doc, 1024).is_err());

        let be_utf16: Vec<u8> = [0xFE, 0xFF, 0x00, b'R', 0x00, b'u', 0x00, b's', 0x00, b't'].into();
        assert_eq!(PdfExtractor::decode_pdf_text_string(&be_utf16), "Rust");
    }

    #[test]
    fn test_epub_extractor_dedup_lossy_utf8_and_path_resolution() {
        let container_xml = br#"<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container"><rootfiles><rootfile full-path="OEBPS/content.opf"/></rootfiles></container>"#;
        let opf_xml = br#"<package xmlns="http://www.idpf.org/2007/opf" version="3.0"><metadata xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>The Great Gatsby</dc:title></metadata><manifest><item id="c1" href="ch1.xhtml" media-type="application/xhtml+xml"/><item id="c2" href="ch2.xhtml" media-type="application/xhtml+xml"/></manifest><spine><itemref idref="c1"/><itemref idref="c1"/><itemref idref="c2"/></spine></package>"#;
        let ch1 = b"<html><head><title>IgnoredHead</title><style>p{color:red}</style></head><body><h1>Ch1</h1><p>In my younger\xa0years my father gave me advice.</p></body></html>";
        let ch2 = b"<html><body><p>Between West Egg and New York.</p></body></html>";

        let epub_bytes = crate::system::zip::create_test_zip(&[
            ("META-INF/container.xml", &container_xml[..]),
            ("OEBPS/content.opf", &opf_xml[..]),
            ("OEBPS/ch1.xhtml", &ch1[..]),
            ("OEBPS/ch2.xhtml", &ch2[..]),
        ]);

        let temp_dir = crate::fs_util::tempdir().unwrap();
        let epub_file = temp_dir.path().join("gatsby.epub");
        std::fs::write(&epub_file, &epub_bytes).unwrap();
        let doc = DocumentLoader::load_file(&epub_file, &WebScraper::new().unwrap()).unwrap();
        assert_eq!(doc.title.as_deref(), Some("The Great Gatsby"));
        assert!(doc.url.starts_with("file://"));
        assert_eq!(doc.content.matches("In my younger").count(), 1);
        assert!(doc.content.contains("West Egg"));
        assert!(!doc.content.contains("IgnoredHead"));
        assert!(!doc.content.contains("color:red"));

        assert_eq!(
            resolve_epub_path("OEBPS/text/", "..\\..\\secret%2ffile.xhtml?v=1#top"),
            "secret/file.xhtml"
        );
    }
}
