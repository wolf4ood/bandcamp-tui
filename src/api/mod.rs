//! Thin client for the undocumented JSON endpoints behind bandcamp.com.
//!
//! Bandcamp has no public fan API; these are the same calls the website and the
//! mobile app make. Keep every endpoint in this module so a change on their side
//! is a one-place fix.

pub mod auth;
pub mod band;
pub mod daily;
pub mod discover;
pub mod fan;
pub mod feed;
pub mod models;
pub mod search;
pub mod social;
pub mod tralbum;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::Url;
use reqwest::cookie::Jar;
use serde::Serialize;
use serde::de::DeserializeOwned;
use thiserror::Error;

pub const BASE_URL: &str = "https://bandcamp.com";

/// Bandcamp serves everything to a browser-like agent; keep the app name in it for honesty.
const USER_AGENT: &str = concat!(
    "Mozilla/5.0 (X11; Linux x86_64; rv:130.0) Gecko/20100101 Firefox/130.0 bandcamp-tui/",
    env!("CARGO_PKG_VERSION")
);

#[derive(Debug, Error)]
pub enum ApiError {
    #[error("not logged in")]
    NotLoggedIn,
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("bandcamp answered HTTP {status} for {path}")]
    Status {
        status: u16,
        path: String,
        body: String,
    },
    #[error("could not decode the response from {path}: {message}")]
    Decode {
        path: String,
        message: String,
        body: String,
    },
    /// Bandcamp's own `{"error": true, "error_message": "..."}` envelope (sent with HTTP 200).
    #[error("bandcamp: {0}")]
    Bandcamp(String),
    /// A mutating callback wants this anti-CSRF token in the body (internal; retried automatically).
    #[error("bandcamp asked for a fresh crumb")]
    InvalidCrumb(Option<String>),
}

impl ApiError {
    /// One line suitable for the status bar.
    pub fn user_message(&self) -> String {
        match self {
            ApiError::NotLoggedIn => "Bandcamp rejected the session cookie.".to_owned(),
            ApiError::Http(err) if err.is_timeout() => {
                "Timed out talking to bandcamp.com.".to_owned()
            }
            ApiError::Http(err) if err.is_connect() => {
                "Could not connect to bandcamp.com.".to_owned()
            }
            ApiError::Http(err) => format!("Network error: {err}"),
            ApiError::Status { status, .. } => format!("Bandcamp answered HTTP {status}."),
            ApiError::Decode { .. } => {
                "Unexpected response from Bandcamp (see the log).".to_owned()
            }
            ApiError::Bandcamp(message) => format!("Bandcamp says: {message}"),
            ApiError::InvalidCrumb(_) => "Bandcamp rejected the request token.".to_owned(),
        }
    }
}

/// Cheap to clone; clones share the HTTP pool and the cookie jar.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    jar: Arc<Jar>,
    /// Anti-CSRF tokens per callback name, scraped from pages and refreshed from
    /// `invalid_crumb` replies.
    crumbs: Arc<Mutex<HashMap<String, String>>>,
    /// Pages whose `#js-crumbs-data` element carries this session's crumbs.
    crumb_pages: Arc<Mutex<Vec<String>>>,
}

