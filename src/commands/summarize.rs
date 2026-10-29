use anyhow::{Context, Result};
use crate::cache::{Cache, CachedSummary, SetBookSummaryParams};
use crate::cli::Cli;
use crate::llm::{self, LlmClient, LlmOutput, StreamMode};
use crate::scraper::{ScrapedPage, WebScraper};
use crate::search::{BookSearcher, SearchResult};
use crate::ui::Ui;
use serde::Serialize;
use std::io::{self, IsTerminal, Write};

pub use crate::sources::document::MAX_FILE_BYTES;
pub const MAX_STDIN_BYTES: usize = 10 * 1024 * 1024; // 10 MB limit for standard input

#[derive(Serialize)]
pub struct SummaryOutcome {
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub target_type: &'static str,
    pub summary: String,
    pub word_count: usize,
    pub streamed_to_stdout: bool,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub cached: bool,
}

impl SummaryOutcome {
    pub fn from_llm(
        target_type: &'static str,
        title: String,
        author: Option<String>,
        url: Option<String>,
        word_count: usize,
        output: LlmOutput,
    ) -> Self {
        if let Some(warn) = &output.warning {
            Ui::warn(warn);
        }
        Self {
            title,
            author,
            url,
            target_type,
            summary: output.content,
            word_count,
            streamed_to_stdout: output.streamed_to_stdout,
            prompt_tokens: output.prompt_tokens,
            completion_tokens: output.completion_tokens,
            duration_ms: output.duration_ms,
            tokens_per_second: output.tokens_per_second,
            cached: false,
        }
    }

    pub fn from_cached(
        target_type: &'static str,
        cached: CachedSummary,
        author: Option<String>,
        url: Option<String>,
    ) -> Self {
        Self {
            title: cached.title,
            author,
            url,
            target_type,
            summary: cached.summary,
            word_count: 0,
            streamed_to_stdout: false,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 0,
            tokens_per_second: None,
            cached: true,
        }
    }
}

pub async fn fetch_or_cache_page(
    url: &str,
    scraper: &WebScraper,
    cache: &Cache,
    cache_hit_msg: &str,
    spinner_msg: &'static str,
    done_msg: &str,
) -> Result<ScrapedPage> {
    if let Some(cached_page) = cache.get_page(url) {
        Ui::info(cache_hit_msg);
        return Ok(cached_page);
    }
    let page = Ui::with_spinner(spinner_msg, done_msg, scraper.scrape(url)).await?;
    cache.set_page(url, &page);
    Ok(page)
}

pub async fn handle_file(
    path: &std::path::Path,
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let doc = crate::sources::DocumentLoader::load_file(path, scraper)?;

    Ui::info(&format!(
        "Loaded {} words from file '{}'",
        doc.word_count,
        path.display()
    ));

    let mode = args.stream_mode(&doc.title);
    let path_str = path.display().to_string();
    let output = llm
        .summarize_document_counted(
            &doc.title,
            Some(&path_str),
            &doc.content,
            Some(doc.word_count),
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        "file",
        doc.title,
        None,
        Some(path_str),
        doc.word_count,
        output,
    ))
}

pub async fn handle_stdin(
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<SummaryOutcome> {
    use std::io::Read;
    let mut raw = String::new();
    let mut stdin_limited = io::stdin().take((MAX_STDIN_BYTES + 1) as u64);
    stdin_limited
        .read_to_string(&mut raw)
        .context("Failed to read from standard input")?;

    if raw.len() > MAX_STDIN_BYTES {
        anyhow::bail!(
            "Standard input exceeded security limit of {} MB",
            MAX_STDIN_BYTES / (1024 * 1024)
        );
    }

    if raw.trim().is_empty() {
        anyhow::bail!("Standard input was empty.");
    }

    let is_html = raw.contains("<!DOCTYPE") || raw.contains("<html") || raw.contains("<body");
    let (content, word_count, title) = if is_html
        && let Ok(norm) = scraper.normalize("stdin", &raw)
    {
        let t = norm.title.unwrap_or_else(|| "Standard Input".to_string());
        (norm.content, norm.word_count, t)
    } else {
        let words = raw.split_whitespace().count();
        (raw, words, "Standard Input".to_string())
    };

    Ui::info(&format!("Read {} words from standard input", word_count));

    let mode = args.stream_mode(&title);
    let output = llm
        .summarize_document_counted(
            &title,
            Some("stdin"),
            &content,
            Some(word_count),
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        "stdin",
        title,
        None,
        None,
        word_count,
        output,
    ))
}

