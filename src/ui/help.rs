//! Keybinding overlay.

use ratatui::Frame;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, Paragraph};

use super::{centered, theme};
use crate::app::{App, Screen};

pub fn draw(frame: &mut Frame, app: &App) {
    let login: &[(&str, &str)] = &[
        ("Enter", "check the cookie and log in"),
        ("Esc", "back to the app"),
        ("Ctrl+U", "clear the field"),
        ("Ctrl+V / paste", "insert the cookie"),
        ("F1", "toggle this help"),
        ("Ctrl+Q  Ctrl+C", "quit"),
    ];
    let main: &[(&str, &str)] = &[
        ("1-5  Tab", "switch section"),
        ("j/k  ↑/↓  g/G", "move · top / bottom"),
        ("Enter", "open album  ·  in album: play from track"),
        ("p  a", "play now · add to queue"),
        (
            "w  /  S  R",
            "collection: wishlist · search · sort · reload",
        ),
        (
            "f  t  x  X",
            "discover: filters · add tag · drop last tag · reset",
        ),
        ("w  v", "feed: switch list · story filter"),
        (
            "v  w  l",
            "daily: switch section · in an article: tracks / text · album",
        ),
        (
            "F  W  A",
            "follow / unfollow · toggle wishlist · open artist page",
        ),
        (
            "/  f  t",
            "search: edit query · cycle type · matching tag → discover",
        ),
        ("o", "open in the browser"),
        ("Esc  Backspace", "back / clear search"),
        ("Space  n  b  s", "pause · next · previous · stop"),
        ("←/→  +/-", "seek 10s · volume"),
        ("Q", "queue (Enter play · d remove · c clear)"),
        ("r  L", "refresh session · log in / log out"),
        ("?  F1  q", "help · quit"),
    ];
    let keys = match app.screen {
        Screen::Main => main,
        _ => login,
    };
    let lines: Vec<Line> = keys
        .iter()
        .map(|(key, what)| {
            Line::from(vec![
                Span::styled(format!("{key:>16}  "), theme::ACCENT),
                Span::raw(*what),
            ])
        })
        .collect();
    let area = centered(frame.area(), 64, lines.len() as u16 + 2);
    frame.render_widget(Clear, area);
    frame.render_widget(Paragraph::new(lines).block(theme::panel("Keys")), area);
}
