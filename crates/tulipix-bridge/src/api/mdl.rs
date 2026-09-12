//! Downloader — `tulipix-mdl` at the Flutter boundary.
//!
//! Paste a streaming URL (Spotify, Apple, Amazon, YT Music, SoundCloud,
//! Bandcamp, Qobuz, Deezer, Tidal) or search, and the tracklist that comes back
//! becomes a queue: tick the rows you want, pick the primary artist where a
//! track credits several, then download. Each finished file is tagged from the
//! *provider's* metadata rather than YouTube's guess and ingested straight into
//! the library, so it appears under Songs/Albums/Artists without a rescan.
//!
//! # What this module owns and what it does not
//!
//! Every decision -- which provider a URL belongs to, how a page is scraped,
//! what yt-dlp is asked for, how the tags are written, what the manifest
//! records -- lives in `tulipix-mdl`. This file spawns that work, keeps the
//! queue, and turns progress into events.
//!
//! # Why the events are deltas, not snapshots
//!
//! A hundred-track download emits four stage transitions per track. Answering
//! each with a whole `MdlState` would push the entire queue across the bridge
//! four hundred times, and the queue is the largest thing in the state. So
//! progress arrives as [`MdlEvent::Row`] -- one index, one stage, one percent --
//! and Dart patches its copy. [`MdlEvent::Changed`] is reserved for the moments
//! where the shape itself moved (a resolve landed, the queue was cleared, a
//! history page loaded) and Dart re-reads the lot.
//!
//! The CLI log goes further and does not live here at all: Rust emits each line
//! as [`MdlEvent::Cli`] and Dart owns the buffer. The Slint build had to keep
//! it in Rust because a Slint model cannot be appended to from Dart -- there is
//! no Dart. Here, keeping 40 KB of scrollback in the snapshot would be paying
//! for a limitation the port does not have.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::sync::Arc;

use flutter_rust_bridge::frb;
use anyhow::Result;
use tulipix_mdl::types::{
    DownloadOptions, NameMethod, Playlist, Progress, ProviderId, Stage, Track,
};

use crate::db::music_pool;
use crate::frb_generated::StreamSink;

// ── the surface ─────────────────────────────────────────────────────────────

/// One track in the queue.
///
/// `art_url` crosses as a URL, not as decoded pixels. The Slint build had to
/// decode to RGBA in Rust because `slint::Image` is `!Send` and the rows live
/// in a cross-thread static; Flutter has `Image.network` with its own cache, so
/// the bytes never need to touch this side.
#[derive(Debug, Clone)]
pub struct MdlRow {
    pub title: String,
    /// "3:45" from provider metadata, empty when the provider did not say.
    pub length: String,
    pub album: String,
    /// Every credited artist — the per-row primary-artist menu.
    pub artists: Vec<String>,
    /// The one credited as primary. Ends up as `artists[0]` at download time,
    /// which is what the library groups by.
    pub main_artist: String,
    /// queued | searching | downloading | tagging | done | skipped | failed |
    /// in library
    pub stage: String,
    pub percent: f64,
    /// The produced filename once done; the failure reason once failed.
    pub file: String,
    pub selected: bool,
    pub art_url: String,
}

/// One row of the Downloaded log.
#[derive(Debug, Clone)]
pub struct MdlHistoryRow {
    pub title: String,
    pub artists: String,
    pub album: String,
    pub provider: String,
    /// "3h ago".
    pub when: String,
    pub path: String,
    /// The library id, so play goes through the same path as any other track
    /// and the player resolves cover art and the album page. -1 when the file
    /// is no longer in the library.
    pub item_id: i64,
}

/// One row of the Searches log.
#[derive(Debug, Clone)]
pub struct MdlSearchRow {
    pub url: String,
    /// track | album | playlist
    pub kind: String,
    pub title: String,
    pub provider: String,
    pub when: String,
}

#[derive(Debug, Clone, Default)]
pub struct MdlState {
    /// What is in the input field.
    pub url: String,
    /// url | search
    pub mode: String,
    /// YT Music | Spotify — which backend a search uses.
    pub search_provider: String,
    /// Provider display name for the current URL, empty when unsupported.
    /// This is what makes the badge appear as you type.
    pub provider_badge: String,
    pub dest: String,
    /// idle | resolving | resolved | downloading | done | error
    pub status: String,
    /// Set alongside `status = "error"`; never both.
    pub error: String,

    pub rows: Vec<MdlRow>,
    /// track | album | playlist — what the last resolve turned out to be.
    pub kind: String,
    pub title: String,
    pub selected: i64,
    /// Deduped union of every credited artist across the queue.
    pub all_artists: Vec<String>,
    /// "" | name | length | artist
    pub sort: String,
    /// 1 asc · 2 desc
    pub sort_dir: i64,

    pub done: i64,
    pub skipped: i64,
    pub failed: i64,

    /// opus | m4a | mp3 | flac | wav
    pub format: String,
    /// kbps; ignored for the lossless formats.
    pub bitrate: i64,
    pub name_method: String,
    /// Concurrent tracks, 1–4.
    pub parallel: i64,
    /// yt-dlp `--concurrent-fragments` per track, 1–8.
    pub threads: i64,

    /// A YouTube link that does not look like tagged music. Not an error — the
    /// resolve still worked — but worth saying before a video becomes a song.
    pub yt_warn: bool,

    pub history: Vec<MdlHistoryRow>,
    pub history_page: i64,
    pub history_pages: i64,
    pub searches: Vec<MdlSearchRow>,
    pub search_page: i64,
    pub search_pages: i64,
}

