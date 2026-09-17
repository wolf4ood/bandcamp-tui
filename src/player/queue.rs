//! The play queue: a flat list of tracks and a cursor.

use std::time::Duration;

use super::QueuedTrack;
use crate::api::models::Tralbum;

#[derive(Default)]
pub struct Queue {
    pub tracks: Vec<QueuedTrack>,
    pub current: Option<usize>,
}

impl Queue {
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }

    pub fn current_track(&self) -> Option<&QueuedTrack> {
        self.current.and_then(|i| self.tracks.get(i))
    }

    pub fn clear(&mut self) {
        self.tracks.clear();
        self.current = None;
    }

    /// Adopt a queue read from disk; a cursor past the end is dropped rather than trusted.
    pub fn restore(&mut self, tracks: Vec<QueuedTrack>, current: Option<usize>) {
        self.current = current.filter(|&i| i < tracks.len());
        self.tracks = tracks;
    }

    /// Replace the queue with `tracks`; returns the index to start from.
    pub fn replace(
        &mut self,
        tracks: Vec<QueuedTrack>,
        start_track_id: Option<u64>,
    ) -> Option<usize> {
        self.tracks = tracks;
        self.current = None;
        let start = start_track_id
            .and_then(|id| self.tracks.iter().position(|t| t.track_id == id))
            .unwrap_or(0);
        (!self.tracks.is_empty()).then_some(start)
    }

    pub fn append(&mut self, tracks: Vec<QueuedTrack>) {
        self.tracks.extend(tracks);
    }

    pub fn remove(&mut self, index: usize) {
        if index >= self.tracks.len() {
            return;
        }
        self.tracks.remove(index);
        self.current = match self.current {
            Some(c) if c == index => None,
            Some(c) if c > index => Some(c - 1),
            other => other,
        };
    }

    pub fn next_index(&self) -> Option<usize> {
        let next = self.current.map_or(0, |c| c + 1);
        (next < self.tracks.len()).then_some(next)
    }

    pub fn prev_index(&self) -> Option<usize> {
        match self.current {
            Some(0) | None => None,
            Some(c) => Some(c - 1),
        }
    }

    /// Point every queued track of `tralbum` at freshly fetched stream URLs.
    pub fn refresh_urls(&mut self, tralbum: &Tralbum) {
        let tralbum_ref = tralbum.tralbum_ref();
        for queued in self.tracks.iter_mut().filter(|t| t.tralbum == tralbum_ref) {
            if let Some(track) = tralbum
                .tracks
                .iter()
                .find(|t| t.track_id == queued.track_id)
            {
                queued.url = track.stream_url().map(str::to_owned);
            }
        }
    }
}

/// Turn album details into queue entries (non-streamable tracks keep `url: None`).
pub fn tracks_from(tralbum: &Tralbum) -> Vec<QueuedTrack> {
    let album = tralbum
        .is_album()
        .then(|| tralbum.title.clone())
        .or_else(|| tralbum.album_title.clone());
    tralbum
        .tracks
        .iter()
        .map(|track| QueuedTrack {
            track_id: track.track_id,
            title: track.title.clone(),
            artist: track
                .band_name
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| tralbum.tralbum_artist.clone()),
            album: album.clone(),
            duration: Duration::from_secs_f64(track.duration.max(0.0)),
            url: track.stream_url().map(str::to_owned),
            tralbum: tralbum.tralbum_ref(),
            art_id: tralbum.art_id,
            page_url: track
                .track_url
                .clone()
                .or_else(|| Some(tralbum.bandcamp_url.clone()))
                .filter(|s| !s.is_empty()),
            retried: false,
        })
        .collect()
}

/// Full-size cover art for an `art_id`, as served by Bandcamp's image CDN.
pub fn art_url(art_id: u64) -> String {
    format!("https://f4.bcbits.com/img/a{art_id}_10.jpg")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::models::{TralbumKind, TralbumRef};

    fn track(id: u64) -> QueuedTrack {
        QueuedTrack {
            track_id: id,
            title: format!("t{id}"),
            artist: "a".into(),
            album: None,
            duration: Duration::from_secs(1),
            url: Some("u".into()),
            tralbum: TralbumRef {
                band_id: 1,
                id: 1,
                kind: TralbumKind::Album,
            },
            art_id: None,
            page_url: None,
            retried: false,
        }
    }

    #[test]
    fn art_url_points_at_the_cdn() {
        assert_eq!(art_url(42), "https://f4.bcbits.com/img/a42_10.jpg");
    }

    #[test]
    fn restore_drops_a_cursor_past_the_end() {
        let mut q = Queue::default();
        q.restore(vec![track(1), track(2)], Some(5));
        assert_eq!(q.current, None);
        q.restore(vec![track(1), track(2)], Some(1));
        assert_eq!(q.current, Some(1));
        assert_eq!(q.current_track().map(|t| t.track_id), Some(2));
    }

    #[test]
    fn cursor_moves_and_survives_removal() {
        let mut q = Queue::default();
        assert_eq!(
            q.replace(vec![track(1), track(2), track(3)], Some(2)),
            Some(1)
        );
        q.current = Some(1);
        assert_eq!(q.next_index(), Some(2));
        assert_eq!(q.prev_index(), Some(0));
        q.remove(0);
        assert_eq!(q.current, Some(0));
        q.remove(0);
        assert_eq!(q.current, None);
        assert_eq!(q.next_index(), Some(0));
        assert_eq!(q.len(), 1);
    }
}
