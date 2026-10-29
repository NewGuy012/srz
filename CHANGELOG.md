# Changelog

All notable changes to `srz` are documented in this file.

## Release Notes Template

```markdown
## What's Changed in vX.Y.Z

> One to two sentence summary describing the primary focus of this release.

### Changes
- **Feature / Component**: Description.
- **Improvement / Refactor**: Description.
- **Fix / Security**: Description.
```

---

## v0.4.0

> Adds automated cache retention policies and size-based LRU pruning, fixes Google News RSS and YouTube transcript extraction, and expands default news aggregation to 5 articles.

### Changes
- **Cache Retention & Pruning**: Added configurable TTL expiration (`pages_ttl_days`, `search_ttl_days`, `max_size_mb`), opportunistic auto-pruning, and the `srz cache prune` subcommand with `--older-than` and `--max-size` flags.
- **Safe Cache Clean**: Decoupled database history reset from cache cleaning; `srz cache clean` safely removes cache files while preserving DuckDB library records unless `--reset-db` is explicitly passed.
- **Google News RSS Fix**: Implemented a dedicated XML RSS parser to resolve HTML5 void-element link stripping, adding CDATA/entity decoding, `<guid>` fallbacks, and buffering candidates.
- **News Summary Expansion**: Updated default news limit to 5 top articles across `srz news` and auto-detected queries.
- **YouTube Transcripts**: Integrated YouTube's Innertube Player API to bypass web-only PoToken bot challenges and added support for format 3 (`srv3`) timed text XML.

---

## v0.3.0

> Introduces an interactive terminal library explorer (`srz browse`), grounded document Q&A (`srz ask`), YouTube caption ingestion, native PDF extraction, and completes Phase 3 security hardening.

### Changes
- **Interactive TUI Explorer**: Added `srz browse`, a full-screen terminal interface powered by `ratatui` with live search filtering, library inspection, and reader view.
- **Grounded Q&A**: Added `srz ask` for targeted single-turn queries and multi-turn chat grounded in summarized document context.
- **YouTube Ingestion**: Added support for summarizing YouTube video URLs with automatic timed-text closed-caption extraction.
- **Native PDF Extraction**: Implemented pure-Rust in-memory PDF extraction without external dependencies.
- **Terminal Markdown & Paging**: Added syntax highlighting and rich markdown formatting in terminal output with automatic `less` pager integration.
- **Security Hardening**: Restored 100% safe Rust, added prompt delimiter framing for untrusted text, sanitized ANSI/OSC terminal escapes, and enforced bounded HTTP stream readers.
- **Architecture & Performance**: Modularized codebase into dedicated domain directories (`commands`, `sources`, `storage`, `ui`), introduced shared HTTP connection pooling, and optimized streaming markdown rendering.

---

## v0.2.0

> Adds an in-place self-updater (`srz upgrade`), automated Homebrew tap integration, and cross-platform support for Windows PowerShell.

### Changes
- **Self-Update Utility**: Added `srz upgrade` for in-place binary upgrades with automatic GitHub Release checks, SHA-256 verification, and atomic replacement.
- **Install Origin Detection**: Added environment detection to determine if `srz` was installed via Homebrew, Linuxbrew, or direct binary download.
- **Windows Support**: Added `x86_64-pc-windows-msvc` target builds and a one-line PowerShell installer (`install.ps1`).
- **Automation & Pipelines**: Added automated formula bumping for `NewGuy012/homebrew-tap` and dynamic release version resolution.
- **Network Resilience**: Added automatic HTTP redirect handling for release binary downloads.

---

## v0.1.0

> Initial public release of `srz`: a modular CLI to summarize webpages, documents, books, and live news using local LLMs.

### Changes
- **Multi-Source Summarization**: Added support for summarizing URLs, local text files, and piped standard input (`srz file.txt`, `cat doc.txt | srz -`).
- **Live News Search**: Added `srz news` to query and summarize real-time articles via Google News RSS.
- **Book Metadata Resolution**: Added automatic metadata enrichment via Open Library and Wikipedia OpenSearch.
- **Streaming Telemetry**: Implemented Server-Sent Events (SSE) streaming with live token throughput tracking and thinking counters.
- **Embedded Database**: Integrated DuckDB for local reading history, token consumption metrics, and full-text library search.
- **Summary Styles**: Added presets for `concise`, `detailed`, `tldr`, `critique`, `eli5`, and `flashcards`.
- **Export & Viewing**: Added automatic styled HTML/Markdown view generation and standalone library dashboard (`srz index`).
- **Configuration & Tooling**: Added profile configuration (`~/.config/srz/config.toml`), shell completion generation (`srz completions`), and JSON output mode (`--json`).
