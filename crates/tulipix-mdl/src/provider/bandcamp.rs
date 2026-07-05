//! Bandcamp provider — stub (Task 12).

use super::Provider;
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub struct Bandcamp;

#[async_trait::async_trait]
impl Provider for Bandcamp {
    fn id(&self) -> ProviderId {
        ProviderId::Bandcamp
    }
    fn matches(&self, _url: &Url) -> bool {
        false
    }
    async fn fetch(&self, _client: &reqwest::Client, _url: &str) -> Result<Playlist> {
        anyhow::bail!("Bandcamp not implemented yet")
    }
}
