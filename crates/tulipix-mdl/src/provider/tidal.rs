//! Tidal provider — port of mdl `providers/Tidal.ts`.
//!
//! Albums and single tracks come from Tidal's public GraphQL endpoint, which
//! answers catalogue queries without a token. Playlists need both: the endpoint
//! returns the track list but not the playlist's own name, so the page is read
//! for its Open Graph tags alongside.
//!
//! Only catalogue metadata is read. Nothing here touches a stream — Tidal audio
//! is subscriber-only, and this crate's audio always comes from YouTube.

use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::json;
use url::Url;

pub struct Tidal;

const GRAPHQL: &str = "https://gqlapi.tidal.com/";

/// The track fields every one of the three queries selects.
const TRACK_FIELDS: &str =
    "album { title } artists { name } duration id image { original large medium small xsmall } title";

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize)]
struct TdImage {
    original: Option<String>,
    large: Option<String>,
    medium: Option<String>,
    small: Option<String>,
    xsmall: Option<String>,
}

#[derive(Deserialize)]
struct TdAlbumRef {
    title: Option<String>,
}

#[derive(Deserialize)]
struct TdTrack {
    id: Option<serde_json::Value>,
    title: Option<String>,
    /// Seconds.
    duration: Option<f64>,
    artists: Option<Vec<Named>>,
    album: Option<TdAlbumRef>,
    image: Option<TdImage>,
}

#[derive(Deserialize)]
struct TdAlbum {
    id: Option<serde_json::Value>,
    title: Option<String>,
    artists: Option<Vec<Named>>,
    image: Option<TdImage>,
    tracks: Option<Vec<TdTrack>>,
}

#[derive(Deserialize)]
struct TdPlaylistTracks {
    items: Option<Vec<TdTrack>>,
}

#[derive(Deserialize)]
struct TdData {
    album: Option<TdAlbum>,
    track: Option<TdTrack>,
    #[serde(rename = "playlistTracks")]
    playlist_tracks: Option<TdPlaylistTracks>,
}

#[derive(Deserialize)]
struct TdError {
    message: Option<String>,
}

#[derive(Deserialize)]
struct TdResponse {
    data: Option<TdData>,
    errors: Option<Vec<TdError>>,
}

/// Metadata scraped off a playlist page.
#[derive(Debug, Default, PartialEq)]
pub struct PageMeta {
    pub title: Option<String>,
    pub owner: Option<String>,
    pub artwork_url: Option<String>,
    pub source_url: Option<String>,
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

/// Tidal's image URLs come back protocol-relative about half the time.
fn asset_url(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        return None;
    }
    Some(match value.strip_prefix("//") {
        Some(rest) => format!("https://{rest}"),
        None => value.to_string(),
    })
}

fn best_image(image: Option<&TdImage>) -> Option<String> {
    let image = image?;
    [
        image.original.as_deref(),
        image.large.as_deref(),
        image.medium.as_deref(),
        image.small.as_deref(),
        image.xsmall.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(|v| asset_url(Some(v)))
}

fn meta_content(html: &str, attribute: &str, key: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?i)<meta[^>]+{attribute}=["']{}["'][^>]+content=["']([^"']+)["'][^>]*>"#,
        regex::escape(key)
    ))
    .ok()?;
    trimmed(Some(&re.captures(html)?.get(1)?.as_str().to_string()))
}

impl Tidal {
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

