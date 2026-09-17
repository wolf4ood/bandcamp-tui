//! Bandcamp Daily: the article list and an open article with its featured tracks.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table, Wrap};

use super::{fmt_duration, theme};
use crate::api::daily::{ArticleBlock, Franchise};
use crate::app::App;
use crate::app::daily::ArticleFocus;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.daily.view.is_some() {
        draw_article(frame, app, area);
    } else {
        draw_list(frame, app, area);
    }
}

// ----- article list ------------------------------------------------------------------

fn draw_list(frame: &mut Frame, app: &mut App, area: Rect) {
    let [tabs_area, table_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let mut spans = Vec::new();
    for franchise in Franchise::ALL {
        let style = if franchise == app.daily.franchise {
            theme::SELECTED
        } else {
            theme::ACCENT
        };
        spans.push(Span::styled(format!(" {} ", franchise.label()), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw("v/V switch").dim());
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_area);

    let tick = app.tick;
    let page = &app.daily.articles;
    let rows: Vec<Row> = page
        .items
        .iter()
        .map(|a| {
            let date = a
                .date
                .map(|d| d.format("%b %d").to_string())
                .unwrap_or_default();
            Row::new(vec![
                Cell::from(date),
                Cell::from(Span::styled(a.franchise.clone(), theme::ACCENT)),
                Cell::from(a.title.clone()),
            ])
        })
        .collect();
    let mut title = format!(
        "Daily · {} · {} articles{}",
        app.daily.franchise.label(),
        page.items.len(),
        if page.more { "+" } else { "" }
    );
    if page.loading {
        title.push_str(&format!(" {}", theme::spinner(tick)));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(17),
            Constraint::Min(20),
        ],
    )
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(&title));
    frame.render_stateful_widget(table, table_area, &mut app.daily.articles.table);

    let page = &app.daily.articles;
    let line = if let Some(error) = page.error.as_deref() {
        Line::styled(format!("{error}  (R reloads)"), theme::ERROR)
    } else if let Some(article) = app.daily.selected_article() {
        Line::from(Span::raw(article.url.clone()).dim())
    } else if page.started && !page.loading {
        Line::from("Nothing here yet.").dim()
    } else {
        Line::raw("")
    };
    frame.render_widget(Paragraph::new(line), bottom_area);
}

// ----- open article ------------------------------------------------------------------

