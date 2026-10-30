use crate::db::{HistoryEntry, LibraryStats};
use crate::fs_util;
use std::path::{Path, PathBuf};

pub struct RenderedFiles {
    pub html_path: PathBuf,
    pub md_path: PathBuf,
}

/// Returns `true` if `path` has a `.html` or `.htm` extension (case-insensitive).
pub fn is_html_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

/// Ensures both `.md` and `.html` view files exist in `views_dir`.
/// When `force_write` is `false` (e.g., on a cache hit) and both files already exist on disk,
/// skips Markdown-to-HTML rendering and disk writes.
pub fn ensure_view_files(
    views_dir: &Path,
    title: &str,
    markdown: &str,
    force_write: bool,
) -> std::io::Result<RenderedFiles> {
    let slug = slugify(title);
    let md_path = views_dir.join(format!("{}.md", slug));
    let html_path = views_dir.join(format!("{}.html", slug));

    if !force_write && md_path.exists() && html_path.exists() {
        return Ok(RenderedFiles { html_path, md_path });
    }

    fs_util::create_private_dir_all(views_dir)?;
    fs_util::write_private_file(&md_path, markdown)?;

    let html_doc = render_html_document(title, markdown);
    fs_util::write_private_file(&html_path, html_doc)?;

    Ok(RenderedFiles { html_path, md_path })
}

/// Launches the default system browser to view the specified file or URL.
pub fn open_in_browser(path: &Path) -> std::io::Result<()> {
    let abs_path = path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    });

    open::that_detached(&abs_path)
}

/// Converts a title into a clean, filesystem-safe ASCII slug (e.g. `"Dune" by Frank Herbert` -> `dune-by-frank-herbert`).
pub fn slugify(title: &str) -> String {
    let mut slug = String::with_capacity(title.len());
    let mut prev_dash = false;

    for ch in title.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash && !slug.is_empty() && ch != '\'' && ch != '"' && ch != '’' {
            slug.push('-');
            prev_dash = true;
        }
    }

    let trimmed = slug.trim_matches('-');
    if trimmed.is_empty() {
        "summary".to_string()
    } else if trimmed.len() > 80 {
        trimmed[..80].trim_end_matches('-').to_string()
    } else {
        trimmed.to_string()
    }
}

const SHARED_THEME_CSS: &str = r#"    :root {
      --bg: #faf9f5;
      --surface: #ffffff;
      --text: #1c1e21;
      --muted: #5f6672;
      --accent: #0f62fe;
      --border: #e5e4de;
      --code-bg: #f1f0ea;
      --quote-bg: #f4f3ed;
      --badge-bg: #f1f0ea;
      --card-hover: #f7f6f0;
    }
    @media (prefers-color-scheme: dark) {
      :root {
        --bg: #111317;
        --surface: #181b20;
        --text: #e6e8ec;
        --muted: #9aa2b1;
        --accent: #6ea8fe;
        --border: #2a2f3a;
        --code-bg: #222630;
        --quote-bg: #1b1f27;
        --badge-bg: #222630;
        --card-hover: #1e2229;
      }
    }
    * { box-sizing: border-box; }"#;

