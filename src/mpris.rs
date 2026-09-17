//! MPRIS2: publish playback on the D-Bus session bus and take remote controls.
//!
//! `mpris_server::Player` is `Rc`-based, so it lives on its own thread with a
//! current-thread runtime. The UI thread describes its state as a `Snapshot`
//! after every message batch; only the fields that changed cross the channel.
//! Remote calls come back as `Message::Mpris`.

use std::thread::{self, JoinHandle};
use std::time::Duration;

use mpris_server::zbus::fdo::DBusProxy;
use mpris_server::zbus::{self, Connection};
use mpris_server::{Metadata, PlaybackStatus, Player, Time, TrackId};
use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};

use crate::app::Message;

pub const BUS_SUFFIX: &str = "bandcamp_tui";
const TRACK_PATH_PREFIX: &str = "/io/github/bandcamp_tui/track/";
/// Position jumps beyond this on the same track are reported as seeks.
const SEEK_THRESHOLD: Duration = Duration::from_millis(1500);

/// What the desktop shows for the current track.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackMeta {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Duration,
    pub art_url: Option<String>,
    pub page_url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Status {
    Playing,
    Paused,
    #[default]
    Stopped,
}

impl From<Status> for PlaybackStatus {
    fn from(status: Status) -> Self {
        match status {
            Status::Playing => PlaybackStatus::Playing,
            Status::Paused => PlaybackStatus::Paused,
            Status::Stopped => PlaybackStatus::Stopped,
        }
    }
}

/// Everything MPRIS exposes; cheap to build after every message batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Snapshot {
    pub track_id: Option<u64>,
    pub status: Status,
    pub position: Duration,
    /// Percent, like `Config::volume`.
    pub volume: u8,
    pub can_next: bool,
    pub can_prev: bool,
    pub can_play: bool,
    pub can_pause: bool,
}

#[derive(Debug, Clone, PartialEq)]
enum Update {
    Metadata(Option<TrackMeta>),
    Status(Status),
    Position(Duration),
    Seeked(Duration),
    Volume(f64),
    CanGoNext(bool),
    CanGoPrevious(bool),
    CanPlay(bool),
    CanPause(bool),
    Shutdown,
}

/// A method call or property write from the bus.
#[derive(Debug, Clone, PartialEq)]
pub enum MprisCommand {
    Play,
    Pause,
    PlayPause,
    Stop,
    Next,
    Previous,
    /// Relative, microseconds.
    Seek {
        offset_us: i64,
    },
    SetPosition {
        track_id: u64,
        position: Duration,
    },
    /// 0.0 ..= 1.0
    SetVolume(f64),
    Quit,
}

pub struct MprisHandle {
    tx: UnboundedSender<Update>,
    last: Snapshot,
    thread: Option<JoinHandle<()>>,
}