pub async fn handle_web(
    url: &str,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let is_youtube = crate::sources::youtube::extract_youtube_video_id(url);
    let is_pdf = url.split('?').next().unwrap_or(url).ends_with(".pdf");
    let target_type = if is_youtube.is_some() {
        "youtube"
    } else if is_pdf {
        "file"
    } else {
        "web"
    };

    // 1. Check for a cached summary matching URL + style + model
    if let Some(cached) = cache.get_web_summary(url, args.style, &args.model) {
        Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            target_type,
            cached,
            None,
            Some(url.to_string()),
        ));
    }

    // 2. Fetch or load cached page / transcript / PDF
    let page = if let Some(video_id) = is_youtube {
        if let Some(cached_page) = cache.get_page(url) {
            Ui::info("Loaded YouTube transcript from local cache.");
            cached_page
        } else {
            let fetched = Ui::with_spinner(
                "Fetching YouTube video metadata and captions...",
                "YouTube transcript and metadata fetched.",
                crate::sources::youtube::fetch_youtube_transcript(scraper.client(), video_id),
            )
            .await?;
            cache.set_page(url, &fetched);
            fetched
        }
    } else if is_pdf {
        if let Some(cached_page) = cache.get_page(url) {
            Ui::info("Loaded PDF content from local cache.");
            cached_page
        } else {
            let fetched = Ui::with_spinner(
                "Downloading and extracting PDF text...",
                "PDF text layer extracted.",
                async {
                    let resp = scraper
                        .client()
                        .get(url)
                        .send()
                        .await
                        .with_context(|| format!("Failed to download PDF from {}", url))?;
                    let bytes = resp.bytes().await?;
                    crate::sources::pdf::PdfExtractor::extract_from_bytes(&bytes, None)
                },
            )
            .await?;
            cache.set_page(url, &fetched);
            fetched
        }
    } else {
        fetch_or_cache_page(
            url,
            scraper,
            cache,
            "Loaded webpage content from local cache.",
            "Scraping and normalizing webpage content...",
            "Webpage content scraped and normalized.",
        )
        .await?
    };

    let title_display = page.title.clone().unwrap_or_else(|| url.to_string());

    Ui::info(&format!(
        "Extracted {} words from '{}' ({})",
        page.word_count, title_display, page.url
    ));

    let mode = args.stream_mode(&title_display);
    let output = llm
        .summarize_webpage(
            page.title.as_deref(),
            url,
            &page.content,
            page.word_count,
            args.style,
            mode,
        )
        .await?;

    if !output.truncated {
        cache.set_web_summary(
            url,
            args.style,
            &args.model,
            &title_display,
            &output.content,
        );
    }

    Ok(SummaryOutcome::from_llm(
        target_type,
        title_display,
        None,
        Some(url.to_string()),
        page.word_count,
        output,
    ))
}

pub async fn handle_news(
    topic: &str,
    timeframe: crate::cli::NewsTimeframe,
    limit: usize,
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let news_client = crate::news::GoogleNewsClient::with_client(scraper.client().clone());

    Ui::info(&format!(
        "Searching Google News for '{}' ({}, max articles: {})...",
        topic,
        timeframe.display_label(),
        limit
    ));

    let pb = Ui::spinner("Fetching and reading news articles...");
    let sources_res = news_client
        .collect_news_articles(topic, timeframe, limit, scraper)
        .await;
    pb.finish_and_clear();

    let sources = sources_res?;

    Ui::success(&format!(
        "Aggregated {} news article(s) on '{}' for briefing synthesis.",
        sources.len(),
        topic
    ));

    let mut digest = String::new();
    let mut total_words = 0;
    for (i, src) in sources.iter().enumerate() {
        total_words += src.word_count;
        digest.push_str(&format!(
            "### Article {}: {}\n- Publisher: {}\n- URL: {}\n",
            i + 1,
            src.title,
            src.source_name,
            src.url
        ));
        if let Some(date) = &src.pub_date {
            digest.push_str(&format!("- Published: {}\n", date));
        }
        digest.push_str(&format!("\n{}\n\n---\n\n", src.content));
    }

    let title_display = format!("News: {}", topic);
    let mode = args.stream_mode(&title_display);
    let author_str = format!("{} news sources", sources.len());
    let search_url = format!(
        "https://news.google.com/search?q={}",
        urlencoding::encode(topic)
    );

    let output = llm
        .summarize_news_digest(
            topic,
            timeframe.display_label(),
            &digest,
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        "news",
        title_display,
        Some(author_str),
        Some(search_url),
        total_words,
        output,
    ))
}

