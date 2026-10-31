# Command Reference

`srz` provides a unified CLI interface where root targets are automatically routed based on input structure, accompanied by specialized subcommands.

---

## 1. Root Target Dispatch (`srz <TARGET>`)

```bash
srz <TARGET> [FLAGS]
```

### Auto-Detection Rules
* **URLs (`http://` or `https://`)**:
  * If the domain is `youtube.com` or `youtu.be`: Routes to **YouTube Timed-Text Ingestion**.
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

### Session Management Flags

* `--chats`: List saved persistent chat sessions stored in DuckDB.
* `--resume <ID>`: Resume a previous conversation session by ID.
* `--show-chat <ID>`: Print complete transcript of a past chat session.
* `--delete-chat <ID>`: Delete a saved chat session.

---

## 4. Live News Digests (`srz news`)

```bash
srz news <TOPIC> [FLAGS]
```

Aggregates and synthesizes top news stories from Google News RSS feeds.

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
* `srz config init`: Generate a starter `~/.config/srz/config.toml` file with documented options.

---

## 9. Diagnostics (`srz doctor` & `srz models`)

```bash
# Diagnose LLM endpoint latency, DuckDB integrity, cache health, and pager setup
srz doctor

# List available models reported by the connected backend
srz models
```

---

## 10. Self Updates & Shell Completions

```bash
# Check for updates or upgrade in place
srz upgrade --check
srz upgrade

# Generate autocomplete scripts (bash, zsh, fish, powershell, elvish)
eval "$(srz completions zsh)"
```
