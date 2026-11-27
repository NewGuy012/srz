# Command Reference

`srz` uses explicit subcommands for every ingestion source (`web`, `document`, `book`, `news`, `stdin`) alongside interactive Q&A (`ask`) and library utilities.

---

## Key Features & Subcommands Quick Reference

| Category | Command / Flag | Description |
| :--- | :--- | :--- |
| **Ingestion** | `srz web <URL>` | Summarizes webpages (Readability), YouTube video transcripts, or remote `.pdf` URLs. |
| | `srz document <PATH>` | Ingests local documents (`.pdf`, `.epub`, `.html`, `.md`, `.txt`, `.json`, or extensionless PDF/EPUB/HTML via magic bytes; 20 MB ceiling). |
| | `srz book "<TITLE>" [--author <A>]` | Resolves book metadata and synopses from Open Library, Wikipedia, and DuckDuckGo. |
| | `srz news "<TOPIC\|FEED_URL>" [-t all]` | Aggregates and summarizes top stories from Google News RSS or any RSS/Atom/JSON feed URL. |
| | `cat ... \| srz stdin` | Reads piped command outputs, logs, or `git diff` streams (10 MB ceiling). |
| **Styling & Output** | `--style <PRESET>` | 3 summary formats: `concise` (default), `bullets`, `detailed`. |
| | `--max-tokens <NUM>` | Sets max completion tokens (style defaults: `concise` `1536`, `bullets` `2048`, `detailed` `8192`). |
| | `--context-window <NUM>` | Overrides auto-detected server context window in tokens (e.g. `32768`). |
| | `--open` (`-b`) | Opens the summary in a distraction-free HTML reader with light/dark mode. |
| | `--plain` | Outputs raw Markdown without terminal colors or automatic paging. |
| | `--paging <auto\|always\|never>` | Controls terminal output pager (`never` disables paging). |
| | `-o <FILE.html\|md>` | Saves the rendered output directly to an HTML or Markdown file. |
| | `--json` | Emits machine-readable JSON on `stdout` for script integration. |
| | `--stream` | Streams LLM output tokens live to `stdout` as they are generated. |
| | `--show-thinking` (`--thinking`) | Streams full raw model reasoning trace to `stderr` instead of compact `💭` timing lines. |
| **Interactive Tools** | `srz ask [--web\|--document\|--book\|--library] [Q]` | Grounded Q&A against a web URL, local document, book, saved summary, or latest read (alias `chat`). |
| | `srz ask -i` | Interactive multi-turn chat REPL grounded on source context. |
| | `srz ask [--chats\|--resume <ID>]` | Lists saved chat sessions or resumes a previous multi-turn conversation. |
| | `srz ask [--show-chat\|--delete-chat] <ID>` | Inspects or deletes a persisted multi-turn chat session transcript. |
| | `srz browse` (aliases `tui`, `library`) | Interactive TUI dashboard (`[j/k]` navigate, `[/]` filter, `[a]` ask/chat, `[y]` copy, `[d]` delete, `[Enter]` open). |
| **Library & History** | `srz find "keyword"` | Offline stop-word-filtered relevance search (10× title boost) across saved summaries in `srz.db`. |
| | `srz history [--limit N]` | Displays recent reads, word counts, model usage, and generation speeds. |
| | `srz stats` | Telemetry overview: reading streaks, days active, time saved, top domains, and model throughput percentiles (P50/P95). |
| | `srz index [--open]` | Generates and opens an interactive HTML library dashboard. |
| | `srz export -f <FORMAT> -o <PATH>` | Export summarized library to Obsidian/Logseq Markdown vault, JSON, or CSV. |
| **System & Config** | `--profile <NAME>` | Switches LLM backends defined in `~/.config/srz/config.toml`. |
| | `srz config <init\|show\|path>` | Manages and inspects the local configuration file. |
| | `srz cache <dir\|size\|clean\|prune>` | Cache inspection, TTL/LRU pruning (`--older-than`, `--max-size`), and safe clean (`--reset-db`). |
| | `srz models` | Lists available models reported by the local LLM backend. |
| | `srz recommend` | Recommends optimal model and context window for host hardware. |
| | `srz doctor` | Tests backend reachability, latency, cache health, and database integrity. |
| | `srz upgrade [--check\|--force]` | Safe, in-place binary self-updater with SHA-256 integrity verification. |
| | `srz completions <SHELL>` | Generates autocomplete scripts for bash, zsh, fish, powershell, and elvish. |

---

## 1. Ingestion Subcommands (`web`, `document`, `book`, `stdin`)

```bash
srz web <URL> [FLAGS]
srz document <PATH> [FLAGS]
srz book <TITLE> [--author <AUTHOR>] [FLAGS]
srz stdin [FLAGS]
```

### Explicit Intent, Scoped Format Detection

Instead of guessing intent from bare root strings, `srz` requires an explicit top-level subcommand (`web`, `document`, `book`, `news`, or `stdin`) and keeps format detection strictly inside the two container subcommands:

