//! Spotify provider — port of mdl `providers/Spotify.ts`.
//!
//! Strategy: hit `open.spotify.com/embed/{kind}/{id}`, extract the
//! `__NEXT_DATA__` JSON blob, deserialize `props.pageProps.state.data.entity`,
//! normalize into tracks.

use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;
use url::Url;

pub struct Spotify;

#[derive(Deserialize)]
struct Payload {
    props: Option<Props>,
}
#[derive(Deserialize)]
struct Props {
    #[serde(rename = "pageProps")]
    page_props: Option<PageProps>,
}
#[derive(Deserialize)]
struct PageProps {
    state: Option<State>,
}
#[derive(Deserialize)]
struct State {
    data: Option<Data>,
}
#[derive(Deserialize)]
struct Data {
    entity: Option<Entity>,
}
#[derive(Deserialize)]
struct Entity {
    id: Option<String>,
    uri: Option<String>,
    name: Option<String>,
    title: Option<String>,
    subtitle: Option<String>,
    duration: Option<u64>,
    artists: Option<Vec<Artist>>,
    #[serde(rename = "trackList")]
    track_list: Option<Vec<TrackItem>>,
    #[serde(rename = "coverArt")]
    cover_art: Option<CoverArt>,
    #[serde(rename = "visualIdentity")]
    visual_identity: Option<VisualIdentity>,
}
#[derive(Deserialize)]
struct Artist {
    name: Option<String>,
}
#[derive(Deserialize)]
struct TrackItem {
    title: Option<String>,
    subtitle: Option<String>,
    uri: Option<String>,
    duration: Option<u64>,
}
#[derive(Deserialize)]
struct CoverArt {
    sources: Option<Vec<ImgSrc>>,
}
#[derive(Deserialize)]
struct VisualIdentity {
    image: Option<Vec<ImgSrc>>,
}
#[derive(Deserialize)]
struct ImgSrc {
    url: Option<String>,
    // Spotify serves several fixed sizes; pick the biggest so the embedded
    // cover (and the library thumb extracted from it) is sharp, not a 64px chip.
    width: Option<u64>,
}

/// Largest-by-width image URL from a source list (Spotify orders these small→
/// large or vice-versa depending on the surface, so never just take `.first()`).
fn largest_src(srcs: Option<&Vec<ImgSrc>>) -> Option<&str> {
    srcs?
        .iter()
        .filter(|s| s.url.as_deref().map(|u| !u.is_empty()).unwrap_or(false))
        .max_by_key(|s| s.width.unwrap_or(0))
        .and_then(|s| s.url.as_deref())
}

// ── Search (anonymous web-player token) ──────────────────────────────────────
// Spotify has no public search without auth. The web player bootstraps an
// anonymous access token from `open.spotify.com/get_access_token`; while that
// unofficial endpoint answers, the regular `api.spotify.com/v1/search` accepts
// the token. It DOES change occasionally — callers must treat any error here
// as "fall back to the YT Music search".

#[derive(Deserialize)]
struct AnonToken {
    #[serde(rename = "accessToken")]
    access_token: Option<String>,
}
#[derive(Deserialize)]
struct SearchResp {
    tracks: Option<SearchTracks>,
}
#[derive(Deserialize)]
struct SearchTracks {
    items: Option<Vec<SearchTrack>>,
}
#[derive(Deserialize)]
struct SearchTrack {
    id: Option<String>,
    name: Option<String>,
    duration_ms: Option<u64>,
    artists: Option<Vec<Artist>>,
    album: Option<SearchAlbum>,
    external_urls: Option<SearchUrls>,
}
#[derive(Deserialize)]
struct SearchAlbum {
    name: Option<String>,
    images: Option<Vec<ImgSrc>>,
}
#[derive(Deserialize)]
struct SearchUrls {
    spotify: Option<String>,
}

