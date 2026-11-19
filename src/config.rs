use anyhow::{Context, Result};
use clap::{Parser, Subcommand, ValueEnum};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

pub const SECS_PER_DAY: u64 = 86_400;
const DEFAULT_NEWS_LIMIT: usize = 5;
const MAX_CONFIG_BYTES: u64 = 1024 * 1024;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SummaryStyle {
    /// Concise summary with key takeaways
    Concise,
    /// Detailed comprehensive summary
    Detailed,
    /// Bulleted key points
    Bullets,
}

impl SummaryStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Concise => "concise",
            Self::Detailed => "detailed",
            Self::Bullets => "bullets",
        }
    }

    /// Default completion token limit suited for each style when not explicitly overridden.
    pub fn default_max_tokens(&self) -> u32 {
        match self {
            Self::Concise => 1536,
            Self::Bullets => 2048,
            Self::Detailed => 8192,
        }
    }

    pub fn prompt_guidance(&self) -> &'static str {
        match self {
            Self::Concise => {
                "Provide a concise, high-level summary capturing the core message in 2-3 brief paragraphs."
            }
            Self::Detailed => {
                "Provide a detailed, structured summary covering all major sections, themes, arguments, and conclusions."
            }
            Self::Bullets => {
                "Provide a structured summary formatted as clear, concise bullet points highlighting key insights."
            }
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum NewsTimeframe {
    /// Past 24 hours / current day
    Day,
    /// Past 7 days / week
    Week,
    /// Past 30 days / month
    Month,
    /// Any time / all results (recency-ranked)
    #[default]
    All,
}

impl NewsTimeframe {
    pub fn as_google_query_suffix(&self) -> &'static str {
        match self {
            Self::Day => "when:1d",
            Self::Week => "when:7d",
            Self::Month => "when:30d",
            Self::All => "",
        }
    }

    pub fn display_label(&self) -> &'static str {
        match self {
            Self::Day => "today / past 24 hours",
            Self::Week => "past week",
            Self::Month => "past month",
            Self::All => "all time",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum PagingMode {
    /// Automatically page output when it exceeds terminal height
    #[default]
    Auto,
    /// Always page output through a terminal pager
    Always,
    /// Never page output
    Never,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ExportFormat {
    /// Markdown vault with Obsidian/Logseq frontmatter
    #[default]
    Markdown,
    /// Structured JSON file
    Json,
    /// Comma-separated values (CSV)
    Csv,
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    /// Bourne Again SHell (bash)
    Bash,
    /// Elvish shell
    Elvish,
    /// Friendly Interactive SHell (fish)
    Fish,
    /// PowerShell
    #[value(name = "powershell")]
    PowerShell,
    /// Z SHell (zsh)
    Zsh,
}

#[inline]
fn redact_secret(secret: &str) -> &'static str {
    if secret.trim().is_empty() {
        ""
    } else {
        "[REDACTED]"
    }
}

#[derive(Parser)]
#[command(
    name = "srz",
    version,
    about = "Modular CLI to summarize URLs, documents, books, YouTube, news, and Q&A using local LLMs",
    long_about = "A fast, modular CLI that extracts and summarizes web pages, YouTube transcripts, \
                  local PDF/EPUB/text files, stdin, books, and news feeds via OpenAI-compatible \
                  backends (llama.cpp, Ollama, LM Studio, vLLM, or cloud endpoints)."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Auto-detected target (URL, local file, news query, book title, or '-' for stdin)
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,

    /// Local OpenAI-compatible base URL
    #[arg(long, global = true, default_value = "http://localhost:9931/v1")]
    pub base_url: String,

    /// Model ID matching your backend (e.g. llama-server or Ollama)
    #[arg(
        long,
        global = true,
        default_value = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
    )]
    pub model: String,

    /// OpenAI API key (prefer OPENAI_API_KEY env var or config.toml to avoid shell history exposure)
    #[arg(long, global = true, default_value = "")]
    pub api_key: String,

    /// Output summary style
    #[arg(long, global = true, value_enum, default_value = "concise")]
    pub style: SummaryStyle,

    /// Maximum tokens to generate (defaults to the active `--style` preset)
    #[arg(long, global = true)]
    pub max_tokens: Option<u32>,

    /// Override context window size in tokens (auto-detected from server by default)
    #[arg(long = "context-window", global = true)]
    pub context_window: Option<usize>,

    /// Bypass local cache and force fresh web scraping and LLM summarization
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Path to optional config file (defaults to platform srz config.toml)
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    /// Named configuration profile from config.toml to use
    #[arg(long, global = true)]
    pub profile: Option<String>,

    /// Save the summary to an output file
    #[arg(short, long, global = true)]
    pub output: Option<PathBuf>,

    /// Suppress status messages, spinners, and diagnostics
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Display verbose diagnostic and timing logs
    #[arg(short, long, global = true)]
    pub verbose: bool,

    /// Format output as machine-readable JSON
    #[arg(long, global = true)]
    pub json: bool,

    /// Automatically open the styled HTML summary in the default web browser
    #[arg(short = 'b', long, global = true)]
    pub open: bool,

    /// Stream LLM output tokens live to stdout as they are generated
    #[arg(long, global = true)]
    pub stream: bool,

    /// Stream full raw model reasoning trace to stderr
    #[arg(long = "show-thinking", alias = "thinking", global = true)]
    pub show_thinking: bool,

    /// Control terminal paging of summary output
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub paging: PagingMode,

    /// Output raw, unstyled Markdown without terminal formatting or ANSI colors
    #[arg(long, global = true)]
    pub plain: bool,
}

