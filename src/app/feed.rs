//! State of the Feed section: the story stream plus the following lists.

use ratatui::widgets::TableState;

use crate::api::feed::{Story, StoryKind};
use crate::api::social::{FollowList, Followee};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedTab {
    Stories,
    Bands,
    Fans,
    Followers,
}

impl FeedTab {
    pub const ALL: [FeedTab; 4] = [
        FeedTab::Stories,
        FeedTab::Bands,
        FeedTab::Fans,
        FeedTab::Followers,
    ];

    pub fn title(self) -> &'static str {
        match self {
            FeedTab::Stories => "Feed",
            FeedTab::Bands => "Artists",
            FeedTab::Fans => "Fans",
            FeedTab::Followers => "Followers",
        }
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn follow_list(self) -> Option<FollowList> {
        match self {
            FeedTab::Stories => None,
            FeedTab::Bands => Some(FollowList::Bands),
            FeedTab::Fans => Some(FollowList::Fans),
            FeedTab::Followers => Some(FollowList::Followers),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoryFilter {
    #[default]
    All,
    Releases,
    Purchases,
}

impl StoryFilter {
    pub fn next(self) -> Self {
        match self {
            StoryFilter::All => StoryFilter::Releases,
            StoryFilter::Releases => StoryFilter::Purchases,
            StoryFilter::Purchases => StoryFilter::All,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            StoryFilter::All => "everything",
            StoryFilter::Releases => "new releases",
            StoryFilter::Purchases => "fan purchases",
        }
    }

    pub fn accepts(self, story: &Story) -> bool {
        match self {
            StoryFilter::All => true,
            StoryFilter::Releases => story.kind == StoryKind::NewRelease,
            StoryFilter::Purchases => story.kind == StoryKind::FanPurchase,
        }
    }
}

/// A list that grows page by page with a continuation token.
pub struct Paged<T> {
    pub items: Vec<T>,
    pub table: TableState,
    pub next_token: Option<String>,
    pub more: bool,
    pub loading: bool,
    pub started: bool,
    pub error: Option<String>,
}

impl<T> Default for Paged<T> {
    fn default() -> Self {
        Self {
            items: Vec::new(),
            table: TableState::default(),
            next_token: None,
            more: true,
            loading: false,
            started: false,
            error: None,
        }
    }
}

impl<T> Paged<T> {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn absorb(&mut self, items: Vec<T>, more: bool, next_token: Option<String>) {
        let token_moved = next_token.is_some() && next_token != self.next_token;
        self.more = more && !items.is_empty() && token_moved;
        self.next_token = next_token;
        self.items.extend(items);
        if self.table.selected().is_none() && !self.items.is_empty() {
            self.table.select(Some(0));
        }
    }

    /// Move the selection within `len` visible rows.
    pub fn move_within(&mut self, len: usize, delta: isize) {
        if len == 0 {
            return;
        }
        let current = self.table.selected().unwrap_or(0) as isize;
        let next = current.saturating_add(delta).clamp(0, len as isize - 1) as usize;
        self.table.select(Some(next));
    }

    pub fn wants_more(&self, visible_len: usize) -> bool {
        self.more
            && !self.loading
            && self.started
            && self.table.selected().is_none_or(|i| i + 8 >= visible_len)
    }
}

pub struct Feed {
    pub tab: FeedTab,
    pub stories: Paged<Story>,
    pub filter: StoryFilter,
    pub bands: Paged<Followee>,
    pub fans: Paged<Followee>,
    pub followers: Paged<Followee>,
}

impl Feed {
    pub fn new() -> Self {
        Self {
            tab: FeedTab::Stories,
            stories: Paged::default(),
            filter: StoryFilter::default(),
            bands: Paged::default(),
            fans: Paged::default(),
            followers: Paged::default(),
        }
    }

    pub fn follow_page(&self, list: FollowList) -> &Paged<Followee> {
        match list {
            FollowList::Bands => &self.bands,
            FollowList::Fans => &self.fans,
            FollowList::Followers => &self.followers,
        }
    }

    pub fn follow_page_mut(&mut self, list: FollowList) -> &mut Paged<Followee> {
        match list {
            FollowList::Bands => &mut self.bands,
            FollowList::Fans => &mut self.fans,
            FollowList::Followers => &mut self.followers,
        }
    }

    /// Indices into `stories.items` that pass the filter.
    pub fn visible_stories(&self) -> Vec<usize> {
        self.stories
            .items
            .iter()
            .enumerate()
            .filter(|(_, s)| self.filter.accepts(s))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_story(&self) -> Option<&Story> {
        let visible = self.visible_stories();
        let i = self.stories.table.selected()?;
        visible.get(i).map(|idx| &self.stories.items[*idx])
    }

    pub fn selected_followee(&self) -> Option<&Followee> {
        let list = self.tab.follow_list()?;
        let page = self.follow_page(list);
        page.table.selected().and_then(|i| page.items.get(i))
    }

    pub fn cycle_filter(&mut self) {
        self.filter = self.filter.next();
        let len = self.visible_stories().len();
        let selected = self.stories.table.selected().unwrap_or(0);
        self.stories
            .table
            .select((len > 0).then_some(selected.min(len - 1)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paged_absorbs_and_stops_when_the_token_stalls() {
        let mut page: Paged<u32> = Paged {
            started: true,
            ..Default::default()
        };
        page.absorb(vec![1, 2, 3], true, Some("t1".into()));
        assert!(page.more);
        assert_eq!(page.table.selected(), Some(0));
        assert!(page.wants_more(3));
        page.absorb(vec![4], true, Some("t1".into()));
        assert!(!page.more, "same token again means no progress");
        page.move_within(4, 10);
        assert_eq!(page.table.selected(), Some(3));
    }

    #[test]
    fn story_filter_and_selection_agree() {
        use crate::api::feed::Story;
        let story = |kind| Story {
            kind,
            story_type: String::new(),
            date: None,
            band_id: Some(1),
            band_name: "b".into(),
            item: None,
            item_title: "t".into(),
            item_url: None,
            fan_name: None,
            fan_id: None,
            why: None,
            also_collected_count: None,
        };
        let mut feed = Feed::new();
        feed.stories.started = true;
        feed.stories.absorb(
            vec![
                story(StoryKind::NewRelease),
                story(StoryKind::FanPurchase),
                story(StoryKind::NewRelease),
            ],
            false,
            None,
        );
        feed.stories.table.select(Some(2));
        assert_eq!(feed.selected_story().unwrap().kind, StoryKind::NewRelease);
        feed.cycle_filter(); // releases only: 2 visible, selection clamped to 1
        assert_eq!(feed.visible_stories(), vec![0, 2]);
        assert_eq!(feed.stories.table.selected(), Some(1));
        feed.cycle_filter(); // purchases only
        assert_eq!(feed.visible_stories(), vec![1]);
        assert_eq!(feed.selected_story().unwrap().kind, StoryKind::FanPurchase);
    }
}
