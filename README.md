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

## Prerequisites: LLM Backend Server

`srz` requires an OpenAI API-compliant LLM server. While cloud provider APIs (OpenAI, Groq) are supported, running a **local LLM is strongly preferred** for complete privacy, zero subscription fees, and offline capability:

- **macOS**: [Llama-macOS](https://github.com/ggml-org/Llama-macOS) (recommended local default; connects out-of-the-box on port `9931`).
- **Windows & Linux**: [Ollama](https://ollama.com/) (recommended local default; port `11434`).
- **Other Backends**: Compatible with LM Studio, llama.cpp, vLLM, or cloud providers (OpenAI, Groq).

See [Configuring Your LLM Backend](#configuring-your-llm-backend) below for step-by-step setup instructions and profile templates.

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

`srz` auto-detects whether you pass a URL, book title, local document, news query, or piped stream:

```bash
# Summarize anything (URLs, books, documents, piped text)
srz https://example.com/article
srz "Dune"                              # Searches Wikipedia & Open Library
srz paper.pdf                           # Supports PDF, EPUB, MD, and TXT
git diff | srz -                        # Reads standard input

# Live news briefings & natural queries
srz news "javascript"                   # Today's news digest
srz "current AI news" -t week           # Auto-detected news query (day/week/month)

# Custom styles & browser reader view
srz "Dune" --style tldr --open          # Presets: concise, detailed, bullets, tldr, eli5...

# Offline library, search & diagnostics
srz index --open                        # Interactive HTML library dashboard
srz find "ecology"                      # Full-text search past summaries
srz history                             # View past summaries & token metrics
srz doctor                              # Verify LLM connection, cache, and database
```

Run `srz --help` to see all available subcommands (`web`, `book`, `file`, `news`, `index`, `find`, `history`, `stats`, `doctor`, `models`, `config`, `cache`, `completions`) and flags.

## Features

### Content Sources & Ingestion

- **Web Articles**: Cleanly extracts and summarizes any webpage, stripping ads, navbars, and cookie banners.
- **Books by Title**: Synthesizes verified book overviews from Wikipedia and Open Library without needing an ebook file (`srz "Dune"`).
- **Live News Briefings**: Aggregates top stories from Google News on any topic or timeframe, with natural prompt support (`srz "current javascript news"`).
- **Files, PDFs, EPUBs & Stdin**: Summarizes local Markdown, text files, PDF papers, EPUB ebooks, or piped terminal output (`git diff | srz -`).

### LLM Engine & Synthesis

- **8 Summary Styles**: Tailor summaries with presets: `concise`, `detailed`, `bullets`, `executive`, `tldr`, `critique`, `eli5`, or `flashcards`.
- **Parallel Chunking for Long Texts**: Automatically splits large documents and map-reduces sections in parallel (`--concurrency <N>`).
- **Loading Spinners & Optional Live Streaming**: Clean loading spinner and phase feedback with summary presented cleanly upon completion; live token streaming is available on demand via `--stream`.

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

## Configuring Your LLM Backend

`srz` connects to any OpenAI-compatible API. Local inference is preferred for privacy and zero cost, but cloud providers are fully supported.

### Local LLMs (Preferred)

- **macOS (Default)**: Install [Llama-macOS](https://github.com/ggml-org/Llama-macOS). Launch the app and load a model—`srz` connects automatically to `http://localhost:9931/v1` with zero setup.
- **Windows & Linux**: Install [Ollama](https://ollama.com/) and run a model:
  ```bash
  ollama run gemma2:9b
  export OPENAI_BASE_URL="http://localhost:11434/v1" OPENAI_MODEL="gemma2:9b"
  # Windows PowerShell: $env:OPENAI_BASE_URL="http://localhost:11434/v1"; $env:OPENAI_MODEL="gemma2:9b"
  ```
- **Diagnostics**: Run `srz doctor` to test server latency and `srz models` to list loaded models.

### Cloud Providers (Optional)

Set your endpoint and API key via environment variables or CLI flags:

```bash
# Groq
export OPENAI_BASE_URL="https://api.groq.com/openai/v1" OPENAI_MODEL="llama-3.3-70b-versatile" OPENAI_API_KEY="gsk_..."

# OpenAI
export OPENAI_BASE_URL="https://api.openai.com/v1" OPENAI_MODEL="gpt-4o-mini" OPENAI_API_KEY="sk-..."

# Or via CLI flag
srz "1984" --base-url https://api.openai.com/v1 --model gpt-4o-mini --api-key "sk-..."
```

### Profile Switching (`~/.config/srz/config.toml`)

Run `srz config init` to create a config file. Define local and cloud profiles to switch instantly:

```toml
# Default local backend
base_url = "http://localhost:9931/v1"
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"

# Local Ollama profile
[profiles.ollama]
base_url = "http://localhost:11434/v1"
model = "gemma2:9b"

# Cloud profile (Groq)
[profiles.groq]
base_url = "https://api.groq.com/openai/v1"
model = "llama-3.3-70b-versatile"
api_key = "gsk_..."
```

```bash
srz "Dune"                   # Uses default local model
srz "Dune" --profile ollama  # Uses local Ollama
srz "Dune" --profile groq    # Uses Groq cloud
```

## Architecture & Modules

See [`OVERVIEW.md`](OVERVIEW.md) for detailed Mermaid architecture and sequence diagrams. The codebase is organized into modular components with single responsibilities:

- `src/main.rs`: CLI entry point coordinating the high-level summarization pipeline and subcommand handlers.
- `src/cli.rs`: Argument parsing, subcommand definitions, natural news query detection, and style guidance with `clap`.
- `src/config.rs`: Configuration file (`~/.config/srz/config.toml`) loading, template generation, and named profile support.
- `src/cache.rs`: Persistent disk cache (`~/.cache/srz`) and private `0700`/`0600` file/directory permission helpers.
- `src/db.rs`: Embedded DuckDB engine for reading history, performance telemetry, and offline full-text library search.
- `src/scraper.rs`: Webpage fetching (with 10 MB cap), DOM normalization, and local PDF/EPUB document text extraction.
- `src/news.rs`: Google News RSS client, `batchexecute` RPC redirect decoding, and concurrent article aggregation.
- `src/search.rs`: Open Library metadata resolution plus Wikipedia & DuckDuckGo candidate search and ranking.
- `src/llm.rs`: Zero-copy OpenAI SSE streaming client and parallel map-reduce synthesis engine.
- `src/chunker.rs`: Zero-copy text windowing and paragraph chunking for long documents.
- `src/render.rs`: Standalone HTML5 reader views, interactive library dashboard (`srz index`), and Markdown rendering.
- `src/ui.rs`: Terminal spinners, `NO_COLOR` detection, and status output with `indicatif`.

## Running Tests

```bash
cargo test
```
