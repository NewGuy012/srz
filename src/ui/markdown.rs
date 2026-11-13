//! Lightweight CommonMark/GFM subset parser for HTML and ANSI terminal rendering.

use std::borrow::Cow;

/// Returns `true` if `url` is a safe web, mailto, or relative anchor URL for HTML rendering.
/// Rejects dangerous schemes (`javascript:`, `vbscript:`, `data:`, `file:`) and control characters.
pub fn is_safe_markdown_url(url: &str) -> bool {
    let trimmed = url.trim();
    if trimmed.is_empty() || trimmed.chars().any(|c| c.is_control()) {
        return false;
    }
    if trimmed.starts_with('#') || trimmed.starts_with('/') {
        return true;
    }
    if let Some(colon_pos) = trimmed.find(':') {
        let slash_pos = trimmed.find('/');
        let query_pos = trimmed.find('?');
        let hash_pos = trimmed.find('#');
        let first_delimiter = [slash_pos, query_pos, hash_pos]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(usize::MAX);
        if colon_pos < first_delimiter {
            let scheme = &trimmed[..colon_pos];
            return scheme.eq_ignore_ascii_case("http")
                || scheme.eq_ignore_ascii_case("https")
                || scheme.eq_ignore_ascii_case("mailto");
        }
    }
    true
}

/// Converts CommonMark/GFM Markdown into sanitized HTML (escaping raw HTML tags and blocking dangerous link schemes).
pub fn markdown_to_html(md: &str) -> String {
    let mut out = String::with_capacity(md.len() * 3 / 2);
    render_blocks(md, &mut out, true);
    out
}

/// Renders Markdown into ANSI-formatted terminal text after stripping unsafe control/escape sequences.
pub fn render_markdown(markdown: &str) -> String {
    let safe = crate::ui::sanitize_terminal_text_cow(markdown);
    let mut out = String::with_capacity(safe.len() * 2);
    render_blocks(safe.as_ref(), &mut out, false);
    out.trim_end().to_string()
}

