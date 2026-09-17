//! Session validation. Login itself is "paste the browser's identity cookie".

use std::collections::HashSet;

use super::models::{CollectionSummaryResponse, TralbumKind};
use super::{ApiError, Client};

/// What we know about the logged-in fan after validating the cookie.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub fan_id: u64,
    pub username: String,
    pub url: String,
    pub item_count: usize,
    pub following_count: usize,
    /// Owned albums and tracks, from the summary's `tralbum_lookup` keys (`a123`, `t456`).
    pub owned: HashSet<(TralbumKind, u64)>,
    /// From `follows.following` keys; bands are `b<id>` (or bare ids), fans `f<id>`.
    pub following_bands: HashSet<u64>,
    pub following_fans: HashSet<u64>,
}

/// Split Bandcamp's `<letter><id>` map keys.
fn split_key(key: &str) -> Option<(char, u64)> {
    let (prefix, digits) = match key.chars().next() {
        Some(c) if c.is_ascii_digit() => ('b', key),
        Some(c) => (c, &key[c.len_utf8()..]),
        None => return None,
    };
    digits.parse().ok().map(|id| (prefix, id))
}

/// Turn whatever the user pasted into the bare cookie value.
///
/// Accepts `identity=...`, a full `Cookie:` header fragment, or a quoted value.
pub fn normalize_cookie(raw: &str) -> String {
    let mut value = raw.trim();
    if let Some(rest) = value.strip_prefix("identity=") {
        value = rest;
    }
    if let Some((first, _)) = value.split_once(';') {
        value = first;
    }
    value.trim().trim_matches('"').trim_matches('\'').to_owned()
}

/// Ask Bandcamp who owns the cookie currently in the client's jar.
pub async fn validate_session(client: &Client) -> Result<Session, ApiError> {
    let response: CollectionSummaryResponse = client
        .get_json("/api/fan/2/collection_summary", &[])
        .await?;
    let Some(fan_id) = response.fan_id else {
        tracing::info!("collection_summary carried no fan_id: cookie not accepted");
        return Err(ApiError::NotLoggedIn);
    };
    let summary = response.collection_summary;
    let owned = summary
        .tralbum_lookup
        .iter()
        .filter_map(|(key, entry)| {
            let (prefix, id) = split_key(key)?;
            let kind = if entry.item_type.is_empty() {
                TralbumKind::parse(&prefix.to_string())
            } else {
                TralbumKind::parse(&entry.item_type)
            };
            Some((
                kind,
                if entry.item_id != 0 {
                    entry.item_id
                } else {
                    id
                },
            ))
        })
        .collect();
    let mut following_bands = HashSet::new();
    let mut following_fans = HashSet::new();
    for (key, active) in &summary.follows.following {
        if !active {
            continue;
        }
        match split_key(key) {
            Some(('f', id)) => {
                following_fans.insert(id);
            }
            Some((_, id)) => {
                following_bands.insert(id);
            }
            None => {}
        }
    }
    Ok(Session {
        fan_id,
        username: summary.username,
        url: summary.url,
        item_count: summary.tralbum_lookup.len(),
        following_count: following_bands.len(),
        owned,
        following_bands,
        following_fans,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_map_keys() {
        assert_eq!(split_key("b123"), Some(('b', 123)));
        assert_eq!(split_key("f9"), Some(('f', 9)));
        assert_eq!(split_key("42"), Some(('b', 42)));
        assert_eq!(split_key("x"), None);
    }

    #[test]
    fn normalizes_pasted_cookie_forms() {
        assert_eq!(normalize_cookie("  7%09abc%09def  "), "7%09abc%09def");
        assert_eq!(normalize_cookie("identity=7%09abc"), "7%09abc");
        assert_eq!(normalize_cookie("identity=7%09abc; Path=/"), "7%09abc");
        assert_eq!(normalize_cookie("\"7%09abc\""), "7%09abc");
        assert_eq!(normalize_cookie(""), "");
    }
}