#[derive(Debug, Clone)]
pub enum MdlCmd {
    Refresh,
    SetUrl { url: String },
    /// url | search
    SetMode { mode: String },
    SetSearchProvider { name: String },
    Resolve,
    Search,
    /// A resolve is one opaque `await` inside a provider client — there is no
    /// flag to poll, so the only honest stop is aborting the task.
    CancelResolve,
    ClearAll,

    ToggleRow { index: i64 },
    SelectAll { all: bool },
    SetMainArtist { index: i64, artist: String },
    /// Every row that credits `artist` takes it as primary.
    BulkMainArtist { artist: String },
    /// name | length | artist. Re-sending the active key flips the direction.
    SortBy { key: String },

    SetDest { path: String },
    SetFormat { name: String },
    SetBitrate { kbps: i64 },
    SetNameMethod { label: String },
    SetParallel { n: i64 },
    SetThreads { n: i64 },

    Download,
    Cancel,
    /// Re-download one failed row with the last run's settings.
    Retry { index: i64 },

    LoadHistory { page: i64 },
    LoadSearches { page: i64 },
    ClearHistory,
    ClearSearches,
    /// Put a URL from the Searches log back in the box.
    UseSearch { url: String },
    RevealFile { path: String },
}

#[derive(Debug, Clone)]
pub enum MdlEvent {
    /// The snapshot moved in a way a delta cannot describe. Re-read it.
    Changed,
    /// One queue row advanced. `index` is into `MdlState.rows`.
    Row {
        index: i64,
        stage: String,
        percent: f64,
        file: String,
    },
    Counters { done: i64, skipped: i64, failed: i64 },
    Status { text: String },
    /// One line of the activity log. Dart owns the buffer — see the module note.
    Cli { line: String },
}

// ── session ─────────────────────────────────────────────────────────────────

/// Options and destination of the last run, so a per-row Retry rebuilds the
/// same pipeline for one track instead of guessing at it.
#[frb(ignore)]
#[derive(Clone)]
struct DlCtx {
    dest: PathBuf,
    format: String,
    bitrate: u32,
    method: NameMethod,
    threads: usize,
}

/// `frb(ignore)`: private state, not part of the contract. It also holds a
/// `Playlist`, a `tulipix-mdl` type frb cannot see.
#[frb(ignore)]
struct Session {
    ui: MdlState,
    /// The last resolve, kept so Download reuses it rather than going back to
    /// the provider, and so a sort never needs the network again.
    playlist: Option<Playlist>,
    /// What `playlist` was resolved from. Guards against a stale queue when the
    /// URL is edited and Download is pressed without re-resolving.
    resolved_url: String,
    /// `index_map[k]` is the queue row that worker track `k+1` came from. The
    /// worker only ever sees the selected, not-already-owned subset.
    index_map: Vec<usize>,
    last_ctx: Option<DlCtx>,
}

impl Session {
    fn new() -> Self {
        Session {
            ui: MdlState {
                mode: "url".into(),
                search_provider: "YT Music".into(),
                status: "idle".into(),
                dest: default_music_dir().to_string_lossy().into_owned(),
                format: "opus".into(),
                bitrate: 128,
                name_method: "Artist - Song".into(),
                parallel: 2,
                threads: 2,
                sort_dir: 1,
                history_pages: 1,
                search_pages: 1,
                ..Default::default()
            },
            playlist: None,
            resolved_url: String::new(),
            index_map: Vec::new(),
            last_ctx: None,
        }
    }
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Session::new()))
}

fn lock() -> MutexGuard<'static, Session> {
    session().lock().unwrap_or_else(|e| e.into_inner())
}

/// Every touch of the session goes through here. A `MutexGuard` merely in
/// lexical scope across an `.await` makes the whole future non-`Send`, and
/// frb's dispatcher requires `Send` — so the guard is confined to a closure
/// that cannot contain one.
fn with<R>(f: impl FnOnce(&mut Session) -> R) -> R {
    f(&mut lock())
}

fn events() -> &'static Mutex<Vec<StreamSink<MdlEvent>>> {
    static E: OnceLock<Mutex<Vec<StreamSink<MdlEvent>>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Vec::new()))
}

fn emit(event: MdlEvent) {
    if let Ok(mut sinks) = events().lock() {
        sinks.retain(|s| s.add(event.clone()).is_ok());
    }
}

fn cli(line: impl Into<String>) {
    emit(MdlEvent::Cli { line: line.into() });
}

fn set_status(text: &str) {
    with(|s| {
        s.ui.status = text.to_string();
        s.ui.error.clear();
    });
    emit(MdlEvent::Status { text: text.to_string() });
}

fn set_error(message: impl std::fmt::Display) {
    let message = message.to_string();
    cli(format!("  {message}"));
    with(|s| {
        s.ui.status = "error".into();
        s.ui.error = message;
    });
    emit(MdlEvent::Changed);
}

/// Cancels the in-flight download when set.
fn cancel_flag() -> &'static Arc<AtomicBool> {
    static C: OnceLock<Arc<AtomicBool>> = OnceLock::new();
    C.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// The in-flight resolve/search, so Cancel can drop it.
fn resolve_task() -> &'static Mutex<Option<tokio::task::AbortHandle>> {
    static R: OnceLock<Mutex<Option<tokio::task::AbortHandle>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(None))
}

/// Bumped on every resolve so a slow art fetch from a previous URL can notice
/// it has been superseded and stop writing into the current queue.
fn art_gen() -> &'static AtomicU64 {
    static G: OnceLock<AtomicU64> = OnceLock::new();
    G.get_or_init(|| AtomicU64::new(0))
}