* **`srz web <URL>`**: Requires an `http://` or `https://` URL. Links pointing to `youtube.com` or `youtu.be` route directly to the YouTube timed-text caption and chapter client; URLs ending in `.pdf` download and extract binary PDF text; all other web links pass to the Mozilla Readability scraper (`dom_smoothie`) to strip navigation bars, advertisements, and boilerplate.
* **`srz document <PATH>`**: Reads local files (under a 20 MB ceiling, expanding leading `~`). Format is detected automatically from file extension **and** magic-byte signatures: PDFs (`%PDF-`), EPUB ebooks (ZIP + `application/epub+zip` / `META-INF/container.xml`), HTML (`<!doctype` / `<html` / `<body`), Markdown, JSON, and plain text.
* **`srz book <TITLE> [--author <AUTHOR>]`**: Queries Open Library, Wikipedia, and DuckDuckGo concurrently to fetch metadata and targeted plot/overview synopses, falling back to Open Library descriptions or LLM trained knowledge when needed.
* **`srz stdin`**: Reads piped text, logs, or diffs from standard input (`cat file.diff | srz stdin`, 10 MB ceiling), automatically normalizing HTML if detected.

### Global Flags

| Flag | Short | Description |
| :--- | :--- | :--- |
| `--style <STYLE>` | | Summary format preset: `concise` (default), `bullets`, `detailed`. |
| `--model <MODEL>` | | Override the configured LLM model name. |
| `--base-url <URL>` | | Local or remote OpenAI-compatible base URL (default: `http://localhost:9931/v1`). |
| `--api-key <KEY>` | | OpenAI API key (optional for local LLMs; prefers `OPENAI_API_KEY` or `config.toml`, and warns when passed via CLI). |
| `--profile <NAME>` | | Load a named configuration profile from `config.toml`. |
| `--config <PATH>` | | Custom configuration file path (defaults to `$SRZ_CONFIG` / `$SRZ_CONFIG_PATH` / `~/.config/srz/config.toml`). |
| `--context-window <N>` | | Override server context window size in tokens (must be `> 0`; clamped to `2,048`–`2,097,152`; auto-detected if omitted). |
| `--max-tokens <N>` | | Maximum tokens to generate (must be `> 0`; defaults to the active `--style` preset: `concise` `1536`, `bullets` `2048`, `detailed` `8192`). |
| `--open` | `-b` | Automatically open the rendered HTML summary in your default web browser. |
| `--output <FILE>` | `-o` | Save the generated output directly to a file (`.html` or `.md`). |
| `--plain` | | Output raw Markdown without terminal formatting, ANSI colors, or paging. |
| `--paging <MODE>` | | Terminal paging control: `auto` (default), `always`, `never`. |
| `--stream` | | Stream LLM output tokens live to `stdout` as they are generated. |
| `--show-thinking` | `--thinking` | Stream full raw model reasoning trace to `stderr` (default: compact timing summary like `💭 Thought for X.Xs`). |
| `--json` | | Emit machine-readable JSON output for scripts and automation. |
| `--no-cache` | | Bypass the local cache and force fresh scraping and synthesis. |
| `--quiet` | `-q` | Suppress status spinners and non-essential progress messages. |
| `--verbose` | `-v` | Display detailed diagnostic timings and network logs. |

---

## 2. Interactive Terminal Explorer (`srz browse`)

```bash
srz browse [FLAGS]
# Aliases: srz tui, srz library
```

Launches a lightweight interactive, full-screen `crossterm` terminal dashboard for exploring past reads and managing summaries.

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
| `Ctrl+U` | Clear the active search filter query (while focused or browsing) |
| `Ctrl+C` / `Ctrl+D` / `Ctrl+Q` | Exit immediately |

---

## 3. Grounded Q&A & Multi-Turn Chat (`srz ask`)

```bash
srz ask [--web <URL> | --document <PATH> | --book <TITLE> [--author <AUTHOR>] | --library <QUERY>] [QUESTION] [FLAGS]
# Alias: srz chat
```

Performs question-answering grounded directly on document context, protected with prompt injection quarantine tags (`<source_document>`).

* **Single-turn query against latest read**:
  ```bash
  srz ask "What are the main findings?"
  ```
* **Single-turn query against an explicit web page, YouTube video, local document, book, or saved summary**:
  ```bash
  srz ask --document assets/document.pdf "What are the primary conclusions?"
  srz ask --web "https://www.youtube.com/watch?v=5C_HPTJg5ek" "What are the key points in this talk?"
  srz ask --book "Dune" --author "Frank Herbert" "Who are the Fremen?"
  srz ask --library "rust history" "When did 1.0 ship?"
  ```
* **Interactive multi-turn chat REPL**:
  ```bash
  srz ask -i
  srz ask --document assets/document.pdf -i
  ```
  *Inside the REPL, ask questions naturally. Type `exit`, `quit`, or press `Ctrl+C` / `Ctrl+D` to end the session.*

### Session Management & Output Flags

