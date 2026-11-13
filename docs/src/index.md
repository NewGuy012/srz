<p align="center">
  <img src="assets/logo.svg" alt="srz - Rust CLI for Summarizing Webpages, Books, and Streams" width="760">
</p>

# `srz` (Summarize)

`srz` (Summarize) is a high-performance, modular Rust CLI tool for extracting and synthesizing structured summaries using local OpenAI-compliant LLMs (such as **llama.cpp**, **Ollama**, **LM Studio**, or **vLLM**) from:

- 🌐 **Webpages & articles**
- 📚 **Books**
- 📰 **Live news digests & universal RSS/Atom feeds**
- 📺 **YouTube video transcripts**
- 📄 **PDF documents, EPUB ebooks, & local files**
- ⌨️ **Unix stdin streams**

---

## What Makes `srz` Different?

Most AI tools require subscriptions and send your private data to cloud servers. `srz` is built to be simple, fast, and completely private:

- **100% Private** — Everything runs directly on your computer. Your files, documents, and reading history never leave your machine.
- **One Tool for Any Content** — Summarize web articles, YouTube videos, PDFs, books, and news feeds with a single command. Clutter and ads are removed automatically.
- **Built-in Offline Search** — Your summaries are saved locally so you can easily search and browse your past reading history anytime, even without an internet connection.

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
- [**Architecture & Design**](architecture.md): Systems architecture, lifecycle sequence flows, ingestion pipelines, embedded `redb` schema, offline library search & BM25 Q&A grounding, performance optimizations, and security model.

### Deep-Dive Guides & Audits

- 🦀 [**Rust Learning Guide**](rust_learning_guide.md): Architectural deep-dive and systems programming concepts using `srz` as a case study.
- 🛡️ [**Security Audit**](security_audit.md): Comprehensive security analysis, memory safety guarantees, and threat mitigations.
- 📋 [**Changelog**](changelog.md): Release history, version highlights, and breaking changes.
