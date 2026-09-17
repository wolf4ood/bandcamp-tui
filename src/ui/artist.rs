//! Artist / label page: bio, links, discography (or roster for labels).

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use super::theme;
use crate::api::social::FollowTarget;
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let tick = app.tick;
    let following = app
        .artist
        .as_ref()
        .is_some_and(|a| app.is_following(FollowTarget::Band(a.band_id)));
    let owned: Vec<bool> = app
        .artist
        .as_ref()
        .and_then(|a| a.details.as_ref())
        .map(|d| {
            d.discography
                .iter()
                .map(|i| app.is_owned(i.tralbum()))
                .collect()
        })
        .unwrap_or_default();
    let Some(view) = app.artist.as_mut() else {
        return;
    };

    let block = theme::panel(&view.name);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header_area, list_area, bottom_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .areas(inner);

    let Some(details) = view.details.as_ref() else {
        let text = match &view.error {
            Some(error) => Line::styled(error.as_str(), theme::ERROR),
            None => Line::from(format!("{} Loading…", theme::spinner(tick))),
        };
        frame.render_widget(Paragraph::new(text), header_area);
        return;
    };

    let mut first = vec![Span::styled(details.name.clone(), theme::TITLE)];
    if details.is_label() {
        first.push(Span::raw("  label").dim());
    }
    if let Some(location) = details.location.as_deref().filter(|s| !s.is_empty()) {
        first.push(Span::raw(format!("  ·  {location}")).dim());
    }
    first.push(if following {
        Span::styled("  ✓ following", theme::ACCENT)
    } else {
        Span::raw("  F to follow").dim()
    });
    let mut links = vec![Span::raw(details.bandcamp_url.clone()).dim()];
    for site in details.sites.iter().take(3) {
        links.push(Span::raw(format!("  ·  {}", site.title)).dim());
    }
    let mut counts = vec![Span::raw(format!("{} releases", details.discography.len()))];
    if details.is_label() {
        counts.push(Span::raw(format!("  ·  {} artists", details.artists.len())));
    }
    if !details.shows.is_empty() {
        counts.push(Span::raw(format!(
            "  ·  {} upcoming shows",
            details.shows.len()
        )));
    }
    let bio = details
        .bio
        .as_deref()
        .unwrap_or_default()
        .replace("\r\n", " ")
        .replace('\n', " ");
    let header = vec![
        Line::from(first),
        Line::from(links),
        Line::from(counts).dim(),
        Line::from(bio).dim(),
    ];
    frame.render_widget(
        Paragraph::new(header).wrap(Wrap { trim: true }),
        header_area,
    );

    if view.roster {
        let rows: Vec<Row> = details
            .artists
            .iter()
            .map(|a| {
                Row::new(vec![
                    Cell::from(a.name.clone()),
                    Cell::from(a.location.clone().unwrap_or_default()),
                ])
            })
            .collect();
        let table = Table::new(rows, [Constraint::Percentage(55), Constraint::Min(10)])
            .header(Row::new(["Artist", "Location"]).style(theme::TITLE))
            .column_spacing(2)
            .row_highlight_style(theme::SELECTED)
            .highlight_symbol("▶ ");
        frame.render_stateful_widget(table, list_area, &mut view.roster_table);
        frame.render_widget(
            Paragraph::new(Line::from("Enter opens the artist · w back to the discography").dim()),
            bottom_area,
        );
        return;
    }

    let rows: Vec<Row> = details
        .discography
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let released = item
                .release_date()
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            let artist = item
                .artist()
                .filter(|a| *a != details.name)
                .unwrap_or_default()
                .to_owned();
            let mark = if owned.get(i).copied().unwrap_or(false) {
                "✓"
            } else {
                ""
            };
            Row::new(vec![
                Cell::from(mark),
                Cell::from(Span::raw(if item.is_album() { "album" } else { "track" }).dim()),
                Cell::from(item.title.clone()),
                Cell::from(Span::raw(artist).dim()),
                Cell::from(released),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(5),
            Constraint::Min(20),
            Constraint::Percentage(25),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(["", "", "Title", "Artist", "Released"]).style(theme::TITLE))
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, list_area, &mut view.table);
    let hint = if details.is_label() {
        "w shows the label's artists"
    } else {
        "✓ marks releases in your collection"
    };
    frame.render_widget(Paragraph::new(Line::from(hint).dim()), bottom_area);
}
