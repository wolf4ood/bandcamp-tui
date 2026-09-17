//! Serde models for Bandcamp responses. Everything is `#[serde(default)]` and
//! lenient because the payloads are undocumented and change shape without notice.

use std::collections::HashMap;
use std::fmt;

use chrono::NaiveDateTime;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

/// Treat an explicit JSON `null` like a missing field.
pub(crate) fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

/// Bandcamp flags arrive as `true`, `1`, `null`, `"1"` or are missing entirely.
pub(crate) fn truthy<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(match Value::deserialize(deserializer)? {
        Value::Null => false,
        Value::Bool(b) => b,
        Value::Number(n) => n.as_f64().unwrap_or(0.0) != 0.0,
        Value::String(s) => !(s.is_empty() || s == "0" || s == "false"),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    })
}

/// A map that Bandcamp sometimes sends as `null` or an empty array.
pub(crate) fn lenient_map<'de, D, V>(deserializer: D) -> Result<HashMap<String, V>, D::Error>
where
    D: Deserializer<'de>,
    V: Deserialize<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| {
                V::deserialize(v)
                    .map(|v| (k, v))
                    .map_err(serde::de::Error::custom)
            })
            .collect(),
        _ => Ok(HashMap::new()),
    }
}

/// A list that Bandcamp sometimes sends as `null`.
pub(crate) fn lenient_vec<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Array(items) => items
            .into_iter()
            .map(|v| T::deserialize(v).map_err(serde::de::Error::custom))
            .collect(),
        _ => Ok(Vec::new()),
    }
}

/// Parse Bandcamp's `19 Jun 2026 04:45:30 GMT` timestamps.
pub fn parse_bc_date(text: &str) -> Option<NaiveDateTime> {
    NaiveDateTime::parse_from_str(text.trim(), "%d %b %Y %H:%M:%S GMT").ok()
}

// ----- identity ---------------------------------------------------------------

/// `GET /api/fan/2/collection_summary`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CollectionSummaryResponse {
    /// `null` when the request carried no valid identity cookie.
    pub fan_id: Option<u64>,
    #[serde(deserialize_with = "null_default")]
    pub collection_summary: CollectionSummary,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CollectionSummary {
    pub fan_id: Option<u64>,
    pub username: String,
    pub url: String,
    /// Keyed by `a<album_id>` / `t<track_id>`.
    #[serde(deserialize_with = "lenient_map")]
    pub tralbum_lookup: HashMap<String, TralbumLookupEntry>,
    #[serde(deserialize_with = "null_default")]
    pub follows: Follows,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TralbumLookupEntry {
    pub item_type: String,
    pub item_id: u64,
    pub band_id: u64,
    pub purchased: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Follows {
    /// Keyed by `b<band_id>` etc.
    #[serde(deserialize_with = "lenient_map")]
    pub following: HashMap<String, bool>,
}

// ----- tralbum references -----------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TralbumKind {
    Album,
    Track,
}

impl TralbumKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TralbumKind::Album => "a",
            TralbumKind::Track => "t",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "t" | "track" => TralbumKind::Track,
            _ => TralbumKind::Album,
        }
    }
}

/// Enough to fetch an album or track from the mobile API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TralbumRef {
    pub band_id: u64,
    pub id: u64,
    pub kind: TralbumKind,
}

impl fmt::Display for TralbumRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.kind.as_str(), self.id)
    }
}

// ----- fan collection ---------------------------------------------------------

/// One entry of a collection or wishlist listing.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CollectionItem {
    pub item_type: String,
    pub item_id: u64,
    pub tralbum_type: String,
    pub tralbum_id: u64,
    pub album_id: Option<u64>,
    pub album_title: Option<String>,
    pub band_id: u64,
    pub band_name: String,
    pub band_url: Option<String>,
    pub item_title: String,
    pub item_url: String,
    pub item_art_id: Option<u64>,
    pub item_art_url: Option<String>,
    pub purchased: Option<String>,
    pub added: Option<String>,
    pub updated: Option<String>,
    pub featured_track: Option<u64>,
    pub featured_track_title: Option<String>,
    pub featured_track_duration: Option<f64>,
    pub featured_track_number: Option<u32>,
    pub num_streamable_tracks: Option<u32>,
    #[serde(deserialize_with = "truthy")]
    pub hidden: bool,
    #[serde(deserialize_with = "truthy")]
    pub is_private: bool,
    #[serde(deserialize_with = "truthy")]
    pub is_preorder: bool,
    pub also_collected_count: Option<u64>,
    /// Pagination token of this item (`<unix>:<id>:<a|t>::`).
    pub token: Option<String>,
}

