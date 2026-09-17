//! Cookie-paste login screen.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Wrap};

use super::{centered, theme};
use crate::app::App;

const WIDTH: u16 = 76;
const HEIGHT: u16 = 16;

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let panel = centered(area, WIDTH, HEIGHT);
    let block = theme::panel("Log in to Bandcamp");
    let inner = block.inner(panel);
    frame.render_widget(block, panel);

    let [instructions, input_area, message_area] = Layout::vertical([
        Constraint::Length(8),
        Constraint::Length(2),
        Constraint::Min(0),
    ])
    .areas(inner.inner(ratatui::layout::Margin::new(1, 0)));

    let text = vec![
        Line::from("Bandcamp has no login API, so this app reuses your browser session."),
        Line::raw(""),
        Line::from(vec![
            Span::raw("1. Log in at "),
            Span::styled("bandcamp.com", theme::ACCENT),
            Span::raw(" in your browser."),
        ]),
        Line::from("2. Open DevTools → Application/Storage → Cookies → https://bandcamp.com."),
        Line::from(vec![
            Span::raw("3. Copy the value of the cookie named "),
            Span::styled("identity", theme::ACCENT),
            Span::raw(" and paste it below."),
        ]),
        Line::from(vec![
            Span::raw("It is stored in the "),
            Span::styled(app.store_name.as_str(), theme::ACCENT),
            Span::raw("."),
        ])
        .dim(),
        Line::raw(""),
        Line::from("Esc goes back. Discover, search, Daily and playback need no login.").dim(),
    ];
    frame.render_widget(
        Paragraph::new(text).wrap(Wrap { trim: false }),
        instructions,
    );

    // Masked input: the cookie is a secret and longer than the box.
    let label = "identity: ";
    let field_width = input_area
        .width
        .saturating_sub(label.chars().count() as u16)
        .max(1) as usize;
    let input = &app.login.input;
    let scroll = input.visual_scroll(field_width);
    let shown = input
        .value()
        .chars()
        .count()
        .saturating_sub(scroll)
        .min(field_width);
    let masked: String = "•".repeat(shown);
    let line = Line::from(vec![Span::styled(label, theme::ACCENT), Span::raw(masked)]);
    frame.render_widget(Paragraph::new(line).underlined(), input_area);
    if !app.login.busy {
        let cursor_x = input_area.x
            + label.chars().count() as u16
            + (input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position((
            cursor_x.min(input_area.right().saturating_sub(1)),
            input_area.y,
        ));
    }

    let mut lines = Vec::new();
    if app.login.busy {
        lines.push(Line::from(vec![
            Span::styled(theme::spinner(app.tick), theme::ACCENT),
            Span::raw(" Checking with bandcamp.com…"),
        ]));
    }
    if let Some(error) = &app.login.error {
        lines.push(Line::styled(error.as_str(), theme::ERROR));
    }
    if let Some(hint) = &app.login.hint {
        lines.push(Line::styled(hint.as_str(), theme::WARNING));
    }
    frame.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: true }),
        message_area,
    );
}
