//! Collection / wishlist listing.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use super::theme;
use crate::api::fan::ListKind;
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [tabs_area, table_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let collection_count = app.session.as_ref().map(|s| s.item_count);
    draw_tabs(frame, app, tabs_area, collection_count);

    let tab = app.library.tab;
    let list = app.library.list_mut();
    let loaded = list.visible().len();
    let mut title = match (&list.search, tab) {
        (Some(search), _) => format!("{} · \"{}\" · {loaded} matches", tab.title(), search.key),
        (None, ListKind::Collection) => match collection_count {
            Some(total) => format!("Collection · {loaded} of {total} loaded"),
            None => format!("Collection · {loaded} loaded"),
        },
        (None, ListKind::Wishlist) => {
            let more = if list.more { "+" } else { "" };
            format!("Wishlist · {loaded}{more} loaded")
        }
    };
    if list.sort != crate::app::library::SortKey::Recent {
        title.push_str(&format!(" · by {}", list.sort.label()));
    }
    if list.loading || list.search.as_ref().is_some_and(|s| s.loading) {
        title.push_str(&format!(" {}", theme::spinner(app.tick)));
    }

    let list = app.library.list_mut();
    let table = items_table(list.visible(), &title);
    frame.render_stateful_widget(table, table_area, list.table_state());

    draw_bottom(frame, app, bottom_area);
}

/// The album/track table shared by the library and fan pages.
pub fn items_table<'a>(items: &[crate::api::models::CollectionItem], title: &'a str) -> Table<'a> {
    let rows: Vec<Row> = items
        .iter()
        .map(|item| {
            let tracks = if item.is_album() {
                item.num_streamable_tracks
                    .map(|n| format!("{n} tr"))
                    .unwrap_or_default()
            } else {
                "single".to_owned()
            };
            let date = item
                .date()
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let mut row = Row::new(vec![
                Cell::from(item.band_name.clone()),
                Cell::from(item.item_title.clone()),
                Cell::from(tracks),
                Cell::from(date),
            ]);
            if item.hidden || item.is_preorder {
                row = row.dim();
            }
            row
        })
        .collect();
    let header = Row::new(["Artist", "Title", "Tracks", "Date"]).style(theme::TITLE);
    Table::new(
        rows,
        [
            Constraint::Percentage(32),
            Constraint::Min(20),
            Constraint::Length(7),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(title))
}

fn draw_tabs(frame: &mut Frame, app: &App, area: Rect, collection_count: Option<usize>) {
    let mut spans = Vec::new();
    for kind in [ListKind::Collection, ListKind::Wishlist] {
        let label = match (kind, collection_count) {
            (ListKind::Collection, Some(n)) => format!(" Collection ({n}) "),
            (ListKind::Collection, None) => " Collection ".to_owned(),
            (ListKind::Wishlist, _) => " Wishlist ".to_owned(),
        };
        let style = if kind == app.library.tab {
            theme::SELECTED
        } else {
            theme::ACCENT
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw("w switches").dim());
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_bottom(frame: &mut Frame, app: &App, area: Rect) {
    if app.library.search_open {
        let label = format!("Search {}: ", app.library.tab.title().to_lowercase());
        let width = area
            .width
            .saturating_sub(label.chars().count() as u16)
            .max(1) as usize;
        let input = &app.library.search_input;
        let scroll = input.visual_scroll(width);
        let shown: String = input.value().chars().skip(scroll).take(width).collect();
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(label.clone(), theme::ACCENT),
                Span::raw(shown),
            ])),
            area,
        );
        let x = area.x
            + label.chars().count() as u16
            + (input.visual_cursor().saturating_sub(scroll)) as u16;
        frame.set_cursor_position((x.min(area.right().saturating_sub(1)), area.y));
        return;
    }
    let list = app.library.list();
    let line = if let Some(error) = &list.error {
        Line::styled(format!("{error}  (R reloads)"), theme::ERROR)
    } else if let Some(item) = list.selected_item() {
        let mut parts = vec![Span::raw(item.item_url.clone()).dim()];
        if item.hidden {
            parts.push(Span::raw("  hidden").dim());
        }
        if item.is_preorder {
            parts.push(Span::raw("  pre-order").dim());
        }
        if let Some(n) = item.also_collected_count {
            parts.push(Span::raw(format!("  ♥ {n}")).dim());
        }
        Line::from(parts)
    } else if app.session.is_none() {
        Line::from("Log in (L) to see your collection.").dim()
    } else if list.started && !list.loading {
        Line::from("Nothing here yet.").dim()
    } else {
        Line::raw("")
    };
    frame.render_widget(Paragraph::new(line), area);
}
