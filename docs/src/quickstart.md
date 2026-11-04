# Quick Start

`srz` connects to any OpenAI API-compliant LLM server. While cloud provider APIs are supported, running a **local LLM is strongly preferred** for complete privacy, zero subscription fees, and offline capability.

> 💡 **New to Local LLMs?**  
> A local LLM runner (such as **Llama GUI**, **llama.cpp**, or **Ollama**) is an application that runs language models directly on your computer's CPU or GPU. This ensures 100% privacy—no text, summaries, transcripts, or personal reading habits ever leave your machine.

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

### Verify Connectivity & Recommendations

Get an instant recommendation for your hardware and test connection health:

```bash
# Instant model and context window recommendation
srz recommend

# Full connectivity, cache health, and database diagnostics
srz doctor

# List models reported by the backend
srz models
```

---

## 2. Recommended Models & Context Length by Hardware

Before running your first summary, you can easily determine which model and context window to select based on your computer's RAM.

### 1-Command Auto-Advisor: `srz recommend`

Run `srz recommend` to inspect your host hardware and receive a customized model, context length, and ready launch commands for both `llama.cpp` and `Ollama`:

```bash
srz recommend
```

```text
=== srz Recommended Model & Context Length ===

Host Machine:
  • Device: Apple M5 Pro
  • Memory: 24.0 GB RAM (24–36 GB Tier (High-End Pro / Max Laptop))

Recommended Local LLM Setup (Llama GUI / llama.cpp):
  • Model:   gemma-4
  • Size:    E4B
  • Quant:   Q8_0
  • Context: 64k ctx (65536 tokens, ~8.5 GB total)

How to Load in Your Runner:
  • Llama GUI (Llama-macOS / Llama-Windows - localhost:9931):
      Open Llama GUI and select/download:
      gemma-4  |  E4B  |  Q8_0  |  64k ctx
  • llama.cpp CLI (headless llama-server):
      llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 65536 --port 9931
  • Ollama:
      ollama run gemma2:9b --ctx-size 65536

Summary Synthesis Speed:
  💡 Fast 1-Pass Summaries: Documents up to ~45k tokens (~183 pages) synthesize in a single pass.
     Larger documents automatically use parallel map-reduce chunking without OOM crashes.
```

### Hardware Tier Reference Matrix

| Host Memory | Typical Devices | Model Name | Parameters | Quantization | Disk Size | Context Window (`n_ctx`) | Llama GUI Selection |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **8 GB RAM** | MacBook Air 8GB, Thin-and-light Laptops | `gemma-4` | **~4B** (Effective) | **`Q8_0`** | **~8.6 GB** | **`8k ctx`** (8,192) | `gemma-4 \| E4B \| Q8_0 \| 8k ctx` |
| **16 GB RAM** | MacBook Pro/Air 16GB, Standard PC Laptops | `gemma-4` | **~4B** (Effective) | **`Q8_0`** | **~8.6 GB** | **`32k ctx`** (32,768) | `gemma-4 \| E4B \| Q8_0 \| 32k ctx` |
| **24–36 GB RAM** | MacBook Pro M Pro / Max, 32GB PCs | `gemma-4` | **~4B** (Effective) | **`Q8_0`** | **~8.6 GB** | **`64k ctx`** (65,536) | `gemma-4 \| E4B \| Q8_0 \| 64k ctx` |
| **64+ GB RAM** | MacBook Pro M Max, Mac Studio, Workstations | `gemma-4` | **26B Total / ~4B Active** (MoE) | **`Q8_0`** | **~28.0 GB** | **`128k ctx`** (131,072) | `gemma-4 \| 26B-A4B \| Q8_0 \| 128k ctx` |

### Single-Pass vs. Map-Reduce Chunking

- **1-Pass Mode**: When document token count $\le \sim 70\%$ of the configured server context window (`n_ctx`), `srz` synthesizes the document in one pass with complete cross-chapter coherence.
- **Chunking Mode**: When documents exceed this threshold (e.g. 200+ page books or large document bundles on small context servers), `srz` automatically partitions the text into token-budgeted chunks, summarizes them in parallel up to the server's slot capacity (`total_slots`), and executes a final reduction pass.

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

## 4. Interactive Grounded Q&A (`srz ask`)

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

## 5. Terminal Explorer & Offline Library Search

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
