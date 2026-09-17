//! Site-wide search (`POST /api/bcsearch_public_api/1/autocomplete_elastic`),
//! the JSON behind the website's search box. Public, no cookie needed.

use serde::{Deserialize, Serialize};

use super::models::{TralbumKind, TralbumRef, lenient_vec, truthy};
use super::{ApiError, Client};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SearchFilter {
    #[default]
    All,
    Artists,
    Albums,
    Tracks,
    Fans,
}

impl SearchFilter {
    pub const ALL: [SearchFilter; 5] = [
        SearchFilter::All,
        SearchFilter::Artists,
        SearchFilter::Albums,
        SearchFilter::Tracks,
        SearchFilter::Fans,
    ];

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn label(self) -> &'static str {
        match self {
            SearchFilter::All => "everything",
            SearchFilter::Artists => "artists & labels",
            SearchFilter::Albums => "albums",
            SearchFilter::Tracks => "tracks",
            SearchFilter::Fans => "fans",
        }
    }

    fn code(self) -> &'static str {
        match self {
            SearchFilter::All => "",
            SearchFilter::Artists => "b",
            SearchFilter::Albums => "a",
            SearchFilter::Tracks => "t",
            SearchFilter::Fans => "f",
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SearchResult {
    /// `a` album, `t` track, `b` band or label, `f` fan.
    #[serde(rename = "type")]
    pub kind: String,
    pub id: u64,
    pub name: String,
    pub band_id: Option<u64>,
    pub band_name: Option<String>,
    pub album_name: Option<String>,
    pub album_id: Option<u64>,
    pub art_id: Option<u64>,
    pub img_id: Option<u64>,
    pub item_url_root: Option<String>,
    pub item_url_path: Option<String>,
    pub location: Option<String>,
    pub genre_name: Option<String>,
    #[serde(deserialize_with = "truthy")]
    pub is_label: bool,
    #[serde(deserialize_with = "lenient_vec")]
    pub tag_names: Vec<String>,
    pub username: Option<String>,
    pub collection_size: Option<u64>,
}

impl SearchResult {
    pub fn url(&self) -> Option<String> {
        self.item_url_path
            .clone()
            .or_else(|| self.item_url_root.clone())
    }

    /// Albums and tracks can be opened and played.
    pub fn tralbum(&self) -> Option<TralbumRef> {
        let kind = match self.kind.as_str() {
            "a" => TralbumKind::Album,
            "t" => TralbumKind::Track,
            _ => return None,
        };
        Some(TralbumRef {
            band_id: self.band_id?,
            id: self.id,
            kind,
        })
    }

    pub fn is_band(&self) -> bool {
        self.kind == "b"
    }

    pub fn is_fan(&self) -> bool {
        self.kind == "f"
    }

    pub fn kind_label(&self) -> &'static str {
        match self.kind.as_str() {
            "a" => "album",
            "t" => "track",
            "b" if self.is_label => "label",
            "b" => "artist",
            "f" => "fan",
            _ => "",
        }
    }

    /// The band behind the result, for following and artist pages.
    pub fn band(&self) -> Option<u64> {
        if self.is_band() {
            Some(self.id)
        } else {
            self.band_id
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchResponse {
    pub results: Vec<SearchResult>,
    /// Tag names that matched (only for the "everything" filter).
    pub tags: Vec<String>,
}

pub async fn search(
    client: &Client,
    text: &str,
    filter: SearchFilter,
) -> Result<SearchResponse, ApiError> {
    #[derive(Serialize)]
    struct Body<'a> {
        search_text: &'a str,
        search_filter: &'a str,
        full_page: bool,
        fan_id: Option<u64>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Raw {
        auto: Auto,
        tag: Tag,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Auto {
        #[serde(deserialize_with = "lenient_vec")]
        results: Vec<SearchResult>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Tag {
        #[serde(deserialize_with = "lenient_vec")]
        matches: Vec<TagMatch>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct TagMatch {
        norm_name: String,
    }
    let body = Body {
        search_text: text,
        search_filter: filter.code(),
        full_page: true,
        fan_id: None,
    };
    let raw: Raw = client
        .post_json("/api/bcsearch_public_api/1/autocomplete_elastic", &body)
        .await?;
    Ok(SearchResponse {
        results: raw.auto.results,
        tags: raw
            .tag
            .matches
            .into_iter()
            .map(|t| t.norm_name)
            .filter(|t| !t.is_empty())
            .collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real search plus the artist page it leads to. Needs network: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn search_and_band_live() {
        let client = Client::new().unwrap();
        let response = search(&client, "king gizzard", SearchFilter::All)
            .await
            .unwrap();
        assert!(!response.results.is_empty());
        let band = response
            .results
            .iter()
            .find(|r| r.is_band())
            .expect("a band result");
        eprintln!(
            "band: {} ({}) tags {:?}; tags matched {:?}",
            band.name, band.id, band.tag_names, response.tags
        );
        let details = crate::api::band::band_details(&client, band.id)
            .await
            .unwrap();
        eprintln!(
            "{}: {} releases, label={}",
            details.name,
            details.discography.len(),
            details.is_label()
        );
        assert!(!details.discography.is_empty());
        let fans = search(&client, "theseahorse", SearchFilter::Fans)
            .await
            .unwrap();
        let fan = fans
            .results
            .iter()
            .find(|r| r.is_fan())
            .expect("a fan result");
        eprintln!("fan: {} ({}) {:?}", fan.name, fan.id, fan.collection_size);
        assert_eq!(fan.id, 10162377);
    }

    #[test]
    fn decodes_mixed_results() {
        let json = r#"[
            {"type":"a","id":4003345202,"art_id":1143099231,"name":"Rock You All","band_id":2309449160,"band_name":"Rattlesnake",
             "item_url_root":"https://rattlesnake-band.bandcamp.com","item_url_path":"https://rattlesnake-band.bandcamp.com/album/rock-you-all","tag_names":["Metal","Rock"]},
            {"type":"b","id":2309449160,"img_id":15435300,"name":"RATTLESNAKE","item_url_root":"https://rattlesnake-band.bandcamp.com","location":"Geneva, Switzerland","is_label":false,"tag_names":null,"genre_name":"Rock"},
            {"type":"t","id":906915445,"name":"Rattlesnake","band_id":3468254819,"band_name":"Flying Mojito Bros","item_url_path":"https://flyingmojitobros.bandcamp.com/track/rattlesnake","album_name":null},
            {"type":"f","id":10162377,"name":"The Seahorse","username":"theseahorse","item_url_root":"https://bandcamp.com/theseahorse","collection_size":65,"genre_name":"Rock"}
        ]"#;
        let results: Vec<SearchResult> = serde_json::from_str(json).unwrap();
        assert_eq!(results[0].tralbum().unwrap().kind, TralbumKind::Album);
        assert_eq!(results[0].kind_label(), "album");
        assert!(results[1].is_band() && results[1].tag_names.is_empty());
        assert_eq!(results[1].band(), Some(2309449160));
        assert_eq!(results[2].tralbum().unwrap().kind, TralbumKind::Track);
        assert_eq!(results[2].band(), Some(3468254819));
        assert!(results[3].is_fan());
        assert_eq!(
            results[3].url().as_deref(),
            Some("https://bandcamp.com/theseahorse")
        );
    }
}
