use anyhow::Result;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use std::io::{Stdout, Write, stdout};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::cache::Cache;
use crate::render;
use crate::storage::db::{Database, LibraryItem};
use crate::ui::terminal::{char_display_width, wrap_terminal_text};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    List,
    Preview,
}

struct AppState {
    items: Vec<LibraryItem>,
    filtered_indices: Vec<usize>,
    selected: Option<usize>,
    list_offset: usize,
    scroll_offset: u16,
    filter_query: String,
    is_filtering: bool,
    focus: FocusPane,
    status_message: Option<(String, Instant)>,
    views_dir: PathBuf,
}

impl AppState {
    fn new(mut items: Vec<LibraryItem>, views_dir: PathBuf) -> Self {
        for item in &mut items {
            item.title = crate::ui::sanitize_title(&item.title);
            item.model = crate::ui::sanitize_title(&item.model);
            item.style = crate::ui::sanitize_title(&item.style);
            item.created_at = crate::ui::sanitize_title(&item.created_at);
            if let Some(ref mut author) = item.author {
                *author = crate::ui::sanitize_title(author);
            }
            if let Some(ref mut url) = item.url {
                *url = crate::ui::sanitize_title(url);
            }
            item.summary = crate::ui::sanitize_terminal_text(&item.summary);
        }

        let indices: Vec<usize> = (0..items.len()).collect();
        let selected = (!indices.is_empty()).then_some(0);
        Self {
            items,
            filtered_indices: indices,
            selected,
            list_offset: 0,
            scroll_offset: 0,
            filter_query: String::new(),
            is_filtering: false,
            focus: FocusPane::List,
            status_message: None,
            views_dir,
        }
    }

    fn apply_filter(&mut self) {
        let q = self.filter_query.trim();
        self.filtered_indices.clear();
        if q.is_empty() {
            self.filtered_indices.extend(0..self.items.len());
        } else {
            for (idx, item) in self.items.iter().enumerate() {
                if item.matches_query(q) {
                    self.filtered_indices.push(idx);
                }
            }
        }
        self.selected = (!self.filtered_indices.is_empty()).then_some(0);
        self.list_offset = 0;
        self.scroll_offset = 0;
    }

    fn selected_item(&self) -> Option<&LibraryItem> {
        self.selected
            .and_then(|sel| self.filtered_indices.get(sel))
            .and_then(|&idx| self.items.get(idx))
    }

    fn step_selection(&mut self, forward: bool) {
        let len = self.filtered_indices.len();
        if len == 0 {
            return;
        }
        let current = self.selected.unwrap_or(0);
        let next = if forward {
            (current + 1) % len
        } else if current > 0 {
            current - 1
        } else {
            len - 1
        };
        self.selected = Some(next);
        self.scroll_offset = 0;
    }

