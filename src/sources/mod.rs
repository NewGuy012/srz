pub mod news;
pub mod scraper;
pub mod search;
pub mod youtube;

pub use news::GoogleNewsClient;
pub use scraper::WebScraper;
pub use search::BookSearcher;
pub use youtube::{extract_youtube_video_id, fetch_youtube_transcript};
