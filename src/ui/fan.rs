//! Another fan's public collection.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::{library, theme};
use crate::api::social::FollowTarget;
use crate::app::App;

pub fn draw(frame: &mut Frame, app: &mut App, area: Rect) {
    let tick = app.tick;
    let following = app
        .fan
        .as_ref()
        .is_some_and(|f| app.is_following(FollowTarget::Fan(f.fan_id)));
    let Some(view) = app.fan.as_mut() else { return };
    let [header_area, table_area, bottom_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);

    let mut header = vec![Span::styled(view.name.clone(), theme::TITLE)];
    if let Some(url) = &view.url {
        header.push(Span::raw(format!("  {url}")).dim());
    }
    header.push(if following {
        Span::styled("  ✓ following", theme::ACCENT)
    } else {
        Span::raw("  F to follow").dim()
    });
    frame.render_widget(Paragraph::new(Line::from(header)), header_area);

    let list = &mut view.list;
    let mut title = format!(
        "{}'s collection · {} loaded{}",
        view.name,
        list.items.len(),
        if list.more { "+" } else { "" }
    );
    if list.loading {
        title.push_str(&format!(" {}", theme::spinner(tick)));
    }
    let table = library::items_table(list.visible(), &title);
    frame.render_stateful_widget(table, table_area, list.table_state());

    let line = if let Some(error) = &list.error {
        Line::styled(error.as_str(), theme::ERROR)
    } else if let Some(item) = list.selected_item() {
        Line::from(Span::raw(item.item_url.clone()).dim())
    } else if list.started && !list.loading {
        Line::from("This collection is empty or private.").dim()
    } else {
        Line::raw("")
    };
    frame.render_widget(Paragraph::new(line), bottom_area);
}
