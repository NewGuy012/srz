# `srz` (Summarize)

`srz` (Summarize) is a fast, modular Rust CLI tool for extracting and synthesizing structured summaries from **webpages**, **YouTube transcripts**, **PDF documents**, **local text files**, **Unix stdin streams**, **books**, and **live news feeds** using local OpenAI-compliant LLMs (such as **llama.cpp**, **Ollama**, **LM Studio**, or **vLLM**).

---

## Key Highlights

* **100% Local & Private**: Runs entirely on your local machine with zero telemetry, subscriptions, or external cloud API keys required.
* **Multi-Source Ingestion**:
  * 🌐 Webpages via reader-mode HTML extraction.
  * 📺 YouTube video transcripts via timed-text captions.
  * 📄 PDF documents with pure-Rust in-memory extraction.
  * 📚 Books with automatic metadata enrichment via Open Library and Wikipedia.
  * 📰 Live news digests with streaming Google News RSS.
  * ⌨️ Standard input streams (`cat file.txt | srz -`).
* **Interactive Terminal Explorer (`srz browse`)**: Full-screen keyboard-navigated TUI built on Ratatui with live search filtering, reader views, and clipboard export.
* **Offline BM25 Full-Text Search (`srz find`)**: Embedded DuckDB indexes every read into an inverted term index with $2.5\times$ title boosting, Robertson-Sparck Jones IDF ranking, and query-focused snippet extraction.
* **Grounded Document Q&A & Multi-Turn Chat (`srz ask`)**: Ask targeted single-turn queries or maintain persistent multi-turn chat sessions grounded directly in document content.
* **Reading Analytics & Model Telemetry (`srz stats`)**: Track reading streaks, active days, estimated reading time saved (225 wpm baseline), and local model token throughput percentiles (P50/P95).
* **Consolidated DuckDB Storage & TTL Caching**: High-speed embedded storage (`srz.duckdb` and `cache.duckdb`) with atomic transactions, SHA-256 hash indexing, and millisecond-accurate TTL pruning.
* **Terminal Display & Paging**: Syntax-highlighted output with automatic `$PAGER` integration (`less -RFX`) and terminal escape sanitization.
