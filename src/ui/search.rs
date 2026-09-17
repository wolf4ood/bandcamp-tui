//! Site-wide search: a query line, a type filter and the results.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use super::theme;
use crate::api::search::SearchFilter;
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [input_area, filter_area, table_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    // Query line.
    let label = "Search: ";
    let s = &app.search;
    let width = input_area.width.saturating_sub(label.len() as u16).max(1) as usize;
    let scroll = s.input.visual_scroll(width);
    let shown: String = s.input.value().chars().skip(scroll).take(width).collect();
    let line = if s.input_open {
        Line::from(vec![Span::styled(label, theme::ACCENT), Span::raw(shown)])
    } else if s.query.is_empty() {
        Line::from(vec![
            Span::styled(label, theme::ACCENT),
            Span::raw("press / to type").dim(),
        ])
    } else {
        Line::from(vec![
            Span::styled(label, theme::ACCENT),
            Span::styled(s.query.clone(), theme::TITLE),
        ])
    };
    frame.render_widget(Paragraph::new(line), input_area);
    if s.input_open {
        let x = input_area.x
            + label.len() as u16
            + (s.input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position((x.min(input_area.right().saturating_sub(1)), input_area.y));
    }

    // Filter chips.
    let mut spans = Vec::new();
    for filter in SearchFilter::ALL {
        let style = if filter == s.filter {
            theme::SELECTED
        } else {
            theme::ACCENT
        };
        spans.push(Span::styled(format!(" {} ", filter.label()), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw("f / Tab switches").dim());
    if !s.tags.is_empty() {
        spans.push(Span::raw(format!("   tags: {} (t)", s.tags.join(" · "))).dim());
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), filter_area);

    // Results.
    let rows: Vec<Row> = s
        .results
        .iter()
        .map(|r| {
            let detail = match r.kind.as_str() {
                "a" | "t" => r.band_name.clone().unwrap_or_default(),
                "b" => r.location.clone().unwrap_or_default(),
                "f" => r
                    .collection_size
                    .map(|n| format!("{n} items"))
                    .unwrap_or_default(),
                _ => String::new(),
            };
            let extra = match r.kind.as_str() {
                "b" => r.genre_name.clone().unwrap_or_default(),
                "a" | "t" => r
                    .tag_names
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
                "f" => r.genre_name.clone().unwrap_or_default(),
                _ => String::new(),
            };
            Row::new(vec![
                Cell::from(Span::raw(r.kind_label()).dim()),
                Cell::from(r.name.clone()),
                Cell::from(detail),
                Cell::from(Span::raw(extra).dim()),
            ])
        })
        .collect();
    let mut title = if s.query.is_empty() {
        "Search".to_owned()
    } else {
        format!("Search · {} results", s.results.len())
    };
    if s.loading {
        title.push_str(&format!(" {}", theme::spinner(app.tick)));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Percentage(40),
            Constraint::Min(15),
            Constraint::Percentage(25),
        ],
    )
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(&title));
    frame.render_stateful_widget(table, table_area, &mut app.search.table);

    let s = &app.search;
    let line = if let Some(error) = &s.error {
        Line::styled(error.as_str(), theme::ERROR)
    } else if let Some(r) = s.selected() {
        Line::from(Span::raw(r.url().unwrap_or_default()).dim())
    } else if !s.query.is_empty() && !s.loading {
        Line::from("No results.").dim()
    } else {
        Line::from("Search artists, labels, albums, tracks and fans.").dim()
    };
    frame.render_widget(Paragraph::new(line), bottom_area);
}
