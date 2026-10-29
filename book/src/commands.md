# Command Reference

`srz` provides a unified CLI interface where root targets are automatically routed based on input structure, accompanied by specialized subcommands.

---

## 1. Root Target Dispatch (`srz <TARGET>`)

```bash
srz <TARGET> [FLAGS]
```

### Auto-Detection Rules:
* If `<TARGET>` starts with `http://` or `https://`:
  * If the domain is `youtube.com` or `youtu.be`, routes to **YouTube Timed-Text Ingestion**.
  * Otherwise, routes to **Web Scraper**.
* If `<TARGET>` ends with `.pdf`: Routes to **In-Memory PDF Extractor**.
* If `<TARGET>` is `-`: Routes to **Stdin Stream Reader**.
* If `<TARGET>` exists on the filesystem: Routes to **Local File Reader**.
* Otherwise: Routes to **Book Metadata Resolver**.

### Key Global Flags:
* `-s, --style <STYLE>`: Summary style preset (`concise` [default], `detailed`, `tldr`, `critique`, `eli5`, `flashcards`).
* `-m, --model <MODEL>`: Override the configured LLM model name.
* `--plain`: Disable ANSI colors, bold styling, and terminal markdown formatting.
* `--open`: Open the generated HTML view in your default web browser upon completion.
* `--json`: Output structured JSON containing title, metadata, word count, and summary.
* `-v, --verbose`: Display verbose debugging logs.
* `-q, --quiet`: Suppress spinners and non-essential progress output.

---

## 2. Interactive Terminal Explorer (`srz browse`)

```bash
srz browse [FLAGS]
```
Launches a full-screen Ratatui terminal dashboard.

### Keybindings:
* `j` / `Down` / `k` / `Up`: Navigate library items.
* `Enter`: Open the reader view for the selected summary.
* `/`: Focus live search input to filter items.
* `Tab`: Switch filter tab (`All`, `Web`, `PDF`, `Books`, `News`, `Chat`).
* `y` / `c`: Copy summary text to system clipboard.
* `d`: Delete selected item from library history.
* `Esc` / `q`: Exit reader view or close application.

---

## 3. Grounded Q&A & Multi-Turn Chat (`srz ask`)

```bash
srz ask [TARGET] [QUESTION] [FLAGS]
```

* **Single-turn query**: `srz ask document.pdf "Summarize section 3"`
* **Interactive chat**: `srz ask document.pdf`

### Session Management Flags:
* `--chats`: List past persistent chat sessions stored in DuckDB.
* `--resume <ID>`: Resume a previous conversation by session ID.
* `--show-chat <ID>`: Print complete transcript of a past chat session.
* `--delete-chat <ID>`: Delete a chat session.

---

## 4. Offline BM25 Search (`srz find`)

```bash
srz find <QUERY> [FLAGS]
```
Executes an offline full-text search across your saved library using DuckDB's inverted term index table.
* Ranks results with BM25 scoring ($k_1=1.2, b=0.75$).
* Features $2.5\times$ title weighting and query-focused snippet extraction.
* `--limit <N>`: Maximum number of search results (default: 10).

---

## 5. Reading Habit & Model Telemetry (`srz stats`)

```bash
srz stats
```
Displays aggregated reading metrics and inference performance:
* Active reading days and current reading streak.
* Estimated reading time saved (based on 225 words per minute).
* Top source domains.
* Model token generation throughput percentiles (P50 and P95 tokens/sec).

---

## 6. Cache Management (`srz cache`)

```bash
srz cache <SUBCOMMAND>
```
* `srz cache`: Display cache statistics (pages, metadata, summaries, view files, total size).
* `srz cache clean`: Clear all ephemeral cache entries.
* `srz cache prune`: Evict expired items according to TTL and LRU budget.
  * `--older-than <DURATION>`: Evict entries older than duration (e.g. `7d`, `24h`).
  * `--max-size <BYTES>`: Prune least-recently-used items until total cache is under budget (e.g. `500MB`, `1GB`).

---

## 7. Diagnostics (`srz doctor`)

```bash
srz doctor
```
Verifies environment health, checking:
* Local LLM endpoint connectivity and active models.
* DuckDB library database integrity (`srz.duckdb`).
* Cache database accessibility (`cache.duckdb`).
* System clipboard accessibility (`arboard`).
* System pager configuration (`$PAGER`, `less`).

---

## 8. Shell Completions (`srz completions`)

```bash
srz completions <SHELL>
```
Generates completion scripts for `bash`, `zsh`, or `fish`.
```bash
# Add to ~/.zshrc:
eval "$(srz completions zsh)"
```
