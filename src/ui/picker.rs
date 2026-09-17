//! The Discover filter picker: a field menu, then a filterable option list.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Clear, List, ListItem, ListState, Paragraph};

use super::{centered, theme};
use crate::app::App;
use crate::app::discover::Field;

pub fn draw(frame: &mut Frame, app: &App) {
    let Some(picker) = &app.discover.picker else {
        return;
    };
    let visible = picker.visible();
    let has_filter = picker.field.is_some();
    let rows = visible.len().clamp(1, 18) as u16;
    let height = rows + 2 + u16::from(has_filter);
    let area = centered(frame.area(), 56, height);
    frame.render_widget(Clear, area);

    let title = match picker.field {
        None => "Filters".to_owned(),
        Some(field) => field.label().to_owned(),
    };
    let block = theme::panel(&title);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let (filter_area, list_area) = if has_filter {
        let [f, l] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        (Some(f), l)
    } else {
        (None, inner)
    };

    if let Some(filter_area) = filter_area {
        let hint = match picker.field {
            Some(Field::Location) => "type to filter, or a city name and Enter to search",
            _ => "type to filter",
        };
        let value = picker.filter.value();
        let line = if value.is_empty() {
            Line::from(vec![
                Span::styled("> ", theme::ACCENT),
                Span::raw(hint).dim(),
            ])
        } else {
            Line::from(vec![Span::styled("> ", theme::ACCENT), Span::raw(value)])
        };
        frame.render_widget(Paragraph::new(line), filter_area);
        let x = filter_area.x + 2 + picker.filter.visual_cursor() as u16;
        frame.set_cursor_position((x.min(filter_area.right().saturating_sub(1)), filter_area.y));
    }

    let items: Vec<ListItem> = visible
        .iter()
        .map(|i| ListItem::new(picker.options[*i].label.clone()))
        .collect();
    let items = if items.is_empty() {
        vec![ListItem::new(Line::from("no matches").dim())]
    } else {
        items
    };
    let list = List::new(items)
        .highlight_style(theme::SELECTED)
        .highlight_symbol("▶ ");
    let mut state =
        ListState::default().with_selected((!visible.is_empty()).then_some(picker.selected));
    frame.render_stateful_widget(list, list_area, &mut state);
}
