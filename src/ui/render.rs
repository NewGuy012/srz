use super::markdown::markdown_to_html;
use crate::db::{HistoryEntry, LibraryStats};
use crate::fs_util;
use crate::system::text_util::slugify_unique;
use std::borrow::Cow;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub(crate) struct RenderedFiles {
    pub html_path: PathBuf,
    pub md_path: PathBuf,
}

pub(crate) fn is_html_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("html") || e.eq_ignore_ascii_case("htm"))
}

pub(crate) fn ensure_view_files_unique(
    views_dir: &Path,
    title: &str,
    markdown: &str,
    unique_key: Option<&str>,
    reasoning: Option<&str>,
    force_write: bool,
) -> std::io::Result<RenderedFiles> {
    let slug = slugify_unique(title, unique_key);
    let md_path = views_dir.join(format!("{slug}.md"));
    let html_path = views_dir.join(format!("{slug}.html"));

    if !force_write && md_path.exists() && html_path.exists() {
        fs_util::reject_symlink(&md_path, "reuse markdown symlink view")?;
        fs_util::reject_symlink(&html_path, "reuse html symlink view")?;
        return Ok(RenderedFiles { html_path, md_path });
    }

    fs_util::create_private_dir_all(views_dir)?;
    fs_util::write_private_file(&md_path, markdown)?;

    let html_doc = render_html_document_with_reasoning(title, markdown, reasoning);
    fs_util::write_private_file(&html_path, html_doc)?;

    Ok(RenderedFiles { html_path, md_path })
}

#[cfg(target_os = "windows")]
fn normalize_browser_path(path: PathBuf) -> PathBuf {
    let raw = path.to_string_lossy();
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{unc}"))
    } else if let Some(trimmed) = raw.strip_prefix(r"\\?\") {
        PathBuf::from(trimmed)
    } else {
        path
    }
}

#[cfg(not(target_os = "windows"))]
fn normalize_browser_path(path: PathBuf) -> PathBuf {
    path
}

pub(crate) fn open_in_browser(path: &Path) -> std::io::Result<()> {
    let abs_path = normalize_browser_path(path.canonicalize().unwrap_or_else(|_| {
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(path))
                .unwrap_or_else(|_| path.to_path_buf())
        }
    }));

    #[cfg(target_os = "macos")]
    let mut cmd = std::process::Command::new("open");
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = std::process::Command::new(fs_util::system_root_bin("rundll32.exe"));
        c.arg("url.dll,FileProtocolHandler");
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut cmd = std::process::Command::new("xdg-open");

    let mut child = cmd
        .arg(&abs_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    std::thread::spawn(move || {
        let _ = child.wait();
    });

    Ok(())
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

fn render_html_page(title: &str, csp: &str, extra_css: &str, body_html: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <meta http-equiv="Content-Security-Policy" content="{csp}">
  <title>{title}</title>
  <style>
{SHARED_THEME_CSS}
{extra_css}
  </style>
</head>
<body>
{body_html}</body>
</html>
"#
    )
}