* `--web <URL>`, `--document <PATH>`, `--book <TITLE>` (with optional `--author <AUTHOR>`), `--library <QUERY>`: Mutually exclusive source selectors (`--author` requires `--book`; cannot be combined with session management flags).
* `--chats`: List saved persistent chat sessions stored in local storage.
* `--resume <ID>`: Resume a previous conversation session by ID while preserving the original source `target_type` and `target_url` (emits a warning if source re-loading fails and falls back to the stored library summary).
* `--show-chat <ID>`: Print complete transcript of a past chat session.
* `--delete-chat <ID>`: Delete a saved chat session.
* Conflicting session management flags (`--chats`, `--resume`, `--show-chat`, `--delete-chat`) are rejected upfront, and `srz ask` also honors `--json` and `-o` / `--output`.

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
* **Summarize Direct RSS/Atom/JSON Feed**:
  ```bash
  srz news "https://blog.rust-lang.org/feed.xml"
  ```
* `-t, --timeframe <TIMEFRAME>`: Recency filter: `all` (default), `day` (past 24h), `week` (past 7 days), `month` (past 30 days).
* `-l, --limit <N>`: Maximum number of articles to aggregate (default: 5, max: 50).

---

## 5. Offline Library Search (`srz find`)

```bash
srz find <QUERY>
```

Executes an offline, stop-word-filtered relevance search across all saved summaries in `srz.db` without internet access (requires a non-empty query).

* **10× Title Weight Boost**: Matched query terms in document titles score `10.0` each compared to `1.0` for matches in summary bodies, breaking score ties by most recent entry ID.
* **ASCII Fast Path & Unicode Fallback**: Evaluates ASCII terms with zero-allocation case-insensitive scanning and falls back to UTF-8 safe lowercase matching for Unicode queries.
* Extracts contextual preview snippets centered around matched query terms. (For grounded document Q&A, `srz ask` additionally ranks document chunks using Robertson-Sparck Jones BM25 with $k_1 = 1.2, b = 0.75$.)

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

## 7. Library Vault & Data Export (`srz export`)

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

---

## 8. Storage & Cache Maintenance (`srz cache`)

```bash
srz cache <SUBCOMMAND>
```

* `srz cache dir`: Display the absolute path to the local cache directory (`~/.cache/srz`).
* `srz cache size`: Show disk usage broken down by pages, search metadata, summaries, and views.
* `srz cache prune`: Evict expired items based on age or disk size budget (running with no flags enforces the configured `max_size_mb` cap):
  ```bash
  srz cache prune
  srz cache prune --older-than 14d
  srz cache prune --max-size 500MB
  ```
* `srz cache clean` (aliases: `clear`, `purge`, `wipe`): Clear cached pages, search metadata, server capabilities, and summary cache lookup mappings from `srz.db`, plus rendered files in `views/`. Library history and chat sessions are preserved unless `--reset-db` is passed.
  * `--reset-db`: Deletes and recreates the `srz.db` database entirely.

---

## 9. Configuration Management (`srz config`)

```bash
srz config <SUBCOMMAND>
```

* `srz config path`: Display path to `config.toml` and report whether it exists.
* `srz config show`: Print currently active configuration settings.
* `srz config init`: Atomically generate a `0600` starter `~/.config/srz/config.toml` tailored to your host RAM, CPU, and recommended context window (refuses to overwrite existing files).

---

## 10. Diagnostics & Recommendations (`srz recommend`, `srz doctor`, `srz models`)

```bash
# Instant hardware & model recommendation card
srz recommend
srz recommend --json

# Comprehensive diagnostic audit (hardware, LLM latency, n_ctx, database, cache, pager)
srz doctor

# List available models reported by the connected backend
srz models
```

### Hardware Profiling & Diagnostic Advisories

Running `srz recommend` or `srz doctor` profiles your physical machine without invasive C FFI bindings. By inspecting host system characteristics via native kernel and procfs hardware profiling, `srz` classifies your system into hardware tiers and suggests optimal model configurations:

The advisor recommends high-fidelity Google Gemma 4 models tailored to your physical RAM, matching the exact selection cards shown in Llama GUI (such as `gemma-4` with size `E4B` or `26B-A4B` at `Q8_0` quantization) while configuring the full model ID (`ggml-org/gemma-4-E4B-it-GGUF:Q8_0`) for seamless backend API communication. It estimates an advisory single-pass token ceiling (`70%` of the recommended context window), while at runtime `DynamicBudget` calculates the exact threshold after reserving completion tokens, a 500-token system margin, and a 5% cushion—beyond which CPU heading-skeleton and centroid BM25 salience compression activates.

When running `srz doctor`, the engine performs an active end-to-end audit: it pings the local LLM endpoint, probes operational context lengths (`n_ctx`), and verifies the integrity of local storage and cache directories. If the connected runner is constrained by sub-optimal context sizing (under 8k tokens), `srz doctor` provides concrete terminal guidance to unlock maximum throughput.

For the complete tier-by-tier hardware breakdown, refer to the canonical [Hardware Tier Reference Matrix](quickstart.md#hardware-tier-reference-matrix).

---

## 11. Self Updates & Shell Completions

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

# Elvish (add to ~/.config/elvish/rc.elv):
eval (srz completions elvish | slurp)
```
