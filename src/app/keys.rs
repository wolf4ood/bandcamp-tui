//! Keymap. One place to look when a key does something surprising.

use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use tui_input::backend::crossterm::to_input_request;

use super::{App, Intent, Screen, Section};

pub fn handle_key(app: &mut App, key: KeyEvent) {
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    if ctrl && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q')) {
        app.quit();
        return;
    }

    if app.show_help {
        if matches!(
            key.code,
            KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::F(1)
        ) {
            app.show_help = false;
        }
        return;
    }

    match app.screen {
        Screen::Booting => {}
        Screen::Login => login_key(app, key),
        Screen::Main => main_key(app, key),
    }
}

fn login_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::F(1) => app.toggle_help(),
        KeyCode::Enter => app.submit_login(),
        KeyCode::Esc => app.close_login(),
        _ if app.login.busy => {}
        _ => {
            if let Some(request) = to_input_request(&Event::Key(key)) {
                app.login.input.handle(request);
            }
        }
    }
}

fn main_key(app: &mut App, key: KeyEvent) {
    if app.show_queue {
        queue_key(app, key);
        return;
    }
    if app.section == Section::Collection && app.library.search_open {
        search_input_key(app, key);
        return;
    }
    if app.section == Section::Search
        && app.search.input_open
        && !app.album_open_here()
        && !app.artist_open_here()
        && !app.fan_open_here()
    {
        global_search_input_key(app, key);
        return;
    }
    if app.section == Section::Discover {
        if app.discover.picker.is_some() {
            picker_key(app, key);
            return;
        }
        if app.discover.tag_entry.is_some() {
            tag_entry_key(app, key);
            return;
        }
    }

    match key.code {
        KeyCode::Char('q') => app.quit(),
        KeyCode::Char('?') | KeyCode::F(1) => app.toggle_help(),
        KeyCode::Tab => app.set_section(app.section.next()),
        KeyCode::BackTab => app.set_section(app.section.prev()),
        KeyCode::Char(c @ '1'..='5') => {
            let index = c.to_digit(10).unwrap_or(1) as usize - 1;
            app.set_section(Section::ALL[index]);
        }
        KeyCode::Char('r') => app.refresh_session(),
        KeyCode::Char('L') => app.toggle_login(),
        // Playback works from every section.
        KeyCode::Char(' ') => app.toggle_pause(),
        KeyCode::Char('n') => app.next_track(),
        KeyCode::Char('b') => app.prev_track(),
        KeyCode::Char('s') => app.stop_playback(),
        KeyCode::Left => app.seek_by(-10),
        KeyCode::Right => app.seek_by(10),
        KeyCode::Char('+') | KeyCode::Char('=') => app.change_volume(5),
        KeyCode::Char('-') => app.change_volume(-5),
        KeyCode::Char('Q') => app.toggle_queue(),
        KeyCode::Char('F') => app.toggle_follow(),
        KeyCode::Char('W') => app.toggle_wishlist(),
        KeyCode::Char('A') => app.open_artist_on_screen(),
        _ if app.album_open_here() => album_key(app, key),
        _ if app.artist_open_here() => artist_key(app, key),
        _ if app.fan_open_here() => fan_key(app, key),
        _ => match app.section {
            Section::Collection => library_key(app, key),
            Section::Discover => discover_key(app, key),
            Section::Feed => feed_key(app, key),
            Section::Search => search_key(app, key),
            Section::Daily => daily_key(app, key),
        },
    }
}

