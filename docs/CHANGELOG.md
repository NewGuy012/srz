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

> Introduces automated cache retention policies and LRU pruning, expands news aggregation with dedicated RSS parsing, upgrades YouTube ingestion via Innertube API, and completes comprehensive security and performance hardening across all subsystems.

### Changes
- **Cache Retention & Storage Management**: Added configurable TTL expiration (`pages_ttl_days`, `search_ttl_days`, `max_size_mb`), opportunistic auto-pruning, and the `srz cache prune` subcommand with `--older-than` and `--max-size` flags while keeping DuckDB library records safe.
- **Cryptographic Cache Hashing**: Upgraded cache key hashing across all namespaces to 256-bit SHA-256 for persistent, collision-free, toolchain-independent indexing.
- **Live News Ingestion**: Implemented a streaming XML RSS parser resolving HTML5 void-element stripping and expanded default aggregation to 5 articles.
- **YouTube Transcripts & SSRF Defense**: Integrated YouTube's Innertube Player API supporting format 3 (`srv3`) XML/JSON3 captions, enforced strict YouTube host authority validation, and pinned caption downloads to HTTPS `api/timedtext`.
- **Prompt Injection Defense**: Hardened closing tag delimiter escaping (`escape_closing_tag`) against case and whitespace variations, sanitized prompt metadata headers, and encapsulated map-reduce synthesis chunks.
- **Terminal Display & BiDi Sanitization**: Enhanced terminal sanitization to strip 8-bit C1 controls, string-type sequences (OSC, DCS, SOS, PM, APC), and Unicode Bidirectional (BiDi) override controls across all direct output paths.
- **Resource Limits & Security Hardening**: Enforced 1 MB SSE line buffers and 10 MB content buffers, added HTTP client timeouts, rejected filesystem symlinks in `fs_util`, and enforced semver validation in installer scripts.
- **Performance & Architectural Refinements**: Eliminated duplicate search RAM in `srz browse`, introduced `Database::with_transaction` for ACID safety, and reorganized repository root into `assets/`, `docs/`, and `scripts/`.

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
