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
    pub no_cache: Option<bool>,
    pub quiet: Option<bool>,
    pub verbose: Option<bool>,
    pub open: Option<bool>,
    pub concurrency: Option<usize>,
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CacheConfig {
    pub max_size_mb: Option<u64>,
    pub pages_ttl_days: Option<u64>,
    pub search_ttl_days: Option<u64>,
    pub summaries_ttl_days: Option<u64>,
    pub views_ttl_days: Option<u64>,
    pub auto_prune: Option<bool>,
}

impl CacheConfig {
    pub fn to_policy(&self) -> crate::cache::CachePolicy {
        let mut policy = crate::cache::CachePolicy::default();
        if let Some(mb) = self.max_size_mb {
            policy.max_bytes = mb.saturating_mul(1024).saturating_mul(1024);
        }
        if let Some(days) = self.pages_ttl_days {
            policy.pages_ttl = std::time::Duration::from_secs(days.saturating_mul(86400));
        }
        if let Some(days) = self.search_ttl_days {
            policy.search_ttl = std::time::Duration::from_secs(days.saturating_mul(86400));
        }
        if let Some(days) = self.summaries_ttl_days {
            policy.summaries_ttl = std::time::Duration::from_secs(days.saturating_mul(86400));
        }
        if let Some(days) = self.views_ttl_days {
            policy.views_ttl = std::time::Duration::from_secs(days.saturating_mul(86400));
        }
        if let Some(false) = self.auto_prune {
            policy.auto_prune_interval = std::time::Duration::MAX;
        }
        policy
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(flatten)]
    pub settings: ProfileConfig,
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
}

impl std::ops::Deref for Config {
    type Target = ProfileConfig;

    fn deref(&self) -> &Self::Target {
        &self.settings
    }
}

impl std::ops::DerefMut for Config {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.settings
    }
}

impl Config {
    /// Returns the default path for `config.toml` (`$SRZ_CONFIG`, `$XDG_CONFIG_HOME/srz/config.toml`, or `~/.config/srz/config.toml`).
    pub fn default_config_path() -> PathBuf {
        if let Some(env_path) = crate::fs_util::non_empty_env_path("SRZ_CONFIG") {
            return env_path;
        }
        if let Some(xdg) = crate::fs_util::non_empty_env_path("XDG_CONFIG_HOME") {
            return xdg.join("srz").join("config.toml");
        }
        if let Some(home) = crate::fs_util::non_empty_env_path("HOME") {
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

# Default summary style (concise, detailed, bullets)
# style = "concise"

# Maximum completion tokens (thinking + summary tokens)
# max_tokens = 8192

# Auto-open generated HTML view in browser
# open = false

# Parallel chunk concurrency for map-reduce
# concurrency = 1

# Stream tokens live to stdout (default: false)
# stream = false

# Cache retention policy and storage budget
# [cache]
# max_size_mb = 500
# pages_ttl_days = 7
# search_ttl_days = 14
# summaries_ttl_days = 90
# views_ttl_days = 14
# auto_prune = true

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
