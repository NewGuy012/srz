pub mod document;
pub mod news;
pub mod scraper;
pub mod search;
pub mod youtube;

pub use document::{
    DocumentLoader, EpubExtractor, LocalDocument, MAX_EPUB_CHAPTER_BYTES, MAX_EPUB_CHAPTER_COUNT,
    MAX_EPUB_TOTAL_BYTES, MAX_EPUB_XML_BYTES, MAX_FILE_BYTES, MAX_PDF_TEXT_BYTES, PdfExtractor,
};
pub use news::GoogleNewsClient;
pub use scraper::WebScraper;
pub use search::{BookSearcher, is_valid_work_key, parse_work_description};
pub use youtube::{extract_youtube_video_id, fetch_youtube_transcript};

#[cfg(test)]
mod tests {
    use super::document::{MAX_PDF_TEXT_BYTES, PdfExtractor, read_file_bytes_limited};

    #[test]
    fn test_read_file_bytes_limited_enforces_size_and_type() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let file = tmp.path().join("sample.bin");
        std::fs::write(&file, b"0123456789").unwrap();

        let exact = read_file_bytes_limited(&file, 10, "Test").unwrap();
        assert_eq!(exact, b"0123456789");

        let err = read_file_bytes_limited(&file, 9, "Test").unwrap_err();
        assert!(err.to_string().contains("too large"));

        let dir_err = read_file_bytes_limited(tmp.path(), 1024, "Test").unwrap_err();
        assert!(dir_err.to_string().contains("not a regular file"));
    }

    #[test]
    fn test_pdf_extract_text_bounded_enforces_limit() {
        let pdf_bytes =
            std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/document.pdf")).unwrap();
        let doc = pdf_extract::Document::load_mem(&pdf_bytes).unwrap();

        let extracted = PdfExtractor::extract_pdf_text_bounded(&doc, MAX_PDF_TEXT_BYTES).unwrap();
        assert!(!extracted.trim().is_empty());

        let err = PdfExtractor::extract_pdf_text_bounded(&doc, 16).unwrap_err();
        assert!(err.to_string().contains("exceeds maximum allowed size"));
    }
}