impl fmt::Debug for Cli {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cli")
            .field("command", &self.command)
            .field("target", &self.target)
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &redact_secret(&self.api_key))
            .field("style", &self.style)
            .field("max_tokens", &self.max_tokens)
            .field("context_window", &self.context_window)
            .field("no_cache", &self.no_cache)
            .field("config", &self.config)
            .field("profile", &self.profile)
            .field("output", &self.output)
            .field("quiet", &self.quiet)
            .field("verbose", &self.verbose)
            .field("json", &self.json)
            .field("open", &self.open)
            .field("stream", &self.stream)
            .field("show_thinking", &self.show_thinking)
            .field("paging", &self.paging)
            .field("plain", &self.plain)
            .finish()
    }
}

#[derive(Subcommand, Debug, PartialEq, Eq)]
pub enum Commands {
    /// Manage the local disk cache (dir, size, clean, prune)
    Cache {
        #[command(subcommand)]
        action: CacheCommand,
    },
    /// Manage srz configuration (path, show, init)
    Config {
        #[command(subcommand)]
        action: ConfigSubcommand,
    },
    /// View recent summarization history and metrics
    History {
        /// Maximum entries to display
        #[arg(short, long, default_value_t = 10)]
        limit: usize,
    },
    /// Display aggregate library and LLM performance statistics
    Stats,
    /// Search your offline summarized library for keywords or phrases
    Find {
        /// Keyword or query to search for across titles and summaries
        query: String,
    },
    /// List available models from the local LLM backend
    Models,
    /// Diagnose system health, LLM connectivity, database, and cache
    Doctor,
    /// Recommend optimal model, context window, and ready launch commands for your machine
    Recommend,
    /// Generate and view the standalone HTML library dashboard
    Index,
    /// Launch the interactive terminal UI library explorer
    #[command(alias = "tui", alias = "library")]
    Browse,
    /// Export summarized library to Markdown, JSON, or CSV
    Export {
        /// Target export format
        #[arg(short, long, value_enum, default_value = "markdown")]
        format: ExportFormat,

        /// Output destination path (directory for markdown vault; file for json/csv)
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Ask questions grounded on a document, URL, file, or the most recent read
    #[command(alias = "chat")]
    Ask {
        /// Target document (URL/file/title) or question
        first: Option<String>,

        /// Question to ask (when target is provided as the first argument)
        second: Option<String>,

        /// Run in interactive multi-turn chat mode
        #[arg(short, long)]
        interactive: bool,

        /// List saved Q&A chat sessions
        #[arg(long)]
        chats: bool,

        /// Resume an existing chat session by its session ID
        #[arg(long)]
        resume: Option<i64>,

        /// Print the complete transcript of a past chat session
        #[arg(long)]
        show_chat: Option<i64>,

        /// Delete a saved chat session by its session ID
        #[arg(long)]
        delete_chat: Option<i64>,
    },
    /// Search and summarize latest news on a topic using RSS/Atom/JSON feeds
    News {
        /// Topic, search phrase, or direct feed URL
        topic: String,
        /// Timeframe for news recency
        #[arg(short = 't', long, value_enum, default_value = "all")]
        timeframe: NewsTimeframe,
        /// Maximum number of top articles to aggregate and summarize
        #[arg(short, long, default_value_t = DEFAULT_NEWS_LIMIT)]
        limit: usize,
    },
    /// Generate shell auto-completion script
    Completions {
        /// Target shell to generate completions for
        #[arg(value_enum)]
        shell: Shell,
    },
    /// Check for and install the latest srz release
    Upgrade {
        /// Only check if an update is available without downloading or installing
        #[arg(short, long)]
        check: bool,

        /// Force reinstall even if already on the latest version
        #[arg(short, long)]
        force: bool,

        /// Install a specific version (e.g. "0.2.0" or "v0.2.0")
        #[arg(long)]
        version: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone, PartialEq, Eq)]
pub enum CacheCommand {
    /// Print the cache directory path
    Dir,
    /// Show total disk usage and entry counts in the cache
    Size,
    /// Remove cached entries (pages, search, summaries, views)
    #[command(alias = "clear", alias = "purge", alias = "wipe")]
    Clean {
        /// Also reset and wipe the analytics/history database
        #[arg(long)]
        reset_db: bool,
    },
    /// Prune expired or old cached entries based on retention policy
    Prune {
        /// Remove entries older than duration (e.g. 7d, 24h, 30m, 60s, 2w)
        #[arg(long)]
        older_than: Option<String>,

        /// Evict oldest entries until cache size is within limit (e.g. 500MB, 1GB, 50MB)
        #[arg(long)]
        max_size: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSubcommand {
    /// Print the config file path and existence status
    Path,
    /// Display active configuration
    Show,
    /// Generate a default config.toml template
    Init,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedAction<'a> {
    Web {
        url: &'a str,
    },
    Book {
        title: &'a str,
    },
    File {
        path: PathBuf,
    },
    Stdin,
    News {
        topic: String,
        timeframe: NewsTimeframe,
        limit: usize,
    },
    Ask {
        target: Option<String>,
        question: Option<String>,
        interactive: bool,
        list_chats: bool,
        resume: Option<i64>,
        show_chat: Option<i64>,
        delete_chat: Option<i64>,
    },
    Subcommand(&'a Commands),
}

impl Cli {
    /// Returns the explicit `--max-tokens` / config value or the active `--style` default.
    pub fn effective_max_tokens(&self) -> u32 {
        self.max_tokens
            .unwrap_or_else(|| self.style.default_max_tokens())
    }

    pub fn stream_mode<'a>(&self, header_title: &'a str) -> crate::llm::StreamMode<'a> {
        if self.stream && self.output.is_none() && !self.json {
            crate::llm::StreamMode::Stdout { header_title }
        } else {
            crate::llm::StreamMode::Buffered
        }
    }

    pub fn resolve_action(&self) -> Result<ResolvedAction<'_>> {
        if self.max_tokens == Some(0) {
            anyhow::bail!("--max-tokens must be greater than 0");
        }
        if self.context_window == Some(0) {
            anyhow::bail!("--context-window must be greater than 0");
        }
        if let Some(cmd) = &self.command {
            match cmd {
                Commands::History { limit } if *limit == 0 => {
                    anyhow::bail!("--limit must be greater than 0");
                }
                Commands::Find { query } if query.trim().is_empty() => {
                    anyhow::bail!("Search query must not be empty");
                }
                Commands::Ask {
                    first,
                    second,
                    interactive,
                    chats,
                    resume,
                    show_chat,
                    delete_chat,
                } => {
                    let mgmt_count = usize::from(*chats)
                        + usize::from(show_chat.is_some())
                        + usize::from(delete_chat.is_some());
                    if mgmt_count > 1 {
                        anyhow::bail!("--chats, --show-chat, and --delete-chat cannot be combined");
                    }
                    let (target, question) = match (first.as_deref(), second.as_deref()) {
                        (Some(f), Some(s)) => {
                            (Some(f.trim().to_string()), Some(s.trim().to_string()))
                        }
                        (Some(f), None) if is_probable_question(f) => {
                            (None, Some(f.trim().to_string()))
                        }
                        (Some(f), None) => (Some(f.trim().to_string()), None),
                        (None, _) => (None, None),
                    };
                    Ok(ResolvedAction::Ask {
                        target,
                        question,
                        interactive: *interactive || resume.is_some(),
                        list_chats: *chats,
                        resume: *resume,
                        show_chat: *show_chat,
                        delete_chat: *delete_chat,
                    })
                }
                Commands::News {
                    topic,
                    timeframe,
                    limit,
                } => {
                    let max_limit = crate::sources::news::MAX_NEWS_LIMIT;
                    if *limit == 0 || *limit > max_limit {
                        anyhow::bail!("--limit must be between 1 and {max_limit}");
                    }
                    let clean_topic =
                        if crate::sources::news::GoogleNewsClient::is_direct_feed(topic) {
                            topic.trim().to_string()
                        } else {
                            clean_news_query(topic)
                        };
                    Ok(ResolvedAction::News {
                        topic: clean_topic,
                        timeframe: *timeframe,
                        limit: *limit,
                    })
                }
                _ => Ok(ResolvedAction::Subcommand(cmd)),
            }
        } else if let Some(target) = &self.target {
            let trimmed = target.trim();
            if trimmed == "-" {
                Ok(ResolvedAction::Stdin)
            } else if crate::system::text_util::is_http_url(trimmed) {
                if is_feed_url(trimmed) {
                    Ok(ResolvedAction::News {
                        topic: trimmed.to_string(),
                        timeframe: NewsTimeframe::All,
                        limit: DEFAULT_NEWS_LIMIT,
                    })
                } else {
                    Ok(ResolvedAction::Web { url: trimmed })
                }
            } else if is_file_like(trimmed) {
                Ok(ResolvedAction::File {
                    path: expand_tilde(trimmed),
                })
            } else if is_news_query(trimmed) {
                Ok(ResolvedAction::News {
                    topic: clean_news_query(trimmed),
                    timeframe: NewsTimeframe::All,
                    limit: DEFAULT_NEWS_LIMIT,
                })
            } else if trimmed.eq_ignore_ascii_case("version") {
                anyhow::bail!(
                    "Unrecognized command '{trimmed}'. Run `srz --version` to print the version."
                );
            } else if let Some(suggestion) = suggest_command(trimmed) {
                anyhow::bail!(
                    "Unrecognized command '{trimmed}'.\n\n  \
                     tip: a similar command exists: '{suggestion}'"
                );
            } else {
                Ok(ResolvedAction::Book { title: trimmed })
            }
        } else {
            anyhow::bail!(
                "Please provide a URL, local file, book title, or '-' for stdin. Run `srz --help` for details."
            );
        }
    }

