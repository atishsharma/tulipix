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

/// Filename layout for a downloaded track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameMethod {
    ArtistSong,   // "Artist - Song"
    ArtistsSong,  // "Artist1, Artist2 - Song"
    AlbumSong,    // "Album - Song"
    Numbered,     // "01 - Artist - Song"
    SongOnly,     // "Song"
}

impl NameMethod {
    /// Match the dropdown label the UI sends.
    pub fn from_label(label: &str) -> NameMethod {
        match label {
            "Artists - Song" => NameMethod::ArtistsSong,
            "Album - Song" => NameMethod::AlbumSong,
            "## - Artist - Song" => NameMethod::Numbered,
            "Song" => NameMethod::SongOnly,
            _ => NameMethod::ArtistSong,
        }
    }

    /// Build the (unsanitized) file stem for a track at 1-based `index`.
    pub fn stem(self, index: usize, track: &Track) -> String {
        let primary = track.artists.first().map(String::as_str).unwrap_or("Unknown");
        let all = track.artists.join(", ");
        let album = track.album.as_deref().unwrap_or("Unknown Album");
        match self {
            NameMethod::ArtistSong => format!("{primary} - {}", track.title),
            NameMethod::ArtistsSong => format!("{all} - {}", track.title),
            NameMethod::AlbumSong => format!("{album} - {}", track.title),
            NameMethod::Numbered => format!("{index:02} - {primary} - {}", track.title),
            NameMethod::SongOnly => track.title.clone(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub dest_dir: std::path::PathBuf,
    /// Number of tracks fetched concurrently (1–4).
    pub parallelism: usize,
    /// yt-dlp `--concurrent-fragments N` per track (1–8). Only speeds up
    /// fragmented (DASH/HLS) streams; a harmless no-op on progressive audio.
    pub threads_per_download: usize,
    /// Audio format: opus | m4a | mp3 | flac | wav.
    pub format: String,
    pub name_method: NameMethod,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: Vec<(Track, String)>,
}
