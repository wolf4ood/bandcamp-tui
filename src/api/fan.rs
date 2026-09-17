//! A fan's collection and wishlist (`/api/fancollection/1/*`).
//!
//! Public profiles work without a cookie; the identity cookie adds private
//! items and the search endpoint for the logged-in fan.

use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use super::models::{CollectionPage, SearchItemsResponse};
use super::{ApiError, Client};

pub const PAGE_SIZE: u32 = 40;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    Collection,
    Wishlist,
}

impl ListKind {
    pub fn title(self) -> &'static str {
        match self {
            ListKind::Collection => "Collection",
            ListKind::Wishlist => "Wishlist",
        }
    }

    fn items_path(self) -> &'static str {
        match self {
            ListKind::Collection => "/api/fancollection/1/collection_items",
            ListKind::Wishlist => "/api/fancollection/1/wishlist_items",
        }
    }

    fn search_type(self) -> &'static str {
        match self {
            ListKind::Collection => "collection",
            ListKind::Wishlist => "wishlist",
        }
    }
}

/// Token that selects the newest page: everything older than "now".
pub fn initial_token() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{now}::a::")
}

#[derive(Serialize)]
struct ItemsRequest<'a> {
    fan_id: u64,
    older_than_token: &'a str,
    count: u32,
}

pub async fn list_items(
    client: &Client,
    kind: ListKind,
    fan_id: u64,
    older_than_token: &str,
    count: u32,
) -> Result<CollectionPage, ApiError> {
    let body = ItemsRequest {
        fan_id,
        older_than_token,
        count,
    };
    client.post_json(kind.items_path(), &body).await
}

#[derive(Serialize)]
struct SearchRequest<'a> {
    fan_id: u64,
    search_key: &'a str,
    search_type: &'a str,
}

pub async fn search_items(
    client: &Client,
    kind: ListKind,
    fan_id: u64,
    search_key: &str,
) -> Result<SearchItemsResponse, ApiError> {
    let body = SearchRequest {
        fan_id,
        search_key,
        search_type: kind.search_type(),
    };
    client
        .post_json("/api/fancollection/1/search_items", &body)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_token_has_bandcamp_shape() {
        let token = initial_token();
        let parts: Vec<&str> = token.split(':').collect();
        assert_eq!(parts.len(), 5, "{token}");
        assert!(parts[0].parse::<u64>().is_ok());
        assert_eq!(parts[2], "a");
    }
}
