//! Music section — five tabs, one player, four symbols.
//!
//! `ui/page_music.slint` is 731 KB and declares 751 properties and callbacks on
//! one component, because Slint has no store: My Music, Podcasts, Audiobooks,
//! Radio and YouTube all hang off the same `MusicPage`, and every value any of
//! them can read has to be a property on it. Flutter has state management, so
//! the ephemeral half — which dialog is open, which chip is hovered, what is
//! typed into a box that has not been submitted — stays in Dart and never
//! crosses the boundary. What crosses is one snapshot, one command, one event
//! stream, and a lazy artwork resolver.
//!
//! The five tabs are one section, not five: they share a player bar, a queue,
//! a volume, and exactly one mpv process. Starting a podcast stops the album.
//! That is why `view` is a field of one state struct rather than five separate
//! sections — the interconnection is the feature, and splitting it would mean
//! five snapshots that have to agree with each other about who is playing.
//!
//! The domain crate is untouched: every query below is either a call into
//! `tulipix_music::*` or SQL against the schema that crate defines.

use anyhow::Result;
use flutter_rust_bridge::frb;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::api::photos::human_size;
use crate::db::{music_pool, podcasts_pool, radio_pool, youtube_pool};
use crate::frb_generated::StreamSink;
use crate::mpv;

/// Songs list page. Eight across, four down -- the grid on the Songs tab is
/// pinned to eight columns, so a page is exactly the four rows that fit above
/// the fold and the pager in row 2 steps a screen at a time. A library of
/// 40,000 tracks is one `LIMIT`, not one model.
const SONGS_PAGE: i64 = 32;
/// Album / artist / genre / folder grids. Seven across, four down — the same
/// arithmetic the Songs grid uses, against the seven columns Slint pins these
/// to.
const BROWSE_PAGE: i64 = 28;
/// The Folders grid. Seven across, three down -- a folder tile is taller than
/// an album's, and the fourth row would be under the fold.
const FOLDER_PAGE: i64 = 21;
/// Loved and History, which are lists rather than grids: twenty-five rows is
/// about a screen of them.
const LIST_ROWS: i64 = 25;
/// Podcast episode lists, YouTube download lists, radio station lists.
const LIST_PAGE: i64 = 30;
/// Home rails. Two rows of seven on a wide window, and nothing below the fold
/// is worth a query.
const RAIL: i64 = 14;
/// How many search hits a YouTube query asks for.
const YT_HITS: usize = 20;
/// Cached YouTube audio files kept before the oldest is evicted. Matches the
/// Slint build's `YT_CACHE_KEEP`.
const YT_CACHE_KEEP: i64 = 60;

// ---------------------------------------------------------------- state ----

/// One row of any track list — the songs page, an album's tracks, the queue,
/// a playlist, the history. One shape, because every list of tracks in the
/// section shows the same seven things and a second shape would only mean two
/// places to fix when the eighth arrives.
#[derive(Debug, Clone)]
pub struct Track {
    pub item_id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_s: f64,
    pub path: String,
    /// Album cover path, or "" when the album has none on disk — the tile then
    /// calls `music_ensure_art`, which is where embedded artwork gets pulled
    /// out of the file. Same lazy pass as the photo grid's thumbnails.
    pub art: String,
    pub loved: bool,
    pub stars: i64,
    pub play_count: i64,
    pub track_no: i64,
    pub year: i64,
    pub genre: String,
    /// "" when the track has no stored lyrics; "synced" when they carry LRC
    /// timestamps, "plain" when they do not.
    pub lyrics: String,
}

/// An album, artist, genre, playlist or folder tile. `key` carries the
/// identity for the ones that have no integer id: a genre is its name, a
/// folder is its absolute path.
#[derive(Debug, Clone)]
pub struct BrowseCard {
    pub id: i64,
    pub key: String,
    pub title: String,
    pub subtitle: String,
    pub count: i64,
    pub art: String,
    pub loved: bool,
    pub stars: i64,
}

/// The four figures on the My Music home header, pre-formatted: they are read,
/// not computed against, and "3 days" is not a number Dart should be deriving
/// from milliseconds on every rebuild.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub week: String,
    pub total: String,
    pub genre: String,
    pub streak: String,
}

/// One song in the Tags & Lyrics manager, with the coverage verdict that the
/// list's filter and status pill are drawn from.
#[derive(Debug, Clone)]
pub struct MgrRow {
    pub track: Track,
    /// Lyrics tab: Synced | Normal | Missing. Tags tab: Tagged | Missing.
    pub status: String,
}

/// One LRCLIB candidate, as offered in the manager's search results.
#[derive(Debug, Clone)]
pub struct LyricHit {
    pub title: String,
    pub sub: String,
    /// Synced | Plain -- whether it carries timestamps.
    pub kind: String,
}

#[derive(Debug, Clone)]
pub struct LyricLine {
    pub at_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct PodcastShow {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub category: String,
    pub description: String,
    /// Either an http(s) URL or a local path to a custom thumbnail; Dart tells
    /// them apart by the scheme and picks `Image.network` or `Image.file`.
    pub art: String,
    pub episodes: i64,
    pub unplayed: i64,
    pub latest: i64,
    pub feed_url: String,
}

/// One card in the Trends grid — the baked directory, not a subscription.
///
/// `idx` is the position in `tulipix_music::pod_trends::feed_urls()`, which is
/// what the artwork cache is keyed on; Dart passes `feed_url` back for every
/// action, so the two never have to agree on an ordering.
#[derive(Debug, Clone)]
pub struct PodTrend {
    pub idx: i64,
    pub feed_url: String,
    pub title: String,
    pub author: String,
    pub category: String,
    pub art: String,
    pub subscribed: bool,
}

/// The read-only info card, opened from a Trends card or a subscribed show.
///
/// Deliberately never loads the episode list: it exists to answer "what is
/// this show" before subscribing, and pulling a back catalogue to answer that
/// is what made the Slint one slow before it stopped doing it.
#[derive(Debug, Clone)]
pub struct PodInfo {
    pub feed_url: String,
    /// -1 when the show is not subscribed (a Trends card).
    pub podcast_id: i64,
    pub title: String,
    pub author: String,
    pub category: String,
    pub description: String,
    pub art: String,
    pub episodes: i64,
    pub latest: i64,
    pub subscribed: bool,
}

#[derive(Debug, Clone)]
pub struct Episode {
    pub id: i64,
    pub podcast_id: i64,
    pub show: String,
    pub title: String,
    pub description: String,
    pub published: i64,
    pub duration_s: f64,
    pub art: String,
    pub audio_url: String,
    /// "" until the episode has been saved for offline.
    pub downloaded: String,
    pub position_s: f64,
    pub played: bool,
}

#[derive(Debug, Clone)]
pub struct BookCard {
    pub folder: String,
    pub title: String,
    pub author: String,
    pub art: String,
    pub chapters: i64,
    pub finished: bool,
    /// 0..1 across the whole book, not the open chapter.
    pub progress: f64,
    pub total_s: f64,
}

#[derive(Debug, Clone)]
pub struct Chapter {
    pub item_id: i64,
    pub title: String,
    pub duration_s: f64,
    pub position_s: f64,
    pub finished: bool,
}

#[derive(Debug, Clone)]
pub struct Bookmark {
    pub item_id: i64,
    pub position_s: f64,
    pub label: String,
    /// "Ch 3 · 12:40" — the chapter index has to be resolved against the book's
    /// track order, which Dart does not have.
    pub when: String,
}

#[derive(Debug, Clone)]
pub struct Station {
    pub uuid: String,
    pub name: String,
    pub url: String,
    pub favicon: String,
    pub country: String,
    pub tags: String,
    pub codec: String,
    pub bitrate: i64,
    pub favourite: bool,
}

#[derive(Debug, Clone)]
pub struct RadioCategory {
    pub label: String,
    pub icon: String,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct YtVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    /// Remote URL for a search hit, a local cached JPEG for anything the store
    /// has seen before.
    pub thumb: String,
    pub duration: i64,
    /// Set for cached and downloaded rows; "" for a search hit.
    pub media_path: String,
    pub meta: String,
    /// 0..1 watch position, from `yt_progress`.
    pub progress: f64,
    pub quality: String,
}

#[derive(Debug, Clone)]
pub struct YtSub {
    pub channel_id: String,
    pub title: String,
    pub avatar: String,
    pub videos: i64,
    pub subs: i64,
    pub subscribed: bool,
}

#[derive(Debug, Clone)]
pub struct YtPlaylist {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub cover: String,
    pub source_url: String,
}

/// What the player bar shows, wherever the user is. Survives a tab switch on
/// purpose: leaving My Music for Radio must not blank a playing album.
#[derive(Debug, Clone, Default)]
pub struct NowPlaying {
    /// idle | music | podcast | book | radio | youtube — which tab owns the
    /// process, and therefore what Next means.
    pub mode: String,
    pub item_id: i64,
    pub key: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    pub playing: bool,
    pub loaded: bool,
    pub pos: f64,
    pub dur: f64,
    pub volume: f64,
    pub muted: bool,
    pub loved: bool,
    pub stars: i64,
    /// The ICY `media-title` of a radio stream — the only place the actual
    /// song is readable, because the station name is ours, not the stream's.
    pub stream_title: String,
}

#[derive(Debug, Clone)]
pub struct MusicState {
    // --- shell ---
    /// mymusic | podcasts | audiobooks | radio | youtube
    pub view: String,
    pub query: String,
    /// Non-empty when the last command failed in a way the user should see.
    pub status: String,

    // --- transport, shared by all five tabs ---
    pub now: NowPlaying,
    pub shuffle: bool,
    /// off | all | one
    pub repeat: String,
    pub sleep_min: i64,
    pub queue: Vec<Track>,
    pub lyrics: Vec<LyricLine>,
    pub lyrics_plain: String,
    pub lyrics_offset_ms: i64,
    pub eq_preset: String,
    pub eq_bands: Vec<f64>,
    pub eq_on: bool,
    pub devices: Vec<String>,
    pub device: String,
    pub gapless: bool,
    pub crossfade: f64,
    pub replaygain: String,
    pub preamp_db: f64,

    // --- My Music ---
    /// home | songs | albums | artists | genres | playlists | folders |
    /// favorites | history
    pub lib_tab: String,
    pub track_count: i64,
    pub songs: Vec<Track>,
    pub song_total: i64,
    pub song_page: i64,
    pub song_pages: i64,
    /// title | artist | album | added | plays | duration
    pub song_sort: String,
    pub song_dir: String,
    pub cards: Vec<BrowseCard>,
    pub card_total: i64,
    pub browse_page: i64,
    pub browse_pages: i64,
    /// name | count
    pub browse_sort: String,
    pub browse_dir: String,
    pub stats: Stats,
    pub rail_recent: Vec<Track>,
    pub rail_most: Vec<Track>,
    pub rail_loved: Vec<Track>,
    pub rail_fresh: Vec<Track>,
    pub rail_albums: Vec<BrowseCard>,
    pub rail_artists: Vec<BrowseCard>,
    /// The watched roots themselves, as opposed to the folders tracks were
    /// found in — the Library tab lists these and the Folders tab lists those.
    pub roots: Vec<BrowseCard>,
    /// Every playlist, whatever tab is open.
    ///
    /// `cards` only holds playlists while the Playlists tab is the one showing,
    /// so "add this to a playlist" had to switch the tab, read the cards and
    /// switch back — which reloaded the library twice and left the section on
    /// whatever tab lost the race. The list is a dozen rows; carrying it on
    /// every snapshot is cheaper than the round trip it replaces.
    pub playlists: Vec<BrowseCard>,

    // --- Tags & Lyrics manager ---
    /// A modal over the Songs tab, in two halves: which of the library's songs
    /// still have no tags, and which still have no words. Both are the same
    /// shape -- three stat cards that double as filters, a coverage bar, a
    /// paged list with a status pill and a per-row action.
    pub mgr_open: bool,
    /// tags | lyrics
    pub mgr_tab: String,
    /// list | search | view
    pub mgr_mode: String,
    /// all | synced | missing (lyrics) · all | tagged | missing (tags)
    pub mgr_filter: String,
    pub mgr_page: i64,
    pub mgr_pages: i64,
    pub mgr_total: i64,
    /// Synced (lyrics) or fully tagged (tags).
    pub mgr_ok: i64,
    pub mgr_missing: i64,
    pub mgr_rows: Vec<MgrRow>,
    /// The song the search or the read-only view belongs to.
    pub mgr_item_id: i64,
    pub mgr_title: String,
    pub mgr_q_name: String,
    pub mgr_q_artist: String,
    pub mgr_q_album: String,
    pub mgr_searching: bool,
    pub mgr_results: Vec<LyricHit>,
    /// Whatever the view or the picked candidate holds.
    pub mgr_view_rows: Vec<LyricLine>,
    pub mgr_view_plain: String,
    /// A picked candidate that has not been saved yet.
    pub mgr_dirty: bool,
    pub mgr_status: String,
    pub mgr_progress: f64,
    /// A bulk fetch is running; the coverage bar is a progress bar until it
    /// finishes.
    pub mgr_busy: bool,

    // --- album / artist / genre / playlist / folder detail ---
    pub detail_open: bool,
    /// album | artist | genre | playlist | folder
    pub detail_kind: String,
    pub detail_id: i64,
    pub detail_key: String,
    pub detail_title: String,
    pub detail_subtitle: String,
    pub detail_art: String,
    pub detail_tracks: Vec<Track>,
    pub detail_albums: Vec<BrowseCard>,
    /// The heart and the stars on the *subject* of the page -- the album or the
    /// artist itself, which is a different row from any of its tracks.
    pub detail_loved: bool,
    pub detail_stars: i64,
    /// Who made the album this page is about, so its subtitle can be a link to
    /// their page. 0 on every other kind.
    pub detail_artist_id: i64,
    /// An artist biography, fetched once from MusicBrainz + Wikipedia and kept
    /// in `artists.bio` from then on. Empty for every other kind of page.
    pub detail_info: String,

    // --- Podcasts ---
    /// home | subscribed | downloads
    pub pod_tab: String,
    pub pod_shows: Vec<PodcastShow>,
    pub pod_total: i64,
    pub pod_page: i64,
    pub pod_pages: i64,
    pub pod_categories: Vec<String>,
    pub pod_cat: String,
    pub pod_latest: Vec<Episode>,
    pub pod_downloads: Vec<Episode>,
    pub pod_detail_open: bool,
    pub pod_detail: Option<PodcastShow>,
    pub pod_episodes: Vec<Episode>,
    pub pod_ep_page: i64,
    pub pod_ep_pages: i64,
    /// new | old
    pub pod_ep_sort: String,
    pub pod_speed: f64,
    pub pod_queue: Vec<Episode>,
    /// Home "Your shows" — only the pinned ones, in `pod_home_sort` order.
    pub pod_home: Vec<PodcastShow>,
    pub pod_home_page: i64,
    pub pod_home_pages: i64,
    pub pod_home_sort: String,
    pub pod_dl_sort: String,
    pub pod_trends: Vec<PodTrend>,
    pub pod_trends_loading: bool,
    pub pod_trends_sort: String,
    pub pod_trends_page: i64,
    pub pod_trends_pages: i64,
    pub pod_info: Option<PodInfo>,
    /// Show notes for one episode. Empty title = the panel is closed.
    pub pod_transcript_title: String,
    pub pod_transcript_text: String,
    /// One progress line for every long podcast job — refresh, subscribe,
    /// reset. They never overlap (each one blocks the button that starts the
    /// next), so three separate sets of fields would only be three ways to
    /// spell the same bar.
    pub pod_busy: bool,
    pub pod_frac: f64,
    pub pod_status: String,
    /// The search box for the open tab. Each tab keeps its own, so a filter on
    /// Downloads does not silently hide half of Subscribed.
    pub pod_query: String,
    /// The episode currently saving offline, and how far in. -1 = none.
    pub pod_dl_id: i64,
    pub pod_dl_frac: f64,
    pub pod_dl_title: String,

    // --- Audiobooks ---
    /// all | progress | finished | folders
    pub book_tab: String,
    pub books: Vec<BookCard>,
    pub book_detail_open: bool,
    pub book_detail: Option<BookCard>,
    pub book_chapters: Vec<Chapter>,
    pub book_bookmarks: Vec<Bookmark>,
    pub book_speed: f64,
    pub book_resume_index: i64,

    // --- Radio ---
    /// home | favourites | recent
    pub radio_tab: String,
    pub radio_categories: Vec<RadioCategory>,
    pub radio_cat_open: bool,
    pub radio_cat_title: String,
    pub radio_stations: Vec<Station>,
    pub radio_total: i64,
    pub radio_page: i64,
    pub radio_pages: i64,
    /// default | name | bitrate
    pub radio_sort: String,

    // --- YouTube ---
    /// home | subscriptions | playlists | cached | downloads
    pub yt_tab: String,
    pub yt_results: Vec<YtVideo>,
    pub yt_recent: Vec<String>,
    pub yt_cached: Vec<YtVideo>,
    pub yt_downloads: Vec<YtVideo>,
    pub yt_dl_page: i64,
    pub yt_dl_pages: i64,
    pub yt_dl_total: i64,
    pub yt_subs: Vec<YtSub>,
    pub yt_sub_count: i64,
    pub yt_subs_page: i64,
    pub yt_subs_pages: i64,
    pub yt_playlists: Vec<YtPlaylist>,
    pub yt_channel_open: bool,
    pub yt_channel_id: String,
    pub yt_channel_title: String,
    pub yt_channel_avatar: String,
    pub yt_channel_subscribed: bool,
    /// latest | popular — which list `yt_channel_videos` holds.
    pub yt_channel_mode: String,
    pub yt_channel_videos: Vec<YtVideo>,
    pub yt_playlist_open: bool,
    pub yt_playlist_id: i64,
    pub yt_playlist_title: String,
    pub yt_playlist_videos: Vec<YtVideo>,
    pub yt_status: String,
    /// Home's centre column when nothing has been searched: the newest video
    /// from each subscribed channel. Without it Home was an empty box until
    /// you typed something, which is not what a subscriptions feed is for.
    pub yt_recommended: Vec<YtVideo>,
    /// A further page of search hits exists.
    pub yt_results_more: bool,
    /// new | old | az
    pub yt_dl_sort: String,
    /// name | videos | subscribers
    pub yt_subs_sort: String,
    /// asc | desc
    pub yt_subs_dir: String,
    /// sub | unsub — which half of the channel table the page lists.
    pub yt_subs_filter: String,
    /// default | title | duration
    pub yt_playlist_sort: String,
    /// "142 videos · 4.2M subscribers", when the channel row knows.
    pub yt_channel_sub: String,
    /// Downloads in flight or waiting, oldest first. `frac` is only meaningful
    /// on the first one — yt-dlp runs one at a time.
    pub yt_jobs: Vec<YtJob>,
    pub yt_busy: bool,
    /// Preferred download height. 0 = best video, < 0 = audio only,
    /// `-999` = no default, so the picker asks.
    pub yt_default_res: i64,
    /// Channel ids pinned to the Home rail, newest first.
    pub yt_home_channels: Vec<String>,
    /// The pinned channels themselves, resolved and in pin order. Falls back to
    /// the most-followed subscriptions while nothing is pinned.
    pub yt_home_subs: Vec<YtSub>,
    /// auto | piped | ytdlp
    pub yt_fetcher: String,
    /// Counts refresh — a yt-dlp spawn per channel, so it needs a bar.
    pub yt_fetch_busy: bool,
    pub yt_fetch_frac: f64,
    pub yt_fetch_msg: String,
    /// Channel page: the scoped search box and its hits, kept apart from
    /// `yt_channel_videos` so clearing the box restores the listing.
    pub yt_channel_query: String,
    pub yt_channel_results: Vec<YtVideo>,
    /// How many pages of the channel listing have been pulled, and whether
    /// another exists.
    pub yt_channel_page: i64,
    pub yt_channel_has_next: bool,
    /// A video is on screen rather than on the deck. The player bar drops its
    /// transport while this is true — the picture carries its own.
    pub yt_watching: bool,
    /// The playlist currently being played through, so its card can show it.
    /// -1 = none.
    pub yt_playing_pl_id: i64,
    /// Gradient outline on the Home rails. On by default, off for people who
    /// find it loud.
    pub yt_home_connect: bool,
}

#[derive(Debug, Clone)]
pub struct YtJob {
    pub video_id: String,
    pub title: String,
    /// 0..1 for the running job, 0 for anything still queued.
    pub frac: f64,
    pub running: bool,
}

// -------------------------------------------------------------- commands ----

#[derive(Debug, Clone)]
pub enum MusicCmd {
    /// Re-read the current view. Sent on mount, after an external change, and
    /// by the player tick when the track has moved on.
    Refresh,

    /// mymusic | podcasts | audiobooks | radio | youtube
    SetView { name: String },
    Search { query: String },

    // --- My Music ---
    SetLibTab { name: String },
    SetSongSort { mode: String, dir: String },
    SetSongPage { page: i64 },
    SetBrowseSort { mode: String, dir: String },
    SetBrowsePage { page: i64 },
    OpenAlbum { album_id: i64 },
    OpenArtist { artist_id: i64 },
    /// The album / artist of whatever is on the deck. Every player draws the
    /// track's title over its album and its subtitle over its artist, and in
    /// Slint both are links -- `open-now-album` / `open-now-artist`. The ids
    /// are not on `NowPlaying`, so they are looked up here rather than carried
    /// on every tick.
    OpenNowAlbum,
    OpenNowArtist,
    OpenGenre { name: String },
    OpenFolder { path: String },

    // --- Tags & Lyrics manager ---
    MgrOpen { tab: String },
    MgrClose,
    MgrSetTab { tab: String },
    MgrSetFilter { key: String },
    MgrSetPage { page: i64 },
    /// One step back: preview → results, results → list.
    MgrBack,
    /// Open the search form for one song, seeded from its tags.
    MgrSearchOpen { item_id: i64 },
    MgrSetQuery { name: String, artist: String, album: String },
    MgrSearch,
    /// Preview one of the results.
    MgrPick { index: i64 },
    /// Write the previewed candidate to the library.
    MgrSave,
    /// Read-only look at what is already stored for a song.
    MgrView { item_id: i64 },
    /// Fetch words for everything the current filter calls missing.
    MgrSyncAll,
    OpenPlaylist { playlist_id: i64 },
    CloseDetail,
    AddFolder { path: String },
    RemoveRoot { path: String },
    Scan,
    /// Read tags for every music item that has none yet. Separate from `Scan`
    /// because the walk is cheap and the ffprobe pass is not.
    ReadTags,
    SaveTags {
        item_id: i64,
        title: String,
        artist: String,
        album: String,
        album_artist: String,
        genre: String,
        date: String,
        track_no: i64,
        disc_no: i64,
    },
    DeleteTrack { item_id: i64 },

    // --- the right-click menu on a song ------------------------------------
    //
    // Slint gives every song row and every song tile the same menu, and most
    // of it was already commands here -- play, love, rate, edit tags, delete.
    // These are the rest of it.
    /// Hand the file to whatever the desktop opens audio with.
    SongPlayDefault { item_id: i64 },
    /// The album / artist page of any track, not only the one on the deck.
    SongViewAlbum { item_id: i64 },
    SongViewArtist { item_id: i64 },
    /// Queue the tracks that sound closest to this one, by the DSP embeddings
    /// in `tulipix_music::embeddings`.
    SongSonic { item_id: i64 },
    /// Play the music video linked to this song, if one is.
    SongPlayVideo { item_id: i64 },
    /// Link one — `path` comes from a chooser opened on the Dart side.
    SongLinkVideo { item_id: i64, path: String },
    /// Covers for the two kinds that have no column to hold one: a genre is a
    /// string and a playlist is a name. `key` is the genre, or the playlist id
    /// as text.
    SetCardArt { kind: String, key: String, path: String },

    // --- transport ---
    /// Replace the queue with what the current view is showing and start at
    /// `index`. This is what clicking a row in a list means, as opposed to
    /// `Play`, which starts one track and leaves the queue alone.
    PlayList { item_ids: Vec<i64>, index: i64, source: String },
    PlayPause,
    Next,
    Prev,
    Stop,
    Seek { secs: f64 },
    SetVolume { volume: f64 },
    ToggleMute,
    ToggleShuffle,
    CycleRepeat,
    SetSleep { minutes: i64 },
    Love { item_id: i64 },
    Rate { item_id: i64, stars: i64 },
    /// Loving or rating a whole album or artist, which the browse tiles do.
    /// Stored on the `albums` / `artists` row rather than fanned out over its
    /// tracks: the Slint build does the same, and a fan-out would make "loved
    /// album" and "every track loved" the same thing, which they are not.
    AlbumFav { album_id: i64 },
    AlbumRate { album_id: i64, stars: i64 },
    ArtistFav { artist_id: i64 },
    ArtistRate { artist_id: i64, stars: i64 },
    QueueAdd { item_ids: Vec<i64> },
    QueueRemove { item_id: i64 },
    QueueClear,
    QueueMove { from: i64, to: i64 },
    QueuePlayAt { index: i64 },
    PlaylistCreate { name: String },
    /// loved | recent — the two smart playlists the Slint build offers.
    PlaylistCreateSmart { kind: String },
    /// Open the "Recently added" playlist, rebuilding it first.
    ///
    /// Not a smart playlist: the rule engine has no date-added field, so this
    /// is a real playlist whose contents are rewritten each time it is asked
    /// for. That is what "View all" on the Recently added rail means -- the
    /// rail is one row of a list, and the list has to exist somewhere you can
    /// scroll it.
    PlaylistOpenFresh,
    PlaylistDelete { playlist_id: i64 },
    PlaylistAdd { playlist_id: i64, item_ids: Vec<i64> },
    PlaylistRemove { playlist_id: i64, item_id: i64 },
    PlaylistMove { playlist_id: i64, from: i64, to: i64 },
    SetEqPreset { name: String },
    SetEqBand { index: i64, gain_db: f64 },
    ToggleEq,
    /// One command for the six audio settings rather than six: they all land
    /// in the same `advanced` map and all mean "respawn mpv with new flags".
    SetAudio { key: String, value: String },
    FetchLyrics { item_id: i64 },
    LyricsOffset { delta_ms: i64 },
    ClearHistory,

    // --- Podcasts ---
    PodSetTab { name: String },
    PodSubscribe { url: String },
    PodUnsubscribe { podcast_id: i64 },
    PodOpen { podcast_id: i64 },
    PodBack,
    PodRefresh,
    PodRefreshOne { podcast_id: i64 },
    PodSetCat { name: String },
    PodSetPage { page: i64 },
    PodSetEpSort { mode: String },
    PodSetEpPage { page: i64 },
    PodPlay { episode_id: i64 },
    PodSkip { secs: f64 },
    PodSetSpeed { speed: f64 },
    PodDownload { episode_id: i64 },
    PodRemoveDownload { episode_id: i64 },
    PodClearDownloads,
    PodQueueAdd { episode_id: i64 },
    PodQueueRemove { episode_id: i64 },
    PodQueueClear,
    PodOpmlImport { path: String },
    PodOpmlExport { path: String },
    PodResetAll,
    PodSetHomeSort { mode: String },
    PodSetHomePage { page: i64 },
    PodToggleHome { podcast_id: i64 },
    PodSetDlSort { mode: String },
    PodTrendsLoad,
    PodSetTrendsSort { mode: String },
    PodSetTrendsPage { page: i64 },
    PodTrendSubscribe { feed_url: String },
    /// `podcast_id` < 0 means the card is a Trends entry; `feed_url` is then
    /// the only handle on it.
    PodInfoOpen { podcast_id: i64, feed_url: String },
    PodInfoClose,
    PodInfoSetCategory { name: String },
    PodSetThumb { podcast_id: i64, path: String },
    PodTranscript { episode_id: i64 },
    PodTranscriptClose,
    /// Filter the open tab. Stored per tab.
    PodSearch { query: String },

    // --- Audiobooks ---
    BookSetTab { name: String },
    BookOpen { folder: String },
    BookBack,
    BookPlay { item_id: i64 },
    BookChapter { delta: i64 },
    BookSetSpeed { speed: f64 },
    BookSetFinished { finished: bool },
    BookFlagFolder { folder: String, on: bool },
    BookmarkAdd { label: String },
    BookmarkJump { index: i64 },
    BookmarkRemove { index: i64 },
    BookmarkRename { index: i64, label: String },

    // --- Radio ---
    RadioSetTab { name: String },
    RadioSetGenre { index: i64 },
    RadioBack,
    RadioSetPage { page: i64 },
    RadioSetSort { mode: String },
    RadioPlay { index: i64 },
    RadioFav { index: i64 },
    RadioSearch { query: String },
    RadioRefresh,
    RadioClearRecent,
    RadioAdd { name: String, url: String },

    // --- YouTube ---
    YtSetTab { name: String },
    YtSearch { query: String },
    YtClearRecent,
    YtPlay { video_id: String },
    YtPlayLocal { media_path: String, video_id: String },
    YtDownload { video_id: String, quality: String },
    YtRemoveCached { video_id: String },
    YtRemoveDownload { video_id: String },
    YtClearCached,
    YtClearDownloads,
    YtSetDlPage { page: i64 },
    YtOpenChannel { channel_id: String },
    YtChannelBack,
    /// latest | popular
    YtChannelMode { mode: String },
    YtSubscribe { channel_id: String, title: String },
    YtUnsub { channel_id: String },
    YtSetSubsPage { page: i64 },
    YtRefreshSubs,
    YtImportSubs { path: String },
    YtCreatePlaylist { name: String },
    YtDeletePlaylist { playlist_id: i64 },
    YtOpenPlaylist { playlist_id: i64 },
    YtPlaylistBack,
    YtAddToPlaylist { playlist_id: i64, video_id: String },
    YtRemoveFromPlaylist { playlist_id: i64, video_id: String },
    /// Append the next page of hits to the current results.
    YtSearchMore,
    YtSetDlSort { mode: String },
    YtSetSubsSort { mode: String },
    YtToggleSubsDir,
    YtToggleSubsFilter,
    YtSetPlaylistSort { mode: String },
    /// Queue every video in the open playlist.
    YtPlaylistPlayAll { playlist_id: i64 },
    /// Subscribe by channel URL or @handle rather than by finding it first.
    YtAddChannelUrl { url: String },
    /// Import a YouTube playlist URL as a local playlist.
    YtImportPlaylistUrl { url: String },
    /// Remember a download height. 0 = best video, < 0 = audio only.
    YtSetDefaultRes { height: i64 },
    /// Forget it, so the picker comes back.
    YtResetVideoPrefs,
    YtPinHome { channel_id: String },
    YtUnpinHome { channel_id: String },
    /// auto | piped | ytdlp
    YtSetFetcher { name: String },
    /// Search inside the open channel. Empty query clears back to the listing.
    YtChannelSearch { query: String },
    /// Pull another 30 videos onto the open channel's listing.
    YtChannelLoadMore,
    /// Watch a video rather than hearing it. `height` follows the same spelling
    /// as the download picker: 0 = best, < 0 is meaningless here and is treated
    /// as best, anything else is a cap.
    YtWatch { video_id: String, height: i64 },
    /// Watch whatever is playing as audio right now, from where it has got to.
    YtWatchCurrent,
    /// Close the picture without waiting for the end of the video.
    YtStopWatching,
    YtToggleHomeConnect,
}

#[derive(Debug, Clone)]
pub enum MusicEvent {
    /// The player moved. Throttled to once a second in the mpv reader, which
    /// is as often as a seek bar and a clock can usefully change.
    Tick { pos: f64, dur: f64, playing: bool },
    /// Something started, stopped or was replaced — the snapshot the UI is
    /// holding is now wrong about more than the position.
    TrackChanged,
    /// The socket closed with this launch still current: a natural end, so the
    /// queue should advance. Dart answers with `Next` rather than Rust doing
    /// it, for the same reason the section polls rather than pushes state — one
    /// place decides what happens next, and it is the one holding the queue.
    Ended,
    /// Something the UI is already showing was filled in behind it -- an
    /// artist biography that had to be fetched, artwork that had to be
    /// resolved. The held snapshot is not wrong, only incomplete, so the answer
    /// is a plain Refresh rather than anything to do with the player.
    Stale,
    ScanProgress { root: String, done: i64, total: i64 },
    ScanFinished { inserted: i64, updated: i64, missing: i64 },
    Failed { message: String },

    /// Put this on the deck. `props` are mpv properties as `k=v`, in the order
    /// they must be applied — ReplayGain and gapless from the domain crate, the
    /// output device, the EQ chain, volume, mute, loop-file. `token` comes back
    /// on every report so a reply from the source this one replaced is dropped.
    AudioPlay { token: i64, src: String, start_at: f64, props: Vec<String> },
    /// Stop the deck. Also sent immediately before every `AudioPlay`.
    AudioStop,
    /// One mpv property. `value` is a JSON literal: `true`, `85`, `"inf"`.
    AudioProp { name: String, value: String },
    AudioSeek { secs: f64 },

    /// Watch a YouTube video in the in-app player — the same `VideoLayer` the
    /// Videos section uses, so a film and a music video are the one player.
    /// Audio is stopped first; when Dart reports the video ended, audio for the
    /// same video resumes where the picture left off.
    VideoPlay { token: i64, src: String, start_at: f64, props: Vec<String> },
    /// Close the picture. Also sent immediately before every `VideoPlay`.
    VideoStop,