    fn collection_id(url: &str) -> Result<String> {
        let path = Url::parse(url)?.path().to_string();
        Regex::new(r"(?i)/(?:album|playlist|track)/([0-9a-z-]+)")
            .unwrap()
            .captures(&path)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| anyhow!("Could not determine the Tidal collection id from the URL."))
    }

    pub fn parse_playlist_html(html: &str, source_url: &str) -> Result<PageMeta> {
        let title = meta_content(html, "property", "og:title")
            .ok_or_else(|| anyhow!("Could not find Tidal playlist metadata in the page."))?;
        let description = meta_content(html, "property", "og:description")
            .or_else(|| meta_content(html, "name", "description"));
        let owner = description.as_deref().and_then(|d| {
            Regex::new(r"(?i)^Playlist by (.+)$")
                .unwrap()
                .captures(d)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
        });
        Ok(PageMeta {
            title: Some(title),
            owner,
            artwork_url: asset_url(meta_content(html, "property", "og:image").as_deref()),
            source_url: meta_content(html, "property", "og:url")
                .or_else(|| Some(source_url.to_string())),
        })
    }

    fn normalize(track: &TdTrack, collection_art: Option<&str>) -> Option<Track> {
        let title = trimmed(track.title.as_ref())?;
        let artists: Vec<String> = track
            .artists
            .as_ref()
            .map(|list| list.iter().filter_map(|a| trimmed(a.name.as_ref())).collect())
            .unwrap_or_default();
        if artists.is_empty() {
            return None;
        }
        let id = id_string(track.id.as_ref());
        Some(Track {
            id: id
                .clone()
                .unwrap_or_else(|| format!("{}-{title}", artists.join(","))),
            title,
            artists,
            album: track.album.as_ref().and_then(|a| trimmed(a.title.as_ref())),
            artwork_url: get_first_non_empty(&[
                best_image(track.image.as_ref()).as_deref(),
                collection_art,
            ]),
            duration_ms: track
                .duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(|d| (d * 1000.0).round() as u64),
            source_url: id.map(|id| format!("https://tidal.com/browse/track/{id}")),
        })
    }

    async fn graphql(
        client: &reqwest::Client,
        query: &str,
        variables: serde_json::Value,
    ) -> Result<TdData> {
        let payload: TdResponse = client
            .post(GRAPHQL)
            .header("content-type", "application/json")
            .header("user-agent", "Mozilla/5.0")
            .json(&json!({ "query": query, "variables": variables }))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        // GraphQL answers 200 with an `errors` array; without this check the
        // failure shows up much later as "no tracks were found".
        if let Some(message) = payload
            .errors
            .as_ref()
            .and_then(|e| e.iter().find_map(|e| trimmed(e.message.as_ref())))
        {
            bail!("Tidal request failed: {message}");
        }
        payload
            .data
            .ok_or_else(|| anyhow!("Tidal returned no data for the request."))
    }
}