    pub fn apply_config(
        &mut self,
        config: &Config,
        matches: Option<&clap::ArgMatches>,
    ) -> Result<()> {
        self.apply_config_with(config, matches, |k| std::env::var(k).ok())
    }

    pub(crate) fn apply_config_with(
        &mut self,
        config: &Config,
        matches: Option<&clap::ArgMatches>,
        env_get: impl Fn(&str) -> Option<String>,
    ) -> Result<()> {
        let profile = match self.profile.as_deref() {
            Some(name) => Some(config.profiles.get(name).ok_or_else(|| {
                anyhow::anyhow!(
                    "Unknown configuration profile '{}' (not found in config.toml)",
                    name
                )
            })?),
            None => None,
        };

        let was_provided = |id: &str| -> bool {
            matches
                .and_then(|m| m.value_source(id))
                .is_some_and(|s| s == clap::parser::ValueSource::CommandLine)
        };

        if was_provided("api_key") && !self.api_key.trim().is_empty() {
            crate::ui::Ui::warn(
                "Passing `--api-key` on the CLI can expose secrets in process listings; prefer `OPENAI_API_KEY` or `config.toml`.",
            );
        }

        macro_rules! apply_cfg {
            ($field:ident, env: $env_key:literal) => {
                if !was_provided(stringify!($field)) {
                    if let Some(val) = env_get($env_key).filter(|v| !v.trim().is_empty()) {
                        self.$field = val;
                    } else if let Some(val) = profile
                        .and_then(|p| p.$field.clone())
                        .or_else(|| config.settings.$field.clone())
                    {
                        self.$field = val;
                    }
                }
            };
            ($field:ident) => {
                if !was_provided(stringify!($field))
                    && let Some(val) = profile
                        .and_then(|p| p.$field.clone())
                        .or_else(|| config.settings.$field.clone())
                {
                    self.$field = val;
                }
            };
            (opt $field:ident) => {
                if !was_provided(stringify!($field))
                    && let Some(val) = profile
                        .and_then(|p| p.$field)
                        .or_else(|| config.settings.$field)
                {
                    self.$field = Some(val);
                }
            };
        }

        apply_cfg!(base_url, env: "OPENAI_BASE_URL");
        apply_cfg!(model, env: "OPENAI_MODEL");
        apply_cfg!(api_key, env: "OPENAI_API_KEY");
        apply_cfg!(style);
        apply_cfg!(opt max_tokens);
        apply_cfg!(no_cache);
        apply_cfg!(quiet);
        apply_cfg!(verbose);
        apply_cfg!(open);
        apply_cfg!(stream);
        apply_cfg!(show_thinking);
        apply_cfg!(opt context_window);
        Ok(())
    }
}

/// Expands a leading `~/` (or bare `~`) against `$HOME` or `$USERPROFILE`.
pub(crate) fn expand_tilde(s: &str) -> PathBuf {
    let rest = if s == "~" {
        Some("")
    } else {
        s.strip_prefix("~/").or_else(|| s.strip_prefix("~\\"))
    };
    if let Some(sub) = rest
        && let Some(home) = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .or_else(|| std::env::var_os("USERPROFILE").filter(|v| !v.is_empty()))
    {
        return if sub.is_empty() {
            PathBuf::from(home)
        } else {
            PathBuf::from(home).join(sub)
        };
    }
    PathBuf::from(s)
}

pub fn is_file_like(s: &str) -> bool {
    if crate::system::text_util::is_http_url(s) {
        return false;
    }
    let expanded = expand_tilde(s);
    if expanded.exists() {
        return true;
    }
    if s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with('/')
        || s.starts_with("~/")
        || s == "~"
        || s.contains('\\')
        || (s.len() >= 3 && s.as_bytes()[0].is_ascii_alphabetic() && s.as_bytes()[1] == b':')
        || (s.contains('/') && !s.chars().any(char::is_whitespace))
    {
        return true;
    }
    if let Some(ext) = expanded.extension().and_then(|e| e.to_str()) {
        const FILE_LIKE_EXTS: &[&str] = &[
            "pdf", "md", "markdown", "txt", "text", "html", "htm", "epub", "json",
        ];
        FILE_LIKE_EXTS
            .iter()
            .any(|&target| ext.eq_ignore_ascii_case(target))
    } else {
        false
    }
}

/// Returns `true` if `url` is an HTTP(S) syndication feed URL (RSS, Atom, or JSON Feed).
pub fn is_feed_url(url: &str) -> bool {
    let trimmed = url.trim();
    if !crate::system::text_util::is_http_url(trimmed) {
        return false;
    }
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let (path_part, query_part) = match without_fragment.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (without_fragment, None),
    };
    let trimmed_path = path_part.trim_end_matches('/');
    const FEED_SUFFIXES: &[&str] = &[
        ".rss",
        ".atom",
        "/rss",
        "/feed",
        "/atom",
        "/feed.xml",
        "/rss.xml",
        "/atom.xml",
        "/index.xml",
        "/feed.json",
    ];
    if FEED_SUFFIXES.iter().any(|&suffix| {
        crate::system::text_util::ends_with_ascii_case_insensitive(trimmed_path, suffix)
    }) || crate::system::text_util::find_ascii_case_insensitive(trimmed_path, "/rss/").is_some()
    {
        return true;
    }
    if let Some(q) = query_part {
        const QUERY_PATTERNS: &[&str] = &["format=rss", "format=atom", "output=rss", "feed=rss"];
        if QUERY_PATTERNS.iter().any(|&pattern| {
            crate::system::text_util::find_ascii_case_insensitive(q, pattern).is_some()
        }) {
            return true;
        }
    }
    false
}

