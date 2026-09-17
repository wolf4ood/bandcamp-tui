//! Discover: a filter bar like the website's, a results table, and the tag entry.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table};

use super::{fmt_count, theme};
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [bar_area, related_area, table_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    draw_filter_bar(frame, app, bar_area);

    let d = &app.discover;
    let related = if d.related.is_empty() {
        Line::raw("")
    } else {
        Line::from(vec![
            Span::raw("related: ").dim(),
            Span::raw(d.related.join(" · ")).dim(),
            Span::raw("   (t adds a tag)").dim(),
        ])
    };
    frame.render_widget(Paragraph::new(related), related_area);

    let rows: Vec<Row> = d
        .results
        .iter()
        .map(|r| {
            let kind = if r.is_music() {
                r.track_count.map(|n| format!("{n} tr")).unwrap_or_default()
            } else {
                "merch".to_owned()
            };
            let released = r
                .release_date()
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let mut row = Row::new(vec![
                Cell::from(r.band_name.clone()),
                Cell::from(r.title.clone()),
                Cell::from(r.band_location.clone().unwrap_or_default()),
                Cell::from(released),
                Cell::from(r.price_label()),
                Cell::from(kind),
            ]);
            if !r.is_music() {
                row = row.dim();
            }
            row
        })
        .collect();
    let mut title = match d.total {
        Some(total) => format!(
            "Discover · {} results · {} loaded",
            fmt_count(total),
            d.results.len()
        ),
        None => format!("Discover · {} loaded", d.results.len()),
    };
    if d.loading {
        title.push_str(&format!(" {}", theme::spinner(app.tick)));
    }
    let header =
        Row::new(["Artist", "Title", "Location", "Released", "Price", ""]).style(theme::TITLE);
    let table = Table::new(
        rows,
        [
            Constraint::Percentage(24),
            Constraint::Min(20),
            Constraint::Percentage(18),
            Constraint::Length(10),
            Constraint::Length(15),
            Constraint::Length(6),
        ],
    )
    .header(header)
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(&title));
    frame.render_stateful_widget(table, table_area, &mut app.discover.table);

    draw_bottom(frame, app, bottom_area, table_area);
}

fn draw_filter_bar(frame: &mut Frame, app: &App, area: Rect) {
    let d = &app.discover;
    let f = &d.filters;
    let o = &d.options;
    let chip = |label: &str, value: String, active: bool| {
        vec![
            Span::raw(format!("{label} ")).dim(),
            Span::styled(
                value,
                if active {
                    theme::SELECTED
                } else {
                    theme::ACCENT
                },
            ),
            Span::raw("  "),
        ]
    };
    let mut spans = Vec::new();
    spans.extend(chip(
        "Genre",
        f.genre_label(o).to_owned(),
        f.genre.is_some(),
    ));
    spans.extend(chip(
        "Sub",
        f.subgenre_label(o).to_owned(),
        f.subgenre.is_some(),
    ));
    let tags = if f.tags.is_empty() {
        "+".to_owned()
    } else {
        f.tags
            .iter()
            .map(|t| format!("+{t}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    spans.extend(chip("Tags", tags, !f.tags.is_empty()));
    spans.extend(chip(
        "Format",
        f.category_label(o).to_owned(),
        f.category_id != 0,
    ));
    spans.extend(chip("Sort", f.slice_label(o).to_owned(), f.slice != "top"));
    spans.extend(chip(
        "Time",
        f.time_label(o).to_owned(),
        f.time_facet_id.is_some(),
    ));
    spans.extend(chip("Where", f.location_label.clone(), f.geoname_id != 0));
    spans.push(Span::raw("f filters · t tags").dim());
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_bottom(frame: &mut Frame, app: &App, area: Rect, table_area: Rect) {
    let d = &app.discover;
    if let Some(entry) = &d.tag_entry {
        let label = "Add tag: ";
        let width = area.width.saturating_sub(label.len() as u16).max(1) as usize;
        let scroll = entry.input.visual_scroll(width);
        let shown: String = entry
            .input
            .value()
            .chars()
            .skip(scroll)
            .take(width)
            .collect();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label, theme::ACCENT),
                Span::raw(shown),
            ])),
            area,
        );
        let x = area.x
            + label.len() as u16
            + (entry.input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));

        if !entry.suggestions.is_empty() {
            let height = (entry.suggestions.len() as u16 + 2).min(table_area.height);
            let popup = Rect::new(
                area.x,
                area.y.saturating_sub(height),
                44.min(area.width),
                height,
            );
            frame.render_widget(Clear, popup);
            let items: Vec<ListItem> = entry
                .suggestions
                .iter()
                .map(|s| {
                    let count = if s.count > 0 {
                        format!("  {}", fmt_count(s.count)).dim()
                    } else {
                        Span::raw("")
                    };
                    ListItem::new(Line::from(vec![Span::raw(s.display_name.clone()), count]))
                })
                .collect();
            let title = if entry.suggestions_for.is_empty() {
                "related tags"
            } else {
                "matching tags"
            };
            let list = List::new(items)
                .highlight_style(theme::SELECTED)
                .highlight_symbol("▶ ")
                .block(theme::panel(title));
            let mut state = ListState::default().with_selected(entry.selected);
            frame.render_stateful_widget(list, popup, &mut state);
        }
        return;
    }
    let line = if let Some(error) = &d.error {
        Line::styled(format!("{error}  (R retries)"), theme::ERROR)
    } else if let Some(r) = d.selected() {
        let mut parts = vec![Span::raw(r.clean_url()).dim()];
        if r.is_owned {
            parts.push(Span::raw("  ✓ in your collection").dim());
        } else if r.is_wishlisted {
            parts.push(Span::raw("  ♥ wishlisted").dim());
        }
        if let Some(track) = &r.featured_track {
            parts.push(Span::raw(format!("  featured: {}", track.title)).dim());
        }
        Line::from(parts)
    } else if d.started && !d.loading {
        Line::from("No results for these filters.").dim()
    } else {
        Line::raw("")
    };
    frame.render_widget(Paragraph::new(line), area);
}
