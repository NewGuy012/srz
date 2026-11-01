# Architecture & Design

`srz` is built from the ground up for instantaneous responsiveness, low memory overhead, and 100% local privacy.

---

## 1. Modular Subsystem Structure

The repository is organized into cohesive modules within `src/`:

```text
src/
├── commands/     # CLI subcommand workflows and action dispatchers (ask, browse, cache...)
├── config/       # CLI argument parsing, flags, and config file profiles
├── engine/       # LLM streaming client, prompt framing, and BPE token chunking
├── sources/      # Content ingestion: web scraping, book search, YouTube, and news RSS
├── storage/      # Local disk caching and embedded DuckDB analytics
├── system/       # Filesystem security helpers, text utilities, and in-place self-updater
├── ui/           # Terminal console formatting, spinners, and HTML reader views
├── lib.rs        # Library crate root and backwards-compatible module exports
└── main.rs       # Binary entry point and CLI orchestration
```

---

## 2. Ingestion & LLM Streaming Pipeline

```mermaid
flowchart TD
    subgraph Inputs["1. Input Ingestion & Routing"]
        CLI["CLI Args / Env / config.toml"] --> Router{"Router / Auto-Detect"}
        Router -->|"YouTube URL"| YTFlow["YouTube Timed-Text Client (quick-xml)"]
        Router -->|"Feed URL (.rss/.atom)"| NewsFlow["Universal Feed Parser (feed-rs)"]
        Router -->|"Web URL"| WebFlow["Web Scraper (dom_smoothie Readability)"]
        Router -->|"PDF File (.pdf)"| PDFFlow["Pure-Rust PDF Extractor (pdf-extract)"]
        Router -->|"Local Path or Stdin (-)"| FileFlow["File / Stdin Reader"]
        Router -->|"Book Title"| BookFlow["Book Resolver (Open Library + Wikipedia)"]
        Router -->|"News Query (news ...)"| NewsFlow
        Router -->|"ask [TARGET] [Q]"| AskFlow["Grounded Q&A Handler"]
        Router -->|"browse / tui"| TuiFlow["Ratatui TUI Dashboard"]
    end

    subgraph Grounding["2. Grounding & Token Budgeting"]
        YTFlow --> Chunker{"Tokens > 16k?"}
        WebFlow --> Chunker
        PDFFlow --> Chunker
        FileFlow --> Chunker
        BookFlow --> Chunker
        AskFlow --> MultiTurn["Multi-Turn Chat / Q&A Prompt Framing"]
        Chunker -->|"Yes"| MapReduce["Parallel Map-Reduce Chunk Summaries (tiktoken-rs)"]
        Chunker -->|"No"| SinglePass["Direct Single Pass"]
        NewsFlow --> NewsDigest["News Digest Briefing Synthesis"]
    end

    subgraph Inference["3. Zero-Copy Inference"]
        MapReduce --> SSE["Zero-Copy OpenAI SSE Streaming Client"]
        SinglePass --> SSE
        NewsDigest --> SSE
        MultiTurn --> SSE
    end

    subgraph Presentation["4. Presentation & Storage"]
        SSE --> TermMarkdown["Rich Terminal Markdown (Termimad)"]
        TermMarkdown --> Pager["System Pager ($PAGER / less -RFX)"]
        SSE --> CacheStore[("Consolidated Cache (cache.duckdb)")]
        SSE --> DuckDBStore[("Embedded DuckDB (srz.duckdb)")]
        CacheStore --> Views["Rendered HTML & MD Reader Views"]
    end
```

---

## 3. Storage & Cache Architecture

All state is stored under `~/.cache/srz/` (or `$XDG_CACHE_HOME/srz/`), secured with owner-only Unix permissions (`0700` directories, `0600` files):

