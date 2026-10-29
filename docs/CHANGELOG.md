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

## v0.5.0 (Unreleased)

> Expands DuckDB utilization across the srz engine with pure-offline BM25 relevance ranking for full-text library search, rich reading habits and model telemetry in srz stats, persistent multi-turn chat sessions with resumption support for srz ask, consolidated embedded DuckDB caching, and engine-wide performance and prompt framing optimizations.

### Changes
- **BM25 Full-Text Search (`srz find`)**: Implemented an offline inverted term index table (`library_terms`) in DuckDB with $2.5\times$ title boosting, term frequency saturation ($k_1=1.2, b=0.75$), and Robertson-Sparck Jones IDF calculation, returning relevance-ranked search results and query-focused snippet extraction without external network dependencies.
- **Rich Habit & Model Telemetry (`srz stats`)**: Expanded reading analytics to compute active reading days, reading streaks, estimated reading time saved (based on human reading speed of 225 wpm), top source domains, and model P50/P95 token throughput percentiles using pure DuckDB SQL aggregation without external ICU extensions.
- **Persistent Multi-Turn Chat Sessions (`srz ask`)**: Added chat session persistence in DuckDB (`chat_sessions`, `chat_messages`) supporting `--chats` (list past sessions), `--resume <ID>` (continue conversation from previous context), `--show-chat <ID>` (review complete message transcripts), and `--delete-chat <ID>`.
- **DuckDB Cache Consolidation & Native Millisecond TTL Pruning**: Consolidated ephemeral disk cache (scraped web pages, search metadata, and LLM summaries) into an embedded `cache.duckdb` database with atomic transactions and transparent fallback to legacy JSON files; aligned `cache_summaries` schema, introduced DuckDB-native `epoch_ms(cached_at)::BIGINT` queries for millisecond-accurate TTL pruning without ICU extensions, and unified cache directory traversals in `Cache::prune` into a single pass.
- **Search & Render Performance Optimizations**:
  - `LibraryItem::matches_query`: Short-circuits metadata fields before checking full summaries, and eliminates intermediate heap allocations for non-ASCII search terms against ASCII metadata.
  - `escape_html_to` & `escape_html_cow`: Scans text in multi-byte chunks via `rest.find(['&', '<', '>', '"', '\''])` pattern matching and returns `Cow::Borrowed` with zero allocations when no characters require escaping.
  - Canonical `target_meta` mapping helper deduplicating icon, category, and badge formatting.
- **Prompt Framing & Delimiter Consolidation**: Replaced fragmented manual closing tag escaping across all LLM pipelines (`summarize_document_counted`, `summarize_book_with_context`, `summarize_news_digest`, and `commands::ask::format_grounded_context`) with a centralized `format_untrusted_section` helper.
- **Centralized Text Utilities (`system::text_util`)**: Unified HTML entity decoding (`decode_html_entities`) and ASCII case-insensitive search (`find_ascii_case_insensitive`, `contains_ascii_case_insensitive`) into `src/system/text_util.rs`, eliminating ad-hoc cross-module dependencies between `sources::news` and `sources::youtube`.
- **CLI Flag & Subcommand Simplification**: Streamlined command-line interface by hiding redundant subcommands (`web`, `book`, `file`) from `--help` in favor of primary root arguments (`srz <TARGET>`), and removed legacy flags (`--author`, `--no-search`, `-y/--yes`, `--max-tokens`, `--concurrency`, `--no-pager`, and `--namespace` on prune).

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