/// Downloader Search mode via Spotify: anonymous token + `/v1/search`, results
/// as a [`Playlist`] so the existing queue/tag/download pipeline applies
/// unchanged (audio still comes from YouTube per track — Spotify only supplies
/// the metadata: full artist credits, album, duration, art).
pub async fn search(client: &reqwest::Client, query: &str, limit: usize) -> Result<Playlist> {
    let q = query.trim();
    if q.is_empty() {
        bail!("Type something to search.");
    }
    let tok: AnonToken = client
        .get("https://open.spotify.com/get_access_token?reason=transport&productType=web_player")
        .header(
            "user-agent",
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36",
        )
        .header("accept", "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await
        .map_err(|e| anyhow!("Spotify token endpoint changed shape: {e}"))?;
    let token = tok
        .access_token
        .filter(|t| !t.is_empty())
        .ok_or_else(|| anyhow!("Spotify did not hand out an anonymous token."))?;
    let mut u = Url::parse("https://api.spotify.com/v1/search").unwrap();
    u.query_pairs_mut()
        .append_pair("type", "track")
        .append_pair("limit", &limit.clamp(1, 50).to_string())
        .append_pair("q", q);
    let resp: SearchResp = client
        .get(u)
        .bearer_auth(&token)
        .header("accept", "application/json")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let tracks: Vec<Track> = resp
        .tracks
        .and_then(|t| t.items)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| {
            let id = t.id?;
            let title = t.name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())?;
            let artists: Vec<String> = t
                .artists
                .unwrap_or_default()
                .into_iter()
                .filter_map(|a| a.name)
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            if artists.is_empty() {
                return None;
            }
            let (album, artwork_url) = match t.album {
                Some(al) => (
                    al.name.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()),
                    largest_src(al.images.as_ref()).map(String::from),
                ),
                None => (None, None),
            };
            Some(Track {
                source_url: t
                    .external_urls
                    .and_then(|u| u.spotify)
                    .or_else(|| Some(format!("https://open.spotify.com/track/{id}"))),
                id,
                title,
                artists,
                album,
                artwork_url,
                duration_ms: t.duration_ms,
            })
        })
        .collect();
    if tracks.is_empty() {
        bail!("No Spotify tracks matched.");
    }
    Ok(Playlist {
        id: format!("spsearch:{q}"),
        title: format!("Search: {q}"),
        owner: None,
        artwork_url: tracks.first().and_then(|t| t.artwork_url.clone()),
        provider: ProviderId::Spotify,
        source_url: format!("spsearch:{q}"),
        tracks,
    })
}

impl Spotify {
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

    fn extract_id(value: &str, kind: &str) -> Option<String> {
        let re = Regex::new(&format!(r"(?:{kind}/|spotify:{kind}:)([A-Za-z0-9]+)")).ok()?;
        re.captures(value)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
    }

