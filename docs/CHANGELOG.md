# Changelog

All notable changes to `srz` are documented in this file.

## Release Notes Template

```markdown
## vX.Y.Z

### Summary

> One to two sentence summary describing the primary focus of this release.

### Changes
- **Feature / Component**: Description.
- **Improvement / Refactor**: Description.
- **Fix / Security**: Description.
```

---

## v0.6.0

### Summary

> Maximizes Rust crate ecosystem adoption, eliminates fragile custom parsing, and optimizes core pipelines with deduplicated token and word chunking, single-pass cache pruning, zero-allocation CLI query predicates, and consolidated DuckDB query mappers.

### Changes
- **Unified Text & Token Chunking (`engine::chunker`)**: Deduplicated paragraph splitting and overlap calculation between token and word chunking into a shared `chunk_with_metric` implementation. Added an $O(1)$ fast-path check when document byte length fits within the token budget.
- **Streaming Telemetry & Document Summarization Optimization (`engine::llm`)**: Streamlined `SseStreamState::append_content_delta` to deduplicate token rate calculations via `compute_tokens_per_second`. Removed unused legacy chunk constants and added early-exit context checks in `summarize_document_counted` to avoid redundant BPE tokenization for short texts.
- **Source Ingestion & Sanitization (`sources::youtube`, `commands::summarize`, `commands::ask`)**: Eliminated obsolete legacy XML parsing helpers in YouTube transcript ingestion, refactored Innertube player fetching into a clean configuration loop, switched video ID extraction to zero-allocation slice matching, and used ASCII case-insensitive checks for stdin HTML detection. Updated `ask` session hints to interpolate the active numeric session ID.
- **Storage Layer & Pruning Optimization (`storage::cache`, `storage::db`)**: Refactored `Cache::prune` to collect surviving file metadata during Pass 1, eliminating redundant filesystem re-scans in Pass 2 (LRU size eviction). Separated disk I/O routines (`read_disk_entry`, `write_disk_entry`) to prevent double-locking DuckDB mutexes. Centralized DuckDB security extension lockdown pragmas and query row mappers for chat sessions and search results, pre-tokenized search terms for snippet extraction, and optimized `tokenize_terms` to filter empty tokens before lowercase conversion.
- **UI & Query Parsing Optimization (`ui::render`, `config::cli`)**: Removed obsolete manual markdown inline rendering helpers (`render_inline`, `render_inline_to`), used zero-allocation `escape_html_cow` in HTML document and card rendering, and optimized CLI query classification (`is_news_query`, `clean_news_query`, `is_probable_question`) to eliminate intermediate string and vector heap allocations.
- **Cryptographic Hex Formatting (`system::text_util`)**: Unified SHA-256 and byte-to-hex formatting across cache key hashing and release verification into a single-pass `format_hex` helper.
- **Universal Syndication Feeds (`sources::news`)**: Replaced hand-rolled XML string matching with `feed-rs`, adding robust support for RSS 0.9/1.0/2.0, Atom, and JSON Feed formats while keeping Google News recency query resolution and URL un-obfuscation as the default.
- **High-Fidelity Article Extraction (`sources::scraper`)**: Integrated Mozilla Readability DOM density scoring via `dom_smoothie` into `WebScraper::normalize`. Automatically strips advertising wrappers, navigation bars, cookie prompts, and sidebars from arbitrary web pages before LLM synthesis.
- **Token-Budgeted Document Chunking (`engine::chunker`)**: Added exact OpenAI BPE token counting (`TextChunker::count_tokens`) and token-budgeted paragraph chunking (`TextChunker::chunk_by_tokens`) via `tiktoken-rs`, aligning map-reduce chunk boundaries with model context windows (`MAX_CONTEXT_TOKENS = 16_000`, `CHUNK_MAX_TOKENS = 12_000`).
- **HTML Entity Decoding (`system::text_util`)**: Replaced manual substring matching loops in `decode_html_entities` with `html-escape`, providing high-performance, compliant decoding while preserving zero-copy borrowed strings when no entities are present.
- **YouTube XML Captions (`sources::youtube`)**: Replaced manual byte slicing and tag scanning in `parse_xml_timedtext` and `parse_xml_format3_timedtext` with `quick-xml` streaming event readers, handling timed text without custom cursor math.
- **GitHub Flavored Markdown Rendering (`ui::render`)**: Integrated `pulldown-cmark` into `markdown_to_html`, replacing over 170 lines of custom parser code with compliant CommonMark/GFM rendering (supporting tables, strikethrough, and task lists) and defensive HTML tag sanitization.
- **Cross-Platform Browser Launcher (`ui::render`)**: Replaced platform-conditional subprocess branches (`open`, `xdg-open`, `rundll32.exe`) with `open::that_detached` for non-blocking browser launching across macOS, Linux, and Windows.
- **Interactive REPL History & Line Editing (`commands::ask`)**: Upgraded `srz ask -i` interactive chat REPL using `rustyline`, providing in-line arrow-key editing, history navigation, clean Ctrl+C/Ctrl+D termination, and preloading prior turns from resumed chat sessions.

