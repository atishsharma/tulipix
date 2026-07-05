//! URL detection + short-link resolution + dispatch — port of mdl `Providers.ts`.

use crate::provider::registry;
use crate::types::{Playlist, ProviderId};
use anyhow::{anyhow, Result};
use url::Url;

fn parse_http(url: &str) -> Option<Url> {
    let u = Url::parse(url.trim()).ok()?;
    matches!(u.scheme(), "http" | "https").then_some(u)
}

/// Detect the provider for a URL (direct match, then short-link host).
pub fn detect_provider(url: &str) -> Option<ProviderId> {
    let u = parse_http(url)?;
    let reg = registry();
    if let Some(p) = reg.iter().find(|p| p.matches(&u)) {
        return Some(p.id());
    }
    let host = u.host_str()?.to_lowercase();
    reg.iter()
        .find(|p| p.short_link_hosts().iter().any(|h| *h == host))
        .map(|p| p.id())
}

/// Resolve a URL to a [`Playlist`]. Follows short links (HTTP redirect) first,
/// then dispatches to the matching provider.
pub async fn resolve_url(client: &reqwest::Client, url: &str) -> Result<Playlist> {
    let u = parse_http(url)
        .ok_or_else(|| anyhow!("Invalid URL. Provide a full http(s) music URL."))?;
    let reg = registry();

    if let Some(p) = reg.iter().find(|p| p.matches(&u)) {
        let normalized = p.normalize(&u);
        return p.fetch(client, &normalized).await;
    }

    // Short link: follow redirects, re-detect on the resolved URL.
    let host = u.host_str().unwrap_or("").to_lowercase();
    let is_short = reg
        .iter()
        .any(|p| p.short_link_hosts().iter().any(|h| *h == host));
    if !is_short {
        return Err(anyhow!("Unsupported music URL."));
    }
    let resp = client
        .get(u.as_str())
        .header("user-agent", "mdl/0.1")
        .send()
        .await?;
    let final_url = resp.url().clone();
    let p = reg
        .iter()
        .find(|p| p.matches(&final_url))
        .ok_or_else(|| anyhow!("Could not resolve the short link to a supported music URL."))?;
    let normalized = p.normalize(&final_url);
    p.fetch(client, &normalized).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProviderId;

    #[test]
    fn detects_spotify_album() {
        assert_eq!(
            detect_provider("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3"),
            Some(ProviderId::Spotify)
        );
    }

    #[test]
    fn detects_spotify_short_link() {
        assert_eq!(
            detect_provider("https://spotify.link/abcdef"),
            Some(ProviderId::Spotify)
        );
    }

    #[test]
    fn detects_apple_music_playlist() {
        assert_eq!(
            detect_provider("https://music.apple.com/us/playlist/foo/pl.u-abc123"),
            Some(ProviderId::AppleMusic)
        );
    }

    #[test]
    fn detects_youtube_music_playlist() {
        assert_eq!(
            detect_provider("https://music.youtube.com/playlist?list=PLabc123"),
            Some(ProviderId::YoutubeMusic)
        );
    }

    #[test]
    fn rejects_unknown() {
        assert_eq!(detect_provider("https://example.com/song/1"), None);
    }

    #[test]
    fn rejects_non_http() {
        assert_eq!(detect_provider("ftp://open.spotify.com/album/abc"), None);
    }
}