    /// A command from outside the app: a media key, the desktop's media applet,
    /// a Bluetooth remote, or the tray menu.
    ///
    /// A name and a number rather than an arm each, because the far side of
    /// this is Dart and one string covers every command MPRIS defines. Rust
    /// does not act on it: the deck lives in Dart now, so the answer is a
    /// `MusicCmd` sent from there, exactly as if a button had been clicked.
    ///
    /// toggle · play · pause · stop · next · prev · shuffle · repeat ·
    /// seek (absolute seconds) · seekby (relative seconds, signed) ·
    /// volume (0..130) · raise · mini · quit
    Remote { action: String, value: f64 },
}

// --------------------------------------------------------------- session ----

/// Everything the view is currently looking at. Lives in Rust because every
/// command has to re-query against it, and a snapshot that took its filters
/// from Dart would need the whole of the above sent back on every keystroke.
///
/// `frb(ignore)`: private state, not part of the contract.
#[frb(ignore)]
#[derive(Debug, Clone)]
struct Session {
    view: String,
    query: String,
    lib_tab: String,
    song_sort: String,
    song_dir: String,
    song_page: i64,
    browse_sort: String,
    browse_dir: String,
    browse_page: i64,

    detail_open: bool,
    detail_kind: String,
    detail_id: i64,
    detail_key: String,

    shuffle: bool,
    repeat: String,
    sleep_min: i64,
    sleep_deadline: Option<std::time::Instant>,
    lyrics_offset_ms: i64,
    /// The order Prev walks back through. Not the queue: under shuffle the
    /// queue order and the played order are different, and Prev means the
    /// latter.
    history: Vec<i64>,

    pod_tab: String,
    pod_cat: String,
    pod_page: i64,
    pod_open: i64,
    pod_ep_sort: String,
    pod_ep_page: i64,
    pod_speed: f64,
    pod_queue: Vec<i64>,
    pod_home_sort: String,
    pod_home_page: i64,
    pod_dl_sort: String,
    pod_trends_sort: String,
    pod_trends_page: i64,
    /// The built directory, cached for the session. Rebuilding it is a network
    /// walk over three dozen feeds; the persisted half is in `podcast_trends`.
    pod_trends: Vec<tulipix_music::pod_trends::TrendMeta>,
    pod_trends_loading: bool,
    pod_info: Option<PodInfo>,
    pod_transcript: Option<(String, String)>,
    /// tab name -> its filter.
    pod_queries: HashMap<String, String>,

    book_tab: String,
    book_open: String,

    radio_tab: String,
    radio_cat_open: bool,
    radio_cat_title: String,
    radio_page: i64,
    radio_sort: String,
    radio_sort_dir: String,
    /// Search results and category listings are network answers, not rows in a
    /// table: they are held here so a repaint does not re-fetch them.
    radio_list: Vec<tulipix_music::radio::Station>,

    yt_tab: String,
    yt_results: Vec<YtVideo>,
    yt_dl_page: i64,
    yt_subs_page: i64,
    yt_channel_open: bool,
    yt_channel_id: String,
    yt_channel_title: String,
    yt_channel_avatar: String,
    yt_channel_mode: String,
    yt_channel_subscribed: bool,
    yt_channel_videos: Vec<YtVideo>,
    yt_playlist_open: bool,
    yt_playlist_id: i64,
    yt_playlist_title: String,
    /// YouTube has no `play_queue` table — the ids are not `items` rows — so
    /// its queue lives here for as long as the app does. Losing it on restart
    /// is the same behaviour the Slint build has.
    yt_queue: Vec<String>,
    yt_queue_pos: i64,
    yt_status: String,
    yt_recommended: Vec<YtVideo>,
    yt_results_more: bool,
    yt_search_q: String,
    yt_dl_sort: String,
    yt_subs_sort: String,
    yt_subs_dir: String,
    yt_subs_filter: String,
    yt_playlist_sort: String,
    yt_channel_query: String,
    yt_channel_results: Vec<YtVideo>,
    yt_channel_page: i64,
    yt_channel_has_next: bool,
    yt_playing_pl_id: i64,

    mgr_open: bool,
    mgr_tab: String,
    mgr_mode: String,
    mgr_filter: String,
    mgr_page: i64,
    mgr_item_id: i64,
    mgr_title: String,
    mgr_q_name: String,
    mgr_q_artist: String,
    mgr_q_album: String,
    mgr_searching: bool,
    mgr_results: Vec<LyricHit>,
    /// The raw content behind each result, parallel to `mgr_results`. Kept so
    /// picking one is a preview rather than a second network round trip, and
    /// so Save has something to write.
    mgr_bodies: Vec<(bool, String)>,
    mgr_picked: Option<usize>,
    mgr_status: String,
    /// A bulk fetch is running. Separate from `mgr_status`, which also carries
    /// one-shot notes like "Saved." that nothing is waiting on.
    mgr_busy: bool,

    status: String,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            view: "mymusic".into(),
            query: String::new(),
            lib_tab: "home".into(),
            song_sort: "title".into(),
            song_dir: "asc".into(),
            song_page: 0,
            browse_sort: "name".into(),
            browse_dir: "asc".into(),
            browse_page: 0,
            detail_open: false,
            detail_kind: "album".into(),
            detail_id: 0,
            detail_key: String::new(),
            shuffle: false,
            repeat: "off".into(),
            sleep_min: 0,
            sleep_deadline: None,
            lyrics_offset_ms: 0,
            history: Vec::new(),
            pod_tab: "home".into(),
            pod_cat: "All".into(),
            pod_page: 0,
            pod_open: -1,
            pod_ep_sort: "new".into(),
            pod_ep_page: 0,
            pod_speed: 1.0,
            pod_queue: Vec::new(),
            pod_home_sort: "name".into(),
            pod_home_page: 0,
            pod_dl_sort: "dl".into(),
            pod_trends_sort: "name".into(),
            pod_trends_page: 0,
            pod_trends: Vec::new(),
            pod_trends_loading: false,
            pod_info: None,
            pod_transcript: None,
            pod_queries: HashMap::new(),
            book_tab: "all".into(),
            book_open: String::new(),
            radio_tab: "home".into(),
            radio_cat_open: false,
            radio_cat_title: String::new(),
            radio_page: 0,
            radio_sort: "default".into(),
            radio_sort_dir: "desc".into(),
            radio_list: Vec::new(),
            yt_tab: "home".into(),
            yt_results: Vec::new(),
            yt_dl_page: 0,
            yt_subs_page: 0,
            yt_channel_open: false,
            yt_channel_id: String::new(),
            yt_channel_title: String::new(),
            yt_channel_avatar: String::new(),
            yt_channel_mode: "latest".into(),
            yt_channel_subscribed: false,
            yt_channel_videos: Vec::new(),
            yt_playlist_open: false,
            yt_playlist_id: 0,
            yt_playlist_title: String::new(),
            yt_queue: Vec::new(),
            yt_queue_pos: 0,
            yt_status: String::new(),
            yt_recommended: Vec::new(),
            yt_results_more: false,
            yt_search_q: String::new(),
            yt_dl_sort: "new".into(),
            yt_subs_sort: "subscribers".into(),
            yt_subs_dir: "desc".into(),
            yt_subs_filter: tulipix_music::yt_prefs::subs_filter(),
            yt_playlist_sort: "default".into(),
            yt_channel_query: String::new(),
            yt_channel_results: Vec::new(),
            yt_channel_page: 1,
            yt_channel_has_next: false,
            yt_playing_pl_id: -1,
            mgr_open: false,
            mgr_tab: "lyrics".into(),
            mgr_mode: "list".into(),
            mgr_filter: "all".into(),
            mgr_page: 0,
            mgr_item_id: 0,
            mgr_title: String::new(),
            mgr_q_name: String::new(),
            mgr_q_artist: String::new(),
            mgr_q_album: String::new(),
            mgr_searching: false,
            mgr_results: Vec::new(),
            mgr_bodies: Vec::new(),
            mgr_picked: None,
            mgr_status: String::new(),
            mgr_busy: false,
            status: String::new(),
        }
    }
}

fn session() -> &'static Mutex<Session> {
    static S: std::sync::OnceLock<Mutex<Session>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(Session::default()))
}

fn lock() -> std::sync::MutexGuard<'static, Session> {
    // A poisoned lock here means a previous command panicked mid-mutation. The
    // section is a view over a database, not a bank ledger: the recoverable
    // answer is to keep the (possibly stale) view rather than take the app
    // down on the next repaint.
    session().lock().unwrap_or_else(|e| e.into_inner())
}

