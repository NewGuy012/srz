use clap::{Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
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

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum NewsTimeframe {
    /// Past 24 hours / current day (default)
    #[default]
    Day,
    /// Past 7 days / week
    Week,
    /// Past 30 days / month
    Month,
    /// Any time / all results
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
            Self::Day => "Today / Past 24 Hours",
            Self::Week => "Past Week",
            Self::Month => "Past Month",
            Self::All => "All Time",
        }
    }
}

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum PagingMode {
    /// Automatically page output when it exceeds terminal height
    #[default]
    Auto,
    /// Always page output through a terminal pager
    Always,
    /// Never page output
    Never,
}

#[derive(Parser, Debug)]
#[command(
    name = "srz",
    version,
    about = "Modular CLI to summarize webpages and books using local OpenAI-compliant LLMs",
    long_about = "A fast, modular CLI that extracts and summarizes webpages and books. \
                  Supports local OpenAI-compatible backends like Ollama, LM Studio, vLLM, and llama.cpp. \
                  For books, searches the web by default for grounded synopses, with automatic fallback to LLM memory."
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// Auto-detected target (URL or book title) when no subcommand is specified
    #[arg(value_name = "TARGET")]
    pub target: Option<String>,

    /// Local OpenAI-compatible base URL
    #[arg(
        long,
        global = true,
        env = "OPENAI_BASE_URL",
        default_value = "http://localhost:9931/v1"
    )]
    pub base_url: String,

    /// Model name to use
    #[arg(
        long,
        global = true,
        env = "OPENAI_MODEL",
        default_value = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
    )]
    pub model: String,

    /// OpenAI API key (optional for local LLMs)
    #[arg(long, global = true, env = "OPENAI_API_KEY", default_value = "")]
    pub api_key: String,

    /// Output summary style
    #[arg(long, global = true, value_enum, default_value = "concise")]
    pub style: SummaryStyle,

    /// Maximum tokens to generate (includes thinking + summary tokens; tuned for 32k context)
    #[arg(long, global = true, default_value_t = 8192)]
    pub max_tokens: u32,

    /// Skip web search for books (force using LLM trained knowledge directly)
    #[arg(long, global = true)]
    pub no_search: bool,

    /// Bypass local cache and force fresh web scraping and LLM summarization
    #[arg(long, global = true)]
    pub no_cache: bool,

    /// Path to optional config file (defaults to ~/.config/srz/config.toml)
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

    /// Automatically accept best reference match without prompting
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Automatically open the styled HTML summary in the default web browser
    #[arg(short = 'b', long, global = true)]
    pub open: bool,

    /// Concurrency limit for parallel map-reduce document chunking
    #[arg(long, global = true, default_value_t = 1)]
    pub concurrency: usize,

    /// Stream LLM output tokens live to stdout as they are generated (default: false)
    #[arg(long, global = true)]
    pub stream: bool,

    /// Control terminal paging of summary output (auto, always, never)
    #[arg(long, global = true, value_enum, default_value = "auto")]
    pub paging: PagingMode,

    /// Output raw, unstyled Markdown without terminal formatting or ANSI colors
    #[arg(long, global = true)]
    pub plain: bool,
}

