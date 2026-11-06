# Changelog

All notable changes to `srz` are documented in this file.

## Release Notes Template

> [!TIP]
> Keep release notes concise, user-friendly, and focused on practical user impact. Avoid internal implementation minutiae or duplicate bullets; group related refactors into cohesive, readable highlights.

```markdown
## vX.Y.Z

### Summary

> Concise 1–2 sentence overview highlighting the primary focus, key features, and user benefits of this release.

### Changes
- **User-Facing Feature / Command**: Clear description of the new capability and how it benefits the user.
- **Performance / Usability Enhancement**: Concise explanation of the optimization or UX improvement.
- **Fix / Security Hardening**: Brief description of the bug resolved or security enhancement.
```

---

## v0.7.5

### Summary

> Unifies caching and persistence into a single DuckDB store (`srz.duckdb`), adds pure SQL windowed LRU pruning, shortens news caching to 2 hours, makes reader view file exports strictly opt-in, and adds intuitive cache clear aliases.

### Changes
- **Unified DuckDB Storage (`src/storage/`)**: Consolidated all caching into `srz.duckdb` and eliminated redundant cache tables in favor of indexed library lookups, removing cross-database locking overhead.
- **Pure SQL LRU Pruning (`src/storage/cache.rs`)**: Replaced in-memory sorting with windowed SQL eviction in DuckDB, reclaiming disk space atomically with zero memory allocations.
- **2-Hour News TTL (`src/sources/news.rs`, `src/config/file.rs`)**: Shortened news and feed caching to 2 hours (`news_ttl_hours`) to keep briefings fresh while preserving fast repeat lookups.
- **Opt-In View Exports (`src/main.rs`)**: Made standalone HTML and Markdown file generation in `views/` strictly opt-in via `--open` or `--output`, keeping disk storage clean.
- **Cache Clear Aliases (`src/config/cli.rs`, `src/storage/cache.rs`)**: Added `srz cache clear` (along with `purge` and `wipe`), invalidating cached summaries and server capabilities while keeping user reading history intact.

---

## v0.7.4

### Summary

> Introduces an Antigravity/Gemini-style compact thinking UI with live one-liner step summaries and collapsible dropdowns, native zero-dependency EPUB book extraction, sub-millisecond cache fast-paths and offline chat management, dynamic per-turn BM25 grounding in interactive Q&A, calibrated single-pass token chunking, and deterministic release CI builds.

### Changes
- **Compact Thinking UI (`src/engine/llm.rs`, `src/ui/render.rs`)**: Replaced multi-line raw thinking trace dumps with a live, real-time one-liner spinner summary (`💭 Thinking: <Step> (Xs)...`) and an Antigravity/Gemini-style collapsible dropdown indicator (`💭 Thought for Xs ›`), preserving the full chain-of-thought in HTML reader views and opt-in `--show-thinking` CLI streaming.
- **Native EPUB Ingestion (`src/sources/epub.rs`, `src/sources/document.rs`)**: Pure-Rust zero-dependency EPUB book extraction and chapter stitching in reading order using existing zip and XML dependencies.
- **Cache Fast-Paths & Offline Chat (`src/main.rs`, `src/commands/ask.rs`)**: Instantaneous sub-millisecond cache hit delivery for web, books, and local files before initializing network or LLM engines, complete reasoning trace preservation, and offline chat management (`--chats`, `--show-chat`, `--delete-chat`).
- **Dynamic BM25 Grounding & Index Filtering (`src/commands/ask.rs`, `src/storage/db.rs`)**: Re-ranks grounding context dynamically per turn in interactive Q&A, optimizes single-pass BM25 chunk scoring, and filters stop words and short tokens in the library search index to prevent table bloat.
- **Engine & Streaming Fixes (`src/engine/llm.rs`, `src/ui/printer.rs`)**: Calibrated single-pass tokenization ratios eliminating N+1 `/tokenize` loops, aligned tree reduction tokens with active profiles, and preserved line buffering in `StreamMarkdownRenderer` when color is disabled.
- **Security Hardening (`src/storage/db.rs`, `src/sources/youtube.rs`)**: Sanitized YAML frontmatter scalars and filename collisions in Markdown vault exports, validated SQL attachment identifiers, and hardened YouTube transcript parsing against malformed escapes.
- **Build & CI Pipeline (`.github/workflows/release.yml`)**: Streamlined release workflow using `rust-cache` for deterministic, reliable release artifact generation.

---

## v0.7.3

### Summary

> Delivers major performance, UX, and architectural upgrades: in-process hardware profiling with `sysinfo`, semantic HTML-to-Markdown scraping with `htmd`, crash-safe atomic file persistence, strict XDG data directory compliance, seamless in-TUI grounded chat (`a` in `srz browse`), user-facing DuckDB SQL analytics (`srz query`), multi-format library vault exports (`srz export`), unified URL context fetching, and RAG/cache optimizations.