impl CollectionItem {
    pub fn tralbum(&self) -> TralbumRef {
        TralbumRef {
            band_id: self.band_id,
            id: self.tralbum_id,
            kind: TralbumKind::parse(&self.tralbum_type),
        }
    }

    pub fn is_album(&self) -> bool {
        self.tralbum().kind == TralbumKind::Album
    }

    /// Purchase date for the collection, add date for the wishlist.
    pub fn date(&self) -> Option<NaiveDateTime> {
        self.purchased
            .as_deref()
            .or(self.added.as_deref())
            .and_then(parse_bc_date)
    }
}

/// `POST /api/fancollection/1/{collection_items,wishlist_items}`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct CollectionPage {
    pub items: Vec<CollectionItem>,
    #[serde(deserialize_with = "truthy")]
    pub more_available: bool,
    pub last_token: Option<String>,
    /// Only the featured track per item; full track lists come from `tralbum_details`.
    #[serde(deserialize_with = "lenient_map")]
    pub tracklists: HashMap<String, Vec<FeaturedTrack>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FeaturedTrack {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub track_number: Option<u32>,
    pub duration: f64,
    #[serde(deserialize_with = "lenient_map")]
    pub file: HashMap<String, String>,
}

/// `POST /api/fancollection/1/search_items`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchItemsResponse {
    pub tralbums: Vec<CollectionItem>,
    pub search_key: String,
}

// ----- album / track details --------------------------------------------------

/// `GET /api/mobile/24/tralbum_details`
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tralbum {
    pub id: u64,
    pub title: String,
    pub tralbum_artist: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub bandcamp_url: String,
    /// Unix seconds.
    pub release_date: Option<i64>,
    pub about: Option<String>,
    pub credits: Option<String>,
    pub price: Option<f64>,
    pub currency: Option<String>,
    #[serde(deserialize_with = "truthy")]
    pub is_purchasable: bool,
    #[serde(deserialize_with = "truthy")]
    pub free_download: bool,
    pub art_id: Option<u64>,
    #[serde(deserialize_with = "null_default")]
    pub band: Band,
    pub label: Option<String>,
    pub album_id: Option<u64>,
    pub album_title: Option<String>,
    pub num_downloadable_tracks: Option<u32>,
    pub tracks: Vec<Track>,
    pub tags: Vec<Tag>,
}

impl Tralbum {
    pub fn tralbum_ref(&self) -> TralbumRef {
        TralbumRef {
            band_id: self.band.band_id,
            id: self.id,
            kind: TralbumKind::parse(&self.kind),
        }
    }

    pub fn is_album(&self) -> bool {
        TralbumKind::parse(&self.kind) == TralbumKind::Album
    }

    pub fn release_date(&self) -> Option<chrono::NaiveDate> {
        self.release_date
            .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
            .map(|dt| dt.date_naive())
    }

