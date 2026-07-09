//! YouTube Music provider.
//!
//! Upstream mdl uses `youtubei.js` (Innertube), which has no Rust equivalent.
//! Native fit: resolve the track list with the app's bundled **yt-dlp** — the
//! same tool the YouTube section already uses. `matches` + `parse_ytdlp_json`
//! are pure and unit-tested; `fetch` shells yt-dlp.

use super::Provider;
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde_json::Value;
use tulipix_core::proc::NoWindow;
use url::Url;

pub struct YoutubeMusic;

/// Build a [`Track`] from a yt-dlp info/entry object (handles both the flat
/// playlist-entry shape and the rich single-video shape).
fn track_from_json(entry: &Value) -> Option<Track> {
    let id = entry.get("id").and_then(|v| v.as_str())?.to_string();
    // yt-dlp populates `track`/`artist`/`album` for recognized music; fall back
    // to generic video fields otherwise.
    let title = entry
        .get("track")
        .and_then(|v| v.as_str())
        .or_else(|| entry.get("title").and_then(|v| v.as_str()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())?
        .to_string();
    let artists: Vec<String> = entry
        .get("artist")
        .and_then(|v| v.as_str())
        .map(|s| s.split(',').map(|a| a.trim().to_string()).filter(|a| !a.is_empty()).collect::<Vec<_>>())
        .filter(|v| !v.is_empty())
        .or_else(|| {
            entry
                .get("channel")
                .or_else(|| entry.get("uploader"))
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .map(|s| vec![s.trim_end_matches(" - Topic").to_string()])
        })
        .unwrap_or_default();
    if artists.is_empty() {
        return None;
    }
    let album = entry
        .get("album")
        .and_then(|v| v.as_str())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .map(String::from);
    let duration_ms = entry
        .get("duration")
        .and_then(|v| v.as_f64())
        .map(|s| (s * 1000.0) as u64);
    let artwork_url = entry
        .get("thumbnails")
        .and_then(|v| v.as_array())
        .and_then(|a| a.last())
        .and_then(|t| t.get("url"))
        .and_then(|u| u.as_str())
        .map(String::from)
        .or_else(|| entry.get("thumbnail").and_then(|v| v.as_str()).map(String::from));
    Some(Track {
        id: id.clone(),
        title,
        artists,
        album,
        artwork_url,
        duration_ms,
        source_url: Some(format!("https://music.youtube.com/watch?v={id}")),
    })
}

impl YoutubeMusic {
    /// Parse yt-dlp `-J` output (playlist with `entries`, or a single video)
    /// into a [`Playlist`].
    pub fn parse_ytdlp_json(stdout: &str, source_url: &str) -> Result<Playlist> {
        let root: Value = serde_json::from_str(stdout.trim())
            .map_err(|e| anyhow!("yt-dlp produced invalid JSON: {e}"))?;

        if let Some(entries) = root.get("entries").and_then(|v| v.as_array()) {
            let tracks: Vec<Track> = entries.iter().filter_map(track_from_json).collect();
            if tracks.is_empty() {
                bail!("No tracks were found in the YouTube Music playlist.");
            }
            let title = root
                .get("title")
                .and_then(|v| v.as_str())
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .unwrap_or("YouTube Music Playlist")
                .to_string();
            let id = root
                .get("id")
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| title.clone());
            let artwork_url = root
                .get("thumbnails")
                .and_then(|v| v.as_array())
                .and_then(|a| a.last())
                .and_then(|t| t.get("url"))
                .and_then(|u| u.as_str())
                .map(String::from);
            return Ok(Playlist {
                id,
                title,
                owner: None,
                artwork_url,
                provider: ProviderId::YoutubeMusic,
                source_url: source_url.to_string(),
                tracks,
            });
        }

        // Single video.
        let track = track_from_json(&root)
            .ok_or_else(|| anyhow!("Could not find YouTube Music track metadata."))?;
        Ok(Playlist {
            id: track.id.clone(),
            title: track.title.clone(),
            owner: track.artists.first().cloned(),
            artwork_url: track.artwork_url.clone(),
            provider: ProviderId::YoutubeMusic,
            source_url: track.source_url.clone().unwrap_or_else(|| source_url.to_string()),
            tracks: vec![track],
        })
    }
}

