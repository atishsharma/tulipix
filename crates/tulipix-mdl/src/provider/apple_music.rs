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

/// `{w}/{h}/{f}` template -> concrete 3000x3000 jpg URL.
fn normalize_artwork(artwork: Option<&Value>) -> Option<String> {
    let template = artwork?
        .get("dictionary")?
        .get("url")?
        .as_str()?
        .trim();
    if template.is_empty() {
        return None;
    }
    // Request Apple's high-res master (the CDN returns the native size, clamped
    // down from this) so the embedded cover is sharp, not a 300px thumbnail.
    Some(
        template
            .replace("{w}", "3000")
            .replace("{h}", "3000")
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
            Some("https://is1-ssl.mzstatic.com/image/thumb/Features/v4/test/3000x3000SC.DN01.jpg")
        );
        assert_eq!(pl.tracks.len(), 2);
        let t0 = &pl.tracks[0];
        assert_eq!(t0.id, "1868862384");
        assert_eq!(t0.title, "SWIM");
        assert_eq!(t0.artists, vec!["BTS".to_string()]);
        assert_eq!(t0.album.as_deref(), Some("ARIRANG"));
        assert_eq!(
            t0.artwork_url.as_deref(),
            Some("https://is1-ssl.mzstatic.com/image/thumb/Music211/v4/test/3000x3000bb.jpg")
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
            Some("https://is1-ssl.mzstatic.com/image/thumb/Features/v4/test/3000x3000SC.DN01.jpg")
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

// ── search ──────────────────────────────────────────────────────────────────
//
// Nothing above this line is involved. Resolving a music.apple.com URL means
// scraping a `serialized-server-data` blob out of a page; searching goes to the
// iTunes Search API, which Apple documents, publishes without a key, and has
// kept stable for fifteen years. Two different doors into the same catalogue,
// and this is by far the better-behaved one.

#[derive(serde::Deserialize)]
struct ItunesPage {
    results: Option<Vec<ItunesTrack>>,
}

#[derive(serde::Deserialize)]
struct ItunesTrack {
    #[serde(rename = "trackId")]
    track_id: Option<i64>,
    #[serde(rename = "trackName")]
    track_name: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "collectionName")]
    collection_name: Option<String>,
    /// 100x100 as published. The catalogue serves any size from the same path,
    /// so it is rewritten below rather than embedded at thumbnail resolution.
    #[serde(rename = "artworkUrl100")]
    artwork_url100: Option<String>,
    #[serde(rename = "trackTimeMillis")]
    track_time_millis: Option<u64>,
    #[serde(rename = "trackViewUrl")]
    track_view_url: Option<String>,
}

/// `…/100x100bb.jpg` → `…/600x600bb.jpg`. The tagger embeds whatever it is
/// given, and a 100px cover in a music library is a thumbnail forever.
fn big_art(url: &str) -> String {
    url.replace("/100x100bb.", "/600x600bb.")
}

fn clean(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Search Apple's catalogue by name, through the iTunes Search API.
pub async fn search(client: &reqwest::Client, query: &str, limit: usize) -> Result<Playlist> {
    let q = query.trim();
    if q.is_empty() {
        bail!("Type something to search.");
    }
    let url = Url::parse_with_params(
        "https://itunes.apple.com/search",
        &[
            ("term", q),
            ("entity", "song"),
            ("media", "music"),
            ("limit", &limit.clamp(1, 200).to_string()),
        ],
    )?
    .to_string();

    // The endpoint answers `text/javascript` rather than `application/json`,
    // which is a leftover from when it was a JSONP API. `json()` in reqwest
    // does not check the content type, but say so here so the next person does
    // not go looking for a bug when they see it in a network trace.
    let page: ItunesPage = client
        .get(&url)
        .header("user-agent", "Mozilla/5.0")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let tracks: Vec<Track> = page
        .results
        .unwrap_or_default()
        .iter()
        .filter_map(itunes_to_track)
        .collect();
    if tracks.is_empty() {
        bail!("Apple found nothing for “{q}”.");
    }
    Ok(Playlist {
        id: format!("search:{q}"),
        title: format!("Search: {q}"),
        owner: None,
        artwork_url: tracks.iter().find_map(|t| t.artwork_url.clone()),
        provider: ProviderId::AppleMusic,
        source_url: url,
        tracks,
    })
}

fn itunes_to_track(r: &ItunesTrack) -> Option<Track> {
    let title = clean(r.track_name.as_ref())?;
    let artist = clean(r.artist_name.as_ref())?;
    Some(Track {
        id: r
            .track_id
            .map(|i| i.to_string())
            .unwrap_or_else(|| format!("{artist}-{title}")),
        title,
        artists: vec![artist],
        album: clean(r.collection_name.as_ref()),
        artwork_url: clean(r.artwork_url100.as_ref()).map(|u| big_art(&u)),
        duration_ms: r.track_time_millis.filter(|ms| *ms > 0),
        source_url: clean(r.track_view_url.as_ref()),
    })
}

#[cfg(test)]
mod search_tests {
    use super::*;

    const PAYLOAD: &str = r#"{"resultCount":2,"results":[
      {"trackId":1,"trackName":"Roads","artistName":"Portishead",
       "collectionName":"Dummy",
       "artworkUrl100":"https://is1.mzstatic.com/image/thumb/x/100x100bb.jpg",
       "trackTimeMillis":304000,
       "trackViewUrl":"https://music.apple.com/us/album/roads/1?i=2"},
      {"trackId":2,"artistName":"No Title Here"}
    ]}"#;

    #[test]
    fn a_payload_becomes_tracks_and_skips_the_untitled() {
        let page: ItunesPage = serde_json::from_str(PAYLOAD).unwrap();
        let tracks: Vec<Track> = page
            .results
            .unwrap()
            .iter()
            .filter_map(itunes_to_track)
            .collect();
        assert_eq!(tracks.len(), 1, "a result with no track name is not a track");
        assert_eq!(tracks[0].title, "Roads");
        assert_eq!(tracks[0].artists, vec!["Portishead".to_string()]);
        assert_eq!(tracks[0].album.as_deref(), Some("Dummy"));
        assert_eq!(tracks[0].duration_ms, Some(304_000));
    }

    #[test]
    fn artwork_is_asked_for_at_a_useful_size() {
        assert_eq!(
            big_art("https://is1.mzstatic.com/image/thumb/x/100x100bb.jpg"),
            "https://is1.mzstatic.com/image/thumb/x/600x600bb.jpg"
        );
        // Anything that is not the published shape is left alone rather than
        // mangled.
        assert_eq!(big_art("https://example.invalid/a.jpg"), "https://example.invalid/a.jpg");
    }

    #[test]
    fn a_duration_of_zero_is_no_duration() {
        let r = ItunesTrack {
            track_id: Some(9),
            track_name: Some("T".into()),
            artist_name: Some("A".into()),
            collection_name: None,
            artwork_url100: None,
            track_time_millis: Some(0),
            track_view_url: None,
        };
        // Zero would otherwise reach the downloader's duration check as a real
        // target and refuse every hit.
        assert_eq!(itunes_to_track(&r).unwrap().duration_ms, None);
    }
}
