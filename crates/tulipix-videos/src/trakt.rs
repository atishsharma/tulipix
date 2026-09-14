//! Trakt.tv — what you watched, sent to your own Trakt account.
//!
//! Trakt has no plain API key: every request is signed with an app's client
//! id, and anything about *your* account needs an access token. The token
//! comes from the device flow, which is the one flow that works in an app
//! with no browser redirect of its own:
//!
//!   1. POST `/oauth/device/code` with the client id → a short user code and
//!      a URL to type it into;
//!   2. the user opens that URL and types the code;
//!   3. POST `/oauth/device/token` every few seconds until it stops answering
//!      `400 authorization_pending` and hands over the token.
//!
//! After that, a finished film or episode is one POST to `/sync/history`.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const TRAKT_BASE: &str = "https://api.trakt.tv";
pub const TRAKT_API_VERSION: &str = "2";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_url: String,
    /// Seconds the code is good for.
    pub expires_in: i64,
    /// Seconds to wait between polls. Polling faster earns a `429`.
    pub interval: i64,
}

/// What one poll of the token endpoint came back with.
#[derive(Debug, Clone, PartialEq)]
pub enum DevicePoll {
    /// The user has not typed the code in yet.
    Pending,
    /// Poll slower: `interval` was not respected.
    SlowDown,
    Token(String),
    Expired,
    Denied,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Movie,
    Episode,
}

pub struct TraktClient {
    pub client_id: String,
    pub client_secret: String,
    http: reqwest::Client,
}

impl TraktClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            http: tulipix_core::net::http().clone(),
        }
    }

    fn headers(&self, token: Option<&str>) -> reqwest::header::HeaderMap {
        use reqwest::header::{HeaderMap, HeaderValue, AUTHORIZATION, CONTENT_TYPE};
        let mut h = HeaderMap::new();
        h.insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
        h.insert("trakt-api-version", HeaderValue::from_static(TRAKT_API_VERSION));
        if let Ok(v) = HeaderValue::from_str(&self.client_id) {
            h.insert("trakt-api-key", v);
        }
        if let Some(t) = token {
            if let Ok(v) = HeaderValue::from_str(&format!("Bearer {t}")) {
                h.insert(AUTHORIZATION, v);
            }
        }
        h
    }

    /// Step one: ask for a code to show the user.
    pub async fn device_code(&self) -> Result<DeviceCode> {
        let v: serde_json::Value = self
            .http
            .post(format!("{TRAKT_BASE}/oauth/device/code"))
            .headers(self.headers(None))
            .json(&serde_json::json!({ "client_id": self.client_id }))
            .send()
            .await?
            .error_for_status()
            .context("Trakt turned the client id down")?
            .json()
            .await?;
        Ok(DeviceCode {
            device_code: v["device_code"].as_str().unwrap_or_default().to_string(),
            user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
            verification_url: v["verification_url"]
                .as_str()
                .unwrap_or("https://trakt.tv/activate")
                .to_string(),
            expires_in: v["expires_in"].as_i64().unwrap_or(600),
            interval: v["interval"].as_i64().unwrap_or(5).max(1),
        })
    }

    /// Step three, once. The caller sleeps `interval` between calls.
    pub async fn poll_token(&self, device_code: &str) -> Result<DevicePoll> {
        let resp = self
            .http
            .post(format!("{TRAKT_BASE}/oauth/device/token"))
            .headers(self.headers(None))
            .json(&serde_json::json!({
                "code": device_code,
                "client_id": self.client_id,
                "client_secret": self.client_secret,
            }))
            .send()
            .await?;
        Ok(match resp.status().as_u16() {
            200 => {
                let v: serde_json::Value = resp.json().await?;
                match v["access_token"].as_str() {
                    Some(t) if !t.is_empty() => DevicePoll::Token(t.to_string()),
                    _ => DevicePoll::Pending,
                }
            }
            400 => DevicePoll::Pending,
            404 | 410 => DevicePoll::Expired,
            409 | 418 => DevicePoll::Denied,
            429 => DevicePoll::SlowDown,
            other => anyhow::bail!("Trakt answered {other} while linking"),
        })
    }

    /// Whether the saved token is still good — and, as a side effect, the
    /// account's name, which is what Settings shows instead of "linked".
    pub async fn username(&self, token: &str) -> Result<Option<String>> {
        let resp = self
            .http
            .get(format!("{TRAKT_BASE}/users/settings"))
            .headers(self.headers(Some(token)))
            .send()
            .await?;
        if matches!(resp.status().as_u16(), 401 | 403) {
            return Ok(None);
        }
        let v: serde_json::Value = resp.error_for_status()?.json().await?;
        Ok(v["user"]["username"].as_str().map(str::to_string))
    }

    /// Add one finished film or episode to the account's history.
    ///
    /// Trakt matches on ids first and titles second, so an item with a TMDB
    /// id lands on the right film even when the filename was creative.
    pub async fn add_to_history(&self, token: &str, item: &HistoryItem) -> Result<()> {
        let body = item.body();
        let resp = self
            .http
            .post(format!("{TRAKT_BASE}/sync/history"))
            .headers(self.headers(Some(token)))
            .json(&body)
            .send()
            .await?;
        if matches!(resp.status().as_u16(), 401 | 403) {
            anyhow::bail!("Trakt no longer accepts the saved token — link the account again");
        }
        resp.error_for_status()?;
        Ok(())
    }
}

