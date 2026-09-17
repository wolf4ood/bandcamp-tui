//! Album / track detail with the track list.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use super::{fmt_duration, theme};
use crate::app::{App, PlaybackState};

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let tick = app.tick;
    let playing = (app.playback.state != PlaybackState::Idle)
        .then_some(app.playback.track_id)
        .flatten();
    let (following, wishlisted, owned) = match app.album.as_ref() {
        Some(view) => (
            app.is_following(crate::api::social::FollowTarget::Band(
                view.source.tralbum.band_id,
            )),
            app.is_wishlisted(view.source.tralbum),
            app.is_owned(view.source.tralbum),
        ),
        None => (false, false, false),
    };
    let Some(view) = app.album.as_mut() else {
        return;
    };

    let title = view
        .tralbum
        .as_ref()
        .map_or(view.source.title.clone(), |t| t.title.clone());
    let block = theme::panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header_area, tracks_area, about_area] = Layout::vertical([
        Constraint::Length(5),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .areas(inner);

    let Some(tralbum) = view.tralbum.as_ref() else {
        let text = match &view.error {
            Some(error) => Line::styled(error.as_str(), theme::ERROR),
            None => Line::from(format!("{} Loading…", theme::spinner(tick))),
        };
        frame.render_widget(Paragraph::new(text), header_area);
        return;
    };

    // Header: artist, release, price, tags.
    let mut artist = vec![Span::styled(tralbum.tralbum_artist.clone(), theme::TITLE)];
    if let Some(location) = tralbum.band.location.as_deref().filter(|s| !s.is_empty()) {
        artist.push(Span::raw(format!("  ·  {location}")).dim());
    }
    artist.push(if following {
        Span::styled("  ✓ following", theme::ACCENT)
    } else {
        Span::raw("  F to follow").dim()
    });
    if owned {
        artist.push(Span::styled("  ✓ in your collection", theme::ACCENT));
    } else if wishlisted {
        artist.push(Span::styled("  ♥ wishlisted", theme::ACCENT));
    } else {
        artist.push(Span::raw("  W to wishlist").dim());
    }
    let mut release = Vec::new();
    if let Some(date) = tralbum.release_date() {
        release.push(Span::raw(format!("released {date}")));
    }
    if let Some(label) = tralbum.label.as_deref().filter(|s| !s.is_empty()) {
        release.push(Span::raw(format!("  ·  {label}")).dim());
    }
    if tralbum.is_album() {
        release.push(
            Span::raw(format!(
                "  ·  {} tracks, {}",
                tralbum.tracks.len(),
                fmt_duration(std::time::Duration::from_secs_f64(
                    tralbum.total_duration_secs()
                ))
            ))
            .dim(),
        );
    }
    let price = match (
        tralbum.price,
        tralbum.currency.as_deref(),
        tralbum.is_purchasable,
    ) {
        (_, _, false) => "not for sale".to_owned(),
        (Some(p), Some(c), true) if p > 0.0 => format!("{c} {p:.2}"),
        (_, _, true) if tralbum.free_download => "free download".to_owned(),
        _ => "name your price".to_owned(),
    };
    let tags: Vec<String> = tralbum.tags.iter().map(|t| t.name.clone()).collect();
    let header = vec![
        Line::from(artist),
        Line::from(release),
        Line::from(vec![
            Span::styled(price, theme::ACCENT),
            Span::raw("  ·  "),
            Span::raw(tralbum.bandcamp_url.clone()).dim(),
        ]),
        Line::from(tags.join(" · ")).dim(),
    ];
    frame.render_widget(
        Paragraph::new(header).wrap(Wrap { trim: true }),
        header_area,
    );

    // Tracks.
    let rows: Vec<Row> = tralbum
        .tracks
        .iter()
        .map(|track| {
            let marker = if playing == Some(track.track_id) {
                "♫"
            } else {
                ""
            };
            let number = track.track_num.map(|n| n.to_string()).unwrap_or_default();
            let mut row = Row::new(vec![
                Cell::from(marker),
                Cell::from(number),
                Cell::from(track.title.clone()),
                Cell::from(fmt_duration(std::time::Duration::from_secs_f64(
                    track.duration.max(0.0),
                ))),
            ]);
            if track.stream_url().is_none() {
                row = row.dim();
            }
            row
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(8),
        ],
    )
    .column_spacing(1)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ");
    frame.render_stateful_widget(table, tracks_area, &mut view.table);

    // About / credits.
    let about = tralbum
        .about
        .as_deref()
        .or(tralbum.credits.as_deref())
        .unwrap_or_default()
        .replace("\r\n", " ")
        .replace('\n', " ");
    frame.render_widget(
        Paragraph::new(about).dim().wrap(Wrap { trim: true }),
        about_area,
    );
}
