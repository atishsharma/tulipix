//! Deezer provider — port of mdl `providers/Deezer.ts`.
//!
//! Deezer publishes a plain, keyless REST API at `api.deezer.com`, so this one
//! is a JSON client rather than a scraper. An album carries its tracks inline;
//! a playlist pages them 100 at a time behind a `next` cursor, which is
//! followed to the end because a 500-track playlist that silently downloads its
//! first hundred is worse than one that fails.

use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;
use url::Url;

pub struct Deezer;

/// Guard against a malformed `next` chain walking forever.
const MAX_PAGES: usize = 60;

#[derive(Deserialize, Clone)]
struct Named {
    name: Option<String>,
}

#[derive(Deserialize, Clone)]
struct DzAlbumRef {
    title: Option<String>,
    cover_xl: Option<String>,
}

#[derive(Deserialize, Clone)]
struct DzTrack {
    id: Option<serde_json::Value>,
    title: Option<String>,
    link: Option<String>,
    /// Seconds, unlike every other provider here.
    duration: Option<f64>,
    artist: Option<Named>,
    album: Option<DzAlbumRef>,
}

#[derive(Deserialize)]
struct DzTrackPage {
    data: Option<Vec<DzTrack>>,
    next: Option<String>,
}

#[derive(Deserialize)]
struct DzCollection {
    id: Option<serde_json::Value>,
    title: Option<String>,
    link: Option<String>,
    picture_xl: Option<String>,
    creator: Option<Named>,
    artist: Option<Named>,
    tracks: Option<DzTrackPage>,
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

impl Deezer {
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
        Regex::new(r"/(?:album|playlist|track)/(\d+)")
            .unwrap()
            .captures(&path)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| anyhow!("Could not determine the Deezer collection id from the URL."))
    }

    fn normalize(track: &DzTrack, collection_art: Option<&str>) -> Option<Track> {
        let title = trimmed(track.title.as_ref())?;
        let artist = track.artist.as_ref().and_then(|a| trimmed(a.name.as_ref()))?;
        Some(Track {
            id: id_string(track.id.as_ref()).unwrap_or_else(|| format!("{artist}-{title}")),
            title,
            artists: vec![artist],
            album: track.album.as_ref().and_then(|a| trimmed(a.title.as_ref())),
            artwork_url: get_first_non_empty(&[
                track.album.as_ref().and_then(|a| a.cover_xl.as_deref()),
                collection_art,
            ]),
            duration_ms: track
                .duration
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(|d| (d * 1000.0).round() as u64),
            source_url: trimmed(track.link.as_ref()),
        })
    }

    /// Build a playlist from an already-fetched collection + its tracks. Split
    /// out so the parsing is testable without a network.
    fn build(
        collection: &DzCollection,
        tracks: &[DzTrack],
        kind: &str,
        source_url: &str,
        fallback_id: &str,
    ) -> Result<Playlist> {
        let art = trimmed(collection.picture_xl.as_ref());
        let normalized: Vec<Track> = tracks
            .iter()
            .filter_map(|t| Self::normalize(t, art.as_deref()))
            .collect();
        if normalized.is_empty() {
            bail!("No tracks were found in the Deezer {kind}.");
        }
        Ok(Playlist {
            id: id_string(collection.id.as_ref()).unwrap_or_else(|| fallback_id.to_string()),
            title: trimmed(collection.title.as_ref()).unwrap_or_else(|| {
                format!("Deezer {}", if kind == "album" { "Album" } else { "Playlist" })
            }),
            owner: collection
                .creator
                .as_ref()
                .and_then(|c| trimmed(c.name.as_ref()))
                .or_else(|| collection.artist.as_ref().and_then(|a| trimmed(a.name.as_ref()))),
            artwork_url: art,
            provider: ProviderId::Deezer,
            source_url: trimmed(collection.link.as_ref())
                .unwrap_or_else(|| source_url.to_string()),
            tracks: normalized,
        })
    }

    async fn json<T: serde::de::DeserializeOwned>(
        client: &reqwest::Client,
        url: &str,
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

    /// Follow the `next` cursor to the end of a playlist.
    async fn all_tracks(client: &reqwest::Client, first: &str) -> Result<Vec<DzTrack>> {
        let mut out: Vec<DzTrack> = Vec::new();
        let mut cursor = Some(first.to_string());
        let mut pages = 0usize;
        while let Some(url) = cursor {
            let page: DzTrackPage = Self::json(client, &url).await?;
            out.extend(page.data.unwrap_or_default());
            pages += 1;
            if pages >= MAX_PAGES {
                break;
            }
            cursor = trimmed(page.next.as_ref());
        }
        Ok(out)
    }
}