fn events() -> &'static Mutex<Vec<StreamSink<MusicEvent>>> {
    static E: std::sync::OnceLock<Mutex<Vec<StreamSink<MusicEvent>>>> = std::sync::OnceLock::new();
    E.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn emit(event: MusicEvent) {
    if let Ok(mut sinks) = events().lock() {
        // `add` fails once Dart has cancelled the subscription; dropping those
        // here is the only place they get collected.
        sinks.retain(|s| s.add(event.clone()).is_ok());
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ------------------------------------------------------------- exported ----

/// Apply one command and return the resulting snapshot. Every mutation goes
/// through here, so Dart never has to guess what a write did to the view.
pub async fn music_dispatch(cmd: MusicCmd) -> Result<MusicState> {
    lock().status.clear();
    if let Err(e) = apply(cmd).await {
        tracing::warn!(error = %e, "music command failed");
        lock().status = e.to_string();
        emit(MusicEvent::Failed { message: e.to_string() });
    }
    snapshot().await
}

/// Dart reports the picture closed — end of file, Escape, or the close button.
///
/// `pos` is where the video got to, and it is the whole reason this exists:
/// audio resumes there, so switching to the picture and back does not lose
/// your place. A report for a token that is no longer current is a reply from
/// a video this one already replaced, and is dropped.
pub async fn music_video_ended(token: i64, pos: f64, dur: f64) {
    let id = {
        let mut g = match yt_watch().lock() {
            Ok(g) => g,
            Err(_) => return,
        };
        if g.0 != token {
            return;
        }
        let id = g.1.clone();
        *g = (0, String::new());
        id
    };
    if id.is_empty() {
        return;
    }
    // Past the end is a finished video, not a place to resume from.
    let resume = if dur > 0.0 && pos >= dur - 1.0 { 0.0 } else { pos };
    if let Ok(pool) = youtube_pool().await {
        let _ = tulipix_music::youtube::store::save_progress(pool, &id, resume, dur).await;
    }
    // `play_youtube` reads `progress_of` itself, so writing the position above
    // IS the handoff -- seeking again here would fight it.
    if let Err(e) = play_youtube(&id).await {
        tracing::warn!(error = %e, video = %id, "could not resume audio after the video");
    }
}

/// Player ticks, track changes, end-of-file and scan progress. Registering a
/// second sink is harmless — every sink gets every event.
#[frb(sync)]
pub fn music_events(sink: StreamSink<MusicEvent>) {
    if let Ok(mut sinks) = events().lock() {
        sinks.push(sink);
    }
    // Here rather than at process start: the surfaces exist to be driven, and
    // remote commands arrive as events — registering before there is a sink to
    // deliver them to would drop the first ones. Idempotent.
    crate::shellsurface::start();
}

/// Kill the player. Synchronous, and called from the window's exit handler for
/// the same reason `transfer_shutdown` is: process exit would stop the audio
/// too, but not before the speakers have had a second of it after the window
/// went away.
#[frb(sync)]
pub fn music_shutdown() {
    mpv::stop();
}

// ---------------------------------------------------------- from the deck ---
//
// Bare functions rather than `MusicCmd` arms, and `sync` rather than async: a
// dispatch returns a whole `MusicState`, and the deck reports about once a
// second. Paying for a snapshot — every card, every queue row — per tick is the
// cost this shape exists to avoid. What the reports change is read by the next
// snapshot something else asks for.

/// Where playback got to. Sent about once a second, and whenever
/// pause/volume/mute/title changes. `title` is mpv's `media-title`, which for a
/// radio stream is the ICY title and the only place the actual song is
/// readable.
#[frb(sync)]
pub fn music_audio_tick(
    token: i64,
    pos: f64,
    dur: f64,
    paused: bool,
    volume: f64,
    muted: bool,
    title: String,
) {
    mpv::report(token, pos, dur, paused, volume, muted, title);
}

/// End of source. This is what advances the queue — it runs the `on_eof` the
/// launch registered, which emits `Ended`, which Dart answers with `Next`.
#[frb(sync)]
pub fn music_audio_ended(token: i64) {
    mpv::ended(token);
}

/// The source could not be opened — a file that moved, a codec that is not
/// there, a stream that refused.
#[frb(sync)]
pub fn music_audio_failed(token: i64, message: String) {
    mpv::failed(token, message);
}

/// Resolve artwork for one thing, rendering it if that is what it takes, and
/// return an absolute path.
///
/// `kind` is track | album | artist | folder | book | podcast | yt.
/// Called by the tile when it scrolls into view, so a cold library paints
/// progressively rather than blocking the first snapshot on thousands of
/// ffmpeg invocations — the same lazy pass `photos_ensure_thumb` runs.
/// Everything the Properties row of the song menu shows.
///
/// Not a command: `music_dispatch` answers with the whole section snapshot, and
/// none of this belongs on it — it is read once, for one song, when a menu row
/// is clicked. Same shape as `music_ensure_art`, for the same reason.
///
/// `bpm`, `key` and `dr` are whatever the Slint build's analysis pass wrote;
/// they stay empty here rather than being computed, because the analyser lives
/// in a crate that pulls in Slint and this one may not link it.
#[derive(Debug, Clone)]
pub struct SongProps {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub genre: String,
    pub release: String,
    pub credits: String,
    pub duration: String,
    pub format: String,
    /// "FLAC · 981 kbps · 44.1 kHz · stereo", as far as the tags know.
    pub stream: String,
    pub size: String,
    pub path: String,
    /// "128 BPM · A minor · DR 9", or "" when nothing has analysed it.
    pub analysis: String,
    pub has_video: bool,
}

pub async fn music_song_props(item_id: i64) -> Result<SongProps> {
    let pool = music_pool().await?;
    let path = track_path(pool, item_id).await.unwrap_or_default();
    let p = Path::new(&path);
    let bytes = std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);

    // Artist and album are ids on `track_meta`, not strings — the same join
    // every other query here does.
    #[allow(clippy::type_complexity)]
    let row: Option<(String, String, String, f64, String, i64, i64, i64, String)> =
        sqlx::query_as(
            "SELECT COALESCE(tm.title, ''), COALESCE(ar.name, ''), COALESCE(al.title, ''), \
                    COALESCE(tm.duration_s, 0.0), COALESCE(tm.genre, ''), \
                    COALESCE(tm.bitrate, 0), COALESCE(tm.sample_rate, 0), \
                    COALESCE(tm.channels, 0), COALESCE(tm.codec, '') \
             FROM track_meta tm \
             LEFT JOIN artists ar ON ar.id = tm.artist_id \
             LEFT JOIN albums al ON al.id = tm.album_id \
             WHERE tm.item_id = ?",
        )
        .bind(item_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let (title, artist, album, duration_s, genre, bitrate, sample_rate, channels, codec) =
        row.unwrap_or_default();

    // Two columns the Slint build adds on demand rather than in a migration, so
    // both builds have to assume they may not be there yet.
    for sql in [
        "ALTER TABLE track_meta ADD COLUMN release_date TEXT",
        "ALTER TABLE track_meta ADD COLUMN credits TEXT",
    ] {
        let _ = sqlx::query(sql).execute(pool).await;
    }
    let extra: Option<(String, String, Option<f64>, String, Option<f64>)> = sqlx::query_as(
        "SELECT COALESCE(release_date, ''), COALESCE(credits, ''), bpm, \
                COALESCE(music_key, ''), dr_score \
         FROM track_meta WHERE item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let (release, credits, bpm, key, dr) = extra.unwrap_or_default();

    let mut parts: Vec<String> = Vec::new();
    if let Some(b) = bpm {
        parts.push(format!("{b:.0} BPM"));
    }
    if !key.is_empty() {
        parts.push(key);
    }
    if let Some(d) = dr {
        parts.push(format!("DR {d:.0}"));
    }

    let mut stream: Vec<String> = Vec::new();
    if !codec.is_empty() {
        stream.push(codec.to_uppercase());
    }
    if bitrate > 0 {
        stream.push(format!("{} kbps", bitrate / 1000));
    }
    if sample_rate > 0 {
        stream.push(format!("{:.1} kHz", sample_rate as f64 / 1000.0));
    }
    if channels > 0 {
        stream.push(match channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} ch"),
        });
    }

    Ok(SongProps {
        title,
        artist,
        album,
        genre,
        release,
        credits,
        duration: if duration_s > 0.0 { fmt_clock(duration_s) } else { String::new() },
        format: p
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_uppercase(),
        stream: stream.join(" · "),
        size: human_size(bytes),
        path,
        analysis: parts.join(" · "),
        has_video: tulipix_music::video_link::has_video(pool, item_id)
            .await
            .unwrap_or(false),
    })
}

pub async fn music_ensure_art(kind: String, key: String) -> Result<Option<String>> {
    match kind.as_str() {
        "track" => {
            let Ok(item_id) = key.parse::<i64>() else { return Ok(None) };
            let pool = music_pool().await?;
            let path: Option<String> =
                sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                    .bind(item_id)
                    .fetch_optional(pool)
                    .await?;
            let Some(path) = path else { return Ok(None) };
            Ok(art_for_file(Path::new(&path)))
        }
        "album" => {
            let Ok(id) = key.parse::<i64>() else { return Ok(None) };
            let pool = music_pool().await?;
            let cover: Option<String> =
                sqlx::query_scalar("SELECT cover_path FROM albums WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?
                    .flatten();
            if let Some(c) = cover.filter(|c| Path::new(c).exists()) {
                return Ok(Some(c));
            }
            // No stored cover: fall back to the first track's, which is what
            // gives an album a face on a library that was never tagged.
            let path: Option<String> = sqlx::query_scalar(
                "SELECT i.abs_path FROM items i JOIN track_meta tm ON tm.item_id = i.id \
                 WHERE tm.album_id = ? AND i.missing_since IS NULL \
                 ORDER BY COALESCE(tm.track_no, 0) LIMIT 1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            let Some(path) = path else { return Ok(None) };
            let found = art_for_file(Path::new(&path));
            if let Some(p) = &found {
                // Remember it, so the next paint of this grid is one SELECT
                // rather than one ffmpeg per tile.
                let _ = sqlx::query("UPDATE albums SET cover_path = ? WHERE id = ?")
                    .bind(p)
                    .bind(id)
                    .execute(pool)
                    .await;
            }
            Ok(found)
        }
        "artist" => {
            let Ok(id) = key.parse::<i64>() else { return Ok(None) };
            let pool = music_pool().await?;
            let stored: Option<String> =
                sqlx::query_scalar("SELECT image_path FROM artists WHERE id = ?")
                    .bind(id)
                    .fetch_optional(pool)
                    .await?
                    .flatten();
            if let Some(c) = stored.filter(|c| Path::new(c).exists()) {
                return Ok(Some(c));
            }
            let album: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM albums WHERE artist_id = ? AND cover_path IS NOT NULL LIMIT 1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            match album {
                Some(a) => Box::pin(music_ensure_art("album".into(), a.to_string())).await,
                None => Ok(None),
            }
        }
        // A playlist has no cover of its own, so it wears the newest thing in
        // it — which also means the tile changes as the playlist does.
        "playlist" => {
            let Ok(id) = key.parse::<i64>() else { return Ok(None) };
            let pool = music_pool().await?;
            let newest: Option<i64> = sqlx::query_scalar(
                "SELECT item_id FROM playlist_items WHERE playlist_id = ? \
                 ORDER BY position DESC LIMIT 1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            match newest {
                Some(item) => {
                    Box::pin(music_ensure_art("track".into(), item.to_string())).await
                }
                None => Ok(None),
            }
        }
        // Same borrowing for a genre, from the first track tagged with it.
        "genre" => {
            let pool = music_pool().await?;
            let first: Option<i64> = sqlx::query_scalar(
                "SELECT tm.item_id FROM track_meta tm JOIN items i ON i.id = tm.item_id \
                 WHERE tm.genre = ? AND i.missing_since IS NULL \
                 ORDER BY tm.item_id LIMIT 1",
            )
            .bind(&key)
            .fetch_optional(pool)
            .await?;
            match first {
                Some(item) => {
                    Box::pin(music_ensure_art("track".into(), item.to_string())).await
                }
                None => Ok(None),
            }
        }
        "folder" => Ok(folder_cover(Path::new(&key))),
        // A book's cover is the same chain the shelf card uses: a stored /
        // net-resolved choice, a sidecar image, then the first chapter's
        // embedded art. `folder_cover` alone is why a tagged rip with no loose
        // jpg drew the placeholder here while its card had a picture.
        "book" => {
            let pool = music_pool().await?;
            let stored: Option<String> =
                sqlx::query_scalar("SELECT path FROM audiobook_covers WHERE folder = ?")
                    .bind(&key)
                    .fetch_optional(pool)
                    .await
                    .unwrap_or_default();
            Ok(book_cover(pool, &key, stored.as_deref()).await)
        }
        "yt" | "podcast" => {
            // A remote URL cached to disk once. Dart could fetch these itself,
            // but then two builds would hold two copies of the same artwork in
            // two caches, and the Slint one is already on disk.
            Ok(cache_remote(&key).await)
        }
        _ => Ok(None),
    }
}

// -------------------------------------------------------------- artwork ----

fn art_cache_dir() -> PathBuf {
    let dir = tulipix_core::paths::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("music_art");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// A stable, filesystem-safe name for an arbitrary key. Not a hash for
/// prettiness — a URL contains slashes and a path contains both.
fn art_key(src: &str) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in src.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// Cover for one audio file: the folder's `cover.jpg` if there is one,
/// otherwise the picture embedded in the file, extracted once with ffmpeg.
fn art_for_file(path: &Path) -> Option<String> {
    if let Some(parent) = path.parent() {
        if let Some(c) = folder_cover(parent) {
            return Some(c);
        }
    }
    let dest = art_cache_dir().join(format!("{}.jpg", art_key(&path.to_string_lossy())));
    if dest.exists() {
        return Some(dest.to_string_lossy().into_owned());
    }
    // `-an` and the attached-pic map: without them ffmpeg happily writes the
    // whole audio stream into a .jpg and the tile shows a broken image.
    let status = std::process::Command::new(tulipix_core::thumbs::tool_bin("ffmpeg"))
        .args(["-v", "error", "-y", "-i"])
        .arg(path)
        .args(["-an", "-vcodec", "copy"])
        .arg(&dest)
        .no_window_compat()
        .status();
    match status {
        Ok(s) if s.success() && dest.exists() => Some(dest.to_string_lossy().into_owned()),
        _ => {
            // Leave nothing behind: a zero-byte file here would be returned
            // forever by the `dest.exists()` check above.
            let _ = std::fs::remove_file(&dest);
            None
        }
    }
}

/// `tulipix_music::folders::pick_cover` over a real directory listing.
fn folder_cover(dir: &Path) -> Option<String> {
    let names: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
        .collect();
    let picked = tulipix_music::folders::pick_cover(&names)?;
    Some(dir.join(picked).to_string_lossy().into_owned())
}

/// Download a remote artwork URL once and hand back the local path. A key that
/// is already a local file is passed straight through — podcast shows can carry
/// a user-picked thumbnail rather than a feed URL.
async fn cache_remote(src: &str) -> Option<String> {
    if src.is_empty() {
        return None;
    }
    let as_path = Path::new(src);
    if as_path.is_file() {
        return Some(src.to_string());
    }
    if !src.starts_with("http") {
        return None;
    }
    let ext = src
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 4 && !e.contains('/'))
        .unwrap_or("jpg");
    let dest = art_cache_dir().join(format!("{}.{ext}", art_key(src)));
    if dest.exists() {
        return Some(dest.to_string_lossy().into_owned());
    }
    let bytes = tulipix_core::net::http()
        .get(src)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await
        .ok()?
        .bytes()
        .await
        .ok()?;
    std::fs::write(&dest, &bytes).ok()?;
    Some(dest.to_string_lossy().into_owned())
}

/// `NoWindow` for `std::process::Command`, spelled locally so this module does
/// not have to import the trait at every call site.
trait NoWindowCompat {
    fn no_window_compat(&mut self) -> &mut Self;
}

impl NoWindowCompat for std::process::Command {
    fn no_window_compat(&mut self) -> &mut Self {
        use tulipix_core::proc::NoWindow;
        self.no_window()
    }
}

// ------------------------------------------------------------- settings ----
//
// The audio knobs live in `settings.json`'s `advanced` map under `music.*`,
// which is where the Slint build put them. Same keys, same values, so a
// crossfade set in one build is in force in the other.

/// The whole `advanced` map at once.
///
/// [`setting`] re-reads settings.json on every call, which is fine for the six
/// audio knobs and not fine for a grid: a library with fifty genres was fifty
/// loads of the same file per snapshot. One load, then lookups.
fn settings_map() -> HashMap<String, String> {
    tulipix_core::settings::Settings::load()
        .map(|s| s.advanced)
        .unwrap_or_default()
}

/// A cover that was chosen by hand, if it is still where it was left.
fn pref_cover(prefs: &HashMap<String, String>, key: &str) -> String {
    prefs
        .get(key)
        .filter(|p| !p.is_empty() && Path::new(p.as_str()).exists())
        .cloned()
        .unwrap_or_default()
}

fn setting(key: &str, default: &str) -> String {
    tulipix_core::settings::Settings::load()
        .ok()
        .and_then(|s| s.advanced.get(key).cloned())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn set_setting(key: &str, value: &str) {
    let Ok(mut s) = tulipix_core::settings::Settings::load() else { return };
    s.advanced.insert(key.to_string(), value.to_string());
    if let Err(e) = s.save() {
        tracing::warn!(error = %e, key, "settings save failed");
    }
}

fn load_eq() -> tulipix_music::eq::Equalizer {
    let mut eq = tulipix_music::eq::Equalizer::default();
    let raw = setting("music.eq.bands", "");
    for (i, part) in raw.split(',').enumerate().take(10) {
        if let Ok(v) = part.trim().parse::<f64>() {
            eq.gains_db[i] = v;
        }
    }
    eq.enabled = setting("music.eq.on", "0") == "1";
    eq.clamp();
    eq
}

fn save_eq(eq: &tulipix_music::eq::Equalizer) {
    let joined = eq
        .gains_db
        .iter()
        .map(|g| format!("{g}"))
        .collect::<Vec<_>>()
        .join(",");
    set_setting("music.eq.bands", &joined);
    set_setting("music.eq.on", if eq.enabled { "1" } else { "0" });
}

/// The mpv flags for one launch: the domain crate's ReplayGain/gapless option
/// set, the domain crate's device options, this session's EQ filter graph, and
/// the volume/mute/loop the transport owns.
fn audio_args(repeat_one: bool) -> Vec<String> {
    use tulipix_music::player::{AudioConfig, ReplayGainMode};
    let cfg = AudioConfig {
        gapless: setting("music.gapless", "1") != "0",
        crossfade_s: setting("music.crossfade", "0").parse().unwrap_or(0.0),
        replaygain: match setting("music.replaygain", "off").as_str() {
            "track" => ReplayGainMode::Track,
            "album" => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        },
        preamp_db: setting("music.preamp", "0")
            .parse::<f64>()
            .unwrap_or(0.0)
            .clamp(-12.0, 12.0),
    };
    let mut args = cfg.mpv_options();
    args.extend(tulipix_music::output_device::device_options(
        &setting("music.device", "auto"),
        setting("music.exclusive", "0") == "1",
    ));
    args.push(format!("--af={}", full_af(&load_eq().mpv_af())));
    args.push(format!("--volume={}", setting("music.volume", "80")));
    if setting("music.muted", "0") == "1" {
        args.push("--mute=yes".into());
    }
    if repeat_one {
        // mpv loops the file itself, so EOF never fires and the queue never
        // advances — which is exactly what repeat-one means.
        args.push("--loop-file=inf".into());
    }
    args
}

/// Enumerate output devices through mpv, exactly as the Slint build does.
/// Cached for the session: it is a process spawn, and the answer only changes
/// when hardware is plugged in.
fn output_devices() -> Vec<String> {
    static D: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    D.get_or_init(|| {
        let out = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"))
            .arg("--audio-device=help")
            .no_window_compat()
            .output();
        let mut devs = match out {
            Ok(o) => tulipix_music::output_device::parse_device_list(&String::from_utf8_lossy(
                &o.stdout,
            )),
            Err(_) => Vec::new(),
        };
        if devs.is_empty() {
            devs.push(tulipix_music::output_device::default_device());
        }
        devs.into_iter().map(|d| d.id).collect()
    })
    .clone()
}

// ------------------------------------------------------------- playback ----

/// Hand a source to mpv and wire its two callbacks. Every play in every tab
/// goes through here, which is what makes the five tabs one player: the
/// singleton in `mpv` can only hold one process, so starting anything stops
/// whatever else was going.
fn launch(src: &str, slot: mpv::Slot, start_s: Option<f64>) {
    let repeat_one = lock().repeat == "one";
    let args = audio_args(repeat_one);
    mpv::play(
        src,
        slot,
        &args,
        start_s,
        |obs| {
            emit(MusicEvent::Tick {
                pos: obs.pos,
                dur: obs.dur,
                playing: !obs.paused,
            });
        },
        || emit(MusicEvent::Ended),
    );
    emit(MusicEvent::TrackChanged);
}

/// Write the real listening time onto the play that is ending.
///
/// `record_play` stamps a row the moment a track starts, and stamps it with an
/// `ms_played` of zero — at the start nothing has been listened to yet. Nothing
/// ever wrote the figure back, in either build, so the Home strip's "This week"
/// and "All time" have always summed a column of zeroes and shown 0m. The last
/// position the deck reported for the outgoing track is that figure.
///
/// ponytail: track change only. A seek counts as listening, and the last track
/// of a session is closed out by the next play rather than at stop --
/// `music_shutdown` is sync and cannot reach the pool. Accumulating real
/// playing time across pauses and seeks would want a counter on the tick.
async fn close_out_play(pool: &sqlx::SqlitePool) {
    let prev = mpv::now_playing().item_id;
    let played_ms = (mpv::observe().pos * 1000.0).round() as i64;
    if prev <= 0 || played_ms <= 0 {
        return;
    }
    let _ = sqlx::query(
        "UPDATE play_history SET ms_played = ? \
         WHERE id = (SELECT MAX(id) FROM play_history WHERE item_id = ?)",
    )
    .bind(played_ms)
    .bind(prev)
    .execute(pool)
    .await;
}

/// Play one library track and make it the now-playing.
async fn play_track(pool: &sqlx::SqlitePool, item_id: i64) -> Result<()> {
    close_out_play(pool).await;
    let row: Option<(String, String, String, String)> = sqlx::query_as(
        "SELECT i.abs_path, COALESCE(tm.title, ''), COALESCE(ar.name, ''), COALESCE(al.title, '') \
         FROM items i JOIN track_meta tm ON tm.item_id = i.id \
         LEFT JOIN artists ar ON ar.id = tm.artist_id \
         LEFT JOIN albums al ON al.id = tm.album_id \
         WHERE i.id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((path, title, artist, album)) = row else {
        anyhow::bail!("track {item_id} is not in the library");
    };
    if !Path::new(&path).exists() {
        anyhow::bail!("file is missing: {path}");
    }
    let art = art_for_file(Path::new(&path)).unwrap_or_default();
    let display = if title.is_empty() {
        Path::new(&path)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    } else {
        title
    };
    // Before `launch`, not after: launch emits TrackChanged and Dart answers
    // that with a Refresh, so the row has to be down before the event goes out
    // or that snapshot reads the rails without the track that just started.
    //
    // Counted at the start rather than at EOF: a skipped track is still a track
    // that was chosen, and the Slint build records it the same way.
    let _ = tulipix_music::queue::record_play(pool, item_id, 0).await;
    launch(&path, mpv::Slot::Music, None);
    mpv::set_now_playing(mpv::NowPlaying {
        item_id,
        title: display,
        artist,
        album,
        art,
        key: item_id.to_string(),
    });
    {
        let mut s = lock();
        // Prev walks the played order, not the queue order — under shuffle
        // those are different, and Prev means "the one I just heard".
        if s.history.last() != Some(&item_id) {
            s.history.push(item_id);
        }
        if s.history.len() > 200 {
            s.history.remove(0);
        }
    }
    // Words follow the track, without being asked. Detached: an LRCLIB round
    // trip is network latency and the track is already playing.
    tokio::spawn(ensure_lyrics(item_id));
    Ok(())
}

/// The queue as ids, in order.
async fn queue_ids(pool: &sqlx::SqlitePool) -> Vec<i64> {
    tulipix_music::queue::list(pool).await.unwrap_or_default()
}

/// Pseudo-random index for shuffle. No `rand` dependency for one draw a track:
/// the nanosecond field of the clock is as unpredictable as this needs to be,
/// and it is what the Slint build uses for the same purpose.
fn rand_index(len: usize, avoid: usize) -> usize {
    if len <= 1 {
        return 0;
    }
    let n = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    let mut i = n % len;
    if i == avoid {
        i = (i + 1) % len;
    }
    i
}

/// Advance (or step back) in whichever queue the running slot owns.
async fn step(pool: &sqlx::SqlitePool, forward: bool) -> Result<()> {
    match mpv::current_slot() {
        mpv::Slot::Podcast => return step_podcast(forward).await,
        mpv::Slot::Book => return step_chapter(pool, forward).await,
        mpv::Slot::Radio => return step_station(forward).await,
        mpv::Slot::Youtube => return step_youtube(forward).await,
        _ => {}
    }

    let ids = queue_ids(pool).await;
    if ids.is_empty() {
        mpv::stop();
        return Ok(());
    }
    let (shuffle, repeat) = {
        let s = lock();
        (s.shuffle, s.repeat.clone())
    };
    if repeat == "one" && forward {
        let cur = mpv::now_playing().item_id;
        if cur != 0 {
            return play_track(pool, cur).await;
        }
    }
    if !forward {
        // Prev: the played order, which under shuffle is not the queue order.
        let prev = {
            let mut s = lock();
            s.history.pop();
            s.history.last().copied()
        };
        if let Some(id) = prev {
            return play_track(pool, id).await;
        }
    }
    let cur = mpv::now_playing().item_id;
    let at = ids.iter().position(|i| *i == cur).unwrap_or(0);
    let next = if shuffle && forward {
        rand_index(ids.len(), at)
    } else if forward {
        if at + 1 >= ids.len() {
            if repeat != "all" {
                mpv::stop();
                return Ok(());
            }
            0
        } else {
            at + 1
        }
    } else if at == 0 {
        if repeat != "all" {
            return Ok(());
        }
        ids.len() - 1
    } else {
        at - 1
    };
    play_track(pool, ids[next]).await
}

async fn step_podcast(forward: bool) -> Result<()> {
    let (queue, cur) = {
        let s = lock();
        (s.pod_queue.clone(), mpv::now_playing().key)
    };
    if queue.is_empty() {
        mpv::stop();
        return Ok(());
    }
    let cur_id = cur.parse::<i64>().unwrap_or(-1);
    let at = queue.iter().position(|i| *i == cur_id).unwrap_or(0);
    let next = if forward { at + 1 } else { at.wrapping_sub(1) };
    let Some(id) = queue.get(next).copied() else {
        mpv::stop();
        return Ok(());
    };
    play_episode(id).await
}

async fn step_chapter(pool: &sqlx::SqlitePool, forward: bool) -> Result<()> {
    let folder = lock().book_open.clone();
    if folder.is_empty() {
        return Ok(());
    }
    let ids = tulipix_music::audiobooks::book_chapters(pool, &folder).await?;
    let cur = mpv::now_playing().item_id;
    let at = ids.iter().position(|i| *i == cur).unwrap_or(0);
    let next = if forward {
        at + 1
    } else if at == 0 {
        return Ok(());
    } else {
        at - 1
    };
    let Some(id) = ids.get(next).copied() else {
        mpv::stop();
        return Ok(());
    };
    play_chapter(pool, id).await
}

async fn step_station(forward: bool) -> Result<()> {
    let (list, cur) = {
        let s = lock();
        (s.radio_list.clone(), mpv::now_playing().key)
    };
    if list.is_empty() {
        return Ok(());
    }
    let at = list.iter().position(|s| s.stationuuid == cur).unwrap_or(0);
    let next = if forward {
        (at + 1) % list.len()
    } else {
        (at + list.len() - 1) % list.len()
    };
    play_station(next).await
}

async fn step_youtube(forward: bool) -> Result<()> {
    let (queue, at) = {
        let s = lock();
        (s.yt_queue.clone(), s.yt_queue_pos as usize)
    };
    if queue.is_empty() {
        mpv::stop();
        return Ok(());
    }
    let next = if forward {
        if at + 1 >= queue.len() {
            mpv::stop();
            return Ok(());
        }
        at + 1
    } else if at == 0 {
        return Ok(());
    } else {
        at - 1
    };
    lock().yt_queue_pos = next as i64;
    let id = queue[next].clone();
    play_youtube(&id).await
}

/// Where offline podcast episodes and YouTube media live. Same directories the
/// Slint build uses, so a file saved in one build plays in the other.
fn podcast_offline_dir() -> PathBuf {
    let d = tulipix_core::paths::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("podcast_offline");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn yt_media_dir() -> PathBuf {
    let d = tulipix_core::paths::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("youtube_cache");
    let _ = std::fs::create_dir_all(&d);
    d
}

fn yt_download_dir() -> PathBuf {
    let d = tulipix_core::paths::data_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("youtube_downloads");
    let _ = std::fs::create_dir_all(&d);
    d
}

async fn play_episode(episode_id: i64) -> Result<()> {
    let pool = podcasts_pool().await?;
    let row: Option<(String, String, f64, String, String, String)> = sqlx::query_as(
        "SELECT e.title, e.audio_url, e.position_s, COALESCE(e.downloaded_path, ''), \
                COALESCE(p.title, ''), COALESCE(NULLIF(p.custom_image, ''), COALESCE(e.image_url, p.image_url), '') \
         FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = ?",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;
    let Some((title, url, position, downloaded, show, art)) = row else {
        anyhow::bail!("episode {episode_id} is gone");
    };
    // A saved copy always wins: an episode kept for a flight should not go to
    // the network when the plane has no network.
    let src = if !downloaded.is_empty() && Path::new(&downloaded).exists() {
        downloaded
    } else {
        url
    };
    let speed = lock().pod_speed;
    let mut args = audio_args(false);
    args.push(format!("--speed={speed}"));
    // Podcast CDNs answer mpv's default agent with 403; the domain crate keeps
    // the agent string that gets through.
    args.push(format!(
        "--user-agent={}",
        tulipix_core::net::BROWSER_UA
    ));
    mpv::play(
        &src,
        mpv::Slot::Podcast,
        &args,
        Some(position),
        |obs| {
            emit(MusicEvent::Tick {
                pos: obs.pos,
                dur: obs.dur,
                playing: !obs.paused,
            })
        },
        || emit(MusicEvent::Ended),
    );
    let art_local = cache_remote(&art).await.unwrap_or_default();
    mpv::set_now_playing(mpv::NowPlaying {
        item_id: 0,
        title,
        artist: show,
        album: String::new(),
        art: art_local,
        key: episode_id.to_string(),
    });
    emit(MusicEvent::TrackChanged);
    Ok(())
}

async fn play_chapter(pool: &sqlx::SqlitePool, item_id: i64) -> Result<()> {
    let (position, speed) = tulipix_music::audiobooks::resume(pool, item_id)
        .await
        .unwrap_or((0.0, 1.0));
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT i.abs_path, COALESCE(tm.title, '') FROM items i \
         JOIN track_meta tm ON tm.item_id = i.id WHERE i.id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((path, title)) = row else {
        anyhow::bail!("chapter {item_id} is not in the library");
    };
    let folder = Path::new(&path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut args = audio_args(false);
    args.push(format!("--speed={}", tulipix_music::audiobooks::clamp_speed(speed)));
    if setting("music.book.skip-silence", "0") == "1" {
        // The same filter the Slint build's skip-silence toggle installs.
        args.push("--af-append=lavfi=[silenceremove=1:0:-50dB]".into());
    }
    mpv::play(
        &path,
        mpv::Slot::Book,
        &args,
        Some(position),
        |obs| {
            emit(MusicEvent::Tick {
                pos: obs.pos,
                dur: obs.dur,
                playing: !obs.paused,
            })
        },
        || emit(MusicEvent::Ended),
    );
    let book = Path::new(&folder)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    mpv::set_now_playing(mpv::NowPlaying {
        item_id,
        title: if title.is_empty() { book.clone() } else { title },
        artist: book,
        album: String::new(),
        art: folder_cover(Path::new(&folder)).unwrap_or_default(),
        key: folder,
    });
    emit(MusicEvent::TrackChanged);
    Ok(())
}

async fn play_station(index: usize) -> Result<()> {
    let station = {
        let s = lock();
        s.radio_list.get(index).cloned()
    };
    let Some(station) = station else {
        anyhow::bail!("no station at {index}");
    };
    let mut args = audio_args(false);
    args.push(format!("--user-agent={}", tulipix_core::net::BROWSER_UA));
    // A stream has no duration and no end; without this mpv gives up on the
    // first hiccup instead of reconnecting.
    args.push("--stream-lavf-o=reconnect=1,reconnect_streamed=1,reconnect_delay_max=5".into());
    mpv::play(
        &station.url,
        mpv::Slot::Radio,
        &args,
        None,
        |obs| {
            emit(MusicEvent::Tick {
                pos: obs.pos,
                dur: obs.dur,
                playing: !obs.paused,
            })
        },
        || emit(MusicEvent::Ended),
    );
    mpv::set_now_playing(mpv::NowPlaying {
        item_id: 0,
        title: station.name.clone(),
        artist: station.country.clone(),
        album: station.tags.clone(),
        art: cache_remote(&station.favicon).await.unwrap_or_default(),
        key: station.stationuuid.clone(),
    });
    if let Ok(pool) = radio_pool().await {
        let _ = tulipix_music::radio::touch_played(pool, &station, now_secs()).await;
    }
    emit(MusicEvent::TrackChanged);
    Ok(())
}

async fn play_youtube(video_id: &str) -> Result<()> {
    let pool = youtube_pool().await?;
    let meta: Option<(String, String, String)> = sqlx::query_as(
        "SELECT COALESCE(title, ''), COALESCE(channel, ''), COALESCE(thumb_path, '') \
         FROM yt_cached WHERE video_id = ? \
         UNION ALL \
         SELECT COALESCE(title, ''), COALESCE(channel, ''), COALESCE(thumb_path, '') \
         FROM yt_downloaded WHERE video_id = ? LIMIT 1",
    )
    .bind(video_id)
    .bind(video_id)
    .fetch_optional(pool)
    .await?;
    let (title, channel, thumb) = meta.unwrap_or_else(|| {
        // Not in the store yet: this is a search hit, so take what the results
        // list already has rather than a second network round trip.
        let s = lock();
        s.yt_results
            .iter()
            .chain(s.yt_channel_videos.iter())
            .find(|v| v.video_id == video_id)
            .map(|v| (v.title.clone(), v.channel.clone(), v.thumb.clone()))
            .unwrap_or_default()
    });
    let resume = tulipix_music::youtube::store::progress_of(pool, video_id)
        .await
        .unwrap_or(0.0);

    let cached = yt_media_dir().join(format!("{video_id}.opus"));
    let src = if cached.exists() {
        cached.to_string_lossy().into_owned()
    } else {
        let url = format!("https://www.youtube.com/watch?v={video_id}");
        let out = tokio::process::Command::new(tulipix_core::ytdlp::bin())
            .args(["-g", "-f", "bestaudio/best", "--no-playlist"])
            .args(tulipix_core::ytdlp::common_args())
            .arg(&url)
            .no_window_async()
            .output()
            .await;
        // Keep the stderr: "could not resolve a stream for dQw4w9WgXcQ" named
        // the video and nothing the user could do about it, and a refusal is
        // the overwhelmingly common reason this fails.
        let out = out.map_err(|e| anyhow::anyhow!("yt-dlp could not be launched: {e}"))?;
        let resolved = out.status.success().then(|| {
            String::from_utf8_lossy(&out.stdout)
                .lines()
                .map(|l| l.trim().to_string())
                .find(|l| !l.is_empty())
        }).flatten();
        match resolved {
            Some(u) => u,
            None => anyhow::bail!(
                "{}",
                tulipix_core::ytdlp::friendly_error(&String::from_utf8_lossy(&out.stderr))
            ),
        }
    };

    let mut args = audio_args(false);
    args.push(format!("--user-agent={}", tulipix_core::net::BROWSER_UA));
    mpv::play(
        &src,
        mpv::Slot::Youtube,
        &args,
        Some(resume),
        |obs| {
            emit(MusicEvent::Tick {
                pos: obs.pos,
                dur: obs.dur,
                playing: !obs.paused,
            })
        },
        || emit(MusicEvent::Ended),
    );
    mpv::set_now_playing(mpv::NowPlaying {
        item_id: 0,
        title,
        artist: channel,
        album: String::new(),
        art: cache_remote(&thumb).await.unwrap_or_default(),
        key: video_id.to_string(),
    });
    emit(MusicEvent::TrackChanged);
    Ok(())
}

/// `NoWindow` for the tokio Command, same reason as the std one above.
trait NoWindowAsync {
    fn no_window_async(&mut self) -> &mut Self;
}

impl NoWindowAsync for tokio::process::Command {
    fn no_window_async(&mut self) -> &mut Self {
        use tulipix_core::proc::NoWindow;
        self.no_window()
    }
}

// -------------------------------------------------------------- commands ----

async fn apply(cmd: MusicCmd) -> Result<()> {
    match cmd {
        MusicCmd::Refresh => {
            // The sleep timer is checked here rather than on a timer of its
            // own: a snapshot happens at least once a second while anything is
            // playing, which is precision enough for a fade that takes a
            // minute, and it costs no extra thread.
            check_sleep();
        }
        MusicCmd::SetView { name } => {
            let mut s = lock();
            s.view = name;
            s.status.clear();
        }
        MusicCmd::Search { query } => {
            let mut s = lock();
            s.query = query;
            s.song_page = 0;
            s.browse_page = 0;
        }

        // --- My Music -------------------------------------------------------
        MusicCmd::SetLibTab { name } => {
            let mut s = lock();
            s.lib_tab = name;
            s.song_page = 0;
            s.browse_page = 0;
            // Leaving a tab closes what was drilled into from it, so a later
            // Refresh cannot resurrect a detail page the user has left.
            s.detail_open = false;
        }
        MusicCmd::SetSongSort { mode, dir } => {
            let mut s = lock();
            s.song_sort = mode;
            s.song_dir = dir;
            s.song_page = 0;
        }
        MusicCmd::SetSongPage { page } => lock().song_page = page.max(0),
        MusicCmd::SetBrowseSort { mode, dir } => {
            let mut s = lock();
            s.browse_sort = mode;
            s.browse_dir = dir;
            s.browse_page = 0;
        }
        MusicCmd::SetBrowsePage { page } => lock().browse_page = page.max(0),
        MusicCmd::OpenAlbum { album_id } => open_detail("album", album_id, String::new()),
        MusicCmd::OpenArtist { artist_id } => open_detail("artist", artist_id, String::new()),
        MusicCmd::OpenNowAlbum => open_now_detail("album").await?,
        MusicCmd::OpenNowArtist => open_now_detail("artist").await?,
        MusicCmd::OpenGenre { name } => open_detail("genre", 0, name),
        MusicCmd::OpenFolder { path } => open_detail("folder", 0, path),

        MusicCmd::MgrOpen { tab } => {
            let mut s = lock();
            s.mgr_open = true;
            s.mgr_tab = tab;
            s.mgr_mode = "list".into();
            s.mgr_filter = "all".into();
            s.mgr_page = 0;
        }
        MusicCmd::MgrClose => lock().mgr_open = false,
        MusicCmd::MgrSetTab { tab } => {
            let mut s = lock();
            s.mgr_tab = tab;
            s.mgr_mode = "list".into();
            s.mgr_filter = "all".into();
            s.mgr_page = 0;
        }
        MusicCmd::MgrSetFilter { key } => {
            let mut s = lock();
            s.mgr_filter = key;
            s.mgr_page = 0;
        }
        MusicCmd::MgrSetPage { page } => lock().mgr_page = page.max(0),
        MusicCmd::MgrBack => {
            let mut s = lock();
            // Preview → results → list, one rung at a time. Backing out of a
            // preview must not throw away the search that produced it.
            if s.mgr_picked.is_some() {
                s.mgr_picked = None;
            } else {
                s.mgr_mode = "list".into();
                s.mgr_results.clear();
                s.mgr_bodies.clear();
            }
        }
        MusicCmd::MgrSearchOpen { item_id } => mgr_search_open(item_id).await?,
        MusicCmd::MgrSetQuery {
            name,
            artist,
            album,
        } => {
            let mut s = lock();
            s.mgr_q_name = name;
            s.mgr_q_artist = artist;
            s.mgr_q_album = album;
        }
        MusicCmd::MgrSearch => mgr_search().await,
        MusicCmd::MgrPick { index } => {
            let mut s = lock();
            let i = index.max(0) as usize;
            if i < s.mgr_bodies.len() {
                s.mgr_picked = Some(i);
            }
        }
        MusicCmd::MgrSave => mgr_save().await?,
        MusicCmd::MgrView { item_id } => {
            let mut s = lock();
            s.mgr_item_id = item_id;
            s.mgr_mode = "view".into();
            s.mgr_picked = None;
        }
        MusicCmd::MgrSyncAll => mgr_sync_all(),
        MusicCmd::OpenPlaylist { playlist_id } => {
            open_detail("playlist", playlist_id, String::new())
        }
        MusicCmd::CloseDetail => {
            let mut s = lock();
            s.detail_open = false;
        }
        MusicCmd::AddFolder { path } => {
            let dir = PathBuf::from(&path);
            if !dir.is_dir() {
                anyhow::bail!("not a folder: {path}");
            }
            // The Add button is section-scoped: the folder lands in whichever
            // music sub-section is open, which is what the Slint build does and
            // the only reason a folder picked on the Audiobooks tab becomes a
            // shelf instead of four hundred songs in My Music.
            let view = lock().view.clone();
            tulipix_common::set_folder_section(&path, &view);
            add_watched_folder(&dir);
            scan_watched().await?;
        }
        MusicCmd::RemoveRoot { path } => {
            let keep: Vec<PathBuf> = load_watched_folders()
                .into_iter()
                .filter(|p| p.to_string_lossy() != path)
                .collect();
            save_watched_folders(&keep);
        }
        MusicCmd::Scan => scan_watched().await?,
        MusicCmd::ReadTags => read_missing_tags().await?,
        MusicCmd::SaveTags {
            item_id,
            title,
            artist,
            album,
            album_artist,
            genre,
            date,
            track_no,
            disc_no,
        } => {
            let pool = music_pool().await?;
            let path: Option<String> =
                sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                    .bind(item_id)
                    .fetch_optional(pool)
                    .await?;
            let Some(path) = path else {
                anyhow::bail!("track {item_id} is not in the library");
            };
            write_file_tags(
                &path,
                &title,
                &artist,
                &album,
                &album_artist,
                &genre,
                &date,
                track_no,
                disc_no,
            );
            let tags = tulipix_music::tags::TrackTags {
                title: tulipix_music::tags::clean(&title),
                artist: tulipix_music::tags::clean(&artist),
                album: tulipix_music::tags::clean(&album),
                album_artist: tulipix_music::tags::clean(&album_artist),
                genre: tulipix_music::tags::clean(&genre),
                year: tulipix_music::tags::parse_year(&date),
                track_no: if track_no > 0 { Some(track_no) } else { None },
                disc_no: if disc_no > 0 { Some(disc_no) } else { None },
                ..Default::default()
            };
            tulipix_music::scan::upsert_track(pool, item_id, &path, &tags).await?;
        }
        MusicCmd::DeleteTrack { item_id } => {
            let pool = music_pool().await?;
            let path: Option<String> =
                sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                    .bind(item_id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(p) = path {
                // The file first: a row without a file is a missing track the
                // next scan cleans up, but a file without a row comes straight
                // back on that same scan.
                //
                // To the recycle bin, not off the disk. The menu row says
                // "delete from system" and the Slint build has always meant the
                // recoverable kind; an unlink here made the same words mean
                // something the user cannot undo.
                if let Err(e) = tulipix_platform::fm::move_to_trash(Path::new(&p)) {
                    anyhow::bail!("could not delete {p}: {e}");
                }
            }
            tulipix_core::populator::remove(pool, item_id).await?;
        }

        MusicCmd::SongPlayDefault { item_id } => {
            let pool = music_pool().await?;
            let path = track_path(pool, item_id).await?;
            tulipix_platform::fm::open_default(Path::new(&path))?;
        }
        MusicCmd::SongViewAlbum { item_id } => open_item_detail(item_id, "album").await?,
        MusicCmd::SongViewArtist { item_id } => open_item_detail(item_id, "artist").await?,
        MusicCmd::SongSonic { item_id } => {
            let pool = music_pool().await?;
            // The embeddings are computed by the analysis pass, which lives in
            // a Slint-only crate this one cannot link. So this reads an index
            // rather than building one: with nothing stored it says so instead
            // of quietly queueing nothing.
            let hits = tulipix_music::embeddings::similar(pool, item_id, 25)
                .await
                .unwrap_or_default();
            if hits.is_empty() {
                anyhow::bail!("no sonic index for this track yet");
            }
            let ids: Vec<i64> = hits.into_iter().map(|(id, _)| id).collect();
            for id in &ids {
                tulipix_music::queue::enqueue(pool, *id, "sonic").await?;
            }
            lock().status = format!("Queued {} similar tracks.", ids.len());
        }
        MusicCmd::SongPlayVideo { item_id } => {
            let pool = music_pool().await?;
            let _ = sqlx::query("ALTER TABLE music_video_link ADD COLUMN video_path TEXT")
                .execute(pool)
                .await;
            let path: Option<String> = sqlx::query_scalar(
                "SELECT video_path FROM music_video_link \
                 WHERE item_id = ? AND video_path IS NOT NULL",
            )
            .bind(item_id)
            .fetch_optional(pool)
            .await?
            .flatten();
            let Some(path) = path.filter(|p| Path::new(p).exists()) else {
                anyhow::bail!("no music video linked — use “Link music video…” first");
            };
            // The windowed player the Slint build hands this to does not exist
            // here, and the deck is audio-only. The desktop's video player is
            // the honest answer rather than a second one inside this window.
            tulipix_platform::fm::open_default(Path::new(&path))?;
        }
        MusicCmd::SongLinkVideo { item_id, path } => {
            let pool = music_pool().await?;
            let _ = sqlx::query("ALTER TABLE music_video_link ADD COLUMN video_path TEXT")
                .execute(pool)
                .await;
            // The videos library's own id when the file happens to be indexed
            // there, zero when it is just a file on disk. Either way the path
            // is what plays it.
            let video_id: i64 = match tulipix_common::pool_for("videos").await {
                Ok(vp) => sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
                    .bind(&path)
                    .fetch_optional(&vp)
                    .await
                    .ok()
                    .flatten()
                    .unwrap_or(0),
                Err(_) => 0,
            };
            tulipix_music::video_link::link(pool, item_id, video_id).await?;
            sqlx::query("UPDATE music_video_link SET video_path = ? WHERE item_id = ?")
                .bind(&path)
                .bind(item_id)
                .execute(pool)
                .await?;
            lock().status = "Music video linked.".into();
        }
        MusicCmd::SetCardArt { kind, key, path } => {
            if !path.is_empty() && !Path::new(&path).exists() {
                anyhow::bail!("no such image: {path}");
            }
            // Albums and artists have a column for this; genres and playlists
            // have no row at all and keep theirs as a preference. Writing the
            // column is what makes the change survive into the Slint build and
            // into `music_ensure_art`, which reads it before it reads anything
            // else.
            match kind.as_str() {
                "album" | "artist" => {
                    let Ok(id) = key.parse::<i64>() else {
                        anyhow::bail!("not an id: {key}")
                    };
                    let pool = music_pool().await?;
                    let sql = if kind == "album" {
                        "UPDATE albums SET cover_path = ? WHERE id = ?"
                    } else {
                        "UPDATE artists SET image_path = ? WHERE id = ?"
                    };
                    sqlx::query(sql)
                        .bind(if path.is_empty() { None } else { Some(path.as_str()) })
                        .bind(id)
                        .execute(pool)
                        .await?;
                }
                // A book's cover belongs in `audiobook_covers`, which is the
                // one place the resolution chain looks first -- and the same
                // table the Slint build writes, so a cover picked in either
                // build is the cover in both.
                "book" => {
                    let pool = music_pool().await?;
                    tulipix_music::ab_meta::set_cover(pool, &key, &path).await?;
                    if let Ok(mut g) = ab_cover_memo().lock() {
                        g.retain(|(folder, _), _| folder != &key);
                    }
                }
                _ => set_setting(&format!("music.{kind}.cover.{key}"), &path),
            }
        }

        // --- transport ------------------------------------------------------
        MusicCmd::PlayList {
            item_ids,
            index,
            source,
        } => {
            let pool = music_pool().await?;
            if item_ids.is_empty() {
                return Ok(());
            }
            tulipix_music::queue::replace(pool, &item_ids, &source).await?;
            let at = (index.max(0) as usize).min(item_ids.len() - 1);
            play_track(pool, item_ids[at]).await?;
        }
        MusicCmd::PlayPause => {
            let obs = mpv::observe();
            if !obs.loaded {
                // Nothing on the deck: the play button starts the queue, which
                // is what it does everywhere else in the app.
                let pool = music_pool().await?;
                let ids = queue_ids(pool).await;
                if let Some(first) = ids.first().copied() {
                    play_track(pool, first).await?;
                }
                return Ok(());
            }
            mpv::set_paused(!obs.paused);
        }
        MusicCmd::Next => {
            let pool = music_pool().await?;
            step(pool, true).await?;
        }
        MusicCmd::Prev => {
            // Restart rather than step back when the track is already a few
            // seconds in — the universal behaviour of a Prev button.
            if mpv::observe().pos > 4.0 && mpv::current_slot() != mpv::Slot::Radio {
                mpv::seek_absolute(0.0);
                return Ok(());
            }
            let pool = music_pool().await?;
            step(pool, false).await?;
        }
        MusicCmd::Stop => {
            mpv::stop();
            emit(MusicEvent::TrackChanged);
        }
        MusicCmd::Seek { secs } => mpv::seek_absolute(secs.max(0.0)),
        MusicCmd::SetVolume { volume } => {
            let v = volume.clamp(0.0, 130.0);
            mpv::set_property("volume", &format!("{v}"));
            set_setting("music.volume", &format!("{v:.0}"));
        }
        MusicCmd::ToggleMute => {
            let muted = !mpv::observe().muted;
            mpv::set_property("mute", if muted { "true" } else { "false" });
            set_setting("music.muted", if muted { "1" } else { "0" });
        }
        MusicCmd::ToggleShuffle => {
            let mut s = lock();
            s.shuffle = !s.shuffle;
        }
        MusicCmd::CycleRepeat => {
            let next = {
                let s = lock();
                match s.repeat.as_str() {
                    "off" => "all",
                    "all" => "one",
                    _ => "off",
                }
            };
            lock().repeat = next.into();
            // repeat-one is mpv's `loop-file`, and it can be changed on a
            // running process rather than needing a respawn.
            mpv::set_property("loop-file", if next == "one" { "\"inf\"" } else { "\"no\"" });
        }
        MusicCmd::SetSleep { minutes } => {
            let mut s = lock();
            s.sleep_min = minutes.max(0);
            s.sleep_deadline = if minutes > 0 {
                Some(
                    std::time::Instant::now()
                        + std::time::Duration::from_secs(minutes as u64 * 60),
                )
            } else {
                None
            };
        }
        MusicCmd::Love { item_id } => {
            let pool = music_pool().await?;
            tulipix_music::rating::toggle_loved(pool, item_id).await?;
        }
        MusicCmd::Rate { item_id, stars } => {
            let pool = music_pool().await?;
            tulipix_music::rating::set_stars(pool, item_id, stars.clamp(0, 5) as u8).await?;
        }
        MusicCmd::AlbumFav { album_id } => {
            let pool = music_pool().await?;
            ensure_browse_columns(pool).await;
            sqlx::query("UPDATE albums SET loved = CASE COALESCE(loved, 0) WHEN 1 THEN 0 ELSE 1 END WHERE id = ?")
                .bind(album_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::AlbumRate { album_id, stars } => {
            let pool = music_pool().await?;
            ensure_browse_columns(pool).await;
            sqlx::query("UPDATE albums SET rating = ? WHERE id = ?")
                .bind(stars.clamp(0, 5))
                .bind(album_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::ArtistFav { artist_id } => {
            let pool = music_pool().await?;
            ensure_browse_columns(pool).await;
            sqlx::query("UPDATE artists SET loved = CASE COALESCE(loved, 0) WHEN 1 THEN 0 ELSE 1 END WHERE id = ?")
                .bind(artist_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::ArtistRate { artist_id, stars } => {
            let pool = music_pool().await?;
            ensure_browse_columns(pool).await;
            sqlx::query("UPDATE artists SET rating = ? WHERE id = ?")
                .bind(stars.clamp(0, 5))
                .bind(artist_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::QueueAdd { item_ids } => {
            let pool = music_pool().await?;
            for id in item_ids {
                tulipix_music::queue::enqueue(pool, id, "manual").await?;
            }
        }
        MusicCmd::QueueRemove { item_id } => {
            let pool = music_pool().await?;
            sqlx::query("DELETE FROM play_queue WHERE item_id = ?")
                .bind(item_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::QueueClear => {
            let pool = music_pool().await?;
            tulipix_music::queue::clear(pool).await?;
        }
        MusicCmd::QueueMove { from, to } => {
            let pool = music_pool().await?;
            tulipix_music::queue::move_item(pool, from.max(0) as usize, to.max(0) as usize)
                .await?;
        }
        MusicCmd::QueuePlayAt { index } => {
            let pool = music_pool().await?;
            let ids = queue_ids(pool).await;
            let Some(id) = ids.get(index.max(0) as usize).copied() else {
                return Ok(());
            };
            play_track(pool, id).await?;
        }
        MusicCmd::PlaylistCreate { name } => {
            let pool = music_pool().await?;
            if name.trim().is_empty() {
                anyhow::bail!("a playlist needs a name");
            }
            tulipix_music::playlists::create(pool, name.trim(), None).await?;
        }
        MusicCmd::PlaylistCreateSmart { kind } => {
            let pool = music_pool().await?;
            create_smart_playlist(pool, &kind).await?;
        }
        MusicCmd::PlaylistOpenFresh => {
            let pool = music_pool().await?;
            let id = rebuild_fresh_playlist(pool).await?;
            let mut s = lock();
            s.lib_tab = "playlists".into();
            s.browse_page = 0;
            s.detail_open = true;
            s.detail_kind = "playlist".into();
            s.detail_id = id;
            s.detail_key = id.to_string();
        }
        MusicCmd::PlaylistDelete { playlist_id } => {
            let pool = music_pool().await?;
            tulipix_music::playlists::delete(pool, playlist_id).await?;
            let mut s = lock();
            if s.detail_kind == "playlist" && s.detail_id == playlist_id {
                s.detail_open = false;
            }
        }
        MusicCmd::PlaylistAdd {
            playlist_id,
            item_ids,
        } => {
            let pool = music_pool().await?;
            for id in item_ids {
                tulipix_music::playlists::append(pool, playlist_id, id).await?;
            }
        }
        MusicCmd::PlaylistRemove {
            playlist_id,
            item_id,
        } => {
            let pool = music_pool().await?;
            sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ? AND item_id = ?")
                .bind(playlist_id)
                .bind(item_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::PlaylistMove {
            playlist_id,
            from,
            to,
        } => {
            let pool = music_pool().await?;
            tulipix_music::playlists::reorder(
                pool,
                playlist_id,
                from.max(0) as usize,
                to.max(0) as usize,
            )
            .await?;
        }
        MusicCmd::SetEqPreset { name } => {
            let eq = tulipix_music::eq::Equalizer::preset(&name)
                .unwrap_or_else(tulipix_music::eq::Equalizer::flat);
            save_eq(&eq);
            set_setting("music.eq.preset", &name);
            apply_eq_live(&eq);
        }
        MusicCmd::SetEqBand { index, gain_db } => {
            let mut eq = load_eq();
            if let Some(slot) = eq.gains_db.get_mut(index.max(0) as usize) {
                *slot = gain_db;
            }
            eq.enabled = true;
            eq.clamp();
            save_eq(&eq);
            // A hand-moved band is no longer any named preset.
            set_setting("music.eq.preset", "custom");
            apply_eq_live(&eq);
        }
        MusicCmd::ToggleEq => {
            let mut eq = load_eq();
            eq.enabled = !eq.enabled;
            save_eq(&eq);
            apply_eq_live(&eq);
        }
        MusicCmd::SetAudio { key, value } => {
            if !key.starts_with("music.") {
                anyhow::bail!("not an audio setting: {key}");
            }
            set_setting(&key, &value);
        }
        MusicCmd::FetchLyrics { item_id } => fetch_lyrics(item_id).await?,
        MusicCmd::LyricsOffset { delta_ms } => {
            let mut s = lock();
            s.lyrics_offset_ms += delta_ms;
        }
        MusicCmd::ClearHistory => {
            let pool = music_pool().await?;
            sqlx::query("DELETE FROM play_history").execute(pool).await?;
            lock().history.clear();
        }

        // --- Podcasts -------------------------------------------------------
        MusicCmd::PodSetTab { name } => {
            let mut s = lock();
            s.pod_tab = name;
            s.pod_page = 0;
        }
        MusicCmd::PodSubscribe { url } => {
            pod_job_set(true, 0.0, "Subscribing…");
            let r = subscribe_podcast(&url).await;
            pod_job_set(false, 1.0, "");
            r?;
        }
        MusicCmd::PodUnsubscribe { podcast_id } => {
            let pool = podcasts_pool().await?;
            sqlx::query("DELETE FROM podcast_episodes WHERE podcast_id = ?")
                .bind(podcast_id)
                .execute(pool)
                .await?;
            sqlx::query("DELETE FROM podcasts WHERE id = ?")
                .bind(podcast_id)
                .execute(pool)
                .await?;
            let mut s = lock();
            if s.pod_open == podcast_id {
                s.pod_open = -1;
            }
        }
        MusicCmd::PodOpen { podcast_id } => {
            let mut s = lock();
            s.pod_open = podcast_id;
            s.pod_ep_page = 0;
        }
        MusicCmd::PodBack => lock().pod_open = -1,
        MusicCmd::PodRefresh => {
            let pool = podcasts_pool().await?;
            let feeds: Vec<(i64, String)> =
                sqlx::query_as("SELECT id, feed_url FROM podcasts ORDER BY id")
                    .fetch_all(pool)
                    .await?;
            let total = feeds.len() as i64;
            for (i, (id, url)) in feeds.into_iter().enumerate() {
                emit(MusicEvent::ScanProgress {
                    root: url.clone(),
                    done: i as i64,
                    total,
                });
                pod_job_set(
                    true,
                    if total > 0 { i as f64 / total as f64 } else { 0.0 },
                    &format!("Refreshing {}/{total}", i + 1),
                );
                if let Err(e) = refresh_feed(id, &url).await {
                    tracing::warn!(error = %e, feed = %url, "podcast refresh failed");
                }
            }
            pod_job_set(false, 1.0, "");
            emit(MusicEvent::ScanFinished {
                inserted: 0,
                updated: total,
                missing: 0,
            });
        }
        MusicCmd::PodRefreshOne { podcast_id } => {
            let pool = podcasts_pool().await?;
            let url: Option<String> =
                sqlx::query_scalar("SELECT feed_url FROM podcasts WHERE id = ?")
                    .bind(podcast_id)
                    .fetch_optional(pool)
                    .await?;
            if let Some(url) = url {
                refresh_feed(podcast_id, &url).await?;
            }
        }
        MusicCmd::PodSetCat { name } => {
            let mut s = lock();
            s.pod_cat = name;
            s.pod_page = 0;
        }
        MusicCmd::PodSetPage { page } => lock().pod_page = page.max(0),
        MusicCmd::PodSetEpSort { mode } => {
            let mut s = lock();
            s.pod_ep_sort = mode;
            s.pod_ep_page = 0;
        }
        MusicCmd::PodSetEpPage { page } => lock().pod_ep_page = page.max(0),
        MusicCmd::PodPlay { episode_id } => {
            {
                let mut s = lock();
                if !s.pod_queue.contains(&episode_id) {
                    s.pod_queue.push(episode_id);
                }
            }
            play_episode(episode_id).await?;
        }
        MusicCmd::PodSkip { secs } => {
            let obs = mpv::observe();
            mpv::seek_absolute((obs.pos + secs).max(0.0));
        }
        MusicCmd::PodSetSpeed { speed } => {
            let s = speed.clamp(0.5, 3.0);
            lock().pod_speed = s;
            if mpv::current_slot() == mpv::Slot::Podcast {
                mpv::set_property("speed", &format!("{s}"));
            }
        }
        MusicCmd::PodDownload { episode_id } => download_episode(episode_id).await?,
        MusicCmd::PodRemoveDownload { episode_id } => {
            let pool = podcasts_pool().await?;
            let path: Option<String> =
                sqlx::query_scalar("SELECT downloaded_path FROM podcast_episodes WHERE id = ?")
                    .bind(episode_id)
                    .fetch_optional(pool)
                    .await?
                    .flatten();
            if let Some(p) = path {
                let _ = std::fs::remove_file(&p);
            }
            sqlx::query("UPDATE podcast_episodes SET downloaded_path = NULL WHERE id = ?")
                .bind(episode_id)
                .execute(pool)
                .await?;
        }
        MusicCmd::PodClearDownloads => {
            let pool = podcasts_pool().await?;
            for (_, path) in tulipix_music::podcasts::cleanup_candidates(pool)
                .await
                .unwrap_or_default()
            {
                let _ = std::fs::remove_file(&path);
            }
            sqlx::query("UPDATE podcast_episodes SET downloaded_path = NULL")
                .execute(pool)
                .await?;
        }
        MusicCmd::PodQueueAdd { episode_id } => {
            let mut s = lock();
            if !s.pod_queue.contains(&episode_id) {
                s.pod_queue.push(episode_id);
            }
        }
        MusicCmd::PodQueueRemove { episode_id } => {
            let mut s = lock();
            s.pod_queue.retain(|e| *e != episode_id);
        }
        MusicCmd::PodQueueClear => lock().pod_queue.clear(),
        MusicCmd::PodOpmlImport { path } => {
            let body = std::fs::read_to_string(&path)?;
            for (_, url) in tulipix_music::podcasts::parse_opml(&body) {
                if let Err(e) = subscribe_podcast(&url).await {
                    tracing::warn!(error = %e, feed = %url, "opml subscribe failed");
                }
            }
        }
        MusicCmd::PodOpmlExport { path } => {
            let pool = podcasts_pool().await?;
            let subs: Vec<(String, String)> =
                sqlx::query_as("SELECT COALESCE(title, ''), feed_url FROM podcasts ORDER BY title")
                    .fetch_all(pool)
                    .await?;
            std::fs::write(&path, tulipix_music::podcasts::to_opml(&subs))?;
        }
        MusicCmd::PodResetAll => {
            let pool = podcasts_pool().await?;
            pod_job_set(true, 0.0, "Removing downloads…");
            let files = tulipix_music::podcasts::cleanup_candidates(pool)
                .await
                .unwrap_or_default();
            let total = files.len().max(1);
            for (i, (_, p)) in files.into_iter().enumerate() {
                let _ = std::fs::remove_file(&p);
                pod_job_set(true, i as f64 / total as f64, "Removing downloads…");
            }
            sqlx::query("DELETE FROM podcast_episodes").execute(pool).await?;
            sqlx::query("DELETE FROM podcasts").execute(pool).await?;
            {
                let mut s = lock();
                s.pod_open = -1;
                s.pod_queue.clear();
                s.pod_info = None;
            }
            pod_job_set(false, 1.0, "");
        }
        MusicCmd::PodSetHomeSort { mode } => {
            let mut s = lock();
            s.pod_home_sort = mode;
            s.pod_home_page = 0;
        }
        MusicCmd::PodSetHomePage { page } => lock().pod_home_page = page.max(0),
        MusicCmd::PodToggleHome { podcast_id } => {
            let pool = podcasts_pool().await?;
            sqlx::query(
                "UPDATE podcasts SET home_pinned = CASE COALESCE(home_pinned,0) WHEN 0 THEN 1 ELSE 0 END \
                 WHERE id = ?",
            )
            .bind(podcast_id)
            .execute(pool)
            .await?;
        }
        MusicCmd::PodSetDlSort { mode } => {
            let mut s = lock();
            s.pod_dl_sort = mode;
            s.pod_page = 0;
        }
        MusicCmd::PodTrendsLoad => {
            // Cached for the session: the build is a network walk over three
            // dozen feeds, and the tab is re-entered constantly.
            if !lock().pod_trends.is_empty() {
                return Ok(());
            }
            lock().pod_trends_loading = true;
            emit(MusicEvent::Stale);
            let pool = podcasts_pool().await?;
            let metas = tulipix_music::pod_trends::build(pool, tulipix_core::net::http()).await;
            let mut s = lock();
            s.pod_trends = metas;
            s.pod_trends_loading = false;
        }
        MusicCmd::PodSetTrendsSort { mode } => {
            let mut s = lock();
            s.pod_trends_sort = mode;
            s.pod_trends_page = 0;
        }
        MusicCmd::PodSetTrendsPage { page } => lock().pod_trends_page = page.max(0),
        MusicCmd::PodTrendSubscribe { feed_url } => {
            pod_job_set(true, 0.0, "Subscribing…");
            let r = subscribe_podcast(&feed_url).await;
            pod_job_set(false, 1.0, "");
            r?;
        }
        MusicCmd::PodInfoOpen { podcast_id, feed_url } => {
            let info = pod_info(podcast_id, &feed_url).await;
            lock().pod_info = info;
        }
        MusicCmd::PodInfoClose => lock().pod_info = None,
        MusicCmd::PodInfoSetCategory { name } => {
            let feed = lock().pod_info.as_ref().map(|i| i.feed_url.clone());
            if let Some(feed) = feed {
                let pool = podcasts_pool().await?;
                sqlx::query("UPDATE podcasts SET category = ? WHERE feed_url = ?")
                    .bind(&name)
                    .bind(&feed)
                    .execute(pool)
                    .await?;
                if let Some(i) = lock().pod_info.as_mut() {
                    i.category = name;
                }
            }
        }
        MusicCmd::PodSetThumb { podcast_id, path } => {
            let pool = podcasts_pool().await?;
            sqlx::query("UPDATE podcasts SET custom_image = ? WHERE id = ?")
                .bind(&path)
                .bind(podcast_id)
                .execute(pool)
                .await?;
            if let Some(i) = lock().pod_info.as_mut() {
                if i.podcast_id == podcast_id {
                    i.art = path;
                }
            }
        }
        MusicCmd::PodTranscript { episode_id } => {
            let pool = podcasts_pool().await?;
            let row: Option<(String, String)> = sqlx::query_as(
                "SELECT COALESCE(title,''), COALESCE(description,'') FROM podcast_episodes WHERE id = ?",
            )
            .bind(episode_id)
            .fetch_optional(pool)
            .await?;
            lock().pod_transcript = row.map(|(t, d)| {
                let body = tulipix_music::podcasts::strip_html(&d);
                let body = if body.trim().is_empty() {
                    "This episode has no show notes.".to_string()
                } else {
                    body
                };
                (t, body)
            });
        }
        MusicCmd::PodTranscriptClose => lock().pod_transcript = None,
        MusicCmd::PodSearch { query } => {
            let mut s = lock();
            let tab = s.pod_tab.clone();
            s.pod_queries.insert(tab, query.trim().to_string());
            s.pod_page = 0;
            s.pod_trends_page = 0;
        }

        // --- Audiobooks -----------------------------------------------------
        MusicCmd::BookSetTab { name } => lock().book_tab = name,
        MusicCmd::BookOpen { folder } => lock().book_open = folder,
        MusicCmd::BookBack => lock().book_open.clear(),
        MusicCmd::BookPlay { item_id } => {
            let pool = music_pool().await?;
            play_chapter(pool, item_id).await?;
        }
        MusicCmd::BookChapter { delta } => {
            let pool = music_pool().await?;
            step_chapter(pool, delta >= 0).await?;
        }
        MusicCmd::BookSetSpeed { speed } => {
            let v = tulipix_music::audiobooks::clamp_speed(speed);
            let folder = lock().book_open.clone();
            if !folder.is_empty() {
                let pool = music_pool().await?;
                let ids = tulipix_music::audiobooks::book_chapters(pool, &folder).await?;
                tulipix_music::audiobooks::set_book_speed(pool, &ids, v).await?;
            }
            if mpv::current_slot() == mpv::Slot::Book {
                mpv::set_property("speed", &format!("{v}"));
            }
        }
        MusicCmd::BookSetFinished { finished } => {
            let folder = lock().book_open.clone();
            let pool = music_pool().await?;
            let ids = tulipix_music::audiobooks::book_chapters(pool, &folder).await?;
            tulipix_music::audiobooks::set_finished(pool, &ids, finished).await?;
        }
        MusicCmd::BookFlagFolder { folder, on } => {
            let pool = music_pool().await?;
            // Tag and flag move together, or they undo each other: the tag is
            // what `apply_folder_sections` re-asserts after every scan, so
            // clearing the flag alone put the book straight back on the shelf
            // the next time anything rescanned.
            tulipix_common::set_folder_section(
                &folder,
                if on { "audiobooks" } else { "mymusic" },
            );
            tulipix_music::audiobooks::set_folder_flag(pool, &folder, on).await?;
        }
        MusicCmd::BookmarkAdd { label } => {
            let pool = music_pool().await?;
            let item_id = mpv::now_playing().item_id;
            if item_id == 0 {
                anyhow::bail!("nothing is playing to bookmark");
            }
            tulipix_music::audiobooks::add_bookmark(pool, item_id, mpv::observe().pos, &label)
                .await?;
        }
        MusicCmd::BookmarkJump { index } => {
            let pool = music_pool().await?;
            let marks = book_bookmarks(pool).await?;
            let Some(bm) = marks.get(index.max(0) as usize) else {
                return Ok(());
            };
            let (item_id, at) = (bm.item_id, bm.position_s);
            play_chapter(pool, item_id).await?;
            mpv::seek_absolute(at);
        }
        MusicCmd::BookmarkRemove { index } => {
            let pool = music_pool().await?;
            let marks = book_bookmarks(pool).await?;
            if let Some(bm) = marks.get(index.max(0) as usize) {
                tulipix_music::audiobooks::remove_bookmark(pool, bm.item_id, bm.position_s)
                    .await?;
            }
        }
        MusicCmd::BookmarkRename { index, label } => {
            let pool = music_pool().await?;
            let marks = book_bookmarks(pool).await?;
            if let Some(bm) = marks.get(index.max(0) as usize) {
                tulipix_music::audiobooks::rename_bookmark(
                    pool,
                    bm.item_id,
                    bm.position_s,
                    &label,
                )
                .await?;
            }
        }

        // --- Radio ----------------------------------------------------------
        MusicCmd::RadioSetTab { name } => {
            let mut s = lock();
            s.radio_tab = name;
            s.radio_cat_open = false;
            s.radio_page = 0;
        }
        MusicCmd::RadioSetGenre { index } => open_radio_category(index).await?,
        MusicCmd::RadioBack => {
            let mut s = lock();
            s.radio_cat_open = false;
            s.radio_page = 0;
        }
        MusicCmd::RadioSetPage { page } => lock().radio_page = page.max(0),
        MusicCmd::RadioSetSort { mode } => {
            let mut s = lock();
            if s.radio_sort == mode {
                s.radio_sort_dir = if s.radio_sort_dir == "desc" { "asc" } else { "desc" }.into();
            } else {
                s.radio_sort = mode;
                s.radio_sort_dir = "desc".into();
            }
        }
        MusicCmd::RadioPlay { index } => play_station(index.max(0) as usize).await?,
        MusicCmd::RadioFav { index } => {
            let station = { lock().radio_list.get(index.max(0) as usize).cloned() };
            let Some(station) = station else { return Ok(()) };
            let pool = radio_pool().await?;
            let favs = tulipix_music::radio::favourite_uuids(pool)
                .await
                .unwrap_or_default();
            if favs.contains(&station.stationuuid) {
                tulipix_music::radio::remove_favourite(pool, &station.stationuuid).await?;
            } else {
                tulipix_music::radio::add_favourite(pool, &station).await?;
            }
        }
        MusicCmd::RadioSearch { query } => radio_search(&query).await?,
        MusicCmd::RadioRefresh => radio_refresh_all().await?,
        MusicCmd::RadioClearRecent => {
            let pool = radio_pool().await?;
            tulipix_music::radio::clear_recents(pool).await?;
        }
        MusicCmd::RadioAdd { name, url } => {
            if name.trim().is_empty() || !url.starts_with("http") {
                anyhow::bail!("a station needs a name and an http(s) stream URL");
            }
            let pool = radio_pool().await?;
            let st = tulipix_music::radio::custom_station(name.trim(), url.trim());
            tulipix_music::radio::add_favourite(pool, &st).await?;
        }

        // --- YouTube --------------------------------------------------------
        MusicCmd::YtSetTab { name } => {
            let mut s = lock();
            s.yt_tab = name;
            s.yt_dl_page = 0;
            s.yt_subs_page = 0;
        }
        MusicCmd::YtSearch { query } => {
            lock().yt_search_q = query.trim().to_string();
            yt_search(&query, false).await?
        }
        MusicCmd::YtSearchMore => {
            let q = lock().yt_search_q.clone();
            if !q.is_empty() {
                yt_search(&q, true).await?;
            }
        }
        MusicCmd::YtSetDlSort { mode } => {
            let mut s = lock();
            s.yt_dl_sort = mode;
            s.yt_dl_page = 0;
        }
        MusicCmd::YtSetSubsSort { mode } => {
            let mut s = lock();
            s.yt_subs_sort = mode;
            s.yt_subs_page = 0;
        }
        MusicCmd::YtToggleSubsDir => {
            let mut s = lock();
            s.yt_subs_dir = if s.yt_subs_dir == "desc" { "asc".into() } else { "desc".into() };
        }
        MusicCmd::YtToggleHomeConnect => {
            let on = setting("music.yt.home-connect", "1") != "0";
            set_setting("music.yt.home-connect", if on { "0" } else { "1" });
        }
        MusicCmd::YtToggleSubsFilter => {
            let next = {
                let mut s = lock();
                s.yt_subs_filter =
                    if s.yt_subs_filter == "sub" { "unsub".into() } else { "sub".into() };
                s.yt_subs_page = 0;
                s.yt_subs_filter.clone()
            };
            // Persisted, so the page opens where you left it -- the Slint
            // build has always remembered this and they share the key.
            tulipix_music::yt_prefs::store_subs_filter(&next);
        }
        MusicCmd::YtSetPlaylistSort { mode } => lock().yt_playlist_sort = mode,
        MusicCmd::YtPlaylistPlayAll { playlist_id } => {
            let pool = youtube_pool().await?;
            let items =
                tulipix_music::youtube::store::playlist_items(pool, playlist_id).await?;
            let ids: Vec<String> = items.into_iter().map(|p| p.video_id).collect();
            let Some(first) = ids.first().cloned() else {
                anyhow::bail!("that playlist is empty");
            };
            {
                let mut s = lock();
                s.yt_queue = ids;
                s.yt_queue_pos = 0;
                s.yt_playing_pl_id = playlist_id;
            }
            play_youtube(&first).await?;
            cache_youtube_audio(&first).await;
        }
        MusicCmd::YtAddChannelUrl { url } => {
            let (id, title) = yt_channel_from_url(&url).await?;
            let pool = youtube_pool().await?;
            tulipix_music::youtube::store::import_subs(
                pool,
                &[tulipix_music::youtube::subscriptions::ImportedSub {
                    channel_id: id,
                    title,
                }],
            )
            .await?;
        }
        MusicCmd::YtImportPlaylistUrl { url } => yt_import_playlist_url(&url).await?,
        MusicCmd::YtSetDefaultRes { height } => {
            tulipix_music::yt_prefs::store_default_res(Some(height));
        }
        MusicCmd::YtResetVideoPrefs => tulipix_music::yt_prefs::store_default_res(None),
        MusicCmd::YtPinHome { channel_id } => {
            tulipix_music::yt_prefs::add_home_channel(&channel_id);
        }
        MusicCmd::YtUnpinHome { channel_id } => {
            tulipix_music::yt_prefs::remove_home_channel(&channel_id);
        }
        MusicCmd::YtSetFetcher { name } => tulipix_music::yt_prefs::store_fetcher(&name),
        MusicCmd::YtChannelSearch { query } => yt_channel_search(&query).await?,
        MusicCmd::YtWatch { video_id, height } => {
            // If this same video is on the deck, hand its position over.
            let start = if mpv::current_slot() == mpv::Slot::Youtube
                && mpv::now_playing().key == video_id
            {
                mpv::observe().pos
            } else {
                let pool = youtube_pool().await?;
                tulipix_music::youtube::store::progress_of(pool, &video_id)
                    .await
                    .unwrap_or(0.0)
            };
            yt_watch_video(&video_id, height, start).await?;
        }
        MusicCmd::YtWatchCurrent => {
            if mpv::current_slot() != mpv::Slot::Youtube {
                anyhow::bail!("nothing from YouTube is playing");
            }
            let id = mpv::now_playing().key;
            if id.is_empty() {
                anyhow::bail!("nothing from YouTube is playing");
            }
            let height = match tulipix_music::yt_prefs::default_res() {
                h if h > 0 => h,
                _ => 0,
            };
            yt_watch_video(&id, height, mpv::observe().pos).await?;
        }
        MusicCmd::YtStopWatching => {
            if let Ok(mut g) = yt_watch().lock() {
                *g = (0, String::new());
            }
            emit(MusicEvent::VideoStop);
        }
        MusicCmd::YtChannelLoadMore => {
            let (id, page) = {
                let s = lock();
                (s.yt_channel_id.clone(), s.yt_channel_page)
            };
            if !id.is_empty() {
                yt_open_channel_pages(&id, page + 1).await?;
            }
        }
        MusicCmd::YtClearRecent => {
            let pool = youtube_pool().await?;
            tulipix_music::youtube::store::clear_recent_searches(pool).await?;
        }
        MusicCmd::YtPlay { video_id } => {
            {
                let mut s = lock();
                // A single tap is no longer "playing that playlist", however
                // the queue got here.
                if !s.yt_queue.contains(&video_id) {
                    s.yt_queue.push(video_id.clone());
                    s.yt_playing_pl_id = -1;
                }
                let at = s
                    .yt_queue
                    .iter()
                    .position(|v| *v == video_id)
                    .unwrap_or(0) as i64;
                s.yt_queue_pos = at;
            }
            play_youtube(&video_id).await?;
            cache_youtube_audio(&video_id).await;
        }
        MusicCmd::YtPlayLocal {
            media_path,
            video_id,
        } => {
            if !Path::new(&media_path).exists() {
                anyhow::bail!("that download is no longer on disk");
            }
            let args = audio_args(false);
            mpv::play(
                &media_path,
                mpv::Slot::Youtube,
                &args,
                None,
                |obs| {
                    emit(MusicEvent::Tick {
                        pos: obs.pos,
                        dur: obs.dur,
                        playing: !obs.paused,
                    })
                },
                || emit(MusicEvent::Ended),
            );
            let pool = youtube_pool().await?;
            let meta: Option<(String, String, String)> = sqlx::query_as(
                "SELECT COALESCE(title, ''), COALESCE(channel, ''), COALESCE(thumb_path, '') \
                 FROM yt_downloaded WHERE video_id = ? LIMIT 1",
            )
            .bind(&video_id)
            .fetch_optional(pool)
            .await?;
            let (title, channel, thumb) = meta.unwrap_or_default();
            mpv::set_now_playing(mpv::NowPlaying {
                item_id: 0,
                title,
                artist: channel,
                album: String::new(),
                art: cache_remote(&thumb).await.unwrap_or_default(),
                key: video_id,
            });
            emit(MusicEvent::TrackChanged);
        }
        MusicCmd::YtDownload { video_id, quality } => {
            // The card knows the title; the job list is drawn before yt-dlp has
            // said anything, so take it from what is already on screen.
            let title = {
                let s = lock();
                s.yt_results
                    .iter()
                    .chain(s.yt_channel_videos.iter())
                    .chain(s.yt_recommended.iter())
                    .find(|v| v.video_id == video_id)
                    .map(|v| v.title.clone())
                    .unwrap_or_else(|| video_id.clone())
            };
            yt_job_push(&video_id, &title);
            let r = yt_download(&video_id, &quality).await;
            yt_job_done(&video_id);
            r?;
        }
        MusicCmd::YtRemoveCached { video_id } => {
            let pool = youtube_pool().await?;
            if let Some(p) = tulipix_music::youtube::store::remove_cached(pool, &video_id).await? {
                let _ = std::fs::remove_file(&p);
            }
        }
        MusicCmd::YtRemoveDownload { video_id } => {
            let pool = youtube_pool().await?;
            if let Some(p) =
                tulipix_music::youtube::store::remove_download(pool, &video_id).await?
            {
                let _ = std::fs::remove_file(&p);
            }
        }
        MusicCmd::YtClearCached => {
            let pool = youtube_pool().await?;
            for p in tulipix_music::youtube::store::clear_cached(pool).await? {
                let _ = std::fs::remove_file(&p);
            }
        }
        MusicCmd::YtClearDownloads => {
            let pool = youtube_pool().await?;
            for p in tulipix_music::youtube::store::clear_downloads(pool).await? {
                let _ = std::fs::remove_file(&p);
            }
        }
        MusicCmd::YtSetDlPage { page } => lock().yt_dl_page = page.max(0),
        MusicCmd::YtOpenChannel { channel_id } => yt_open_channel(&channel_id).await?,
        MusicCmd::YtChannelBack => {
            let mut s = lock();
            s.yt_channel_open = false;
            s.yt_channel_videos.clear();
        }
        MusicCmd::YtChannelMode { mode } => {
            let id = {
                let mut s = lock();
                s.yt_channel_mode = mode;
                s.yt_channel_id.clone()
            };
            if !id.is_empty() {
                yt_open_channel(&id).await?;
            }
        }
        MusicCmd::YtSubscribe { channel_id, title } => {
            let pool = youtube_pool().await?;
            tulipix_music::youtube::store::import_subs(
                pool,
                &[tulipix_music::youtube::subscriptions::ImportedSub {
                    channel_id: channel_id.clone(),
                    title,
                }],
            )
            .await?;
            let mut s = lock();
            if s.yt_channel_id == channel_id {
                s.yt_channel_subscribed = true;
            }
        }
        MusicCmd::YtUnsub { channel_id } => {
            let pool = youtube_pool().await?;
            tulipix_music::youtube::store::unsubscribe(pool, &channel_id).await?;
            let mut s = lock();
            if s.yt_channel_id == channel_id {
                s.yt_channel_subscribed = false;
            }
        }
        MusicCmd::YtSetSubsPage { page } => lock().yt_subs_page = page.max(0),
        MusicCmd::YtRefreshSubs => yt_refresh_subs().await?,
        MusicCmd::YtImportSubs { path } => {
            let body = std::fs::read_to_string(&path)?;
            let subs = tulipix_music::youtube::subscriptions::parse(&body)?;
            let pool = youtube_pool().await?;
            let n = tulipix_music::youtube::store::import_subs(pool, &subs).await?;
            lock().yt_status = format!("Imported {n} channels");
        }
        MusicCmd::YtCreatePlaylist { name } => {
            let pool = youtube_pool().await?;
            if name.trim().is_empty() {
                anyhow::bail!("a playlist needs a name");
            }
            tulipix_music::youtube::store::create_playlist(pool, name.trim()).await?;
        }
        MusicCmd::YtDeletePlaylist { playlist_id } => {
            let pool = youtube_pool().await?;
            tulipix_music::youtube::store::delete_playlist(pool, playlist_id).await?;
            let mut s = lock();
            if s.yt_playlist_id == playlist_id {
                s.yt_playlist_open = false;
            }
        }
        MusicCmd::YtOpenPlaylist { playlist_id } => {
            let pool = youtube_pool().await?;
            let meta =
                tulipix_music::youtube::store::get_playlist(pool, playlist_id).await?;
            let mut s = lock();
            s.yt_playlist_open = true;
            s.yt_playlist_id = playlist_id;
            s.yt_playlist_title = meta.map(|m| m.0).unwrap_or_default();
        }
        MusicCmd::YtPlaylistBack => lock().yt_playlist_open = false,
        MusicCmd::YtAddToPlaylist {
            playlist_id,
            video_id,
        } => {
            let pool = youtube_pool().await?;
            let known = {
                let s = lock();
                s.yt_results
                    .iter()
                    .chain(s.yt_channel_videos.iter())
                    .find(|v| v.video_id == video_id)
                    .cloned()
            };
            let v = known.unwrap_or(YtVideo {
                video_id: video_id.clone(),
                title: String::new(),
                channel: String::new(),
                thumb: String::new(),
                duration: 0,
                media_path: String::new(),
                meta: String::new(),
                progress: 0.0,
                quality: String::new(),
            });
            tulipix_music::youtube::store::add_to_playlist(
                pool,
                playlist_id,
                &v.video_id,
                &v.title,
                &v.channel,
                &v.thumb,
                v.duration,
            )
            .await?;
        }
        MusicCmd::YtRemoveFromPlaylist {
            playlist_id,
            video_id,
        } => {
            let pool = youtube_pool().await?;
            tulipix_music::youtube::store::remove_from_playlist(pool, playlist_id, &video_id)
                .await?;
        }
    }
    Ok(())
}

// ------------------------------------------------ tags & lyrics manager ----

/// Rows per page in the manager's list. Ten fits the 640x680 modal Slint
/// draws without the list needing a scrollbar of its own.
const MGR_PAGE: i64 = 10;

/// The joins the manager counts over. `TRACK_SELECT` already carries them --
/// this is the same FROM without the columns, for the three COUNT(*)s behind
/// the stat cards.
const MGR_FROM: &str = " FROM items i JOIN track_meta tm ON tm.item_id = i.id \
     LEFT JOIN artists ar ON ar.id = tm.artist_id \
     LEFT JOIN albums al ON al.id = tm.album_id \
     LEFT JOIN lyrics ly ON ly.item_id = i.id";

/// What "done" and "not done" mean on each tab.
///
/// Lyrics: synced words are the goal, plain words are progress, nothing is
/// missing. Tags: a song with both an artist and an album is tagged.
fn mgr_clauses(tab: &str) -> (&'static str, &'static str) {
    if tab == "tags" {
        (
            "COALESCE(ar.name, '') != '' AND COALESCE(al.title, '') != ''",
            "(COALESCE(ar.name, '') = '' OR COALESCE(al.title, '') = '')",
        )
    } else {
        ("ly.synced = 1", "ly.item_id IS NULL")
    }
}

fn mgr_status_of(tab: &str, t: &Track) -> String {
    if tab == "tags" {
        if t.artist.is_empty() || t.album.is_empty() {
            "Missing".into()
        } else {
            "Tagged".into()
        }
    } else {
        match t.lyrics.as_str() {
            "synced" => "Synced".into(),
            "plain" => "Normal".into(),
            _ => "Missing".into(),
        }
    }
}

/// Everything the manager modal draws.
///
/// Two halves, because two things drive it. The counts and the paged list
/// belong to the modal and run only while it is open — three `COUNT(*)`s over
/// the library on every snapshot would be paid by every screen in the section.
/// The preview belongs to the search, which the player's side panel can also
/// open, so it is filled whenever one is in flight.
async fn mgr_load(pool: &sqlx::SqlitePool, s: &Session, st: &mut MusicState) {
    if s.mgr_open {
        mgr_load_list(pool, s, st).await;
    }
    mgr_load_preview(pool, s, st).await;
}

async fn mgr_load_list(pool: &sqlx::SqlitePool, s: &Session, st: &mut MusicState) {
    let (ok_sql, missing_sql) = mgr_clauses(&s.mgr_tab);
    let count = |extra: String| async move {
        let sql = format!("SELECT COUNT(*){MGR_FROM}{MUSIC_WHERE}{extra}");
        sqlx::query_scalar::<_, i64>(&sql)
            .fetch_one(pool)
            .await
            .unwrap_or(0)
    };
    st.mgr_total = count(String::new()).await;
    st.mgr_ok = count(format!(" AND {ok_sql}")).await;
    st.mgr_missing = count(format!(" AND {missing_sql}")).await;
    st.mgr_progress = if st.mgr_total > 0 {
        st.mgr_ok as f64 / st.mgr_total as f64
    } else {
        0.0
    };

    let filter = match s.mgr_filter.as_str() {
        "missing" => format!(" AND {missing_sql}"),
        "all" => String::new(),
        // "synced" on the lyrics tab, "tagged" on the tags tab -- one key each,
        // and both mean the same thing to the query.
        _ => format!(" AND {ok_sql}"),
    };
    let shown = match s.mgr_filter.as_str() {
        "missing" => st.mgr_missing,
        "all" => st.mgr_total,
        _ => st.mgr_ok,
    };
    st.mgr_pages = (shown.max(1) as f64 / MGR_PAGE as f64).ceil() as i64;
    let page = s.mgr_page.min((st.mgr_pages - 1).max(0));
    let sql = format!(
        "{TRACK_SELECT}{MUSIC_WHERE}{filter} ORDER BY COALESCE(tm.title, '') LIMIT ? OFFSET ?"
    );
    let rows = sqlx::query_as::<_, TrackRow>(&sql)
        .bind(MGR_PAGE)
        .bind(page * MGR_PAGE)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    st.mgr_rows = rows
        .into_iter()
        .map(|r| {
            let track = into_track(r);
            MgrRow {
                status: mgr_status_of(&s.mgr_tab, &track),
                track,
            }
        })
        .collect();
}

/// The read-only view and the preview draw from the same two fields: one reads
/// the library, the other the candidate that has not been saved yet.
async fn mgr_load_preview(pool: &sqlx::SqlitePool, s: &Session, st: &mut MusicState) {
    let picked = s.mgr_picked.and_then(|i| s.mgr_bodies.get(i).cloned());
    let (synced, content) = match picked {
        Some(body) => body,
        None if s.mgr_mode == "view" && s.mgr_item_id != 0 => {
            let row: Option<(i64, String)> =
                sqlx::query_as("SELECT synced, content FROM lyrics WHERE item_id = ?")
                    .bind(s.mgr_item_id)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten();
            row.map(|(sy, c)| (sy == 1, c)).unwrap_or((false, String::new()))
        }
        None => (false, String::new()),
    };
    st.mgr_view_plain = content.clone();
    st.mgr_view_rows = if synced {
        tulipix_music::lyrics::parse_lrc(&content)
            .into_iter()
            .map(|(at_ms, text)| LyricLine { at_ms, text })
            .collect()
    } else {
        Vec::new()
    };
    st.mgr_dirty = s.mgr_picked.is_some();
}

/// Open the search form for one song, seeded from its tags.
async fn mgr_search_open(item_id: i64) -> Result<()> {
    let pool = music_pool().await?;
    let row: Option<(String, String, String)> = sqlx::query_as(
        "SELECT COALESCE(tm.title, ''), COALESCE(ar.name, ''), COALESCE(al.title, '') \
         FROM track_meta tm \
         LEFT JOIN artists ar ON ar.id = tm.artist_id \
         LEFT JOIN albums al ON al.id = tm.album_id \
         WHERE tm.item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let (title, artist, album) = row.unwrap_or_default();
    let mut s = lock();
    s.mgr_item_id = item_id;
    s.mgr_title = title.clone();
    s.mgr_mode = "search".into();
    s.mgr_q_name = title;
    s.mgr_q_artist = artist;
    s.mgr_q_album = album;
    s.mgr_results.clear();
    s.mgr_bodies.clear();
    s.mgr_picked = None;
    Ok(())
}

/// LRCLIB `/search`, which unlike `/get` matches loosely and returns a list.
///
/// The bodies come down with the results, so picking one is a preview rather
/// than a second round trip -- and Save has something to write without asking
/// the network again.
async fn mgr_search() {
    let (name, artist, album) = {
        let mut s = lock();
        s.mgr_searching = true;
        s.mgr_status = "Searching LRCLIB…".into();
        s.mgr_results.clear();
        s.mgr_bodies.clear();
        s.mgr_picked = None;
        (
            s.mgr_q_name.clone(),
            s.mgr_q_artist.clone(),
            s.mgr_q_album.clone(),
        )
    };
    let q = format!("{name} {artist}");
    let q = q.trim();
    if q.is_empty() {
        let mut s = lock();
        s.mgr_searching = false;
        s.mgr_status = "Type a song name first.".into();
        return;
    }
    let base = tulipix_music::lyrics::LRCLIB_BASE;
    let url = format!(
        "{base}/search?q={}&album_name={}",
        q.replace(' ', "+"),
        album.replace(' ', "+")
    );
    let resp = tulipix_core::net::http().get(&url).send().await;
    let body: Option<serde_json::Value> = match resp {
        Ok(r) => r.json().await.ok(),
        Err(_) => None,
    };

    let mut hits = Vec::new();
    let mut bodies = Vec::new();
    if let Some(serde_json::Value::Array(items)) = body {
        for item in items.into_iter().take(30) {
            let synced = item["syncedLyrics"].as_str().unwrap_or("").to_string();
            let plain = item["plainLyrics"].as_str().unwrap_or("").to_string();
            if synced.trim().is_empty() && plain.trim().is_empty() {
                continue;
            }
            let is_synced = !synced.trim().is_empty();
            hits.push(LyricHit {
                title: item["trackName"].as_str().unwrap_or("").to_string(),
                sub: format!(
                    "{} · {}",
                    item["artistName"].as_str().unwrap_or(""),
                    item["albumName"].as_str().unwrap_or("")
                ),
                kind: if is_synced { "Synced".into() } else { "Plain".into() },
            });
            bodies.push((is_synced, if is_synced { synced } else { plain }));
        }
    }
    let mut s = lock();
    s.mgr_searching = false;
    s.mgr_status = if hits.is_empty() {
        "No results.".into()
    } else {
        format!("{} results", hits.len())
    };
    s.mgr_results = hits;
    s.mgr_bodies = bodies;
}

/// Write the previewed candidate to the library.
async fn mgr_save() -> Result<()> {
    let (item_id, body) = {
        let s = lock();
        (
            s.mgr_item_id,
            s.mgr_picked.and_then(|i| s.mgr_bodies.get(i).cloned()),
        )
    };
    let Some((synced, content)) = body else {
        return Ok(());
    };
    let pool = music_pool().await?;
    tulipix_music::lyrics::store(pool, item_id, &content, synced, "lrclib").await?;
    let mut s = lock();
    s.mgr_picked = None;
    s.mgr_mode = "list".into();
    s.mgr_results.clear();
    s.mgr_bodies.clear();
    s.mgr_status = "Saved.".into();
    // The track on the deck may be the one that just got words.
    drop(s);
    emit(MusicEvent::TrackChanged);
    Ok(())
}

/// Fetch words for everything with none, in the background.
///
/// Detached, because a hundred LRCLIB round trips inside one dispatch would
/// hold the command channel for minutes. Progress goes into the manager's own
/// fields and not onto `ScanProgress`: the page-wide strip belongs to a library
/// scan, and a bulk fetch started from inside a modal should report inside that
/// modal rather than pushing a second progress bar over the grid behind it.
/// `TrackChanged` is what pulls a fresh snapshot while it runs.
fn mgr_sync_all() {
    tokio::spawn(async move {
        let Ok(pool) = music_pool().await else { return };
        let sql = format!("SELECT i.id{MGR_FROM}{MUSIC_WHERE} AND ly.item_id IS NULL LIMIT 500");
        let ids: Vec<i64> = sqlx::query_scalar(&sql)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
        let total = ids.len();
        if total == 0 {
            lock().mgr_status = "Nothing missing.".into();
            emit(MusicEvent::TrackChanged);
            return;
        }
        for (n, id) in ids.into_iter().enumerate() {
            {
                let mut s = lock();
                s.mgr_busy = true;
                s.mgr_status = format!("Fetching lyrics… {} of {total}", n + 1);
            }
            emit(MusicEvent::TrackChanged);
            let _ = fetch_lyrics(id).await;
        }
        {
            let mut s = lock();
            s.mgr_busy = false;
            s.mgr_status = format!("Fetched {total} of {total}.");
        }
        emit(MusicEvent::TrackChanged);
    });
}

/// Open the album or artist page of the track on the deck.
///
/// A single scalar read per click, not a field on `NowPlaying`: the ids are
/// wanted twice in the life of a track and the struct crosses the bridge on
/// every snapshot.
async fn open_now_detail(kind: &str) -> Result<()> {
    let item_id = mpv::now_playing().item_id;
    if item_id == 0 {
        return Ok(());
    }
    open_item_detail(item_id, kind).await
}

/// Where a library item actually lives on disk.
async fn track_path(pool: &sqlx::SqlitePool, item_id: i64) -> Result<String> {
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    path.ok_or_else(|| anyhow::anyhow!("that track is not in the library any more"))
}

/// The same, for any track: the right-click menu offers "View album" and "View
/// artist" on a row that is not the one playing.
async fn open_item_detail(item_id: i64, kind: &str) -> Result<()> {
    let pool = music_pool().await?;
    let column = if kind == "album" { "album_id" } else { "artist_id" };
    let id: Option<i64> =
        sqlx::query_scalar(&format!("SELECT {column} FROM track_meta WHERE item_id = ?"))
            .bind(item_id)
            .fetch_optional(pool)
            .await?
            .flatten();
    let Some(id) = id else {
        anyhow::bail!("this track has no {kind}");
    };
    open_detail(kind, id, String::new());
    Ok(())
}

/// Lyrics for a track that has none, in the background.
///
/// Slint calls `load_music_lyrics` on every track change, which is a DB read
/// and, on a miss, an LRCLIB fetch that gets cached. The port only ever read
/// the DB, so a track whose words had never been fetched showed an empty panel
/// until someone pressed a button -- every track, every time, for a library
/// nobody had fetched lyrics for. This is the fetch half.
///
/// `TrackChanged` is what says the words arrived. There is no narrower event,
/// and inventing one to save a snapshot the deck was about to ask for anyway
/// would be a bridge regeneration for nothing.
async fn ensure_lyrics(item_id: i64) {
    let Ok(pool) = music_pool().await else { return };
    let stored: Option<(String,)> =
        sqlx::query_as("SELECT content FROM lyrics WHERE item_id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    if stored.map(|(c,)| !c.trim().is_empty()).unwrap_or(false) {
        return;
    }
    if fetch_lyrics(item_id).await.is_ok() {
        // Only if the track is still the one we fetched for: a fast skip
        // through an album would otherwise put the last song's words under the
        // current one.
        if mpv::now_playing().item_id == item_id {
            emit(MusicEvent::TrackChanged);
        }
    }
}

fn open_detail(kind: &str, id: i64, key: String) {
    let mut s = lock();
    s.detail_open = true;
    s.detail_kind = kind.into();
    s.detail_id = id;
    s.detail_key = key;
}

/// Push a changed EQ onto the running process. mpv accepts `af` as a property,
/// so this does not need the respawn a device or ReplayGain change does.
/// The `ebur128` meter, labelled so its property name is stable, appended to
/// whatever the equalizer contributes. This is what gives the visualizer its
/// energy — the bar shape is synthetic, the pulse is the real audio.
fn full_af(eq_af: &str) -> String {
    const VIS: &str = "@vis:ebur128=metadata=1:video=0";
    if eq_af.is_empty() {
        VIS.to_string()
    } else {
        format!("{eq_af},{VIS}")
    }
}

fn apply_eq_live(eq: &tulipix_music::eq::Equalizer) {
    let af = full_af(&eq.mpv_af());
    let value = serde_json::to_string(&af).unwrap_or_else(|_| "\"\"".into());
    mpv::set_property("af", &value);
}

/// Stop (or fade) when the sleep timer is up. Called from every snapshot.
fn check_sleep() {
    let deadline = lock().sleep_deadline;
    let Some(deadline) = deadline else { return };
    let now = std::time::Instant::now();
    if now >= deadline {
        mpv::stop();
        let mut s = lock();
        s.sleep_deadline = None;
        s.sleep_min = 0;
        emit(MusicEvent::TrackChanged);
        return;
    }
    // The last thirty seconds ramp the volume down, which is the whole point
    // of a sleep timer rather than a kill switch.
    let left = deadline.duration_since(now).as_secs_f64();
    if left < 30.0 {
        let base: f64 = setting("music.volume", "80").parse().unwrap_or(80.0);
        mpv::set_property("volume", &format!("{:.0}", base * (left / 30.0)));
    }
}

// ---------------------------------------------------------------- library ----

// The watched-folder list is a JSON array of paths in the config dir. This is
// `tulipix_common::{load,save,add}_watched_folder` minus the slint dependency
// that crate carries — same file, same format, so both builds see one list.
// Photos reads the same file; a folder of music inside a photo library is
// filtered out by extension, not by a second list.
fn watched_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

fn load_watched_folders() -> Vec<PathBuf> {
    let Some(p) = watched_path() else { return Vec::new() };
    let Ok(body) = std::fs::read_to_string(p) else { return Vec::new() };
    serde_json::from_str::<Vec<String>>(&body)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn save_watched_folders(folders: &[PathBuf]) {
    let Some(p) = watched_path() else { return };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let list: Vec<String> = folders
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    match serde_json::to_string_pretty(&list) {
        Ok(body) => {
            if let Err(e) = std::fs::write(p, body) {
                tracing::warn!(error = %e, "write watched folders");
            }
        }
        Err(e) => tracing::warn!(error = %e, "serialise watched folders"),
    }
}

/// `pub(crate)`: the Downloader adds its destination to the same list, so a
/// download outside the library root is still watched.
pub(crate) fn add_watched_folder(dir: &Path) {
    let mut existing = load_watched_folders();
    if existing.iter().any(|p| p == dir) {
        return;
    }
    existing.push(dir.to_path_buf());
    save_watched_folders(&existing);
}

/// Which folders have been reassigned away from My Music. Same file the Slint
/// build reads and writes -- `music_folder_sections.json` -- so a folder added
/// to Audiobooks in either build is an audiobook folder in both. The key
/// normalisation (no trailing separator) is the shared writer's, and getting it
/// wrong is what used to make the flag pass match nothing.
fn folder_sections() -> std::collections::HashMap<String, String> {
    tulipix_common::load_folder_sections()
}

/// Push the folder→section map into `is_audiobook`. The tag is written when the
/// folder is picked; the `track_meta` rows it applies to only exist after the
/// scan, so this has to run at the end of one or the tag means nothing.
async fn apply_folder_sections(pool: &sqlx::SqlitePool) {
    let sections = folder_sections();
    if let Err(e) = tulipix_music::audiobooks::apply_folder_sections(pool, &sections).await {
        tracing::warn!(error = %e, "audiobook section flags");
    }
}

/// Audio extensions the scanner accepts. Not a second list: it is exactly what
/// `tulipix_music::formats::FORMATS` declares, so the audit table in Settings
/// and the accept-filter can never disagree.
fn is_audio(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(tulipix_music::formats::is_supported)
        .unwrap_or(false)
}

/// Walk every watched root as a music library, prune the non-audio rows the
/// shared populator inserted, and seed a `track_meta` row for each survivor so
/// the browse joins are cheap.
pub(crate) async fn scan_watched() -> Result<()> {
    let pool = music_pool().await?;
    let roots = load_watched_folders();
    let total = roots.len() as i64;
    let mut inserted = 0i64;
    let mut updated = 0i64;
    let mut missing = 0i64;
    for (i, dir) in roots.iter().enumerate() {
        emit(MusicEvent::ScanProgress {
            root: dir.to_string_lossy().into_owned(),
            done: i as i64,
            total,
        });
        let lib = tulipix_core::libraries::Library {
            id: dir.to_string_lossy().into_owned(),
            path: dir.clone(),
            section: tulipix_core::libraries::Section::Music,
            last_scan: None,
            item_count: 0,
            size_bytes: 0,
            exclude_globs: Vec::new(),
            cadence_override: Some(tulipix_core::libraries::ScanCadence::Manual),
            realtime_notify: false,
        };
        let stats = match tulipix_core::populator::populate(pool, &lib).await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, root = %dir.display(), "music scan failed");
                continue;
            }
        };
        inserted += stats.inserted as i64;
        updated += stats.updated as i64;
        missing += stats.missing as i64;

        let prefix = format!("{}%", dir.to_string_lossy());
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, abs_path FROM items WHERE section = 'music' AND abs_path LIKE ?",
        )
        .bind(&prefix)
        .fetch_all(pool)
        .await?;
        for (id, p) in rows {
            if is_audio(Path::new(&p)) {
                sqlx::query("INSERT OR IGNORE INTO track_meta (item_id) VALUES (?)")
                    .bind(id)
                    .execute(pool)
                    .await?;
                // `folder` is what the Folders tab groups on, and it is only
                // ever derived here.
                let folder = Path::new(&p)
                    .parent()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default();
                sqlx::query("UPDATE track_meta SET folder = ? WHERE item_id = ? AND (folder IS NULL OR folder = '')")
                    .bind(&folder)
                    .bind(id)
                    .execute(pool)
                    .await?;
            } else {
                sqlx::query("DELETE FROM items WHERE id = ?")
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
        }
    }
    // Before the finish event, because every My Music view the UI rebuilds on
    // it filters on `is_audiobook` and the rows to flag only exist now.
    apply_folder_sections(pool).await;
    emit(MusicEvent::ScanFinished {
        inserted,
        updated,
        missing,
    });
    // A newly-scanned library has rows and no metadata; reading it is the half
    // that makes the section usable, so it follows the walk rather than
    // waiting for a second button.
    read_missing_tags().await
}

/// ffprobe every music item that has no title yet, and upsert what comes back.
/// Chunked and progress-reported: a first scan of a large library is minutes of
/// process spawns, and a UI with no sign of life during it looks hung.
async fn read_missing_tags() -> Result<()> {
    let pool = music_pool().await?;
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT i.id, i.abs_path FROM items i \
         LEFT JOIN track_meta tm ON tm.item_id = i.id \
         WHERE i.section = 'music' AND i.missing_since IS NULL \
           AND (tm.item_id IS NULL OR tm.title IS NULL OR tm.title = '')",
    )
    .fetch_all(pool)
    .await?;
    let total = rows.len() as i64;
    if total == 0 {
        return Ok(());
    }
    for (i, (id, path)) in rows.into_iter().enumerate() {
        if i % 20 == 0 {
            emit(MusicEvent::ScanProgress {
                root: "tags".into(),
                done: i as i64,
                total,
            });
        }
        let p = PathBuf::from(&path);
        let tags = tokio::task::spawn_blocking(move || ffprobe_tags(&p)).await?;
        if let Err(e) = tulipix_music::scan::upsert_track(pool, id, &path, &tags).await {
            tracing::warn!(error = %e, path = %path, "tag upsert failed");
        }
    }
    yt_fetch_set(false, 1.0, "");
    emit(MusicEvent::ScanFinished {
        inserted: 0,
        updated: total,
        missing: 0,
    });
    Ok(())
}

/// Read audio tags via the bundled (or PATH) ffprobe. The parsing rules —
/// which key means what, how a date becomes a year, what a track number looks
/// like — are `tulipix_music::tags`; only the process spawn is here.
fn ffprobe_tags(path: &Path) -> tulipix_music::tags::TrackTags {
    use tulipix_music::tags::{self, TrackTags};
    let mut t = TrackTags::default();
    let out = std::process::Command::new(tulipix_core::thumbs::tool_bin("ffprobe"))
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .no_window_compat()
        .output();
    let Ok(out) = out else { return fallback_title(t, path) };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        return fallback_title(t, path);
    };
    let fmt = &json["format"];
    let tagv = &fmt["tags"];
    // ffmpeg emits TITLE for some containers and title for others.
    let get = |k: &str| -> Option<String> {
        tagv.as_object().and_then(|o| {
            o.iter()
                .find(|(kk, _)| kk.to_lowercase() == k)
                .and_then(|(_, v)| v.as_str())
                .map(|s| s.to_string())
        })
    };
    t.title = get("title").and_then(|s| tags::clean(&s));
    t.artist = get("artist").and_then(|s| tags::clean(&s));
    t.album = get("album").and_then(|s| tags::clean(&s));
    t.album_artist = get("album_artist").and_then(|s| tags::clean(&s));
    t.genre = get("genre").and_then(|s| tags::clean(&s));
    t.year = get("date").or_else(|| get("year")).and_then(|s| tags::parse_year(&s));
    t.track_no = get("track").and_then(|s| tags::parse_track_no(&s));
    t.disc_no = get("disc").and_then(|s| tags::parse_track_no(&s));
    t.duration_s = fmt["duration"].as_str().and_then(|s| s.parse().ok());
    if let Some(streams) = json["streams"].as_array() {
        if let Some(a) = streams.iter().find(|s| s["codec_type"] == "audio") {
            t.codec = a["codec_name"].as_str().map(|s| s.to_string());
            t.sample_rate = a["sample_rate"].as_str().and_then(|s| s.parse().ok());
            t.channels = a["channels"].as_i64();
            t.bitrate = a["bit_rate"].as_str().and_then(|s| s.parse().ok());
        }
    }
    t.container = path
        .extension()
        .and_then(|e| e.to_str())
        .and_then(tags::container_from_ext)
        .map(|s| s.to_string());
    fallback_title(t, path)
}

fn fallback_title(
    mut t: tulipix_music::tags::TrackTags,
    path: &Path,
) -> tulipix_music::tags::TrackTags {
    if t.title.is_none() {
        t.title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string());
    }
    t
}

/// Write tags back into the file with ffmpeg: stream-copy to a sibling temp,
/// then rename over the original, so a decode failure leaves the source alone.
#[allow(clippy::too_many_arguments)]
fn write_file_tags(
    path: &str,
    title: &str,
    artist: &str,
    album: &str,
    album_artist: &str,
    genre: &str,
    date: &str,
    track_no: i64,
    disc_no: i64,
) {
    let src = Path::new(path);
    let Some(ext) = src.extension().and_then(|e| e.to_str()) else { return };
    let tmp = src.with_extension(format!("tulipix-tmp.{ext}"));
    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("ffmpeg"));
    cmd.no_window_compat();
    cmd.args(["-v", "error", "-y", "-i"])
        .arg(src)
        .args(["-map_metadata", "0", "-c", "copy"]);
    for (key, val) in [
        ("title", title),
        ("artist", artist),
        ("album", album),
        ("album_artist", album_artist),
        ("genre", genre),
        ("date", date),
    ] {
        if !val.is_empty() {
            cmd.arg("-metadata").arg(format!("{key}={val}"));
        }
    }
    if track_no > 0 {
        cmd.arg("-metadata").arg(format!("track={track_no}"));
    }
    if disc_no > 0 {
        cmd.arg("-metadata").arg(format!("disc={disc_no}"));
    }
    cmd.arg(&tmp);
    let ok = matches!(cmd.status(), Ok(st) if st.success())
        && std::fs::metadata(&tmp).map(|m| m.len() > 0).unwrap_or(false);
    if ok {
        if let Err(e) = std::fs::rename(&tmp, src) {
            tracing::warn!(error = %e, "tag write-back rename failed");
            let _ = std::fs::remove_file(&tmp);
        }
    } else {
        let _ = std::fs::remove_file(&tmp);
    }
}

/// The two smart playlists the Slint build offers, built through the domain
/// crate's rule evaluator rather than hand-written SQL.
/// The newest hundred tracks in the library, as a playlist, rewritten from
/// scratch every time it is opened.
///
/// Rebuilt rather than appended to: "recently added" is a window that moves,
/// and a playlist that only ever grows would be "everything ever added" within
/// a month.
async fn rebuild_fresh_playlist(pool: &sqlx::SqlitePool) -> Result<i64> {
    const NAME: &str = "Recently added";
    const KEEP: i64 = 100;
    let id = match tulipix_music::playlists::find_by_name(pool, NAME).await? {
        Some(id) => id,
        None => tulipix_music::playlists::create(pool, NAME, None).await?,
    };
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let ids: Vec<i64> = sqlx::query_scalar(&format!(
        "SELECT i.id FROM items i JOIN track_meta tm ON tm.item_id = i.id         {MUSIC_WHERE} ORDER BY i.added DESC LIMIT ?"
    ))
    .bind(KEEP)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for item in ids {
        tulipix_music::playlists::append(pool, id, item).await?;
    }
    Ok(id)
}

async fn create_smart_playlist(pool: &sqlx::SqlitePool, kind: &str) -> Result<()> {
    use tulipix_music::playlists::{Combine, Condition, Field, Op, SmartRule};
    let (name, rule) = match kind {
        "loved" => (
            "Loved",
            SmartRule {
                combine: Combine::All,
                conditions: vec![Condition {
                    field: Field::Loved,
                    op: Op::Eq,
                    value: "1".into(),
                }],
                limit: None,
            },
        ),
        "recent" => (
            "Recently played",
            SmartRule {
                combine: Combine::All,
                conditions: vec![Condition {
                    field: Field::PlayCount,
                    op: Op::Gt,
                    value: "0".into(),
                }],
                limit: Some(100),
            },
        ),
        other => anyhow::bail!("unknown smart playlist: {other}"),
    };
    if tulipix_music::playlists::find_by_name(pool, name)
        .await?
        .is_some()
    {
        anyhow::bail!("\"{name}\" already exists");
    }
    tulipix_music::playlists::create(pool, name, Some(&rule)).await?;
    Ok(())
}

/// LRCLIB lookup for one track, stored through the domain crate so both builds
/// read the same `lyrics` rows.
async fn fetch_lyrics(item_id: i64) -> Result<()> {
    let pool = music_pool().await?;
    let row: Option<(String, String, String, f64)> = sqlx::query_as(
        "SELECT COALESCE(ar.name, ''), COALESCE(tm.title, ''), COALESCE(al.title, ''), \
                COALESCE(tm.duration_s, 0.0) \
         FROM track_meta tm \
         LEFT JOIN artists ar ON ar.id = tm.artist_id \
         LEFT JOIN albums al ON al.id = tm.album_id \
         WHERE tm.item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((artist, title, album, dur)) = row else {
        anyhow::bail!("track {item_id} is not in the library");
    };
    if title.is_empty() {
        anyhow::bail!("this track has no title to search with");
    }
    let url = tulipix_music::lyrics::get_url(&artist, &title, &album, dur);
    let hit: tulipix_music::lyrics::LrclibHit = tulipix_core::net::http()
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if let Some(synced) = hit.synced_lyrics.filter(|s| !s.trim().is_empty()) {
        tulipix_music::lyrics::store(pool, item_id, &synced, true, "lrclib").await?;
    } else if let Some(plain) = hit.plain_lyrics.filter(|s| !s.trim().is_empty()) {
        tulipix_music::lyrics::store(pool, item_id, &plain, false, "lrclib").await?;
    } else {
        anyhow::bail!("LRCLIB has no lyrics for this track");
    }
    Ok(())
}

/// Every bookmark in the open book, chapter-ordered, with its "Ch 3 · 12:40"
/// label pre-built — the chapter index needs the book's track order, which is
/// a query Dart does not have.
async fn book_bookmarks(pool: &sqlx::SqlitePool) -> Result<Vec<Bookmark>> {
    let folder = lock().book_open.clone();
    if folder.is_empty() {
        return Ok(Vec::new());
    }
    let ids = tulipix_music::audiobooks::book_chapters(pool, &folder).await?;
    let raw = tulipix_music::audiobooks::book_bookmarks(pool, &ids).await?;
    Ok(raw
        .into_iter()
        .map(|(item_id, position_s, label)| {
            let chapter = ids.iter().position(|i| *i == item_id).unwrap_or(0) + 1;
            Bookmark {
                item_id,
                position_s,
                label,
                when: format!("Ch {chapter} · {}", fmt_clock(position_s)),
            }
        })
        .collect())
}

/// Seconds as m:ss (or h:mm:ss past an hour). Formatted here rather than in
/// Dart so the two builds round the same way.
fn fmt_clock(secs: f64) -> String {
    let s = secs.max(0.0) as i64;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

// --------------------------------------------------------------- podcasts ----

/// Fetch a feed and hand it to the domain crate's upsert. The XML parsing,
/// the episode pruning and the "keep what the user touched" rule all live
/// there; what is here is one HTTP GET.
async fn subscribe_podcast(url: &str) -> Result<()> {
    let url = url.trim();
    if !url.starts_with("http") {
        anyhow::bail!("that is not a feed URL");
    }
    let body = tulipix_core::net::http()
        .get(url)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await?
        .error_for_status()?
        .text()
        .await?;
    let feed = tulipix_music::podcasts::parse_feed(&body);
    if feed.episodes.is_empty() && feed.title.is_none() {
        anyhow::bail!("nothing at that URL looks like a podcast feed");
    }
    let pool = podcasts_pool().await?;
    // Progress, not a spinner: a back catalogue is routinely hundreds of
    // episodes and the insert loop is the slow part, so the bar is the honest
    // answer to "is it stuck". Throttled to ~40 repaints however long the feed.
    tulipix_music::podcasts::subscribe_with_progress(pool, url, &feed, |done, total| {
        let step = (total / 40).max(1);
        if total > 0 && (done % step == 0 || done >= total) {
            pod_job_set(true, done as f64 / total as f64, &format!("Adding… {done}/{total}"));
        }
    })
    .await?;
    Ok(())
}

async fn refresh_feed(podcast_id: i64, url: &str) -> Result<()> {
    let _ = podcast_id;
    subscribe_podcast(url).await
}

/// Save one episode for offline. Streams to disk through the long-transfer
/// client: a two-hour episode is not a request a total timeout should abort.
async fn download_episode(episode_id: i64) -> Result<()> {
    let pool = podcasts_pool().await?;
    let row: Option<(String, String)> = sqlx::query_as(
        "SELECT audio_url, COALESCE(title, '') FROM podcast_episodes WHERE id = ?",
    )
    .bind(episode_id)
    .fetch_optional(pool)
    .await?;
    let Some((url, title)) = row else {
        anyhow::bail!("episode {episode_id} is gone");
    };
    let ext = url
        .rsplit('.')
        .next()
        .filter(|e| e.len() <= 4 && !e.contains('/'))
        .unwrap_or("mp3");
    let dest = podcast_offline_dir().join(format!("ep-{episode_id}.{ext}"));
    // Chunked rather than `.bytes()`: an episode is routinely 80MB and the row
    // showed nothing at all until the whole thing had landed. `chunk()` is on
    // the plain response, so this needs no stream crate.
    let mut resp = tulipix_core::net::http_stream()
        .get(&url)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await?
        .error_for_status()?;
    let total = resp.content_length().unwrap_or(0);
    let mut got: u64 = 0;
    let mut file = std::fs::File::create(&dest)?;
    pod_dl_set(episode_id, 0.0, &title);
    let wrote = async {
        use std::io::Write;
        while let Some(chunk) = resp.chunk().await? {
            file.write_all(&chunk)?;
            got += chunk.len() as u64;
            if total > 0 {
                pod_dl_set(episode_id, (got as f64 / total as f64).clamp(0.0, 1.0), &title);
            }
        }
        file.flush()?;
        Ok::<(), anyhow::Error>(())
    }
    .await;
    drop(file);
    if let Err(e) = wrote {
        // A half-written file is worse than none: it would play as a truncated
        // episode and `mark_downloaded` never runs to say otherwise.
        let _ = std::fs::remove_file(&dest);
        pod_dl_clear();
        return Err(e);
    }
    pod_dl_clear();
    tulipix_music::podcasts::mark_downloaded(
        pool,
        episode_id,
        &dest.to_string_lossy(),
    )
    .await?;
    Ok(())
}

// ------------------------------------------------------------------ radio ----

/// Emoji for the curated categories, index-aligned to `radio::PRESETS`. The
/// Slint page carries the same list; it is decoration, so it lives on the
/// presenting side rather than in the domain crate.
fn radio_icon(label: &str) -> &'static str {
    match label.to_ascii_lowercase().as_str() {
        l if l.contains("news") => "📰",
        l if l.contains("talk") => "🎙",
        l if l.contains("sport") => "⚽",
        l if l.contains("class") => "🎻",
        l if l.contains("jazz") => "🎷",
        l if l.contains("rock") => "🎸",
        l if l.contains("pop") => "🎤",
        l if l.contains("dance") || l.contains("electro") => "🎛",
        l if l.contains("devot") || l.contains("bhaj") => "🕉",
        l if l.contains("bolly") || l.contains("hindi") => "🎬",
        l if l.contains("tamil") || l.contains("telugu") || l.contains("malay") => "🪘",
        l if l.contains("retro") || l.contains("old") => "📻",
        _ => "🎵",
    }
}

/// Open one curated category: the cache first, the network only when it is
/// empty. Refresh is the explicit way to go to the network for all of them.
async fn open_radio_category(index: i64) -> Result<()> {
    let pool = radio_pool().await?;
    let presets = tulipix_music::radio::PRESETS;
    let Some((label, query)) = presets.get(index.max(0) as usize) else {
        anyhow::bail!("no category at {index}");
    };
    let mut list = tulipix_music::radio::load_cache(pool, label)
        .await
        .unwrap_or_default();
    if list.is_empty() {
        list = fetch_stations(&tulipix_music::radio::browse_url(query, 100)).await?;
        let _ = tulipix_music::radio::save_cache(pool, label, &list).await;
    }
    let mut s = lock();
    s.radio_cat_open = true;
    s.radio_cat_title = (*label).to_string();
    s.radio_list = list;
    s.radio_page = 0;
    Ok(())
}

async fn fetch_stations(url: &str) -> Result<Vec<tulipix_music::radio::Station>> {
    let list: Vec<tulipix_music::radio::Station> = tulipix_core::net::http()
        .get(url)
        // radio-browser asks callers to identify themselves, and unlike the
        // media hosts it is happier with a real name than a browser string.
        .header(reqwest::header::USER_AGENT, "Tulipix/1.0")
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    Ok(list)
}

async fn radio_search(query: &str) -> Result<()> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(());
    }
    let pool = radio_pool().await?;
    // The cache first, so a search offline still finds the stations already
    // seen. Only fall through to the network when it has nothing.
    let mut list = tulipix_music::radio::search_cache(pool, q)
        .await
        .unwrap_or_default();
    if list.is_empty() {
        list = fetch_stations(&tulipix_music::radio::search_url(q, 100)).await?;
    }
    let mut s = lock();
    s.radio_cat_open = true;
    s.radio_cat_title = format!("Search “{q}”");
    s.radio_list = list;
    s.radio_page = 0;
    Ok(())
}

/// Re-fetch every curated category into the cache. Progress-reported: it is a
/// dozen round trips and the page should say so.
async fn radio_refresh_all() -> Result<()> {
    let pool = radio_pool().await?;
    let presets = tulipix_music::radio::PRESETS;
    let total = presets.len() as i64;
    for (i, (label, query)) in presets.iter().enumerate() {
        emit(MusicEvent::ScanProgress {
            root: (*label).to_string(),
            done: i as i64,
            total,
        });
        match fetch_stations(&tulipix_music::radio::browse_url(query, 100)).await {
            Ok(list) if !list.is_empty() => {
                let _ = tulipix_music::radio::save_cache(pool, label, &list).await;
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(error = %e, preset = %label, "radio refresh failed"),
        }
    }
    emit(MusicEvent::ScanFinished {
        inserted: 0,
        updated: total,
        missing: 0,
    });
    Ok(())
}

// ---------------------------------------------------------------- youtube ----

/// yt-dlp as JSON. Every YouTube listing goes through it rather than a Piped
/// instance: the setting that chooses between them is a shell concern (phase
/// 4 owns Settings), and yt-dlp is the backend that always answers.
async fn ytdlp_json(args: Vec<String>) -> Option<serde_json::Value> {
    let out = tokio::process::Command::new(tulipix_core::ytdlp::bin())
        .args(tulipix_core::ytdlp::common_args())
        .args(&args)
        .no_window_async()
        .output()
        .await
        .ok()?;
    if !out.status.success() {
        // This returns Option, so the caller draws an empty list either way —
        // but a refused listing and a genuinely empty one look identical on
        // screen, and only one of them is worth a line in the log.
        let stderr = String::from_utf8_lossy(&out.stderr);
        if tulipix_core::ytdlp::is_access_error(&stderr) {
            tracing::warn!(%stderr, "youtube: yt-dlp was refused — update it, or set cookies");
        }
    }
    serde_json::from_slice(&out.stdout).ok()
}

fn yt_pick_thumb(v: &serde_json::Value) -> String {
    if let Some(t) = v.get("thumbnail").and_then(|x| x.as_str()) {
        if !t.is_empty() {
            return t.to_string();
        }
    }
    if let Some(arr) = v.get("thumbnails").and_then(|x| x.as_array()) {
        // Last is largest in yt-dlp's ordering.
        for t in arr.iter().rev() {
            if let Some(u) = t.get("url").and_then(|x| x.as_str()) {
                return u.to_string();
            }
        }
    }
    String::new()
}

fn yt_fmt_count(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1e6)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1e3)
    } else {
        n.to_string()
    }
}

fn yt_entry(e: &serde_json::Value) -> YtVideo {
    let views = e.get("view_count").and_then(|x| x.as_i64()).unwrap_or(0);
    YtVideo {
        video_id: e.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        title: e.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        channel: e
            .get("channel")
            .or_else(|| e.get("uploader"))
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string(),
        thumb: yt_pick_thumb(e),
        duration: e.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64,
        media_path: String::new(),
        meta: if views > 0 {
            format!("{} views", yt_fmt_count(views))
        } else {
            String::new()
        },
        progress: 0.0,
        quality: String::new(),
    }
}

/// A Shorts filter over our own row shape. The domain crate's rule is by
/// duration and title marker; reproduce it here rather than converting both
/// ways through `piped::Video` for one predicate.
fn drop_shorts(v: Vec<YtVideo>) -> Vec<YtVideo> {
    v.into_iter()
        .filter(|x| {
            let short_marker = x.title.to_lowercase().contains("#short");
            !(short_marker || (x.duration > 0 && x.duration <= 60))
        })
        .collect()
}

/// `more` asks for another page. yt-dlp's `ytsearchN:` has no cursor, so the
/// next page is a bigger N with what we already have trimmed off the front --
/// which is exactly what the Slint build does and is why "Load more" is a
/// button rather than an infinite scroll.
async fn yt_search(query: &str, more: bool) -> Result<()> {
    let q = query.trim().to_string();
    if q.is_empty() {
        let mut s = lock();
        s.yt_results.clear();
        s.yt_status.clear();
        s.yt_results_more = false;
        return Ok(());
    }
    let have = if more { lock().yt_results.len() } else { 0 };
    let want = have + YT_HITS;
    lock().yt_status = if more { "Loading more…".into() } else { "Searching…".into() };
    let json = ytdlp_json(vec![
        "--flat-playlist".into(),
        "-J".into(),
        "--no-warnings".into(),
        format!("ytsearch{want}:{q}"),
    ])
    .await;
    let hits = json
        .as_ref()
        .and_then(|j| j.get("entries"))
        .and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry).collect::<Vec<_>>())
        .unwrap_or_default();
    let hits = drop_shorts(hits);
    if let Ok(pool) = youtube_pool().await {
        let _ = tulipix_music::youtube::store::push_recent_search(pool, &q).await;
    }
    let mut s = lock();
    s.yt_status = if hits.is_empty() {
        "No results — is yt-dlp installed?".into()
    } else {
        String::new()
    };
    // A page that came back no longer than what we already had means the
    // search is exhausted, whatever N we asked for.
    s.yt_results_more = hits.len() > have;
    s.yt_results = hits;
    Ok(())
}

/// Resolve a channel URL or `@handle` to `(channel_id, title)`.
async fn yt_channel_from_url(url: &str) -> Result<(String, String)> {
    let url = url.trim();
    if url.is_empty() {
        anyhow::bail!("paste a channel URL or @handle");
    }
    let target = if url.starts_with("http") {
        url.to_string()
    } else {
        format!("https://www.youtube.com/{}", url.trim_start_matches('/'))
    };
    let json = ytdlp_json(vec![
        "--flat-playlist".into(),
        "--playlist-items".into(),
        "1".into(),
        "-J".into(),
        "--no-warnings".into(),
        target,
    ])
    .await
    .ok_or_else(|| anyhow::anyhow!("yt-dlp could not read that channel"))?;
    let pick = |k: &str| json.get(k).and_then(|v| v.as_str()).map(String::from);
    let id = pick("channel_id")
        .or_else(|| pick("uploader_id"))
        .or_else(|| pick("id"))
        .ok_or_else(|| anyhow::anyhow!("that URL has no channel in it"))?;
    let title = pick("channel")
        .or_else(|| pick("uploader"))
        .or_else(|| pick("title"))
        .unwrap_or_else(|| id.clone());
    Ok((id, title))
}

/// Import a YouTube playlist URL as a local playlist, ids and all.
async fn yt_import_playlist_url(url: &str) -> Result<()> {
    let url = url.trim();
    if !url.starts_with("http") {
        anyhow::bail!("that is not a playlist URL");
    }
    lock().yt_status = "Reading playlist…".into();
    let json = ytdlp_json(vec![
        "--flat-playlist".into(),
        "-J".into(),
        "--no-warnings".into(),
        url.to_string(),
    ])
    .await
    .ok_or_else(|| anyhow::anyhow!("yt-dlp could not read that playlist"))?;
    let name = json
        .get("title")
        .and_then(|v| v.as_str())
        .unwrap_or("Imported playlist")
        .to_string();
    let ids: Vec<String> = json
        .get("entries")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| e.get("id").and_then(|v| v.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default();
    if ids.is_empty() {
        anyhow::bail!("that playlist is empty or private");
    }
    let pool = youtube_pool().await?;
    let id = tulipix_music::youtube::store::create_remote_playlist(
        pool,
        &name,
        url,
        ids.len() as i64,
    )
    .await?;
    tulipix_music::youtube::store::add_playlist_ids(pool, id, &ids).await?;
    lock().yt_status = format!("Imported {} videos", ids.len());
    Ok(())
}

/// `pages` is how many 30-video blocks to pull. Opening a channel asks for one;
/// Load more re-asks for one more and replaces the list, because yt-dlp's
/// `--playlist-end` has no cursor to continue from.
async fn yt_open_channel_pages(channel_id: &str, pages: i64) -> Result<()> {
    let mode = lock().yt_channel_mode.clone();
    let url = if mode == "popular" {
        format!("https://www.youtube.com/channel/{channel_id}/videos?view=0&sort=p&flow=grid")
    } else {
        format!("https://www.youtube.com/channel/{channel_id}/videos")
    };
    lock().yt_status = "Loading channel…".into();
    let json = ytdlp_json(vec![
        "--flat-playlist".into(),
        "-J".into(),
        "--no-warnings".into(),
        "--playlist-end".into(),
        (pages.max(1) * CHANNEL_PAGE).to_string(),
        url,
    ])
    .await;
    let title = json
        .as_ref()
        .and_then(|j| j.get("channel").or_else(|| j.get("title")))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    let videos = json
        .as_ref()
        .and_then(|j| j.get("entries"))
        .and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry).collect::<Vec<_>>())
        .unwrap_or_default();
    let videos = drop_shorts(videos);

    // Cache it, so re-opening the channel is instant and works offline.
    if let Ok(pool) = youtube_pool().await {
        let cache: Vec<tulipix_music::youtube::store::ChannelVid> = videos
            .iter()
            .map(|v| tulipix_music::youtube::store::ChannelVid {
                video_id: v.video_id.clone(),
                title: v.title.clone(),
                channel: v.channel.clone(),
                meta: v.meta.clone(),
                info: String::new(),
                thumb_path: v.thumb.clone(),
                duration: v.duration,
            })
            .collect();
        if !cache.is_empty() {
            let _ = if mode == "popular" {
                tulipix_music::youtube::store::set_channel_popular(pool, channel_id, &cache).await
            } else {
                tulipix_music::youtube::store::set_channel_cache(pool, channel_id, &cache).await
            };
        }
    }

    let subscribed = match youtube_pool().await {
        Ok(pool) => tulipix_music::youtube::store::list_subs(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .any(|s| s.channel_id == channel_id && s.subscribed),
        Err(_) => false,
    };

    // Everything the assignment needs is computed before the lock is taken:
    // reading one field of a guard while assigning to another is a borrow the
    // compiler is entitled to refuse.
    let status = if videos.is_empty() {
        "Could not reach YouTube".to_string()
    } else {
        String::new()
    };
    let heading = if title.is_empty() {
        channel_id.to_string()
    } else {
        title
    };
    let mut s = lock();
    s.yt_channel_open = true;
    s.yt_channel_id = channel_id.to_string();
    s.yt_channel_title = heading;
    s.yt_channel_subscribed = subscribed;
    s.yt_status = status;
    // A block that came back short is the end of the channel, whatever we
    // asked for -- which is the only signal yt-dlp gives us.
    s.yt_channel_has_next = videos.len() as i64 >= pages.max(1) * CHANNEL_PAGE;
    s.yt_channel_page = pages.max(1);
    s.yt_channel_videos = videos;
    Ok(())
}

/// Videos per block on a channel page.
const CHANNEL_PAGE: i64 = 30;

async fn yt_open_channel(channel_id: &str) -> Result<()> {
    {
        let mut s = lock();
        s.yt_channel_query.clear();
        s.yt_channel_results.clear();
    }
    yt_open_channel_pages(channel_id, 1).await
}

/// Search within one channel. yt-dlp has no channel-scoped search, so this is
/// a site search pinned to the channel URL.
async fn yt_channel_search(query: &str) -> Result<()> {
    let (id, q) = {
        let mut s = lock();
        s.yt_channel_query = query.trim().to_string();
        (s.yt_channel_id.clone(), s.yt_channel_query.clone())
    };
    if q.is_empty() || id.is_empty() {
        lock().yt_channel_results.clear();
        return Ok(());
    }
    lock().yt_status = "Searching the channel…".into();
    let json = ytdlp_json(vec![
        "--flat-playlist".into(),
        "-J".into(),
        "--no-warnings".into(),
        "--playlist-end".into(),
        "20".into(),
        format!("https://www.youtube.com/channel/{id}/search?query={}", urlish(&q)),
    ])
    .await;
    let hits = json
        .as_ref()
        .and_then(|j| j.get("entries"))
        .and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry).collect::<Vec<_>>())
        .unwrap_or_default();
    let hits = drop_shorts(hits);
    let mut s = lock();
    s.yt_status = if hits.is_empty() {
        "Nothing in this channel matched".into()
    } else {
        String::new()
    };
    s.yt_channel_results = hits;
    Ok(())
}

/// Percent-encode the handful of characters a query can carry that would
/// otherwise end the URL. Not a general encoder -- yt-dlp takes the rest.
fn urlish(q: &str) -> String {
    q.chars()
        .map(|c| match c {
            ' ' => "+".to_string(),
            '&' | '?' | '#' | '%' | '+' | '/' => format!("%{:02X}", c as u8),
            _ => c.to_string(),
        })
        .collect()
}

/// Re-fetch subscriber and video counts for every channel. Manual only: it is
/// one yt-dlp spawn per channel and nothing about it is urgent.
async fn yt_refresh_subs() -> Result<()> {
    let pool = youtube_pool().await?;
    let subs = tulipix_music::youtube::store::list_subs(pool).await?;
    let total = subs.len() as i64;
    for (i, sub) in subs.iter().enumerate() {
        emit(MusicEvent::ScanProgress {
            root: sub.title.clone(),
            done: i as i64,
            total,
        });
        yt_fetch_set(
            true,
            if total > 0 { i as f64 / total as f64 } else { 0.0 },
            &format!("{} ({}/{total})", sub.title, i + 1),
        );
        let url = format!("https://www.youtube.com/channel/{}", sub.channel_id);
        let Some(j) = ytdlp_json(vec![
            "--flat-playlist".into(),
            "-J".into(),
            "--no-warnings".into(),
            "--playlist-items".into(),
            "0".into(),
            url,
        ])
        .await
        else {
            continue;
        };
        let avatar = j
            .get("thumbnails")
            .and_then(|x| x.as_array())
            .and_then(|arr| {
                arr.iter()
                    .find(|t| {
                        t.get("id")
                            .and_then(|x| x.as_str())
                            .map(|s| s.contains("avatar"))
                            .unwrap_or(false)
                    })
                    .or_else(|| arr.first())
            })
            .and_then(|t| t.get("url"))
            .and_then(|x| x.as_str())
            .map(|s| s.to_string());
        let avatar_local = match avatar.as_deref() {
            Some(u) => cache_remote(u).await,
            None => None,
        };
        let _ = tulipix_music::youtube::store::set_sub_meta(
            pool,
            &sub.channel_id,
            avatar_local.as_deref(),
            j.get("playlist_count").and_then(|x| x.as_i64()),
            j.get("channel_follower_count").and_then(|x| x.as_i64()),
        )
        .await;
    }
    emit(MusicEvent::ScanFinished {
        inserted: 0,
        updated: total,
        missing: 0,
    });
    Ok(())
}

/// Download one video at the asked-for quality. "audio" extracts Opus;
/// anything else is a height cap on the muxed file.
/// Downloads in flight or waiting. yt-dlp runs one at a time, so the head of
/// this list is the running one and the rest are queued.
fn yt_jobs() -> &'static Mutex<Vec<YtJob>> {
    static C: std::sync::OnceLock<Mutex<Vec<YtJob>>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// The video on screen: `(token, video_id)`. `token` is what makes a report
/// from a picture this one replaced identifiable, and therefore ignorable.
fn yt_watch() -> &'static Mutex<(i64, String)> {
    static C: std::sync::OnceLock<Mutex<(i64, String)>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new((0, String::new())))
}

/// Resolve a watchable stream and hand it to the in-app player.
///
/// Audio stops first: libmpv playing the soundtrack twice, once per deck, is
/// the obvious failure and there is no reason to keep the audio around when
/// the picture carries it. `start` is where the audio had got to, so the video
/// opens on the same frame you were listening to.
async fn yt_watch_video(video_id: &str, height: i64, start: f64) -> Result<()> {
    let fmt = if height <= 0 {
        "best".to_string()
    } else {
        // `best[height<=N]` alone fails on videos with no muxed stream at that
        // cap; the `/best` fallback is what keeps those playable.
        format!("best[height<={height}]/best")
    };
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let out = tokio::process::Command::new(tulipix_core::ytdlp::bin())
        .args(["-g", "-f", fmt.as_str(), "--no-playlist"])
        .args(tulipix_core::ytdlp::common_args())
        .arg(&url)
        .no_window_async()
        .output()
        .await?;
    let src = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{}",
                tulipix_core::ytdlp::friendly_error(&String::from_utf8_lossy(&out.stderr))
            )
        })?;

    mpv::stop();
    let token = {
        let mut g = yt_watch()
            .lock()
            .map_err(|_| anyhow::anyhow!("watch state is poisoned"))?;
        // Any non-zero, ever-increasing value; the wall clock is one we have.
        let token = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(1)
            .max(g.0 + 1);
        *g = (token, video_id.to_string());
        token
    };
    emit(MusicEvent::VideoStop);
    emit(MusicEvent::VideoPlay {
        token,
        src,
        start_at: start.max(0.0),
        // A remote stream URL is bound to the agent that resolved it; the
        // default one gets 403 from Google's CDN.
        props: vec![format!("user-agent={}", tulipix_core::net::BROWSER_UA)],
    });
    emit(MusicEvent::TrackChanged);
    Ok(())
}

/// The counts-refresh bar. Same shape as `pod_job` and for the same reason:
/// one yt-dlp spawn per channel, forty channels, and nothing on screen.
fn yt_fetch() -> &'static Mutex<(bool, f64, String)> {
    static C: std::sync::OnceLock<Mutex<(bool, f64, String)>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

fn yt_fetch_set(busy: bool, frac: f64, msg: &str) {
    if let Ok(mut g) = yt_fetch().lock() {
        *g = (busy, frac, msg.to_string());
    }
    emit(MusicEvent::Stale);
}

fn yt_job_list() -> Vec<YtJob> {
    yt_jobs().lock().map(|g| g.clone()).unwrap_or_default()
}

fn yt_job_push(video_id: &str, title: &str) {
    if let Ok(mut g) = yt_jobs().lock() {
        if !g.iter().any(|j| j.video_id == video_id) {
            g.push(YtJob {
                video_id: video_id.to_string(),
                title: title.to_string(),
                frac: 0.0,
                running: false,
            });
        }
    }
    emit(MusicEvent::Stale);
}

/// yt-dlp writes a progress line several times a second. Only a change the eye
/// can see is worth waking the UI for, so the repaint is gated on the whole
/// percent moving.
fn yt_job_progress(video_id: &str, frac: f64) {
    let mut changed = false;
    if let Ok(mut g) = yt_jobs().lock() {
        if let Some(j) = g.iter_mut().find(|j| j.video_id == video_id) {
            changed = (frac * 100.0) as i64 != (j.frac * 100.0) as i64 || !j.running;
            j.frac = frac;
            j.running = true;
        }
    }
    if changed {
        emit(MusicEvent::Stale);
    }
}

fn yt_job_done(video_id: &str) {
    if let Ok(mut g) = yt_jobs().lock() {
        g.retain(|j| j.video_id != video_id);
    }
    emit(MusicEvent::Stale);
}

/// `[download]  42.3% of ...` -> 0.423. yt-dlp writes one of these per update
/// when given `--newline`, which is the only reason a bar is possible at all
/// without re-implementing the downloader.
fn yt_dl_percent(line: &str) -> Option<f64> {
    let rest = line.trim().strip_prefix("[download]")?.trim_start();
    let pct = rest.split('%').next()?.trim();
    pct.parse::<f64>().ok().map(|p| (p / 100.0).clamp(0.0, 1.0))
}

async fn yt_download(video_id: &str, quality: &str) -> Result<()> {
    let dir = yt_download_dir();
    let url = format!("https://www.youtube.com/watch?v={video_id}");
    let template = dir.join(format!("{video_id}.%(ext)s"));
    let audio = quality.eq_ignore_ascii_case("audio");
    let mut args: Vec<String> = vec!["--no-playlist".into(), "-o".into()];
    args.push(template.to_string_lossy().into_owned());
    if audio {
        args.extend([
            "-f".into(),
            "bestaudio".into(),
            "-x".into(),
            "--audio-format".into(),
            "opus".into(),
        ]);
    } else if quality.eq_ignore_ascii_case("best") {
        args.extend([
            "-f".into(),
            "bestvideo+bestaudio/best".into(),
            "--merge-output-format".into(),
            "mkv".into(),
        ]);
    } else {
        let height: i64 = quality.trim_end_matches('p').parse().unwrap_or(1080);
        args.extend([
            "-f".into(),
            format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]"),
            "--merge-output-format".into(),
            "mkv".into(),
        ]);
    }
    args.push(url);
    args.push("--newline".into());
    args.extend(tulipix_core::ytdlp::common_args());
    let mut child = tokio::process::Command::new(tulipix_core::ytdlp::bin())
        .args(&args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .no_window_async()
        .spawn()?;
    // Drained rather than inherited: a failed download used to report only
    // "could not download <id>", because the reason went to a pipe nobody read.
    let stderr_task = child.stderr.take().map(|e| {
        tokio::spawn(async move {
            use tokio::io::AsyncReadExt;
            let mut buf = String::new();
            let mut e = e;
            let _ = e.read_to_string(&mut buf).await;
            buf
        })
    });
    if let Some(out) = child.stdout.take() {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(f) = yt_dl_percent(&line) {
                yt_job_progress(video_id, f);
            }
        }
    }
    let status = child.wait().await?;
    let stderr = match stderr_task {
        Some(t) => t.await.unwrap_or_default(),
        None => String::new(),
    };
    if !status.success() {
        yt_job_done(video_id);
        anyhow::bail!("{}", tulipix_core::ytdlp::friendly_error(&stderr));
    }
    // yt-dlp picks the extension, so find what it actually wrote rather than
    // assuming: a merge that fell back to mp4 is still a successful download.
    let written = std::fs::read_dir(&dir)?
        .flatten()
        .map(|e| e.path())
        .find(|p| {
            p.file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s == video_id)
                .unwrap_or(false)
        });
    let Some(written) = written else {
        yt_job_done(video_id);
        anyhow::bail!("yt-dlp reported success but wrote no file");
    };
    let known = {
        let s = lock();
        s.yt_results
            .iter()
            .chain(s.yt_channel_videos.iter())
            .find(|v| v.video_id == video_id)
            .cloned()
    };
    let (title, channel, thumb, dur) = known
        .map(|v| (v.title, v.channel, v.thumb, v.duration))
        .unwrap_or_default();
    let pool = youtube_pool().await?;
    tulipix_music::youtube::store::record_download(
        pool,
        video_id,
        &title,
        &channel,
        &cache_remote(&thumb).await.unwrap_or_default(),
        &written.to_string_lossy(),
        dur,
        if audio { "audio" } else { "video" },
        &written
            .extension()
            .map(|e| e.to_string_lossy().to_uppercase())
            .unwrap_or_default(),
        if audio { "Audio" } else { quality },
    )
    .await?;
    Ok(())
}