### Changes
- **Hardware & Scraping Modernization (`src/system/`, `src/sources/`)**: Replaced subprocess inspection with pure-Rust `sysinfo` hardware profiling, and integrated `htmd` for semantic Markdown scraping preserving headings, tables, and code fences.
- **Storage & XDG Architecture (`src/system/fs_util.rs`, `src/storage/db.rs`)**: Migrated user database and chats to `XDG_DATA_HOME` (`~/.local/share/srz`) with automatic migration from legacy cache paths, and hardened writes with atomic tempfiles and `0600` permissions.
- **Interactive Tools & Analytics (`src/commands/`, `src/ui/tui.rs`)**: Added `srz query` for analytical SQL queries, `srz export` for Obsidian/Logseq Markdown vaults, JSON, CSV, and Parquet, and the `a` keybinding in `srz browse` for instant grounded chat.
- **Unified Ingestion (`src/commands/`)**: Unified URL fetching so `srz ask <url>` natively supports YouTube video transcripts and remote PDFs alongside web articles.
- **RAG & Cache Optimizations (`src/engine/chunker.rs`, `src/storage/cache.rs`, `src/ui/render.rs`)**: Eliminated redundant BPE passes during context selection (cutting tokenization passes by ~67%), bypassed startup DDL for existing caches, batched LRU evictions in a single transaction, and pre-allocated HTML card buffers.
- **Correctness & Safety (`src/commands/library.rs`, `src/config/cli.rs`, `src/main.rs`)**: Sanitized terminal tables and guarded against multi-byte UTF-8 slice panics in `handle_query`, registered missing typo suggestions for `query`/`export`, and removed unreachable CLI match arms.

---

## v0.7.2

### Summary

> Introduces intelligent local model diagnostics and CLI typo suggestions, week-by-default news search recency with empty-feed guidance, comprehensive Security Audit #5 hardening, simplified Llama GUI model recommendations, and dynamic token budgeting up to 128k context.

### Changes
- **News Recency Defaults & Guidance (`srz news`)**: Defaults news search recency to `week` (past 7 days) across CLI options and natural queries to prevent empty feeds on niche topics, and adds actionable diagnostic tips suggesting `-t month` or `-t all` when feeds return no results.
- **Local Model Diagnostics & Command Typo Guidance (`src/engine/llm.rs`, `src/config/cli.rs`)**: Replaces raw JSON dumps with clear 'Local model not detected' diagnostics on model mismatch, suggests closest available models from the backend via string similarity, and offers command typo suggestions (e.g. `srz doctur` -> `doctor`).
- **Security Audit #5 Hardening (Findings 26–31)**: Enforces input bounds across JSON responses, syndication feeds, and config files; neutralizes dangerous URI schemes in markdown; sanitizes control codes in terminal outputs; and adds symlink guards to database initialization.
- **Static Analysis & Session ID Disambiguation (`src/commands/ask.rs`)**: Disambiguated chat session IDs to clear CodeQL `rust/cleartext-logging` false positives.
- **Dual Model Naming Strategy (`srz recommend`, `src/config/cli.rs`)**: Retains full verbose model IDs (`ggml-org/gemma-4-E4B-it-GGUF:Q8_0`) for CLI defaults, configuration templates, and backend API requests while providing concise Llama GUI model cards (`gemma-4` with `E4B` / `26B-A4B` badges and `Q8_0` quant) in `srz recommend` and diagnostic tips.
- **Dynamic Token Budget Precision (`format_untrusted_section`)**: Honors dynamic token budgets (e.g., 28k–56k on 32k–64k servers) up to 128k context rather than clamping to legacy 16,000 token limits.
- **Storage & Ingestion Deduplication (`src/storage/`, `src/system/`, `src/sources/`)**: Centralized URL helpers, unified DuckDB query constants (`select_library_base_sql`), consolidated hardened DuckDB connection opening (`open_hardened_duckdb_file`), optimized feed parsing, removed vestigial `no_search` cache parameters, and unified HTML and plain-text fallback parsing into `DocumentLoader::normalize_or_plain_text`.
- **Allocation & Streaming Optimizations (`src/ui/`, `src/engine/`, `src/commands/`)**: Implemented zero-allocation terminal text sanitization with `Cow<'_, str>` for streaming SSE tokens, eliminated double allocations in `SseStreamState::new`, removed repeated ASCII lowercasing in error diagnostics, and cached per-turn token counts in interactive multi-turn chat sessions to eliminate $O(N^2)$ recalculation overhead.

---

## v0.7.1

### Summary

> Fixes cross-platform compiler and Clippy warnings on Linux and Windows targets, and removes the `rust-version` constraint to allow Cargo to freely resolve the latest dependencies.

### Changes
- **Cross-Platform Compatibility & Clippy Fixes**: Resolved target-specific unused import warnings under `-D warnings` on Linux (`std::process::Command` in `hardware.rs`) and Windows (`super::*` in `fs_util.rs`), expanding cross-platform unit test coverage.
- **Dependency Resolution**: Removed the explicit `rust-version` constraint from `Cargo.toml` to allow Cargo's 2024 resolver to freely select the latest SemVer-compatible releases (including `uuid` v1.29.0) without holding packages back or emitting MSRV warnings.

---

## v0.7.0

### Summary

