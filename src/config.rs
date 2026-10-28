use crate::cli::SummaryStyle;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProfileConfig {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub style: Option<SummaryStyle>,
    pub max_tokens: Option<u32>,
    pub no_search: Option<bool>,
    pub no_cache: Option<bool>,
    pub quiet: Option<bool>,
    pub verbose: Option<bool>,
    pub open: Option<bool>,
    pub concurrency: Option<usize>,
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub api_key: Option<String>,
    pub style: Option<SummaryStyle>,
    pub max_tokens: Option<u32>,
    pub no_search: Option<bool>,
    pub no_cache: Option<bool>,
    pub quiet: Option<bool>,
    pub verbose: Option<bool>,
    pub open: Option<bool>,
    pub concurrency: Option<usize>,
    pub stream: Option<bool>,
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
}

impl Config {
    /// Returns the default path for `config.toml` (`$SRZ_CONFIG`, `$XDG_CONFIG_HOME/srz/config.toml`, or `~/.config/srz/config.toml`).
    pub fn default_config_path() -> PathBuf {
        if let Some(env_path) = crate::cache::non_empty_env_path("SRZ_CONFIG") {
            return env_path;
        }
        if let Some(xdg) = crate::cache::non_empty_env_path("XDG_CONFIG_HOME") {
            return xdg.join("srz").join("config.toml");
        }
        if let Some(home) = crate::cache::non_empty_env_path("HOME") {
            return home.join(".config").join("srz").join("config.toml");
        }
        env::temp_dir().join("srz").join("config.toml")
    }

    /// Loads the configuration file from a specific path.
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let content = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file at {:?}", path))?;
        let config: Config = toml::from_str(&content)
            .with_context(|| format!("Failed to parse TOML configuration from {:?}", path))?;
        Ok(config)
    }

    /// Loads default configuration if the default config file exists.
    pub fn load_default() -> (Self, Option<PathBuf>) {
        let path = Self::default_config_path();
        if path.exists() {
            match Self::load_from_path(&path) {
                Ok(cfg) => (cfg, Some(path)),
                Err(_) => (Self::default(), None),
            }
        } else {
            (Self::default(), None)
        }
    }

    /// Generates a starter `config.toml` template string.
    pub fn default_template() -> &'static str {
        r#"# srz (Summarize) configuration file
# Default path: ~/.config/srz/config.toml

# Default local LLM endpoint (e.g. llama.cpp, Ollama, LM Studio, vLLM)
# base_url = "http://localhost:9931/v1"
# model = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
# api_key = ""

# Default summary style (concise, detailed, bullets, executive, tldr, critique, eli5, flashcards)
# style = "concise"

# Maximum completion tokens (thinking + summary tokens)
# max_tokens = 8192

# Auto-open generated HTML view in browser
# open = false

# Parallel chunk concurrency for map-reduce
# concurrency = 1

# Stream tokens live to stdout (default: false)
# stream = false

# Named profiles (switch with `srz --profile <name> ...`)
# [profiles.fast]
# base_url = "http://localhost:11434/v1"
# model = "qwen2.5:3b"
# style = "concise"

# [profiles.deep]
# base_url = "http://localhost:11434/v1"
# model = "deepseek-r1:14b"
# style = "detailed"
# max_tokens = 16384
"#
    }
}
