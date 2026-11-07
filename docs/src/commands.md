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
| | `srz browse` (alias `tui`) | Interactive TUI dashboard (`[j/k]` navigate, `[/]` filter, `[a]` ask/chat, `[y]` copy, `[d]` delete, `[Enter]` open). |
| **Library & History** | `srz find "keyword"` | Offline BM25 relevance-ranked search across past summaries in DuckDB. |
| | `srz history [--limit N]` | Displays recent reads, word counts, model usage, and generation speeds. |
| | `srz stats` | Telemetry overview: reading streaks, days active, time saved, top domains, and model throughput percentiles (P50/P95). |
| | `srz index [--open]` | Generates and opens an interactive HTML library dashboard. |
| | `srz query "<SQL>"` | Execute read-only analytical SQL queries on DuckDB with formatted tables, `--csv`, or `--json`. |
| | `srz export -f <FORMAT> -o <PATH>` | Export summarized library to Obsidian/Logseq Markdown vault, JSON, CSV, or Parquet. |
| **System & Config** | `--profile <NAME>` | Switches LLM backends defined in `~/.config/srz/config.toml`. |
| | `srz config <init\|show\|path>` | Manages and inspects the local configuration file. |
| | `srz cache <dir\|size\|clean\|prune>` | Cache inspection, TTL/LRU pruning (`--older-than`, `--max-size`), and safe clean (`--reset-db`). |
| | `srz models` | Lists available models reported by the local LLM backend. |
| | `srz recommend` | Recommends optimal model and context window for host hardware. |
| | `srz doctor` | Tests backend reachability, latency, cache health, and database integrity. |
| | `srz upgrade [--check\|--force]` | Safe, in-place binary self-updater with SHA-256 integrity verification. |
| | `srz completions <SHELL>` | Generates autocomplete scripts for bash, zsh, fish, and powershell. |

---

## 1. Root Target Dispatch (`srz <TARGET>`)

```bash
srz <TARGET> [FLAGS]
```

### How Target Auto-Detection Works

Instead of forcing you to memorize dedicated subcommands like `srz web`, `srz file`, or `srz book`, `srz` inspects the structure of your `<TARGET>` argument and dispatches it through specialized ingestion pipelines:

When `<TARGET>` begins with `http://` or `https://`, the router analyzes the URL domain and path. Links pointing to `youtube.com` or `youtu.be` route directly to the YouTube timed-text caption client. If the URL path or query parameters indicate a syndication feed (such as `.rss`, `.xml`, `.atom`, or `/feed`), the target is parsed via the universal syndication engine. All other standard web links pass to the web scraper, where Mozilla Readability scores DOM container density to strip navigation bars, advertisements, and cookie notices.

Local targets receive similarly intelligent routing. If `<TARGET>` matches an existing file path or ends with a standard document extension (`.pdf`, `.html`, `.md`, or `.txt`), the document loader reads and validates the file under a 20 MB ceiling. Passing a single hyphen (`-`) tells `srz` to ingest data from standard input, enabling seamless pipeline integration with commands like `git diff | srz -`. Queries beginning with `news ` route to the Google News RSS aggregator, while any remaining freeform text strings are treated as book titles, triggering concurrent metadata and synopsis resolution across Open Library and Wikipedia.

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
| `--show-thinking` | `--thinking` | Stream full raw model reasoning trace to `stderr` (default: compact one-liner dropdown). |
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
| `Tab` | Switch focus between summary list and reader preview |
| `a` | Seamlessly launch grounded interactive chat (`srz ask`) on the selected document |
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
* **Single-turn query against a specific document, web page, YouTube video, or remote PDF**:
  ```bash
  srz ask assets/document.pdf "What are the primary conclusions?"
  srz ask "https://www.youtube.com/watch?v=5C_HPTJg5ek" "What are the key points in this talk?"
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
* `-t, --timeframe <TIMEFRAME>`: Recency filter: `week` (default, past 7 days), `day` (past 24h), `month` (past 30 days), `all`.
* `-l, --limit <N>`: Maximum number of articles to aggregate (default: 5).

---

## 5. Offline BM25 Search (`srz find`)

```bash
srz find <QUERY>
```

Executes a local Robertson-Sparck Jones BM25 search ranking across your entire DuckDB reading history without internet access.

* **2.5× Title Weight Boost**: Terms matching document titles receive 2.5× the weight of terms matching summary bodies.
* **Document Length Normalization**: Sized with tuned parameters ($k_1 = 1.2, b = 0.75$) to balance term repetition and document length.
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

## 7. Analytical SQL Interface (`srz query`)

```bash
srz query "<SQL>" [FLAGS]
```

Executes an arbitrary read-only analytical SQL query against the local DuckDB database (`srz.duckdb`). Results are formatted into an aligned terminal table by default, or exported as CSV or JSON.

```bash
# Query model usage and token counts
srz query "SELECT model, count(*) AS runs, avg(tokens_per_second) FROM history GROUP BY model"