fn push_escaped(out: &mut String, text: &str, html: bool) {
    if !html {
        out.push_str(text);
        return;
    }
    for ch in text.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

fn render_blocks(md: &str, out: &mut String, html: bool) {
    let lines: Vec<&str> = md.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim();

        if trimmed.is_empty() {
            i += 1;
            continue;
        }

        // 1. Fenced code block (``` or ~~~)
        if let Some((fence_char, fence_len, info)) = parse_fence_open(trimmed) {
            i += 1;
            let code_start = i;
            while i < lines.len() && !is_fence_close(lines[i].trim(), fence_char, fence_len) {
                i += 1;
            }
            let code_lines = &lines[code_start..i];
            if i < lines.len() {
                i += 1; // consume closing fence
            }
            if html {
                let lang = info.split_whitespace().next().unwrap_or("");
                if lang.is_empty() {
                    out.push_str("<pre><code>");
                } else {
                    out.push_str("<pre><code class=\"language-");
                    push_escaped(out, lang, true);
                    out.push_str("\">");
                }
                for code_line in code_lines {
                    push_escaped(out, code_line, true);
                    out.push('\n');
                }
                out.push_str("</code></pre>\n");
            } else {
                out.push_str("\x1b[32m");
                for code_line in code_lines {
                    out.push_str(code_line);
                    out.push('\n');
                }
                out.push_str("\x1b[0m\n\n");
            }
            continue;
        }

        // 2. Thematic break (---, ***, ___)
        if is_thematic_break(trimmed) {
            i += 1;
            if html {
                out.push_str("<hr>\n");
            } else {
                out.push_str("\x1b[2m──────────\x1b[0m\n\n");
            }
            continue;
        }

        // 3. ATX Heading (# .. ######)
        if let Some((level, heading_text)) = parse_heading(trimmed) {
            i += 1;
            if html {
                out.push_str(&format!("<h{}>", level));
                render_inlines(heading_text, out, true);
                out.push_str(&format!("</h{}>\n", level));
            } else {
                match level {
                    1 => out.push_str("\x1b[1;36;4m"),
                    2 => out.push_str("\x1b[1;36m"),
                    _ => out.push_str("\x1b[1;33m"),
                }
                render_inlines(heading_text, out, false);
                out.push_str("\x1b[0m\n\n");
            }
            continue;
        }

        // 4. GFM Table
        if i + 1 < lines.len()
            && line.contains('|')
            && let Some(alignments) = parse_table_separator(lines[i + 1])
        {
            let headers = split_table_row(line);
            i += 2;
            let mut rows = Vec::new();
            while i < lines.len() {
                let r = lines[i].trim();
                if r.is_empty() || !r.contains('|') {
                    break;
                }
                rows.push(split_table_row(r));
                i += 1;
            }
            render_table(&headers, &alignments, &rows, out, html);
            continue;
        }

        // 5. Blockquote (> ...)
        if line.trim_start().starts_with('>') {
            let mut quote_buf = String::new();
            while i < lines.len() {
                let ql = lines[i].trim_start();
                if let Some(rest) = ql.strip_prefix('>') {
                    let body = rest.strip_prefix(' ').unwrap_or(rest);
                    quote_buf.push_str(body);
                    quote_buf.push('\n');
                    i += 1;
                } else {
                    break;
                }
            }
            if html {
                out.push_str("<blockquote>\n");
                render_blocks(&quote_buf, out, true);
                out.push_str("</blockquote>\n");
            } else {
                let mut inner = String::new();
                render_blocks(&quote_buf, &mut inner, false);
                for ql in inner.trim_end().lines() {
                    out.push_str("\x1b[2m│ \x1b[0m");
                    out.push_str(ql);
                    out.push('\n');
                }
                out.push('\n');
            }
            continue;
        }

        // 6. Ordered / Unordered List
        if parse_list_marker(line).is_some() {
            i = render_list(&lines, i, out, html);
            continue;
        }

        // 7. Paragraph
        let para_start = i;
        while i < lines.len() {
            let cur = lines[i];
            let cur_trimmed = cur.trim();
            if cur_trimmed.is_empty()
                || parse_fence_open(cur_trimmed).is_some()
                || is_thematic_break(cur_trimmed)
                || parse_heading(cur_trimmed).is_some()
                || cur.trim_start().starts_with('>')
                || parse_list_marker(cur).is_some()
                || (i + 1 < lines.len()
                    && cur.contains('|')
                    && parse_table_separator(lines[i + 1]).is_some())
            {
                break;
            }
            i += 1;
        }

        if html {
            out.push_str("<p>");
        }
        for (idx, p_line) in lines[para_start..i].iter().enumerate() {
            let trimmed_start = p_line.trim_start();
            let (content, hard_break) = if let Some(stripped) = trimmed_start.strip_suffix("  ") {
                (stripped, true)
            } else if let Some(stripped) = trimmed_start.strip_suffix('\\') {
                (stripped, true)
            } else {
                (trimmed_start, false)
            };
            render_inlines(content, out, html);
            if idx + 1 < i - para_start {
                if hard_break {
                    if html {
                        out.push_str("<br />\n");
                    } else {
                        out.push('\n');
                    }
                } else if html {
                    out.push('\n');
                } else {
                    out.push(' ');
                }
            }
        }
        if html {
            out.push_str("</p>\n");
        } else {
            out.push_str("\n\n");
        }
    }
}

fn parse_fence_open(trimmed: &str) -> Option<(u8, usize, &str)> {
    let first = *trimmed.as_bytes().first()?;
    if first != b'`' && first != b'~' {
        return None;
    }
    let count = trimmed.bytes().take_while(|&b| b == first).count();
    if count < 3 {
        return None;
    }
    let info = trimmed[count..].trim();
    if first == b'`' && info.contains('`') {
        return None;
    }
    Some((first, count, info))
}

fn is_fence_close(trimmed: &str, fence_char: u8, min_len: usize) -> bool {
    let count = trimmed.bytes().take_while(|&b| b == fence_char).count();
    count >= min_len && trimmed[count..].trim().is_empty()
}

