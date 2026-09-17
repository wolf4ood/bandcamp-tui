//! Feed: the story stream and the following lists.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Cell, Paragraph, Row, Table};

use super::theme;
use crate::api::feed::StoryKind;
use crate::api::social::FollowTarget;
use crate::app::App;
use crate::app::feed::FeedTab;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let [tabs_area, table_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let mut spans = Vec::new();
    for tab in FeedTab::ALL {
        let style = if tab == app.feed.tab {
            theme::SELECTED
        } else {
            theme::ACCENT
        };
        spans.push(Span::styled(format!(" {} ", tab.title()), style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::raw("w switches").dim());
    if app.feed.tab == FeedTab::Stories {
        spans.push(Span::raw(format!("   showing {} (v)", app.feed.filter.label())).dim());
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), tabs_area);

    match app.feed.tab.follow_list() {
        None => draw_stories(frame, app, table_area),
        Some(list) => draw_followees(frame, app, list, table_area),
    }
    draw_bottom(frame, app, bottom_area);
}

fn draw_stories(frame: &mut Frame, app: &mut App, area: Rect) {
    let tick = app.tick;
    let visible = app.feed.visible_stories();
    let rows: Vec<Row> = visible
        .iter()
        .map(|i| {
            let s = &app.feed.stories.items[*i];
            let date = s
                .date
                .map(|d| d.format("%b %d").to_string())
                .unwrap_or_default();
            let kind = match s.kind {
                StoryKind::NewRelease => Span::styled("new", theme::ACCENT),
                StoryKind::FanPurchase => Span::raw("bought").dim(),
                StoryKind::Other => Span::raw(s.story_type.clone()).dim(),
            };
            let mut text = vec![Span::raw(s.headline())];
            if let Some(why) = &s.why {
                text.push(Span::raw(format!("  “{why}”")).dim());
            }
            Row::new(vec![
                Cell::from(date),
                Cell::from(Line::from(kind)),
                Cell::from(Line::from(text)),
            ])
        })
        .collect();
    let page = &app.feed.stories;
    let mut title = format!("Feed · {} stories", visible.len());
    if page.loading {
        title.push_str(&format!(" {}", theme::spinner(tick)));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Min(20),
        ],
    )
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(&title));
    frame.render_stateful_widget(table, area, &mut app.feed.stories.table);
}

fn draw_followees(
    frame: &mut Frame,
    app: &mut App,
    list: crate::api::social::FollowList,
    area: Rect,
) {
    let tick = app.tick;
    let followed_bands = app.followed_bands.clone();
    let followed_fans = app.followed_fans.clone();
    let page = app.feed.follow_page_mut(list);
    let rows: Vec<Row> = page
        .items
        .iter()
        .map(|f| {
            let following = if f.is_band() {
                f.band_id.is_some_and(|b| followed_bands.contains(&b))
            } else {
                f.fan_id.is_some_and(|fan| followed_fans.contains(&fan))
            };
            let since = f
                .date_followed()
                .map(|d| d.format("%Y-%m-%d").to_string())
                .unwrap_or_default();
            Row::new(vec![
                Cell::from(if following { "✓" } else { "" }),
                Cell::from(f.name.clone()),
                Cell::from(f.location.clone().unwrap_or_default()),
                Cell::from(since),
            ])
        })
        .collect();
    let mut title = format!(
        "{} · {} loaded{}",
        list.title(),
        page.items.len(),
        if page.more { "+" } else { "" }
    );
    if page.loading {
        title.push_str(&format!(" {}", theme::spinner(tick)));
    }
    let header = Row::new(["", "Name", "Location", "Since"]).style(theme::TITLE);
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Percentage(45),
            Constraint::Min(10),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .column_spacing(2)
    .row_highlight_style(theme::SELECTED)
    .highlight_symbol("▶ ")
    .block(theme::panel(&title));
    frame.render_stateful_widget(table, area, &mut page.table);
}

fn draw_bottom(frame: &mut Frame, app: &App, area: Rect) {
    let error = match app.feed.tab.follow_list() {
        None => app.feed.stories.error.as_deref(),
        Some(list) => app.feed.follow_page(list).error.as_deref(),
    };
    let line = if let Some(error) = error {
        Line::styled(format!("{error}  (R reloads)"), theme::ERROR)
    } else if let Some(story) = app
        .feed
        .selected_story()
        .filter(|_| app.feed.tab == FeedTab::Stories)
    {
        let mut parts = vec![Span::raw(story.item_url.clone().unwrap_or_default()).dim()];
        if let Some(band) = story.band_id
            && app.is_following(FollowTarget::Band(band))
        {
            parts.push(Span::raw("  ✓ following").dim());
        }
        if let Some(fan) = story.fan_id
            && app.is_following(FollowTarget::Fan(fan))
        {
            parts.push(Span::raw("  ✓ following this fan").dim());
        }
        if let Some(item) = story.item {
            if app.is_owned(item) {
                parts.push(Span::raw("  ✓ owned").dim());
            } else if app.is_wishlisted(item) {
                parts.push(Span::raw("  ♥ wishlisted").dim());
            }
        }
        if let Some(n) = story.also_collected_count {
            parts.push(Span::raw(format!("  ♥ {n}")).dim());
        }
        Line::from(parts)
    } else if let Some(f) = app.feed.selected_followee() {
        Line::from(Span::raw(f.url().unwrap_or_default()).dim())
    } else if app.session.is_none() {
        Line::from("Log in (L) to see your feed.").dim()
    } else {
        let started = match app.feed.tab.follow_list() {
            None => app.feed.stories.started && !app.feed.stories.loading,
            Some(list) => app.feed.follow_page(list).started && !app.feed.follow_page(list).loading,
        };
        if started {
            Line::from("Nothing here yet.").dim()
        } else {
            Line::raw("")
        }
    };
    frame.render_widget(Paragraph::new(line), area);
}
