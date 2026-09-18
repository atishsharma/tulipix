//! Music section — library scan, metadata read, MusicBrainz/Cover-Art lookup,
//! the libmpv audio path (gapless / crossfade / ReplayGain / 10-band EQ),
//! local analysis (BPM + key, dynamic-range metering, AI stem separation),
//! synchronized lyrics (LRCLIB fetch + manual time-tagging), manual & smart
//! playlists, scrobbling (Last.fm + ListenBrainz), CLAP/PANNs embeddings →
//! Sonic Similar, browse/dashboard views, the now-playing queue, ratings,
//! podcasts, audiobooks, internet radio, library import (iTunes/foobar2000),
//! sleep timer, headphone EQ presets, format audit, folder browse, in-app
//! YouTube/SoundCloud search via yt-dlp, the streamed-Opus cache, grid
//! density, and the music-video link.
//!
//! Per the proxy model every module keys on `items.id`; analysis and user
//! state survive file moves.

pub mod schema;

pub mod ab_meta;
pub mod album_flip;
pub mod analysis;
pub mod audiobooks;
pub mod bpm_key;
pub mod browse;
pub mod cast;
pub mod dashboard;
pub mod mini_player;
pub mod motion;
pub mod sidebar_collapse;
pub mod similar;
pub mod discogs;
pub mod dl_history;
pub mod dr_meter;
pub mod embeddings;
pub mod eq;
pub mod folders;
pub mod formats;
pub mod grid_density;
pub mod headphone_eq;
pub mod import;
pub mod listen_prefs;
pub mod listenbrainz;
pub mod lyrics;
pub mod lyrics_sync;
pub mod musicbrainz;
pub mod opus_cache;
pub mod output_device;
pub mod player;
pub mod playlists;
pub mod pod_trends;
pub mod podcasts;
pub mod queue;
pub mod radio;
pub mod rating;
pub mod replaygain;
pub mod scan;
pub mod scrobble;
pub mod sleep_timer;
pub mod spotify_api;
pub mod stem;
pub mod tags;
pub mod video_link;
pub mod waveform;
pub mod visualizer;
pub mod youtube_data;
pub mod yt_search;
pub mod youtube;
pub mod yt_prefs;
