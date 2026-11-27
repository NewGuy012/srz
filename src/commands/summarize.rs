use crate::cache::{Cache, CachedSummary};
use crate::commands::{
    file_url, open_db_or_warn, print_json, read_line_bounded, update_index_dashboard,
};
use crate::config::{Cli, NewsTimeframe};
use crate::db::NewSummaryRecord;
use crate::engine::prompts::truncate_words_preserving_lines;
use crate::llm::{LlmClient, LlmOutput};
use crate::render::{
    RenderedFiles, ensure_view_files_unique, is_html_path, open_in_browser,
    render_html_document_with_reasoning,
};
use crate::scraper::{ScrapedPage, WebScraper};
use crate::search::{BookSearcher, SearchResult, extract_wikipedia_synopsis, rank_candidates};
use crate::sources::document::{DocumentLoader, MAX_FILE_BYTES, PdfExtractor};
use crate::sources::news::GoogleNewsClient;
use crate::sources::youtube::fetch_youtube_transcript;
use crate::ui::{Ui, display_summary_full, sanitize_terminal_text_cow, sanitize_title};
use anyhow::{Context, Result};
use serde::Serialize;
use std::io::{self, IsTerminal, Read, Write};
use std::path::Path;

const MAX_STDIN_BYTES: usize = 10 * 1024 * 1024;

#[derive(Serialize)]
pub(super) struct SummaryOutcome {
    pub title: String,
    pub author: Option<String>,
    pub url: Option<String>,
    pub target_type: &'static str,
    pub summary: String,
    pub reasoning: Option<String>,
    pub word_count: usize,
    #[serde(skip_serializing)]
    pub streamed_to_stdout: bool,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub cached: bool,
    pub hint: Option<String>,
    #[serde(skip_serializing)]
    pub cache_key: Option<String>,
}

impl SummaryOutcome {
    fn from_llm(
        target_type: &'static str,
        title: String,
        author: Option<String>,
        url: Option<String>,
        word_count: usize,
        output: LlmOutput,
        cache_key: Option<String>,
    ) -> Self {
        if let Some(warn) = &output.warning {
            Ui::warn(warn);
        }
        let cache_key = cache_key.filter(|_| !output.truncated);
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
            cache_key,
        }
    }

    pub(super) fn from_cached(
        target_type: &'static str,
        cached: CachedSummary,
        fallback_url: Option<String>,
    ) -> Self {
        Self {
            title: cached.title,
            author: cached.author,
            url: cached.url.or(fallback_url),
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
            cache_key: None,
        }
    }
}

pub(super) async fn fetch_or_cache_page_with<F>(
    url: &str,
    cache: &Cache,
    status_msgs: Option<(&str, &'static str, &str)>,
    fetch_fut: F,
) -> Result<ScrapedPage>
where
    F: std::future::Future<Output = Result<ScrapedPage>>,
{
    if let Some(cached_page) = cache.get_page(url) {
        if let Some((hit_msg, _, _)) = status_msgs {
            Ui::info(hit_msg);
        }
        return Ok(cached_page);
    }
    let page = match status_msgs {
        Some((_, spinner_msg, done_msg)) => {
            Ui::with_spinner(spinner_msg, done_msg, fetch_fut).await?
        }
        None => fetch_fut.await?,
    };
    cache.set_page(url, &page);
    Ok(page)
}

pub(super) fn is_pdf_url(url: &str) -> bool {
    let clean = url.split(['?', '#']).next().unwrap_or(url);
    crate::system::text_util::ends_with_ascii_case_insensitive(clean, ".pdf")
}

pub(super) fn target_type_for_url(url: &str) -> &'static str {
    if crate::sources::youtube::extract_youtube_video_id(url).is_some() {
        "youtube"
    } else if is_pdf_url(url) {
        "file"
    } else {
        "web"
    }
}