    pub fn parse_collection_html(html: &str, source_url: &str) -> Result<Playlist> {
        let re = Regex::new(
            r#"(?s)<script id="__NEXT_DATA__" type="application/json">(.*?)</script>"#,
        )
        .unwrap();
        let json = re
            .captures(html)
            .and_then(|c| c.get(1))
            .ok_or_else(|| anyhow!("Could not find Spotify collection data in the page."))?;
        let payload: Payload = serde_json::from_str(json.as_str().trim())?;
        let entity = payload
            .props
            .and_then(|p| p.page_props)
            .and_then(|p| p.state)
            .and_then(|s| s.data)
            .and_then(|d| d.entity)
            .ok_or_else(|| anyhow!("Could not parse Spotify collection data."))?;

        let kind = Self::collection_kind(source_url);
        let title = get_first_non_empty(&[entity.title.as_deref(), entity.name.as_deref()])
            .unwrap_or_else(|| format!("Spotify {kind}"));
        let artwork = get_first_non_empty(&[
            largest_src(entity.cover_art.as_ref().and_then(|c| c.sources.as_ref())),
            largest_src(entity.visual_identity.as_ref().and_then(|v| v.image.as_ref())),
        ]);
        let owner = get_first_non_empty(&[entity.subtitle.as_deref()]).or_else(|| {
            entity
                .artists
                .as_ref()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.name.as_deref())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|s| !s.is_empty())
        });

        let tracks: Vec<Track> = if kind == "track" {
            let artists: Vec<String> = entity
                .artists
                .as_ref()
                .map(|a| {
                    a.iter()
                        .filter_map(|x| x.name.as_ref().map(|s| s.trim().to_string()))
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            let id = entity
                .id
                .clone()
                .or_else(|| entity.uri.as_deref().and_then(|u| Self::extract_id(u, "track")));
            match id {
                Some(id) if !title.is_empty() && !artists.is_empty() => vec![Track {
                    id,
                    title: title.clone(),
                    artists,
                    album: None,
                    artwork_url: artwork.clone(),
                    duration_ms: entity.duration,
                    source_url: Some(source_url.to_string()),
                }],
                _ => vec![],
            }
        } else {
            entity
                .track_list
                .unwrap_or_default()
                .into_iter()
                .filter_map(|t| {
                    let ttitle = t.title.as_ref()?.trim().to_string();
                    if ttitle.is_empty() {
                        return None;
                    }
                    let artists: Vec<String> = t
                        .subtitle
                        .as_deref()
                        .unwrap_or("")
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if artists.is_empty() {
                        return None;
                    }
                    let track_id = t
                        .uri
                        .as_deref()
                        .and_then(|u| Self::extract_id(u, "track"));
                    let id = track_id
                        .clone()
                        .unwrap_or_else(|| format!("{}-{}", artists.join(","), ttitle));
                    Some(Track {
                        id,
                        title: ttitle,
                        artists,
                        album: Some(title.clone()),
                        artwork_url: artwork.clone(),
                        duration_ms: t.duration,
                        source_url: track_id
                            .map(|id| format!("https://open.spotify.com/track/{id}")),
                    })
                })
                .collect()
        };

        if tracks.is_empty() {
            bail!("No tracks were found in the Spotify {kind}.");
        }
        let id = entity
            .id
            .filter(|s| !s.trim().is_empty())
            .or_else(|| entity.uri.as_deref().and_then(|u| Self::extract_id(u, kind)))
            .or_else(|| Self::extract_id(source_url, kind))
            .unwrap_or_else(|| format!("{kind}-spotify"));
        Ok(Playlist {
            id,
            title,
            owner,
            artwork_url: artwork,
            provider: ProviderId::Spotify,
            source_url: source_url.to_string(),
            tracks,
        })
    }
}

// ── Per-track enrichment ─────────────────────────────────────────────────────
// The embed's `trackList` carries a title, an artist string and a duration —
// and nothing else. Tagging from that alone means every track in a PLAYLIST
// gets the playlist's name written into its album tag, and a single-track link
// gets no album at all. The public track page states the real one in its
// `og:description`: `Artist · Song · Album · Year`.

/// How many track pages are read at once. Eight is upstream's number: enough
/// that a 100-track playlist resolves in a few seconds, few enough that Spotify
/// does not start refusing.
const ENRICH_CONCURRENCY: usize = 8;

/// `(album, artwork_url)` out of a track page's meta tags.
fn parse_track_page(html: &str) -> (Option<String>, Option<String>) {
    let re = Regex::new(
        r#"<meta[^>]+(?:property|name)=["']([^"']+)["'][^>]+content=["']([^"']*)["'][^>]*>"#,
    )
    .unwrap();
    let mut og_description = None;
    let mut tw_description = None;
    let mut og_image = None;
    let mut tw_image = None;
    for c in re.captures_iter(html) {
        let value = decode_entities(&c[2]);
        match &c[1] {
            "og:description" => og_description = Some(value),
            "twitter:description" => tw_description = Some(value),
            "og:image" => og_image = Some(value),
            "twitter:image" => tw_image = Some(value),
            _ => {}
        }
    }
    let description =
        get_first_non_empty(&[og_description.as_deref(), tw_description.as_deref()]);
    let artwork = get_first_non_empty(&[og_image.as_deref(), tw_image.as_deref()]);
    (album_from_description(description.as_deref()), artwork)
}

