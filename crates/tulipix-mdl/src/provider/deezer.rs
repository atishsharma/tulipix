//! Deezer provider — stub (Task 9).

use super::Provider;
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub struct Deezer;

#[async_trait::async_trait]
impl Provider for Deezer {
    fn id(&self) -> ProviderId {
        ProviderId::Deezer
    }
    fn matches(&self, _url: &Url) -> bool {
        false
    }
    async fn fetch(&self, _client: &reqwest::Client, _url: &str) -> Result<Playlist> {
        anyhow::bail!("Deezer not implemented yet")
    }
}
