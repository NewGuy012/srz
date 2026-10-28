<p align="center">
  <img src="assets/logo.svg" alt="srz - Rust CLI for Summarizing Webpages, Books, and Streams" width="760">
</p>

# `srz` (Summarize)

A fast, modular Rust CLI for distilling **webpages**, **local files**, **stdin streams**, **books**, and **live news digests** into structured summaries using any local OpenAI-compliant LLM (such as **llama.cpp**, **Ollama**, **LM Studio**, or **vLLM**).

## Highlights

- **Summarize anything**: works with web links, book titles, local files, piped text, and news topics.
- **Multi-source news briefings**: aggregates top articles from Google News RSS for any topic, decodes destination publisher URLs, and synthesizes executive briefings.
- **Fact-checked book summaries**: pulls synopses from Wikipedia and Open Library—no ebook files or API keys needed.
- **100% local & private**: runs on your own machine using `llama.cpp`, `Ollama`, `LM Studio`, or `vLLM`.
- **Searchable personal library**: saves every summary locally so you can search past reads offline.
- **Clean reader views**: generates one-click HTML and Markdown pages with automatic light and dark mode.

## Installation

Ensure you have Rust and Cargo installed:

```bash
cargo build --release
```

The compiled binary will be located at `target/release/srz`.

## Usage

`srz` automatically detects whether you pass a URL, a local file, piped text, or a book title:

```bash
# Summarize a webpage
srz https://example.com/article

# Summarize a book by title (optionally filter by author)
srz "The Hobbit"
srz book "Dune" --author "Frank Herbert"

# Summarize local files (Markdown, text, HTML, PDF, EPUB) or piped input
srz README.md
srz paper.pdf
srz novel.epub
git diff | srz -

# Search and summarize latest news on any topic (today by default, or past week/month)
srz news "javascript"
srz news "artificial intelligence" --timeframe week --limit 6
srz "current javascript news"
srz "latest rust news"

# Choose a summary style (concise, detailed, bullets, executive, tldr, critique, eli5, flashcards)
srz https://example.com/article --style tldr
srz paper.pdf --style critique
srz novel.epub --style eli5

# Auto-open styled HTML reader view in browser
srz "Dune" --open

# Check server health and discover models
srz doctor
srz models

# Search, browse, and launch standalone HTML library dashboard
srz index --open
srz find "ecology"
srz history
srz stats
```

Run `srz --help` to see all available subcommands (`web`, `book`, `file`, `news`, `cache`, `config`, `models`, `doctor`, `index`, `completions`) and flags.

## Features

### Content Sources & Ingestion

- **Webpage Summarization**: Scrapes any URL, strips boilerplate (ads, navigation, footers, footnote tables), normalizes the article text into clean paragraphs, and generates a structured summary.
- **Book Summarization by Title**: Summarizes books without needing to supply ebook files.
  - **Canonical Metadata & Web Grounding**: Resolves author and publication year via Open Library, then queries Wikipedia (OpenSearch + full-text) and DuckDuckGo with candidate scoring and interactive disambiguation (`-y` to auto-accept).
  - **Trained Knowledge Fallback**: If no online reference is found (or when `--no-search` is passed), falls back to the LLM's trained memory.
- **Live News Briefings & Natural Query Routing**: Aggregates top stories from Google News RSS (`srz news <topic> [-t/--timeframe <day|week|month|all>] [-l/--limit <N>]`).
  - **Canonical URL Resolution**: Decodes Google News' opaque redirect tokens into real publisher URLs via Google's `batchexecute` RPC, then scrapes the full articles concurrently.
  - **Natural Language Auto-Routing**: Automatically recognizes natural prompts like `srz "current javascript news"`, `srz "latest rust news"`, or `srz "tech news"`, strips conversational filler words while preserving the subject matter, and generates a structured executive briefing.
- **Local Documents, Ebooks, & Stdin**: Summarizes local files (`srz file <path>` or auto-detected `srz README.md`), PDF documents (`srz paper.pdf`), EPUB books (`srz book.epub`), and standard input (`git diff | srz -`).

### LLM Engine & Synthesis

