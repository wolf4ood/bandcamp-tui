//! Following (public lists) and the logged-in mutations: follow, wishlist.
//!
//! Mutations go through the site's `*_cb` callbacks exactly like the website's
//! follow and wishlist buttons do; see `Client::post_callback` for the crumb dance.

use serde::Deserialize;
use serde_json::{Map, Value, json};

use super::models::{TralbumRef, truthy};
use super::{ApiError, Client};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowList {
    Bands,
    Fans,
    Followers,
}

impl FollowList {
    pub fn title(self) -> &'static str {
        match self {
            FollowList::Bands => "Artists you follow",
            FollowList::Fans => "Fans you follow",
            FollowList::Followers => "Followers",
        }
    }

    fn path(self) -> &'static str {
        match self {
            FollowList::Bands => "/api/fancollection/1/following_bands",
            FollowList::Fans => "/api/fancollection/1/following_fans",
            FollowList::Followers => "/api/fancollection/1/followers",
        }
    }
}

/// One entry of a following / followers list (a band or a fan).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Followee {
    pub band_id: Option<u64>,
    pub fan_id: Option<u64>,
    pub name: String,
    pub location: Option<String>,
    pub image_id: Option<u64>,
    pub url_hints: UrlHints,
    /// Fan profile URL.
    pub trackpipe_url: Option<String>,
    pub date_followed: Option<String>,
    #[serde(deserialize_with = "truthy")]
    pub is_following: bool,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct UrlHints {
    pub subdomain: Option<String>,
    pub custom_domain: Option<String>,
}

impl Followee {
    pub fn is_band(&self) -> bool {
        self.band_id.is_some()
    }

    pub fn url(&self) -> Option<String> {
        if let Some(url) = &self.trackpipe_url {
            return Some(url.clone());
        }
        if let Some(domain) = self
            .url_hints
            .custom_domain
            .as_deref()
            .filter(|d| !d.is_empty())
        {
            return Some(format!("https://{domain}"));
        }
        self.url_hints
            .subdomain
            .as_deref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("https://{s}.bandcamp.com"))
    }

    pub fn date_followed(&self) -> Option<chrono::NaiveDateTime> {
        self.date_followed
            .as_deref()
            .and_then(super::models::parse_bc_date)
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FollowPage {
    /// Bandcamp's spelling.
    pub followeers: Vec<Followee>,
    #[serde(deserialize_with = "truthy")]
    pub more_available: bool,
    pub last_token: Option<String>,
}

pub async fn follow_list(
    client: &Client,
    list: FollowList,
    fan_id: u64,
    older_than_token: &str,
    count: u32,
) -> Result<FollowPage, ApiError> {
    let body = json!({ "fan_id": fan_id, "older_than_token": older_than_token, "count": count });
    client.post_json(list.path(), &body).await
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FollowTarget {
    Band(u64),
    Fan(u64),
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct CallbackReply {
    #[serde(deserialize_with = "truthy")]
    ok: bool,
}

/// Follow or unfollow a band or fan as `fan_id`.
pub async fn set_follow(
    client: &Client,
    fan_id: u64,
    target: FollowTarget,
    follow: bool,
) -> Result<(), ApiError> {
    let action = if follow { "follow" } else { "unfollow" };
    let (path, mut body) = match target {
        FollowTarget::Band(band_id) => (
            "/fan_follow_band_cb",
            object(&[("band_id", json!(band_id))]),
        ),
        FollowTarget::Fan(follow_id) => {
            ("/fan_follow_cb", object(&[("follow_id", json!(follow_id))]))
        }
    };
    body.insert("fan_id".into(), json!(fan_id));
    body.insert("action".into(), json!(action));
    let reply: CallbackReply = client.post_callback(path, &body).await?;
    if reply.ok {
        Ok(())
    } else {
        Err(ApiError::Bandcamp(format!("{action} was not accepted")))
    }
}

/// Add or remove an album / track from the fan's wishlist.
pub async fn set_wishlist(
    client: &Client,
    fan_id: u64,
    item: TralbumRef,
    wishlisted: bool,
) -> Result<(), ApiError> {
    let path = if wishlisted {
        "/collect_item_cb"
    } else {
        "/uncollect_item_cb"
    };
    let body = object(&[
        ("band_id", json!(item.band_id)),
        ("fan_id", json!(fan_id)),
        ("item_id", json!(item.id)),
        ("item_type", json!(item.kind.as_str())),
    ]);
    let reply: CallbackReply = client.post_callback(path, &body).await?;
    if reply.ok {
        Ok(())
    } else {
        Err(ApiError::Bandcamp(
            "wishlist change was not accepted".to_owned(),
        ))
    }
}

fn object(fields: &[(&str, Value)]) -> Map<String, Value> {
    fields
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Public following list of a real fan, plus the callback path without a
    /// session. Needs network: `cargo test -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn follow_list_and_callback_live() {
        use crate::api::fan::initial_token;
        let client = Client::new().unwrap();
        let page = follow_list(&client, FollowList::Bands, 10162377, &initial_token(), 5)
            .await
            .unwrap();
        assert_eq!(page.followeers.len(), 5);
        assert!(page.more_available);
        assert!(
            page.followeers
                .iter()
                .all(|f| f.is_band() && f.url().is_some())
        );
        eprintln!(
            "follows: {:?}",
            page.followeers.iter().map(|f| &f.name).collect::<Vec<_>>()
        );

        let err = set_follow(&client, 10162377, FollowTarget::Band(2632533392), true)
            .await
            .unwrap_err();
        eprintln!("logged-out follow -> {err}");
        assert!(matches!(
            err,
            ApiError::NotLoggedIn | ApiError::Bandcamp(_) | ApiError::InvalidCrumb(_)
        ));
    }

    #[test]
    fn decodes_follow_page() {
        let json = r#"{"followeers":[
            {"band_id":3323526727,"image_id":7855575,"url_hints":{"subdomain":"goldenlanemarley","custom_domain":null},
             "name":"Bob Marley","is_following":false,"location":"Nine Mile, Jamaica","date_followed":"16 Nov 2025 06:50:24 GMT","token":"1763275824:3323526727"},
            {"fan_id":5701775,"band_id":null,"fan_url":null,"image_id":20966887,"trackpipe_url":"https://bandcamp.com/yoshi333",
             "name":"yoshi333","is_following":false,"location":null,"date_followed":"05 Dec 2025 21:40:55 GMT","token":"1764970855:5701775"}
        ],"more_available":true,"last_token":"1763275824:3323526727"}"#;
        let page: FollowPage = serde_json::from_str(json).unwrap();
        assert_eq!(page.followeers.len(), 2);
        let band = &page.followeers[0];
        assert!(band.is_band());
        assert_eq!(
            band.url().as_deref(),
            Some("https://goldenlanemarley.bandcamp.com")
        );
        assert_eq!(
            band.date_followed().unwrap().to_string(),
            "2025-11-16 06:50:24"
        );
        let fan = &page.followeers[1];
        assert!(!fan.is_band());
        assert_eq!(fan.url().as_deref(), Some("https://bandcamp.com/yoshi333"));
        assert!(page.more_available);
    }
}
