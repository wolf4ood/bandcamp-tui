//! Top bar: section tabs on the left, who is logged in on the right.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Tabs};

use super::theme;
use crate::app::{App, Screen, Section};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let who = match (&app.session, app.screen) {
        (Some(session), _) => format!("{} ", session.username),
        (None, Screen::Booting) => app
            .config
            .username
            .as_deref()
            .map(|u| format!("{u}? "))
            .unwrap_or_default(),
        (None, _) => "not logged in (L) ".to_owned(),
    };
    let who_width = who.chars().count() as u16;
    let [tabs_area, who_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(who_width)]).areas(area);

    let titles: Vec<Line> = Section::ALL
        .iter()
        .map(|section| {
            Line::from(vec![
                Span::styled(format!("{}", section.index() + 1), theme::ACCENT.dim()),
                Span::raw(" "),
                Span::raw(section.title()),
            ])
        })
        .collect();
    let tabs = Tabs::new(titles)
        .select(app.section.index())
        .highlight_style(theme::SELECTED)
        .divider(" ")
        .padding(" ", " ");
    let tabs = if app.screen == Screen::Main {
        tabs
    } else {
        tabs.dim()
    };
    frame.render_widget(tabs, tabs_area);
    frame.render_widget(
        Paragraph::new(who).right_aligned().style(theme::ACCENT),
        who_area,
    );
}
