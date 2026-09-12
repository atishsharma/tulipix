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
/// The Folders grid: six across, three down.
const FOLDER_PAGE: i64 = 18;
/// The Genres grid: seven across, three down.
const GENRE_PAGE: i64 = 21;
/// Loved and History, which are lists rather than grids: twenty-five rows is
/// about a screen of them.
const LIST_ROWS: i64 = 25;
/// Loved's songs, under its two shelves: twenty to a page.
const LOVED_ROWS: i64 = 20;
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
    /// Which disc of a set. 0 when the file is not tagged with one, which is
    /// every single-disc album -- the detail page only draws a divider once it
    /// sees more than one distinct value, so an untagged library is unchanged.
    pub disc_no: i64,
    /// Measured tempo, or 0 when the track has not been analysed. Analysis is
    /// opt-in and per-track, so most of a library reads 0 and the UI shows
    /// nothing rather than a guess.
    pub bpm: f64,
    /// Camelot key code ("8A"), or empty.
    pub music_key: String,
    /// Dynamic-range score, or 0. Roughly: under 8 is a loudness-war master,
    /// over 14 is a dynamic one.
    pub dr_score: f64,
    pub year: i64,
    pub genre: String,
    /// "" when the track has no stored lyrics; "synced" when they carry LRC
    /// timestamps, "plain" when they do not.
    pub lyrics: String,
    /// Whether the file carries composer, performer, producer or remixer tags.
    /// Only a flag: the credits themselves come from `music_track_credits`,
    /// fetched when a row is actually expanded.
    pub has_credits: bool,
}

/// An album, artist, genre, playlist or folder tile. `key` carries the
/// identity for the ones that have no integer id: a genre is its name, a
/// folder is its absolute path. A folder's `id` is its most-played track,
/// whose cover the tile wears.
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


/// One row of a listening breakdown, pre-formatted for the same reason [`Stats`]
/// is: these are read, not computed against.
#[derive(Debug, Clone)]
pub struct Tally {
    pub label: String,
    /// What to open when the row is tapped. 0 when the row is not a place --
    /// a genre has no id, and neither does an hour of the day.
    pub key: i64,
    /// Listening time, already written out ("12h 30m").
    pub value: String,
    pub plays: i64,
    /// This row against the biggest in its list, 0..1, so a bar is a width and
    /// not a division Dart repeats on every rebuild.
    pub frac: f64,
}

/// What the listening summary shows for one window of time.
#[derive(Debug, Clone, Default)]
pub struct Listening {
    /// "Last 7 days" / "This year" / "All time".
    pub range: String,
    pub total: String,
    pub artists: Vec<Tally>,
    pub genres: Vec<Tally>,
    pub tracks: Vec<Tally>,
    /// Twenty-four bars, midnight first, each 0..1 against the busiest hour.
    pub hours: Vec<f64>,
    /// "21:00 - 22:00", or empty when nothing has been played at all.
    pub peak_hour: String,
    /// Started again and again, never finished.
    pub abandoned: Vec<Tally>,
}

/// Everything the Stats Center shows for one window. One struct, so opening the
/// popup and switching its range are one round trip each.
#[derive(Debug, Clone, Default)]
pub struct StatsCenter {
    pub listening: Listening,
    /// Against the same length of time just before: "+12%" / "−4%", or empty
    /// for all time and for a window with nothing before it.
    pub delta: String,
    pub plays: i64,
    pub streak: String,
    pub new_artists: i64,
    /// "87%", or "—" when no play in the window had a length to judge by.
    pub finished: String,
    /// Listening per day for the last 22 weeks, oldest first and today last,
    /// each 0..1 against the busiest day.
    pub days: Vec<f64>,
    pub tracks: i64,
    pub albums: i64,
    pub artists: i64,
    pub bytes: i64,
    /// "14 days 6 h" of music on the shelves.
    pub length: String,
    /// Codec shares, most first: `label` "FLAC", `value` "58%", `plays` the
    /// track count, `frac` 0..1 of the library. Four, then "Other".
    pub formats: Vec<Tally>,
    /// Tracks the analysis pass has not measured yet.
    pub pending: i64,
}

/// One copy of a recording the library holds more than once.
#[derive(Debug, Clone)]
pub struct DupeCopy {
    pub track: Track,
    /// "FLAC - 44.1 kHz" / "MP3 - 320 kbps": what the choice is actually being
    /// made on, since the filename is exactly what cannot be trusted here.
    pub quality: String,
    /// How alike this copy is to the keeper, as a percentage.
    pub confidence: i64,
    /// The best copy on quality. Never more than one per group.
    pub keep: bool,
    /// The file's size, for "space to free".
    pub bytes: i64,
    /// How many playlists hold this copy -- the places a delete would empty if
    /// they were not moved to the kept copy first.
    pub playlists: i64,
}

/// Copies of one recording, keeper first.
#[derive(Debug, Clone)]
pub struct DupeGroup {
    pub copies: Vec<DupeCopy>,
}


/// One track whose words match a lyric search, and where the words are sung.
#[derive(Debug, Clone)]
pub struct LyricMatch {
    pub item_id: i64,
    pub title: String,
    pub artist: String,
    pub art: String,
    /// The matching line itself.
    pub line: String,
    /// "1:14", or empty when the stored lyrics carry no timings.
    pub at: String,
    /// Where to seek to. Negative when there is nowhere: the hit still opens
    /// the track, it just starts at the beginning.
    pub secs: f64,
}

/// An album that was started and never finished.
#[derive(Debug, Clone)]
pub struct ResumeCard {
    pub album_id: i64,
    /// The track it would resume at.
    pub item_id: i64,
    pub title: String,
    pub artist: String,
    pub art: String,
    /// "Left off at track 6 of 10 - 2 days ago".
    pub note: String,
    /// 0..1, drawn as a bar across the art.
    pub progress: f64,
}

/// One thing a smart-playlist condition can be about.
#[derive(Debug, Clone)]
pub struct SmartField {
    pub id: String,
    pub label: String,
    /// text | number | bool | days -- which input the editor should offer.
    pub kind: String,
}

/// One comparison a condition can make.
#[derive(Debug, Clone)]
pub struct SmartOp {
    pub id: String,
    pub label: String,
}

/// What an editor can build a rule out of. Read from the engine rather than
/// written out again in Dart, so a field added in Rust appears in the editor
/// instead of quietly never being offerable.
#[derive(Debug, Clone)]
pub struct SmartSchema {
    pub fields: Vec<SmartField>,
    pub ops: Vec<SmartOp>,
}

#[derive(Debug, Clone)]
pub struct SmartCondition {
    pub field: String,
    pub op: String,
    pub value: String,
}

/// A smart playlist's rule, in the ids [`SmartSchema`] speaks.
#[derive(Debug, Clone, Default)]
pub struct SmartRuleView {
    /// all | any
    pub combine: String,
    pub conditions: Vec<SmartCondition>,
    /// 0 for no limit.
    pub limit: i64,
}



/// One folder in the browse tree.
#[derive(Debug, Clone)]
pub struct FolderNode {
    pub path: String,
    /// What to show. A run of single-child folders is collapsed into one row,
    /// so this can be several segments -- "home/you/Music" rather than three
    /// rows you have to click through.
    pub name: String,
    /// Tracks sitting directly here.
    pub direct: i64,
    /// Tracks here and everywhere beneath.
    pub total: i64,
    pub has_children: bool,
}

/// A labelled value: the album's details panel, and a track's credits.
#[derive(Debug, Clone)]
pub struct MetaRow {
    pub label: String,
    /// Free text, as tagged. A composer field holds "Yorke, Greenwood" or
    /// "Yorke/Greenwood" or one name, and nothing normalises that.
    pub value: String,
}

/// An extra shelf under a detail page, with its own heading.
///
/// One list of these rather than a field per shelf: an album page shows other
/// editions and the rest of the artist's work, an artist page shows the records
/// they only guest on, and every one of those is the same row of tiles with a
/// different title over it.
#[derive(Debug, Clone)]
pub struct Shelf {
    pub title: String,
    /// album | artist -- what tapping a tile opens. The tiles are the same
    /// either way; only the page behind them differs.
    pub kind: String,
    pub cards: Vec<BrowseCard>,
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
    /// How many of `queue` the station put there rather than the user.
    ///
    /// A count and not a flag per row: the queue panel draws one chip that
    /// says how many were suggested and takes them all back out, and a
    /// per-track source on [`Track`] would be carried by every list in the
    /// section for the sake of one of them.
    pub queue_suggested: i64,
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
    /// Visualizer style (0..5) and whether it is drawn. Kept here rather than
    /// in the Dart controller because the Slint build persists both, and a
    /// choice that resets on every launch is not a choice.
    pub viz_style: i64,
    pub viz_on: bool,
    /// Renderers found on the network, by short name. Empty until Discover has
    /// been asked for — SSDP is a multicast round trip, not something to run on
    /// every snapshot.
    pub cast_devices: Vec<String>,
    /// The one being played to, and whether it is. Empty and false is the
    /// normal state: casting is a thing you ask for.
    pub cast_target: String,
    pub cast_active: bool,
    pub cast_busy: bool,

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
    /// The same lookup's structured facts -- "Group", "GB", "Formed 1985" --
    /// which used to be folded into the prose and are now chips beside it.
    pub detail_facts: Vec<String>,
    /// The artist's MusicBrainz id, so the page can link back to where its
    /// biography came from. Empty when the lookup has not run or found nothing.
    pub detail_mbid: String,
    /// Whether the open playlist is a smart one. Only a smart playlist has
    /// rules to edit, and offering the editor on a manual one would turn it
    /// into a smart one the moment anything was saved.
    pub detail_is_smart: bool,
    /// The record's own details -- label, catalogue number, exact release date,
    /// source format, album gain, when it entered the library. Album pages
    /// only, and only the rows that have a value: a panel of six "unknown"s is
    /// worse than a panel of two facts.
    pub detail_meta: Vec<MetaRow>,
    /// Extra shelves under the page: other editions and the rest of the
    /// artist's work on an album, the records they only guest on for an artist,
    /// the artists who define a genre.
    pub detail_shelves: Vec<Shelf>,
    /// A bar per decade on a genre page: how the genre spreads across the
    /// years you own, which is the thing a flat list of tracks cannot say.
    pub detail_bars: Vec<Tally>,
    /// Up to four covers for a playlist with no picture of its own. Empty for
    /// every other kind, and for a playlist the user has set an image on.
    pub detail_collage: Vec<String>,
    /// A playlist's description. Empty everywhere else.
    pub detail_note: String,
    /// When anything on the page was last played -- "3 days ago" -- or empty
    /// when nothing on it ever has been.
    pub detail_last_played: String,
    /// The track after the last one played from this album, or 0 when the
    /// album was finished, never started, or is on the deck now. The same
    /// resume point Home's shelf offers, for the one album that is open.
    pub detail_resume_item: i64,
    /// Every track in the library, so a smart playlist can say "48 of 3,912".
    /// Smart playlist pages only.
    pub detail_library_total: i64,
    /// A smart playlist's rule, one sentence per condition: "Rating is at
    /// least 4". Empty for every other page.
    pub detail_rules: Vec<String>,
    /// What the folder's files take on disk, in bytes. Folder pages only.
    pub detail_disk_bytes: i64,
    /// The watched folder this one sits under, and when it (or this folder on
    /// its own) was last scanned. Folder pages only; empty when unknown.
    pub detail_root: String,
    pub detail_scanned: String,

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
    /// Walk one folder page's folder rather than every root. Only a folder
    /// inside a watched root: this is a rescan, not a way to add one.
    RescanFolder { path: String },
    /// Open a folder page's folder in the desktop file manager. Same rule.
    RevealFolder { path: String },
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
    /// Keep one copy of a recording and delete the others. Each dropped copy's
    /// plays, loved state, rating and playlist places move to the kept copy
    /// first -- `play_history` and `playlist_items` cascade on the item row, so
    /// a plain delete would take the history with the worse rip. The files go
    /// to the recycle bin, as `DeleteTrack`'s do.
    DupeMerge { keep_id: i64, drop_ids: Vec<i64> },
    /// "Not duplicates": the finder stops showing this exact group.
    DupeDismiss { item_ids: Vec<i64> },
    /// Show one track's file in the desktop file manager, selected.
    RevealTrack { item_id: i64 },
    /// Set one tag field across many tracks at once.
    ///
    /// `field` is artist | album_artist | album | genre | year. Not title or
    /// track number: those are per-track by definition, and a bulk edit that
    /// gave fifty files the same title would be a way to lose a library rather
    /// than fix one. Retagging fifty mislabelled tracks was fifty separate
    /// trips through the manager.
    BulkTag { item_ids: Vec<i64>, field: String, value: String },

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
    /// Play one track starting at `secs`. What a lyric search result does: the
    /// point of finding the line is landing on it.
    LyricJump { item_id: i64, secs: f64 },
    /// Put an album back on from where it was abandoned.
    ResumeAlbum { album_id: i64 },
    /// Play a folder and everything beneath it -- a whole discography
    /// directory to the queue in one action, rather than one album at a time.
    PlayFolderTree { path: String },
    /// Open where an artist's biography came from, in the desktop browser.
    /// `source` is `musicbrainz` or `wikipedia`; the URL is built here rather
    /// than passed in, so nothing the UI holds can become an argument to the
    /// system opener.
    OpenArtistSource { artist_id: i64, source: String },
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
    /// Drop the tracks the station appended, keep everything queued by hand.
    StationStop,
    /// Fill the queue from the last few things played, without waiting for it
    /// to run dry.
    StationStart,
    QueueMove { from: i64, to: i64 },
    QueuePlayAt { index: i64 },
    PlaylistCreate { name: String },
    /// What a playlist is for, in the owner's own words.
    PlaylistDescribe { playlist_id: i64, text: String },
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
    /// Write the playlist, in its order, as an `#EXTM3U` file at `path`, which
    /// comes from a save dialog on the Dart side.
    PlaylistExport { playlist_id: i64, path: String },
    /// A copy with the same tracks, rule and description, named "… copy".
    PlaylistDuplicate { playlist_id: i64 },
    SetEqPreset { name: String },
    SetEqBand { index: i64, gain_db: f64 },
    ToggleEq,
    // --- casting -----------------------------------------------------------
    //
    // `tulipix_music::cast` has lived in the *music* crate since before the
    // port and been used only by the video streamer, because video casts
    // streams — things that already have a URL a speaker can fetch. Music is
    // local files, so the missing half was never the SOAP; it was an HTTP
    // origin. See `crate::cast_serve`.
    /// Look for renderers on the network. A few seconds of SSDP.
    CastDiscover,
    /// Send what is playing to one of them, by the name Discover listed.
    CastTo { device: String },
    /// Stop the renderer and take playback back.
    CastStop,