pub(crate) fn render_html_document_with_reasoning(
    title: &str,
    markdown: &str,
    reasoning: Option<&str>,
) -> String {
    let escaped_title = escape_html_cow(title);
    let body_html = markdown_to_html(markdown);
    let reasoning_html = match reasoning {
        Some(r) if !r.trim().is_empty() => {
            let r_rendered = markdown_to_html(r);
            format!(
                r#"    <details class="thought-drawer" style="margin-bottom: 2rem; padding: 1rem 1.25rem; background: var(--code-bg); border: 1px solid var(--border); border-radius: 8px;">
      <summary style="cursor: pointer; font-weight: 600; color: var(--muted); user-select: none;">💭 Thought Process &amp; Model Reasoning &amp; Analytical Trace &rsaquo;</summary>
      <div style="margin-top: 1rem; color: var(--text); font-size: 0.95rem; line-height: 1.6;">
        {r_rendered}
      </div>
    </details>
"#
            )
        }
        _ => String::new(),
    };

    const DOC_CSS: &str = r#"    body {
      margin: 0; padding: 3rem 1.25rem 5rem; background: var(--bg); color: var(--text);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      line-height: 1.72; font-size: 1.05rem; -webkit-font-smoothing: antialiased;
    }
    main {
      max-width: 760px; margin: 0 auto; background: var(--surface); padding: 2.75rem 3.25rem;
      border-radius: 14px; border: 1px solid var(--border); box-shadow: 0 8px 30px rgba(0, 0, 0, 0.06);
    }
    @media (max-width: 640px) {
      body { padding: 1rem 0.5rem 3rem; }
      main { padding: 1.5rem 1.25rem; }
    }
    header.meta {
      border-bottom: 1px solid var(--border); padding-bottom: 1rem; margin-bottom: 2rem;
      font-size: 0.85rem; color: var(--muted); text-transform: uppercase; letter-spacing: 0.06em; font-weight: 600;
    }
    h1, h2, h3, h4 {
      color: var(--text); line-height: 1.3; margin-top: 2rem; margin-bottom: 0.75rem;
      font-weight: 700; letter-spacing: -0.015em;
    }
    h1 { font-size: 2rem; margin-top: 0; }
    h2 { font-size: 1.45rem; padding-bottom: 0.35rem; border-bottom: 1px solid var(--border); }
    h3 { font-size: 1.2rem; }
    p { margin: 0 0 1.1rem; }
    ul, ol { margin: 0 0 1.25rem; padding-left: 1.5rem; }
    li { margin-bottom: 0.45rem; }
    li > strong { color: var(--text); }
    hr { border: 0; border-top: 1px solid var(--border); margin: 2rem 0; }
    blockquote {
      margin: 1.25rem 0; padding: 0.85rem 1.25rem; border-left: 4px solid var(--accent);
      background: var(--quote-bg); border-radius: 0 8px 8px 0; color: var(--muted);
    }
    code {
      font-family: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, monospace;
      font-size: 0.9em; background: var(--code-bg); padding: 0.15em 0.4em; border-radius: 5px;
    }
    pre { background: var(--code-bg); padding: 1rem 1.25rem; border-radius: 8px; overflow-x: auto; margin: 1.25rem 0; }
    pre code { background: none; padding: 0; }
    table { border-collapse: collapse; width: 100%; margin: 1.25rem 0; font-size: 0.95rem; }
    th, td { border: 1px solid var(--border); padding: 0.5rem 0.75rem; text-align: left; }
    th { background: var(--code-bg); font-weight: 600; }"#;

    let main_html = format!(
        "  <main>\n    <header class=\"meta\">srz &bull; Summary: {escaped_title}</header>\n{reasoning_html}{body_html}  </main>\n"
    );
    render_html_page(
        &format!("Summary: {escaped_title}"),
        "default-src 'none'; style-src 'unsafe-inline'; img-src 'self' data:;",
        DOC_CSS,
        &main_html,
    )
}

pub(crate) fn push_html_escaped(out: &mut String, text: &str, escape_single_quote: bool) {
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' if escape_single_quote => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
}

fn escape_html_cow(text: &str) -> Cow<'_, str> {
    if !text.contains(['&', '<', '>', '"', '\'']) {
        Cow::Borrowed(text)
    } else {
        let mut out = String::with_capacity(text.len() + 16);
        push_html_escaped(&mut out, text, true);
        Cow::Owned(out)
    }
}

pub(crate) fn format_author_by(author: Option<&str>) -> String {
    match author {
        Some(a) if !a.trim().is_empty() => format!(" by {}", crate::ui::sanitize_title(a)),
        _ => String::new(),
    }
}

// (target_type, icon, badge, filter_plural)
const TARGET_TYPES: &[(&str, &str, &str, &str)] = &[
    ("book", "📚", "📚 Book", "Books"),
    ("web", "🌐", "🌐 Web", "Web"),
    ("youtube", "📺", "📺 Video", "Videos"),
    ("news", "📰", "📰 News", "News"),
    ("file", "📄", "📄 File", "Files"),
    ("stdin", "⌨️", "⌨️ Stdin", "Stdin"),
];