impl Client {
    pub fn new() -> Result<Self, reqwest::Error> {
        let jar = Arc::new(Jar::default());
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .cookie_provider(jar.clone())
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            http,
            jar,
            crumbs: Arc::new(Mutex::new(HashMap::new())),
            crumb_pages: Arc::new(Mutex::new(Vec::new())),
        })
    }

    /// Pages to scrape crumbs from once logged in (the fan's profile and feed).
    pub fn set_crumb_pages(&self, pages: Vec<String>) {
        *self.crumb_pages.lock().expect("crumb lock") = pages;
    }

    fn crumb_for(&self, path: &str) -> Option<String> {
        self.crumbs
            .lock()
            .expect("crumb lock")
            .get(path.trim_start_matches('/'))
            .cloned()
    }

    fn remember_crumb(&self, path: &str, crumb: String) {
        self.crumbs
            .lock()
            .expect("crumb lock")
            .insert(path.trim_start_matches('/').to_owned(), crumb);
    }

    /// Fetch the crumb pages and absorb whatever `#js-crumbs-data` holds.
    async fn refresh_crumbs(&self) -> Result<usize, ApiError> {
        let pages = self.crumb_pages.lock().expect("crumb lock").clone();
        let mut learned = 0;
        for page in pages {
            let html = self.get_html(&page).await?;
            let found = parse_crumbs(&html);
            tracing::debug!(page, n = found.len(), "crumbs scraped");
            learned += found.len();
            let mut crumbs = self.crumbs.lock().expect("crumb lock");
            crumbs.extend(found);
        }
        Ok(learned)
    }

    pub fn url(path: &str) -> Url {
        Url::parse(BASE_URL)
            .expect("BASE_URL is valid")
            .join(path)
            .expect("endpoint paths are valid")
    }

    /// Install (or replace) the `identity` session cookie.
    pub fn set_identity_cookie(&self, value: &str) {
        let cookie = format!("identity={value}; Domain=bandcamp.com; Path=/; Secure; HttpOnly");
        self.jar.add_cookie_str(&cookie, &Self::url("/"));
    }

    pub async fn get_json<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<T, ApiError> {
        let response = self.http.get(Self::url(path)).query(query).send().await?;
        self.decode(path, response).await
    }

    pub async fn post_json<T: DeserializeOwned, B: Serialize + ?Sized>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let response = self.http.post(Self::url(path)).json(body).send().await?;
        self.decode(path, response).await
    }

    #[allow(dead_code)] // first used by the page scrapers in step 2
    /// POST to one of the site's legacy callbacks (follow, wishlist, feed).
    ///
    /// These are form-encoded (the site's JavaScript sends `FormData`; a JSON
    /// body is ignored and yields "missing or mismatched fan_id"), want an
    /// `Origin` header and a per-path `crumb`. A stale crumb is answered with
    /// `{"error":"invalid_crumb","crumb":...}`, which we remember and retry with.
    pub async fn post_callback<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Map<String, serde_json::Value>,
    ) -> Result<T, ApiError> {
        let mut body = body.clone();
        let mut refreshed = false;
        if self.crumb_for(path).is_none()
            && !self.crumb_pages.lock().expect("crumb lock").is_empty()
        {
            self.refresh_crumbs().await?;
            refreshed = true;
        }
        for attempt in 0..3 {
            if let Some(crumb) = self.crumb_for(path) {
                body.insert("crumb".to_owned(), serde_json::Value::String(crumb));
            }
            let form = form_fields(&body);
            let response = self
                .http
                .post(Self::url(path))
                .header("Origin", BASE_URL)
                .header("Referer", format!("{BASE_URL}/"))
                .header("X-Requested-With", "XMLHttpRequest")
                .form(&form)
                .send()
                .await?;
            match self.decode(path, response).await {
                Err(ApiError::InvalidCrumb(Some(crumb))) if attempt < 2 => {
                    tracing::debug!(path, attempt, "retrying with the crumb from the reply");
                    self.remember_crumb(path, crumb);
                }
                Err(ApiError::InvalidCrumb(None)) if !refreshed && attempt < 2 => {
                    tracing::debug!(path, attempt, "retrying after scraping crumbs");
                    refreshed = true;
                    if self.refresh_crumbs().await? == 0 {
                        return Err(ApiError::InvalidCrumb(None));
                    }
                }
                other => return other,
            }
        }
        Err(ApiError::InvalidCrumb(None))
    }

    pub async fn get_html(&self, path: &str) -> Result<String, ApiError> {
        let response = self.http.get(Self::url(path)).send().await?;
        let status = response.status();
        let body = response.text().await?;
        tracing::debug!(
            path,
            status = status.as_u16(),
            bytes = body.len(),
            "GET html"
        );
        check_status(path, status, body)
    }

    async fn decode<T: DeserializeOwned>(
        &self,
        path: &str,
        response: reqwest::Response,
    ) -> Result<T, ApiError> {
        let status = response.status();
        let body = response.text().await?;
        tracing::debug!(
            path,
            status = status.as_u16(),
            bytes = body.len(),
            "json request"
        );
        let body = check_status(path, status, body)?;
        check_error_envelope(path, &body)?;
        serde_json::from_str(&body).map_err(|err| {
            tracing::warn!(path, %err, body = snippet(&body), "decode failed");
            ApiError::Decode {
                path: path.to_owned(),
                message: err.to_string(),
                body: snippet(&body),
            }
        })
    }
}