# Output as CSV
srz query "SELECT title, word_count, style FROM history" --csv

# Output as JSON
srz query "SELECT id, title, target_type FROM library LIMIT 5" --json
```

---

## 8. Library Vault & Data Export (`srz export`)

```bash
srz export --format <FORMAT> --output <PATH>
```

Exports your offline summarized library into standard portable formats:

* **Markdown Vault (`--format markdown`)**: Exports every summary into an individual Markdown file equipped with Obsidian and Logseq-compatible YAML frontmatter (`title`, `target_type`, `model`, `style`, `word_count`, `created_at`, `author`, `url`).
  ```bash
  srz export --format markdown ~/Obsidian/Vault/Summaries/
  ```
* **Structured JSON (`--format json`)**: Exports the entire library and metadata into a single formatted JSON file.
  ```bash
  srz export --format json ~/backup_library.json
  ```
* **CSV (`--format csv`)**: Exports all library rows and timing metrics as a spreadsheet-friendly CSV.
  ```bash
  srz export --format csv ~/summaries.csv
  ```
* **Parquet (`--format parquet`)**: Exports to compressed Apache Parquet for data science workflows with Polars, Pandas, or DuckDB CLI.
  ```bash
  srz export --format parquet ~/library.parquet
  ```

---

## 9. Storage & Cache Maintenance (`srz cache`)

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

## 10. Configuration Management (`srz config`)

```bash
srz config <SUBCOMMAND>
```

* `srz config path`: Display path to `config.toml` and report whether it exists.
* `srz config show`: Print currently active configuration settings.
* `srz config init`: Generate a starter `~/.config/srz/config.toml` file tailored to your host RAM, CPU, and recommended context window.

---

## 11. Diagnostics & Recommendations (`srz recommend`, `srz doctor`, `srz models`)

```bash
# Instant hardware & model recommendation card
srz recommend
srz recommend --json

# Comprehensive diagnostic audit (hardware, LLM latency, n_ctx, DuckDB, cache, pager)
srz doctor

# List available models reported by the connected backend
srz models
```

### Hardware Profiling & Diagnostic Advisories

Running `srz recommend` or `srz doctor` profiles your physical machine without invasive C FFI bindings. By inspecting host system characteristics via native kernel and procfs hardware profiling, `srz` classifies your system into hardware tiers and suggests optimal model configurations:

The advisor recommends high-fidelity Google Gemma 4 models tailored to your physical RAM, matching the exact selection cards shown in Llama GUI (such as `gemma-4` with size `E4B` or `26B-A4B` at `Q8_0` quantization) while configuring the full model ID (`ggml-org/gemma-4-E4B-it-GGUF:Q8_0`) for seamless backend API communication. It calculates your single-pass token synthesis ceiling—the approximate 70% threshold of your context window—beyond which map-reduce chunking activates.

When running `srz doctor`, the engine performs an active end-to-end audit: it pings the local LLM endpoint, probes operational context lengths (`n_ctx`) and concurrency slots (`total_slots`), and verifies the integrity of DuckDB databases and cache directories. If the connected runner is constrained by sub-optimal context sizing (under 8k tokens) or slot bottlenecks, `srz doctor` provides concrete terminal guidance to unlock maximum throughput.

For the complete tier-by-tier hardware breakdown, refer to the canonical [Hardware Tier Reference Matrix](quickstart.md#hardware-tier-reference-matrix).

---

## 12. Self Updates & Shell Completions

```bash
# Check for updates or upgrade in place
srz upgrade --check
srz upgrade

# Generate autocomplete scripts (bash, zsh, fish, powershell)
# Zsh (add to ~/.zshrc):
eval "$(srz completions zsh)"

# Bash (add to ~/.bashrc):
eval "$(srz completions bash)"

# Fish (add to ~/.config/fish/config.fish):
srz completions fish | source

# PowerShell (add to $PROFILE):
srz completions powershell | Out-String | Invoke-Expression
```