fn is_thematic_break(trimmed: &str) -> bool {
    let mut marker: Option<char> = None;
    let mut count = 0usize;
    for ch in trimmed.chars() {
        if ch == ' ' || ch == '\t' {
            continue;
        }
        if ch == '-' || ch == '*' || ch == '_' {
            if let Some(m) = marker {
                if m != ch {
                    return false;
                }
            } else {
                marker = Some(ch);
            }
            count += 1;
        } else {
            return false;
        }
    }
    count >= 3
}

fn parse_heading(trimmed: &str) -> Option<(usize, &str)> {
    let level = trimmed.bytes().take_while(|&b| b == b'#').count();
    if level == 0 || level > 6 {
        return None;
    }
    let rest = &trimmed[level..];
    if !rest.is_empty() && !rest.starts_with([' ', '\t']) {
        return None;
    }
    let text = rest.trim();
    let stripped = text.trim_end_matches('#');
    let final_text = if stripped.len() < text.len()
        && (stripped.is_empty() || stripped.ends_with([' ', '\t']))
    {
        stripped.trim()
    } else {
        text
    };
    Some((level, final_text))
}

#[derive(Clone, Copy)]
enum ColAlign {
    None,
    Left,
    Center,
    Right,
}

fn split_table_row(line: &str) -> Vec<&str> {
    let mut trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix('|') {
        trimmed = rest;
    }
    if let Some(rest) = trimmed.strip_suffix('|') {
        trimmed = rest;
    }
    trimmed.split('|').map(str::trim).collect()
}

fn parse_table_separator(line: &str) -> Option<Vec<ColAlign>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }
    let cells = split_table_row(trimmed);
    if cells.is_empty() {
        return None;
    }
    let mut aligns = Vec::with_capacity(cells.len());
    for cell in cells {
        if cell.is_empty() {
            return None;
        }
        let left = cell.starts_with(':');
        let right = cell.ends_with(':');
        let dashes = cell.trim_matches(':');
        if dashes.is_empty() || !dashes.bytes().all(|b| b == b'-') {
            return None;
        }
        aligns.push(match (left, right) {
            (true, true) => ColAlign::Center,
            (true, false) => ColAlign::Left,
            (false, true) => ColAlign::Right,
            (false, false) => ColAlign::None,
        });
    }
    Some(aligns)
}

fn align_style(align: ColAlign) -> &'static str {
    match align {
        ColAlign::Left => " style=\"text-align: left\"",
        ColAlign::Center => " style=\"text-align: center\"",
        ColAlign::Right => " style=\"text-align: right\"",
        ColAlign::None => "",
    }
}

fn render_table_row_html(tag: &str, cells: &[&str], alignments: &[ColAlign], out: &mut String) {
    out.push_str("<tr>");
    for (c, &align) in alignments.iter().enumerate() {
        out.push('<');
        out.push_str(tag);
        out.push_str(align_style(align));
        out.push('>');
        if let Some(cell) = cells.get(c) {
            render_inlines(cell, out, true);
        }
        out.push_str("</");
        out.push_str(tag);
        out.push('>');
    }
    out.push_str("</tr>");
}

fn render_table(
    headers: &[&str],
    alignments: &[ColAlign],
    rows: &[Vec<&str>],
    out: &mut String,
    html: bool,
) {
    let cols = alignments.len();
    if html {
        out.push_str("<table><thead>");
        render_table_row_html("th", headers, alignments, out);
        out.push_str("</thead><tbody>\n");
        for row in rows {
            render_table_row_html("td", row, alignments, out);
            out.push('\n');
        }
        out.push_str("</tbody></table>\n");
    } else {
        out.push_str("\x1b[1m");
        for c in 0..cols {
            if c > 0 {
                out.push_str(" | ");
            }
            if let Some(cell) = headers.get(c) {
                render_inlines(cell, out, false);
            }
        }
        out.push_str("\x1b[0m\n");
        for row in rows {
            for c in 0..cols {
                if c > 0 {
                    out.push_str(" | ");
                }
                if let Some(cell) = row.get(c) {
                    render_inlines(cell, out, false);
                }
            }
            out.push('\n');
        }
        out.push('\n');
    }
}

