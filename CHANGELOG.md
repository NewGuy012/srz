# Changelog

All notable changes to `srz` are documented in this file.

## v0.8.0

### Summary
Delivers major performance, UX, and architectural upgrades: in-process hardware detection with `sysinfo`, semantic HTML-to-Markdown scraping with `htmd`, crash-safe atomic file persistence, strict XDG data directory compliance, seamless in-TUI grounded chat (`a` in `srz browse`), user-facing DuckDB SQL analytics (`srz query`), and multi-format library vault exports (`srz export`).

### Changes
- **In-Process Hardware Profiling**: Integrated `sysinfo` to detect host memory and processor architecture in pure Rust, eliminating subprocess startup latency on Windows and macOS.
- **Semantic HTML-to-Markdown Scraping**: Integrated `htmd` to convert web articles into clean semantic Markdown, significantly enhancing document comprehension and summary structure for local LLMs.
- **Crash-Safe Atomic File Writes**: Implemented atomic persistence with `tempfile::NamedTempFile` and strict `0600` POSIX permissions.
- **Strict XDG Standard Directory Compliance**: Separated persistent user data (`XDG_DATA_HOME` / `~/.local/share/srz`) from disposable web caches (`XDG_CACHE_HOME` / `~/.cache/srz`) with automatic migration.
- **Seamless In-TUI Grounded Chat**: Added the `a` keybinding in `srz browse` to launch an interactive grounded Q&A session (`srz ask`) directly on the highlighted document without exiting the browser.
- **User-Facing SQL Analytics CLI (`srz query`)**: Added `srz query "<SQL>"` to execute analytical queries on DuckDB with terminal table, `--csv`, or `--json` output.
- **Multi-Format Library Vault Export (`srz export`)**: Added `srz export` supporting Obsidian/Logseq Markdown vaults, JSON, CSV, and Parquet.
- **Unified URL Content Fetching**: Unified URL extraction across `summarize` and `ask`, enabling `srz ask <url>` to natively query YouTube video transcripts and remote PDF documents in addition to scraped web pages.
- **RAG Context Selection Optimization**: Added `chunk_by_tokens_with_counts` and eliminated triple full-document BPE tokenization passes during context selection, cutting tokenization overhead by ~67%.
- **Cache Initialization & Eviction Optimization**: Streamlined schema verification so initialized caches skip redundant DDL executions on startup, and batched Pass 2 LRU row evictions into a single DuckDB transaction.
- **Allocation & Memory Reductions**: Pre-allocated dashboard cards buffer and streamed HTML cards directly via `write!`, eliminating intermediate heap string allocations per library entry.
- **Correctness & Output Hardening**: Sanitized terminal outputs and guarded against multi-byte UTF-8 slice panics in `handle_query`, registered missing `query` and `export` subcommands in typo suggestions, and removed duplicate unreachable CLI action match arms.

## v0.7.2

### Summary
Introduces intelligent local model diagnostics and CLI typo suggestions, week-by-default news search recency with empty-feed guidance, comprehensive Security Audit #5 hardening, simplified Llama GUI model recommendations, and dynamic token budgeting up to 128k context.

### Changes
- **News Recency Defaults & Guidance**: Defaults news search recency to `week` and adds actionable tips suggesting `-t month` or `-t all` on empty feeds.
- **Local Model Diagnostics & Command Typo Guidance**: Adds clear 'Local model not detected' diagnostics with fuzzy model matching, and typo suggestions for mistyped commands.
- **Security Audit #5 Hardening**: Enforces input bounds, neutralizes dangerous URI schemes in markdown, sanitizes terminal controls, and adds symlink guards to database initialization.
- **Dynamic Token Budget Precision**: Honors dynamic token budgets up to 128k context without legacy clamping.
- **Storage, Feed & Chunker Optimizations**: Centralized URL helpers, unified DuckDB query templates, zero-copy terminal sanitization, and cached per-turn chat tokens.
