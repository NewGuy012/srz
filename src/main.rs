mod chunker;
mod cli;
mod llm;
mod scraper;
mod search;
mod ui;

use anyhow::{Context, Result};
use clap::Parser;
use cli::{Cli, ResolvedAction};
use llm::LlmClient;
use scraper::WebScraper;
use search::BookSearcher;
use std::fs;
use ui::Ui;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let action = match args.resolve_action() {
        Ok(action) => action,
        Err(msg) => {
            Ui::error(&msg);
            std::process::exit(1);
        }
    };

    let llm = match LlmClient::new(&args.base_url, &args.model, &args.api_key, args.max_tokens) {
        Ok(client) => client,
        Err(err) => {
            Ui::error(&format!("Failed to initialize LLM client: {}", err));
            std::process::exit(1);
        }
    };

    let scraper = match WebScraper::new() {
        Ok(s) => s,
        Err(err) => {
            Ui::error(&format!("Failed to initialize scraper: {}", err));
            std::process::exit(1);
        }
    };

    let summary_result = match action {
        ResolvedAction::Web { url } => handle_web(&url, &scraper, &llm, &args).await,
        ResolvedAction::Book {
            title,
            author,
            no_search,
        } => handle_book(&title, author.as_deref(), no_search, &scraper, &llm, &args).await,
    };

    match summary_result {
        Ok((title, summary)) => {
            if let Some(output_path) = &args.output {
                fs::write(output_path, &summary)
                    .with_context(|| format!("Failed to write output to {:?}", output_path))?;
                Ui::success(&format!(
                    "Summary successfully saved to {}",
                    output_path.display()
                ));
            } else {
                Ui::header(&format!("Summary: {}", title));
                println!("{}\n", summary);
            }
        }
        Err(err) => {
            Ui::error(&format!("Summarization failed: {:#}", err));
            std::process::exit(1);
        }
    }

    Ok(())
}

async fn handle_web(
    url: &str,
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<(String, String)> {
    let spinner = Ui::spinner("Scraping and normalizing webpage content...");
    let page = match scraper.scrape(url).await {
        Ok(p) => {
            spinner.finish_with_message("Webpage content scraped and normalized.");
            p
        }
        Err(e) => {
            spinner.abandon();
            return Err(e);
        }
    };

    let title_display = page
        .title
        .clone()
        .unwrap_or_else(|| url.to_string());

    Ui::info(&format!(
        "Extracted {} words from '{}' ({})",
        page.word_count, title_display, page.url
    ));

    let spinner = Ui::spinner("Generating summary with local LLM...");
    let summary = match llm
        .summarize_webpage(page.title.as_deref(), url, &page.content, args.style)
        .await
    {
        Ok(s) => {
            spinner.finish_with_message("Summary generated.");
            s
        }
        Err(e) => {
            spinner.abandon();
            return Err(e);
        }
    };

    Ok((title_display, summary))
}

async fn handle_book(
    title: &str,
    author: Option<&str>,
    no_search: bool,
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<(String, String)> {
    let book_display = match author {
        Some(auth) => format!("\"{}\" by {}", title, auth),
        None => format!("\"{}\"", title),
    };

    // If search is enabled (default), attempt web search first
    if !no_search {
        let spinner = Ui::spinner("Searching web for book synopsis (Wikipedia / DuckDuckGo)...");
        let searcher = BookSearcher::new()?;

        let candidates = searcher.search_candidates(title, author).await;

        if !candidates.is_empty() {
            spinner.finish_with_message(format!("Found {} search candidate(s) online.", candidates.len()));

            let mut fallback_snippet = None;
            let mut fallback_url = None;

            for res in &candidates {
                Ui::info(&format!("Attempting reference: {} ({})", res.title, res.url));
                let scrape_spinner = Ui::spinner("Scraping reference page...");

                match scraper.scrape(&res.url).await {
                    Ok(page) => {
                        scrape_spinner.finish_with_message("Reference page scraped.");
                        Ui::info(&format!(
                            "Grounded reference text: {} words (budgeted to 3,500 words for LLM context)",
                            page.word_count
                        ));

                        let llm_spinner = Ui::spinner("Synthesizing grounded book summary with local LLM...");
                        match llm
                            .summarize_book_with_context(
                                title,
                                author,
                                &res.url,
                                &page.content,
                                args.style,
                            )
                            .await
                        {
                            Ok(summary) => {
                                llm_spinner.finish_with_message("Grounded summary complete.");
                                return Ok((book_display, summary));
                            }
                            Err(e) => {
                                llm_spinner.abandon();
                                return Err(e);
                            }
                        }
                    }
                    Err(err) => {
                        scrape_spinner.abandon();
                        Ui::warn(&format!(
                            "Could not scrape {}: {}. Trying next reference...",
                            res.url, err
                        ));
                        if fallback_snippet.is_none() && res.snippet.is_some() {
                            fallback_snippet = res.snippet.clone();
                            fallback_url = Some(res.url.clone());
                        }
                    }
                }
            }

            // If scraping all candidate pages failed, try using search snippet
            if let (Some(snippet), Some(url)) = (fallback_snippet, fallback_url) {
                Ui::info("Using search snippet as grounded context...");
                let llm_spinner = Ui::spinner("Synthesizing summary from snippet...");
                if let Ok(summary) = llm
                    .summarize_book_with_context(title, author, &url, &snippet, args.style)
                    .await
                {
                    llm_spinner.finish_with_message("Grounded summary complete.");
                    return Ok((book_display, summary));
                }
                llm_spinner.abandon();
            }

            Ui::warn("All online references failed to scrape. Falling back to LLM internal memory.");
        } else {
            spinner.finish_with_message("No web results found.");
            Ui::warn("No web synopsis found online. Falling back to LLM trained memory.");
        }
    } else {
        Ui::info("Web search disabled. Using local LLM memory directly.");
    }

    // Fallback: summarize from local LLM trained memory
    let spinner = Ui::spinner("Generating book summary from LLM trained knowledge...");
    let summary = match llm.summarize_book_from_memory(title, author, args.style).await {
        Ok(s) => {
            spinner.finish_with_message("Summary generated from LLM memory.");
            s
        }
        Err(e) => {
            spinner.abandon();
            return Err(e);
        }
    };

    Ok((book_display, summary))
}
