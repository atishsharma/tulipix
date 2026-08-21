//! The VidSrc-family hosts — the fallback lane, and the only one that carries
//! films and non-anime series.
//!
//! All three of these services work the same way and are documented the same
//! way: you build an embed URL from a TMDB id (plus season and episode for a
//! series), fetch it, and find the HLS playlist the player was going to load.
//! So they are one implementation with a different URL template each, not three
//! providers — [`hosts`] is the whole difference between them.
//!
//! The unwrap is deliberately generic: fetch, look for a playlist, and if the
//! page only contains another player frame, follow that once and look again.
//! Nothing here encodes a particular page's markup, because that markup changes
//! monthly and a structural rule survives it.

use anyhow::{Context, Result};
use async_trait::async_trait;

use super::source::{Playable, PlayableKind, Source};
use super::{Episode, EpisodeRef, Title};

/// One host's addresses. Kept as data so the Servers modal can add or replace
/// one without a code change, the same way the Stream tab's host pool works.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: &'static str,
    pub label: &'static str,
    pub origin: &'static str,
    /// `{id}` is the TMDB id.
    pub movie: &'static str,
    /// `{id}`, `{season}`, `{episode}`.
    pub series: &'static str,
}

pub fn hosts() -> Vec<Host> {
    vec![
        Host {
            id: "vidsrc",
            label: "VidSrc",
            origin: "https://vidsrc.to",
            movie: "https://vidsrc.to/embed/movie/{id}",
            series: "https://vidsrc.to/embed/tv/{id}/{season}/{episode}",
        },
        Host {
            id: "videasy",
            label: "videasy",
            origin: "https://player.videasy.to",
            movie: "https://player.videasy.to/movie/{id}",
            series: "https://player.videasy.to/tv/{id}/{season}/{episode}",
        },
        Host {
            id: "vidking",
            label: "vidking",
            origin: "https://www.vidking.net",
            movie: "https://www.vidking.net/embed/movie/{id}",
            series: "https://www.vidking.net/embed/tv/{id}/{season}/{episode}",
        },
    ]
}

/// How many frames deep to follow before giving up. One is enough for every
/// layout these services have used; more just turns a dead host into a slow one.
const MAX_HOPS: usize = 2;

pub struct VidSrc {
    host: Host,
    http: reqwest::Client,
}

impl VidSrc {
    pub fn new(host: Host) -> Self {
        Self { host, http: tulipix_core::net::http().clone() }
    }

    fn embed_url(&self, ep: &EpisodeRef) -> Option<String> {
        let id = ep.title.tmdb_id?;
        Some(if ep.title.is_series() {
            self.host
                .series
                .replace("{id}", &id.to_string())
                .replace("{season}", &ep.season.to_string())
                .replace("{episode}", &ep.episode.to_string())
        } else {
            self.host.movie.replace("{id}", &id.to_string())
        })
    }

    async fn fetch(&self, url: &str, referer: &str) -> Result<String> {
        let resp = self
            .http
            .get(url)
            .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
            .header(reqwest::header::REFERER, referer)
            .send()
            .await
            .with_context(|| format!("{}: request failed", self.host.id))?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("{} {}", status.as_u16(), reason(status));
        }
        resp.text().await.context("body was not text")
    }
}

fn reason(s: reqwest::StatusCode) -> &'static str {
    match s.as_u16() {
        403 => "forbidden",
        404 => "not found",
        429 => "rate limited",
        500..=599 => "server error",
        _ => "unexpected status",
    }
}

#[async_trait]
impl Source for VidSrc {
    fn id(&self) -> &'static str {
        self.host.id
    }

    fn label(&self) -> &'static str {
        self.host.label
    }

    async fn episodes(&self, title: &Title, _audio: &str) -> Result<Vec<Episode>> {
        // These hosts have no episode index of their own — you ask for an
        // episode by TMDB numbering and either get it or do not. The count comes
        // from the metadata provider that produced the title.
        let Some(n) = title.episodes.filter(|n| *n > 0) else {
            return Ok(Vec::new());
        };
        if title.tmdb_id.is_none() {
            return Ok(Vec::new());
        }
        Ok((1..=n)
            .map(|number| Episode { number, season: 1, ..Episode::default() })
            .collect())
    }

    async fn resolve(&self, ep: &EpisodeRef) -> Result<Vec<Playable>> {
        let Some(start) = self.embed_url(ep) else {
            // No TMDB id — this is an anime-lane title these hosts cannot index.
            return Ok(Vec::new());
        };
        let mut url = start;
        let mut referer = format!("{}/", self.host.origin);
        for _ in 0..MAX_HOPS {
            let body = self.fetch(&url, &referer).await?;
            let found = playlists(&body);
            if !found.is_empty() {
                return Ok(found
                    .into_iter()
                    .map(|(u, height)| {
                        let mut p = Playable::new(u, PlayableKind::M3u8, self.host.id);
                        p.label = if height > 0 { format!("{height}p") } else { "auto".into() };
                        p.height = height;
                        p.uploader = self.host.label.to_string();
                        p.headers = vec![("Referer".into(), format!("{}/", self.host.origin))];
                        p
                    })
                    .collect());
            }
            match next_frame(&body) {
                Some(next) => {
                    referer = url;
                    url = absolutise(&next, self.host.origin);
                }
                None => break,
            }
        }
        Ok(Vec::new())
    }

    async fn ping(&self) -> Result<u32> {
        let started = std::time::Instant::now();
        self.fetch(self.host.origin, self.host.origin).await?;
        Ok(started.elapsed().as_millis().min(u128::from(u32::MAX)) as u32)
    }
}