    /// Load an AutoEq `ParametricEQ.txt` and flatten it onto the ten bands.
    ///
    /// The catalogue AutoEq's device auto-detection wants is thousands of files
    /// and is not bundled, so the file is chosen rather than found. A ten-band
    /// graphic EQ cannot hold a twelve-filter correction exactly; this keeps
    /// the broad tilt, which is most of what a headphone correction is.
    ApplyAutoEq { path: String },
    /// Bring play counts and star ratings over from an iTunes / Apple Music
    /// `Library.xml`. Matched by absolute path, so nothing this library already
    /// knows — analysis especially — is overwritten by the import.
    ImportItunes { path: String },
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
    /// Renderers from the last discovery, the one being played to, and its
    /// AVTransport control endpoint — which is the handle Stop needs.
    cast_devices: Vec<tulipix_music::cast::CastDevice>,
    cast_target: String,
    cast_control: Option<String>,
    cast_busy: bool,
    sleep_min: i64,
    sleep_deadline: Option<std::time::Instant>,
    /// `SleepMode::EndOfTrack`, the arm the bridge never implemented. Read and
    /// cleared by the end-of-source hook, which is the only place that knows a
    /// track has finished rather than been skipped.
    sleep_end_of_track: bool,
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
            cast_devices: Vec::new(),
            cast_target: String::new(),
            cast_control: None,
            cast_busy: false,
            sleep_min: 0,
            sleep_deadline: None,
            sleep_end_of_track: false,
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
    /// What the album is filed under, which is not always the track's artist.
    /// Not on `Track` — it is one string per album and would ride on every row
    /// of every list for the sake of one dialog — so the tag editor reads it
    /// from here when it opens.
    pub album_artist: String,
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
    let extra: Option<(String, String, Option<f64>, String, Option<f64>, String)> = sqlx::query_as(
        "SELECT COALESCE(release_date, ''), COALESCE(credits, ''), bpm, \
                COALESCE(music_key, ''), dr_score, COALESCE(album_artist, '') \
         FROM track_meta WHERE item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let (release, credits, bpm, key, dr, album_artist) = extra.unwrap_or_default();

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
        album_artist,
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

/// True while the library analysis is walking. One pass at a time: two would
/// fight over the same rows and double the CPU for the same result.
static ANALYSE_RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Set by `music_analyse_stop`, cleared when the pass ends.
static ANALYSE_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// How many library tracks have never been analysed.
pub async fn music_analyse_pending() -> Result<i64> {
    let pool = music_pool().await?;
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM track_meta tm JOIN items i ON i.id = tm.item_id \
         WHERE i.section = 'music' AND i.missing_since IS NULL \
           AND COALESCE(tm.is_audiobook, 0) = 0 \
           AND (tm.bpm IS NULL OR tm.dr_score IS NULL OR NOT EXISTS ( \
                 SELECT 1 FROM track_embeddings te \
                 WHERE te.item_id = tm.item_id AND te.model = 'dsp-v1'))",
    )
    .fetch_one(pool)
    .await
    .unwrap_or(0))
}

/// Analyse every track that has not been measured yet, in the background.
///
/// Returns how many it is about to walk and gets out of the way -- a library
/// pass is minutes of ffmpeg, not something to hold a Dart future open for.
/// Progress rides the same `ScanProgress` / `ScanFinished` events the folder
/// scan uses, so the section's existing progress bar shows it without knowing
/// this exists.
pub async fn music_analyse_all() -> Result<i64> {
    use std::sync::atomic::Ordering;
    if ANALYSE_RUNNING.swap(true, Ordering::SeqCst) {
        return Ok(0); // already walking
    }
    ANALYSE_STOP.store(false, Ordering::SeqCst);

    let pool = music_pool().await?;
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT tm.item_id, i.abs_path FROM track_meta tm JOIN items i ON i.id = tm.item_id \
         WHERE i.section = 'music' AND i.missing_since IS NULL \
           AND COALESCE(tm.is_audiobook, 0) = 0 \
           AND (tm.bpm IS NULL OR tm.dr_score IS NULL OR NOT EXISTS ( \
                 SELECT 1 FROM track_embeddings te \
                 WHERE te.item_id = tm.item_id AND te.model = 'dsp-v1')) \
         ORDER BY tm.item_id",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let total = rows.len() as i64;
    if total == 0 {
        ANALYSE_RUNNING.store(false, Ordering::SeqCst);
        return Ok(0);
    }

    tokio::spawn(async move {
        let Ok(pool) = music_pool().await else {
            ANALYSE_RUNNING.store(false, Ordering::SeqCst);
            return;
        };
        let mut done = 0i64;
        for (item_id, path) in rows {
            if ANALYSE_STOP.load(Ordering::Relaxed) {
                break;
            }
            emit(MusicEvent::ScanProgress {
                root: format!("Analysing {}", short_name(&path)),
                done,
                total,
            });
            let src = std::path::PathBuf::from(&path);
            let measured =
                tokio::task::spawn_blocking(move || tulipix_music::analysis::analyse(&src)).await;
            match measured {
                Ok(Ok(a)) => {
                    if let Err(e) = tulipix_music::analysis::store(pool, item_id, &a).await {
                        tracing::debug!(item_id, error = %e, "analyse store");
                    }
                    deposit(&path, &a);
                }
                // A file ffmpeg cannot read is skipped, not retried: the next
                // pass would fail on it identically and never reach the rest.
                // It stays unanalysed, which is the honest state for it.
                Ok(Err(e)) => tracing::debug!(item_id, error = %e, "analyse"),
                Err(e) => tracing::warn!(item_id, error = %e, "analyse task"),
            }
            done += 1;
        }
        ANALYSE_RUNNING.store(false, Ordering::SeqCst);
        ANALYSE_STOP.store(false, Ordering::SeqCst);
        // Whatever the reason it stopped, the bar has to come down and the
        // section has to re-read: the rows it is showing now carry figures.
        emit(MusicEvent::ScanFinished { inserted: done, updated: 0, missing: total - done });
    });

    Ok(total)
}

/// Ask the running pass to stop after the track it is on.
pub async fn music_analyse_stop() -> Result<()> {
    ANALYSE_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
    Ok(())
}

/// The file's own name, for a progress line that has to fit on one row.
fn short_name(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Decode one track and measure its tempo, key and dynamic range.
///
/// The pass `tulipix_music::{bpm_key, dr_meter}` were written to receive: both
/// own the maths and the columns and say the decode "happens in the worker".
/// There was no worker. Results are written into the columns those modules own,
/// so the next snapshot carries them on `Track` with no second query.
///
/// Returns the tempo, or 0 when the track could not be measured -- a caller
/// wanting the whole result reads it off the refreshed `Track`.
pub async fn music_analyse(item_id: i64) -> Result<f64> {
    let pool = music_pool().await?;
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    let Some(path) = path else { return Ok(0.0) };
    let src = std::path::PathBuf::from(&path);
    let done = tokio::task::spawn_blocking(move || tulipix_music::analysis::analyse(&src)).await?;
    let a = match done {
        Ok(a) => a,
        Err(e) => {
            tracing::debug!(item_id, error = %e, "analyse");
            return Ok(0.0);
        }
    };
    tulipix_music::analysis::store(pool, item_id, &a).await?;
    deposit(&path, &a);
    Ok(a.bpm.unwrap_or(0.0))
}

/// File the two artefacts a decode produced alongside its numbers.
///
/// Seeding both caches here means neither the seekbar nor the visualiser ever
/// runs ffmpeg over a track the analysis pass has already seen.
fn deposit(path: &str, a: &tulipix_music::analysis::Analysis) {
    if !a.envelope.is_empty() {
        tulipix_music::waveform::put(Path::new(path), &a.envelope);
    }
    if !a.spectrogram.is_empty() {
        tulipix_music::waveform::spec_put(Path::new(path), &a.spectrogram);
    }
}

/// The loudness envelope for one track, as `BUCKETS` bytes of 0..255.
///
/// Lazy and cached exactly like `music_ensure_art`: the first call decodes,
/// every later one reads 400 bytes off disk. Empty when the track has no audio
/// ffmpeg can read, which the seekbar draws as a plain line rather than as an
/// error -- a waveform is an enhancement, not a precondition for scrubbing.
pub async fn music_waveform(item_id: i64) -> Result<Vec<u8>> {
    let pool = music_pool().await?;
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    let Some(path) = path else { return Ok(Vec::new()) };
    // ffmpeg plus a full read of its output: not something to do on the runtime
    // that is also answering the UI.
    let out = tokio::task::spawn_blocking(move || {
        tulipix_music::waveform::peaks(Path::new(&path))
    })
    .await?;
    Ok(out.unwrap_or_else(|e| {
        tracing::debug!(item_id, error = %e, "waveform");
        Vec::new()
    }))
}

/// How many bands a spectrogram column holds, so the UI can slice one.
#[frb(sync)]
pub fn music_spectrogram_bands() -> u32 {
    tulipix_music::analysis::SPEC_BANDS as u32
}

/// How many spectrogram columns cover one second of playback.
#[frb(sync)]
pub fn music_spectrogram_hz() -> u32 {
    tulipix_music::analysis::SPEC_HZ
}

/// The precomputed spectrum of one track: `music_spectrogram_bands()` levels
/// per column, `music_spectrogram_hz()` columns a second, row-major by column.
///
/// This is how a real spectrum reaches the visualiser at all. media_kit exposes
/// no PCM callback and mpv's filters return loudness but not bands, so there is
/// no live tap to read -- the analysis pass decodes each track once and the UI
/// indexes the result by playback position.
///
/// Unlike `music_waveform` this never decodes on demand: a spectrogram is two
/// orders of magnitude more work than an envelope, and doing it inside a UI call
/// would stall the section for seconds on a track the batch has not reached.
/// Empty until the pass has seen the track, which the visualiser draws as its
/// existing synthetic animation rather than as an error.
pub async fn music_spectrogram(item_id: i64) -> Result<Vec<u8>> {
    let pool = music_pool().await?;
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    let Some(path) = path else { return Ok(Vec::new()) };
    let bands = tulipix_music::analysis::SPEC_BANDS;
    Ok(
        tokio::task::spawn_blocking(move || {
            tulipix_music::waveform::spec_cached(Path::new(&path), bands)
        })
        .await?
        .unwrap_or_default(),
    )
}


// ---------------------------------------------------------------- listening --

/// Turn a breakdown into rows a list can draw directly.
fn tallies(rows: Vec<tulipix_music::dashboard::Tally>) -> Vec<Tally> {
    let top = rows.iter().map(|r| r.ms).max().unwrap_or(0).max(1) as f64;
    rows.into_iter()
        .map(|r| Tally {
            label: r.label,
            key: r.key,
            value: tulipix_music::dashboard::fmt_listen(r.ms),
            plays: r.plays,
            frac: (r.ms as f64 / top).clamp(0.0, 1.0),
        })
        .collect()
}

/// The listening summary for one window: `days` back from now, or 0 for all time.
///
/// Play history has been recorded since the first version of this app and until
/// now only ever fed a "recently played" shelf. Everything here is one pass over
/// that same table -- no new column, no new write path, and nothing leaves the
/// machine.
pub async fn music_listening(days: i64) -> Result<Listening> {
    const ROWS: i64 = 12;
    let pool = music_pool().await?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let since = (days > 0).then(|| now - days * 86_400);
    let range = match days {
        0 => "All time".to_string(),
        7 => "Last 7 days".to_string(),
        30 => "Last 30 days".to_string(),
        365 => "Last 12 months".to_string(),
        n => format!("Last {n} days"),
    };

    use tulipix_music::dashboard as d;
    let hours = d::by_hour(pool, since).await.unwrap_or([0; 24]);
    let peak = hours.iter().copied().max().unwrap_or(0);
    let top = peak.max(1) as f64;

    Ok(Listening {
        range,
        total: d::fmt_listen(hours.iter().sum()),
        artists: tallies(d::by_artist(pool, since, ROWS).await.unwrap_or_default()),
        genres: tallies(d::by_genre(pool, since, ROWS).await.unwrap_or_default()),
        tracks: tallies(d::by_track(pool, since, ROWS).await.unwrap_or_default()),
        peak_hour: match hours.iter().position(|h| *h == peak) {
            Some(h) if peak > 0 => format!("{h:02}:00 - {:02}:00", (h + 1) % 24),
            _ => String::new(),
        },
        hours: hours.iter().map(|h| *h as f64 / top).collect(),
        abandoned: tallies(d::abandoned(pool, ROWS).await.unwrap_or_default()),
    })
}

/// The Stats Center for one window: `days` back from now, or 0 for all time.
///
/// The listening summary plus the figures around it -- the change against the
/// window before, plays, finished, new artists, a 22-week calendar -- and what
/// the shelves hold. All of it is play history and `track_meta`; nothing new is
/// recorded to answer it.
pub async fn music_stats_center(days: i64) -> Result<StatsCenter> {
    use tulipix_music::dashboard as d;
    const CALENDAR_DAYS: i64 = 22 * 7;
    let pool = music_pool().await?;
    let listening = music_listening(days).await?;
    let f = d::figures(pool, days).await.unwrap_or_default();
    let streak = d::stats(pool).await.unwrap_or_default().streak_days;
    let cal = d::by_day(pool, CALENDAR_DAYS).await.unwrap_or_default();
    let busiest = cal.iter().copied().max().unwrap_or(0).max(1) as f64;
    let shelf = d::shelf(pool).await.unwrap_or_default();

    let whole = shelf.tracks.max(1) as f64;
    let share = |label: String, n: i64| Tally {
        label,
        key: 0,
        value: format!("{:.0}%", n as f64 / whole * 100.0),
        plays: n,
        frac: n as f64 / whole,
    };
    let mut formats: Vec<Tally> =
        shelf.formats.iter().take(4).map(|(c, n)| share(c.clone(), *n)).collect();
    let rest: i64 = shelf.formats.iter().skip(4).map(|(_, n)| n).sum();
    if rest > 0 {
        formats.push(share("Other".into(), rest));
    }

    let hours = (shelf.secs / 3600.0).round() as i64;
    let length = match (hours / 24, hours % 24) {
        (0, h) => format!("{h} h"),
        (1, h) => format!("1 day {h} h"),
        (n, h) => format!("{n} days {h} h"),
    };

    Ok(StatsCenter {
        delta: if f.prev_ms > 0 {
            let pct = ((f.ms - f.prev_ms) as f64 / f.prev_ms as f64 * 100.0).round() as i64;
            if pct >= 0 { format!("+{pct}%") } else { format!("\u{2212}{}%", -pct) }
        } else {
            String::new()
        },
        plays: f.plays,
        streak: match streak {
            0 => "—".into(),
            1 => "1 day".into(),
            n => format!("{n} days"),
        },
        new_artists: f.new_artists,
        finished: if f.judged > 0 {
            format!("{}%", (f.finished * 100 + f.judged / 2) / f.judged)
        } else {
            "—".into()
        },
        days: cal.iter().map(|v| *v as f64 / busiest).collect(),
        tracks: shelf.tracks,
        albums: shelf.albums,
        artists: shelf.artists,
        bytes: shelf.bytes,
        length,
        formats,
        pending: music_analyse_pending().await.unwrap_or(0),
        listening,
    })
}

// --------------------------------------------------------------- duplicates --

/// Every recording the library holds more than once, biggest group first.
///
/// Minutes of nothing on a large library that has never been analysed: the
/// comparison is the fingerprint the analysis pass stores, so an unanalysed
/// library correctly reports none rather than guessing from filenames.
pub async fn music_duplicates() -> Result<Vec<DupeGroup>> {
    let pool = music_pool().await?;
    let dismissed: std::collections::HashSet<String> =
        sqlx::query_scalar("SELECT key FROM dupe_dismissed")
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .collect();
    let groups: Vec<_> = tulipix_music::similar::duplicates(pool)
        .await?
        .into_iter()
        .filter(|g| !dismissed.contains(&dupe_key(g.iter().map(|d| d.item_id))))
        .collect();
    if groups.is_empty() {
        return Ok(Vec::new());
    }

    let ids: Vec<i64> = groups
        .iter()
        .flat_map(|g| g.iter().map(|d| d.item_id))
        .collect();
    let tracks: std::collections::HashMap<i64, Track> = tracks_by_ids(pool, &ids)
        .await
        .into_iter()
        .map(|t| (t.item_id, t))
        .collect();
    let quality = quality_labels(pool, &ids).await;
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT i.id, i.size, \
                (SELECT COUNT(DISTINCT pi.playlist_id) FROM playlist_items pi WHERE pi.item_id = i.id) \
         FROM items i WHERE i.id IN ({holes})"
    );
    let mut q = sqlx::query_as::<_, (i64, i64, i64)>(sqlx::AssertSqlSafe(&*sql));
    for id in &ids {
        q = q.bind(id);
    }
    let files: std::collections::HashMap<i64, (i64, i64)> = q
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, bytes, lists)| (id, (bytes, lists)))
        .collect();

    Ok(groups
        .into_iter()
        .map(|g| {
            let mut copies: Vec<DupeCopy> = g
                .into_iter()
                .filter_map(|d| {
                    let (bytes, playlists) = files.get(&d.item_id).copied().unwrap_or((0, 0));
                    Some(DupeCopy {
                        track: tracks.get(&d.item_id)?.clone(),
                        quality: quality.get(&d.item_id).cloned().unwrap_or_default(),
                        confidence: (d.confidence * 100.0).round() as i64,
                        keep: d.keep,
                        bytes,
                        playlists,
                    })
                })
                .collect();
            // Keeper first, then the closest match: the list reads as "this one,
            // and here is what it replaces".
            copies.sort_by(|a, b| b.keep.cmp(&a.keep).then(b.confidence.cmp(&a.confidence)));
            DupeGroup { copies }
        })
        // A group can lose members to a missing `items` row between the two
        // queries; one surviving copy is not a duplicate of anything.
        .filter(|g| g.copies.len() > 1)
        .collect())
}

