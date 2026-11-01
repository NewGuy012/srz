# Quick Start

`srz` connects to any OpenAI API-compliant LLM server. While cloud provider APIs are supported, running a **local LLM is strongly preferred** for complete privacy, zero subscription fees, and offline capability.

---

## 1. LLM Backend Setup

### Recommended Local Backends (Default Port `9931`)

`srz` connects by default to `http://localhost:9931/v1`, requiring zero configuration for official desktop runners:

* **macOS**: Install [Llama-macOS](https://github.com/ggml-org/Llama-macOS). Launch the application, load any GGUF model, and `srz` connects immediately.
* **Windows**: Install [Llama-Windows](https://github.com/ggml-org/Llama-Windows). Launch the application, load a model, and start summarizing.
* **Linux**: Build or run [llama.cpp](https://github.com/ggml-org/llama.cpp):
  ```bash
  # Start llama-server on port 9931 for zero configuration:
  llama-server -m model.gguf --port 9931

  # Or on default port 8080:
  llama-server -m model.gguf --port 8080
  export OPENAI_BASE_URL="http://localhost:8080/v1"
  ```

### Other Local & Cloud Backends

* **Ollama**: `ollama run mistral` (connects on port `11434`):
  ```bash
  export OPENAI_BASE_URL="http://localhost:11434/v1" OPENAI_MODEL="mistral"
  ```
* **LM Studio**: Start Local Server on port `1234`:
  ```bash
  export OPENAI_BASE_URL="http://localhost:1234/v1"
  ```
* **Cloud Providers (OpenAI, Anthropic, Google, Groq)**:
  ```bash
  export OPENAI_BASE_URL="https://api.openai.com/v1" OPENAI_MODEL="gpt-4o-mini" OPENAI_API_KEY="sk-..."
  ```

### Verify Connectivity

Test your connection and inspect available models:

```bash
# Check connectivity, cache health, and database integrity
srz doctor

# List models reported by the backend
srz models
```

---

## 2. Recommended Models & Context Length by Hardware

Before running your first summary, you can easily determine which model and context window to select based on your computer's RAM.

### 1-Click Auto-Advisor: `srz doctor`

Run `srz doctor` to inspect your host hardware and receive a customized model, context length, and launch command:

```bash
srz doctor
```

```text
=== srz System & LLM Health Diagnostics ===

Hardware Profile & Recommendations:
  • Host Device: Apple M5 Pro (24.0 GB RAM / 24–36 GB Tier (High-End Pro / Max Laptop))
  • Recommended Family: Google Gemma 4 (High fidelity Q8_0, laptop battery & memory optimized)
  • Recommended Model: ggml-org/gemma-4-E4B-it-GGUF:Q8_0 (Q8_0)
  • Recommended Context: 65536 tokens
  • Recommended Command: llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 65536 --port 9931
```

### Hardware Tier Reference Matrix

| Host Memory | Typical Laptops / Devices | Recommended Model | Quantization | Context Window (`n_ctx`) | Approximate RAM Usage | Ready Launch Command |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **8 GB RAM** | MacBook Air 8GB, Standard Laptops | `gemma-4-E4B-it` | **`Q8_0`** | **8,192** (8k) | ~5.0 GB total | `llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 8192 --port 9931` |
| **16 GB RAM** | MacBook Pro/Air 16GB, Standard PC Laptops | `gemma-4-E4B-it` | **`Q8_0`** | **32,768** (32k) | ~6.5 GB total | `llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 32768 --port 9931` |
| **24–36 GB RAM** | MacBook Pro M Pro / Max, 32GB PCs | `gemma-4-E4B-it` | **`Q8_0`** | **65,536** (64k) | ~8.5 GB total | `llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 65536 --port 9931` |
| **64+ GB RAM** | MacBook Pro M Max, Mac Studio, Workstations | `gemma-4-26B-it` | **`Q8_0`** | **131,072** (128k) | ~28–32 GB total | `llama-server -m gemma-4-26B-it-Q8_0.gguf -c 131072 --port 9931` |

### Why We Recommend Google Gemma 4 (`Q8_0`) for Laptops

* **Battery & Compute Efficiency**: Gemma 4 is built for exceptional inference speed per watt, making it optimal for laptop battery life.
* **High-Fidelity `Q8_0` Precision**: Heavy 4-bit quantizations (`Q4_K_M`) degrade summarization nuance and can cause hallucinations in complex documents. `Q8_0` maintains near-lossless reasoning fidelity while comfortably fitting into unified memory.
* **Context Scaling Without Degraded Quality**: Gemma 4 supports up to 128k context windows natively.
* **Zero-Drift Native Tokenization**: `srz` queries the server's native `/tokenize` endpoint, completely eliminating tokenizer drift between client chunking and server execution.

---

## 3. Basic Usage Examples

### Summarize a Webpage or Article
Pass any URL directly. `srz` extracts clean text using Mozilla Readability, stripping ads and sidebars:

```bash
srz "https://en.wikipedia.org/wiki/Rust_(programming_language)"
```

### Summarize a YouTube Video
Extract and synthesize closed-caption transcripts from YouTube URLs:

```bash
srz "https://youtu.be/5C_HPTJg5ek"
```

### Summarize a Local PDF or Document
Pass a path to a `.pdf`, `.html`, `.md`, or `.txt` file:

```bash
srz assets/document.pdf
```

### Summarize Unix Standard Input
Pipe terminal command output, logs, or git diffs directly into `srz`:

```bash
git diff | srz -
cat server.log | srz -
```

### Summarize a Book by Title
`srz` queries Open Library and Wikipedia to enrich book metadata and synopses before summarizing:

```bash
srz "The Rust Programming Language" --open
```

### Summarize Live News & RSS Feeds
Aggregate and synthesize news stories from Google News RSS or summarize direct syndication feeds:

```bash
# Recent news on a topic (past 24 hours by default)
srz news "Rust language updates"

# Filter by recency (day, week, month, all)
srz news "artificial intelligence" -t week

# Or summarize any RSS, Atom, or JSON feed directly
srz "https://blog.rust-lang.org/feed.xml"
```

---

## 3. Interactive Grounded Q&A (`srz ask`)

Ask targeted questions or start an interactive chat session grounded directly in document text (persisted to DuckDB):

```bash
# Ask a question about your most recently summarized item
srz ask "What are the main findings?"

# Ask a question about a specific document or file
srz ask assets/document.pdf "What are the primary conclusions?"

# Launch an interactive multi-turn chat REPL (type 'exit' or Ctrl+D to quit)
srz ask -i

# Inspect and resume saved chat sessions from DuckDB
srz ask --chats
srz ask --resume 1
srz ask --show-chat 1
```

---

## 4. Terminal Explorer & Offline Library Search

### Full-Screen Ratatui Terminal Dashboard (`srz browse`)
Explore your reading history, filter items by category, copy summaries to clipboard, or read in fullscreen:

```bash
srz browse
```

### Fast Offline BM25 Search (`srz find`)
Search through past summaries stored in DuckDB without network access:

```bash
srz find "memory safety"
```

### Analytics & Maintenance
```bash
srz history        # View recent reads and word counts
srz stats          # Reading streak, time saved, and model token speeds
srz cache size     # View disk usage
srz cache prune    # Evict items older than 14 days
```
