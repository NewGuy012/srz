# Configuration

`srz` reads its persistent settings from `~/.config/srz/config.toml` (or `$XDG_CONFIG_HOME/srz/config.toml` on Linux, `%APPDATA%\srz\config.toml` on Windows).

You can generate a starter configuration file at any time using:

```bash
srz config init
```

---

## Example `config.toml`

```toml
# srz (Summarize) configuration file
# Default path: ~/.config/srz/config.toml

# Default local LLM endpoint (e.g. Llama-macOS, Llama-Windows, llama.cpp, Ollama)
base_url = "http://localhost:9931/v1"
model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
api_key = ""

# Default summary style: concise (default), detailed, bullets
style = "concise"

# Maximum completion tokens (thinking + summary tokens)
max_tokens = 8192

# Auto-open generated HTML view in your browser (default: false)
open = false

# Stream tokens live to stdout (default: false)
stream = false

# Concurrency limit for parallel map-reduce document chunking
concurrency = 1

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

# Local fast model profile (e.g. Ollama)
[profiles.fast]
base_url = "http://localhost:11434/v1"
model = "qwen2.5:3b"
style = "concise"

# Local deep reasoning profile
[profiles.deep]
base_url = "http://localhost:11434/v1"
model = "deepseek-r1:14b"
style = "detailed"
max_tokens = 16384
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

1. **Explicit CLI flags** (e.g. `--model`, `--base-url`, `--style`, `--stream`).
2. **Profile settings** selected via `--profile <NAME>` in `config.toml`.
3. **Environment variables** (`OPENAI_BASE_URL`, `OPENAI_MODEL`, `OPENAI_API_KEY`).
4. **Root settings** defined in `config.toml`.
5. **Hardcoded engine defaults** (`http://localhost:9931/v1`, `concise`, 500 MB cache).

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