- **Multiple Summary Styles**: Choose between `concise` (default), `detailed`, `bullets`, `executive`, `tldr`, `critique`, `eli5`, and `flashcards` to fit your use case.
- **Parallel Text Chunking**: Automatic windowed chunking and parallel map-reduce synthesis (`--concurrency <N>`) with aggregated token telemetry for long documents (> 12,000 words, tuned for a 32k context window).
- **Universal Token Tracking**: Streams tokens (`stream: true`) with `stream_options` to `stdout` in real time while displaying live chain-of-thought token progress on `stderr`.

### Offline Library, Storage & Analytics

- **Embedded DuckDB Engine**: Built-in DuckDB database (`~/.cache/srz/srz.duckdb`) tracking your reading history, token speeds, and providing instant full-text search across all past summaries (`srz find "query"`, `srz history`, `srz stats`).
- **Standalone HTML Library Dashboard**: Run `srz index` (or `srz index --open`) to view an offline, searchable dashboard of all past reads with interactive filtering by category (`📚 Books`, `🌐 Web`, `📰 News`, `📄 Files`, `⌨️ Stdin`).
- **Clickable Styled HTML & Markdown Views**: Automatically saves a styled, responsive `.html` reader view (with light/dark mode) and `.md` file in `~/.cache/srz/views/` and prints clickable `file://` links so you can open the summary in your browser or editor with one click (`--open` / `-b` to launch browser automatically).
- **Smart Disk Caching**: Automatically caches book metadata, search candidates, scraped reference pages, and generated summaries in `~/.cache/srz` (`srz cache dir | size | clean`). Pass `--no-cache` to force a fresh fetch.

### Configuration & CLI Experience

- **Optional Config File & Named Profiles**: Persist settings in `~/.config/srz/config.toml` (initialize with `srz config init`) and switch named model presets on the fly (`srz --profile fast ...` vs `srz --profile deep ...`).
- **Server Health & Model Discovery**: Run `srz doctor` to verify LLM backend latency, database integrity, cache health, and config files. Run `srz models` to list available backend models.
- **Machine-Readable JSON Output**: Pass `--json` to `history`, `stats`, `find`, `models`, `doctor`, or summarization commands for automated scripting and editor integrations.
- **Strict I/O & Terminal Hygiene**: Strict separation—summary on `stdout`, diagnostics on `stderr`. Respects `NO_COLOR` and terminal detection (`is_terminal`). Supports quiet (`-q`) and verbose (`-v`) flags.
- **Shell Auto-Completions**: Generate tab-completion scripts out-of-the-box (`srz completions <bash|zsh|fish|powershell>`).

## Configuring Your Local LLM

By default, `srz` connects to your local llama.cpp app:
- **Base URL**: `http://localhost:9931/v1`
- **Model**: `ggml-org/gemma-4-E4B-it-GGUF:Q8_0`

You can customize the endpoint and model either via CLI flags or environment variables:

### Via CLI Flags:
```bash
srz book "1984" --base-url http://localhost:9931/v1 --model ggml-org/gemma-4-E4B-it-GGUF:Q8_0
```

### Via Environment Variables:
```bash
export OPENAI_BASE_URL="http://localhost:9931/v1"
export OPENAI_MODEL="ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
export OPENAI_API_KEY=""   # Optional
```

## Architecture & Modules

See [`OVERVIEW.md`](OVERVIEW.md) for detailed Mermaid architecture and sequence diagrams. The codebase is organized into modular components with single responsibilities:

- `src/main.rs`: CLI entry point coordinating the high-level summarization pipeline.
- `src/cli.rs`: Argument parsing, subcommand definitions, and style guidance with `clap`.
- `src/cache.rs`: Persistent disk cache (`~/.cache/srz`) for metadata, search results, scraped pages, and summaries.
- `src/db.rs`: Embedded DuckDB engine for reading history, performance telemetry, and offline full-text library search.
- `src/scraper.rs`: Webpage fetching, DOM parsing, boilerplate removal, and text normalization.
- `src/news.rs`: Google News RSS client, batchexecute RPC redirect decoding, and concurrent article aggregation.
- `src/search.rs`: Open Library metadata resolution plus Wikipedia & DuckDuckGo candidate search and ranking.
- `src/llm.rs`: Zero-copy OpenAI SSE streaming client and map-reduce synthesis engine.
- `src/chunker.rs`: Text windowing and paragraph chunking for long articles.
- `src/render.rs`: Standalone HTML5 reader view and Markdown rendering.
- `src/ui.rs`: Terminal spinners, `NO_COLOR` detection, and status output with `indicatif`.

## Running Tests

```bash
cargo test
```
