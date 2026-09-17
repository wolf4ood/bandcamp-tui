//! Queue overlay.

use ratatui::Frame;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, Row, Table, TableState};

use super::{centered, fmt_duration, theme};
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    let width = (area.width * 8 / 10).max(40);
    let height = (app.queue.len() as u16 + 3).clamp(5, area.height * 7 / 10);
    let popup = centered(area, width, height);
    frame.render_widget(Clear, popup);

    let rows: Vec<Row> = app
        .queue
        .tracks
        .iter()
        .enumerate()
        .map(|(i, track)| {
            let marker = if app.queue.current == Some(i) {
                "♫"
            } else {
                ""
            };
            let mut row = Row::new(vec![
                Cell::from(marker),
                Cell::from(track.title.clone()),
                Cell::from(track.artist.clone()),
                Cell::from(fmt_duration(track.duration)),
            ]);
            if track.url.is_none() {
                row = row.dim();
            }
            row
        })
        .collect();
    let title = format!("Queue · {} tracks", app.queue.len());
    let table = Table::new(
        rows,
        [
            ratatui::layout::Constraint::Length(1),
            ratatui::layout::Constraint::Percentage(50),
            ratatui::layout::Constraint::Min(10),
            ratatui::layout::Constraint::Length(8),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(&title));
    let mut state =
        TableState::default().with_selected((!app.queue.is_empty()).then_some(app.queue_selected));
    frame.render_stateful_widget(table, popup, &mut state);
    if app.queue.is_empty() {
        let inner = popup.inner(ratatui::layout::Margin::new(2, 1));
        frame.render_widget(
            ratatui::widgets::Paragraph::new(Line::from(
                Span::raw("Empty. Press a on an album to queue it.").dim(),
            )),
            inner,
        );
    }
}