pub(super) async fn fetch_url_content(
    url: &str,
    scraper: &WebScraper,
    cache: &Cache,
    verbose: bool,
) -> Result<ScrapedPage> {
    let video_id = crate::sources::youtube::extract_youtube_video_id(url);
    let is_pdf = is_pdf_url(url);

    let status_msgs = verbose.then(|| {
        if video_id.is_some() {
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
    });

    if let Some(vid) = video_id {
        fetch_or_cache_page_with(
            url,
            cache,
            status_msgs,
            fetch_youtube_transcript(scraper.client(), vid),
        )
        .await
    } else if is_pdf {
        fetch_or_cache_page_with(url, cache, status_msgs, async {
            crate::scraper::validate_public_http_url(url)?;
            let bytes = crate::scraper::fetch_bytes_limited(
                scraper.client().get(url),
                MAX_FILE_BYTES as usize,
                &format!("PDF from {url}"),
            )
            .await?;
            let mut page =
                tokio::task::spawn_blocking(move || PdfExtractor::extract_from_bytes(&bytes, None))
                    .await
                    .context("PDF extraction task failed")??;
            page.url = url.to_string();
            Ok(page)
        })
        .await
    } else {
        fetch_or_cache_page_with(url, cache, status_msgs, scraper.scrape(url)).await
    }
}

/// Deterministic cache key based on canonical path, file size, and mtime.
pub(super) fn file_cache_key(path: &Path) -> Option<String> {
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

pub(super) fn canonical_file_url_string(path: &Path) -> String {
    std::fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .display()
        .to_string()
}

pub(super) async fn load_file_with_cache(
    path: &Path,
    scraper: &WebScraper,
    cache: &Cache,
) -> Result<(ScrapedPage, Option<String>)> {
    let file_key = file_cache_key(path);
    let doc = if let Some(ref k) = file_key
        && let Some(cached_page) = cache.get_page(k)
    {
        cached_page
    } else {
        let path_owned = path.to_path_buf();
        let scraper_owned = scraper.clone();
        let loaded = tokio::task::spawn_blocking(move || {
            DocumentLoader::load_file(&path_owned, &scraper_owned)
        })
        .await
        .context("Document loader task failed")??;
        if let Some(ref k) = file_key {
            cache.set_page(k, &loaded);
        }
        loaded
    };
    Ok((doc, file_key))
}

pub(super) async fn handle_file(
    path: &Path,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let canonical_str = canonical_file_url_string(path);
    let (doc, file_key) = load_file_with_cache(path, scraper, cache).await?;
    let title = doc.title.unwrap_or_else(|| path.display().to_string());

    Ui::info(&format!(
        "Loaded {} words from file '{}'",
        doc.word_count,
        path.display()
    ));

    let mode = args.stream_mode(&title);
    let output = llm
        .summarize_document_counted(
            &title,
            Some(&canonical_str),
            &doc.content,
            Some(doc.word_count),
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        "file",
        title,
        None,
        Some(canonical_str),
        doc.word_count,
        output,
        file_key,
    ))
}

pub(super) async fn handle_stdin(
    scraper: &WebScraper,
    llm: &LlmClient,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let scraper_owned = scraper.clone();
    let (content, word_count, title) = tokio::task::spawn_blocking(move || -> Result<_> {
        let mut raw = String::new();
        io::stdin()
            .take((MAX_STDIN_BYTES + 1) as u64)
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

        Ok(DocumentLoader::normalize_or_plain_text(
            &scraper_owned,
            "stdin",
            raw,
            || "Standard Input".to_string(),
        ))
    })
    .await
    .context("Stdin reader task failed")??;

    Ui::info(&format!("Read {word_count} words from standard input"));

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
        "stdin", title, None, None, word_count, output, None,
    ))
}

pub(super) async fn summarize_fetched_web_page(
    url: &str,
    page: ScrapedPage,
    llm: &LlmClient,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let target_type = target_type_for_url(url);
    let title_display = page.title.clone().unwrap_or_else(|| url.to_string());

    Ui::info(&format!(
        "Extracted {} words from '{}' ({})",
        page.word_count, title_display, page.url
    ));

    let mode = args.stream_mode(&title_display);
    let output = llm
        .summarize_document_counted(
            page.title.as_deref().unwrap_or("Untitled Webpage"),
            Some(url),
            &page.content,
            Some(page.word_count),
            args.style,
            mode,
        )
        .await?;

    Ok(SummaryOutcome::from_llm(
        target_type,
        title_display,
        None,
        Some(url.to_string()),
        page.word_count,
        output,
        Some(url.to_string()),
    ))
}