pub fn is_news_query(s: &str) -> bool {
    let mut words = s.split_whitespace();
    let Some(first) = words.next() else {
        return false;
    };
    if first.eq_ignore_ascii_case("news") || first.eq_ignore_ascii_case("headlines") {
        return true;
    }
    let first_is_recent =
        first.eq_ignore_ascii_case("current") || first.eq_ignore_ascii_case("latest");
    let mut has_update_or_story = false;

    for word in words {
        if word.eq_ignore_ascii_case("news") || word.eq_ignore_ascii_case("headlines") {
            return true;
        }
        if first_is_recent
            && (word.eq_ignore_ascii_case("updates") || word.eq_ignore_ascii_case("stories"))
        {
            has_update_or_story = true;
        }
    }

    first_is_recent && has_update_or_story
}

pub fn clean_news_query(s: &str) -> String {
    const STOP_WORDS: &[&str] = &[
        "current",
        "latest",
        "today's",
        "todays",
        "recent",
        "news",
        "headline",
        "headlines",
        "about",
        "on",
        "regarding",
        "for",
    ];

    let words: Vec<&str> = s
        .split_whitespace()
        .filter(|w| !STOP_WORDS.iter().any(|stop| w.eq_ignore_ascii_case(stop)))
        .collect();

    if words.is_empty() {
        "top stories".to_string()
    } else {
        words.join(" ")
    }
}

