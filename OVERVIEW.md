# `srz` Architectural Overview

`srz` (Summarize) is a high-performance, modular Rust CLI tool for extracting and synthesizing structured summaries from **webpages**, **local files**, **Unix stdin streams**, and **books** using local OpenAI-compliant LLMs (such as llama.cpp, Ollama, LM Studio, or vLLM).

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
        Router -->|"cache / history / stats / find / completions"| UtilFlow["Utility Handlers"]
    end

    subgraph BookResolution["2. Book Resolution & Grounding"]
        BookFlow --> Meta["Open Library API (Author & Year)"]
        Meta --> MultiSearch["Concurrent Search: OpenSearch + DDG + Wiki"]
        MultiSearch --> Scorer["Candidate Scoring & Disambiguation"]
        Scorer --> RefScrape["Scrape Best Reference Page"]
    end

    subgraph LLMEngine["3. LLM Synthesis & Streaming"]
        WebFlow --> Chunker{"Word Count > 12k?"}
        FileFlow --> Chunker
        RefScrape --> Chunker
        Chunker -->|"Yes"| MapReduce["Map-Reduce Chunk Summaries"]
        Chunker -->|"No"| SinglePass["Direct Single Pass"]
        MapReduce --> SSE["OpenAI-Compatible SSE Streaming Client"]
        SinglePass --> SSE
    end

    subgraph Outputs["4. Output & Persistence"]
        SSE -->|"Tokens (Live)"| Stdout["stdout (Pure Markdown or JSON)"]
        SSE -->|"Thinking Counter & Spinner"| Stderr["stderr (UI Diagnostics)"]
        SSE --> CacheStore["Disk Cache (~/.cache/srz/)"]
        SSE --> DuckDBStore["DuckDB Engine (srz.duckdb)"]
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
    participant LLM as LlmClient (SSE)
    participant DB as DuckDB (srz.duckdb)

    User->>CLI: srz book "The Hobbit"
    CLI->>Cache: Check summary cache (kind, title, style, model)
    alt Cache Hit
        Cache-->>CLI: Return cached summary
        CLI->>User: Print summary immediately (0ms compute)
    else Cache Miss
        CLI->>Search: Resolve metadata & candidates (Open Library + OpenSearch)
        Search-->>CLI: Ranked candidates (The Hobbit #1, Score: 115)
        CLI->>Scraper: Scrape canonical reference page
        Scraper-->>CLI: Cleaned article text (1,395 words)
        CLI->>LLM: POST /v1/chat/completions (stream: true)
        loop SSE Token Stream
            LLM-->>User: Stream visible tokens to stdout
            LLM-->>CLI: Update thinking token counter on stderr
        end
        CLI->>Cache: Save summary, raw page, and HTML/MD view
        CLI->>DB: INSERT into history & library tables
        CLI-->>User: Complete with clickable file:// view link
    end
```

---

## 3. Storage & Cache Architecture

All state is isolated under `~/.cache/srz/` (or `$XDG_CACHE_HOME/srz/`), completely bypassable via `--no-cache` and resettable via `srz cache clean`:

```mermaid
graph LR
    subgraph Root["~/.cache/srz/"]
        subgraph Namespaces["JSON Cache Directories"]
            P["pages/"] --- P_desc["Scraped webpage DOM & text"]
            S["search/"] --- S_desc["Candidate search results & scores"]
            M["summaries/"] --- M_desc["Completed LLM summaries"]
            V["views/"] --- V_desc["Rendered HTML reader & .md files"]
        end
        subgraph DB["Analytics Database"]
            D["srz.duckdb"]
            D --> T1["Table: history (IDs, tokens, tok/s, durations)"]
            D --> T2["Table: library (Full-text search for srz find)"]
        end
    end
```

---

## 4. Component Directory & Responsibilities

| File | Module | Primary Responsibility |
| :--- | :--- | :--- |
| [`src/main.rs`](file:///Users/myoo/Documents/Repo/srz/src/main.rs) | `main` | Entry point, pipeline orchestration, TTY checking, and subcommand handlers. |
| [`src/cli.rs`](file:///Users/myoo/Documents/Repo/srz/src/cli.rs) | `cli` | `clap` argument parsing, style definitions, and target action resolution. |
| [`src/llm.rs`](file:///Users/myoo/Documents/Repo/srz/src/llm.rs) | `llm` | SSE client, thinking tokens, `stream_options`, and prompt crafting. |
| [`src/search.rs`](file:///Users/myoo/Documents/Repo/srz/src/search.rs) | `search` | Open Library metadata, Wikipedia OpenSearch/fulltext, DDG, and candidate scoring. |
| [`src/scraper.rs`](file:///Users/myoo/Documents/Repo/srz/src/scraper.rs) | `scraper` | HTTP client, DOM parsing, boilerplate removal, and word counting. |
| [`src/chunker.rs`](file:///Users/myoo/Documents/Repo/srz/src/chunker.rs) | `chunker` | Text windowing and paragraph chunking for long articles (> 12k words). |
| [`src/cache.rs`](file:///Users/myoo/Documents/Repo/srz/src/cache.rs) | `cache` | Disk caching in `~/.cache/srz/` with case-insensitive ASCII hashing. |
| [`src/db.rs`](file:///Users/myoo/Documents/Repo/srz/src/db.rs) | `db` | Embedded DuckDB engine tracking telemetry and offline full-text library search. |
| [`src/render.rs`](file:///Users/myoo/Documents/Repo/srz/src/render.rs) | `render` | Markdown-to-HTML typography rendering and slugification. |
| [`src/ui.rs`](file:///Users/myoo/Documents/Repo/srz/src/ui.rs) | `ui` | Spinners (`indicatif`), terminal detection (`is_terminal`), and `NO_COLOR` hygiene. |

---

## 5. Key CLI Features & Flags

- **Universal Input**: `srz <URL>`, `srz <BOOK>`, `srz <FILE>`, or `cat ... | srz -`.
- **Telemetry & Stats**: `srz stats` (speeds, total tokens) and `srz history` (past reads).
- **Offline Search**: `srz find "keyword"` searches past summaries in DuckDB.
- **Machine-Readable**: Add `--json` to any command for JSON stdout.
- **Unix Pipeline Friendly**: Strict separation—summary on `stdout`, diagnostics on `stderr`. Pass `-q` / `--quiet` to mute all stderr notices.
- **Shell Completions**: `srz completions zsh > ~/.zfunc/_srz` (supports bash, zsh, fish, powershell, elvish).
