use anyhow::{Context, Result};
use std::path::Path;

use crate::scraper::ScrapedPage;

pub struct PdfExtractor;

impl PdfExtractor {
    /// Extracts text from a PDF file path.
    pub fn extract_file(path: &Path) -> Result<ScrapedPage> {
        crate::sources::document::DocumentLoader::validate_file(path)?;
        let bytes = std::fs::read(path)
            .with_context(|| format!("Failed to read PDF file at {:?}", path))?;
        Self::extract_from_bytes(&bytes, Some(path))
    }

    /// Extracts text from in-memory PDF bytes, parsing the PDF document structure only once.
    pub fn extract_from_bytes(bytes: &[u8], path: Option<&Path>) -> Result<ScrapedPage> {
        let mut doc =
            pdf_extract::Document::load_mem(bytes).context("Failed to parse PDF document")?;
        if doc.is_encrypted() {
            let _ = doc.decrypt("");
        }
        let mut raw_text = String::new();
        pdf_extract::output_doc(&doc, &mut pdf_extract::PlainTextOutput::new(&mut raw_text))
            .context("Failed to extract text layer from PDF document")?;

        let (title, _author) = Self::extract_metadata_from_doc(&doc, path);
        let content = Self::post_process_text(&raw_text);
        let word_count = content.split_whitespace().count();

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
            let trimmed = line.trim();
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
                cleaned.push(' ');
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
        let buffer = PdfExtractor::build_pdf_with_stream_content(content);
        let page = PdfExtractor::extract_from_bytes(&buffer, None).unwrap();
        assert!(page.content.contains("Hello from Rust PDF extractor"));
        assert_eq!(page.title.as_deref(), Some("PDF Document"));
    }
}
