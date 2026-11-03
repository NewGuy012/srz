# Command Reference

`srz` provides a unified CLI interface where root targets are automatically routed based on input structure, accompanied by specialized subcommands.

---

## Key Features & Subcommands Quick Reference

| Category | Command / Flag | Description |
| :--- | :--- | :--- |
| **Ingestion** | `srz <URL>` | Summarizes articles, blogs, documentation, RSS/Atom/JSON feeds, or online PDFs. |
| | `srz <YOUTUBE_URL>` | Extracts timed-text transcripts and chapters from YouTube videos. |
| | `srz "Book Title"` | Resolves synopses from Wikipedia and Open Library by title alone. |
| | `srz <FILE>` | Ingests local `.pdf`, `.html`, `.md`, or `.txt` documents (20 MB ceiling). |
| | `cat ... \| srz -` | Reads piped command outputs, logs, or `git diff` streams (10 MB ceiling). |
| | `srz news "<topic\|feed_url>" [-t week]` | Aggregates and summarizes top stories from Google News RSS or any RSS/Atom/JSON feed URL. |
| **Styling & Output** | `--style <PRESET>` | 3 summary formats: `concise` (default), `bullets`, `detailed`. |
| | `--max-tokens <NUM>` | Sets the maximum completion tokens to generate (default: `8192`). |
| | `--context-window <NUM>` | Overrides auto-detected server context window in tokens (e.g. `32768`). |
| | `--concurrency <NUM>` | Sets parallel map-reduce chunking concurrency (default: autoscaled to server slots). |
| | `--open` (`-b`) | Opens the summary in a distraction-free HTML reader with light/dark mode. |
| | `--plain` | Outputs raw Markdown without terminal colors or automatic paging. |
| | `--paging <auto\|always\|never>` | Controls terminal output pager (`never` disables paging). |
| | `-o <FILE.html\|md>` | Saves the rendered output directly to an HTML or Markdown file. |
| | `--json` | Emits machine-readable JSON on `stdout` for script integration. |
| | `--stream` | Streams LLM output tokens live to `stdout` as they are generated. |
| **Interactive Tools** | `srz ask [TARGET] [Q]` | Grounded Q&A against a document, URL, file, or latest read. |
| | `srz ask -i` | Interactive multi-turn chat REPL grounded on source context. |
| | `srz ask [--chats\|--resume <ID>]` | Lists saved chat sessions or resumes a previous multi-turn conversation. |
| | `srz ask [--show-chat\|--delete-chat] <ID>` | Inspects or deletes a persisted multi-turn chat session transcript. |
| | `srz browse` (alias `tui`) | Interactive TUI dashboard (`[j/k]` navigate, `[/]` filter, `[y]` copy, `[d]` delete, `[Enter]` open). |
| **Library & History** | `srz find "keyword"` | Offline BM25 relevance-ranked search across past summaries in DuckDB. |
| | `srz history [--limit N]` | Displays recent reads, word counts, model usage, and generation speeds. |
| | `srz stats` | Telemetry overview: reading streaks, days active, time saved, top domains, and model throughput percentiles (P50/P95). |
| | `srz index [--open]` | Generates and opens an interactive HTML library dashboard. |
| **System & Config** | `--profile <NAME>` | Switches LLM backends defined in `~/.config/srz/config.toml`. |
| | `srz config <init\|show\|path>` | Manages and inspects the local configuration file. |
| | `srz cache <dir\|size\|clean\|prune>` | Cache inspection, TTL/LRU pruning (`--older-than`, `--max-size`), and safe clean (`--reset-db`). |
| | `srz models` | Lists available models reported by the local LLM backend. |
| | `srz recommend` | Recommends optimal model, context window, and ready launch commands for host hardware. |
| | `srz doctor` | Tests backend reachability, latency, cache health, and database integrity. |
| | `srz upgrade [--check\|--force]` | Safe, in-place binary self-updater with SHA-256 integrity verification. |
| | `srz completions <SHELL>` | Generates autocomplete scripts for bash, zsh, fish, powershell, and elvish. |

---

## 1. Root Target Dispatch (`srz <TARGET>`)

```bash
srz <TARGET> [FLAGS]
```

### Auto-Detection Rules
* **URLs (`http://` or `https://`)**:
  * If the domain is `youtube.com` or `youtu.be`: Routes to **YouTube Timed-Text Ingestion**.
  * If the path or query matches a syndication feed (`.xml`, `.rss`, `.atom`, `/feed`, etc.): Routes to **Syndication Feed & News Aggregator**.
  * Otherwise: Routes to **Web Scraper** (Mozilla Readability content extraction).
* **Local Files**:
  * If `<TARGET>` points to an existing file or has a recognizable document extension (`.pdf`, `.html`, `.md`, `.txt` up to 20 MB ceiling): Routes to **Local Document Loader**.
