//! Provider trait + registry — port of mdl `Providers.ts` interface + `utils.ts`.

use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub mod spotify;
pub mod apple_music;
pub mod amazon_music;
pub mod youtube_music;
pub mod soundcloud;
pub mod bandcamp;
pub mod qobuz;
pub mod deezer;
pub mod tidal;

#[async_trait::async_trait]
pub trait Provider: Send + Sync {
    fn id(&self) -> ProviderId;
    fn short_link_hosts(&self) -> &'static [&'static str] {
        &[]
    }
    fn matches(&self, url: &Url) -> bool;
    /// Normalize a matched URL (default: strip query + hash).
    fn normalize(&self, url: &Url) -> String {
        strip_query_and_hash(url)
    }
    async fn fetch(&self, client: &reqwest::Client, url: &str) -> Result<Playlist>;
}

pub fn registry() -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(spotify::Spotify),
        Box::new(apple_music::AppleMusic),
        Box::new(amazon_music::AmazonMusic),
        Box::new(youtube_music::YoutubeMusic),
        Box::new(soundcloud::Soundcloud),
        Box::new(bandcamp::Bandcamp),
        Box::new(qobuz::Qobuz),
        Box::new(deezer::Deezer),
        Box::new(tidal::Tidal),
    ]
}

/// First trimmed, non-empty candidate (port of `getFirstNonEmptyString`).
pub fn get_first_non_empty(candidates: &[Option<&str>]) -> Option<String> {
    candidates
        .iter()
        .flatten()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(String::from)
}

pub fn strip_query_and_hash(url: &Url) -> String {
    let mut u = url.clone();
    u.set_query(None);
    u.set_fragment(None);
    u.to_string()
}
