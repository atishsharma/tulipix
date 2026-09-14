//! Spotify Web API — search and artist detail, for music the local library
//! has only a filename for.
//!
//! The client-credentials flow: the app's id and secret, Base64'd into one
//! `Basic` header, exchanged for a token that lasts an hour. No user account
//! and no browser redirect, which is why this flow and not the other: it can
//! read the catalogue but nothing about a person, which is all this needs.
//!
//! The token is cached until a minute before it expires, because a token
//! request per search would double every lookup.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use tulipix_core::util::unix_secs_i64 as now;

pub const SPOTIFY_TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
pub const SPOTIFY_BASE: &str = "https://api.spotify.com/v1";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpotifyTrack {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub year: Option<i64>,
    pub duration_ms: i64,
    pub art_url: Option<String>,
    /// 0-100, Spotify's own. What "recommended" is sorted by.
    pub popularity: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SpotifyArtist {
    pub id: String,
    pub name: String,
    pub genres: Vec<String>,
    pub followers: i64,
    pub popularity: i64,
    pub image_url: Option<String>,
}

pub struct SpotifyClient {
    client_id: String,
    client_secret: String,
    http: reqwest::Client,
    /// The bearer token and the second it stops being good.
    token: Mutex<Option<(String, i64)>>,
}

impl SpotifyClient {
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            http: tulipix_core::net::http().clone(),
            token: Mutex::new(None),
        }
    }

    /// A live token, from the cache when there is one with a minute left.
    pub async fn token(&self) -> Result<String> {
        if let Some((t, until)) = self.token.lock().ok().and_then(|g| g.clone()) {
            if now() < until {
                return Ok(t);
            }
        }
        let basic = base64_std(format!("{}:{}", self.client_id, self.client_secret).as_bytes());
        let v: serde_json::Value = self
            .http
            .post(SPOTIFY_TOKEN_URL)
            .header(reqwest::header::AUTHORIZATION, format!("Basic {basic}"))
            .header(
                reqwest::header::CONTENT_TYPE,
                "application/x-www-form-urlencoded",
            )
            .body("grant_type=client_credentials")
            .send()
            .await?
            .error_for_status()
            .context("Spotify turned the client id and secret down")?
            .json()
            .await?;
        let token = v["access_token"]
            .as_str()
            .filter(|t| !t.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Spotify sent no access token"))?
            .to_string();
        // A minute short of what they said, so a token never expires between
        // being read here and arriving there.
        let until = now() + v["expires_in"].as_i64().unwrap_or(3600) - 60;
        if let Ok(mut g) = self.token.lock() {
            *g = Some((token.clone(), until));
        }
        Ok(token)
    }

    async fn get(&self, path: &str, query: &[(&str, String)]) -> Result<serde_json::Value> {
        let token = self.token().await?;
        Ok(self
            .http
            .get(format!("{SPOTIFY_BASE}{path}"))
            .bearer_auth(token)
            .query(query)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Tracks matching a free-text query, most popular first.
    pub async fn search_tracks(&self, query: &str, limit: u32) -> Result<Vec<SpotifyTrack>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let v = self
            .get(
                "/search",
                &[
                    ("q", query.trim().to_string()),
                    ("type", "track".into()),
                    ("limit", limit.clamp(1, 50).to_string()),
                ],
            )
            .await?;
        Ok(parse_tracks(&v))
    }

    /// The artist page for a name, for the genres and the picture.
    pub async fn artist(&self, name: &str) -> Result<Option<SpotifyArtist>> {
        if name.trim().is_empty() {
            return Ok(None);
        }
        let v = self
            .get(
                "/search",
                &[
                    ("q", name.trim().to_string()),
                    ("type", "artist".into()),
                    ("limit", "5".into()),
                ],
            )
            .await?;
        Ok(best_artist(&v, name))
    }

    /// One cheap call, for Settings' Test button. A wrong id or secret fails
    /// at the token, which is the answer worth having.
    pub async fn check(&self) -> Result<bool> {
        self.token().await.map(|t| !t.is_empty())
    }
}

fn parse_tracks(v: &serde_json::Value) -> Vec<SpotifyTrack> {
    v["tracks"]["items"]
        .as_array()
        .map(|a| a.iter().map(parse_track).collect())
        .unwrap_or_default()
}

fn parse_track(t: &serde_json::Value) -> SpotifyTrack {
    SpotifyTrack {
        id: t["id"].as_str().unwrap_or_default().to_string(),
        title: t["name"].as_str().unwrap_or_default().to_string(),
        artist: t["artists"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|x| x["name"].as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default(),
        album: t["album"]["name"].as_str().unwrap_or_default().to_string(),
        year: t["album"]["release_date"]
            .as_str()
            .and_then(|d| d.get(..4))
            .and_then(|y| y.parse().ok()),
        duration_ms: t["duration_ms"].as_i64().unwrap_or(0),
        art_url: t["album"]["images"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|i| i["url"].as_str())
            .map(str::to_string),
        popularity: t["popularity"].as_i64().unwrap_or(0),
    }
}

/// Exact name first, as with Discogs: a search for "Air" otherwise lands on
/// whoever released something last week.
fn best_artist(v: &serde_json::Value, want: &str) -> Option<SpotifyArtist> {
    let items = v["artists"]["items"].as_array()?;
    let pick = items
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .is_some_and(|n| n.trim().eq_ignore_ascii_case(want.trim()))
        })
        .or_else(|| items.first())?;
    Some(SpotifyArtist {
        id: pick["id"].as_str().unwrap_or_default().to_string(),
        name: pick["name"].as_str().unwrap_or_default().to_string(),
        genres: pick["genres"]
            .as_array()
            .map(|g| g.iter().filter_map(|x| x.as_str()).map(str::to_string).collect())
            .unwrap_or_default(),
        followers: pick["followers"]["total"].as_i64().unwrap_or(0),
        popularity: pick["popularity"].as_i64().unwrap_or(0),
        image_url: pick["images"]
            .as_array()
            .and_then(|a| a.first())
            .and_then(|i| i["url"].as_str())
            .map(str::to_string),
    })
}