impl Cli {
    /// Returns the effective paging mode.
    pub fn effective_paging(&self) -> PagingMode {
        self.paging
    }
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Scrape and summarize a webpage from a URL
    Web {
        /// Webpage URL (http:// or https://)
        url: String,
    },
    /// Summarize a book by title
    Book {
        /// Title of the book
        title: String,
    },
    /// Summarize a local file or piped input from standard input (-)
    File {
        /// Path to the local file, or '-' for stdin
        path: PathBuf,
    },
    /// Manage the local disk cache (dir, size, clean)
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
        /// Maximum entries to display (default: 10)
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
    /// Generate and view the standalone HTML library dashboard
    Index,
    /// Launch the interactive terminal UI library explorer
    #[command(alias = "tui", alias = "library")]
    Browse,
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
    /// Search and summarize latest news on a topic using Google News RSS
    News {
        /// Topic or query to search for (e.g. "javascript", "rust", "tech")
        topic: String,
        /// Timeframe for news recency (day, week, month, all; default: day)
        #[arg(short = 't', long, value_enum, default_value = "day")]
        timeframe: NewsTimeframe,
        /// Maximum number of top articles to aggregate and summarize (default: 5)
        #[arg(short, long, default_value_t = 5)]
        limit: usize,
    },
    /// Generate shell auto-completion script
    Completions {
        /// Target shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
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
    /// Remove cached files (pages, search, summaries, views)
    Clean {
        /// Also reset and wipe the DuckDB analytics/history database
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

        /// Only prune a specific cache namespace (pages, search, summaries, views)
        #[arg(long)]
        namespace: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigSubcommand {
    /// Print the config file path and existence status
    Path,
    /// Display active configuration
    Show,
    /// Generate a default ~/.config/srz/config.toml template
    Init,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ResolvedAction<'a> {
    Web {
        url: &'a str,
    },
    Book {
        title: &'a str,
        no_search: bool,
    },
    File {
        path: PathBuf,
    },
    Stdin,
    Cache(CacheCommand),
    Config(ConfigSubcommand),
    History {
        limit: usize,
    },
    Stats,
    Find {
        query: &'a str,
    },
    Models,
    Doctor,
    Index,
    Browse,
    Ask {
        target: Option<String>,
        question: Option<String>,
        interactive: bool,
        list_chats: bool,
        resume: Option<i64>,
        show_chat: Option<i64>,
        delete_chat: Option<i64>,
    },
    News {
        topic: String,
        timeframe: NewsTimeframe,
        limit: usize,
    },
    Completions {
        shell: clap_complete::Shell,
    },
    Upgrade {
        check: bool,
        force: bool,
        version: Option<String>,
    },
}

impl Cli {
    pub fn stream_mode<'a>(&self, header_title: &'a str) -> crate::llm::StreamMode<'a> {
        if self.stream && self.output.is_none() && !self.json {
            crate::llm::StreamMode::Stdout { header_title }
        } else {
            crate::llm::StreamMode::Buffered
        }
    }

    pub fn resolve_action(&self) -> Result<ResolvedAction<'_>, &'static str> {
        if let Some(cmd) = &self.command {
            match cmd {
                Commands::Web { url } => Ok(ResolvedAction::Web { url: url.trim() }),
                Commands::Book { title } => Ok(ResolvedAction::Book {
                    title: title.trim(),
                    no_search: self.no_search,
                }),
                Commands::File { path } => {
                    if path.as_os_str() == "-" {
                        Ok(ResolvedAction::Stdin)
                    } else {
                        Ok(ResolvedAction::File { path: path.clone() })
                    }
                }
                Commands::Cache { action } => Ok(ResolvedAction::Cache(action.clone())),
                Commands::Config { action } => Ok(ResolvedAction::Config(*action)),
                Commands::History { limit } => Ok(ResolvedAction::History { limit: *limit }),
                Commands::Stats => Ok(ResolvedAction::Stats),
                Commands::Find { query } => Ok(ResolvedAction::Find {
                    query: query.trim(),
                }),
                Commands::Models => Ok(ResolvedAction::Models),
                Commands::Doctor => Ok(ResolvedAction::Doctor),
                Commands::Index => Ok(ResolvedAction::Index),
                Commands::Browse => Ok(ResolvedAction::Browse),
                Commands::Ask {
                    first,
                    second,
                    interactive,
                    chats,
                    resume,
                    show_chat,
                    delete_chat,
                } => {
                    let (target, question) = match (first.as_deref(), second.as_deref()) {
                        (Some(f), Some(s)) => {
                            (Some(f.trim().to_string()), Some(s.trim().to_string()))
                        }
                        (Some(f), None) => {
                            if is_probable_question(f) {
                                (None, Some(f.trim().to_string()))
                            } else {
                                (Some(f.trim().to_string()), None)
                            }
                        }
                        (None, None) => (None, None),
                        (None, Some(s)) => (None, Some(s.trim().to_string())),
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
                } => Ok(ResolvedAction::News {
                    topic: topic.trim().to_string(),
                    timeframe: *timeframe,
                    limit: *limit,
                }),
                Commands::Completions { shell } => {
                    Ok(ResolvedAction::Completions { shell: *shell })
                }
                Commands::Upgrade {
                    check,
                    force,
                    version,
                } => Ok(ResolvedAction::Upgrade {
                    check: *check,
                    force: *force,
                    version: version.clone(),
                }),
            }
        } else if let Some(target) = &self.target {
            let trimmed = target.trim();
            if trimmed == "-" {
                Ok(ResolvedAction::Stdin)
            } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                Ok(ResolvedAction::Web { url: trimmed })
            } else {
                let candidate_path = std::path::Path::new(trimmed);
                if candidate_path.is_file() || is_file_like(trimmed) {
                    Ok(ResolvedAction::File {
                        path: candidate_path.to_path_buf(),
                    })
                } else if is_news_query(trimmed) {
                    let clean_topic = clean_news_query(trimmed);
                    Ok(ResolvedAction::News {
                        topic: clean_topic,
                        timeframe: NewsTimeframe::Day,
                        limit: 5,
                    })
                } else {
                    Ok(ResolvedAction::Book {
                        title: trimmed,
                        no_search: self.no_search,
                    })
                }
            }
        } else {
            Err(
                "Please provide a URL, local file, book title, or '-' for stdin. Run `srz --help` for details.",
            )
        }
    }

    pub fn apply_config(
        &mut self,
        config: &crate::config::Config,
        matches: Option<&clap::ArgMatches>,
    ) {
        let profile = self.profile.as_deref().and_then(|p| config.profiles.get(p));

        let was_provided = |id: &str| -> bool {
            if let Some(m) = matches {
                m.value_source(id) == Some(clap::parser::ValueSource::CommandLine)
                    || m.value_source(id) == Some(clap::parser::ValueSource::EnvVariable)
            } else {
                false
            }
        };

        macro_rules! apply_field {
            ($field:ident) => {
                if !was_provided(stringify!($field))
                    && let Some(val) = profile
                        .and_then(|p| p.$field.clone())
                        .or_else(|| config.$field.clone())
                {
                    self.$field = val;
                }
            };
        }

        apply_field!(base_url);
        apply_field!(model);
        apply_field!(api_key);
        apply_field!(style);
        apply_field!(max_tokens);
        apply_field!(no_search);
        apply_field!(no_cache);
        apply_field!(quiet);
        apply_field!(verbose);
        apply_field!(open);
        apply_field!(concurrency);
        apply_field!(stream);
    }
}

pub fn is_file_like(s: &str) -> bool {
    if s.starts_with("http://") || s.starts_with("https://") {
        return false;
    }
    let path = std::path::Path::new(s);
    if path.is_file() {
        return true;
    }
    if s.starts_with("./")
        || s.starts_with("../")
        || s.starts_with('/')
        || s.starts_with('~')
        || s.contains('/')
        || s.contains('\\')
    {
        return true;
    }
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let lower = ext.to_ascii_lowercase();
        matches!(
            lower.as_str(),
            "pdf" | "md" | "markdown" | "txt" | "text" | "html" | "htm" | "epub" | "json"
        )
    } else {
        false
    }
}

pub fn is_news_query(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    let words: Vec<&str> = lower.split_whitespace().collect();
    if words.is_empty() {
        return false;
    }
    words.contains(&"news")
        || words.contains(&"headlines")
        || ((words[0] == "current" || words[0] == "latest")
            && words.len() > 1
            && (words.contains(&"updates") || words.contains(&"stories")))
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

    let filtered: Vec<&str> = s
        .split_whitespace()
        .filter(|w| !STOP_WORDS.contains(&w.to_ascii_lowercase().as_str()))
        .collect();

    if filtered.is_empty() {
        "top stories".to_string()
    } else {
        filtered.join(" ")
    }
}

pub fn is_probable_question(s: &str) -> bool {
    let trimmed = s.trim();
    if trimmed.ends_with('?') {
        return true;
    }
    let lower = trimmed.to_ascii_lowercase();
    let question_prefixes = [
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
    for prefix in question_prefixes {
        if lower.starts_with(prefix)
            && lower.len() > prefix.len()
            && lower[prefix.len()..].starts_with(' ')
        {
            return true;
        }
    }
    false
}
