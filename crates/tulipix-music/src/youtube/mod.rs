//! `np.p4.music.youtube` — the YouTube category: Takeout-imported subscriptions,
//! Piped-backed search + channel browsing (yt-dlp fallback), local playlists,
//! an auto-cache of streamed videos, and an explicit downloads store. All tables
//! are self-contained (no `items` FK), so they live in their own `youtube.db`
//! section like podcasts/radio.

pub mod store;
pub mod piped;
pub mod subscriptions;
pub mod thumbs;
