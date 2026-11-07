use crate::cache::{Cache, CachedSummary, SetBookSummaryParams};
use crate::cli::Cli;
use crate::llm::{self, LlmClient, LlmOutput, StreamMode};
use crate::scraper::{ScrapedPage, WebScraper};
use crate::search::{BookSearcher, SearchResult};
use crate::sources::document::DocumentLoader;
use crate::ui::Ui;
use anyhow::{Context, Result};
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
    pub reasoning: Option<String>,
    pub word_count: usize,
    pub streamed_to_stdout: bool,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub cached: bool,
    pub hint: Option<String>,
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
            reasoning: output.reasoning,
            word_count,
            streamed_to_stdout: output.streamed_to_stdout,
            prompt_tokens: output.prompt_tokens,
            completion_tokens: output.completion_tokens,
            duration_ms: output.duration_ms,
            tokens_per_second: output.tokens_per_second,
            cached: false,
            hint: output.hint,
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
            reasoning: cached.reasoning,
            word_count: cached.word_count,
            streamed_to_stdout: false,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 0,
            tokens_per_second: None,
            cached: true,
            hint: None,
        }
    }
}

pub async fn fetch_or_cache_page_with<F>(
    url: &str,
    cache: &Cache,
    cache_hit_msg: &str,
    spinner_msg: &'static str,
    done_msg: &str,
    fetch_fut: F,
) -> Result<ScrapedPage>
where
    F: std::future::Future<Output = Result<ScrapedPage>>,
{
    if let Some(cached_page) = cache.get_page(url) {
        if !cache_hit_msg.is_empty() {
            Ui::info(cache_hit_msg);
        }
        return Ok(cached_page);
    }
    let page = if spinner_msg.is_empty() {
        fetch_fut.await?
    } else {
        Ui::with_spinner(spinner_msg, done_msg, fetch_fut).await?
    };
    cache.set_page(url, &page);
    Ok(page)
}

pub async fn fetch_or_cache_page(
    url: &str,
    scraper: &WebScraper,
    cache: &Cache,
    cache_hit_msg: &str,
    spinner_msg: &'static str,
    done_msg: &str,
) -> Result<ScrapedPage> {
    fetch_or_cache_page_with(
        url,
        cache,
        cache_hit_msg,
        spinner_msg,
        done_msg,
        scraper.scrape(url),
    )
    .await
}

/// Returns true if the URL points to a remote PDF file (case-insensitive, ignoring query parameters and fragments).
pub fn is_pdf_url(url: &str) -> bool {
    let clean = url.split(['?', '#']).next().unwrap_or(url);
    clean.ends_with(".pdf") || clean.ends_with(".PDF")
}

/// Determines the target type label ("youtube", "file", or "web") for a given URL.
pub fn target_type_for_url(url: &str) -> &'static str {
    if crate::sources::youtube::extract_youtube_video_id(url).is_some() {
        "youtube"
    } else if is_pdf_url(url) {
        "file"
    } else {
        "web"
    }
}

/// Dispatches URL fetching according to destination type (YouTube transcript, remote PDF, or webpage scrape)
/// and caches the resulting content.
pub async fn fetch_url_content(
    url: &str,
    scraper: &WebScraper,
    cache: &Cache,
    verbose: bool,
) -> Result<ScrapedPage> {
    let is_youtube = crate::sources::youtube::extract_youtube_video_id(url);
    let is_pdf = is_pdf_url(url);

    let (cache_msg, spinner_msg, done_msg) = if verbose {
        if is_youtube.is_some() {
            (
                "Loaded YouTube transcript from local cache.",
                "Fetching YouTube video metadata and captions...",
                "YouTube transcript and metadata fetched.",
            )
        } else if is_pdf {
            (
                "Loaded PDF content from local cache.",
                "Downloading and extracting PDF text...",
                "PDF text layer extracted.",
            )
        } else {
            (
                "Loaded webpage content from local cache.",
                "Scraping and normalizing webpage content...",
                "Webpage content scraped and normalized.",
            )
        }
    } else {
        ("", "", "")
    };

    if let Some(video_id) = is_youtube {
        fetch_or_cache_page_with(
            url,
            cache,
            cache_msg,
            spinner_msg,
            done_msg,
            crate::sources::youtube::fetch_youtube_transcript(scraper.client(), video_id),
        )
        .await
    } else if is_pdf {
        fetch_or_cache_page_with(url, cache, cache_msg, spinner_msg, done_msg, async {
            let resp = scraper
                .client()
                .get(url)
                .send()
                .await
                .with_context(|| format!("Failed to download PDF from {}", url))?;
            if !resp.status().is_success() {
                anyhow::bail!(
                    "Failed to download PDF from {} (HTTP status: {})",
                    url,
                    resp.status()
                );
            }
            let bytes = crate::scraper::read_response_bytes_limited(
                resp,
                crate::sources::document::MAX_FILE_BYTES as usize,
            )
            .await?;
            crate::sources::pdf::PdfExtractor::extract_from_bytes(&bytes, None)
        })
        .await
    } else {
        fetch_or_cache_page(url, scraper, cache, cache_msg, spinner_msg, done_msg).await
    }
}