* **Unix Standard Input**:
  * If `<TARGET>` is `-`: Routes to **Unix Stdin Stream Reader** (10 MB ceiling).
* **News Queries**:
  * If `<TARGET>` begins with `news ` (e.g. `srz "news rust"`): Routes to **Google News RSS Aggregator**.
* **Books**:
  * Any other text target routes to the **Book Pipeline**, resolving metadata and synopses from Open Library and Wikipedia.

### Global Flags

| Flag | Short | Description |
| :--- | :--- | :--- |
| `--style <STYLE>` | `-s` | Summary format preset: `concise` (default), `bullets`, `detailed`. |
| `--model <MODEL>` | `-m` | Override the configured LLM model name. |
| `--base-url <URL>` | | Local or remote OpenAI-compatible base URL (default: `http://localhost:9931/v1`). |
| `--api-key <KEY>` | | OpenAI API key (optional for local LLMs). |
| `--profile <NAME>` | | Load a named configuration profile from `~/.config/srz/config.toml`. |
| `--config <PATH>` | | Custom configuration file path (defaults to `~/.config/srz/config.toml`). |
| `--context-window <N>` | | Override server context window size in tokens (e.g. 8192-131072; auto-detected if omitted). |
| `--max-tokens <N>` | | Maximum tokens to generate (thinking + summary tokens; default: 8192). |
| `--concurrency <N>` | | Max concurrent worker tasks during map-reduce document chunking (default: 1). |
| `--open` | `-b` | Automatically open the rendered HTML summary in your default web browser. |
| `--output <FILE>` | `-o` | Save the generated output directly to a file (`.html` or `.md`). |
| `--plain` | | Output raw Markdown without terminal formatting, ANSI colors, or paging. |
| `--paging <MODE>` | | Terminal paging control: `auto` (default), `always`, `never`. |
| `--stream` | | Stream LLM output tokens live to `stdout` as they are generated. |
| `--json` | | Emit machine-readable JSON output for scripts and automation. |
| `--no-cache` | | Bypass the local DuckDB cache and force fresh scraping and synthesis. |
| `--quiet` | `-q` | Suppress status spinners and non-essential progress messages. |
| `--verbose` | `-v` | Display detailed diagnostic timings and network logs. |

---

## 2. Interactive Terminal Explorer (`srz browse`)

```bash
srz browse [FLAGS]
# Aliases: srz tui, srz library
```

Launches an interactive, full-screen Ratatui terminal dashboard for exploring past reads and managing summaries.

### Keybindings

| Key | Action |
| :--- | :--- |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `Enter` | Open the reader view for the selected summary |
| `/` | Focus live search bar to filter summaries in real time |
| `Tab` | Cycle category filters (`All`, `Web`, `PDF`, `Books`, `News`, `Chat`) |
| `y` / `c` | Copy summary markdown to system clipboard |
| `d` | Delete selected summary from history |
| `Esc` / `q` | Exit search focus, close reader view, or quit application |

---

## 3. Grounded Q&A & Multi-Turn Chat (`srz ask`)

```bash
srz ask [TARGET] [QUESTION] [FLAGS]
# Alias: srz chat
```

Performs question-answering grounded directly on document context, protected with prompt injection quarantine tags (`<source_document>`).

* **Single-turn query against latest read**:
  ```bash
  srz ask "What are the main findings?"
  ```
* **Single-turn query against a specific document or URL**:
  ```bash
  srz ask assets/document.pdf "What are the primary conclusions?"
  ```
* **Interactive multi-turn chat REPL**:
  ```bash
  srz ask -i
  srz ask assets/document.pdf -i
  ```
  *Inside the REPL, ask questions naturally. Type `exit`, `quit`, or press `Ctrl+C` / `Ctrl+D` to end the session.*

### Session Management Flags

* `--chats`: List saved persistent chat sessions stored in DuckDB.
* `--resume <ID>`: Resume a previous conversation session by ID.
* `--show-chat <ID>`: Print complete transcript of a past chat session.
* `--delete-chat <ID>`: Delete a saved chat session.

---

## 4. Live News Digests & Feeds (`srz news`)

```bash
srz news <TOPIC_OR_FEED_URL> [FLAGS]
```

Aggregates and synthesizes top news stories from Google News RSS topics or directly from syndication feeds.

* **Search by Topic**:
  ```bash
  srz news "artificial intelligence" -t week
  ```
* **Summarize Direct RSS/Atom Feed**:
  ```bash
  srz news "https://blog.rust-lang.org/feed.xml"
  # Or pass directly as root target:
  srz "https://blog.rust-lang.org/feed.xml"
  ```
* `-t, --timeframe <TIMEFRAME>`: Recency filter: `day` (default, past 24h), `week` (past 7 days), `month` (past 30 days), `all`.
* `-l, --limit <N>`: Maximum number of articles to aggregate (default: 5).

---

## 5. Offline BM25 Search (`srz find`)