/// Pull the audio down in the background after a stream starts, so the second
/// play of the same video is local. Fire-and-forget: a failure here costs
/// nothing, because the stream is already playing.
async fn cache_youtube_audio(video_id: &str) {
    let id = video_id.to_string();
    tokio::spawn(async move {
        let dir = yt_media_dir();
        let out = dir.join(format!("{id}.opus"));
        if out.exists() {
            return;
        }
        let template = dir.join(format!("{id}.%(ext)s"));
        let url = format!("https://www.youtube.com/watch?v={id}");
        let _ = tokio::process::Command::new(tulipix_core::ytdlp::bin())
            .args(["-f", "bestaudio", "-x", "--audio-format", "opus", "--no-playlist"])
            .args(tulipix_core::ytdlp::common_args())
            .arg("-o")
            .arg(&template)
            .arg(&url)
            .no_window_async()
            .status()
            .await;
        if !out.exists() {
            return;
        }
        let Ok(pool) = youtube_pool().await else { return };
        let (title, channel, thumb, dur) = {
            let s = lock();
            s.yt_results
                .iter()
                .chain(s.yt_channel_videos.iter())
                .find(|v| v.video_id == id)
                .map(|v| (v.title.clone(), v.channel.clone(), v.thumb.clone(), v.duration))
                .unwrap_or_default()
        };
        let _ = tulipix_music::youtube::store::record_cached(
            pool,
            &id,
            &title,
            &channel,
            &thumb,
            &out.to_string_lossy(),
            dur,
        )
        .await;
        // Two caps, because neither alone is a bound: the count keeps the list
        // short, the byte cap keeps the disk honest.
        for (_v, p) in tulipix_music::youtube::store::evict_cached_over(pool, YT_CACHE_KEEP)
            .await
            .unwrap_or_default()
        {
            let _ = std::fs::remove_file(&p);
        }
    });
}