pub fn is_probable_question(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.ends_with('?') {
        return true;
    }
    let Some(first_word) = trimmed.split_whitespace().next() else {
        return false;
    };
    if trimmed.len() <= first_word.len() {
        return false;
    }
    const QUESTION_PREFIXES: &[&str] = &[
        "what",
        "why",
        "how",
        "who",
        "when",
        "where",
        "which",
        "can",
        "could",
        "should",
        "would",
        "is",
        "are",
        "explain",
        "summarize",
        "tell",
        "describe",
        "clarify",
    ];
    QUESTION_PREFIXES
        .iter()
        .any(|prefix| first_word.eq_ignore_ascii_case(prefix))
}

/// Suggests a matching subcommand for a single-token typo, or `None` for non-commands.
pub fn suggest_command(input: &str) -> Option<String> {
    let trimmed = input.trim();

    if trimmed.is_empty()
        || trimmed.contains(char::is_whitespace)
        || trimmed.contains('/')
        || trimmed.contains('\\')
        || trimmed.len() < 3
    {
        return None;
    }

    let input_lower = trimmed.to_ascii_lowercase();
    let root = <Cli as clap::CommandFactory>::command();
    let mut candidates: Vec<String> = root
        .get_subcommands()
        .flat_map(|s| std::iter::once(s.get_name()).chain(s.get_all_aliases()))
        .chain(["help"])
        .map(str::to_string)
        .collect();
    candidates.sort();
    candidates.dedup();

    if let Some(exact) = candidates.iter().find(|cmd| input_lower == cmd.as_str()) {
        return Some(exact.clone());
    }

    let input_len = input_lower.chars().count();
    candidates
        .into_iter()
        .filter_map(|cmd| {
            let cmd_len = cmd.chars().count();
            let dist = crate::system::text_util::damerau_levenshtein(&input_lower, &cmd);
            let is_candidate = match input_len {
                0..=2 => false,
                3 => cmd_len == 3 && dist == 1,
                4 => cmd_len <= 5 && dist == 1,
                _ => (cmd_len as isize - input_len as isize).abs() <= 2 && dist <= 2,
            };
            is_candidate.then_some((cmd, dist))
        })
        .min_by_key(|&(_, dist)| dist)
        .map(|(cmd, _)| cmd)
}