/// A duplicate group's identity in `dupe_dismissed`: its item ids, sorted and
/// comma-joined.
fn dupe_key(ids: impl Iterator<Item = i64>) -> String {
    let mut v: Vec<i64> = ids.collect();
    v.sort_unstable();
    v.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

/// "FLAC - 44.1 kHz" / "MP3 - 320 kbps" for each of `ids`.
async fn quality_labels(
    pool: &sqlx::SqlitePool,
    ids: &[i64],
) -> std::collections::HashMap<i64, String> {
    if ids.is_empty() {
        return Default::default();
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT item_id, codec, bitrate, sample_rate FROM track_meta WHERE item_id IN ({holes})"
    );
    let mut q = sqlx::query_as::<_, (i64, Option<String>, Option<i64>, Option<i64>)>(sqlx::AssertSqlSafe(&*sql));
    for id in ids {
        q = q.bind(id);
    }
    q.fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, codec, bitrate, rate)| {
            let mut parts = Vec::new();
            let c = codec.unwrap_or_default();
            if !c.is_empty() {
                parts.push(c.to_uppercase());
            }
            match bitrate {
                Some(b) if b > 0 => parts.push(format!("{} kbps", b / 1000)),
                _ => {
                    if let Some(r) = rate.filter(|r| *r > 0) {
                        parts.push(format!("{:.1} kHz", r as f64 / 1000.0));
                    }
                }
            }
            (id, parts.join(" \u{b7} "))
        })
        .collect()
}

// ---------------------------------------------------------- similar artists --

/// Artists on these shelves who sound like this one, best first.
///
/// Local: the ranking is shared genres, overlapping era and the distance between
/// the two artists' fingerprint centroids. Every suggestion is therefore
/// something already in the library and playable right now, which is the whole
/// reason for not asking a recommendation API.
pub async fn music_similar_artists(artist_id: i64, limit: u32) -> Result<Vec<BrowseCard>> {
    let pool = music_pool().await?;
    let hits = tulipix_music::similar::similar_artists(pool, artist_id, limit as usize).await?;
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let art = artist_images(pool).await;

    let holes = vec!["?"; hits.len()].join(",");
    let sql = format!(
        "SELECT ar.id, ar.name, COUNT(tm.item_id) \
         FROM artists ar JOIN track_meta tm ON tm.artist_id = ar.id \
         JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
         WHERE ar.id IN ({holes}) GROUP BY ar.id"
    );
    let mut q = sqlx::query_as::<_, (i64, String, i64)>(sqlx::AssertSqlSafe(&*sql));
    for (id, _) in &hits {
        q = q.bind(id);
    }
    let names: std::collections::HashMap<i64, (String, i64)> = q
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name, n)| (id, (name, n)))
        .collect();

    Ok(hits
        .into_iter()
        .filter_map(|(id, _score)| {
            let (name, n) = names.get(&id)?.clone();
            Some(BrowseCard {
                id,
                key: id.to_string(),
                title: name,
                subtitle: if n == 1 {
                    "1 track".to_string()
                } else {
                    format!("{n} tracks")
                },
                count: n,
                art: art.get(&id).cloned().unwrap_or_default(),
                loved: false,
                stars: 0,
            })
        })
        .collect())
}


// ------------------------------------------------------------ lyric search --

/// Tracks whose words contain `query`, with the line and where it is sung.
///
/// The lyrics have been stored since the section shipped -- synced timestamps
/// and all -- and nothing has ever searched them. A half-remembered line is
/// often the only thing anyone remembers about a song.
pub async fn music_lyric_search(query: String) -> Result<Vec<LyricMatch>> {
    const HITS: i64 = 40;
    let pool = music_pool().await?;
    let hits = tulipix_music::lyrics::search(pool, &query, HITS).await?;
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let ids: Vec<i64> = hits.iter().map(|h| h.item_id).collect();
    let tracks: HashMap<i64, Track> = tracks_by_ids(pool, &ids)
        .await
        .into_iter()
        .map(|t| (t.item_id, t))
        .collect();
    Ok(hits
        .into_iter()
        .filter_map(|h| {
            let t = tracks.get(&h.item_id)?;
            let secs = if h.ms >= 0 { h.ms as f64 / 1000.0 } else { -1.0 };
            Some(LyricMatch {
                item_id: h.item_id,
                title: t.title.clone(),
                artist: t.artist.clone(),
                art: t.art.clone(),
                line: h.line,
                at: if secs >= 0.0 { fmt_clock(secs) } else { String::new() },
                secs,
            })
        })
        .collect())
}

// ----------------------------------------------------------- resume albums --

/// Albums left part-way through, most recently abandoned first.
pub async fn music_resume_albums() -> Result<Vec<ResumeCard>> {
    const RAIL: i64 = 8;
    let pool = music_pool().await?;
    let rows = tulipix_music::dashboard::resume_albums(pool, RAIL).await?;
    if rows.is_empty() {
        return Ok(Vec::new());
    }
    // Not the one on the deck: an album you are listening to right now is not
    // one you abandoned, and offering to resume it is nonsense.
    let playing_album: Option<i64> = match mpv::now_playing().item_id {
        0 => None,
        id => sqlx::query_scalar("SELECT album_id FROM track_meta WHERE item_id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten(),
    };

    let mut out = Vec::new();
    for r in rows {
        if Some(r.album_id) == playing_album {
            continue;
        }
        let row: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT al.title, ar.name, al.cover_path FROM albums al \
             LEFT JOIN artists ar ON ar.id = al.artist_id WHERE al.id = ?",
        )
        .bind(r.album_id)
        .fetch_optional(pool)
        .await?;
        let Some((title, artist, cover)) = row else { continue };
        out.push(ResumeCard {
            album_id: r.album_id,
            item_id: r.item_id,
            title,
            artist: artist.unwrap_or_default(),
            art: cover.filter(|c| Path::new(c).exists()).unwrap_or_default(),
            note: format!(
                "Left off at track {} of {} \u{b7} {}",
                r.done + 1,
                r.total,
                tulipix_music::dashboard::fmt_ago(r.played_at)
            ),
            progress: r.done as f64 / r.total.max(1) as f64,
        });
    }
    Ok(out)
}

// ------------------------------------------------------------ smart rules --

/// What a smart-playlist editor can build a rule out of.
#[frb(sync)]
pub fn music_smart_schema() -> SmartSchema {
    use tulipix_music::playlists::{Field, Op};
    SmartSchema {
        fields: Field::all()
            .iter()
            .map(|(f, id, label)| SmartField {
                id: (*id).to_string(),
                label: (*label).to_string(),
                kind: f.kind().to_string(),
            })
            .collect(),
        ops: Op::all()
            .iter()
            .map(|(_, id, label)| SmartOp {
                id: (*id).to_string(),
                label: (*label).to_string(),
            })
            .collect(),
    }
}

/// A view rule to the engine's own type. Conditions naming a field or operator
/// this build does not have are dropped rather than failing the whole rule --
/// an editor should still open a rule saved by a newer build, minus the parts
/// it cannot show.
fn to_rule(view: &SmartRuleView) -> tulipix_music::playlists::SmartRule {
    use tulipix_music::playlists::{Combine, Condition, Field, Op, SmartRule};
    SmartRule {
        combine: if view.combine == "any" { Combine::Any } else { Combine::All },
        conditions: view
            .conditions
            .iter()
            .filter_map(|c| {
                Some(Condition {
                    field: Field::from_id(&c.field)?,
                    op: Op::from_id(&c.op)?,
                    value: c.value.clone(),
                })
            })
            .collect(),
        limit: (view.limit > 0).then_some(view.limit),
    }
}

fn from_rule(rule: &tulipix_music::playlists::SmartRule) -> SmartRuleView {
    use tulipix_music::playlists::Combine;
    SmartRuleView {
        combine: match rule.combine {
            Combine::Any => "any".to_string(),
            Combine::All => "all".to_string(),
        },
        conditions: rule
            .conditions
            .iter()
            .map(|c| SmartCondition {
                field: c.field.id().to_string(),
                op: c.op.id().to_string(),
                value: c.value.clone(),
            })
            .collect(),
        limit: rule.limit.unwrap_or(0),
    }
}

/// A rule as a person would say it, one sentence per condition: "Rating is at
/// least 4", "Added in the last 365 days". The labels are the engine's own, so
/// the page and the editor name a field the same way.
fn rule_sentences(rule: &tulipix_music::playlists::SmartRule) -> Vec<String> {
    use tulipix_music::playlists::{Combine, Field, Op};
    let field = |f: &Field| {
        Field::all().iter().find(|(x, _, _)| x == f).map(|(_, _, l)| *l).unwrap_or("")
    };
    let op = |o: &Op| Op::all().iter().find(|(x, _, _)| x == o).map(|(_, _, l)| *l).unwrap_or("");
    let mut out: Vec<String> = rule
        .conditions
        .iter()
        .map(|c| {
            let v = c.value.trim();
            match (&c.field, &c.op) {
                (Field::Loved, o) => {
                    let yes = matches!(v, "1" | "true" | "yes");
                    if yes == matches!(o, Op::Ne) { "Not loved".into() } else { "Loved".into() }
                }
                // "Added (days ago) is less than 90" is the editor's wording
                // for a field that is a count of days; said plainly it is this.
                (Field::Added, Op::Lt | Op::Lte) => format!("Added in the last {v} days"),
                (Field::Added, Op::Gt | Op::Gte) => format!("Added more than {v} days ago"),
                (f, o) => format!("{} {} {v}", field(f), op(o)),
            }
        })
        .collect();
    if matches!(rule.combine, Combine::Any) && out.len() > 1 {
        out.insert(0, "Any one of these".into());
    }
    // `evaluate` orders newest-added first before it limits.
    if let Some(n) = rule.limit.filter(|n| *n > 0) {
        out.push(format!("The newest {n}"));
    }
    out
}

/// How many tracks a rule matches right now.
///
/// The live count under the editor. `evaluate` is the same call the playlist
/// itself makes, so the number shown is the number that will be in it -- not an
/// estimate from a different query.
pub async fn music_smart_preview(rule: SmartRuleView) -> Result<i64> {
    let pool = music_pool().await?;
    Ok(tulipix_music::playlists::evaluate(pool, &to_rule(&rule))
        .await?
        .len() as i64)
}

/// The rule behind a playlist, or an empty one to start from.
pub async fn music_smart_load(playlist_id: i64) -> Result<SmartRuleView> {
    let pool = music_pool().await?;
    Ok(
        match tulipix_music::playlists::rule_of(pool, playlist_id).await? {
            Some(r) => from_rule(&r),
            None => SmartRuleView {
                combine: "all".into(),
                conditions: Vec::new(),
                limit: 0,
            },
        },
    )
}