// ── exported ────────────────────────────────────────────────────────────────

pub fn mdl_events(sink: StreamSink<MdlEvent>) {
    if let Ok(mut sinks) = events().lock() {
        sinks.push(sink);
    }
}

/// Apply one command and return the resulting snapshot.
pub async fn mdl_dispatch(cmd: MdlCmd) -> Result<MdlState> {
    match cmd {
        MdlCmd::Refresh => {}

        MdlCmd::SetUrl { url } => with(|s| {
            s.ui.provider_badge = detect_badge(&url);
            s.ui.url = url;
        }),
        MdlCmd::SetMode { mode } => with(|s| s.ui.mode = mode),
        MdlCmd::SetSearchProvider { name } => with(|s| s.ui.search_provider = name),

        MdlCmd::Resolve => start_resolve(),
        MdlCmd::Search => start_search(),
        MdlCmd::CancelResolve => {
            if let Some(h) = resolve_task().lock().ok().and_then(|mut g| g.take()) {
                h.abort();
            }
            // Supersede any art fetch the aborted resolve already started.
            art_gen().fetch_add(1, Ordering::Relaxed);
            cli("  cancelled");
            set_status("idle");
        }
        MdlCmd::ClearAll => clear_all(),

        MdlCmd::ToggleRow { index } => with(|s| {
            if let Some(r) = s.ui.rows.get_mut(index.max(0) as usize) {
                r.selected = !r.selected;
            }
            recount(s);
        }),
        MdlCmd::SelectAll { all } => with(|s| {
            for r in s.ui.rows.iter_mut() {
                r.selected = all;
            }
            recount(s);
        }),
        MdlCmd::SetMainArtist { index, artist } => with(|s| {
            if let Some(r) = s.ui.rows.get_mut(index.max(0) as usize) {
                if r.artists.iter().any(|a| a == &artist) {
                    r.main_artist = artist;
                }
            }
        }),
        MdlCmd::BulkMainArtist { artist } => with(|s| {
            for r in s.ui.rows.iter_mut() {
                if r.artists.iter().any(|a| a == &artist) {
                    r.main_artist = artist.clone();
                }
            }
        }),
        MdlCmd::SortBy { key } => with(|s| sort_queue(s, &key)),

        MdlCmd::SetDest { path } => with(|s| s.ui.dest = path),
        MdlCmd::SetFormat { name } => with(|s| s.ui.format = name),
        MdlCmd::SetBitrate { kbps } => with(|s| s.ui.bitrate = kbps.clamp(0, 320)),
        MdlCmd::SetNameMethod { label } => with(|s| s.ui.name_method = label),
        MdlCmd::SetParallel { n } => with(|s| s.ui.parallel = n.clamp(1, 4)),
        MdlCmd::SetThreads { n } => with(|s| s.ui.threads = n.clamp(1, 8)),

        MdlCmd::Download => start_download(),
        MdlCmd::Cancel => {
            cancel_flag().store(true, Ordering::Relaxed);
            cli("  cancelling — tracks already in flight will finish");
        }
        MdlCmd::Retry { index } => start_retry(index.max(0) as usize),

        MdlCmd::LoadHistory { page } => load_history(page).await,
        MdlCmd::LoadSearches { page } => load_searches(page).await,
        MdlCmd::ClearHistory => {
            if let Ok(pool) = music_pool().await {
                let _ = tulipix_music::dl_history::clear_history(pool).await;
            }
            load_history(0).await;
        }
        MdlCmd::ClearSearches => {
            if let Ok(pool) = music_pool().await {
                let _ = tulipix_music::dl_history::clear_searches(pool).await;
            }
            load_searches(0).await;
        }
        MdlCmd::UseSearch { url } => with(|s| {
            s.ui.provider_badge = detect_badge(&url);
            s.ui.url = url;
            s.ui.mode = "url".into();
        }),
        MdlCmd::RevealFile { path } => {
            if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(Path::new(&path)) {
                set_error(e);
            }
        }
    }
    Ok(with(|s| s.ui.clone()))
}