/// Generates a deterministic cache key for a local file based on canonical path, size, and mtime.
pub fn file_cache_key(path: &std::path::Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    let mtime_ms = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Some(format!(
        "{}:{}:{}",
        canonical.display(),
        meta.len(),
        mtime_ms
    ))
}

pub async fn handle_file(
    path: &std::path::Path,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let path_str = path.display().to_string();
    let file_key = file_cache_key(path);

    if let Some(ref k) = file_key
        && let Some(cached) = cache.get_file_summary(k, args.style, &args.model)
    {
        Ui::info("Loaded summary from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            "file",
            cached,
            None,
            Some(path_str),
        ));
    }

    let doc = crate::sources::DocumentLoader::load_file(path, scraper)?;

    Ui::info(&format!(
        "Loaded {} words from file '{}'",
        doc.word_count,
        path.display()
    ));

    let mode = args.stream_mode(&doc.title);
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

    if !output.truncated
        && let Some(ref k) = file_key
    {
        cache.set_file_summary(
            k,
            args.style,
            &args.model,
            &doc.title,
            &output.content,
            doc.word_count,
            output.reasoning.as_deref(),
        );
    }

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

    let (content, word_count, title) =
        DocumentLoader::normalize_or_plain_text(scraper, "stdin", raw, || {
            "Standard Input".to_string()
        });

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
        "stdin", title, None, None, word_count, output,
    ))
}

pub async fn handle_web(
    url: &str,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    if crate::cli::is_feed_url(url) {
        return handle_news(
            url,
            crate::cli::NewsTimeframe::All,
            5,
            scraper,
            llm,
            cache,
            args,
        )
        .await;
    }

    let target_type = target_type_for_url(url);

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
    let page = fetch_url_content(url, scraper, cache, true).await?;

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
        cache.set_web_summary_with_words(
            url,
            args.style,
            &args.model,
            &title_display,
            &output.content,
            page.word_count,
            output.reasoning.as_deref(),
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
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let title_display = format!("News: {}", topic);
    let is_direct_feed = crate::system::text_util::is_http_url(topic);
    let search_url = if is_direct_feed {
        topic.to_string()
    } else {
        format!(
            "https://news.google.com/search?q={}",
            crate::system::text_util::url_encode(topic)
        )
    };

    // 1. Check for a cached news briefing matching topic + timeframe + style + model within news TTL (default: 2h)
    if let Some(cached) = cache.get_news_summary(topic, timeframe, args.style, &args.model) {
        Ui::info("Loaded news briefing from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            "news",
            cached,
            None,
            Some(search_url),
        ));
    }

    let news_client = crate::news::GoogleNewsClient::with_client(scraper.client().clone());

    if is_direct_feed {
        Ui::info(&format!(
            "Fetching syndication feed '{}' (max articles: {})...",
            topic, limit
        ));
    } else {
        Ui::info(&format!(
            "Searching Google News for '{}' ({}, max articles: {})...",
            topic,
            timeframe.display_label(),
            limit
        ));
    }

    let pb = Ui::spinner("Fetching and reading news articles...");
    let sources = match news_client
        .collect_news_articles_with_cache(topic, timeframe, limit, scraper, Some(cache))
        .await
    {
        Ok(sources) => {
            Ui::finish_spinner(
                &pb,
                &format!(
                    "Aggregated {} news article(s) on '{}' for briefing synthesis.",
                    sources.len(),
                    topic
                ),
            );
            sources
        }
        Err(err) => {
            pb.finish_and_clear();
            return Err(err);
        }
    };

    let mut digest = String::new();
    let mut total_words = 0;
    for (i, src) in sources.iter().enumerate() {
        total_words += src.word_count;
        let safe_title = crate::ui::sanitize_title(&src.title);
        let safe_publisher = crate::ui::sanitize_title(&src.source_name);
        let safe_url = crate::ui::sanitize_title(&src.url);
        digest.push_str(&format!(
            "### Article {}: {}\n- Publisher: {}\n- URL: {}\n",
            i + 1,
            safe_title,
            safe_publisher,
            safe_url
        ));
        if let Some(date) = &src.pub_date {
            let safe_date = crate::ui::sanitize_title(date);
            digest.push_str(&format!("- Published: {}\n", safe_date));
        }
        digest.push_str(&format!("\n{}\n\n---\n\n", src.content));
    }

    let mode = args.stream_mode(&title_display);
    let author_str = format!("{} news sources", sources.len());
    let output = llm
        .summarize_news_digest(topic, timeframe.display_label(), &digest, args.style, mode)
        .await?;

    if !output.truncated {
        cache.set_news_summary(
            topic,
            timeframe,
            args.style,
            &args.model,
            &title_display,
            &output.content,
            total_words,
            output.reasoning.as_deref(),
        );
    }

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
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let book_display = match author {
        Some(auth) => format!("\"{}\" by {}", title, auth),
        None => format!("\"{}\"", title),
    };

    // 1. Check for a cached summary matching title + author + style + model
    if let Some(cached) = cache.get_book_summary(title, author, args.style, &args.model) {
        Ui::info("Loaded book summary from local cache (pass `--no-cache` to refresh).");
        return Ok(SummaryOutcome::from_cached(
            "book",
            cached,
            author.map(|a| a.to_string()),
            None,
        ));
    }

    let mode = args.stream_mode(&book_display);
    let query = BookQuery { title, author };

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
            display_title: &book_display,
            summary: &output.content,
            word_count,
            reasoning: output.reasoning.as_deref(),
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
}

