//! The fan's feed (`POST /fan_dash_feed_updates`): new releases from followed
//! artists and purchases by followed fans.
//!
//! The reply is only reachable when logged in, so the parser is deliberately
//! forgiving: it looks for the story list under a few plausible keys and reads
//! each story's fields with fallbacks. Unknown layouts are logged at debug level.

use chrono::NaiveDateTime;
use serde_json::{Value, json};

use super::models::{TralbumKind, TralbumRef, parse_bc_date};
use super::{ApiError, Client};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoryKind {
    /// A followed artist or label released something.
    NewRelease,
    /// A followed fan bought something.
    FanPurchase,
    Other,
}

#[derive(Debug, Clone)]
pub struct Story {
    pub kind: StoryKind,
    pub story_type: String,
    pub date: Option<NaiveDateTime>,
    pub band_id: Option<u64>,
    pub band_name: String,
    pub item: Option<TralbumRef>,
    pub item_title: String,
    pub item_url: Option<String>,
    pub fan_name: Option<String>,
    pub fan_id: Option<u64>,
    /// The fan's comment on a purchase.
    pub why: Option<String>,
    pub also_collected_count: Option<u64>,
}

impl Story {
    /// One-line summary like the website shows.
    pub fn headline(&self) -> String {
        match self.kind {
            StoryKind::NewRelease => format!("{} — {}", self.band_name, self.item_title),
            StoryKind::FanPurchase => format!(
                "{} bought {} — {}",
                self.fan_name.as_deref().unwrap_or("a fan"),
                self.band_name,
                self.item_title
            ),
            StoryKind::Other => {
                let who = self.fan_name.as_deref().unwrap_or(self.band_name.as_str());
                format!("{who}: {} — {}", self.band_name, self.item_title)
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct FeedPage {
    pub stories: Vec<Story>,
    /// Pass as `older_than` to get the next page.
    pub oldest_story_date: Option<f64>,
    /// Number of raw entries seen (to tell "empty feed" from "unrecognised layout").
    pub raw_entries: usize,
}

/// `older_than` is a unix timestamp in whole seconds; the first page uses "now".
pub async fn feed(client: &Client, fan_id: u64, older_than: i64) -> Result<FeedPage, ApiError> {
    let body = json!({ "fan_id": fan_id, "older_than": older_than });
    let value: Value = client
        .post_callback("/fan_dash_feed_updates", body.as_object().expect("object"))
        .await?;
    Ok(parse_feed(&value))
}

pub fn parse_feed(value: &Value) -> FeedPage {
    let stories_node = value.get("stories").unwrap_or(value);
    let entries = ["entries", "stories", "items", "feed"]
        .iter()
        .filter_map(|k| stories_node.get(k).or_else(|| value.get(k)))
        .find_map(Value::as_array)
        .cloned()
        .or_else(|| stories_node.as_array().cloned())
        .unwrap_or_default();
    if entries.is_empty() {
        let keys: Vec<&String> = value
            .as_object()
            .map(|m| m.keys().collect())
            .unwrap_or_default();
        tracing::debug!(?keys, "feed reply carried no stories");
    } else if let Some(first) = entries.first().and_then(Value::as_object) {
        let keys: Vec<&String> = first.keys().collect();
        tracing::debug!(?keys, n = entries.len(), "feed stories");
    }
    let oldest = ["oldest_story_date", "oldest", "older_than"]
        .iter()
        .filter_map(|k| stories_node.get(k).or_else(|| value.get(k)))
        .find_map(number);
    let stories = entries.iter().filter_map(parse_story).collect();
    FeedPage {
        stories,
        oldest_story_date: oldest,
        raw_entries: entries.len(),
    }
}

fn parse_story(entry: &Value) -> Option<Story> {
    let obj = entry.as_object()?;
    let text = |keys: &[&str]| {
        keys.iter()
            .find_map(|k| obj.get(*k).and_then(Value::as_str))
            .map(str::to_owned)
    };
    let num = |keys: &[&str]| keys.iter().find_map(|k| obj.get(*k).and_then(number));
    let story_type = text(&["story_type", "type"]).unwrap_or_default();
    let kind = match story_type.as_str() {
        "nr" | "new_release" | "release" => StoryKind::NewRelease,
        "fp" | "np" | "fan_purchase" | "purchase" | "collected" => StoryKind::FanPurchase,
        _ if obj.get("fan_id").and_then(number).is_some()
            && obj.get("band_id").and_then(number).is_some() =>
        {
            StoryKind::FanPurchase
        }
        _ => StoryKind::Other,
    };
    let item_id = num(&["tralbum_id", "item_id", "album_id", "track_id"]).map(|n| n as u64);
    let band_id = num(&["band_id"]).map(|n| n as u64);
    let item_kind = text(&["tralbum_type", "item_type"]).map(|t| TralbumKind::parse(&t));
    let item = match (band_id, item_id) {
        (Some(band_id), Some(id)) => Some(TralbumRef {
            band_id,
            id,
            kind: item_kind.unwrap_or(TralbumKind::Album),
        }),
        _ => None,
    };
    let date = obj
        .get("story_date")
        .or_else(|| obj.get("date"))
        .and_then(|v| match v {
            Value::String(s) => parse_bc_date(s),
            _ => number(v)
                .and_then(|n| chrono::DateTime::from_timestamp(n as i64, 0))
                .map(|d| d.naive_utc()),
        });
    let band_name = text(&["band_name", "artist", "artist_name"]).unwrap_or_default();
    let item_title =
        text(&["item_title", "album_title", "title", "track_title"]).unwrap_or_default();
    if band_name.is_empty() && item_title.is_empty() {
        return None;
    }
    Some(Story {
        kind,
        story_type,
        date,
        band_id,
        band_name,
        item,
        item_title,
        item_url: text(&["item_url", "url", "album_url", "track_url"]),
        fan_name: text(&["fan_name", "fan_username", "username", "name"])
            .filter(|_| kind != StoryKind::NewRelease),
        fan_id: num(&["fan_id"]).map(|n| n as u64),
        why: text(&["why", "message", "comment"]).filter(|w| !w.trim().is_empty()),
        also_collected_count: num(&["also_collected_count"]).map(|n| n as u64),
    })
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_plausible_feed_reply() {
        let json = r#"{"ok":true,"stories":{"entries":[
            {"story_type":"nr","story_date":"16 Sep 2026 07:25:05 GMT","band_id":2632533392,"band_name":"King Gizzard",
             "item_id":824574161,"item_type":"a","item_title":"Alien Metal","item_url":"https://kinggizzard.bandcamp.com/album/alien-metal"},
            {"story_type":"fp","story_date":1789554156,"fan_id":10162377,"fan_name":"The Seahorse","band_id":1982236960,
             "band_name":"Primus","tralbum_id":2346829034,"tralbum_type":"a","album_title":"A Handful of Nuggs","why":"Great!","also_collected_count":794},
            {"story_type":"zz"}
        ],"oldest_story_date":1789554156.5,"track_list":[]}}"#;
        let page = parse_feed(&serde_json::from_str(json).unwrap());
        assert_eq!(page.raw_entries, 3);
        assert_eq!(page.stories.len(), 2);
        assert_eq!(page.oldest_story_date, Some(1789554156.5));
        let nr = &page.stories[0];
        assert_eq!(nr.kind, StoryKind::NewRelease);
        assert_eq!(nr.headline(), "King Gizzard — Alien Metal");
        assert_eq!(nr.item.unwrap().id, 824574161);
        assert_eq!(nr.date.unwrap().to_string(), "2026-09-16 07:25:05");
        let fp = &page.stories[1];
        assert_eq!(fp.kind, StoryKind::FanPurchase);
        assert_eq!(
            fp.headline(),
            "The Seahorse bought Primus — A Handful of Nuggs"
        );
        assert_eq!(fp.why.as_deref(), Some("Great!"));
        assert!(fp.date.is_some());
    }

    #[test]
    fn tolerates_a_flat_list() {
        let page = parse_feed(
            &serde_json::from_str(
                r#"{"stories":[{"band_name":"X","title":"Y","band_id":1,"item_id":2}]}"#,
            )
            .unwrap(),
        );
        assert_eq!(page.stories.len(), 1);
        assert_eq!(page.stories[0].kind, StoryKind::Other);
        assert!(
            parse_feed(&serde_json::json!({"ok": true}))
                .stories
                .is_empty()
        );
    }
}
