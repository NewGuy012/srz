use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

use crate::render;
use crate::scraper::WebScraper;
use crate::sources::pdf::PdfExtractor;

pub const MAX_FILE_BYTES: u64 = 20 * 1024 * 1024; // 20 MB limit for local files

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

    /// Validates and loads a local document (PDF, HTML, Markdown, or plain text).
    pub fn load_file(path: &Path, scraper: &WebScraper) -> Result<LocalDocument> {
        Self::validate_file(path)?;

        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("epub")) {
            anyhow::bail!(
                "Document format not supported: {:?}. Supported formats include PDF, HTML, Markdown, and plain text.",
                path
            );
        }

        let fallback_title = || {
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Local File".to_string())
        };

        let (content, word_count, title) = if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("pdf"))
        {
            let bytes = fs::read(path)
                .with_context(|| format!("Failed to read PDF file at {:?}", path))?;
            let page = PdfExtractor::extract_from_bytes(&bytes, Some(path))?;
            let t = page.title.unwrap_or_else(fallback_title);
            (page.content, page.word_count, t)
        } else {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("Failed to read file: {:?}", path))?;

            if render::is_html_path(path) {
                let norm = scraper.normalize(&path.display().to_string(), &raw)?;
                let t = norm.title.unwrap_or_else(fallback_title);
                (norm.content, norm.word_count, t)
            } else {
                let words = raw.split_whitespace().count();
                (raw, words, fallback_title())
            }
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
}
