//! Apple Music provider — port of mdl `providers/AppleMusic.ts`.
//!
//! Extracts the `serialized-server-data` JSON blob, then walks the arbitrary
//! JSON tree to find the collection header (album/playlist) and song items.

use super::{get_first_non_empty, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde_json::Value;
use url::Url;

pub struct AppleMusic;

/// Depth-first visit of every JSON node (object/array/leaf).
fn walk<'a>(value: &'a Value, visitor: &mut dyn FnMut(&'a Value)) {
    visitor(value);
    match value {
        Value::Array(items) => {
            for item in items {
                walk(item, visitor);
            }
        }
        Value::Object(map) => {
            for v in map.values() {
                walk(v, visitor);
            }
        }
        _ => {}
    }
}

fn cd_kind(obj: &Value) -> Option<&str> {
    obj.get("contentDescriptor")?.get("kind")?.as_str()
}

fn store_adam_id(obj: &Value) -> Option<&str> {
    obj.get("contentDescriptor")?
        .get("identifiers")?
        .get("storeAdamID")?
        .as_str()
}

/// `{w}/{h}/{f}` template -> concrete 1200x1200 jpg URL.
fn normalize_artwork(artwork: Option<&Value>) -> Option<String> {
    let template = artwork?
        .get("dictionary")?
        .get("url")?
        .as_str()?
        .trim();
    if template.is_empty() {
        return None;
    }
    Some(
        template
            .replace("{w}", "1200")
            .replace("{h}", "1200")
            .replace("{f}", "jpg"),
    )
}

impl AppleMusic {
    fn extract_serialized(html: &str) -> Result<Value> {
        let re = Regex::new(
            r#"(?s)<script type="application/json" id="serialized-server-data">(.*?)</script>"#,
        )
        .unwrap();
        let raw = re
            .captures(html)
            .and_then(|c| c.get(1))
            .ok_or_else(|| {
                anyhow!("Could not find serialized Apple Music collection data in the page.")
            })?;
        Ok(serde_json::from_str(raw.as_str())?)
    }

    fn find_header(root: &Value) -> Option<&Value> {
        let mut found: Option<&Value> = None;
        walk(root, &mut |node| {
            if found.is_some() || !node.is_object() {
                return;
            }
            let kind = cd_kind(node);
            if matches!(kind, Some("album") | Some("playlist")) && node.get("title").map_or(false, |t| t.is_string()) {
                found = Some(node);
            }
        });
        found
    }

    fn find_tracks(root: &Value) -> Vec<&Value> {
        let mut tracks: Vec<&Value> = Vec::new();
        walk(root, &mut |node| {
            if node.is_object()
                && cd_kind(node) == Some("song")
                && node.get("title").map_or(false, |t| t.is_string())
                && node.get("artistName").map_or(false, |a| a.is_string())
            {
                tracks.push(node);
            }
        });
        // Dedupe by storeAdamID, preserving first-seen order.
        let mut seen = std::collections::HashSet::new();
        tracks
            .into_iter()
            .filter(|t| match store_adam_id(t) {
                Some(id) => seen.insert(id.to_string()),
                None => false,
            })
            .collect()
    }

    fn normalize_track(track: &Value, collection_artwork: Option<&str>) -> Track {
        let artist = track
            .get("artistName")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown artist")
            .to_string();
        let title = track
            .get("title")
            .and_then(|v| v.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .unwrap_or("Unknown title")
            .to_string();
        let id = store_adam_id(track)
            .map(String::from)
            .unwrap_or_else(|| format!("{artist}-{title}"));
        let album = track
            .get("tertiaryLinks")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|l| l.get("title"))
            .and_then(|t| t.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let track_artwork = normalize_artwork(track.get("artwork"));
        let artwork_url = get_first_non_empty(&[
            track_artwork.as_deref(),
            collection_artwork,
        ]);
        let duration_ms = track.get("duration").and_then(|v| v.as_u64());
        let source_url = track
            .get("contentDescriptor")
            .and_then(|c| c.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from);
        Track {
            id,
            title,
            artists: vec![artist],
            album,
            artwork_url,
            duration_ms,
            source_url,
        }
    }

    fn extract_track_id(source_url: &str) -> Option<String> {
        let u = Url::parse(source_url).ok()?;
        if let Some(i) = u.query_pairs().find(|(k, _)| k == "i").map(|(_, v)| v.trim().to_string()) {
            if !i.is_empty() {
                return Some(i);
            }
        }
        let re = Regex::new(r"(?i)/song/[^/]+/(\d+)/?$").unwrap();
        re.captures(u.path()).and_then(|c| c.get(1)).map(|m| m.as_str().to_string())
    }

    pub fn parse_playlist_html(
        html: &str,
        source_url: &str,
        track_id: Option<&str>,
    ) -> Result<Playlist> {
        let root = Self::extract_serialized(html)?;
        let header = Self::find_header(&root);
        let collection_artwork = header.and_then(|h| normalize_artwork(h.get("artwork")));
        let tracks: Vec<Track> = Self::find_tracks(&root)
            .into_iter()
            .map(|t| Self::normalize_track(t, collection_artwork.as_deref()))
            .collect();

        let header = header
            .ok_or_else(|| anyhow!("Could not find Apple Music collection metadata in the page."))?;

        let selected = track_id.and_then(|tid| tracks.iter().find(|t| t.id == tid).cloned());
        if tracks.is_empty() || (track_id.is_some() && selected.is_none()) {
            bail!("No tracks were found in the Apple Music collection.");
        }

        if let Some(track) = selected {
            return Ok(Playlist {
                id: track.id.clone(),
                title: track.title.clone(),
                owner: track.artists.first().cloned(),
                artwork_url: track.artwork_url.clone(),
                provider: ProviderId::AppleMusic,
                source_url: track.source_url.clone().unwrap_or_else(|| source_url.to_string()),
                tracks: vec![track],
            });
        }

        let is_album = cd_kind(header) == Some("album");
        let id = store_adam_id(header)
            .map(String::from)
            .unwrap_or_else(|| format!("{}-apple", if is_album { "album" } else { "playlist" }));
        let title = header
            .get("title")
            .and_then(|t| t.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| format!("Apple Music {}", if is_album { "Album" } else { "Playlist" }));
        let owner = header
            .get("subtitleLinks")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|l| l.get("title"))
            .and_then(|t| t.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from);
        let coll_url = header
            .get("contentDescriptor")
            .and_then(|c| c.get("url"))
            .and_then(|u| u.as_str())
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(String::from)
            .unwrap_or_else(|| source_url.to_string());

        Ok(Playlist {
            id,
            title,
            owner,
            artwork_url: collection_artwork,
            provider: ProviderId::AppleMusic,
            source_url: coll_url,
            tracks,
        })
    }
}

#[async_trait::async_trait]
impl Provider for AppleMusic {
    fn id(&self) -> ProviderId {
        ProviderId::AppleMusic
    }
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &["apple.co"]
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(|h| h.to_lowercase()).unwrap_or_default();
        if host != "music.apple.com" && host != "geo.music.apple.com" {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        let path = if path.is_empty() { "/" } else { path };
        let pats = [
            r"(?i)^/(?:[a-z]{2}(?:-[a-z]{2})?/)?album/[^/]+/\d+$",
            r"(?i)^/(?:[a-z]{2}(?:-[a-z]{2})?/)?playlist/[^/]+/pl\.[A-Za-z0-9._-]+$",
            r"(?i)^/(?:[a-z]{2}(?:-[a-z]{2})?/)?song/[^/]+/\d+$",
        ];
        pats.iter().any(|p| Regex::new(p).unwrap().is_match(path))
    }
    fn normalize(&self, url: &Url) -> String {
        // Preserve the `i=` single-song selector; drop everything else.
        let i = url.query_pairs().find(|(k, _)| k == "i").map(|(_, v)| v.into_owned());
        let mut u = url.clone();
        u.set_query(None);
        u.set_fragment(None);
        if let Some(i) = i.filter(|s| !s.trim().is_empty()) {
            u.query_pairs_mut().append_pair("i", i.trim());
        }
        u.to_string()
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let track_id = Self::extract_track_id(url);
        let html = client
            .get(url)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        Self::parse_playlist_html(&html, url, track_id.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE_HTML: &str = r#"<!DOCTYPE html><html><body>
<script type="application/json" id="serialized-server-data">{"data":[{"id":"playlist-detail-header - pl.test","title":"New Music Daily","subtitleLinks":[{"title":"Apple Music"}],"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/thumb/Features/v4/test/{w}x{h}SC.DN01.{f}"}},"contentDescriptor":{"kind":"playlist","identifiers":{"storeAdamID":"pl.test"},"url":"https://music.apple.com/us/playlist/new-music-daily/pl.test"}},{"id":"track-lockup - pl.test - 1868862384","title":"SWIM","artistName":"BTS","duration":159008,"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/test/{w}x{h}bb.{f}"}},"tertiaryLinks":[{"title":"ARIRANG"}],"contentDescriptor":{"kind":"song","identifiers":{"storeAdamID":"1868862384"},"url":"https://music.apple.com/us/album/swim/1868862375?i=1868862384"}},{"id":"track-lockup - pl.test - 1871085694","title":"Click Clack Symphony. (feat. Hans Zimmer)","artistName":"RAYE","duration":301674,"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/test-2/{w}x{h}bb.{f}"}},"tertiaryLinks":[{"title":"Album Name"}],"contentDescriptor":{"kind":"song","identifiers":{"storeAdamID":"1871085694"},"url":"https://music.apple.com/us/album/click-clack-symphony-feat-hans-zimmer/1871085677?i=1871085694"}}],"userTokenHash":""}</script>
</body></html>"#;

    #[test]
    fn parses_playlist() {
        let pl = AppleMusic::parse_playlist_html(
            FIXTURE_HTML,
            "https://music.apple.com/us/playlist/new-music-daily/pl.test",
            None,
        )
        .unwrap();
        assert_eq!(pl.provider, ProviderId::AppleMusic);
        assert_eq!(pl.id, "pl.test");
        assert_eq!(pl.title, "New Music Daily");
        assert_eq!(pl.owner.as_deref(), Some("Apple Music"));
        assert_eq!(
            pl.artwork_url.as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Features/v4/test/1200x1200SC.DN01.jpg")
        );
        assert_eq!(pl.tracks.len(), 2);
        let t0 = &pl.tracks[0];
        assert_eq!(t0.id, "1868862384");
        assert_eq!(t0.title, "SWIM");
        assert_eq!(t0.artists, vec!["BTS".to_string()]);
        assert_eq!(t0.album.as_deref(), Some("ARIRANG"));
        assert_eq!(
            t0.artwork_url.as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/test/1200x1200bb.jpg")
        );
        assert_eq!(t0.duration_ms, Some(159008));
        assert_eq!(
            t0.source_url.as_deref(),
            Some("https://music.apple.com/us/album/swim/1868862375?i=1868862384")
        );
    }

    #[test]
    fn parses_album() {
        let html = r#"<script type="application/json" id="serialized-server-data">{"data":[{"id":"album-detail-header - 1440837083","title":"Whenever You Need Somebody","subtitleLinks":[{"title":"Rick Astley"}],"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/album/{w}x{h}bb.{f}"}},"contentDescriptor":{"kind":"album","identifiers":{"storeAdamID":"1440837083"},"url":"https://music.apple.com/us/album/whenever-you-need-somebody/1440837083"}},{"id":"track-lockup - 1440837083 - 1440837084","title":"Never Gonna Give You Up","artistName":"Rick Astley","duration":213000,"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/track/{w}x{h}bb.{f}"}},"tertiaryLinks":[{"title":"Whenever You Need Somebody"}],"contentDescriptor":{"kind":"song","identifiers":{"storeAdamID":"1440837084"},"url":"https://music.apple.com/us/album/never-gonna-give-you-up/1440837083?i=1440837084"}}],"userTokenHash":""}</script>"#;
        let pl = AppleMusic::parse_playlist_html(
            html,
            "https://music.apple.com/us/album/whenever-you-need-somebody/1440837083",
            None,
        )
        .unwrap();
        assert_eq!(pl.id, "1440837083");
        assert_eq!(pl.title, "Whenever You Need Somebody");
        assert_eq!(pl.owner.as_deref(), Some("Rick Astley"));
        let t = &pl.tracks[0];
        assert_eq!(t.id, "1440837084");
        assert_eq!(t.title, "Never Gonna Give You Up");
        assert_eq!(t.album.as_deref(), Some("Whenever You Need Somebody"));
        assert_eq!(t.duration_ms, Some(213000));
    }

    #[test]
    fn falls_back_to_collection_artwork() {
        let html = r#"<script type="application/json" id="serialized-server-data">{"data":[{"id":"playlist-detail-header - pl.test","title":"New Music Daily","subtitleLinks":[{"title":"Apple Music"}],"artwork":{"dictionary":{"url":"https://is1-ssl.mzstatic.com/image/thumb/Features/v4/test/{w}x{h}SC.DN01.{f}"}},"contentDescriptor":{"kind":"playlist","identifiers":{"storeAdamID":"pl.test"},"url":"https://music.apple.com/us/playlist/new-music-daily/pl.test"}},{"id":"track-lockup - pl.test - 1868862384","title":"SWIM","artistName":"BTS","duration":159008,"tertiaryLinks":[{"title":"ARIRANG"}],"contentDescriptor":{"kind":"song","identifiers":{"storeAdamID":"1868862384"},"url":"https://music.apple.com/us/album/swim/1868862375?i=1868862384"}}],"userTokenHash":""}</script>"#;
        let pl = AppleMusic::parse_playlist_html(
            html,
            "https://music.apple.com/us/playlist/new-music-daily/pl.test",
            None,
        )
        .unwrap();
        assert_eq!(
            pl.tracks[0].artwork_url.as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Features/v4/test/1200x1200SC.DN01.jpg")
        );
    }

    #[test]
    fn matches_apple_urls() {
        assert!(AppleMusic.matches(
            &Url::parse("https://music.apple.com/us/playlist/foo/pl.u-abc123").unwrap()
        ));
        assert!(AppleMusic.matches(
            &Url::parse("https://music.apple.com/us/album/foo/1440837083").unwrap()
        ));
        assert!(AppleMusic.matches(
            &Url::parse("https://music.apple.com/gb/song/foo/12345?i=67890").unwrap()
        ));
        assert!(!AppleMusic.matches(&Url::parse("https://music.apple.com/us/artist/foo/123").unwrap()));
        assert!(!AppleMusic.matches(&Url::parse("https://example.com/album/foo/1").unwrap()));
    }

    #[test]
    fn normalize_preserves_i_param() {
        let u = Url::parse("https://music.apple.com/us/album/foo/123?i=456&x=y").unwrap();
        assert_eq!(AppleMusic.normalize(&u), "https://music.apple.com/us/album/foo/123?i=456");
    }
}
