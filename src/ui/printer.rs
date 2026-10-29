use std::env;
use std::io::{self, IsTerminal, Write};
use std::process::{Command, Stdio};
use std::sync::LazyLock;
use termimad::crossterm::style::{Attribute, Color};
use termimad::MadSkin;

use crate::config::cli::PagingMode;
use crate::ui::Ui;

static SKIN: LazyLock<MadSkin> = LazyLock::new(create_skin);

/// Creates a tailored MadSkin for srz terminal markdown output.
pub fn create_skin() -> MadSkin {
    let mut skin = MadSkin::default();

    // Headers
    skin.headers[0].set_fg(Color::Cyan);
    skin.headers[0].compound_style.add_attr(Attribute::Underlined);
    skin.headers[1].set_fg(Color::Cyan);
    skin.headers[2].set_fg(Color::Yellow);

    // Inline formatting
    skin.bold.set_fg(Color::White);
    skin.italic.set_fg(Color::Magenta);
    skin.inline_code.set_fg(Color::Green);

    // Code blocks
    skin.code_block.set_fg(Color::Green);

    // Bullet points
    skin.bullet = termimad::StyledChar::from_fg_char(Color::Cyan, '•');

    skin
}

/// Renders markdown string into ANSI formatted terminal text.
pub fn render_markdown(markdown: &str, width: Option<usize>) -> String {
    let term_width = width.unwrap_or_else(|| {
        crossterm::terminal::size()
            .map(|(w, _)| w as usize)
            .unwrap_or(80)
    });
    SKIN.text(markdown, Some(term_width)).to_string()
}

/// Prints formatted summary to terminal, launching a pager if the text exceeds terminal height.
pub fn display_summary(
    title: &str,
    content: &str,
    paging_mode: PagingMode,
    plain: bool,
) -> io::Result<()> {
    let stdout_is_tty = io::stdout().is_terminal();
    let sanitized_title = crate::ui::sanitize_title(title);
    let sanitized_content = crate::ui::sanitize_terminal_text(content);

    // If output is redirected/piped, or plain mode is requested, or colors are disabled:
    if !stdout_is_tty || plain || !Ui::use_color() {
        if stdout_is_tty && !plain {
            Ui::header(&format!("Summary: {}", sanitized_title));
        }
        println!("{}\n", sanitized_content);
        return Ok(());
    }

    // Terminal mode: format markdown
    let header = format!("\n\x1b[1;36m=== Summary: {} ===\x1b[0m\n\n", sanitized_title);
    let formatted_body = render_markdown(&sanitized_content, None);
    let full_output = format!("{}{}\n", header, formatted_body);

    let (_, term_height) = crossterm::terminal::size().unwrap_or((80, 24));
    let line_count = full_output.lines().count();

    let should_page = match paging_mode {
        PagingMode::Always => true,
        PagingMode::Never => false,
        PagingMode::Auto => line_count > term_height.saturating_sub(4) as usize,
    };

    if should_page {
        if let Err(err) = run_pager(&full_output) {
            Ui::verbose(&format!("Pager error, falling back to direct print: {err}"));
            print!("{}", full_output);
            io::stdout().flush()?;
        }
    } else {
        print!("{}", full_output);
        io::stdout().flush()?;
    }

    Ok(())
}

/// Pipes content through the system pager ($PAGER or less -RFX).
fn run_pager(content: &str) -> io::Result<()> {
    let pager_cmd = env::var("PAGER").unwrap_or_else(|_| "less".to_string());
    let mut parts = pager_cmd.split_whitespace();
    let bin = parts.next().unwrap_or("less");
    let mut args: Vec<&str> = parts.collect();

    // If using default `less`, add flags to display colors and quit if fits on one screen
    if bin == "less" && args.is_empty() {
        args = vec!["-R", "-F", "-X"];
    }

    let mut child = Command::new(bin)
        .args(&args)
        .stdin(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        // Ignore BrokenPipe error when user exits pager early (e.g. presses 'q')
        if let Err(err) = stdin.write_all(content.as_bytes())
            && err.kind() != io::ErrorKind::BrokenPipe
        {
            return Err(err);
        }
    }

    let _ = child.wait();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_markdown_basic() {
        let md = "# Title\n\nThis is **bold** and *italic* text with `code`.\n\n* Bullet 1\n* Bullet 2";
        let rendered = render_markdown(md, Some(80));
        assert!(rendered.contains("Title"));
        assert!(rendered.contains("bold"));
        assert!(rendered.contains("Bullet"));
    }

    #[test]
    fn test_create_skin() {
        let skin = create_skin();
        assert_eq!(skin.headers[0].compound_style.get_fg(), Some(Color::Cyan));
    }
}
