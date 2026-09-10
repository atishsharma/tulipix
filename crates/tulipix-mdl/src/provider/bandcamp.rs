//! Bandcamp provider — port of mdl `providers/Bandcamp.ts`.
//!
//! Bandcamp publishes four different page shapes and this reads all four:
//!
//! * an **album** and a **track** page carry schema.org JSON-LD (`MusicAlbum` /
//!   `MusicRecording`), which is the richest and most stable source;
//! * a fan **playlist** keeps its data in a `data-blob="…"` attribute;
//! * a **Bandcamp Daily** list keeps one player payload per embed in
//!   `data-player-infos="…"`.
//!
//! Cover art is not linked directly anywhere: every payload gives an image id,
//! and the URL is built from it.

use super::{get_first_non_empty, strip_query_and_hash, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use serde::Deserialize;
use url::Url;

pub struct Bandcamp;

// ── shared shapes ────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct Named {
    name: Option<String>,
}

/// A schema.org `additionalProperty` entry — where Bandcamp hides its ids.
#[derive(Deserialize)]
struct SchemaProp {
    name: Option<String>,
    value: Option<serde_json::Value>,
}

/// `image` is a string on some pages and a list on others.
#[derive(Deserialize)]
#[serde(untagged)]
enum StringOrList {
    One(String),
    Many(Vec<String>),
}

impl StringOrList {
    fn first(&self) -> Option<&str> {
        match self {
            StringOrList::One(s) => Some(s.as_str()),
            StringOrList::Many(v) => v.first().map(String::as_str),
        }
    }
}

// ── fan playlist (`data-blob`) ───────────────────────────────────────────────

#[derive(Deserialize)]
struct BcAlbumRef {
    title: Option<String>,
}

#[derive(Deserialize)]
struct BcTrack {
    id: Option<serde_json::Value>,
    title: Option<String>,
    #[serde(rename = "artistName")]
    artist_name: Option<String>,
    #[serde(rename = "artId")]
    art_id: Option<i64>,
    /// Seconds.
    duration: Option<f64>,
    url: Option<String>,
    album: Option<BcAlbumRef>,
}

#[derive(Deserialize)]
struct BcAppData {
    #[serde(rename = "playlistId")]
    playlist_id: Option<serde_json::Value>,
    #[serde(rename = "imageId")]
    image_id: Option<i64>,
    title: Option<String>,
    curator: Option<Named>,
    tracks: Option<Vec<BcTrack>>,
}

#[derive(Deserialize)]
struct BcBlob {
    #[serde(rename = "appData")]
    app_data: Option<BcAppData>,
}

// ── album / track (JSON-LD) ──────────────────────────────────────────────────

