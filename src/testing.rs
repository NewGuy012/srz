//! Test utilities, fixtures, and helpers for `srz` integration and unit testing.

use std::fs::File;
use std::io::Result;
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Temporary Directory Fixture
// ---------------------------------------------------------------------------

/// RAII temporary directory fixture for tests with automatic deletion on drop.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
}

impl TempDir {
    pub fn new() -> Result<Self> {
        Self::new_in(std::env::temp_dir(), "srz-test")
    }

    pub fn new_in(parent: impl AsRef<Path>, prefix: &str) -> Result<Self> {
        let parent = parent.as_ref();
        if !parent.exists() {
            std::fs::create_dir_all(parent)?;
        }
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir_name = format!("{}-{}-{:x}-{}", prefix, std::process::id(), nonce, count);
        let path = parent.join(dir_name);
        crate::system::fs_util::create_private_dir_all(&path)?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.path
    }
}

impl std::ops::Deref for TempDir {
    type Target = Path;
    fn deref(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Convenience function to create an RAII temporary directory for testing.
pub fn tempdir() -> Result<TempDir> {
    TempDir::new()
}

/// Opens a file with private permissions (0o600 on Unix) for writing test fixtures.
pub fn open_private_file_for_write(path: &Path) -> Result<File> {
    crate::system::fs_util::open_private_file_for_write(path)
}

// ---------------------------------------------------------------------------
// PDF Test Generation Fixtures
// ---------------------------------------------------------------------------

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
    build_pdf_with_stream_content(content)
}

// ---------------------------------------------------------------------------
// Re-exports of Test-Oriented Helpers
// ---------------------------------------------------------------------------

pub use crate::commands::ask::format_grounded_context;
pub use crate::config::cli::Config;
pub use crate::engine::chunker::TextChunker;
pub use crate::sources::scraper::clean_raw_text;
pub use crate::ui::render::{ensure_view_files_unique, render_html_document_with_reasoning};
pub use crate::ui::terminal::Ui;
