//! Data model — port of mdl `schemas.ts` + `types.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderId {
    Spotify,
    AppleMusic,
    AmazonMusic,
    YoutubeMusic,
    Soundcloud,
    Bandcamp,
    Qobuz,
    Deezer,
    Tidal,
}

impl ProviderId {
    pub fn display_name(self) -> &'static str {
        match self {
            ProviderId::Spotify => "Spotify",
            ProviderId::AppleMusic => "Apple Music",
            ProviderId::AmazonMusic => "Amazon Music",
            ProviderId::YoutubeMusic => "YouTube Music",
            ProviderId::Soundcloud => "SoundCloud",
            ProviderId::Bandcamp => "Bandcamp",
            ProviderId::Qobuz => "Qobuz",
            ProviderId::Deezer => "Deezer",
            ProviderId::Tidal => "Tidal",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artists: Vec<String>,
    #[serde(default)]
    pub album: Option<String>,
    #[serde(default)]
    pub artwork_url: Option<String>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
    #[serde(default)]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub owner: Option<String>,
    #[serde(default)]
    pub artwork_url: Option<String>,
    pub provider: ProviderId,
    pub source_url: String,
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Initializing,
    SearchingYoutube,
    DownloadingAudio,
    WritingMetadata,
    WritingManifest,
    Skipped,
    Failed,
    Completed,
}

#[derive(Debug, Clone)]
pub struct Progress {
    pub track_index: usize,
    pub total: usize,
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: usize,
    pub stage: Stage,
    pub percent: f32,
    pub message: String,
    pub title: String,
    pub file_name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub dest_dir: std::path::PathBuf,
    pub parallelism: usize,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: Vec<(Track, String)>,
}
