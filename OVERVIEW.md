# `srz` Architectural Overview

`srz` (Summarize) is a high-performance, modular Rust CLI tool for extracting and synthesizing structured summaries from **webpages**, **local files**, **Unix stdin streams**, **books**, and **live news digests** using local OpenAI-compliant LLMs (such as llama.cpp, Ollama, LM Studio, or vLLM).

---

## 1. High-Level Architecture

The diagram below illustrates the end-to-end lifecycle of an `srz` invocation:

```mermaid
flowchart TD
    subgraph Inputs["1. Input Ingestion & Routing"]
        CLI["CLI Args / Flags / Env"] --> Router{"Router / Auto-Detect"}
        Router -->|"URL (http://...)"| WebFlow["Web Scraper"]
        Router -->|"Local Path or Stdin (-)"| FileFlow["File / Stdin Reader"]
        Router -->|"Book Title"| BookFlow["Book Pipeline"]
        Router -->|"News Query or Subcommand"| NewsFlow["Google News RSS Client"]
        Router -->|"cache / history / stats / find / completions"| UtilFlow["Utility Handlers"]
    end

    subgraph Resolution["2. Resolution & Grounding"]
        BookFlow --> Meta["Open Library Metadata (Cached / 3s API)"]
        Meta --> MultiSearch["Concurrent Search: OpenSearch + Wiki + DDG"]
        MultiSearch --> Scorer["PreparedQuery Scoring & Disambiguation (-y)"]
        Scorer --> RefScrape["Scrape Best Reference Page (Cached / Live)"]

        NewsFlow --> Feed["Google News RSS (when:1d/7d/30d)"]
        Feed --> RPC["Decode Obfuscated URLs (batchexecute RPC)"]
        RPC --> NewsScrape["Concurrent Article Scraping"]
    end

    subgraph LLMEngine["3. LLM Synthesis & Streaming"]
        WebFlow --> Chunker{"Word Count > 12k?"}
        FileFlow --> Chunker
        RefScrape --> Chunker
        Chunker -->|"Yes"| MapReduce["Map-Reduce Chunk Summaries + Aggregated Telemetry"]
        Chunker -->|"No"| SinglePass["Direct Single Pass"]
        NewsScrape --> NewsBriefing["News Digest Briefing Synthesis"]
        MapReduce --> SSE["Zero-Copy OpenAI SSE Streaming Client (SseStreamState)"]
        SinglePass --> SSE
        NewsBriefing --> SSE
    end

    subgraph Outputs["4. Output & Persistence"]
        SSE -->|"Tokens (Batched Flush)"| Stdout["stdout (Pure Markdown or JSON)"]
        SSE -->|"Throttled Thinking & Token Counter"| Stderr["stderr (UI Diagnostics)"]
        SSE --> CacheStore["Disk Cache (~/.cache/srz/)"]
        SSE --> DuckDBStore["Lazy DuckDB Engine (Transactional Write)"]
        CacheStore --> Views["Rendered HTML & MD (views/)"]
    end
```

---

## 2. Request & Execution Sequence

```mermaid
sequenceDiagram
    autonumber
    actor User
    participant CLI as CLI & Router
    participant Cache as Disk Cache (~/.cache/srz)
    participant Search as BookSearcher & Open Library
    participant Scraper as WebScraper
    participant LLM as LlmClient (SseStreamState)
    participant DB as DuckDB (Lazy Init)

    User->>CLI: srz book "The Hobbit"
    CLI->>Cache: Check summary cache (kind, title, style, model)
    alt Cache Hit (DuckDB Not Opened)
        Cache-->>CLI: Return cached summary
        CLI->>Cache: Ensure existing HTML/MD view files (skip re-render)
        CLI->>User: Print summary & clickable file:// links immediately
    else Cache Miss
        CLI->>Cache: Check cached book metadata & search candidates
        CLI->>Search: Resolve metadata (Open Library) & concurrent candidates (if uncached)
        Search-->>CLI: Ranked candidates via PreparedQuery (The Hobbit #1, Score: 115)
        opt Ambiguous Top Candidates (Interactive TTY & not -y)
            CLI->>User: Prompt reference selection [1-4, Enter for 1]
        end
        CLI->>Scraper: Fetch or cache canonical reference page (.mw-parser-output)
        Scraper-->>CLI: Cleaned article text without footnote bloat
        CLI->>LLM: POST /v1/chat/completions (stream: true, include_usage: true)
        loop Zero-Copy SSE Token Stream
            LLM-->>User: Stream visible tokens to stdout (flushed per chunk)
            LLM-->>CLI: Update throttled thinking/token counter on stderr
        end
        CLI->>Cache: Save summary and render HTML/MD view files
        CLI->>DB: Lazily open srz.duckdb & transactional INSERT (history + library)
        CLI-->>User: Complete with clickable file:// view links
    end
```

---

## 3. Storage & Cache Architecture

All state is isolated under `~/.cache/srz/` (or `$XDG_CACHE_HOME/srz/`), unified via `Cache::default_cache_dir()`, completely bypassable via `--no-cache`, and resettable via `srz cache clean`:

```mermaid
graph LR
    subgraph Root["~/.cache/srz/"]
        subgraph Namespaces["JSON Cache Directories"]
            P["pages/"] --- P_desc["Scraped webpage DOM & normalized text"]
            S["search/"] --- S_desc["Open Library metadata & scored search candidates"]
            M["summaries/"] --- M_desc["Completed LLM summaries"]
            V["views/"] --- V_desc["Rendered HTML5 reader & .md files"]
        end
        subgraph DB["Analytics Database (Lazy-Opened)"]
            D["srz.duckdb"]
            D --> T1["Table: history (book 📚, web 🌐, news 📰, file 📄, stdin ⌨️ telemetry)"]
            D --> T2["Table: library (Offline full-text search for srz find)"]
        end
    end
```