/// One thing that was watched.
#[derive(Debug, Clone, PartialEq)]
pub struct HistoryItem {
    pub kind: Kind,
    pub title: String,
    pub year: Option<i64>,
    pub tmdb_id: Option<i64>,
    /// Episodes only.
    pub season: Option<i64>,
    pub episode: Option<i64>,
    /// When it finished, ISO-8601 UTC. Trakt takes "now" when it is empty.
    pub watched_at: String,
}

impl HistoryItem {
    /// The `/sync/history` envelope. A film goes under `movies`; an episode
    /// goes under `shows` as one season holding one episode, which is the
    /// shape Trakt documents for "I watched this one episode".
    pub fn body(&self) -> serde_json::Value {
        let mut ids = serde_json::Map::new();
        if let Some(id) = self.tmdb_id {
            ids.insert("tmdb".into(), serde_json::json!(id));
        }
        let watched_at = if self.watched_at.trim().is_empty() {
            serde_json::Value::Null
        } else {
            serde_json::json!(self.watched_at)
        };
        match self.kind {
            Kind::Movie => serde_json::json!({
                "movies": [{
                    "title": self.title,
                    "year": self.year,
                    "ids": ids,
                    "watched_at": watched_at,
                }]
            }),
            Kind::Episode => serde_json::json!({
                "shows": [{
                    "title": self.title,
                    "year": self.year,
                    "ids": ids,
                    "seasons": [{
                        "number": self.season.unwrap_or(1),
                        "episodes": [{
                            "number": self.episode.unwrap_or(1),
                            "watched_at": watched_at,
                        }],
                    }],
                }]
            }),
        }
    }
}

/// Unix seconds as Trakt wants them: `2026-09-14T12:00:00.000Z`.
pub fn iso8601_utc(secs: i64) -> String {
    // Days since the epoch → civil date, the same arithmetic `logging` uses.
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.000Z",
        tod / 3600,
        (tod % 3600) / 60,
        tod % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn epoch_and_a_known_date_render() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00.000Z");
        // 2024-02-29 12:24:56 UTC
        assert_eq!(iso8601_utc(1_709_209_496), "2024-02-29T12:24:56.000Z");
    }

    #[test]
    fn a_film_goes_under_movies_with_its_tmdb_id() {
        let b = HistoryItem {
            kind: Kind::Movie,
            title: "Arrival".into(),
            year: Some(2016),
            tmdb_id: Some(329865),
            season: None,
            episode: None,
            watched_at: "2026-09-14T12:00:00.000Z".into(),
        }
        .body();
        assert_eq!(b["movies"][0]["title"], "Arrival");
        assert_eq!(b["movies"][0]["ids"]["tmdb"], 329865);
        assert_eq!(b["movies"][0]["watched_at"], "2026-09-14T12:00:00.000Z");
        assert!(b.get("shows").is_none());
    }

    #[test]
    fn an_episode_goes_under_shows_as_one_season_holding_one_episode() {
        let b = HistoryItem {
            kind: Kind::Episode,
            title: "The Expanse".into(),
            year: None,
            tmdb_id: None,
            season: Some(3),
            episode: Some(7),
            watched_at: String::new(),
        }
        .body();
        assert_eq!(b["shows"][0]["seasons"][0]["number"], 3);
        assert_eq!(b["shows"][0]["seasons"][0]["episodes"][0]["number"], 7);
        // No id to send, so the object is empty rather than holding a null.
        assert_eq!(b["shows"][0]["ids"], serde_json::json!({}));
        // Blank time means "now" to Trakt, sent as null rather than "".
        assert!(b["shows"][0]["seasons"][0]["episodes"][0]["watched_at"].is_null());
    }
}
