//! Videos section — scan, ffprobe metadata, TMDB/TVDB scraper, episodes,
//! watch-progress, chapters, recent rail, folder browse, star/archive/trash,
//! local subtitle picker, OpenSubtitles client, manual sub picker, styling,
//! LibreTranslate, word-level FTS, the Addic7ed fallback and Trakt history.

pub mod abr;
pub mod agents;
pub mod anime;
pub mod audio_normalize;
pub mod av1;
pub mod cast_deep_link;
pub mod chapters;
pub mod collections;
pub mod discover;
pub mod dlna_server;
pub mod episodes;
pub mod extras;
pub mod ffprobe;
pub mod folders;
pub mod last_accessed;
pub mod livetv;
pub mod naming;
pub mod personal_pool;
pub mod pre_roll;
pub mod scan;
pub mod schema;
pub mod skip_intro;
// Stream Plus (Videos → Stream Plus): the anime lane and the second provider
// stack. Its own module tree — it shares nothing with `stream` but the DB file.
pub mod splus;
pub mod star_archive_trash;
pub mod stream;
pub mod sub_addic7ed;
pub mod sub_local;
pub mod sub_manual;
pub mod sub_opensubtitles;
pub mod sub_search;
pub mod sub_styling;
pub mod sub_translate;
pub mod tmdb;
pub mod trakt;
pub mod trailer_preview;
pub mod tv_themes;
pub mod video_360;
pub mod watch_progress;
pub mod watchlist;