/// Renders a standalone HTML5 document with editorial typography and automatic light/dark mode.
pub fn render_html_document(title: &str, markdown: &str) -> String {
    let escaped_title = escape_html(title);
    let body_html = markdown_to_html(markdown);

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src data: https:;">
  <title>Summary: {escaped_title}</title>
  <style>
{SHARED_THEME_CSS}
    body {{
      margin: 0;
      padding: 3rem 1.25rem 5rem;
      background: var(--bg);
      color: var(--text);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      line-height: 1.72;
      font-size: 1.05rem;
      -webkit-font-smoothing: antialiased;
    }}
    main {{
      max-width: 760px;
      margin: 0 auto;
      background: var(--surface);
      padding: 2.75rem 3.25rem;
      border-radius: 14px;
      border: 1px solid var(--border);
      box-shadow: 0 8px 30px rgba(0, 0, 0, 0.06);
    }}
    @media (max-width: 640px) {{
      body {{ padding: 1rem 0.5rem 3rem; }}
      main {{ padding: 1.5rem 1.25rem; }}
    }}
    header.meta {{
      border-bottom: 1px solid var(--border);
      padding-bottom: 1rem;
      margin-bottom: 2rem;
      font-size: 0.85rem;
      color: var(--muted);
      text-transform: uppercase;
      letter-spacing: 0.06em;
      font-weight: 600;
    }}
    h1, h2, h3, h4 {{
      color: var(--text);
      line-height: 1.3;
      margin-top: 2rem;
      margin-bottom: 0.75rem;
      font-weight: 700;
      letter-spacing: -0.015em;
    }}
    h1 {{ font-size: 2rem; margin-top: 0; }}
    h2 {{
      font-size: 1.45rem;
      padding-bottom: 0.35rem;
      border-bottom: 1px solid var(--border);
    }}
    h3 {{ font-size: 1.2rem; }}
    p {{ margin: 0 0 1.1rem; }}
    ul, ol {{
      margin: 0 0 1.25rem;
      padding-left: 1.5rem;
    }}
    li {{ margin-bottom: 0.45rem; }}
    li > strong {{ color: var(--text); }}
    hr {{
      border: 0;
      border-top: 1px solid var(--border);
      margin: 2rem 0;
    }}
    blockquote {{
      margin: 1.25rem 0;
      padding: 0.85rem 1.25rem;
      border-left: 4px solid var(--accent);
      background: var(--quote-bg);
      border-radius: 0 8px 8px 0;
      color: var(--muted);
    }}
    code {{
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      font-size: 0.9em;
      background: var(--code-bg);
      padding: 0.15em 0.4em;
      border-radius: 5px;
    }}
    pre {{
      background: var(--code-bg);
      padding: 1rem 1.25rem;
      border-radius: 8px;
      overflow-x: auto;
      margin: 1.25rem 0;
    }}
    pre code {{
      background: none;
      padding: 0;
    }}
  </style>
</head>
<body>
  <main>
    <header class="meta">srz &bull; Summary: {escaped_title}</header>
{body_html}  </main>
</body>
</html>
"#,
        SHARED_THEME_CSS = SHARED_THEME_CSS,
        escaped_title = escaped_title,
        body_html = body_html,
    )
}

/// Converts CommonMark / GFM Markdown into semantic HTML using `pulldown-cmark`.
pub fn markdown_to_html(md: &str) -> String {
    let mut options = pulldown_cmark::Options::empty();
    options.insert(pulldown_cmark::Options::ENABLE_TABLES);
    options.insert(pulldown_cmark::Options::ENABLE_STRIKETHROUGH);
    options.insert(pulldown_cmark::Options::ENABLE_TASKLISTS);
    options.insert(pulldown_cmark::Options::ENABLE_HEADING_ATTRIBUTES);

    // Sanitize raw HTML tags into escaped text to prevent HTML injection from untrusted sources
    let parser = pulldown_cmark::Parser::new_ext(md, options).map(|event| match event {
        pulldown_cmark::Event::Html(text) => pulldown_cmark::Event::Text(text),
        pulldown_cmark::Event::InlineHtml(text) => pulldown_cmark::Event::Text(text),
        other => other,
    });

    let mut html_output = String::with_capacity(md.len() * 3 / 2);
    pulldown_cmark::html::push_html(&mut html_output, parser);
    html_output.replace("<hr />", "<hr>")
}

/// Escapes a single character for HTML and appends to `out`.
pub fn escape_html_char(out: &mut String, ch: char) {
    match ch {
        '&' => out.push_str("&amp;"),
        '<' => out.push_str("&lt;"),
        '>' => out.push_str("&gt;"),
        '"' => out.push_str("&quot;"),
        '\'' => out.push_str("&#39;"),
        _ => out.push(ch),
    }
}

/// Appends HTML-escaped text directly into the provided output buffer using fast chunk scanning.
pub fn escape_html_to(out: &mut String, text: &str) {
    let mut rest = text;
    while let Some(idx) = rest.find(['&', '<', '>', '"', '\'']) {
        out.push_str(&rest[..idx]);
        let ch = rest[idx..].chars().next().unwrap();
        escape_html_char(out, ch);
        rest = &rest[idx + ch.len_utf8()..];
    }
    out.push_str(rest);
}

