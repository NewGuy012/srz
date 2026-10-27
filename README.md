# `srz` (Summarize)

A modular, minimalist Rust CLI for summarizing **webpages** and **books** using any local OpenAI-compliant LLM (such as **Ollama**, **LM Studio**, **vLLM**, or **llama.cpp**).

---

## Features

- **Webpage Summarization**: Scrapes any URL, strips boilerplate (ads, navigation, footers), normalizes the article text into clean paragraphs, and generates a structured summary.
- **Book Summarization by Title**: Summarizes books without needing to supply ebook files.
  - **Web Search by Default**: Queries free, zero-API sources (DuckDuckGo & Wikipedia) for the book's synopsis and overview to ground the LLM's summary with accurate, hallucination-free facts.
  - **Trained Knowledge Fallback**: If no online reference is found (or when `--no-search` is passed), falls back to the LLM's trained memory.
- **Local OpenAI API-Compliant**: Built for local servers like **llama.cpp** (default: `http://localhost:9931/v1`), **Ollama**, **LM Studio**, and **vLLM**.
- **Embedded DuckDB Engine**: Built-in DuckDB database (`~/.cache/srz/srz.duckdb`) tracking your reading history, token speeds, and providing instant full-text search across all past summaries.
- **Real-Time SSE Streaming & Live Thinking Counter**: Streams tokens (`stream: true`) to `stdout` in real time while displaying live chain-of-thought token progress on `stderr`.
- **Clickable Styled HTML & Markdown Views**: Automatically saves a styled, responsive `.html` reader view (with light/dark mode) and `.md` file in `~/.cache/srz/views/` and prints clickable `file://` links so you can open the summary in your browser or editor with one click.
- **Strict `stdout` / `stderr` Separation**: All spinners, headers, and diagnostics are emitted to `stderr` so `stdout` can be piped cleanly (`srz "Dune" | pbcopy`).
- **Polished CLI**: Built using `clap` (subcommands, flags, env vars) and `indicatif` (interactive spinners and status indicators).
- **Multiple Summary Styles**: `concise`, `detailed` (default), `bullets`, or `executive`.
- **Smart Disk Caching**: Automatically caches search results, scraped reference pages, and generated summaries in `~/.cache/srz` (`srz cache dir | size | clean`). Pass `--no-cache` to force a fresh fetch.
- **Text Chunking**: Automatic windowed chunking and map-reduce synthesis for long web pages (> 12,000 words, tuned for a 32k context window).

---

## Installation

Ensure you have Rust and Cargo installed:

```bash
cargo build --release
```

The compiled binary will be located at `target/release/srz`.

---

## Usage

### 1. Summarizing a Webpage

```bash
# Using the `web` subcommand
srz web https://example.com/article

# Or using auto-detection
srz https://example.com/article

# Save summary to a file
srz web https://example.com/article -o summary.md
```

### 2. Summarizing a Book

```bash
# Default: Searches web for book synopsis and synthesizes with local LLM
srz book "Atomic Habits"

# Provide author to refine search
srz book "Dune" --author "Frank Herbert"

# Skip web search and query local LLM memory directly
srz book "Thinking, Fast and Slow" --no-search

# Auto-detected positional argument
srz "To Kill a Mockingbird"

# Force fresh web scrape & LLM generation (bypass local cache)
srz book "Dune" --no-cache
```

### 3. Choosing a Summary Style

```bash
srz web https://example.com/article --style bullets
srz book "Clean Code" --style executive
```

Styles available:
- `detailed` (default): Comprehensive breakdown covering all major themes and arguments.
- `concise`: High-level summary in 2–3 concise paragraphs.
- `bullets`: Key takeaways and actionable points formatted as bullet points.
- `executive`: Structured as Executive Overview, Core Findings, Strategic Insights, and Impact.

### 4. Managing the Local Cache

```bash
# Print the cache directory path
srz cache dir

# Inspect cached file counts and disk size
srz cache size

# Clear all cached pages, search results, and summaries
srz cache clean
```

### 5. History, Stats, and Library Search (DuckDB)

```bash
# View recent reads, token metrics, and speeds
srz history
srz history --limit 20

# View aggregate performance metrics across all runs
srz stats

# Search your local summarized library offline for keywords/topics
srz find "spice"
srz find "ecology"
```

---

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

---

## Architecture & Modules

The codebase is organized into modular components with single responsibilities:

- [`src/main.rs`](file:///Users/myoo/Documents/Repo/srz/src/main.rs): CLI entry point coordinating the high-level summarization pipeline.
- [`src/cli.rs`](file:///Users/myoo/Documents/Repo/srz/src/cli.rs): Argument parsing, subcommand definitions, and style guidance with `clap`.
- [`src/cache.rs`](file:///Users/myoo/Documents/Repo/srz/src/cache.rs): Persistent disk cache (`~/.cache/srz`) for search results, scraped pages, and summaries.
- [`src/scraper.rs`](file:///Users/myoo/Documents/Repo/srz/src/scraper.rs): Webpage fetching, DOM parsing, boilerplate removal, and text normalization.
- [`src/search.rs`](file:///Users/myoo/Documents/Repo/srz/src/search.rs): DuckDuckGo & Wikipedia search integration (zero API keys required).
- [`src/llm.rs`](file:///Users/myoo/Documents/Repo/srz/src/llm.rs): OpenAI API client connecting to local LLM endpoints.
- [`src/chunker.rs`](file:///Users/myoo/Documents/Repo/srz/src/chunker.rs): Text windowing and chunking for long articles.
- [`src/ui.rs`](file:///Users/myoo/Documents/Repo/srz/src/ui.rs): Terminal spinners and status output with `indicatif`.

---

## Running Tests

```bash
cargo test
```
