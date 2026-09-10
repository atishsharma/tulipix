//! Native port of the `mdl` music downloader.
//!
//! - [`resolve`] — URL -> [`Playlist`] via the [`provider`] scrapers.
//! - [`download`] — per-track search/download/tag pipeline (bundled yt-dlp + lofty).
//! - [`manifest`] — `.mdl.json` skip/resync bookkeeping.

pub mod types;
pub mod provider;
pub mod resolve;
pub mod manifest;
pub mod download;

pub use provider::deezer::search as search_deezer;
pub use provider::spotify::search as search_spotify;
pub use provider::youtube_music::search as search_ytmusic;
pub use resolve::{detect_provider, resolve_url};
pub use types::{Playlist, Progress, ProviderId, Stage, Track};
