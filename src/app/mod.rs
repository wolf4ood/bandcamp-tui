//! Application state and the message loop.
//!
//! The UI thread owns `App`; network, keyring and audio work run elsewhere and
//! report back through `Message`s.

pub mod daily;
pub mod discover;
pub mod event;
pub mod feed;
pub mod keys;
pub mod library;
pub mod search;

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{Event, KeyEventKind};
use ratatui::widgets::TableState;
use tokio::sync::mpsc::{self, UnboundedSender};
use tui_input::{Input, InputRequest};

use crate::api::auth::{self, Session};
use crate::api::band::{self as band_api, BandDetails};
use crate::api::daily::{self as daily_api, ArticleDetail, ArticlePage, FeaturedTrack};
use crate::api::discover as discover_api;
use crate::api::discover::{DiscoverOptions, DiscoverResponse, Geoname, RelatedTag, TagSuggestion};
use crate::api::fan::{self, ListKind};
use crate::api::feed::{self as feed_api, FeedPage};
use crate::api::models::{
    CollectionItem, CollectionPage, SearchItemsResponse, Tralbum, TralbumKind, TralbumRef,
};
use crate::api::search::{self as search_api, SearchResponse};
use crate::api::social::{self, FollowList, FollowPage, FollowTarget};
use crate::api::{ApiError, Client, tralbum};
use crate::config::Config;
#[cfg(target_os = "linux")]
use crate::mpris::{self, MprisCommand, MprisHandle};
use crate::player::queue::{self, Queue};
use crate::player::store::{QueueStore, SavedQueue};
use crate::player::{PlayerCommand, PlayerEvent, PlayerHandle, QueuedTrack};
use crate::secrets::CookieStore;
use crate::ui;
use daily::{ArticleFocus, ArticleView, Daily, queued_track};
use discover::{Discover, Field, Picker, TagEntry};
use feed::Feed;
use library::{ItemList, Library, SearchResults};
use search::Search;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Collection,
    Discover,
    Feed,
    Search,
    Daily,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Collection,
        Section::Discover,
        Section::Feed,
        Section::Search,
        Section::Daily,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Section::Collection => "Collection",
            Section::Discover => "Discover",
            Section::Feed => "Feed",
            Section::Search => "Search",
            Section::Daily => "Daily",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }

    pub fn next(self) -> Self {
        Self::ALL[(self.index() + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        Self::ALL[(self.index() + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Reading the stored cookie and validating it.
    Booting,
    /// The cookie-paste page. Reached with `L` from the main screen (or on
    /// startup); `Esc` goes back. Everything public works without it.
    Login,
    Main,
}

#[derive(Default)]
pub struct LoginState {
    pub input: Input,
    pub error: Option<String>,
    pub hint: Option<String>,
    pub busy: bool,
}

/// Why a session check was started; decides what a failure means.
pub enum SessionCheck {
    /// The stored cookie at startup.
    Boot(String),
    /// A cookie the user just pasted; persist it on success.
    Login(String),
    /// `r` on the main screen; the cookie is already in the jar.
    Refresh,
}

/// What to do with album details once they arrive.
#[derive(Debug, Clone, Copy)]
pub enum Intent {
    Show,
    Play {
        start_track: Option<u64>,
    },
    Enqueue,
    /// Stream URLs are stale: refresh them and replay this queue entry.
    Refresh {
        queue_index: usize,
        /// The entry already failed once; a second failure is final.
        after_failure: bool,
    },
}

/// Where an album view came from and what to show before details arrive.
#[derive(Debug, Clone)]
pub struct AlbumSource {
    pub tralbum: TralbumRef,
    pub title: String,
    pub url: String,
    pub section: Section,
}

pub struct AlbumView {
    pub source: AlbumSource,
    pub tralbum: Option<Tralbum>,
    pub loading: bool,
    pub error: Option<String>,
    pub table: TableState,
}

/// An artist or label page.
pub struct ArtistView {
    pub band_id: u64,
    pub name: String,
    pub url: Option<String>,
    pub section: Section,
    pub details: Option<BandDetails>,
    pub loading: bool,
    pub error: Option<String>,
    /// Discography (false) or label roster (true).
    pub roster: bool,
    pub table: TableState,
    pub roster_table: TableState,
}

/// Another fan's public collection.
pub struct FanView {
    pub fan_id: u64,
    pub name: String,
    pub url: Option<String>,
    pub section: Section,
    pub list: ItemList,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlaybackState {
    #[default]
    Idle,
    Loading,
    Playing,
    Paused,
}

#[derive(Default)]
pub struct Playback {
    pub state: PlaybackState,
    pub position: Duration,
    pub track_id: Option<u64>,
    /// Set when no audio device could be opened.
    pub unavailable: Option<String>,
}

#[allow(clippy::large_enum_variant)] // transient, a handful in flight at most
pub enum Message {
    Terminal(Event),
    Tick,
    CookieLoaded(Result<Option<String>, String>),
    SessionChecked {
        check: SessionCheck,
        result: Result<Session, ApiError>,
    },
    CookieSaved(Result<(), String>),
    CookieCleared(Result<(), String>),
    PageLoaded {
        kind: ListKind,
        generation: u64,
        result: Result<CollectionPage, ApiError>,
    },
    SearchLoaded {
        kind: ListKind,
        key: String,
        generation: u64,
        result: Result<SearchItemsResponse, ApiError>,
    },
    TralbumLoaded {
        tralbum: TralbumRef,
        intent: Intent,
        generation: u64,
        result: Result<Tralbum, ApiError>,
    },
    Player(PlayerEvent),
    /// A remote control call from the D-Bus session bus.
    #[cfg(target_os = "linux")]
    Mpris(MprisCommand),
    DiscoverOptionsLoaded(Result<DiscoverOptions, ApiError>),
    DiscoverLoaded {
        request_id: u64,
        result: Result<DiscoverResponse, ApiError>,
    },
    TagSuggestionsLoaded {
        prefix: String,
        result: Result<Vec<TagSuggestion>, ApiError>,
    },
    RelatedTagsLoaded {
        key: String,
        result: Result<Vec<RelatedTag>, ApiError>,
    },
    GeonamesLoaded {
        query: String,
        result: Result<Vec<Geoname>, ApiError>,
    },
    FeedLoaded {
        generation: u64,
        result: Result<FeedPage, ApiError>,
    },
    FollowListLoaded {
        list: FollowList,
        generation: u64,
        result: Result<FollowPage, ApiError>,
    },
    DailyListLoaded {
        generation: u64,
        result: Result<ArticlePage, ApiError>,
    },
    DailyArticleLoaded {
        url: String,
        generation: u64,
        result: Result<ArticleDetail, ApiError>,
    },
    FollowChanged {
        target: FollowTarget,
        follow: bool,
        result: Result<(), ApiError>,
    },
    WishlistChanged {
        item: TralbumRef,
        wishlisted: bool,
        result: Result<(), ApiError>,
    },
    GlobalSearchLoaded {
        request_id: u64,
        result: Result<SearchResponse, ApiError>,
    },
    BandLoaded {
        band_id: u64,
        result: Result<BandDetails, ApiError>,
    },
    FanPageLoaded {
        fan_id: u64,
        generation: u64,
        result: Result<CollectionPage, ApiError>,
    },
}

pub struct App {
    pub screen: Screen,
    pub section: Section,
    pub session: Option<Session>,
    pub login: LoginState,
    pub status: String,
    pub show_help: bool,
    pub tick: u64,
    pub config: Config,
    pub store_name: String,
    pub library: Library,
    pub discover: Discover,
    pub feed: Feed,
    pub daily: Daily,
    /// Bands and fans the logged-in fan follows, as far as we have seen.
    pub followed_bands: HashSet<u64>,
    pub followed_fans: HashSet<u64>,
    pub wishlisted: HashSet<(TralbumKind, u64)>,
    pub owned: HashSet<(TralbumKind, u64)>,
    pub search: Search,
    pub album: Option<AlbumView>,
    pub artist: Option<ArtistView>,
    pub fan: Option<FanView>,
    pub queue: Queue,
    pub queue_selected: usize,
    pub show_queue: bool,
    pub playback: Playback,
    /// Where the queue is persisted; `None` (tests) disables persistence.
    queue_store: Option<Arc<QueueStore>>,
    /// The last snapshot written to (or read from) disk, to write only on change.
    queue_saved: SavedQueue,
    /// Tralbums restored from disk whose stream URLs have not been fetched this run.
    stale_tralbums: HashSet<TralbumRef>,
    store: Arc<dyn CookieStore>,
    client: Client,
    player: PlayerHandle,
    /// Started by `run`, not `new`, so tests never touch the bus.
    #[cfg(target_os = "linux")]
    mpris: Option<MprisHandle>,
    tx: UnboundedSender<Message>,
    /// Bumped on logout so late responses from the previous session are dropped.
    generation: u64,
    should_quit: bool,
}

pub async fn run(mut terminal: DefaultTerminal, config: Config, no_login: bool) -> Result<()> {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let store = crate::secrets::open(config.secret_store)?;
    let queue_store = Arc::new(QueueStore::new(crate::config::queue_path()?));
    let mut app = App::new(config, store, Some(queue_store), tx.clone())?;
    #[cfg(target_os = "linux")]
    if app.config.mpris {
        app.mpris = Some(MprisHandle::spawn(tx.clone(), app.mpris_snapshot()));
    }
    event::spawn(tx);
    if no_login {
        app.on_cookie_loaded(Ok(None));
    } else {
        app.load_cookie();
    }

    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;
        let Some(message) = rx.recv().await else {
            break;
        };
        app.handle(message);
        // Coalesce whatever else is queued before the next redraw.
        while let Ok(message) = rx.try_recv() {
            app.handle(message);
        }
        app.persist_queue();
        #[cfg(target_os = "linux")]
        app.sync_mpris();
    }
    if let Some(write) = app.persist_queue() {
        let _ = write.await;
    }
    app.player.send(PlayerCommand::Shutdown);
    #[cfg(target_os = "linux")]
    if let Some(mpris) = app.mpris.take() {
        mpris.shutdown();
    }
    Ok(())
}

impl App {
    fn new(
        config: Config,
        store: Arc<dyn CookieStore>,
        queue_store: Option<Arc<QueueStore>>,
        tx: UnboundedSender<Message>,
    ) -> Result<Self> {
        let player = PlayerHandle::spawn(
            tokio::runtime::Handle::current(),
            tx.clone(),
            f32::from(config.volume) / 100.0,
        );
        let queue_saved = queue_store
            .as_ref()
            .and_then(|store| match store.load() {
                Ok(saved) => saved,
                Err(err) => {
                    tracing::error!(?err, "loading the saved queue failed");
                    None
                }
            })
            .unwrap_or_default();
        let mut queue = Queue::default();
        queue.restore(queue_saved.tracks.clone(), queue_saved.current);
        // Saved stream URLs are signed and have almost certainly expired.
        let stale_tralbums = queue.tracks.iter().map(|t| t.tralbum).collect();
        Ok(Self {
            screen: Screen::Booting,
            section: Section::Collection,
            session: None,
            login: LoginState::default(),
            status: "Starting…".to_owned(),
            show_help: false,
            tick: 0,
            store_name: store.describe(),
            config,
            library: Library::new(),
            discover: Discover::new(),
            feed: Feed::new(),
            daily: Daily::new(),
            followed_bands: HashSet::new(),
            followed_fans: HashSet::new(),
            wishlisted: HashSet::new(),
            owned: HashSet::new(),
            search: Search::new(),
            album: None,
            artist: None,
            fan: None,
            queue_selected: queue.current.unwrap_or(0),
            queue,
            show_queue: false,
            playback: Playback::default(),
            queue_store,
            queue_saved,
            stale_tralbums,
            store,
            client: Client::new().context("building HTTP client")?,
            player,
            #[cfg(target_os = "linux")]
            mpris: None,
            tx,
            generation: 0,
            should_quit: false,
        })
    }

    pub fn quit(&mut self) {
        self.should_quit = true;
    }

    pub fn busy(&self) -> bool {
        self.screen == Screen::Booting
            || self.login.busy
            || self.library.list().loading
            || self
                .library
                .list()
                .search
                .as_ref()
                .is_some_and(|s| s.loading)
            || self.album.as_ref().is_some_and(|a| a.loading)
            || self.discover.loading
            || self.search.loading
            || self.artist.as_ref().is_some_and(|a| a.loading)
            || self.fan.as_ref().is_some_and(|f| f.list.loading)
            || self.feed.stories.loading
            || self
                .feed
                .tab
                .follow_list()
                .is_some_and(|l| self.feed.follow_page(l).loading)
            || self.daily.articles.loading
            || self.daily.view.as_ref().is_some_and(|v| v.loading)
            || self.playback.state == PlaybackState::Loading
    }

    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Terminal(Event::Key(key)) if key.kind != KeyEventKind::Release => {
                keys::handle_key(self, key);
            }
            Message::Terminal(Event::Paste(text)) => self.on_paste(text),
            Message::Terminal(_) => {}
            Message::Tick => self.tick = self.tick.wrapping_add(1),
            Message::CookieLoaded(result) => self.on_cookie_loaded(result),
            Message::SessionChecked { check, result } => self.on_session_checked(check, result),
            Message::CookieSaved(Ok(())) => {
                tracing::info!("cookie saved to {}", self.store_name);
            }
            Message::CookieSaved(Err(err)) => {
                tracing::error!(%err, "saving cookie failed");
                self.status = format!("Logged in, but the cookie could not be saved: {err}");
            }
            Message::CookieCleared(Ok(())) => {}
            Message::CookieCleared(Err(err)) => {
                tracing::error!(%err, "clearing cookie failed");
                self.status = format!("Could not remove the stored cookie: {err}");
            }
            Message::PageLoaded {
                kind,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.on_page_loaded(kind, result);
                }
            }
            Message::SearchLoaded {
                kind,
                key,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.on_search_loaded(kind, key, result);
                }
            }
            Message::TralbumLoaded {
                tralbum,
                intent,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.on_tralbum_loaded(tralbum, intent, result);
                }
            }
            Message::Player(event) => self.on_player_event(event),
            #[cfg(target_os = "linux")]
            Message::Mpris(command) => self.on_mpris_command(command),
            Message::DiscoverOptionsLoaded(Ok(options)) => {
                self.discover.options = options;
                self.discover.options_live = true;
            }
            Message::DiscoverOptionsLoaded(Err(err)) => {
                tracing::warn!(%err, "live discover options unavailable, using the embedded snapshot");
            }
            Message::DiscoverLoaded { request_id, result } => {
                self.on_discover_loaded(request_id, result)
            }
            Message::TagSuggestionsLoaded { prefix, result } => {
                self.on_tag_suggestions(prefix, result)
            }
            Message::RelatedTagsLoaded { key, result } => self.on_related_tags(key, result),
            Message::GeonamesLoaded { query, result } => self.on_geonames(query, result),
            Message::FeedLoaded { generation, result } => {
                if generation == self.generation {
                    self.on_feed_loaded(result);
                }
            }
            Message::FollowListLoaded {
                list,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.on_follow_list_loaded(list, result);
                }
            }
            Message::DailyListLoaded { generation, result } => {
                if generation == self.generation {
                    self.on_daily_list_loaded(result);
                }
            }
            Message::DailyArticleLoaded {
                url,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.on_daily_article_loaded(url, result);
                }
            }
            Message::FollowChanged {
                target,
                follow,
                result,
            } => self.on_follow_changed(target, follow, result),
            Message::WishlistChanged {
                item,
                wishlisted,
                result,
            } => {
                self.on_wishlist_changed(item, wishlisted, result);
            }
            Message::GlobalSearchLoaded { request_id, result } => {
                self.on_global_search_loaded(request_id, result)
            }
            Message::BandLoaded { band_id, result } => self.on_band_loaded(band_id, result),
            Message::FanPageLoaded {
                fan_id,
                generation,
                result,
            } => {
                if generation == self.generation {
                    self.on_fan_page_loaded(fan_id, result);
                }
            }
        }
    }

    /// True when the artist page belongs to the section on screen (and no album covers it).
    pub fn artist_open_here(&self) -> bool {
        !self.album_open_here()
            && self
                .artist
                .as_ref()
                .is_some_and(|a| a.section == self.section)
    }

    /// True when a fan page belongs to the section on screen (and nothing covers it).
    pub fn fan_open_here(&self) -> bool {
        !self.album_open_here()
            && !self.artist_open_here()
            && self.fan.as_ref().is_some_and(|f| f.section == self.section)
    }

    /// Close the topmost page in this section; false when there was none.
    pub fn close_top_view(&mut self) -> bool {
        if self.album_open_here() {
            self.album = None;
        } else if self.artist_open_here() {
            self.artist = None;
        } else if self.fan_open_here() {
            self.fan = None;
        } else {
            return false;
        }
        true
    }

    /// True when the album view belongs to the section on screen.
    pub fn album_open_here(&self) -> bool {
        self.album
            .as_ref()
            .is_some_and(|a| a.source.section == self.section)
    }

    pub fn set_section(&mut self, section: Section) {
        self.section = section;
        match section {
            Section::Discover => self.ensure_discover_loaded(),
            Section::Feed => self.ensure_feed_loaded(),
            Section::Daily => self.ensure_daily_loaded(),
            _ => {}
        }
    }

    fn on_paste(&mut self, text: String) {
        let input = match self.screen {
            Screen::Login if !self.login.busy => &mut self.login.input,
            Screen::Main if self.section == Section::Collection && self.library.search_open => {
                &mut self.library.search_input
            }
            Screen::Main if self.section == Section::Search && self.search.input_open => {
                &mut self.search.input
            }
            Screen::Main
                if self.section == Section::Discover && self.discover.tag_entry.is_some() =>
            {
                &mut self.discover.tag_entry.as_mut().expect("checked").input
            }
            Screen::Main if self.section == Section::Discover && self.discover.picker.is_some() => {
                &mut self.discover.picker.as_mut().expect("checked").filter
            }
            _ => return,
        };
        for ch in text.chars().filter(|c| !c.is_control()) {
            input.handle(InputRequest::InsertChar(ch));
        }
    }

    // ----- session lifecycle -------------------------------------------------

    fn load_cookie(&mut self) {
        let store = self.store.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || store.load())
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| describe_secret_error(&e)));
            let _ = tx.send(Message::CookieLoaded(result));
        });
    }

    fn on_cookie_loaded(&mut self, result: Result<Option<String>, String>) {
        match result {
            Ok(Some(cookie)) => {
                let who = self.config.username.as_deref().unwrap_or("saved session");
                self.status = format!("Checking {who}…");
                self.check_session(SessionCheck::Boot(cookie));
            }
            Ok(None) => {
                self.enter_anonymous();
                self.status = "Not logged in. Press L to log in.".to_owned();
            }
            Err(err) => {
                tracing::error!(%err, "loading cookie failed");
                // Shown on the login page when the user opens it.
                self.login.hint = Some(err);
                self.enter_anonymous();
                self.status = "Could not read the stored session. Press L to log in.".to_owned();
            }
        }
    }

    /// Install the cookie (if the check carries one) and ask Bandcamp who it belongs to.
    fn check_session(&mut self, check: SessionCheck) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            if let SessionCheck::Boot(cookie) | SessionCheck::Login(cookie) = &check {
                client.set_identity_cookie(cookie);
            }
            let result = auth::validate_session(&client).await;
            let _ = tx.send(Message::SessionChecked { check, result });
        });
    }

    fn on_session_checked(&mut self, check: SessionCheck, result: Result<Session, ApiError>) {
        self.login.busy = false;
        match result {
            Ok(session) => {
                tracing::info!(fan_id = session.fan_id, username = %session.username, "session valid");
                self.status = format!("Logged in as {}.", session.username);
                if self.config.username.as_deref() != Some(session.username.as_str()) {
                    self.config.username = Some(session.username.clone());
                    self.save_config();
                }
                self.client.set_crumb_pages(vec![
                    format!("/{}", session.username),
                    format!("/{}/feed", session.username),
                ]);
                self.followed_bands
                    .extend(session.following_bands.iter().copied());
                self.followed_fans
                    .extend(session.following_fans.iter().copied());
                self.owned.extend(session.owned.iter().copied());
                self.session = Some(session);
                self.login = LoginState::default();
                self.screen = Screen::Main;
                if let SessionCheck::Login(cookie) = check {
                    self.save_cookie(cookie);
                }
                self.ensure_list_loaded(self.library.tab);
                if self.section == Section::Feed {
                    self.ensure_feed_loaded();
                }
            }
            Err(err) => {
                tracing::warn!(%err, "session check failed");
                match (check, err) {
                    // A pasted cookie that Bandcamp does not accept. The user may
                    // have left the login page meanwhile, so do not force it back.
                    (SessionCheck::Login(_), err) => {
                        self.login.error = Some(err.user_message());
                        self.status = "Login failed.".to_owned();
                    }
                    // The stored cookie is dead: forget it and carry on anonymously.
                    (
                        check @ (SessionCheck::Boot(_) | SessionCheck::Refresh),
                        ApiError::NotLoggedIn,
                    ) => {
                        self.forget_session();
                        self.clear_cookie();
                        self.login.error =
                            Some("The saved session has expired. Paste a fresh cookie.".to_owned());
                        if matches!(check, SessionCheck::Boot(_)) {
                            self.set_section(Section::Discover);
                        }
                        self.status = "Session expired. Press L to log in again.".to_owned();
                    }
                    // Network trouble at startup: run anonymously, but keep the cookie
                    // in the login field so Enter (or `r`) retries with it.
                    (SessionCheck::Boot(cookie), err) => {
                        self.login = LoginState::default();
                        self.login.input = Input::new(cookie);
                        self.login.error = Some(err.user_message());
                        self.login.hint =
                            Some("Press Enter to retry with the saved cookie.".to_owned());
                        // The cookie went into the jar before the check; anonymous
                        // requests must not carry it.
                        self.reset_client();
                        self.enter_anonymous();
                        self.status =
                            "Could not verify the saved session. Press L to retry.".to_owned();
                    }
                    // A refresh that failed for network reasons: stay put, report it.
                    (SessionCheck::Refresh, err) => {
                        self.status = format!("Refresh failed: {}", err.user_message());
                    }
                }
            }
        }
    }

    pub fn submit_login(&mut self) {
        if self.login.busy {
            return;
        }
        let cookie = auth::normalize_cookie(self.login.input.value());
        if cookie.is_empty() {
            self.login.error = Some("Paste the identity cookie first.".to_owned());
            return;
        }
        self.login.error = None;
        self.login.busy = true;
        self.status = "Checking the cookie with bandcamp.com…".to_owned();
        self.check_session(SessionCheck::Login(cookie));
    }

    /// Re-validate the current session (also refreshes the counts). Without a
    /// session, retry the cookie left in the login field (a saved cookie that
    /// could not be verified at startup), if any.
    pub fn refresh_session(&mut self) {
        match &self.session {
            Some(session) => {
                self.status = format!("Refreshing {}…", session.username);
                self.check_session(SessionCheck::Refresh);
            }
            None if !self.login.input.value().trim().is_empty() => self.submit_login(),
            None => self.status = "Not logged in. Press L to log in.".to_owned(),
        }
    }

    /// `L`: open the login page when logged out, log out otherwise.
    pub fn toggle_login(&mut self) {
        if self.session.is_some() {
            self.logout();
            return;
        }
        // Keep `input` and `hint`: a boot-retry cookie or a keyring hint may be waiting.
        self.login.error = None;
        self.screen = Screen::Login;
        self.status = "Paste your Bandcamp identity cookie.".to_owned();
    }

    /// `Esc` on the login page. Allowed while a check is running: a late
    /// success lands on the main screen anyway, a failure only records the error.
    pub fn close_login(&mut self) {
        self.screen = Screen::Main;
        self.status = "Not logged in. Press L to log in.".to_owned();
    }

    pub fn logout(&mut self) {
        tracing::info!("logging out");
        self.forget_session();
        self.clear_cookie();
        self.config.username = None;
        self.save_config();
        self.status = "Logged out. Press L to log in again.".to_owned();
    }

    /// Show the main screen without a session, starting on the public Discover section.
    fn enter_anonymous(&mut self) {
        self.screen = Screen::Main;
        self.set_section(Section::Discover);
    }

    /// Replace the HTTP client so the cookie jar is empty again.
    fn reset_client(&mut self) {
        match Client::new() {
            Ok(client) => self.client = client,
            Err(err) => tracing::error!(?err, "could not rebuild HTTP client"),
        }
    }

    /// Drop everything tied to the current account and stay on the main screen.
    /// Playback and the queue survive: streams are public.
    fn forget_session(&mut self) {
        // Late replies from the old session are dropped by the generation check;
        // that also strands in-flight public loads, so the views that carry a
        // spinner are reset and the current section reloads below.
        self.generation += 1;
        self.reset_client();
        self.session = None;
        self.library = Library::new();
        self.feed = Feed::new();
        self.daily = Daily::new();
        self.followed_bands.clear();
        self.followed_fans.clear();
        self.wishlisted.clear();
        self.owned.clear();
        self.search = Search::new();
        self.album = None;
        self.artist = None;
        self.fan = None;
        self.login = LoginState::default();
        self.screen = Screen::Main;
        self.set_section(self.section);
    }

    fn save_config(&self) {
        if let Err(err) = self.config.save() {
            tracing::error!(?err, "saving config failed");
        }
    }

    /// Write the queue to disk if it changed since the last write. Called after every
    /// handled message, so a crash loses at most the message being handled.
    fn persist_queue(&mut self) -> Option<tokio::task::JoinHandle<()>> {
        let store = self.queue_store.clone()?;
        let saved = SavedQueue::from(&self.queue);
        if saved == self.queue_saved {
            return None;
        }
        self.queue_saved = saved.clone();
        Some(tokio::task::spawn_blocking(move || {
            if let Err(err) = store.save(&saved) {
                tracing::error!(?err, "saving the queue failed");
            }
        }))
    }

    fn save_cookie(&self, cookie: String) {
        let store = self.store.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || store.save(&cookie))
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| describe_secret_error(&e)));
            let _ = tx.send(Message::CookieSaved(result));
        });
    }

    fn clear_cookie(&self) {
        let store = self.store.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = tokio::task::spawn_blocking(move || store.clear())
                .await
                .map_err(|e| e.to_string())
                .and_then(|r| r.map_err(|e| describe_secret_error(&e)));
            let _ = tx.send(Message::CookieCleared(result));
        });
    }

    // ----- library -------------------------------------------------------------

    fn ensure_list_loaded(&mut self, kind: ListKind) {
        if !self.library.list_for(kind).started {
            self.load_more(kind);
        }
    }

    fn load_more(&mut self, kind: ListKind) {
        let Some(fan_id) = self.session.as_ref().map(|s| s.fan_id) else {
            self.status = "Log in (L) to see your collection.".to_owned();
            return;
        };
        let list = self.library.list_for_mut(kind);
        if list.loading || (!list.more && list.started) {
            return;
        }
        list.loading = true;
        list.started = true;
        list.error = None;
        let token = list.next_token.clone().unwrap_or_else(fan::initial_token);
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = fan::list_items(&client, kind, fan_id, &token, fan::PAGE_SIZE).await;
            let _ = tx.send(Message::PageLoaded {
                kind,
                generation,
                result,
            });
        });
    }

    fn on_page_loaded(&mut self, kind: ListKind, result: Result<CollectionPage, ApiError>) {
        self.library.list_for_mut(kind).loading = false;
        match result {
            Ok(page) => {
                tracing::debug!(
                    ?kind,
                    items = page.items.len(),
                    more = page.more_available,
                    "page loaded"
                );
                let keys: Vec<(TralbumKind, u64)> = page
                    .items
                    .iter()
                    .map(|i| (i.tralbum().kind, i.tralbum_id))
                    .collect();
                match kind {
                    ListKind::Wishlist => self.wishlisted.extend(keys),
                    ListKind::Collection => self.owned.extend(keys),
                }
                self.library.list_for_mut(kind).absorb_page(page);
            }
            Err(err) => {
                self.library.list_for_mut(kind).error = Some(err.user_message());
                self.status = format!("{}: {}", kind.title(), err.user_message());
            }
        }
    }

    pub fn library_move(&mut self, delta: isize) {
        self.library.list_mut().move_by(delta);
        let kind = self.library.tab;
        if self.library.list().wants_more() {
            self.load_more(kind);
        }
    }

    pub fn toggle_library_tab(&mut self) {
        self.library.toggle_tab();
        self.ensure_list_loaded(self.library.tab);
    }

    pub fn reload_library(&mut self) {
        let kind = self.library.tab;
        self.library.list_mut().reset();
        self.load_more(kind);
    }

    pub fn run_search(&mut self, key: String) {
        let Some(fan_id) = self.session.as_ref().map(|s| s.fan_id) else {
            self.status = "Log in (L) to search your collection.".to_owned();
            return;
        };
        let kind = self.library.tab;
        self.library.list_mut().search = Some(SearchResults {
            key: key.clone(),
            items: Vec::new(),
            table: TableState::default(),
            loading: true,
        });
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = fan::search_items(&client, kind, fan_id, &key).await;
            let _ = tx.send(Message::SearchLoaded {
                kind,
                key,
                generation,
                result,
            });
        });
    }

    fn on_search_loaded(
        &mut self,
        kind: ListKind,
        key: String,
        result: Result<SearchItemsResponse, ApiError>,
    ) {
        let list = self.library.list_for_mut(kind);
        let Some(search) = list.search.as_mut().filter(|s| s.key == key) else {
            return;
        };
        search.loading = false;
        match result {
            Ok(response) => {
                search.items = response.tralbums;
                search.table.select((!search.items.is_empty()).then_some(0));
                let n = search.items.len();
                list.cycle_sort();
                list.cycle_sort();
                list.cycle_sort(); // re-apply the current sort to the results
                self.status = format!("{n} matches for \"{key}\".");
            }
            Err(err) => {
                list.search = None;
                self.status = format!("Search failed: {}", err.user_message());
            }
        }
    }

    pub fn clear_search(&mut self) {
        let list = self.library.list_mut();
        if list.search.take().is_some() {
            self.library.search_input.reset();
            self.status = "Search cleared.".to_owned();
        }
    }

    pub fn open_selected(&mut self, intent: Intent) {
        let Some(item) = self.library.list().selected_item().cloned() else {
            return;
        };
        self.open_item(item, intent);
    }

    fn open_item(&mut self, item: CollectionItem, intent: Intent) {
        let source = AlbumSource {
            tralbum: item.tralbum(),
            title: item.item_title,
            url: item.item_url,
            section: Section::Collection,
        };
        self.open_source(source, intent);
    }

    fn open_source(&mut self, source: AlbumSource, intent: Intent) {
        let tralbum = source.tralbum;
        if let Intent::Show = intent {
            self.album = Some(AlbumView {
                source,
                tralbum: None,
                loading: true,
                error: None,
                table: TableState::default(),
            });
        } else {
            self.status = format!("Loading {}…", source.title);
        }
        self.fetch_tralbum(tralbum, intent);
    }

    fn fetch_tralbum(&mut self, tralbum_ref: TralbumRef, intent: Intent) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = tralbum::tralbum_details(&client, tralbum_ref).await;
            let _ = tx.send(Message::TralbumLoaded {
                tralbum: tralbum_ref,
                intent,
                generation,
                result,
            });
        });
    }

    fn on_tralbum_loaded(
        &mut self,
        tralbum_ref: TralbumRef,
        intent: Intent,
        result: Result<Tralbum, ApiError>,
    ) {
        let tralbum = match result {
            Ok(tralbum) => tralbum,
            Err(err) => {
                tracing::warn!(%tralbum_ref, %err, "tralbum details failed");
                match intent {
                    Intent::Show => {
                        if let Some(view) = self
                            .album
                            .as_mut()
                            .filter(|v| v.source.tralbum == tralbum_ref)
                        {
                            view.loading = false;
                            view.error = Some(err.user_message());
                        }
                    }
                    Intent::Refresh { .. } => {
                        self.playback.state = PlaybackState::Idle;
                        self.status =
                            format!("Could not refresh the stream: {}", err.user_message());
                    }
                    _ => self.status = format!("Could not load the album: {}", err.user_message()),
                }
                return;
            }
        };
        match intent {
            Intent::Show => {
                if let Some(view) = self
                    .album
                    .as_mut()
                    .filter(|v| v.source.tralbum == tralbum_ref)
                {
                    view.loading = false;
                    view.table.select((!tralbum.tracks.is_empty()).then_some(0));
                    view.tralbum = Some(tralbum);
                }
            }
            Intent::Play { start_track } => {
                let tracks = queue::tracks_from(&tralbum);
                match self.queue.replace(tracks, start_track) {
                    Some(index) => self.play_index(index),
                    None => self.status = format!("{} has no tracks.", tralbum.title),
                }
            }
            Intent::Enqueue => {
                let tracks = queue::tracks_from(&tralbum);
                let n = tracks.len();
                let was_idle = self.playback.state == PlaybackState::Idle;
                let first_new = self.queue.len();
                self.queue.append(tracks);
                self.status = format!("Queued {n} tracks from {}.", tralbum.title);
                if was_idle && self.queue.current.is_none() {
                    self.play_index(first_new);
                }
            }
            Intent::Refresh {
                queue_index,
                after_failure,
            } => {
                self.queue.refresh_urls(&tralbum);
                if let Some(track) = self.queue.tracks.get_mut(queue_index)
                    && after_failure
                {
                    track.retried = true;
                }
                self.play_index(queue_index);
            }
        }
    }

    pub fn open_in_browser(&mut self, url: Option<String>) {
        let Some(url) = url else { return };
        match open::that_detached(&url) {
            Ok(()) => self.status = format!("Opened {url}"),
            Err(err) => self.status = format!("Could not open a browser: {err}"),
        }
    }

    // ----- album view ------------------------------------------------------------

    pub fn album_move(&mut self, delta: isize) {
        let Some(view) = self.album.as_mut() else {
            return;
        };
        let len = view.tralbum.as_ref().map_or(0, |t| t.tracks.len());
        if len == 0 {
            return;
        }
        let current = view.table.selected().unwrap_or(0) as isize;
        let next = current.saturating_add(delta).clamp(0, len as isize - 1) as usize;
        view.table.select(Some(next));
    }

    pub fn play_album_from_selection(&mut self) {
        let start = self.album.as_ref().and_then(|view| {
            let tralbum = view.tralbum.as_ref()?;
            let index = view.table.selected()?;
            tralbum.tracks.get(index).map(|t| t.track_id)
        });
        self.play_album(start);
    }

    pub fn play_album(&mut self, start_track: Option<u64>) {
        let Some(view) = self.album.as_ref() else {
            return;
        };
        match &view.tralbum {
            Some(tralbum) => {
                let tracks = queue::tracks_from(tralbum);
                match self.queue.replace(tracks, start_track) {
                    Some(index) => self.play_index(index),
                    None => self.status = "Nothing to play.".to_owned(),
                }
            }
            None => self.status = "Still loading…".to_owned(),
        }
    }

    pub fn enqueue_album(&mut self) {
        let Some(view) = self.album.as_ref() else {
            return;
        };
        let Some(tralbum) = view.tralbum.clone() else {
            return;
        };
        self.on_tralbum_loaded(tralbum.tralbum_ref(), Intent::Enqueue, Ok(tralbum));
    }

    // ----- playback ------------------------------------------------------------------

    pub fn play_index(&mut self, index: usize) {
        let Some(track) = self.queue.tracks.get(index) else {
            return;
        };
        self.queue.current = Some(index);
        self.queue_selected = index;
        if self.stale_tralbums.remove(&track.tralbum) {
            // Restored from disk: fetch fresh stream URLs instead of trying a dead one.
            self.playback.state = PlaybackState::Loading;
            self.playback.position = Duration::ZERO;
            self.playback.track_id = Some(track.track_id);
            self.status = format!("Loading {} — {}…", track.title, track.artist);
            self.fetch_tralbum(
                track.tralbum,
                Intent::Refresh {
                    queue_index: index,
                    after_failure: false,
                },
            );
            return;
        }
        match &track.url {
            Some(url) => {
                self.playback.state = PlaybackState::Loading;
                self.playback.position = Duration::ZERO;
                self.playback.track_id = Some(track.track_id);
                self.status = format!("Loading {} — {}…", track.title, track.artist);
                self.player.send(PlayerCommand::Load {
                    track_id: track.track_id,
                    url: url.clone(),
                });
            }
            None => {
                self.status = format!("{} is not streamable, skipping.", track.title);
                self.next_track();
            }
        }
    }

    pub fn next_track(&mut self) {
        match self.queue.next_index() {
            Some(index) => self.play_index(index),
            None => {
                if self.playback.state != PlaybackState::Idle {
                    self.stop_playback();
                    self.status = "End of queue.".to_owned();
                }
            }
        }
    }

    pub fn prev_track(&mut self) {
        // Like every player: restart the track unless we are near its start.
        if self.playback.position > Duration::from_secs(3) {
            self.seek_to(Duration::ZERO);
            return;
        }
        if let Some(index) = self.queue.prev_index() {
            self.play_index(index);
        }
    }

    pub fn toggle_pause(&mut self) {
        match self.playback.state {
            PlaybackState::Playing => {
                self.playback.state = PlaybackState::Paused;
                self.player.send(PlayerCommand::Pause);
            }
            PlaybackState::Paused => {
                self.playback.state = PlaybackState::Playing;
                self.player.send(PlayerCommand::Resume);
            }
            PlaybackState::Idle => {
                if let Some(index) = self
                    .queue
                    .current
                    .or_else(|| (!self.queue.is_empty()).then_some(0))
                {
                    self.play_index(index);
                }
            }
            PlaybackState::Loading => {}
        }
    }

    pub fn stop_playback(&mut self) {
        self.player.send(PlayerCommand::Stop);
        self.playback.state = PlaybackState::Idle;
        self.playback.position = Duration::ZERO;
        self.playback.track_id = None;
    }

    pub fn seek_by(&mut self, seconds: i64) {
        if matches!(
            self.playback.state,
            PlaybackState::Playing | PlaybackState::Paused
        ) {
            self.player.send(PlayerCommand::SeekBy(seconds));
        }
    }

    pub(crate) fn seek_to(&mut self, position: Duration) {
        if matches!(
            self.playback.state,
            PlaybackState::Playing | PlaybackState::Paused
        ) {
            self.player.send(PlayerCommand::SeekTo(position));
        }
    }

    pub fn change_volume(&mut self, delta: i16) {
        let volume = (i16::from(self.config.volume) + delta).clamp(0, 100) as u8;
        self.set_volume(volume);
    }

    /// Absolute volume in percent; persisted like every other config change.
    pub fn set_volume(&mut self, volume: u8) {
        let volume = volume.min(100);
        if volume != self.config.volume {
            self.config.volume = volume;
            self.player
                .send(PlayerCommand::SetVolume(f32::from(volume) / 100.0));
            self.save_config();
        }
        self.status = format!("Volume {volume}%");
    }

    fn on_player_event(&mut self, event: PlayerEvent) {
        match event {
            PlayerEvent::Loading { track_id } => tracing::debug!(track_id, "buffering"),
            PlayerEvent::Started { track_id } => {
                if self.playback.track_id == Some(track_id) {
                    self.playback.state = PlaybackState::Playing;
                    if let Some(track) = self.queue.current_track() {
                        self.status = format!("Playing {} — {}", track.title, track.artist);
                    }
                }
            }
            PlayerEvent::Progress {
                track_id,
                position,
                paused,
            } => {
                if self.playback.track_id == Some(track_id) {
                    self.playback.position = position;
                    self.playback.state = if paused {
                        PlaybackState::Paused
                    } else {
                        PlaybackState::Playing
                    };
                }
            }
            PlayerEvent::Ended { track_id } => {
                if self.playback.track_id == Some(track_id) {
                    self.next_track();
                }
            }
            PlayerEvent::Failed { track_id, message } => {
                if self.playback.track_id != Some(track_id) {
                    return;
                }
                let Some(index) = self.queue.current else {
                    return;
                };
                let retry = self.queue.tracks.get(index).map(|t| (t.retried, t.tralbum));
                match retry {
                    // Signed stream URLs expire; fetch fresh ones once.
                    Some((false, tralbum)) => {
                        tracing::info!(track_id, %message, "refreshing stream urls");
                        self.status = "Stream failed, refreshing…".to_owned();
                        self.fetch_tralbum(
                            tralbum,
                            Intent::Refresh {
                                queue_index: index,
                                after_failure: true,
                            },
                        );
                    }
                    _ => {
                        self.playback.state = PlaybackState::Idle;
                        self.status = format!("Playback failed: {message}");
                    }
                }
            }
            PlayerEvent::Unavailable(message) => {
                self.playback.state = PlaybackState::Idle;
                self.playback.unavailable = Some(message.clone());
                self.status = format!("No audio output: {message}");
            }
        }
    }

    // ----- mpris -----------------------------------------------------------------------

    /// The current track stays visible while stopped, because Play resumes it.
    #[cfg(target_os = "linux")]
    fn mpris_snapshot(&self) -> mpris::Snapshot {
        let state = self.playback.state;
        mpris::Snapshot {
            track_id: self.queue.current_track().map(|t| t.track_id),
            status: match state {
                PlaybackState::Idle => mpris::Status::Stopped,
                PlaybackState::Loading | PlaybackState::Playing => mpris::Status::Playing,
                PlaybackState::Paused => mpris::Status::Paused,
            },
            position: self.playback.position,
            volume: self.config.volume,
            can_next: self.queue.next_index().is_some(),
            can_prev: self.queue.prev_index().is_some()
                || self.playback.position > Duration::from_secs(3),
            can_play: !self.queue.is_empty(),
            can_pause: matches!(state, PlaybackState::Playing | PlaybackState::Paused),
        }
    }

    #[cfg(target_os = "linux")]
    fn mpris_meta(&self) -> Option<mpris::TrackMeta> {
        let track = self.queue.current_track()?;
        Some(mpris::TrackMeta {
            track_id: track.track_id,
            title: track.title.clone(),
            artist: track.artist.clone(),
            album: track.album.clone(),
            duration: track.duration,
            art_url: track.art_id.map(queue::art_url),
            page_url: track.page_url.clone(),
        })
    }

    /// Called after every message batch; sends only what changed.
    #[cfg(target_os = "linux")]
    fn sync_mpris(&mut self) {
        let Some(handle) = self.mpris.as_ref() else {
            return;
        };
        let snapshot = self.mpris_snapshot();
        let meta = handle
            .track_changed(&snapshot)
            .then(|| self.mpris_meta())
            .flatten();
        if let Some(handle) = self.mpris.as_mut() {
            handle.sync(snapshot, meta);
        }
    }

    #[cfg(target_os = "linux")]
    fn on_mpris_command(&mut self, command: MprisCommand) {
        tracing::debug!(?command, "mpris");
        let duration = self.queue.current_track().map(|t| t.duration);
        match command {
            MprisCommand::Play => {
                if self.playback.state != PlaybackState::Playing {
                    self.toggle_pause();
                }
            }
            MprisCommand::Pause => {
                if self.playback.state == PlaybackState::Playing {
                    self.toggle_pause();
                }
            }
            MprisCommand::PlayPause => self.toggle_pause(),
            MprisCommand::Stop => self.stop_playback(),
            MprisCommand::Next => self.next_track(),
            MprisCommand::Previous => self.prev_track(),
            MprisCommand::Seek { offset_us } => {
                let offset = Duration::from_micros(offset_us.unsigned_abs());
                let target = if offset_us < 0 {
                    self.playback.position.saturating_sub(offset)
                } else {
                    self.playback.position + offset
                };
                // Seeking past the end skips to the next track, as the spec asks.
                if duration.is_some_and(|d| target >= d) {
                    self.next_track();
                } else {
                    self.seek_to(target);
                }
            }
            MprisCommand::SetPosition { track_id, position } => {
                if self.playback.track_id == Some(track_id)
                    && duration.is_none_or(|d| position <= d)
                {
                    self.seek_to(position);
                }
            }
            MprisCommand::SetVolume(volume) => {
                self.set_volume((volume.clamp(0.0, 1.0) * 100.0).round() as u8);
            }
            MprisCommand::Quit => self.quit(),
        }
    }

    // ----- discover ---------------------------------------------------------------

    fn ensure_discover_loaded(&mut self) {
        if !self.discover.started {
            self.load_discover_options();
            self.discover_search();
        }
    }

    fn load_discover_options(&self) {
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = discover_api::options(&client).await;
            let _ = tx.send(Message::DiscoverOptionsLoaded(result));
        });
    }

    /// Run the current filters from the first page.
    pub fn discover_search(&mut self) {
        self.discover.reset_results();
        self.discover.started = true;
        self.load_discover_page();
        self.load_related_tags();
    }

    fn load_discover_page(&mut self) {
        let d = &mut self.discover;
        if d.loading || !d.more {
            return;
        }
        d.loading = true;
        d.error = None;
        let request = d.filters.request(d.next_cursor.as_deref());
        let request_id = d.request_id;
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = discover_api::discover(&client, &request).await;
            let _ = tx.send(Message::DiscoverLoaded { request_id, result });
        });
    }

    fn on_discover_loaded(&mut self, request_id: u64, result: Result<DiscoverResponse, ApiError>) {
        let d = &mut self.discover;
        if request_id != d.request_id {
            return;
        }
        d.loading = false;
        match result {
            Ok(response) => {
                tracing::debug!(n = response.results.len(), total = ?response.result_count, "discover page");
                if response.result_count.is_some() {
                    d.total = response.result_count;
                }
                let cursor_moved = response.cursor.is_some() && response.cursor != d.next_cursor;
                d.more = !response.results.is_empty() && cursor_moved;
                d.next_cursor = response.cursor;
                for r in &response.results {
                    if let Some(t) = r.tralbum() {
                        if r.is_wishlisted {
                            self.wishlisted.insert((t.kind, t.id));
                        }
                        if r.is_owned {
                            self.owned.insert((t.kind, t.id));
                        }
                    }
                }
                let d = &mut self.discover;
                d.results.extend(response.results);
                if d.table.selected().is_none() && !d.results.is_empty() {
                    d.table.select(Some(0));
                }
            }
            Err(err) => {
                d.error = Some(err.user_message());
                self.status = format!("Discover: {}", err.user_message());
            }
        }
    }

    fn load_related_tags(&mut self) {
        let tags = self.discover.filters.tag_names();
        if tags.is_empty() {
            return;
        }
        let key = tags.join(",");
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = discover_api::related_tags(&client, &tags, 8).await;
            let _ = tx.send(Message::RelatedTagsLoaded { key, result });
        });
    }

    fn on_related_tags(&mut self, key: String, result: Result<Vec<RelatedTag>, ApiError>) {
        if key != self.discover.filters.tag_names().join(",") {
            return;
        }
        match result {
            Ok(tags) => self.discover.related = tags.into_iter().map(|t| t.norm_name).collect(),
            Err(err) => tracing::debug!(%err, "related tags failed"),
        }
    }

    pub fn discover_move(&mut self, delta: isize) {
        self.discover.move_by(delta);
        if self.discover.wants_more() {
            self.load_discover_page();
        }
    }

    pub fn discover_open(&mut self, intent: Intent) {
        let Some(result) = self.discover.selected().cloned() else {
            return;
        };
        let url = result.clean_url();
        match result.tralbum() {
            Some(tralbum) => self.open_source(
                AlbumSource {
                    tralbum,
                    title: result.title,
                    url,
                    section: Section::Discover,
                },
                intent,
            ),
            None => {
                self.status = "Merch opens in the browser.".to_owned();
                self.open_in_browser(Some(result.clean_url()));
            }
        }
    }

    pub fn open_picker(&mut self, field: Option<Field>) {
        let options = self.discover.picker_options(field);
        let current = field.map(|f| self.discover.current_key(f));
        self.discover.picker = Some(Picker::new(field, options, current.as_deref()));
    }

    pub fn picker_back(&mut self) {
        match self.discover.picker.as_ref().map(|p| p.field) {
            Some(Some(_)) => self.open_picker(None),
            _ => self.discover.picker = None,
        }
    }

    pub fn picker_enter(&mut self) {
        let Some(picker) = self.discover.picker.as_ref() else {
            return;
        };
        match picker.field {
            None => {
                let Some(option) = picker.selected_option() else {
                    return;
                };
                let field = Field::ALL.iter().copied().find(|f| f.label() == option.key);
                if let Some(field) = field {
                    self.open_picker(Some(field));
                }
            }
            Some(field) => {
                if let Some(option) = picker.selected_option().cloned() {
                    let changed = self.discover.apply(field, &option);
                    self.discover.picker = None;
                    if changed {
                        self.discover_search();
                    }
                } else if field == Field::Location {
                    let query = picker.filter.value().trim().to_owned();
                    self.search_geonames(query);
                }
            }
        }
    }

    pub fn picker_input(&mut self, request: InputRequest) {
        let Some(picker) = self.discover.picker.as_mut() else {
            return;
        };
        picker.filter.handle(request);
        picker.clamp();
        if picker.field == Some(Field::Location) {
            let query = picker.filter.value().trim().to_owned();
            if query.chars().count() >= 3 && picker.visible().is_empty() {
                self.search_geonames(query);
            }
        }
    }

    fn search_geonames(&mut self, query: String) {
        let Some(picker) = self.discover.picker.as_mut() else {
            return;
        };
        if query.chars().count() < 2 || picker.pending_query.as_deref() == Some(query.as_str()) {
            return;
        }
        picker.pending_query = Some(query.clone());
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = discover_api::geoname_search(&client, &query, 8).await;
            let _ = tx.send(Message::GeonamesLoaded { query, result });
        });
    }

    fn on_geonames(&mut self, query: String, result: Result<Vec<Geoname>, ApiError>) {
        let Some(picker) = self.discover.picker.as_mut() else {
            return;
        };
        if picker.field != Some(Field::Location) {
            return;
        }
        if picker.pending_query.as_deref() == Some(query.as_str()) {
            picker.pending_query = None;
        }
        match result {
            Ok(geonames) => picker.add_geonames(geonames),
            Err(err) => self.status = format!("Location search failed: {}", err.user_message()),
        }
    }

    pub fn open_tag_entry(&mut self) {
        let suggestions = self
            .discover
            .related
            .iter()
            .map(|t| TagSuggestion {
                norm_name: t.clone(),
                display_name: t.replace('-', " "),
                count: 0,
            })
            .collect();
        self.discover.tag_entry = Some(TagEntry {
            suggestions,
            ..Default::default()
        });
    }

    pub fn tag_entry_input(&mut self, request: InputRequest) {
        let Some(entry) = self.discover.tag_entry.as_mut() else {
            return;
        };
        entry.input.handle(request);
        entry.selected = None;
        let prefix = entry.input.value().trim().to_owned();
        if prefix.chars().count() < 2 {
            entry.suggestions_for.clear();
            let related = self.discover.related.clone();
            entry.suggestions = related
                .into_iter()
                .map(|t| TagSuggestion {
                    norm_name: t.clone(),
                    display_name: t.replace('-', " "),
                    count: 0,
                })
                .collect();
            return;
        }
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = discover_api::tag_suggestions(&client, &prefix).await;
            let _ = tx.send(Message::TagSuggestionsLoaded { prefix, result });
        });
    }

    fn on_tag_suggestions(&mut self, prefix: String, result: Result<Vec<TagSuggestion>, ApiError>) {
        let Some(entry) = self.discover.tag_entry.as_mut() else {
            return;
        };
        if entry.input.value().trim() != prefix {
            return;
        }
        if let Ok(mut suggestions) = result {
            suggestions.truncate(10);
            entry.suggestions = suggestions;
            entry.suggestions_for = prefix;
            entry.selected = None;
        }
    }

    pub fn tag_entry_enter(&mut self) {
        let Some(entry) = self.discover.tag_entry.as_ref() else {
            return;
        };
        let chosen = entry.chosen();
        self.discover.tag_entry = None;
        let Some(tag) = chosen else { return };
        if self.discover.filters.tags.contains(&tag) {
            self.status = format!("Tag {tag} is already set.");
            return;
        }
        self.discover.filters.tags.push(tag);
        self.discover_search();
    }

    pub fn remove_last_tag(&mut self) {
        if self.discover.filters.tags.pop().is_some() {
            self.discover_search();
        }
    }

    pub fn reset_discover_filters(&mut self) {
        if self.discover.filters != discover::Filters::default() {
            self.discover.filters = discover::Filters::default();
            self.discover_search();
        }
    }

    // ----- daily -----------------------------------------------------------------------

    fn ensure_daily_loaded(&mut self) {
        if !self.daily.articles.started {
            self.load_daily_page();
        }
    }

    fn load_daily_page(&mut self) {
        let page = &mut self.daily.articles;
        if page.loading || (!page.more && page.started) {
            return;
        }
        page.loading = true;
        page.started = true;
        page.error = None;
        let number = self.daily.next_page();
        let franchise = self.daily.franchise;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = daily_api::list(&client, franchise, number).await;
            let _ = tx.send(Message::DailyListLoaded { generation, result });
        });
    }

    fn on_daily_list_loaded(&mut self, result: Result<ArticlePage, ApiError>) {
        let page = &mut self.daily.articles;
        page.loading = false;
        match result {
            Ok(listing) => {
                tracing::info!(articles = listing.articles.len(), next = ?listing.next_page, "daily page");
                let more = listing.next_page.is_some();
                let token = listing.next_page.map(|n| n.to_string());
                page.absorb(listing.articles, more, token);
                if page.items.is_empty() {
                    page.error =
                        Some("No articles found; the site layout may have changed.".to_owned());
                }
            }
            Err(err) => {
                page.error = Some(err.user_message());
                self.status = format!("Daily: {}", err.user_message());
            }
        }
    }

    pub fn daily_move(&mut self, delta: isize) {
        let len = self.daily.articles.items.len();
        self.daily.articles.move_within(len, delta);
        if self.daily.articles.wants_more(len) {
            self.load_daily_page();
        }
    }

    pub fn daily_set_franchise(&mut self, next: bool) {
        self.daily.franchise = if next {
            self.daily.franchise.next()
        } else {
            self.daily.franchise.prev()
        };
        self.reload_daily();
    }

    pub fn reload_daily(&mut self) {
        self.daily.articles.reset();
        self.load_daily_page();
    }

    /// Enter on the article list: fetch the article and show it in place.
    pub fn daily_open_article(&mut self) {
        let Some(article) = self.daily.selected_article().cloned() else {
            return;
        };
        self.daily.view = Some(ArticleView::new(article));
        self.fetch_daily_article();
    }

    /// `R` inside an article: fetch it again (after an error, or to refresh).
    pub fn daily_reload_article(&mut self) {
        if let Some(view) = self.daily.view.as_mut() {
            view.loading = true;
            view.error = None;
            self.fetch_daily_article();
        }
    }

    fn fetch_daily_article(&mut self) {
        let Some(url) = self.daily.view.as_ref().map(|v| v.article.url.clone()) else {
            return;
        };
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = daily_api::article(&client, &url).await;
            let _ = tx.send(Message::DailyArticleLoaded {
                url,
                generation,
                result,
            });
        });
    }

    fn on_daily_article_loaded(&mut self, url: String, result: Result<ArticleDetail, ApiError>) {
        let Some(view) = self.daily.view.as_mut().filter(|v| v.article.url == url) else {
            return;
        };
        view.loading = false;
        match result {
            Ok(detail) => {
                tracing::info!(
                    albums = detail.featured.len(),
                    blocks = detail.body.len(),
                    "daily article"
                );
                if detail.featured.is_empty() {
                    view.focus = ArticleFocus::Text;
                } else {
                    view.tracks.select(Some(0));
                }
                view.detail = Some(detail);
            }
            Err(err) => {
                tracing::warn!(url, %err, "daily article failed");
                view.error = Some(err.user_message());
            }
        }
    }

    pub fn daily_close_article(&mut self) {
        self.daily.view = None;
    }

    pub fn daily_toggle_focus(&mut self) {
        if let Some(view) = self.daily.view.as_mut() {
            view.focus = view.focus.toggle();
        }
    }

    /// The featured track under the cursor in an open article.
    fn daily_selected_track(&self) -> Option<&FeaturedTrack> {
        self.daily.view.as_ref().and_then(|v| v.selected_track())
    }

    /// Enter / p: play the article's featured tracks from the selected one, straight
    /// from the stream URLs embedded in the page (no album fetch, like the website).
    pub fn daily_play_track(&mut self) {
        let Some(view) = self.daily.view.as_ref() else {
            return;
        };
        let Some(start) = view.selected_track().map(|t| t.track_id) else {
            return;
        };
        let tracks: Vec<QueuedTrack> = view.featured().iter().map(queued_track).collect();
        match self.queue.replace(tracks, Some(start)) {
            Some(index) => self.play_index(index),
            None => self.status = "This article has no playable tracks.".to_owned(),
        }
    }

    /// `a`: append the selected featured track; start it when nothing is playing.
    pub fn daily_enqueue_track(&mut self) {
        let Some(track) = self.daily_selected_track().map(queued_track) else {
            return;
        };
        let title = track.title.clone();
        let was_idle = self.playback.state == PlaybackState::Idle;
        let first_new = self.queue.len();
        self.queue.append(vec![track]);
        self.status = format!("Queued {title}.");
        if was_idle && self.queue.current.is_none() {
            self.play_index(first_new);
        }
    }

    /// `l`: the album (or single) the selected track comes from.
    pub fn daily_open_album(&mut self) {
        let Some(track) = self.daily_selected_track().cloned() else {
            return;
        };
        let title = if track.artist.is_empty() {
            track.album
        } else {
            format!("{} — {}", track.artist, track.album)
        };
        self.open_source(
            AlbumSource {
                tralbum: track.tralbum,
                title,
                url: track.url,
                section: Section::Daily,
            },
            Intent::Show,
        );
    }

    /// `o`: the selected track's page, else the article, else nothing.
    pub fn daily_selected_url(&self) -> Option<String> {
        match self.daily.view.as_ref() {
            Some(view) => match view.focus {
                ArticleFocus::Tracks => view
                    .selected_track()
                    .map(|a| a.url.clone())
                    .filter(|u| !u.is_empty())
                    .or_else(|| Some(view.article.url.clone())),
                ArticleFocus::Text => Some(view.article.url.clone()),
            },
            None => self.daily.selected_article().map(|a| a.url.clone()),
        }
    }

    // ----- feed & following ---------------------------------------------------------

    fn ensure_feed_loaded(&mut self) {
        match self.feed.tab.follow_list() {
            None if !self.feed.stories.started => self.load_feed_page(),
            Some(list) if !self.feed.follow_page(list).started => self.load_follow_page(list),
            _ => {}
        }
    }

    fn load_feed_page(&mut self) {
        let Some(fan_id) = self.session.as_ref().map(|s| s.fan_id) else {
            self.status = "Log in (L) to see your feed.".to_owned();
            return;
        };
        let page = &mut self.feed.stories;
        if page.loading || (!page.more && page.started) {
            return;
        }
        page.loading = true;
        page.started = true;
        page.error = None;
        let older_than = page
            .next_token
            .as_deref()
            .and_then(|t| t.parse::<f64>().ok())
            .map(|t| t.floor() as i64)
            .unwrap_or_else(|| {
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0)
            });
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = feed_api::feed(&client, fan_id, older_than).await;
            let _ = tx.send(Message::FeedLoaded { generation, result });
        });
    }

    fn on_feed_loaded(&mut self, result: Result<FeedPage, ApiError>) {
        let page = &mut self.feed.stories;
        page.loading = false;
        match result {
            Ok(feed) => {
                tracing::info!(stories = feed.stories.len(), raw = feed.raw_entries, oldest = ?feed.oldest_story_date, "feed page");
                if feed.stories.is_empty() && feed.raw_entries > 0 {
                    page.error = Some(
                        "Feed entries were not understood; please share the debug log.".to_owned(),
                    );
                }
                let token = feed.oldest_story_date.map(|d| d.to_string());
                let more = !feed.stories.is_empty() || feed.raw_entries > 0;
                page.absorb(feed.stories, more, token);
            }
            Err(err) => {
                page.error = Some(err.user_message());
                self.status = format!("Feed: {}", err.user_message());
            }
        }
    }

    fn load_follow_page(&mut self, list: FollowList) {
        let Some(fan_id) = self.session.as_ref().map(|s| s.fan_id) else {
            self.status = "Log in (L) to see your feed.".to_owned();
            return;
        };
        let page = self.feed.follow_page_mut(list);
        if page.loading || (!page.more && page.started) {
            return;
        }
        page.loading = true;
        page.started = true;
        page.error = None;
        let token = page.next_token.clone().unwrap_or_else(fan::initial_token);
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = social::follow_list(&client, list, fan_id, &token, fan::PAGE_SIZE).await;
            let _ = tx.send(Message::FollowListLoaded {
                list,
                generation,
                result,
            });
        });
    }

    fn on_follow_list_loaded(&mut self, list: FollowList, result: Result<FollowPage, ApiError>) {
        self.feed.follow_page_mut(list).loading = false;
        match result {
            Ok(page) => {
                if list != FollowList::Followers {
                    for f in &page.followeers {
                        match (f.band_id, f.fan_id) {
                            (Some(band), _) => {
                                self.followed_bands.insert(band);
                            }
                            (None, Some(fan)) => {
                                self.followed_fans.insert(fan);
                            }
                            _ => {}
                        }
                    }
                }
                self.feed.follow_page_mut(list).absorb(
                    page.followeers,
                    page.more_available,
                    page.last_token,
                );
            }
            Err(err) => {
                self.feed.follow_page_mut(list).error = Some(err.user_message());
                self.status = format!("{}: {}", list.title(), err.user_message());
            }
        }
    }

    pub fn feed_move(&mut self, delta: isize) {
        match self.feed.tab.follow_list() {
            None => {
                let len = self.feed.visible_stories().len();
                self.feed.stories.move_within(len, delta);
                if self.feed.stories.wants_more(len) {
                    self.load_feed_page();
                }
            }
            Some(list) => {
                let page = self.feed.follow_page_mut(list);
                let len = page.items.len();
                page.move_within(len, delta);
                if page.wants_more(len) {
                    self.load_follow_page(list);
                }
            }
        }
    }

    pub fn feed_next_tab(&mut self) {
        self.feed.tab = self.feed.tab.next();
        self.ensure_feed_loaded();
    }

    pub fn reload_feed(&mut self) {
        match self.feed.tab.follow_list() {
            None => {
                self.feed.stories.reset();
                self.load_feed_page();
            }
            Some(list) => {
                self.feed.follow_page_mut(list).reset();
                self.load_follow_page(list);
            }
        }
    }

    /// Enter / p / a on the feed: stories open their album, people open in the browser.
    pub fn feed_open(&mut self, intent: Intent) {
        match self.feed.tab.follow_list() {
            None => {
                let Some(story) = self.feed.selected_story().cloned() else {
                    return;
                };
                match story.item {
                    Some(tralbum) => self.open_source(
                        AlbumSource {
                            tralbum,
                            title: story.item_title,
                            url: story.item_url.unwrap_or_default(),
                            section: Section::Feed,
                        },
                        intent,
                    ),
                    None => self.open_in_browser(story.item_url),
                }
            }
            Some(_) => {
                let followee = self.feed.selected_followee().cloned();
                match followee {
                    Some(f) if f.is_band() => {
                        if let Some(id) = f.band_id {
                            self.open_artist(id, f.name.clone(), f.url(), Section::Feed);
                        }
                    }
                    Some(_) => {
                        self.open_fan_on_screen();
                    }
                    None => {}
                }
            }
        }
    }

    pub fn feed_selected_url(&self) -> Option<String> {
        match self.feed.tab.follow_list() {
            None => self.feed.selected_story().and_then(|s| s.item_url.clone()),
            Some(_) => self.feed.selected_followee().and_then(|f| f.url()),
        }
    }

    /// The band (or fan) behind whatever is selected on screen.
    fn follow_target_on_screen(&self) -> Option<FollowTarget> {
        if self.album_open_here() {
            let view = self.album.as_ref()?;
            return Some(FollowTarget::Band(view.source.tralbum.band_id));
        }
        if self.artist_open_here() {
            return self.artist.as_ref().map(|a| FollowTarget::Band(a.band_id));
        }
        if self.fan_open_here() {
            return self.fan.as_ref().map(|f| FollowTarget::Fan(f.fan_id));
        }
        match self.section {
            Section::Collection => self
                .library
                .list()
                .selected_item()
                .map(|i| FollowTarget::Band(i.band_id)),
            Section::Discover => self
                .discover
                .selected()
                .map(|r| FollowTarget::Band(r.band_id)),
            Section::Feed => match self.feed.tab.follow_list() {
                None => self
                    .feed
                    .selected_story()
                    .and_then(|s| s.band_id)
                    .map(FollowTarget::Band),
                Some(_) => {
                    let f = self.feed.selected_followee()?;
                    match (f.band_id, f.fan_id) {
                        (Some(band), _) => Some(FollowTarget::Band(band)),
                        (None, Some(fan)) => Some(FollowTarget::Fan(fan)),
                        _ => None,
                    }
                }
            },
            Section::Search => self.search.selected().and_then(|r| {
                if r.is_fan() {
                    Some(FollowTarget::Fan(r.id))
                } else {
                    r.band().map(FollowTarget::Band)
                }
            }),
            Section::Daily => self
                .daily_selected_track()
                .map(|t| FollowTarget::Band(t.tralbum.band_id)),
        }
    }

    pub fn is_following(&self, target: FollowTarget) -> bool {
        match target {
            FollowTarget::Band(id) => self.followed_bands.contains(&id),
            FollowTarget::Fan(id) => self.followed_fans.contains(&id),
        }
    }

    pub fn toggle_follow(&mut self) {
        let Some(fan_id) = self.session.as_ref().map(|s| s.fan_id) else {
            self.status = "Log in (L) to follow artists and fans.".to_owned();
            return;
        };
        let Some(target) = self.follow_target_on_screen() else {
            self.status = "Nothing to follow here.".to_owned();
            return;
        };
        if let FollowTarget::Fan(id) = target
            && id == fan_id
        {
            self.status = "That is you.".to_owned();
            return;
        }
        let follow = !self.is_following(target);
        self.set_following(target, follow); // optimistic
        self.status = if follow {
            "Following…"
        } else {
            "Unfollowing…"
        }
        .to_owned();
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = social::set_follow(&client, fan_id, target, follow).await;
            let _ = tx.send(Message::FollowChanged {
                target,
                follow,
                result,
            });
        });
    }

    fn set_following(&mut self, target: FollowTarget, follow: bool) {
        let set = match target {
            FollowTarget::Band(_) => &mut self.followed_bands,
            FollowTarget::Fan(_) => &mut self.followed_fans,
        };
        let id = match target {
            FollowTarget::Band(id) | FollowTarget::Fan(id) => id,
        };
        if follow {
            set.insert(id);
        } else {
            set.remove(&id);
        }
    }

    fn on_follow_changed(
        &mut self,
        target: FollowTarget,
        follow: bool,
        result: Result<(), ApiError>,
    ) {
        match result {
            Ok(()) => {
                self.status = if follow { "Followed." } else { "Unfollowed." }.to_owned();
                if let Some(session) = self.session.as_mut()
                    && let FollowTarget::Band(_) = target
                {
                    session.following_count = if follow {
                        session.following_count + 1
                    } else {
                        session.following_count.saturating_sub(1)
                    };
                }
            }
            Err(err) => {
                self.set_following(target, !follow);
                tracing::warn!(?target, follow, %err, "follow change failed");
                self.status = format!("Follow change failed: {}", err.user_message());
            }
        }
    }

    /// The album or track behind whatever is selected on screen.
    fn wishlist_item_on_screen(&self) -> Option<TralbumRef> {
        if self.album_open_here() {
            return self.album.as_ref().map(|v| v.source.tralbum);
        }
        if self.artist_open_here() {
            return self.artist_selected_item().map(|i| i.tralbum());
        }
        if self.fan_open_here() {
            return self.fan.as_ref()?.list.selected_item().map(|i| i.tralbum());
        }
        match self.section {
            Section::Collection => self.library.list().selected_item().map(|i| i.tralbum()),
            Section::Discover => self.discover.selected().and_then(|r| r.tralbum()),
            Section::Feed => self.feed.selected_story().and_then(|s| s.item),
            Section::Search => self.search.selected().and_then(|r| r.tralbum()),
            Section::Daily => self.daily_selected_track().map(|t| t.tralbum),
        }
    }

    pub fn is_wishlisted(&self, item: TralbumRef) -> bool {
        self.wishlisted.contains(&(item.kind, item.id))
    }

    pub fn is_owned(&self, item: TralbumRef) -> bool {
        self.owned.contains(&(item.kind, item.id))
    }

    pub fn toggle_wishlist(&mut self) {
        let Some(fan_id) = self.session.as_ref().map(|s| s.fan_id) else {
            self.status = "Log in (L) to use the wishlist.".to_owned();
            return;
        };
        let Some(item) = self.wishlist_item_on_screen() else {
            self.status = "Nothing to wishlist here.".to_owned();
            return;
        };
        if self.is_owned(item) {
            self.status = "Already in your collection.".to_owned();
            return;
        }
        let wishlisted = !self.is_wishlisted(item);
        self.set_wishlisted(item, wishlisted); // optimistic
        self.status = if wishlisted {
            "Adding to wishlist…"
        } else {
            "Removing from wishlist…"
        }
        .to_owned();
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = social::set_wishlist(&client, fan_id, item, wishlisted).await;
            let _ = tx.send(Message::WishlistChanged {
                item,
                wishlisted,
                result,
            });
        });
    }

    fn set_wishlisted(&mut self, item: TralbumRef, wishlisted: bool) {
        if wishlisted {
            self.wishlisted.insert((item.kind, item.id));
        } else {
            self.wishlisted.remove(&(item.kind, item.id));
        }
    }

    fn on_wishlist_changed(
        &mut self,
        item: TralbumRef,
        wishlisted: bool,
        result: Result<(), ApiError>,
    ) {
        match result {
            Ok(()) => {
                self.status = if wishlisted {
                    "Added to your wishlist."
                } else {
                    "Removed from your wishlist."
                }
                .to_owned();
                // The wishlist listing is now stale; reload it next time it is shown.
                self.library.wishlist.reset();
                if self.section == Section::Collection && self.library.tab == ListKind::Wishlist {
                    self.load_more(ListKind::Wishlist);
                }
            }
            Err(err) => {
                self.set_wishlisted(item, !wishlisted);
                tracing::warn!(%item, wishlisted, %err, "wishlist change failed");
                self.status = format!("Wishlist change failed: {}", err.user_message());
            }
        }
    }

    // ----- search ------------------------------------------------------------------

    pub fn run_global_search(&mut self) {
        let text = self.search.input.value().trim().to_owned();
        self.search.input_open = false;
        if text.is_empty() {
            return;
        }
        self.search.query = text.clone();
        self.search.loading = true;
        self.search.error = None;
        self.search.request_id += 1;
        let request_id = self.search.request_id;
        let filter = self.search.filter;
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = search_api::search(&client, &text, filter).await;
            let _ = tx.send(Message::GlobalSearchLoaded { request_id, result });
        });
    }

    fn on_global_search_loaded(
        &mut self,
        request_id: u64,
        result: Result<SearchResponse, ApiError>,
    ) {
        if request_id != self.search.request_id {
            return;
        }
        self.search.loading = false;
        match result {
            Ok(response) => {
                let n = response.results.len();
                self.search.set_results(response.results, response.tags);
                self.status = format!("{n} results for \"{}\".", self.search.query);
            }
            Err(err) => {
                self.search.error = Some(err.user_message());
                self.status = format!("Search failed: {}", err.user_message());
            }
        }
    }

    pub fn cycle_search_filter(&mut self) {
        self.search.filter = self.search.filter.next();
        if !self.search.query.is_empty() {
            self.search.input = Input::new(self.search.query.clone());
            self.run_global_search();
        }
    }

    /// Enter / p / a on a search result.
    pub fn search_open(&mut self, intent: Intent) {
        let Some(result) = self.search.selected().cloned() else {
            return;
        };
        if let Some(tralbum) = result.tralbum() {
            let title = match &result.band_name {
                Some(band) => format!("{} — {}", band, result.name),
                None => result.name.clone(),
            };
            self.open_source(
                AlbumSource {
                    tralbum,
                    title,
                    url: result.url().unwrap_or_default(),
                    section: Section::Search,
                },
                intent,
            );
        } else if result.is_band() {
            self.open_artist(
                result.id,
                result.name.clone(),
                result.url(),
                Section::Search,
            );
        } else if result.is_fan() {
            self.open_fan(
                result.id,
                result.name.clone(),
                result.url(),
                Section::Search,
            );
        }
    }

    /// Jump to Discover with the first matching tag of the current search.
    pub fn search_tag_to_discover(&mut self) {
        let Some(tag) = self.search.tags.first().cloned() else {
            self.status = "No matching tag for this search.".to_owned();
            return;
        };
        self.discover.filters = discover::Filters::default();
        self.discover.filters.tags.push(tag);
        self.discover.started = true;
        self.discover_search();
        self.set_section(Section::Discover);
    }

    // ----- artist page -----------------------------------------------------------------

    pub fn open_artist(
        &mut self,
        band_id: u64,
        name: String,
        url: Option<String>,
        section: Section,
    ) {
        if self
            .album
            .as_ref()
            .is_some_and(|a| a.source.section == section)
        {
            self.album = None;
        }
        self.artist = Some(ArtistView {
            band_id,
            name,
            url,
            section,
            details: None,
            loading: true,
            error: None,
            roster: false,
            table: TableState::default(),
            roster_table: TableState::default(),
        });
        let client = self.client.clone();
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let result = band_api::band_details(&client, band_id).await;
            let _ = tx.send(Message::BandLoaded { band_id, result });
        });
    }

    fn on_band_loaded(&mut self, band_id: u64, result: Result<BandDetails, ApiError>) {
        let Some(view) = self.artist.as_mut().filter(|a| a.band_id == band_id) else {
            return;
        };
        view.loading = false;
        match result {
            Ok(details) => {
                view.name = details.name.clone();
                view.url = Some(details.bandcamp_url.clone());
                view.table
                    .select((!details.discography.is_empty()).then_some(0));
                view.roster_table
                    .select((!details.artists.is_empty()).then_some(0));
                view.details = Some(details);
            }
            Err(err) => view.error = Some(err.user_message()),
        }
    }

    /// The band behind whatever is selected, for `A`.
    fn artist_on_screen(&self) -> Option<(u64, String, Option<String>)> {
        if self.album_open_here() {
            let view = self.album.as_ref()?;
            let name = view
                .tralbum
                .as_ref()
                .map_or_else(|| view.source.title.clone(), |t| t.tralbum_artist.clone());
            return Some((view.source.tralbum.band_id, name, None));
        }
        if self.artist_open_here() {
            return self.artist_selected_item().map(|i| {
                let name = i.artist().unwrap_or_default().to_owned();
                (i.band_id, name, None)
            });
        }
        if self.fan_open_here() {
            let item = self.fan.as_ref()?.list.selected_item()?;
            return Some((item.band_id, item.band_name.clone(), item.band_url.clone()));
        }
        match self.section {
            Section::Collection => self
                .library
                .list()
                .selected_item()
                .map(|i| (i.band_id, i.band_name.clone(), i.band_url.clone())),
            Section::Discover => self
                .discover
                .selected()
                .map(|r| (r.band_id, r.band_name.clone(), r.band_url.clone())),
            Section::Feed => match self.feed.tab.follow_list() {
                None => self
                    .feed
                    .selected_story()
                    .and_then(|s| s.band_id.map(|id| (id, s.band_name.clone(), None))),
                Some(_) => self
                    .feed
                    .selected_followee()
                    .and_then(|f| f.band_id.map(|id| (id, f.name.clone(), f.url()))),
            },
            Section::Search => self.search.selected().and_then(|r| {
                r.band().map(|id| {
                    (
                        id,
                        r.band_name.clone().unwrap_or_else(|| r.name.clone()),
                        r.item_url_root.clone(),
                    )
                })
            }),
            Section::Daily => self
                .daily_selected_track()
                .map(|t| (t.tralbum.band_id, t.artist.clone(), None)),
        }
    }

    pub fn open_artist_on_screen(&mut self) {
        match self.artist_on_screen() {
            Some((band_id, name, url)) => {
                let section = self.section;
                self.open_artist(band_id, name, url, section);
            }
            None => self.status = "No artist selected.".to_owned(),
        }
    }

    pub fn artist_selected_item(&self) -> Option<&crate::api::band::DiscographyItem> {
        let view = self.artist.as_ref()?;
        if view.roster {
            return None;
        }
        let details = view.details.as_ref()?;
        view.table
            .selected()
            .and_then(|i| details.discography.get(i))
    }

    pub fn artist_move(&mut self, delta: isize) {
        let Some(view) = self.artist.as_mut() else {
            return;
        };
        let Some(details) = view.details.as_ref() else {
            return;
        };
        let (len, table) = if view.roster {
            (details.artists.len(), &mut view.roster_table)
        } else {
            (details.discography.len(), &mut view.table)
        };
        if len == 0 {
            return;
        }
        let current = table.selected().unwrap_or(0) as isize;
        table.select(Some(
            current.saturating_add(delta).clamp(0, len as isize - 1) as usize,
        ));
    }

    pub fn artist_toggle_roster(&mut self) {
        if let Some(view) = self.artist.as_mut()
            && view.details.as_ref().is_some_and(|d| d.is_label())
        {
            view.roster = !view.roster;
        }
    }

    /// Enter / p / a on the artist page.
    pub fn artist_open(&mut self, intent: Intent) {
        let Some(view) = self.artist.as_ref() else {
            return;
        };
        let section = view.section;
        if view.roster {
            let Some(details) = view.details.as_ref() else {
                return;
            };
            let Some(artist) = view
                .roster_table
                .selected()
                .and_then(|i| details.artists.get(i))
            else {
                return;
            };
            let (id, name) = (artist.id, artist.name.clone());
            self.open_artist(id, name, None, section);
            return;
        }
        let Some(item) = self.artist_selected_item() else {
            return;
        };
        let title = match item.artist() {
            Some(artist) if artist != view.name => format!("{artist} — {}", item.title),
            _ => item.title.clone(),
        };
        self.open_source(
            AlbumSource {
                tralbum: item.tralbum(),
                title,
                url: String::new(),
                section,
            },
            intent,
        );
    }

    // ----- fan page -----------------------------------------------------------------------

    pub fn open_fan(&mut self, fan_id: u64, name: String, url: Option<String>, section: Section) {
        if self
            .album
            .as_ref()
            .is_some_and(|a| a.source.section == section)
        {
            self.album = None;
        }
        if self.artist.as_ref().is_some_and(|a| a.section == section) {
            self.artist = None;
        }
        self.fan = Some(FanView {
            fan_id,
            name,
            url,
            section,
            list: ItemList::new(ListKind::Collection),
        });
        self.load_fan_page();
    }

    fn load_fan_page(&mut self) {
        let Some(view) = self.fan.as_mut() else {
            return;
        };
        let list = &mut view.list;
        if list.loading || (!list.more && list.started) {
            return;
        }
        list.loading = true;
        list.started = true;
        list.error = None;
        let token = list.next_token.clone().unwrap_or_else(fan::initial_token);
        let fan_id = view.fan_id;
        let client = self.client.clone();
        let tx = self.tx.clone();
        let generation = self.generation;
        tokio::spawn(async move {
            let result = fan::list_items(
                &client,
                ListKind::Collection,
                fan_id,
                &token,
                fan::PAGE_SIZE,
            )
            .await;
            let _ = tx.send(Message::FanPageLoaded {
                fan_id,
                generation,
                result,
            });
        });
    }

    fn on_fan_page_loaded(&mut self, fan_id: u64, result: Result<CollectionPage, ApiError>) {
        let Some(view) = self.fan.as_mut().filter(|f| f.fan_id == fan_id) else {
            return;
        };
        view.list.loading = false;
        match result {
            Ok(page) => view.list.absorb_page(page),
            Err(err) => view.list.error = Some(err.user_message()),
        }
    }

    pub fn fan_move(&mut self, delta: isize) {
        let Some(view) = self.fan.as_mut() else {
            return;
        };
        view.list.move_by(delta);
        if view.list.wants_more() {
            self.load_fan_page();
        }
    }

    /// Enter / p / a on a fan page item.
    pub fn fan_open(&mut self, intent: Intent) {
        let Some(view) = self.fan.as_ref() else {
            return;
        };
        let section = view.section;
        let Some(item) = view.list.selected_item().cloned() else {
            return;
        };
        self.open_source(
            AlbumSource {
                tralbum: item.tralbum(),
                title: item.item_title,
                url: item.item_url,
                section,
            },
            intent,
        );
    }

    /// Open the selected fan (feed lists, search) as a page instead of the browser.
    pub fn open_fan_on_screen(&mut self) -> bool {
        let section = self.section;
        let target = match self.section {
            Section::Feed => self
                .feed
                .selected_followee()
                .filter(|f| !f.is_band())
                .and_then(|f| f.fan_id.map(|id| (id, f.name.clone(), f.url()))),
            Section::Search => self
                .search
                .selected()
                .filter(|r| r.is_fan())
                .map(|r| (r.id, r.name.clone(), r.url())),
            _ => None,
        };
        match target {
            Some((id, name, url)) => {
                self.open_fan(id, name, url, section);
                true
            }
            None => false,
        }
    }

    // ----- queue overlay ---------------------------------------------------------

    pub fn toggle_queue(&mut self) {
        self.show_queue = !self.show_queue;
        if self.show_queue {
            self.queue_selected = self.queue.current.unwrap_or(0);
        }
    }

    pub fn queue_move(&mut self, delta: isize) {
        let len = self.queue.len();
        if len == 0 {
            return;
        }
        let next = (self.queue_selected as isize + delta).clamp(0, len as isize - 1);
        self.queue_selected = next as usize;
    }

    pub fn queue_remove_selected(&mut self) {
        let index = self.queue_selected;
        if index >= self.queue.len() {
            return;
        }
        let was_current = self.queue.current == Some(index);
        self.queue.remove(index);
        if was_current {
            self.stop_playback();
        }
        self.queue_selected = index.min(self.queue.len().saturating_sub(1));
    }

    pub fn queue_clear(&mut self) {
        self.stop_playback();
        self.queue.clear();
        self.queue_selected = 0;
        self.status = "Queue cleared.".to_owned();
    }
}