---

## v0.5.0

### Summary

> Expands DuckDB utilization across the engine with offline BM25 library search, reading habit and model telemetry in `srz stats`, persistent multi-turn chat sessions with resumption for `srz ask`, consolidated DuckDB caching, and CI/workflow modernization.

### Changes
- **BM25 Full-Text Search (`srz find`)**: Implemented an offline inverted term index table (`library_terms`) in DuckDB with $2.5\times$ title boosting, term frequency saturation ($k_1=1.2, b=0.75$), and Robertson-Sparck Jones IDF calculation for relevance-ranked search results and snippet extraction.
- **Habit & Model Telemetry (`srz stats`)**: Added analytics for active reading days, reading streaks, estimated reading time saved (225 wpm baseline), top source domains, and model P50/P95 token throughput percentiles using pure DuckDB SQL.
- **Persistent Multi-Turn Chat Sessions (`srz ask`)**: Added chat session storage in DuckDB (`chat_sessions`, `chat_messages`) supporting `--chats` (list sessions), `--resume <ID>` (continue conversation), `--show-chat <ID>` (transcript review), and `--delete-chat <ID>`.
- **DuckDB Cache Consolidation & Millisecond TTL Pruning**: Consolidated ephemeral disk caches (pages, search metadata, summaries) into embedded `cache.duckdb` with atomic transactions, millisecond-accurate TTL pruning via `epoch_ms(cached_at)`, and transparent JSON fallback.
- **Search & Render Performance**: Optimized `LibraryItem::matches_query` with early metadata short-circuiting, implemented zero-copy `Cow::Borrowed` multi-byte HTML escaping (`escape_html_cow`), and unified canonical target metadata formatting.
- **Prompt Framing & Delimiter Consolidation**: Replaced fragmented manual tag escaping with a centralized `format_untrusted_section` helper across all LLM pipelines.
- **Centralized Text Utilities (`system::text_util`)**: Unified HTML entity decoding and ASCII case-insensitive search into `src/system/text_util.rs`.
- **CLI Flag & Subcommand Simplification**: Streamlined CLI interface by hiding redundant subcommands (`web`, `book`, `file`) from `--help` in favor of primary root arguments (`srz <TARGET>`), and removed legacy flags.
- **CI & Workflow Modernization**: Upgraded GitHub Actions workflows to Node 24 (`actions/checkout@v5`, `mozilla-actions/sccache-action@v0.0.11`), fixed `rust-cache` action inputs, pinned stable `ubuntu-24.04` runners, and fixed cache TTL test flakiness on Windows runners.
- **License & Dependency Audits (`deny.toml`)**: Configured `cargo-deny` with allowed licenses (MPL-2.0, CDLA-Permissive-2.0) and dependency multi-version allowances.

---

## v0.4.0

### Summary

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

### Summary

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

### Summary

> Adds an in-place self-updater (`srz upgrade`), automated Homebrew tap integration, and cross-platform support for Windows PowerShell.

### Changes
- **Self-Update Utility**: Added `srz upgrade` for in-place binary upgrades with automatic GitHub Release checks, SHA-256 verification, and atomic replacement.
- **Install Origin Detection**: Added environment detection to determine if `srz` was installed via Homebrew, Linuxbrew, or direct binary download.
- **Windows Support**: Added `x86_64-pc-windows-msvc` target builds and a one-line PowerShell installer (`install.ps1`).
- **Automation & Pipelines**: Added automated formula bumping for `NewGuy012/homebrew-tap` and dynamic release version resolution.
- **Network Resilience**: Added automatic HTTP redirect handling for release binary downloads.

---

## v0.1.0

### Summary

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