impl MprisHandle {
    /// Start the bus thread. Registration failures are logged; the handle then
    /// simply drops every update.
    pub fn spawn(events: UnboundedSender<Message>, initial: Snapshot) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let thread = thread::Builder::new()
            .name("mpris".into())
            .spawn(move || run(rx, events, initial))
            .expect("spawning the mpris thread");
        Self {
            tx,
            last: initial,
            thread: Some(thread),
        }
    }

    pub fn track_changed(&self, now: &Snapshot) -> bool {
        self.last.track_id != now.track_id
    }

    /// Send whatever differs from the last snapshot. `meta` is only needed
    /// when `track_changed` is true.
    pub fn sync(&mut self, now: Snapshot, meta: Option<TrackMeta>) {
        for update in diff(&self.last, &now, meta) {
            if self.tx.send(update).is_err() {
                break;
            }
        }
        self.last = now;
    }

    /// Release the bus name before the process exits.
    pub fn shutdown(mut self) {
        let _ = self.tx.send(Update::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn diff(last: &Snapshot, now: &Snapshot, meta: Option<TrackMeta>) -> Vec<Update> {
    let mut updates = Vec::new();
    let same_track = last.track_id == now.track_id;
    if !same_track {
        updates.push(Update::Metadata(meta));
    }
    if last.status != now.status {
        updates.push(Update::Status(now.status));
    }
    if last.volume != now.volume {
        updates.push(Update::Volume(f64::from(now.volume) / 100.0));
    }
    if last.can_next != now.can_next {
        updates.push(Update::CanGoNext(now.can_next));
    }
    if last.can_prev != now.can_prev {
        updates.push(Update::CanGoPrevious(now.can_prev));
    }
    if last.can_play != now.can_play {
        updates.push(Update::CanPlay(now.can_play));
    }
    if last.can_pause != now.can_pause {
        updates.push(Update::CanPause(now.can_pause));
    }
    if last.position != now.position {
        updates.push(Update::Position(now.position));
        let jump = now.position.abs_diff(last.position);
        if same_track && jump > SEEK_THRESHOLD {
            updates.push(Update::Seeked(now.position));
        }
    }
    updates
}

pub fn track_path(track_id: u64) -> String {
    format!("{TRACK_PATH_PREFIX}{track_id}")
}

pub fn parse_track_path(id: &TrackId) -> Option<u64> {
    id.as_str().strip_prefix(TRACK_PATH_PREFIX)?.parse().ok()
}

fn time(duration: Duration) -> Time {
    Time::from_micros(i64::try_from(duration.as_micros()).unwrap_or(i64::MAX))
}

fn run(rx: UnboundedReceiver<Update>, events: UnboundedSender<Message>, initial: Snapshot) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            tracing::warn!(%err, "mpris runtime failed");
            return;
        }
    };
    runtime.block_on(async move {
        let suffix = match bus_suffix().await {
            Ok(suffix) => suffix,
            Err(err) => {
                tracing::warn!(%err, "no D-Bus session bus, MPRIS is off");
                return;
            }
        };
        let player = match build_player(&suffix, initial).await {
            Ok(player) => player,
            Err(err) => {
                tracing::warn!(%err, "registering the MPRIS player failed");
                return;
            }
        };
        tracing::info!("MPRIS player registered as org.mpris.MediaPlayer2.{suffix}");
        connect(&player, events);
        serve(player, rx).await;
        tracing::info!("mpris thread stopped");
    });
}

/// The plain name unless another instance already owns it.
async fn bus_suffix() -> zbus::Result<String> {
    let connection = Connection::session().await?;
    let dbus = DBusProxy::new(&connection).await?;
    let name = format!("org.mpris.MediaPlayer2.{BUS_SUFFIX}");
    let taken = dbus.name_has_owner(name.as_str().try_into()?).await?;
    Ok(if taken {
        format!("{BUS_SUFFIX}.instance{}", std::process::id())
    } else {
        BUS_SUFFIX.to_owned()
    })
}

async fn build_player(suffix: &str, initial: Snapshot) -> zbus::Result<Player> {
    Player::builder(suffix)
        .identity("Bandcamp TUI")
        .desktop_entry("bandcamp-tui")
        .can_quit(true)
        .can_raise(false)
        .can_control(true)
        .can_seek(true)
        .can_play(initial.can_play)
        .can_pause(initial.can_pause)
        .can_go_next(initial.can_next)
        .can_go_previous(initial.can_prev)
        .volume(f64::from(initial.volume) / 100.0)
        .playback_status(initial.status.into())
        .build()
        .await
}

fn connect(player: &Player, events: UnboundedSender<Message>) {
    let send = move |command: MprisCommand| {
        if events.send(Message::Mpris(command)).is_err() {
            tracing::debug!("ui is gone, dropping mpris command");
        }
    };
    let s = send.clone();
    player.connect_play_pause(move |_| s(MprisCommand::PlayPause));
    let s = send.clone();
    player.connect_play(move |_| s(MprisCommand::Play));
    let s = send.clone();
    player.connect_pause(move |_| s(MprisCommand::Pause));
    let s = send.clone();
    player.connect_stop(move |_| s(MprisCommand::Stop));
    let s = send.clone();
    player.connect_next(move |_| s(MprisCommand::Next));
    let s = send.clone();
    player.connect_previous(move |_| s(MprisCommand::Previous));
    let s = send.clone();
    player.connect_quit(move |_| s(MprisCommand::Quit));
    let s = send.clone();
    player.connect_seek(move |_, offset| {
        s(MprisCommand::Seek {
            offset_us: offset.as_micros(),
        })
    });
    let s = send.clone();
    player.connect_set_position(move |_, id, position| {
        if let Some(track_id) = parse_track_path(id) {
            let micros = u64::try_from(position.as_micros()).unwrap_or(0);
            s(MprisCommand::SetPosition {
                track_id,
                position: Duration::from_micros(micros),
            });
        }
    });
    player.connect_set_volume(move |_, volume| send(MprisCommand::SetVolume(volume)));
}

