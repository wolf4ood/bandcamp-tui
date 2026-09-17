//! Persistent now-playing bar with progress.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Gauge, Paragraph};

use super::{fmt_duration, theme};
use crate::app::{App, PlaybackState};

pub fn draw(frame: &mut Frame, app: &App, area: Rect) {
    let block = theme::panel("Now playing");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let right = match app.queue.current {
        Some(i) => format!(
            "vol {}%  ·  {}/{}",
            app.config.volume,
            i + 1,
            app.queue.len()
        ),
        None if app.queue.is_empty() => format!("vol {}%", app.config.volume),
        None => format!("vol {}%  ·  {} queued", app.config.volume, app.queue.len()),
    };
    let right_width = right.chars().count() as u16 + 1;
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).areas(top);
    frame.render_widget(Paragraph::new(right).right_aligned().dim(), right_area);

    let track = app.queue.current_track();
    let (icon, title_line) = match (app.playback.state, track) {
        (PlaybackState::Idle, _) | (_, None) => (
            Span::styled("■ ", theme::ACCENT),
            Line::from(Span::raw("Nothing playing").dim()),
        ),
        (state, Some(track)) => {
            let icon = match state {
                PlaybackState::Loading => theme::spinner(app.tick),
                PlaybackState::Playing => "▶",
                PlaybackState::Paused => "⏸",
                PlaybackState::Idle => "■",
            };
            let mut spans = vec![
                Span::styled(track.title.clone(), theme::TITLE),
                Span::raw(format!("  {}", track.artist)),
            ];
            if let Some(album) = &track.album {
                spans.push(Span::raw(format!("  ·  {album}")).dim());
            }
            (
                Span::styled(format!("{icon} "), theme::ACCENT),
                Line::from(spans),
            )
        }
    };
    let mut spans = vec![icon];
    spans.extend(title_line.spans);
    frame.render_widget(Paragraph::new(Line::from(spans)), left_area);

    if let Some(message) = &app.playback.unavailable {
        frame.render_widget(
            Paragraph::new(format!("audio unavailable: {message}")).style(theme::WARNING),
            bottom,
        );
        return;
    }
    let duration = track.map(|t| t.duration).unwrap_or_default();
    let position = app
        .playback
        .position
        .min(duration.max(app.playback.position));
    let ratio = if duration.is_zero() {
        0.0
    } else {
        (position.as_secs_f64() / duration.as_secs_f64()).clamp(0.0, 1.0)
    };
    let label = if track.is_some() {
        format!("{} / {}", fmt_duration(position), fmt_duration(duration))
    } else {
        String::new()
    };
    frame.render_widget(
        Gauge::default()
            .ratio(ratio)
            .label(label)
            .use_unicode(true)
            .gauge_style(theme::ACCENT.on_black()),
        bottom,
    );
}
