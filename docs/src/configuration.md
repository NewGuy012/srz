# Configuration

`srz` resolves its configuration file from `$SRZ_CONFIG`, then `$SRZ_CONFIG_PATH`, then `$XDG_CONFIG_HOME/srz/config.toml`, falling back to `~/.config/srz/config.toml` (or `%LOCALAPPDATA%` / `%APPDATA%` equivalents via home directories on Windows).

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
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
context_window = 65536
api_key = ""

# Default summary style: concise (default), detailed, bullets
style = "concise"

# Maximum completion tokens (thinking + summary tokens)
max_tokens = 8192

# Auto-open generated HTML view in your browser (default: false)
open = false

# Stream tokens live to stdout (default: false)
stream = false

# Stream full raw model reasoning trace to stderr (default: false)
# show_thinking = false

# Storage budget and retention policies
[cache]
max_size_mb = 500         # Disk budget (LRU eviction past threshold)
pages_ttl_days = 7        # Scraped web content & news TTL
search_ttl_days = 14      # Book metadata & search candidates TTL
summaries_ttl_days = 90   # Generated LLM outputs TTL
views_ttl_days = 14       # Rendered HTML/MD reader views TTL
auto_prune = true         # Opportunistic background cleanup
# news_ttl_hours = 2      # Optional override for news/RSS TTL (default: 2 hours)

# Cloud profile (e.g. OpenAI or Anthropic)
[profiles.openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
api_key = "sk-..."

# Quick mode: Fast concise summaries for news and short articles
[profiles.fast]
base_url = "http://localhost:9931/v1"
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
style = "concise"
context_window = 8192

# Deep research / book mode: Full synthesis with maximum context window
[profiles.deep]
base_url = "http://localhost:9931/v1"
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
style = "detailed"
context_window = 65536
max_tokens = 8192

# Optional custom scripting profile (user-defined example, not emitted by `srz config init`)
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

Runtime configuration follows a predictable layered resolution model (`Cli::apply_config`). Explicit command-line arguments and environment variables supplied to `clap` take top priority, ahead of `config.toml`. Within `config.toml`, named profile settings (`--profile <NAME>`) override root-level settings. If no configuration is provided at any layer, `srz` falls back to style-aware limits and safe local-first engine defaults:

1. **Explicit CLI flags** (e.g. `--model`, `--base-url`, `--style`, `--stream`, `--show-thinking`, `--context-window`).
2. **Environment variables** (`OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_API_KEY`).
3. **Profile settings** selected via `--profile <NAME>` in `config.toml`.
4. **Root settings** defined in `config.toml`.
5. **Style-aware & hardcoded defaults** (`http://localhost:9931/v1`, `concise`, style-based `max_tokens`, 500 MB cache).

---

## Cache & Storage Settings

The `[cache]` table controls how `srz` manages disk storage across `srz.db` and `~/.cache/srz/`:

| Setting | Type | Default | Description |
| :--- | :--- | :--- | :--- |
| `max_size_mb` | integer | `500` | Total storage budget in megabytes across `srz.db` and `views/`. LRU eviction triggers when exceeded. |
| `pages_ttl_days` | integer | `7` | Retention for cached web pages, YouTube transcripts, and PDF text in `srz.db`. |
| `news_ttl_hours` | integer | `2` | Short-lived retention for news articles and syndication feeds to prevent stale briefings. |
| `search_ttl_days` | integer | `14` | Retention for Open Library book metadata and search query candidates. |
| `summaries_ttl_days` | integer | `90` | Cache lookup retention window for generated summaries in `srz.db` (`(target, model, style)`). |
| `views_ttl_days` | integer | `14` | Retention for opt-in standalone HTML and Markdown view files in `views/` (generated via `--open`). |
| `auto_prune` | boolean | `true` | Enables opportunistic background cache cleanup (throttled to once every 24 hours via `~/.cache/srz/.last_prune`). |

> 🔒 **Privacy & Isolation**: All databases and view files are created with owner-only Unix permissions (`0700` directories, `0600` files), and `config.toml` rejects symlinks and oversized files (`CWE-59`). To completely bypass caching for a single command, pass `--no-cache`. View file export (`.html`/`.md`) is strictly opt-in via `--open` or explicit output flags (`--output`).

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
