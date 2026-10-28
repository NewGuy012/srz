use crate::cache;
use crate::db::{HistoryEntry, LibraryStats};
use std::path::{Path, PathBuf};

pub struct RenderedFiles {
    pub html_path: PathBuf,
    pub md_path: PathBuf,
}

/// Returns `true` if `path` has a `.html` or `.htm` extension (case-insensitive).
pub fn is_html_path(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
        .unwrap_or(false)
}

/// Saves both a `.md` file and a standalone, styled `.html` document into `views_dir`,
/// returning their absolute paths for clickable terminal links.
pub fn save_view_files(
    views_dir: &Path,
    title: &str,
    markdown: &str,
) -> std::io::Result<RenderedFiles> {
    ensure_view_files(views_dir, title, markdown, true)
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

    cache::create_private_dir_all(views_dir)?;
    cache::write_private_file(&md_path, markdown)?;

    let html_doc = render_html_document(title, markdown);
    cache::write_private_file(&html_path, html_doc)?;

    Ok(RenderedFiles { html_path, md_path })
}

/// Launches the default system browser to view the specified file or URL.
pub fn open_in_browser(path: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open").arg(path).spawn()?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open").arg(path).spawn()?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()?;
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Unsupported operating system for auto-opening browser",
        ));
    }
    Ok(())
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
    :root {{
      --bg: #faf9f5;
      --surface: #ffffff;
      --text: #1c1e21;
      --muted: #5f6672;
      --accent: #0f62fe;
      --border: #e5e4de;
      --code-bg: #f1f0ea;
      --quote-bg: #f4f3ed;
    }}
    @media (prefers-color-scheme: dark) {{
      :root {{
        --bg: #111317;
        --surface: #181b20;
        --text: #e6e8ec;
        --muted: #9aa2b1;
        --accent: #6ea8fe;
        --border: #2a2f3a;
        --code-bg: #222630;
        --quote-bg: #1b1f27;
      }}
    }}
    * {{ box-sizing: border-box; }}
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
"#
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListState {
    None,
    Ul,
    Ol,
}

/// Lightweight, zero-dependency Markdown-to-HTML converter tailored for LLM summaries.
pub fn markdown_to_html(md: &str) -> String {
    let mut out = String::with_capacity(md.len() + md.len() / 2);
    let mut list_state = ListState::None;
    let mut in_code_block = false;
    let mut paragraph_buf = String::new();

    let close_paragraph = |out: &mut String, buf: &mut String| {
        if !buf.is_empty() {
            out.push_str("    <p>");
            out.push_str(buf);
            out.push_str("</p>\n");
            buf.clear();
        }
    };

    let set_list_state = |out: &mut String, current: &mut ListState, next: ListState| {
        if *current != next {
            match *current {
                ListState::Ul => out.push_str("    </ul>\n"),
                ListState::Ol => out.push_str("    </ol>\n"),
                ListState::None => {}
            }
            match next {
                ListState::Ul => out.push_str("    <ul>\n"),
                ListState::Ol => out.push_str("    <ol>\n"),
                ListState::None => {}
            }
            *current = next;
        }
    };

    for raw_line in md.lines() {
        let trimmed = raw_line.trim();

        if trimmed.starts_with("```") {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::None);
            if in_code_block {
                out.push_str("</code></pre>\n");
                in_code_block = false;
            } else {
                out.push_str("    <pre><code>");
                in_code_block = true;
            }
            continue;
        }

        if in_code_block {
            out.push_str(&escape_html(raw_line));
            out.push('\n');
            continue;
        }

        if trimmed.is_empty() {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::None);
            continue;
        }

        // Horizontal rule (`---`, `***`, `___`)
        if (trimmed.len() >= 3
            && (trimmed.chars().all(|c| c == '-')
                || trimmed.chars().all(|c| c == '*')
                || trimmed.chars().all(|c| c == '_')))
            || trimmed == "* * *"
            || trimmed == "- - -"
        {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::None);
            out.push_str("    <hr>\n");
            continue;
        }

        // Headings (`#` .. `####`)
        if let Some((level, heading_text)) = parse_heading(trimmed) {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::None);
            out.push_str(&format!(
                "    <h{level}>{}</h{level}>\n",
                render_inline(heading_text)
            ));
            continue;
        }

        // Blockquote (`> ...`)
        if let Some(quote) = trimmed.strip_prefix('>') {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::None);
            out.push_str("    <blockquote>");
            out.push_str(&render_inline(quote.trim()));
            out.push_str("</blockquote>\n");
            continue;
        }

        // Unordered list item (`- ...` or `* ...`)
        if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::Ul);
            out.push_str("      <li>");
            out.push_str(&render_inline(item.trim()));
            out.push_str("</li>\n");
            continue;
        }

        // Ordered list item (`1. ...`)
        if let Some(item) = parse_ordered_list_item(trimmed) {
            close_paragraph(&mut out, &mut paragraph_buf);
            set_list_state(&mut out, &mut list_state, ListState::Ol);
            out.push_str("      <li>");
            out.push_str(&render_inline(item));
            out.push_str("</li>\n");
            continue;
        }

        // Regular paragraph line (if a line starts with `**Label:**`, break onto its own paragraph for readability)
        set_list_state(&mut out, &mut list_state, ListState::None);
        if !paragraph_buf.is_empty() && trimmed.starts_with("**") {
            close_paragraph(&mut out, &mut paragraph_buf);
        } else if !paragraph_buf.is_empty() {
            paragraph_buf.push(' ');
        }
        paragraph_buf.push_str(&render_inline(trimmed));
    }

    if in_code_block {
        out.push_str("</code></pre>\n");
    }
    close_paragraph(&mut out, &mut paragraph_buf);
    set_list_state(&mut out, &mut list_state, ListState::None);

    out
}

