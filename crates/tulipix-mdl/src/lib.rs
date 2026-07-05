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

pub use resolve::{detect_provider, resolve_url};
pub use types::{Playlist, Progress, ProviderId, Stage, Track};