/// Standard Base64, for the one `Basic` header this needs. Twelve lines
/// against a dependency the music crate does not otherwise have.
pub fn base64_std(bytes: &[u8]) -> String {
    const A: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(A[(n >> 18) as usize & 63] as char);
        out.push(A[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { A[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { A[n as usize & 63] as char } else { '=' });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_rfc_vectors() {
        assert_eq!(base64_std(b""), "");
        assert_eq!(base64_std(b"f"), "Zg==");
        assert_eq!(base64_std(b"fo"), "Zm8=");
        assert_eq!(base64_std(b"foo"), "Zm9v");
        assert_eq!(base64_std(b"foob"), "Zm9vYg==");
        assert_eq!(base64_std(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_std(b"foobar"), "Zm9vYmFy");
        // What the header actually carries.
        assert_eq!(base64_std(b"id:secret"), "aWQ6c2VjcmV0");
    }

    #[test]
    fn a_search_payload_reads_whole() {
        let v = serde_json::json!({"tracks":{"items":[{
            "id":"t1","name":"Autobahn","duration_ms":1383000,"popularity":57,
            "artists":[{"name":"Kraftwerk"},{"name":"Guest"}],
            "album":{"name":"Autobahn","release_date":"1974-11-01","images":[{"url":"https://i/1.jpg"}]}
        }]}});
        let t = parse_tracks(&v);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].artist, "Kraftwerk, Guest");
        assert_eq!(t[0].year, Some(1974));
        assert_eq!(t[0].duration_ms, 1_383_000);
        assert_eq!(t[0].art_url.as_deref(), Some("https://i/1.jpg"));
    }

    #[test]
    fn the_exact_artist_wins_over_the_first_hit() {
        let v = serde_json::json!({"artists":{"items":[
            {"id":"a","name":"Air Supply","genres":["pop"],"followers":{"total":9},"popularity":40},
            {"id":"b","name":"Air","genres":["french house"],"followers":{"total":7},"popularity":30}
        ]}});
        assert_eq!(best_artist(&v, "air").unwrap().id, "b");
        assert_eq!(best_artist(&v, "nobody").unwrap().id, "a");
        assert!(best_artist(&serde_json::json!({"artists":{"items":[]}}), "x").is_none());
    }
}
