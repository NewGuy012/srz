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

> Introduces configurable cache retention policies and LRU pruning, expands news aggregation to 5 articles with dedicated RSS parsing, upgrades YouTube ingestion via Innertube API, optimizes TUI memory efficiency, and completes Phase 4 security hardening (SHA-256 cache hashing, C1/BiDi terminal sanitization, prompt delimiter hardening, SSRF prevention, and symlink rejection).

### Changes
- **Cache Retention & Size-Based Pruning**: Added configurable TTL expiration (`pages_ttl_days`, `search_ttl_days`, `summaries_ttl_days`, `views_ttl_days`, `max_size_mb`, `auto_prune`), opportunistic background auto-pruning, and the `srz cache prune` subcommand supporting `--older-than`, `--max-size`, and `--namespace` filters.
- **Safe Cache Cleaning**: Decoupled database history reset from cache cleaning; `srz cache clean` safely removes cached web and search files while preserving DuckDB library records unless `--reset-db` is explicitly passed.
- **Cryptographic SHA-256 Cache Hashing**: Replaced 64-bit `DefaultHasher` with 256-bit cryptographic SHA-256 (`Sha256Hasher` implementing `std::hash::Hasher`), generating persistent 64-character lowercase hex keys across all cache namespaces.
- **Google News RSS XML Parser**: Implemented a dedicated streaming XML RSS parser resolving HTML5 void-element link stripping, adding CDATA/entity decoding, `<guid>` fallbacks, and candidate deduplication.
- **Expanded News Aggregation**: Increased default news aggregation limit from 3 to 5 top articles across `srz news` and natural query auto-detection.
- **YouTube Ingestion & Innertube API**: Integrated YouTube's Innertube Player API to reliably extract captions bypassing web-only bot challenges, adding support for format 3 (`srv3`) and `json3` timed text transcripts.
- **YouTube Host Validation & Caption SSRF Defense**: Enforced strict URL authority validation in `extract_youtube_video_id` (`youtube.com`, `*.youtube.com`, `youtu.be`) and restricted caption track fetching to HTTPS `api/timedtext` endpoints to prevent SSRF against internal services or cloud metadata.
- **Prompt Injection Defense & Delimiter Hardening**: Upgraded `escape_closing_tag` to parse closing delimiters case-insensitively while tolerating tag whitespace variations (`</source_document>`, `< / source_document >`), sanitized prompt metadata headers (`Title: {}`, `Author: {}`, `Topic: {}`), and encapsulated map-reduce synthesis chunks within `<section_summaries>` blocks.
- **Terminal Escape & BiDi Sanitization**: Enhanced `sanitize_terminal_text` to strip 8-bit C1 controls (`U+0080`..=`U+009F`), string-type sequences (OSC, DCS, SOS, PM, APC), and Unicode Bidirectional (BiDi) override controls (`U+202A`..=`U+202E`, `U+2066`..=`U+2069`, `U+061C`, `U+200E`, `U+200F`) preventing Trojan Source UI spoofing. Applied terminal sanitization across all direct print sites (summary display, streaming chunks, disambiguation menus, history, find, and doctor commands).
- **Bounded Resource Limits & Streaming Timeouts**: Added `MAX_SSE_LINE_BYTES` (1 MB) and `MAX_CONTENT_BUFFER_BYTES` (10 MB) to SSE stream processing, replaced unconstrained `.json().await` calls in search and updater routines with bounded byte readers, and configured 5-second connect and 60-second request timeouts on HTTP clients.
- **Filesystem & Symlink Hardening**: Updated `fs_util::open_private_file_for_write` and `fs_util::create_private_dir_all` to inspect target paths and reject symlinks (`io::ErrorKind::InvalidInput`), and enforced private `0600` permissions on auto-prune marker writes.
- **Arithmetic & Conversion Safety**: Converted cache configuration calculations to saturating arithmetic and hardened float duration and byte parsers against `NaN`, infinity, negatives, and overflow.
- **Installer & Browser Launch Security**: Added strict semver regex validation for `$VERSION` in `install.sh` and `install.ps1`, and canonicalized paths in `open_in_browser` to absolute paths to prevent command-line argument injection.
- **TUI Memory & Performance Optimization**: Refactored `srz browse` (`AppState`) to perform zero-allocation on-the-fly case-insensitive search matching (`LibraryItem::matches_query`), eliminating duplicate lowercased search text blobs in RAM and avoiding deep clones during item deletion.
- **Database Transaction Safety**: Added `Database::with_transaction` helper to guarantee ACID atomicity and automatic rollback across library recording and deletion operations.
- **Codebase & CI Reorganization**: Cleaned repository root into `assets/`, `docs/`, and `scripts/` directories, added README usage test suites, and expanded `.github/workflows/ci.yml` path triggers to include `Cargo.toml`, `Cargo.lock`, and workflow files.

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