fn library_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.library_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.library_move(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.library_move(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.library_move(-10),
        KeyCode::Char('g') | KeyCode::Home => app.library.list_mut().select(0),
        KeyCode::Char('G') | KeyCode::End => app.library.list_mut().select_last(),
        KeyCode::Enter | KeyCode::Char('l') => app.open_selected(Intent::Show),
        KeyCode::Char('p') => app.open_selected(Intent::Play { start_track: None }),
        KeyCode::Char('a') => app.open_selected(Intent::Enqueue),
        KeyCode::Char('o') => {
            let url = app
                .library
                .list()
                .selected_item()
                .map(|i| i.item_url.clone());
            app.open_in_browser(url);
        }
        KeyCode::Char('/') => app.library.search_open = true,
        KeyCode::Char('w') => app.toggle_library_tab(),
        KeyCode::Char('S') => app.library.list_mut().cycle_sort(),
        KeyCode::Char('R') => app.reload_library(),
        KeyCode::Esc => app.clear_search(),
        _ => {}
    }
}

fn search_input_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => {
            let key = app.library.search_input.value().trim().to_owned();
            app.library.search_open = false;
            if key.is_empty() {
                app.clear_search();
            } else {
                app.run_search(key);
            }
        }
        KeyCode::Esc => {
            app.library.search_open = false;
            app.clear_search();
        }
        _ => {
            if let Some(request) = to_input_request(&Event::Key(key)) {
                app.library.search_input.handle(request);
            }
        }
    }
}

fn album_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.album_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.album_move(-1),
        KeyCode::Char('g') | KeyCode::Home => app.album_move(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.album_move(isize::MAX / 2),
        KeyCode::Enter => app.play_album_from_selection(),
        KeyCode::Char('p') => app.play_album(None),
        KeyCode::Char('a') => app.enqueue_album(),
        KeyCode::Char('o') => {
            let url = app.album.as_ref().map(|a| {
                a.tralbum
                    .as_ref()
                    .map_or(a.source.url.clone(), |t| t.bandcamp_url.clone())
            });
            app.open_in_browser(url);
        }
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => {
            app.close_top_view();
        }
        _ => {}
    }
}

