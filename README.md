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

## Prerequisites: Local LLM Server

`srz` requires an OpenAI API-compliant LLM server running on your machine (or access to a cloud provider API). Local inference is preferred for total privacy, zero fees, and offline use:

- **macOS**: [Llama-macOS](https://github.com/ggml-org/Llama-macOS) (recommended; connects out-of-the-box on port `9931`).
- **Windows & Linux**: [Ollama](https://ollama.com/) (recommended; port `11434`).
- **Other Backends**: Compatible with LM Studio, llama.cpp, vLLM, or cloud providers (OpenAI, Groq).

See [Configuring Your Local LLM](#configuring-your-local-llm) below for step-by-step setup instructions and profile templates.

## Installation

Ensure you have Rust and Cargo installed:

```bash
cargo build --release
```

The compiled binary will be located at `target/release/srz`. To install it directly to your Cargo binary path:

```bash
cargo install --path .
```

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

- **Web Articles**: Cleanly extracts and summarizes any webpage, stripping ads, navbars, and cookie banners.
- **Books by Title**: Synthesizes verified book overviews from Wikipedia and Open Library without needing an ebook file (`srz "Dune"`).
- **Live News Briefings**: Aggregates top stories from Google News on any topic or timeframe, with natural prompt support (`srz "current javascript news"`).
- **Files, PDFs, EPUBs & Stdin**: Summarizes local Markdown, text files, PDF papers, EPUB ebooks, or piped terminal output (`git diff | srz -`).

### LLM Engine & Synthesis

- **8 Summary Styles**: Tailor summaries with presets: `concise`, `detailed`, `bullets`, `executive`, `tldr`, `critique`, `eli5`, or `flashcards`.
- **Parallel Chunking for Long Texts**: Automatically splits large documents and map-reduces sections in parallel (`--concurrency <N>`).
- **Live Token Streaming**: Streams response tokens to `stdout` in real time while tracking model speed and thinking tokens.

### Offline Library & Storage

- **Offline Full-Text Search**: Built-in DuckDB database keeps a searchable history of past summaries (`srz find <query>`).
- **Visual Library Dashboard**: Run `srz index --open` to browse, search, and filter your past summaries in an interactive HTML dashboard.
- **Instant Reader Views**: Generates clean, dark/light-mode HTML reader files and prints one-click `file://` links (`--open` to launch in browser).
- **Smart Local Cache**: Caches fetched pages, book synopses, and summaries locally so repeated lookups are instant (`--no-cache` to bypass).

### Configuration & CLI Experience

- **Config File & Profiles**: Save settings in `~/.config/srz/config.toml` and toggle named model profiles on the fly (`--profile fast`).
- **Health Checks & Model Discovery**: Run `srz doctor` to test server connectivity and `srz models` to see available backend models.
- **Pipeline & Scripting Friendly**: Pass `--json` for automation, `-o` to save files, and enjoy clean `stdout`/`stderr` separation.
- **Shell Auto-Completions**: Instant tab-completion setup for `bash`, `zsh`, `fish`, and `powershell`.

## Configuring Your Local LLM

While local inference is preferred, `srz` works with both local servers and cloud providers.

### 1. macOS Setup (Llama-macOS)
1. Install and launch [Llama-macOS](https://github.com/ggml-org/Llama-macOS) (native Apple Silicon app powered by `llama.cpp` Metal acceleration).
2. Download or select your preferred GGUF model in the app (e.g., Gemma 4, Llama 3, or Mistral).
3. `srz` connects automatically to `http://localhost:9931/v1` with zero configuration needed.

### 2. Windows & Linux Setup (Ollama)
1. Install [Ollama](https://ollama.com/).
2. Pull and run your preferred model:
   ```bash
   ollama run gemma2:9b
   ```
3. Configure `srz` to use Ollama's endpoint (`http://localhost:11434/v1`):
   ```bash
   # Windows (PowerShell)
   $env:OPENAI_BASE_URL="http://localhost:11434/v1"
   $env:OPENAI_MODEL="gemma2:9b"

   # Linux / macOS (bash/zsh)
   export OPENAI_BASE_URL="http://localhost:11434/v1"
   export OPENAI_MODEL="gemma2:9b"
   ```
   *(Or save these in `~/.config/srz/config.toml` as shown below).*

### 3. Verify Connection & Models
Run diagnostic commands to ensure `srz` can reach your LLM backend:
```bash
srz doctor   # Verifies endpoint latency, database, cache, and config health
srz models   # Lists all models currently available on the backend
```

### 4. Named Profiles in `~/.config/srz/config.toml` (Recommended)
Run `srz config init` to create a starter config, then define your local and cloud profiles:
```toml
# Default local backend (fast, private, zero cost)
base_url = "http://localhost:9931/v1"
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"

# Local Ollama profile (Windows / Linux)
[profiles.ollama]
base_url = "http://localhost:11434/v1"
model = "gemma2:9b"

# Optional Cloud Profiles (Groq, OpenAI, OpenRouter, DeepSeek)
[profiles.groq]
base_url = "https://api.groq.com/openai/v1"
model = "llama-3.3-70b-versatile"
api_key = "gsk_..."

[profiles.openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
api_key = "sk-..."
```

Run with your default local model or toggle profiles on the fly:
```bash
srz "The Hobbit"                   # Runs locally via Llama-macOS
srz "The Hobbit" --profile ollama  # Runs via local Ollama
srz "The Hobbit" --profile groq    # Runs via Groq cloud
srz "The Hobbit" --profile openai  # Runs via OpenAI cloud
```

### 5. Overriding via Flags or Environment Variables
```bash
# Connect to local Ollama via CLI flag
srz "1984" --base-url http://localhost:11434/v1 --model gemma2:9b

# Connect to cloud OpenAI via CLI flag
srz "1984" --base-url https://api.openai.com/v1 --model gpt-4o-mini --api-key "sk-..."
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
