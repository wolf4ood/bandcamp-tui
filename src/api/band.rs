//! Artist and label pages from the mobile API (`POST /api/mobile/24/band_details`).

use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::models::{TralbumKind, TralbumRef, lenient_vec, parse_bc_date, truthy};
use super::{ApiError, Client};

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct BandDetails {
    pub id: u64,
    pub name: String,
    pub bio: Option<String>,
    pub location: Option<String>,
    pub bandcamp_url: String,
    pub bio_image_id: Option<u64>,
    #[serde(deserialize_with = "lenient_vec")]
    pub discography: Vec<DiscographyItem>,
    /// A label's roster; empty for artists.
    #[serde(deserialize_with = "lenient_vec")]
    pub artists: Vec<LabelArtist>,
    #[serde(deserialize_with = "lenient_vec")]
    pub sites: Vec<Site>,
    #[serde(deserialize_with = "lenient_vec")]
    pub shows: Vec<Show>,
    #[serde(deserialize_with = "lenient_vec")]
    pub merch: Vec<Value>,
}

impl BandDetails {
    pub fn is_label(&self) -> bool {
        !self.artists.is_empty()
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DiscographyItem {
    pub item_id: u64,
    /// `album` or `track`.
    pub item_type: String,
    pub title: String,
    pub band_id: u64,
    pub band_name: Option<String>,
    pub artist_name: Option<String>,
    pub art_id: Option<u64>,
    /// `14 Aug 2026 04:00:52 GMT`
    pub release_date: Option<String>,
    #[serde(deserialize_with = "truthy")]
    pub is_purchasable: bool,
}

impl DiscographyItem {
    pub fn tralbum(&self) -> TralbumRef {
        TralbumRef {
            band_id: self.band_id,
            id: self.item_id,
            kind: TralbumKind::parse(&self.item_type),
        }
    }

    pub fn is_album(&self) -> bool {
        self.tralbum().kind == TralbumKind::Album
    }

    pub fn release_date(&self) -> Option<NaiveDateTime> {
        self.release_date.as_deref().and_then(parse_bc_date)
    }

    /// The credited artist when it differs from the page's band (labels, compilations).
    pub fn artist(&self) -> Option<&str> {
        self.artist_name
            .as_deref()
            .or(self.band_name.as_deref())
            .filter(|s| !s.is_empty())
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct LabelArtist {
    pub id: u64,
    pub name: String,
    pub location: Option<String>,
    pub image_id: Option<u64>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Site {
    pub url: String,
    pub title: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Show {
    pub loc: Option<String>,
    pub date: Option<String>,
    pub venue: Option<String>,
    pub uri: Option<String>,
    pub utc_date: Option<i64>,
}

pub async fn band_details(client: &Client, band_id: u64) -> Result<BandDetails, ApiError> {
    #[derive(Serialize)]
    struct Body {
        band_id: u64,
    }
    client
        .post_json("/api/mobile/24/band_details", &Body { band_id })
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_band_details() {
        let json = r#"{"id":256015751,"name":"p(doom)","bio":"","location":"Melbourne, Australia","bandcamp_url":"https://pdoomrecords.bandcamp.com",
            "artists":[{"id":2632533392,"name":"King Gizzard & The Lizard Wizard","image_id":39319826,"featured_date":0,"location":"Melbourne, Australia"}],
            "discography":[{"item_id":824574161,"item_type":"album","artist_name":"King Gizzard","band_name":"p(doom)","title":"Alien Metal","art_id":1,"release_date":"14 Aug 2026 04:00:52 GMT","is_purchasable":true,"band_id":2632533392},
                           {"item_id":5,"item_type":"track","title":"Single","band_id":256015751}],
            "sites":[{"url":"http://facebook.com/x","title":"Facebook"}],"shows":null,"merch":null}"#;
        let band: BandDetails = serde_json::from_str(json).unwrap();
        assert!(band.is_label());
        assert_eq!(band.discography.len(), 2);
        let first = &band.discography[0];
        assert!(first.is_album());
        assert_eq!(first.tralbum().band_id, 2632533392);
        assert_eq!(first.artist(), Some("King Gizzard"));
        assert_eq!(
            first.release_date().unwrap().to_string(),
            "2026-08-14 04:00:52"
        );
        assert!(!band.discography[1].is_album());
        assert!(band.shows.is_empty() && band.merch.is_empty());
    }
}
