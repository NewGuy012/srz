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
    pub context_window: Option<usize>,
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

    /// Generates a starter `config.toml` template string tailored for the detected hardware.
    pub fn template_for_hardware(hw: &crate::system::hardware::HardwareProfile) -> String {
        format!(
            r#"# srz (Summarize) configuration file
# Default path: ~/.config/srz/config.toml
# Tailored for host: {} ({:.1} GB RAM / {})
# Recommended local setup: {}

# Default local LLM endpoint (Llama-macOS, Llama-Windows, llama.cpp, Ollama)
# Connects automatically with Llama GUI running on localhost:9931
base_url = "http://localhost:9931/v1"
model = "{}"
context_window = {}
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
model = "{}"
style = "concise"
context_window = 8192

# Deep research / book mode: Full synthesis with maximum context window
[profiles.deep]
base_url = "http://localhost:9931/v1"
model = "{}"
style = "detailed"
context_window = {}
max_tokens = 8192
"#,
            hw.chip_name,
            hw.total_memory_gb,
            hw.tier.display_name(),
            hw.llama_gui_selection(),
            hw.recommended_model,
            hw.recommended_context,
            hw.recommended_model,
            hw.recommended_model,
            hw.recommended_context,
        )
    }

    /// Generates a starter `config.toml` template string using detected host hardware.
    pub fn default_template() -> String {
        Self::template_for_hardware(&crate::system::hardware::HardwareProfile::detect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_template_is_valid_toml() {
        let template = Config::default_template();
        let cfg: Config =
            toml::from_str(&template).expect("default template must parse as valid TOML");
        assert_eq!(cfg.base_url.as_deref(), Some("http://localhost:9931/v1"));
        assert!(cfg.model.is_some());
        assert!(cfg.context_window.is_some());
        assert_eq!(cfg.style, Some(SummaryStyle::Concise));
        assert_eq!(cfg.max_tokens, Some(8192));
        assert!(cfg.profiles.contains_key("openai"));
        assert!(cfg.profiles.contains_key("fast"));
        assert!(cfg.profiles.contains_key("deep"));
    }

    #[test]
    fn test_load_from_path_lifecycle() {
        let tmp = tempfile::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");

        // Missing file returns error
        assert!(Config::load_from_path(&config_path).is_err());

        // Valid file loads correctly
        let valid_toml = r#"
            base_url = "http://localhost:11434/v1"
            model = "llama3:8b"
        "#;
        fs::write(&config_path, valid_toml).unwrap();
        let loaded = Config::load_from_path(&config_path).expect("Should load valid config");
        assert_eq!(
            loaded.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(loaded.model.as_deref(), Some("llama3:8b"));

        // Malformed TOML returns error
        fs::write(&config_path, "not a [valid toml").unwrap();
        assert!(Config::load_from_path(&config_path).is_err());
    }
}
