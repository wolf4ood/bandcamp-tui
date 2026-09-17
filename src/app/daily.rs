//! State of the Daily section: the article list and the open article.

use ratatui::widgets::TableState;

use super::feed::Paged;
use crate::api::daily::{Article, ArticleDetail, FeaturedTrack, Franchise};
use crate::player::QueuedTrack;

/// Which pane of an open article takes the keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArticleFocus {
    #[default]
    Tracks,
    Text,
}

impl ArticleFocus {
    pub fn toggle(self) -> Self {
        match self {
            ArticleFocus::Tracks => ArticleFocus::Text,
            ArticleFocus::Text => ArticleFocus::Tracks,
        }
    }
}

pub struct ArticleView {
    pub article: Article,
    pub detail: Option<ArticleDetail>,
    pub loading: bool,
    pub error: Option<String>,
    pub focus: ArticleFocus,
    pub tracks: TableState,
    /// Reader scroll offset in lines.
    pub scroll: u16,
}

impl ArticleView {
    pub fn new(article: Article) -> Self {
        Self {
            article,
            detail: None,
            loading: true,
            error: None,
            focus: ArticleFocus::Tracks,
            tracks: TableState::default(),
            scroll: 0,
        }
    }

    pub fn featured(&self) -> &[FeaturedTrack] {
        self.detail.as_ref().map_or(&[], |d| d.featured.as_slice())
    }

    pub fn selected_track(&self) -> Option<&FeaturedTrack> {
        self.tracks.selected().and_then(|i| self.featured().get(i))
    }

    pub fn move_track(&mut self, delta: isize) {
        let len = self.featured().len();
        if len == 0 {
            return;
        }
        let current = self.tracks.selected().unwrap_or(0) as isize;
        let next = current.saturating_add(delta).clamp(0, len as isize - 1) as usize;
        self.tracks.select(Some(next));
    }

    pub fn scroll_by(&mut self, delta: i32) {
        self.scroll = (i32::from(self.scroll) + delta).clamp(0, i32::from(u16::MAX)) as u16;
    }
}

/// A featured track as a queue entry. The signed URL expires after about a day;
/// when it fails, the player's usual retry refetches the album's URLs by `tralbum`.
pub fn queued_track(track: &FeaturedTrack) -> QueuedTrack {
    QueuedTrack {
        track_id: track.track_id,
        title: track.title.clone(),
        artist: track.artist.clone(),
        album: Some(track.album.clone()).filter(|s| !s.is_empty()),
        duration: track.duration,
        url: track.stream_url.clone(),
        tralbum: track.tralbum,
        art_id: track.art_id,
        page_url: Some(track.url.clone()).filter(|s| !s.is_empty()),
        retried: false,
    }
}

pub struct Daily {
    pub franchise: Franchise,
    /// `next_token` holds the next page number.
    pub articles: Paged<Article>,
    pub view: Option<ArticleView>,
}

impl Daily {
    pub fn new() -> Self {
        Self {
            franchise: Franchise::default(),
            articles: Paged::default(),
            view: None,
        }
    }

    pub fn selected_article(&self) -> Option<&Article> {
        self.articles
            .table
            .selected()
            .and_then(|i| self.articles.items.get(i))
    }

    /// The next page to fetch: the stored continuation or the first page.
    pub fn next_page(&self) -> u32 {
        self.articles
            .next_token
            .as_deref()
            .and_then(|t| t.parse().ok())
            .unwrap_or(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn article(n: u32) -> Article {
        Article {
            url: format!("https://daily.bandcamp.com/lists/{n}"),
            title: format!("Article {n}"),
            franchise: "Lists".into(),
            date: None,
            art_url: None,
        }
    }

    #[test]
    fn page_numbers_ride_in_the_token() {
        let mut daily = Daily::new();
        assert_eq!(daily.next_page(), 1);
        daily.articles.started = true;
        daily
            .articles
            .absorb(vec![article(1)], true, Some("2".into()));
        assert_eq!(daily.next_page(), 2);
        assert_eq!(
            daily.selected_article().map(|a| a.title.as_str()),
            Some("Article 1")
        );
        daily.articles.absorb(vec![article(2)], false, None);
        assert!(!daily.articles.more);
    }

    #[test]
    fn article_view_moves_within_featured_tracks() {
        let mut view = ArticleView::new(article(1));
        view.move_track(1);
        assert_eq!(
            view.tracks.selected(),
            None,
            "nothing to select while loading"
        );
        view.scroll_by(-5);
        assert_eq!(view.scroll, 0);
        view.scroll_by(3);
        assert_eq!(view.scroll, 3);
    }
}
