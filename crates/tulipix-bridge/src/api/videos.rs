//! Videos section — four pages behind one contract.
//!
//! The Slint side of this is three `.slint` pages (`page_videos`, `page_livetv`,
//! `page_stream_plus`) and a 7,000-line glue crate, declaring 445 properties and
//! 88 callbacks. None of that shape survives here: Flutter has state management,
//! so the ephemeral half — which modal is open, what has been typed but not
//! submitted, which card the pointer is over — stays in Dart, and what crosses
//! the boundary is one snapshot, one command, one event stream and three lazy
//! resolvers.
//!
//! The snapshot is a *painted* state, not a recomputation. Every handler writes
//! into it and `videos_dispatch` hands back a clone, which is exactly what the
//! Slint build does with its properties — and the reason a keystroke in the
//! local search box does not re-query the Live TV channel list or the Stream
//! download ledger.
//!
//! Playback is out-of-process mpv with its own window (`crate::vmpv`), for the
//! four pages alike. There is no embedded player and there will not be one.

use anyhow::Result;
use flutter_rust_bridge::frb;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::db::videos_pool;
use crate::frb_generated::StreamSink;

/// Tiles added to the local grid per page. `ShowMore` grows the window by
/// another one of these — the grid is not virtualised on either side, and every
/// tile owns a decoded poster.
pub(crate) const PAGE: usize = 300;

/// Grid poster tier. 2:3 artwork at 300px wide is sharp at the 150px the tile
/// draws it at, on a 2× display.
const GRID_W: u32 = 300;
const GRID_H: u32 = 450;

// ---------------------------------------------------------------- state ----

/// One poster in the local library grid. Richer than a photo tile: it carries
/// playback state, so the poster can paint a resume bar, a watched check and
/// the star badge without a second query.
#[derive(Debug, Clone, Default)]
pub struct VideoTile {
    pub item_id: i64,
    /// Position in the section's play list, which is what a click hands back.
    pub index: i64,
    pub path: String,
    /// Absolute path to the poster — the TMDB one when the scraper found it,
    /// otherwise the rendered frame. Empty until `videos_ensure_thumb` has run,
    /// which the tile widget calls when it scrolls into view.
    pub thumb: String,
    pub label: String,
    pub starred: bool,
    pub watched: bool,
    /// 0..1 resume fraction; 0 is unstarted or unknown.
    pub progress: f64,
    /// Pre-formatted "1:42:08", or empty when the duration is unknown.
    pub duration: String,
    /// TV episode coordinates. Both 0 when the file is not an episode.
    pub season: i64,
    pub episode: i64,
}

/// One TV show card: poster, title, episode count.
#[derive(Debug, Clone, Default)]
pub struct ShowCard {
    pub id: i64,
    pub title: String,
    pub cover: String,
    pub count: i64,
}

/// A season's worth of episode tiles, for the drilled-in show browser. The
/// tiles keep their global `index`, so click-to-play still maps.
#[derive(Debug, Clone, Default)]
pub struct VideoSeason {
    pub number: i64,
    pub label: String,
    pub tiles: Vec<VideoTile>,
}

/// One card in a Discover rail.
#[derive(Debug, Clone, Default)]
pub struct DiscoverCard {
    pub tmdb_id: i64,
    pub title: String,
    /// 0 = unknown.
    pub year: i64,
    pub poster: String,
    /// 0..10 TMDB vote average; 0 = unknown.
    pub vote: f64,
    /// "Jun 14, 2025", pre-formatted, or empty.
    pub release_date: String,
    pub is_movie: bool,
}

// ---- Stream tab ----

/// One result, saved title or resume card. `id` is the catalogue's subject id,
/// opaque to the UI.
#[derive(Debug, Clone, Default)]
pub struct StreamCard {
    pub id: String,
    pub title: String,
    /// The year on a result; the episode on a resume card.
    pub year: String,
    pub poster: String,
    pub is_series: bool,
    /// Seasons on a split show; doubles as the new-episode badge on a saved one.
    pub seasons: i64,
    /// "2024 · Drama · ★ 8.7" on a saved card, "18 min left" on a resume one.
    pub meta: String,
    pub overview: String,
    /// 0..1, drives the bar on a Continue Watching or History card.
    pub progress: f64,
}

#[derive(Debug, Clone, Default)]
pub struct StreamEpisode {
    pub number: i64,
    /// Empty when the catalogue never named it.
    pub title: String,
    pub progress: f64,
}

/// One line of the History page: everything about a past session on one row.
#[derive(Debug, Clone, Default)]
pub struct StreamHistoryRow {
    pub id: String,
    pub title: String,
    /// "Series · S02E04" / "Movie".
    pub kind: String,
    /// "34% · 48 min left" / "Watched".
    pub state: String,
    /// "2 days ago".
    pub ago: String,
    pub poster: String,
    pub progress: f64,
    pub finished: bool,
    pub season: i64,
    pub episode: i64,
}

#[derive(Debug, Clone, Default)]
pub struct StreamDownloadRow {
    pub id: i64,
    /// "Severance · S01E04 · 720p".
    pub title: String,
    /// Downloading / Waiting / Saved / Failed / Missing.
    pub state: String,
    /// Size progress, the error, or where it was saved.
    pub detail: String,
    pub dest: String,
    pub progress: f64,
    /// Queued or running.
    pub active: bool,
    /// Finished, and the file is still there.
    pub done: bool,
    /// Failed, cancelled, or finished-but-missing.
    pub failed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct StreamHostHealth {
    pub host: String,
    /// "142 ms", or why it failed.
    pub note: String,
    pub ok: bool,
}

/// A pickable option in the detail pane — a language cut, or a subtitle track.
#[derive(Debug, Clone, Default)]
pub struct StreamChip {
    pub id: String,
    pub label: String,
    pub active: bool,
}

/// One playable file. `index` maps back to the resolved list held in Rust.
#[derive(Debug, Clone, Default)]
pub struct StreamFileRow {
    pub index: i64,
    /// "1080p".
    pub label: String,
    /// "2.1 GB · hevc".
    pub sub: String,
    pub uploader: String,
    /// "3 subs" / "No subs".
    pub subs: String,
    pub has_subs: bool,
}

/// The Stream tab, whole. Painted by its handlers, cloned by the snapshot.
#[derive(Debug, Clone, Default)]
pub struct StreamView {
    pub query: String,
    pub busy: bool,
    pub status: String,
    pub results: Vec<StreamCard>,
    pub more: bool,
    pub suggestions: Vec<String>,
    pub recent: Vec<String>,
    pub trending: Vec<StreamCard>,
    pub resume: Vec<StreamCard>,
    pub vertical: Vec<StreamCard>,
    pub bookmarks: Vec<StreamCard>,
    pub bm_sort: String,
    pub bm_asc: bool,
    pub history: Vec<StreamHistoryRow>,
    pub history_page: i64,
    pub history_pages: i64,
    pub downloads: Vec<StreamDownloadRow>,
    pub dl_pending: i64,
    pub dl_label: String,
    pub dl_frac: f64,
    pub dl_active: bool,

    pub detail_open: bool,
    pub title: String,
    pub meta: String,
    pub overview: String,
    pub cover: String,
    pub is_series: bool,
    pub dubs: Vec<StreamChip>,
    pub seasons: Vec<i64>,
    pub episodes: Vec<StreamEpisode>,
    pub season: i64,
    pub episode: i64,
    pub files: Vec<StreamFileRow>,
    pub subs: Vec<StreamChip>,
    /// -1 = subtitles off.
    pub sub_choice: i64,
    /// The quality filter. "" means every rung.
    pub resolution: String,
    /// Index into `files` the info-box actions target; -1 = none armed.
    pub current: i64,
    pub current_label: String,
    pub bookmarked: bool,

    pub preview_open: bool,
    pub preview_title: String,
    pub preview_meta: String,
    pub preview_overview: String,
    pub preview_seasons: i64,
    pub preview_is_series: bool,
    pub preview_langs: String,
    pub preview_cover: String,

    /// moviebox | fourk.
    pub source: String,
    pub hosts_text: String,
    pub hosts_error: String,
    pub hosts_checking: bool,
    pub host_health: Vec<StreamHostHealth>,
    pub hosts_summary: String,
    /// The host list in fastest-first order, offered after a test.
    pub hosts_ranked: String,
    pub key_text: String,
    pub key_error: String,
    pub fourk_base: String,
    pub fourk_default: String,
    pub fourk_error: String,

    pub cast_devices: Vec<String>,
    pub cast_active: bool,
    pub cast_target: String,