/// Returns a zero-allocation `Cow::Borrowed` when `text` requires no escaping.
pub fn escape_html_cow(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.contains(['&', '<', '>', '"', '\'']) {
        std::borrow::Cow::Borrowed(text)
    } else {
        let mut out = String::with_capacity(text.len() + 16);
        escape_html_to(&mut out, text);
        std::borrow::Cow::Owned(out)
    }
}

pub fn escape_html(text: &str) -> String {
    escape_html_cow(text).into_owned()
}

/// Appends rendered inline Markdown (`**bold**`, `*italic*`, `` `code` ``) directly
/// into the output buffer with HTML escaping in a single pass without intermediate allocations.
pub fn render_inline_to(out: &mut String, text: &str) {
    let bytes = text.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        // Inline code: `...`
        if bytes[i] == b'`'
            && let Some(end) = text[i + 1..].find('`')
        {
            out.push_str("<code>");
            escape_html_to(out, &text[i + 1..i + 1 + end]);
            out.push_str("</code>");
            i += end + 2;
            continue;
        }

        // Bold: **...**
        if i + 1 < bytes.len()
            && bytes[i] == b'*'
            && bytes[i + 1] == b'*'
            && let Some(end) = text[i + 2..].find("**")
        {
            out.push_str("<strong>");
            render_inline_to(out, &text[i + 2..i + 2 + end]);
            out.push_str("</strong>");
            i += end + 4;
            continue;
        }

        // Italic: *...*
        if bytes[i] == b'*'
            && let Some(end) = text[i + 1..].find('*')
            && end > 0
        {
            out.push_str("<em>");
            escape_html_to(out, &text[i + 1..i + 1 + end]);
            out.push_str("</em>");
            i += end + 2;
            continue;
        }

        let ch = text[i..].chars().next().unwrap();
        escape_html_char(out, ch);
        i += ch.len_utf8();
    }
}

/// Renders inline Markdown (`**bold**`, `*italic*`, `` `code` ``) after HTML-escaping.
pub fn render_inline(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    render_inline_to(&mut out, text);
    out
}

/// Formats an author name with a leading " by " if present.
pub fn format_author_by(author: Option<&str>) -> String {
    match author {
        Some(a) if !a.trim().is_empty() => format!(" by {}", crate::ui::sanitize_title(a)),
        _ => String::new(),
    }
}

/// Returns the canonical `(icon, label, badge)` tuple for a summary target type.
pub fn target_meta(target_type: &str) -> (&'static str, &'static str, &'static str) {
    match target_type {
        "book" => ("📚", "Book", "📚 Book"),
        "web" => ("🌐", "Web", "🌐 Web"),
        "news" => ("📰", "News", "📰 News"),
        "file" => ("📄", "File", "📄 File"),
        "stdin" => ("⌨️", "Stdin", "⌨️ Stdin"),
        "youtube" => ("📺", "Video", "📺 Video"),
        _ => ("🌐", "Doc", "📝 Doc"),
    }
}

/// Returns the display emoji icon for a summary target type.
pub fn target_icon(target_type: &str) -> &'static str {
    target_meta(target_type).0
}

/// Returns the formatted badge label (icon + category name) for a summary target type.
pub fn target_badge(target_type: &str) -> &'static str {
    target_meta(target_type).2
}

