//! Bandcamp Daily (daily.bandcamp.com): the editorial site.
//!
//! There is no JSON API for it, but the pages are server-rendered and easy to
//! read: listing pages are `div.list-article` cards, and every article carries a
//! `data-player-infos` attribute describing the embedded players: each one
//! features a track and already carries its signed stream URL, so the article's
//! tracks play without another request. Nothing here needs a login.

use std::collections::HashMap;
use std::time::Duration;

use chrono::NaiveDate;
use scraper::{ElementRef, Html, Selector};
use serde::Deserialize;

use super::models::{TralbumKind, TralbumRef};
use super::{ApiError, Client};

pub const DAILY_URL: &str = "https://daily.bandcamp.com";

/// A Daily section ("franchise" in the site's own markup).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Franchise {
    #[default]
    Lists,
    AlbumOfTheDay,
    Features,
    SceneReport,
    BigUps,
    EssentialReleases,
    LabelProfile,
    /// Every section, newest first (`/latest`).
    All,
}

impl Franchise {
    /// Tab order: Lists first (the user's favourite), everything last.
    pub const ALL: [Franchise; 8] = [
        Franchise::Lists,
        Franchise::AlbumOfTheDay,
        Franchise::Features,
        Franchise::SceneReport,
        Franchise::BigUps,
        Franchise::EssentialReleases,
        Franchise::LabelProfile,
        Franchise::All,
    ];

    /// The path segment of the listing page.
    pub fn slug(self) -> &'static str {
        match self {
            Franchise::All => "latest",
            Franchise::AlbumOfTheDay => "album-of-the-day",
            Franchise::Lists => "lists",
            Franchise::Features => "features",
            Franchise::SceneReport => "scene-report",
            Franchise::BigUps => "big-ups",
            Franchise::EssentialReleases => "essential-releases",
            Franchise::LabelProfile => "label-profile",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Franchise::All => "All",
            Franchise::AlbumOfTheDay => "Album of the Day",
            Franchise::Lists => "Lists",
            Franchise::Features => "Features",
            Franchise::SceneReport => "Scene Report",
            Franchise::BigUps => "Big Ups",
            Franchise::EssentialReleases => "Essential Releases",
            Franchise::LabelProfile => "Label Profile",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Self> {
        let slug = slug.trim_matches('/');
        Self::ALL
            .into_iter()
            .filter(|f| *f != Franchise::All)
            .find(|f| f.slug() == slug)
    }

    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }

    pub fn prev(self) -> Self {
        let i = Self::ALL.iter().position(|f| *f == self).unwrap_or(0);
        Self::ALL[(i + Self::ALL.len() - 1) % Self::ALL.len()]
    }
}

/// One card on a listing page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Article {
    pub url: String,
    pub title: String,
    /// The section label as printed ("Album of the Day", "Lists", …).
    pub franchise: String,
    pub date: Option<NaiveDate>,
    pub art_url: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ArticlePage {
    pub articles: Vec<Article>,
    pub next_page: Option<u32>,
}

/// The track an embedded player features, as the website plays it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeaturedTrack {
    pub track_id: u64,
    pub title: String,
    pub artist: String,
    /// The parent album's title (or the track's own when it is a single).
    pub album: String,
    /// The parent album or single: for the album view, follow, wishlist and URL refresh.
    pub tralbum: TralbumRef,
    /// The Bandcamp page of the album or single.
    pub url: String,
    pub art_id: Option<u64>,
    pub duration: Duration,
    /// Signed mp3-128 URL, valid for about a day; `None` when not streamable.
    pub stream_url: Option<String>,
    pub track_number: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArticleBlock {
    Heading(String),
    Paragraph(String),
}

#[derive(Debug, Clone, Default)]
pub struct ArticleDetail {
    pub blurb: Option<String>,
    pub author: Option<String>,
    pub body: Vec<ArticleBlock>,
    pub featured: Vec<FeaturedTrack>,
}

/// One page of a section listing; pages start at 1.
pub async fn list(
    client: &Client,
    franchise: Franchise,
    page: u32,
) -> Result<ArticlePage, ApiError> {
    let url = format!("{DAILY_URL}/{}?page={page}", franchise.slug());
    let html = client.get_html(&url).await?;
    Ok(parse_list(&html, page))
}

