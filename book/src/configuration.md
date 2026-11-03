# Configuration

`srz` reads its persistent settings from `~/.config/srz/config.toml` (or `$XDG_CONFIG_HOME/srz/config.toml` on Linux, `%APPDATA%\srz\config.toml` / `$env:APPDATA\srz\config.toml` on Windows).

You can generate a starter configuration file tailored to your machine's hardware at any time using:

```bash
srz config init
```

> 💡 **Tip**: When run, `srz config init` automatically detects your physical RAM and chip architecture, setting the recommended `model` and `context_window` out of the box. You can also run `srz recommend` at any time to inspect your hardware specs and launch commands.

---

## Example Hardware-Tailored `config.toml`

```toml
# srz (Summarize) configuration file
# Default path: ~/.config/srz/config.toml
# Tailored for host: Apple M-Series (24.0 GB RAM / 24–36 GB Tier)

# Default local LLM endpoint (e.g. Llama-macOS, Llama-Windows, llama.cpp, Ollama)
# Connects automatically with Llama GUI running on localhost:9931
base_url = "http://localhost:9931/v1"
model = "gemma-4"
context_window = 65536
api_key = ""

# Default summary style: concise (default), detailed, bullets
style = "concise"

# Maximum completion tokens (thinking + summary tokens)
max_tokens = 8192

# Auto-open generated HTML view in your browser (default: false)
open = false

# Parallel chunk concurrency for map-reduce
concurrency = 1

# Stream tokens live to stdout (default: false)
stream = false

# Storage budget and retention policies
[cache]
max_size_mb = 500         # Disk budget (LRU eviction past threshold)
pages_ttl_days = 7        # Scraped web content & news TTL
search_ttl_days = 14      # Book metadata & search candidates TTL
summaries_ttl_days = 90   # Generated LLM outputs TTL
views_ttl_days = 14       # Rendered HTML/MD reader views TTL
auto_prune = true         # Opportunistic background cleanup

# Cloud profile (e.g. OpenAI or Anthropic)
[profiles.openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
api_key = "sk-..."

# Quick mode: Fast concise summaries for news and short articles
[profiles.fast]
base_url = "http://localhost:9931/v1"
model = "gemma-4"
style = "concise"
context_window = 8192

# Deep research / book mode: Full synthesis with maximum context window
[profiles.deep]
base_url = "http://localhost:9931/v1"
model = "gemma-4"
style = "detailed"
context_window = 65536
max_tokens = 8192

# Scripting profile with spinners disabled and cache bypassed
[profiles.quiet]
base_url = "http://localhost:9931/v1"
quiet = true
no_cache = true
```

---

## Configuration Profiles

Named profiles allow you to switch between multiple models, inference endpoints, and summary styles seamlessly without altering your default settings:

```bash
# Use default local backend
srz "The Rust Programming Language"

# Use the cloud profile defined under [profiles.openai]
srz "The Rust Programming Language" --profile openai

# Use the fast profile defined under [profiles.fast]
srz "https://news.ycombinator.com" --profile fast
```

---

## Precedence & Resolution Order

When resolving runtime settings, `srz` applies the following priority (highest to lowest):

1. **Explicit CLI flags** (e.g. `--model`, `--base-url`, `--style`, `--stream`, `--context-window`).
2. **Profile settings** selected via `--profile <NAME>` in `config.toml`.
3. **Environment variables** (`OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_API_KEY`).
4. **Root settings** defined in `config.toml`.
5. **Hardcoded engine defaults** (`http://localhost:9931/v1`, `concise`, 500 MB cache).

---

## Cache & Storage Settings

The `[cache]` table controls how `srz` manages disk storage under `~/.cache/srz/`:

| Setting | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `max_size_mb` | integer | `500` | Total storage budget in megabytes across `cache.duckdb` and `views/`. LRU eviction triggers when exceeded. |
| `pages_ttl_days` | integer | `7` | Retention for scraped DOM and web article text (YouTube and PDFs retain for 30 days). |
| `search_ttl_days` | integer | `14` | Retention for Open Library book metadata and search query candidates. |
| `summaries_ttl_days` | integer | `90` | Retention for generated LLM summary outputs (`(target, model, style)`). |
| `views_ttl_days` | integer | `14` | Retention for rendered standalone HTML and Markdown view files in `views/`. |
| `auto_prune` | boolean | `true` | Enables opportunistic background cache cleanup (throttled to once every 24 hours via `~/.cache/srz/.last_prune`). |

> 🔒 **Privacy & Isolation**: All cache databases and view files are created with owner-only Unix permissions (`0700` directories, `0600` files). To completely bypass caching for a single command, pass `--no-cache`.

---

## Configuration Commands

```bash
# Display path to the configuration file and its existence
srz config path

# View all active resolved configuration settings
srz config show

# Generate a default ~/.config/srz/config.toml starter template
srz config init
```
