use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use std::io::{Stdout, stdout};
use std::time::{Duration, Instant};

use crate::cache::Cache;
use crate::render;
use crate::storage::db::{Database, LibraryItem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusPane {
    List,
    Preview,
}

struct AppState {
    items: Vec<LibraryItem>,
    filtered_indices: Vec<usize>,
    list_state: ListState,
    scroll_offset: u16,
    filter_query: String,
    is_filtering: bool,
    focus: FocusPane,
    status_message: Option<(String, Instant)>,
    views_dir: std::path::PathBuf,
}

impl AppState {
    fn new(mut items: Vec<LibraryItem>, views_dir: std::path::PathBuf) -> Self {
        for item in &mut items {
            item.title = crate::ui::sanitize_title(&item.title);
            item.model = crate::ui::sanitize_title(&item.model);
            item.style = crate::ui::sanitize_title(&item.style);
            item.created_at = crate::ui::sanitize_title(&item.created_at);
            if let Some(ref mut author) = item.author {
                *author = crate::ui::sanitize_title(author);
            }
            if let Some(ref mut url) = item.url {
                *url = crate::ui::sanitize_terminal_text(url);
            }
            item.summary = crate::ui::sanitize_terminal_text(&item.summary);
        }

        let indices: Vec<usize> = (0..items.len()).collect();
        let mut list_state = ListState::default();
        if !indices.is_empty() {
            list_state.select(Some(0));
        }
        Self {
            items,
            filtered_indices: indices,
            list_state,
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

        if self.filtered_indices.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(0));
        }
        self.scroll_offset = 0;
    }

    fn selected_item(&self) -> Option<&LibraryItem> {
        self.list_state
            .selected()
            .and_then(|sel| self.filtered_indices.get(sel))
            .and_then(|&idx| self.items.get(idx))
    }

    fn select_next(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let next = if current + 1 < self.filtered_indices.len() {
            current + 1
        } else {
            0
        };
        self.list_state.select(Some(next));
        self.scroll_offset = 0;
    }

    fn select_prev(&mut self) {
        if self.filtered_indices.is_empty() {
            return;
        }
        let current = self.list_state.selected().unwrap_or(0);
        let prev = if current > 0 {
            current - 1
        } else {
            self.filtered_indices.len().saturating_sub(1)
        };
        self.list_state.select(Some(prev));
        self.scroll_offset = 0;
    }

    fn scroll_up(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_sub(amount);
    }

    fn scroll_down(&mut self, amount: u16) {
        self.scroll_offset = self.scroll_offset.saturating_add(amount);
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

/// RAII guard that enables raw mode and enters alternate screen on creation,
/// and restores canonical terminal state (leaves alternate screen, disables raw mode, shows cursor)
/// unconditionally when dropped, even on early exit, error, or panic.
pub struct TerminalSessionGuard;

impl TerminalSessionGuard {
    pub fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = stdout();
        execute!(stdout, EnterAlternateScreen)?;
        Ok(Self)
    }
}

impl Drop for TerminalSessionGuard {
    fn drop(&mut self) {
        let mut stdout = stdout();
        let _ = disable_raw_mode();
        let _ = execute!(stdout, LeaveAlternateScreen);
        let _ = execute!(stdout, crossterm::cursor::Show);
    }
}

/// Runs the interactive terminal UI library explorer.
pub fn run_tui(db: &Database, cache: &Cache) -> Result<TuiAction> {
    let _guard = TerminalSessionGuard::new()?;
    let stdout = stdout();
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let items = db.get_all_library_items().unwrap_or_default();
    let mut state = AppState::new(items, cache.views_dir());

    run_loop(&mut terminal, &mut state, db)
}

fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    state: &mut AppState,
    db: &Database,
) -> Result<TuiAction> {
    let mut needs_redraw = true;
    loop {
        // Clear status after 3 seconds
        if let Some((_, created)) = &state.status_message
            && created.elapsed() > Duration::from_secs(3)
        {
            state.status_message = None;
            needs_redraw = true;
        }

        if needs_redraw {
            terminal
                .draw(|f| ui_draw(f, state))
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            needs_redraw = false;
        }

        if event::poll(Duration::from_millis(100))? {
            match event::read()? {
                Event::Resize(_, _) => {
                    needs_redraw = true;
                }
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    needs_redraw = true;
                    if state.is_filtering {
                        match key.code {
                            KeyCode::Esc => {
                                state.is_filtering = false;
                            }
                            KeyCode::Enter => {
                                state.is_filtering = false;
                            }
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

                    // Normal navigation mode
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
                        KeyCode::Char('/') => {
                            state.is_filtering = true;
                        }
                        KeyCode::Tab => {
                            state.focus = match state.focus {
                                FocusPane::List => FocusPane::Preview,
                                FocusPane::Preview => FocusPane::List,
                            };
                        }
                        KeyCode::Up | KeyCode::Char('k') => match state.focus {
                            FocusPane::List => state.select_prev(),
                            FocusPane::Preview => state.scroll_up(2),
                        },
                        KeyCode::Down | KeyCode::Char('j') => match state.focus {
                            FocusPane::List => state.select_next(),
                            FocusPane::Preview => state.scroll_down(2),
                        },
                        KeyCode::PageUp | KeyCode::Char('K') => {
                            state.scroll_up(10);
                        }
                        KeyCode::PageDown | KeyCode::Char('J') => {
                            state.scroll_down(10);
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

fn ui_draw(f: &mut Frame, state: &mut AppState) {
    let size = f.area();

    let root_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header banner
            Constraint::Min(5),    // Main workspace (list + preview)
            Constraint::Length(1), // Footer status bar
        ])
        .split(size);

    // 1. Header Banner
    let total_count = state.items.len();
    let shown_count = state.filtered_indices.len();
    let header_text = vec![Line::from(vec![
        Span::styled(
            " srz ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            " Library Explorer ",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({shown_count}/{total_count} summaries)"),
            Style::default().fg(Color::DarkGray),
        ),
    ])];
    let header_block = Paragraph::new(header_text).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    f.render_widget(header_block, root_layout[0]);

    // 2. Main Workspace Split (List Left, Preview Right)
    let main_layout = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(38), Constraint::Percentage(62)])
        .split(root_layout[1]);

    // Left Pane: Search Input + Item List
    let left_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(4)])
        .split(main_layout[0]);

    // Search Box
    let search_style = if state.is_filtering {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    let search_title = if state.is_filtering {
        " Search (Type to filter, Enter/Esc to finish) "
    } else {
        " Search (Press '/' to filter) "
    };
    let search_prompt = if state.filter_query.is_empty() && !state.is_filtering {
        Span::styled("All items", Style::default().fg(Color::DarkGray))
    } else {
        Span::styled(&state.filter_query, Style::default().fg(Color::White))
    };
    let search_widget = Paragraph::new(search_prompt).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(search_style)
            .title(search_title),
    );
    f.render_widget(search_widget, left_layout[0]);

    // List of Summaries
    let list_border_style = if state.focus == FocusPane::List {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list_items: Vec<ListItem> = state
        .filtered_indices
        .iter()
        .filter_map(|&idx| state.items.get(idx))
        .map(|item| {
            let badge = render::target_badge(&item.target_type);
            let date = if item.created_at.len() >= 10 {
                &item.created_at[0..10]
            } else {
                &item.created_at
            };

            let line1 = Line::from(vec![
                Span::styled(format!("{badge} "), Style::default().fg(Color::Yellow)),
                Span::styled(&item.title, Style::default().add_modifier(Modifier::BOLD)),
            ]);

            let line2 = Line::from(vec![
                Span::styled(
                    format!("  [{}]", item.style),
                    Style::default().fg(Color::Green),
                ),
                Span::styled(format!(" {date}"), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!(" • {} words", item.word_count),
                    Style::default().fg(Color::DarkGray),
                ),
            ]);

            ListItem::new(vec![line1, line2, Line::from("")])
        })
        .collect();

    let list_widget = List::new(list_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(list_border_style)
                .title(" Summaries "),
        )
        .highlight_style(
            Style::default()
                .bg(Color::Rgb(20, 50, 70))
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(list_widget, left_layout[1], &mut state.list_state);

    // Right Pane: Metadata Details + Markdown Summary Viewer
    let right_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(5)])
        .split(main_layout[1]);

    if let Some(item) = state.selected_item() {
        // Metadata Card
        let author_or_url = item
            .author
            .as_deref()
            .or(item.url.as_deref())
            .unwrap_or("N/A");
        let tokens_info = match (item.prompt_tokens, item.completion_tokens) {
            (Some(p), Some(c)) => format!("{p} prompt + {c} comp = {} total", p + c),
            _ => "N/A".to_string(),
        };
        let speed_info = item
            .tokens_per_second
            .map(|tps| format!("{tps:.1} tok/s"))
            .unwrap_or_else(|| "N/A".to_string());

        let meta_lines = vec![
            Line::from(vec![
                Span::styled("Title:     ", Style::default().fg(Color::Cyan)),
                Span::styled(
                    &item.title,
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
            ]),
            Line::from(vec![
                Span::styled("Source:    ", Style::default().fg(Color::Cyan)),
                Span::styled(author_or_url, Style::default().fg(Color::DarkGray)),
            ]),
            Line::from(vec![
                Span::styled("Model:     ", Style::default().fg(Color::Cyan)),
                Span::styled(&item.model, Style::default().fg(Color::Yellow)),
                Span::styled("  Style: ", Style::default().fg(Color::Cyan)),
                Span::styled(&item.style, Style::default().fg(Color::Green)),
            ]),
            Line::from(vec![
                Span::styled("Tokens:    ", Style::default().fg(Color::Cyan)),
                Span::styled(tokens_info, Style::default().fg(Color::White)),
                Span::styled("  Speed: ", Style::default().fg(Color::Cyan)),
                Span::styled(speed_info, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(vec![
                Span::styled("Generated: ", Style::default().fg(Color::Cyan)),
                Span::styled(&item.created_at, Style::default().fg(Color::DarkGray)),
            ]),
        ];

        let meta_widget = Paragraph::new(meta_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray))
                .title(" Telemetry & Details "),
        );
        f.render_widget(meta_widget, right_layout[0]);

        // Summary Content Viewer
        let preview_border_style = if state.focus == FocusPane::Preview {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let summary_widget = Paragraph::new(item.summary.as_str())
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(preview_border_style)
                    .title(" Summary Content (Tab to focus, J/K to scroll) "),
            )
            .wrap(Wrap { trim: false })
            .scroll((state.scroll_offset, 0));

        f.render_widget(summary_widget, right_layout[1]);
    } else {
        let empty_widget = Paragraph::new("No summary selected or library is empty.").block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Summary Content "),
        );
        f.render_widget(empty_widget, right_layout[1]);
    }

    // 3. Footer Status Bar
    let status_text = if let Some((msg, _)) = &state.status_message {
        Line::from(Span::styled(
            format!(" {msg} "),
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD),
        ))
    } else {
        Line::from(vec![
            Span::styled(" [↑/↓/j/k]", Style::default().fg(Color::Cyan)),
            Span::raw(" Navigate "),
            Span::styled("[/]", Style::default().fg(Color::Cyan)),
            Span::raw(" Search "),
            Span::styled("[Tab]", Style::default().fg(Color::Cyan)),
            Span::raw(" Focus "),
            Span::styled("[a]", Style::default().fg(Color::Cyan)),
            Span::raw(" Ask/Chat "),
            Span::styled("[Enter/b]", Style::default().fg(Color::Cyan)),
            Span::raw(" Browser "),
            Span::styled("[y]", Style::default().fg(Color::Cyan)),
            Span::raw(" Copy "),
            Span::styled("[d]", Style::default().fg(Color::Cyan)),
            Span::raw(" Delete "),
            Span::styled("[q]", Style::default().fg(Color::Cyan)),
            Span::raw(" Quit "),
        ])
    };
    let footer_widget = Paragraph::new(status_text);
    f.render_widget(footer_widget, root_layout[2]);
}

