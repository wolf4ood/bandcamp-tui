//! State of the Discover section: filters, a paged result list, and the two
//! overlays (option picker, tag entry).

use ratatui::widgets::TableState;
use tui_input::Input;

use crate::api::discover::{
    DiscoverOptions, DiscoverRequest, DiscoverResult, FIRST_CURSOR, Geoname, PAGE_SIZE,
    TagSuggestion,
};

/// The current Discover query, mirroring the website's filter bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filters {
    pub genre: Option<String>,
    pub subgenre: Option<String>,
    pub tags: Vec<String>,
    pub category_id: i64,
    pub slice: String,
    /// `None` is the website's "fresh".
    pub time_facet_id: Option<i64>,
    pub geoname_id: i64,
    pub location_label: String,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            genre: None,
            subgenre: None,
            tags: Vec::new(),
            category_id: 0,
            slice: "top".to_owned(),
            time_facet_id: None,
            geoname_id: 0,
            location_label: "from anywhere".to_owned(),
        }
    }
}

impl Filters {
    pub fn tag_names(&self) -> Vec<String> {
        self.genre
            .iter()
            .chain(self.subgenre.iter())
            .chain(self.tags.iter())
            .cloned()
            .collect()
    }

    pub fn request(&self, cursor: Option<&str>) -> DiscoverRequest {
        DiscoverRequest {
            tag_norm_names: self.tag_names(),
            geoname_id: self.geoname_id,
            slice: self.slice.clone(),
            time_facet_id: self.time_facet_id,
            category_id: self.category_id,
            size: PAGE_SIZE,
            cursor: cursor.unwrap_or(FIRST_CURSOR).to_owned(),
            include_result_types: vec!["a", "s"],
        }
    }

    pub fn genre_label<'a>(&'a self, options: &'a DiscoverOptions) -> &'a str {
        match &self.genre {
            Some(slug) => options
                .genres
                .iter()
                .find(|g| &g.slug == slug)
                .map_or(slug.as_str(), |g| g.label.as_str()),
            None => "all genres",
        }
    }

    pub fn subgenre_label<'a>(&'a self, options: &'a DiscoverOptions) -> &'a str {
        match &self.subgenre {
            Some(slug) => options
                .subgenres
                .iter()
                .find(|s| &s.slug == slug)
                .map_or(slug.as_str(), |s| s.label.as_str()),
            None => "any",
        }
    }

    pub fn slice_label<'a>(&'a self, options: &'a DiscoverOptions) -> &'a str {
        options
            .slices
            .iter()
            .find(|s| s.slug == self.slice)
            .map_or(self.slice.as_str(), |s| s.label.as_str())
    }

    pub fn time_label<'a>(&self, options: &'a DiscoverOptions) -> &'a str {
        match self.time_facet_id {
            Some(id) => DiscoverOptions::label_for(&options.times, id).unwrap_or("?"),
            None => "fresh",
        }
    }

    pub fn category_label<'a>(&self, options: &'a DiscoverOptions) -> &'a str {
        DiscoverOptions::label_for(&options.categories, self.category_id).unwrap_or("all")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Genre,
    Subgenre,
    Format,
    Sort,
    Time,
    Location,
}

