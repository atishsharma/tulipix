//! SoundCloud provider — port of mdl `providers/SoundCloud.ts`.
//!
//! SoundCloud renders its pages server-side and leaves the data it hydrated
//! them from in a `window.__sc_hydration = [...]` array. That array is the API:
//! one entry is `hydratable: "playlist"` (a set) or `"sound"` (a single track),
//! and its `data` is the same JSON the private API would return, without a
//! client id to obtain or an endpoint to guess.

use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use serde::Deserialize;
use url::Url;

pub struct Soundcloud;

#[derive(Deserialize)]
struct Hydration {
    hydratable: Option<String>,
    data: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct ScUser {
    username: Option<String>,
}

#[derive(Deserialize)]
struct ScPublisher {
    album_title: Option<String>,
    artist: Option<String>,
}

#[derive(Deserialize)]
struct ScTrack {
    id: Option<serde_json::Value>,
    title: Option<String>,
    artwork_url: Option<String>,
    permalink_url: Option<String>,
    // Milliseconds already — SoundCloud is one of the few that says so.
    duration: Option<f64>,
    publisher_metadata: Option<ScPublisher>,
    user: Option<ScUser>,
}

#[derive(Deserialize)]
struct ScPlaylist {
    id: Option<serde_json::Value>,
    title: Option<String>,
    artwork_url: Option<String>,
    permalink_url: Option<String>,
    tracks: Option<Vec<ScTrack>>,
    user: Option<ScUser>,
}

/// Ids come back as numbers here and as strings elsewhere in the same payload.
fn id_string(v: Option<&serde_json::Value>) -> Option<String> {
    match v? {
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        _ => None,
    }
}

fn trimmed(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

impl Soundcloud {
    /// The hydration array, parsed.
    fn hydration(html: &str) -> Result<Vec<Hydration>> {
        // Scanned rather than matched with a regex: the array holds arbitrary
        // user text (track descriptions), and a lazy `(\[.*?\])` stops at the
        // first `];</script>` *inside* one of those instead of at the end.
        let at = html
            .find("window.__sc_hydration")
            .ok_or_else(|| anyhow!("Could not find SoundCloud hydration data in the page."))?;
        let rest = &html[at..];
        let start = rest
            .find('[')
            .ok_or_else(|| anyhow!("Could not find SoundCloud hydration data in the page."))?;
        let end = rest
            .find("];</script>")
            .or_else(|| rest.find("];"))
            .ok_or_else(|| anyhow!("Could not read SoundCloud hydration data."))?;
        if end <= start {
            bail!("Could not read SoundCloud hydration data.");
        }
        Ok(serde_json::from_str(&rest[start..=end])?)
    }

    fn entry(html: &str, kind: &str) -> Result<serde_json::Value> {
        Self::hydration(html)?
            .into_iter()
            .find(|h| h.hydratable.as_deref() == Some(kind))
            .and_then(|h| h.data)
            .ok_or_else(|| anyhow!("Could not find SoundCloud {kind} metadata in the page."))
    }

    fn normalize(track: &ScTrack, collection_art: Option<&str>) -> Option<Track> {
        let id = id_string(track.id.as_ref())?;
        let title = trimmed(track.title.as_ref())?;
        // The uploader's name is the fallback artist: on SoundCloud most tracks
        // carry no publisher metadata at all, and the channel IS the artist.
        let artist = track
            .publisher_metadata
            .as_ref()
            .and_then(|p| trimmed(p.artist.as_ref()))
            .or_else(|| track.user.as_ref().and_then(|u| trimmed(u.username.as_ref())))?;
        Some(Track {
            id,
            title,
            artists: vec![artist],
            album: track
                .publisher_metadata
                .as_ref()
                .and_then(|p| trimmed(p.album_title.as_ref())),
            artwork_url: get_first_non_empty(&[track.artwork_url.as_deref(), collection_art]),
            duration_ms: track.duration.filter(|d| d.is_finite() && *d > 0.0).map(|d| d.round() as u64),
            source_url: trimmed(track.permalink_url.as_ref()),
        })
    }

    pub fn parse_playlist_html(html: &str, source_url: &str) -> Result<Playlist> {
        let raw = Self::entry(html, "playlist")?;
        let playlist: ScPlaylist = serde_json::from_value(raw)?;
        let id = id_string(playlist.id.as_ref())
            .ok_or_else(|| anyhow!("Could not find SoundCloud playlist metadata in the page."))?;
        let title = trimmed(playlist.title.as_ref())
            .ok_or_else(|| anyhow!("Could not find SoundCloud playlist metadata in the page."))?;
        let art = trimmed(playlist.artwork_url.as_ref());
        let tracks: Vec<Track> = playlist
            .tracks
            .unwrap_or_default()
            .iter()
            .filter_map(|t| Self::normalize(t, art.as_deref()))
            .collect();
        if tracks.is_empty() {
            bail!("No tracks were found in the SoundCloud playlist.");
        }
        Ok(Playlist {
            id,
            title,
            owner: playlist.user.as_ref().and_then(|u| trimmed(u.username.as_ref())),
            artwork_url: art,
            provider: ProviderId::Soundcloud,
            source_url: trimmed(playlist.permalink_url.as_ref())
                .unwrap_or_else(|| source_url.to_string()),
            tracks,
        })
    }

    pub fn parse_track_html(html: &str, source_url: &str) -> Result<Playlist> {
        let raw = Self::entry(html, "sound")?;
        let track: ScTrack = serde_json::from_value(raw)?;
        let normalized = Self::normalize(&track, None)
            .ok_or_else(|| anyhow!("Could not normalize the SoundCloud track metadata."))?;
        Ok(Playlist {
            id: normalized.id.clone(),
            title: normalized.title.clone(),
            owner: normalized.artists.first().cloned(),
            artwork_url: normalized.artwork_url.clone(),
            provider: ProviderId::Soundcloud,
            source_url: normalized
                .source_url
                .clone()
                .unwrap_or_else(|| source_url.to_string()),
            tracks: vec![normalized],
        })
    }

    /// Anything without `/sets/` in the path is a single track.
    fn is_track_url(url: &str) -> bool {
        Url::parse(url)
            .map(|u| !u.path().contains("/sets/"))
            .unwrap_or(true)
    }
}

#[async_trait::async_trait]
impl Provider for Soundcloud {
    fn id(&self) -> ProviderId {
        ProviderId::Soundcloud
    }
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &["on.soundcloud.com", "snd.sc"]
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase).unwrap_or_default();
        if !["soundcloud.com", "www.soundcloud.com", "m.soundcloud.com"].contains(&host.as_str()) {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        // `/user/sets/name`, `/discover/sets/name`, or `/user/track`. The two
        // exclusions below are the pages that have that shape but hold nothing
        // to download: `/discover/<anything>` and a bare `/user/sets` index.
        let shaped = (segments.len() == 3 && segments[1] == "sets")
            || (segments.len() == 3 && segments[0] == "discover" && segments[1] == "sets")
            || segments.len() == 2;
        if !shaped {
            return false;
        }
        !(segments.len() == 2 && (segments[0] == "discover" || segments[1] == "sets"))
    }
    fn normalize(&self, url: &Url) -> String {
        strip_query_and_hash(url)
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let html = client
            .get(url)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        if Self::is_track_url(url) {
            Self::parse_track_html(&html, url)
        } else {
            Self::parse_playlist_html(&html, url)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SET: &str = r#"<html><body><script>window.__sc_hydration = [
{"hydratable":"playlist","data":{"id":123,"title":"My Set","permalink_url":"https://soundcloud.com/u/sets/my-set",
 "artwork_url":"https://i1.sndcdn.com/set.jpg","user":{"username":"DJ U"},
 "tracks":[
   {"id":1,"title":"First","duration":210000,"permalink_url":"https://soundcloud.com/u/first",
    "publisher_metadata":{"artist":"Real Artist","album_title":"Real Album"}},
   {"id":2,"title":"Second","duration":180000,"user":{"username":"Uploader"}}
 ]}}];</script></body></html>"#;

    #[test]
    fn a_set_reads_its_tracks_out_of_the_hydration_blob() {
        let pl = Soundcloud::parse_playlist_html(SET, "https://soundcloud.com/u/sets/my-set").unwrap();
        assert_eq!(pl.id, "123");
        assert_eq!(pl.title, "My Set");
        assert_eq!(pl.owner.as_deref(), Some("DJ U"));
        assert_eq!(pl.tracks.len(), 2);
        assert_eq!(pl.tracks[0].artists, vec!["Real Artist".to_string()]);
        assert_eq!(pl.tracks[0].album.as_deref(), Some("Real Album"));
        assert_eq!(pl.tracks[0].duration_ms, Some(210_000));
        // No publisher metadata → the uploader is the artist, and the track
        // falls back to the set's cover.
        assert_eq!(pl.tracks[1].artists, vec!["Uploader".to_string()]);
        assert_eq!(pl.tracks[1].artwork_url.as_deref(), Some("https://i1.sndcdn.com/set.jpg"));
    }

    #[test]
    fn a_single_track_page_is_a_one_track_playlist() {
        let html = r#"<script>window.__sc_hydration = [{"hydratable":"sound","data":
{"id":9,"title":"Solo","duration":1000,"user":{"username":"Someone"},
 "permalink_url":"https://soundcloud.com/someone/solo"}}];</script>"#;
        let pl = Soundcloud::parse_track_html(html, "https://soundcloud.com/someone/solo").unwrap();
        assert_eq!(pl.tracks.len(), 1);
        assert_eq!(pl.title, "Solo");
        assert_eq!(pl.owner.as_deref(), Some("Someone"));
    }

    #[test]
    fn only_track_and_set_urls_match() {
        let m = |u: &str| Soundcloud.matches(&Url::parse(u).unwrap());
        assert!(m("https://soundcloud.com/artist/track-name"));
        assert!(m("https://soundcloud.com/artist/sets/a-set"));
        assert!(m("https://m.soundcloud.com/artist/track-name"));
        // Index pages have the same shape and nothing to download.
        assert!(!m("https://soundcloud.com/discover/charts"));
        assert!(!m("https://soundcloud.com/artist/sets"));
        assert!(!m("https://soundcloud.com/artist"));
        assert!(!m("https://example.com/artist/track"));
    }
}
