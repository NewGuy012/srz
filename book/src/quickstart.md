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

## 2. Basic Usage Examples

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

### Summarize Live News Feeds
Aggregate and synthesize top news stories from Google News RSS:

```bash
# Recent news on a topic (past 24 hours by default)
srz news "Rust language updates"

# Filter by recency (day, week, month, all)
srz news "artificial intelligence" -t week
```

---

## 3. Interactive Grounded Q&A (`srz ask`)

Ask targeted questions or start an interactive chat session grounded directly in document text:

```bash
# Ask a question about your most recently summarized item
srz ask "What are the main findings?"

# Ask a question about a specific document or file
srz ask assets/document.pdf "What are the primary conclusions?"

# Launch an interactive multi-turn chat REPL
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