#[derive(Deserialize)]
struct BcRelease {
    #[serde(rename = "additionalProperty")]
    additional_property: Option<Vec<SchemaProp>>,
    image: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct BcAlbumItem {
    #[serde(rename = "@id")]
    id: Option<String>,
    name: Option<String>,
    duration: Option<String>,
    #[serde(rename = "mainEntityOfPage")]
    main_entity_of_page: Option<String>,
    #[serde(rename = "additionalProperty")]
    additional_property: Option<Vec<SchemaProp>>,
}

#[derive(Deserialize)]
struct BcListElement {
    item: Option<BcAlbumItem>,
}

#[derive(Deserialize)]
struct BcTrackList {
    #[serde(rename = "itemListElement")]
    item_list_element: Option<Vec<BcListElement>>,
}

#[derive(Deserialize)]
struct BcAlbumSchema {
    name: Option<String>,
    image: Option<StringOrList>,
    #[serde(rename = "byArtist")]
    by_artist: Option<Named>,
    publisher: Option<Named>,
    #[serde(rename = "albumRelease")]
    album_release: Option<Vec<BcRelease>>,
    track: Option<BcTrackList>,
}

#[derive(Deserialize)]
struct BcInAlbum {
    name: Option<String>,
    #[serde(rename = "albumRelease")]
    album_release: Option<Vec<BcRelease>>,
}

#[derive(Deserialize)]
struct BcTrackSchema {
    #[serde(rename = "@id")]
    id: Option<String>,
    name: Option<String>,
    duration: Option<String>,
    image: Option<StringOrList>,
    #[serde(rename = "byArtist")]
    by_artist: Option<Named>,
    publisher: Option<Named>,
    #[serde(rename = "inAlbum")]
    in_album: Option<BcInAlbum>,
    #[serde(rename = "mainEntityOfPage")]
    main_entity_of_page: Option<String>,
    #[serde(rename = "additionalProperty")]
    additional_property: Option<Vec<SchemaProp>>,
}

// ── Bandcamp Daily (`data-player-infos`) ─────────────────────────────────────

#[derive(Deserialize)]
struct BcDailyTrack {
    artist: Option<String>,
    art_id: Option<i64>,
    audio_track_duration: Option<f64>,
    track_id: Option<serde_json::Value>,
    track_number: Option<i64>,
    track_title: Option<String>,
}

#[derive(Deserialize)]
struct BcDailyPlayer {
    art_id: Option<i64>,
    band_name: Option<String>,
    featured_track_number: Option<i64>,
    player_id: Option<String>,
    title: Option<String>,
    tracklist: Option<Vec<BcDailyTrack>>,
    tralbum_url: Option<String>,
}

// ── helpers ──────────────────────────────────────────────────────────────────

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

/// Bandcamp gives an image id, never a URL. `_71` is the large square.
fn image_url(image_id: Option<i64>) -> Option<String> {
    let id = image_id?;
    (id > 0).then(|| format!("https://f4.bcbits.com/img/{id:010}_71.jpg"))
}

fn schema_prop(properties: Option<&Vec<SchemaProp>>, name: &str) -> Option<String> {
    properties?
        .iter()
        .find(|p| p.name.as_deref() == Some(name))
        .and_then(|p| id_string(p.value.as_ref()))
}

fn decode_entities(value: &str) -> String {
    let mut out = value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    for (pattern, radix) in [(r"&#x([0-9a-fA-F]+);", 16u32), (r"&#(\d+);", 10)] {
        let re = Regex::new(pattern).unwrap();
        out = re
            .replace_all(&out, |c: &regex::Captures| {
                u32::from_str_radix(&c[1], radix)
                    .ok()
                    .and_then(char::from_u32)
                    .map(|c| c.to_string())
                    .unwrap_or_default()
            })
            .into_owned();
    }
    out.replace("&amp;", "&")
}

/// `PT3M45S` → milliseconds. Schema.org durations, which is what the JSON-LD
/// pages carry instead of a plain number.
fn iso_duration_ms(value: Option<&str>) -> Option<u64> {
    let re = Regex::new(r"(?i)^P(?:(\d+)D)?T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?$").unwrap();
    let c = re.captures(value?.trim())?;
    let part = |i: usize| -> f64 {
        c.get(i)
            .and_then(|m| m.as_str().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let seconds = (((part(1) * 24.0 + part(2)) * 60.0) + part(3)) * 60.0 + part(4);
    (seconds > 0.0).then(|| (seconds * 1000.0).round() as u64)
}

fn meta_property(html: &str, property: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?i)<meta\s+property="{}"\s+content="([^"]+)""#,
        regex::escape(property)
    ))
    .ok()?;
    let value = decode_entities(re.captures(html)?.get(1)?.as_str()).trim().to_string();
    (!value.is_empty()).then_some(value)
}