pub async fn article(client: &Client, url: &str) -> Result<ArticleDetail, ApiError> {
    let html = client.get_html(url).await?;
    parse_article(&html, url)
}

// ----- listing pages ---------------------------------------------------------------

fn parse_list(html: &str, page: u32) -> ArticlePage {
    let document = Html::parse_document(html);
    let card = selector("div.list-article");
    let title = selector("a.title");
    let franchise = selector("a.franchise");
    let info = selector(".article-info-text");
    let thumb = selector("a.thumb img");
    let pagination = selector("a.pagination-link");

    let articles: Vec<Article> = document
        .select(&card)
        .filter_map(|card| {
            let link = card.select(&title).next()?;
            let href = link.value().attr("href")?;
            let title = text_of(link);
            if title.is_empty() {
                return None;
            }
            let franchise = card
                .select(&franchise)
                .next()
                .map(|f| {
                    f.value()
                        .attr("href")
                        .and_then(Franchise::from_slug)
                        .map(|f| f.label().to_owned())
                        .unwrap_or_else(|| title_case(&text_of(f)))
                })
                .unwrap_or_default();
            let date = card
                .select(&info)
                .next()
                .and_then(|info| parse_card_date(&text_of(info)));
            let art_url = card
                .select(&thumb)
                .next()
                .and_then(|img| img.value().attr("src"))
                .map(str::to_owned);
            Some(Article {
                url: absolute(href),
                title,
                franchise,
                date,
                art_url,
            })
        })
        .collect();
    let has_next = document.select(&pagination).next().is_some();
    ArticlePage {
        next_page: (has_next && !articles.is_empty()).then_some(page + 1),
        articles,
    }
}

/// "ALBUM OF THE DAY · September 17, 2026" → the date after the last separator.
fn parse_card_date(info: &str) -> Option<NaiveDate> {
    let tail = info.rsplit('·').next()?.trim();
    NaiveDate::parse_from_str(tail, "%B %d, %Y").ok()
}

// ----- article pages ---------------------------------------------------------------