#[derive(Clone, Default, Deserialize, PartialEq, Eq)]
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
    pub stream: Option<bool>,
    pub show_thinking: Option<bool>,
    pub context_window: Option<usize>,
}

impl fmt::Debug for ProfileConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProfileConfig")
            .field("base_url", &self.base_url)
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_deref().map(redact_secret))
            .field("style", &self.style)
            .field("max_tokens", &self.max_tokens)
            .field("no_cache", &self.no_cache)
            .field("quiet", &self.quiet)
            .field("verbose", &self.verbose)
            .field("open", &self.open)
            .field("stream", &self.stream)
            .field("show_thinking", &self.show_thinking)
            .field("context_window", &self.context_window)
            .finish()
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct CacheConfig {
    pub max_size_mb: Option<u64>,
    pub pages_ttl_days: Option<u64>,
    pub news_ttl_hours: Option<u64>,
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
            policy.pages_ttl = std::time::Duration::from_secs(days.saturating_mul(SECS_PER_DAY));
        }
        if let Some(hours) = self.news_ttl_hours {
            policy.news_ttl = std::time::Duration::from_secs(hours.saturating_mul(3600));
        }
        if let Some(days) = self.search_ttl_days {
            policy.search_ttl = std::time::Duration::from_secs(days.saturating_mul(SECS_PER_DAY));
        }
        if let Some(days) = self.summaries_ttl_days {
            policy.summaries_ttl =
                std::time::Duration::from_secs(days.saturating_mul(SECS_PER_DAY));
        }
        if let Some(days) = self.views_ttl_days {
            policy.views_ttl = std::time::Duration::from_secs(days.saturating_mul(SECS_PER_DAY));
        }
        if let Some(false) = self.auto_prune {
            policy.auto_prune_interval = std::time::Duration::MAX;
        }
        policy
    }
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
pub struct Config {
    #[serde(flatten)]
    pub settings: ProfileConfig,
    #[serde(default)]
    pub profiles: HashMap<String, ProfileConfig>,
    #[serde(default)]
    pub cache: Option<CacheConfig>,
}

