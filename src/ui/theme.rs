//! Colours and small shared widgets. Bandcamp's teal is the only brand colour we borrow.

use ratatui::style::{Color, Modifier, Style};
use ratatui::widgets::{Block, BorderType, Borders};

pub const TEAL: Color = Color::Rgb(29, 160, 195);
pub const ACCENT: Style = Style::new().fg(TEAL);
pub const TITLE: Style = Style::new().fg(TEAL).add_modifier(Modifier::BOLD);
pub const SELECTED: Style = Style::new()
    .fg(Color::Black)
    .bg(TEAL)
    .add_modifier(Modifier::BOLD);
pub const ERROR: Style = Style::new().fg(Color::Red);
pub const WARNING: Style = Style::new().fg(Color::Yellow);
pub const STATUS_BAR: Style = Style::new().bg(Color::DarkGray).fg(Color::White);

const SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

pub fn spinner(tick: u64) -> &'static str {
    SPINNER[(tick % SPINNER.len() as u64) as usize]
}

pub fn panel(title: &str) -> Block<'_> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(Color::DarkGray));
    if title.is_empty() {
        block
    } else {
        block.title(format!(" {title} ")).title_style(TITLE)
    }
}
