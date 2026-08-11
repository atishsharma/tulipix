//! Amazon Music provider — port of mdl `providers/AmazonMusic.ts`.
//!
//! Amazon Music is a single-page app: the HTML a browser gets holds no track
//! list at all. What it does hold — for crawlers — is a full set of Open Graph
//! meta tags, and a different set for every `?do=play&trackAsin=…` variant of
//! the same URL. So the shape here is: read the collection page for its track
//! ASINs, then read one page per ASIN for that track's tags.
//!
//! The requests go out under a crawler's user agent because that is who those
//! tags are served to; with a browser UA the same URLs return the empty app
//! shell and the whole thing yields nothing.

use super::{get_first_non_empty, Provider};
use crate::types::{Playlist, ProviderId, Track};
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use std::collections::HashMap;
use url::Url;

pub struct AmazonMusic;

/// The tags this provider reads are the ones Amazon serves to link crawlers.
const BOT_UA: &str =
    "facebookexternalhit/1.1 (+http://www.facebook.com/externalhit_uatext.php)";

/// Track pages read at once. Four is upstream's number and Amazon is stricter
/// than most about a burst.
const CONCURRENCY: usize = 4;

/// What a collection page yields.
#[derive(Debug, Default, PartialEq)]
pub struct CollectionPage {
    pub title: Option<String>,
    pub owner: Option<String>,
    pub artwork_url: Option<String>,
    pub track_asins: Vec<String>,
}

/// What one track page yields. The artist and album arrive as URLs, not names —
/// resolving them means another page each, which is why they are deduplicated
/// before being fetched.
#[derive(Debug, Default, PartialEq, Clone)]
pub struct TrackPage {
    pub title: Option<String>,
    pub artist_url: Option<String>,
    pub album_url: Option<String>,
    pub artwork_url: Option<String>,
    pub duration_ms: Option<u64>,
    pub source_url: Option<String>,
}

