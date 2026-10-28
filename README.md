<p align="center">
  <img src="assets/logo.svg" alt="srz - Rust CLI for Summarizing Webpages, Books, and Streams" width="760">
</p>

# `srz` (Summarize)

A fast, modular Rust CLI for distilling **webpages**, **local files**, **stdin streams**, and **books** into structured summaries using any local OpenAI-compliant LLM (such as **llama.cpp**, **Ollama**, **LM Studio**, or **vLLM**).

## Highlights

- **Summarize anything**: works with web links, book titles, local files, and piped text.
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

# Summarize a local file or piped input
srz README.md
git diff | srz -

# Choose a summary style (concise, detailed, bullets, executive)
srz https://example.com/article --style bullets

# Search and browse your local library
srz find "ecology"
srz history
srz stats
```

Run `srz --help` to see all available subcommands (`web`, `book`, `file`, `cache`, `completions`) and flags.

## Features

- **Webpage Summarization**: Scrapes any URL, strips boilerplate (ads, navigation, footers, footnote tables), normalizes the article text into clean paragraphs, and generates a structured summary.
- **Local File & Stdin Summarization**: Summarizes local files (`srz file <path>` or auto-detected `srz README.md`) and standard input (`git diff | srz -`).
- **Book Summarization by Title**: Summarizes books without needing to supply ebook files.
  - **Canonical Metadata & Web Grounding**: Resolves author and publication year via Open Library, then queries Wikipedia (OpenSearch + full-text) and DuckDuckGo with candidate scoring and interactive disambiguation (`-y` to auto-accept).
  - **Trained Knowledge Fallback**: If no online reference is found (or when `--no-search` is passed), falls back to the LLM's trained memory.
- **Universal Token Tracking**: Streams tokens (`stream: true`) with `stream_options` to `stdout` in real time while displaying live chain-of-thought token progress on `stderr`.
- **Machine-Readable JSON Output**: Pass `--json` to `history`, `stats`, `find`, or summarization commands for automated scripting and editor integrations.
- **Embedded DuckDB Engine**: Built-in DuckDB database (`~/.cache/srz/srz.duckdb`) tracking your reading history, token speeds, and providing instant full-text search across all past summaries.
- **Shell Auto-Completions**: Generate tab-completion scripts out-of-the-box (`srz completions <bash|zsh|fish|powershell>`).
- **Clickable Styled HTML & Markdown Views**: Automatically saves a styled, responsive `.html` reader view (with light/dark mode) and `.md` file in `~/.cache/srz/views/` and prints clickable `file://` links so you can open the summary in your browser or editor with one click.
- **Strict `stdout` / `stderr` & `NO_COLOR` Hygiene**: Respects `NO_COLOR` and terminal detection (`is_terminal`). Supports quiet (`-q`) and verbose (`-v`) flags.
- **Multiple Summary Styles**: `concise` (default), `detailed`, `bullets`, or `executive`.
- **Smart Disk Caching**: Automatically caches book metadata, search candidates, scraped reference pages, and generated summaries in `~/.cache/srz` (`srz cache dir | size | clean`). Pass `--no-cache` to force a fresh fetch.
- **Text Chunking**: Automatic windowed chunking and map-reduce synthesis with aggregated token telemetry for long documents (> 12,000 words, tuned for a 32k context window).

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
- `src/search.rs`: Open Library metadata resolution plus Wikipedia & DuckDuckGo candidate search and ranking.
- `src/llm.rs`: Zero-copy OpenAI SSE streaming client and map-reduce synthesis engine.
- `src/chunker.rs`: Text windowing and paragraph chunking for long articles.
- `src/render.rs`: Standalone HTML5 reader view and Markdown rendering.
- `src/ui.rs`: Terminal spinners, `NO_COLOR` detection, and status output with `indicatif`.

## Running Tests

```bash
cargo test
```