/// The value of an HTML attribute whose content is a JSON payload.
fn data_attribute(html: &str, attribute: &str, must_contain: Option<&str>) -> Option<String> {
    let re = Regex::new(&format!(r#"{attribute}="([^"]+)""#)).ok()?;
    re.captures_iter(html)
        .map(|c| c[1].to_string())
        .find(|raw| must_contain.map(|needle| raw.contains(needle)).unwrap_or(true))
        .map(|raw| decode_entities(&raw))
}

/// The first JSON-LD block whose `@type` matches.
fn json_ld(html: &str, kind: &str) -> Option<serde_json::Value> {
    let re = Regex::new(r#"(?s)<script type="application/ld\+json">\s*(.*?)\s*</script>"#).ok()?;
    re.captures_iter(html)
        .filter_map(|c| serde_json::from_str::<serde_json::Value>(&c[1]).ok())
        .find(|v| v.get("@type").and_then(|t| t.as_str()) == Some(kind))
}

impl Bandcamp {
    /// A fan playlist: `data-blob` → `appData`.
    pub fn parse_playlist_html(html: &str, source_url: &str) -> Result<Playlist> {
        let raw = data_attribute(html, "data-blob", Some("appData"))
            .ok_or_else(|| anyhow!("Could not find Bandcamp playlist data in the page."))?;
        let blob: BcBlob = serde_json::from_str(&raw)?;
        let app = blob
            .app_data
            .ok_or_else(|| anyhow!("Could not find Bandcamp playlist metadata in the page."))?;
        let id = id_string(app.playlist_id.as_ref())
            .ok_or_else(|| anyhow!("Could not find Bandcamp playlist metadata in the page."))?;
        let title = trimmed(app.title.as_ref())
            .ok_or_else(|| anyhow!("Could not find Bandcamp playlist metadata in the page."))?;
        let art = image_url(app.image_id);
        let tracks: Vec<Track> = app
            .tracks
            .unwrap_or_default()
            .iter()
            .filter_map(|t| {
                let title = trimmed(t.title.as_ref())?;
                let artist = trimmed(t.artist_name.as_ref())?;
                Some(Track {
                    id: id_string(t.id.as_ref()).unwrap_or_else(|| format!("{artist}-{title}")),
                    title,
                    artists: vec![artist],
                    album: t.album.as_ref().and_then(|a| trimmed(a.title.as_ref())),
                    artwork_url: get_first_non_empty(&[
                        image_url(t.art_id).as_deref(),
                        art.as_deref(),
                    ]),
                    duration_ms: t
                        .duration
                        .filter(|d| d.is_finite() && *d > 0.0)
                        .map(|d| (d * 1000.0).round() as u64),
                    source_url: trimmed(t.url.as_ref()),
                })
            })
            .collect();
        if tracks.is_empty() {
            bail!("No tracks were found in the Bandcamp playlist.");
        }
        Ok(Playlist {
            id,
            title,
            owner: app.curator.as_ref().and_then(|c| trimmed(c.name.as_ref())),
            artwork_url: art,
            provider: ProviderId::Bandcamp,
            source_url: source_url.to_string(),
            tracks,
        })
    }

    /// An album page: schema.org `MusicAlbum`.
    pub fn parse_album_html(html: &str, source_url: &str) -> Result<Playlist> {
        let album: BcAlbumSchema = serde_json::from_value(
            json_ld(html, "MusicAlbum")
                .ok_or_else(|| anyhow!("Could not find Bandcamp album data in the page."))?,
        )?;
        let release = album.album_release.as_ref().and_then(|r| r.first());
        let art = get_first_non_empty(&[
            release
                .and_then(|r| r.image.as_ref())
                .and_then(|i| i.first())
                .map(String::as_str),
            album.image.as_ref().and_then(StringOrList::first),
        ]);
        let album_id = schema_prop(release.and_then(|r| r.additional_property.as_ref()), "item_id");
        let title = trimmed(album.name.as_ref());
        let (Some(album_id), Some(title)) = (album_id, title) else {
            bail!("Could not find Bandcamp album metadata in the page.");
        };
        // The album artist, used for every track on it. Upstream leaves an
        // album track with an EMPTY artist list — the JSON-LD does not repeat
        // the artist per track — which then reaches the tagger and the filename
        // as "Unknown". On an album that is simply wrong: every track on it is
        // by the album's artist unless it says otherwise.
        let artist = album
            .by_artist
            .as_ref()
            .and_then(|a| trimmed(a.name.as_ref()))
            .or_else(|| album.publisher.as_ref().and_then(|p| trimmed(p.name.as_ref())));
        let tracks: Vec<Track> = album
            .track
            .as_ref()
            .and_then(|t| t.item_list_element.as_ref())
            .map(|list| {
                list.iter()
                    .filter_map(|element| {
                        let item = element.item.as_ref()?;
                        let name = trimmed(item.name.as_ref())?;
                        let track_id =
                            schema_prop(item.additional_property.as_ref(), "track_id")?;
                        Some(Track {
                            id: track_id,
                            title: name,
                            artists: artist.clone().into_iter().collect(),
                            album: Some(title.clone()),
                            artwork_url: art.clone(),
                            duration_ms: iso_duration_ms(item.duration.as_deref()),
                            source_url: trimmed(item.main_entity_of_page.as_ref())
                                .or_else(|| trimmed(item.id.as_ref())),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        if tracks.is_empty() {
            bail!("No tracks were found in the Bandcamp album.");
        }
        Ok(Playlist {
            id: album_id,
            title,
            owner: artist,
            artwork_url: art,
            provider: ProviderId::Bandcamp,
            source_url: source_url.to_string(),
            tracks,
        })
    }

    /// A single track page: schema.org `MusicRecording`.
    pub fn parse_track_html(html: &str, source_url: &str) -> Result<Playlist> {
        let track: BcTrackSchema = serde_json::from_value(
            json_ld(html, "MusicRecording")
                .ok_or_else(|| anyhow!("Could not find Bandcamp track data in the page."))?,
        )?;
        let release = track
            .in_album
            .as_ref()
            .and_then(|a| a.album_release.as_ref())
            .and_then(|r| r.first());
        let track_id = schema_prop(track.additional_property.as_ref(), "track_id")
            .or_else(|| schema_prop(release.and_then(|r| r.additional_property.as_ref()), "item_id"));
        let title = trimmed(track.name.as_ref());
        let artist = track
            .by_artist
            .as_ref()
            .and_then(|a| trimmed(a.name.as_ref()))
            .or_else(|| track.publisher.as_ref().and_then(|p| trimmed(p.name.as_ref())));
        let (Some(track_id), Some(title), Some(artist)) = (track_id, title, artist) else {
            bail!("Could not find Bandcamp track metadata in the page.");
        };
        let art = get_first_non_empty(&[
            track.image.as_ref().and_then(StringOrList::first),
            release
                .and_then(|r| r.image.as_ref())
                .and_then(|i| i.first())
                .map(String::as_str),
        ]);
        let normalized = Track {
            id: track_id,
            title,
            artists: vec![artist.clone()],
            album: track.in_album.as_ref().and_then(|a| trimmed(a.name.as_ref())),
            artwork_url: art,
            duration_ms: iso_duration_ms(track.duration.as_deref()),
            source_url: trimmed(track.main_entity_of_page.as_ref())
                .or_else(|| trimmed(track.id.as_ref()))
                .or_else(|| Some(source_url.to_string())),
        };
        Ok(Playlist {
            id: normalized.id.clone(),
            title: normalized.title.clone(),
            owner: Some(artist),
            artwork_url: normalized.artwork_url.clone(),
            provider: ProviderId::Bandcamp,
            source_url: normalized
                .source_url
                .clone()
                .unwrap_or_else(|| source_url.to_string()),
            tracks: vec![normalized],
        })
    }

    /// A Bandcamp Daily article: one embedded player per recommendation, each
    /// pointing at the one track of an album the writer picked.
    pub fn parse_daily_html(html: &str, source_url: &str) -> Result<Playlist> {
        let title = meta_property(html, "og:title")
            .ok_or_else(|| anyhow!("Could not find Bandcamp Daily playlist metadata in the page."))?;
        let art = meta_property(html, "og:image");
        let raw = data_attribute(html, "data-player-infos", None)
            .ok_or_else(|| anyhow!("Could not find Bandcamp Daily player data in the page."))?;
        let players: Vec<BcDailyPlayer> = serde_json::from_str(&raw)?;
        let tracks: Vec<Track> = players
            .iter()
            .filter_map(|player| {
                let featured = player
                    .tracklist
                    .as_ref()?
                    .iter()
                    .find(|t| t.track_number == player.featured_track_number)?;
                let title = trimmed(featured.track_title.as_ref())?;
                let artist = trimmed(featured.artist.as_ref())
                    .or_else(|| trimmed(player.band_name.as_ref()))?;
                Some(Track {
                    id: id_string(featured.track_id.as_ref())
                        .or_else(|| trimmed(player.player_id.as_ref()))
                        .unwrap_or_else(|| title.clone()),
                    title,
                    artists: vec![artist],
                    album: trimmed(player.title.as_ref()),
                    artwork_url: get_first_non_empty(&[
                        image_url(featured.art_id.or(player.art_id)).as_deref(),
                        art.as_deref(),
                    ]),
                    duration_ms: featured
                        .audio_track_duration
                        .filter(|d| d.is_finite() && *d > 0.0)
                        .map(|d| (d * 1000.0).round() as u64),
                    source_url: trimmed(player.tralbum_url.as_ref()),
                })
            })
            .collect();
        if tracks.is_empty() {
            bail!("No playable tracks were found in the Bandcamp Daily article.");
        }
        Ok(Playlist {
            id: source_url.to_string(),
            title,
            owner: Some("Bandcamp Daily".into()),
            artwork_url: art,
            provider: ProviderId::Bandcamp,
            source_url: source_url.to_string(),
            tracks,
        })
    }

    /// Dispatch on the URL, which is what tells the four page shapes apart.
    pub fn parse(html: &str, source_url: &str) -> Result<Playlist> {
        if source_url.contains("://daily.bandcamp.com/") {
            Self::parse_daily_html(html, source_url)
        } else if source_url.contains("/track/") {
            Self::parse_track_html(html, source_url)
        } else if source_url.contains("/album/") {
            Self::parse_album_html(html, source_url)
        } else {
            Self::parse_playlist_html(html, source_url)
        }
    }
}

#[async_trait::async_trait]
impl Provider for Bandcamp {
    fn id(&self) -> ProviderId {
        ProviderId::Bandcamp
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase).unwrap_or_default();
        let path = url.path().trim_end_matches('/');
        if host == "daily.bandcamp.com" {
            return Regex::new(r"(?i)^/lists/[^/]+$").unwrap().is_match(path);
        }
        // Every artist gets their own subdomain; bandcamp.com itself serves fan
        // pages and the odd hosted album.
        if !(host == "bandcamp.com" || host == "www.bandcamp.com" || host.ends_with(".bandcamp.com"))
        {
            return false;
        }
        [
            r"(?i)^/album/[^/]+$",
            r"(?i)^/track/[^/]+$",
            r"(?i)^/[^/]+/album/[^/]+$",
            r"(?i)^/[^/]+/playlist/[^/]+$",
        ]
        .iter()
        .any(|p| Regex::new(p).unwrap().is_match(path))
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
        Self::parse(&html, url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_album_page_reads_its_json_ld() {
        let html = r#"<html><head>
<script type="application/ld+json">
{"@type":"MusicAlbum","name":"Ghost Ship","image":"https://f4.bcbits.com/img/a123_10.jpg",
 "byArtist":{"name":"The Band"},
 "albumRelease":[{"additionalProperty":[{"name":"item_id","value":998877}],
                  "image":["https://f4.bcbits.com/img/release_10.jpg"]}],
 "track":{"itemListElement":[
   {"item":{"name":"Opening","duration":"PT3M45S","additionalProperty":[{"name":"track_id","value":1}],
            "mainEntityOfPage":"https://band.bandcamp.com/track/opening"}},
   {"item":{"name":"Closing","duration":"PT4M","additionalProperty":[{"name":"track_id","value":2}]}}
 ]}}
</script></head></html>"#;
        let pl = Bandcamp::parse_album_html(html, "https://band.bandcamp.com/album/ghost-ship").unwrap();
        assert_eq!(pl.id, "998877");
        assert_eq!(pl.title, "Ghost Ship");
        assert_eq!(pl.owner.as_deref(), Some("The Band"));
        assert_eq!(pl.tracks.len(), 2);
        // Every track on an album is by the album's artist — upstream leaves
        // this empty and the tagger writes "Unknown".
        assert_eq!(pl.tracks[0].artists, vec!["The Band".to_string()]);
        assert_eq!(pl.tracks[0].album.as_deref(), Some("Ghost Ship"));
        assert_eq!(pl.tracks[0].duration_ms, Some(225_000));
        assert_eq!(pl.tracks[1].duration_ms, Some(240_000));
        assert_eq!(
            pl.artwork_url.as_deref(),
            Some("https://f4.bcbits.com/img/release_10.jpg")
        );
    }

    #[test]
    fn a_track_page_reads_its_json_ld() {
        let html = r#"<script type="application/ld+json">
{"@type":"MusicRecording","name":"Opening","duration":"PT3M45S",
 "byArtist":{"name":"The Band"},"image":["https://f4.bcbits.com/img/t.jpg"],
 "additionalProperty":[{"name":"track_id","value":42}],
 "inAlbum":{"name":"Ghost Ship"},
 "mainEntityOfPage":"https://band.bandcamp.com/track/opening"}
</script>"#;
        let pl = Bandcamp::parse_track_html(html, "https://band.bandcamp.com/track/opening").unwrap();
        assert_eq!(pl.tracks.len(), 1);
        assert_eq!(pl.tracks[0].id, "42");
        assert_eq!(pl.tracks[0].album.as_deref(), Some("Ghost Ship"));
        assert_eq!(pl.tracks[0].artists, vec!["The Band".to_string()]);
    }

    #[test]
    fn a_fan_playlist_reads_its_data_blob() {
        let html = r#"<div data-blob="{&quot;appData&quot;:{&quot;playlistId&quot;:77,&quot;title&quot;:&quot;Mixtape&quot;,&quot;imageId&quot;:12345,&quot;curator&quot;:{&quot;name&quot;:&quot;A Fan&quot;},&quot;tracks&quot;:[{&quot;id&quot;:5,&quot;title&quot;:&quot;One&quot;,&quot;artistName&quot;:&quot;Someone&quot;,&quot;duration&quot;:180,&quot;url&quot;:&quot;https://x.bandcamp.com/track/one&quot;}]}}"></div>"#;
        let pl = Bandcamp::parse_playlist_html(html, "https://bandcamp.com/user/playlist/mix").unwrap();
        assert_eq!(pl.id, "77");
        assert_eq!(pl.title, "Mixtape");
        assert_eq!(pl.owner.as_deref(), Some("A Fan"));
        assert_eq!(pl.tracks[0].duration_ms, Some(180_000));
        // Image ids are zero-padded to ten digits, never linked directly.
        assert_eq!(
            pl.artwork_url.as_deref(),
            Some("https://f4.bcbits.com/img/0000012345_71.jpg")
        );
    }

    #[test]
    fn a_daily_list_takes_the_featured_track_of_each_player() {
        let html = r#"<html><head>
<meta property="og:title" content="The Best Of March">
<meta property="og:image" content="https://daily.bandcamp.com/hero.jpg">
</head><body><div data-player-infos="[{&quot;band_name&quot;:&quot;A Band&quot;,&quot;title&quot;:&quot;An Album&quot;,&quot;featured_track_number&quot;:2,&quot;art_id&quot;:99,&quot;tralbum_url&quot;:&quot;https://a.bandcamp.com/album/an-album&quot;,&quot;tracklist&quot;:[{&quot;track_number&quot;:1,&quot;track_title&quot;:&quot;Skip&quot;},{&quot;track_number&quot;:2,&quot;track_title&quot;:&quot;Pick&quot;,&quot;track_id&quot;:7,&quot;audio_track_duration&quot;:200}]}]"></div></body></html>"#;
        let pl = Bandcamp::parse_daily_html(html, "https://daily.bandcamp.com/lists/best-of-march").unwrap();
        assert_eq!(pl.title, "The Best Of March");
        assert_eq!(pl.owner.as_deref(), Some("Bandcamp Daily"));
        assert_eq!(pl.tracks.len(), 1);
        // The FEATURED track, not the first one.
        assert_eq!(pl.tracks[0].title, "Pick");
        assert_eq!(pl.tracks[0].artists, vec!["A Band".to_string()]);
        assert_eq!(pl.tracks[0].duration_ms, Some(200_000));
    }

    #[test]
    fn iso_durations_become_milliseconds() {
        assert_eq!(iso_duration_ms(Some("PT3M45S")), Some(225_000));
        assert_eq!(iso_duration_ms(Some("PT1H2M3S")), Some(3_723_000));
        assert_eq!(iso_duration_ms(Some("PT0S")), None);
        assert_eq!(iso_duration_ms(Some("nonsense")), None);
        assert_eq!(iso_duration_ms(None), None);
    }

    #[test]
    fn matches_artist_subdomains_and_daily_lists() {
        let m = |u: &str| Bandcamp.matches(&Url::parse(u).unwrap());
        assert!(m("https://band.bandcamp.com/album/ghost-ship"));
        assert!(m("https://band.bandcamp.com/track/opening"));
        assert!(m("https://bandcamp.com/someone/playlist/mix"));
        assert!(m("https://daily.bandcamp.com/lists/best-of-march"));
        assert!(!m("https://daily.bandcamp.com/features/interview"));
        assert!(!m("https://band.bandcamp.com/music"));
        assert!(!m("https://example.com/album/x"));
    }
}

// ── search ──────────────────────────────────────────────────────────────────
//
// Bandcamp's own search box calls `api/fuzzysearch/1/autocomplete_elastic`, a
// POST that answers JSON without a key. It is undocumented, which is the
// honest caveat on this one: it is not promised to anybody and could change.
// The failure mode is a clear error rather than wrong data — the shape either
// deserializes or it does not — and the whole catalogue is otherwise reachable
// only by already knowing the URL, which is the least useful kind of
// reachable for the label whose whole point is finding people you have not
// heard of.

#[derive(Deserialize)]
struct BcSearchResponse {
    auto: Option<BcAuto>,
}

#[derive(Deserialize)]
struct BcAuto {
    results: Option<Vec<BcHit>>,
}

#[derive(Deserialize)]
struct BcHit {
    /// `t` track · `a` album · `b` band. Only tracks become rows.
    #[serde(rename = "type")]
    kind: Option<String>,
    id: Option<i64>,
    name: Option<String>,
    band_name: Option<String>,
    album_name: Option<String>,
    img: Option<String>,
    item_url_path: Option<String>,
}

fn bc_clean(v: Option<&String>) -> Option<String> {
    v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn bc_hit_to_track(h: &BcHit) -> Option<Track> {
    // Albums and bands come back in the same list. An album is not a row here:
    // the queue is tracks, and an album hit would have to be fetched before it
    // became any. Pasting its URL is the path that already does that.
    if h.kind.as_deref() != Some("t") {
        return None;
    }
    let title = bc_clean(h.name.as_ref())?;
    let artist = bc_clean(h.band_name.as_ref())?;
    Some(Track {
        id: h
            .id
            .map(|i| i.to_string())
            .unwrap_or_else(|| format!("{artist}-{title}")),
        title,
        artists: vec![artist],
        album: bc_clean(h.album_name.as_ref()),
        artwork_url: bc_clean(h.img.as_ref()),
        // Autocomplete carries no length. The downloader's duration check reads
        // 0 as "the provider did not say" and falls back to the first hit, so
        // a Bandcamp search gets the old behaviour rather than a wrong one.
        duration_ms: None,
        source_url: bc_clean(h.item_url_path.as_ref()),
    })
}

/// Search Bandcamp by name.
pub async fn search(client: &reqwest::Client, query: &str, limit: usize) -> Result<Playlist> {
    let q = query.trim();
    if q.is_empty() {
        bail!("Type something to search.");
    }
    let body = serde_json::json!({
        "search_text": q,
        "search_filter": "t",
        "full_page": false,
        "fan_id": serde_json::Value::Null,
    });
    let resp: BcSearchResponse = client
        .post("https://bandcamp.com/api/fuzzysearch/1/autocomplete_elastic")
        .header("user-agent", "Mozilla/5.0")
        .json(&body)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;

    let tracks: Vec<Track> = resp
        .auto
        .and_then(|a| a.results)
        .unwrap_or_default()
        .iter()
        .filter_map(bc_hit_to_track)
        .take(limit.max(1))
        .collect();
    if tracks.is_empty() {
        bail!("Bandcamp found no tracks for “{q}”.");
    }
    Ok(Playlist {
        id: format!("search:{q}"),
        title: format!("Search: {q}"),
        owner: None,
        artwork_url: tracks.iter().find_map(|t| t.artwork_url.clone()),
        provider: ProviderId::Bandcamp,
        source_url: format!("https://bandcamp.com/search?q={q}"),
        tracks,
    })
}

#[cfg(test)]
mod search_tests {
    use super::*;

    const PAYLOAD: &str = r#"{"auto":{"results":[
      {"type":"t","id":11,"name":"Opening","band_name":"Ghost Ship",
       "album_name":"Harbour","img":"https://f4.bcbits.com/img/a1_10.jpg",
       "item_url_path":"https://band.bandcamp.com/track/opening"},
      {"type":"a","id":12,"name":"Harbour","band_name":"Ghost Ship"},
      {"type":"b","id":13,"name":"Ghost Ship"},
      {"type":"t","id":14,"band_name":"No Name Here"}
    ]}}"#;

    #[test]
    fn only_tracks_become_rows() {
        let resp: BcSearchResponse = serde_json::from_str(PAYLOAD).unwrap();
        let tracks: Vec<Track> = resp
            .auto
            .and_then(|a| a.results)
            .unwrap()
            .iter()
            .filter_map(bc_hit_to_track)
            .collect();
        assert_eq!(tracks.len(), 1, "the album, the band and the untitled track are not rows");
        assert_eq!(tracks[0].title, "Opening");
        assert_eq!(tracks[0].album.as_deref(), Some("Harbour"));
        assert_eq!(
            tracks[0].source_url.as_deref(),
            Some("https://band.bandcamp.com/track/opening")
        );
    }

    #[test]
    fn no_length_is_no_length_rather_than_zero() {
        let resp: BcSearchResponse = serde_json::from_str(PAYLOAD).unwrap();
        let t = resp
            .auto
            .and_then(|a| a.results)
            .unwrap()
            .iter()
            .find_map(bc_hit_to_track)
            .unwrap();
        // The downloader reads None as "unknown" and keeps its old behaviour.
        // Zero would look like a target and refuse every candidate.
        assert_eq!(t.duration_ms, None);
    }
}