/// The entities a meta tag can carry. Amazon escapes its attribute values, and
/// `&#x3D;` in a track link is the difference between finding ten ASINs and
/// finding none.
fn decode_attribute(value: &str) -> String {
    let mut out = value
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    for (re, radix) in [(r"&#x([0-9a-fA-F]+);", 16u32), (r"&#(\d+);", 10)] {
        let re = Regex::new(re).unwrap();
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
    // `&amp;` last: decoding it first would turn `&amp;#39;` into an entity
    // that the passes above then decode a second time.
    out.replace("&amp;", "&")
}

fn meta_property(html: &str, property: &str) -> Option<String> {
    let re = Regex::new(&format!(
        r#"(?i)<meta\s+property="{}"\s+content="([^"]*)""#,
        regex::escape(property)
    ))
    .ok()?;
    let raw = re.captures(html)?.get(1)?.as_str();
    let value = decode_attribute(raw).trim().to_string();
    (!value.is_empty()).then_some(value)
}

impl AmazonMusic {
    pub fn parse_collection_html(html: &str) -> Result<CollectionPage> {
        let title = meta_property(html, "og:title");
        let description = meta_property(html, "og:description");
        let owner = description.as_deref().and_then(|d| {
            Regex::new(r"(?i)^Playlist by (.+)$")
                .unwrap()
                .captures(d)
                .and_then(|c| c.get(1))
                .map(|m| m.as_str().trim().to_string())
        });
        // ASINs appear in the page's own play links, in whichever escaping the
        // renderer happened to use.
        let re = Regex::new(r"trackAsin(?:&#x3D;|=|&amp;trackAsin=)([A-Z0-9]{10})").unwrap();
        let mut track_asins: Vec<String> = Vec::new();
        for c in re.captures_iter(html) {
            let asin = c[1].to_string();
            if !track_asins.contains(&asin) {
                track_asins.push(asin);
            }
        }
        if title.is_none() || track_asins.is_empty() {
            bail!("Could not find Amazon Music collection metadata in the page.");
        }
        Ok(CollectionPage {
            artwork_url: meta_property(html, "og:image"),
            owner,
            title,
            track_asins,
        })
    }

    pub fn parse_track_html(html: &str) -> TrackPage {
        TrackPage {
            title: meta_property(html, "og:title"),
            artist_url: meta_property(html, "music:musician"),
            album_url: meta_property(html, "music:album"),
            artwork_url: meta_property(html, "og:image"),
            // Seconds in the tag.
            duration_ms: meta_property(html, "music:duration")
                .and_then(|d| d.parse::<f64>().ok())
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(|d| (d * 1000.0).round() as u64),
            source_url: meta_property(html, "al:web:url"),
        }
    }

    /// An artist or album page's own name. Its `og:title` is `Name – Amazon
    /// Music`, so the part before the dash is the answer.
    fn name_from_page(html: &str) -> Option<String> {
        meta_property(html, "og:title")
            .and_then(|t| t.split(" – ").next().map(|s| s.trim().to_string()))
            .filter(|s| !s.is_empty())
    }

    fn collection_id(url: &str) -> Result<String> {
        let path = Url::parse(url)?.path().to_string();
        Regex::new(r"(?i)/(?:albums|user-playlists|playlists)/([A-Za-z0-9]+)")
            .unwrap()
            .captures(&path)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_string())
            .ok_or_else(|| {
                anyhow!("Could not determine the Amazon Music collection id from the URL.")
            })
    }

    fn track_asin(url: &str) -> Option<String> {
        Url::parse(url)
            .ok()?
            .query_pairs()
            .find(|(k, _)| k == "trackAsin")
            .map(|(_, v)| v.trim().to_string())
            .filter(|s| !s.is_empty())
    }

    /// `<collection>?do=play&trackAsin=<asin>` — the URL whose meta tags
    /// describe one track rather than the collection.
    fn playback_url(source_url: &str, asin: &str) -> Result<String> {
        let mut url = Url::parse(source_url)?;
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("do", "play")
            .append_pair("trackAsin", asin);
        Ok(url.to_string())
    }

    async fn html(client: &reqwest::Client, url: &str) -> Result<String> {
        Ok(client
            .get(url)
            .header("user-agent", BOT_UA)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?)
    }

    /// Read `urls` concurrently, in submission order, dropping the ones that
    /// fail — one unreachable track page must not sink a 50-track playlist.
    async fn fetch_many(client: &reqwest::Client, urls: &[String]) -> Vec<Option<String>> {
        let mut out: Vec<Option<String>> = vec![None; urls.len()];
        let mut start = 0usize;
        while start < urls.len() {
            let end = (start + CONCURRENCY).min(urls.len());
            let mut set = tokio::task::JoinSet::new();
            for (offset, url) in urls[start..end].iter().enumerate() {
                let client = client.clone();
                let url = url.clone();
                let at = start + offset;
                set.spawn(async move { (at, Self::html(&client, &url).await.ok()) });
            }
            while let Some(Ok((at, html))) = set.join_next().await {
                out[at] = html;
            }
            start = end;
        }
        out
    }

    /// Resolve each distinct artist/album URL to its name, once.
    async fn resolve_names(
        client: &reqwest::Client,
        pages: &[TrackPage],
    ) -> HashMap<String, String> {
        let mut wanted: Vec<String> = Vec::new();
        for page in pages {
            for url in [page.artist_url.as_ref(), page.album_url.as_ref()]
                .into_iter()
                .flatten()
            {
                if !wanted.contains(url) {
                    wanted.push(url.clone());
                }
            }
        }
        let fetched = Self::fetch_many(client, &wanted).await;
        wanted
            .into_iter()
            .zip(fetched)
            .filter_map(|(url, html)| Some((url, Self::name_from_page(&html?)?)))
            .collect()
    }

    fn track_of(
        asin: &str,
        page: &TrackPage,
        names: &HashMap<String, String>,
        collection_art: Option<&str>,
    ) -> Option<Track> {
        let title = page.title.clone()?;
        let artist = page.artist_url.as_ref().and_then(|u| names.get(u))?.clone();
        Some(Track {
            id: asin.to_string(),
            title,
            artists: vec![artist],
            album: page.album_url.as_ref().and_then(|u| names.get(u)).cloned(),
            artwork_url: get_first_non_empty(&[page.artwork_url.as_deref(), collection_art]),
            duration_ms: page.duration_ms,
            source_url: page.source_url.clone(),
        })
    }
}