    fn set_status(&mut self, msg: impl Into<String>) {
        self.status_message = Some((msg.into(), Instant::now()));
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TuiAction {
    #[default]
    Exit,
    Ask {
        target: String,
    },
}

struct TerminalSessionGuard;

impl TerminalSessionGuard {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut out = stdout();
        execute!(out, EnterAlternateScreen, cursor::Hide)?;
        Ok(Self)
    }
}

impl Drop for TerminalSessionGuard {
    fn drop(&mut self) {
        let mut out = stdout();
        let _ = disable_raw_mode();
        let _ = execute!(out, LeaveAlternateScreen, cursor::Show);
    }
}

/// Runs the interactive terminal UI library explorer.
pub fn run_tui(db: &Database, cache: &Cache) -> Result<TuiAction> {
    let _guard = TerminalSessionGuard::new()?;
    let mut out = stdout();
    let items = db.get_all_library_items().unwrap_or_default();
    let mut state = AppState::new(items, cache.views_dir());
    run_loop(&mut out, &mut state, db)
}

fn run_loop(out: &mut Stdout, state: &mut AppState, db: &Database) -> Result<TuiAction> {
    let mut needs_redraw = true;
    loop {
        if let Some((_, created)) = &state.status_message
            && created.elapsed() > Duration::from_secs(3)
        {
            state.status_message = None;
            needs_redraw = true;
        }

        if needs_redraw {
            let (cols, rows) = crossterm::terminal::size().unwrap_or((80, 24));
            let frame = render_frame(cols as usize, rows as usize, state);
            out.write_all(frame.as_bytes())?;
            out.flush()?;
            needs_redraw = false;
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Resize(_, _) => needs_redraw = true,
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    needs_redraw = true;
                    if state.is_filtering {
                        match key.code {
                            KeyCode::Esc | KeyCode::Enter => state.is_filtering = false,
                            KeyCode::Backspace => {
                                state.filter_query.pop();
                                state.apply_filter();
                            }
                            KeyCode::Char(c) => {
                                state.filter_query.push(c);
                                state.apply_filter();
                            }
                            _ => {}
                        }
                        continue;
                    }

                    match key.code {
                        KeyCode::Char('q') => return Ok(TuiAction::Exit),
                        KeyCode::Esc => {
                            if !state.filter_query.is_empty() {
                                state.filter_query.clear();
                                state.apply_filter();
                                state.set_status("Filter cleared");
                            } else {
                                return Ok(TuiAction::Exit);
                            }
                        }
                        KeyCode::Char('a') => {
                            if let Some(item) = state.selected_item() {
                                let target = item.url.clone().unwrap_or_else(|| item.title.clone());
                                return Ok(TuiAction::Ask { target });
                            }
                        }
                        KeyCode::Char('/') => state.is_filtering = true,
                        KeyCode::Tab => {
                            state.focus = match state.focus {
                                FocusPane::List => FocusPane::Preview,
                                FocusPane::Preview => FocusPane::List,
                            };
                        }
                        KeyCode::Up | KeyCode::Char('k') => match state.focus {
                            FocusPane::List => state.step_selection(false),
                            FocusPane::Preview => {
                                state.scroll_offset = state.scroll_offset.saturating_sub(2);
                            }
                        },
                        KeyCode::Down | KeyCode::Char('j') => match state.focus {
                            FocusPane::List => state.step_selection(true),
                            FocusPane::Preview => {
                                state.scroll_offset = state.scroll_offset.saturating_add(2);
                            }
                        },
                        KeyCode::PageUp | KeyCode::Char('K') => {
                            state.scroll_offset = state.scroll_offset.saturating_sub(10);
                        }
                        KeyCode::PageDown | KeyCode::Char('J') => {
                            state.scroll_offset = state.scroll_offset.saturating_add(10);
                        }
                        KeyCode::Char('y') | KeyCode::Char('c') => {
                            if let Some(item) = state.selected_item() {
                                if copy_to_clipboard(&item.summary).is_ok() {
                                    state.set_status("✓ Summary copied to clipboard");
                                } else {
                                    state.set_status("⚠ Clipboard unavailable");
                                }
                            }
                        }
                        KeyCode::Enter | KeyCode::Char('b') => {
                            if let Some(item) = state.selected_item() {
                                let files = render::ensure_view_files_unique(
                                    &state.views_dir,
                                    &item.title,
                                    &item.summary,
                                    item.url.as_deref(),
                                    item.reasoning.as_deref(),
                                    false,
                                );
                                if let Ok(files) = files {
                                    if render::open_in_browser(&files.html_path).is_ok() {
                                        state.set_status(format!("✓ Opened {}", item.title));
                                    } else {
                                        state.set_status("⚠ Could not open default browser");
                                    }
                                }
                            }
                        }
                        KeyCode::Char('d') => {
                            if let Some(item) = state.selected_item() {
                                let id = item.id;
                                let title = item.title.clone();
                                if db.delete_library_item(id).is_ok() {
                                    if let Some(pos) = state.items.iter().position(|it| it.id == id)
                                    {
                                        state.items.remove(pos);
                                    }
                                    state.apply_filter();
                                    state.set_status(format!("✓ Deleted '{}'", title));
                                } else {
                                    state.set_status("⚠ Failed to delete item from database");
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }
}

/// Formats `text` (preserving CSI SGR escapes) to fit cleanly within `max_cols` display columns,
/// truncating with `…` on overflow, padding trailing spaces, and resetting styles at line end.
fn fit_padded_line(text: &str, max_cols: usize, row_bg: Option<&str>) -> String {
    let mut out = String::with_capacity(text.len() + max_cols + 16);
    if max_cols == 0 {
        return out;
    }
    if let Some(bg) = row_bg {
        out.push_str(bg);
    }

    let bytes = text.as_bytes();
    let mut i = 0;
    let mut col = 0;
    let mut has_sgr = row_bg.is_some();
    let mut trunc_byte_len = out.len();
    let mut trunc_col = 0;
    let limit = max_cols.saturating_sub(1);
    let mut overflowed = false;

    while i < bytes.len() {
        if bytes[i] == b'\x1b' && bytes.get(i + 1) == Some(&b'[') {
            let start = i;
            i += 2;
            while i < bytes.len() && !(b'@'..=b'~').contains(&bytes[i]) {
                i += 1;
            }
            if i < bytes.len() {
                if bytes[i] == b'm' {
                    let seq = &text[start..=i];
                    out.push_str(seq);
                    has_sgr = seq != "\x1b[0m" && seq != "\x1b[m";
                    if !has_sgr && let Some(bg) = row_bg {
                        out.push_str(bg);
                        has_sgr = true;
                    }
                }
                i += 1;
            }
            continue;
        }

        let ch = text[i..].chars().next().unwrap_or('\0');
        let cw = if ch == '\t' {
            2
        } else {
            char_display_width(ch)
        };
        if col + cw <= limit {
            if ch == '\t' {
                out.push_str("  ");
            } else {
                out.push(ch);
            }
            col += cw;
            trunc_byte_len = out.len();
            trunc_col = col;
            i += ch.len_utf8();
        } else if col + cw <= max_cols {
            let next_i = i + ch.len_utf8();
            if text[next_i..].trim_end().is_empty() {
                if ch == '\t' {
                    out.push_str("  ");
                } else {
                    out.push(ch);
                }
                col += cw;
                break;
            }
            overflowed = true;
            break;
        } else {
            overflowed = true;
            break;
        }
    }

    if overflowed {
        out.truncate(trunc_byte_len);
        out.push('…');
        col = trunc_col + 1;
    }

    for _ in col..max_cols {
        out.push(' ');
    }
    if has_sgr {
        out.push_str("\x1b[0m");
    }
    out
}

fn render_box(
    width: usize,
    height: usize,
    title: &str,
    border_ansi: &str,
    rows: &[(&str, Option<&str>)],
) -> Vec<String> {
    let mut lines = Vec::with_capacity(height);
    if width < 2 || height < 2 {
        return lines;
    }
    let inner_w = width - 2;
    let inner_h = height - 2;

    // Top border with inline title
    let mut top = String::with_capacity(width * 2);
    top.push_str(border_ansi);
    top.push('┌');
    let mut title_cols = 0;
    for ch in title.chars() {
        let cw = char_display_width(ch);
        if title_cols + cw > inner_w {
            break;
        }
        top.push(ch);
        title_cols += cw;
    }
    for _ in title_cols..inner_w {
        top.push('─');
    }
    top.push_str("┐\x1b[0m");
    lines.push(top);

    for r in 0..inner_h {
        let (content, bg) = rows.get(r).copied().unwrap_or(("", None));
        let padded = fit_padded_line(content, inner_w, bg);
        lines.push(format!(
            "{border_ansi}│\x1b[0m{padded}{border_ansi}│\x1b[0m"
        ));
    }

    let mut bottom = String::with_capacity(width * 2);
    bottom.push_str(border_ansi);
    bottom.push('└');
    for _ in 0..inner_w {
        bottom.push('─');
    }
    bottom.push_str("┘\x1b[0m");
    lines.push(bottom);
    lines
}

fn render_frame(cols: usize, rows: usize, state: &mut AppState) -> String {
    let mut frame = String::with_capacity(cols * rows * 2);
    if cols < 24 || rows < 12 {
        for r in 0..rows {
            let msg = if r == 0 {
                "Terminal too small (min 24x12)"
            } else {
                ""
            };
            frame.push_str(&format!(
                "\x1b[{};1H{}",
                r + 1,
                fit_padded_line(msg, cols, None)
            ));
        }
        return frame;
    }

    const CYAN: &str = "\x1b[36m";
    const DARK: &str = "\x1b[90m";

    let header_line = format!(
        "\x1b[1;30;46m srz \x1b[0;1;36m Library Explorer \x1b[0;90m({}/{} summaries)\x1b[0m",
        state.filtered_indices.len(),
        state.items.len()
    );
    let header_rows = render_box(cols, 3, "", DARK, &[(&header_line, None)]);

    let main_h = rows - 4;
    let left_w = (cols * 38 / 100).max(16).min(cols.saturating_sub(18));
    let right_w = cols - left_w;

    let search_title = if state.is_filtering {
        " Search (Type to filter, Enter/Esc to finish) "
    } else {
        " Search (Press '/' to filter) "
    };
    let search_border = if state.is_filtering { "\x1b[33m" } else { DARK };
    let search_body = if state.filter_query.is_empty() && !state.is_filtering {
        "\x1b[90mAll items\x1b[0m".to_string()
    } else if state.is_filtering {
        format!("\x1b[97m{}▌\x1b[0m", state.filter_query)
    } else {
        format!("\x1b[97m{}\x1b[0m", state.filter_query)
    };
    let search_rows = render_box(
        left_w,
        3,
        search_title,
        search_border,
        &[(&search_body, None)],
    );

    let list_h = main_h.saturating_sub(3);
    let list_inner_h = list_h.saturating_sub(2);
    let visible_slots = (list_inner_h / 3).max(1);
    if let Some(sel) = state.selected {
        if sel < state.list_offset {
            state.list_offset = sel;
        } else if sel >= state.list_offset + visible_slots {
            state.list_offset = sel + 1 - visible_slots;
        }
    } else {
        state.list_offset = 0;
    }

    let mut list_owned: Vec<(String, Option<&'static str>)> = Vec::with_capacity(list_inner_h);
    let end = (state.list_offset + visible_slots).min(state.filtered_indices.len());
    for pos in state.list_offset..end {
        if let Some(item) = state
            .filtered_indices
            .get(pos)
            .and_then(|&idx| state.items.get(idx))
        {
            let is_sel = state.selected == Some(pos);
            let bg = is_sel.then_some("\x1b[48;2;20;50;70m");
            let prefix = if is_sel { "▶ " } else { "  " };
            let badge = render::target_badge(&item.target_type);
            let date = item.created_at.get(..10).unwrap_or(&item.created_at);
            list_owned.push((
                format!("{prefix}\x1b[33m{badge} \x1b[1;97m{}\x1b[0m", item.title),
                bg,
            ));
            list_owned.push((
                format!(
                    "    \x1b[32m[{}]\x1b[90m {date} • {} words\x1b[0m",
                    item.style, item.word_count
                ),
                bg,
            ));
            list_owned.push((String::new(), None));
        }
    }
    let list_refs: Vec<(&str, Option<&str>)> =
        list_owned.iter().map(|(s, bg)| (s.as_str(), *bg)).collect();
    let list_border = if state.focus == FocusPane::List {
        CYAN
    } else {
        DARK
    };
    let list_rows = render_box(left_w, list_h, " Summaries ", list_border, &list_refs);

    let meta_h = 7usize.min(main_h.saturating_sub(3));
    let preview_h = main_h.saturating_sub(meta_h);
    let preview_inner_w = right_w.saturating_sub(2).max(1);
    let preview_inner_h = preview_h.saturating_sub(2);

    let (meta_rows, preview_rows) = if let Some(item) = state.selected_item() {
        let source = item
            .author
            .as_deref()
            .or(item.url.as_deref())
            .unwrap_or("N/A");
        let tokens = match (item.prompt_tokens, item.completion_tokens) {
            (Some(p), Some(c)) => format!("{p} prompt + {c} comp = {} total", p + c),
            _ => "N/A".to_string(),
        };
        let speed = item
            .tokens_per_second
            .map(|tps| format!("{tps:.1} tok/s"))
            .unwrap_or_else(|| "N/A".to_string());
        let meta_owned = [
            format!("\x1b[36mTitle:     \x1b[1;97m{}\x1b[0m", item.title),
            format!("\x1b[36mSource:    \x1b[90m{source}\x1b[0m"),
            format!(
                "\x1b[36mModel:     \x1b[33m{}  \x1b[36mStyle: \x1b[32m{}\x1b[0m",
                item.model, item.style
            ),
            format!("\x1b[36mTokens:    \x1b[97m{tokens}  \x1b[36mSpeed: \x1b[33m{speed}\x1b[0m"),
            format!("\x1b[36mGenerated: \x1b[90m{}\x1b[0m", item.created_at),
        ];
        let meta_refs: Vec<(&str, Option<&str>)> =
            meta_owned.iter().map(|s| (s.as_str(), None)).collect();
        let meta_box = render_box(right_w, meta_h, " Telemetry & Details ", DARK, &meta_refs);

        let rendered_md = crate::ui::markdown::render_markdown(&item.summary);
        let wrapped = wrap_terminal_text(&rendered_md, preview_inner_w);
        let all_lines: Vec<&str> = wrapped.lines().collect();
        let max_scroll = all_lines.len().saturating_sub(preview_inner_h);
        let scroll = (state.scroll_offset as usize).min(max_scroll);
        state.scroll_offset = scroll as u16;
        let preview_slice: Vec<(&str, Option<&str>)> = all_lines
            .iter()
            .skip(scroll)
            .take(preview_inner_h)
            .map(|&l| (l, None))
            .collect();
        let preview_border = if state.focus == FocusPane::Preview {
            CYAN
        } else {
            DARK
        };
        let preview_box = render_box(
            right_w,
            preview_h,
            " Summary Content (Tab to focus, J/K to scroll) ",
            preview_border,
            &preview_slice,
        );
        (meta_box, preview_box)
    } else {
        let meta_box = render_box(right_w, meta_h, " Telemetry & Details ", DARK, &[]);
        let preview_box = render_box(
            right_w,
            preview_h,
            " Summary Content ",
            DARK,
            &[("No summary selected or library is empty.", None)],
        );
        (meta_box, preview_box)
    };

    let footer_raw = if let Some((msg, _)) = &state.status_message {
        format!("\x1b[1;30;42m {msg} \x1b[0m")
    } else {
        "\x1b[36m [↑/↓/j/k]\x1b[0m Navigate \x1b[36m[/]\x1b[0m Search \x1b[36m[Tab]\x1b[0m Focus \x1b[36m[a]\x1b[0m Ask/Chat \x1b[36m[Enter/b]\x1b[0m Browser \x1b[36m[y]\x1b[0m Copy \x1b[36m[d]\x1b[0m Delete \x1b[36m[q]\x1b[0m Quit ".to_string()
    };
    let footer_line = fit_padded_line(&footer_raw, cols, None);

    for (idx, line) in header_rows.into_iter().enumerate() {
        frame.push_str(&format!("\x1b[{};1H{line}", idx + 1));
    }
    let left_all = search_rows.into_iter().chain(list_rows);
    let right_all = meta_rows.into_iter().chain(preview_rows);
    for (idx, (l, r)) in left_all.zip(right_all).enumerate() {
        frame.push_str(&format!("\x1b[{};1H{l}{r}", idx + 4));
    }
    frame.push_str(&format!("\x1b[{};1H{footer_line}", rows));
    frame
}

fn pipe_to_command(cmd: &str, args: &[&str], text: &str) -> std::io::Result<()> {
    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take()
        && let Err(err) = stdin.write_all(text.as_bytes())
        && err.kind() != std::io::ErrorKind::BrokenPipe
    {
        return Err(err);
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{cmd} exited with error")))
    }
}

fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        pipe_to_command("pbcopy", &[], text)
    }

    #[cfg(target_os = "windows")]
    {
        let clip = crate::fs_util::system_root_bin("clip.exe");
        pipe_to_command(&clip.to_string_lossy(), &[], text)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        for (cmd, args) in [
            ("wl-copy", &[][..]),
            ("xclip", &["-selection", "clipboard"][..]),
            ("xsel", &["--clipboard", "--input"][..]),
        ] {
            if pipe_to_command(cmd, args, text).is_ok() {
                return Ok(());
            }
        }
        Err(std::io::Error::other(
            "No clipboard tool found (tried wl-copy, xclip, xsel)",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_item(
        id: i64,
        target_type: &str,
        title: &str,
        author: Option<&str>,
        url: Option<&str>,
        summary: &str,
    ) -> LibraryItem {
        LibraryItem {
            meta: crate::db::HistoryEntry {
                id,
                target_type: target_type.into(),
                title: title.into(),
                author: author.map(Into::into),
                url: url.map(Into::into),
                style: "concise".into(),
                model: "gemma".into(),
                word_count: 500,
                prompt_tokens: None,
                completion_tokens: None,
                duration_ms: 100,
                tokens_per_second: None,
                created_at: "2026-10-28 12:00".into(),
            },
            summary: summary.into(),
            reasoning: None,
        }
    }

    #[test]
    fn test_app_state_filtering_and_sanitization() {
        let mut dirty = make_item(
            1,
            "web",
            "\x1b[31mRed Title\x1b[0m\nSecond Line",
            Some("Evil\u{202E}Author\x1b]0;Title\x07"),
            Some("https://example.com/\x1b[2Jevil\npath"),
            "Malicious \x1b[31;1mANSI\x1b[0m and \u{202E}BiDi summary",
        );
        dirty.style = "concise\r\n".into();
        dirty.model = "gemma\x07".into();

        let dune = make_item(
            2,
            "book",
            "Dune",
            Some("Frank Herbert"),
            None,
            "Spice and sandworms on Arrakis",
        );

        let mut state = AppState::new(vec![dirty, dune], PathBuf::from("/tmp"));
        let item = &state.items[0];
        assert_eq!(item.title, "Red Title Second Line");
        assert!(!item.author.as_ref().unwrap().contains('\u{202E}'));
        assert!(!item.url.as_ref().unwrap().contains('\n'));
        assert!(!item.summary.contains("\x1b[31;1m"));
        assert!(!item.summary.contains('\u{202E}'));
        assert_eq!(item.style, "concise");
        assert_eq!(item.model, "gemma");

        for (query, expected) in [
            ("arrakis", Some("Dune")),
            ("red title", Some("Red Title Second Line")),
            ("nonexistent", None),
        ] {
            state.filter_query = query.into();
            state.apply_filter();
            assert_eq!(
                state.selected_item().map(|i| i.title.as_str()),
                expected,
                "query={query}"
            );
        }
    }

    #[test]
    fn test_render_frame_and_fit_padded_line() {
        let fitted = fit_padded_line("\x1b[36mHello 世界\x1b[0m", 8, None);
        assert!(fitted.ends_with("\x1b[0m"));
        assert!(fitted.contains('…'));

        let item = make_item(
            1,
            "book",
            "Dune",
            Some("Frank Herbert"),
            None,
            "**Arrakis**",
        );
        let mut state = AppState::new(vec![item], PathBuf::from("/tmp"));
        let frame = render_frame(100, 30, &mut state);
        assert!(frame.contains("Library Explorer"));
        assert!(frame.contains("Dune"));
        assert!(frame.contains("Arrakis"));
        assert!(render_frame(20, 10, &mut state).contains("Terminal too small"));
    }
}