struct ListMarker<'a> {
    indent: usize,
    ordered: bool,
    number: &'a str,
    content: &'a str,
}

fn parse_list_marker(line: &str) -> Option<ListMarker<'_>> {
    let indent = line.bytes().take_while(|&b| b == b' ').count();
    let rest = &line[indent..];

    if let Some(after) = rest
        .strip_prefix("- ")
        .or_else(|| rest.strip_prefix("* "))
        .or_else(|| rest.strip_prefix("+ "))
    {
        return Some(ListMarker {
            indent,
            ordered: false,
            number: "",
            content: after,
        });
    }

    let digits_len = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits_len > 0 && digits_len <= 9 {
        let after_digits = &rest[digits_len..];
        if let Some(after) = after_digits
            .strip_prefix(". ")
            .or_else(|| after_digits.strip_prefix(") "))
        {
            return Some(ListMarker {
                indent,
                ordered: true,
                number: &rest[..digits_len],
                content: after,
            });
        }
    }

    None
}

fn render_list(lines: &[&str], start_idx: usize, out: &mut String, html: bool) -> usize {
    let mut stack: Vec<(usize, bool)> = Vec::new();
    let mut i = start_idx;

    while i < lines.len() {
        let Some(marker) = parse_list_marker(lines[i]) else {
            break;
        };

        while let Some(&(top_indent, top_ord)) = stack.last() {
            if marker.indent < top_indent
                || (marker.indent == top_indent && marker.ordered != top_ord)
            {
                stack.pop();
                if html {
                    out.push_str(if top_ord { "</ol>\n" } else { "</ul>\n" });
                }
            } else {
                break;
            }
        }

        if stack
            .last()
            .is_none_or(|&(top_indent, _)| marker.indent > top_indent)
        {
            stack.push((marker.indent, marker.ordered));
            if html {
                out.push_str(if marker.ordered { "<ol>\n" } else { "<ul>\n" });
            }
        }

        let depth = stack.len();
        let (task_checkbox, body) = parse_task_prefix(marker.content);

        if html {
            out.push_str("<li>");
            if let Some(checked) = task_checkbox {
                if checked {
                    out.push_str("<input disabled=\"\" type=\"checkbox\" checked=\"\"/> ");
                } else {
                    out.push_str("<input disabled=\"\" type=\"checkbox\"/> ");
                }
            }
            render_inlines(body, out, true);
            out.push_str("</li>\n");
        } else {
            for _ in 0..depth.saturating_sub(1) {
                out.push_str("  ");
            }
            if marker.ordered {
                out.push_str(&format!("\x1b[36m{}.\x1b[0m ", marker.number));
            } else {
                out.push_str("\x1b[36m•\x1b[0m ");
            }
            if let Some(checked) = task_checkbox {
                out.push_str(if checked { "[x] " } else { "[ ] " });
            }
            render_inlines(body, out, false);
            out.push('\n');
        }

        i += 1;
    }

    while let Some((_, ord)) = stack.pop() {
        if html {
            out.push_str(if ord { "</ol>\n" } else { "</ul>\n" });
        }
    }
    if !html {
        out.push('\n');
    }
    i
}

fn parse_task_prefix(content: &str) -> (Option<bool>, &str) {
    if let Some(rest) = content.strip_prefix("[ ] ") {
        (Some(false), rest)
    } else if let Some(rest) = content
        .strip_prefix("[x] ")
        .or_else(|| content.strip_prefix("[X] "))
    {
        (Some(true), rest)
    } else {
        (None, content)
    }
}

