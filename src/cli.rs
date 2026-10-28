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
    /// Executive summary format (overview, context, impact)
    Executive,
}

impl SummaryStyle {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Concise => "concise",
            Self::Detailed => "detailed",
            Self::Bullets => "bullets",
            Self::Executive => "executive",
        }
    }

    pub fn prompt_guidance(&self) -> &'static str {
        match self {
            Self::Concise => "Provide a concise, high-level summary capturing the core message in 2-3 brief paragraphs.",
            Self::Detailed => "Provide a detailed, structured summary covering all major sections, themes, arguments, and conclusions.",
            Self::Bullets => "Provide a structured summary formatted as clear, concise bullet points highlighting key insights.",
            Self::Executive => "Provide an executive summary format: Executive Overview, Core Findings/Themes, Strategic Insights, and Takeaways.",
        }
    }
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

    /// Book author (helps search accuracy)
    #[arg(long, global = true)]
    pub author: Option<String>,

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
    /// Generate shell auto-completion script
    Completions {
        /// Target shell to generate completions for
        #[arg(value_enum)]
        shell: clap_complete::Shell,
    },
}

#[derive(Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheCommand {
    /// Print the cache directory path
    Dir,
    /// Show total disk usage and entry counts in the cache
    Size,
    /// Remove all cached pages, search results, and summaries
    Clean,
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
        author: Option<&'a str>,
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
    Completions {
        shell: clap_complete::Shell,
    },
}

impl Cli {
    pub fn stream_mode<'a>(&self, header_title: &'a str) -> crate::llm::StreamMode<'a> {
        if self.output.is_none() && !self.json {
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
                    author: self.author.as_deref(),
                    no_search: self.no_search,
                }),
                Commands::File { path } => {
                    if path.as_os_str() == "-" {
                        Ok(ResolvedAction::Stdin)
                    } else {
                        Ok(ResolvedAction::File {
                            path: path.clone(),
                        })
                    }
                }
                Commands::Cache { action } => Ok(ResolvedAction::Cache(*action)),
                Commands::Config { action } => Ok(ResolvedAction::Config(*action)),
                Commands::History { limit } => Ok(ResolvedAction::History { limit: *limit }),
                Commands::Stats => Ok(ResolvedAction::Stats),
                Commands::Find { query } => Ok(ResolvedAction::Find {
                    query: query.trim(),
                }),
                Commands::Models => Ok(ResolvedAction::Models),
                Commands::Doctor => Ok(ResolvedAction::Doctor),
                Commands::Completions { shell } => Ok(ResolvedAction::Completions { shell: *shell }),
            }
        } else if let Some(target) = &self.target {
            let trimmed = target.trim();
            if trimmed == "-" {
                Ok(ResolvedAction::Stdin)
            } else if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                Ok(ResolvedAction::Web { url: trimmed })
            } else {
                let candidate_path = std::path::Path::new(trimmed);
                if candidate_path.is_file() {
                    Ok(ResolvedAction::File {
                        path: candidate_path.to_path_buf(),
                    })
                } else {
                    Ok(ResolvedAction::Book {
                        title: trimmed,
                        author: self.author.as_deref(),
                        no_search: self.no_search,
                    })
                }
            }
        } else {
            Err("Please provide a URL, local file, book title, or '-' for stdin. Run `srz --help` for details.")
        }
    }

    pub fn apply_config(
        &mut self,
        config: &crate::config::Config,
        matches: Option<&clap::ArgMatches>,
    ) {
        let profile = self
            .profile
            .as_deref()
            .and_then(|p| config.profiles.get(p));

        let was_provided = |id: &str| -> bool {
            if let Some(m) = matches {
                m.value_source(id) == Some(clap::parser::ValueSource::CommandLine)
                    || m.value_source(id) == Some(clap::parser::ValueSource::EnvVariable)
            } else {
                false
            }
        };

        if !was_provided("base_url") {
            if let Some(url) = profile
                .and_then(|p| p.base_url.as_ref())
                .or(config.base_url.as_ref())
            {
                self.base_url = url.clone();
            }
        }

        if !was_provided("model") {
            if let Some(m) = profile
                .and_then(|p| p.model.as_ref())
                .or(config.model.as_ref())
            {
                self.model = m.clone();
            }
        }

        if !was_provided("api_key") {
            if let Some(k) = profile
                .and_then(|p| p.api_key.as_ref())
                .or(config.api_key.as_ref())
            {
                self.api_key = k.clone();
            }
        }

        if !was_provided("style") {
            if let Some(s) = profile.and_then(|p| p.style).or(config.style) {
                self.style = s;
            }
        }

        if !was_provided("max_tokens") {
            if let Some(t) = profile.and_then(|p| p.max_tokens).or(config.max_tokens) {
                self.max_tokens = t;
            }
        }

        if !was_provided("no_search") {
            if let Some(ns) = profile.and_then(|p| p.no_search).or(config.no_search) {
                self.no_search = ns;
            }
        }

        if !was_provided("no_cache") {
            if let Some(nc) = profile.and_then(|p| p.no_cache).or(config.no_cache) {
                self.no_cache = nc;
            }
        }

        if !was_provided("quiet") {
            if let Some(q) = profile.and_then(|p| p.quiet).or(config.quiet) {
                self.quiet = q;
            }
        }

        if !was_provided("verbose") {
            if let Some(v) = profile.and_then(|p| p.verbose).or(config.verbose) {
                self.verbose = v;
            }
        }

        if !was_provided("open") {
            if let Some(o) = profile.and_then(|p| p.open).or(config.open) {
                self.open = o;
            }
        }

        if !was_provided("concurrency") {
            if let Some(c) = profile.and_then(|p| p.concurrency).or(config.concurrency) {
                self.concurrency = c;
            }
        }
    }
}
