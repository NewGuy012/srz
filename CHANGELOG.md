# Changelog

All notable changes to `srz` are documented in this file.

## v0.7.3

### Summary
Delivers major performance, UX, and architectural upgrades: in-process hardware profiling with `sysinfo`, semantic HTML-to-Markdown scraping with `htmd`, crash-safe atomic file persistence, strict XDG data directory compliance, seamless in-TUI grounded chat (`a` in `srz browse`), user-facing DuckDB SQL analytics (`srz query`), multi-format library vault exports (`srz export`), unified URL context fetching, and RAG/cache optimizations.

### Changes
- **Hardware & Scraping Modernization**: Replaced subprocess inspection with pure-Rust `sysinfo` hardware profiling, and integrated `htmd` for semantic Markdown scraping preserving headings, tables, and code fences.
- **Storage & XDG Architecture**: Migrated user database and chats to `XDG_DATA_HOME` (`~/.local/share/srz`) with automatic migration, and hardened writes with atomic tempfiles and `0600` permissions.
- **Interactive Tools & Analytics**: Added `srz query` for SQL analytics, `srz export` for Obsidian/Logseq Markdown vaults, JSON, CSV, and Parquet, and the `a` keybinding in `srz browse` for instant grounded chat.
- **Unified Ingestion**: Unified URL fetching so `srz ask <url>` natively supports YouTube video transcripts and remote PDFs alongside web articles.
- **RAG & Cache Optimizations**: Eliminated redundant BPE passes during context selection (cutting tokenization by ~67%), bypassed startup DDL for existing caches, batched LRU evictions in a single transaction, and pre-allocated HTML card buffers.
- **Correctness & Safety**: Sanitized terminal tables and guarded against multi-byte UTF-8 slice panics in `handle_query`, registered missing typo suggestions for `query`/`export`, and removed unreachable CLI match arms.

## v0.7.2

### Summary
Introduces intelligent local model diagnostics and CLI typo suggestions, week-by-default news search recency with empty-feed guidance, comprehensive Security Audit #5 hardening, simplified Llama GUI model recommendations, and dynamic token budgeting up to 128k context.

### Changes
- **News Recency Defaults & Guidance**: Defaults news search recency to `week` and adds actionable tips suggesting `-t month` or `-t all` on empty feeds.
- **Local Model Diagnostics & Command Typo Guidance**: Adds clear 'Local model not detected' diagnostics with fuzzy model matching, and typo suggestions for mistyped commands.
- **Security Audit #5 Hardening**: Enforces input bounds, neutralizes dangerous URI schemes in markdown, sanitizes terminal controls, and adds symlink guards to database initialization.
- **Dynamic Token Budget Precision**: Honors dynamic token budgets up to 128k context without legacy clamping.
- **Storage, Feed & Chunker Optimizations**: Centralized URL helpers, unified DuckDB query templates, zero-copy terminal sanitization, and cached per-turn chat tokens.