fn describe_secret_error(err: &crate::secrets::SecretError) -> String {
    match err.hint() {
        Some(hint) => format!("{err}. {hint}"),
        None => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{Band, Track, TralbumKind};
    use crate::secrets::FileStore;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn item(id: u64, band: &str, title: &str) -> CollectionItem {
        CollectionItem {
            item_id: id,
            tralbum_id: id,
            tralbum_type: "a".into(),
            band_id: 7,
            band_name: band.into(),
            item_title: title.into(),
            item_url: format!("https://x.bandcamp.com/album/{id}"),
            purchased: Some("19 Jun 2026 04:45:30 GMT".into()),
            num_streamable_tracks: Some(3),
            ..Default::default()
        }
    }

    fn render(app: &mut App) -> String {
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buffer.area.height {
            for x in 0..buffer.area.width {
                text.push_str(buffer[(x, y)].symbol());
            }
            text.push('\n');
        }
        text
    }

    #[tokio::test]
    async fn renders_library_album_and_queue() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-app-test"),
        ));
        let mut app = App::new(Config::default(), store, None, tx).unwrap();
        app.screen = Screen::Main;
        app.session = Some(Session {
            fan_id: 1,
            username: "tester".into(),
            url: "https://bandcamp.com/tester".into(),
            item_count: 2,
            following_count: 0,
            owned: Default::default(),
            following_bands: Default::default(),
            following_fans: Default::default(),
        });
        app.library.collection.started = true;
        app.on_page_loaded(
            ListKind::Collection,
            Ok(CollectionPage {
                items: vec![
                    item(1, "Primus", "A Handful of Nuggs"),
                    item(2, "Beats Antique", "A Thousand Faces"),
                ],
                more_available: false,
                ..Default::default()
            }),
        );
        let screen = render(&mut app);
        assert!(screen.contains("Primus"), "{screen}");
        assert!(screen.contains("2 of 2 loaded"), "{screen}");
        assert!(screen.contains("Nothing playing"), "{screen}");

        // Open the selected album and feed it details.
        app.open_selected(Intent::Show);
        assert!(app.album.as_ref().unwrap().loading);
        let tralbum = Tralbum {
            id: 1,
            title: "A Handful of Nuggs".into(),
            tralbum_artist: "Primus".into(),
            kind: "a".into(),
            bandcamp_url: "https://x.bandcamp.com/album/1".into(),
            band: Band {
                band_id: 7,
                name: "Primus".into(),
                location: Some("CA".into()),
            },
            tracks: vec![
                Track {
                    track_id: 11,
                    title: "Holy Diver".into(),
                    track_num: Some(1),
                    duration: 256.7,
                    is_streamable: true,
                    streaming_url: [("mp3-128".to_owned(), "https://example.invalid/a".to_owned())]
                        .into(),
                    ..Default::default()
                },
                Track {
                    track_id: 12,
                    title: "Silent".into(),
                    track_num: Some(2),
                    duration: 10.0,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let tralbum_ref = TralbumRef {
            band_id: 7,
            id: 1,
            kind: TralbumKind::Album,
        };
        app.on_tralbum_loaded(tralbum_ref, Intent::Show, Ok(tralbum.clone()));
        let screen = render(&mut app);
        assert!(screen.contains("Holy Diver"), "{screen}");
        assert!(screen.contains("2 tracks"), "{screen}");

        // Play from the album view: queue gets both tracks, playback starts loading.
        app.album_move(1);
        app.play_album_from_selection();
        assert_eq!(app.queue.len(), 2);
        // Track 2 is not streamable, so the app skips to the end of the queue.
        assert_eq!(app.playback.state, PlaybackState::Idle);
        app.play_album(None);
        assert_eq!(app.playback.state, PlaybackState::Loading);
        assert_eq!(app.playback.track_id, Some(11));
        app.toggle_queue();
        let screen = render(&mut app);
        assert!(screen.contains("Queue · 2 tracks"), "{screen}");

        // A failed stream refreshes URLs once, then gives up.
        app.on_player_event(PlayerEvent::Failed {
            track_id: 11,
            message: "403".into(),
        });
        assert!(app.status.contains("refreshing"));
        app.on_tralbum_loaded(
            tralbum_ref,
            Intent::Refresh {
                queue_index: 0,
                after_failure: true,
            },
            Ok(tralbum),
        );
        assert!(app.queue.tracks[0].retried);
        app.on_player_event(PlayerEvent::Failed {
            track_id: 11,
            message: "403".into(),
        });
        assert_eq!(app.playback.state, PlaybackState::Idle);
        assert!(app.status.contains("Playback failed"));
    }

    #[tokio::test]
    async fn renders_discover_with_picker_and_tags() {
        use crate::api::discover::{DiscoverResult, FeaturedPreview};
        use tui_input::InputRequest;

        let (tx, _rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-app-test-2"),
        ));
        let mut app = App::new(Config::default(), store, None, tx).unwrap();
        app.screen = Screen::Main;
        app.set_section(Section::Discover);
        assert!(app.discover.started && app.discover.loading);
        let request_id = app.discover.request_id;
        app.on_discover_loaded(
            request_id,
            Ok(DiscoverResponse {
                results: vec![
                    DiscoverResult {
                        result_type: "a".into(),
                        item_type: "a".into(),
                        item_id: 824574161,
                        band_id: 2632533392,
                        title: "Alien Metal".into(),
                        band_name: "King Gizzard".into(),
                        band_location: Some("Melbourne, Australia".into()),
                        release_date: Some("2026-08-14 04:00:52 UTC".into()),
                        track_count: Some(8),
                        item_url:
                            "https://kinggizzard.bandcamp.com/album/alien-metal?from=discover_page"
                                .into(),
                        featured_track: Some(FeaturedPreview {
                            title: "Kill".into(),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    DiscoverResult {
                        result_type: "s".into(),
                        item_type: "p".into(),
                        item_id: 5,
                        title: "Shirt".into(),
                        band_name: "Zomby".into(),
                        item_url: "https://z.bandcamp.com/merch/shirt".into(),
                        ..Default::default()
                    },
                ],
                result_count: Some(1942109),
                cursor: Some("AoMI".into()),
            }),
        );
        let screen = render(&mut app);
        assert!(screen.contains("1,942,109 results"), "{screen}");
        assert!(screen.contains("King Gizzard"), "{screen}");
        assert!(screen.contains("merch"), "{screen}");
        assert!(screen.contains("featured: Kill"), "{screen}");

        // Picker: field menu → genre list → filter → apply → new search.
        app.open_picker(None);
        let screen = render(&mut app);
        assert!(screen.contains("Filters"), "{screen}");
        app.picker_enter(); // Genre
        for ch in "elect".chars() {
            app.picker_input(InputRequest::InsertChar(ch));
        }
        let screen = render(&mut app);
        assert!(screen.contains("electronic"), "{screen}");
        app.picker_enter();
        assert!(app.discover.picker.is_none());
        assert_eq!(app.discover.filters.genre.as_deref(), Some("electronic"));
        assert!(
            app.discover.results.is_empty(),
            "filters changed, results reset"
        );
        assert!(app.discover.request_id > request_id);

        // Tag entry: typed text becomes a slug and triggers a search.
        app.open_tag_entry();
        for ch in "Dark Ambient".chars() {
            app.tag_entry_input(InputRequest::InsertChar(ch));
        }
        let screen = render(&mut app);
        assert!(screen.contains("Add tag: Dark Ambient"), "{screen}");
        app.tag_entry_enter();
        assert_eq!(app.discover.filters.tags, vec!["dark-ambient".to_owned()]);
        let screen = render(&mut app);
        assert!(screen.contains("+dark-ambient"), "{screen}");
        app.remove_last_tag();
        assert!(app.discover.filters.tags.is_empty());

        // Opening a music result creates an album view in the Discover section.
        let request_id = app.discover.request_id;
        app.on_discover_loaded(
            request_id,
            Ok(DiscoverResponse {
                results: vec![DiscoverResult {
                    result_type: "a".into(),
                    item_type: "a".into(),
                    item_id: 1,
                    band_id: 2,
                    title: "X".into(),
                    band_name: "Y".into(),
                    item_url: "https://y.bandcamp.com/album/x".into(),
                    ..Default::default()
                }],
                result_count: Some(1),
                cursor: None,
            }),
        );
        app.discover_open(Intent::Show);
        assert!(app.album_open_here());
        assert_eq!(
            app.album.as_ref().unwrap().source.url,
            "https://y.bandcamp.com/album/x"
        );
        app.set_section(Section::Collection);
        assert!(!app.album_open_here());
    }

    #[tokio::test]
    async fn renders_feed_and_toggles_follow_and_wishlist() {
        use crate::api::feed::{Story, StoryKind};
        use crate::api::social::Followee;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-app-test-3"),
        ));
        let mut app = App::new(Config::default(), store, None, tx).unwrap();
        app.screen = Screen::Main;
        app.session = Some(Session {
            fan_id: 1,
            username: "tester".into(),
            url: "https://bandcamp.com/tester".into(),
            item_count: 0,
            following_count: 1,
            owned: Default::default(),
            following_bands: [7].into(),
            following_fans: Default::default(),
        });
        app.followed_bands.insert(7);
        app.set_section(Section::Feed);
        assert!(app.feed.stories.loading);
        let story = Story {
            kind: StoryKind::NewRelease,
            story_type: "nr".into(),
            date: None,
            band_id: Some(7),
            band_name: "Primus".into(),
            item: Some(TralbumRef {
                band_id: 7,
                id: 1,
                kind: TralbumKind::Album,
            }),
            item_title: "A Handful of Nuggs".into(),
            item_url: Some("https://primusband.bandcamp.com/album/nuggs".into()),
            fan_name: None,
            fan_id: None,
            why: None,
            also_collected_count: Some(794),
        };
        app.on_feed_loaded(Ok(FeedPage {
            stories: vec![story],
            oldest_story_date: Some(1.0),
            raw_entries: 1,
        }));
        let screen = render(&mut app);
        assert!(screen.contains("Primus — A Handful of Nuggs"), "{screen}");
        assert!(screen.contains("✓ following"), "{screen}");

        // Wishlist toggle is optimistic and reverts on failure.
        app.toggle_wishlist();
        assert!(app.is_wishlisted(TralbumRef {
            band_id: 7,
            id: 1,
            kind: TralbumKind::Album
        }));
        let screen = render(&mut app);
        assert!(screen.contains("♥ wishlisted"), "{screen}");
        while let Ok(message) = rx.try_recv() {
            if let Message::WishlistChanged {
                item, wishlisted, ..
            } = message
            {
                app.on_wishlist_changed(item, wishlisted, Err(ApiError::NotLoggedIn));
            }
        }
        // (the real reply may still be in flight; simulate the failure explicitly)
        app.on_wishlist_changed(
            TralbumRef {
                band_id: 7,
                id: 1,
                kind: TralbumKind::Album,
            },
            true,
            Err(ApiError::NotLoggedIn),
        );
        assert!(!app.is_wishlisted(TralbumRef {
            band_id: 7,
            id: 1,
            kind: TralbumKind::Album
        }));
        assert!(app.status.contains("Wishlist change failed"));

        // Unfollow the story's band, then the reply confirms it.
        app.toggle_follow();
        assert!(!app.followed_bands.contains(&7));
        app.on_follow_changed(FollowTarget::Band(7), false, Ok(()));
        assert_eq!(app.session.as_ref().unwrap().following_count, 0);
        assert_eq!(app.status, "Unfollowed.");

        // Following lists.
        app.feed_next_tab();
        assert_eq!(app.feed.tab, feed::FeedTab::Bands);
        assert!(app.feed.bands.loading);
        app.on_follow_list_loaded(
            FollowList::Bands,
            Ok(FollowPage {
                followeers: vec![Followee {
                    band_id: Some(9),
                    name: "Bob Marley".into(),
                    location: Some("Nine Mile".into()),
                    ..Default::default()
                }],
                more_available: false,
                last_token: None,
            }),
        );
        assert!(app.followed_bands.contains(&9));
        let screen = render(&mut app);
        assert!(screen.contains("Artists you follow"), "{screen}");
        assert!(screen.contains("Bob Marley"), "{screen}");
        assert!(screen.contains("Nine Mile"), "{screen}");
    }

    #[tokio::test]
    async fn renders_search_artist_and_fan_pages() {
        use crate::api::band::{DiscographyItem, LabelArtist};
        use crate::api::search::SearchResult;
        use tui_input::InputRequest;

        let (tx, _rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-app-test-4"),
        ));
        let mut app = App::new(Config::default(), store, None, tx).unwrap();
        app.screen = Screen::Main;
        app.set_section(Section::Search);
        for ch in "gizzard".chars() {
            app.search.input.handle(InputRequest::InsertChar(ch));
        }
        app.run_global_search();
        assert!(app.search.loading && !app.search.input_open);
        let request_id = app.search.request_id;
        app.on_global_search_loaded(
            request_id,
            Ok(SearchResponse {
                results: vec![
                    SearchResult {
                        kind: "b".into(),
                        id: 256015751,
                        name: "p(doom)".into(),
                        location: Some("Melbourne".into()),
                        is_label: true,
                        item_url_root: Some("https://pdoomrecords.bandcamp.com".into()),
                        ..Default::default()
                    },
                    SearchResult {
                        kind: "a".into(),
                        id: 1,
                        name: "Alien Metal".into(),
                        band_id: Some(2),
                        band_name: Some("King Gizzard".into()),
                        ..Default::default()
                    },
                    SearchResult {
                        kind: "f".into(),
                        id: 10162377,
                        name: "The Seahorse".into(),
                        collection_size: Some(65),
                        item_url_root: Some("https://bandcamp.com/theseahorse".into()),
                        ..Default::default()
                    },
                ],
                tags: vec!["gizzard".into()],
            }),
        );
        let screen = render(&mut app);
        assert!(screen.contains("3 results"), "{screen}");
        assert!(screen.contains("label"), "{screen}");
        assert!(screen.contains("tags: gizzard"), "{screen}");

        // A label result opens an artist page with a roster.
        app.search_open(Intent::Show);
        assert!(app.artist_open_here());
        app.on_band_loaded(
            256015751,
            Ok(BandDetails {
                id: 256015751,
                name: "p(doom)".into(),
                bandcamp_url: "https://pdoomrecords.bandcamp.com".into(),
                discography: vec![DiscographyItem {
                    item_id: 824574161,
                    item_type: "album".into(),
                    title: "Alien Metal".into(),
                    band_id: 2632533392,
                    artist_name: Some("King Gizzard".into()),
                    release_date: Some("14 Aug 2026 04:00:52 GMT".into()),
                    ..Default::default()
                }],
                artists: vec![LabelArtist {
                    id: 2632533392,
                    name: "King Gizzard".into(),
                    location: Some("Melbourne".into()),
                    ..Default::default()
                }],
                ..Default::default()
            }),
        );
        let screen = render(&mut app);
        assert!(screen.contains("1 releases  ·  1 artists"), "{screen}");
        assert!(screen.contains("2026-08-14"), "{screen}");
        app.artist_toggle_roster();
        let screen = render(&mut app);
        assert!(screen.contains("Enter opens the artist"), "{screen}");
        app.artist_toggle_roster();

        // Enter on a release opens the album on top; Esc returns to the artist.
        app.artist_open(Intent::Show);
        assert!(app.album_open_here() && !app.artist_open_here());
        assert_eq!(
            app.album.as_ref().unwrap().source.title,
            "King Gizzard — Alien Metal"
        );
        assert!(app.close_top_view());
        assert!(app.artist_open_here());
        assert!(app.close_top_view());
        assert!(!app.artist_open_here());

        // A fan result opens their public collection.
        app.search.table.select(Some(2));
        app.search_open(Intent::Show);
        assert!(app.fan_open_here());
        app.on_fan_page_loaded(
            10162377,
            Ok(CollectionPage {
                items: vec![item(9, "Primus", "A Handful of Nuggs")],
                more_available: false,
                ..Default::default()
            }),
        );
        let screen = render(&mut app);
        assert!(screen.contains("The Seahorse's collection"), "{screen}");
        assert!(screen.contains("Primus"), "{screen}");
        // `A` on a fan's item opens that artist page over the fan page.
        app.open_artist_on_screen();
        assert!(app.artist_open_here() && !app.fan_open_here());
        assert_eq!(app.artist.as_ref().unwrap().band_id, 7);
    }

    /// Drives the real Discover endpoints. Needs network: `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn discover_live_round_trip() {
        use tui_input::InputRequest;

        let (tx, mut rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-app-test-live"),
        ));
        let mut app = App::new(Config::default(), store, None, tx).unwrap();
        app.screen = Screen::Main;
        app.set_section(Section::Discover);

        // Pump messages until a condition holds (or we give up).
        async fn pump(
            app: &mut App,
            rx: &mut mpsc::UnboundedReceiver<Message>,
            until: impl Fn(&App) -> bool,
        ) {
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            while !until(app) {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                assert!(!remaining.is_zero(), "timed out; status: {}", app.status);
                match tokio::time::timeout(remaining, rx.recv()).await {
                    Ok(Some(message)) => app.handle(message),
                    _ => panic!("channel closed"),
                }
            }
        }

        pump(&mut app, &mut rx, |a| {
            a.discover.options_live && !a.discover.results.is_empty()
        })
        .await;
        assert!(app.discover.total.unwrap_or(0) > 1000);
        assert!(app.discover.results.iter().all(|r| !r.title.is_empty()));
        assert!(app.discover.more);

        // Genre + subgenre filter, related tags arrive.
        let electronic = app
            .discover
            .picker_options(Some(Field::Genre))
            .into_iter()
            .find(|o| o.key == "electronic")
            .unwrap();
        assert!(app.discover.apply(Field::Genre, &electronic));
        app.discover_search();
        pump(&mut app, &mut rx, |a| {
            !a.discover.results.is_empty() && !a.discover.related.is_empty()
        })
        .await;
        eprintln!("related to electronic: {:?}", app.discover.related);

        // Location search through the picker.
        app.open_picker(Some(Field::Location));
        for ch in "reykjav".chars() {
            app.picker_input(InputRequest::InsertChar(ch));
        }
        pump(&mut app, &mut rx, |a| {
            a.discover
                .picker
                .as_ref()
                .is_some_and(|p| !p.visible().is_empty())
        })
        .await;
        let label = app
            .discover
            .picker
            .as_ref()
            .unwrap()
            .selected_option()
            .unwrap()
            .label
            .clone();
        eprintln!("location match: {label}");
        assert!(label.to_lowercase().contains("reykjav"));
        app.picker_enter();
        assert_ne!(app.discover.filters.geoname_id, 0);
        pump(&mut app, &mut rx, |a| !a.discover.loading).await;
        eprintln!(
            "results in {}: {:?}",
            app.discover.filters.location_label, app.discover.total
        );

        // Tag autocomplete.
        app.open_tag_entry();
        for ch in "ambi".chars() {
            app.tag_entry_input(InputRequest::InsertChar(ch));
        }
        pump(&mut app, &mut rx, |a| {
            a.discover
                .tag_entry
                .as_ref()
                .is_some_and(|e| e.suggestions_for == "ambi")
        })
        .await;
        let names: Vec<String> = app
            .discover
            .tag_entry
            .as_ref()
            .unwrap()
            .suggestions
            .iter()
            .map(|s| s.norm_name.clone())
            .collect();
        eprintln!("suggestions: {names:?}");
        assert!(names.contains(&"ambient".to_owned()));

        // Open the first result's album details for real.
        app.discover.tag_entry = None;
        app.reset_discover_filters();
        pump(&mut app, &mut rx, |a| {
            !a.discover.results.is_empty() && !a.discover.loading
        })
        .await;
        app.discover_open(Intent::Show);
        pump(&mut app, &mut rx, |a| {
            a.album.as_ref().is_some_and(|v| !v.loading)
        })
        .await;
        let view = app.album.as_ref().unwrap();
        assert!(view.error.is_none(), "{:?}", view.error);
        let tralbum = view.tralbum.as_ref().unwrap();
        eprintln!(
            "album: {} — {} ({} tracks)",
            tralbum.tralbum_artist,
            tralbum.title,
            tralbum.tracks.len()
        );
        assert!(!tralbum.tracks.is_empty());
    }

    #[tokio::test]
    async fn restores_the_saved_queue_and_refreshes_its_urls() {
        let dir =
            std::env::temp_dir().join(format!("bandcamp-tui-app-test-{}", std::process::id()));
        let queue_store = Arc::new(QueueStore::new(dir.join("queue.json")));
        let tralbum_ref = TralbumRef {
            band_id: 7,
            id: 1,
            kind: TralbumKind::Album,
        };
        let stale = |id: u64, title: &str| crate::player::QueuedTrack {
            track_id: id,
            title: title.into(),
            artist: "Primus".into(),
            album: Some("A Handful of Nuggs".into()),
            duration: Duration::from_secs(200),
            url: Some("https://example.invalid/expired".into()),
            tralbum: tralbum_ref,
            art_id: None,
            page_url: None,
            retried: false,
        };
        queue_store
            .save(&SavedQueue {
                tracks: vec![stale(11, "Holy Diver"), stale(12, "Silent")],
                current: Some(1),
                ..Default::default()
            })
            .unwrap();

        let (tx, _rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(dir.join("identity")));
        let mut app = App::new(Config::default(), store, Some(queue_store.clone()), tx).unwrap();
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.current, Some(1));
        assert_eq!(app.queue_selected, 1);
        assert_eq!(app.playback.state, PlaybackState::Idle);
        // Nothing changed yet, so nothing is written.
        assert!(app.persist_queue().is_none());

        // Play resumes the current entry, but fetches fresh URLs before loading it.
        app.toggle_pause();
        assert_eq!(app.playback.state, PlaybackState::Loading);
        assert_eq!(app.playback.track_id, Some(12));
        assert!(app.stale_tralbums.is_empty());
        let tralbum = Tralbum {
            id: 1,
            title: "A Handful of Nuggs".into(),
            tralbum_artist: "Primus".into(),
            kind: "a".into(),
            band: Band {
                band_id: 7,
                name: "Primus".into(),
                location: None,
            },
            tracks: vec![
                Track {
                    track_id: 11,
                    title: "Holy Diver".into(),
                    duration: 200.0,
                    is_streamable: true,
                    streaming_url: [(
                        "mp3-128".to_owned(),
                        "https://example.invalid/fresh-a".to_owned(),
                    )]
                    .into(),
                    ..Default::default()
                },
                Track {
                    track_id: 12,
                    title: "Silent".into(),
                    duration: 200.0,
                    is_streamable: true,
                    streaming_url: [(
                        "mp3-128".to_owned(),
                        "https://example.invalid/fresh-b".to_owned(),
                    )]
                    .into(),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        app.on_tralbum_loaded(
            tralbum_ref,
            Intent::Refresh {
                queue_index: 1,
                after_failure: false,
            },
            Ok(tralbum),
        );
        assert_eq!(app.playback.state, PlaybackState::Loading);
        assert_eq!(app.playback.track_id, Some(12));
        assert_eq!(
            app.queue.tracks[0].url.as_deref(),
            Some("https://example.invalid/fresh-a")
        );
        // A proactive refresh does not spend the one failure retry.
        assert!(!app.queue.tracks[1].retried);

        // The refreshed URLs reach the disk, and clearing the queue removes the file.
        app.persist_queue().unwrap().await.unwrap();
        let on_disk = queue_store.load().unwrap().unwrap();
        assert_eq!(
            on_disk.tracks[1].url.as_deref(),
            Some("https://example.invalid/fresh-b")
        );
        assert_eq!(on_disk.current, Some(1));
        app.queue_clear();
        app.persist_queue().unwrap().await.unwrap();
        assert_eq!(queue_store.load().unwrap(), None);
    }

    #[tokio::test]
    async fn renders_daily_articles_and_plays_featured_tracks() {
        use crate::api::daily::{Article, ArticleBlock, ArticleDetail, ArticlePage, FeaturedTrack};
        use ratatui::crossterm::event::{KeyCode, KeyEvent};

        let (tx, mut rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-app-test-daily"),
        ));
        let mut app = App::new(Config::default(), store, None, tx).unwrap();
        app.screen = Screen::Main;
        app.set_section(Section::Daily);
        assert!(app.daily.articles.loading);
        let article = Article {
            url: "https://daily.bandcamp.com/lists/hardanger-fiddle-album-guide".into(),
            title: "The Secret Life of the Hardanger Fiddle".into(),
            franchise: "Lists".into(),
            date: chrono::NaiveDate::from_ymd_opt(2026, 9, 17),
            art_url: None,
        };
        app.on_daily_list_loaded(Ok(ArticlePage {
            articles: vec![article],
            next_page: Some(2),
        }));
        assert_eq!(app.daily.next_page(), 2);
        let screen = render(&mut app);
        assert!(
            screen.contains("The Secret Life of the Hardanger Fiddle"),
            "{screen}"
        );
        assert!(screen.contains("Lists"), "{screen}");
        assert!(screen.contains("1 articles+"), "{screen}");

        // Enter fetches the article; the reply fills the featured tracks and the text.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(app.daily.view.as_ref().unwrap().loading);
        let album = TralbumRef {
            band_id: 42,
            id: 1377959249,
            kind: TralbumKind::Album,
        };
        let track = |id: u64, title: &str, url: Option<&str>| FeaturedTrack {
            track_id: id,
            title: title.into(),
            artist: "Spelemenn frå Agder".into(),
            album: "Ein gammel ein".into(),
            tralbum: album,
            url: "https://nonegeland.bandcamp.com/album/ein-gammel-ein".into(),
            art_id: Some(1),
            duration: Duration::from_secs(171),
            stream_url: url.map(str::to_owned),
            track_number: 8,
        };
        app.on_daily_article_loaded(
            "https://daily.bandcamp.com/lists/hardanger-fiddle-album-guide".into(),
            Ok(ArticleDetail {
                blurb: Some("Norway's most striking folk instrument.".into()),
                author: Some("Peter Margasak".into()),
                body: vec![
                    ArticleBlock::Paragraph("First paragraph of the guide.".into()),
                    ArticleBlock::Heading("A heading".into()),
                ],
                featured: vec![
                    track(
                        2812227069,
                        "Sørensens vals",
                        Some("https://t4.bcbits.com/stream/a/mp3-128/2812227069?token=x"),
                    ),
                    track(3599636109, "Drifting Like A Bird", None),
                ],
            }),
        );
        let screen = render(&mut app);
        assert!(screen.contains("by Peter Margasak"), "{screen}");
        assert!(
            screen.contains("Norway's most striking folk instrument."),
            "{screen}"
        );
        assert!(screen.contains("Sørensens vals"), "{screen}");
        assert!(screen.contains("Ein gammel ein"), "{screen}");
        assert!(screen.contains("2:51"), "{screen}");
        assert!(screen.contains("First paragraph of the guide."), "{screen}");
        assert!(screen.contains("Tracks · 2"), "{screen}");
        assert_eq!(
            app.daily_selected_url().as_deref(),
            Some("https://nonegeland.bandcamp.com/album/ein-gammel-ein")
        );
        assert_eq!(app.wishlist_item_on_screen(), Some(album));

        // `p` plays straight from the embedded URL: no album fetch, queue = the article.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('p')));
        assert_eq!(app.queue.len(), 2);
        assert_eq!(app.queue.current, Some(0));
        assert_eq!(app.playback.track_id, Some(2812227069));
        assert_eq!(app.playback.state, PlaybackState::Loading);
        assert!(
            app.status
                .contains("Loading Sørensens vals — Spelemenn frå Agder"),
            "{}",
            app.status
        );
        while let Ok(message) = rx.try_recv() {
            assert!(
                !matches!(message, Message::TralbumLoaded { .. }),
                "playing a featured track must not fetch the album"
            );
        }

        // `a` on the second row appends it; playback is already busy so it just queues.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('j')));
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('a')));
        assert_eq!(app.queue.len(), 3);
        assert_eq!(app.queue.tracks[2].url, None);
        assert_eq!(app.status, "Queued Drifting Like A Bird.");

        // `l` opens the album view for the track's album, Esc returns to the article.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('l')));
        assert!(app.album_open_here());
        assert_eq!(app.album.as_ref().unwrap().source.tralbum, album);
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(!app.album_open_here());
        assert!(app.daily.view.is_some());

        // `w` moves to the text; `o` then points at the article; Esc goes back to the list.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('w')));
        assert_eq!(
            app.daily_selected_url().as_deref(),
            Some("https://daily.bandcamp.com/lists/hardanger-fiddle-album-guide")
        );
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert!(app.daily.view.is_none());

        // Switching section resets and reloads the list.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('v')));
        assert_eq!(
            app.daily.franchise,
            crate::api::daily::Franchise::AlbumOfTheDay
        );
        assert!(app.daily.articles.items.is_empty());
        assert!(app.daily.articles.loading);
    }

    fn test_session() -> Session {
        Session {
            fan_id: 1,
            username: "tester".into(),
            url: "https://bandcamp.com/tester".into(),
            item_count: 2,
            following_count: 0,
            owned: Default::default(),
            following_bands: Default::default(),
            following_fans: Default::default(),
        }
    }

    fn anonymous_app() -> App {
        let (tx, _rx) = mpsc::unbounded_channel();
        let store = Arc::new(FileStore::new(
            std::env::temp_dir().join("bandcamp-tui-anon-test"),
        ));
        // The username matches the test session so a login never rewrites the config file.
        let config = Config {
            username: Some("tester".into()),
            ..Config::default()
        };
        App::new(config, store, None, tx).unwrap()
    }

    #[tokio::test]
    async fn anonymous_boot_lands_on_discover() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let mut app = anonymous_app();
        app.on_cookie_loaded(Ok(None));
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.section, Section::Discover);
        assert!(app.session.is_none());
        assert_eq!(app.status, "Not logged in. Press L to log in.");
        let screen = render(&mut app);
        assert!(screen.contains("not logged in (L)"), "{screen}");

        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('1')));
        assert_eq!(app.section, Section::Collection);
        let screen = render(&mut app);
        assert!(
            screen.contains("Log in (L) to see your collection."),
            "{screen}"
        );
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('R')));
        assert_eq!(app.status, "Log in (L) to see your collection.");

        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('3')));
        assert_eq!(app.status, "Log in (L) to see your feed.");
        let screen = render(&mut app);
        assert!(screen.contains("Log in (L) to see your feed."), "{screen}");
        assert!(screen.contains("L log in"), "{screen}");

        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('F')));
        assert!(app.status.starts_with("Log in (L)"), "{}", app.status);
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('W')));
        assert!(app.status.starts_with("Log in (L)"), "{}", app.status);

        // Public sections stay usable.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('5')));
        assert_eq!(app.section, Section::Daily);
        assert!(app.daily.articles.loading);
    }

    #[tokio::test]
    async fn login_page_round_trip() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        let mut app = anonymous_app();
        app.on_cookie_loaded(Ok(None));
        // Not Search: its query box is open by default and would swallow the key.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('5')));

        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('L')));
        assert_eq!(app.screen, Screen::Login);
        let screen = render(&mut app);
        assert!(screen.contains("Log in to Bandcamp"), "{screen}");
        assert!(screen.contains("playback need no login."), "{screen}");

        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.section, Section::Daily);

        // Log in from the page; the reply lands on the main screen.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('L')));
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('c')));
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Enter));
        assert!(app.login.busy);
        app.on_session_checked(SessionCheck::Login("c".into()), Ok(test_session()));
        assert_eq!(app.screen, Screen::Main);
        assert!(app.session.is_some());
        assert!(app.library.collection.loading);

        // A rejected paste no longer drags the user back to the login page.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('5')));
        app.on_session_checked(
            SessionCheck::Login("bad".into()),
            Err(ApiError::NotLoggedIn),
        );
        assert_eq!(app.screen, Screen::Main);
        assert!(app.login.error.is_some());

        // Logging out keeps the queue and stays on the main screen.
        // (`logout` additionally clears the cookie and writes the config file.)
        app.queue.append(vec![crate::player::QueuedTrack {
            track_id: 11,
            title: "Holy Diver".into(),
            artist: "Primus".into(),
            album: None,
            duration: Duration::from_secs(200),
            url: Some("https://example.invalid/stream".into()),
            tralbum: TralbumRef {
                band_id: 7,
                id: 1,
                kind: TralbumKind::Album,
            },
            art_id: None,
            page_url: None,
            retried: false,
        }]);
        app.forget_session();
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.section, Section::Daily);
        assert!(app.session.is_none());
        assert_eq!(app.queue.len(), 1);
        let screen = render(&mut app);
        assert!(screen.contains("not logged in (L)"), "{screen}");
    }

    #[tokio::test]
    async fn boot_failures_stay_anonymous() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent};
        // A dead stored cookie.
        let mut app = anonymous_app();
        app.screen = Screen::Booting;
        app.on_session_checked(
            SessionCheck::Boot("dead".into()),
            Err(ApiError::NotLoggedIn),
        );
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.section, Section::Discover);
        assert!(app.session.is_none());
        assert!(app.login.error.is_some());
        assert!(app.status.starts_with("Session expired"), "{}", app.status);

        // Network trouble: the cookie waits in the login field.
        let mut app = anonymous_app();
        app.screen = Screen::Booting;
        app.on_session_checked(
            SessionCheck::Boot("c".into()),
            Err(ApiError::Bandcamp("down".into())),
        );
        assert_eq!(app.screen, Screen::Main);
        assert_eq!(app.section, Section::Discover);
        assert_eq!(app.login.input.value(), "c");
        assert!(app.login.hint.is_some());
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('L')));
        assert_eq!(app.screen, Screen::Login);
        let screen = render(&mut app);
        assert!(screen.contains("retry with the saved cookie"), "{screen}");
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Esc));
        // `r` retries with that cookie.
        keys::handle_key(&mut app, KeyEvent::from(KeyCode::Char('r')));
        assert!(app.login.busy);
    }
}