fn check_status(path: &str, status: reqwest::StatusCode, body: String) -> Result<String, ApiError> {
    if status == reqwest::StatusCode::FORBIDDEN && body.contains("invalid_crumb") {
        // The callbacks send their crumb challenge with a 403.
        return check_error_envelope(path, &body).map(|()| body);
    }
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err(ApiError::NotLoggedIn);
    }
    if !status.is_success() {
        tracing::warn!(
            path,
            status = status.as_u16(),
            body = snippet(&body),
            "request failed"
        );
        return Err(ApiError::Status {
            status: status.as_u16(),
            path: path.to_owned(),
            body: snippet(&body),
        });
    }
    Ok(body)
}

/// Bandcamp reports many failures as HTTP 200 with an error envelope. Three shapes
/// exist: `{"error":true,"error_message":...}`, `{"error":true,"exception":...}`
/// from the `*_cb` callbacks (or `{"error":"invalid_crumb","crumb":...}`), and
/// `{"__api_special__":"exception","error_type":...}` from the newest endpoints.
fn check_error_envelope(path: &str, body: &str) -> Result<(), ApiError> {
    use serde_json::Value;
    let Ok(Value::Object(map)) = serde_json::from_str::<Value>(body) else {
        return Ok(()); // not an object: let the real decoder judge
    };
    let text = |key: &str| map.get(key).and_then(Value::as_str).map(str::to_owned);
    let message = match map.get("error") {
        Some(Value::String(code)) if code == "invalid_crumb" => {
            return Err(ApiError::InvalidCrumb(text("crumb")));
        }
        Some(Value::String(code)) => Some(code.clone()),
        Some(Value::Bool(true)) => Some(
            text("error_message")
                .or_else(|| text("exception"))
                .unwrap_or_else(|| "unknown error".to_owned()),
        ),
        _ if map.contains_key("__api_special__") => {
            Some(text("error_type").unwrap_or_else(|| "exception".to_owned()))
        }
        _ => None,
    };
    let Some(message) = message else {
        return Ok(());
    };
    tracing::warn!(path, %message, "bandcamp error envelope");
    let lower = message.to_ascii_lowercase();
    if lower.contains("logged in") {
        return Err(ApiError::NotLoggedIn);
    }
    if lower.contains("crumb") {
        // "old or no crumb specified for request ... rejecting"
        return Err(ApiError::InvalidCrumb(text("crumb")));
    }
    Err(ApiError::Bandcamp(message))
}

/// Flatten a JSON object into form fields the way `FormData.append(k, v.toString())` would.
fn form_fields(body: &serde_json::Map<String, serde_json::Value>) -> Vec<(String, String)> {
    body.iter()
        .map(|(k, v)| {
            let value = match v {
                serde_json::Value::String(s) => s.clone(),
                serde_json::Value::Null => String::new(),
                other => other.to_string(),
            };
            (k.clone(), value)
        })
        .collect()
}