/// Generates a standalone HTML5 dashboard index of all past summaries in the library.
pub fn render_index_html(stats: &LibraryStats, entries: &[HistoryEntry]) -> String {
    let mut cards_html = String::new();

    if entries.is_empty() {
        cards_html.push_str(
            r#"    <div class="empty-state">
      <h3>No summaries in library yet</h3>
      <p>Summarize a book, webpage, or file to build your personal offline library.</p>
    </div>"#,
        );
    } else {
        for entry in entries {
            let icon = target_icon(&entry.target_type);
            let slug = slugify(&entry.title);
            let author_line = if let Some(a) = &entry.author {
                format!(r#"<div class="entry-author">by {}</div>"#, escape_html(a))
            } else {
                String::new()
            };
            let speed_str = entry
                .tokens_per_second
                .map(|s| format!("{:.1} tok/s", s))
                .unwrap_or_else(|| "-".to_string());
            let time_str = format!("{:.1}s", entry.duration_ms as f64 / 1000.0);
            let search_data = format!(
                "{} {} {} {}",
                entry.title,
                entry.author.as_deref().unwrap_or(""),
                entry.model,
                entry.target_type
            );

            cards_html.push_str(&format!(
                r#"    <div class="entry-card" data-type="{target_type}" data-search="{search_data}">
      <div class="entry-header">
        <a class="entry-title" href="./{slug}.html">{icon} {title}</a>
        <span class="entry-date">{date}</span>
      </div>
      {author_line}
      <div class="entry-badges">
        <span class="badge">Type: {target_type}</span>
        <span class="badge">Style: {style}</span>
        <span class="badge">Model: {model}</span>
        <span class="badge">{words} words</span>
        <span class="badge">Speed: {speed_str}</span>
        <span class="badge">Time: {time_str}</span>
      </div>
      <div class="actions">
        <a class="action-link" href="./{slug}.html">Open Reader View &rarr;</a>
        <a class="action-link" href="./{slug}.md">Markdown (.md)</a>
      </div>
    </div>
"#,
                target_type = escape_html(&entry.target_type),
                search_data = escape_html(&search_data),
                slug = slug,
                icon = icon,
                title = escape_html(&entry.title),
                date = escape_html(&entry.created_at),
                author_line = author_line,
                style = escape_html(&entry.style),
                model = escape_html(&entry.model),
                words = entry.word_count,
                speed_str = speed_str,
                time_str = time_str
            ));
        }
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';">
  <title>srz &bull; Summary Library</title>
  <style>
{SHARED_THEME_CSS}
    body {{
      margin: 0;
      padding: 2.5rem 1.25rem 5rem;
      background: var(--bg);
      color: var(--text);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      line-height: 1.6;
    }}
    .container {{
      max-width: 900px;
      margin: 0 auto;
    }}
    header.hero {{
      margin-bottom: 2rem;
    }}
    header.hero h1 {{
      font-size: 2.2rem;
      margin: 0 0 0.5rem;
      letter-spacing: -0.02em;
    }}
    header.hero p {{
      color: var(--muted);
      margin: 0;
      font-size: 1.05rem;
    }}
    .stats-bar {{
      display: grid;
      grid-template-columns: repeat(auto-fit, minmax(130px, 1fr));
      gap: 1rem;
      margin-bottom: 2rem;
    }}
    .stat-card {{
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 1rem 1.25rem;
      text-align: center;
    }}
    .stat-val {{
      font-size: 1.6rem;
      font-weight: 700;
      color: var(--text);
      line-height: 1.2;
    }}
    .stat-lbl {{
      font-size: 0.8rem;
      text-transform: uppercase;
      letter-spacing: 0.05em;
      color: var(--muted);
      margin-top: 0.25rem;
    }}
    .controls {{
      display: flex;
      flex-wrap: wrap;
      gap: 0.75rem;
      align-items: center;
      margin-bottom: 1.75rem;
    }}
    .search-box {{
      flex: 1;
      min-width: 240px;
      padding: 0.75rem 1rem;
      border-radius: 8px;
      border: 1px solid var(--border);
      background: var(--surface);
      color: var(--text);
      font-size: 0.95rem;
      outline: none;
    }}
    .filter-btn {{
      padding: 0.55rem 0.95rem;
      border-radius: 8px;
      border: 1px solid var(--border);
      background: var(--surface);
      color: var(--muted);
      cursor: pointer;
      font-size: 0.88rem;
      font-weight: 500;
      transition: all 0.15s ease;
    }}
    .filter-btn.active, .filter-btn:hover {{
      background: var(--accent);
      color: #ffffff;
      border-color: var(--accent);
    }}
    .entry-card {{
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 10px;
      padding: 1.25rem 1.5rem;
      margin-bottom: 1rem;
      transition: background 0.15s ease;
    }}
    .entry-card:hover {{
      background: var(--card-hover);
    }}
    .entry-header {{
      display: flex;
      align-items: baseline;
      justify-content: space-between;
      gap: 1rem;
    }}
    .entry-title {{
      font-size: 1.2rem;
      font-weight: 600;
      color: var(--text);
      text-decoration: none;
    }}
    .entry-title:hover {{
      color: var(--accent);
    }}
    .entry-date {{
      font-size: 0.85rem;
      color: var(--muted);
      white-space: nowrap;
    }}
    .entry-author {{
      color: var(--muted);
      font-size: 0.95rem;
      margin: 0.2rem 0 0.6rem;
    }}
    .entry-badges {{
      display: flex;
      flex-wrap: wrap;
      gap: 0.4rem;
      margin-top: 0.6rem;
      font-size: 0.8rem;
    }}
    .badge {{
      background: var(--badge-bg);
      padding: 0.2rem 0.55rem;
      border-radius: 5px;
      color: var(--muted);
    }}
    .actions {{
      display: flex;
      gap: 1.2rem;
      margin-top: 0.8rem;
    }}
    .action-link {{
      font-size: 0.85rem;
      color: var(--accent);
      text-decoration: none;
      font-weight: 500;
    }}
    .action-link:hover {{
      text-decoration: underline;
    }}
    .empty-state {{
      text-align: center;
      padding: 3rem 1rem;
      background: var(--surface);
      border: 1px solid var(--border);
      border-radius: 10px;
      color: var(--muted);
    }}
  </style>
</head>
<body>
  <div class="container">
    <header class="hero">
      <h1>srz &bull; Summary Library</h1>
      <p>Personal searchable reading library and distillation archive</p>
    </header>

    <div class="stats-bar">
      <div class="stat-card">
        <div class="stat-val">{total_summaries}</div>
        <div class="stat-lbl">Total Summaries</div>
      </div>
      <div class="stat-card">
        <div class="stat-val">{book_summaries}</div>
        <div class="stat-lbl">Books</div>
      </div>
      <div class="stat-card">
        <div class="stat-val">{web_summaries}</div>
        <div class="stat-lbl">Web Articles</div>
      </div>
      <div class="stat-card">
        <div class="stat-val">{file_summaries}</div>
        <div class="stat-lbl">Files</div>
      </div>
      <div class="stat-card">
        <div class="stat-val">{total_words}</div>
        <div class="stat-lbl">Words Read</div>
      </div>
    </div>

    <div class="controls">
      <input type="text" id="search" class="search-box" placeholder="Search summaries by title, author, or model...">
      <button class="filter-btn active" data-filter="all">All</button>
      <button class="filter-btn" data-filter="book">📚 Books</button>
      <button class="filter-btn" data-filter="web">🌐 Web</button>
      <button class="filter-btn" data-filter="youtube">📺 Videos</button>
      <button class="filter-btn" data-filter="news">📰 News</button>
      <button class="filter-btn" data-filter="file">📄 Files</button>
      <button class="filter-btn" data-filter="stdin">⌨️ Stdin</button>
    </div>

    <div id="entries-list">
{cards_html}    </div>
  </div>

  <script>
    const searchInput = document.getElementById('search');
    const filterBtns = document.querySelectorAll('.filter-btn');
    const cards = document.querySelectorAll('.entry-card');
    let currentFilter = 'all';

    function filterCards() {{
      const q = (searchInput.value || '').toLowerCase().trim();
      cards.forEach(card => {{
        const type = card.getAttribute('data-type');
        const text = (card.getAttribute('data-search') || '').toLowerCase();
        const matchesType = (currentFilter === 'all' || type === currentFilter);
        const matchesQuery = (!q || text.includes(q));
        card.style.display = (matchesType && matchesQuery) ? 'block' : 'none';
      }});
    }}

    searchInput.addEventListener('input', filterCards);
    filterBtns.forEach(btn => {{
      btn.addEventListener('click', () => {{
        filterBtns.forEach(b => b.classList.remove('active'));
        btn.classList.add('active');
        currentFilter = btn.getAttribute('data-filter');
        filterCards();
      }});
    }});
  </script>
</body>
</html>
"#,
        SHARED_THEME_CSS = SHARED_THEME_CSS,
        total_summaries = stats.total_summaries,
        book_summaries = stats.book_summaries,
        web_summaries = stats.web_summaries,
        file_summaries = stats.file_summaries,
        total_words = stats.total_source_words,
        cards_html = cards_html
    )
}

/// Ensures the `index.html` library dashboard file is generated or updated in `views_dir`.
pub fn ensure_index_file(
    views_dir: &Path,
    stats: &LibraryStats,
    entries: &[HistoryEntry],
) -> std::io::Result<PathBuf> {
    fs_util::create_private_dir_all(views_dir)?;
    let html = render_index_html(stats, entries);
    let index_path = views_dir.join("index.html");
    fs_util::write_private_file(&index_path, html)?;
    Ok(index_path)
}
