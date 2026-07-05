//! SoundCloud provider — stub (Task 8).

use super::Provider;
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub struct Soundcloud;

#[async_trait::async_trait]
impl Provider for Soundcloud {
    fn id(&self) -> ProviderId {
        ProviderId::Soundcloud
    }
    fn matches(&self, _url: &Url) -> bool {
        false
    }
    async fn fetch(&self, _client: &reqwest::Client, _url: &str) -> Result<Playlist> {
        anyhow::bail!("SoundCloud not implemented yet")
    }
}
