//! Rendering. Screens read `App`; the few that scroll keep their table state in it.

mod album;
mod artist;
mod daily;
mod discover;
mod fan;
mod feed;
mod help;
mod library;
mod login;
mod nav;
mod picker;
mod player_bar;
mod queue;
mod search;
pub mod theme;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{App, Screen, Section};

pub fn draw(frame: &mut Frame, app: &mut App) {
    let [nav_area, content, player_area, status_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    nav::draw(frame, app, nav_area);
    match app.screen {
        Screen::Booting => booting(frame, app, content),
        Screen::Login => login::draw(frame, app, content),
        Screen::Main if app.album_open_here() => album::draw(frame, app, content),
        Screen::Main if app.artist_open_here() => artist::draw(frame, app, content),
        Screen::Main if app.fan_open_here() => fan::draw(frame, app, content),
        Screen::Main => match app.section {
            Section::Collection => library::draw(frame, app, content),
            Section::Discover => discover::draw(frame, app, content),
            Section::Feed => feed::draw(frame, app, content),
            Section::Search => search::draw(frame, app, content),
            Section::Daily => daily::draw(frame, app, content),
        },
    }
    player_bar::draw(frame, app, player_area);
    status_line(frame, app, status_area);
    if app.screen == Screen::Main
        && app.section == Section::Discover
        && app.discover.picker.is_some()
    {
        picker::draw(frame, app);
    }
    if app.show_queue {
        queue::draw(frame, app);
    }
    if app.show_help {
        help::draw(frame, app);
    }
}

fn booting(frame: &mut Frame, app: &App, area: Rect) {
    let text = Line::from(vec![
        Span::raw(theme::spinner(app.tick)),
        Span::raw(" "),
        Span::raw(app.status.as_str()),
    ]);
    frame.render_widget(
        Paragraph::new(text).centered().block(theme::panel("")),
        area,
    );
}

fn status_line(frame: &mut Frame, app: &App, area: Rect) {
    let hints = match app.screen {
        Screen::Booting => "Ctrl+C quit",
        Screen::Login => "Enter log in · Esc back · Ctrl+U clear · F1 help · Ctrl+Q quit",
        Screen::Main if app.show_queue => "Enter play · d remove · c clear · Esc close",
        Screen::Main if app.section == Section::Collection && app.library.search_open => {
            "Enter search · Esc cancel"
        }
        Screen::Main if app.section == Section::Discover && app.discover.picker.is_some() => {
            "↑/↓ choose · Enter apply · Esc back"
        }
        Screen::Main if app.section == Section::Discover && app.discover.tag_entry.is_some() => {
            "type a tag · ↑/↓ suggestions · Enter add · Esc cancel"
        }
        Screen::Main if app.album_open_here() => {
            "Enter play · a queue · A artist · F follow · W wishlist · o browser · Esc back"
        }
        Screen::Main if app.artist_open_here() => {
            "Enter open · p play · a queue · w roster · F follow · o browser · Esc back"
        }
        Screen::Main if app.fan_open_here() => {
            "Enter open · p play · a queue · F follow fan · o browser · Esc back"
        }
        Screen::Main
            if app.session.is_none()
                && matches!(app.section, Section::Collection | Section::Feed) =>
        {
            "L log in · 1-5 sections · Q queue · ? help"
        }
        Screen::Main if app.section == Section::Search && app.search.input_open => {
            "type · Enter search · Tab filter · Esc results"
        }
        Screen::Main if app.section == Section::Search => {
            "Enter open · p play · a queue · / edit · f filter · t tag → discover · ? help"
        }
        Screen::Main if app.section == Section::Discover => {
            "Enter open · p play · a queue · f filters · t tag · F follow · W wishlist · ? help"
        }
        Screen::Main if app.section == Section::Feed => {
            "Enter open · p play · a queue · w lists · v filter · F follow · W wishlist · ? help"
        }
        Screen::Main if app.section == Section::Daily && app.daily.view.is_some() => {
            "w tracks/text · Enter play · a queue · l album · o browser · Esc back"
        }
        Screen::Main if app.section == Section::Daily => {
            "Enter read · v section · R reload · o browser · ? help"
        }
        Screen::Main => "Enter open · p play · a queue · / search · w wishlist · Q queue · ? help",
    };
    let mut left = Vec::new();
    if app.busy() {
        left.push(Span::styled(theme::spinner(app.tick), theme::ACCENT));
        left.push(Span::raw(" "));
    }
    left.push(Span::raw(app.status.as_str()));

    let hints_width = hints.chars().count() as u16 + 1;
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(hints_width)]).areas(area);
    frame.render_widget(
        Paragraph::new(Line::from(left)).style(theme::STATUS_BAR),
        left_area,
    );
    frame.render_widget(
        Paragraph::new(hints)
            .right_aligned()
            .style(theme::STATUS_BAR)
            .dim(),
        right_area,
    );
}

/// A rectangle of the given size centred in `area`, clamped to it.
pub fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    let x = area.x + (area.width - width) / 2;
    let y = area.y + (area.height - height) / 2;
    Rect::new(x, y, width, height)
}

/// `1,942,109`
pub fn fmt_count(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// `m:ss`, or `h:mm:ss` past an hour.
pub fn fmt_duration(duration: Duration) -> String {
    let secs = duration.as_secs();
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs % 3600) / 60, secs % 60)
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_counts() {
        assert_eq!(fmt_count(0), "0");
        assert_eq!(fmt_count(999), "999");
        assert_eq!(fmt_count(1000), "1,000");
        assert_eq!(fmt_count(1942109), "1,942,109");
    }

    #[test]
    fn formats_durations() {
        assert_eq!(fmt_duration(Duration::from_secs(0)), "0:00");
        assert_eq!(fmt_duration(Duration::from_secs(65)), "1:05");
        assert_eq!(fmt_duration(Duration::from_secs(3725)), "1:02:05");
    }
}