    pub sub_scale: f64,
    pub sub_delay: f64,
    pub autoplay: bool,
    pub to_library: bool,
}

// ---- Live TV ----

#[derive(Debug, Clone, Default)]
pub struct LiveChannel {
    /// Position in the unfiltered channel list, so a click survives a filter,
    /// a sort and a page turn.
    pub index: i64,
    pub id: String,
    pub name: String,
    pub group: String,
    pub logo: String,
}

#[derive(Debug, Clone, Default)]
pub struct LiveGroup {
    pub name: String,
    pub count: i64,
}

/// One selectable iptv-org playlist. `kind` is category | language | country.
#[derive(Debug, Clone, Default)]
pub struct LivePlaylist {
    pub name: String,
    pub url: String,
    pub kind: String,
    pub picked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct LiveView {
    pub channels: Vec<LiveChannel>,
    pub groups: Vec<LiveGroup>,
    /// "" = every group.
    pub group: String,
    pub query: String,
    pub busy: bool,
    pub status: String,
    /// Loaded in total, before the search and the group filter.
    pub total: i64,
    /// Survivors of both.
    pub matched: i64,
    pub picked_count: i64,
    pub page: i64,
    pub pages: i64,
    /// default | name | group | res.
    pub sort: String,
    pub asc: bool,

    /// -1 when no card on this page is the one on air.
    pub now_index: i64,
    pub now_name: String,
    /// The name, cut to twenty characters for the header pill.
    pub now_label: String,
    pub now_group: String,
    pub now_id: String,
    pub now_url: String,
    pub now_res: String,
    pub now_logo: String,
    pub now_cache: String,

    /// country | language | category.
    pub config_tab: String,
    pub config_query: String,
    pub playlists: Vec<LivePlaylist>,
}

// ---- Stream Plus ----

#[derive(Debug, Clone, Default)]
pub struct SplusCard {
    pub key: String,
    pub title: String,
    pub note: String,
    pub poster: String,
    /// The format — "TV" / "MOVIE".
    pub badge: String,
    pub progress: f64,
    pub saved: bool,
    /// Hidden by the age gate; drawn as a locked card.
    pub blocked: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SplusEpisode {
    pub number: i64,
    pub title: String,
    pub thumb: String,
    pub progress: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SplusFile {
    pub index: i64,
    /// "1080p · AllManga".
    pub label: String,
    /// "412 MB · h264".
    pub sub: String,
    pub uploader: String,
    pub subs: String,
    pub has_subs: bool,
    /// The source answered, with nothing.
    pub failed: bool,
    pub note: String,
}

#[derive(Debug, Clone, Default)]
pub struct SplusChip {
    pub id: String,
    pub label: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SplusHistory {
    pub key: String,
    pub title: String,
    /// "Series · S02E07" / "Movie".
    pub kind: String,
    /// "62% · 18 min left" / "Watched".
    pub state: String,
    pub ago: String,
    /// "AllManga · dub · 1080p".
    pub detail: String,
    pub poster: String,
    pub progress: f64,
    pub finished: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SplusDownload {
    pub id: i64,
    pub title: String,
    pub state: String,
    pub detail: String,
    pub progress: f64,
    pub active: bool,
    pub done: bool,
    pub failed: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SplusHealth {
    pub source: String,
    pub label: String,
    /// "1.1 s" / "403 forbidden".
    pub note: String,
    pub ok: bool,
    pub enabled: bool,
    /// Serving its hour at the bottom after three failures in a row.
    pub sunk: bool,
}

#[derive(Debug, Clone, Default)]
pub struct SplusView {
    /// home | search | detail | library | downloads | settings.
    pub view: String,
    pub busy: bool,
    pub status: String,
    pub query: String,
    /// anime | movie | tv.
    pub lane: String,
    pub results: Vec<SplusCard>,
    pub results_total: i64,
    pub results_more: bool,
    pub suggestions: Vec<String>,
    pub recent: Vec<String>,

    pub featured: Vec<SplusCard>,
    pub continue_rows: Vec<SplusHistory>,
    pub trending: Vec<SplusCard>,
    pub seasonal: Vec<SplusCard>,
    pub season_label: String,

    pub d_title: String,
    pub d_meta: String,
    pub d_overview: String,
    pub d_cover: String,
    pub d_badge: String,
    pub d_saved: bool,
    pub d_skip_note: String,
    pub d_audio: Vec<SplusChip>,
    pub d_seasons: Vec<SplusChip>,
    pub d_episodes: Vec<SplusEpisode>,
    pub d_files: Vec<SplusFile>,
    pub d_subs: Vec<SplusChip>,
    pub d_episode: i64,
    pub d_file: i64,
    pub d_resolved: String,
    pub d_resolving: bool,

    pub saved: Vec<SplusCard>,
    pub history: Vec<SplusHistory>,
    /// added | watched | title | year | progress.
    pub sort: String,
    /// saved | history.
    pub lib_tab: String,

    pub downloads: Vec<SplusDownload>,
    pub dl_active: i64,
    pub dl_rate: String,

    pub health: Vec<SplusHealth>,
    pub audio_pref: String,
    pub skip_op_ed: bool,
    pub watched_pct: i64,
    pub autoplay_secs: i64,
    pub hide_finished: bool,
    pub subs_wyzie: bool,
    pub subs_subdl: bool,
    pub subs_lang: String,
    pub subs_with_file: bool,
    pub quality: i64,
    pub into_library: bool,
    pub slots: i64,
    pub notify_download: bool,
    pub notify_episode: bool,
    pub download_dir: String,

    pub hosts_text: String,
    pub hosts_checking: bool,
    pub hosts_saved: bool,
}

/// The whole section. One value out, whatever the user was doing.
#[derive(Debug, Clone, Default)]
pub struct VideosState {
    /// tv | movies | local | discover | livetv | stream | splus.
    pub kind: String,
    /// library | continue | starred | archive | trash.
    pub category: String,
    pub query: String,
    /// Rows matching the current tab, before paging.
    pub item_count: i64,
    /// How many remain past the end of `tiles`.
    pub more_count: i64,
    pub tiles: Vec<VideoTile>,
    pub shows: Vec<ShowCard>,
    pub next_up: Vec<VideoTile>,
    pub seasons: Vec<VideoSeason>,
    pub show_open: bool,
    pub show_title: String,

    pub discover_trending_movies: Vec<DiscoverCard>,
    pub discover_trending_shows: Vec<DiscoverCard>,
    pub discover_upcoming: Vec<DiscoverCard>,
    pub discover_on_air: Vec<DiscoverCard>,
    pub discover_busy: bool,

    pub stream: StreamView,
    pub live: LiveView,
    pub splus: SplusView,
}

// -------------------------------------------------------------- commands ----

/// One flat enum rather than one per page.
///
/// A nested `VideosCmd::Stream(StreamCmd)` would read better in Rust and worse
/// in Dart — every call site would carry two constructors — and the Slint
/// callbacks it replaces are flat and prefixed for exactly the same reason.
#[derive(Debug, Clone)]
pub enum VideosCmd {
    /// Re-read the current view. Sent on mount and after an external change.
    Refresh,
    /// tv | movies | local | discover | livetv | stream | splus. Entering a tab
    /// is also what loads it, the way `set-kind` does on the Slint side.
    SetKind { kind: String },
    /// library | continue | starred | archive | trash.
    SetCategory { name: String },
    Search { query: String },
    ShowMore,
    AddFolder { path: String },
    Scan,
    /// Drop the cached frames for everything in the section and rescan.
    ClearThumbs,
    /// Play the tile at this index in an mpv window.
    Play { index: i64 },
    /// Play one item by its library id, wherever it is -- Home's cards and its
    /// Continue rows hold an id and no grid position, and `Play` indexes the
    /// grid that happens to be open.
    PlayItem { id: i64 },
    /// star | archive | trash | restore | delete-forever | mark-watched |
    /// reveal | play. Which of them a tile offers depends on the tab.
    TileAction { index: i64, action: String },
    OpenShow { show_id: i64 },
    ShowBack,
    RefreshDiscover,

    // ---- Stream ----
    StreamSearch { query: String },
    StreamSearchMore,
    StreamHome,
    StreamOpen { id: String },
    /// A Continue Watching or History card, opened against the catalogue it was
    /// actually played from rather than whichever is selected now.
    StreamOpenResume { id: String },
    /// A trending or vertical-drama pick. Both lists come from MovieBox's feed
    /// whatever the provider button says, so their ids only mean something there.
    StreamOpenPick { id: String },
    StreamBack,
    StreamPlay { index: i64 },
    StreamSetDub { id: String },
    StreamSetSeason { season: i64 },
    StreamSetEpisode { episode: i64 },
    StreamSetSub { index: i64 },
    StreamSetResolution { res: String },
    StreamSuggest { query: String },
    StreamSuggestClear,
    /// Details for the card under the cursor. An empty id closes the preview.
    StreamPreview { id: String },
    StreamDownload { index: i64 },
    StreamDownloadCancel,
    StreamDownloadDismiss,
    StreamSetCurrent { index: i64 },
    StreamPlayCurrent,
    StreamDownloadCurrent,
    StreamDownloadSeason,
    StreamTrailer,
    StreamToggleBookmark,
    StreamBookmarksLoad,
    StreamBookmarkRemove { id: String },
    StreamBookmarksSort { key: String, asc: bool },
    StreamRecentClear,
    StreamFeedLoad,
    StreamHostsLoad,
    StreamHostsSave { text: String },
    StreamHostsReset,
    StreamHostsCheck { text: String },
    StreamKeySave { key: String },
    StreamKeyReset,
    StreamSetSource { name: String },
    StreamSourceLoad,
    StreamFourkSave { url: String },
    StreamFourkReset,
    StreamDownloadsLoad,
    StreamDownloadsResume,
    StreamDownloadPlay { id: i64 },
    StreamDownloadRetry { id: i64 },
    StreamDownloadForget { id: i64 },
    StreamDownloadDeleteFile { id: i64 },
    StreamDownloadReveal { id: i64 },
    StreamDownloadsClear,
    StreamDownloadsCancelAll,
    StreamHistoryLoad { page: i64 },
    StreamHistoryClear,
    StreamHistoryRemove { id: String, page: i64 },
    StreamHistoryPlay { id: String, season: i64, episode: i64 },
    StreamCastDiscover,
    StreamCastTo { device: String },
    StreamCastStop,
    StreamSetSubScale { value: f64 },
    StreamSetSubDelay { value: f64 },
    StreamSetAutoplay { on: bool },
    StreamSetToLibrary { on: bool },

    // ---- Live TV ----
    LiveRefresh,
    LiveSearch { query: String },
    LiveSetGroup { group: String },
    LiveSetPage { page: i64 },
    LiveSetSort { key: String, asc: bool },
    LivePlay { index: i64 },
    /// Fill in the cache line for the now-playing sheet, which touches the disk.
    LiveInfoOpened,
    /// Open (or re-filter) the playlist picker.
    LiveConfigLoad { tab: String, query: String },
    LiveTogglePlaylist { url: String },
    LiveConfigClear,
    LiveConfigSave,

    // ---- Stream Plus ----
    SplusSetView { view: String },
    SplusHomeLoad,
    SplusSearch { query: String },
    SplusSetLane { lane: String },
    SplusSuggest { query: String },
    SplusLoadMore,
    SplusRecentClear,
    SplusOpenCard { key: String },
    SplusBack,
    SplusSetAudio { id: String },
    SplusSetSeason { id: String },
    SplusSetEpisode { episode: i64 },
    SplusSetFile { index: i64 },
    SplusSetSub { id: String },
    SplusPlay,
    SplusDownload,
    SplusDownloadSeason,
    SplusToggleSave,
    SplusCast,
    SplusAddToLibrary,
    SplusRetrySource,
    SplusLibraryLoad,
    SplusSetLibTab { tab: String },
    SplusSetSort { key: String },
    SplusSavedRemove { key: String },
    SplusHistoryPlay { key: String },
    SplusHistoryRemove { key: String },
    SplusHistoryClear,
    SplusDownloadsLoad,
    SplusDlCancel { id: i64 },
    SplusDlRetry { id: i64 },
    SplusDlPlay { id: i64 },
    SplusDlReveal { id: i64 },
    SplusDlDelete { id: i64 },
    SplusDlCancelAll,
    SplusDlClear,
    SplusSettingsLoad,
    SplusSetSourceEnabled { id: String, on: bool },
    SplusMoveSource { id: String, delta: i64 },
    SplusSourceTest,
    SplusSourceReset,
    SplusSetFlag { key: String, on: bool },
    SplusSetText { key: String, value: String },
    SplusSetNum { key: String, value: i64 },
    SplusSetDownloadDir { path: String },
    SplusServersOpened,
    SplusHostsSave { text: String },
    SplusHostsReset,
    SplusHostsCheck,
}

#[derive(Debug, Clone)]
pub enum VideosEvent {
    ScanStarted { root: String },
    ScanFinished { scanned: i64, inserted: i64, updated: i64, missing: i64 },
    /// A background task moved something the snapshot shows — the download
    /// runner finishing a job, mpv exiting and writing progress back, the
    /// autoplay chain starting the next episode. Dart answers with `Refresh`.
    Changed,
    /// Download progress, which arrives many times a second and must not cost a
    /// snapshot each time. The Downloads page patches its own row from this.
    DownloadTick { id: i64, progress: f64, detail: String, label: String, active: bool },
    Failed { message: String },

    /// Open this in the in-app player.
    ///
    /// `props` are mpv properties as `k=v`, in the order they must be applied:
    /// the Settings→Playback options first, then whatever the caller added.
    /// `sub-file=` repeats — mpv appends each to a list, so Dart loads them in
    /// order and the first is the chosen track. `token` comes back on every
    /// report so a reply from the source this one replaced can be dropped.
    VideoPlay { token: i64, src: String, start_at: f64, props: Vec<String> },
    /// Close the player. Also sent immediately before every `VideoPlay`.
    VideoStop,
}

// --------------------------------------------------------------- session ----

/// Everything the section is looking at, painted and private halves together.
///
/// One lock rather than one per page: `snapshot` reads all of it, and two locks
/// taken in different orders by two handlers is the deadlock this does not have.
///
/// `frb(ignore)`: `pub(crate)` is not part of the Dart contract, and the
/// generator would otherwise try to emit codecs for the domain types inside it.
#[frb(ignore)]
pub(crate) struct Session {
    /// What Dart sees.
    pub ui: VideosState,

    // ---- local library ----
    /// How many tiles the grid may materialise.
    pub limit: usize,
    /// Set by `ShowMore` alone: every other rebuild starts at one page again.
    pub keep_limit: bool,
    pub show_id: Option<i64>,
    /// Tile index → absolute path, and → item id. Rebuilt with the grid, so a
    /// click resolves against what is actually on screen.
    pub paths: Vec<String>,
    pub ids: Vec<i64>,

    pub st: crate::vid_stream::StreamSession,
    pub live: crate::vid_livetv::LiveSession,
    pub sp: crate::vid_splus::SplusSession,
}

impl Session {
    fn new() -> Self {
        let mut ui = VideosState {
            kind: "local".into(),
            category: "library".into(),
            ..Default::default()
        };
        crate::vid_stream::init_view(&mut ui.stream);
        crate::vid_livetv::init_view(&mut ui.live);
        crate::vid_splus::init_view(&mut ui.splus);
        Self {
            ui,
            limit: PAGE,
            keep_limit: false,
            show_id: None,
            paths: Vec::new(),
            ids: Vec::new(),
            st: crate::vid_stream::StreamSession::default(),
            live: crate::vid_livetv::LiveSession::default(),
            sp: crate::vid_splus::SplusSession::default(),
        }
    }

    /// A session the sub-page tests can mutate without a database or a runtime.
    #[cfg(test)]
    pub(crate) fn probe() -> Self {
        Self::new()
    }
}

fn cell() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Session::new()))
}

/// Read or mutate the session. The guard cannot escape the closure, which is
/// the point: a `MutexGuard` merely *in lexical scope* across an `.await` makes
/// the whole future non-`Send`, and frb's dispatcher requires `Send`. Keeping
/// the rule by shape rather than by care is the only version of it that holds.
pub(crate) fn with<R>(f: impl FnOnce(&mut Session) -> R) -> R {
    let mut g = cell().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

fn events() -> &'static Mutex<Vec<StreamSink<VideosEvent>>> {
    static E: OnceLock<Mutex<Vec<StreamSink<VideosEvent>>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Vec::new()))
}

pub(crate) fn emit(event: VideosEvent) {
    if let Ok(mut sinks) = events().lock() {
        // `add` fails once Dart has cancelled the subscription; dropping those
        // here is the only place they get collected.
        sinks.retain(|s| s.add(event.clone()).is_ok());
    }
}

pub(crate) async fn pool() -> Result<&'static sqlx::SqlitePool> {
    videos_pool().await
}

pub(crate) fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ------------------------------------------------------------- exported ----

/// Apply one command and hand back the resulting snapshot.
pub async fn videos_dispatch(cmd: VideosCmd) -> Result<VideosState> {
    apply(cmd).await?;
    Ok(with(|s| s.ui.clone()))
}

/// Scan progress, background changes and download ticks. Registering a second
/// sink is harmless — every sink gets every event.
#[frb(sync)]
pub fn videos_events(sink: StreamSink<VideosEvent>) {
    if let Ok(mut sinks) = events().lock() {
        sinks.push(sink);
    }
}

/// Render (or return) the grid poster for one library item.
///
/// Lazy, like the Photos grid's: the tile widget calls this when it scrolls into
/// view. A TMDB poster, where the scraper found one, is already on disk and
/// comes back without any work; everything else is one ffmpeg frame grab.
pub async fn videos_ensure_thumb(item_id: i64) -> Result<Option<String>> {
    let pool = pool().await?;
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "SELECT i.abs_path, mv.poster_local \
         FROM items i LEFT JOIN movies mv ON mv.item_id = i.id \
         WHERE i.id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let Some((abs, poster)) = row else { return Ok(None) };
    if let Some(p) = poster.filter(|p| Path::new(p).exists()) {
        return Ok(Some(p));
    }
    // An episode's poster hangs off its show, not off the file.
    let show_poster: Option<String> = sqlx::query_scalar(
        "SELECT s.poster_local FROM episodes e JOIN shows s ON s.id = e.show_id \
         WHERE e.item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .flatten();
    if let Some(p) = show_poster.filter(|p| Path::new(p).exists()) {
        return Ok(Some(p));
    }
    Ok(render_frame(PathBuf::from(abs)).await)
}

/// The resolved URL of the armed Stream file, ready for the clipboard.
///
/// Resolving happens here rather than in Dart because a 4KHDHub row points at a
/// landing page, and pasting that somewhere is not what "copy the stream link"
/// means. The copy itself is Flutter's `Clipboard`, which is the platform's own
/// and one dependency this crate does not have to carry.
pub async fn videos_stream_link(index: i64) -> Result<String> {
    crate::vid_stream::resolve_link(index).await
}

/// Same, for the Stream Plus file that is picked. Nothing to resolve there —
/// its sources hand back direct links — so this is a lookup.
#[frb(sync)]
pub fn videos_splus_link() -> String {
    crate::vid_splus::picked_link()
}

/// Close the player. Called from the app's exit hook, for the same reason
/// `music_shutdown` is: whatever is on should stop when the app goes, and the
/// hooks this drops are the ones that would otherwise write a position back
/// against a session nobody is watching.
#[frb(sync)]
pub fn videos_shutdown() {
    crate::vmpv::stop();
}

// -------------------------------------------------------- from the player ---
//
// Bare functions rather than `VideosCmd` arms, for the same reason the deck's
// reports are: a dispatch returns a whole `VideosState`, and these say nothing
// the grid needs redrawn for.

/// The player has a running clock. Live TV waits for this before it says
/// "Playing" — negotiating a live stream can take seconds.
#[frb(sync)]
pub fn videos_playback_started(token: i64) {
    crate::vmpv::started(token);
}

/// The player closed at `pos` of `dur`. This is what writes a film's position
/// back into `watch_progress`, so Dart must send it on every way out of the
/// player — closing it, not only reaching the end.
pub async fn videos_playback_ended(token: i64, pos: f64, dur: f64) {
    crate::vmpv::ended(token, pos, dur).await;
}

// ------------------------------------------------------- player services ----
//
// What the in-app player needs that Dart cannot reach: the settings file, the
// OS keyring, and the network. Bare functions rather than `VideosCmd` arms for
// the same reason the playback reports are — none of this redraws the grid.

/// A remembered setting belonging to the video player: its keymap, and where
/// screenshots go.
///
/// The key must start with `player.`. This is a key/value hatch for one
/// widget's own preferences, not a general way for Dart to write settings.json
/// — everything else in that file is a typed field or a curated panel row, and
/// an unguarded setter would quietly make it neither.
#[frb(sync)]
pub fn player_pref_get(key: String) -> String {
    if !key.starts_with("player.") {
        return String::new();
    }
    tulipix_core::settings::Settings::load()
        .map(|s| s.text(&key))
        .unwrap_or_default()
}

#[frb(sync)]
pub fn player_pref_set(key: String, value: String) {
    if !key.starts_with("player.") {
        return;
    }
    let Ok(mut s) = tulipix_core::settings::Settings::load() else {
        return;
    };
    // An empty value removes the key rather than storing a blank one, so
    // "reset to defaults" leaves settings.json as it was before the player
    // ever wrote to it.
    if value.is_empty() {
        s.advanced.remove(&key);
    } else {
        s.advanced.insert(key, value);
    }
    let _ = s.save();
}

/// Where `screenshot-to-file` writes, created if it is not there yet.
///
/// The user's Pictures folder, because a still of a film is something they went
/// looking for and the app's own data directory is not where anyone looks. An
/// explicit `player.screenshot-dir` wins, and a machine with no Pictures folder
/// falls back to somewhere that certainly exists.
#[frb(sync)]
pub fn player_screenshot_dir() -> String {
    let custom = player_pref_get("player.screenshot-dir".into());
    let dir = if custom.trim().is_empty() {
        pictures_dir()
            .map(|d| d.join("Tulipix"))
            .or_else(|| tulipix_core::paths::data_dir().map(|d| d.join("screenshots")))
            .unwrap_or_else(std::env::temp_dir)
    } else {
        PathBuf::from(custom.trim())
    };
    let _ = std::fs::create_dir_all(&dir);
    dir.display().to_string()
}

fn pictures_dir() -> Option<PathBuf> {
    let home = if cfg!(target_os = "windows") {
        std::env::var_os("USERPROFILE")
    } else {
        std::env::var_os("HOME")
    }?;
    let dir = PathBuf::from(home).join("Pictures");
    dir.is_dir().then_some(dir)
}

/// One subtitle, from either source.
pub struct SubtitleHit {
    /// Which source it came from: "opensubtitles" or "addic7ed". The download
    /// needs to know, because the two are fetched in completely different
    /// ways.
    pub provider: String,
    /// OpenSubtitles' file id. 0 for an Addic7ed result, which has none.
    pub file_id: i64,
    /// Addic7ed's download path and the page it was listed on, which its
    /// download needs as a Referer. Both empty for OpenSubtitles.
    pub link: String,
    pub referer: String,
    pub language: String,
    /// The uploader's release name — how you tell a 23.976 fps rip from a 25
    /// fps one, which is the difference between in sync and unwatchable. Empty
    /// when they left it blank.
    pub release: String,
    pub downloads: i64,
    pub from_trusted: bool,
}

/// Search OpenSubtitles for whatever is playing.
///
/// Hash search first when the source is a real local file: it identifies the
/// exact cut, and a subtitle matched that way is the only one that reliably
/// lands in sync. A stream, a file too small to hash, or a hash nothing matches
/// falls back to the title query — which is a guess, and why the release name
/// is in the result rows.
pub async fn videos_subtitle_search(
    source: String,
    title: String,
    language: String,
    season: i64,
    episode: i64,
) -> Result<Vec<SubtitleHit>> {
    let lang = if language.trim().is_empty() { "en" } else { language.trim() };
    let path = PathBuf::from(&source);
    let a7 = crate::services::addic7ed();

    // OpenSubtitles needs a key. Without one it used to be the whole of the
    // answer, so its absence was the error; now Addic7ed may still have
    // something, and only a search with no source left at all is an error.
    let mut hits = Vec::new();
    match os_client() {
        Ok(client) => {
            if path.is_file() {
                if let Ok((hash, _)) = tulipix_videos::sub_opensubtitles::osdb_hash(&path) {
                    // A failure here is not fatal: the query search below is
                    // the fallback, and it reports its own errors. Swallowing
                    // this one silently is the difference between "no hash
                    // match" and "the whole search is broken", and only the
                    // second is worth a message.
                    hits = client.search_by_hash(hash, lang).await.unwrap_or_default();
                }
            }
            if hits.is_empty() {
                let query = if title.trim().is_empty() {
                    path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string()
                } else {
                    title.trim().to_string()
                };
                if !query.is_empty() {
                    match client
                        .search_by_query(&query, lang, positive(season), positive(episode))
                        .await
                    {
                        Ok(found) => hits = found,
                        Err(e) if a7.is_none() => return Err(e),
                        Err(e) => tracing::debug!(error = %e, "opensubtitles: query search"),
                    }
                }
            }
        }
        Err(e) if a7.is_none() => return Err(e),
        Err(e) => tracing::debug!(error = %e, "opensubtitles: no key, Addic7ed only"),
    }

    let mut out: Vec<SubtitleHit> = hits
        .into_iter()
        .map(|h| SubtitleHit {
            provider: "opensubtitles".into(),
            file_id: h.file_id,
            link: String::new(),
            referer: String::new(),
            language: h.language,
            release: h.release.unwrap_or_default(),
            downloads: h.download_count.unwrap_or_default(),
            from_trusted: h.from_trusted,
        })
        .collect();

    // Then Addic7ed, under whatever OpenSubtitles had. It is the source that
    // has tonight's episode, and the one that has nothing at all for a film,
    // so it is a second list rather than a replacement. Its failures are a log
    // line: the results above are already on screen.
    if let Some(a7) = a7 {
        let show = addic7ed_show_title(&title, &source);
        match a7.search(&show, season, episode, lang).await {
            Ok(found) => out.extend(found.into_iter().map(|h| SubtitleHit {
                provider: "addic7ed".into(),
                file_id: 0,
                link: h.link,
                referer: h.referer,
                language: h.language,
                release: h.release,
                downloads: h.downloads,
                // Addic7ed has no trusted-uploader mark; a finished version is
                // the closest thing it has to one.
                from_trusted: h.completed,
            })),
            Err(e) => tracing::debug!(error = %e, "addic7ed: search"),
        }
    }

    Ok(out)
}

/// The show name Addic7ed files an episode under. Its URLs are keyed on the
/// series, never the episode, so a filename like `The.Expanse.S03E07.1080p`
/// has to lose everything after the series name first.
fn addic7ed_show_title(title: &str, source: &str) -> String {
    if !title.trim().is_empty() {
        return title.trim().to_string();
    }
    let stem = Path::new(source)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    tulipix_videos::agents::parse_filename(stem).title
}

/// Fetch one result and return the path it landed at, ready for `sub-add`.
pub async fn videos_subtitle_download(
    source: String,
    provider: String,
    file_id: i64,
    link: String,
    referer: String,
    language: String,
) -> Result<String> {
    let lang = if language.trim().is_empty() { "en" } else { language.trim() };
    let anchor = subtitle_anchor(&source);
    if provider == "addic7ed" {
        let a7 = crate::services::addic7ed()
            .ok_or_else(|| anyhow::anyhow!("Addic7ed is switched off in Settings › Services."))?;
        let hit = tulipix_videos::sub_addic7ed::A7Subtitle {
            release: String::new(),
            language: lang.to_string(),
            link,
            completed: true,
            downloads: 0,
            referer,
        };
        let out = a7.save_as_sibling(&anchor, &hit, lang).await?;
        return Ok(out.display().to_string());
    }
    let client = os_client()?;
    let link = client.download_link(file_id).await?;
    let out = client.save_as_sibling(&anchor, &link, lang, "srt").await?;
    Ok(out.display().to_string())
}

/// Where a downloaded subtitle should land.
///
/// Beside the film when that is possible, because that is where `sub_local`
/// looks on the next launch and the download becomes permanent rather than
/// something to do again tomorrow. A stream, or a folder the app cannot write
/// to, falls back to its own directory: `save_as_sibling` reads only the stem
/// and the parent of what it is handed, so an anchor placed in the fallback
/// folder puts the file there under the same name.
fn subtitle_anchor(source: &str) -> PathBuf {
    let path = Path::new(source);
    if path.is_file() {
        if let Some(parent) = path.parent() {
            if tulipix_core::paths::dir_is_writable(parent) {
                return path.to_path_buf();
            }
        }
    }
    let stem: String = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.'))
        .take(80)
        .collect();
    let stem = if stem.trim().is_empty() { "subtitle".to_string() } else { stem };
    let dir = tulipix_core::paths::data_dir()
        .map(|d| d.join("subtitles"))
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&dir);
    dir.join(stem)
}

fn positive(n: i64) -> Option<i64> {
    (n > 0).then_some(n)
}

fn os_client() -> Result<tulipix_videos::sub_opensubtitles::OpenSubtitlesClient> {
    use tulipix_videos::sub_opensubtitles::{OpenSubtitlesClient, OsCredentials};
    let api_key = tulipix_core::api_keys::fetch("opensubtitles")
        .ok()
        .flatten()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No OpenSubtitles API key yet. Add one in Settings \u{2192} Services \u{2192} API keys."
            )
        })?;
    // OpenSubtitles requires an identifying User-Agent and rejects generic
    // ones; "<app> v<version>" is the form their docs ask for.
    Ok(OpenSubtitlesClient::new(OsCredentials {
        api_key,
        user_agent: format!("Tulipix v{}", env!("CARGO_PKG_VERSION")),
    }))
}

