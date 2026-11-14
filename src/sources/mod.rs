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
