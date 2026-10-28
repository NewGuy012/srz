use std::fs;
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

    fs::create_dir_all(views_dir)?;
    fs::write(&md_path, markdown)?;

    let html_doc = render_html_document(title, markdown);
    fs::write(&html_path, html_doc)?;

    Ok(RenderedFiles { html_path, md_path })
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
            _ => out.push(ch),
        }
    }
    out
}

/// Renders inline Markdown (`**bold**`, `*italic*`, `` `code` ``) after HTML-escaping.
fn render_inline(text: &str) -> String {
    let escaped = escape_html(text);
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
            out.push_str(&render_inline(&escaped[i + 2..i + 2 + end]));
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