#[async_trait::async_trait]
impl Provider for AmazonMusic {
    fn id(&self) -> ProviderId {
        ProviderId::AmazonMusic
    }
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &["amzn.to"]
    }
    fn matches(&self, url: &Url) -> bool {
        let host = url.host_str().map(str::to_lowercase).unwrap_or_default();
        // Every storefront: music.amazon.com, .co.uk, .co.jp, .com.br…
        if !Regex::new(r"(?i)^music\.amazon\.[a-z.]+$").unwrap().is_match(&host) {
            return false;
        }
        let path = url.path().trim_end_matches('/');
        [
            r"(?i)^/albums/[A-Za-z0-9]+$",
            r"(?i)^/user-playlists/[A-Za-z0-9]+$",
            r"(?i)^/playlists/[A-Za-z0-9]+$",
        ]
        .iter()
        .any(|p| Regex::new(p).unwrap().is_match(path))
    }
    /// The query is NOT stripped here: `?trackAsin=…` is what distinguishes a
    /// single track from the collection it lives in.
    fn normalize(&self, url: &Url) -> String {
        let mut url = url.clone();
        url.set_fragment(None);
        url.to_string()
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist> {
        // One track out of a collection.
        if let Some(asin) = Self::track_asin(url) {
            let html = Self::html(client, &Self::playback_url(url, &asin)?).await?;
            let page = Self::parse_track_html(&html);
            let names = Self::resolve_names(client, std::slice::from_ref(&page)).await;
            let track = Self::track_of(&asin, &page, &names, None)
                .ok_or_else(|| anyhow!("Could not find Amazon Music track metadata in the page."))?;
            return Ok(Playlist {
                id: track.id.clone(),
                title: track.title.clone(),
                owner: track.artists.first().cloned(),
                artwork_url: track.artwork_url.clone(),
                provider: ProviderId::AmazonMusic,
                source_url: track.source_url.clone().unwrap_or_else(|| url.to_string()),
                tracks: vec![track],
            });
        }

        let kind = if Url::parse(url)?.path().contains("/albums/") {
            "album"
        } else {
            "playlist"
        };
        let collection = Self::parse_collection_html(&Self::html(client, url).await?)?;

        let urls: Vec<String> = collection
            .track_asins
            .iter()
            .map(|asin| Self::playback_url(url, asin))
            .collect::<Result<_>>()?;
        let pages: Vec<TrackPage> = Self::fetch_many(client, &urls)
            .await
            .into_iter()
            .map(|html| html.as_deref().map(Self::parse_track_html).unwrap_or_default())
            .collect();
        let names = Self::resolve_names(client, &pages).await;

        let tracks: Vec<Track> = collection
            .track_asins
            .iter()
            .zip(&pages)
            .filter_map(|(asin, page)| {
                Self::track_of(asin, page, &names, collection.artwork_url.as_deref())
            })
            .collect();
        if tracks.is_empty() {
            bail!("No tracks were found in the Amazon Music {kind}.");
        }

        Ok(Playlist {
            id: Self::collection_id(url)?,
            title: collection.title.clone().unwrap_or_else(|| {
                format!(
                    "Amazon Music {}",
                    if kind == "album" { "Album" } else { "Playlist" }
                )
            }),
            owner: collection.owner.clone(),
            artwork_url: collection.artwork_url.clone(),
            provider: ProviderId::AmazonMusic,
            source_url: url.to_string(),
            tracks,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_collection_page_yields_its_track_asins() {
        let html = r#"<html><head>
<meta property="og:title" content="Road Trip Mix">
<meta property="og:description" content="Playlist by Amazon Music">
<meta property="og:image" content="https://m.media-amazon.com/images/cover.jpg">
</head><body>
<a href="/user-playlists/abc?do&#x3D;play&amp;trackAsin&#x3D;B01N4M2HFA">One</a>
<a href="/user-playlists/abc?do=play&trackAsin=B07QLXYZ12">Two</a>
<a href="/user-playlists/abc?do=play&trackAsin=B07QLXYZ12">Two again</a>
</body></html>"#;
        let page = AmazonMusic::parse_collection_html(html).unwrap();
        assert_eq!(page.title.as_deref(), Some("Road Trip Mix"));
        assert_eq!(page.owner.as_deref(), Some("Amazon Music"));
        // Escaped and plain forms both count, and a repeat is not a second track.
        assert_eq!(page.track_asins, ["B01N4M2HFA", "B07QLXYZ12"]);
    }

    #[test]
    fn a_page_without_tracks_is_an_error_not_an_empty_playlist() {
        let html = r#"<meta property="og:title" content="Some Page">"#;
        assert!(AmazonMusic::parse_collection_html(html).is_err());
    }

    #[test]
    fn a_track_page_yields_its_tags() {
        let html = r#"<html><head>
<meta property="og:title" content="Midnight City">
<meta property="music:musician" content="https://music.amazon.com/artists/B001&amp;x=1">
<meta property="music:album" content="https://music.amazon.com/albums/B002">
<meta property="music:duration" content="240">
<meta property="og:image" content="https://m.media-amazon.com/images/t.jpg">
<meta property="al:web:url" content="https://music.amazon.com/albums/B002?trackAsin=B003">
</head></html>"#;
        let page = AmazonMusic::parse_track_html(html);
        assert_eq!(page.title.as_deref(), Some("Midnight City"));
        assert_eq!(page.duration_ms, Some(240_000));
        assert_eq!(
            page.artist_url.as_deref(),
            Some("https://music.amazon.com/artists/B001&x=1")
        );
    }

    #[test]
    fn an_artist_page_name_stops_at_the_dash() {
        let html = r#"<meta property="og:title" content="M83 – Amazon Music">"#;
        assert_eq!(AmazonMusic::name_from_page(html).as_deref(), Some("M83"));
    }

    #[test]
    fn attribute_entities_are_decoded_once() {
        assert_eq!(decode_attribute("a&amp;b"), "a&b");
        assert_eq!(decode_attribute("do&#x3D;play"), "do=play");
        assert_eq!(decode_attribute("Rock &#38; Roll"), "Rock & Roll");
        // The literal text `&amp;#39;` is an ampersand followed by `#39;`, not
        // an apostrophe — decoding `&amp;` first would produce one.
        assert_eq!(decode_attribute("&amp;#39;"), "&#39;");
    }

    #[test]
    fn matches_every_storefront_but_only_collection_paths() {
        let m = |u: &str| AmazonMusic.matches(&Url::parse(u).unwrap());
        assert!(m("https://music.amazon.com/albums/B01N4M2HFA"));
        assert!(m("https://music.amazon.co.uk/user-playlists/abc123"));
        assert!(m("https://music.amazon.com/playlists/abc123"));
        assert!(m("https://music.amazon.com/albums/B01N4M2HFA?trackAsin=B002"));
        assert!(!m("https://music.amazon.com/artists/B001"));
        assert!(!m("https://example.com/albums/B01N4M2HFA"));
    }

    #[test]
    fn the_playback_url_replaces_the_query_rather_than_appending_to_it() {
        let url = AmazonMusic::playback_url(
            "https://music.amazon.com/albums/B01?trackAsin=OLD&ref=x",
            "B02",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://music.amazon.com/albums/B01?do=play&trackAsin=B02"
        );
    }
}
