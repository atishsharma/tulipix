//! `.mdl.json` sync manifest — port of mdl `manifest.ts`.
//!
//! Records which source tracks already landed on disk so re-runs skip them.

use crate::types::{ProviderId, Track};
use serde::{Deserialize, Serialize};
use std::path::Path;

pub const MANIFEST_FILE_NAME: &str = ".mdl.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestTrack {
    pub source_track_id: String,
    pub title: String,
    #[serde(default)]
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    pub file_name: String,
    pub relative_path: String,
    pub downloaded_at: String,
}

impl ManifestTrack {
    pub fn from_track(track: &Track, file_name: &str) -> Self {
        ManifestTrack {
            source_track_id: track.id.clone(),
            title: track.title.clone(),
            artists: track.artists.clone(),
            album: track.album.clone(),
            file_name: file_name.to_string(),
            relative_path: file_name.to_string(),
            downloaded_at: now_iso8601(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub version: u32,
    pub provider: ProviderId,
    pub playlist_id: String,
    pub playlist_title: String,
    pub playlist_url: String,
    pub generated_at: String,
    #[serde(default)]
    pub tracks: Vec<ManifestTrack>,
}

impl Manifest {
    pub fn new(provider: ProviderId, playlist_id: &str, title: &str, url: &str) -> Self {
        Manifest {
            version: 1,
            provider,
            playlist_id: playlist_id.to_string(),
            playlist_title: title.to_string(),
            playlist_url: url.to_string(),
            generated_at: now_iso8601(),
            tracks: Vec::new(),
        }
    }

    /// Insert or replace a track by its `source_track_id`.
    pub fn upsert(&mut self, track: ManifestTrack) {
        match self
            .tracks
            .iter_mut()
            .find(|t| t.source_track_id == track.source_track_id)
        {
            Some(existing) => *existing = track,
            None => self.tracks.push(track),
        }
        self.generated_at = now_iso8601();
    }

    pub fn contains(&self, source_track_id: &str) -> Option<&ManifestTrack> {
        self.tracks
            .iter()
            .find(|t| t.source_track_id == source_track_id)
    }
}

/// Load `<dir>/.mdl.json`, returning `None` if absent or malformed.
pub fn load(dir: &Path) -> Option<Manifest> {
    let body = std::fs::read_to_string(dir.join(MANIFEST_FILE_NAME)).ok()?;
    serde_json::from_str(&body).ok()
}

pub fn save(dir: &Path, manifest: &Manifest) -> std::io::Result<()> {
    let body = serde_json::to_string_pretty(manifest)
        .unwrap_or_else(|_| "{}".to_string());
    std::fs::write(dir.join(MANIFEST_FILE_NAME), body)
}

fn now_iso8601() -> String {
    // Seconds since epoch is enough for bookkeeping; avoids a chrono dep.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::ProviderId;

    fn mtrack(id: &str, file: &str) -> ManifestTrack {
        ManifestTrack {
            source_track_id: id.into(),
            title: "T".into(),
            artists: vec![],
            album: None,
            file_name: file.into(),
            relative_path: file.into(),
            downloaded_at: "0".into(),
        }
    }

    #[test]
    fn upsert_replaces_by_source_id() {
        let mut m = Manifest::new(ProviderId::Spotify, "pid", "Title", "url");
        m.upsert(mtrack("t1", "a.opus"));
        m.upsert(mtrack("t1", "b.opus"));
        assert_eq!(m.tracks.len(), 1);
        assert_eq!(m.tracks[0].file_name, "b.opus");
        assert!(m.contains("t1").is_some());
        assert!(m.contains("nope").is_none());
    }

    #[test]
    fn roundtrips_to_disk() {
        let dir = std::env::temp_dir().join(format!("mdl-manifest-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut m = Manifest::new(ProviderId::AppleMusic, "pid", "Title", "url");
        m.upsert(mtrack("t1", "a.opus"));
        save(&dir, &m).unwrap();
        let loaded = load(&dir).unwrap();
        assert_eq!(loaded.tracks.len(), 1);
        assert_eq!(loaded.provider, ProviderId::AppleMusic);
        std::fs::remove_dir_all(&dir).ok();
    }
}
