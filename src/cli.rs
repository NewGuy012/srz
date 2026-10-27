use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
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
    #[arg(long, global = true, value_enum, default_value = "detailed")]
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

    /// Save the summary to an output file
    #[arg(short, long, global = true)]
    pub output: Option<PathBuf>,
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
    /// Manage the local disk cache (dir, size, clean)
    Cache {
        #[command(subcommand)]
        action: CacheCommand,
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
    Cache(CacheCommand),
    History {
        limit: usize,
    },
    Stats,
    Find {
        query: &'a str,
    },
}

impl Cli {
    pub fn resolve_action(&self) -> Result<ResolvedAction<'_>, &'static str> {
        if let Some(cmd) = &self.command {
            match cmd {
                Commands::Web { url } => Ok(ResolvedAction::Web { url: url.trim() }),
                Commands::Book { title } => Ok(ResolvedAction::Book {
                    title: title.trim(),
                    author: self.author.as_deref(),
                    no_search: self.no_search,
                }),
                Commands::Cache { action } => Ok(ResolvedAction::Cache(*action)),
                Commands::History { limit } => Ok(ResolvedAction::History { limit: *limit }),
                Commands::Stats => Ok(ResolvedAction::Stats),
                Commands::Find { query } => Ok(ResolvedAction::Find {
                    query: query.trim(),
                }),
            }
        } else if let Some(target) = &self.target {
            let trimmed = target.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                Ok(ResolvedAction::Web { url: trimmed })
            } else {
                Ok(ResolvedAction::Book {
                    title: trimmed,
                    author: self.author.as_deref(),
                    no_search: self.no_search,
                })
            }
        } else {
            Err("Please provide a URL or book title, or use `srz web <URL>` / `srz book <TITLE>` / `srz cache <ACTION>`. Run `srz --help` for details.")
        }
    }
}
