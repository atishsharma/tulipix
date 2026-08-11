//! Qobuz provider — port of mdl `providers/Qobuz.ts`.
//!
//! Qobuz's public catalogue API needs an `app_id`, which is not secret: the web
//! player ships it inside its own bundle. That bundle is fetched once per run
//! and the id cached, because it changes about as often as the player is
//! rebuilt and a second fetch per playlist buys nothing.
//!
//! Only catalogue metadata is read here. Streams are subscriber-only and are
//! not touched — audio comes from YouTube like every other provider in this
//! crate.

use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;
use std::sync::{Mutex, OnceLock};
use url::Url;

pub struct Qobuz;

/// The web player's own bundle, where the app id lives.
const MAIN_JS: &str = "https://open.qobuz.com/resources/2.2.2/js/main.js";

fn app_id_cache() -> &'static Mutex<Option<String>> {
    static CACHE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize)]
struct QbImage {
    large: Option<String>,
    small: Option<String>,
    thumbnail: Option<String>,
}

#[derive(Deserialize)]
struct QbAlbumRef {
    title: Option<String>,
    image: Option<QbImage>,
}

#[derive(Deserialize)]
struct QbTrack {
    id: Option<serde_json::Value>,
    title: Option<String>,
    /// Seconds.
    duration: Option<f64>,
    performer: Option<Named>,
    album: Option<QbAlbumRef>,
}

#[derive(Deserialize)]
struct QbTrackList {
    items: Option<Vec<QbTrack>>,
}

#[derive(Deserialize)]
struct QbCollection {
    id: Option<serde_json::Value>,
    /// Playlists use `name`; albums use `title`. Both are read.
    name: Option<String>,
    title: Option<String>,
    url: Option<String>,
    image: Option<QbImage>,
    images300: Option<Vec<String>>,
    owner: Option<Named>,
    artist: Option<Named>,
    tracks: Option<QbTrackList>,
}

