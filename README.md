<p align="center">
  <img src="assets/logo.svg" alt="srz - Rust CLI for Summarizing Webpages, Books, and Streams" width="760">
</p>

# `srz` (Summarize)

`srz` (Summarize) is a high-performance, modular Rust CLI tool for extracting and synthesizing structured summaries using local OpenAI-compliant LLMs (such as **llama.cpp**, **Ollama**, **LM Studio**, or **vLLM**) from:

- 🌐 **Webpages**
- 📺 **YouTube video transcripts**
- 📄 **PDF documents & local files**
- ⌨️ **Unix stdin streams**
- 📚 **Books**
- 📰 **Live news digests**


## Highlights

```mermaid
%%{init: {"flowchart": {"minNodeWidth": 210, "minNodeHeight": 54}}}%%
flowchart LR
    subgraph Sources["Input Sources"]
        S1["🌐 Web & YouTube URLs\nArticles & Captions"]
        S2["📄 PDFs & Documents\nFiles & Stdin Streams"]
        S3["📚 Book Synopses\nOpen Library Overviews"]
        S4["📰 Live News Feeds\nGoogle News RSS"]
    end

    subgraph Core["srz Core (100% Private)"]
        E1["⚡ Ingest Pipeline\nMap-Reduce Chunking"]
        E2["🔒 100% Local Inference\nOffline LLM Engine"]
        E3["📝 Summary Generation\nPresets & Synthesis"]
        E1 --> E2
        E2 --> E3
    end

    subgraph Highlights["Key Highlights"]
        H1["⚡ Rich Terminal Paging\nSyntax-Colored Output"]
        H2["💬 Grounded Q&A Chat\nInteractive srz ask"]
        H3["🖥️ TUI Dashboard View\nRatatui srz browse"]
        H4["📖 Clean Reader Views\nDark-Mode HTML & MD"]
    end

    S1 --> E1
    S2 --> E1
    S3 --> E1
    S4 --> E1

    E3 --> H1
    E3 --> H2
    E3 --> H3
    E3 --> H4
```

- 🔒 **100% Local & Private**: Runs on your machine with complete privacy, zero subscriptions, and no API keys.
- 🎯 **3 Summary Presets**: Tailor output with `concise` (default), `bullets`, and `detailed`.
- ⚡ **Rich Terminal Paging**: Syntax-highlighted CLI output with automatic paging for long summaries.
- 💬 **Interactive Grounded Q&A**: Ask questions grounded on any document or past read via `srz ask`.
- 🖥️ **Interactive TUI Dashboard**: Browse, search, filter, and copy past summaries with `srz browse`.
- 🗄️ **Offline Searchable Library**: Automatically saves and indexes every summary in embedded DuckDB.
- 📖 **Distraction-Free Reader Views**: Generates clean HTML and Markdown pages with dark mode support.


## Prerequisites: LLM Backend Server

`srz` requires an OpenAI API-compliant LLM server. While cloud provider APIs (OpenAI, Anthropic, Google, Groq, etc.) are supported, running a **local LLM is strongly preferred** for complete privacy, zero subscription fees, and offline capability:

- **macOS**: [Llama-macOS](https://github.com/ggml-org/Llama-macOS) (recommended local default; connects out-of-the-box on port `9931`).
- **Windows**: [Llama-Windows](https://github.com/ggml-org/Llama-Windows) (recommended local default; connects out-of-the-box on port `9931`).
- **Linux**: [llama.cpp](https://github.com/ggml-org/llama.cpp) (recommended local default; run `llama-server`).
- **Other Backends**: Compatible with Ollama, LM Studio, vLLM, or cloud providers (OpenAI, Anthropic, Google Gemini, Groq, etc.).

See [Configuring Your LLM Backend](#configuring-your-llm-backend) below for step-by-step setup instructions and profile templates.


## Installation

### Quick Install

**macOS & Linux**:

```bash
curl -LsSf https://raw.githubusercontent.com/NewGuy012/srz/master/install.sh | sh
```

**Windows**:

```powershell
powershell -ExecutionPolicy ByPass -c "irm https://raw.githubusercontent.com/NewGuy012/srz/master/install.ps1 | iex"
```

### Homebrew (macOS & Linux)

```bash
brew install NewGuy012/tap/srz
```

### Build from Source (Cargo)

If you prefer building with Rust from source:

```bash
cargo install --path .
```


## Usage

### Summarize
```bash
srz "https://en.wikipedia.org/wiki/Rust_(programming_language)" # Webpage, blog, or article
srz "https://youtu.be/5C_HPTJg5ek"    # YouTube video: "Rust in 100 Seconds" (captions & timed transcript)
srz document.pdf                      # Local PDF document (pure-Rust extractor)
srz "The Rust Programming Language" --style bullets --open  # Book synopsis & open in browser
srz news "Rust language updates" -t week  # Live news digest (day, week, month)
git diff | srz -                      # Unix stdin stream
```

### Ask & Chat
```bash
srz ask "What are the main findings?" # Grounded Q&A based on latest read
srz ask document.pdf "Key takeaways?" # Grounded Q&A on a specific document
srz ask -i                            # Interactive multi-turn chat REPL
```

### Library, Search & Telemetry
```bash
srz browse                            # Interactive TUI dashboard (browse, search, copy)
srz find "keyword"                    # Offline full-text search across past summaries
srz history                           # Recent reads, word counts, and generation speeds
srz stats                             # Aggregate telemetry: words read, tokens generated
```

Run `srz --help` to see all available subcommands and flags.


## Features

- [**Universal Ingestion**](OVERVIEW.md#6-key-cli-features--commands): Webpages, YouTube captions, PDFs, local text files, Unix pipes (`-`), book synopses, and live news.
- [**Interactive Q&A (`srz ask`)**](OVERVIEW.md#module-responsibilities): Ask questions or launch a multi-turn chat REPL grounded on any document or past read.
- [**TUI Dashboard (`srz browse`)**](OVERVIEW.md#performance--memory-efficiency): Browse, search, filter, and copy past summaries in a split-pane terminal UI.
- [**3 Summary Presets**](OVERVIEW.md#6-key-cli-features--commands): Tailor summaries with `concise` (default), `bullets`, or `detailed`.
- [**Offline Library & Search**](OVERVIEW.md#3-storage--cache-architecture): Embedded DuckDB indexes every read for instant full-text search (`srz find`) and HTML dashboards (`srz index`).
- [**Reader Views & Paging**](OVERVIEW.md#module-responsibilities): Standalone dark-mode HTML/Markdown exports, plus automatic terminal paging (`less -RFX`).
- [**Map-Reduce Chunking**](OVERVIEW.md#performance--memory-efficiency): Splits and synthesizes long documents exceeding LLM context windows in parallel.
- [**Private & Secure**](OVERVIEW.md#security--privacy): 100% local inference, zero tracking, safe Rust, private file permissions (`0700`/`0600`), and SHA-256 verified binary updates.
- [**Profiles & Diagnostics**](OVERVIEW.md#6-key-cli-features--commands): Switch backends in `config.toml`, test server health (`srz doctor`), and self-update (`srz upgrade`).

> For architectural diagrams, lifecycle execution sequences, and in-depth subsystem specs, see [**`OVERVIEW.md`**](OVERVIEW.md).


## Configuring Your LLM Backend

`srz` connects to any OpenAI-compatible API. Local inference is preferred for privacy and zero cost, but cloud providers are fully supported.

### Local LLMs (Preferred)

- **macOS & Windows (Default)**: Install [Llama-macOS](https://github.com/ggml-org/Llama-macOS) (macOS) or [Llama-Windows](https://github.com/ggml-org/Llama-Windows) (Windows). Launch the app and load a model—`srz` connects automatically to `http://localhost:9931/v1` with zero setup.
- **Linux**: Build or install [llama.cpp](https://github.com/ggml-org/llama.cpp) and launch `llama-server`:
  ```bash
  # Start llama-server on port 9931 for zero configuration:
  llama-server -m model.gguf --port 9931

  # Or on default port 8080:
  llama-server -m model.gguf --port 8080
  export OPENAI_BASE_URL="http://localhost:8080/v1"
  ```
- **Diagnostics**: Run `srz doctor` to test server latency and `srz models` to list loaded models.

### Cloud Providers (Optional)

`srz` works with any OpenAI-compatible provider (OpenAI, Anthropic, Google, Groq, OpenRouter):

```bash
export OPENAI_BASE_URL="https://api.openai.com/v1" OPENAI_MODEL="gpt-4o-mini" OPENAI_API_KEY="sk-..."

# Or on the fly via CLI flag:
srz "1984" --base-url "https://api.openai.com/v1" --model "gpt-4o-mini" --api-key "sk-..."
```

### Profile Switching (`~/.config/srz/config.toml`)

Run `srz config init` to generate a starter config, then configure named profiles:

```toml
# Default local model
base_url = "http://localhost:9931/v1"
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"

# Cloud profile (e.g. OpenAI or Anthropic)
[profiles.openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
api_key = "sk-..."
```

Switch profiles on the fly with `--profile`:

```bash
srz "The Rust Programming Language"                      # Default local model
srz "The Rust Programming Language" --profile openai     # Named cloud profile
```


## Architecture & Modules

See [`OVERVIEW.md`](OVERVIEW.md) for detailed Mermaid architecture diagrams and execution sequences.


## Running Tests & Static Analysis

```bash
# Run full automated test suite (79 tests)
cargo test

# Strict static analysis (warnings denied)
cargo clippy --all-targets -- -D warnings
```