/// The subset of a `data-player-infos` entry we care about; everything optional so a
/// markup change degrades to "no tracks" rather than a decode error.
#[derive(Deserialize, Default)]
#[serde(default)]
struct PlayerInfo {
    band_id: Option<u64>,
    band_name: Option<String>,
    parent_tralbum_id: Option<u64>,
    parent_tralbum_type: Option<String>,
    title: Option<String>,
    tralbum_url: Option<String>,
    art_id: Option<u64>,
    /// 1-based index into `tracklist`; the track the page's player shows.
    featured_track_number: Option<u32>,
    tracklist: Vec<PlayerTrack>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PlayerTrack {
    track_id: Option<u64>,
    track_title: Option<String>,
    artist: Option<String>,
    art_id: Option<u64>,
    audio_track_duration: Option<f64>,
    audio_url: HashMap<String, String>,
    track_number: Option<u32>,
}

impl PlayerInfo {
    /// The featured track, or the first one when the number is missing or off.
    fn featured(&self) -> Option<&PlayerTrack> {
        self.featured_track_number
            .and_then(|n| self.tracklist.iter().find(|t| t.track_number == Some(n)))
            .or_else(|| self.tracklist.first())
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LinkedData {
    description: Option<String>,
    author: Option<LinkedAuthor>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct LinkedAuthor {
    name: Option<String>,
}

fn parse_article(html: &str, path: &str) -> Result<ArticleDetail, ApiError> {
    let document = Html::parse_document(html);
    let players = selector("[data-player-infos]");
    let Some(blob) = document
        .select(&players)
        .next()
        .and_then(|e| e.value().attr("data-player-infos"))
    else {
        return Err(ApiError::Decode {
            path: path.to_owned(),
            message: "no data-player-infos attribute found".to_owned(),
            body: String::new(),
        });
    };
    let infos: Vec<PlayerInfo> = serde_json::from_str(blob).map_err(|err| ApiError::Decode {
        path: path.to_owned(),
        message: err.to_string(),
        body: String::new(),
    })?;

    let mut featured: Vec<FeaturedTrack> = Vec::new();
    for info in &infos {
        let (Some(band_id), Some(id)) = (info.band_id, info.parent_tralbum_id) else {
            continue;
        };
        let Some(track) = info.featured() else {
            continue;
        };
        let Some(track_id) = track.track_id else {
            continue;
        };
        if featured.iter().any(|f| f.track_id == track_id) {
            continue;
        }
        let kind = TralbumKind::parse(info.parent_tralbum_type.as_deref().unwrap_or("a"));
        let album = info.title.clone().unwrap_or_default();
        featured.push(FeaturedTrack {
            track_id,
            title: track.track_title.clone().unwrap_or_else(|| album.clone()),
            artist: track
                .artist
                .clone()
                .filter(|s| !s.is_empty())
                .or_else(|| info.band_name.clone())
                .unwrap_or_default(),
            album,
            tralbum: TralbumRef { band_id, id, kind },
            url: info.tralbum_url.clone().unwrap_or_default(),
            art_id: track.art_id.or(info.art_id),
            duration: Duration::from_secs_f64(track.audio_track_duration.unwrap_or(0.0).max(0.0)),
            stream_url: track.audio_url.get("mp3-128").cloned(),
            track_number: track.track_number.unwrap_or(1),
        });
    }

    let body = document
        .select(&selector("article p, article h3"))
        .filter_map(|e| {
            let text = text_of(e);
            if text.is_empty() {
                return None;
            }
            Some(match e.value().name() {
                "h3" => ArticleBlock::Heading(text),
                _ => ArticleBlock::Paragraph(text),
            })
        })
        .collect();

    let linked: LinkedData = document
        .select(&selector(r#"script[type="application/ld+json"]"#))
        .filter_map(|s| serde_json::from_str(&s.text().collect::<String>()).ok())
        .next()
        .unwrap_or_default();
    let blurb = document
        .select(&selector(r#"meta[property="og:description"]"#))
        .next()
        .and_then(|m| m.value().attr("content"))
        .map(collapse)
        .filter(|s| !s.is_empty())
        .or(linked.description);
    let author = linked
        .author
        .and_then(|a| a.name)
        .map(|s| collapse(&s))
        .filter(|s| !s.is_empty());

    Ok(ArticleDetail {
        blurb,
        author,
        body,
        featured,
    })
}

// ----- helpers ------------------------------------------------------------------------

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("static selector")
}

fn text_of(element: ElementRef<'_>) -> String {
    collapse(&element.text().collect::<String>())
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn absolute(href: &str) -> String {
    if href.starts_with("http://") || href.starts_with("https://") {
        href.to_owned()
    } else {
        format!("{DAILY_URL}/{}", href.trim_start_matches('/'))
    }
}

/// "ALBUM OF THE DAY" → "Album Of The Day" (only for sections we do not know by slug).
fn title_case(text: &str) -> String {
    text.split_whitespace()
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => {
                    first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase()
                }
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = r#"
    <html><body>
      <div class="list-article  aotd">
        <a href="/album-of-the-day/actress-radical-frame-review" class="thumb aotd-image">
          <img src="https://f4.bcbits.com/img/0047292430_2.jpg">
        </a>
        <div class="article-info-text">
          <a class="franchise" href="/album-of-the-day">ALBUM OF THE DAY</a>
          <span class="middot">&middot;</span>
          September 17, 2026
        </div>
        <div class="title-wrapper"><a class="title" href="/album-of-the-day/actress-radical-frame-review">Actress, “Radical Frame”</a></div>
      </div>
      <div class="list-article ">
        <a href="/best-soul/best-soul-sept" class="thumb ">
          <img src="https://f4.bcbits.com/img/1_150.jpg">
        </a>
        <div class="article-info-text">
          <a class="franchise" href="/best-soul">BEST SOUL</a>
          <span class="middot">&middot;</span>
          not a date
        </div>
        <div class="title-wrapper"><a class="title" href="https://daily.bandcamp.com/best-soul/best-soul-sept">The Best Soul</a></div>
      </div>
      <a class="pagination-link" href="/latest?page=2">older</a>
    </body></html>"#;

    #[test]
    fn listing_cards_become_articles() {
        let page = parse_list(LISTING, 1);
        assert_eq!(page.next_page, Some(2));
        assert_eq!(page.articles.len(), 2);
        let first = &page.articles[0];
        assert_eq!(
            first.url,
            "https://daily.bandcamp.com/album-of-the-day/actress-radical-frame-review"
        );
        assert_eq!(first.title, "Actress, “Radical Frame”");
        assert_eq!(first.franchise, "Album of the Day");
        assert_eq!(first.date, NaiveDate::from_ymd_opt(2026, 9, 17));
        assert_eq!(
            first.art_url.as_deref(),
            Some("https://f4.bcbits.com/img/0047292430_2.jpg")
        );
        let second = &page.articles[1];
        assert_eq!(
            second.url,
            "https://daily.bandcamp.com/best-soul/best-soul-sept"
        );
        assert_eq!(second.franchise, "Best Soul");
        assert_eq!(second.date, None);
    }

    #[test]
    fn last_page_has_no_continuation() {
        let page = parse_list("<html><body><p>nothing</p></body></html>", 3);
        assert!(page.articles.is_empty());
        assert_eq!(page.next_page, None);
    }

    const ARTICLE: &str = r#"
    <html><head>
      <meta property="og:description" content="Another restless   electronic masterpiece.">
      <script type="application/ld+json">{"@type":"Article","headline":"Actress","author":{"@type":"Person","name":"April Clare Welsh"}}</script>
    </head><body>
      <div data-player-infos="[
        {&quot;player_id&quot;:&quot;t2&quot;,&quot;band_id&quot;:42,&quot;band_name&quot;:&quot;Actress&quot;,&quot;parent_tralbum_id&quot;:1559192766,&quot;parent_tralbum_type&quot;:&quot;a&quot;,&quot;title&quot;:&quot;Radical Frame&quot;,&quot;tralbum_url&quot;:&quot;https://actress.bandcamp.com/album/radical-frame&quot;,&quot;art_id&quot;:1290224174,&quot;featured_track_number&quot;:2,
         &quot;tracklist&quot;:[{&quot;track_id&quot;:1,&quot;track_title&quot;:&quot;The Experiment&quot;,&quot;track_number&quot;:1,&quot;audio_track_duration&quot;:12.0,&quot;audio_url&quot;:{&quot;mp3-128&quot;:&quot;https://t4.bcbits.com/stream/x/mp3-128/1?token=a&quot;}},
                            {&quot;track_id&quot;:2,&quot;track_title&quot;:&quot;Sheets (feat. Rainy Miller)&quot;,&quot;artist&quot;:&quot;Actress&quot;,&quot;track_number&quot;:2,&quot;audio_track_duration&quot;:984.175,&quot;audio_url&quot;:{&quot;mp3-128&quot;:&quot;https://t4.bcbits.com/stream/y/mp3-128/2?token=b&quot;}}]},
        {&quot;player_id&quot;:&quot;t2&quot;,&quot;band_id&quot;:42,&quot;parent_tralbum_id&quot;:1559192766,&quot;parent_tralbum_type&quot;:&quot;a&quot;,&quot;featured_track_number&quot;:2,&quot;tracklist&quot;:[{&quot;track_id&quot;:2,&quot;track_number&quot;:2}]},
        {&quot;band_id&quot;:7,&quot;parent_tralbum_id&quot;:99,&quot;parent_tralbum_type&quot;:&quot;t&quot;,&quot;title&quot;:&quot;Single&quot;,&quot;band_name&quot;:&quot;Solo&quot;,&quot;tracklist&quot;:[{&quot;track_id&quot;:99,&quot;track_number&quot;:1,&quot;audio_url&quot;:{}}]},
        {&quot;band_id&quot;:8,&quot;parent_tralbum_id&quot;:100,&quot;tracklist&quot;:[]},
        {&quot;title&quot;:&quot;no ids&quot;}
      ]"></div>
      <article class="aotd">
        <h3>Ignored player heading? No: headings are kept</h3>
        <p>First   paragraph.</p>
        <div class="mplayer"><span>player chrome</span></div>
        <p>  </p>
        <p>Second paragraph.</p>
      </article>
    </body></html>"#;

    #[test]
    fn article_yields_featured_tracks_text_and_metadata() {
        let detail = parse_article(ARTICLE, "test").unwrap();
        assert_eq!(
            detail.blurb.as_deref(),
            Some("Another restless electronic masterpiece.")
        );
        assert_eq!(detail.author.as_deref(), Some("April Clare Welsh"));
        assert_eq!(
            detail.body,
            vec![
                ArticleBlock::Heading("Ignored player heading? No: headings are kept".into()),
                ArticleBlock::Paragraph("First paragraph.".into()),
                ArticleBlock::Paragraph("Second paragraph.".into()),
            ]
        );
        assert_eq!(
            detail.featured.len(),
            2,
            "duplicate track dropped, empty and id-less players skipped: {:?}",
            detail.featured
        );
        let track = &detail.featured[0];
        assert_eq!(track.track_id, 2, "the featured track, not the first");
        assert_eq!(track.title, "Sheets (feat. Rainy Miller)");
        assert_eq!(track.artist, "Actress");
        assert_eq!(track.album, "Radical Frame");
        assert_eq!(
            track.tralbum,
            TralbumRef {
                band_id: 42,
                id: 1559192766,
                kind: TralbumKind::Album
            }
        );
        assert_eq!(
            track.url,
            "https://actress.bandcamp.com/album/radical-frame"
        );
        assert_eq!(track.art_id, Some(1290224174));
        assert_eq!(track.duration, Duration::from_secs_f64(984.175));
        assert_eq!(
            track.stream_url.as_deref(),
            Some("https://t4.bcbits.com/stream/y/mp3-128/2?token=b")
        );
        assert_eq!(track.track_number, 2);

        let single = &detail.featured[1];
        assert_eq!(single.track_id, 99);
        assert_eq!(single.tralbum.kind, TralbumKind::Track);
        assert_eq!(single.title, "Single", "falls back to the player title");
        assert_eq!(single.artist, "Solo", "falls back to the band name");
        assert_eq!(single.stream_url, None);
    }

    #[test]
    fn article_without_players_is_a_decode_error() {
        let err = parse_article("<html><body><article><p>x</p></article></body></html>", "p")
            .unwrap_err();
        assert!(matches!(err, ApiError::Decode { .. }));
    }

    /// Hits daily.bandcamp.com; run with `cargo test daily -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn live_listing_and_article_parse() {
        let client = Client::new().unwrap();
        let page = list(&client, Franchise::AlbumOfTheDay, 1).await.unwrap();
        assert!(page.articles.len() > 5, "{page:?}");
        assert_eq!(page.next_page, Some(2));
        let first = &page.articles[0];
        assert!(
            first
                .url
                .starts_with("https://daily.bandcamp.com/album-of-the-day/")
        );
        assert_eq!(first.franchise, "Album of the Day");
        assert!(first.date.is_some(), "{first:?}");

        let detail = article(&client, &first.url).await.unwrap();
        assert!(!detail.featured.is_empty(), "{detail:?}");
        let track = &detail.featured[0];
        assert!(track.tralbum.band_id > 0 && track.track_id > 0, "{track:?}");
        assert!(track.stream_url.is_some(), "{track:?}");
        assert!(track.duration > Duration::ZERO, "{track:?}");
        assert!(
            !track.title.is_empty() && !track.album.is_empty(),
            "{track:?}"
        );
        assert!(detail.body.len() >= 2, "{detail:?}");
        assert!(detail.blurb.is_some());
        assert!(detail.author.is_some());

        let lists = list(&client, Franchise::Lists, 2).await.unwrap();
        assert!(!lists.articles.is_empty());
        let detail = article(&client, &lists.articles[0].url).await.unwrap();
        assert!(detail.featured.len() > 1, "{detail:?}");
        assert!(
            detail
                .body
                .iter()
                .any(|b| matches!(b, ArticleBlock::Heading(_))),
            "lists carry per-album headings"
        );
    }
}