pub(crate) fn target_icon(target_type: &str) -> &'static str {
    TARGET_TYPES
        .iter()
        .find(|&&(k, ..)| k == target_type)
        .map_or("📝", |&(_, icon, ..)| icon)
}

pub(crate) fn target_badge(target_type: &str) -> &'static str {
    TARGET_TYPES
        .iter()
        .find(|&&(k, ..)| k == target_type)
        .map_or("📝 Doc", |&(_, _, badge, _)| badge)
}

fn render_index_html(stats: &LibraryStats, entries: &[HistoryEntry]) -> String {
    let mut cards_html = String::with_capacity(entries.len().max(1) * 640);

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
            let slug = slugify_unique(&entry.title, entry.url.as_deref());
            let author_line = match entry
                .author
                .as_deref()
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                Some(a) => format!(
                    r#"<div class="entry-author">by {}</div>"#,
                    escape_html_cow(a)
                ),
                None => String::new(),
            };
            let speed_str = entry
                .tokens_per_second
                .map(|s| format!("{s:.1} tok/s"))
                .unwrap_or_else(|| "-".to_string());
            let time_str = format!("{:.1}s", entry.duration_ms as f64 / 1000.0);
            let search_data = format!(
                "{} {} {} {}",
                entry.title,
                entry.author.as_deref().unwrap_or(""),
                entry.model,
                entry.target_type
            );

            let _ = write!(
                cards_html,
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
                target_type = escape_html_cow(&entry.target_type),
                search_data = escape_html_cow(&search_data),
                title = escape_html_cow(&entry.title),
                date = escape_html_cow(&entry.created_at),
                style = escape_html_cow(&entry.style),
                model = escape_html_cow(&entry.model),
                words = entry.word_count,
            );
        }
    }

    let mut filter_buttons = String::from(
        "      <button class=\"filter-btn active\" data-filter=\"all\">All</button>\n",
    );
    for &(key, icon, _, plural) in TARGET_TYPES {
        let _ = writeln!(
            filter_buttons,
            "      <button class=\"filter-btn\" data-filter=\"{key}\">{icon} {plural}</button>"
        );
    }

    const INDEX_CSS: &str = r#"    body {
      margin: 0; padding: 2.5rem 1.25rem 5rem; background: var(--bg); color: var(--text);
      font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
      line-height: 1.6;
    }
    .container { max-width: 900px; margin: 0 auto; }
    header.hero { margin-bottom: 2rem; }
    header.hero h1 { font-size: 2.2rem; margin: 0 0 0.5rem; letter-spacing: -0.02em; }
    header.hero p { color: var(--muted); margin: 0; font-size: 1.05rem; }
    .stats-bar { display: grid; grid-template-columns: repeat(auto-fit, minmax(130px, 1fr)); gap: 1rem; margin-bottom: 2rem; }
    .stat-card { background: var(--surface); border: 1px solid var(--border); border-radius: 10px; padding: 1rem 1.25rem; text-align: center; }
    .stat-val { font-size: 1.6rem; font-weight: 700; color: var(--text); line-height: 1.2; }
    .stat-lbl { font-size: 0.8rem; text-transform: uppercase; letter-spacing: 0.05em; color: var(--muted); margin-top: 0.25rem; }
    .controls { display: flex; flex-wrap: wrap; gap: 0.75rem; align-items: center; margin-bottom: 1.75rem; }
    .search-box {
      flex: 1; min-width: 240px; padding: 0.75rem 1rem; border-radius: 8px; border: 1px solid var(--border);
      background: var(--surface); color: var(--text); font-size: 0.95rem; outline: none;
    }
    .filter-btn {
      padding: 0.55rem 0.95rem; border-radius: 8px; border: 1px solid var(--border); background: var(--surface);
      color: var(--muted); cursor: pointer; font-size: 0.88rem; font-weight: 500; transition: all 0.15s ease;
    }
    .filter-btn.active, .filter-btn:hover { background: var(--accent); color: #ffffff; border-color: var(--accent); }
    .entry-card {
      background: var(--surface); border: 1px solid var(--border); border-radius: 10px;
      padding: 1.25rem 1.5rem; margin-bottom: 1rem; transition: background 0.15s ease;
    }
    .entry-card:hover { background: var(--card-hover); }
    .entry-header { display: flex; align-items: baseline; justify-content: space-between; gap: 1rem; }
    .entry-title { font-size: 1.2rem; font-weight: 600; color: var(--text); text-decoration: none; }
    .entry-title:hover { color: var(--accent); }
    .entry-date { font-size: 0.85rem; color: var(--muted); white-space: nowrap; }
    .entry-author { color: var(--muted); font-size: 0.95rem; margin: 0.2rem 0 0.6rem; }
    .entry-badges { display: flex; flex-wrap: wrap; gap: 0.4rem; margin-top: 0.6rem; font-size: 0.8rem; }
    .badge { background: var(--badge-bg); padding: 0.2rem 0.55rem; border-radius: 5px; color: var(--muted); }
    .actions { display: flex; gap: 1.2rem; margin-top: 0.8rem; }
    .action-link { font-size: 0.85rem; color: var(--accent); text-decoration: none; font-weight: 500; }
    .action-link:hover { text-decoration: underline; }
    .empty-state {
      text-align: center; padding: 3rem 1rem; background: var(--surface); border: 1px solid var(--border);
      border-radius: 10px; color: var(--muted);
    }"#;

    let body = format!(
        r#"  <div class="container">
    <header class="hero">
      <h1>srz &bull; Summary Library</h1>
      <p>Personal searchable reading library and distillation archive</p>
    </header>

    <div class="stats-bar">
      <div class="stat-card"><div class="stat-val">{total_summaries}</div><div class="stat-lbl">Total Summaries</div></div>
      <div class="stat-card"><div class="stat-val">{book_summaries}</div><div class="stat-lbl">Books</div></div>
      <div class="stat-card"><div class="stat-val">{web_summaries}</div><div class="stat-lbl">Web Articles</div></div>
      <div class="stat-card"><div class="stat-val">{file_summaries}</div><div class="stat-lbl">Files</div></div>
      <div class="stat-card"><div class="stat-val">{total_words}</div><div class="stat-lbl">Words Read</div></div>
    </div>

    <div class="controls">
      <input type="text" id="search" class="search-box" placeholder="Search summaries by title, author, or model...">
{filter_buttons}    </div>

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
"#,
        total_summaries = stats.total_summaries,
        book_summaries = stats.book_summaries,
        web_summaries = stats.web_summaries,
        file_summaries = stats.file_summaries,
        total_words = stats.total_source_words,
    );
    render_html_page(
        "srz &bull; Summary Library",
        "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline';",
        INDEX_CSS,
        &body,
    )
}