/// Create or rewrite a smart playlist. `playlist_id` 0 creates one.
///
/// Returns the id, so an editor that just created one can open it.
pub async fn music_smart_save(
    playlist_id: i64,
    name: String,
    rule: SmartRuleView,
) -> Result<i64> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("a playlist needs a name");
    }
    let pool = music_pool().await?;
    let rule = to_rule(&rule);
    let id = if playlist_id > 0 {
        tulipix_music::playlists::update_smart(pool, playlist_id, name, &rule).await?;
        playlist_id
    } else {
        if tulipix_music::playlists::find_by_name(pool, name).await?.is_some() {
            anyhow::bail!("\"{name}\" already exists");
        }
        tulipix_music::playlists::create(pool, name, Some(&rule)).await?
    };
    emit(MusicEvent::Stale);
    Ok(id)
}


/// Turn a row of album ids into tiles, keeping the order they were given.
async fn album_cards(pool: &sqlx::SqlitePool, ids: &[i64]) -> Vec<BrowseCard> {
    if ids.is_empty() {
        return Vec::new();
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT al.id, al.title, ar.name, COALESCE(al.year, 0), \
                (SELECT COUNT(*) FROM track_meta t \
                 JOIN items i2 ON i2.id = t.item_id AND i2.missing_since IS NULL \
                 WHERE t.album_id = al.id), \
                al.cover_path \
         FROM albums al LEFT JOIN artists ar ON ar.id = al.artist_id \
         WHERE al.id IN ({holes})"
    );
    let mut q =
        sqlx::query_as::<_, (i64, String, Option<String>, i64, i64, Option<String>)>(sqlx::AssertSqlSafe(&*sql));
    for id in ids {
        q = q.bind(id);
    }
    let mut by_id: HashMap<i64, BrowseCard> = q
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(id, title, artist, year, count, cover)| {
            (
                id,
                BrowseCard {
                    id,
                    key: id.to_string(),
                    title,
                    subtitle: [
                        (year > 0).then(|| year.to_string()),
                        artist.filter(|a| !a.is_empty()),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" \u{b7} "),
                    count,
                    art: cover
                        .filter(|c| !c.is_empty() && Path::new(c).exists())
                        .unwrap_or_default(),
                    loved: false,
                    stars: 0,
                },
            )
        })
        .collect();
    ids.iter().filter_map(|id| by_id.remove(id)).collect()
}

/// The album's own details, from the tags of the tracks on it.
///
/// Aggregated with `MAX` rather than read off one row. `MAX` skips NULLs, so a
/// catalogue number that only track one carries still reaches the panel — a
/// ripper writing release-level fields into the first track only is the common
/// case, not an error. Where every track agrees, `MAX` returns that shared
/// value; where they disagree it picks one, which is the right shape of answer
/// for a panel that is showing a property of the release.
async fn album_meta(pool: &sqlx::SqlitePool, album_id: i64) -> Vec<MetaRow> {
    let row: Option<(
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<i64>,
        Option<i64>,
        Option<i64>,
        Option<f64>,
        Option<i64>,
    )> = sqlx::query_as(
        "SELECT MAX(tm.label), MAX(tm.catalog_no), MAX(tm.release_date), MAX(tm.codec), \
                MAX(tm.bitrate), MAX(tm.sample_rate), MAX(tm.channels), \
                MAX(tm.replaygain_album), MIN(i.added) \
         FROM track_meta tm JOIN items i ON i.id = tm.item_id \
         WHERE tm.album_id = ? AND i.missing_since IS NULL",
    )
    .bind(album_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    let Some((label, catalog, date, codec, bitrate, rate, channels, gain, added)) = row else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut push = |label: &str, value: Option<String>| {
        if let Some(v) = value.filter(|v| !v.trim().is_empty()) {
            out.push(MetaRow { label: label.to_string(), value: v });
        }
    };
    push("Label", label);
    push("Catalogue", catalog);
    push("Released", date);

    // "FLAC 16/44" for lossless, "MP3 320 kbps" for lossy: the number that
    // says what you have is the depth for one and the rate for the other.
    let mut format = Vec::new();
    if let Some(c) = codec.filter(|c| !c.is_empty()) {
        format.push(c.to_uppercase());
    }
    if let Some(r) = rate.filter(|r| *r > 0) {
        format.push(format!("{:.1} kHz", r as f64 / 1000.0));
    }
    if let Some(b) = bitrate.filter(|b| *b > 0) {
        format.push(format!("{} kbps", b / 1000));
    }
    if let Some(ch) = channels.filter(|c| *c > 0) {
        format.push(match ch {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} ch"),
        });
    }
    push("Format", (!format.is_empty()).then(|| format.join(" \u{b7} ")));
    push(
        "Album gain",
        gain.filter(|g| *g != 0.0).map(|g| format!("{g:+.1} dB")),
    );
    push(
        "Added",
        added
            .filter(|a| *a > 0)
            .map(tulipix_music::dashboard::fmt_date),
    );
    out
}

