# Quick Start

`srz` connects to any local OpenAI-compatible inference server. 

### Prerequisites

Ensure a local LLM runner is active:
* **Ollama**: `ollama run mistral` (listens on `http://localhost:11434/v1`)
* **llama.cpp**: `./llama-server -m model.gguf --port 8080`
* **LM Studio**: Start Local Server (listens on `http://localhost:1234/v1`)
* **vLLM**: `vllm serve model --port 8000`

---

## Basic Usage Examples

### 1. Summarize a Webpage
Pass any HTTP/HTTPS URL directly as the target:
```bash
srz https://en.wikipedia.org/wiki/Information_theory
```

### 2. Summarize a YouTube Video
Extract and synthesize summaries from YouTube video closed captions:
```bash
srz https://www.youtube.com/watch?v=dQw4w9WgXcQ
```

### 3. Summarize a Local PDF or Document
Pass a path to a PDF, markdown, or plain text file:
```bash
srz path/to/document.pdf
srz notes.md
```

### 4. Summarize Standard Input
Pipe command output or stream text directly into `srz`:
```bash
cat large_log.txt | srz -
curl -sSL https://example.com | srz -
```

### 5. Summarize a Book
`srz` automatically queries Open Library and Wikipedia to enrich book synopses:
```bash
srz "Dune"
srz "Neuromancer" --author "William Gibson"
```

### 6. Live News Feeds
Aggregate and summarize top news articles on any topic:
```bash
srz news "artificial intelligence"
srz news "fusion energy" --time 7d
```

### 7. Interactive Terminal Explorer
Launch the full-screen terminal dashboard to explore reading history, search summaries, or copy text:
```bash
srz browse
```

### 8. Grounded Document Q&A
Ask direct questions grounded on any source:
```bash
srz ask report.pdf "What are the key revenue metrics?"
```
Or start an interactive multi-turn chat conversation:
```bash
srz ask report.pdf
```
