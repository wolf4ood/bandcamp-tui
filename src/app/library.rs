//! State of the Collection section: two paginated lists plus in-collection search.

use ratatui::widgets::TableState;
use tui_input::Input;

use crate::api::fan::ListKind;
use crate::api::models::{CollectionItem, CollectionPage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortKey {
    /// Server order: most recently purchased / added first.
    #[default]
    Recent,
    Artist,
    Title,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            SortKey::Recent => SortKey::Artist,
            SortKey::Artist => SortKey::Title,
            SortKey::Title => SortKey::Recent,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SortKey::Recent => "recent",
            SortKey::Artist => "artist",
            SortKey::Title => "title",
        }
    }
}

pub struct SearchResults {
    pub key: String,
    pub items: Vec<CollectionItem>,
    pub table: TableState,
    pub loading: bool,
}

pub struct ItemList {
    pub kind: ListKind,
    pub items: Vec<CollectionItem>,
    pub table: TableState,
    pub next_token: Option<String>,
    pub more: bool,
    pub loading: bool,
    /// At least one page was requested.
    pub started: bool,
    pub error: Option<String>,
    pub sort: SortKey,
    pub search: Option<SearchResults>,
}

impl ItemList {
    pub fn new(kind: ListKind) -> Self {
        Self {
            kind,
            items: Vec::new(),
            table: TableState::default(),
            next_token: None,
            more: true,
            loading: false,
            started: false,
            error: None,
            sort: SortKey::default(),
            search: None,
        }
    }

    pub fn reset(&mut self) {
        let sort = self.sort;
        *self = Self::new(self.kind);
        self.sort = sort;
    }

    /// What the table shows: search results when a search is active.
    pub fn visible(&self) -> &[CollectionItem] {
        match &self.search {
            Some(search) => &search.items,
            None => &self.items,
        }
    }

    pub fn table_state(&mut self) -> &mut TableState {
        match &mut self.search {
            Some(search) => &mut search.table,
            None => &mut self.table,
        }
    }

    pub fn selected(&self) -> Option<usize> {
        let state = match &self.search {
            Some(search) => &search.table,
            None => &self.table,
        };
        state.selected().filter(|i| *i < self.visible().len())
    }

    pub fn selected_item(&self) -> Option<&CollectionItem> {
        self.selected().map(|i| &self.visible()[i])
    }

    pub fn select(&mut self, index: usize) {
        let len = self.visible().len();
        let index = if len == 0 {
            None
        } else {
            Some(index.min(len - 1))
        };
        self.table_state().select(index);
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            return;
        }
        let current = self.selected().unwrap_or(0) as isize;
        let next = (current + delta).clamp(0, len as isize - 1) as usize;
        self.select(next);
    }

    pub fn select_last(&mut self) {
        let len = self.visible().len();
        self.select(len.saturating_sub(1));
    }

    /// True when the selection is close enough to the end to fetch the next page.
    pub fn wants_more(&self) -> bool {
        self.search.is_none()
            && self.more
            && !self.loading
            && self.started
            && self.selected().is_none_or(|i| i + 8 >= self.items.len())
    }

    pub fn absorb_page(&mut self, page: CollectionPage) {
        let selected_id = self.selected_item().map(|i| i.item_id);
        self.items.extend(page.items);
        self.more = page.more_available && page.last_token.is_some();
        self.next_token = page.last_token;
        self.apply_sort();
        if let Some(id) = selected_id
            && let Some(index) = self.items.iter().position(|i| i.item_id == id)
        {
            self.table.select(Some(index));
        } else if self.table.selected().is_none() && !self.items.is_empty() {
            self.table.select(Some(0));
        }
    }

    pub fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        let selected_id = self.selected_item().map(|i| i.item_id);
        self.apply_sort();
        if let Some(id) = selected_id
            && let Some(index) = self.visible().iter().position(|i| i.item_id == id)
        {
            self.select(index);
        }
    }

    fn apply_sort(&mut self) {
        let sort = self.sort;
        let by = |items: &mut Vec<CollectionItem>| match sort {
            SortKey::Recent => items.sort_by_key(|a| std::cmp::Reverse(a.date())),
            SortKey::Artist => items.sort_by(|a, b| {
                a.band_name
                    .to_lowercase()
                    .cmp(&b.band_name.to_lowercase())
                    .then_with(|| {
                        a.item_title
                            .to_lowercase()
                            .cmp(&b.item_title.to_lowercase())
                    })
            }),
            SortKey::Title => items.sort_by_key(|a| a.item_title.to_lowercase()),
        };
        by(&mut self.items);
        if let Some(search) = &mut self.search {
            by(&mut search.items);
        }
    }
}

pub struct Library {
    pub tab: ListKind,
    pub collection: ItemList,
    pub wishlist: ItemList,
    pub search_input: Input,
    pub search_open: bool,
}

impl Library {
    pub fn new() -> Self {
        Self {
            tab: ListKind::Collection,
            collection: ItemList::new(ListKind::Collection),
            wishlist: ItemList::new(ListKind::Wishlist),
            search_input: Input::default(),
            search_open: false,
        }
    }

    pub fn list(&self) -> &ItemList {
        self.list_for(self.tab)
    }

    pub fn list_mut(&mut self) -> &mut ItemList {
        self.list_for_mut(self.tab)
    }

    pub fn list_for(&self, kind: ListKind) -> &ItemList {
        match kind {
            ListKind::Collection => &self.collection,
            ListKind::Wishlist => &self.wishlist,
        }
    }

    pub fn list_for_mut(&mut self, kind: ListKind) -> &mut ItemList {
        match kind {
            ListKind::Collection => &mut self.collection,
            ListKind::Wishlist => &mut self.wishlist,
        }
    }

    pub fn toggle_tab(&mut self) {
        self.tab = match self.tab {
            ListKind::Collection => ListKind::Wishlist,
            ListKind::Wishlist => ListKind::Collection,
        };
        self.search_open = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, band: &str, title: &str, purchased: &str) -> CollectionItem {
        CollectionItem {
            item_id: id,
            tralbum_id: id,
            tralbum_type: "a".into(),
            band_name: band.into(),
            item_title: title.into(),
            purchased: Some(purchased.into()),
            ..Default::default()
        }
    }

    fn page(items: Vec<CollectionItem>, more: bool) -> CollectionPage {
        CollectionPage {
            items,
            more_available: more,
            last_token: more.then(|| "tok".to_owned()),
            ..Default::default()
        }
    }

    #[test]
    fn pages_accumulate_and_sort_keeps_selection() {
        let mut list = ItemList::new(ListKind::Collection);
        list.started = true;
        list.absorb_page(page(
            vec![
                item(1, "Zed", "B", "02 Jan 2026 00:00:00 GMT"),
                item(2, "Alpha", "A", "01 Jan 2026 00:00:00 GMT"),
            ],
            true,
        ));
        assert_eq!(list.selected(), Some(0));
        assert!(list.wants_more());
        list.select(1);
        list.cycle_sort(); // artist
        assert_eq!(list.sort, SortKey::Artist);
        assert_eq!(list.selected_item().unwrap().item_id, 2);
        assert_eq!(list.items[0].band_name, "Alpha");
        list.absorb_page(page(
            vec![item(3, "Mid", "C", "03 Jan 2026 00:00:00 GMT")],
            false,
        ));
        assert!(!list.more);
        assert!(!list.wants_more());
        assert_eq!(list.selected_item().unwrap().item_id, 2);
        list.move_by(10);
        assert_eq!(list.selected(), Some(2));
        list.move_by(-10);
        assert_eq!(list.selected(), Some(0));
    }
}