impl Field {
    pub const ALL: [Field; 6] = [
        Field::Genre,
        Field::Subgenre,
        Field::Format,
        Field::Sort,
        Field::Time,
        Field::Location,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Field::Genre => "Genre",
            Field::Subgenre => "Subgenre",
            Field::Format => "Format",
            Field::Sort => "Sort",
            Field::Time => "Time",
            Field::Location => "Location",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PickerOption {
    pub label: String,
    /// Field-specific: a slug, an id, or `""` for "all / any / fresh".
    pub key: String,
}

/// A list menu: first the field to change, then that field's options.
pub struct Picker {
    pub field: Option<Field>,
    pub options: Vec<PickerOption>,
    pub filter: Input,
    pub selected: usize,
    /// Location searches in flight are keyed by their query text.
    pub pending_query: Option<String>,
}

impl Picker {
    pub fn new(
        field: Option<Field>,
        options: Vec<PickerOption>,
        current_key: Option<&str>,
    ) -> Self {
        let selected = current_key
            .and_then(|key| options.iter().position(|o| o.key == key))
            .unwrap_or(0);
        Self {
            field,
            options,
            filter: Input::default(),
            selected,
            pending_query: None,
        }
    }

    /// Indices of options matching the filter text.
    pub fn visible(&self) -> Vec<usize> {
        let needle = self.filter.value().trim().to_lowercase();
        self.options
            .iter()
            .enumerate()
            .filter(|(_, o)| needle.is_empty() || o.label.to_lowercase().contains(&needle))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn selected_option(&self) -> Option<&PickerOption> {
        let visible = self.visible();
        visible.get(self.selected).map(|i| &self.options[*i])
    }

    pub fn move_by(&mut self, delta: isize) {
        let len = self.visible().len();
        if len == 0 {
            self.selected = 0;
            return;
        }
        self.selected = (self.selected as isize + delta).clamp(0, len as isize - 1) as usize;
    }

    pub fn clamp(&mut self) {
        let len = self.visible().len();
        self.selected = self.selected.min(len.saturating_sub(1));
    }

    /// Merge location search results in, keeping the presets on top.
    pub fn add_geonames(&mut self, geonames: Vec<Geoname>) {
        for geoname in geonames {
            if !self.options.iter().any(|o| o.key == geoname.id) {
                self.options.push(PickerOption {
                    label: geoname.fullname,
                    key: geoname.id,
                });
            }
        }
        self.clamp();
    }
}

/// Free-text tag entry with autocomplete.
#[derive(Default)]
pub struct TagEntry {
    pub input: Input,
    pub suggestions: Vec<TagSuggestion>,
    pub selected: Option<usize>,
    /// Prefix the current suggestions were fetched for.
    pub suggestions_for: String,
}

impl TagEntry {
    pub fn move_by(&mut self, delta: isize) {
        if self.suggestions.is_empty() {
            self.selected = None;
            return;
        }
        let current = self.selected.map_or(-1, |s| s as isize);
        let next = (current + delta).clamp(-1, self.suggestions.len() as isize - 1);
        self.selected = (next >= 0).then_some(next as usize);
    }

    /// The tag to add on Enter: the highlighted suggestion, else the typed text.
    pub fn chosen(&self) -> Option<String> {
        if let Some(i) = self.selected
            && let Some(s) = self.suggestions.get(i)
        {
            return Some(s.norm_name.clone());
        }
        let typed = normalize_tag(self.input.value());
        (!typed.is_empty()).then_some(typed)
    }
}

/// Bandcamp tag slugs: lowercase, words joined by dashes.
pub fn normalize_tag(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for ch in text.trim().to_lowercase().chars() {
        if ch.is_alphanumeric() {
            out.push(ch);
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

pub struct Discover {
    pub options: DiscoverOptions,
    pub options_live: bool,
    pub filters: Filters,
    pub results: Vec<DiscoverResult>,
    pub table: TableState,
    pub next_cursor: Option<String>,
    pub more: bool,
    pub loading: bool,
    pub started: bool,
    pub error: Option<String>,
    pub total: Option<u64>,
    /// Bumped whenever the filters change so stale pages are dropped.
    pub request_id: u64,
    pub related: Vec<String>,
    pub picker: Option<Picker>,
    pub tag_entry: Option<TagEntry>,
}

impl Discover {
    pub fn new() -> Self {
        Self {
            options: DiscoverOptions::embedded(),
            options_live: false,
            filters: Filters::default(),
            results: Vec::new(),
            table: TableState::default(),
            next_cursor: None,
            more: true,
            loading: false,
            started: false,
            error: None,
            total: None,
            request_id: 0,
            related: Vec::new(),
            picker: None,
            tag_entry: None,
        }
    }

    /// Forget the results (the filters changed).
    pub fn reset_results(&mut self) {
        self.results.clear();
        self.table = TableState::default();
        self.next_cursor = None;
        self.more = true;
        self.loading = false;
        self.error = None;
        self.total = None;
        self.related.clear();
        self.request_id += 1;
    }

    pub fn selected(&self) -> Option<&DiscoverResult> {
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

    pub fn wants_more(&self) -> bool {
        self.more
            && !self.loading
            && self.started
            && self
                .table
                .selected()
                .is_none_or(|i| i + 8 >= self.results.len())
    }

    /// Options for the picker of `field`, with `""` meaning "all / any / fresh".
    pub fn picker_options(&self, field: Option<Field>) -> Vec<PickerOption> {
        let o = &self.options;
        let f = &self.filters;
        match field {
            None => Field::ALL
                .iter()
                .map(|field| {
                    let value = match field {
                        Field::Genre => f.genre_label(o).to_owned(),
                        Field::Subgenre => f.subgenre_label(o).to_owned(),
                        Field::Format => f.category_label(o).to_owned(),
                        Field::Sort => f.slice_label(o).to_owned(),
                        Field::Time => f.time_label(o).to_owned(),
                        Field::Location => f.location_label.clone(),
                    };
                    PickerOption {
                        label: format!("{:<9} {value}", field.label()),
                        key: field.label().to_owned(),
                    }
                })
                .collect(),
            Some(Field::Genre) => std::iter::once(PickerOption {
                label: "all genres".into(),
                key: String::new(),
            })
            .chain(o.genres.iter().map(|g| PickerOption {
                label: g.label.clone(),
                key: g.slug.clone(),
            }))
            .collect(),
            Some(Field::Subgenre) => {
                let mut list = vec![PickerOption {
                    label: "any".into(),
                    key: String::new(),
                }];
                match &f.genre {
                    Some(genre) => list.extend(o.subgenres_of(genre).map(|s| PickerOption {
                        label: s.label.clone(),
                        key: s.slug.clone(),
                    })),
                    None => list.extend(o.subgenres.iter().map(|s| PickerOption {
                        label: format!("{} ({})", s.label, s.parent_slug),
                        key: s.slug.clone(),
                    })),
                }
                list
            }
            Some(Field::Format) => o
                .categories
                .iter()
                .map(|c| PickerOption {
                    label: c.label.clone(),
                    key: c.id.to_string(),
                })
                .collect(),
            Some(Field::Sort) => o
                .slices
                .iter()
                .map(|s| PickerOption {
                    label: s.label.clone(),
                    key: s.slug.clone(),
                })
                .collect(),
            Some(Field::Time) => o
                .times
                .iter()
                .map(|t| PickerOption {
                    label: t.label.clone(),
                    key: if t.slug == "fresh" {
                        String::new()
                    } else {
                        t.id.to_string()
                    },
                })
                .collect(),
            Some(Field::Location) => o
                .locations
                .iter()
                .map(|l| PickerOption {
                    label: l.label.clone(),
                    key: l.id.to_string(),
                })
                .collect(),
        }
    }

    pub fn current_key(&self, field: Field) -> String {
        let f = &self.filters;
        match field {
            Field::Genre => f.genre.clone().unwrap_or_default(),
            Field::Subgenre => f.subgenre.clone().unwrap_or_default(),
            Field::Format => f.category_id.to_string(),
            Field::Sort => f.slice.clone(),
            Field::Time => f.time_facet_id.map(|id| id.to_string()).unwrap_or_default(),
            Field::Location => f.geoname_id.to_string(),
        }
    }

    /// Apply a picked option; returns true when the filters changed.
    pub fn apply(&mut self, field: Field, option: &PickerOption) -> bool {
        let before = self.filters.clone();
        let key = option.key.as_str();
        match field {
            Field::Genre => {
                self.filters.genre = (!key.is_empty()).then(|| key.to_owned());
                // A subgenre belongs to a genre; drop it when the genre changes.
                if self.filters.genre != before.genre {
                    self.filters.subgenre = None;
                }
            }
            Field::Subgenre => {
                self.filters.subgenre = (!key.is_empty()).then(|| key.to_owned());
                if let Some(parent) = self
                    .options
                    .subgenres
                    .iter()
                    .find(|s| s.slug == key)
                    .map(|s| s.parent_slug.clone())
                    && self.filters.genre.is_none()
                {
                    self.filters.genre = Some(parent);
                }
            }
            Field::Format => self.filters.category_id = key.parse().unwrap_or(0),
            Field::Sort => self.filters.slice = key.to_owned(),
            Field::Time => self.filters.time_facet_id = key.parse().ok(),
            Field::Location => {
                self.filters.geoname_id = key.parse().unwrap_or(0);
                self.filters.location_label = option.label.clone();
            }
        }
        self.filters != before
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_tags_like_bandcamp() {
        assert_eq!(normalize_tag("  Dark Ambient "), "dark-ambient");
        assert_eq!(normalize_tag("lo-fi / chill"), "lo-fi-chill");
        assert_eq!(normalize_tag("!!!"), "");
    }

    #[test]
    fn picking_options_updates_filters() {
        let mut d = Discover::new();
        let genres = d.picker_options(Some(Field::Genre));
        let electronic = genres
            .iter()
            .find(|o| o.key == "electronic")
            .unwrap()
            .clone();
        assert!(d.apply(Field::Genre, &electronic));
        assert!(!d.apply(Field::Genre, &electronic));
        let subs = d.picker_options(Some(Field::Subgenre));
        assert!(subs.len() > 5 && subs[0].key.is_empty());
        assert!(d.apply(Field::Subgenre, &subs[1]));
        assert_eq!(d.filters.tag_names().len(), 2);
        // Changing the genre drops the subgenre.
        let rock = genres.iter().find(|o| o.key == "rock").unwrap().clone();
        assert!(d.apply(Field::Genre, &rock));
        assert_eq!(d.filters.subgenre, None);
        let fresh = PickerOption {
            label: "fresh".into(),
            key: String::new(),
        };
        d.filters.time_facet_id = Some(1);
        assert!(d.apply(Field::Time, &fresh));
        assert_eq!(d.filters.time_facet_id, None);
        let req = d.filters.request(None);
        assert_eq!(req.cursor, "*");
        assert_eq!(req.tag_norm_names, vec!["rock".to_owned()]);
    }

    #[test]
    fn picker_filters_and_clamps() {
        let d = Discover::new();
        let mut picker = Picker::new(
            Some(Field::Location),
            d.picker_options(Some(Field::Location)),
            Some("2950159"),
        );
        assert_eq!(picker.selected_option().unwrap().label, "berlin");
        picker.filter = Input::new("lon".into());
        picker.clamp();
        assert_eq!(picker.visible().len(), 1);
        assert_eq!(picker.selected_option().unwrap().label, "london");
        picker.add_geonames(vec![Geoname {
            id: "1".into(),
            name: "Londrina".into(),
            fullname: "Londrina, Brazil".into(),
        }]);
        assert_eq!(picker.visible().len(), 2);
    }

    #[test]
    fn tag_entry_prefers_highlighted_suggestion() {
        let mut entry = TagEntry {
            input: Input::new("Dark Amb".into()),
            ..Default::default()
        };
        assert_eq!(entry.chosen().as_deref(), Some("dark-amb"));
        entry.suggestions = vec![TagSuggestion {
            norm_name: "dark-ambient".into(),
            display_name: "dark ambient".into(),
            count: 1,
        }];
        entry.move_by(1);
        assert_eq!(entry.chosen().as_deref(), Some("dark-ambient"));
        entry.move_by(-1);
        assert_eq!(entry.selected, None);
    }
}