```mermaid
graph LR
    subgraph CacheRoot["~/.cache/srz/ (0700)"]
        subgraph CacheEngine["Consolidated Cache (cache.duckdb)"]
            CP["cache_pages (7d TTL, 30d YouTube/PDF)"]
            CS["cache_search (14d TTL)"]
            CM["cache_summaries (90d TTL / persistent)"]
        end
        subgraph ViewsDir["Rendered Reader Views"]
            V["views/"] --- V_desc["HTML5 reader & Markdown view files (14d TTL)"]
        end
        subgraph LibraryDB["Analytics, Search & Sessions (srz.duckdb)"]
            D["srz.duckdb"]
            D --> T1["Table: history (metadata, telemetry, words, tokens, latency)"]
            D --> T2["Table: library (id FK, full summary text)"]
            D --> T3["Table: library_terms (BM25 inverted index & title boosting)"]
            D --> T4["Table: chat_sessions & chat_messages (multi-turn history)"]
        end
    end
```

### Retention Policies & Lifecycle Schedule

| Namespace / Asset | Retention Policy (TTL) | Eviction Trigger | Notes |
| :--- | :--- | :--- | :--- |
| **`cache_pages`** | **7 days** | SQL TTL expiration / LRU | Scraped DOM and clean text; YouTube & PDFs retain for 30 days. |
| **`cache_search`** | **14 days** | SQL TTL expiration / LRU | Open Library metadata and search candidates. |
| **`cache_summaries`** | **90 days** | LRU past max budget | Deterministic for `(target, model, style)` to save inference tokens. |
| **`views/`** | **14 days** | TTL expiration / LRU | Ephemeral reader views; lazily regenerated when opened. |
| **Total Cache Budget** | **500 MB** (configurable) | LRU eviction by size & `mtime` | Evicts oldest entries across caches down to 85% headroom. |
| **Library DuckDB (`srz.duckdb`)** | **Permanent (Not a cache)** | Explicit `--reset-db` only | Preserved across `srz cache clean`; contains reading history, habits, BM25 index, and chats. |

---

## 4. Offline BM25 Full-Text Search Algorithm

When executing `srz find <QUERY>`, `srz` computes relevance scores offline across the library using the Robertson-Sparck Jones BM25 formula:

\[
\text{Score}(D, Q) = \sum_{t \in Q} \text{IDF}(t) \cdot \frac{f(t, D) \cdot (k_1 + 1)}{f(t, D) + k_1 \cdot \left(1 - b + b \cdot \frac{|D|}{\text{avgdl}}\right)}
\]

* **$2.5\times$ Title Weighting**: Terms matching document titles receive $2.5\times$ the weight of terms matching summary bodies.
* **Tuned Parameters**: Term frequency saturation $k_1 = 1.2$, document length normalization $b = 0.75$.
* **Snippet Extraction**: Pre-tokenizes query terms to extract high-density preview sentences for fast scanning.
* **Pure SQL Execution**: Runs directly inside DuckDB without requiring external search engine plugins or background daemons.

---

## 5. Security & Privacy Guarantees

* **100% Safe Rust**: The entire crate enforces `#![forbid(unsafe_code)]` at `src/lib.rs` and `src/main.rs`. Zero `unsafe` blocks are permitted.
* **Strict Local Isolation**: All caching, indexing, and chat history remain strictly local on your device. Zero telemetry, tracking, or cloud phone-home requests.
* **Prompt Injection Hardening**: Untrusted source content is strictly quarantined inside tagged XML blocks (`<source_document>`) with automated case-insensitive escaping of closing tags (`escape_closing_tag`).
* **Terminal Escape & Trojan Source Defense**: Document titles and streaming outputs are stripped of ANSI escape sequences, C1 control characters, and Unicode Bidirectional (BiDi) override codepoints to prevent terminal spoofing.
* **Resource Ceilings**: Strict bounded limits prevent memory exhaustion: 10 MB for web responses and stdin, 20 MB for local files, and 50 MB for update binaries.
* **Cryptographically Verified Upgrades**: `srz upgrade` verifies SHA-256 checksums and SLSA provenance attestations before replacing binaries.