fn pipe_to_command(cmd: &str, args: &[&str], text: &str) -> std::io::Result<()> {
    let mut child = std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        std::io::Write::write_all(&mut stdin, text.as_bytes())?;
    }
    let status = child.wait()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("{cmd} exited with error")))
    }
}

/// Copies text to the system clipboard using the platform CLI.
fn copy_to_clipboard(text: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    return pipe_to_command("pbcopy", &[], text);

    #[cfg(target_os = "windows")]
    return pipe_to_command("clip", &[], text);

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

    #[test]
    fn test_badge_for_type() {
        assert_eq!(render::target_badge("book"), "📚 Book");
        assert_eq!(render::target_badge("web"), "🌐 Web");
        assert_eq!(render::target_badge("news"), "📰 News");
        assert_eq!(render::target_badge("youtube"), "📺 Video");
        assert_eq!(render::target_badge("other"), "📝 Doc");
        assert_eq!(render::target_icon("youtube"), "📺");
    }

    #[test]
    fn test_app_state_filtering() {
        let items = vec![
            LibraryItem {
                id: 1,
                target_type: "book".into(),
                title: "Dune".into(),
                author: Some("Frank Herbert".into()),
                url: None,
                summary: "Spice and sandworms on Arrakis".into(),
                style: "concise".into(),
                model: "gemma".into(),
                word_count: 500,
                prompt_tokens: Some(100),
                completion_tokens: Some(50),
                duration_ms: 1200,
                tokens_per_second: Some(41.6),
                created_at: "2026-10-28 12:00".into(),
                reasoning: None,
            },
            LibraryItem {
                id: 2,
                target_type: "web".into(),
                title: "Rust 2026 Edition".into(),
                author: None,
                url: Some("https://rust-lang.org".into()),
                summary: "New features in modern Rust".into(),
                style: "detailed".into(),
                model: "gemma".into(),
                word_count: 800,
                prompt_tokens: Some(150),
                completion_tokens: Some(70),
                duration_ms: 1500,
                tokens_per_second: Some(46.6),
                created_at: "2026-10-28 13:00".into(),
                reasoning: None,
            },
        ];

        let mut state = AppState::new(items, std::path::PathBuf::from("/tmp"));
        assert_eq!(state.filtered_indices.len(), 2);

        state.filter_query = "arrakis".into();
        state.apply_filter();
        assert_eq!(state.filtered_indices.len(), 1);
        assert_eq!(state.selected_item().unwrap().title, "Dune");

        state.filter_query = "rust".into();
        state.apply_filter();
        assert_eq!(state.filtered_indices.len(), 1);
        assert_eq!(state.selected_item().unwrap().title, "Rust 2026 Edition");

        state.filter_query = "nonexistent".into();
        state.apply_filter();
        assert_eq!(state.filtered_indices.len(), 0);
        assert!(state.selected_item().is_none());
    }

    #[test]
    fn test_app_state_sanitizes_untrusted_item_fields() {
        let malicious_items = vec![LibraryItem {
            id: 1,
            target_type: "web".into(),
            title: "\x1b[31mRed Title\x1b[0m\nSecond Line".into(),
            author: Some("Evil\u{202E}Author\x1b]0;Title\x07".into()),
            url: Some("https://example.com/\x1b[2Jevil".into()),
            summary: "Malicious \x1b[31;1mANSI\x1b[0m and \u{202E}BiDi summary".into(),
            style: "concise\r\n".into(),
            model: "gemma\x07".into(),
            word_count: 100,
            prompt_tokens: None,
            completion_tokens: None,
            duration_ms: 100,
            tokens_per_second: None,
            created_at: "2026-10-28\n12:00".into(),
            reasoning: None,
        }];

        let state = AppState::new(malicious_items, std::path::PathBuf::from("/tmp"));
        let item = &state.items[0];
        assert_eq!(item.title, "Red Title Second Line");
        assert!(!item.author.as_ref().unwrap().contains('\u{202E}'));
        assert!(!item.author.as_ref().unwrap().contains('\x1b'));
        assert!(!item.url.as_ref().unwrap().contains("\x1b[2J"));
        assert!(!item.summary.contains("\x1b[31;1m"));
        assert!(!item.summary.contains('\u{202E}'));
        assert_eq!(item.style, "concise");
        assert_eq!(item.model, "gemma");
    }
}