```bash
srz find <QUERY>
```

Executes a local Robertson-Sparck Jones BM25 search ranking across your entire DuckDB reading history without internet access.

* $2.5\times$ title weight boost for matching terms.
* Document length normalization ($k_1=1.2, b=0.75$).
* Extracts relevance-focused preview snippets centered around query terms.

---

## 6. Reading Analytics & History

```bash
# View recent reads, token metrics, and model speeds
srz history [--limit <N>]

# View reading habit telemetry, streaks, time saved, and P50/P95 throughput
srz stats

# Generate and open the interactive standalone HTML library dashboard
srz index [--open]
```

---

## 7. Storage & Cache Maintenance (`srz cache`)

```bash
srz cache <SUBCOMMAND>
```

* `srz cache dir`: Display the absolute path to the local cache directory (`~/.cache/srz`).
* `srz cache size`: Show disk usage broken down by pages, search metadata, summaries, and views.
* `srz cache prune`: Evict expired items based on age or disk size budget:
  ```bash
  srz cache prune --older-than 14d
  srz cache prune --max-size 500MB
  ```
* `srz cache clean`: Clear all cached web pages and view files.
  * `--reset-db`: Also wipes the permanent `srz.duckdb` analytics database.

---

## 8. Configuration Management (`srz config`)

```bash
srz config <SUBCOMMAND>
```

* `srz config path`: Display path to `config.toml` and report whether it exists.
* `srz config show`: Print currently active configuration settings.
* `srz config init`: Generate a starter `~/.config/srz/config.toml` file tailored to your host RAM, CPU, and recommended context window.

---

## 9. Diagnostics & Recommendations (`srz recommend`, `srz doctor`, `srz models`)

```bash
# Instant hardware & model recommendation card with ready launch commands
srz recommend
srz recommend --json

# Comprehensive diagnostic audit (hardware, LLM latency, n_ctx, DuckDB, cache, pager)
srz doctor

# List available models reported by the connected backend
srz models
```

### Hardware & LLM Advisor in `srz recommend` & `srz doctor`
Running `srz recommend` or `srz doctor` automatically profiles host memory and suggests the optimal local configuration:
* **Host Hardware Profile**: Displays detected host RAM (e.g. 24 GB RAM, `Tier32GB`) and CPU architecture.
* **Hardware Steering Advisor**: Recommends high-fidelity Gemma 4 models (e.g., `ggml-org/gemma-4-E4B-it-GGUF:Q8_0` or `26B-A4B-it:Q8_0`) with the optimal context window for your RAM.
* **Multi-Runner Commands**: Ready-to-copy launch commands for both `llama-server` and `Ollama`.
* **Single-Pass Synthesis Ceiling**: Displays the token threshold (~70% of context window) before map-reduce chunking kicks in.
* **Server Capabilities (`srz doctor`)**: Probes server `n_ctx` (context length) and `total_slots` (concurrency capacity) from `llama.cpp` (`/props`) or `Ollama` (`/api/show`).
* **Sub-Optimal Context Alerts**: Warns if server context window is configured below 8,192 tokens or concurrency slots are bottlenecked.

| Host Memory | Recommended Model | Precision | Context Window (`n_ctx`) | Total RAM Footprint | llama.cpp Launch Command | Ollama Launch Command |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **8 GB RAM** | `gemma-4-E4B-it` | `Q8_0` | **8,192** (8k) | ~5.0 GB | `llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 8192 --port 9931` | `ollama run gemma2:9b --ctx-size 8192` |
| **16 GB RAM** | `gemma-4-E4B-it` | `Q8_0` | **32,768** (32k) | ~6.5 GB | `llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 32768 --port 9931` | `ollama run gemma2:9b --ctx-size 32768` |
| **24–36 GB RAM** | `gemma-4-E4B-it` | `Q8_0` | **65,536** (64k) | ~8.5 GB | `llama-server -m gemma-4-E4B-it-Q8_0.gguf -c 65536 --port 9931` | `ollama run gemma2:9b --ctx-size 65536` |
| **64+ GB RAM** | `gemma-4-26B-it` | `Q8_0` | **131,072** (128k) | ~28–32 GB | `llama-server -m gemma-4-26B-it-Q8_0.gguf -c 131072 --port 9931` | `ollama run gemma2:27b --ctx-size 131072` |

---

## 10. Self Updates & Shell Completions

```bash
# Check for updates or upgrade in place
srz upgrade --check
srz upgrade

# Generate autocomplete scripts (bash, zsh, fish, powershell, elvish)
# Zsh (add to ~/.zshrc):
eval "$(srz completions zsh)"

# Bash (add to ~/.bashrc):
eval "$(srz completions bash)"

# Fish (add to ~/.config/fish/config.fish):
srz completions fish | source

# PowerShell (add to $PROFILE):
srz completions powershell | Out-String | Invoke-Expression
```