pub(super) async fn handle_news(
    topic: &str,
    timeframe: NewsTimeframe,
    limit: usize,
    scraper: &WebScraper,
    llm: &LlmClient,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let title_display = format!("News: {topic}");
    let is_direct_feed = GoogleNewsClient::is_direct_feed(topic);
    let search_url = GoogleNewsClient::search_url_for_topic(topic);
    let news_client = GoogleNewsClient::with_client(scraper.client().clone());

    if is_direct_feed {
        Ui::info(&format!(
            "Fetching syndication feed '{topic}' (max articles: {limit})..."
        ));
    } else {
        Ui::info(&format!(
            "Searching Google News for '{topic}' ({}, max articles: {limit})...",
            timeframe.display_label()
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
                    "Aggregated {} news article(s) on '{topic}' for briefing synthesis.",
                    sources.len()
                ),
            );
            sources
        }
        Err(err) => {
            pb.finish_and_clear();
            return Err(err);
        }
    };

    let budget = llm.budget();
    let per_article_words = if !sources.is_empty() {
        (budget.single_pass_threshold.saturating_mul(3) / 4 / sources.len()).clamp(150, 800)
    } else {
        600
    };

    let mut digest = String::new();
    let mut total_words = 0;
    for (i, src) in sources.iter().enumerate() {
        let trimmed_content = truncate_words_preserving_lines(&src.content, per_article_words);
        total_words += trimmed_content.split_whitespace().count();

        digest.push_str(&format!(
            "### Article {}: {}\n- Publisher: {}\n- URL: {}\n",
            i + 1,
            sanitize_title(&src.title),
            sanitize_title(&src.source_name),
            sanitize_title(&src.url)
        ));
        if let Some(date) = &src.pub_date {
            digest.push_str(&format!("- Published: {}\n", sanitize_title(date)));
        }
        digest.push_str(&format!("\n{trimmed_content}\n\n---\n\n"));
    }

    let mode = args.stream_mode(&title_display);
    let author_str = format!("{} news sources", sources.len());
    let output = llm
        .summarize_news_digest(topic, timeframe.display_label(), &digest, args.style, mode)
        .await?;

    Ok(SummaryOutcome::from_llm(
        "news",
        title_display,
        Some(author_str),
        Some(search_url),
        total_words,
        output,
        Some(Cache::news_cache_title(topic, timeframe)),
    ))
}

pub(super) struct PreparedBookQuery {
    title: String,
    requested_author: Option<String>,
    resolved_title: String,
    resolved_author: Option<String>,
    work_key: Option<String>,
    candidates: Vec<SearchResult>,
    searcher: BookSearcher,
}

