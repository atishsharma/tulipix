//! TV show theme music — fetch from a configurable theme-song endpoint
//! (default `tvthemes.plexapp.com`, override per-user in Settings → Sources)
//! and cache to disk so the show detail page can loop it without re-fetching.
//!
//! The fetcher is wrapped behind a `ThemeProvider` trait so tests inject a
//! `FakeProvider` and avoid network during `cargo test`.

use anyhow::{Context, Result};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThemeAudio {
    pub tvdb_id: i64,
    pub cached_path: PathBuf,
    pub source_url: String,
    pub bytes: usize,
}

#[async_trait]
pub trait ThemeProvider: Send + Sync {
    /// Fetch the theme song bytes for a given TVDB id. `None` when the source
    /// has no theme on file — UI silently falls back to ambient silence.
    async fn fetch(&self, tvdb_id: i64) -> Result<Option<Vec<u8>>>;
    fn url_for(&self, tvdb_id: i64) -> String;
}

pub struct PlexThemesProvider {
    pub base: String,
    http: reqwest::Client,
}

impl PlexThemesProvider {
    pub fn new() -> Self {
        Self {
            base: "https://tvthemes.plexapp.com".into(),
            http: tulipix_core::net::http().clone(),
        }
    }
}

impl Default for PlexThemesProvider {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl ThemeProvider for PlexThemesProvider {
    fn url_for(&self, tvdb_id: i64) -> String {
        format!("{}/{tvdb_id}.mp3", self.base)
    }

    async fn fetch(&self, tvdb_id: i64) -> Result<Option<Vec<u8>>> {
        let url = self.url_for(tvdb_id);
        let resp = self.http.get(&url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let bytes = resp.error_for_status()?.bytes().await?;
        Ok(Some(bytes.to_vec()))
    }
}

fn cache_filename(url: &str) -> String {
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    format!("{:x}.mp3", h.finalize())
}

/// Returns the local cache path. Pulls + writes on cache miss; otherwise
/// returns the existing file untouched.
pub async fn fetch_or_cached<P: ThemeProvider + ?Sized>(
    provider: &P,
    tvdb_id: i64,
    cache_dir: &Path,
) -> Result<Option<ThemeAudio>> {
    std::fs::create_dir_all(cache_dir).ok();
    let url = provider.url_for(tvdb_id);
    let path = cache_dir.join(cache_filename(&url));
    if path.exists() {
        let bytes = std::fs::metadata(&path).map(|m| m.len() as usize).unwrap_or(0);
        return Ok(Some(ThemeAudio {
            tvdb_id,
            cached_path: path,
            source_url: url,
            bytes,
        }));
    }
    match provider.fetch(tvdb_id).await? {
        Some(bytes) => {
            std::fs::write(&path, &bytes)
                .with_context(|| format!("write theme cache: {}", path.display()))?;
            Ok(Some(ThemeAudio {
                tvdb_id,
                cached_path: path,
                source_url: url,
                bytes: bytes.len(),
            }))
        }
        None => Ok(None),
    }
}

/// Browse-aware playback controller — tracks which show is currently focused
/// and decides whether to start/stop/fade the loop. Lives outside any media
/// engine so we can drive both mpv and a future rodio fallback.
#[derive(Debug, Default)]
pub struct ThemePlayback {
    active: Option<i64>,
}

impl ThemePlayback {
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the action the player should take given the newly focused
    /// `tvdb_id`. `None` means stop the current loop (browsed away from any
    /// show), `Some(id)` with `id == previously active` is a no-op.
    pub fn focus(&mut self, tvdb_id: Option<i64>) -> ThemeAction {
        match (self.active, tvdb_id) {
            (Some(cur), Some(new)) if cur == new => ThemeAction::NoChange,
            (Some(cur), Some(new)) => {
                self.active = Some(new);
                ThemeAction::Crossfade { from: cur, to: new }
            }
            (Some(cur), None) => {
                self.active = None;
                ThemeAction::Stop(cur)
            }
            (None, Some(new)) => {
                self.active = Some(new);
                ThemeAction::Start(new)
            }
            (None, None) => ThemeAction::NoChange,
        }
    }

    pub fn active(&self) -> Option<i64> {
        self.active
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeAction {
    NoChange,
    Start(i64),
    Stop(i64),
    Crossfade { from: i64, to: i64 },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    struct FakeProvider {
        rows: Vec<(i64, Option<Vec<u8>>)>,
        calls: Mutex<Vec<i64>>,
    }

    #[async_trait]
    impl ThemeProvider for FakeProvider {
        fn url_for(&self, tvdb_id: i64) -> String {
            format!("fake://{tvdb_id}")
        }
        async fn fetch(&self, tvdb_id: i64) -> Result<Option<Vec<u8>>> {
            self.calls.lock().unwrap().push(tvdb_id);
            for (id, bytes) in &self.rows {
                if *id == tvdb_id {
                    return Ok(bytes.clone());
                }
            }
            Ok(None)
        }
    }

    #[tokio::test]
    async fn fetch_writes_to_cache_then_serves_from_disk() {
        let tmp = TempDir::new().unwrap();
        let provider = FakeProvider {
            rows: vec![(123, Some(b"OggS_fake_bytes".to_vec()))],
            calls: Mutex::new(Vec::new()),
        };
        let first = fetch_or_cached(&provider, 123, tmp.path()).await.unwrap().unwrap();
        assert_eq!(first.bytes, 15);
        assert!(first.cached_path.exists());
        let second = fetch_or_cached(&provider, 123, tmp.path()).await.unwrap().unwrap();
        assert_eq!(second.cached_path, first.cached_path);
        assert_eq!(provider.calls.lock().unwrap().len(), 1, "second call hit cache");
    }

    #[tokio::test]
    async fn missing_theme_returns_none() {
        let tmp = TempDir::new().unwrap();
        let provider = FakeProvider {
            rows: vec![],
            calls: Mutex::new(Vec::new()),
        };
        let got = fetch_or_cached(&provider, 9, tmp.path()).await.unwrap();
        assert!(got.is_none());
    }

    #[test]
    fn playback_focus_transitions() {
        let mut pb = ThemePlayback::new();
        assert_eq!(pb.focus(Some(1)), ThemeAction::Start(1));
        assert_eq!(pb.focus(Some(1)), ThemeAction::NoChange);
        assert_eq!(pb.focus(Some(2)), ThemeAction::Crossfade { from: 1, to: 2 });
        assert_eq!(pb.focus(None), ThemeAction::Stop(2));
        assert_eq!(pb.focus(None), ThemeAction::NoChange);
    }

    #[test]
    fn cache_filename_is_stable_and_url_specific() {
        let a = cache_filename("https://x/y/1.mp3");
        let b = cache_filename("https://x/y/1.mp3");
        let c = cache_filename("https://x/y/2.mp3");
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