pub(crate) fn ensure_index_file(
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

#[cfg(test)]
mod tests {
    use super::super::markdown::is_safe_markdown_url;
    use super::*;

    #[test]
    fn test_markdown_url_and_xss_sanitization() {
        for safe in [
            "https://example.com",
            "http://example.com/page",
            "mailto:user@example.com",
            "#anchor",
            "/relative/path",
            "relative/doc.html",
        ] {
            assert!(is_safe_markdown_url(safe), "should allow {safe}");
        }
        for dangerous in [
            "javascript:alert(1)",
            "JAVASCRIPT:alert('xss')",
            "vbscript:msgbox(1)",
            "data:text/html,<script>alert(1)</script>",
            "file:///etc/passwd",
            "",
            "   ",
            "http://example.com/\0evil",
            "//evil.example/phish",
            "/\\evil.example/smb",
            "\\\\server\\share",
        ] {
            assert!(!is_safe_markdown_url(dangerous), "should block {dangerous}");
        }

        let html = markdown_to_html(
            "[Safe](https://example.com) [Bad](javascript:alert(1)) [Data](data:text/html;base64,PHNjcmlwdD4=)",
        );
        assert!(html.contains(r#"href="https://example.com""#));
        assert!(!html.contains("javascript:") && !html.contains("data:text/html"));
        assert!(html.contains(r##"href="#"##));
    }

    #[test]
    fn test_html_document_gfm_and_view_files() {
        let md = "# Dune\n\n***\n\n## Overview\n\n*Fiction*\n\n- A\n\n| H1 | H2 |\n|---|---|\n| C1 | C2 |\n\n~~old~~";
        let full_doc = render_html_document_with_reasoning("\"O'Reilly\"", md, Some("Trace..."));
        for expected in [
            "<!DOCTYPE html>",
            "Summary: &quot;O&#39;Reilly&quot;",
            "img-src 'self' data:;",
            "border-collapse: collapse;",
            "thought-drawer",
            "Trace...",
            "<h1>Dune</h1>",
            "<table>",
            "<th>H1</th>",
            "<td>C1</td>",
            "<del>old</del>",
        ] {
            assert!(full_doc.contains(expected), "missing {expected}");
        }

        let tmp = crate::fs_util::tempdir().unwrap();
        let rendered =
            ensure_view_files_unique(tmp.path(), "Dune", "Init", None, None, true).unwrap();
        assert_eq!(std::fs::read_to_string(&rendered.md_path).unwrap(), "Init");
        let reused =
            ensure_view_files_unique(tmp.path(), "Dune", "New", None, None, false).unwrap();
        assert_eq!(std::fs::read_to_string(&reused.md_path).unwrap(), "Init");
        let overwrote =
            ensure_view_files_unique(tmp.path(), "Dune", "New", None, None, true).unwrap();
        assert_eq!(std::fs::read_to_string(&overwrote.md_path).unwrap(), "New");

        #[cfg(unix)]
        {
            std::fs::remove_file(&overwrote.md_path).unwrap();
            std::os::unix::fs::symlink(tmp.path().join("other"), &overwrote.md_path).unwrap();
            assert!(ensure_view_files_unique(tmp.path(), "Dune", "X", None, None, false).is_err());
        }
    }

    #[test]
    fn test_target_meta_and_index_dashboard() {
        assert_eq!(target_icon("unknown"), "📝");
        assert_eq!(target_badge("unknown"), "📝 Doc");

        let stats = LibraryStats {
            total_summaries: 2,
            book_summaries: 1,
            web_summaries: 1,
            total_source_words: 5000,
            ..LibraryStats::default()
        };
        let entries = vec![
            HistoryEntry {
                id: 1,
                target_type: "book".into(),
                title: "Dune".into(),
                author: Some("Frank Herbert".into()),
                url: None,
                style: "concise".into(),
                model: "gemma-4".into(),
                word_count: 3000,
                prompt_tokens: Some(2000),
                completion_tokens: Some(400),
                duration_ms: 6000,
                tokens_per_second: Some(45.0),
                created_at: "2026-10-28 10:00".into(),
            },
            HistoryEntry {
                id: 2,
                target_type: "web".into(),
                title: "Rust 2026".into(),
                author: Some("   ".into()),
                url: Some("https://rust-lang.org".into()),
                style: "bullets".into(),
                model: "gemma-4".into(),
                word_count: 2000,
                prompt_tokens: None,
                completion_tokens: None,
                duration_ms: 6000,
                tokens_per_second: None,
                created_at: "2026-10-28 10:15".into(),
            },
        ];

        let html = render_index_html(&stats, &entries);
        assert!(html.contains("Dune") && html.contains("by Frank Herbert"));
        assert!(!html.contains("by    "));
        assert!(html.contains("data-filter=\"book\""));
        let web_slug = slugify_unique("Rust 2026", Some("https://rust-lang.org"));
        assert!(html.contains(&format!("href=\"./{web_slug}.html\"")));

        let tmp = crate::fs_util::tempdir().unwrap();
        let index_file = ensure_index_file(tmp.path(), &stats, &entries).unwrap();
        assert_eq!(std::fs::read_to_string(&index_file).unwrap(), html);
    }
}