// --------------------------------------------------------------- queries ----

/// Every track list in the section reads these fourteen columns. One shape,
/// one join, one place to fix when a fifteenth is wanted.
///
/// Every default here is written in the type of the column it stands in for --
/// `0.0` for `duration_s`, not `0`. SQLite types values, not columns, so a
/// track with no duration made COALESCE hand back an INTEGER where every other
/// row gave a REAL, and sqlx's `f64` accepts only `DataType::Float`: one
/// untagged file failed the decode for the whole statement. It is a query that
/// resolves a *set* of ids, so the damage was never one row -- Recently played,
/// Most played, Loved, Recently added and the queue all came back empty
/// together, from the moment a single such track landed in any of them.
const TRACK_SELECT: &str = "SELECT i.id, i.abs_path, COALESCE(tm.title, ''), \
     COALESCE(ar.name, ''), COALESCE(al.title, ''), COALESCE(tm.duration_s, 0.0), \
     COALESCE(tm.loved, 0), COALESCE(tm.rating, 0), COALESCE(tm.play_count, 0), \
     COALESCE(tm.track_no, 0), COALESCE(tm.year, 0), COALESCE(tm.genre, ''), \
     COALESCE(al.cover_path, ''), \
     CASE WHEN ly.item_id IS NULL THEN '' WHEN ly.synced = 1 THEN 'synced' ELSE 'plain' END \
     FROM items i JOIN track_meta tm ON tm.item_id = i.id \
     LEFT JOIN artists ar ON ar.id = tm.artist_id \
     LEFT JOIN albums al ON al.id = tm.album_id \
     LEFT JOIN lyrics ly ON ly.item_id = i.id";