/// One track's credits, fetched when a row is actually expanded.
///
/// Not on [`Track`]: four free-text fields on every row of every list would ride
/// the queue, the songs grid and the search results for the sake of the one row
/// somebody opened. `Track::has_credits` says whether there is anything here.
pub async fn music_track_credits(item_id: i64) -> Result<Vec<MetaRow>> {
    let pool = music_pool().await?;
    let row: Option<(Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as(
            "SELECT composer, performer, producer, remixer FROM track_meta WHERE item_id = ?",
        )
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    let Some((composer, performer, producer, remixer)) = row else {
        return Ok(Vec::new());
    };
    Ok([
        ("Written by", composer),
        ("Performed by", performer),
        ("Produced by", producer),
        ("Remixed by", remixer),
    ]
    .into_iter()
    .filter_map(|(label, value)| {
        let v = value?;
        (!v.trim().is_empty()).then(|| MetaRow {
            label: label.to_string(),
            value: v.trim().to_string(),
        })
    })
    .collect())
}


// ---------------------------------------------------------------- folders --

/// The folders directly under `under`, or the top of the tree when it is empty.
///
/// One level per call: a library on a slow disk should not pay for a tree
/// nobody expanded. The flat list this replaces was the one thing a folder view
/// must not be.
pub async fn music_folder_children(under: String) -> Result<Vec<FolderNode>> {
    let pool = music_pool().await?;
    Ok(tulipix_music::folders::children(pool, &under)
        .await?
        .into_iter()
        .map(|n| FolderNode {
            path: n.path,
            name: n.name,
            direct: n.direct,
            total: n.total,
            has_children: n.has_children,
        })
        .collect())
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
            // An album of this artist that already has a cached cover is the
            // cheapest answer, so it is tried first.
            let album: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM albums WHERE artist_id = ? AND cover_path IS NOT NULL LIMIT 1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            if let Some(a) = album {
                if let Some(found) =
                    Box::pin(music_ensure_art("album".into(), a.to_string())).await?
                {
                    remember_artist_art(pool, id, &found).await;
                    return Ok(Some(found));
                }
            }
            // Nothing cached yet. This is the case that left every artist
            // blank: on a library nobody has browsed, NO album has a
            // `cover_path` -- covers are extracted lazily -- so requiring one
            // above meant the chain ended here and the tile stayed empty for
            // good, because Dart remembers a miss for the session. Slint never
            // had the problem: its artist tile is the artist's first track's
            // embedded art, taken straight off the file. Do that.
            let track: Option<String> = sqlx::query_scalar(
                "SELECT i.abs_path FROM items i JOIN track_meta tm ON tm.item_id = i.id \
                 WHERE tm.artist_id = ? AND i.missing_since IS NULL \
                 ORDER BY COALESCE(tm.album_id, 0), COALESCE(tm.track_no, 0) LIMIT 1",
            )
            .bind(id)
            .fetch_optional(pool)
            .await?;
            let Some(track) = track else { return Ok(None) };
            let found = art_for_file(Path::new(&track));
            if let Some(p) = &found {
                remember_artist_art(pool, id, p).await;
            }
            Ok(found)
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

/// Remember a resolved artist picture in `artists.image_path`, so the next
/// paint of the grid is one SELECT rather than one ffmpeg per tile -- the same
/// bargain the album arm makes with `albums.cover_path`.
///
/// It is the column the user's own "replace art" writes, which is what makes
/// this safe to overwrite only when it is empty: a picture somebody chose must
/// not be replaced by one we extracted.
async fn remember_artist_art(pool: &sqlx::SqlitePool, artist_id: i64, path: &str) {
    let _ = sqlx::query(
        "UPDATE artists SET image_path = ? \
         WHERE id = ? AND (image_path IS NULL OR image_path = '')",
    )
    .bind(path)
    .bind(artist_id)
    .execute(pool)
    .await;
}

/// Artist id → stored picture, for the two card builders. One query beats one
/// `music_ensure_art` round trip per tile: the resolver still runs for artists
/// that have never been resolved, but it never runs twice for the same one.
async fn artist_images(
    pool: &sqlx::SqlitePool,
) -> std::collections::HashMap<i64, String> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, image_path FROM artists \
         WHERE image_path IS NOT NULL AND image_path <> ''",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter()
        .filter(|(_, p)| Path::new(p).exists())
        .collect()
}

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
    let key = art_key(&path.to_string_lossy());
    let dir = art_cache_dir();
    let dest = dir.join(format!("{key}.jpg"));
    if dest.exists() {
        return Some(dest.to_string_lossy().into_owned());
    }
    // Most music has no picture in the file, and asking ffmpeg about the same
    // artless track on every refresh cost a process each time and printed
    // "Output file does not contain any stream" each time. The empty marker
    // remembers the answer; deleting the art cache asks again.
    let none = dir.join(format!("{key}.none"));
    if none.exists() {
        return None;
    }
    // `-an` and the attached-pic map: without them ffmpeg happily writes the
    // whole audio stream into a .jpg and the tile shows a broken image.
    //
    // Both pipes to null: a track with no cover is the ordinary case, not an
    // error to report, and ffmpeg writes its complaint to the terminal the app
    // was started from.
    let status = std::process::Command::new(tulipix_core::thumbs::tool_bin("ffmpeg"))
        .args(["-v", "error", "-y", "-i"])
        .arg(path)
        .args(["-an", "-vcodec", "copy"])
        .arg(&dest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .no_window_compat()
        .status();
    match status {
        Ok(s) if s.success() && dest.exists() => Some(dest.to_string_lossy().into_owned()),
        _ => {
            // Leave nothing behind: a zero-byte file here would be returned
            // forever by the `dest.exists()` check above.
            let _ = std::fs::remove_file(&dest);
            let _ = std::fs::write(&none, b"");
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
        || {
            // "After this track" is decided here rather than in the queue,
            // because the queue has no idea which track is the last one. A
            // skip does not reach this hook, so pressing Next with the timer
            // armed keeps going — which is what "after *this* track" means.
            if end_of_track_sleep_due() {
                mpv::stop();
                emit(MusicEvent::TrackChanged);
            } else {
                emit(MusicEvent::Ended);
            }
        },
    );
    emit(MusicEvent::TrackChanged);
}

/// Read and clear the end-of-track sleep flag. One shot: the timer is spent
/// once it fires, the same way a deadline is.
fn end_of_track_sleep_due() -> bool {
    let mut s = lock();
    if !s.sleep_end_of_track {
        return false;
    }
    s.sleep_end_of_track = false;
    s.sleep_min = 0;
    true
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
    // The one place that knows how much of the outgoing track was actually
    // heard, which is exactly what decides whether it counts as a listen.
    crate::api::scrobble::on_play_finished(
        pool,
        prev,
        played_ms as f64 / 1000.0,
        mpv::observe().dur,
    )
    .await;
}

/// Play one library track and make it the now-playing.
async fn play_track(pool: &sqlx::SqlitePool, item_id: i64) -> Result<()> {
    play_track_at(pool, item_id, None).await
}

/// Play a track, optionally starting part-way in.
///
/// `start_s` reaches mpv as `--start`, which is a load-time option rather than
/// a seek: seeking straight after a load races the file actually being open,
/// and lands wherever it lands. A lyric hit at 1:14 has to be at 1:14.
async fn play_track_at(
    pool: &sqlx::SqlitePool,
    item_id: i64,
    start_s: Option<f64>,
) -> Result<()> {
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
    launch(&path, mpv::Slot::Music, start_s.filter(|t| *t > 1.0));
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

/// A roll in 0..1 for the shuffle draw. No `rand` dependency for one number a
/// track: the nanosecond field of the clock is as unpredictable as this needs
/// to be, and it is what the Slint build uses for the same purpose.
fn roll() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as f64 / 1_000_000_000.0)
        .unwrap_or(0.5)
}

/// The next track under shuffle, spaced rather than uniform.
///
/// Uniform random clusters, and the clustering is what makes shuffle feel
/// broken -- three songs by one artist in a row reads as a bug even though it
/// is exactly what random does. The weighting lives in the domain crate where
/// it is tested; this is the query that feeds it.
async fn shuffle_next(pool: &sqlx::SqlitePool, ids: &[i64], avoid: usize) -> usize {
    if ids.len() <= 1 {
        return 0;
    }
    let holes = vec!["?"; ids.len()].join(",");
    let sql = format!(
        "SELECT item_id, artist_id, album_id, COALESCE(loved, 0), last_played \
         FROM track_meta WHERE item_id IN ({holes})"
    );
    let mut q = sqlx::query_as::<_, (i64, Option<i64>, Option<i64>, i64, Option<i64>)>(sqlx::AssertSqlSafe(&*sql));
    for id in ids {
        q = q.bind(id);
    }
    let facts: HashMap<i64, tulipix_music::queue::Draw> = q
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(item_id, artist_id, album_id, loved, last_played)| {
            (
                item_id,
                tulipix_music::queue::Draw {
                    item_id,
                    artist_id,
                    album_id,
                    loved: loved != 0,
                    last_played,
                },
            )
        })
        .collect();
    // In queue order, and every id gets a row: a track with no `track_meta`
    // still has to be drawable or shuffle would skip it forever.
    let draws: Vec<tulipix_music::queue::Draw> = ids
        .iter()
        .map(|id| {
            facts.get(id).cloned().unwrap_or(tulipix_music::queue::Draw {
                item_id: *id,
                ..Default::default()
            })
        })
        .collect();

    // Newest first, which is the order the spacing rule reads.
    let recent: Vec<i64> = {
        let s = lock();
        s.history.iter().rev().copied().collect()
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    tulipix_music::queue::weighted_pick(&draws, &recent, now, roll()).unwrap_or(avoid)
}

/// How many seeds a station is built from. The last few tracks rather than the
/// last one, so a run does not swing on whichever thing happened to be playing
/// when the queue ended.
const STATION_SEEDS: usize = 3;
/// How many tracks a station appends at a time. Enough to keep playing for
/// twenty minutes; short enough that it is re-seeded from what you actually let
/// play rather than committing to a whole evening up front.
const STATION_RUN: usize = 6;

/// Keep going when the queue runs out, if the user asked for that.
///
/// Off by default and read from settings on every call: this appends tracks
/// nobody chose, so it has to be something switched on deliberately, and a
/// setting changed mid-session has to take effect at the next track and not at
/// the next restart.
///
/// Returns whether anything was actually added. Everything it queues is marked
/// `station` in `play_queue.source`, so the UI can show what it chose and why,
/// and a "stop this" is a delete by source.
async fn autoplay_extend(pool: &sqlx::SqlitePool, queued: &[i64]) -> bool {
    let on = tulipix_core::settings::Settings::load()
        .map(|s| s.flag("music.autoplay", false))
        .unwrap_or(false);
    if !on {
        return false;
    }
    let seeds: Vec<i64> = {
        let s = lock();
        s.history
            .iter()
            .rev()
            .take(STATION_SEEDS)
            .copied()
            .collect()
    };
    if seeds.is_empty() {
        return false;
    }
    let picks = tulipix_music::similar::station(pool, &seeds, queued, STATION_RUN)
        .await
        .unwrap_or_default();
    if picks.is_empty() {
        return false;
    }
    for id in &picks {
        if let Err(e) = tulipix_music::queue::enqueue(pool, *id, "station").await {
            tracing::debug!(item_id = id, error = %e, "station enqueue");
            return false;
        }
    }
    lock().status = format!("Kept going with {} similar tracks.", picks.len());
    true
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

    let mut ids = queue_ids(pool).await;
    if ids.is_empty() {
        if forward && autoplay_extend(pool, &ids).await {
            ids = queue_ids(pool).await;
        }
        if ids.is_empty() {
            mpv::stop();
            return Ok(());
        }
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
        shuffle_next(pool, &ids, at).await
    } else if forward {
        if at + 1 >= ids.len() {
            if repeat == "all" {
                0
            } else if autoplay_extend(pool, &ids).await {
                // The station appended, so there is a next track after all and
                // it is the first thing it added.
                let grown = queue_ids(pool).await;
                let Some(next) = grown.get(at + 1).copied() else {
                    mpv::stop();
                    return Ok(());
                };
                return play_track(pool, next).await;
            } else {
                mpv::stop();
                return Ok(());
            }
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
        MusicCmd::RescanFolder { path } => {
            let dir = watched_dir(&path)?;
            let pool = music_pool().await?;
            let (inserted, updated, missing) = scan_folder(pool, &dir).await?;
            finish_scan(pool, inserted, updated, missing).await?;
        }
        MusicCmd::RevealFolder { path } => {
            tulipix_platform::fm::open_default(&watched_dir(&path)?)?;
        }
        MusicCmd::PlaylistExport { playlist_id, path } => {
            let pool = music_pool().await?;
            let mut paths = Vec::new();
            for id in tulipix_music::playlists::items(pool, playlist_id).await? {
                // A track deleted since it was added is a gap in the file, not
                // a failed export.
                if let Ok(p) = track_path(pool, id).await {
                    paths.push(PathBuf::from(p));
                }
            }
            let mut out = PathBuf::from(&path);
            if out.extension().is_none() {
                out.set_extension("m3u");
            }
            std::fs::write(&out, tulipix_music::playlists::write_m3u(&paths))?;
            lock().status = format!("Saved {} tracks to {}.", paths.len(), out.display());
        }
        MusicCmd::PlaylistDuplicate { playlist_id } => {
            let pool = music_pool().await?;
            let Some(name) = sqlx::query_scalar::<_, String>("SELECT name FROM playlists WHERE id = ?")
                .bind(playlist_id)
                .fetch_optional(pool)
                .await?
            else {
                anyhow::bail!("that playlist is not there any more");
            };
            // Names are matched exactly, so a second copy needs a name of its own.
            let mut copy = format!("{name} copy");
            let mut n = 2;
            while tulipix_music::playlists::find_by_name(pool, &copy).await?.is_some() {
                copy = format!("{name} copy {n}");
                n += 1;
            }
            let rule = tulipix_music::playlists::rule_of(pool, playlist_id).await?;
            let id = tulipix_music::playlists::create(pool, &copy, rule.as_ref()).await?;
            for item in tulipix_music::playlists::items(pool, playlist_id).await? {
                tulipix_music::playlists::append(pool, id, item).await?;
            }
            let note = sqlx::query_scalar::<_, Option<String>>(
                "SELECT description FROM playlists WHERE id = ?",
            )
            .bind(playlist_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten()
            .unwrap_or_default();
            if !note.trim().is_empty() {
                sqlx::query("UPDATE playlists SET description = ? WHERE id = ?")
                    .bind(note)
                    .bind(id)
                    .execute(pool)
                    .await?;
            }
            lock().status = format!("Made \u{201c}{copy}\u{201d}.");
        }
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
        MusicCmd::BulkTag {
            item_ids,
            field,
            value,
        } => {
            let pool = music_pool().await?;
            if item_ids.is_empty() {
                return Ok(());
            }
            let value = value.trim().to_string();
            let mut done = 0usize;
            for id in &item_ids {
                // Read what is there, change the one field, write the lot back.
                // Going through the existing save keeps one path that writes a
                // file's tags, so the ffmpeg stream-copy and its temp-file
                // safety are not reimplemented here.
                let row: Option<(String, String, String, String, String, i64, i64, i64)> =
                    sqlx::query_as(
                        "SELECT COALESCE(tm.title, ''), COALESCE(ar.name, ''), \
                                COALESCE(al.title, ''), COALESCE(tm.album_artist, ''), \
                                COALESCE(tm.genre, ''), COALESCE(tm.year, 0), \
                                COALESCE(tm.track_no, 0), COALESCE(tm.disc_no, 0) \
                         FROM track_meta tm \
                         LEFT JOIN artists ar ON ar.id = tm.artist_id \
                         LEFT JOIN albums al ON al.id = tm.album_id \
                         WHERE tm.item_id = ?",
                    )
                    .bind(id)
                    .fetch_optional(pool)
                    .await?;
                let Some((title, artist, album, album_artist, genre, year, track_no, disc_no)) =
                    row
                else {
                    continue;
                };
                let mut next = (artist, album, album_artist, genre, year.to_string());
                match field.as_str() {
                    "artist" => next.0 = value.clone(),
                    "album" => next.1 = value.clone(),
                    "album_artist" => next.2 = value.clone(),
                    "genre" => next.3 = value.clone(),
                    "year" => next.4 = value.clone(),
                    other => anyhow::bail!("cannot bulk-edit {other}"),
                }
                Box::pin(apply(MusicCmd::SaveTags {
                    item_id: *id,
                    title,
                    artist: next.0,
                    album: next.1,
                    album_artist: next.2,
                    genre: next.3,
                    date: next.4,
                    track_no,
                    disc_no,
                }))
                .await?;
                done += 1;
            }
            lock().status = format!(
                "Set {field} on {done} track{}.",
                if done == 1 { "" } else { "s" }
            );
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
        MusicCmd::DupeMerge { keep_id, drop_ids } => {
            let pool = music_pool().await?;
            let mut gone = 0;
            for drop in drop_ids.into_iter().filter(|d| *d != keep_id) {
                let path: Option<String> =
                    sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                        .bind(drop)
                        .fetch_optional(pool)
                        .await?;
                let Some(p) = path else { continue };
                // History before the file: both tables cascade on the item
                // row, so whatever is not moved first is deleted with it.
                let mut tx = pool.begin().await?;
                sqlx::query("UPDATE play_history SET item_id = ? WHERE item_id = ?")
                    .bind(keep_id)
                    .bind(drop)
                    .execute(&mut *tx)
                    .await?;
                // A playlist that held both copies now holds the kept one twice,
                // which is what it played before: its length and order stay.
                sqlx::query("UPDATE playlist_items SET item_id = ? WHERE item_id = ?")
                    .bind(keep_id)
                    .bind(drop)
                    .execute(&mut *tx)
                    .await?;
                sqlx::query(
                    "UPDATE track_meta SET \
                       play_count  = COALESCE(play_count, 0) \
                                   + COALESCE((SELECT play_count FROM track_meta WHERE item_id = ?), 0), \
                       loved       = MAX(loved, COALESCE((SELECT loved FROM track_meta WHERE item_id = ?), 0)), \
                       rating      = MAX(rating, COALESCE((SELECT rating FROM track_meta WHERE item_id = ?), 0)), \
                       last_played = MAX(COALESCE(last_played, 0), \
                                         COALESCE((SELECT last_played FROM track_meta WHERE item_id = ?), 0)) \
                     WHERE item_id = ?",
                )
                .bind(drop)
                .bind(drop)
                .bind(drop)
                .bind(drop)
                .bind(keep_id)
                .execute(&mut *tx)
                .await?;
                tx.commit().await?;
                if let Err(e) = tulipix_platform::fm::move_to_trash(Path::new(&p)) {
                    anyhow::bail!("could not delete {p}: {e}");
                }
                tulipix_core::populator::remove(pool, drop).await?;
                gone += 1;
            }
            lock().status = format!(
                "Deleted {gone} cop{}; the kept one has their plays and playlist places.",
                if gone == 1 { "y" } else { "ies" }
            );
        }
        MusicCmd::DupeDismiss { item_ids } => {
            let pool = music_pool().await?;
            sqlx::query("INSERT OR REPLACE INTO dupe_dismissed (key, at) VALUES (?, ?)")
                .bind(dupe_key(item_ids.into_iter()))
                .bind(now_secs())
                .execute(pool)
                .await?;
        }
        MusicCmd::RevealTrack { item_id } => {
            let pool = music_pool().await?;
            let path = track_path(pool, item_id).await?;
            tulipix_platform::fm::reveal_in_file_manager(Path::new(&path))?;
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
            // The analysis pass that fills this index now lives in this
            // workspace and runs from Settings, but it still has to have been
            // run: with nothing stored this says so rather than quietly
            // queueing nothing. `station` rather than a raw nearest-neighbour
            // search because the nearest 25 tracks to anything are usually the
            // rest of its own album, which is not a discovery.
            let queued = queue_ids(pool).await;
            let ids = tulipix_music::similar::station(pool, &[item_id], &queued, 25)
                .await
                .unwrap_or_default();
            if ids.is_empty() {
                anyhow::bail!("nothing to match yet -- run the library analysis in Settings");
            }
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
        MusicCmd::PlayFolderTree { path } => {
            let pool = music_pool().await?;
            let ids = tulipix_music::folders::tracks_under(pool, &path).await?;
            if ids.is_empty() {
                anyhow::bail!("nothing playable under that folder");
            }
            tulipix_music::queue::replace(pool, &ids, "folder").await?;
            play_track(pool, ids[0]).await?;
            lock().status = format!("Playing {} tracks.", ids.len());
        }
        MusicCmd::OpenArtistSource { artist_id, source } => {
            let pool = music_pool().await?;
            let row: Option<(String, Option<String>)> =
                sqlx::query_as("SELECT name, mbid FROM artists WHERE id = ?")
                    .bind(artist_id)
                    .fetch_optional(pool)
                    .await?;
            let Some((name, mbid)) = row else {
                anyhow::bail!("that artist is not in the library");
            };
            let url = match source.as_str() {
                "musicbrainz" => match mbid.filter(|m| !m.trim().is_empty()) {
                    // Percent-encoding is not needed: an MBID is a UUID and
                    // anything else is refused rather than pasted into a URL.
                    Some(m) if m.chars().all(|c| c.is_ascii_hexdigit() || c == '-') => {
                        format!("https://musicbrainz.org/artist/{m}")
                    }
                    _ => anyhow::bail!("no MusicBrainz id for {name}"),
                },
                "wikipedia" => format!(
                    "https://en.wikipedia.org/w/index.php?search={}",
                    tulipix_music::musicbrainz::urlencode(&name)
                ),
                other => anyhow::bail!("unknown source: {other}"),
            };
            tulipix_platform::fm::open_default(Path::new(&url))?;
        }
        MusicCmd::LyricJump { item_id, secs } => {
            let pool = music_pool().await?;
            // The track alone, not its album: this is a jump to one line, and
            // replacing the queue with a record nobody asked for would be a
            // surprise on top of a surprise.
            play_track_at(pool, item_id, (secs > 0.0).then_some(secs)).await?;
        }
        MusicCmd::ResumeAlbum { album_id } => {
            let pool = music_pool().await?;
            // Recomputed here rather than trusted from the card: the rail may
            // have been on screen for an hour, and another track of the album
            // may have played since.
            let Some(point) = tulipix_music::dashboard::resume_albums(pool, 64)
                .await?
                .into_iter()
                .find(|r| r.album_id == album_id)
            else {
                anyhow::bail!("that album has no unplayed tracks left");
            };
            let ids: Vec<(i64,)> = sqlx::query_as(
                "SELECT tm.item_id FROM track_meta tm \
                 JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
                 WHERE tm.album_id = ? AND COALESCE(tm.is_audiobook, 0) = 0 \
                 ORDER BY COALESCE(tm.disc_no, 0), COALESCE(tm.track_no, 0), tm.item_id",
            )
            .bind(album_id)
            .fetch_all(pool)
            .await?;
            let ids: Vec<i64> = ids.into_iter().map(|(id,)| id).collect();
            let Some(at) = ids.iter().position(|id| *id == point.item_id) else {
                anyhow::bail!("that album has no unplayed tracks left");
            };
            // The whole album goes in the queue, not just the rest of it: Prev
            // from the resume point should reach the tracks already heard.
            tulipix_music::queue::replace(pool, &ids, "album").await?;
            play_track(pool, ids[at]).await?;
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
            // Three modes, not two. The menu has offered "After this track"
            // as -1 since the port, and `minutes.max(0)` quietly turned it
            // into Off — so the one arm of `sleep_timer::SleepMode` the bridge
            // never implemented was also the one the UI already promised.
            let had_deadline = {
                let mut s = lock();
                let had = s.sleep_deadline.is_some();
                s.sleep_min = minutes;
                s.sleep_end_of_track = minutes < 0;
                s.sleep_deadline = if minutes > 0 {
                    Some(
                        std::time::Instant::now()
                            + std::time::Duration::from_secs(minutes as u64 * 60),
                    )
                } else {
                    None
                };
                had
            };
            // `check_sleep` ramps mpv's volume down over the final thirty
            // seconds and nothing else ever puts it back. Cancelling or
            // pushing out a timer that was already fading therefore used to
            // leave playback quiet, with the volume slider still showing the
            // number it no longer had. Undo the ramp whenever the deadline
            // moves; harmless when the fade had not started.
            if had_deadline {
                restore_volume();
            }
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
        MusicCmd::StationStop => {
            let pool = music_pool().await?;
            let gone = tulipix_music::queue::clear_source(pool, "station").await?;
            lock().status = if gone > 0 {
                format!("Removed {gone} suggested tracks.")
            } else {
                "Nothing was suggested.".to_string()
            };
        }
        MusicCmd::StationStart => {
            let pool = music_pool().await?;
            // Seeded from what is playing when there is no history yet -- asking
            // for a station before the first track finishes is the normal way to
            // reach for this, and refusing on an empty history would be obtuse.
            let mut seeds: Vec<i64> = {
                let s = lock();
                s.history.iter().rev().take(STATION_SEEDS).copied().collect()
            };
            let cur = mpv::now_playing().item_id;
            if cur != 0 && !seeds.contains(&cur) {
                seeds.insert(0, cur);
            }
            if seeds.is_empty() {
                anyhow::bail!("play something first -- a station is built from what you were listening to");
            }
            let queued = queue_ids(pool).await;
            let picks =
                tulipix_music::similar::station(pool, &seeds, &queued, STATION_RUN * 2).await?;
            if picks.is_empty() {
                anyhow::bail!("nothing to match yet -- run the library analysis in Settings");
            }
            for id in &picks {
                tulipix_music::queue::enqueue(pool, *id, "station").await?;
            }
            lock().status = format!("Queued {} suggested tracks.", picks.len());
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
        MusicCmd::PlaylistDescribe { playlist_id, text } => {
            let pool = music_pool().await?;
            let text = text.trim();
            sqlx::query("UPDATE playlists SET description = ?, updated = ? WHERE id = ?")
                .bind((!text.is_empty()).then_some(text))
                .bind(
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                )
                .bind(playlist_id)
                .execute(pool)
                .await?;
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
        MusicCmd::CastDiscover => cast_discover().await,
        MusicCmd::CastTo { device } => cast_to(device).await?,
        MusicCmd::CastStop => cast_stop().await,

        MusicCmd::ApplyAutoEq { path } => {
            use tulipix_music::eq::BANDS_HZ;
            use tulipix_music::headphone_eq as aeq;

            let text = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("could not read {path}: {e}"))?;
            let parsed = aeq::parse_autoeq(&text);
            if parsed.filters.is_empty() {
                anyhow::bail!(
                    "no filters in that file — AutoEq's ParametricEQ.txt is the \
                     one to pick, not the graphic-EQ or convolution export"
                );
            }
            let bands = aeq::to_bands(&parsed, &BANDS_HZ);

            let mut eq = load_eq();
            for (slot, gain) in eq.gains_db.iter_mut().zip(bands.iter()) {
                *slot = *gain;
            }
            eq.enabled = true;
            // `clamp` is doing real work here rather than guarding a typo: a
            // correction with an 8 dB boost and a -7 dB preamp lands inside
            // ±12, but an aggressive one will not, and a band pinned at the
            // limit is the honest version of "this is as far as ten bands go".
            eq.clamp();
            save_eq(&eq);
            set_setting("music.eq.preset", "custom");
            apply_eq_live(&eq);

            let name = Path::new(&path)
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("that curve")
                .to_string();
            let n = parsed.filters.len();
            lock().status = format!(
                "Applied {name} — {n} filters onto 10 bands, preamp {:+.1} dB",
                parsed.preamp_db
            );
        }
        MusicCmd::ImportItunes { path } => {
            let pool = music_pool().await?;
            let xml = std::fs::read_to_string(&path)
                .map_err(|e| anyhow::anyhow!("could not read {path}: {e}"))?;
            let tracks = tulipix_music::import::parse_itunes(&xml);
            if tracks.is_empty() {
                anyhow::bail!(
                    "no tracks in that file — iTunes calls it Library.xml, and \
                     File → Library → Export Library is what writes it"
                );
            }
            let found = tracks.len();
            let merged = tulipix_music::import::merge(pool, &tracks).await?;
            // Say both numbers. A library that moved between machines matches
            // on very little, and "42 of 3,100" is the sentence that explains
            // why rather than looking like a failure.
            lock().status = format!(
                "Imported play counts and ratings for {merged} of {found} tracks \
                 — the rest are not in this library under the same path"
            );
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
        sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*sql))
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
    let rows = sqlx::query_as::<_, TrackRow>(sqlx::AssertSqlSafe(&*sql))
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
        let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(&*sql))
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
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("SELECT {column} FROM track_meta WHERE item_id = ?")))
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

// ------------------------------------------------------------------- cast ----

/// "Living Room (Sonos One)" out of whatever the device called itself. The
/// same trimming `vid_stream` does, for the same reason: a renderer's
/// advertised name often carries a model string nobody reads.
fn short_device(name: &str) -> String {
    crate::cast_serve::short_name(name)
}

/// Look for renderers. SSDP is a multicast question with a few seconds of
/// answers, so it is a command rather than something the snapshot does.
async fn cast_discover() {
    lock().cast_busy = true;
    let found = tokio::task::spawn_blocking(crate::cast_serve::discover)
        .await
        .unwrap_or_default();
    let n = found.len();
    let mut s = lock();
    s.cast_devices = found;
    s.cast_busy = false;
    s.status = match n {
        0 => "No speakers found. They have to be on the same network, and some \
              need waking before they answer."
            .to_string(),
        1 => "1 speaker found.".to_string(),
        n => format!("{n} speakers found."),
    };
}

/// Hand the current track to `device`.
///
/// The renderer pulls the audio itself, so the file is published on a local
/// HTTP origin first — see [`crate::cast_serve`]. Local playback stops: the
/// alternative is hearing the same track twice, a second or two apart, which
/// is worse than either.
async fn cast_to(device: String) -> Result<()> {
    use tulipix_music::cast as dlna;

    // What is on the deck, by the one record that knows: every player in the
    // section lands in `mpv::set_now_playing`, so this is right for a track
    // started from Songs, from an album page or from the queue alike.
    let item_id = mpv::now_playing().item_id;
    if item_id == 0 {
        anyhow::bail!("play something first — casting sends what is on the deck");
    }
    let pool = music_pool().await?;
    let path = track_path(pool, item_id).await.unwrap_or_default();
    if path.is_empty() {
        anyhow::bail!("that track has no file to send");
    }
    let dev = {
        let s = lock();
        s.cast_devices
            .iter()
            .find(|d| short_device(&d.name) == device)
            .cloned()
    };
    let Some(dev) = dev else {
        anyhow::bail!("{device} is no longer listed — look again");
    };

    lock().cast_busy = true;
    let client = tulipix_core::net::http().clone();

    // The device description says where its AVTransport control endpoint is.
    // A renderer without one can show pictures or nothing at all, and either
    // way it is not going to play this.
    let desc = match client.get(&dev.location).send().await {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(e) => {
            lock().cast_busy = false;
            anyhow::bail!("{device} did not answer: {e}");
        }
    };
    let Some(ctl) = dlna::parse_control_url(&desc) else {
        lock().cast_busy = false;
        anyhow::bail!("{device} cannot play audio (it has no AVTransport)");
    };
    let ctl = dlna::resolve_url(&dev.location, &ctl);

    let url = match crate::cast_serve::publish(Path::new(&path)).await {
        Ok(u) => u,
        Err(e) => {
            lock().cast_busy = false;
            return Err(e);
        }
    };

    for (action, body) in [
        ("SetAVTransportURI", dlna::soap_set_uri(0, &url)),
        ("Play", dlna::soap_play(0)),
    ] {
        let ok = client
            .post(&ctl)
            .header("SOAPACTION", dlna::soap_action_header(action))
            .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
            .body(body)
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);
        if !ok {
            crate::cast_serve::unpublish();
            lock().cast_busy = false;
            anyhow::bail!("{device} refused the track");
        }
    }

    // Only now: a failed handoff should leave the music where it was rather
    // than silent on both machines.
    mpv::stop();
    let mut s = lock();
    s.cast_control = Some(ctl);
    s.cast_target = device.clone();
    s.cast_busy = false;
    s.status = format!("Playing on {device}. The queue stays here — press Next to send the next track.");
    Ok(())
}

/// Stop the renderer and stop publishing.
async fn cast_stop() {
    use tulipix_music::cast as dlna;
    let ctl = {
        let mut s = lock();
        s.cast_target.clear();
        s.cast_busy = false;
        s.cast_control.take()
    };
    crate::cast_serve::unpublish();
    let Some(ctl) = ctl else { return };
    let _ = tulipix_core::net::http()
        .post(&ctl)
        .header("SOAPACTION", dlna::soap_action_header("Stop"))
        .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
        .body(dlna::soap_stop(0))
        .send()
        .await;
    lock().status = "Stopped casting.".to_string();
}

/// Put mpv's volume back where the settings say it belongs. The only thing
/// that ever moves it out from under the user is the sleep fade, so this is
/// the undo for that.
fn restore_volume() {
    let base = setting("music.volume", "80");
    mpv::set_property("volume", &base);
}

/// Stop (or fade) when the sleep timer is up. Called from every snapshot.
fn check_sleep() {
    let deadline = lock().sleep_deadline;
    let Some(deadline) = deadline else { return };
    let now = std::time::Instant::now();
    if now >= deadline {
        mpv::stop();
        {
            let mut s = lock();
            s.sleep_deadline = None;
            s.sleep_min = 0;
            s.sleep_end_of_track = false;
        }
        // The next track must not start at whatever the ramp reached.
        restore_volume();
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

/// Where a folder page's "last scanned" is kept: one setting per folder
/// walked, so a rescan of the folder alone and a rescan of its root both count.
fn scan_key(path: &str) -> String {
    format!("music.scanned.{}", path.trim_end_matches('/'))
}

/// A folder the page named, if it is still a folder and sits inside a watched
/// root. The path comes from the UI, and a string the UI holds does not become
/// an argument to the scanner or the system opener without passing this.
fn watched_dir(path: &str) -> Result<PathBuf> {
    let dir = PathBuf::from(path);
    if !dir.is_dir() {
        anyhow::bail!("not a folder any more: {path}");
    }
    if !load_watched_folders().iter().any(|root| dir.starts_with(root)) {
        anyhow::bail!("{path} is not inside a watched folder");
    }
    Ok(dir)
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
        match scan_folder(pool, dir).await {
            Ok((ins, upd, miss)) => {
                inserted += ins;
                updated += upd;
                missing += miss;
            }
            Err(e) => tracing::warn!(error = %e, root = %dir.display(), "music scan failed"),
        }
    }
    finish_scan(pool, inserted, updated, missing).await
}

/// Walk one folder -- a watched root, or any folder inside one -- and seed
/// `track_meta` for what it holds. Returns inserted, updated, missing.
///
/// A folder inside a root is safe to hand the populator on its own: it only
/// marks missing what sits under the path it was given.
async fn scan_folder(pool: &sqlx::SqlitePool, dir: &Path) -> Result<(i64, i64, i64)> {
    let lib = tulipix_core::libraries::Library {
        id: dir.to_string_lossy().into_owned(),
        path: dir.to_path_buf(),
        section: tulipix_core::libraries::Section::Music,
        last_scan: None,
        item_count: 0,
        size_bytes: 0,
        exclude_globs: Vec::new(),
        cadence_override: Some(tulipix_core::libraries::ScanCadence::Manual),
        realtime_notify: false,
    };
    let stats = tulipix_core::populator::populate(pool, &lib).await?;

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
    // What a folder page's "last scanned" reads.
    set_setting(&scan_key(&dir.to_string_lossy()), &now_secs().to_string());
    Ok((stats.inserted as i64, stats.updated as i64, stats.missing as i64))
}

/// The end of any scan, whole-library or one folder.
async fn finish_scan(
    pool: &sqlx::SqlitePool,
    inserted: i64,
    updated: i64,
    missing: i64,
) -> Result<()> {
    // Before the finish event, because every My Music view the UI rebuilds on
    // it filters on `is_audiobook` and the rows to flag only exist now.
    apply_folder_sections(pool).await;
    if let Err(e) = rebuild_fresh_playlist(pool).await {
        tracing::warn!(error = %e, "recently added playlist");
    }
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

/// ffprobe every music item the current reader has not seen, and upsert what
/// comes back.
///
/// "Not seen" is no title yet *or* a `tags_version` below the reader's own --
/// see [`tulipix_music::scan::TAGS_VERSION`]. Chunked and progress-reported: a
/// first scan of a large library is minutes of process spawns, and a UI with no
/// sign of life during it looks hung.
async fn read_missing_tags() -> Result<()> {
    let pool = music_pool().await?;
    // Never read, or read by an older version of the reader.
    //
    // The second half is what makes a new tag field reach a library that has
    // already been scanned. Without it this only ever touched rows that were
    // never tagged at all, so a column added today would stay empty forever on
    // every existing install and the Rescan button would walk the whole disk
    // and change nothing.
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT i.id, i.abs_path FROM items i \
         LEFT JOIN track_meta tm ON tm.item_id = i.id \
         WHERE i.section = 'music' AND i.missing_since IS NULL \
           AND (tm.item_id IS NULL OR tm.title IS NULL OR tm.title = '' \
                OR COALESCE(tm.tags_version, 0) < ?)",
    )
    .bind(tulipix_music::scan::TAGS_VERSION)
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
    // Credits and release details. Already in the files of anyone who tags
    // properly, and never read until now. Each key has more than one spelling
    // in the wild -- Vorbis comments, ID3 frames and MP4 atoms all landed on
    // different names for the same thing -- so each is tried in turn.
    t.composer = get("composer").and_then(|s| tags::clean(&s));
    t.performer = get("performer")
        .or_else(|| get("albumartist_credit"))
        .and_then(|s| tags::clean(&s));
    t.producer = get("producer").and_then(|s| tags::clean(&s));
    t.remixer = get("remixer")
        .or_else(|| get("mixartist"))
        .and_then(|s| tags::clean(&s));
    t.label = get("label")
        .or_else(|| get("publisher"))
        .or_else(|| get("organization"))
        .and_then(|s| tags::clean(&s));
    t.catalog_no = get("catalognumber")
        .or_else(|| get("catalog_number"))
        .or_else(|| get("catalogid"))
        .and_then(|s| tags::clean(&s));
    // The date verbatim, where `year` above keeps only the first four digits.
    t.release_date = get("originaldate")
        .or_else(|| get("date"))
        .and_then(|s| tags::clean(&s))
        .filter(|s| s.len() > 4);
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

/// The newest hundred tracks in the library, as a playlist that is always
/// there. Checked whenever the Playlists tab is drawn, after every scan, and
/// when opened; written only when the newest hundred have actually changed.
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
    let ids: Vec<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT i.id FROM items i JOIN track_meta tm ON tm.item_id = i.id         {MUSIC_WHERE} ORDER BY i.added DESC LIMIT ?"
    )))
    .bind(KEEP)
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    // The common case by far: nothing was added since the last look.
    if tulipix_music::playlists::items(pool, id).await? == ids {
        return Ok(id);
    }
    sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
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
/// One line of a lyric being hand-timed. `at_ms` is -1 until the line has been
/// stamped — `Option<i64>` crosses the bridge as a nullable box on the Dart
/// side, and a sentinel reads better in a list this long.
#[derive(Debug, Clone)]
pub struct StampedLine {
    pub at_ms: i64,
    pub text: String,
}

/// Write hand-stamped lyrics to the library as LRC.
///
/// The tap-along editor's other half. Dart owns the tapping — it has the
/// playhead — and `tulipix_music::lyrics_sync` owns the two things worth
/// getting right: the constant-offset shift, and the `[mm:ss.xx]` that has to
/// be exactly what every other reader expects.
///
/// `shift_ms` nudges every stamped line before writing. It is the same
/// correction `LyricsOffset` makes at playback time, except this one is
/// written into the words rather than remembered beside them, so it holds in
/// any other player too.
///
/// Returns how many lines carry a timestamp. Zero of them is still worth
/// storing — plain words are better than none — but it stores as unsynced.
pub async fn music_save_lrc(
    item_id: i64,
    lines: Vec<StampedLine>,
    shift_ms: i64,
) -> Result<i64> {
    use tulipix_music::lyrics_sync as sync;

    let mut timed: Vec<sync::TimedLine> = lines
        .into_iter()
        .map(|l| sync::TimedLine {
            ms: (l.at_ms >= 0).then_some(l.at_ms),
            text: l.text,
        })
        .collect();
    if timed.iter().all(|l| l.text.trim().is_empty()) {
        anyhow::bail!("nothing to save");
    }
    if shift_ms != 0 {
        sync::shift_all(&mut timed, shift_ms);
    }
    let stamped = timed.iter().filter(|l| l.ms.is_some()).count() as i64;
    let lrc = sync::to_lrc(&timed);

    let pool = music_pool().await?;
    tulipix_music::lyrics::store(pool, item_id, &lrc, stamped > 0, "manual").await?;
    Ok(stamped)
}

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
// Every column is aliased, because `TrackRow` is a derived `FromRow` and that
// matches by NAME, not position. It used to be a tuple, which sqlx only
// implements `FromRow` for up to sixteen elements -- the seventeenth field is
// what turned this into a wall of trait errors.
const TRACK_SELECT: &str = "SELECT i.id AS id, i.abs_path AS path, \
     COALESCE(tm.title, '') AS title, \
     COALESCE(ar.name, '') AS artist, COALESCE(al.title, '') AS album, \
     COALESCE(tm.duration_s, 0.0) AS dur, \
     COALESCE(tm.loved, 0) AS loved, COALESCE(tm.rating, 0) AS stars, \
     COALESCE(tm.play_count, 0) AS plays, \
     COALESCE(tm.track_no, 0) AS track_no, COALESCE(tm.disc_no, 0) AS disc_no, \
     COALESCE(tm.bpm, 0.0) AS bpm, COALESCE(tm.music_key, '') AS music_key, \
     COALESCE(tm.dr_score, 0.0) AS dr_score, \
     COALESCE(tm.year, 0) AS year, COALESCE(tm.genre, '') AS genre, \
     COALESCE(al.cover_path, '') AS cover, \
     CASE WHEN ly.item_id IS NULL THEN '' WHEN ly.synced = 1 THEN 'synced' ELSE 'plain' END AS lyr, \
     CASE WHEN COALESCE(tm.composer, '') <> '' OR COALESCE(tm.performer, '') <> '' \
               OR COALESCE(tm.producer, '') <> '' OR COALESCE(tm.remixer, '') <> '' \
          THEN 1 ELSE 0 END AS credited \
     FROM items i JOIN track_meta tm ON tm.item_id = i.id \
     LEFT JOIN artists ar ON ar.id = tm.artist_id \
     LEFT JOIN albums al ON al.id = tm.album_id \
     LEFT JOIN lyrics ly ON ly.item_id = i.id";

/// Music, present, and not a book chapter — the filter every My Music list
/// starts from. Audiobooks have their own tab and their own `is_audiobook`
/// flag; a chapter appearing in the songs list is the bug this prevents.
const MUSIC_WHERE: &str =
    " WHERE i.section = 'music' AND i.missing_since IS NULL AND COALESCE(tm.is_audiobook, 0) = 0";

#[derive(sqlx::FromRow)]
#[frb(ignore)]
struct TrackRow {
    id: i64,
    path: String,
    title: String,
    artist: String,
    album: String,
    dur: f64,
    loved: i64,
    stars: i64,
    plays: i64,
    track_no: i64,
    disc_no: i64,
    bpm: f64,
    music_key: String,
    dr_score: f64,
    year: i64,
    genre: String,
    cover: String,
    lyr: String,
    /// Whether this track has any credit tag at all. A flag rather than the
    /// credits themselves: they are four free-text fields nobody reads until a
    /// row is expanded, and carrying them on every track would put them in the
    /// queue, the songs grid and the search results too.
    credited: i64,
}

fn into_track(row: TrackRow) -> Track {
    let TrackRow {
        id,
        path,
        title,
        artist,
        album,
        dur,
        loved,
        stars,
        plays,
        track_no,
        disc_no,
        bpm,
        music_key,
        dr_score,
        year,
        genre,
        cover,
        lyr,
        credited,
    } = row;
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
        track_no,
        disc_no,
        bpm,
        music_key,
        dr_score,
        year,
        genre,
        lyrics: lyr,
        has_credits: credited != 0,
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
    let mut q = sqlx::query_as::<_, TrackRow>(sqlx::AssertSqlSafe(&*sql));
    for id in ids {
        q = q.bind(id);
    }
    let rows = q.fetch_all(pool).await.unwrap_or_default();
    let mut by_id: std::collections::HashMap<i64, Track> =
        rows.into_iter().map(|r| (r.id, into_track(r))).collect();
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
    let mut cq = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    if filtered {
        cq = cq.bind(&like).bind(&like).bind(&like);
    }
    let total = cq.fetch_one(pool).await.unwrap_or(0);

    let sql = format!(
        "{TRACK_SELECT}{MUSIC_WHERE}{filter}{} LIMIT ? OFFSET ?",
        song_order(&s.song_sort, &s.song_dir)
    );
    let mut q = sqlx::query_as::<_, TrackRow>(sqlx::AssertSqlSafe(&*sql));
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
        let images = artist_images(pool).await;
        tulipix_music::browse::artists(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .filter_map(|(id, name, count)| {
                let (loved, stars) = marks.get(&id).copied().unwrap_or((false, 0));
                loved.then(|| BrowseCard {
                    art: images.get(&id).cloned().unwrap_or_default(),
                    id,
                    key: id.to_string(),
                    title: name,
                    subtitle: format!("{count} tracks"),
                    count,
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
    // Always there and always current, and first: it is the one playlist
    // nobody made.
    let fresh = rebuild_fresh_playlist(pool).await.unwrap_or(0);
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT p.id, p.name, p.is_smart, \
                (SELECT COUNT(*) FROM playlist_items pi WHERE pi.playlist_id = p.id) \
         FROM playlists p ORDER BY p.id = ? DESC, p.name COLLATE NOCASE",
    )
    .bind(fresh)
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
    sqlx::query_as::<_, (i64, i64, i64)>(sqlx::AssertSqlSafe(format!(
        "SELECT id, COALESCE(loved, 0), COALESCE(rating, 0) FROM {table} \
         WHERE COALESCE(loved, 0) <> 0 OR COALESCE(rating, 0) <> 0"
    )))
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
            // Whatever has already been resolved ships in the snapshot, the way
            // an album's `cover_path` does. An artist nobody has looked at yet
            // still comes back empty and `music_ensure_art` fills it in on the
            // first paint -- and, now, remembers it.
            let images = artist_images(pool).await;
            tulipix_music::browse::artists(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|(id, name, count)| {
                    let (loved, stars) = marks.get(&id).copied().unwrap_or((false, 0));
                    BrowseCard {
                        art: images.get(&id).cloned().unwrap_or_default(),
                        id,
                        key: id.to_string(),
                        title: name,
                        subtitle: format!("{count} tracks"),
                        count,
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
            // Each folder wears its most-played track's cover: the album's
            // when it is on disk, and otherwise `id` carries the track so the
            // tile can ask `music_ensure_art` for the picture inside the file.
            // Ties go to the newest file, so a folder nobody has played yet
            // shows what arrived last rather than whatever sorts first.
            let tops: HashMap<String, (i64, String)> =
                sqlx::query_as::<_, (String, i64, Option<String>)>(
                    "SELECT tm.folder, tm.item_id, al.cover_path FROM track_meta tm \
                     JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
                     LEFT JOIN albums al ON al.id = tm.album_id \
                     WHERE tm.folder IS NOT NULL AND COALESCE(tm.is_audiobook, 0) = 0 \
                     ORDER BY tm.folder, COALESCE(tm.play_count, 0) DESC, i.added DESC",
                )
                .fetch_all(pool)
                .await
                .unwrap_or_default()
                .into_iter()
                .fold(HashMap::new(), |mut m, (folder, id, cover)| {
                    m.entry(folder).or_insert_with(|| {
                        (id, cover.filter(|c| Path::new(c).exists()).unwrap_or_default())
                    });
                    m
                });
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
                .map(|(folder, count)| {
                    let (top, art) = tops.get(&folder).cloned().unwrap_or_default();
                    BrowseCard {
                        id: top,
                        title: Path::new(&folder)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| folder.clone()),
                        subtitle: folder.clone(),
                        key: folder,
                        count,
                        art,
                        loved: false,
                        stars: 0,
                    }
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

/// How many tiles a browse grid pages by. Folders and genres are three rows,
/// not four: four rows of those tiles do not fit above the fold.
fn browse_page_size(kind: &str) -> i64 {
    match kind {
        "folders" => FOLDER_PAGE,
        "genres" => GENRE_PAGE,
        _ => BROWSE_PAGE,
    }
}

/// An artist's biography, from `artists.bio` -- the lead of their English
/// Wikipedia article, found through MusicBrainz and Wikidata once and written
/// back, so the page is instant every time after the first.
///
/// The column is added the same way `loved` and `rating` are: with a bare
/// `ALTER TABLE` whose "already there" error is the expected case. There is no
/// migration for it, and the Slint build opens the same file.
///
/// A failed lookup writes nothing. Caching the miss would mean an artist who
/// was offline once stays blank forever, and the retry costs one request the
/// next time someone opens the page.
async fn artist_bio(
    pool: &sqlx::SqlitePool,
    artist_id: i64,
    name: &str,
) -> (String, Vec<String>, String) {
    // Same bare-ALTER pattern as `bio` itself: the "already there" error is the
    // expected case, and the Slint build opens the same file.
    let _ = sqlx::query("ALTER TABLE artists ADD COLUMN bio TEXT")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE artists ADD COLUMN facts TEXT")
        .execute(pool)
        .await;
    // Where the prose came from. Rows written before this held MusicBrainz's
    // one-line description in `bio` -- a name, a country, a year: facts, not a
    // biography -- and those are fetched again rather than shown.
    let _ = sqlx::query("ALTER TABLE artists ADD COLUMN bio_src TEXT")
        .execute(pool)
        .await;
    let cached: Option<(Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT bio, facts, mbid, bio_src FROM artists WHERE id = ?")
            .bind(artist_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let (bio, facts, mbid, src) = cached.unwrap_or_default();
    let bio = bio.unwrap_or_default();
    let mbid = mbid.unwrap_or_default();
    let has_facts = facts.as_deref().is_some_and(|f| !f.is_empty());
    let facts_now = split_facts(facts);
    if !bio.trim().is_empty() && src.as_deref() == Some("wikipedia") {
        return (bio, facts_now, mbid);
    }
    if name.trim().is_empty() || !bio_first_try(artist_id) {
        return (String::new(), facts_now, mbid);
    }
    // Off the snapshot's thread. This runs on every refresh while the page is
    // open -- a track change, a tick that finished a song -- and three requests
    // out and back would freeze the page for as long as they took. It lands in
    // the columns and `Stale` brings the page back for it.
    let name = name.to_string();
    let known = mbid.clone();
    tokio::spawn(async move {
        let Ok(pool) = music_pool().await else { return };
        let Ok(client) = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(12))
            .build()
        else {
            return;
        };
        let mut mbid = known;
        let mut facts = String::new();
        if mbid.is_empty() || !has_facts {
            if let Ok(search) = tulipix_music::musicbrainz::lookup_artist(&client, &name).await {
                // Best hit only, and only if it is actually about this artist --
                // MusicBrainz answers every query with something, and a
                // low-scoring hit is a different band with a similar name.
                if let Some(hit) = search
                    .artists
                    .iter()
                    .max_by_key(|a| a.score)
                    .filter(|a| a.score >= 80)
                {
                    facts = tulipix_music::musicbrainz::artist_facts(hit).join(FACT_SEP);
                    if mbid.is_empty() {
                        mbid = hit.id.clone();
                    }
                }
            }
            // MusicBrainz allows one request a second, and the next one is
            // theirs too.
            tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
        }
        let bio = tulipix_music::musicbrainz::wikipedia_bio(&client, &mbid)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        // No prose clears an old one-liner rather than leaving it in the
        // column, and the source is only marked when there is prose -- so a
        // miss, offline or not, is tried again next run. The mbid is only
        // written when the row has none: a tagger may have set a better one,
        // and this is a search hit.
        let _ = sqlx::query(
            "UPDATE artists SET bio = NULLIF(?, ''), \
             bio_src = CASE WHEN ? = '' THEN NULL ELSE 'wikipedia' END, \
             facts = COALESCE(NULLIF(?, ''), facts), \
             mbid = COALESCE(NULLIF(mbid, ''), NULLIF(?, '')) WHERE id = ?",
        )
        .bind(&bio)
        .bind(&bio)
        .bind(&facts)
        .bind(&mbid)
        .bind(artist_id)
        .execute(pool)
        .await;
        if !bio.is_empty() || !facts.is_empty() {
            emit(MusicEvent::Stale);
        }
    });
    (String::new(), facts_now, mbid)
}

/// Facts are stored as one string because there is no list column and this is
/// not data anything queries -- it is three words shown beside a paragraph.
const FACT_SEP: &str = "\u{1f}";

fn split_facts(stored: Option<String>) -> Vec<String> {
    stored
        .unwrap_or_default()
        .split(FACT_SEP)
        .map(str::trim)
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect()
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
    let mut q = sqlx::query_as::<_, TrackRow>(sqlx::AssertSqlSafe(&*sql));
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
    let queue_suggested: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM play_queue WHERE source = 'station'")
            .fetch_one(pool)
            .await
            .unwrap_or(0);

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
        queue_suggested,
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
        viz_style: setting("music.viz.style", "5").parse().unwrap_or(5).clamp(0, 5),
        viz_on: setting("music.viz.on", "1") != "0",
        cast_devices: s.cast_devices.iter().map(|d| short_device(&d.name)).collect(),
        cast_target: s.cast_target.clone(),
        cast_active: s.cast_control.is_some(),
        cast_busy: s.cast_busy,

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
        detail_facts: Vec::new(),
        detail_mbid: String::new(),
        detail_is_smart: false,
        detail_meta: Vec::new(),
        detail_shelves: Vec::new(),
        detail_bars: Vec::new(),
        detail_collage: Vec::new(),
        detail_note: String::new(),
        detail_last_played: String::new(),
        detail_resume_item: 0,
        detail_library_total: 0,
        detail_rules: Vec::new(),
        detail_disk_bytes: 0,
        detail_root: String::new(),
        detail_scanned: String::new(),

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
            let row: (i64, i64) = sqlx::query_as(sqlx::AssertSqlSafe(format!(
                "SELECT COALESCE(loved, 0), COALESCE(rating, 0) FROM {table} WHERE id = ?"
            )))
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
        if s.detail_kind == "playlist" {
            st.detail_is_smart = sqlx::query_scalar::<_, i64>(
                "SELECT is_smart FROM playlists WHERE id = ?",
            )
            .bind(s.detail_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0)
                != 0;
        }
        // The record's own details, and the shelves under it.
        if s.detail_kind == "album" {
            st.detail_meta = album_meta(pool, s.detail_id).await;

            // Other pressings of this record, by the fingerprint of feature 15.
            // A deluxe edition, a remaster and a second rip end up as three
            // albums in a library tagged by three different people, and nothing
            // connected them.
            let editions = tulipix_music::similar::other_editions(pool, s.detail_id)
                .await
                .unwrap_or_default();
            if !editions.is_empty() {
                st.detail_shelves.push(Shelf {
                    kind: "album".into(),
                    title: if editions.len() == 1 {
                        "Another edition in your library".to_string()
                    } else {
                        format!("{} other editions in your library", editions.len())
                    },
                    cards: album_cards(pool, &editions).await,
                });
            }

            // The rest of the artist's work. The album's own artist, not the
            // track artists: a compilation should not shelve everyone on it.
            let more: Vec<(i64,)> = sqlx::query_as(
                "SELECT other.id FROM albums other \
                 JOIN albums this ON this.id = ? AND this.artist_id IS NOT NULL \
                 WHERE other.artist_id = this.artist_id AND other.id <> this.id \
                 ORDER BY COALESCE(other.year, 0) DESC LIMIT 12",
            )
            .bind(s.detail_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            let more: Vec<i64> = more.into_iter().map(|(id,)| id).collect();
            if !more.is_empty() {
                st.detail_shelves.push(Shelf {
                    kind: "album".into(),
                    title: "More by this artist".to_string(),
                    cards: album_cards(pool, &more).await,
                });
            }
        }
        // A genre page was the thinnest of the five: a flat list, no cover, no
        // sense of what is in it. These two say what the genre is in *this*
        // library -- who defines it here, and which decades it lives in.
        if s.detail_kind == "genre" {
            let artists: Vec<(i64, String, i64, String)> = sqlx::query_as(
                "SELECT ar.id, ar.name, COUNT(*), COALESCE(ar.image_path, '') \
                 FROM track_meta tm \
                 JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
                 JOIN artists ar ON ar.id = tm.artist_id \
                 WHERE LOWER(TRIM(tm.genre)) = LOWER(TRIM(?)) \
                   AND COALESCE(tm.is_audiobook, 0) = 0 \
                 GROUP BY ar.id ORDER BY COUNT(*) DESC LIMIT 10",
            )
            .bind(&s.detail_key)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            if !artists.is_empty() {
                st.detail_shelves.push(Shelf {
                    kind: "artist".into(),
                    title: "Who defines it here".to_string(),
                    cards: artists
                        .into_iter()
                        .map(|(id, name, count, art)| BrowseCard {
                            id,
                            key: id.to_string(),
                            title: name,
                            subtitle: if count == 1 {
                                "1 track".to_string()
                            } else {
                                format!("{count} tracks")
                            },
                            count,
                            art: if !art.is_empty() && Path::new(&art).exists() {
                                art
                            } else {
                                String::new()
                            },
                            loved: false,
                            stars: 0,
                        })
                        .collect(),
                });
            }

            let years: Vec<(i64, i64)> = sqlx::query_as(
                "SELECT (tm.year / 10) * 10, COUNT(*) FROM track_meta tm \
                 JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
                 WHERE LOWER(TRIM(tm.genre)) = LOWER(TRIM(?)) \
                   AND COALESCE(tm.is_audiobook, 0) = 0 \
                   AND tm.year > 1000 \
                 GROUP BY 1 ORDER BY 1",
            )
            .bind(&s.detail_key)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            // Two decades is a range; one is just the year everything came out,
            // and a chart of a single bar says nothing a label could not.
            if years.len() > 1 {
                let top = years.iter().map(|(_, n)| *n).max().unwrap_or(1).max(1) as f64;
                st.detail_bars = years
                    .into_iter()
                    .map(|(decade, n)| Tally {
                        label: format!("{}s", decade % 100 / 10 * 10),
                        key: decade,
                        value: n.to_string(),
                        plays: n,
                        frac: n as f64 / top,
                    })
                    .collect();
            }
        }
        // A playlist is the one kind with no identity of its own: no picture,
        // nothing to say what it is for.
        if s.detail_kind == "playlist" {
            st.detail_note = sqlx::query_scalar::<_, Option<String>>(
                "SELECT description FROM playlists WHERE id = ?",
            )
            .bind(s.detail_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten()
            .unwrap_or_default();

            // Only when the user has not chosen a picture: a collage is a
            // stand-in, not something to paint over a deliberate choice.
            if pref_cover(&settings_map(), &format!("music.playlist.cover.{}", s.detail_id))
                .is_empty()
            {
                let covers: Vec<(String,)> = sqlx::query_as(
                    "SELECT DISTINCT al.cover_path FROM playlist_items pi \
                     JOIN track_meta tm ON tm.item_id = pi.item_id \
                     JOIN albums al ON al.id = tm.album_id \
                     WHERE pi.playlist_id = ? AND COALESCE(al.cover_path, '') <> '' \
                     ORDER BY pi.position LIMIT 8",
                )
                .bind(s.detail_id)
                .fetch_all(pool)
                .await
                .unwrap_or_default();
                st.detail_collage = covers
                    .into_iter()
                    .map(|(c,)| c)
                    .filter(|c| Path::new(c).exists())
                    .take(4)
                    .collect();
                // Four or nothing. Two covers in a four-up grid is a broken
                // tile, not a collage.
                if st.detail_collage.len() < 4 {
                    st.detail_collage.clear();
                }
            }
        }
        // Records where this artist is a guest rather than the name on the
        // spine. Today those tracks are filed silently under whoever the
        // compilation belongs to, and the artist page never mentions them.
        if s.detail_kind == "artist" {
            let guest: Vec<(i64,)> = sqlx::query_as(
                "SELECT DISTINCT tm.album_id FROM track_meta tm \
                 JOIN items i ON i.id = tm.item_id AND i.missing_since IS NULL \
                 JOIN albums al ON al.id = tm.album_id \
                 WHERE tm.artist_id = ? \
                   AND COALESCE(tm.is_audiobook, 0) = 0 \
                   AND COALESCE(al.artist_id, -1) <> tm.artist_id \
                 ORDER BY COALESCE(al.year, 0) DESC LIMIT 12",
            )
            .bind(s.detail_id)
            .fetch_all(pool)
            .await
            .unwrap_or_default();
            let guest: Vec<i64> = guest.into_iter().map(|(id,)| id).collect();
            if !guest.is_empty() {
                st.detail_shelves.push(Shelf {
                    kind: "album".into(),
                    title: "Appears on".to_string(),
                    cards: album_cards(pool, &guest).await,
                });
            }
        }
        if s.detail_kind == "artist" {
            let (bio, facts, mbid) = artist_bio(pool, s.detail_id, &st.detail_title).await;
            st.detail_info = bio;
            st.detail_facts = facts;
            st.detail_mbid = mbid;
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
        // What the hero's side card says about the page as a whole. The ids go
        // in as one JSON array rather than a bind per track: a genre can be
        // thousands of rows.
        let ids = serde_json::to_string(&tracks.iter().map(|t| t.item_id).collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".into());
        st.detail_last_played = sqlx::query_scalar::<_, Option<i64>>(
            "SELECT MAX(played_at) FROM play_history \
             WHERE item_id IN (SELECT value FROM json_each(?))",
        )
        .bind(&ids)
        .fetch_one(pool)
        .await
        .ok()
        .flatten()
        .map(tulipix_music::dashboard::fmt_ago)
        .unwrap_or_default();
        // The next track after the last one heard. Not while any of the album
        // is on the deck: that is listening, not having stopped.
        if s.detail_kind == "album" {
            let playing = mpv::now_playing().item_id;
            let last: Option<i64> = sqlx::query_scalar(
                "SELECT ph.item_id FROM play_history ph \
                 JOIN track_meta tm ON tm.item_id = ph.item_id \
                 WHERE tm.album_id = ? ORDER BY ph.played_at DESC LIMIT 1",
            )
            .bind(s.detail_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
            if !tracks.iter().any(|t| t.item_id == playing) {
                st.detail_resume_item = last
                    .and_then(|id| tracks.iter().position(|t| t.item_id == id))
                    .and_then(|at| tracks.get(at + 1))
                    .map(|t| t.item_id)
                    .unwrap_or(0);
            }
        }
        if s.detail_kind == "playlist" && st.detail_is_smart {
            st.detail_library_total = sqlx::query_scalar(
                "SELECT COUNT(*) FROM track_meta tm JOIN items i ON i.id = tm.item_id \
                 WHERE i.section = 'music' AND i.missing_since IS NULL \
                   AND COALESCE(tm.is_audiobook, 0) = 0",
            )
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            if let Ok(Some(rule)) = tulipix_music::playlists::rule_of(pool, s.detail_id).await {
                st.detail_rules = rule_sentences(&rule);
            }
        }
        if s.detail_kind == "folder" {
            st.detail_disk_bytes = sqlx::query_scalar::<_, Option<i64>>(
                "SELECT SUM(size) FROM items WHERE id IN (SELECT value FROM json_each(?))",
            )
            .bind(&ids)
            .fetch_one(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(0);
            // The deepest root it sits under, when roots nest.
            let here = Path::new(&s.detail_key);
            if let Some(root) = load_watched_folders()
                .into_iter()
                .filter(|r| here.starts_with(r))
                .max_by_key(|r| r.as_os_str().len())
            {
                let root = root.to_string_lossy().into_owned();
                let stamp = |p: &str| setting(&scan_key(p), "").parse::<i64>().ok();
                st.detail_scanned = stamp(&s.detail_key)
                    .max(stamp(&root))
                    .map(tulipix_music::dashboard::fmt_ago)
                    .unwrap_or_default();
                st.detail_root = root;
            }
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
            // Twenty to a page, in the Songs tab's order: one "how do songs
            // sort" setting rather than a second one for the same tracks. In
            // SQL, because a sort applied to one page of twenty sorts nothing.
            let loved = format!("{MUSIC_WHERE} AND COALESCE(tm.loved, 0) <> 0");
            st.song_total = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(format!(
                "SELECT COUNT(*) FROM items i JOIN track_meta tm ON tm.item_id = i.id{loved}"
            )))
            .fetch_one(pool)
            .await
            .unwrap_or(0);
            st.song_pages = page_count(st.song_total, LOVED_ROWS);
            let page = clamp_page(s.song_page, st.song_total, LOVED_ROWS);
            set_song_page(st, page);
            st.songs = sqlx::query_as::<_, TrackRow>(sqlx::AssertSqlSafe(format!(
                "{TRACK_SELECT}{loved}{} LIMIT ? OFFSET ?",
                song_order(&s.song_sort, &s.song_dir)
            )))
            .bind(LOVED_ROWS)
            .bind(page * LOVED_ROWS)
            .fetch_all(pool)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(into_track)
            .collect();
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
        let show: Option<ShowRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SHOW_SELECT} WHERE p.id = ?")))
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
        let rows: Vec<EpisodeRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{EPISODE_SELECT} WHERE e.podcast_id = ? \
             ORDER BY COALESCE(e.published, 0) {order} LIMIT ? OFFSET ?"
        )))
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
    let mut cq = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    if filtered {
        cq = cq.bind(s.pod_cat.clone());
    }
    if searching {
        cq = cq.bind(like.clone()).bind(like.clone());
    }
    st.pod_total = cq.fetch_one(pool).await.unwrap_or(0);

    let show_sql =
        format!("{SHOW_SELECT}{where_cat} ORDER BY p.title COLLATE NOCASE LIMIT ? OFFSET ?");
    let mut q = sqlx::query_as::<_, ShowRow>(sqlx::AssertSqlSafe(&*show_sql));
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
        let pinned: Vec<ShowRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{SHOW_SELECT} WHERE COALESCE(p.home_pinned, 0) = 1"
        )))
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
        let rows: Vec<EpisodeRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{EPISODE_SELECT} WHERE e.id IN ( \
                 SELECT id FROM podcast_episodes pe \
                 WHERE pe.published = (SELECT MAX(published) FROM podcast_episodes x \
                                       WHERE x.podcast_id = pe.podcast_id) \
                 GROUP BY pe.podcast_id) \
             ORDER BY COALESCE(e.published, 0) DESC LIMIT ?"
        )))
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
        let rows: Vec<EpisodeRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "{EPISODE_SELECT} WHERE e.downloaded_path IS NOT NULL AND e.downloaded_path != '' \
             ORDER BY {order} LIMIT ? OFFSET ?"
        )))
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
        let mut q = sqlx::query_as::<_, EpisodeRow>(sqlx::AssertSqlSafe(&*queue_sql));
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
        let mut q = sqlx::query_as::<_, (i64, String, f64, f64)>(sqlx::AssertSqlSafe(&*chapter_sql));
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