fn parse_heading(line: &str) -> Option<(usize, &str)> {
    let hashes = line.bytes().take_while(|&b| b == b'#').count();
    if (1..=4).contains(&hashes) {
        let rest = &line[hashes..];
        if let Some(stripped) = rest.strip_prefix(' ') {
            return Some((hashes, stripped.trim()));
        }
    }
    None
}

fn parse_ordered_list_item(line: &str) -> Option<&str> {
    let digits = line.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 {
        let rest = &line[digits..];
        if let Some(item) = rest.strip_prefix(". ") {
            return Some(item.trim());
        }
    }
    None
}

fn escape_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}

/// Renders inline Markdown (`**bold**`, `*italic*`, `` `code` ``) after HTML-escaping.
fn render_inline(text: &str) -> String {
    render_inline_escaped(&escape_html(text))
}

fn render_inline_escaped(escaped: &str) -> String {
    let bytes = escaped.as_bytes();
    let mut out = String::with_capacity(escaped.len() + 16);
    let mut i = 0;

    while i < bytes.len() {
        // Inline code: `...`
        if bytes[i] == b'`'
            && let Some(end) = escaped[i + 1..].find('`')
        {
            out.push_str("<code>");
            out.push_str(&escaped[i + 1..i + 1 + end]);
            out.push_str("</code>");
            i += end + 2;
            continue;
        }

        // Bold: **...**
        if i + 1 < bytes.len() && bytes[i] == b'*' && bytes[i + 1] == b'*'
            && let Some(end) = escaped[i + 2..].find("**")
        {
            out.push_str("<strong>");
            out.push_str(&render_inline_escaped(&escaped[i + 2..i + 2 + end]));
            out.push_str("</strong>");
            i += end + 4;
            continue;
        }

        // Italic: *...*
        if bytes[i] == b'*'
            && let Some(end) = escaped[i + 1..].find('*')
            && end > 0
        {
            out.push_str("<em>");
            out.push_str(&escaped[i + 1..i + 1 + end]);
            out.push_str("</em>");
            i += end + 2;
            continue;
        }

        let ch = escaped[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

/// Returns the display emoji icon for a summary target type.
pub fn target_icon(target_type: &str) -> &'static str {
    match target_type {
        "book" => "📚",
        "file" => "📄",
        "stdin" => "⌨️",
        "news" => "📰",
        _ => "🌐",
    }
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
    :root {{
      --bg: #faf9f5;
      --surface: #ffffff;
      --text: #1c1e21;
      --muted: #5f6672;
      --accent: #0f62fe;
      --border: #e5e4de;
      --badge-bg: #f1f0ea;
      --card-hover: #f7f6f0;
    }}
    @media (prefers-color-scheme: dark) {{
      :root {{
        --bg: #111317;
        --surface: #181b20;
        --text: #e6e8ec;
        --muted: #9aa2b1;
        --accent: #6ea8fe;
        --border: #2a2f3a;
        --badge-bg: #222630;
        --card-hover: #1e2229;
      }}
    }}
    * {{ box-sizing: border-box; }}
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
    cache::create_private_dir_all(views_dir)?;
    let html = render_index_html(stats, entries);
    let index_path = views_dir.join("index.html");
    cache::write_private_file(&index_path, html)?;
    Ok(index_path)
}
