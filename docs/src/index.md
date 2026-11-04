<p align="center">
  <img src="assets/logo.svg" alt="srz - Rust CLI for Summarizing Webpages, Books, and Streams" width="760">
</p>

# `srz` (Summarize)

`srz` (Summarize) is a high-performance, modular Rust CLI tool for extracting and synthesizing structured summaries using local OpenAI-compliant LLMs (such as **llama.cpp**, **Ollama**, **LM Studio**, or **vLLM**) from:

- 🌐 **Webpages & articles** (Mozilla Readability extraction)
- 📺 **YouTube video transcripts**
- 📄 **PDF documents & local files**
- ⌨️ **Unix stdin streams**
- 📚 **Books**
- 📰 **Live news digests & universal RSS/Atom feeds**

---

## Key Highlights

- 🔒 **100% Local & Private**: Runs entirely on your local machine with zero telemetry, subscriptions, or external cloud API keys required.
- 🎯 **3 Summary Presets**: Choose `concise` (default), `bullets`, or `detailed` summaries.
- ⚡ **Terminal Paging & Live Streaming**: Clean CLI output with automatic `$PAGER` integration (`less -RFX`), terminal escape sequence sanitization, and optional live token streaming (`--stream`).
- 💬 **Interactive Q&A & Multi-Turn Chat (`srz ask`)**: Ask targeted questions grounded on source text or engage in persistent multi-turn chat sessions with full transcript recall.
- 🖥️ **Terminal Dashboard (`srz browse`)**: Full-screen keyboard-navigated TUI built on Ratatui with live search filtering, reader views, and clipboard export.
- 🔍 **Offline BM25 Full-Text Search (`srz find`)**: Embedded DuckDB indexes every read into an inverted term index with 2.5× title boosting, Robertson-Sparck Jones IDF ranking, and query-focused snippet extraction.
- 📊 **Reading Analytics & Model Telemetry (`srz stats`)**: Track reading streaks, active days, estimated reading time saved (225 wpm baseline), and local model token throughput percentiles (P50/P95).
- 🗄️ **Consolidated DuckDB Storage & TTL Caching**: High-speed embedded storage (`srz.duckdb` and `cache.duckdb`) with atomic transactions, SHA-256 hash indexing, and millisecond-accurate TTL pruning.
- 📖 **Distraction-Free Reader Views**: Rendered HTML and Markdown views with light/dark theme toggle, opened via `--open` or `srz index`.

---

## Documentation Guide

> 🧭 **Where should I begin?**
> - **Brand new to `srz` or local LLMs?** Start with [Installation](installation.md) and the [Quick Start](quickstart.md) to run your first summary in under 2 minutes.
> - **Looking for specific flags, subcommands, or keybindings?** Jump straight to the [Command Reference](commands.md).
> - **Curious how it works under the hood?** Explore [Architecture & Design](architecture.md) and the deep-dive [Rust Learning Guide](rust_learning_guide.md).

Browse through the chapters:

- [**Installation**](installation.md): Quick one-line curl/PowerShell scripts, Homebrew tap, Cargo build, precompiled releases, and in-place self-upgrades.
- [**Quick Start**](quickstart.md): Connect your local LLM backend (Llama-macOS, Llama-Windows, llama.cpp, Ollama, LM Studio) and run your first summaries.
- [**Command Reference**](commands.md): In-depth guide covering CLI auto-detection, subcommands, flags, interactive TUI keybindings, and session management.
- [**Configuration**](configuration.md): `config.toml` structure, named profiles, storage limits, and cache retention policies.
- [**Architecture & Design**](architecture.md): Systems architecture, lifecycle sequence flows, ingestion pipelines, embedded DuckDB schema, BM25 ranking algorithm, performance optimizations, and security model.

### Deep-Dive Guides & Audits

- 🦀 [**Rust Learning Guide**](rust_learning_guide.md): Architectural deep-dive and systems programming concepts using `srz` as a case study.
- 🛡️ [**Security Audit**](security_audit.md): Comprehensive security analysis, memory safety guarantees, and threat mitigations.
- 📋 [**Changelog**](changelog.md): Release history, version highlights, and breaking changes.