---

## 4. Component Directory & Responsibilities

| File | Module | Primary Responsibility |
| :--- | :--- | :--- |
| [`src/main.rs`](file:///Users/myoo/Documents/Repo/srz/src/main.rs) | `main` | Entry point, pipeline orchestration, lazy DB opening, candidate disambiguation, and subcommand handlers. |
| [`src/cli.rs`](file:///Users/myoo/Documents/Repo/srz/src/cli.rs) | `cli` | `clap` argument parsing, `SummaryStyle` helpers, `stream_mode` selection, news query routing, and target action resolution. |
| [`src/news.rs`](file:///Users/myoo/Documents/Repo/srz/src/news.rs) | `news` | Google News RSS client, `batchexecute` RPC redirect URL decoding, and concurrent multi-article scraping. |
| [`src/llm.rs`](file:///Users/myoo/Documents/Repo/srz/src/llm.rs) | `llm` | Zero-copy SSE client (`SseStreamState`), localhost IPv4 fallback, map-reduce telemetry aggregation, and prompt crafting. |
| [`src/search.rs`](file:///Users/myoo/Documents/Repo/srz/src/search.rs) | `search` | Open Library metadata, Wikipedia OpenSearch/fulltext, DDG search, and hoisted `PreparedQuery` candidate scoring. |
| [`src/scraper.rs`](file:///Users/myoo/Documents/Repo/srz/src/scraper.rs) | `scraper` | HTTP client, MediaWiki/article DOM extraction, footnote-free `html2text` conversion, and single-pass word counting. |
| [`src/chunker.rs`](file:///Users/myoo/Documents/Repo/srz/src/chunker.rs) | `chunker` | Zero-copy `Cow` text windowing and paragraph chunking for long documents (> 12k words). |
| [`src/cache.rs`](file:///Users/myoo/Documents/Repo/srz/src/cache.rs) | `cache` | Disk caching in `~/.cache/srz/` for pages, metadata, searches, and summaries using zero-allocation `AsciiLower` hashing. |
| [`src/db.rs`](file:///Users/myoo/Documents/Repo/srz/src/db.rs) | `db` | Embedded DuckDB engine with transactional writes, reading telemetry (`book`, `web`, `news`, `file`, `stdin`), and zero-copy snippet search. |
| [`src/render.rs`](file:///Users/myoo/Documents/Repo/srz/src/render.rs) | `render` | Markdown-to-HTML5 reader rendering, standalone library dashboard (`srz index`), cached view-file reuse, and slugification. |
| [`src/ui.rs`](file:///Users/myoo/Documents/Repo/srz/src/ui.rs) | `ui` | Spinners (`indicatif`) and `LazyLock`-cached terminal detection (`is_terminal`) and `NO_COLOR`/`CLICOLOR` hygiene. |

---

## 5. Performance & Architectural Optimizations

- **Deferred DuckDB Initialization & View Reuse**: DuckDB (`srz.duckdb`) is opened lazily only when recording a newly generated summary or running `history`, `stats`, `find`, or `cache clean`. Cache hits and `srz completions` execute with zero database initialization or view-file re-rendering overhead.
- **Zero-Copy SSE Streaming (`SseStreamState`)**: SSE chunks deserialize into borrowed `Cow<'a, str>` slices (`#[serde(borrow)]`), drain network buffers once per chunk via a read cursor, batch `stdout` flushes per network read, and throttle spinner updates.
- **Single-Client Search & Metadata Caching**: Book resolution caches Open Library metadata on disk, reuses a single `BookSearcher` HTTP connection pool across metadata and candidate queries, and hoists query normalization (`PreparedQuery`) with zero-allocation ASCII substring matching.
- **MediaWiki-Aware, Footnote-Free Scraping**: Prioritizes `.mw-parser-output` and `#mw-content-text` selectors and configures `html2text` without hyperlink footnote tables or artificial 100-column line wraps, preserving clean paragraph structure and saving prompt tokens.
- **Transactional Telemetry & Release LTO**: Dual `history` + `library` inserts execute in a single atomic DuckDB transaction, map-reduce runs aggregate token counts and durations across all chunks, and release builds use thin LTO, single codegen-unit, and symbol stripping.

---

## 6. Key CLI Features & Flags

- **Universal Input**: `srz <URL>`, `srz <BOOK>`, `srz <FILE>`, `cat ... | srz -`, or `srz news <TOPIC>` (with auto-detection for `"current ... news"`).
- **Timeframe Filtering**: `--timeframe` / `-t` (`day`, `week`, `month`, `all`) and `--limit` / `-l` for news searches.
- **Interactive or Automated Disambiguation**: Prompts when book references have close scores, or pass `-y` / `--yes` to auto-select the top candidate.
- **Telemetry & Stats**: `srz stats` (speeds, total tokens across books, web, news, files, and stdin) and `srz history` (past reads).
- **Offline Search & Dashboard**: `srz find "keyword"` searches past summaries in DuckDB, and `srz index [--open]` opens the interactive library dashboard.
- **Machine-Readable**: Add `--json` to any command for JSON stdout, or `-o summary.html` / `-o summary.md` to save directly to disk.
- **Unix Pipeline Friendly**: Strict separation—summary on `stdout`, diagnostics on `stderr`. Pass `-q` / `--quiet` to mute all stderr notices.
- **Shell Completions**: `srz completions zsh > ~/.zfunc/_srz` (supports bash, zsh, fish, powershell, elvish).