#[cfg(unix)]
fn has_configured_api_key(cfg: &Config) -> bool {
    cfg.settings
        .api_key
        .as_deref()
        .is_some_and(|k| !k.trim().is_empty())
        || cfg
            .profiles
            .values()
            .any(|p| p.api_key.as_deref().is_some_and(|k| !k.trim().is_empty()))
}

fn sanitize_toml_scalar(s: &str) -> String {
    s.chars()
        .filter(|&c| !c.is_control() && c != '"' && c != '\\')
        .collect()
}

impl Config {
    /// Returns the default path for `config.toml` (`$SRZ_CONFIG`, `$SRZ_CONFIG_PATH`, `$XDG_CONFIG_HOME/srz/config.toml`, `~/.config/srz/config.toml`, or `%APPDATA%\srz\config.toml`).
    pub fn default_config_path() -> PathBuf {
        crate::fs_util::default_config_path()
    }

    /// Loads configuration from `path` (refusing symlinks and files larger than 1 MB).
    pub fn load_from_path(path: &Path) -> Result<Self> {
        let bytes =
            crate::fs_util::read_file_bytes_limited(path, MAX_CONFIG_BYTES, true, "Config")?;
        let content = std::str::from_utf8(&bytes)
            .with_context(|| format!("Failed to read config file at {:?}", path))?;
        let cfg: Self = toml::from_str(content)
            .with_context(|| format!("Failed to parse TOML configuration from {:?}", path))?;

        #[cfg(unix)]
        if has_configured_api_key(&cfg)
            && let Ok(meta) = std::fs::symlink_metadata(path)
        {
            use std::os::unix::fs::PermissionsExt;
            if (meta.permissions().mode() & 0o077) != 0 {
                crate::ui::Ui::warn(&format!(
                    "Config file {:?} stores an API key with overly broad permissions; run `chmod 600 {}`",
                    path,
                    path.display()
                ));
            }
        }

        Ok(cfg)
    }

    /// Loads the default config file if present, propagating parse, symlink, or I/O errors.
    pub fn load_default() -> Result<(Self, Option<PathBuf>)> {
        let path = Self::default_config_path();
        match std::fs::symlink_metadata(&path) {
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok((Self::default(), None)),
            Err(err) => {
                Err(err).with_context(|| format!("Failed to inspect config file at {:?}", path))
            }
            Ok(_) => Ok((Self::load_from_path(&path)?, Some(path))),
        }
    }