    pub fn total_duration_secs(&self) -> f64 {
        self.tracks.iter().map(|t| t.duration.max(0.0)).sum()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Band {
    pub band_id: u64,
    pub name: String,
    pub location: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Track {
    pub track_id: u64,
    pub title: String,
    pub track_num: Option<u32>,
    pub duration: f64,
    #[serde(deserialize_with = "truthy")]
    pub is_streamable: bool,
    /// Keyed by encoding, in practice only `mp3-128`.
    #[serde(deserialize_with = "lenient_map")]
    pub streaming_url: HashMap<String, String>,
    pub band_name: Option<String>,
    pub album_title: Option<String>,
    pub track_url: Option<String>,
}

impl Track {
    pub fn stream_url(&self) -> Option<&str> {
        self.streaming_url.get("mp3-128").map(String::as_str)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Tag {
    pub name: String,
    pub norm_name: String,
    #[serde(deserialize_with = "truthy")]
    pub isloc: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_collection_summary() {
        let json = r#"{
            "fan_id": 42,
            "collection_summary": {
                "fan_id": 42,
                "username": "someone",
                "url": "https://bandcamp.com/someone",
                "tralbum_lookup": {
                    "a1": {"item_type": "a", "item_id": 1, "band_id": 9, "purchased": "01 Jan 2024 00:00:00 GMT"},
                    "t2": {"item_type": "t", "item_id": 2, "band_id": 9}
                },
                "follows": {"following": {"b9": true}}
            }
        }"#;
        let parsed: CollectionSummaryResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.fan_id, Some(42));
        assert_eq!(parsed.collection_summary.username, "someone");
        assert_eq!(parsed.collection_summary.tralbum_lookup.len(), 2);
        assert_eq!(parsed.collection_summary.follows.following.len(), 1);
    }

    #[test]
    fn tolerates_logged_out_shape() {
        let parsed: CollectionSummaryResponse =
            serde_json::from_str(r#"{"fan_id": null, "collection_summary": null}"#).unwrap();
        assert_eq!(parsed.fan_id, None);
    }

    #[test]
    fn decodes_collection_page() {
        let json = r#"{
            "items": [{
                "item_type": "album", "item_id": 1083851892, "tralbum_type": "a", "tralbum_id": 1083851892,
                "album_id": 1083851892, "band_id": 3878281903, "band_name": "Daniel Donato's Cosmic Country",
                "item_title": "Ardmore, Pennsylvania (2025-09-19)", "item_url": "https://danieldonato.bandcamp.com/album/x",
                "item_art_id": 2787051412, "purchased": "19 Jun 2026 04:45:30 GMT", "added": "28 Oct 2025 17:48:50 GMT",
                "featured_track": 3053988743, "num_streamable_tracks": 16, "hidden": null, "is_private": false,
                "token": "1781844330:1083851892:a::"
            }, {
                "item_type": "track", "item_id": 4086481121, "tralbum_type": "t", "tralbum_id": 4086481121,
                "album_id": null, "band_id": 272550275, "band_name": "Robert Wynia", "item_title": "Always Laughing",
                "item_url": "https://robertwynia.bandcamp.com/track/always-laughing", "hidden": 1
            }],
            "more_available": true,
            "last_token": "1781838129:2388662022:a::",
            "tracklists": {"a1083851892": [{"id": 3053988743, "title": "Why", "artist": "DD", "track_number": 1,
                "duration": 288.0, "file": {"mp3-128": "https://bandcamp.com/stream_redirect?x"}}]},
            "redownload_urls": [], "purchase_infos": {}, "collectors": {}
        }"#;
        let page: CollectionPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.items.len(), 2);
        assert!(page.more_available);
        assert_eq!(
            page.last_token.as_deref(),
            Some("1781838129:2388662022:a::")
        );
        let album = &page.items[0];
        assert!(album.is_album());
        assert!(!album.hidden);
        assert_eq!(album.tralbum().kind, TralbumKind::Album);
        assert_eq!(album.date().unwrap().to_string(), "2026-06-19 04:45:30");
        let track = &page.items[1];
        assert!(!track.is_album());
        assert!(track.hidden);
        assert_eq!(
            page.tracklists["a1083851892"][0].file["mp3-128"],
            "https://bandcamp.com/stream_redirect?x"
        );
    }

    #[test]
    fn tracklists_may_be_an_empty_array() {
        let page: CollectionPage =
            serde_json::from_str(r#"{"items": [], "more_available": false, "tracklists": []}"#)
                .unwrap();
        assert!(page.tracklists.is_empty());
        assert!(!page.more_available);
    }

    #[test]
    fn decodes_tralbum_details() {
        let json = r#"{
            "id": 824574161, "title": "Alien Metal", "tralbum_artist": "King Gizzard", "type": "a",
            "bandcamp_url": "https://kinggizzard.bandcamp.com/album/alien-metal", "release_date": 1786680052,
            "about": null, "credits": "Recorded by aliens", "price": 0.0, "currency": "AUD", "is_purchasable": true,
            "free_download": false, "art_id": 2759728592, "band": {"band_id": 2632533392, "name": "King Gizzard", "location": "Melbourne"},
            "label": "p(doom)", "tags": [{"name": "Electronic", "norm_name": "electronic", "isloc": false}],
            "tracks": [
                {"track_id": 1, "title": "Sapience", "track_num": 1, "duration": 281.964, "is_streamable": true,
                 "streaming_url": {"mp3-128": "https://bandcamp.com/stream_redirect?a"}},
                {"track_id": 2, "title": "Silent", "track_num": 2, "duration": 100.0, "is_streamable": false, "streaming_url": null}
            ]
        }"#;
        let tralbum: Tralbum = serde_json::from_str(json).unwrap();
        assert!(tralbum.is_album());
        assert_eq!(tralbum.tralbum_ref().band_id, 2632533392);
        assert_eq!(tralbum.release_date().unwrap().to_string(), "2026-08-14");
        assert_eq!(
            tralbum.tracks[0].stream_url(),
            Some("https://bandcamp.com/stream_redirect?a")
        );
        assert_eq!(tralbum.tracks[1].stream_url(), None);
        assert_eq!(tralbum.tags[0].norm_name, "electronic");
    }
}
