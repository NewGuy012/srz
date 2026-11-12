use crate::system::text_util::{XmlEvent, XmlScanner, xml_attr, xml_local_name};
use crate::system::zip::ZipReader;
use anyhow::{Context, Result};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::Path;

use crate::render;
use crate::scraper::{ScrapedPage, WebScraper, clean_text, html_to_markdown};
use crate::system::text_util::url_decode;

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
}

// ---------------------------------------------------------------------------
// EPUB Extractor
// ---------------------------------------------------------------------------

pub struct EpubExtractor;

impl EpubExtractor {
    /// Extracts text and metadata from an EPUB file path.
    pub fn extract_file(path: &Path) -> Result<ScrapedPage> {
        DocumentLoader::validate_file(path)?;
        let bytes =
            fs::read(path).with_context(|| format!("Failed to open EPUB file at {:?}", path))?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    /// Extracts text and metadata from in-memory EPUB bytes.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        let archive = ZipReader::new(bytes).context("Failed to open EPUB as zip archive")?;

        let opf_path = find_opf_path(&archive)?;
        let (title, chapters) = parse_opf(&archive, &opf_path)?;

        let mut book_markdown = String::new();
        let mut total_bytes: usize = 0;
        let mut seen_chapters = HashSet::new();

        for chapter_path in chapters {
            if !seen_chapters.insert(chapter_path.clone()) {
                continue;
            }
            if let Ok(file_bytes) =
                archive.read_file(&chapter_path, MAX_EPUB_CHAPTER_BYTES as usize)
                && let Ok(html) = std::str::from_utf8(&file_bytes)
            {
                let md = html_to_markdown(html);
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
                parse_item_attr(attrs, &mut manifest);
            } else if local.eq_ignore_ascii_case("itemref") {
                parse_itemref_attr(attrs, &mut spine);
            }
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

fn parse_item_attr(attrs: &str, manifest: &mut HashMap<String, (String, String)>) {
    let id = xml_attr(attrs, "id")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let href = xml_attr(attrs, "href")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let media_type = xml_attr(attrs, "media-type")
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    if !id.is_empty() && !href.is_empty() {
        manifest.insert(id, (href, media_type));
    }
}

fn parse_itemref_attr(attrs: &str, spine: &mut Vec<String>) {
    if spine.len() >= MAX_EPUB_CHAPTER_COUNT {
        return;
    }
    if let Some(val) = xml_attr(attrs, "idref") {
        let idref = val.trim();
        if !idref.is_empty() {
            spine.push(idref.to_string());
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
        let buffer = crate::testing::build_pdf_with_stream_content(content);
        let page = PdfExtractor::extract_from_bytes(&buffer, None).unwrap();
        assert!(page.content.contains("Hello from Rust PDF extractor"));
        assert_eq!(page.title.as_deref(), Some("PDF Document"));
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
    }
}
