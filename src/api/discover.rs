//! The Discover page (`/api/discover/1/discover_web`) and its helpers: filter
//! vocabulary, tag autocomplete, related tags and location search.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};

use super::models::{TralbumKind, TralbumRef, truthy};
use super::{ApiError, Client};

pub const PAGE_SIZE: u32 = 40;
/// Bandcamp's own value for the first page.
pub const FIRST_CURSOR: &str = "*";

// ----- filter vocabulary ------------------------------------------------------

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct FilterOption {
    pub id: i64,
    pub label: String,
    pub slug: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct Subgenre {
    pub id: i64,
    pub label: String,
    pub slug: String,
    #[serde(rename = "parentSlug")]
    pub parent_slug: String,
}

/// The option lists the website's Discover page is built from.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct DiscoverOptions {
    pub categories: Vec<FilterOption>,
    pub genres: Vec<FilterOption>,
    pub subgenres: Vec<Subgenre>,
    pub slices: Vec<FilterOption>,
    pub locations: Vec<FilterOption>,
    pub times: Vec<FilterOption>,
}

impl DiscoverOptions {
    /// Snapshot taken from bandcamp.com/discover; refreshed live when possible.
    pub fn embedded() -> Self {
        serde_json::from_str(include_str!("../../assets/discover_options.json"))
            .expect("embedded discover options are valid JSON")
    }

    pub fn subgenres_of(&self, genre_slug: &str) -> impl Iterator<Item = &Subgenre> {
        self.subgenres
            .iter()
            .filter(move |s| s.parent_slug == genre_slug)
    }

    pub fn label_for(list: &[FilterOption], id: i64) -> Option<&str> {
        list.iter().find(|o| o.id == id).map(|o| o.label.as_str())
    }
}

/// Scrape the live option lists out of the Discover page's `data-blob`.
pub async fn options(client: &Client) -> Result<DiscoverOptions, ApiError> {
    let html = client.get_html("/discover").await?;
    parse_options(&html)
}

fn parse_options(html: &str) -> Result<DiscoverOptions, ApiError> {
    #[derive(Deserialize)]
    struct Blob {
        #[serde(rename = "appData")]
        app_data: AppData,
    }
    #[derive(Deserialize)]
    struct AppData {
        #[serde(rename = "initialState")]
        initial_state: DiscoverOptions,
    }
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("[data-blob]").expect("static selector");
    let mut last_error = String::from("no data-blob attribute found");
    for element in document.select(&selector) {
        let Some(blob) = element.value().attr("data-blob") else {
            continue;
        };
        match serde_json::from_str::<Blob>(blob) {
            Ok(blob) => return Ok(blob.app_data.initial_state),
            Err(err) => last_error = err.to_string(),
        }
    }
    Err(ApiError::Decode {
        path: "/discover".to_owned(),
        message: last_error,
        body: String::new(),
    })
}

