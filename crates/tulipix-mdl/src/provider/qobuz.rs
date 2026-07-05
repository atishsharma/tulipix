//! Qobuz provider — stub (Task 11).

use super::Provider;
use crate::types::{Playlist, ProviderId};
use anyhow::Result;
use url::Url;

pub struct Qobuz;

#[async_trait::async_trait]
impl Provider for Qobuz {
    fn id(&self) -> ProviderId {
        ProviderId::Qobuz
    }
    fn matches(&self, _url: &Url) -> bool {
        false
    }
    async fn fetch(&self, _client: &reqwest::Client, _url: &str) -> Result<Playlist> {
        anyhow::bail!("Qobuz not implemented yet")
    }
}