async fn serve(player: Player, mut rx: UnboundedReceiver<Update>) {
    let run = player.run();
    tokio::pin!(run);
    loop {
        tokio::select! {
            _ = &mut run => break,
            update = rx.recv() => match update {
                None | Some(Update::Shutdown) => break,
                Some(update) => {
                    if let Err(err) = apply(&player, update).await {
                        tracing::debug!(%err, "mpris update failed");
                    }
                }
            },
        }
    }
    // Dropping the player closes the connection and releases the bus name.
}

async fn apply(player: &Player, update: Update) -> zbus::Result<()> {
    match update {
        Update::Metadata(None) => player.set_metadata(Metadata::new()).await,
        Update::Metadata(Some(meta)) => player.set_metadata(metadata(meta)?).await,
        Update::Status(status) => player.set_playback_status(status.into()).await,
        Update::Position(position) => {
            player.set_position(time(position));
            Ok(())
        }
        Update::Seeked(position) => player.seeked(time(position)).await,
        Update::Volume(volume) => player.set_volume(volume).await,
        Update::CanGoNext(v) => player.set_can_go_next(v).await,
        Update::CanGoPrevious(v) => player.set_can_go_previous(v).await,
        Update::CanPlay(v) => player.set_can_play(v).await,
        Update::CanPause(v) => player.set_can_pause(v).await,
        Update::Shutdown => Ok(()),
    }
}

