use anyhow::{Context, Result};
use clap::Parser;
use srz::cache::{Cache, CachedSummary};
use srz::cli::{Cli, ResolvedAction};
use srz::llm::{self, LlmClient, LlmOutput};
use srz::scraper::WebScraper;
use srz::search::BookSearcher;
use srz::ui::Ui;
use std::fs;

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
        let p = Ui::with_spinner(
            "Scraping and normalizing webpage content...",
            "Webpage content scraped and normalized.",
            scraper.scrape(url),
        )
        .await?;
        cache.set_page(url, &p);
        p
    };

    let title_display = page.title.clone().unwrap_or_else(|| url.to_string());

    Ui::info(&format!(
        "Extracted {} words from '{}' ({})",
        page.word_count, title_display, page.url
    ));

    let output = Ui::with_spinner(
        "Generating summary with local LLM...",
        "Summary generated.",
        llm.summarize_webpage(page.title.as_deref(), url, &page.content, args.style),
    )
    .await?;

    if let Some(warn) = &output.warning {
        Ui::warn(warn);
    }

    if !output.truncated {
        cache.set_web_summary(
            url,
            args.style,
            &args.model,
            &CachedSummary {
                title: title_display.clone(),
                summary: output.content.clone(),
            },
        );
    }

    Ok((title_display, output.content))
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

    // 2. Generate summary (via grounded web search or LLM memory fallback)
    let output =
        generate_book_summary(title, author, no_search, scraper, llm, cache, args).await?;

    if let Some(warn) = &output.warning {
        Ui::warn(warn);
    }

    // 3. Cache only complete, non-truncated summaries
    if !output.truncated {
        cache.set_book_summary(
            title,
            author,
            args.style,
            &args.model,
            no_search,
            &CachedSummary {
                title: book_display.clone(),
                summary: output.content.clone(),
            },
        );
    }

    Ok((book_display, output.content))
}

async fn generate_book_summary(
    title: &str,
    author: Option<&str>,
    no_search: bool,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<LlmOutput> {
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
                Ui::finish_spinner(
                    &spinner,
                    &format!("Found {} search candidate(s) online.", results.len()),
                );
                cache.set_search_results(title, author, &results);
            } else {
                Ui::finish_spinner(&spinner, "No web results found.");
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
                    match Ui::with_spinner(
                        "Scraping reference page...",
                        "Reference page scraped.",
                        scraper.scrape(&res.url),
                    )
                    .await
                    {
                        Ok(page) => {
                            cache.set_page(&res.url, &page);
                            Ok(page)
                        }
                        Err(err) => Err(err),
                    }
                };

                match page_result {
                    Ok(page) => {
                        Ui::info(&format!(
                            "Grounded reference text: {} words (budgeted up to {} words for 32k LLM context)",
                            page.word_count,
                            llm::MAX_CONTEXT_WORDS
                        ));

                        return Ui::with_spinner(
                            "Synthesizing grounded book summary with local LLM...",
                            "Grounded summary complete.",
                            llm.summarize_book_with_context(
                                title,
                                author,
                                &res.url,
                                &page.content,
                                args.style,
                            ),
                        )
                        .await;
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
                if let Ok(output) = Ui::with_spinner(
                    "Synthesizing summary from snippet...",
                    "Grounded summary complete.",
                    llm.summarize_book_with_context(title, author, &url, &snippet, args.style),
                )
                .await
                {
                    return Ok(output);
                }
            }

            Ui::warn("All online references failed to scrape. Falling back to LLM internal memory.");
        } else {
            Ui::warn("No web synopsis found online. Falling back to LLM trained memory.");
        }
    } else {
        Ui::info("Web search disabled. Using local LLM memory directly.");
    }

    Ui::with_spinner(
        "Generating book summary from LLM trained knowledge...",
        "Summary generated from LLM memory.",
        llm.summarize_book_from_memory(title, author, args.style),
    )
    .await
}