pub async fn handle_book(
    title: &str,
    author: Option<&str>,
    no_search: bool,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let book_display = match author {
        Some(auth) => format!("\"{}\" by {}", title, auth),
        None => format!("\"{}\"", title),
    };

    // 1. Check for a cached summary matching title + author + style + model + search mode
    if let Some(cached) =
        cache.get_book_summary(title, author, args.style, &args.model, no_search)
    {
        Ui::info("Loaded book summary from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            "book",
            cached,
            author.map(|a| a.to_string()),
            None,
        ));
    }

    let mode = args.stream_mode(&book_display);
    let query = BookQuery {
        title,
        author,
        no_search,
    };

    // 2. Generate summary (via grounded web search or LLM memory fallback)
    let (output, word_count, source_url) =
        generate_book_summary(&query, scraper, llm, cache, args, mode).await?;

    // 3. Cache only complete, non-truncated summaries (zero-copy borrow of title & summary)
    if !output.truncated {
        cache.set_book_summary(SetBookSummaryParams {
            title,
            author,
            style: args.style,
            model: &args.model,
            no_search,
            display_title: &book_display,
            summary: &output.content,
        });
    }

    Ok(SummaryOutcome::from_llm(
        "book",
        book_display,
        author.map(|a| a.to_string()),
        source_url,
        word_count,
        output,
    ))
}

struct BookQuery<'a> {
    title: &'a str,
    author: Option<&'a str>,
    no_search: bool,
}

async fn resolve_book_identity(
    title: &str,
    author: Option<&str>,
    cache: &Cache,
    searcher: &mut BookSearcher,
) -> (String, Option<String>) {
    if let Some(auth) = author {
        return (title.to_string(), Some(auth.to_string()));
    }

    let meta_opt = if let Some(cached_meta) = cache.get_book_metadata(title, None) {
        cached_meta
    } else {
        let fetched = searcher.resolve_book_metadata(title, None).await;
        cache.set_book_metadata(title, None, &fetched);
        fetched
    };

    if let Some(meta) = meta_opt {
        let year_str = meta
            .first_publish_year
            .map(|y| format!(" ({})", y))
            .unwrap_or_default();
        if let Some(ref a) = meta.author {
            Ui::info(&format!(
                "Identified book: \"{}\" by {}{}",
                meta.canonical_title, a, year_str
            ));
        }
        (meta.canonical_title, meta.author)
    } else {
        (title.to_string(), None)
    }
}

async fn fetch_or_cache_candidates(
    effective_title: &str,
    effective_author: Option<&str>,
    cache: &Cache,
    searcher: &mut BookSearcher,
) -> Result<Vec<SearchResult>> {
    if let Some(cached_candidates) = cache.get_search_results(effective_title, effective_author) {
        Ui::info(&format!(
            "Loaded {} search candidate(s) from local cache.",
            cached_candidates.len()
        ));
        return Ok(cached_candidates);
    }

    let spinner = Ui::spinner("Searching web for book synopsis (Wikipedia / DuckDuckGo)...");
    let results = searcher
        .search_candidates(effective_title, effective_author)
        .await;
    if !results.is_empty() {
        Ui::finish_spinner(
            &spinner,
            &format!("Found {} search candidate(s) online.", results.len()),
        );
        cache.set_search_results(effective_title, effective_author, &results);
    } else {
        Ui::finish_spinner(&spinner, "No web results found.");
    }
    Ok(results)
}