fn metadata(meta: TrackMeta) -> zbus::Result<Metadata> {
    let mut builder = Metadata::builder()
        .trackid(TrackId::try_from(track_path(meta.track_id))?)
        .length(time(meta.duration))
        .title(meta.title)
        .artist([meta.artist]);
    if let Some(album) = meta.album {
        builder = builder.album(album);
    }
    if let Some(art_url) = meta.art_url {
        builder = builder.art_url(art_url);
    }
    if let Some(url) = meta.page_url {
        builder = builder.url(url);
    }
    Ok(builder.build())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn playing(track_id: u64, secs: u64) -> Snapshot {
        Snapshot {
            track_id: Some(track_id),
            status: Status::Playing,
            position: Duration::from_secs(secs),
            volume: 80,
            can_next: true,
            can_prev: false,
            can_play: true,
            can_pause: true,
        }
    }

    #[test]
    fn track_path_round_trips() {
        let id = TrackId::try_from(track_path(824574161)).unwrap();
        assert_eq!(parse_track_path(&id), Some(824574161));
        assert_eq!(parse_track_path(&TrackId::NO_TRACK), None);
    }

    #[test]
    fn diff_sends_only_changes() {
        let last = playing(1, 10);
        assert!(diff(&last, &last, None).is_empty());

        // Normal progress: position only, no Seeked.
        let now = Snapshot {
            position: Duration::from_millis(10_250),
            ..last
        };
        assert_eq!(
            diff(&last, &now, None),
            vec![Update::Position(Duration::from_millis(10_250))]
        );

        // A jump on the same track is a seek.
        let now = playing(1, 30);
        assert_eq!(
            diff(&last, &now, None),
            vec![
                Update::Position(Duration::from_secs(30)),
                Update::Seeked(Duration::from_secs(30))
            ]
        );

        // A new track resets position without a Seeked signal.
        let now = Snapshot {
            can_prev: true,
            ..playing(2, 0)
        };
        assert_eq!(
            diff(&last, &now, None),
            vec![
                Update::Metadata(None),
                Update::CanGoPrevious(true),
                Update::Position(Duration::ZERO)
            ]
        );

        // Stop clears the track and status.
        let now = Snapshot {
            volume: 50,
            ..Snapshot::default()
        };
        assert_eq!(
            diff(&last, &now, None),
            vec![
                Update::Metadata(None),
                Update::Status(Status::Stopped),
                Update::Volume(0.5),
                Update::CanGoNext(false),
                Update::CanPlay(false),
                Update::CanPause(false),
                Update::Position(Duration::ZERO)
            ]
        );
    }

    #[test]
    fn metadata_carries_every_field() {
        let meta = metadata(TrackMeta {
            track_id: 7,
            title: "Alien Metal".into(),
            artist: "King Gizzard".into(),
            album: Some("Phantom Island".into()),
            duration: Duration::from_secs(231),
            art_url: Some("https://f4.bcbits.com/img/a1_10.jpg".into()),
            page_url: Some("https://kg.bandcamp.com/track/alien-metal".into()),
        })
        .unwrap();
        assert_eq!(meta.title(), Some("Alien Metal"));
        assert_eq!(meta.artist(), Some(vec!["King Gizzard".to_owned()]));
        assert_eq!(meta.album(), Some("Phantom Island"));
        assert_eq!(meta.length(), Some(Time::from_secs(231)));
        assert_eq!(parse_track_path(&meta.trackid().unwrap()), Some(7));
        assert!(meta.art_url().is_some());
        assert!(meta.url().is_some());
    }

    /// Registers on the session bus and talks to the player like playerctl
    /// would. Needs a bus: `dbus-run-session -- cargo test mpris -- --ignored`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn round_trips_over_the_session_bus() {
        use std::collections::HashMap;

        use mpris_server::zbus::fdo::PropertiesProxy;
        use mpris_server::zbus::names::InterfaceName;
        use mpris_server::zbus::zvariant::OwnedValue;

        let (events, mut commands) = mpsc::unbounded_channel::<Message>();
        let mut handle = MprisHandle::spawn(events, Snapshot::default());
        let now = playing(7, 12);
        handle.sync(
            now,
            Some(TrackMeta {
                track_id: 7,
                title: "Alien Metal".into(),
                artist: "King Gizzard".into(),
                album: None,
                duration: Duration::from_secs(231),
                art_url: None,
                page_url: None,
            }),
        );

        let connection = Connection::session().await.unwrap();
        let name = format!("org.mpris.MediaPlayer2.{BUS_SUFFIX}");
        let dbus = DBusProxy::new(&connection).await.unwrap();
        for _ in 0..50 {
            if dbus
                .name_has_owner(name.as_str().try_into().unwrap())
                .await
                .unwrap()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let props = PropertiesProxy::builder(&connection)
            .destination(name.as_str())
            .unwrap()
            .path("/org/mpris/MediaPlayer2")
            .unwrap()
            .build()
            .await
            .unwrap();
        let iface = InterfaceName::try_from("org.mpris.MediaPlayer2.Player").unwrap();
        // Setters run after the bus name appears; poll until the metadata lands.
        let mut metadata: HashMap<String, OwnedValue> = HashMap::new();
        for _ in 0..50 {
            let value = props.get(iface.clone(), "Metadata").await.unwrap();
            metadata = HashMap::try_from(value).unwrap();
            if metadata.contains_key("xesam:title") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        assert_eq!(
            metadata["xesam:title"].downcast_ref::<&str>().unwrap(),
            "Alien Metal"
        );
        assert_eq!(
            metadata["mpris:length"].downcast_ref::<i64>().unwrap(),
            231_000_000
        );
        let status = props.get(iface.clone(), "PlaybackStatus").await.unwrap();
        assert_eq!(status.downcast_ref::<&str>().unwrap(), "Playing");
        let position = props.get(iface.clone(), "Position").await.unwrap();
        assert_eq!(position.downcast_ref::<i64>().unwrap(), 12_000_000);

        connection
            .call_method(
                Some(name.as_str()),
                "/org/mpris/MediaPlayer2",
                Some("org.mpris.MediaPlayer2.Player"),
                "PlayPause",
                &(),
            )
            .await
            .unwrap();
        let command = tokio::time::timeout(Duration::from_secs(5), commands.recv())
            .await
            .unwrap();
        assert!(matches!(
            command,
            Some(Message::Mpris(MprisCommand::PlayPause))
        ));

        handle.shutdown();
        assert!(
            !dbus
                .name_has_owner(name.as_str().try_into().unwrap())
                .await
                .unwrap()
        );
    }
}