    /// Generates a starter `config.toml` template tailored for the detected hardware.
    pub fn template_for_hardware(hw: &crate::system::hardware::HardwareProfile) -> String {
        let chip = sanitize_toml_scalar(&hw.chip_name);
        let model = sanitize_toml_scalar(hw.recommended_model);
        let launch_hint = sanitize_toml_scalar(&hw.llama_gui_selection());
        format!(
            r#"# srz (Summarize) configuration file
# Default path: ~/.config/srz/config.toml
# Tailored for host: {chip} ({:.1} GB RAM / {})
# Recommended local setup: {launch_hint}

# Default local LLM endpoint (Llama-macOS, Llama-Windows, llama.cpp, Ollama)
base_url = "http://localhost:9931/v1"
model = "{model}"
context_window = {}
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

# Cloud profile (set OPENAI_API_KEY env var or uncomment api_key below)
[profiles.openai]
base_url = "https://api.openai.com/v1"
model = "gpt-4o-mini"
# api_key = "sk-..."

# Quick mode: Fast concise summaries for news and short articles
[profiles.fast]
base_url = "http://localhost:9931/v1"
model = "{model}"
style = "concise"
context_window = 8192

# Deep research / book mode: Full synthesis with maximum context window
[profiles.deep]
base_url = "http://localhost:9931/v1"
model = "{model}"
style = "detailed"
context_window = {}
max_tokens = 8192
"#,
            hw.total_memory_gb,
            hw.tier.display_name(),
            hw.recommended_context,
            hw.recommended_context,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_template_for_hardware_and_secret_redaction() {
        let template =
            Config::template_for_hardware(crate::system::hardware::HardwareProfile::detect());
        let cfg: Config =
            toml::from_str(&template).expect("hardware template must parse as valid TOML");
        assert_eq!(
            cfg.settings.base_url.as_deref(),
            Some("http://localhost:9931/v1")
        );
        assert!(cfg.settings.model.is_some());
        assert_eq!(cfg.settings.style, Some(SummaryStyle::Concise));
        assert_eq!(cfg.settings.max_tokens, Some(8192));
        assert!(
            cfg.profiles
                .get("openai")
                .is_some_and(|p| p.api_key.is_none()),
            "placeholder api_key must stay commented out"
        );
        for key in ["openai", "fast", "deep"] {
            assert!(cfg.profiles.contains_key(key));
        }

        let mut cli = Cli::parse_from(["srz", "Dune"]);
        cli.api_key = "sk-top-secret".into();
        let prof = ProfileConfig {
            api_key: Some("sk-profile-secret".into()),
            ..Default::default()
        };
        let cli_dbg = format!("{cli:?}");
        let prof_dbg = format!("{prof:?}");
        assert!(!cli_dbg.contains("sk-top-secret") && cli_dbg.contains("[REDACTED]"));
        assert!(!prof_dbg.contains("sk-profile-secret") && prof_dbg.contains("[REDACTED]"));
    }

    #[test]
    fn test_load_from_path_lifecycle_and_symlink_rejection() {
        let tmp = crate::system::fs_util::tempdir().unwrap();
        let config_path = tmp.path().join("config.toml");
        assert!(Config::load_from_path(&config_path).is_err());

        fs::write(
            &config_path,
            "base_url = \"http://localhost:11434/v1\"\nmodel = \"llama3:8b\"\n",
        )
        .unwrap();
        let loaded = Config::load_from_path(&config_path).unwrap();
        assert_eq!(
            loaded.settings.base_url.as_deref(),
            Some("http://localhost:11434/v1")
        );
        assert_eq!(loaded.settings.model.as_deref(), Some("llama3:8b"));

        fs::write(&config_path, "not a [valid toml").unwrap();
        assert!(Config::load_from_path(&config_path).is_err());

        #[cfg(unix)]
        {
            let real = tmp.path().join("real.toml");
            let sym = tmp.path().join("sym.toml");
            fs::write(&real, "model = \"test\"\n").unwrap();
            std::os::unix::fs::symlink(&real, &sym).unwrap();
            assert!(
                Config::load_from_path(&sym)
                    .unwrap_err()
                    .to_string()
                    .contains("symlink")
            );
        }
    }

    #[test]
    fn test_env_precedence_validation_and_edge_cases() {
        use clap::{CommandFactory, FromArgMatches};
        assert!(
            !Cli::command()
                .render_help()
                .to_string()
                .contains("OPENAI_API_KEY=")
        );

        let matches = Cli::command().get_matches_from(["srz", "Dune", "--model", "cli-model"]);
        let mut cli = Cli::from_arg_matches(&matches).unwrap();
        let cfg = Config {
            settings: ProfileConfig {
                base_url: Some("http://cfg/v1".into()),
                api_key: Some("cfg-key".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        cli.apply_config_with(&cfg, Some(&matches), |k| match k {
            "OPENAI_API_KEY" => Some("sk-env-secret".into()),
            "OPENAI_MODEL" => Some("env-model".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(cli.model, "cli-model");
        assert_eq!(cli.api_key, "sk-env-secret");
        assert_eq!(cli.base_url, "http://cfg/v1");

        // "version" must not self-suggest, and slash-containing titles must not become files.
        assert_eq!(suggest_command("version"), None);
        assert!(!is_file_like("9/11 Commission Report"));
        assert!(is_file_like("assets/document.pdf"));
        assert!(
            Cli::parse_from(["srz", "srz version"])
                .resolve_action()
                .is_ok()
        );
        assert!(
            Cli::parse_from(["srz", "version"])
                .resolve_action()
                .is_err()
        );

        for invalid in [
            &["srz", "Dune", "--max-tokens", "0"][..],
            &["srz", "Dune", "--context-window", "0"][..],
            &["srz", "history", "--limit", "0"][..],
            &["srz", "news", "rust", "--limit", "0"][..],
            &["srz", "news", "rust", "--limit", "999"][..],
            &["srz", "ask", "--chats", "--delete-chat", "1"][..],
            &["srz", "find", "   "][..],
        ] {
            assert!(
                Cli::parse_from(invalid).resolve_action().is_err(),
                "expected validation error for {invalid:?}"
            );
        }
    }
}
