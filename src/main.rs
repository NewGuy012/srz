mod cache;
mod chunker;
mod cli;
mod llm;
mod scraper;
mod search;
mod ui;

use anyhow::{Context, Result};
use cache::{Cache, CachedSummary};
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

    let cache = Cache::new(!args.no_cache);
    if args.no_cache {
        Ui::info("Cache bypassed (`--no-cache`). Forcing fresh fetch and generation.");
    }

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
        ResolvedAction::Web { url } => handle_web(&url, &scraper, &llm, &cache, &args).await,
        ResolvedAction::Book {
            title,
            author,
            no_search,
        } => {
            handle_book(
                &title,
                author.as_deref(),
                no_search,
                &scraper,
                &llm,
                &cache,
                &args,
            )
            .await
        }
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
    cache: &Cache,
    args: &Cli,
) -> Result<(String, String)> {
    // 1. Check for a cached summary matching URL + style + model
    if let Some(cached) = cache.get_web_summary(url, args.style, &args.model) {
        Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
        return Ok((cached.title, cached.summary));
    }

    // 2. Check for a cached scraped page to avoid redundant network requests
    let page = if let Some(cached_page) = cache.get_page(url) {
        Ui::info("Loaded webpage content from local cache.");
        cached_page
    } else {
        let spinner = Ui::spinner("Scraping and normalizing webpage content...");
        match scraper.scrape(url).await {
            Ok(p) => {
                spinner.finish_with_message("Webpage content scraped and normalized.");
                cache.set_page(url, &p);
                p
            }
            Err(e) => {
                spinner.abandon();
                return Err(e);
            }
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

    cache.set_web_summary(
        url,
        args.style,
        &args.model,
        &CachedSummary {
            title: title_display.clone(),
            summary: summary.clone(),
        },
    );

    Ok((title_display, summary))
}

async fn handle_book(
    title: &str,
    author: Option<&str>,
    no_search: bool,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<(String, String)> {
    let book_display = match author {
        Some(auth) => format!("\"{}\" by {}", title, auth),
        None => format!("\"{}\"", title),
    };

    // 1. Check for a cached summary matching title + author + style + model + search mode
    if let Some(cached) =
        cache.get_book_summary(title, author, args.style, &args.model, no_search)
    {
        Ui::info("Loaded book summary from local cache (pass `--no-cache` to refresh).");
        return Ok((cached.title, cached.summary));
    }

    // If search is enabled (default), attempt web search first
    if !no_search {
        let candidates = if let Some(cached_candidates) = cache.get_search_results(title, author) {
            Ui::info(&format!(
                "Loaded {} search candidate(s) from local cache.",
                cached_candidates.len()
            ));
            cached_candidates
        } else {
            let spinner = Ui::spinner("Searching web for book synopsis (Wikipedia / DuckDuckGo)...");
            let searcher = BookSearcher::new()?;
            let results = searcher.search_candidates(title, author).await;
            if !results.is_empty() {
                spinner.finish_with_message(format!(
                    "Found {} search candidate(s) online.",
                    results.len()
                ));
                cache.set_search_results(title, author, &results);
            } else {
                spinner.finish_with_message("No web results found.");
            }
            results
        };

        if !candidates.is_empty() {
            let mut fallback_snippet = None;
            let mut fallback_url = None;

            for res in &candidates {
                Ui::info(&format!("Attempting reference: {} ({})", res.title, res.url));

                let page_result = if let Some(cached_page) = cache.get_page(&res.url) {
                    Ui::info("Loaded reference page from local cache.");
                    Ok(cached_page)
                } else {
                    let scrape_spinner = Ui::spinner("Scraping reference page...");
                    match scraper.scrape(&res.url).await {
                        Ok(page) => {
                            scrape_spinner.finish_with_message("Reference page scraped.");
                            cache.set_page(&res.url, &page);
                            Ok(page)
                        }
                        Err(err) => {
                            scrape_spinner.abandon();
                            Err(err)
                        }
                    }
                };

                match page_result {
                    Ok(page) => {
                        Ui::info(&format!(
                            "Grounded reference text: {} words (budgeted up to {} words for 32k LLM context)",
                            page.word_count,
                            llm::MAX_CONTEXT_WORDS
                        ));

                        let llm_spinner =
                            Ui::spinner("Synthesizing grounded book summary with local LLM...");
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
                                cache.set_book_summary(
                                    title,
                                    author,
                                    args.style,
                                    &args.model,
                                    no_search,
                                    &CachedSummary {
                                        title: book_display.clone(),
                                        summary: summary.clone(),
                                    },
                                );
                                return Ok((book_display, summary));
                            }
                            Err(e) => {
                                llm_spinner.abandon();
                                return Err(e);
                            }
                        }
                    }
                    Err(err) => {
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
                    cache.set_book_summary(
                        title,
                        author,
                        args.style,
                        &args.model,
                        no_search,
                        &CachedSummary {
                            title: book_display.clone(),
                            summary: summary.clone(),
                        },
                    );
                    return Ok((book_display, summary));
                }
                llm_spinner.abandon();
            }

            Ui::warn("All online references failed to scrape. Falling back to LLM internal memory.");
        } else {
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

    cache.set_book_summary(
        title,
        author,
        args.style,
        &args.model,
        no_search,
        &CachedSummary {
            title: book_display.clone(),
            summary: summary.clone(),
        },
    );

    Ok((book_display, summary))
}