/// Music, present, and not a book chapter — the filter every My Music list
/// starts from. Audiobooks have their own tab and their own `is_audiobook`
/// flag; a chapter appearing in the songs list is the bug this prevents.
const MUSIC_WHERE: &str =
    " WHERE i.section = 'music' AND i.missing_since IS NULL AND COALESCE(tm.is_audiobook, 0) = 0";

type TrackRow = (
    i64,
    String,
    String,
    String,
    String,
    f64,
    i64,
    i64,
    i64,
    i64,
    i64,
    String,
    String,
    String,
);

fn into_track(row: TrackRow) -> Track {
    let (id, path, title, artist, album, dur, loved, stars, plays, no, year, genre, cover, lyr) =
        row;
    Track {
        item_id: id,
        // A file with no title tag reads as its filename, which is what the
        // library looked like before anyone tagged it and is still better than
        // a row of blanks.
        title: if title.is_empty() {
            Path::new(&path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        } else {
            title
        },
        artist,
        album,
        duration_s: dur,
        path,
        // Only report a cover the cache already holds — resolving here would
        // make the first snapshot of a cold library an ffmpeg run per row.
        art: if !cover.is_empty() && Path::new(&cover).exists() {
            cover
        } else {
            String::new()
        },
        loved: loved != 0,
        stars,
        play_count: plays,
        track_no: no,
        year,
        genre,
        lyrics: lyr,
    }
}

/// Resolve ids to tracks, preserving the order they were asked for — the
/// rails are ranked lists, and SQL's `IN` is a set.
async fn tracks_by_ids(pool: &sqlx::SqlitePool, ids: &[i64]) -> Vec<Track> {
    if ids.is_empty() {
        return Vec::new();
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!("{TRACK_SELECT} WHERE i.id IN ({holes})");
    let mut q = sqlx::query_as::<_, TrackRow>(&sql);
    for id in ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await.unwrap_or_default();
    let mut by_id: std::collections::HashMap<i64, Track> =
        rows.into_iter().map(|r| (r.0, into_track(r))).collect();
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

fn song_order(mode: &str, dir: &str) -> String {
    let column = match mode {
        "title" => "tm.title COLLATE NOCASE",
        "artist" => "ar.name COLLATE NOCASE",
        "album" => "al.title COLLATE NOCASE",
        "plays" => "tm.play_count",
        "rating" => "tm.rating",
        "duration" => "tm.duration_s",
        _ => "i.added",
    };
    let direction = if dir == "asc" { "ASC" } else { "DESC" };
    // The id tiebreak is not decoration: without it two tracks with the same
    // title land in an order SQLite is free to change between pages, and a row
    // can show up on page 2 as well as page 1.
    format!(" ORDER BY {column} {direction}, i.id ASC")
}

/// One page of the songs list, plus the total behind it and the page it
/// actually landed on.
///
/// The page is clamped here rather than trusted. Nothing resets it when the
/// list shrinks under it -- deleting the last track of the last page, loving
/// something out of a filter, a rescan that drops missing files -- and an
/// offset past the end is a `LIMIT` that returns nothing at all, which draws as
/// an empty tab with a pager reading "7 / 6".
async fn songs_page(pool: &sqlx::SqlitePool, s: &Session) -> (Vec<Track>, i64, i64) {
    let like = format!("%{}%", s.query.trim());
    let filtered = !s.query.trim().is_empty();
    let filter = if filtered {
        " AND (tm.title LIKE ? OR ar.name LIKE ? OR al.title LIKE ?)"
    } else {
        ""
    };

    let count_sql = format!(
        "SELECT COUNT(*) FROM items i JOIN track_meta tm ON tm.item_id = i.id \
         LEFT JOIN artists ar ON ar.id = tm.artist_id \
         LEFT JOIN albums al ON al.id = tm.album_id{MUSIC_WHERE}{filter}"
    );
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    if filtered {
        cq = cq.bind(&like).bind(&like).bind(&like);
    }
    let total = cq.fetch_one(pool).await.unwrap_or(0);

    let sql = format!(
        "{TRACK_SELECT}{MUSIC_WHERE}{filter}{} LIMIT ? OFFSET ?",
        song_order(&s.song_sort, &s.song_dir)
    );
    let mut q = sqlx::query_as::<_, TrackRow>(&sql);
    if filtered {
        q = q.bind(&like).bind(&like).bind(&like);
    }
    let page = clamp_page(s.song_page, total, SONGS_PAGE);
    let rows = q
        .bind(SONGS_PAGE)
        .bind(page * SONGS_PAGE)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    (rows.into_iter().map(into_track).collect(), total, page)
}

/// The last page that has anything on it, given a total and a page size.
fn clamp_page(page: i64, total: i64, per: i64) -> i64 {
    let pages = page_count(total, per);
    page.clamp(0, pages - 1)
}

fn page_count(total: i64, per: i64) -> i64 {
    ((total.max(1) as f64) / per as f64).ceil() as i64
}

/// Albums, artists, genres, playlists or folders — whichever the open tab
/// wants — sorted and paged in Rust. These lists are thousands of rows at
/// most, and doing it here keeps one sort rule for five different queries.
/// `loved` and `rating` are in no migration: the Slint build adds them with a
/// bare `ALTER TABLE` the first time someone taps a heart, so a library nobody
/// has rated has neither column. Both builds open the same file, so this does
/// the same rather than assuming — the error when a column is already there is
/// the expected case and is discarded.
async fn ensure_browse_columns(pool: &sqlx::SqlitePool) {
    for sql in [
        "ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE artists ADD COLUMN loved INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE artists ADD COLUMN rating INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = sqlx::query(sql).execute(pool).await;
    }
}

/// The albums or artists carrying a heart.
///
/// Not a page: the Loved tab shows them all above the track list, and a
/// library with more loved albums than fit on one screen is a library that
/// scrolls.
async fn loved_cards(pool: &sqlx::SqlitePool, kind: &str) -> Vec<BrowseCard> {
    ensure_browse_columns(pool).await;
    let marks = browse_marks(pool, kind).await;
    if marks.is_empty() {
        return Vec::new();
    }
    if kind == "artists" {
        tulipix_music::browse::artists(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, name, count)| {
                let (loved, stars) = marks.get(&id).copied().unwrap_or((false, 0));
                loved.then(|| BrowseCard {
                    id,
                    key: id.to_string(),
                    title: name,
                    subtitle: format!("{count} tracks"),
                    count,
                    art: String::new(),
                    loved,
                    stars,
                })
            })
            .collect()
    } else {
        tulipix_music::browse::albums(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|a| {
                let (loved, stars) = marks.get(&a.album_id).copied().unwrap_or((false, 0));
                loved.then(|| BrowseCard {
                    id: a.album_id,
                    key: a.album_id.to_string(),
                    title: a.title,
                    subtitle: a.artist.unwrap_or_default(),
                    count: a.track_count,
                    art: a.cover_path.filter(|c| Path::new(c).exists()).unwrap_or_default(),
                    loved,
                    stars,
                })
            })
            .collect()
    }
}

/// Every playlist, as tiles. Wanted in two places -- the Playlists tab and the
/// snapshot's always-there list -- so it is one function rather than one query
/// written twice.
///
/// The cover is a preference, not a column: right-clicking a playlist picks an
/// image and it is remembered under `music.playlist.cover.{id}`, which is where
/// the Slint build keeps it too.
async fn playlist_cards(pool: &sqlx::SqlitePool) -> Vec<BrowseCard> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT p.id, p.name, p.is_smart, \
                (SELECT COUNT(*) FROM playlist_items pi WHERE pi.playlist_id = p.id) \
         FROM playlists p ORDER BY p.name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let prefs = settings_map();
    rows.into_iter()
        .map(|(id, name, smart, count)| BrowseCard {
            art: pref_cover(&prefs, &format!("music.playlist.cover.{id}")),
            id,
            key: id.to_string(),
            title: name,
            subtitle: if smart != 0 { "Smart".into() } else { format!("{count} tracks") },
            count,
            loved: false,
            stars: 0,
        })
        .collect()
}

/// What a browse tile shows beyond its name. One query per kind, not per tile —
/// and only the rows that carry a mark, since most do not.
async fn browse_marks(pool: &sqlx::SqlitePool, table: &str) -> HashMap<i64, (bool, i64)> {
    sqlx::query_as::<_, (i64, i64, i64)>(&format!(
        "SELECT id, COALESCE(loved, 0), COALESCE(rating, 0) FROM {table} \
         WHERE COALESCE(loved, 0) <> 0 OR COALESCE(rating, 0) <> 0"
    ))
    .fetch_all(pool)
    .await
    .unwrap_or_default()
    .into_iter()
    .map(|(id, loved, stars)| (id, (loved != 0, stars)))
    .collect()
}

async fn browse_cards(
    pool: &sqlx::SqlitePool,
    kind: &str,
    s: &Session,
) -> (Vec<BrowseCard>, i64, i64) {
    let mut cards: Vec<BrowseCard> = match kind {
        "albums" => {
            let marks = browse_marks(pool, "albums").await;
            tulipix_music::browse::albums(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|a| {
                    let (loved, stars) = marks.get(&a.album_id).copied().unwrap_or((false, 0));
                    BrowseCard {
                        id: a.album_id,
                        key: a.album_id.to_string(),
                        title: a.title,
                        subtitle: a.artist.unwrap_or_default(),
                        count: a.track_count,
                        art: a.cover_path.filter(|c| Path::new(c).exists()).unwrap_or_default(),
                        loved,
                        stars,
                    }
                })
                .collect()
        }
        "artists" => {
            let marks = browse_marks(pool, "artists").await;
            tulipix_music::browse::artists(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(id, name, count)| {
                    let (loved, stars) = marks.get(&id).copied().unwrap_or((false, 0));
                    BrowseCard {
                        id,
                        key: id.to_string(),
                        title: name,
                        subtitle: format!("{count} tracks"),
                        count,
                        art: String::new(),
                        loved,
                        stars,
                    }
                })
                .collect()
        }
        "genres" => {
            let prefs = settings_map();
            tulipix_music::browse::genres(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(name, count)| BrowseCard {
                    id: 0,
                    // Right-click picks a cover for a genre; it is remembered
                    // as a preference under the genre's own name, the way the
                    // Slint build does it — there is no genres table to hold a
                    // column.
                    art: pref_cover(&prefs, &format!("music.genre.cover.{name}")),
                    key: name.clone(),
                    title: name,
                    subtitle: format!("{count} tracks"),
                    count,
                    loved: false,
                    stars: 0,
                })
                .collect()
        }
        "playlists" => playlist_cards(pool).await,
        "folders" => {
            let excluded = folder_sections();
            tulipix_music::folders::list(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|(folder, _)| {
                    // A folder reassigned to Audiobooks belongs to that tab,
                    // not this one.
                    excluded
                        .get(folder)
                        .map(|sec| sec == "mymusic")
                        .unwrap_or(true)
                })
                .map(|(folder, count)| BrowseCard {
                    id: 0,
                    title: Path::new(&folder)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| folder.clone()),
                    subtitle: folder.clone(),
                    key: folder,
                    count,
                    art: String::new(),
                    loved: false,
                    stars: 0,
                })
                .collect()
        }
        _ => Vec::new(),
    };

    let q = s.query.trim().to_lowercase();
    if !q.is_empty() {
        cards.retain(|c| {
            c.title.to_lowercase().contains(&q) || c.subtitle.to_lowercase().contains(&q)
        });
    }
    if s.browse_sort == "count" {
        cards.sort_by(|a, b| a.count.cmp(&b.count));
    } else {
        cards.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    }
    if s.browse_dir == "desc" {
        cards.reverse();
    }
    let total = cards.len() as i64;
    let per = browse_page_size(kind);
    let page = clamp_page(s.browse_page, total, per);
    let start = (page * per).min(total) as usize;
    let end = ((page + 1) * per).min(total) as usize;
    (cards[start..end].to_vec(), total, page)
}

