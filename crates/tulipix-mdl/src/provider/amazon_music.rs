//! Amazon Music provider — stub (Task 10).

use super::Provider;
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub struct AmazonMusic;

#[async_trait::async_trait]
impl Provider for AmazonMusic {
    fn id(&self) -> ProviderId {
        ProviderId::AmazonMusic
    }
    fn matches(&self, _url: &Url) -> bool {
        false
    }
    async fn fetch(&self, _client: &reqwest::Client, _url: &str) -> Result<Playlist> {
        anyhow::bail!("Amazon Music not implemented yet")
    }
}
