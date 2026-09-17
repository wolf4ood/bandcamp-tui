//! In-process playback: rodio on a dedicated thread, fed by HTTP streams that
//! stream-download buffers into a seekable temp file.

pub mod queue;
pub mod store;

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use rodio::Player;
use rodio::decoder::Decoder;
use rodio::stream::{DeviceSinkBuilder, MixerDeviceSink};
use serde::{Deserialize, Serialize};
use stream_download::storage::temp::TempStorageProvider;
use stream_download::{Settings, StreamDownload};
use tokio::sync::mpsc::UnboundedSender;

use crate::api::models::TralbumRef;
use crate::app::Message;

/// One entry of the play queue: everything the UI shows plus how to stream it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QueuedTrack {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration: Duration,
    /// `None` when the track is not streamable.
    pub url: Option<String>,
    pub tralbum: TralbumRef,
    /// Cover art id (see `queue::art_url`).
    pub art_id: Option<u64>,
    /// Bandcamp page of the track, falling back to the album page.
    pub page_url: Option<String>,
    /// Set once a fresh URL was fetched after a failure; the second failure is final.
    /// Never persisted: a restored track gets its one retry back.
    #[serde(skip)]
    pub retried: bool,
}

pub enum PlayerCommand {
    Load { track_id: u64, url: String },
    Pause,
    Resume,
    Stop,
    SeekTo(Duration),
    SeekBy(i64),
    SetVolume(f32),
    Shutdown,
}

#[derive(Debug, Clone)]
pub enum PlayerEvent {
    Loading {
        track_id: u64,
    },
    Started {
        track_id: u64,
    },
    Progress {
        track_id: u64,
        position: Duration,
        paused: bool,
    },
    Ended {
        track_id: u64,
    },
    Failed {
        track_id: u64,
        message: String,
    },
    /// No audio device could be opened; playback is off for this session.
    Unavailable(String),
}

const POLL: Duration = Duration::from_millis(250);

#[derive(Clone)]
pub struct PlayerHandle {
    tx: Sender<PlayerCommand>,
}

impl PlayerHandle {
    /// Start the audio thread. The device is opened lazily on the first `Load`
    /// so a machine without audio can still browse.
    pub fn spawn(
        runtime: tokio::runtime::Handle,
        events: UnboundedSender<Message>,
        volume: f32,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        thread::Builder::new()
            .name("audio".into())
            .spawn(move || run(rx, runtime, events, volume))
            .expect("spawning the audio thread");
        Self { tx }
    }

    pub fn send(&self, command: PlayerCommand) {
        if self.tx.send(command).is_err() {
            tracing::error!("audio thread is gone");
        }
    }
}

struct Output {
    _sink: MixerDeviceSink,
    player: Player,
}