/// The album out of `Artist · Song · Album · Year`.
///
/// The `Song` marker is what anchors it, with an artist segment ahead of it —
/// podcasts and episodes use the same tag with another shape, and a stray
/// "Song" with nothing in front of it is not a track description. A single
/// carries no album at all (`Artist · Song · Year`), which is why the album
/// segment has to be followed by the year rather than merely exist.
fn album_from_description(description: Option<&str>) -> Option<String> {
    let segments: Vec<&str> = description?
        .split(" · ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let song_at = segments.iter().position(|s| s.eq_ignore_ascii_case("song"))?;
    if song_at == 0 || song_at + 2 >= segments.len() {
        return None;
    }
    Some(segments[song_at + 1].to_string())
}

/// The handful of entities Spotify actually emits in a meta tag.
fn decode_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

async fn fetch_track_page(
    client: &reqwest::Client,
    url: &str,
) -> Option<(Option<String>, Option<String>)> {
    let html = client
        .get(url)
        .header("user-agent", "Mozilla/5.0")
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .text()
        .await
        .ok()?;
    Some(parse_track_page(&html))
}

/// Fill in album and artwork from each track's own page.
///
/// Best-effort by design: a track page that 404s or changes shape leaves the
/// track exactly as the embed described it, because a missing album tag is a
/// smaller problem than a failed download.
async fn enrich_tracks(client: &reqwest::Client, tracks: &mut [Track]) {
    let mut start = 0usize;
    while start < tracks.len() {
        let end = (start + ENRICH_CONCURRENCY).min(tracks.len());
        let mut set = tokio::task::JoinSet::new();
        for (offset, track) in tracks[start..end].iter().enumerate() {
            let Some(url) = track.source_url.clone() else {
                continue;
            };
            let client = client.clone();
            let at = start + offset;
            set.spawn(async move { (at, fetch_track_page(&client, &url).await) });
        }
        while let Some(joined) = set.join_next().await {
            let Ok((at, Some((album, artwork)))) = joined else {
                continue;
            };
            if let Some(album) = album {
                tracks[at].album = Some(album);
            }
            if let Some(artwork) = artwork {
                tracks[at].artwork_url = Some(artwork);
            }
        }
        start = end;
    }
}

#[async_trait::async_trait]
impl Provider for Spotify {
    fn id(&self) -> ProviderId {
        ProviderId::Spotify
    }
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &["spotify.link", "spotify.app.link"]
    }
    fn matches(&self, url: &Url) -> bool {
        if url.host_str().map(|h| h.to_lowercase()).as_deref() != Some("open.spotify.com") {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        let pats = [
            r"^/album/[A-Za-z0-9]+$",
            r"^/playlist/[A-Za-z0-9]+$",
            r"^/track/[A-Za-z0-9]+$",
            r"^/intl-[a-z]{2}/album/[A-Za-z0-9]+$",
            r"^/intl-[a-z]{2}/track/[A-Za-z0-9]+$",
            r"^/user/[^/]+/playlist/[A-Za-z0-9]+$",
            r"^/intl-[a-z]{2}/playlist/[A-Za-z0-9]+$",
        ];
        pats.iter().any(|p| Regex::new(p).unwrap().is_match(path))
    }
    fn normalize(&self, url: &Url) -> String {
        strip_query_and_hash(url)
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        let kind = Self::collection_kind(url);
        let id = Self::extract_id(url, kind)
            .ok_or_else(|| anyhow!("Could not determine the Spotify collection id."))?;
        let embed = format!("https://open.spotify.com/embed/{kind}/{id}");
        let html = client
            .get(&embed)
            .header("user-agent", "Mozilla/5.0")
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let mut playlist = Self::parse_collection_html(&html, url)?;
        // An ALBUM link already knows its album — its tracks are the album. A
        // playlist's are not, and a bare track link never carries one.
        if kind != "album" {
            enrich_tracks(client, &mut playlist.tracks).await;
        }
        Ok(playlist)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_track_from_embed() {
        let html = r#"<!DOCTYPE html><html><body>
<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"type":"track","name":"Kookaburra Sits","uri":"spotify:track:1eJdXVLxLoMWu1TkaeSL18","id":"1eJdXVLxLoMWu1TkaeSL18","title":"Kookaburra Sits","artists":[{"name":"ABC Kids","uri":"spotify:artist:6l7J2uM3bM2BCh0tIPhWx8"}],"duration":57720,"visualIdentity":{"image":[{"url":"https://image-cdn-fa.spotifycdn.com/image/track-art"}]}}}}}}}</script>
</body></html>"#;
        let pl = Spotify::parse_collection_html(
            html,
            "https://open.spotify.com/track/1eJdXVLxLoMWu1TkaeSL18",
        )
        .unwrap();
        assert_eq!(pl.id, "1eJdXVLxLoMWu1TkaeSL18");
        assert_eq!(pl.title, "Kookaburra Sits");
        assert_eq!(pl.owner.as_deref(), Some("ABC Kids"));
        assert_eq!(
            pl.artwork_url.as_deref(),
            Some("https://image-cdn-fa.spotifycdn.com/image/track-art")
        );
        assert_eq!(pl.tracks.len(), 1);
        assert_eq!(pl.tracks[0].title, "Kookaburra Sits");
        assert_eq!(pl.tracks[0].artists, vec!["ABC Kids".to_string()]);
        assert_eq!(pl.tracks[0].duration_ms, Some(57720));
    }

    #[test]
    fn parses_playlist_tracklist() {
        let html = r#"<script id="__NEXT_DATA__" type="application/json">{"props":{"pageProps":{"state":{"data":{"entity":{"type":"playlist","name":"My Mix","title":"My Mix","id":"pl123","uri":"spotify:playlist:pl123","subtitle":"Various","coverArt":{"sources":[{"url":"https://cov/art.jpg"}]},"trackList":[{"title":"Song One","subtitle":"Artist A, Artist B","uri":"spotify:track:aaa","duration":180000},{"title":"Song Two","subtitle":"Artist C","uri":"spotify:track:bbb","duration":200000}]}}}}}}</script>"#;
        let pl = Spotify::parse_collection_html(html, "https://open.spotify.com/playlist/pl123")
            .unwrap();
        assert_eq!(pl.title, "My Mix");
        assert_eq!(pl.artwork_url.as_deref(), Some("https://cov/art.jpg"));
        assert_eq!(pl.tracks.len(), 2);
        assert_eq!(pl.tracks[0].artists, vec!["Artist A".to_string(), "Artist B".to_string()]);
        assert_eq!(pl.tracks[0].album.as_deref(), Some("My Mix"));
        assert_eq!(
            pl.tracks[0].source_url.as_deref(),
            Some("https://open.spotify.com/track/aaa")
        );
        assert_eq!(pl.tracks[1].artists, vec!["Artist C".to_string()]);
    }

    #[test]
    fn the_track_page_supplies_the_real_album() {
        let html = r#"<html><head>
<meta property="og:description" content="Artist A &amp; B · Song · Real Album · 2019">
<meta property="og:image" content="https://i.scdn.co/image/track-art">
</head></html>"#;
        let (album, artwork) = parse_track_page(html);
        assert_eq!(album.as_deref(), Some("Real Album"));
        assert_eq!(artwork.as_deref(), Some("https://i.scdn.co/image/track-art"));
    }

    #[test]
    fn a_description_that_is_not_a_song_yields_no_album() {
        // Podcast episodes and shows use the same meta tag with another shape;
        // taking "second from the end" there would tag a book as an album.
        assert_eq!(album_from_description(Some("Podcast · Episode · Jan 2020")), None);
        assert_eq!(album_from_description(Some("Song · Album · 2019")), None);
        assert_eq!(album_from_description(None), None);
        // A single: the year sits where the album would, and is not one.
        assert_eq!(album_from_description(Some("Artist · Song · 2019")), None);
    }

    #[test]
    fn matches_spotify_urls() {
        assert!(Spotify.matches(
            &Url::parse("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3").unwrap()
        ));
        assert!(Spotify.matches(
            &Url::parse("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M").unwrap()
        ));
        assert!(!Spotify.matches(&Url::parse("https://open.spotify.com/artist/abc").unwrap()));
        assert!(!Spotify.matches(&Url::parse("https://example.com/album/abc").unwrap()));
    }
}