/// The `<div id="js-crumbs-data" data-crumbs="{...}">` map: callback name → crumb.
fn parse_crumbs(html: &str) -> HashMap<String, String> {
    let document = scraper::Html::parse_document(html);
    let selector = scraper::Selector::parse("#js-crumbs-data").expect("static selector");
    document
        .select(&selector)
        .filter_map(|el| el.value().attr("data-crumbs"))
        .filter_map(|json| serde_json::from_str::<HashMap<String, serde_json::Value>>(json).ok())
        .flatten()
        .filter_map(|(k, v)| {
            v.as_str()
                .map(|s| (k.trim_start_matches('/').to_owned(), s.to_owned()))
        })
        .collect()
}

fn snippet(body: &str) -> String {
    body.chars().take(200).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_urls_against_the_site_root() {
        assert_eq!(
            Client::url("/api/fan/2/collection_summary").as_str(),
            "https://bandcamp.com/api/fan/2/collection_summary"
        );
        assert_eq!(Client::url("/").as_str(), "https://bandcamp.com/");
    }

    #[test]
    fn maps_logged_out_envelope_to_not_logged_in() {
        let err = check_error_envelope(
            "/x",
            r#"{"error":true,"error_message":"must be logged in"}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::NotLoggedIn));

        let err = check_error_envelope("/x", r#"{"error":true,"error_message":"no such fan"}"#)
            .unwrap_err();
        assert!(matches!(err, ApiError::Bandcamp(m) if m == "no such fan"));

        assert!(check_error_envelope("/x", r#"{"fan_id": 1}"#).is_ok());
        assert!(check_error_envelope("/x", r#"[1,2]"#).is_ok());

        let err = check_error_envelope(
            "/x",
            r#"{"__api_special__":"exception","error_type":"Endpoints::MissingParamError"}"#,
        )
        .unwrap_err();
        assert!(matches!(err, ApiError::Bandcamp(m) if m == "Endpoints::MissingParamError"));

        let err = check_error_envelope("/x", r#"{"error":true,"ok":false,"exception":"exception: InsistError: Failed insist: not logged in"}"#)
            .unwrap_err();
        assert!(matches!(err, ApiError::NotLoggedIn));

        let err = check_error_envelope("/x", r#"{"error":"invalid_crumb","crumb":"abc|123"}"#)
            .unwrap_err();
        assert!(matches!(err, ApiError::InvalidCrumb(Some(c)) if c == "abc|123"));

        assert!(check_error_envelope("/x", r#"{"ok":true,"error":false}"#).is_ok());

        let err = check_error_envelope("/x", r#"{"error":true,"ok":false,"exception":"exception: InsistError: Failed insist: old or no crumb specified for request to '/fan_follow_band_cb', which requires nucrumb -- rejecting"}"#)
            .unwrap_err();
        assert!(matches!(err, ApiError::InvalidCrumb(None)));
    }

    #[test]
    fn form_fields_stringify_like_formdata() {
        let body = serde_json::json!({"fan_id": 10162377, "action": "follow", "older_than": 1789554156, "flag": true});
        let mut fields = form_fields(body.as_object().unwrap());
        fields.sort();
        assert_eq!(
            fields,
            vec![
                ("action".to_owned(), "follow".to_owned()),
                ("fan_id".to_owned(), "10162377".to_owned()),
                ("flag".to_owned(), "true".to_owned()),
                ("older_than".to_owned(), "1789554156".to_owned()),
            ]
        );
    }

    #[test]
    fn scrapes_crumbs_from_a_page() {
        let html = r#"<html><body><div id="js-crumbs-data" data-crumbs="{&quot;fan_follow_band_cb&quot;:&quot;abc|1&quot;,&quot;collect_item_cb&quot;:&quot;def|2&quot;}"></div></body></html>"#;
        let crumbs = parse_crumbs(html);
        assert_eq!(
            crumbs.get("fan_follow_band_cb").map(String::as_str),
            Some("abc|1")
        );
        assert_eq!(crumbs.len(), 2);
        assert!(parse_crumbs(r#"<div id="js-crumbs-data" data-crumbs="{}"></div>"#).is_empty());
        assert!(parse_crumbs("<html></html>").is_empty());
    }
}