#[async_trait::async_trait]
impl Provider for Tidal {
    fn id(&self) -> ProviderId {
        ProviderId::Tidal
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase).unwrap_or_default();
        if !["tidal.com", "listen.tidal.com"].contains(&host.as_str()) {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        [
            r"(?i)^(?:/browse)?/album/\d+$",
            r"(?i)^(?:/browse)?/playlist/[0-9a-f-]+$",
            r"(?i)^(?:/browse)?/track/\d+$",
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

        if kind == "track" {
            let track_id: i64 = id
                .parse()
                .map_err(|_| anyhow!("Could not determine the Tidal track id from the URL."))?;
            let data = Tidal::graphql(
                client,
                &format!("query ($trackId: BigInt!) {{ track(id: $trackId) {{ {TRACK_FIELDS} }} }}"),
                json!({ "trackId": track_id }),
            )
            .await?;
            let raw = data
                .track
                .ok_or_else(|| anyhow!("Could not find Tidal track metadata in the response."))?;
            let track = Self::normalize(&raw, None)
                .ok_or_else(|| anyhow!("Could not find Tidal track metadata in the response."))?;
            return Ok(Playlist {
                id: track.id.clone(),
                title: track.title.clone(),
                owner: track.artists.first().cloned(),
                artwork_url: track.artwork_url.clone(),
                provider: ProviderId::Tidal,
                source_url: track.source_url.clone().unwrap_or_else(|| url.to_string()),
                tracks: vec![track],
            });
        }

        if kind == "album" {
            let album_id: i64 = id
                .parse()
                .map_err(|_| anyhow!("Could not determine the Tidal album id from the URL."))?;
            let data = Tidal::graphql(
                client,
                &format!(
                    "query ($albumId: BigInt!) {{ album(id: $albumId) {{ id title \
                     artists {{ name }} image {{ original large medium small xsmall }} \
                     tracks {{ {TRACK_FIELDS} }} }} }}"
                ),
                json!({ "albumId": album_id }),
            )
            .await?;
            let album = data
                .album
                .ok_or_else(|| anyhow!("Could not find Tidal album metadata in the response."))?;
            let title = trimmed(album.title.as_ref())
                .ok_or_else(|| anyhow!("Could not find Tidal album metadata in the response."))?;
            let art = best_image(album.image.as_ref());
            let tracks: Vec<Track> = album
                .tracks
                .unwrap_or_default()
                .iter()
                .filter_map(|t| Self::normalize(t, art.as_deref()))
                .collect();
            if tracks.is_empty() {
                bail!("No tracks were found in the Tidal album.");
            }
            return Ok(Playlist {
                id: id_string(album.id.as_ref()).unwrap_or(id),
                title,
                owner: album
                    .artists
                    .as_ref()
                    .and_then(|a| a.first())
                    .and_then(|a| trimmed(a.name.as_ref())),
                artwork_url: art,
                provider: ProviderId::Tidal,
                source_url: url.to_string(),
                tracks,
            });
        }

        // Playlist: the endpoint has the tracks, the page has the name.
        let html = client
            .get(url)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let meta = Self::parse_playlist_html(&html, url)?;
        let data = Tidal::graphql(
            client,
            &format!(
                "query ($playlistId: String!) {{ playlistTracks(uuid: $playlistId) \
                 {{ items {{ {TRACK_FIELDS} }} }} }}"
            ),
            json!({ "playlistId": id }),
        )
        .await?;
        let tracks: Vec<Track> = data
            .playlist_tracks
            .and_then(|p| p.items)
            .unwrap_or_default()
            .iter()
            .filter_map(|t| Self::normalize(t, meta.artwork_url.as_deref()))
            .collect();
        if tracks.is_empty() {
            bail!("No tracks were found in the Tidal playlist.");
        }
        Ok(Playlist {
            id,
            title: meta.title.clone().unwrap_or_else(|| "Tidal Playlist".into()),
            owner: meta.owner.clone(),
            artwork_url: meta.artwork_url.clone(),
            provider: ProviderId::Tidal,
            source_url: meta.source_url.clone().unwrap_or_else(|| url.to_string()),
            tracks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_graphql_track_becomes_a_track() {
        let raw: TdTrack = serde_json::from_str(
            r#"{"id":12345,"title":"Bohemian Rhapsody","duration":355,
                "artists":[{"name":"Queen"},{"name":"Someone"}],
                "album":{"title":"A Night at the Opera"},
                "image":{"original":null,"large":"//resources.tidal.com/large.jpg"}}"#,
        )
        .unwrap();
        let track = Tidal::normalize(&raw, None).unwrap();
        assert_eq!(track.artists, vec!["Queen".to_string(), "Someone".to_string()]);
        assert_eq!(track.duration_ms, Some(355_000));
        assert_eq!(track.album.as_deref(), Some("A Night at the Opera"));
        // Protocol-relative asset URLs are made absolute, and a null bigger
        // size falls through to the next one down.
        assert_eq!(
            track.artwork_url.as_deref(),
            Some("https://resources.tidal.com/large.jpg")
        );
        assert_eq!(
            track.source_url.as_deref(),
            Some("https://tidal.com/browse/track/12345")
        );
    }

    #[test]
    fn a_playlist_page_gives_up_its_name_and_owner() {
        let html = r#"<html><head>
<meta property="og:title" content="Late Night Drive">
<meta property="og:description" content="Playlist by Tidal Editorial">
<meta property="og:image" content="//resources.tidal.com/pl.jpg">
<meta property="og:url" content="https://tidal.com/playlist/abc-123">
</head></html>"#;
        let meta = Tidal::parse_playlist_html(html, "https://listen.tidal.com/playlist/abc-123")
            .unwrap();
        assert_eq!(meta.title.as_deref(), Some("Late Night Drive"));
        assert_eq!(meta.owner.as_deref(), Some("Tidal Editorial"));
        assert_eq!(meta.artwork_url.as_deref(), Some("https://resources.tidal.com/pl.jpg"));
        assert_eq!(meta.source_url.as_deref(), Some("https://tidal.com/playlist/abc-123"));
    }

    #[test]
    fn both_browse_and_bare_urls_match() {
        let m = |u: &str| Tidal.matches(&Url::parse(u).unwrap());
        assert!(m("https://tidal.com/browse/album/77640617"));
        assert!(m("https://listen.tidal.com/album/77640617"));
        assert!(m("https://tidal.com/playlist/1c5d01ed-4f05-40c4-bd28-0f73099e8"));
        assert!(m("https://tidal.com/browse/track/77640618"));
        assert!(!m("https://tidal.com/browse/artist/12345"));
        assert!(!m("https://example.com/album/1"));
    }

    #[test]
    fn ids_come_out_of_either_url_shape() {
        assert_eq!(
            Tidal::collection_id("https://tidal.com/browse/album/77640617/").unwrap(),
            "77640617"
        );
        assert_eq!(
            Tidal::collection_id("https://listen.tidal.com/playlist/abc-123").unwrap(),
            "abc-123"
        );
    }
}