// ----- discover results -------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct DiscoverRequest {
    pub tag_norm_names: Vec<String>,
    pub geoname_id: i64,
    pub slice: String,
    pub time_facet_id: Option<i64>,
    pub category_id: i64,
    pub size: u32,
    pub cursor: String,
    pub include_result_types: Vec<&'static str>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DiscoverResponse {
    pub results: Vec<DiscoverResult>,
    pub result_count: Option<u64>,
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Price {
    pub amount: f64,
    pub currency: String,
    #[serde(deserialize_with = "truthy")]
    pub is_money: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FeaturedPreview {
    pub id: u64,
    pub title: String,
    pub band_name: String,
    pub stream_url: Option<String>,
    pub duration: f64,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct DiscoverResult {
    /// `a` album/track, `s` merch.
    pub result_type: String,
    /// `a`, `t`, or `p` for merch packages.
    pub item_type: String,
    pub item_id: u64,
    pub band_id: u64,
    pub title: String,
    pub band_name: String,
    pub band_location: Option<String>,
    /// `2026-08-14 04:00:52 UTC`
    pub release_date: Option<String>,
    pub price: Option<Price>,
    #[serde(deserialize_with = "truthy")]
    pub is_set_price: bool,
    #[serde(deserialize_with = "truthy")]
    pub is_free_download: bool,
    pub track_count: Option<u32>,
    pub duration: Option<f64>,
    pub item_url: String,
    pub band_url: Option<String>,
    pub featured_track: Option<FeaturedPreview>,
    #[serde(deserialize_with = "truthy")]
    pub is_owned: bool,
    #[serde(deserialize_with = "truthy")]
    pub is_wishlisted: bool,
}

impl DiscoverResult {
    pub fn is_music(&self) -> bool {
        self.result_type == "a"
    }

    pub fn tralbum(&self) -> Option<TralbumRef> {
        self.is_music().then(|| TralbumRef {
            band_id: self.band_id,
            id: self.item_id,
            kind: TralbumKind::parse(&self.item_type),
        })
    }

    /// The item page without Bandcamp's `?from=discover_page` tracking suffix.
    pub fn clean_url(&self) -> String {
        self.item_url
            .split_once('?')
            .map_or(self.item_url.clone(), |(base, _)| base.to_owned())
    }

    pub fn release_date(&self) -> Option<NaiveDate> {
        let text = self.release_date.as_deref()?;
        NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S UTC")
            .ok()
            .map(|dt| dt.date())
    }

    pub fn price_label(&self) -> String {
        match &self.price {
            Some(p) if p.amount > 0.0 => format!("{} {:.2}", p.currency, p.amount),
            _ if self.is_free_download => "free".to_owned(),
            _ if !self.is_music() => String::new(),
            _ if !self.is_set_price => "name your price".to_owned(),
            _ => String::new(),
        }
    }
}

pub async fn discover(
    client: &Client,
    request: &DiscoverRequest,
) -> Result<DiscoverResponse, ApiError> {
    client
        .post_json("/api/discover/1/discover_web", request)
        .await
}

// ----- tags and locations ------------------------------------------------------

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TagSuggestion {
    pub norm_name: String,
    pub display_name: String,
    pub count: u64,
}

/// Autocomplete for tag names (`prefix` is what the user typed so far).
pub async fn tag_suggestions(
    client: &Client,
    prefix: &str,
) -> Result<Vec<TagSuggestion>, ApiError> {
    #[derive(Serialize)]
    struct Body<'a> {
        prefix: &'a str,
    }
    client
        .post_json("/api/bcsearch_public_api/1/tag_search", &Body { prefix })
        .await
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct RelatedTag {
    pub name: String,
    pub norm_name: String,
    #[serde(deserialize_with = "truthy")]
    pub isloc: bool,
}

/// Tags Bandcamp considers related to `tags`, most related first, without the inputs.
pub async fn related_tags(
    client: &Client,
    tags: &[String],
    size: u32,
) -> Result<Vec<RelatedTag>, ApiError> {
    #[derive(Serialize)]
    struct Body<'a> {
        tag_names: &'a [String],
        size: u32,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Response {
        single_results: Vec<SingleResult>,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct SingleResult {
        related_tags: Vec<RelatedTag>,
    }
    let response: Response = client
        .post_json(
            "/api/tag_search/2/related_tags",
            &Body {
                tag_names: tags,
                size,
            },
        )
        .await?;
    let mut seen: Vec<String> = tags.to_vec();
    let mut related = Vec::new();
    for single in response.single_results {
        for tag in single.related_tags {
            if !seen.contains(&tag.norm_name) {
                seen.push(tag.norm_name.clone());
                related.push(tag);
            }
        }
    }
    Ok(related)
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Geoname {
    pub id: String,
    pub name: String,
    pub fullname: String,
}

pub async fn geoname_search(
    client: &Client,
    query: &str,
    limit: u32,
) -> Result<Vec<Geoname>, ApiError> {
    #[derive(Serialize)]
    struct Body<'a> {
        q: &'a str,
        n: u32,
        geocoder_fallback: bool,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct Response {
        results: Vec<Geoname>,
    }
    let response: Response = client
        .post_json(
            "/api/location/1/geoname_search",
            &Body {
                q: query,
                n: limit,
                geocoder_fallback: true,
            },
        )
        .await?;
    Ok(response.results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_options_parse_and_look_sane() {
        let options = DiscoverOptions::embedded();
        assert!(options.genres.iter().any(|g| g.slug == "electronic"));
        assert_eq!(
            options
                .slices
                .iter()
                .map(|s| s.slug.as_str())
                .collect::<Vec<_>>(),
            ["top", "new", "rand"]
        );
        assert!(options.subgenres_of("electronic").count() > 10);
        assert_eq!(options.locations[0].id, 0);
        assert_eq!(options.times[0].slug, "fresh");
    }

    #[test]
    fn parses_options_from_page_html() {
        let blob = r#"{&quot;appData&quot;:{&quot;initialState&quot;:{&quot;categories&quot;:[{&quot;id&quot;:0,&quot;label&quot;:&quot;all&quot;,&quot;slug&quot;:&quot;all&quot;}],&quot;genres&quot;:[],&quot;subgenres&quot;:[],&quot;slices&quot;:[],&quot;locations&quot;:[],&quot;times&quot;:[]}}}"#;
        let html =
            format!("<html><body><div id=\"pagedata\" data-blob=\"{blob}\"></div></body></html>");
        let options = parse_options(&html).unwrap();
        assert_eq!(options.categories[0].slug, "all");
        assert!(parse_options("<html></html>").is_err());
    }

    #[test]
    fn decodes_discover_result() {
        let json = r#"{"results":[{"result_type":"a","item_type":"a","item_id":824574161,"band_id":2632533392,
            "title":"Alien Metal","band_name":"King Gizzard","band_location":"Melbourne, Australia",
            "release_date":"2026-08-14 04:00:52 UTC","price":{"amount":0,"currency":"AUD","is_money":true},
            "is_set_price":false,"is_free_download":false,"track_count":8,"duration":2458.7,
            "item_url":"https://kinggizzard.bandcamp.com/album/alien-metal?from=discover_page",
            "featured_track":{"id":1,"title":"Kill","band_name":"KG","stream_url":"https://t4.bcbits.com/x","duration":231.5},
            "is_owned":null,"is_wishlisted":null},
            {"result_type":"s","item_type":"p","item_id":5,"band_id":6,"title":"Shirt","band_name":"Z","item_url":"https://z.bandcamp.com/merch/shirt"}],
            "result_count":1942109,"cursor":"AoMI"}"#;
        let response: DiscoverResponse = serde_json::from_str(json).unwrap();
        let album = &response.results[0];
        assert_eq!(album.tralbum().unwrap().kind, TralbumKind::Album);
        assert_eq!(
            album.clean_url(),
            "https://kinggizzard.bandcamp.com/album/alien-metal"
        );
        assert_eq!(album.release_date().unwrap().to_string(), "2026-08-14");
        assert_eq!(album.price_label(), "name your price");
        assert!(!album.is_owned);
        let merch = &response.results[1];
        assert!(merch.tralbum().is_none());
        assert_eq!(merch.price_label(), "");
        assert_eq!(response.result_count, Some(1942109));
    }
}