#[async_trait::async_trait]
impl Provider for Deezer {
    fn id(&self) -> ProviderId {
        ProviderId::Deezer
    }
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &["deezer.page.link", "link.deezer.com"]
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase).unwrap_or_default();
        if !["deezer.com", "www.deezer.com"].contains(&host.as_str()) {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        // Deezer prefixes a locale on shared links: `/en/album/123`, `/en-gb/…`.
        Regex::new(r"(?i)^(?:/[a-z]{2}(?:-[a-z]{2})?)?/(?:album|playlist|track)/\d+$")
            .unwrap()
            .is_match(path)
    }
    fn normalize(&self, url: &Url) -> String {
        strip_query_and_hash(url)
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let kind = Self::collection_kind(url);
        let id = Self::collection_id(url)?;
        let endpoint = format!("https://api.deezer.com/{kind}/{id}");

        if kind == "track" {
            let raw: DzTrack = Self::json(client, &endpoint).await?;
            let track = Self::normalize(&raw, None)
                .ok_or_else(|| anyhow!("No track was found in the Deezer response."))?;
            return Ok(Playlist {
                id: track.id.clone(),
                title: track.title.clone(),
                owner: track.artists.first().cloned(),
                artwork_url: track.artwork_url.clone(),
                provider: ProviderId::Deezer,
                source_url: track.source_url.clone().unwrap_or_else(|| url.to_string()),
                tracks: vec![track],
            });
        }

        let collection: DzCollection = Self::json(client, &endpoint).await?;
        let tracks = if kind == "album" {
            collection
                .tracks
                .as_ref()
                .and_then(|t| t.data.as_ref())
                .cloned()
                .unwrap_or_default()
        } else {
            Self::all_tracks(client, &format!("{endpoint}/tracks?limit=100")).await?
        };
        Self::build(&collection, &tracks, kind, url, &id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_payload_becomes_a_playlist() {
        let collection: DzCollection = serde_json::from_str(
            r#"{"id":302127,"title":"Discovery","link":"https://www.deezer.com/album/302127",
                "picture_xl":"https://cdn/cover_xl.jpg","artist":{"name":"Daft Punk"},
                "tracks":{"data":[
                  {"id":3135556,"title":"Harder, Better, Faster, Stronger","duration":224,
                   "link":"https://www.deezer.com/track/3135556","artist":{"name":"Daft Punk"},
                   "album":{"title":"Discovery","cover_xl":"https://cdn/track_xl.jpg"}},
                  {"id":3135557,"title":"One More Time","duration":320,"artist":{"name":"Daft Punk"}}
                ]}}"#,
        )
        .unwrap();
        let tracks = collection.tracks.as_ref().unwrap().data.clone().unwrap();
        let pl = Deezer::build(
            &collection,
            &tracks,
            "album",
            "https://www.deezer.com/album/302127",
            "302127",
        )
        .unwrap();
        assert_eq!(pl.id, "302127");
        assert_eq!(pl.title, "Discovery");
        assert_eq!(pl.owner.as_deref(), Some("Daft Punk"));
        assert_eq!(pl.tracks.len(), 2);
        // Seconds → milliseconds, which is what the rest of the pipeline holds.
        assert_eq!(pl.tracks[0].duration_ms, Some(224_000));
        assert_eq!(pl.tracks[0].artwork_url.as_deref(), Some("https://cdn/track_xl.jpg"));
        // No album cover of its own → the collection's.
        assert_eq!(pl.tracks[1].artwork_url.as_deref(), Some("https://cdn/cover_xl.jpg"));
    }

    #[test]
    fn locale_prefixed_urls_still_match() {
        let m = |u: &str| Deezer.matches(&Url::parse(u).unwrap());
        assert!(m("https://www.deezer.com/album/302127"));
        assert!(m("https://www.deezer.com/en/playlist/908622995"));
        assert!(m("https://deezer.com/en-gb/track/3135556"));
        assert!(!m("https://www.deezer.com/artist/27"));
        assert!(!m("https://www.deezer.com/album/not-a-number"));
        assert!(!m("https://example.com/album/1"));
    }

    #[test]
    fn the_collection_id_comes_out_of_the_path() {
        assert_eq!(
            Deezer::collection_id("https://www.deezer.com/en/album/302127").unwrap(),
            "302127"
        );
        assert!(Deezer::collection_id("https://www.deezer.com/en/artist/27").is_err());
    }
}
