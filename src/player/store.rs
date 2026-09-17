//! The play queue on disk, so a quit or a crash does not lose it.
//!
//! Stream URLs are saved as they were, but they are signed and expire; the app
//! refreshes them before the first play of a restored track.

use std::fs;
use std::io;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::QueuedTrack;
use super::queue::Queue;

const VERSION: u32 = 1;

/// What goes to disk: the tracks and the cursor, nothing about playback.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedQueue {
    pub version: u32,
    pub tracks: Vec<QueuedTrack>,
    pub current: Option<usize>,
}

impl Default for SavedQueue {
    fn default() -> Self {
        Self {
            version: VERSION,
            tracks: Vec::new(),
            current: None,
        }
    }
}

impl From<&Queue> for SavedQueue {
    fn from(queue: &Queue) -> Self {
        Self {
            version: VERSION,
            tracks: queue.tracks.clone(),
            current: queue.current,
        }
    }
}

/// A JSON file at a fixed path. Writes go through a temp file and a rename, so a
/// crash mid-write leaves the previous queue intact.
#[derive(Debug)]
pub struct QueueStore {
    path: PathBuf,
}

impl QueueStore {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// `Ok(None)` when there is no file. A file we cannot parse is logged and
    /// ignored too: a stale format must never keep the app from starting.
    pub fn load(&self) -> Result<Option<SavedQueue>> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(err).with_context(|| format!("reading {}", self.path.display()));
            }
        };
        match serde_json::from_str::<SavedQueue>(&text) {
            Ok(saved) if saved.version == VERSION => Ok(Some(saved)),
            Ok(saved) => {
                tracing::warn!(
                    version = saved.version,
                    "ignoring queue file of unknown version"
                );
                Ok(None)
            }
            Err(err) => {
                tracing::warn!(?err, path = %self.path.display(), "ignoring unreadable queue file");
                Ok(None)
            }
        }
    }

    /// An empty queue removes the file instead of writing an empty one.
    pub fn save(&self, saved: &SavedQueue) -> Result<()> {
        if saved.tracks.is_empty() {
            return self.clear();
        }
        if let Some(parent) = self.path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string(saved).context("serializing queue")?;
        let tmp = self.path.with_extension("json.tmp");
        fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        fs::rename(&tmp, &self.path).with_context(|| format!("moving {} into place", tmp.display()))
    }

    pub fn clear(&self) -> Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err).with_context(|| format!("removing {}", self.path.display())),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::api::models::{TralbumKind, TralbumRef};

    fn track(id: u64) -> QueuedTrack {
        QueuedTrack {
            track_id: id,
            title: format!("t{id}"),
            artist: "a".into(),
            album: Some("album".into()),
            duration: Duration::from_millis(1500),
            url: Some(format!("https://example.invalid/{id}")),
            tralbum: TralbumRef {
                band_id: 1,
                id: 1,
                kind: TralbumKind::Album,
            },
            art_id: Some(9),
            page_url: None,
            retried: true,
        }
    }

    fn store(name: &str) -> QueueStore {
        let dir = std::env::temp_dir().join(format!("bandcamp-tui-test-{}", std::process::id()));
        QueueStore::new(dir.join(name))
    }

    #[test]
    fn round_trip_drops_the_retry_latch() {
        let store = store("queue.json");
        store.clear().unwrap();
        assert_eq!(store.load().unwrap(), None);

        let saved = SavedQueue {
            version: VERSION,
            tracks: vec![track(1), track(2)],
            current: Some(1),
        };
        store.save(&saved).unwrap();
        let loaded = store.load().unwrap().unwrap();
        assert_eq!(loaded.current, Some(1));
        assert_eq!(loaded.tracks.len(), 2);
        assert!(loaded.tracks.iter().all(|t| !t.retried));
        let mut expected = saved.clone();
        expected.tracks.iter_mut().for_each(|t| t.retried = false);
        assert_eq!(loaded, expected);

        store.save(&SavedQueue::default()).unwrap();
        assert_eq!(store.load().unwrap(), None);
        assert!(!store.path.exists());
    }

    #[test]
    fn garbage_and_unknown_versions_are_ignored() {
        let store = store("bad-queue.json");
        fs::create_dir_all(store.path.parent().unwrap()).unwrap();
        fs::write(&store.path, "{not json").unwrap();
        assert_eq!(store.load().unwrap(), None);
        fs::write(&store.path, r#"{"version":99,"tracks":[],"current":null}"#).unwrap();
        assert_eq!(store.load().unwrap(), None);
        store.clear().unwrap();
    }
}