fn run(
    rx: mpsc::Receiver<PlayerCommand>,
    runtime: tokio::runtime::Handle,
    events: UnboundedSender<Message>,
    mut volume: f32,
) {
    let emit = |event: PlayerEvent| {
        let _ = events.send(Message::Player(event));
    };
    let mut output: Option<Output> = None;
    let mut current: Option<u64> = None;

    loop {
        match rx.recv_timeout(POLL) {
            Ok(PlayerCommand::Load { track_id, url }) => {
                if output.is_none() {
                    match open_output(volume) {
                        Ok(out) => output = Some(out),
                        Err(message) => {
                            tracing::error!(%message, "no audio output");
                            emit(PlayerEvent::Unavailable(message));
                            continue;
                        }
                    }
                }
                let Some(out) = output.as_ref() else { continue };
                current = None;
                out.player.clear();
                emit(PlayerEvent::Loading { track_id });
                match runtime.block_on(open_stream(&url)) {
                    Ok(decoder) => {
                        out.player.append(decoder);
                        out.player.play();
                        current = Some(track_id);
                        emit(PlayerEvent::Started { track_id });
                    }
                    Err(message) => {
                        tracing::warn!(track_id, %message, "stream failed");
                        emit(PlayerEvent::Failed { track_id, message });
                    }
                }
            }
            Ok(command) => {
                if let Some(out) = output.as_ref() {
                    match command {
                        PlayerCommand::Pause => out.player.pause(),
                        PlayerCommand::Resume => out.player.play(),
                        PlayerCommand::Stop => {
                            out.player.clear();
                            current = None;
                        }
                        PlayerCommand::SeekTo(position) => seek(&out.player, position),
                        PlayerCommand::SeekBy(seconds) => {
                            let position = out.player.get_pos();
                            let target = if seconds < 0 {
                                position.saturating_sub(Duration::from_secs(seconds.unsigned_abs()))
                            } else {
                                position + Duration::from_secs(seconds as u64)
                            };
                            seek(&out.player, target);
                        }
                        PlayerCommand::SetVolume(v) => {
                            volume = v;
                            out.player.set_volume(v);
                        }
                        PlayerCommand::Shutdown => break,
                        PlayerCommand::Load { .. } => unreachable!(),
                    }
                } else if let PlayerCommand::SetVolume(v) = command {
                    volume = v;
                } else if let PlayerCommand::Shutdown = command {
                    break;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                if let (Some(track_id), Some(out)) = (current, output.as_ref()) {
                    if out.player.empty() {
                        current = None;
                        emit(PlayerEvent::Ended { track_id });
                    } else {
                        emit(PlayerEvent::Progress {
                            track_id,
                            position: out.player.get_pos(),
                            paused: out.player.is_paused(),
                        });
                    }
                }
            }
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    tracing::info!("audio thread stopped");
}

fn open_output(volume: f32) -> Result<Output, String> {
    let sink = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
    let player = Player::connect_new(sink.mixer());
    player.set_volume(volume);
    Ok(Output {
        _sink: sink,
        player,
    })
}

fn seek(player: &Player, position: Duration) {
    if let Err(err) = player.try_seek(position) {
        tracing::warn!(%err, "seek failed");
    }
}

type StreamDecoder = Decoder<StreamDownload<TempStorageProvider>>;

async fn open_stream(url: &str) -> Result<StreamDecoder, String> {
    let url = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    let reader = StreamDownload::new_http(url, TempStorageProvider::new(), Settings::default())
        .await
        .map_err(|e| e.to_string())?;
    let byte_len = reader.content_length();
    let mut builder = Decoder::builder()
        .with_data(reader)
        .with_hint("mp3")
        .with_seekable(true);
    if let Some(len) = byte_len {
        builder = builder.with_byte_len(len);
    }
    builder.build().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rodio::Source;

    /// Streams the first seconds of a real public track. Needs network; run with
    /// `cargo test -- --ignored`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[ignore]
    async fn decodes_a_real_bandcamp_stream() {
        use crate::api::models::{TralbumKind, TralbumRef};
        let client = crate::api::Client::new().unwrap();
        // King Gizzard & The Lizard Wizard, "Alien Metal" (streamable, public).
        let tralbum = crate::api::tralbum::tralbum_details(
            &client,
            TralbumRef {
                band_id: 2632533392,
                id: 824574161,
                kind: TralbumKind::Album,
            },
        )
        .await
        .unwrap();
        let url = tralbum.tracks[0].stream_url().unwrap().to_owned();
        // Like the audio thread: the reader blocks while tokio workers download.
        let mut decoder = open_stream(&url).await.unwrap();
        let (rate, channels, total, samples) = tokio::task::spawn_blocking(move || {
            let rate = decoder.sample_rate();
            let channels = decoder.channels();
            let total = decoder.total_duration();
            let samples: Vec<f32> = decoder.by_ref().take(48_000).collect();
            (rate, channels, total, samples)
        })
        .await
        .unwrap();
        assert!(rate.get() >= 22_050);
        assert_eq!(samples.len(), 48_000);
        assert!(samples.iter().any(|s| s.abs() > 0.0), "silence only");
        eprintln!("rate {rate} channels {channels:?} total {total:?}");
    }
}