fn artist_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.artist_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.artist_move(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.artist_move(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.artist_move(-10),
        KeyCode::Char('g') | KeyCode::Home => app.artist_move(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.artist_move(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('l') => app.artist_open(Intent::Show),
        KeyCode::Char('p') => app.artist_open(Intent::Play { start_track: None }),
        KeyCode::Char('a') => app.artist_open(Intent::Enqueue),
        KeyCode::Char('w') => app.artist_toggle_roster(),
        KeyCode::Char('o') => {
            let url = app.artist.as_ref().and_then(|a| a.url.clone());
            app.open_in_browser(url);
        }
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => {
            app.close_top_view();
        }
        _ => {}
    }
}

fn fan_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.fan_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.fan_move(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.fan_move(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.fan_move(-10),
        KeyCode::Char('g') | KeyCode::Home => app.fan_move(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.fan_move(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('l') => app.fan_open(Intent::Show),
        KeyCode::Char('p') => app.fan_open(Intent::Play { start_track: None }),
        KeyCode::Char('a') => app.fan_open(Intent::Enqueue),
        KeyCode::Char('o') => {
            let url = app.fan.as_ref().and_then(|f| f.url.clone());
            app.open_in_browser(url);
        }
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => {
            app.close_top_view();
        }
        _ => {}
    }
}

fn search_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.search.move_by(1),
        KeyCode::Char('k') | KeyCode::Up => app.search.move_by(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.search.move_by(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.search.move_by(-10),
        KeyCode::Char('g') | KeyCode::Home => app.search.move_by(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.search.move_by(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('l') => app.search_open(Intent::Show),
        KeyCode::Char('p') => app.search_open(Intent::Play { start_track: None }),
        KeyCode::Char('a') => app.search_open(Intent::Enqueue),
        KeyCode::Char('o') => {
            let url = app.search.selected().and_then(|r| r.url());
            app.open_in_browser(url);
        }
        KeyCode::Char('/') | KeyCode::Char('i') => app.search.input_open = true,
        KeyCode::Char('f') => app.cycle_search_filter(),
        KeyCode::Char('t') => app.search_tag_to_discover(),
        _ => {}
    }
}

fn global_search_input_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Enter => app.run_global_search(),
        KeyCode::Esc => app.search.input_open = false,
        KeyCode::Tab => app.cycle_search_filter(),
        KeyCode::Down if !app.search.results.is_empty() => app.search.input_open = false,
        _ => {
            if let Some(request) = to_input_request(&Event::Key(key)) {
                app.search.input.handle(request);
            }
        }
    }
}

fn queue_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.queue_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.queue_move(-1),
        KeyCode::Enter => {
            let index = app.queue_selected;
            app.play_index(index);
        }
        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => app.queue_remove_selected(),
        KeyCode::Char('c') => app.queue_clear(),
        KeyCode::Char(' ') => app.toggle_pause(),
        KeyCode::Char('n') => app.next_track(),
        KeyCode::Char('b') => app.prev_track(),
        KeyCode::Esc | KeyCode::Char('Q') | KeyCode::Char('q') => app.show_queue = false,
        _ => {}
    }
}

fn discover_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.discover_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.discover_move(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.discover_move(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.discover_move(-10),
        KeyCode::Char('g') | KeyCode::Home => app.discover_move(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.discover_move(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('l') => app.discover_open(Intent::Show),
        KeyCode::Char('p') => app.discover_open(Intent::Play { start_track: None }),
        KeyCode::Char('a') => app.discover_open(Intent::Enqueue),
        KeyCode::Char('o') => {
            let url = app.discover.selected().map(|r| r.clean_url());
            app.open_in_browser(url);
        }
        KeyCode::Char('f') => app.open_picker(None),
        KeyCode::Char('t') => app.open_tag_entry(),
        KeyCode::Char('x') => app.remove_last_tag(),
        KeyCode::Char('X') => app.reset_discover_filters(),
        KeyCode::Char('R') => app.discover_search(),
        _ => {}
    }
}

fn picker_key(app: &mut App, key: KeyEvent) {
    let field_menu = app
        .discover
        .picker
        .as_ref()
        .is_some_and(|p| p.field.is_none());
    match key.code {
        KeyCode::Esc => app.picker_back(),
        KeyCode::Enter => app.picker_enter(),
        KeyCode::Down => app
            .discover
            .picker
            .as_mut()
            .map(|p| p.move_by(1))
            .unwrap_or(()),
        KeyCode::Up => app
            .discover
            .picker
            .as_mut()
            .map(|p| p.move_by(-1))
            .unwrap_or(()),
        KeyCode::PageDown => app
            .discover
            .picker
            .as_mut()
            .map(|p| p.move_by(10))
            .unwrap_or(()),
        KeyCode::PageUp => app
            .discover
            .picker
            .as_mut()
            .map(|p| p.move_by(-10))
            .unwrap_or(()),
        KeyCode::Char('j') if field_menu => app
            .discover
            .picker
            .as_mut()
            .map(|p| p.move_by(1))
            .unwrap_or(()),
        KeyCode::Char('k') if field_menu => app
            .discover
            .picker
            .as_mut()
            .map(|p| p.move_by(-1))
            .unwrap_or(()),
        KeyCode::Char('q') | KeyCode::Char('f') if field_menu => app.discover.picker = None,
        _ if field_menu => {}
        _ => {
            if let Some(request) = to_input_request(&Event::Key(key)) {
                app.picker_input(request);
            }
        }
    }
}

fn tag_entry_key(app: &mut App, key: KeyEvent) {
    let empty = app
        .discover
        .tag_entry
        .as_ref()
        .is_some_and(|e| e.input.value().is_empty());
    match key.code {
        KeyCode::Esc => app.discover.tag_entry = None,
        KeyCode::Enter => app.tag_entry_enter(),
        KeyCode::Down | KeyCode::Tab => app
            .discover
            .tag_entry
            .as_mut()
            .map(|e| e.move_by(1))
            .unwrap_or(()),
        KeyCode::Up | KeyCode::BackTab => app
            .discover
            .tag_entry
            .as_mut()
            .map(|e| e.move_by(-1))
            .unwrap_or(()),
        KeyCode::Backspace if empty => {
            app.discover.tag_entry = None;
            app.remove_last_tag();
        }
        _ => {
            if let Some(request) = to_input_request(&Event::Key(key)) {
                app.tag_entry_input(request);
            }
        }
    }
}

fn feed_key(app: &mut App, key: KeyEvent) {
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.feed_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.feed_move(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.feed_move(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.feed_move(-10),
        KeyCode::Char('g') | KeyCode::Home => app.feed_move(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.feed_move(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('l') => app.feed_open(Intent::Show),
        KeyCode::Char('p') => app.feed_open(Intent::Play { start_track: None }),
        KeyCode::Char('a') => app.feed_open(Intent::Enqueue),
        KeyCode::Char('o') => {
            let url = app.feed_selected_url();
            app.open_in_browser(url);
        }
        KeyCode::Char('w') => app.feed_next_tab(),
        KeyCode::Char('v') => app.feed.cycle_filter(),
        KeyCode::Char('R') => app.reload_feed(),
        _ => {}
    }
}

fn daily_key(app: &mut App, key: KeyEvent) {
    if app.daily.view.is_some() {
        daily_article_key(app, key);
        return;
    }
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.daily_move(1),
        KeyCode::Char('k') | KeyCode::Up => app.daily_move(-1),
        KeyCode::PageDown | KeyCode::Char('d') => app.daily_move(10),
        KeyCode::PageUp | KeyCode::Char('u') => app.daily_move(-10),
        KeyCode::Char('g') | KeyCode::Home => app.daily_move(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => app.daily_move(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('l') => app.daily_open_article(),
        KeyCode::Char('v') => app.daily_set_franchise(true),
        KeyCode::Char('V') => app.daily_set_franchise(false),
        KeyCode::Char('R') => app.reload_daily(),
        KeyCode::Char('o') => {
            let url = app.daily_selected_url();
            app.open_in_browser(url);
        }
        _ => {}
    }
}

fn daily_article_key(app: &mut App, key: KeyEvent) {
    use crate::app::daily::ArticleFocus;
    let focus = app.daily.view.as_ref().map(|v| v.focus).unwrap_or_default();
    match key.code {
        KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('h') => app.daily_close_article(),
        KeyCode::Char('w') => app.daily_toggle_focus(),
        KeyCode::Char('o') => {
            let url = app.daily_selected_url();
            app.open_in_browser(url);
        }
        KeyCode::Char('R') => app.daily_reload_article(),
        _ => match focus {
            ArticleFocus::Tracks => daily_track_key(app, key),
            ArticleFocus::Text => daily_text_key(app, key),
        },
    }
}

fn daily_track_key(app: &mut App, key: KeyEvent) {
    let Some(view) = app.daily.view.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => view.move_track(1),
        KeyCode::Char('k') | KeyCode::Up => view.move_track(-1),
        KeyCode::Char('g') | KeyCode::Home => view.move_track(isize::MIN / 2),
        KeyCode::Char('G') | KeyCode::End => view.move_track(isize::MAX / 2),
        KeyCode::Enter | KeyCode::Char('p') => app.daily_play_track(),
        KeyCode::Char('a') => app.daily_enqueue_track(),
        KeyCode::Char('l') => app.daily_open_album(),
        _ => {}
    }
}

fn daily_text_key(app: &mut App, key: KeyEvent) {
    let Some(view) = app.daily.view.as_mut() else {
        return;
    };
    match key.code {
        KeyCode::Char('j') | KeyCode::Down => view.scroll_by(1),
        KeyCode::Char('k') | KeyCode::Up => view.scroll_by(-1),
        KeyCode::PageDown | KeyCode::Char('d') => view.scroll_by(10),
        KeyCode::PageUp | KeyCode::Char('u') => view.scroll_by(-10),
        KeyCode::Char('g') | KeyCode::Home => view.scroll = 0,
        KeyCode::Char('G') | KeyCode::End => view.scroll_by(i32::from(u16::MAX)),
        _ => {}
    }
}