fn render_inlines(text: &str, out: &mut String, html: bool) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        // 1. Backslash escape for ASCII punctuation
        if bytes[i] == b'\\' && i + 1 < len && bytes[i + 1].is_ascii_punctuation() {
            push_escaped(out, &text[i + 1..i + 2], html);
            i += 2;
            continue;
        }

        // 2. Inline code span (`...` / ``...``)
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if let Some(close_rel) = find_closing_backticks(&text[i + ticks..], ticks) {
                let code = &text[i + ticks..i + ticks + close_rel];
                let code_trimmed = if code.len() >= 2
                    && code.starts_with(' ')
                    && code.ends_with(' ')
                    && code.bytes().any(|b| b != b' ')
                {
                    &code[1..code.len() - 1]
                } else {
                    code
                };
                if html {
                    out.push_str("<code>");
                    push_escaped(out, code_trimmed, true);
                    out.push_str("</code>");
                } else {
                    out.push_str("\x1b[32m");
                    out.push_str(code_trimmed);
                    out.push_str("\x1b[0m");
                }
                i += ticks + close_rel + ticks;
                continue;
            }
            for _ in 0..ticks {
                out.push('`');
            }
            i += ticks;
            continue;
        }

        // 3. Images ![alt](url) and Links [text](url)
        if (bytes[i] == b'!' && bytes.get(i + 1) == Some(&b'[') || bytes[i] == b'[')
            && let Some((is_img, label, dest, end_idx)) = parse_link_or_image(text, i)
        {
            let safe_url: Cow<'_, str> = if is_safe_markdown_url(dest) {
                Cow::Borrowed(dest.trim())
            } else {
                Cow::Borrowed("#")
            };
            if html {
                if is_img {
                    out.push_str("<img src=\"");
                    push_escaped(out, &safe_url, true);
                    out.push_str("\" alt=\"");
                    push_escaped(out, label, true);
                    out.push_str("\" />");
                } else {
                    out.push_str("<a href=\"");
                    push_escaped(out, &safe_url, true);
                    out.push_str("\">");
                    render_inlines(label, out, true);
                    out.push_str("</a>");
                }
            } else if is_img {
                out.push_str(label);
            } else {
                render_inlines(label, out, false);
            }
            i = end_idx;
            continue;
        }

        // 4. Strikethrough ~~...~~
        if bytes[i] == b'~'
            && bytes.get(i + 1) == Some(&b'~')
            && let Some(close_rel) = find_delim_close(&text[i + 2..], "~~")
            && close_rel > 0
        {
            let inner = &text[i + 2..i + 2 + close_rel];
            if html {
                out.push_str("<del>");
                render_inlines(inner, out, true);
                out.push_str("</del>");
            } else {
                out.push_str("\x1b[9m");
                render_inlines(inner, out, false);
                out.push_str("\x1b[0m");
            }
            i += 2 + close_rel + 2;
            continue;
        }

        // 5. Bold / Italic (*, **, ***, _, __, ___)
        if (bytes[i] == b'*' || bytes[i] == b'_')
            && let Some((open_tag, close_tag, ansi_seq, inner, next_idx)) =
                try_parse_emphasis(text, i)
        {
            if html {
                out.push_str(open_tag);
                render_inlines(inner, out, true);
                out.push_str(close_tag);
            } else {
                out.push_str(ansi_seq);
                render_inlines(inner, out, false);
                out.push_str("\x1b[0m");
            }
            i = next_idx;
            continue;
        }

        let ch = text[i..].chars().next().unwrap_or('\0');
        let mut buf = [0u8; 4];
        push_escaped(out, ch.encode_utf8(&mut buf), html);
        i += ch.len_utf8();
    }
}

fn find_closing_backticks(rest: &str, count: usize) -> Option<usize> {
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            let run = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if run == count {
                return Some(i);
            }
            i += run;
        } else {
            i += 1;
        }
    }
    None
}

fn parse_link_or_image(text: &str, start: usize) -> Option<(bool, &str, &str, usize)> {
    let bytes = text.as_bytes();
    let is_img = bytes[start] == b'!';
    let bracket_start = if is_img { start + 1 } else { start };
    if bytes.get(bracket_start) != Some(&b'[') {
        return None;
    }

    let mut i = bracket_start + 1;
    let mut depth = 1usize;
    let mut label_end = None;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'[' {
            depth += 1;
        } else if bytes[i] == b']' {
            depth -= 1;
            if depth == 0 {
                label_end = Some(i);
                break;
            }
        }
        i += 1;
    }
    let label_end = label_end?;
    if bytes.get(label_end + 1) != Some(&b'(') {
        return None;
    }

    let mut j = label_end + 2;
    let mut paren_depth = 1usize;
    let mut dest_end = None;
    while j < bytes.len() {
        if bytes[j] == b'\\' && j + 1 < bytes.len() {
            j += 2;
            continue;
        }
        if bytes[j] == b'(' {
            paren_depth += 1;
        } else if bytes[j] == b')' {
            paren_depth -= 1;
            if paren_depth == 0 {
                dest_end = Some(j);
                break;
            }
        }
        j += 1;
    }
    let dest_end = dest_end?;
    let label = &text[bracket_start + 1..label_end];
    let raw_dest = text[label_end + 2..dest_end].trim();
    let dest = strip_optional_link_title(raw_dest);
    Some((is_img, label, dest, dest_end + 1))
}