fn draw_article(frame: &mut Frame, app: &mut App, area: Rect) {
    let tick = app.tick;
    let Some(view) = app.daily.view.as_mut() else {
        return;
    };

    let block = theme::panel(&view.article.title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header_area, body_area] =
        Layout::vertical([Constraint::Length(4), Constraint::Min(3)]).areas(inner);

    // Header: section · date · author, then the blurb.
    let mut meta = vec![Span::styled(view.article.franchise.clone(), theme::TITLE)];
    if let Some(date) = view.article.date {
        meta.push(Span::raw(format!("  ·  {}", date.format("%B %-d, %Y"))).dim());
    }
    if let Some(author) = view.detail.as_ref().and_then(|d| d.author.as_deref()) {
        meta.push(Span::raw(format!("  ·  by {author}")).dim());
    }
    let mut header = vec![Line::from(meta)];
    match (&view.detail, &view.error) {
        (None, Some(error)) => {
            header.push(Line::styled(format!("{error}  (R retries)"), theme::ERROR))
        }
        (None, None) => header.push(Line::from(format!("{} Loading…", theme::spinner(tick)))),
        (Some(detail), _) => {
            if let Some(blurb) = detail.blurb.as_deref() {
                header.push(Line::from(Span::raw(blurb).italic()));
            }
        }
    }
    frame.render_widget(
        Paragraph::new(header).wrap(Wrap { trim: true }),
        header_area,
    );

    let Some(detail) = view.detail.as_ref() else {
        return;
    };

    // Body: tracks beside the text, or stacked when the terminal is narrow.
    let track_rows = detail.featured.len() as u16;
    let (tracks_area, text_area) = if track_rows == 0 {
        (None, body_area)
    } else if body_area.width >= 100 {
        let [left, right] =
            Layout::horizontal([Constraint::Percentage(45), Constraint::Percentage(55)])
                .areas(body_area);
        (Some(left), right)
    } else {
        let height = (track_rows + 2).min(body_area.height / 2).max(3);
        let [top, bottom] =
            Layout::vertical([Constraint::Length(height), Constraint::Min(3)]).areas(body_area);
        (Some(top), bottom)
    };

    let tracks_focused = view.focus == ArticleFocus::Tracks && tracks_area.is_some();
    if let Some(tracks_area) = tracks_area {
        let rows: Vec<Row> = detail
            .featured
            .iter()
            .map(|t| {
                let streamable = t.stream_url.is_some();
                let row = Row::new(vec![
                    Cell::from(t.artist.clone()),
                    Cell::from(t.title.clone()),
                    Cell::from(Span::raw(t.album.clone()).dim()),
                    Cell::from(if streamable {
                        fmt_duration(t.duration)
                    } else {
                        "—".to_owned()
                    }),
                ]);
                if streamable { row } else { row.dim() }
            })
            .collect();
        let title = format!(
            "Tracks · {}{}",
            detail.featured.len(),
            if tracks_focused { "" } else { " · w to focus" }
        );
        let highlight = if tracks_focused {
            theme::SELECTED
        } else {
            Style::new().add_modifier(Modifier::BOLD)
        };
        let table = Table::new(
            rows,
            [
                Constraint::Percentage(30),
                Constraint::Min(12),
                Constraint::Percentage(30),
                Constraint::Length(5),
            ],
        )
        .column_spacing(2)
        .row_highlight_style(highlight)
        .highlight_symbol(if tracks_focused { "▶ " } else { "  " })
        .block(theme::panel(&title));
        frame.render_stateful_widget(table, tracks_area, &mut view.tracks);
    }

    // Reader: wrapped by hand so the scroll offset can be clamped to the text.
    let text_focused = !tracks_focused;
    let width = text_area.width.saturating_sub(2) as usize;
    let lines = wrap_blocks(&detail.body, width.max(10));
    let visible = text_area.height.saturating_sub(2) as usize;
    let max_scroll = lines.len().saturating_sub(visible) as u16;
    view.scroll = view.scroll.min(max_scroll);
    let title = if lines.is_empty() {
        "Article".to_owned()
    } else {
        format!(
            "Article · {}%{}",
            if max_scroll == 0 {
                100
            } else {
                (u32::from(view.scroll) * 100 / u32::from(max_scroll)) as u16
            },
            if text_focused { "" } else { " · w to focus" }
        )
    };
    let mut block = theme::panel(&title);
    if text_focused {
        block = block.border_style(theme::ACCENT);
    }
    let body = if lines.is_empty() {
        vec![Line::from("No text found; o opens the article in the browser.").dim()]
    } else {
        lines
    };
    frame.render_widget(
        Paragraph::new(body).block(block).scroll((view.scroll, 0)),
        text_area,
    );
}

/// Greedy word wrap of the article blocks, one blank line between blocks.
fn wrap_blocks(blocks: &[ArticleBlock], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for (i, block) in blocks.iter().enumerate() {
        if i > 0 {
            lines.push(Line::raw(""));
        }
        let (text, style) = match block {
            ArticleBlock::Heading(text) => (text, theme::TITLE),
            ArticleBlock::Paragraph(text) => (text, Style::new()),
        };
        for wrapped in wrap_words(text, width) {
            lines.push(Line::from(Span::styled(wrapped, style)));
        }
    }
    lines
}

fn wrap_words(text: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if Span::raw(candidate.as_str()).width() <= width || current.is_empty() {
            current = candidate;
        } else {
            lines.push(std::mem::replace(&mut current, word.to_owned()));
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_word_boundaries() {
        assert_eq!(
            wrap_words("the quick brown fox jumps", 10),
            vec!["the quick", "brown fox", "jumps"]
        );
        assert_eq!(
            wrap_words("supercalifragilistic", 5),
            vec!["supercalifragilistic"]
        );
        assert!(wrap_words("   ", 5).is_empty());
    }

    #[test]
    fn blocks_are_separated_by_blank_lines() {
        let lines = wrap_blocks(
            &[
                ArticleBlock::Heading("Head".into()),
                ArticleBlock::Paragraph("one two".into()),
            ],
            80,
        );
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[1].width(), 0);
    }
}