fn trimmed(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn id_string(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

/// Biggest image the payload offers, in the order Qobuz sizes them.
fn best_image(image: Option<&QbImage>) -> Option<&str> {
    let image = image?;
    [
        image.large.as_deref(),
        image.small.as_deref(),
        image.thumbnail.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find(|s| !s.trim().is_empty())
}

impl Qobuz {
    fn collection_kind(url: &str) -> &'static str {
        let path = Url::parse(url).map(|u| u.path().to_string()).unwrap_or_default();
        if path.contains("/album/") {
            "album"
        } else if path.contains("/track/") {
            "track"
        } else {
            "playlist"
        }
    }

    /// The id is the LAST path segment on every shape Qobuz publishes — bare
    /// `/album/<id>` from the web player and `/fr-fr/album/<slug>/<id>` from the
    /// store, where the slug in the middle is decoration.
    fn collection_id(url: &str) -> Result<String> {
        let path = Url::parse(url)?.path().trim_end_matches('/').to_string();
        path.rsplit('/')
            .next()
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(str::to_string)
            .ok_or_else(|| anyhow!("Could not determine the Qobuz collection id from the URL."))
    }

    fn normalize(track: &QbTrack, collection_art: Option<&str>) -> Option<Track> {
        let title = trimmed(track.title.as_ref())?;
        let artist = track.performer.as_ref().and_then(|p| trimmed(p.name.as_ref()))?;
        let id = id_string(track.id.as_ref());
        Some(Track {
            id: id.clone().unwrap_or_else(|| format!("{artist}-{title}")),
            title,
            artists: vec![artist],
            album: track.album.as_ref().and_then(|a| trimmed(a.title.as_ref())),
            artwork_url: get_first_non_empty(&[
                best_image(track.album.as_ref().and_then(|a| a.image.as_ref())),
                collection_art,
            ]),
            duration_ms: track
                .duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(|d| (d * 1000.0).round() as u64),
            // The catalogue payload has no permalink; the player's track URL is
            // the canonical one and is built from the id.
            source_url: id.map(|id| format!("https://open.qobuz.com/track/{id}")),
        })
    }

    fn build(collection: &QbCollection, kind: &str, source_url: &str, fallback_id: &str) -> Result<Playlist> {
        let art = get_first_non_empty(&[
            collection
                .images300
                .as_ref()
                .and_then(|v| v.first())
                .map(String::as_str),
            best_image(collection.image.as_ref()),
        ]);
        let tracks: Vec<Track> = collection
            .tracks
            .as_ref()
            .and_then(|t| t.items.as_ref())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|t| Self::normalize(t, art.as_deref()))
                    .collect()
            })
            .unwrap_or_default();
        if tracks.is_empty() {
            bail!("No tracks were found in the Qobuz {kind}.");
        }
        Ok(Playlist {
            id: id_string(collection.id.as_ref()).unwrap_or_else(|| fallback_id.to_string()),
            title: get_first_non_empty(&[
                collection.name.as_deref(),
                collection.title.as_deref(),
            ])
            .unwrap_or_else(|| {
                format!("Qobuz {}", if kind == "album" { "Album" } else { "Playlist" })
            }),
            owner: collection
                .owner
                .as_ref()
                .and_then(|o| trimmed(o.name.as_ref()))
                .or_else(|| collection.artist.as_ref().and_then(|a| trimmed(a.name.as_ref()))),
            artwork_url: art,
            provider: ProviderId::Qobuz,
            source_url: trimmed(collection.url.as_ref()).unwrap_or_else(|| source_url.to_string()),
            tracks,
        })
    }

    /// The app id out of the player bundle.
    fn parse_app_id(source: &str) -> Option<String> {
        for pattern in [r#"qobuzapi=\{app_id:"(\d+)""#, r#"APP_ID:"(\d+)""#] {
            if let Some(id) = Regex::new(pattern)
                .ok()
                .and_then(|re| re.captures(source))
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().to_string())
            {
                return Some(id);
            }
        }
        None
    }

    async fn app_id(client: &reqwest::Client) -> Result<String> {
        if let Some(id) = app_id_cache().lock().ok().and_then(|g| g.clone()) {
            return Ok(id);
        }
        let source = client
            .get(MAIN_JS)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let id = Self::parse_app_id(&source)
            .ok_or_else(|| anyhow!("Could not determine the Qobuz app id."))?;
        if let Ok(mut g) = app_id_cache().lock() {
            *g = Some(id.clone());
        }
        Ok(id)
    }

    async fn json<T: serde::de::DeserializeOwned>(
        client: &reqwest::Client,
        url: Url,
    ) -> Result<T> {
        Ok(client
            .get(url)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    fn endpoint(path: &str, key: &str, id: &str, app_id: &str, extra: bool) -> Url {
        let mut url = Url::parse(&format!("https://www.qobuz.com/api.json/0.2/{path}")).unwrap();
        {
            let mut q = url.query_pairs_mut();
            q.append_pair(key, id);
            q.append_pair("app_id", app_id);
            if extra {
                q.append_pair("extra", "tracks");
                q.append_pair("limit", "500");
            }
        }
        url
    }
}

#[async_trait::async_trait]
impl Provider for Qobuz {
    fn id(&self) -> ProviderId {
        ProviderId::Qobuz
    }
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &["open.qobuz.com"]
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase).unwrap_or_default();
        if !["open.qobuz.com", "play.qobuz.com", "www.qobuz.com", "qobuz.com"]
            .contains(&host.as_str())
        {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        [
            r"(?i)^/album/[A-Za-z0-9]+$",
            r"(?i)^/playlist/[A-Za-z0-9]+$",
            r"(?i)^/track/[A-Za-z0-9]+$",
            r"(?i)^/[a-z]{2}-[a-z]{2}/album/[^/]+/[A-Za-z0-9]+$",
            r"(?i)^/[a-z]{2}-[a-z]{2}/playlists/[^/]+/[A-Za-z0-9]+$",
        ]
        .iter()
        .any(|p| Regex::new(p).unwrap().is_match(path))
    }
    fn normalize(&self, url: &Url) -> String {
        strip_query_and_hash(url)
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let kind = Self::collection_kind(url);
        let id = Self::collection_id(url)?;
        let app_id = Self::app_id(client).await?;

        if kind == "track" {
            let raw: QbTrack =
                Self::json(client, Self::endpoint("track/get", "track_id", &id, &app_id, false))
                    .await?;
            let track = Self::normalize(&raw, None)
                .ok_or_else(|| anyhow!("No track was found in the Qobuz response."))?;
            return Ok(Playlist {
                id: track.id.clone(),
                title: track.title.clone(),
                owner: track.artists.first().cloned(),
                artwork_url: track.artwork_url.clone(),
                provider: ProviderId::Qobuz,
                source_url: track.source_url.clone().unwrap_or_else(|| url.to_string()),
                tracks: vec![track],
            });
        }

        let endpoint = if kind == "album" {
            Self::endpoint("album/get", "album_id", &id, &app_id, false)
        } else {
            Self::endpoint("playlist/get", "playlist_id", &id, &app_id, true)
        };
        let collection: QbCollection = Self::json(client, endpoint).await?;
        Self::build(&collection, kind, url, &id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_payload_becomes_a_playlist() {
        let collection: QbCollection = serde_json::from_str(
            r#"{"id":"0060254705878","title":"Random Access Memories",
                "image":{"large":"https://static/large.jpg","small":"https://static/small.jpg"},
                "artist":{"name":"Daft Punk"},
                "tracks":{"items":[
                  {"id":25489153,"title":"Give Life Back to Music","duration":275,
                   "performer":{"name":"Daft Punk"},
                   "album":{"title":"Random Access Memories"}},
                  {"id":25489154,"title":"Instant Crush","duration":337,"performer":{"name":"Daft Punk"}}
                ]}}"#,
        )
        .unwrap();
        let pl = Qobuz::build(
            &collection,
            "album",
            "https://open.qobuz.com/album/0060254705878",
            "0060254705878",
        )
        .unwrap();
        assert_eq!(pl.id, "0060254705878");
        assert_eq!(pl.title, "Random Access Memories");
        assert_eq!(pl.owner.as_deref(), Some("Daft Punk"));
        assert_eq!(pl.tracks.len(), 2);
        assert_eq!(pl.tracks[0].duration_ms, Some(275_000));
        // No image of its own → the album's large one.
        assert_eq!(pl.tracks[1].artwork_url.as_deref(), Some("https://static/large.jpg"));
        assert_eq!(
            pl.tracks[0].source_url.as_deref(),
            Some("https://open.qobuz.com/track/25489153")
        );
    }

    #[test]
    fn the_app_id_is_read_out_of_the_player_bundle() {
        assert_eq!(
            Qobuz::parse_app_id(r#"…,qobuzapi={app_id:"712109809",app_secret:"x"},…"#).as_deref(),
            Some("712109809")
        );
        assert_eq!(
            Qobuz::parse_app_id(r#"var c={APP_ID:"950096963",OTHER:1}"#).as_deref(),
            Some("950096963")
        );
        assert!(Qobuz::parse_app_id("nothing here").is_none());
    }

    #[test]
    fn store_urls_carry_a_slug_before_the_id() {
        assert_eq!(
            Qobuz::collection_id("https://www.qobuz.com/fr-fr/album/random-access-memories/0060254705878")
                .unwrap(),
            "0060254705878"
        );
        assert_eq!(
            Qobuz::collection_id("https://open.qobuz.com/playlist/5966677/").unwrap(),
            "5966677"
        );
    }

    #[test]
    fn matches_both_player_and_store_urls() {
        let m = |u: &str| Qobuz.matches(&Url::parse(u).unwrap());
        assert!(m("https://open.qobuz.com/album/0060254705878"));
        assert!(m("https://open.qobuz.com/playlist/5966677"));
        assert!(m("https://www.qobuz.com/fr-fr/album/slug/0060254705878"));
        assert!(m("https://www.qobuz.com/us-en/playlists/slug/5966677"));
        assert!(!m("https://open.qobuz.com/artist/12345"));
        assert!(!m("https://example.com/album/1"));
    }
}