/// The formats the picker offers, and the providers a URL can name. Exported so
/// the two sides cannot disagree about a list neither of them owns.
#[frb(sync)]
pub fn mdl_formats() -> Vec<String> {
    ["opus", "m4a", "mp3", "flac", "wav"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// The providers that can be searched by name, as opposed to the nine a URL
/// can name. Exported for the same reason the format list is: the picker had
/// this hard-coded, and a hard-coded list is one that goes stale the first time
/// a provider grows a search.
#[frb(sync)]
pub fn mdl_search_providers() -> Vec<String> {
    ["YT Music", "Spotify", "Apple", "Deezer", "Bandcamp"]
        .iter()
        .map(|s| s.to_string())
        .collect()
}

#[frb(sync)]
pub fn mdl_name_methods() -> Vec<String> {
    [
        "Artist - Song",
        "Artists - Song",
        "Album - Song",
        "## - Artist - Song",
        "Song",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Every provider the resolver understands, for the "what can I paste here"
/// line under the URL field.
#[frb(sync)]
pub fn mdl_providers() -> Vec<String> {
    [
        ProviderId::Spotify,
        ProviderId::AppleMusic,
        ProviderId::AmazonMusic,
        ProviderId::YoutubeMusic,
        ProviderId::Soundcloud,
        ProviderId::Bandcamp,
        ProviderId::Qobuz,
        ProviderId::Deezer,
        ProviderId::Tidal,
    ]
    .iter()
    .map(|p| p.display_name().to_string())
    .collect()
}

// ── resolve and search ──────────────────────────────────────────────────────

fn detect_badge(url: &str) -> String {
    tulipix_mdl::detect_provider(url)
        .map(|p| p.display_name().to_string())
        .unwrap_or_default()
}

/// Resolve the URL in the box and show the tracklist, everything ticked.
fn start_resolve() {
    let url = with(|s| s.ui.url.trim().to_string());
    if url.is_empty() {
        set_error("Paste a music URL first.");
        return;
    }
    set_status("resolving");
    with(|s| s.ui.yt_warn = false);
    cli(format!("▸ resolving {url}"));

    let task = tokio::spawn(async move {
        let client = tulipix_core::net::http().clone();
        match tulipix_mdl::resolve_url(&client, &url).await {
            Ok(pl) => {
                cli(format!(
                    "  resolved: {} — {} track(s) [{}]",
                    pl.title,
                    pl.tracks.len(),
                    pl.provider.display_name()
                ));
                // A YouTube link that carries no album tag and did not come
                // from music.youtube.com is probably a video, not a song. Still
                // resolved — just said out loud before it becomes a library
                // entry with a 16:9 thumbnail for a cover.
                let yt_warn = pl.provider == ProviderId::YoutubeMusic
                    && !url.contains("music.youtube.com")
                    && !pl.tracks.iter().any(|t| t.album.is_some());
                accept(pl, url, yt_warn).await;
            }
            Err(e) => set_error(e),
        }
    });
    if let Ok(mut g) = resolve_task().lock() {
        *g = Some(task.abort_handle());
    }
}

/// Search mode. Results land in the same queue as a resolved URL, so tagging,
/// selection and download are one pipeline rather than two.
fn start_search() {
    let (query, provider) = with(|s| (s.ui.url.trim().to_string(), s.ui.search_provider.clone()));
    if query.is_empty() {
        set_error("Type something to search for.");
        return;
    }
    set_status("resolving");
    with(|s| s.ui.yt_warn = false);

    let task = tokio::spawn(async move {
        let client = tulipix_core::net::http().clone();
        let result = match provider.as_str() {
            "Spotify" => {
                cli(format!("▸ searching Spotify: {query}"));
                match tulipix_mdl::search_spotify(&client, &query, SEARCH_HITS).await {
                    Ok(pl) => Ok(pl),
                    Err(e) => {
                        // The anonymous web-player token is not always given
                        // out. Falling back beats telling someone their search
                        // failed.
                        cli(format!(
                            "  Spotify search failed ({e}) — falling back to YouTube Music"
                        ));
                        tulipix_mdl::search_ytmusic(&query, SEARCH_HITS).await
                    }
                }
            }
            // Keyless and unauthenticated, so there is nothing to fall back
            // from: a failure here is that catalogue's failure, and saying so
            // is more use than quietly answering with somebody else's.
            "Deezer" => {
                cli(format!("▸ searching Deezer: {query}"));
                tulipix_mdl::search_deezer(&client, &query, SEARCH_HITS).await
            }
            "Apple" => {
                cli(format!("▸ searching Apple: {query}"));
                tulipix_mdl::search_apple(&client, &query, SEARCH_HITS).await
            }
            "Bandcamp" => {
                cli(format!("▸ searching Bandcamp: {query}"));
                tulipix_mdl::search_bandcamp(&client, &query, SEARCH_HITS).await
            }
            _ => {
                cli(format!("▸ searching YouTube Music: {query}"));
                tulipix_mdl::search_ytmusic(&query, SEARCH_HITS).await
            }
        };
        match result {
            Ok(pl) => {
                cli(format!("  found {} result(s)", pl.tracks.len()));
                accept(pl, query, false).await;
            }
            Err(e) => set_error(e),
        }
    });
    if let Ok(mut g) = resolve_task().lock() {
        *g = Some(task.abort_handle());
    }
}

const SEARCH_HITS: usize = 12;

/// Seat a freshly resolved playlist as the queue. `source` is the URL (or the
/// query, in search mode) that produced it — Download compares against it to
/// decide whether the cached playlist is still the right one.
async fn accept(pl: Playlist, source: String, yt_warn: bool) {
    let kind = resolve_kind(&pl).to_string();
    with(|s| {
        s.ui.rows = seed_rows(&pl);
        s.ui.kind = kind.clone();
        s.ui.title = pl.title.clone();
        s.ui.yt_warn = yt_warn;
        s.ui.done = 0;
        s.ui.skipped = 0;
        s.ui.failed = 0;
        s.playlist = Some(pl.clone());
        s.resolved_url = source.clone();
        // A fresh queue opens sorted by artist, A to Z. Through `sort_queue`,
        // after the playlist is seated, so the cached tracks are reordered by
        // the same permutation and every row index still names its own track.
        s.ui.sort.clear();
        sort_queue(s, "artist");
        recount(s);
    });
    set_status("resolved");
    emit(MdlEvent::Changed);
    record_search(&pl, source).await;
}

fn seed_rows(pl: &Playlist) -> Vec<MdlRow> {
    // A single track and an album legitimately share one cover; a playlist
    // mixes releases, so each track's own art is used where the provider gave
    // one and the collection cover fills in where it did not.
    let fallback = pl.artwork_url.clone().unwrap_or_default();
    pl.tracks
        .iter()
        .map(|t| MdlRow {
            title: t.title.clone(),
            length: fmt_len(t.duration_ms),
            album: t.album.clone().unwrap_or_default(),
            artists: t.artists.clone(),
            main_artist: t.artists.first().cloned().unwrap_or_default(),
            stage: "queued".into(),
            percent: 0.0,
            file: String::new(),
            selected: true,
            art_url: t.artwork_url.clone().unwrap_or_else(|| fallback.clone()),
        })
        .collect()
}

/// Albums resolve as playlists, so the URL is what separates them.
fn resolve_kind(pl: &Playlist) -> &'static str {
    if pl.tracks.len() == 1 {
        "track"
    } else if pl.source_url.contains("/album") {
        "album"
    } else {
        "playlist"
    }
}

fn fmt_len(ms: Option<u64>) -> String {
    match ms {
        Some(ms) if ms > 0 => {
            let s = ms / 1000;
            format!("{}:{:02}", s / 60, s % 60)
        }
        _ => String::new(),
    }
}

/// Recompute the two figures derived from the rows: how many are ticked, and
/// the deduped artist union that fills the bulk primary-artist menu.
fn recount(s: &mut Session) {
    s.ui.selected = s.ui.rows.iter().filter(|r| r.selected).count() as i64;
    let mut union: Vec<String> = Vec::new();
    for r in &s.ui.rows {
        for a in &r.artists {
            if !union.iter().any(|u| u == a) {
                union.push(a.clone());
            }
        }
    }
    s.ui.all_artists = union;
}

async fn record_search(pl: &Playlist, url: String) {
    let Ok(pool) = music_pool().await else { return };
    let _ = tulipix_music::dl_history::record_search(
        pool,
        &url,
        resolve_kind(pl),
        Some(pl.title.as_str()),
        Some(pl.provider.display_name()),
    )
    .await;
}

fn clear_all() {
    // Supersede any in-flight art fetch before the rows it would write into go.
    art_gen().fetch_add(1, Ordering::Relaxed);
    with(|s| {
        s.ui.rows.clear();
        s.ui.all_artists.clear();
        s.ui.kind.clear();
        s.ui.title.clear();
        s.ui.sort.clear();
        s.ui.sort_dir = 1;
        s.ui.selected = 0;
        s.ui.done = 0;
        s.ui.skipped = 0;
        s.ui.failed = 0;
        s.ui.yt_warn = false;
        s.ui.status = "idle".into();
        s.ui.error.clear();
        s.playlist = None;
        s.resolved_url.clear();
        s.index_map.clear();
    });
    emit(MdlEvent::Changed);
}

/// Sort by `key`, flipping direction when the active key is re-sent. Reorders
/// the cached playlist by the same permutation, because every per-row action
/// downstream — download, retry, primary artist — addresses a track by its row
/// index, and a queue sorted out from under those indices is a bug that only
/// shows up on the wrong file appearing on disk.
fn sort_queue(s: &mut Session, key: &str) {
    let dir = if s.ui.sort == key {
        if s.ui.sort_dir == 1 { 2 } else { 1 }
    } else {
        1
    };
    let n = s.ui.rows.len();
    let durations: Vec<u64> = (0..n)
        .map(|i| {
            s.playlist
                .as_ref()
                .and_then(|p| p.tracks.get(i))
                .and_then(|t| t.duration_ms)
                .unwrap_or(0)
        })
        .collect();

    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&a, &b| {
        let ord = match key {
            "length" => durations[a].cmp(&durations[b]),
            "artist" => s.ui.rows[a]
                .main_artist
                .to_lowercase()
                .cmp(&s.ui.rows[b].main_artist.to_lowercase()),
            _ => s.ui.rows[a]
                .title
                .to_lowercase()
                .cmp(&s.ui.rows[b].title.to_lowercase()),
        };
        if dir == 2 { ord.reverse() } else { ord }
    });

    s.ui.rows = idx.iter().map(|&i| s.ui.rows[i].clone()).collect();
    if let Some(pl) = s.playlist.as_mut() {
        let reordered: Vec<Track> = idx.iter().filter_map(|&i| pl.tracks.get(i).cloned()).collect();
        if reordered.len() == pl.tracks.len() {
            pl.tracks = reordered;
        }
    }
    s.ui.sort = key.to_string();
    s.ui.sort_dir = dir;
}

// ── download ────────────────────────────────────────────────────────────────

/// The system Music folder — where downloads land unless told otherwise.
fn default_music_dir() -> PathBuf {
    std::env::var_os("XDG_MUSIC_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs_home().map(|h| h.join("Music")))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn dirs_home() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Initializing => "queued",
        Stage::SearchingYoutube => "searching",
        Stage::DownloadingAudio => "downloading",
        Stage::WritingMetadata => "tagging",
        Stage::WritingManifest => "saving",
        Stage::Skipped => "skipped",
        Stage::Failed => "failed",
        Stage::Completed => "done",
    }
}

/// Reorder `track.artists` so `main` is first. The library credits
/// `artists[0]` only — the full list stays in the file's own tags.
fn apply_main(track: &mut Track, main: &str) {
    if main.is_empty() {
        return;
    }
    if let Some(pos) = track.artists.iter().position(|a| a == main) {
        if pos != 0 {
            let a = track.artists.remove(pos);
            track.artists.insert(0, a);
        }
    }
}

fn options_from(s: &Session, dest: PathBuf, parallelism: usize) -> DownloadOptions {
    DownloadOptions {
        dest_dir: dest,
        parallelism,
        threads_per_download: s.ui.threads.clamp(1, 8) as usize,
        format: s.ui.format.clone(),
        bitrate: s.ui.bitrate.clamp(0, 320) as u32,
        name_method: NameMethod::from_label(&s.ui.name_method),
    }
}

/// Download the ticked rows. Reuses the resolved playlist when the URL has not
/// changed since; resolves first when it has.
fn start_download() {
    cancel_flag().store(false, Ordering::Relaxed);
    let (url, dest, parallel) = with(|s| {
        (
            s.ui.url.trim().to_string(),
            PathBuf::from(&s.ui.dest),
            s.ui.parallel.clamp(1, 4) as usize,
        )
    });
    if dest.as_os_str().is_empty() {
        set_error("Pick a folder to download into.");
        return;
    }
    // Both the library root and the chosen destination, so a download outside
    // the usual folder is still watched and survives a rescan.
    crate::api::music::add_watched_folder(&default_music_dir());
    crate::api::music::add_watched_folder(&dest);

    let opts = with(|s| options_from(s, dest.clone(), parallel));
    cli(format!("$ mdl download → {}", dest.display()));
    cli(format!(
        "  format={} bitrate={}k name=\"{}\" parallel={} threads={}",
        opts.format,
        opts.bitrate,
        with(|s| s.ui.name_method.clone()),
        opts.parallelism,
        opts.threads_per_download
    ));
    set_status("resolving");

    tokio::spawn(async move {
        // The cached queue only stands in for a fresh resolve while the URL it
        // came from is still the one in the box.
        let cached = with(|s| (s.resolved_url == url).then(|| s.playlist.clone()).flatten());
        let playlist = match cached {
            Some(pl) => pl,
            None => {
                let client = tulipix_core::net::http().clone();
                match tulipix_mdl::resolve_url(&client, &url).await {
                    Ok(pl) => {
                        accept(pl.clone(), url.clone(), false).await;
                        pl
                    }
                    Err(e) => {
                        set_error(e);
                        return;
                    }
                }
            }
        };

        // Ticked rows only, with each row's chosen primary artist applied.
        let picked: Vec<(usize, Track)> = with(|s| {
            playlist
                .tracks
                .iter()
                .enumerate()
                .filter(|(i, _)| s.ui.rows.get(*i).map(|r| r.selected).unwrap_or(true))
                .map(|(i, t)| {
                    let mut t = t.clone();
                    let main = s.ui.rows.get(i).map(|r| r.main_artist.clone()).unwrap_or_default();
                    apply_main(&mut t, &main);
                    (i, t)
                })
                .collect()
        });
        if picked.is_empty() {
            set_status("nothing selected");
            return;
        }

        // Anything already in the library is marked and never fetched again.
        // This is the check that makes re-running a playlist cheap.
        let pool = music_pool().await.ok();
        let mut keep: Vec<(usize, Track)> = Vec::new();
        let mut owned = 0i64;
        for (row, track) in picked {
            let main = track.artists.first().cloned().unwrap_or_default();
            let in_library = match pool {
                Some(p) => {
                    tulipix_music::dl_history::track_in_library(p, &track.title, &main).await
                }
                None => false,
            };
            if in_library {
                owned += 1;
                mark_row(row, "in library", 100.0, "Already in library");
                cli(format!("- in library » {}", track.title));
            } else {
                keep.push((row, track));
            }
        }
        if keep.is_empty() {
            with(|s| s.ui.skipped = owned);
            emit(MdlEvent::Counters { done: 0, skipped: owned, failed: 0 });
            set_status("all in library");
            return;
        }

        let filtered = Playlist {
            tracks: keep.iter().map(|(_, t)| t.clone()).collect(),
            ..playlist.clone()
        };
        let rows: Vec<usize> = keep.iter().map(|(i, _)| *i).collect();
        let tracks: Vec<Track> = keep.into_iter().map(|(_, t)| t).collect();
        let provider = playlist.provider.display_name().to_string();
        run(filtered, rows, tracks, provider, opts, owned).await;
    });
}

/// Re-download one failed row using the last run's destination and settings.
fn start_retry(row: usize) {
    let Some(ctx) = with(|s| s.last_ctx.clone()) else {
        set_error("Nothing to retry — run a download first.");
        return;
    };
    let Some((playlist, mut track, main)) = with(|s| {
        let pl = s.playlist.clone()?;
        let t = pl.tracks.get(row).cloned()?;
        let main = s.ui.rows.get(row).map(|r| r.main_artist.clone()).unwrap_or_default();
        Some((pl, t, main))
    }) else {
        return;
    };
    apply_main(&mut track, &main);
    mark_row(row, "queued", 0.0, "");
    cli(format!("↻ retry ▸ {}", track.title));

    let provider = playlist.provider.display_name().to_string();
    let opts = DownloadOptions {
        dest_dir: ctx.dest,
        parallelism: 1,
        threads_per_download: ctx.threads,
        format: ctx.format,
        bitrate: ctx.bitrate,
        name_method: ctx.method,
    };
    let filtered = Playlist { tracks: vec![track.clone()], ..playlist };
    cancel_flag().store(false, Ordering::Relaxed);
    tokio::spawn(async move {
        run(filtered, vec![row], vec![track], provider, opts, 0).await;
    });
}

fn mark_row(row: usize, stage: &str, percent: f64, file: &str) {
    with(|s| {
        if let Some(r) = s.ui.rows.get_mut(row) {
            r.stage = stage.to_string();
            r.percent = percent;
            r.file = file.to_string();
        }
    });
    emit(MdlEvent::Row {
        index: row as i64,
        stage: stage.to_string(),
        percent,
        file: file.to_string(),
    });
}

/// Drive one `download_playlist` run. Shared by a full download and a Retry, so
/// there is one place that knows how progress becomes rows.
///
/// `rows[k]` is the queue row that worker track `k+1` came from; `tracks[k]` is
/// that track's metadata, primary artist first, for the library ingest.
async fn run(
    filtered: Playlist,
    rows: Vec<usize>,
    tracks: Vec<Track>,
    provider: String,
    opts: DownloadOptions,
    base_skipped: i64,
) {
    with(|s| {
        s.index_map = rows.clone();
        s.last_ctx = Some(DlCtx {
            dest: opts.dest_dir.clone(),
            format: opts.format.clone(),
            bitrate: opts.bitrate,
            method: opts.name_method,
            threads: opts.threads_per_download,
        });
    });
    set_status("downloading");

    let client = tulipix_core::net::http_stream().clone();
    let dest = opts.dest_dir.clone();
    let finished = std::sync::Arc::new(Mutex::new(Vec::<(Track, String)>::new()));
    let collect = finished.clone();

    let on_progress = move |p: Progress| {
        let k = p.track_index.saturating_sub(1);
        if let Some(&row) = rows.get(k) {
            let file = match p.stage {
                // Keep the stage a clean single word so the pill and the Retry
                // button can match on it; the reason goes in the detail line.
                Stage::Failed => p.message.chars().take(FAIL_CHARS).collect(),
                _ => p.file_name.clone().unwrap_or_default(),
            };
            mark_row(row, stage_label(p.stage), p.percent as f64, &file);
        }

        let line = match p.stage {
            Stage::SearchingYoutube => {
                format!("[{}/{}] search  ▸ {}", p.track_index, p.total, p.title)
            }
            Stage::WritingMetadata => {
                format!("[{}/{}] tag     · {}", p.track_index, p.total, p.title)
            }
            Stage::Completed => format!(
                "[{}/{}] done    ✓ {}",
                p.track_index,
                p.total,
                p.file_name.clone().unwrap_or_default()
            ),
            Stage::Failed => format!(
                "[{}/{}] FAILED  ✗ {} — {}",
                p.track_index, p.total, p.title, p.message
            ),
            Stage::Skipped => format!("[{}/{}] skip    » {}", p.track_index, p.total, p.title),
            _ => String::new(),
        };
        if !line.is_empty() {
            cli(line);
        }

        // Ingest is async and this callback is not, so finished files are
        // queued here and swept once the run ends. The alternative is spawning
        // a task per track, which races the manifest write.
        if matches!(p.stage, Stage::Completed) {
            if let (Some(file), Some(track)) = (p.file_name.as_ref(), tracks.get(k)) {
                if let Ok(mut g) = collect.lock() {
                    g.push((track.clone(), file.clone()));
                }
            }
        }

        let (done, skipped, failed) = (
            p.downloaded as i64,
            p.skipped as i64 + base_skipped,
            p.failed as i64,
        );
        with(|s| {
            s.ui.done = done;
            s.ui.skipped = skipped;
            s.ui.failed = failed;
        });
        emit(MdlEvent::Counters { done, skipped, failed });
    };

    let summary = tulipix_mdl::download::download_playlist(
        &client,
        &filtered,
        &opts,
        cancel_flag().clone(),
        on_progress,
    )
    .await;

    let landed = finished.lock().map(|g| g.clone()).unwrap_or_default();
    for (track, file) in landed {
        ingest(&track, &dest.join(&file), &provider).await;
    }

    with(|s| {
        s.ui.done = summary.downloaded as i64;
        s.ui.skipped = summary.skipped as i64 + base_skipped;
        s.ui.failed = summary.failed.len() as i64;
    });
    emit(MdlEvent::Counters {
        done: summary.downloaded as i64,
        skipped: summary.skipped as i64 + base_skipped,
        failed: summary.failed.len() as i64,
    });
    cli(format!(
        "  {} downloaded · {} skipped · {} failed",
        summary.downloaded,
        summary.skipped as i64 + base_skipped,
        summary.failed.len()
    ));
    set_status("done");
    emit(MdlEvent::Changed);
}

/// How much of a failure message rides in the row. The full text is in the log.
const FAIL_CHARS: usize = 80;

/// Put a finished file into the library without waiting for a rescan, and log
/// it to the Downloaded history.
async fn ingest(track: &Track, path: &Path, provider: &str) {
    let Ok(pool) = music_pool().await else { return };
    let abs = path.to_string_lossy().to_string();
    if let Err(e) = tulipix_music::dl_history::ingest_downloaded_track(
        pool,
        &abs,
        &track.title,
        &track.artists,
        track.album.as_deref(),
        track.duration_ms,
    )
    .await
    {
        tracing::warn!(error = %e, "mdl: direct ingest failed");
    }
    let _ = tulipix_music::dl_history::record_download(
        pool,
        &track.title,
        &track.artists.join(", "),
        track.album.as_deref(),
        Some(provider),
        &abs,
    )
    .await;
    // Render the embedded cover now so the new tile and the player show real
    // art immediately rather than after a restart.
    let _ = tulipix_core::thumbs::render_or_cache(
        path,
        tulipix_core::thumbs::ThumbSpec {
            kind: tulipix_core::thumbs::ThumbKind::Audio,
            width: THUMB,
            height: THUMB,
        },
    );
}

const THUMB: u32 = 320;

// ── history ─────────────────────────────────────────────────────────────────

fn fmt_when(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(ts);
    match (now - ts).max(0) {
        0..=59 => "just now".into(),
        d @ 60..=3599 => format!("{}m ago", d / 60),
        d @ 3600..=86399 => format!("{}h ago", d / 3600),
        d => format!("{}d ago", d / 86400),
    }
}

async fn load_history(page: i64) {
    let Ok(pool) = music_pool().await else { return };
    let (rows, pages) = tulipix_music::dl_history::history_page(pool, page)
        .await
        .unwrap_or_default();
    let mut out = Vec::with_capacity(rows.len());
    for r in &rows {
        // The library id, so play goes through the ordinary path and the player
        // can resolve cover art and the album page. A file deleted since the
        // download has no row left, and -1 disables Play rather than failing it.
        let item_id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(&r.abs_path)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten()
            .unwrap_or(-1);
        out.push(MdlHistoryRow {
            title: r.title.clone(),
            artists: r.artists.clone(),
            album: r.album.clone().unwrap_or_default(),
            provider: r.provider.clone().unwrap_or_default(),
            when: fmt_when(r.downloaded_at),
            path: r.abs_path.clone(),
            item_id,
        });
    }
    with(|s| {
        s.ui.history = out;
        s.ui.history_pages = pages.max(1);
        s.ui.history_page = page.clamp(0, (pages - 1).max(0));
    });
    emit(MdlEvent::Changed);
}

async fn load_searches(page: i64) {
    let Ok(pool) = music_pool().await else { return };
    let (rows, pages) = tulipix_music::dl_history::searches_page(pool, page)
        .await
        .unwrap_or_default();
    with(|s| {
        s.ui.searches = rows
            .iter()
            .map(|r| MdlSearchRow {
                url: r.url.clone(),
                kind: r.kind.clone(),
                title: r.title.clone().unwrap_or_default(),
                provider: r.provider.clone().unwrap_or_default(),
                when: fmt_when(r.searched_at),
            })
            .collect();
        s.ui.search_pages = pages.max(1);
        s.ui.search_page = page.clamp(0, (pages - 1).max(0));
    });
    emit(MdlEvent::Changed);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str, artists: &[&str], ms: Option<u64>) -> Track {
        Track {
            id: title.into(),
            title: title.into(),
            artists: artists.iter().map(|a| a.to_string()).collect(),
            album: Some("Alb".into()),
            artwork_url: None,
            duration_ms: ms,
            source_url: None,
        }
    }

    fn playlist(tracks: Vec<Track>, url: &str) -> Playlist {
        Playlist {
            id: "p".into(),
            title: "P".into(),
            owner: None,
            artwork_url: Some("http://art".into()),
            provider: ProviderId::Spotify,
            source_url: url.into(),
            tracks,
        }
    }

    #[test]
    fn seeds_every_row_selected_with_a_primary_artist() {
        let pl = playlist(vec![track("A", &["X", "Y"], Some(185_000))], "u");
        let rows = seed_rows(&pl);
        assert_eq!(rows.len(), 1);
        assert!(rows[0].selected);
        assert_eq!(rows[0].main_artist, "X");
        assert_eq!(rows[0].length, "3:05");
        // No per-track art, so the collection cover fills in.
        assert_eq!(rows[0].art_url, "http://art");
    }

    #[test]
    fn kind_separates_track_album_playlist() {
        assert_eq!(resolve_kind(&playlist(vec![track("A", &["X"], None)], "u")), "track");
        let two = vec![track("A", &["X"], None), track("B", &["Y"], None)];
        assert_eq!(resolve_kind(&playlist(two.clone(), "s/album/1")), "album");
        assert_eq!(resolve_kind(&playlist(two, "s/playlist/1")), "playlist");
    }

    #[test]
    fn primary_artist_moves_to_the_front() {
        let mut t = track("A", &["X", "Y", "Z"], None);
        apply_main(&mut t, "Z");
        assert_eq!(t.artists, vec!["Z", "X", "Y"]);
        // Not credited: left alone rather than inserted.
        apply_main(&mut t, "Q");
        assert_eq!(t.artists, vec!["Z", "X", "Y"]);
    }

    #[test]
    fn sort_reorders_rows_and_tracks_together() {
        let pl = playlist(
            vec![
                track("Beta", &["B"], Some(2000)),
                track("Alpha", &["A"], Some(1000)),
            ],
            "s/playlist/1",
        );
        let mut s = Session::new();
        s.ui.rows = seed_rows(&pl);
        s.playlist = Some(pl);

        sort_queue(&mut s, "name");
        assert_eq!(s.ui.rows[0].title, "Alpha");
        // The cached tracks moved with them — a per-row index still addresses
        // the same song on both sides.
        assert_eq!(s.playlist.as_ref().unwrap().tracks[0].title, "Alpha");
        assert_eq!(s.ui.sort_dir, 1);

        // Re-sending the active key flips it.
        sort_queue(&mut s, "name");
        assert_eq!(s.ui.rows[0].title, "Beta");
        assert_eq!(s.ui.sort_dir, 2);
    }

    #[test]
    fn recount_dedupes_the_artist_union() {
        let pl = playlist(
            vec![track("A", &["X", "Y"], None), track("B", &["Y", "Z"], None)],
            "u",
        );
        let mut s = Session::new();
        s.ui.rows = seed_rows(&pl);
        s.ui.rows[1].selected = false;
        recount(&mut s);
        assert_eq!(s.ui.selected, 1);
        assert_eq!(s.ui.all_artists, vec!["X", "Y", "Z"]);
    }

    #[test]
    fn badge_names_the_provider_and_stays_empty_otherwise() {
        assert_eq!(
            detect_badge("https://open.spotify.com/album/1DFixLWuPkv3KT3TnV35m3"),
            "Spotify"
        );
        assert_eq!(detect_badge("https://example.com/x"), "");
    }
}