#[async_trait::async_trait]
impl Provider for YoutubeMusic {
    fn id(&self) -> ProviderId {
        ProviderId::YoutubeMusic
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(|h| h.to_lowercase()).unwrap_or_default();
        if !["music.youtube.com", "www.youtube.com", "youtube.com"].contains(&host.as_str()) {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        let has = |k: &str| url.query_pairs().any(|(key, v)| key == k && !v.trim().is_empty());
        (path.eq_ignore_ascii_case("/playlist") && has("list"))
            || (path.eq_ignore_ascii_case("/watch") && has("v"))
            || Regex::new(
                r"(?i)^/browse/(?:MPRE|FEmusic_library_privately_owned_release)[A-Za-z0-9_-]+$",
            )
            .unwrap()
            .is_match(path)
    }
    fn normalize(&self, url: &Url) -> String {
        // Preserve `v` + `list`; drop the rest.
        let v = url.query_pairs().find(|(k, _)| k == "v").map(|(_, v)| v.into_owned());
        let list = url.query_pairs().find(|(k, _)| k == "list").map(|(_, v)| v.into_owned());
        let mut u = url.clone();
        u.set_query(None);
        u.set_fragment(None);
        {
            let mut q = u.query_pairs_mut();
            if let Some(v) = v.filter(|s| !s.trim().is_empty()) {
                q.append_pair("v", v.trim());
            }
            if let Some(list) = list.filter(|s| !s.trim().is_empty()) {
                q.append_pair("list", list.trim());
            }
        }
        // url crate leaves a trailing '?' when no pairs were appended; trim it.
        u.to_string().trim_end_matches('?').to_string()
    }
    async fn fetch(&self, _client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let is_watch = Url::parse(url)
            .ok()
            .map(|u| u.path().trim_end_matches('/').eq_ignore_ascii_case("/watch"))
            .unwrap_or(false);
        let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
        let mut cmd = tokio::process::Command::new(bin);
        cmd.arg("-J").arg("--no-warnings");
        // Playlists/albums: flat list (fast). Single video: full info (rich tags).
        if !is_watch {
            cmd.arg("--flat-playlist");
        }
        cmd.arg(url);
        cmd.no_window(); // no console-window flash on Windows
        let out = cmd.output().await?;
        if !out.status.success() {
            bail!("yt-dlp failed: {}", String::from_utf8_lossy(&out.stderr));
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        Self::parse_ytdlp_json(&stdout, url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_playlist_entries() {
        let json = r#"{"id":"PL_TEST","title":"Focus Mix","entries":[
            {"id":"video-1","title":"First Song","track":"First Song","artist":"Artist One, Artist Guest","album":"First Album","duration":210},
            {"id":"video-2","title":"Second Song","channel":"Artist Two - Topic","duration":245.0,"thumbnails":[{"url":"https://i.ytimg.com/track-2.jpg"}]}
        ]}"#;
        let pl = YoutubeMusic::parse_ytdlp_json(json, "https://music.youtube.com/playlist?list=PL_TEST")
            .unwrap();
        assert_eq!(pl.provider, ProviderId::YoutubeMusic);
        assert_eq!(pl.id, "PL_TEST");
        assert_eq!(pl.title, "Focus Mix");
        assert_eq!(pl.tracks.len(), 2);
        assert_eq!(pl.tracks[0].title, "First Song");
        assert_eq!(pl.tracks[0].artists, vec!["Artist One".to_string(), "Artist Guest".to_string()]);
        assert_eq!(pl.tracks[0].album.as_deref(), Some("First Album"));
        assert_eq!(pl.tracks[0].duration_ms, Some(210_000));
        assert_eq!(pl.tracks[0].source_url.as_deref(), Some("https://music.youtube.com/watch?v=video-1"));
        // channel " - Topic" suffix stripped.
        assert_eq!(pl.tracks[1].artists, vec!["Artist Two".to_string()]);
        assert_eq!(pl.tracks[1].artwork_url.as_deref(), Some("https://i.ytimg.com/track-2.jpg"));
    }

    #[test]
    fn parses_single_video() {
        let json = r#"{"id":"abc123","title":"Never Gonna Give You Up","artist":"Rick Astley","album":"Whenever You Need Somebody","duration":213}"#;
        let pl = YoutubeMusic::parse_ytdlp_json(json, "https://music.youtube.com/watch?v=abc123")
            .unwrap();
        assert_eq!(pl.tracks.len(), 1);
        assert_eq!(pl.id, "abc123");
        assert_eq!(pl.title, "Never Gonna Give You Up");
        assert_eq!(pl.owner.as_deref(), Some("Rick Astley"));
        assert_eq!(pl.tracks[0].album.as_deref(), Some("Whenever You Need Somebody"));
        assert_eq!(pl.tracks[0].duration_ms, Some(213_000));
    }

    #[test]
    fn matches_ytmusic_urls() {
        assert!(YoutubeMusic.matches(&Url::parse("https://music.youtube.com/playlist?list=PLabc").unwrap()));
        assert!(YoutubeMusic.matches(&Url::parse("https://music.youtube.com/watch?v=abc123").unwrap()));
        assert!(YoutubeMusic.matches(&Url::parse("https://music.youtube.com/browse/MPREb_abc123").unwrap()));
        assert!(!YoutubeMusic.matches(&Url::parse("https://music.youtube.com/playlist").unwrap()));
        assert!(!YoutubeMusic.matches(&Url::parse("https://example.com/watch?v=abc").unwrap()));
    }

    #[test]
    fn normalize_keeps_v_and_list() {
        let u = Url::parse("https://music.youtube.com/watch?v=abc&list=PL1&foo=bar").unwrap();
        let n = YoutubeMusic.normalize(&u);
        assert!(n.contains("v=abc"));
        assert!(n.contains("list=PL1"));
        assert!(!n.contains("foo=bar"));
    }
}
