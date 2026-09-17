//! State of the Search section.

use ratatui::widgets::TableState;
use tui_input::Input;

use crate::api::search::{SearchFilter, SearchResult};

pub struct Search {
    pub input: Input,
    pub input_open: bool,
    pub filter: SearchFilter,
    /// The query the current results belong to.
    pub query: String,
    pub results: Vec<SearchResult>,
    pub tags: Vec<String>,
    pub table: TableState,
    pub loading: bool,
    pub error: Option<String>,
    /// Bumped per request so late replies are dropped.
    pub request_id: u64,
}

impl Search {
    pub fn new() -> Self {
        Self {
            input: Input::default(),
            input_open: true,
            filter: SearchFilter::default(),
            query: String::new(),
            results: Vec::new(),
            tags: Vec::new(),
            table: TableState::default(),
            loading: false,
            error: None,
            request_id: 0,
        }
    }

    pub fn selected(&self) -> Option<&SearchResult> {
        self.table.selected().and_then(|i| self.results.get(i))
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.results.len();
        if len == 0 {
            return;
        }
        let current = self.table.selected().unwrap_or(0) as isize;
        let next = current.saturating_add(delta).clamp(0, len as isize - 1) as usize;
        self.table.select(Some(next));
    }

    pub fn set_results(&mut self, results: Vec<SearchResult>, tags: Vec<String>) {
        self.results = results;
        self.tags = tags;
        self.table.select((!self.results.is_empty()).then_some(0));
    }
}