/// How many tiles a browse grid pages by. Folders are three rows of seven, not
/// four: the tile carries a path under the name and is a third taller than an
/// album's, so four rows of them do not fit above the fold.
fn browse_page_size(kind: &str) -> i64 {
    if kind == "folders" { FOLDER_PAGE } else { BROWSE_PAGE }
}

/// An artist's blurb, from `artists.bio` -- fetched once from MusicBrainz and
/// written back, so the page is instant every time after the first.
///
/// The column is added the same way `loved` and `rating` are: with a bare
/// `ALTER TABLE` whose "already there" error is the expected case. There is no
/// migration for it, and the Slint build opens the same file.
///
/// A failed lookup writes nothing. Caching the miss would mean an artist who
/// was offline once stays blank forever, and the retry costs one request the
/// next time someone opens the page.
async fn artist_bio(pool: &sqlx::SqlitePool, artist_id: i64, name: &str) -> String {
    let _ = sqlx::query("ALTER TABLE artists ADD COLUMN bio TEXT")
        .execute(pool)
        .await;
    let cached: Option<String> = sqlx::query_scalar("SELECT bio FROM artists WHERE id = ?")
        .bind(artist_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    if let Some(bio) = cached {
        if !bio.trim().is_empty() {
            return bio;
        }
    }
    if name.trim().is_empty() || !bio_first_try(artist_id) {
        return String::new();
    }
    // Off the snapshot's thread. This runs on every refresh while the page is
    // open -- a track change, a tick that finished a song -- and a request that
    // has to reach MusicBrainz and back would freeze the page for as long as it
    // took. It lands in the column and `Stale` brings the page back for it.
    let name = name.to_string();
    tokio::spawn(async move {
        let Ok(pool) = music_pool().await else { return };
        let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .build()
        else {
            return;
        };
        let Ok(search) = tulipix_music::musicbrainz::lookup_artist(&client, &name).await else {
            return;
        };
        // Best hit only, and only if it is actually about this artist --
        // MusicBrainz answers every query with something, and a low-scoring hit
        // is a different band with a similar name.
        let Some(hit) = search
            .artists
            .iter()
            .max_by_key(|a| a.score)
            .filter(|a| a.score >= 80)
        else {
            return;
        };
        let bio = tulipix_music::musicbrainz::artist_blurb(hit);
        if bio.trim().is_empty() {
            return;
        }
        let _ = sqlx::query("UPDATE artists SET bio = ? WHERE id = ?")
            .bind(&bio)
            .bind(artist_id)
            .execute(pool)
            .await;
        emit(MusicEvent::Stale);
    });
    String::new()
}

/// One lookup per artist per run. A miss is not written to the column -- an
/// artist who was offline once should not stay blank forever -- so without this
/// every refresh of an unknown artist's page would fire another request.
fn bio_first_try(artist_id: i64) -> bool {
    static TRIED: std::sync::OnceLock<Mutex<std::collections::HashSet<i64>>> =
        std::sync::OnceLock::new();
    TRIED
        .get_or_init(Default::default)
        .lock()
        .map(|mut g| g.insert(artist_id))
        .unwrap_or(false)
}

/// The tracks behind whatever detail page is open, plus its header fields.
async fn detail_tracks(pool: &sqlx::SqlitePool, s: &Session) -> (Vec<Track>, String, String, String) {
    let (clause, bind_i, bind_s): (&str, Option<i64>, Option<String>) = match s.detail_kind.as_str()
    {
        "album" => (" AND tm.album_id = ?", Some(s.detail_id), None),
        "artist" => (" AND tm.artist_id = ?", Some(s.detail_id), None),
        "genre" => (" AND tm.genre = ?", None, Some(s.detail_key.clone())),
        // Two shapes of the same question. `tm.folder` is what the grid
        // grouped by, but it is written by the scanner and a library imported
        // by two different versions of it can hold both `/music/x` and
        // `/music/x/` -- and a track whose row predates the column has no
        // folder at all. The path prefix catches every one of those, and a
        // folder page that lists nothing is the bug it prevents.
        "folder" => (
            " AND (tm.folder = ? OR i.abs_path LIKE ? || '/%')",
            None,
            Some(s.detail_key.clone()),
        ),
        "playlist" => (
            " AND i.id IN (SELECT item_id FROM playlist_items WHERE playlist_id = ?)",
            Some(s.detail_id),
            None,
        ),
        _ => return (Vec::new(), String::new(), String::new(), String::new()),
    };
    // A playlist has an order of its own; everything else is disc/track order,
    // which is how an album is meant to be heard.
    let order = if s.detail_kind == "playlist" {
        " ORDER BY (SELECT position FROM playlist_items pi WHERE pi.playlist_id = ? AND pi.item_id = i.id)"
    } else {
        " ORDER BY COALESCE(tm.disc_no, 0), COALESCE(tm.track_no, 0), tm.title COLLATE NOCASE"
    };
    let sql = format!("{TRACK_SELECT}{MUSIC_WHERE}{clause}{order}");
    let mut q = sqlx::query_as::<_, TrackRow>(&sql);
    if let Some(v) = bind_i {
        q = q.bind(v);
    }
    if let Some(v) = &bind_s {
        q = q.bind(v);
        if s.detail_kind == "folder" {
            q = q.bind(v.trim_end_matches('/'));
        }
    }
    if s.detail_kind == "playlist" {
        q = q.bind(s.detail_id);
    }
    let tracks: Vec<Track> = q
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(into_track)
        .collect();

    let (title, subtitle, art) = match s.detail_kind.as_str() {
        "album" => {
            let row: (String, String, String) = sqlx::query_as(
                "SELECT al.title, COALESCE(ar.name, ''), COALESCE(al.cover_path, '') \
                 FROM albums al LEFT JOIN artists ar ON ar.id = al.artist_id WHERE al.id = ?",
            )
            .bind(s.detail_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
            (row.0, row.1, row.2)
        }
        "artist" => {
            let name: String = sqlx::query_scalar("SELECT name FROM artists WHERE id = ?")
                .bind(s.detail_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let n = tracks.len();
            (name, format!("{n} tracks"), String::new())
        }
        "playlist" => {
            let name: String = sqlx::query_scalar("SELECT name FROM playlists WHERE id = ?")
                .bind(s.detail_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            let n = tracks.len();
            // The same preference the Playlists grid reads, so the page and the
            // tile you opened it from are one picture rather than two.
            let cover = pref_cover(&settings_map(), &format!("music.playlist.cover.{}", s.detail_id));
            (name, format!("{n} tracks"), cover)
        }
        "folder" => {
            let name = Path::new(&s.detail_key)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| s.detail_key.clone());
            (name, s.detail_key.clone(), String::new())
        }
        _ => {
            let n = tracks.len();
            let cover = if s.detail_kind == "genre" {
                pref_cover(&settings_map(), &format!("music.genre.cover.{}", s.detail_key))
            } else {
                String::new()
            };
            (s.detail_key.clone(), format!("{n} tracks"), cover)
        }
    };
    let art = if !art.is_empty() && Path::new(&art).exists() {
        art
    } else {
        String::new()
    };
    (tracks, title, subtitle, art)
}

// --- podcasts ---

type ShowRow = (i64, String, String, String, String, String, String, i64, i64, i64);

const SHOW_SELECT: &str = "SELECT p.id, COALESCE(p.title, ''), COALESCE(p.author, ''), \
     COALESCE(p.category, ''), COALESCE(p.description, ''), \
     COALESCE(NULLIF(p.custom_image, ''), COALESCE(p.image_url, '')), p.feed_url, \
     (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id), \
     (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id AND e.played = 0), \
     COALESCE((SELECT MAX(published) FROM podcast_episodes e WHERE e.podcast_id = p.id), 0) \
     FROM podcasts p";

fn into_show(r: ShowRow) -> PodcastShow {
    PodcastShow {
        id: r.0,
        title: r.1,
        author: r.2,
        category: r.3,
        description: r.4,
        art: r.5,
        feed_url: r.6,
        episodes: r.7,
        unplayed: r.8,
        latest: r.9,
    }
}

type EpisodeRow = (i64, i64, String, String, String, i64, f64, String, String, String, f64, i64);

const EPISODE_SELECT: &str = "SELECT e.id, e.podcast_id, COALESCE(p.title, ''), \
     COALESCE(e.title, ''), COALESCE(e.description, ''), COALESCE(e.published, 0), \
     COALESCE(e.duration_s, 0), \
     COALESCE(NULLIF(e.image_url, ''), COALESCE(NULLIF(p.custom_image, ''), COALESCE(p.image_url, ''))), \
     e.audio_url, COALESCE(e.downloaded_path, ''), e.position_s, e.played \
     FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id";

fn into_episode(r: EpisodeRow) -> Episode {
    Episode {
        id: r.0,
        podcast_id: r.1,
        show: r.2,
        title: r.3,
        description: r.4,
        published: r.5,
        duration_s: r.6,
        art: r.7,
        audio_url: r.8,
        downloaded: r.9,
        position_s: r.10,
        played: r.11 != 0,
    }
}

/// Pinned shows per Home page. 14, the 2x7 block Slint draws.
const HOME_PAGE: i64 = 14;

/// Trends cards per page. 21, the same 3x7 block the Slint grid draws.
const TRENDS_PAGE: i64 = 21;

/// The info card's payload.
///
/// A subscribed show is answered from `podcasts` -- no network, and it is the
/// only source that knows the category the user edited. An unsubscribed Trends
/// card has no row anywhere, so the feed itself is fetched once; that is the
/// whole point of the card ("what is this show") and it is not on any repaint
/// path, only on the click that opens it.
async fn pod_info(podcast_id: i64, feed_url: &str) -> Option<PodInfo> {
    let pool = podcasts_pool().await.ok()?;
    type Row = (i64, String, String, String, String, String, i64, i64);
    let row: Option<Row> = sqlx::query_as(
        "SELECT p.id, COALESCE(p.title,''), COALESCE(p.author,''), COALESCE(p.category,''), \
                COALESCE(p.description,''), \
                COALESCE(NULLIF(p.custom_image,''), COALESCE(p.image_url,'')), \
                (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id), \
                COALESCE((SELECT MAX(published) FROM podcast_episodes e WHERE e.podcast_id = p.id), 0) \
         FROM podcasts p WHERE p.id = ? OR p.feed_url = ? LIMIT 1",
    )
    .bind(podcast_id)
    .bind(feed_url)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some((id, title, author, category, description, art, episodes, latest)) = row {
        return Some(PodInfo {
            feed_url: feed_url.to_string(),
            podcast_id: id,
            title,
            author,
            category,
            description,
            art,
            episodes,
            latest,
            subscribed: true,
        });
    }

    let cached = {
        let s = lock();
        s.pod_trends.iter().find(|m| m.feed_url == feed_url).cloned()
    };
    let body = tulipix_core::net::http()
        .get(feed_url)
        .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
        .send()
        .await
        .ok()?
        .text()
        .await
        .ok()
        .unwrap_or_default();
    let feed = tulipix_music::podcasts::parse_feed(&body);
    let cached_art = cached
        .as_ref()
        .and_then(|m| m.art.as_ref())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Some(PodInfo {
        feed_url: feed_url.to_string(),
        podcast_id: -1,
        title: feed
            .title
            .clone()
            .or_else(|| cached.as_ref().map(|m| m.title.clone()))
            .unwrap_or_default(),
        author: if feed.author.is_empty() {
            cached.as_ref().map(|m| m.author.clone()).unwrap_or_default()
        } else {
            feed.author.clone()
        },
        category: if feed.category.is_empty() {
            cached.as_ref().map(|m| m.category.clone()).unwrap_or_default()
        } else {
            feed.category.clone()
        },
        description: tulipix_music::podcasts::strip_html(&feed.description),
        art: if cached_art.is_empty() { feed.image_url.clone() } else { cached_art },
        episodes: feed.episodes.len() as i64,
        latest: feed.episodes.iter().filter_map(|e| e.published).max().unwrap_or(0),
        subscribed: false,
    })
}

/// The episode saving offline: `(episode_id, 0..1, title)`. Downloads here run
/// one per command, so there is no queue to model -- unlike the YouTube side,
/// where yt-dlp jobs stack up behind each other.
fn pod_dl() -> &'static Mutex<(i64, f64, String)> {
    static C: std::sync::OnceLock<Mutex<(i64, f64, String)>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new((-1, 0.0, String::new())))
}

fn pod_dl_set(id: i64, frac: f64, title: &str) {
    let mut changed = false;
    if let Ok(mut g) = pod_dl().lock() {
        changed = g.0 != id || (frac * 100.0) as i64 != (g.1 * 100.0) as i64;
        *g = (id, frac, title.to_string());
    }
    if changed {
        emit(MusicEvent::Stale);
    }
}

fn pod_dl_clear() {
    if let Ok(mut g) = pod_dl().lock() {
        *g = (-1, 0.0, String::new());
    }
    emit(MusicEvent::Stale);
}

/// The one progress line every long podcast job writes to: refresh-all,
/// subscribe, reset. They cannot overlap -- each blocks the control that would
/// start the next -- so one channel is the whole story, and the UI has one bar
/// to draw instead of four that are never lit at once.
fn pod_job() -> &'static Mutex<(bool, f64, String)> {
    static C: std::sync::OnceLock<Mutex<(bool, f64, String)>> = std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

fn pod_job_set(busy: bool, frac: f64, status: &str) {
    if let Ok(mut g) = pod_job().lock() {
        *g = (busy, frac, status.to_string());
    }
    emit(MusicEvent::Stale);
}

// --- audiobooks ---

/// Books already looked up online this session (hit or miss), so a shelf that
/// repaints on every tick does not re-hit five APIs per book.
fn ab_tried() -> &'static Mutex<std::collections::HashSet<String>> {
    static C: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// Resolved cover path per (folder, stored choice). The shelf is re-read on
/// every player tick while the tab is open, and resolving one book's cover is
/// nine `stat`s and sometimes an ffmpeg spawn -- fine once, not once a second
/// per book. Keyed by the stored path as well as the folder, so picking a new
/// cover misses the memo rather than needing to invalidate it.
fn ab_cover_memo() -> &'static Mutex<HashMap<(String, String), Option<String>>> {
    static C: std::sync::OnceLock<Mutex<HashMap<(String, String), Option<String>>>> =
        std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// [`tulipix_music::ab_meta::cover_path`] behind that memo.
async fn book_cover(
    pool: &sqlx::SqlitePool,
    folder: &str,
    stored: Option<&str>,
) -> Option<String> {
    let key = (folder.to_string(), stored.unwrap_or_default().to_string());
    if let Some(hit) = ab_cover_memo().lock().ok().and_then(|g| g.get(&key).cloned()) {
        return hit;
    }
    // Only looked up on a miss: the embedded-art fallback is the one source
    // that needs a file to point ffmpeg at.
    let first: Option<String> = sqlx::query_scalar(
        "SELECT i.abs_path FROM items i JOIN track_meta tm ON tm.item_id = i.id \
         WHERE tm.folder = ? ORDER BY i.abs_path LIMIT 1",
    )
    .bind(folder)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let found = tulipix_music::ab_meta::cover_path(
        folder,
        stored,
        first.as_deref().map(Path::new),
    )
    .await
    .map(|p| p.to_string_lossy().into_owned());
    if let Ok(mut g) = ab_cover_memo().lock() {
        g.insert(key, found.clone());
    }
    found
}

/// Resolve title / author / cover for the books that still have none, in the
/// background, then tell the UI to re-read. The chain itself is
/// `tulipix_music::ab_meta`, shared with the Slint build.
fn kick_ab_lookup(folders: Vec<String>) {
    let todo: Vec<String> = match ab_tried().lock() {
        Ok(mut g) => folders.into_iter().filter(|f| g.insert(f.clone())).collect(),
        Err(_) => return,
    };
    if todo.is_empty() {
        return;
    }
    tokio::spawn(async move {
        let Ok(pool) = music_pool().await else { return };
        if tulipix_music::ab_meta::resolve_and_store(pool, &todo).await {
            if let Ok(mut g) = ab_cover_memo().lock() {
                g.retain(|(folder, _), _| !todo.contains(folder));
            }
            emit(MusicEvent::Stale);
        }
    });
}

/// One card per book folder: title, cover, chapter count and how far in it is.
///
/// A book is named and pictured by the same chain the Slint build uses: the
/// title and author resolved from tags / filenames / LibriVox / iTunes / Open
/// Library when there are any, and only the folder's basename and the first
/// track's artist as the placeholder until then. The bare basename is what a
/// rip called `dune_01_64kb` shows, which is why the lookup exists.
async fn book_cards(pool: &sqlx::SqlitePool, tab: &str) -> Vec<BookCard> {
    let folders = tulipix_music::audiobooks::book_folders(pool)
        .await
        .unwrap_or_default();
    tulipix_music::ab_meta::ensure_tables(pool).await;
    let covers = tulipix_music::ab_meta::load_covers(pool).await;
    let meta = tulipix_music::ab_meta::load_meta(pool).await;
    let mut out = Vec::new();
    // Every book, whether or not the open tab keeps it -- see below.
    let mut every: Vec<String> = Vec::new();
    let mut no_art: Vec<String> = Vec::new();
    for (folder, chapters) in folders {
        every.push(folder.clone());
        let ids = tulipix_music::audiobooks::book_chapters(pool, &folder)
            .await
            .unwrap_or_default();
        let (finished_ids, _) = tulipix_music::audiobooks::chapter_states(pool, &ids)
            .await
            .unwrap_or_default();
        let totals: Option<(f64, f64)> = sqlx::query_as(
            "SELECT COALESCE(SUM(tm.duration_s), 0.0), \
                    COALESCE(SUM(COALESCE(ap.position_s, 0.0)), 0.0) \
             FROM track_meta tm LEFT JOIN audiobook_progress ap ON ap.item_id = tm.item_id \
             WHERE tm.folder = ?",
        )
        .bind(&folder)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
        let (total_s, heard_s) = totals.unwrap_or((0.0, 0.0));
        let finished = !ids.is_empty() && finished_ids.len() == ids.len();
        let progress = if total_s > 0.0 {
            (heard_s / total_s).clamp(0.0, 1.0)
        } else {
            0.0
        };
        // Art and identity are resolved for every book, filtered tab or not:
        // the lookup is what fills the OTHER tabs in, and a card that only gets
        // a face while you are looking at it never gets one.
        let art = book_cover(pool, &folder, covers.get(&folder).map(String::as_str)).await;
        if art.is_none() {
            no_art.push(folder.clone());
        }
        let keep = match tab {
            "progress" => progress > 0.0 && !finished,
            "finished" => finished,
            _ => true,
        };
        if !keep {
            continue;
        }
        let resolved = meta.get(&folder);
        let author = match resolved.map(|(_, a)| a.clone()).filter(|a| !a.is_empty()) {
            Some(a) => a,
            // Nothing resolved yet: the chapters' own artist tag is the best
            // guess on disk, and on a tagged rip it is already the author.
            None => sqlx::query_scalar(
                "SELECT COALESCE(ar.name, '') FROM track_meta tm \
                 LEFT JOIN artists ar ON ar.id = tm.artist_id \
                 WHERE tm.folder = ? LIMIT 1",
            )
            .bind(&folder)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or_default(),
        };
        out.push(BookCard {
            title: resolved
                .map(|(t, _)| t.clone())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| tulipix_music::ab_meta::book_title(&folder)),
            author,
            art: art.unwrap_or_default(),
            chapters,
            finished,
            progress,
            total_s,
            folder,
        });
    }
    // Everything with no art at all, plus anything wearing a net cover that
    // never resolved a real title -- the improved chain retries those once.
    let missing = tulipix_music::ab_meta::needs_lookup(&every, &covers, &meta, |f| {
        !no_art.iter().any(|n| n.as_str() == f)
    });
    kick_ab_lookup(missing);
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    out
}

// --- radio ---

fn into_station(s: tulipix_music::radio::Station, favs: &[String]) -> Station {
    Station {
        favourite: favs.contains(&s.stationuuid),
        uuid: s.stationuuid,
        name: s.name,
        url: s.url,
        favicon: s.favicon,
        country: s.country,
        tags: s.tags,
        codec: s.codec,
        bitrate: s.bitrate as i64,
    }
}

// --- youtube ---

fn into_yt(v: tulipix_music::youtube::store::CachedVideo, progress: f64) -> YtVideo {
    YtVideo {
        video_id: v.video_id,
        title: v.title,
        channel: v.channel,
        thumb: v.thumb_path,
        duration: v.duration,
        media_path: v.media_path,
        meta: if v.fmt.is_empty() {
            String::new()
        } else {
            format!("{} · {}", v.fmt, v.quality)
        },
        progress,
        quality: v.quality,
    }
}

// -------------------------------------------------------------- snapshot ----

