# Configuration

`srz` reads its configuration from `~/.config/srz/config.toml` (or `%APPDATA%\srz\config.toml` on Windows).

If no configuration file exists, `srz` creates a default one automatically on first run.

---

## Example `config.toml`

```toml
# General settings
default_style = "concise"
auto_open = false
stream = true
plain = false

# Default LLM Endpoint (e.g. Ollama, llama.cpp, LM Studio)
[llm]
base_url = "http://localhost:11434/v1"
model = "mistral"
api_key = "optional-key"
timeout_secs = 60
context_window = 8192

# Cache Retention Policy
[cache]
enabled = true
pages_ttl_days = 7
search_ttl_days = 14
summaries_ttl_days = 30
max_size_mb = 1024
auto_prune_interval_days = 1

# Custom Profiles
[profiles.fast]
llm = { base_url = "http://localhost:11434/v1", model = "gemma:2b" }
default_style = "tldr"

[profiles.deep]
llm = { base_url = "http://localhost:8080/v1", model = "qwen2.5:14b", context_window = 16384 }
default_style = "detailed"
```

---

## Configuration Profiles

You can select a profile on any command using the `--profile` flag:

```bash
srz https://example.com/article --profile fast
srz paper.pdf --profile deep
```

### Profile Resolution Order:
1. Explicit CLI arguments (e.g. `--model`, `--style`).
2. Specified profile in `config.toml` (`--profile <NAME>`).
3. Root defaults in `config.toml`.
4. Hardcoded engine defaults.