async fn resolve_book_and_candidates_concurrently(
    query_title: &str,
    query_author: Option<&str>,
    cache: &Cache,
    searcher: &BookSearcher,
) -> Result<(String, Option<String>, Vec<SearchResult>)> {
    if let Some(auth) = query_author {
        let effective_title = query_title.to_string();
        let effective_author = Some(auth.to_string());
        let mut searcher_mut = searcher.clone();
        let candidates = fetch_or_cache_candidates(
            &effective_title,
            effective_author.as_deref(),
            cache,
            &mut searcher_mut,
        )
        .await?;
        return Ok((effective_title, effective_author, candidates));
    }

    // Check if metadata is already cached
    let cached_meta = cache.get_book_metadata(query_title, None);
    if let Some(Some(meta)) = cached_meta {
        let effective_title = meta.canonical_title;
        let effective_author = meta.author;
        let year_str = meta
            .first_publish_year
            .map(|y| format!(" ({})", y))
            .unwrap_or_default();
        if let Some(ref a) = effective_author {
            Ui::info(&format!(
                "Identified book: \"{}\" by {}{}",
                effective_title, a, year_str
            ));
        }
        let mut searcher_mut = searcher.clone();
        let candidates = fetch_or_cache_candidates(
            &effective_title,
            effective_author.as_deref(),
            cache,
            &mut searcher_mut,
        )
        .await?;
        return Ok((effective_title, effective_author, candidates));
    }

    // Check if candidates are already cached under the query title
    if let Some(cached_candidates) = cache.get_search_results(query_title, None) {
        Ui::info(&format!(
            "Loaded {} search candidate(s) from local cache.",
            cached_candidates.len()
        ));
        return Ok((query_title.to_string(), None, cached_candidates));
    }

    // Fan-out Open Library metadata resolution AND candidate search concurrently
    let spinner = Ui::spinner(
        "Searching web for book synopsis & metadata (Wikipedia / DDG / Open Library)...",
    );
    let meta_fut = searcher.resolve_book_metadata(query_title, None);
    let candidates_fut = searcher.search_raw_candidates(query_title, None);

    let (meta_opt, mut raw_candidates) = tokio::join!(meta_fut, candidates_fut);

    let (effective_title, effective_author) = if let Some(meta) = meta_opt {
        cache.set_book_metadata(query_title, None, &Some(meta.clone()));
        let year_str = meta
            .first_publish_year
            .map(|y| format!(" ({})", y))
            .unwrap_or_default();
        if let Some(ref a) = meta.author {
            Ui::finish_spinner(
                &spinner,
                &format!(
                    "Identified \"{}\" by {}{}",
                    meta.canonical_title, a, year_str
                ),
            );
        } else {
            Ui::finish_spinner(
                &spinner,
                &format!("Identified \"{}\"", meta.canonical_title),
            );
        }
        (meta.canonical_title, meta.author)
    } else {
        if !raw_candidates.is_empty() {
            Ui::finish_spinner(
                &spinner,
                &format!("Found {} search candidate(s) online.", raw_candidates.len()),
            );
        } else {
            Ui::finish_spinner(&spinner, "No web results found.");
        }
        (query_title.to_string(), None)
    };

    crate::search::rank_candidates(
        &effective_title,
        effective_author.as_deref(),
        &mut raw_candidates,
    );
    if !raw_candidates.is_empty() {
        cache.set_search_results(
            &effective_title,
            effective_author.as_deref(),
            &raw_candidates,
        );
    }

    Ok((effective_title, effective_author, raw_candidates))
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
    if ordered_candidates.len() <= 1 || args.quiet || args.json || !io::stdin().is_terminal() {
        return;
    }

    let top_score = ordered_candidates[0].score;
    let second_score = ordered_candidates[1].score;
    let is_high_confidence = top_score >= 90 && (top_score - second_score) >= 30;
    if is_high_confidence {
        return;
    }

    Ui::header(&format!(
        "Reference Disambiguation for \"{}\"",
        crate::ui::sanitize_title(effective_title)
    ));
    eprintln!("Multiple references found online:");
    let display_limit = ordered_candidates.len().min(4);
    for (i, cand) in ordered_candidates[..display_limit].iter().enumerate() {
        let tag = if i == 0 { " [Recommended]" } else { "" };
        let safe_title = crate::ui::sanitize_title(&cand.title);
        eprintln!("  [{}] {}{}", i + 1, safe_title, tag);
        if let Some(snippet) = &cand.snippet {
            let safe_snippet = crate::ui::sanitize_terminal_text(snippet);
            eprintln!("      \"{}\"", safe_snippet);
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
    let searcher = BookSearcher::with_client(scraper.client().clone());
    let (resolved_title, resolved_author, mut candidates) =
        resolve_book_and_candidates_concurrently(query.title, query.author, cache, &searcher)
            .await?;

    let effective_title = resolved_title.as_str();
    let effective_author = resolved_author.as_deref().or(query.author);

    if !candidates.is_empty() {
        disambiguate_candidates_if_needed(effective_title, &mut candidates, args);

        let mut fallback_snippet = None;
        let mut fallback_url = None;

        for res in &candidates {
            Ui::info(&format!(
                "Attempting reference: {} ({})",
                res.title, res.url
            ));

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
                    let (content_to_use, words_to_use) = if res.url.contains("wikipedia.org/wiki/")
                    {
                        if let Some(synopsis) =
                            crate::search::extract_wikipedia_synopsis(&page.content)
                        {
                            let wc = synopsis.split_whitespace().count();
                            Ui::info(&format!(
                                "Extracted targeted plot synopsis: {} words (streamlined from {} full-page words)",
                                wc, page.word_count
                            ));
                            (synopsis, wc)
                        } else {
                            (page.content, page.word_count)
                        }
                    } else {
                        (page.content, page.word_count)
                    };

                    Ui::info(&format!(
                        "Grounded reference text: {} words (budgeted up to {} words for 32k LLM context)",
                        words_to_use,
                        llm::MAX_CONTEXT_WORDS
                    ));

                    let output = llm
                        .summarize_book_with_context(
                            effective_title,
                            effective_author,
                            &res.url,
                            &content_to_use,
                            args.style,
                            mode,
                        )
                        .await?;
                    return Ok((output, words_to_use, Some(res.url.clone())));
                }
                Err(err) => {
                    Ui::warn(&format!(
                        "Could not scrape {}: {}. Trying next reference...",
                        res.url, err
                    ));
                    if fallback_snippet.is_none()
                        && let Some(snippet) = &res.snippet
                    {
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

    let output = llm
        .summarize_book_from_memory(query.title, query.author, args.style, mode)
        .await?;
    Ok((output, 0, None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_type_for_url() {
        assert_eq!(
            target_type_for_url("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            "youtube"
        );
        assert_eq!(
            target_type_for_url("https://youtu.be/dQw4w9WgXcQ"),
            "youtube"
        );
        assert_eq!(
            target_type_for_url("https://example.com/paper.pdf?token=123"),
            "file"
        );
        assert_eq!(
            target_type_for_url("https://example.com/paper.PDF#page=3"),
            "file"
        );
        assert_eq!(target_type_for_url("https://example.com/article"), "web");
    }
}