fn strip_optional_link_title(raw: &str) -> &str {
    if let Some(after) = raw.strip_prefix('<')
        && let Some(end) = after.find('>')
    {
        return &after[..end];
    }
    raw.split_whitespace().next().unwrap_or(raw)
}

fn find_delim_close(rest: &str, delim: &str) -> Option<usize> {
    let bytes = rest.as_bytes();
    let d_bytes = delim.as_bytes();
    let mut i = 0;
    while i + d_bytes.len() <= bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 2;
            continue;
        }
        if bytes[i] == b'`' {
            let ticks = bytes[i..].iter().take_while(|&&b| b == b'`').count();
            if let Some(close) = find_closing_backticks(&rest[i + ticks..], ticks) {
                i += ticks + close + ticks;
                continue;
            }
            i += ticks;
            continue;
        }
        if &bytes[i..i + d_bytes.len()] == d_bytes {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn is_word_byte(b: Option<u8>) -> bool {
    b.is_some_and(|c| c.is_ascii_alphanumeric())
}

fn try_parse_emphasis(
    text: &str,
    start: usize,
) -> Option<(&'static str, &'static str, &'static str, &str, usize)> {
    let bytes = text.as_bytes();
    let ch = bytes[start];
    let run = bytes[start..].iter().take_while(|&&b| b == ch).count();

    if ch == b'_' && is_word_byte(start.checked_sub(1).and_then(|p| bytes.get(p).copied())) {
        return None;
    }

    let check_len = if run >= 3 {
        3
    } else if run == 2 {
        2
    } else {
        1
    };

    for delim_len in (1..=check_len).rev() {
        let open_end = start + delim_len;
        let Some(&next_b) = bytes.get(open_end) else {
            continue;
        };
        if next_b.is_ascii_whitespace() {
            continue;
        }

        let delim_str = match (ch, delim_len) {
            (b'*', 3) => "***",
            (b'*', 2) => "**",
            (b'*', 1) => "*",
            (b'_', 3) => "___",
            (b'_', 2) => "__",
            (b'_', 1) => "_",
            _ => continue,
        };

        let mut search_offset = open_end;
        while let Some(rel) = find_delim_close(&text[search_offset..], delim_str) {
            let close_pos = search_offset + rel;
            if close_pos == open_end {
                search_offset = close_pos + 1;
                continue;
            }
            // Reject single '*' or '_' matching inside '**' or '__' unless followed by closing
            let prev_b = bytes.get(close_pos - 1).copied();
            let after_close = bytes.get(close_pos + delim_len).copied();
            let prev_ws = prev_b.is_some_and(|b| b.is_ascii_whitespace());
            let underscore_intra = ch == b'_' && is_word_byte(after_close);
            let longer_run = delim_len == 1 && after_close == Some(ch);
            if prev_ws || underscore_intra || longer_run {
                let skip = bytes[close_pos..].iter().take_while(|&&b| b == ch).count();
                search_offset = close_pos + skip.max(1);
                continue;
            }

            let inner = &text[open_end..close_pos];
            let (open_tag, close_tag, ansi_seq) = match delim_len {
                3 => ("<em><strong>", "</strong></em>", "\x1b[1;35;37m"),
                2 => ("<strong>", "</strong>", "\x1b[1;37m"),
                _ => ("<em>", "</em>", "\x1b[35m"),
            };
            return Some((open_tag, close_tag, ansi_seq, inner, close_pos + delim_len));
        }
    }

    None
}