fn disambiguate_candidates_if_needed(
    effective_title: &str,
    ordered_candidates: &mut Vec<SearchResult>,
    args: &Cli,
) {
    if ordered_candidates.len() <= 1
        || args.yes
        || args.quiet
        || args.json
        || !io::stdin().is_terminal()
    {
        return;
    }

    let top_score = ordered_candidates[0].score;
    let second_score = ordered_candidates[1].score;
    let is_high_confidence = top_score >= 90 && (top_score - second_score) >= 30;
    if is_high_confidence {
        return;
    }

    Ui::header(&format!("Reference Disambiguation for \"{}\"", effective_title));
    eprintln!("Multiple references found online:");
    let display_limit = ordered_candidates.len().min(4);
    for (i, cand) in ordered_candidates[..display_limit].iter().enumerate() {
        let tag = if i == 0 { " [Recommended]" } else { "" };
        eprintln!("  [{}] {}{}", i + 1, cand.title, tag);
        if let Some(snippet) = &cand.snippet {
            eprintln!("      \"{}\"", snippet);
        }
    }
    eprint!("\nSelect reference [1-{}, Enter for 1]: ", display_limit);
    let _ = io::stderr().flush();

    let mut input = String::new();
    if io::stdin().read_line(&mut input).is_ok()
        && let Ok(choice) = input.trim().parse::<usize>()
        && (1..=display_limit).contains(&choice)
    {
        let selected = ordered_candidates.remove(choice - 1);
        ordered_candidates.insert(0, selected);
    }
    eprintln!();
}

async fn generate_book_summary(
    query: &BookQuery<'_>,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
    mode: StreamMode<'_>,
) -> Result<(LlmOutput, usize, Option<String>)> {
    if !query.no_search {
        let mut searcher = BookSearcher::with_client(scraper.client().clone());
        let (resolved_title, resolved_author) =
            resolve_book_identity(query.title, query.author, cache, &mut searcher).await;

        let effective_title = resolved_title.as_str();
        let effective_author = resolved_author.as_deref().or(query.author);

        let mut candidates = fetch_or_cache_candidates(
            effective_title,
            effective_author,
            cache,
            &mut searcher,
        )
        .await?;

        if !candidates.is_empty() {
            disambiguate_candidates_if_needed(effective_title, &mut candidates, args);

            let mut fallback_snippet = None;
            let mut fallback_url = None;

            for res in &candidates {
                Ui::info(&format!("Attempting reference: {} ({})", res.title, res.url));

                let page_result = fetch_or_cache_page(
                    &res.url,
                    scraper,
                    cache,
                    "Loaded reference page from local cache.",
                    "Scraping reference page...",
                    "Reference page scraped.",
                )
                .await;

                match page_result {
                    Ok(page) => {
                        Ui::info(&format!(
                            "Grounded reference text: {} words (budgeted up to {} words for 32k LLM context)",
                            page.word_count,
                            llm::MAX_CONTEXT_WORDS
                        ));

                        let output = llm
                            .summarize_book_with_context(
                                effective_title,
                                effective_author,
                                &res.url,
                                &page.content,
                                args.style,
                                mode,
                            )
                            .await?;
                        return Ok((output, page.word_count, Some(res.url.clone())));
                    }
                    Err(err) => {
                        Ui::warn(&format!(
                            "Could not scrape {}: {}. Trying next reference...",
                            res.url, err
                        ));
                        if fallback_snippet.is_none() && let Some(snippet) = &res.snippet {
                            fallback_snippet = Some(snippet.clone());
                            fallback_url = Some(res.url.clone());
                        }
                    }
                }
            }

            // If scraping all candidate pages failed, try using search snippet
            if let (Some(snippet), Some(url)) = (fallback_snippet, fallback_url) {
                Ui::info("Using search snippet as grounded context...");
                let word_count = snippet.split_whitespace().count();
                if let Ok(output) = llm
                    .summarize_book_with_context(
                        effective_title,
                        effective_author,
                        &url,
                        &snippet,
                        args.style,
                        mode,
                    )
                    .await
                {
                    return Ok((output, word_count, Some(url)));
                }
            }

            Ui::warn("All online references failed to scrape. Falling back to LLM internal memory.");
        } else {
            Ui::warn("No web synopsis found online. Falling back to LLM trained memory.");
        }
    } else {
        Ui::info("Web search disabled. Using local LLM memory directly.");
    }

    let output = llm
        .summarize_book_from_memory(query.title, query.author, args.style, mode)
        .await?;
    Ok((output, 0, None))
}