// -------------------------------------------------------------- dispatch ----

async fn apply(cmd: VideosCmd) -> Result<()> {
    match cmd {
        VideosCmd::Refresh => {
            let kind = with(|s| s.ui.kind.clone());
            if is_local_kind(&kind) {
                refresh_library().await?;
            }
        }
        VideosCmd::SetKind { kind } => {
            let previous = with(|s| {
                let was = s.ui.kind.clone();
                s.ui.kind = kind.clone();
                s.limit = PAGE;
                // Leaving the TV tab leaves any drilled-in show with it.
                s.show_id = None;
                s.ui.show_open = false;
                s.ui.show_title.clear();
                was
            });
            let _ = previous;
            match kind.as_str() {
                "discover" => refresh_discover().await?,
                "stream" => crate::vid_stream::enter().await,
                "livetv" => crate::vid_livetv::enter().await,
                "splus" => crate::vid_splus::enter().await,
                _ => refresh_library().await?,
            }
        }
        VideosCmd::SetCategory { name } => {
            with(|s| {
                s.ui.category = name;
                s.limit = PAGE;
            });
            refresh_library().await?;
        }
        VideosCmd::Search { query } => {
            with(|s| {
                s.ui.query = query;
                s.limit = PAGE;
            });
            refresh_library().await?;
        }
        VideosCmd::ShowMore => {
            with(|s| {
                s.limit += PAGE;
                s.keep_limit = true;
            });
            refresh_library().await?;
        }
        VideosCmd::AddFolder { path } => {
            add_watched_folder(Path::new(&path));
            scan_watched().await;
            refresh_library().await?;
        }
        VideosCmd::Scan => {
            scan_watched().await;
            refresh_library().await?;
        }
        VideosCmd::ClearThumbs => {
            clear_thumbs().await;
            scan_watched().await;
            refresh_library().await?;
        }
        VideosCmd::Play { index } => play_at(index).await?,
        VideosCmd::PlayItem { id } => play_item_id(id).await?,
        VideosCmd::TileAction { index, action } => tile_action(index, &action).await?,
        VideosCmd::OpenShow { show_id } => {
            let pool = pool().await?;
            let title: String = sqlx::query_scalar("SELECT title FROM shows WHERE id = ?")
                .bind(show_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .unwrap_or_default();
            with(|s| {
                s.show_id = Some(show_id);
                s.ui.show_title = title;
                s.limit = PAGE;
            });
            refresh_library().await?;
        }
        VideosCmd::ShowBack => {
            with(|s| {
                s.show_id = None;
                s.ui.show_title.clear();
                s.limit = PAGE;
            });
            refresh_library().await?;
        }
        VideosCmd::RefreshDiscover => refresh_discover().await?,

        other => {
            // Every remaining variant belongs to one of the three sub-pages,
            // each of which owns its own dispatch. Splitting here rather than
            // inlining ~110 arms keeps this function the shape of the section.
            if crate::vid_stream::dispatch(&other).await? {
                return Ok(());
            }
            if crate::vid_livetv::dispatch(&other).await? {
                return Ok(());
            }
            crate::vid_splus::dispatch(&other).await?;
        }
    }
    Ok(())
}

/// True for the tabs that draw the on-disk library grid.
fn is_local_kind(kind: &str) -> bool {
    !matches!(kind, "discover" | "stream" | "livetv" | "splus")
}

// -------------------------------------------------------- local library ----

/// One library row, joined across `video_meta`, `watch_progress` and the
/// scraper's two tables.
struct Row {
    abs_path: String,
    item_id: i64,
    starred: bool,
    watched: bool,
    progress: f64,
    duration: String,
    season: i64,
    episode: i64,
    poster: Option<String>,
}

/// The rows belonging to `category`, already ordered the way the tab wants
/// them, narrowed by the top-level kind and the search box.
async fn rows_for(
    pool: &sqlx::SqlitePool,
    category: &str,
    kind: &str,
    show_id: Option<i64>,
    query: &str,
) -> Vec<Row> {
    let (filter, mut order): (&str, String) = match category {
        "continue" => (
            "vm.deleted_at IS NULL AND vm.archived = 0 AND vm.last_accessed IS NOT NULL \
             AND COALESCE(wp.finished,0) = 0 AND COALESCE(wp.position_s,0) > 0",
            "vm.last_accessed DESC".into(),
        ),
        "starred" => ("vm.deleted_at IS NULL AND vm.starred = 1", "i.added DESC, i.id DESC".into()),
        "archive" => ("vm.deleted_at IS NULL AND vm.archived = 1", "i.added DESC, i.id DESC".into()),
        "trash" => ("vm.deleted_at IS NOT NULL", "vm.deleted_at DESC".into()),
        _ => ("vm.deleted_at IS NULL AND vm.archived = 0", "i.added DESC, i.id DESC".into()),
    };
    // The kind narrows what the tab is allowed to show. Movies restricts to
    // scraped movies; TV restricts to episodes, and to one show's episodes once
    // drilled in, ordered by season then number.
    let kind_clause: String = match kind {
        "movies" => " AND vm.item_id IN (SELECT item_id FROM movies)".into(),
        "tv" => {
            order = "ep.season, ep.episode".into();
            match show_id {
                // `show_id` is an i64 read out of our own `shows` table, never
                // off the wire, so interpolating it cannot be an injection.
                Some(id) => format!(" AND ep.show_id = {id}"),
                None => " AND ep.item_id IS NOT NULL".into(),
            }
        }
        _ => String::new(),
    };
    let sql = format!(
        "SELECT i.abs_path, vm.item_id, vm.starred, \
                COALESCE(wp.finished,0) AS finished, \
                COALESCE(wp.position_s, 0.0) AS pos, \
                COALESCE(wp.duration_s, vm.duration_s) AS dur, \
                COALESCE(ep.season, 0) AS season, \
                COALESCE(ep.episode, 0) AS episode, \
                COALESCE(mv.poster_local, sh.poster_local) AS poster \
         FROM video_meta vm \
         JOIN items i ON i.id = vm.item_id \
         LEFT JOIN watch_progress wp ON wp.item_id = vm.item_id \
         LEFT JOIN episodes ep ON ep.item_id = vm.item_id \
         LEFT JOIN shows sh ON sh.id = ep.show_id \
         LEFT JOIN movies mv ON mv.item_id = vm.item_id \
         WHERE {filter}{kind_clause} ORDER BY {order}",
    );
    let rows: Vec<(String, i64, i64, i64, f64, Option<f64>, i64, i64, Option<String>)> =
        sqlx::query_as(sqlx::AssertSqlSafe(&*sql)).fetch_all(pool).await.unwrap_or_default();

    let q = query.trim().to_lowercase();
    rows.into_iter()
        .filter(|(abs, ..)| {
            q.is_empty()
                || Path::new(abs)
                    .file_name()
                    .and_then(|s| s.to_str())
                    .map(|n| n.to_lowercase().contains(&q))
                    .unwrap_or(false)
        })
        .map(|(abs_path, item_id, starred, finished, pos, dur, season, episode, poster)| {
            let progress = match dur {
                Some(d) if d > 0.0 => (pos / d).clamp(0.0, 1.0),
                _ => 0.0,
            };
            Row {
                abs_path,
                item_id,
                starred: starred != 0,
                watched: finished != 0,
                progress,
                duration: dur.map(fmt_duration).unwrap_or_default(),
                season,
                episode,
                poster,
            }
        })
        .collect()
}

/// `H:MM:SS`, or `M:SS` under an hour. Empty for anything under a second, which
/// is what an unprobed file reports.
pub(crate) fn fmt_duration(secs: f64) -> String {
    if !secs.is_finite() || secs < 1.0 {
        return String::new();
    }
    let total = secs as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Rebuild the grid for whatever tab is on screen.
///
/// The TV tab, not drilled into a show and on the Library sub-tab, draws show
/// cards and a Next Up rail instead of a tile grid — the one place the section
/// shows something other than posters of files.
async fn refresh_library() -> Result<()> {
    let pool = pool().await?;
    let (category, kind, query, show_id, limit) = with(|s| {
        if !std::mem::take(&mut s.keep_limit) {
            s.limit = PAGE;
        }
        (
            s.ui.category.clone(),
            s.ui.kind.clone(),
            s.ui.query.clone(),
            s.show_id,
            s.limit,
        )
    });

    if kind == "tv" && show_id.is_none() && category == "library" {
        let cards = show_cards(pool).await;
        let next = tulipix_videos::episodes::next_up(pool, 12).await.unwrap_or_default();
        with(|s| {
            s.ui.shows = cards;
            s.ui.tiles.clear();
            s.ui.seasons.clear();
            s.ui.show_open = false;
            s.ui.more_count = 0;
            s.ui.item_count = s.ui.shows.len() as i64;
        });
        // The Next Up rail plays through the same index → path map the grid
        // uses, so its entries are appended to that map rather than carrying
        // paths of their own.
        let mut tiles: Vec<VideoTile> = Vec::new();
        let mut paths: Vec<String> = Vec::new();
        let mut ids: Vec<i64> = Vec::new();
        for n in &next {
            let abs: Option<String> =
                sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                    .bind(n.episode.item_id)
                    .fetch_optional(pool)
                    .await
                    .ok()
                    .flatten();
            let Some(abs) = abs else { continue };
            let dur_s = n.episode.runtime_min.unwrap_or(0) as f64 * 60.0;
            let progress = match (n.resume_position_s, dur_s > 0.0) {
                (Some(p), true) => (p / dur_s).clamp(0.0, 1.0),
                _ => 0.0,
            };
            tiles.push(VideoTile {
                item_id: n.episode.item_id,
                index: paths.len() as i64,
                path: abs.clone(),
                thumb: n
                    .episode
                    .still_path
                    .clone()
                    .filter(|p| Path::new(p).exists())
                    .unwrap_or_default(),
                label: format!(
                    "{} · S{:02}E{:02}{}",
                    n.show_title,
                    n.episode.season,
                    n.episode.episode,
                    n.episode.title.as_deref().map(|t| format!(" — {t}")).unwrap_or_default()
                ),
                starred: false,
                watched: false,
                progress,
                duration: n.episode.runtime_min.map(|m| format!("{m} min")).unwrap_or_default(),
                season: n.episode.season,
                episode: n.episode.episode,
            });
            paths.push(abs);
            ids.push(n.episode.item_id);
        }
        with(|s| {
            s.ui.next_up = tiles;
            s.paths = paths;
            s.ids = ids;
        });
        return Ok(());
    }

    let rows = rows_for(pool, &category, &kind, show_id, &query).await;
    let total = rows.len();
    let held_back = total.saturating_sub(limit);

    let mut tiles: Vec<VideoTile> = Vec::with_capacity(total.min(limit));
    let mut paths: Vec<String> = Vec::with_capacity(total.min(limit));
    let mut ids: Vec<i64> = Vec::with_capacity(total.min(limit));
    for r in rows.iter().take(limit) {
        let label = Path::new(&r.abs_path)
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        tiles.push(VideoTile {
            item_id: r.item_id,
            index: tiles.len() as i64,
            path: r.abs_path.clone(),
            // A poster already on disk goes straight in; everything else waits
            // for the tile to ask, which is what keeps opening the tab cheap.
            thumb: r
                .poster
                .clone()
                .filter(|p| Path::new(p).exists())
                .unwrap_or_default(),
            label,
            starred: r.starred,
            watched: r.watched,
            progress: r.progress,
            duration: r.duration.clone(),
            season: r.season,
            episode: r.episode,
        });
        paths.push(r.abs_path.clone());
        ids.push(r.item_id);
    }

    // Drilled into a show: group the episodes by season. The rows already come
    // back ordered season,episode, so one pass yields contiguous groups.
    let drilled = kind == "tv" && show_id.is_some();
    let seasons: Vec<VideoSeason> = if drilled {
        let mut groups: Vec<(i64, Vec<VideoTile>)> = Vec::new();
        for t in &tiles {
            if groups.last().map(|g| g.0) != Some(t.season) {
                groups.push((t.season, Vec::new()));
            }
            if let Some(last) = groups.last_mut() {
                last.1.push(t.clone());
            }
        }
        groups
            .into_iter()
            .map(|(number, tiles)| VideoSeason {
                number,
                label: if number > 0 {
                    format!("Season {number}")
                } else {
                    "Specials".to_string()
                },
                tiles,
            })
            .collect()
    } else {
        Vec::new()
    };

    with(|s| {
        s.ui.show_open = drilled;
        s.ui.tiles = tiles;
        s.ui.seasons = seasons;
        s.ui.item_count = total as i64;
        s.ui.more_count = held_back as i64;
        s.paths = paths;
        s.ids = ids;
        if !(kind == "tv" && show_id.is_none() && category == "library") {
            s.ui.shows.clear();
            s.ui.next_up.clear();
        }
    });
    Ok(())
}

/// Show cards: title, episode count, and the best poster there is — the TMDB
/// one where the scraper found it, otherwise the first episode's own frame.
async fn show_cards(pool: &sqlx::SqlitePool) -> Vec<ShowCard> {
    let rows: Vec<(i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.id, s.title, COUNT(e.item_id) AS n, \
                (SELECT i.abs_path FROM episodes e2 JOIN items i ON i.id = e2.item_id \
                 WHERE e2.show_id = s.id ORDER BY e2.season, e2.episode LIMIT 1) AS cover, \
                s.poster_local \
         FROM shows s JOIN episodes e ON e.show_id = s.id \
         GROUP BY s.id ORDER BY s.title",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();

    let mut out = Vec::with_capacity(rows.len());
    for (id, title, count, cover, poster) in rows {
        let art = match poster.filter(|p| Path::new(p).exists()) {
            Some(p) => p,
            // Rendered here rather than lazily: a show card has no item id to
            // hand back, so there is nothing for Dart to ask about later.
            None => match cover {
                Some(abs) => render_frame(PathBuf::from(abs)).await.unwrap_or_default(),
                None => String::new(),
            },
        };
        out.push(ShowCard { id, title, cover: art, count });
    }
    out
}

/// Render (or return the cached) 2:3 frame for one video file.
///
/// CPU-bound and shells out to ffmpeg, so it never runs on a bridge worker.
async fn render_frame(src: PathBuf) -> Option<String> {
    tokio::task::spawn_blocking(move || {
        let spec = tulipix_core::thumbs::ThumbSpec {
            kind: tulipix_core::thumbs::ThumbKind::Video,
            width: GRID_W,
            height: GRID_H,
        };
        tulipix_core::thumbs::render_or_cache(&src, spec)
            .ok()
            .flatten()
            .map(|r| r.path.to_string_lossy().into_owned())
    })
    .await
    .ok()
    .flatten()
}

/// Open the tile at `index` in an mpv window: resume from the stored position,
/// and record the access so the Continue tab picks it up.
async fn play_at(index: i64) -> Result<()> {
    let (path, item_id) = with(|s| {
        let i = index.max(0) as usize;
        (s.paths.get(i).cloned(), s.ids.get(i).copied())
    });
    let Some(path) = path else {
        tracing::warn!(index, "play: no path for that tile");
        return Ok(());
    };
    play_path(path, item_id).await
}

/// Play one item by its library id. The path is read from the row rather than
/// from whatever grid is open, so Home can start a film the Videos page has
/// never listed.
async fn play_item_id(id: i64) -> Result<()> {
    let pool = pool().await?;
    let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let Some(path) = path else {
        tracing::warn!(id, "play: no such item");
        return Ok(());
    };
    play_path(path, Some(id)).await
}

/// Resume where it was left, touch last-accessed, and hand it to mpv. Shared,
/// because a film started from Home has to reach the Continue strip exactly the
/// way one started from the grid does.
async fn play_path(path: String, item_id: Option<i64>) -> Result<()> {
    let pool = pool().await?;
    let mut resume = None;
    if let Some(id) = item_id {
        let _ = tulipix_videos::last_accessed::touch(pool, id).await;
        resume = tulipix_videos::watch_progress::resume(pool, id).await.ok().flatten();
        // First watch: seed a second so the Continue tab picks the film up
        // immediately. The real position overwrites it when mpv exits.
        if resume.is_none() {
            let _ = tulipix_videos::watch_progress::update(pool, id, 1.0, None).await;
        }
    }
    // The hook exists only to tell Dart the grid moved: `vmpv` writes the
    // position back into `watch_progress` itself, from the `item_id` above.
    let on_end: crate::vmpv::PlaybackEnd =
        std::sync::Arc::new(|_pos: f64, _dur: f64| emit(VideosEvent::Changed));
    crate::vmpv::play(path, resume, item_id, Vec::new(), Some(on_end));
    refresh_library().await
}

/// A poster's right-click menu. Which actions a tile offers depends on the tab
/// it is in — Trash offers Restore, everything else offers Trash.
async fn tile_action(index: i64, action: &str) -> Result<()> {
    if action == "play" {
        return play_at(index).await;
    }
    let (path, item_id, category) = with(|s| {
        let i = index.max(0) as usize;
        (s.paths.get(i).cloned(), s.ids.get(i).copied(), s.ui.category.clone())
    });
    if action == "reveal" {
        if let Some(p) = path {
            if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(Path::new(&p)) {
                tracing::error!(error = %e, "reveal failed");
            }
        }
        return Ok(());
    }
    let Some(id) = item_id else { return Ok(()) };
    let pool = pool().await?;
    use tulipix_videos::{star_archive_trash as sat, watch_progress as wp};
    let outcome = match action {
        "star" => sat::set_starred(pool, id, category != "starred").await,
        "archive" => sat::set_archived(pool, id, category != "archive").await,
        "trash" => sat::trash(pool, id, sat::DEFAULT_PURGE_DAYS).await,
        "restore" => sat::restore(pool, id).await,
        // Drop the row from the index only. The file on disk is the user's; the
        // proxy row is ours, and this is the tab for getting rid of ours.
        "delete-forever" => sqlx::query("DELETE FROM video_meta WHERE item_id = ?")
            .bind(id)
            .execute(pool)
            .await
            .map(|_| ())
            .map_err(Into::into),
        "mark-watched" => {
            let now = wp::get(pool, id).await.ok().flatten().map(|p| p.finished).unwrap_or(false);
            // Marking it watched sends it to Trakt; un-marking it does not
            // take it back, because a history entry is a thing that happened.
            if !now {
                trakt_push_watched(id);
            }
            wp::mark_finished(pool, id, !now).await
        }
        _ => Ok(()),
    };
    if let Err(e) = outcome {
        tracing::error!(error = %e, action, "video flag action failed");
    }
    refresh_library().await
}

/// Drop the cached frame for everything the section knows about. The bytes come
/// back on the next scan; what this fixes is a thumbnail that no longer matches
/// the file behind it.
async fn clear_thumbs() {
    let Ok(pool) = pool().await else { return };
    let paths: Vec<String> = sqlx::query_scalar(
        "SELECT abs_path FROM items WHERE section = 'videos' AND missing_since IS NULL",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let removed = tokio::task::spawn_blocking(move || {
        let mut n = 0u32;
        for abs in &paths {
            let p = Path::new(abs);
            let Ok(meta) = std::fs::metadata(p) else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let key = tulipix_core::thumbs::key(p, mtime, meta.len());
            if let Some(dest) = tulipix_core::thumbs::thumb_path(&key) {
                if std::fs::remove_file(&dest).is_ok() {
                    n += 1;
                }
            }
        }
        n
    })
    .await
    .unwrap_or(0);
    tracing::info!(removed, "cleared video thumbnail cache");
}

// ------------------------------------------------------------- discover ----

/// The four TMDB rails, from the DB cache, with a refresh behind them.
///
/// The cache paints first and the network fills in after, so opening the tab
/// with no key configured — or with no connection — still shows whatever was
/// there last time rather than four empty rails.
async fn refresh_discover() -> Result<()> {
    use tulipix_videos::discover::{DiscoverKind, TmdbDiscover, load_feed, refresh};
    let pool = pool().await?;
    with(|s| s.ui.discover_busy = true);

    let key = tmdb_api_key();
    if let Some(k) = key.as_deref() {
        let client = TmdbDiscover::new(k);
        for kind in [
            DiscoverKind::TrendingMoviesDay,
            DiscoverKind::TrendingShowsDay,
            DiscoverKind::UpcomingMovies,
            DiscoverKind::OnTheAirShows,
        ] {
            if let Err(e) = refresh(pool, &client, kind).await {
                tracing::debug!(error = %e, "discover: refresh failed");
            }
        }
    }

    let (movies, shows, upcoming, on_air) = tokio::join!(
        load_feed(pool, DiscoverKind::TrendingMoviesDay),
        load_feed(pool, DiscoverKind::TrendingShowsDay),
        load_feed(pool, DiscoverKind::UpcomingMovies),
        load_feed(pool, DiscoverKind::OnTheAirShows),
    );

    let dir = poster_cache_dir();
    let mut built = Vec::new();
    for items in [movies, shows, upcoming, on_air] {
        built.push(discover_cards(items.unwrap_or_default(), key.as_deref(), dir.as_deref()).await);
    }
    let mut it = built.into_iter();
    let (m, sh, up, air) = (
        it.next().unwrap_or_default(),
        it.next().unwrap_or_default(),
        it.next().unwrap_or_default(),
        it.next().unwrap_or_default(),
    );
    with(|s| {
        s.ui.discover_trending_movies = m;
        s.ui.discover_trending_shows = sh;
        s.ui.discover_upcoming = up;
        s.ui.discover_on_air = air;
        s.ui.discover_busy = false;
    });
    Ok(())
}

async fn discover_cards(
    items: Vec<tulipix_videos::discover::DiscoverItem>,
    api_key: Option<&str>,
    cache_dir: Option<&Path>,
) -> Vec<DiscoverCard> {
    let client = api_key.map(tmdb_client);
    let mut out = Vec::with_capacity(items.len());
    for it in items {
        let mut poster = String::new();
        if let (Some(pp), Some(dir)) = (it.poster_path.as_deref(), cache_dir) {
            // Already on disk? The name is a hash of the URL, same formula the
            // scraper uses, so this never downloads what it already has.
            let hit = cached_tmdb_poster(pp, dir);
            poster = match hit {
                Some(p) => p,
                None => match &client {
                    Some(c) => c
                        .cache_poster(pp, dir)
                        .await
                        .map(|p| p.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    None => String::new(),
                },
            };
        }
        out.push(DiscoverCard {
            tmdb_id: it.tmdb_id,
            title: it.title,
            year: it.year.unwrap_or(0),
            poster,
            vote: it.vote_average.unwrap_or(0.0),
            release_date: it.release_date.as_deref().map(fmt_release_date).unwrap_or_default(),
            is_movie: it.is_movie,
        });
    }
    out
}

/// TMDB's poster base, or the mirror set in Settings › Self-hosted servers.
/// The setting names the image server (`…/t/p`, as TMDB's own is); the size
/// is added here. The client downloads from it and the cache is named by a
/// hash of the URL, so both must build it the same way -- which is why it is
/// one function.
fn tmdb_image_base() -> String {
    let custom = crate::api::shell::load().text("api.tmdb-image-base");
    let custom = custom.trim().trim_end_matches('/');
    match custom {
        "" => "https://image.tmdb.org/t/p/w500".into(),
        c if c.ends_with("/w500") => c.into(),
        c => format!("{c}/w500"),
    }
}

fn tmdb_client(key: impl Into<String>) -> tulipix_videos::tmdb::TmdbClient {
    let mut c = tulipix_videos::tmdb::TmdbClient::new(key);
    c.image_base = tmdb_image_base();
    c
}

fn cached_tmdb_poster(poster_path: &str, dir: &Path) -> Option<String> {
    use sha2::{Digest, Sha256};
    let url = format!("{}{poster_path}", tmdb_image_base());
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let p = dir.join(format!("{}.jpg", h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>()));
    p.exists().then(|| p.to_string_lossy().into_owned())
}

/// "YYYY-MM-DD" → "Jun 14, 2025". Anything shorter comes back as it arrived.
fn fmt_release_date(s: &str) -> String {
    if s.len() < 10 {
        return s.to_string();
    }
    let month = match &s[5..7] {
        "01" => "Jan",
        "02" => "Feb",
        "03" => "Mar",
        "04" => "Apr",
        "05" => "May",
        "06" => "Jun",
        "07" => "Jul",
        "08" => "Aug",
        "09" => "Sep",
        "10" => "Oct",
        "11" => "Nov",
        "12" => "Dec",
        _ => return s.to_string(),
    };
    let day: i32 = s[8..10].parse().unwrap_or(0);
    format!("{month} {day}, {}", &s[0..4])
}

fn tmdb_api_key() -> Option<String> {
    tulipix_core::api_keys::fetch("tmdb").ok().flatten()
}

pub(crate) fn poster_cache_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("videos").join("posters"))
}

// ----------------------------------------------------------------- scan ----

/// Scan every watched folder into the videos database.
///
/// The list is the same JSON file the Slint build keeps — one list, both
/// builds — and this is `tulipix_common::{load,add}_watched_folder` minus the
/// slint dependency that crate carries.
pub(crate) async fn scan_watched() {
    for dir in load_watched_folders() {
        scan_one(&dir).await;
    }
}

/// One folder, read on its own -- a watched root, from Settings' per-folder
/// Rescan as well as from the loop above.
pub(crate) async fn scan_one(dir: &std::path::Path) {
    let Ok(pool) = pool().await else { return };
    {
        emit(VideosEvent::ScanStarted { root: dir.to_string_lossy().into_owned() });
        let lib = tulipix_core::libraries::Library {
            id: dir.to_string_lossy().into_owned(),
            path: dir.to_path_buf(),
            section: tulipix_core::libraries::Section::Videos,
            last_scan: None,
            item_count: 0,
            size_bytes: 0,
            exclude_globs: crate::api::maintenance::exclusions(),
            cadence_override: Some(tulipix_core::libraries::ScanCadence::Manual),
            realtime_notify: false,
        };
        match tulipix_videos::scan::scan_library(pool, &lib).await {
            Ok(st) => {
                emit(VideosEvent::ScanFinished {
                    scanned: st.scanned as i64,
                    inserted: st.inserted as i64,
                    updated: st.updated as i64,
                    missing: st.missing as i64,
                });
                // Everything new needs a duration before the grid can print one,
                // and a season/episode tag before the TV tab can group it.
                ingest_new(pool).await;
            }
            Err(e) => emit(VideosEvent::Failed { message: e.to_string() }),
        }
    }
}

/// ffprobe whatever has no duration yet, then classify anything whose filename
/// carries an SxxEyy tag, then scrape what TMDB will answer for.
///
/// Bounded per pass: a first scan of a large library would otherwise sit here
/// probing thousands of files before the grid drew anything.
async fn ingest_new(pool: &sqlx::SqlitePool) {
    const BATCH: i64 = 200;
    let ids = tulipix_videos::scan::unprocessed_ids(pool, BATCH).await.unwrap_or_default();
    for id in ids {
        if let Err(e) = tulipix_videos::scan::ingest_meta(pool, id).await {
            tracing::debug!(error = %e, id, "ffprobe failed");
            continue;
        }
        let abs: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
        let Some(abs) = abs else { continue };
        classify_episode(pool, id, Path::new(&abs)).await;
        scrape_tmdb(pool, id, Path::new(&abs)).await;
        // TMDB is wrong about anime often enough to be worth a second look:
        // absolute episode numbering, romaji titles, OVAs folded into the
        // series. This runs only for what TMDB left blank, and only when the
        // AniList or AniDB switch is on.
        scrape_anime(pool, id, Path::new(&abs)).await;
    }
}

/// `S01E05`, `1x05`, `s1.e5` — the shapes a release actually uses.
///
/// Deliberately the same scanner the Slint build carries, character for
/// character: a file that groups into a show in one build and not the other is
/// a library that disagrees with itself.
pub(crate) fn parse_season_episode(name: &str) -> Option<(i64, i64)> {
    let lower = name.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut i = 0;
    while i < lb.len() {
        if lb[i] == b's' {
            let s0 = i + 1;
            let mut j = s0;
            while j < lb.len() && lb[j].is_ascii_digit() && j - s0 < 2 {
                j += 1;
            }
            if j == s0 {
                i += 1;
                continue;
            }
            let season: i64 = lower[s0..j].parse().ok()?;
            let mut k = j;
            while k < lb.len() && matches!(lb[k], b'.' | b' ' | b'_' | b'-') {
                k += 1;
            }
            if k < lb.len() && lb[k] == b'e' {
                let e0 = k + 1;
                let mut e = e0;
                while e < lb.len() && lb[e].is_ascii_digit() && e - e0 < 3 {
                    e += 1;
                }
                if e > e0 {
                    let episode: i64 = lower[e0..e].parse().ok()?;
                    return Some((season, episode));
                }
            }
        }
        i += 1;
    }
    None
}

/// Link a tagged file to the show named by its parent directory.
async fn classify_episode(pool: &sqlx::SqlitePool, item_id: i64, path: &Path) {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else { return };
    let Some((season, episode)) = parse_season_episode(name) else { return };
    let title = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .unwrap_or("Unknown Show")
        .to_string();
    let Some(show_id) = get_or_create_show(pool, &title).await else { return };
    let _ =
        tulipix_videos::episodes::upsert(pool, item_id, show_id, season, episode, None, None, None, None, None)
            .await;
}

async fn get_or_create_show(pool: &sqlx::SqlitePool, title: &str) -> Option<i64> {
    if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>("SELECT id FROM shows WHERE title = ?")
        .bind(title)
        .fetch_optional(pool)
        .await
    {
        return Some(id);
    }
    sqlx::query("INSERT INTO shows (title, updated) VALUES (?, ?)")
        .bind(title)
        .bind(now_secs())
        .execute(pool)
        .await
        .ok()?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM shows WHERE title = ?")
        .bind(title)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
}

/// Fetch metadata and a poster for one freshly-scanned file. A no-op without a
/// TMDB key, which is the ordinary state — the section works without one, it
/// just draws frames instead of artwork.
async fn scrape_tmdb(pool: &sqlx::SqlitePool, item_id: i64, path: &Path) {
    use tulipix_videos::tmdb::MetadataProvider as _;
    let Some(key) = tmdb_api_key() else { return };
    let Some(dir) = poster_cache_dir() else { return };
    let client = tmdb_client(key);

    let show_id: Option<i64> = sqlx::query_scalar("SELECT show_id FROM episodes WHERE item_id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();

    if let Some(show_id) = show_id {
        let cached: Option<String> =
            sqlx::query_scalar("SELECT poster_local FROM shows WHERE id = ?")
                .bind(show_id)
                .fetch_optional(pool)
                .await
                .ok()
                .flatten()
                .flatten();
        if cached.is_some() {
            return;
        }
        let title: Option<String> = sqlx::query_scalar("SELECT title FROM shows WHERE id = ?")
            .bind(show_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
        let Some(title) = title else { return };
        let Ok(Some(meta)) = client.search_show(&title).await else { return };
        let _ = sqlx::query(
            "UPDATE shows SET tmdb_id=?, tvdb_id=?, year=?, overview=?, \
             poster_path=?, backdrop_path=?, updated=? WHERE id=?",
        )
        .bind(meta.tmdb_id)
        .bind(meta.tvdb_id)
        .bind(meta.year)
        .bind(&meta.overview)
        .bind(&meta.poster_path)
        .bind(&meta.backdrop_path)
        .bind(now_secs())
        .bind(show_id)
        .execute(pool)
        .await;
        if let Some(pp) = &meta.poster_path {
            if let Ok(local) = client.cache_poster(pp, &dir).await {
                let _ = sqlx::query("UPDATE shows SET poster_local = ? WHERE id = ?")
                    .bind(local.to_string_lossy().as_ref())
                    .bind(show_id)
                    .execute(pool)
                    .await;
            }
        }
        return;
    }

    let cached: Option<String> =
        sqlx::query_scalar("SELECT poster_local FROM movies WHERE item_id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .flatten();
    if cached.is_some() {
        return;
    }
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parsed = tulipix_videos::agents::parse_filename(name);
    let Ok(Some(meta)) = client.search_movie(&parsed.title, parsed.year).await else { return };
    let _ = tulipix_videos::tmdb::upsert_movie(pool, item_id, &meta).await;
    if let Some(pp) = &meta.poster_path {
        if let Ok(local) = client.cache_poster(pp, &dir).await {
            let _ = sqlx::query("UPDATE movies SET poster_local = ? WHERE item_id = ?")
                .bind(local.to_string_lossy().as_ref())
                .bind(item_id)
                .execute(pool)
                .await;
        }
    }
}

/// The anime fallback, for what TMDB left blank.
///
/// TMDB knows anime under its English release title and numbers episodes by
/// season; a file called `[Group] Shingeki no Kyojin - 25.mkv` matches
/// nothing. AniList answers the romaji title with an overview and a poster,
/// and AniDB answers with the episode list AniList has not got. Both are off
/// until switched on in Settings, and neither runs for a file TMDB already
/// answered for.
async fn scrape_anime(pool: &sqlx::SqlitePool, item_id: i64, path: &Path) {
    use tulipix_videos::anime::AnimeProvider as _;

    let anilist = crate::services::anilist();
    let anidb = crate::services::anidb();
    if anilist.is_none() && anidb.is_none() {
        return;
    }
    // Already answered for, by TMDB or by an earlier pass.
    if has_artwork(pool, item_id).await {
        return;
    }
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parsed = tulipix_videos::agents::parse_filename(name);
    if parsed.title.trim().is_empty() {
        return;
    }

    // AniList first: one request, no registration, and it carries the poster.
    // AniDB second, for the episode list and as the fallback when AniList has
    // never heard of the title.
    let mut meta = match &anilist {
        Some(p) => p.search(&parsed.title).await.ok().flatten(),
        None => None,
    };
    let mut source = "anilist";
    let mut episodes = Vec::new();
    if let Some(db) = &anidb {
        match db.find_aid(&parsed.title).await {
            Ok(Some(aid)) => match db.anime(aid).await {
                Ok(Some((m, eps))) => {
                    episodes = eps;
                    match &mut meta {
                        // Both answered: keep AniList's prose and poster, take
                        // AniDB's id so the episode list has something to hang
                        // off.
                        Some(existing) => existing.anidb_id = m.anidb_id,
                        None => {
                            meta = Some(m);
                            source = "anidb";
                        }
                    }
                }
                Ok(None) => {}
                Err(e) => tracing::debug!(error = %e, "anidb: anime lookup"),
            },
            Ok(None) => {}
            Err(e) => tracing::debug!(error = %e, "anidb: title lookup"),
        }
    }
    let Some(meta) = meta else { return };

    if let Err(e) = tulipix_videos::anime::apply_schema(pool).await {
        tracing::warn!(error = %e, "anime: schema");
        return;
    }
    if let Err(e) = tulipix_videos::anime::upsert(pool, item_id, source, &meta).await {
        tracing::warn!(error = %e, "anime: upsert");
        return;
    }
    tracing::info!(item_id, source, title = %meta.title_romaji, episodes = episodes.len(), "anime metadata");

    // AniDB is the only one of the two with per-episode data. An anime file is
    // usually numbered absolutely, which is exactly the number AniDB indexes
    // by, so this fills in the title TMDB could not name. Only where the row
    // has none: a title already written came from somewhere better.
    for ep in &episodes {
        let _ = sqlx::query(
            "UPDATE episodes SET title = COALESCE(NULLIF(title, ''), ?), \
             overview = COALESCE(NULLIF(overview, ''), ?) \
             WHERE item_id = ? AND episode = ?",
        )
        .bind(&ep.title)
        .bind(&ep.overview)
        .bind(item_id)
        .bind(ep.episode)
        .execute(pool)
        .await;
    }

    // Then into the columns the grid actually draws from, so a poster shows
    // without every view learning about `anime_meta`.
    let title = meta
        .title_english
        .clone()
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| meta.title_romaji.clone());
    let poster_local = match (&meta.poster_url, poster_cache_dir()) {
        (Some(url), Some(dir)) => {
            tulipix_videos::tmdb::cache_image(tulipix_core::net::http(), url, &dir)
                .await
                .ok()
                .map(|p| p.to_string_lossy().into_owned())
        }
        _ => None,
    };
    let show_id: Option<i64> = sqlx::query_scalar("SELECT show_id FROM episodes WHERE item_id = ?")
        .bind(item_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    let _ = match show_id {
        Some(show_id) => {
            sqlx::query(
                "UPDATE shows SET year = COALESCE(year, ?), \
                 overview = COALESCE(NULLIF(overview, ''), ?), \
                 poster_local = COALESCE(poster_local, ?) WHERE id = ?",
            )
            .bind(meta.year)
            .bind(&meta.overview)
            .bind(&poster_local)
            .bind(show_id)
            .execute(pool)
            .await
        }
        None => {
            sqlx::query(
                "INSERT INTO movies (item_id, title, year, overview, poster_local, updated) \
                 VALUES (?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(item_id) DO UPDATE SET \
                    year = COALESCE(movies.year, excluded.year), \
                    overview = COALESCE(NULLIF(movies.overview, ''), excluded.overview), \
                    poster_local = COALESCE(movies.poster_local, excluded.poster_local), \
                    updated = excluded.updated",
            )
            .bind(item_id)
            .bind(&title)
            .bind(meta.year)
            .bind(&meta.overview)
            .bind(&poster_local)
            .bind(now_secs())
            .execute(pool)
            .await
        }
    };
}

/// Whether this item already has a poster on disk, through its show or as a
/// film. The one question the anime fallback asks before spending a request.
async fn has_artwork(pool: &sqlx::SqlitePool, item_id: i64) -> bool {
    let show: Option<Option<String>> = sqlx::query_scalar(
        "SELECT s.poster_local FROM episodes e JOIN shows s ON s.id = e.show_id \
         WHERE e.item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if show.flatten().is_some_and(|p| !p.is_empty()) {
        return true;
    }
    let movie: Option<Option<String>> =
        sqlx::query_scalar("SELECT poster_local FROM movies WHERE item_id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    movie.flatten().is_some_and(|p| !p.is_empty())
}

/// Tell Trakt this was watched.
///
/// Called wherever something becomes finished -- the player reaching the end,
/// and "Mark as watched" in the poster menu -- so the account matches the
/// library however it got there. A no-op unless Trakt is switched on and the
/// account is linked, and always detached: a network call has no business
/// holding up the window closing.
pub(crate) fn trakt_push_watched(item_id: i64) {
    let Some((client, token)) = crate::services::trakt() else { return };
    tokio::spawn(async move {
        let Ok(pool) = crate::db::videos_pool().await else { return };
        let Some(item) = trakt_history_item(pool, item_id).await else { return };
        match client.add_to_history(&token, &item).await {
            Ok(()) => tracing::info!(item_id, title = %item.title, "trakt: added to history"),
            Err(e) => tracing::warn!(item_id, error = %e, "trakt: could not add to history"),
        }
    });
}

/// What the library knows about an item, in the shape Trakt takes: an episode
/// when it is one, a film otherwise. `None` when the row names nothing Trakt
/// could match on.
async fn trakt_history_item(
    pool: &sqlx::SqlitePool,
    item_id: i64,
) -> Option<tulipix_videos::trakt::HistoryItem> {
    use tulipix_videos::trakt::{HistoryItem, Kind};
    let watched_at = tulipix_videos::trakt::iso8601_utc(now_secs());

    let episode: Option<(String, Option<i64>, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT s.title, s.year, s.tmdb_id, e.season, e.episode \
         FROM episodes e JOIN shows s ON s.id = e.show_id WHERE e.item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    if let Some((title, year, tmdb_id, season, episode)) = episode {
        if title.trim().is_empty() {
            return None;
        }
        return Some(HistoryItem {
            kind: Kind::Episode,
            title,
            year,
            tmdb_id,
            season: Some(season),
            episode: Some(episode),
            watched_at,
        });
    }

    let movie: Option<(String, Option<i64>, Option<i64>)> =
        sqlx::query_as("SELECT title, year, tmdb_id FROM movies WHERE item_id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    let (title, year, tmdb_id) = movie?;
    (!title.trim().is_empty()).then_some(HistoryItem {
        kind: Kind::Movie,
        title,
        year,
        tmdb_id,
        season: None,
        episode: None,
        watched_at,
    })
}

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

fn add_watched_folder(dir: &Path) -> bool {
    let mut existing = load_watched_folders();
    if existing.iter().any(|p| p == dir) {
        return false;
    }
    existing.push(dir.to_path_buf());
    let Some(p) = watched_path() else { return false };
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let list: Vec<String> = existing.iter().map(|p| p.to_string_lossy().into_owned()).collect();
    match serde_json::to_string_pretty(&list) {
        Ok(body) => std::fs::write(p, body).is_ok(),
        Err(e) => {
            tracing::warn!(error = %e, "serialise watched folders");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn season_and_episode_are_read_out_of_the_shapes_releases_actually_use() {
        assert_eq!(parse_season_episode("Show.S01E05.1080p.mkv"), Some((1, 5)));
        assert_eq!(parse_season_episode("show s2 e12.mp4"), Some((2, 12)));
        assert_eq!(parse_season_episode("Show_s01_e105.mkv"), Some((1, 105)));
        // A film is not an episode, and neither is a stray 's'.
        assert_eq!(parse_season_episode("Dune Part Two 2024.mkv"), None);
        assert_eq!(parse_season_episode("seasons.mkv"), None);
    }

    #[test]
    fn a_duration_is_only_printed_once_there_is_one() {
        assert_eq!(fmt_duration(6128.0), "1:42:08");
        assert_eq!(fmt_duration(95.0), "1:35");
        // An unprobed file reports nothing rather than "0:00".
        assert_eq!(fmt_duration(0.0), "");
        assert_eq!(fmt_duration(f64::NAN), "");
    }

    #[test]
    fn a_release_date_is_reformatted_only_when_it_is_one() {
        assert_eq!(fmt_release_date("2025-06-14"), "Jun 14, 2025");
        assert_eq!(fmt_release_date("2025-13-01"), "2025-13-01");
        assert_eq!(fmt_release_date(""), "");
    }

    #[test]
    fn the_four_remote_tabs_do_not_draw_the_local_grid() {
        for k in ["local", "tv", "movies"] {
            assert!(is_local_kind(k), "{k}");
        }
        for k in ["discover", "stream", "livetv", "splus"] {
            assert!(!is_local_kind(k), "{k}");
        }
    }
}
