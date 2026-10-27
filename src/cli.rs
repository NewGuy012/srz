use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

#[derive(ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
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
    #[arg(long, env = "OPENAI_BASE_URL", default_value = "http://localhost:9931/v1")]
    pub base_url: String,

    /// Model name to use
    #[arg(
        long,
        env = "OPENAI_MODEL",
        default_value = "ggml-org/gemma-4-E4B-it-GGUF:Q8_0"
    )]
    pub model: String,

    /// OpenAI API key (optional for local LLMs)
    #[arg(long, env = "OPENAI_API_KEY", default_value = "")]
    pub api_key: String,

    /// Output summary style
    #[arg(long, value_enum, default_value = "detailed")]
    pub style: SummaryStyle,

    /// Maximum tokens to generate
    #[arg(long, default_value_t = 4096)]
    pub max_tokens: u32,

    /// Skip web search for books (force using LLM trained knowledge directly)
    #[arg(long)]
    pub no_search: bool,

    /// Book author (helps search accuracy)
    #[arg(long)]
    pub author: Option<String>,

    /// Save the summary to an output file
    #[arg(short, long)]
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

        /// Author of the book (optional)
        #[arg(long)]
        author: Option<String>,

        /// Skip web search and rely purely on local LLM memory
        #[arg(long)]
        no_search: bool,
    },
}

pub enum ResolvedAction {
    Web {
        url: String,
    },
    Book {
        title: String,
        author: Option<String>,
        no_search: bool,
    },
}

impl Cli {
    pub fn resolve_action(&self) -> Result<ResolvedAction, String> {
        if let Some(cmd) = &self.command {
            match cmd {
                Commands::Web { url } => Ok(ResolvedAction::Web {
                    url: url.clone(),
                }),
                Commands::Book {
                    title,
                    author,
                    no_search,
                } => Ok(ResolvedAction::Book {
                    title: title.clone(),
                    author: author.clone().or_else(|| self.author.clone()),
                    no_search: *no_search || self.no_search,
                }),
            }
        } else if let Some(target) = &self.target {
            let trimmed = target.trim();
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                Ok(ResolvedAction::Web {
                    url: trimmed.to_string(),
                })
            } else {
                Ok(ResolvedAction::Book {
                    title: trimmed.to_string(),
                    author: self.author.clone(),
                    no_search: self.no_search,
                })
            }
        } else {
            Err("Please provide a URL or book title, or use `srz web <URL>` / `srz book <TITLE>`. Run `srz --help` for details.".to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auto_detect_url() {
        let cli = Cli {
            command: None,
            target: Some("https://example.com/article".to_string()),
            base_url: "http://localhost:9931/v1".to_string(),
            model: "ggml-org/gemma-4-E4B-it-GGUF:Q8_0".to_string(),
            api_key: "".to_string(),
            style: SummaryStyle::Detailed,
            max_tokens: 4096,
            no_search: false,
            author: None,
            output: None,
        };

        match cli.resolve_action().unwrap() {
            ResolvedAction::Web { url } => assert_eq!(url, "https://example.com/article"),
            _ => panic!("Expected Web action"),
        }
    }

    #[test]
    fn test_auto_detect_book() {
        let cli = Cli {
            command: None,
            target: Some("The Great Gatsby".to_string()),
            base_url: "http://localhost:9931/v1".to_string(),
            model: "ggml-org/gemma-4-E4B-it-GGUF:Q8_0".to_string(),
            api_key: "".to_string(),
            style: SummaryStyle::Concise,
            max_tokens: 4096,
            no_search: false,
            author: Some("F. Scott Fitzgerald".to_string()),
            output: None,
        };

        match cli.resolve_action().unwrap() {
            ResolvedAction::Book {
                title,
                author,
                no_search,
            } => {
                assert_eq!(title, "The Great Gatsby");
                assert_eq!(author, Some("F. Scott Fitzgerald".to_string()));
                assert!(!no_search);
            }
            _ => panic!("Expected Book action"),
        }
    }
}