/// Read the whole visible state in one pass.
///
/// Only the open tab is queried. The Slint build keeps all five populated at
/// once because its properties are the model and a tab switch has to find them
/// already filled; here a switch is a command, so four fifths of the work does
/// not have to happen on every keystroke in the search box.
async fn snapshot() -> Result<MusicState> {
    let s = lock().clone();
    let obs = mpv::observe();
    let np_raw = mpv::now_playing();
    let slot = mpv::current_slot();

    let pool = music_pool().await?;

    // --- the player bar, which every tab shares -----------------------------
    let (np_loved, np_stars) = if np_raw.item_id != 0 {
        sqlx::query_as::<_, (i64, i64)>(
            "SELECT COALESCE(loved, 0), COALESCE(rating, 0) FROM track_meta WHERE item_id = ?",
        )
        .bind(np_raw.item_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .unwrap_or((0, 0))
    } else {
        (0, 0)
    };
    let now = NowPlaying {
        mode: slot.as_str().to_string(),
        item_id: np_raw.item_id,
        key: np_raw.key,
        title: np_raw.title,
        artist: np_raw.artist,
        album: np_raw.album,
        art: np_raw.art,
        playing: obs.loaded && !obs.paused,
        loaded: obs.loaded,
        pos: obs.pos,
        dur: obs.dur,
        volume: obs.volume,
        muted: obs.muted,
        loved: np_loved != 0,
        stars: np_stars,
        stream_title: obs.media_title,
    };

    let queue_ids_now = queue_ids(pool).await;
    let queue = tracks_by_ids(pool, &queue_ids_now).await;

    // Lyrics for whatever is playing, so the panel is right wherever it is
    // opened from.
    let (lyrics, lyrics_plain) = if np_raw.item_id != 0 {
        let row: Option<(i64, String)> =
            sqlx::query_as("SELECT synced, content FROM lyrics WHERE item_id = ?")
                .bind(np_raw.item_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten();
        match row {
            Some((1, content)) => (
                tulipix_music::lyrics::parse_lrc(&content)
                    .into_iter()
                    .map(|(at_ms, text)| LyricLine { at_ms, text })
                    .collect(),
                content,
            ),
            Some((_, content)) => (Vec::new(), content),
            None => (Vec::new(), String::new()),
        }
    } else {
        (Vec::new(), String::new())
    };

    let eq = load_eq();
    let job = pod_job().lock().map(|g| g.clone()).unwrap_or_default();
    let yt_jobs_now = yt_job_list();
    let dl = pod_dl().lock().map(|g| g.clone()).unwrap_or((-1, 0.0, String::new()));
    let fetch = yt_fetch().lock().map(|g| g.clone()).unwrap_or_default();

    // --- per-tab -----------------------------------------------------------
    let mut st = MusicState {
        view: s.view.clone(),
        query: s.query.clone(),
        status: s.status.clone(),
        now,
        shuffle: s.shuffle,
        repeat: s.repeat.clone(),
        sleep_min: s.sleep_min,
        queue,
        lyrics,
        lyrics_plain,
        lyrics_offset_ms: s.lyrics_offset_ms,
        eq_preset: setting("music.eq.preset", "flat"),
        eq_bands: eq.gains_db.to_vec(),
        eq_on: eq.enabled,
        devices: output_devices(),
        device: setting("music.device", "auto"),
        gapless: setting("music.gapless", "1") != "0",
        crossfade: setting("music.crossfade", "0").parse().unwrap_or(0.0),
        replaygain: setting("music.replaygain", "off"),
        preamp_db: setting("music.preamp", "0").parse().unwrap_or(0.0),

        mgr_open: s.mgr_open,
        mgr_tab: s.mgr_tab.clone(),
        mgr_mode: if s.mgr_picked.is_some() {
            "preview".into()
        } else {
            s.mgr_mode.clone()
        },
        mgr_filter: s.mgr_filter.clone(),
        mgr_page: s.mgr_page,
        mgr_pages: 1,
        mgr_total: 0,
        mgr_ok: 0,
        mgr_missing: 0,
        mgr_rows: Vec::new(),
        mgr_item_id: s.mgr_item_id,
        mgr_title: s.mgr_title.clone(),
        mgr_q_name: s.mgr_q_name.clone(),
        mgr_q_artist: s.mgr_q_artist.clone(),
        mgr_q_album: s.mgr_q_album.clone(),
        mgr_searching: s.mgr_searching,
        mgr_results: s.mgr_results.clone(),
        mgr_view_rows: Vec::new(),
        mgr_view_plain: String::new(),
        mgr_dirty: false,
        mgr_status: s.mgr_status.clone(),
        mgr_progress: 0.0,
        mgr_busy: s.mgr_busy,

        lib_tab: s.lib_tab.clone(),
        track_count: 0,
        songs: Vec::new(),
        song_total: 0,
        song_page: s.song_page,
        song_pages: 1,
        song_sort: s.song_sort.clone(),
        song_dir: s.song_dir.clone(),
        cards: Vec::new(),
        card_total: 0,
        browse_page: s.browse_page,
        browse_pages: 1,
        browse_sort: s.browse_sort.clone(),
        browse_dir: s.browse_dir.clone(),
        stats: Stats::default(),
        rail_recent: Vec::new(),
        rail_most: Vec::new(),
        rail_loved: Vec::new(),
        rail_fresh: Vec::new(),
        rail_albums: Vec::new(),
        rail_artists: Vec::new(),
        roots: Vec::new(),
        playlists: Vec::new(),

        detail_open: s.detail_open,
        detail_kind: s.detail_kind.clone(),
        detail_id: s.detail_id,
        detail_key: s.detail_key.clone(),
        detail_title: String::new(),
        detail_subtitle: String::new(),
        detail_art: String::new(),
        detail_tracks: Vec::new(),
        detail_albums: Vec::new(),
        detail_loved: false,
        detail_stars: 0,
        detail_artist_id: 0,
        detail_info: String::new(),

        pod_tab: s.pod_tab.clone(),
        pod_shows: Vec::new(),
        pod_total: 0,
        pod_page: s.pod_page,
        pod_pages: 1,
        pod_categories: Vec::new(),
        pod_cat: s.pod_cat.clone(),
        pod_latest: Vec::new(),
        pod_downloads: Vec::new(),
        pod_detail_open: s.pod_open >= 0,
        pod_detail: None,
        pod_episodes: Vec::new(),
        pod_ep_page: s.pod_ep_page,
        pod_ep_pages: 1,
        pod_ep_sort: s.pod_ep_sort.clone(),
        pod_speed: s.pod_speed,
        pod_queue: Vec::new(),
        pod_home: Vec::new(),
        pod_home_page: s.pod_home_page,
        pod_home_pages: 1,
        pod_home_sort: s.pod_home_sort.clone(),
        pod_dl_sort: s.pod_dl_sort.clone(),
        pod_trends: Vec::new(),
        pod_trends_loading: s.pod_trends_loading,
        pod_trends_sort: s.pod_trends_sort.clone(),
        pod_trends_page: s.pod_trends_page,
        pod_trends_pages: 1,
        pod_info: s.pod_info.clone(),
        pod_transcript_title: s.pod_transcript.as_ref().map(|t| t.0.clone()).unwrap_or_default(),
        pod_transcript_text: s.pod_transcript.as_ref().map(|t| t.1.clone()).unwrap_or_default(),
        pod_busy: job.0,
        pod_frac: job.1,
        pod_status: job.2,
        pod_query: s.pod_queries.get(&s.pod_tab).cloned().unwrap_or_default(),
        pod_dl_id: dl.0,
        pod_dl_frac: dl.1,
        pod_dl_title: dl.2,

        book_tab: s.book_tab.clone(),
        books: Vec::new(),
        book_detail_open: !s.book_open.is_empty(),
        book_detail: None,
        book_chapters: Vec::new(),
        book_bookmarks: Vec::new(),
        book_speed: 1.0,
        book_resume_index: -1,

        radio_tab: s.radio_tab.clone(),
        radio_categories: Vec::new(),
        radio_cat_open: s.radio_cat_open,
        radio_cat_title: s.radio_cat_title.clone(),
        radio_stations: Vec::new(),
        radio_total: 0,
        radio_page: s.radio_page,
        radio_pages: 1,
        radio_sort: s.radio_sort.clone(),

        yt_tab: s.yt_tab.clone(),
        yt_results: s.yt_results.clone(),
        yt_recent: Vec::new(),
        yt_cached: Vec::new(),
        yt_downloads: Vec::new(),
        yt_dl_page: s.yt_dl_page,
        yt_dl_pages: 1,
        yt_dl_total: 0,
        yt_subs: Vec::new(),
        yt_sub_count: 0,
        yt_subs_page: s.yt_subs_page,
        yt_subs_pages: 1,
        yt_playlists: Vec::new(),
        yt_channel_open: s.yt_channel_open,
        yt_channel_id: s.yt_channel_id.clone(),
        yt_channel_title: s.yt_channel_title.clone(),
        yt_channel_avatar: s.yt_channel_avatar.clone(),
        yt_channel_subscribed: s.yt_channel_subscribed,
        yt_channel_mode: s.yt_channel_mode.clone(),
        yt_channel_videos: s.yt_channel_videos.clone(),
        yt_playlist_open: s.yt_playlist_open,
        yt_playlist_id: s.yt_playlist_id,
        yt_playlist_title: s.yt_playlist_title.clone(),
        yt_playlist_videos: Vec::new(),
        yt_status: s.yt_status.clone(),
        yt_recommended: s.yt_recommended.clone(),
        yt_results_more: s.yt_results_more,
        yt_dl_sort: s.yt_dl_sort.clone(),
        yt_subs_sort: s.yt_subs_sort.clone(),
        yt_subs_dir: s.yt_subs_dir.clone(),
        yt_subs_filter: s.yt_subs_filter.clone(),
        yt_playlist_sort: s.yt_playlist_sort.clone(),
        yt_channel_sub: String::new(),
        yt_busy: !yt_jobs_now.is_empty(),
        yt_jobs: yt_jobs_now,
        yt_default_res: tulipix_music::yt_prefs::default_res(),
        yt_home_channels: tulipix_music::yt_prefs::home_channels(),
        yt_home_subs: Vec::new(),
        yt_fetcher: tulipix_music::yt_prefs::fetcher(),
        yt_fetch_busy: fetch.0,
        yt_fetch_frac: fetch.1,
        yt_fetch_msg: fetch.2,
        yt_channel_query: s.yt_channel_query.clone(),
        yt_channel_results: s.yt_channel_results.clone(),
        yt_channel_page: s.yt_channel_page,
        yt_channel_has_next: s.yt_channel_has_next,
        yt_watching: yt_watch().lock().map(|g| g.0 != 0).unwrap_or(false),
        yt_playing_pl_id: s.yt_playing_pl_id,
        yt_home_connect: setting("music.yt.home-connect", "1") != "0",
    };

    // Whatever tab is open: the player bar's "add to playlist" is on every one
    // of them.
    st.playlists = playlist_cards(pool).await;

    match s.view.as_str() {
        "mymusic" => fill_mymusic(pool, &s, &mut st).await,
        "podcasts" => fill_podcasts(&s, &mut st).await,
        "audiobooks" => fill_books(pool, &s, &mut st).await,
        "radio" => fill_radio(&s, &mut st).await,
        "youtube" => fill_youtube(&s, &mut st).await,
        _ => {}
    }
    // Cheap unless something is open: `mgr_load` reads nothing at all when the
    // modal is closed and no search is in flight.
    if s.mgr_open || s.mgr_mode != "list" {
        mgr_load(pool, &s, &mut st).await;
    }
    // The tray menu, from here rather than from the deck: this is the only
    // place that has the transport and the shuffle/repeat switches at once, and
    // `update_tray` drops a push that matches the last one, so running it on
    // every snapshot costs one comparison.
    crate::shellsurface::set_tray(tulipix_platform::TrayNowPlaying {
        title: st.now.title.clone(),
        artist: st.now.artist.clone(),
        playing: st.now.playing,
        has_track: st.now.loaded || !st.now.title.is_empty(),
        shuffle: st.shuffle,
        repeat: st.repeat.clone(),
        art_png: Vec::new(),
        popup: false,
    });
    Ok(st)
}

/// Record the page a list actually landed on, in the snapshot *and* in the
/// session. Without the second half the clamp would be cosmetic: the next
/// Refresh would read the stale page straight back out and clamp it again.
fn set_song_page(st: &mut MusicState, page: i64) {
    st.song_page = page;
    lock().song_page = page;
}

async fn fill_mymusic(pool: &sqlx::SqlitePool, s: &Session, st: &mut MusicState) {
    st.track_count = tulipix_music::scan::track_count(pool).await.unwrap_or(0);
    st.roots = load_watched_folders()
        .into_iter()
        .map(|dir| {
            let path = dir.to_string_lossy().into_owned();
            BrowseCard {
                id: 0,
                title: dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone()),
                subtitle: path.clone(),
                key: path,
                count: 0,
                art: String::new(),
                loved: false,
                stars: 0,
            }
        })
        .collect();
    for root in st.roots.iter_mut() {
        let prefix = format!("{}%", root.key);
        root.count = sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM items i JOIN track_meta tm ON tm.item_id = i.id \
             WHERE i.section = 'music' AND i.missing_since IS NULL \
               AND COALESCE(tm.is_audiobook, 0) = 0 AND i.abs_path LIKE ?",
        )
        .bind(&prefix)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    }

    if s.detail_open {
        let (tracks, title, subtitle, art) = detail_tracks(pool, s).await;
        st.detail_title = title;
        st.detail_subtitle = subtitle;
        st.detail_art = art;
        // The heart and the stars belong to the album or the artist row, not to
        // any of the tracks under it -- the same pair the grid tile draws.
        if s.detail_kind == "album" || s.detail_kind == "artist" {
            ensure_browse_columns(pool).await;
            let table = if s.detail_kind == "album" { "albums" } else { "artists" };
            let row: (i64, i64) = sqlx::query_as(&format!(
                "SELECT COALESCE(loved, 0), COALESCE(rating, 0) FROM {table} WHERE id = ?"
            ))
            .bind(s.detail_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or((0, 0));
            st.detail_loved = row.0 != 0;
            st.detail_stars = row.1;
        }
        if s.detail_kind == "album" {
            st.detail_artist_id = sqlx::query_scalar("SELECT artist_id FROM albums WHERE id = ?")
                .bind(s.detail_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .unwrap_or(0);
        }
        if s.detail_kind == "artist" {
            st.detail_info = artist_bio(pool, s.detail_id, &st.detail_title).await;
        }
        // An artist page also lists their albums, which is the one detail page
        // that is two lists rather than one.
        if s.detail_kind == "artist" {
            let rows: Vec<(i64, String, i64, String)> = sqlx::query_as(
                "SELECT al.id, al.title, \
                        (SELECT COUNT(*) FROM track_meta t WHERE t.album_id = al.id), \
                        COALESCE(al.cover_path, '') \
                 FROM albums al WHERE al.artist_id = ? ORDER BY COALESCE(al.year, 0) DESC",
            )
            .bind(s.detail_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            st.detail_albums = rows
                .into_iter()
                .map(|(id, title, count, cover)| BrowseCard {
                    id,
                    key: id.to_string(),
                    title,
                    subtitle: format!("{count} tracks"),
                    count,
                    art: if !cover.is_empty() && Path::new(&cover).exists() {
                        cover
                    } else {
                        String::new()
                    },
                    loved: false,
                    stars: 0,
                })
                .collect();
        }
        st.detail_tracks = tracks;
        return;
    }

    match s.lib_tab.as_str() {
        "home" => {
            let raw = tulipix_music::dashboard::stats(pool)
                .await
                .unwrap_or_default();
            st.stats = Stats {
                week: tulipix_music::dashboard::fmt_listen(raw.week_ms),
                total: tulipix_music::dashboard::fmt_listen(raw.total_ms),
                genre: raw.top_genre.unwrap_or_else(|| "—".into()),
                streak: if raw.streak_days > 0 {
                    format!("{} days", raw.streak_days)
                } else {
                    "—".into()
                },
            };
            let recent = tulipix_music::dashboard::recently_played(pool, RAIL)
                .await
                .unwrap_or_default();
            let most = tulipix_music::dashboard::most_played(pool, RAIL)
                .await
                .unwrap_or_default();
            let loved = tulipix_music::rating::loved(pool, RAIL).await.unwrap_or_default();
            let fresh = tulipix_music::dashboard::new_this_week(pool, RAIL)
                .await
                .unwrap_or_default();
            st.rail_recent = tracks_by_ids(pool, &recent).await;
            st.rail_most = tracks_by_ids(pool, &most).await;
            st.rail_loved = tracks_by_ids(pool, &loved).await;
            st.rail_fresh = tracks_by_ids(pool, &fresh).await;
            let (albums, _, _) = browse_cards(pool, "albums", s).await;
            st.rail_albums = albums.into_iter().take(RAIL as usize).collect();
            let (artists, _, _) = browse_cards(pool, "artists", s).await;
            st.rail_artists = artists.into_iter().take(RAIL as usize).collect();
        }
        "songs" => {
            let (songs, total, page) = songs_page(pool, s).await;
            st.songs = songs;
            st.song_total = total;
            st.song_pages = page_count(total, SONGS_PAGE);
            set_song_page(st, page);
        }
        "favorites" => {
            let ids = tulipix_music::rating::loved(pool, 5_000).await.unwrap_or_default();
            let all = tracks_by_ids(pool, &ids).await;
            st.song_total = all.len() as i64;
            st.song_pages = page_count(st.song_total, LIST_ROWS);
            let page = clamp_page(s.song_page, st.song_total, LIST_ROWS);
            set_song_page(st, page);
            let start = ((page * LIST_ROWS) as usize).min(all.len());
            let end = (((page + 1) * LIST_ROWS) as usize).min(all.len());
            st.songs = all[start..end].to_vec();
            // Loved is three things, the way search is: the artists you loved,
            // the albums you loved, and the tracks. A heart on an album is
            // stored on the album row and is NOT the same claim as a heart on
            // each of its tracks, so a page that only listed tracks could not
            // show it at all.
            //
            // They ride on `rail_artists` / `rail_albums`, which the Home
            // dashboard fills for its own two shelves. One tab is open at a
            // time, so the two uses never collide, and a pair of new fields
            // for the same two lists of cards would only mean two places to
            // keep in step.
            st.rail_artists = loved_cards(pool, "artists").await;
            st.rail_albums = loved_cards(pool, "albums").await;
        }
        "history" => {
            let ids: Vec<i64> = sqlx::query_scalar(
                "SELECT ph.item_id FROM play_history ph ORDER BY ph.played_at DESC LIMIT 500",
            )
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            let all = tracks_by_ids(pool, &ids).await;
            st.song_total = all.len() as i64;
            st.song_pages = page_count(st.song_total, LIST_ROWS);
            let page = clamp_page(s.song_page, st.song_total, LIST_ROWS);
            set_song_page(st, page);
            let start = ((page * LIST_ROWS) as usize).min(all.len());
            let end = (((page + 1) * LIST_ROWS) as usize).min(all.len());
            st.songs = all[start..end].to_vec();
        }
        // The Downloader draws from its own module and needs nothing from the
        // library query — without this arm it would fall through to
        // `browse_cards` and pay for a grid nobody is looking at.
        "downloader" => {}
        other => {
            let (cards, total, page) = browse_cards(pool, other, s).await;
            st.cards = cards;
            st.card_total = total;
            st.browse_pages = page_count(total, browse_page_size(other));
            st.browse_page = page;
            lock().browse_page = page;
        }
    }
}

async fn fill_podcasts(s: &Session, st: &mut MusicState) {
    let Ok(pool) = podcasts_pool().await else { return };

    st.pod_categories = {
        let mut cats: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT category FROM podcasts WHERE category IS NOT NULL AND category != '' \
             ORDER BY category COLLATE NOCASE",
        )
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        cats.insert(0, "All".into());
        cats
    };

    if s.pod_open >= 0 {
        let show: Option<ShowRow> = sqlx::query_as(&format!("{SHOW_SELECT} WHERE p.id = ?"))
            .bind(s.pod_open)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
        st.pod_detail = show.map(into_show);
        let order = if s.pod_ep_sort == "old" {
            "ASC"
        } else {
            "DESC"
        };
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?",
        )
        .bind(s.pod_open)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        let rows: Vec<EpisodeRow> = sqlx::query_as(&format!(
            "{EPISODE_SELECT} WHERE e.podcast_id = ? \
             ORDER BY COALESCE(e.published, 0) {order} LIMIT ? OFFSET ?"
        ))
        .bind(s.pod_open)
        .bind(LIST_PAGE)
        .bind(s.pod_ep_page * LIST_PAGE)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        st.pod_episodes = rows.into_iter().map(into_episode).collect();
        st.pod_ep_pages = (total.max(1) as f64 / LIST_PAGE as f64).ceil() as i64;
        return;
    }

    let query = s.pod_queries.get(&s.pod_tab).cloned().unwrap_or_default();
    let filtered = s.pod_cat != "All";
    let searching = !query.is_empty();
    let like = format!("%{query}%");
    let mut wheres: Vec<&str> = Vec::new();
    if filtered {
        wheres.push("p.category = ?");
    }
    if searching {
        wheres.push("(p.title LIKE ? OR p.author LIKE ?)");
    }
    let where_cat = if wheres.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", wheres.join(" AND "))
    };
    // Bound in the order the placeholders appear, category first.
    let count_sql = format!("SELECT COUNT(*) FROM podcasts p{where_cat}");
    let mut cq = sqlx::query_scalar::<_, i64>(&count_sql);
    if filtered {
        cq = cq.bind(s.pod_cat.clone());
    }
    if searching {
        cq = cq.bind(like.clone()).bind(like.clone());
    }
    st.pod_total = cq.fetch_one(pool).await.unwrap_or(0);

    let show_sql =
        format!("{SHOW_SELECT}{where_cat} ORDER BY p.title COLLATE NOCASE LIMIT ? OFFSET ?");
    let mut q = sqlx::query_as::<_, ShowRow>(&show_sql);
    if filtered {
        q = q.bind(s.pod_cat.clone());
    }
    if searching {
        q = q.bind(like.clone()).bind(like.clone());
    }
    let rows = q
        .bind(LIST_PAGE)
        .bind(s.pod_page * LIST_PAGE)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
    st.pod_shows = rows.into_iter().map(into_show).collect();
    st.pod_pages = (st.pod_total.max(1) as f64 / LIST_PAGE as f64).ceil() as i64;

    if s.pod_tab == "home" {
        // "Your shows" is the shows the user PINNED, not the first page of the
        // subscription list -- the whole point of Home is that it is a chosen
        // shelf. Sorted here rather than in SQL because `latest` is a computed
        // column in SHOW_SELECT.
        let pinned: Vec<ShowRow> = sqlx::query_as(&format!(
            "{SHOW_SELECT} WHERE COALESCE(p.home_pinned, 0) = 1"
        ))
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        let mut home: Vec<PodcastShow> = pinned.into_iter().map(into_show).collect();
        match s.pod_home_sort.as_str() {
            "category" => home.sort_by(|a, b| {
                a.category
                    .to_lowercase()
                    .cmp(&b.category.to_lowercase())
                    .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            }),
            "latest" => home.sort_by(|a, b| b.latest.cmp(&a.latest)),
            _ => home.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        }
        if let Some(q) = s.pod_queries.get("home").filter(|q| !q.is_empty()) {
            let needle = q.to_lowercase();
            home.retain(|h| {
                h.title.to_lowercase().contains(&needle)
                    || h.author.to_lowercase().contains(&needle)
            });
        }
        // Paged, like Slint's: pinning forty shows makes a rail nobody can
        // reach the end of, and the rail is meant to be a shelf.
        st.pod_home_pages = (home.len().max(1) as f64 / HOME_PAGE as f64).ceil() as i64;
        let from = ((s.pod_home_page * HOME_PAGE) as usize).min(home.len());
        let to = (((s.pod_home_page + 1) * HOME_PAGE) as usize).min(home.len());
        st.pod_home = home[from..to].to_vec();

        // Newest episode per show, which is what "what's new" means when you
        // follow forty podcasts and one of them posts daily.
        let rows: Vec<EpisodeRow> = sqlx::query_as(&format!(
            "{EPISODE_SELECT} WHERE e.id IN ( \
                 SELECT id FROM podcast_episodes pe \
                 WHERE pe.published = (SELECT MAX(published) FROM podcast_episodes x \
                                       WHERE x.podcast_id = pe.podcast_id) \
                 GROUP BY pe.podcast_id) \
             ORDER BY COALESCE(e.published, 0) DESC LIMIT ?"
        ))
        .bind(RAIL)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        st.pod_latest = rows.into_iter().map(into_episode).collect();
    }

    if s.pod_tab == "downloads" {
        // "dl" is the order they were saved in, which sqlite gives us for free
        // as the row id -- there is no downloaded_at column and adding one to
        // sort a list nobody paginates past page two is not worth a migration.
        let order = match s.pod_dl_sort.as_str() {
            "new" => "COALESCE(e.published, 0) DESC",
            "old" => "COALESCE(e.published, 0) ASC",
            _ => "e.id DESC",
        };
        let rows: Vec<EpisodeRow> = sqlx::query_as(&format!(
            "{EPISODE_SELECT} WHERE e.downloaded_path IS NOT NULL AND e.downloaded_path != '' \
             ORDER BY {order} LIMIT ? OFFSET ?"
        ))
        .bind(LIST_PAGE)
        .bind(s.pod_page * LIST_PAGE)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        st.pod_downloads = rows.into_iter().map(into_episode).collect();
        if let Some(q) = s.pod_queries.get("downloads").filter(|q| !q.is_empty()) {
            let needle = q.to_lowercase();
            st.pod_downloads.retain(|e| {
                e.title.to_lowercase().contains(&needle)
                    || e.show.to_lowercase().contains(&needle)
            });
        }
        // The pager was reading `pod_pages`, which counts SHOWS -- so the
        // downloads list paged against the wrong total and either stopped
        // early or offered pages with nothing on them.
        let saved: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM podcast_episodes \
             WHERE downloaded_path IS NOT NULL AND downloaded_path != ''",
        )
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        st.pod_pages = (saved.max(1) as f64 / LIST_PAGE as f64).ceil() as i64;
    }

    if s.pod_tab == "trends" {
        let subscribed = tulipix_music::pod_trends::subscribed_urls(pool).await;
        let mut cards: Vec<PodTrend> = s
            .pod_trends
            .iter()
            .enumerate()
            .map(|(i, m)| PodTrend {
                idx: i as i64,
                feed_url: m.feed_url.clone(),
                title: m.title.clone(),
                author: m.author.clone(),
                category: m.category.clone(),
                art: m.art.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default(),
                subscribed: subscribed.iter().any(|u| u == &m.feed_url),
            })
            .collect();
        if let Some(q) = s.pod_queries.get("trends").filter(|q| !q.is_empty()) {
            let needle = q.to_lowercase();
            cards.retain(|c| {
                c.title.to_lowercase().contains(&needle)
                    || c.author.to_lowercase().contains(&needle)
                    || c.category.to_lowercase().contains(&needle)
            });
        }
        if s.pod_trends_sort == "category" {
            cards.sort_by(|a, b| {
                a.category
                    .to_lowercase()
                    .cmp(&b.category.to_lowercase())
                    .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            });
        } else {
            cards.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
        }
        let total = cards.len() as i64;
        st.pod_trends_pages = (total.max(1) as f64 / TRENDS_PAGE as f64).ceil() as i64;
        st.pod_trends = cards
            .into_iter()
            .skip((s.pod_trends_page * TRENDS_PAGE) as usize)
            .take(TRENDS_PAGE as usize)
            .collect();
    }

    if !s.pod_queue.is_empty() {
        let holes = vec!["?"; s.pod_queue.len()].join(",");
        let queue_sql = format!("{EPISODE_SELECT} WHERE e.id IN ({holes})");
        let mut q = sqlx::query_as::<_, EpisodeRow>(&queue_sql);
        for id in &s.pod_queue {
            q = q.bind(id);
        }
        let rows = q.fetch_all(pool).await.unwrap_or_default();
        let mut by_id: std::collections::HashMap<i64, Episode> =
            rows.into_iter().map(|r| (r.0, into_episode(r))).collect();
        st.pod_queue = s
            .pod_queue
            .iter()
            .filter_map(|id| by_id.remove(id))
            .collect();
    }
}

async fn fill_books(pool: &sqlx::SqlitePool, s: &Session, st: &mut MusicState) {
    st.books = book_cards(pool, &s.book_tab).await;
    if s.book_open.is_empty() {
        return;
    }
    st.book_detail = st.books.iter().find(|b| b.folder == s.book_open).cloned();
    if st.book_detail.is_none() {
        // The open book was filtered out by the tab (a finished book while the
        // "In progress" tab is showing). Its detail page still has to work.
        st.book_detail = book_cards(pool, "all")
            .await
            .into_iter()
            .find(|b| b.folder == s.book_open);
    }
    let ids = tulipix_music::audiobooks::book_chapters(pool, &s.book_open)
        .await
        .unwrap_or_default();
    let (finished, resume_id) = tulipix_music::audiobooks::chapter_states(pool, &ids)
        .await
        .unwrap_or_default();
    let rows: Vec<(i64, String, f64, f64)> = if ids.is_empty() {
        Vec::new()
    } else {
        let holes = vec!["?"; ids.len()].join(",");
        let chapter_sql = format!(
            "SELECT tm.item_id, COALESCE(tm.title, ''), COALESCE(tm.duration_s, 0.0), \
                    COALESCE(ap.position_s, 0.0) \
             FROM track_meta tm LEFT JOIN audiobook_progress ap ON ap.item_id = tm.item_id \
             WHERE tm.item_id IN ({holes})"
        );
        let mut q = sqlx::query_as::<_, (i64, String, f64, f64)>(&chapter_sql);
        for id in &ids {
            q = q.bind(id);
        }
        q.fetch_all(pool).await.unwrap_or_default()
    };
    let by_id: std::collections::HashMap<i64, (String, f64, f64)> = rows
        .into_iter()
        .map(|(id, t, d, p)| (id, (t, d, p)))
        .collect();
    st.book_chapters = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let (title, duration_s, position_s) =
                by_id.get(id).cloned().unwrap_or_default();
            Chapter {
                item_id: *id,
                title: if title.is_empty() {
                    format!("Chapter {}", i + 1)
                } else {
                    title
                },
                duration_s,
                position_s,
                finished: finished.contains(id),
            }
        })
        .collect();
    st.book_resume_index = resume_id
        .and_then(|r| ids.iter().position(|i| *i == r))
        .map(|p| p as i64)
        .unwrap_or(-1);
    st.book_bookmarks = book_bookmarks(pool).await.unwrap_or_default();
    // Speed is stored per chapter but set per book, so the first chapter is
    // the book's answer. No chapters means nothing to read it off, and the
    // default stands.
    st.book_speed = match ids.first() {
        Some(first) => tulipix_music::audiobooks::book_speed(pool, *first)
            .await
            .unwrap_or(1.0),
        None => 1.0,
    };
}

async fn fill_radio(s: &Session, st: &mut MusicState) {
    let Ok(pool) = radio_pool().await else { return };
    let favs = tulipix_music::radio::favourite_uuids(pool)
        .await
        .unwrap_or_default();
    let counts: std::collections::HashMap<String, i64> =
        tulipix_music::radio::cache_counts(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
    st.radio_categories = tulipix_music::radio::PRESETS
        .iter()
        .map(|(label, _)| RadioCategory {
            label: (*label).to_string(),
            icon: radio_icon(label).to_string(),
            count: counts.get(*label).copied().unwrap_or(0),
        })
        .collect();
    st.radio_total = counts.values().sum();

    let mut list: Vec<tulipix_music::radio::Station> = if s.radio_cat_open {
        s.radio_list.clone()
    } else {
        match s.radio_tab.as_str() {
            "favourites" => tulipix_music::radio::favourites(pool)
                .await
                .unwrap_or_default(),
            "recent" => tulipix_music::radio::recents(pool, 100)
                .await
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    };
    match s.radio_sort.as_str() {
        "name" => list.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
        "bitrate" => list.sort_by(|a, b| a.bitrate.cmp(&b.bitrate)),
        _ => {}
    }
    if s.radio_sort != "default" && s.radio_sort_dir == "desc" {
        list.reverse();
    }
    let total = list.len() as i64;
    st.radio_pages = (total.max(1) as f64 / LIST_PAGE as f64).ceil() as i64;
    let start = ((s.radio_page * LIST_PAGE) as usize).min(list.len());
    let end = (((s.radio_page + 1) * LIST_PAGE) as usize).min(list.len());
    st.radio_stations = list[start..end]
        .iter()
        .cloned()
        .map(|x| into_station(x, &favs))
        .collect();

    // The list the transport steps through is the whole page's worth, not the
    // window: Next at the bottom of page one should reach page two.
    lock().radio_list = list;
}

fn into_sub(x: &tulipix_music::youtube::store::Sub) -> YtSub {
    YtSub {
        channel_id: x.channel_id.clone(),
        title: x.title.clone(),
        avatar: x.avatar_path.clone().unwrap_or_default(),
        videos: x.video_count.unwrap_or(0),
        subs: x.sub_count.unwrap_or(0),
        subscribed: x.subscribed,
    }
}

async fn fill_youtube(s: &Session, st: &mut MusicState) {
    let Ok(pool) = youtube_pool().await else { return };
    let progress: std::collections::HashMap<String, f32> =
        tulipix_music::youtube::store::progress_map(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();

    st.yt_recent = tulipix_music::youtube::store::list_recent_searches(pool, 6)
        .await
        .unwrap_or_default();

    if s.yt_playlist_open {
        st.yt_playlist_videos = tulipix_music::youtube::store::playlist_items(
            pool,
            s.yt_playlist_id,
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| YtVideo {
            progress: progress.get(&p.video_id).copied().unwrap_or(0.0) as f64,
            video_id: p.video_id,
            title: p.title,
            channel: p.channel,
            thumb: p.thumb_path,
            duration: p.duration,
            media_path: String::new(),
            meta: String::new(),
            quality: String::new(),
        })
        .collect();
        // "default" is the playlist's own order, which is the one thing a
        // playlist actually carries -- so it sorts nothing.
        match s.yt_playlist_sort.as_str() {
            "title" => st
                .yt_playlist_videos
                .sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            "duration" => st.yt_playlist_videos.sort_by(|a, b| a.duration.cmp(&b.duration)),
            _ => {}
        }
        return;
    }

    match s.yt_tab.as_str() {
        "cached" | "home" => {
            st.yt_cached = tulipix_music::youtube::store::list_cached(pool, LIST_PAGE)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    let p = progress.get(&v.video_id).copied().unwrap_or(0.0) as f64;
                    into_yt(v, p)
                })
                .collect();
        }
        _ => {}
    }

    if s.yt_tab == "downloads" || s.yt_tab == "home" {
        let mut all = tulipix_music::youtube::store::list_downloads(pool, 1_000)
            .await
            .unwrap_or_default();
        // `list_downloads` is newest-first already, so "new" is the identity.
        match s.yt_dl_sort.as_str() {
            "old" => all.reverse(),
            "az" => all.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            _ => {}
        }
        st.yt_dl_total = all.len() as i64;
        st.yt_dl_pages = (st.yt_dl_total.max(1) as f64 / LIST_PAGE as f64).ceil() as i64;
        let start = ((s.yt_dl_page * LIST_PAGE) as usize).min(all.len());
        let end = (((s.yt_dl_page + 1) * LIST_PAGE) as usize).min(all.len());
        st.yt_downloads = all[start..end]
            .iter()
            .cloned()
            .map(|v| {
                let p = progress.get(&v.video_id).copied().unwrap_or(0.0) as f64;
                into_yt(v, p)
            })
            .collect();
    }

    if s.yt_tab == "subscriptions" || s.yt_tab == "home" {
        // (see `into_sub` below for the row -> card mapping)
        let all = tulipix_music::youtube::store::list_subs(pool)
            .await
            .unwrap_or_default();
        // The count in the tab badge is subscriptions, always -- it must not
        // move when the page is flipped to show the unsubscribed half.
        st.yt_sub_count = all.iter().filter(|sub| sub.subscribed).count() as i64;
        let want_sub = s.yt_subs_filter != "unsub";
        let mut subs: Vec<_> = all
            .into_iter()
            .filter(|x| x.subscribed == want_sub)
            .collect();
        match s.yt_subs_sort.as_str() {
            "name" => subs.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            "videos" => subs.sort_by(|a, b| a.video_count.unwrap_or(0).cmp(&b.video_count.unwrap_or(0))),
            _ => subs.sort_by(|a, b| a.sub_count.unwrap_or(0).cmp(&b.sub_count.unwrap_or(0))),
        }
        if s.yt_subs_dir == "desc" {
            subs.reverse();
        }
        st.yt_subs_pages = (subs.len().max(1) as f64 / LIST_PAGE as f64).ceil() as i64;
        let start = ((s.yt_subs_page * LIST_PAGE) as usize).min(subs.len());
        let end = (((s.yt_subs_page + 1) * LIST_PAGE) as usize).min(subs.len());
        st.yt_subs = subs[start..end].iter().map(into_sub).collect();
    }

    if s.yt_tab == "home" {
        // The newest video from each subscribed channel, from the per-channel
        // cache the channel pages already fill -- no network, so Home is warm
        // rather than empty on arrival.
        let subs = tulipix_music::youtube::store::list_subs(pool)
            .await
            .unwrap_or_default();
        let mut reco: Vec<YtVideo> = Vec::new();
        for sub in subs.iter().filter(|x| x.subscribed).take(RAIL as usize) {
            let cached = tulipix_music::youtube::store::get_channel_cache(pool, &sub.channel_id)
                .await
                .unwrap_or_default();
            if let Some(v) = cached.into_iter().next() {
                reco.push(YtVideo {
                    progress: progress.get(&v.video_id).copied().unwrap_or(0.0) as f64,
                    video_id: v.video_id,
                    title: v.title,
                    channel: sub.title.clone(),
                    thumb: v.thumb_path,
                    duration: v.duration,
                    media_path: String::new(),
                    meta: String::new(),
                    quality: String::new(),
                });
            }
        }
        st.yt_recommended = reco;

        // The Home rail is the channels you pinned; until you pin any it is
        // the most-followed subscriptions, so the rail is never empty just
        // because you have not discovered pinning yet.
        let pins = tulipix_music::yt_prefs::home_channels();
        let mut rail: Vec<YtSub> = if pins.is_empty() {
            let mut by_reach: Vec<_> = subs.iter().filter(|x| x.subscribed).collect();
            by_reach.sort_by(|a, b| b.sub_count.unwrap_or(0).cmp(&a.sub_count.unwrap_or(0)));
            by_reach
                .into_iter()
                .take(tulipix_music::yt_prefs::HOME_MAX)
                .map(into_sub)
                .collect()
        } else {
            pins.iter()
                .filter_map(|pid| subs.iter().find(|x| &x.channel_id == pid))
                .take(tulipix_music::yt_prefs::HOME_MAX)
                .map(into_sub)
                .collect()
        };
        rail.truncate(tulipix_music::yt_prefs::HOME_MAX);
        st.yt_home_subs = rail;
    }

    if s.yt_channel_open {
        if let Some(sub) = tulipix_music::youtube::store::list_subs(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .find(|x| x.channel_id == s.yt_channel_id)
        {
            st.yt_channel_sub = [
                sub.video_count.map(|n| format!("{n} videos")),
                sub.sub_count.map(|n| format!("{n} subscribers")),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join("  ·  ");
        }
    }

    if s.yt_tab == "playlists" || s.yt_tab == "home" {
        st.yt_playlists = tulipix_music::youtube::store::list_playlists(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|p| YtPlaylist {
                id: p.id,
                name: p.name,
                count: p.count,
                cover: p.cover.unwrap_or_default(),
                source_url: p.source_url.unwrap_or_default(),
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn yt_progress_lines_parse() {
        // Not assert_eq on the float: 42.3/100.0 is not bit-identical to 0.423.
        let got = yt_dl_percent("[download]  42.3% of 120MiB at 3MiB/s").unwrap();
        assert!((got - 0.423).abs() < 1e-9, "{got}");
        assert_eq!(yt_dl_percent("[download] 100% of 120MiB"), Some(1.0));
        assert_eq!(yt_dl_percent("[download]   0.0% of ~1.00MiB"), Some(0.0));
        // Everything else yt-dlp writes on stdout must not move the bar.
        assert_eq!(yt_dl_percent("[youtube] abc123: Downloading webpage"), None);
        assert_eq!(yt_dl_percent("[download] Destination: /tmp/x.mkv"), None);
        assert_eq!(yt_dl_percent("[Merger] Merging formats into \"x.mkv\""), None);
    }

    /// A track with no duration tag must not take the rail down with it.
    ///
    /// SQLite types values, not columns: `COALESCE(tm.duration_s, 0)` returns
    /// an INTEGER for the untagged row and a REAL for every other, and sqlx's
    /// `f64` accepts only Float -- so one such file emptied Recently played,
    /// Most played, Loved, Recently added and the queue at once. The fix is the
    /// `0.0` in `TRACK_SELECT`; this is what fails if it ever goes back to `0`.
    #[tokio::test]
    async fn a_track_with_no_duration_still_resolves() {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        for ddl in [
            "CREATE TABLE items (id INTEGER PRIMARY KEY, abs_path TEXT, section TEXT, \
             missing_since INTEGER, added INTEGER)",
            "CREATE TABLE track_meta (item_id INTEGER, title TEXT, artist_id INTEGER, \
             album_id INTEGER, duration_s REAL, loved INTEGER, rating INTEGER, \
             play_count INTEGER, track_no INTEGER, year INTEGER, genre TEXT, \
             is_audiobook INTEGER DEFAULT 0, last_played INTEGER)",
            "CREATE TABLE artists (id INTEGER PRIMARY KEY, name TEXT)",
            "CREATE TABLE albums (id INTEGER PRIMARY KEY, title TEXT, cover_path TEXT, \
             artist_id INTEGER)",
            "CREATE TABLE lyrics (item_id INTEGER, synced INTEGER, content TEXT)",
        ] {
            sqlx::query(ddl).execute(&pool).await.unwrap();
        }
        for (id, dur) in [(1_i64, Some(180.5_f64)), (2, None)] {
            sqlx::query("INSERT INTO items (id, abs_path, section) VALUES (?, ?, 'music')")
                .bind(id)
                .bind(format!("/m/{id}.flac"))
                .execute(&pool)
                .await
                .unwrap();
            sqlx::query("INSERT INTO track_meta (item_id, title, duration_s) VALUES (?, ?, ?)")
                .bind(id)
                .bind(format!("track {id}"))
                .bind(dur)
                .execute(&pool)
                .await
                .unwrap();
        }

        // The untagged track alone, and then in company: the rails ask for a
        // set, so one bad row used to cost every other row in the same query.
        assert_eq!(tracks_by_ids(&pool, &[2]).await.len(), 1);
        let both = tracks_by_ids(&pool, &[1, 2]).await;
        assert_eq!(both.len(), 2);
        assert_eq!(both[0].duration_s, 180.5);
        assert_eq!(both[1].duration_s, 0.0);
    }

    #[test]
    fn channel_search_query_cannot_end_the_url() {
        assert_eq!(urlish("two words"), "two+words");
        // & and # would truncate the query; % would start an escape.
        assert_eq!(urlish("rock & roll"), "rock+%26+roll");
        assert_eq!(urlish("c#"), "c%23");
        assert_eq!(urlish("100%"), "100%25");
        assert_eq!(urlish("a/b?c"), "a%2Fb%3Fc");
        assert_eq!(urlish("plain"), "plain");
    }
}