async fn fetch_or_cache_candidates(
    effective_title: &str,
    effective_author: Option<&str>,
    cache: &Cache,
    searcher: &BookSearcher,
) -> Vec<SearchResult> {
    if let Some(cached) = cache.get_search_results(effective_title, effective_author) {
        Ui::info(&format!(
            "Loaded {} search candidate(s) from local cache.",
            cached.len()
        ));
        return cached;
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
    results
}

pub(super) async fn prepare_book_query(
    title: &str,
    author: Option<&str>,
    scraper: &WebScraper,
    cache: &Cache,
) -> PreparedBookQuery {
    let searcher = BookSearcher::with_client(scraper.client().clone());

    let (resolved_title, resolved_author, work_key, candidates) =
        if let Some(Some(meta)) = cache.get_book_metadata(title, author) {
            let effective_author = meta.author.clone().or_else(|| author.map(str::to_string));
            if let Some(ref a) = effective_author {
                let year_str = meta
                    .first_publish_year
                    .map(|y| format!(" ({y})"))
                    .unwrap_or_default();
                Ui::info(&format!(
                    "Identified book: \"{}\" by {a}{year_str}",
                    meta.canonical_title
                ));
            }
            let cands = fetch_or_cache_candidates(
                &meta.canonical_title,
                effective_author.as_deref(),
                cache,
                &searcher,
            )
            .await;
            (meta.canonical_title, effective_author, meta.work_key, cands)
        } else if let Some(cached) = cache.get_search_results(title, author) {
            Ui::info(&format!(
                "Loaded {} search candidate(s) from local cache.",
                cached.len()
            ));
            (title.to_string(), author.map(str::to_string), None, cached)
        } else {
            let spinner = Ui::spinner(
                "Searching web for book synopsis & metadata (Wikipedia / DDG / Open Library)...",
            );
            let (meta_opt, mut raw_candidates) = tokio::join!(
                searcher.resolve_book_metadata(title, author),
                searcher.search_raw_candidates(title, author)
            );
            let (effective_title, effective_author, work_key) = if let Some(meta) = meta_opt {
                cache.set_book_metadata(title, author, &Some(meta.clone()));
                let effective_author = meta.author.or_else(|| author.map(str::to_string));
                let year_str = meta
                    .first_publish_year
                    .map(|y| format!(" ({y})"))
                    .unwrap_or_default();
                let done = match &effective_author {
                    Some(a) => format!("Identified \"{}\" by {a}{year_str}", meta.canonical_title),
                    None => format!("Identified \"{}\"", meta.canonical_title),
                };
                Ui::finish_spinner(&spinner, &done);
                (meta.canonical_title, effective_author, meta.work_key)
            } else {
                let done = if raw_candidates.is_empty() {
                    "No web results found.".to_string()
                } else {
                    format!("Found {} search candidate(s) online.", raw_candidates.len())
                };
                Ui::finish_spinner(&spinner, &done);
                (title.to_string(), author.map(str::to_string), None)
            };

            rank_candidates(
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
            (effective_title, effective_author, work_key, raw_candidates)
        };

    PreparedBookQuery {
        title: title.to_string(),
        requested_author: author.map(str::to_string),
        resolved_title,
        resolved_author,
        work_key,
        candidates,
        searcher,
    }
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
    if top_score >= 90 && (top_score - second_score) >= 30 {
        return;
    }

    Ui::header(&format!(
        "Reference Disambiguation for \"{}\"",
        sanitize_title(effective_title)
    ));
    eprintln!("Multiple references found online:");
    let display_limit = ordered_candidates.len().min(4);
    for (i, cand) in ordered_candidates[..display_limit].iter().enumerate() {
        let tag = if i == 0 { " [Recommended]" } else { "" };
        eprintln!("  [{}] {}{tag}", i + 1, sanitize_title(&cand.title));
        if let Some(snippet) = &cand.snippet {
            eprintln!("      \"{}\"", sanitize_terminal_text_cow(snippet));
        }
    }
    eprint!("\nSelect reference [1-{display_limit}, Enter for 1]: ");
    let _ = io::stderr().flush();

    let mut stdin_lock = io::stdin().lock();
    if let Ok(input) = read_line_bounded(&mut stdin_lock, 256)
        && let Ok(choice) = input.trim().parse::<usize>()
        && (1..=display_limit).contains(&choice)
    {
        let selected = ordered_candidates.remove(choice - 1);
        ordered_candidates.insert(0, selected);
    }
    eprintln!();
}

async fn resolve_book_reference_context(
    prep: &mut PreparedBookQuery,
    scraper: &WebScraper,
    cache: &Cache,
    args: &Cli,
) -> Option<(String, String, usize)> {
    if !prep.candidates.is_empty() {
        disambiguate_candidates_if_needed(&prep.resolved_title, &mut prep.candidates, args);

        let mut fallback_snippet = None;
        let mut fallback_url = None;

        for res in &prep.candidates {
            Ui::info(&format!(
                "Attempting reference: {} ({})",
                res.title, res.url
            ));

            match fetch_or_cache_page_with(
                &res.url,
                cache,
                Some((
                    "Loaded reference page from local cache.",
                    "Scraping reference page...",
                    "Reference page scraped.",
                )),
                scraper.scrape(&res.url),
            )
            .await
            {
                Ok(page) => {
                    let (content_to_use, words_to_use) = if res.url.contains("wikipedia.org/wiki/")
                        && let Some(synopsis) = extract_wikipedia_synopsis(&page.content)
                    {
                        let wc = synopsis.split_whitespace().count();
                        Ui::info(&format!(
                            "Extracted targeted plot synopsis: {wc} words (streamlined from {} full-page words)",
                            page.word_count
                        ));
                        (synopsis, wc)
                    } else {
                        (page.content, page.word_count)
                    };

                    Ui::info(&format!("Grounded reference text: {words_to_use} words"));
                    return Some((res.url.clone(), content_to_use, words_to_use));
                }
                Err(err) => {
                    Ui::warn(&format!(
                        "Could not scrape {}: {err}. Trying next reference...",
                        res.url
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

        if let (Some(snippet), Some(url)) = (fallback_snippet, fallback_url) {
            Ui::info("Using search snippet as grounded context...");
            let word_count = snippet.split_whitespace().count();
            return Some((url, snippet, word_count));
        }

        Ui::warn(
            "All online references failed to scrape. Falling back to Open Library or LLM internal memory.",
        );
    } else {
        Ui::warn(
            "No web synopsis found online. Falling back to Open Library or LLM trained memory.",
        );
    }

    if let Some(work_key) = &prep.work_key
        && let Some(desc) = prep.searcher.fetch_work_description(work_key).await
    {
        let wc = desc.split_whitespace().count();
        if wc >= 30 {
            Ui::info(&format!(
                "Using Open Library editorial description as grounded context ({wc} words)..."
            ));
            let work_url = format!(
                "https://openlibrary.org/{}",
                work_key.trim().trim_start_matches('/')
            );
            return Some((work_url, desc, wc));
        }
    }

    None
}

pub(super) async fn summarize_prepared_book(
    mut prep: PreparedBookQuery,
    llm: &LlmClient,
    scraper: &WebScraper,
    cache: &Cache,
    args: &Cli,
) -> Result<SummaryOutcome> {
    let author = prep.resolved_author.clone();
    let book_display = match author.as_deref() {
        Some(auth) => format!("\"{}\" by {auth}", prep.title),
        None => format!("\"{}\"", prep.title),
    };
    let mode = args.stream_mode(&book_display);
    let reference_context = resolve_book_reference_context(&mut prep, scraper, cache, args).await;

    let (output, word_count, source_url) = if let Some((url, content, wc)) = reference_context {
        let out = llm
            .summarize_book_with_context(
                &prep.resolved_title,
                author.as_deref(),
                &url,
                &content,
                args.style,
                mode,
            )
            .await?;
        (out, wc, Some(url))
    } else {
        let out = llm
            .summarize_book_from_memory(&prep.resolved_title, author.as_deref(), args.style, mode)
            .await?;
        (out, 0, None)
    };

    let title_key = Cache::book_cache_title(&prep.title, prep.requested_author.as_deref());
    Ok(SummaryOutcome::from_llm(
        "book",
        book_display,
        author,
        source_url,
        word_count,
        output,
        Some(title_key),
    ))
}

fn print_view_and_hint(files: Option<&RenderedFiles>, hint: Option<&str>) {
    if let Some(f) = files {
        Ui::success(&format!("Styled HTML view: {}", file_url(&f.html_path)));
        Ui::success(&format!("Markdown file:    {}", file_url(&f.md_path)));
    }
    if let Some(h) = hint {
        let safe = sanitize_terminal_text_cow(h);
        if Ui::use_color() {
            eprintln!("\n\x1b[2m{safe}\x1b[0m");
        } else {
            eprintln!("\n{safe}");
        }
    }
}

pub(super) fn present_summary_outcome(
    outcome: SummaryOutcome,
    args: &Cli,
    cache: &Cache,
) -> Result<()> {
    if args.json {
        return print_json(&outcome);
    }

    if let Some(output_path) = &args.output {
        let content = if is_html_path(output_path) {
            std::borrow::Cow::Owned(render_html_document_with_reasoning(
                &outcome.title,
                &outcome.summary,
                outcome.reasoning.as_deref(),
            ))
        } else {
            std::borrow::Cow::Borrowed(outcome.summary.as_str())
        };
        crate::fs_util::write_private_file(output_path, content.as_bytes())
            .with_context(|| format!("Failed to write output to {:?}", output_path))?;

        Ui::success(&format!(
            "Summary successfully saved to {}",
            sanitize_terminal_text_cow(&output_path.display().to_string())
        ));
    } else {
        let is_tty = io::stdout().is_terminal();
        let hint_ref = (!args.plain && !args.quiet)
            .then_some(outcome.hint.as_deref())
            .flatten();

        let view_files = if args.open {
            match ensure_view_files_unique(
                &cache.views_dir(),
                &outcome.title,
                &outcome.summary,
                outcome.url.as_deref(),
                outcome.reasoning.as_deref(),
                !outcome.cached,
            ) {
                Ok(files) => Some(files),
                Err(err) => {
                    Ui::warn(&format!("Failed to generate HTML view: {err}"));
                    None
                }
            }
        } else {
            None
        };

        if args.open
            && let Some(ref files) = view_files
            && let Err(err) = open_in_browser(&files.html_path)
        {
            Ui::warn(&format!("Could not open browser: {err}"));
        }

        if outcome.streamed_to_stdout {
            print_view_and_hint(view_files.as_ref(), hint_ref);
        } else {
            let view_pair = view_files
                .as_ref()
                .map(|f| (f.html_path.as_path(), f.md_path.as_path()));
            display_summary_full(
                &outcome.title,
                &outcome.summary,
                args.paging,
                args.plain,
                if is_tty { view_pair } else { None },
                if is_tty { hint_ref } else { None },
            )?;
            if !is_tty {
                print_view_and_hint(view_files.as_ref(), hint_ref);
            }
        }
    }

    if !outcome.cached
        && let Some(db) = open_db_or_warn(cache)
    {
        let record = NewSummaryRecord {
            target_type: outcome.target_type,
            title: &outcome.title,
            author: outcome.author.as_deref(),
            url: outcome.url.as_deref(),
            style: args.style.as_str(),
            model: &args.model,
            word_count: outcome.word_count,
            prompt_tokens: outcome.prompt_tokens,
            completion_tokens: outcome.completion_tokens,
            duration_ms: outcome.duration_ms,
            tokens_per_second: outcome.tokens_per_second,
            summary: &outcome.summary,
            reasoning: outcome.reasoning.as_deref(),
        };
        if let Err(err) = db.record_summary(&record, outcome.cache_key.as_deref()) {
            Ui::warn(&format!("Failed to save summary to database: {err:#}"));
        }
        if args.open
            && let Err(err) = update_index_dashboard(&db, cache)
        {
            Ui::warn(&format!("Failed to update library dashboard: {err:#}"));
        }
    }

    cache.maybe_auto_prune();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_target_type_for_url() {
        for (url, expected) in [
            ("https://www.youtube.com/watch?v=dQw4w9WgXcQ", "youtube"),
            ("https://youtu.be/dQw4w9WgXcQ", "youtube"),
            ("https://example.com/paper.pdf?token=123", "file"),
            ("https://example.com/paper.PDF#page=3", "file"),
            ("https://example.com/article", "web"),
        ] {
            assert_eq!(target_type_for_url(url), expected, "{url}");
        }
    }

    #[tokio::test]
    async fn test_fetch_url_content_blocks_private_remote_pdf() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let cache = Cache::with_dir(tmp.path().to_path_buf(), false);
        let scraper = WebScraper::new().unwrap();
        let err = fetch_url_content(
            "https://169.254.169.254/latest/meta-data.pdf",
            &scraper,
            &cache,
            false,
        )
        .await
        .unwrap_err();
        assert!(err.to_string().contains("SSRF protection"));
    }
}