/// Every `.m3u8` in the page, with a resolution when the URL admits one.
///
/// Scanning for the extension rather than for a particular attribute is what
/// makes this survive a redesign: however the player is configured, the playlist
/// URL is still a URL ending in `.m3u8`.
fn playlists(body: &str) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = Vec::new();
    for (idx, _) in body.match_indices(".m3u8") {
        let Some(url) = url_around(body, idx) else { continue };
        if out.iter().any(|(u, _)| *u == url) {
            continue;
        }
        let h = [2160, 1440, 1080, 720, 480, 360]
            .into_iter()
            .find(|h| url.contains(&format!("{h}p")) || url.contains(&format!("/{h}/")))
            .unwrap_or(0);
        out.push((url, h));
    }
    out.sort_by(|a, b| b.1.cmp(&a.1));
    out
}

/// Walk out from a hit to the URL that contains it, stopping at whatever quoted
/// or bracketed it.
fn url_around(body: &str, hit: usize) -> Option<String> {
    let bytes = body.as_bytes();
    let stop = |b: u8| matches!(b, b'"' | b'\'' | b'`' | b'<' | b'>' | b' ' | b'\\' | b'\n' | b'\r');
    let mut start = hit;
    while start > 0 && !stop(bytes[start - 1]) {
        start -= 1;
    }
    let mut end = hit + ".m3u8".len();
    while end < bytes.len() && !stop(bytes[end]) {
        end += 1;
    }
    let raw = body.get(start..end)?.trim();
    let raw = raw.strip_prefix("//").map(|r| format!("https://{r}")).unwrap_or_else(|| raw.to_string());
    raw.starts_with("http").then_some(raw)
}

/// The next player frame to follow, if the page is only a wrapper.
fn next_frame(body: &str) -> Option<String> {
    let lower = body.to_ascii_lowercase();
    let at = lower.find("<iframe")?;
    let rest = &body[at..];
    let src_at = rest.to_ascii_lowercase().find("src=")?;
    let after = &rest[src_at + 4..];
    let quote = after.chars().next()?;
    let (open, close) = if quote == '"' || quote == '\'' { (1, quote) } else { (0, ' ') };
    let tail = &after[open..];
    let end = tail.find(close).unwrap_or(tail.len());
    let src = tail.get(..end)?.trim();
    (!src.is_empty()).then(|| src.to_string())
}

fn absolutise(src: &str, origin: &str) -> String {
    if src.starts_with("http") {
        src.to_string()
    } else if let Some(rest) = src.strip_prefix("//") {
        format!("https://{rest}")
    } else if src.starts_with('/') {
        format!("{origin}{src}")
    } else {
        format!("{origin}/{src}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_a_quoted_playlist_and_its_height() {
        let body = r#"<script>var f = "https://cdn.test/hls/1080p/master.m3u8";</script>"#;
        let got = playlists(body);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0, "https://cdn.test/hls/1080p/master.m3u8");
        assert_eq!(got[0].1, 1080);
    }

    #[test]
    fn best_quality_comes_first() {
        let body = r#"'https://c.test/720p/i.m3u8' and 'https://c.test/1080p/i.m3u8'"#;
        let got = playlists(body);
        assert_eq!(got[0].1, 1080);
        assert_eq!(got[1].1, 720);
    }

    #[test]
    fn protocol_relative_urls_are_upgraded_and_junk_is_dropped() {
        assert_eq!(
            playlists(r#"src="//cdn.test/a.m3u8""#)[0].0,
            "https://cdn.test/a.m3u8"
        );
        assert!(playlists(r#"file: "relative/path.m3u8""#).is_empty());
    }

    #[test]
    fn follows_one_frame() {
        let body = r#"<html><iframe src="/player/abc" allowfullscreen></iframe></html>"#;
        assert_eq!(next_frame(body).as_deref(), Some("/player/abc"));
        assert_eq!(absolutise("/player/abc", "https://h.test"), "https://h.test/player/abc");
    }

    #[test]
    fn a_page_with_no_frame_and_no_playlist_stops() {
        assert!(next_frame("<html>nothing here</html>").is_none());
        assert!(playlists("<html>nothing here</html>").is_empty());
    }

    #[test]
    fn embed_urls_use_the_right_template_per_shape() {
        let h = hosts().into_iter().find(|h| h.id == "vidsrc").unwrap();
        let src = VidSrc::new(h);
        let mut ep = EpisodeRef {
            title: Title { tmdb_id: Some(42), format: "TV".into(), ..Title::default() },
            season: 2,
            episode: 7,
            audio: "sub".into(),
        };
        assert_eq!(src.embed_url(&ep).unwrap(), "https://vidsrc.to/embed/tv/42/2/7");
        ep.title.format = "MOVIE".into();
        assert_eq!(src.embed_url(&ep).unwrap(), "https://vidsrc.to/embed/movie/42");
        ep.title.tmdb_id = None;
        assert!(src.embed_url(&ep).is_none());
    }
}