> Introduces intelligent hardware-based model recommendations, server capability discovery with dynamic token budgeting, direct syndication feed ingestion in `srz news`, and normalized DuckDB storage.

### Changes
- **Hardware Advisor & Tailored Setup (`srz recommend`)**: Inspects host RAM tier to recommend optimal Gemma 4 Q8_0 models, launch commands, and context windows (with `--json` support); `srz config init` now auto-populates hardware-tailored defaults and provides connection error guidance.
- **Dynamic Token Budgeting (`DynamicBudget`)**: Automatically detects server context window (`n_ctx`) and slot capacity to mathematically size prompt budgets, chunk overlap, and completion headroom without manual tuning or OOMs.
- **Server-Native Tokenization (`POST /tokenize`)**: Queries native `/tokenize` endpoints in `llama.cpp`, `Ollama`, and `vLLM` for exact token counting across code and multilingual text, cached in DuckDB for instant startup.
- **Syndication Feed Ingestion (`srz news`)**: Supports direct RSS, Atom, and JSON feed URLs with feed auto-detection, concurrent article scraping, and offline caching.
- **Storage & Engine Performance**: Unified DuckDB SQL LRU cache eviction, eliminated dual-writes, and added `--context-window`, `--max-tokens`, and `--concurrency` flags for fine-grained runtime control.

---

## v0.6.2

### Summary

> Expands project documentation with the official mdBook static website, brand logo assets, and architectural and learning guides.

### Changes
- **Documentation Website**: Launched official mdBook documentation site at `https://newguy012.github.io/srz/` with complete CLI guides and tutorials.
- **Book Refresh & Logo**: Synchronized all `book/src` documentation with current CLI features, updated configuration schemas, and added the official logo banner to the landing page.
- **Rust Guide & Security Audit**: Added `RUST_LEARNING_GUIDE.md` systems architecture case study and `SECURITY_AUDIT.md`.
- **README & Overview References**: Integrated doc site links across `README.md` and `docs/OVERVIEW.md`.

---

## v0.6.1

### Summary

> Updates core dependencies to their latest major releases (`feed-rs` v3, `dom_smoothie` v0.19, `tiktoken-rs` v0.12, `rustyline` v19, `quick-xml` v0.42), modernizes CI crates.io release workflows, and improves development tooling.

### Changes
- **Dependency Upgrades**: Upgraded parsing and ingestion libraries (`feed-rs` 3, `dom_smoothie` 0.19, `tiktoken-rs` 0.12, `rustyline` 19, `quick-xml` 0.42, and `html-escape` 0.3) for improved feed compatibility, token budgeting, and terminal prompt ergonomics.
- **CI / Crates.io Release Hardening**: Modernized Cargo token configuration to `CARGO_REGISTRY_TOKEN` and added idempotent crates.io publishing to ensure reliable automated releases and retry resilience.
- **Release Automation**: Updated `./scripts/release.sh` to default to patch version increments (`0.6.0` -> `0.6.1`).
- **Developer Experience**: Added standard Git pre-commit hooks and format-on-save configuration enforcing `cargo fmt` across all staged Rust sources.

---

## v0.6.0

### Summary

> Upgrades core content ingestion with industry-standard Rust parsers (`feed-rs`, `dom_smoothie`, `tiktoken-rs`, `pulldown-cmark`, `quick-xml`, `rustyline`) and streamlines runtime pipelines for faster, zero-allocation processing and single-pass cache management.

### Changes
- **Universal News & Syndication Feeds (`srz news`)**: Integrated `feed-rs` for robust parsing of RSS 0.9/1.0/2.0, Atom, and JSON feeds alongside automated Google News query resolution and URL un-obfuscation.
- **Readability Web Extraction**: Integrated Mozilla Readability DOM density scoring via `dom_smoothie`, cleanly stripping ads, navigation bars, cookie prompts, and sidebars before LLM synthesis.
- **BPE Token Budgeting & Fast Chunking**: Adopted OpenAI `cl100k_base` BPE token counting via `tiktoken-rs` with unified paragraph chunking and an $O(1)$ fast-path that avoids redundant tokenization for short documents.
- **Interactive Chat Line Editing (`srz ask -i`)**: Upgraded the interactive Q&A REPL with `rustyline` for in-line arrow-key editing, command history, Ctrl+C/Ctrl+D handling, and exact session ID display for easy resumption.
- **GitHub Flavored Markdown & Web Views**: Switched Markdown-to-HTML rendering to `pulldown-cmark` for standards-compliant tables, task lists, and strikethrough, opened cross-platform via `open::that_detached`.
- **Streaming YouTube Captions**: Replaced manual caption tag matching with `quick-xml` streaming event parsing and multi-client Innertube player fallbacks.
- **Optimized Cache Pruning**: Refactored cache retention to collect surviving file metadata in a single pass, running LRU size evictions completely in memory without redundant disk re-scans.
- **Zero-Allocation Text & Query Processing**: Replaced ad-hoc string parsing with zero-allocation ASCII case-insensitive matching in CLI routing, term search, and HTML entity decoding (`html-escape`).

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
