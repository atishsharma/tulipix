//! The Stream tab: a remote catalogue, its detail pane, and playback.
//!
//! The typed backend is `tulipix_videos::stream` and none of it is repeated
//! here. What this file is, is the half that used to live in
//! `tulipix_sec_videos::stream` — the shared session, the action epoch, the
//! poster cache and the handlers — rewritten against the bridge's session
//! instead of a Slint window, because that crate depends on slint and nothing
//! the bridge links may.
//!
//! Playback hands the resolved URL to the same windowed mpv the local library
//! uses, with progress written back to `stream_progress`: a remote title has no
//! row in the videos DB for the local `watch_progress` to key off.
//!
//! Not outside `api/`: `rust_input: crate::api` never scans this module, so the
//! domain types it holds — `Details`, `StreamFile`, `Caption` — never have to
//! be Dart-facing. `api::videos` maps the handful of fields that do cross.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use anyhow::Result;
use tulipix_videos::stream::{
    self, Caption, Catalogue, Details, Source, StreamClient, StreamError, StreamFile,
};

use crate::api::videos::{
    self as v, StreamCard, StreamChip, StreamDownloadRow, StreamEpisode, StreamFileRow,
    StreamHistoryRow, StreamHostHealth, StreamView, VideosCmd, VideosEvent, with,
};

/// What the catalogue returns per page.
const PAGE_LEN: usize = 20;
/// The resume slider holds this many of the most recent unfinished titles.
const RESUME_LEN: usize = 5;
/// Six picks under the search box — the landing screen is a search page first.
const TRENDING_LEN: usize = 6;
/// How many vertical dramas the left-hand slider cycles through.
const VERTICAL_LEN: usize = 10;
/// Rows per page of the History list, and how far back it goes at all.
const HISTORY_PAGE: usize = 25;
const HISTORY_MAX: i64 = 500;
/// Rows the Downloads page lists.
const LEDGER_LIMIT: i64 = 200;
/// Saved shows checked for new episodes in one pass. `due_for_check` can return
/// forty; forty sequential detail fetches on tab open is a lot of traffic for a
/// badge, and whatever is left is still due next time.
const REFRESH_BATCH: usize = 8;

/// Cache keys for the two landing lists.
const KIND_TRENDING: &str = "trending";
const KIND_VERTICAL: &str = "vertical";

// ---------------------------------------------------------------- state ----

/// Everything the tab remembers between commands, none of which crosses to
/// Dart. Eleven separate statics is what this was in the Slint build, and the
/// mismatches between them were real bugs — a subject id from one field used
/// with a season from another.
pub(crate) struct StreamSession {
    /// The subject id the open title was fetched with.
    ///
    /// Resource lookups must use *this*, not `Details::id`: the detail payload's
    /// own id is frequently absent, and an empty `subjectId=` is a 400.
    pub open_id: String,
    pub details: Option<Details>,
    /// `(season, episode)`. `(0, 0)` is a film — no episode axis.
    pub selection: (i64, i64),
    /// `(season number, subject id)` for a show the catalogue splits across one
    /// subject per season. Empty when the subject carries its own seasons.
    pub season_subjects: Vec<(i64, String)>,
    /// Every subject of a split show maps to that show's full season list, so
    /// opening any season still knows about its siblings.
    pub season_map: std::collections::HashMap<String, Vec<(i64, String)>>,
    pub files: Vec<StreamFile>,
    pub subs: Vec<Caption>,
    pub sub_choice: Option<usize>,
    /// The language *name*, kept apart from the index: the next episode may
    /// list its tracks in a different order.
    pub sub_lang: Option<String>,
    pub resolution: String,
    pub preview_cache: std::collections::HashMap<String, Details>,
    pub current: usize,
    pub query: String,
    pub page: usize,
    pub results: Vec<CardData>,
    pub episodes: Vec<stream::EpisodeInfo>,
    pub watched: std::collections::HashMap<(i64, i64), f32>,
    /// A request to open a title *at a particular episode and start playing* —
    /// History's Play button. Keyed by subject, so a request that never resolves
    /// cannot fire against whatever is opened next.
    pub pending_play: Option<(String, i64, i64)>,
    /// Control endpoint of the cast session in progress, so Stop can reach it.
    pub cast_session: Option<String>,
    pub cast_targets: Vec<tulipix_music::cast::CastDevice>,
}

impl Default for StreamSession {
    fn default() -> Self {
        Self {
            open_id: String::new(),
            details: None,
            selection: (0, 0),
            season_subjects: Vec::new(),
            season_map: std::collections::HashMap::new(),
            files: Vec::new(),
            subs: Vec::new(),
            sub_choice: None,
            sub_lang: Some("English".to_string()),
            // The quality filter is remembered across restarts, so the tab
            // opens on the rung the user last chose rather than back at "all".
            resolution: stream::quality::filter(),
            preview_cache: std::collections::HashMap::new(),
            current: 0,
            query: String::new(),
            page: 0,
            results: Vec::new(),
            episodes: Vec::new(),
            watched: std::collections::HashMap::new(),
            pending_play: None,
            cast_session: None,
            cast_targets: Vec::new(),
        }
    }
}

/// One card, in a form the async side can build before anything is painted.
#[derive(Debug, Clone, Default)]
pub(crate) struct CardData {
    pub id: String,
    pub title: String,
    /// Second line: the year, or "Season 2 · Episode 4" on a resumable card.
    pub line: String,
    /// Resume cards only: "18 min left".
    pub meta: String,
    /// Resume cards only: "34% watched".
    pub note: String,
    pub poster: Option<PathBuf>,
    pub is_series: bool,
    pub seasons: i64,
    pub progress: f64,
}

impl CardData {
    fn of(hit: &stream::SearchHit, poster: Option<PathBuf>) -> Self {
        Self {
            id: hit.id.clone(),
            title: hit.title.clone(),
            line: hit.year.clone(),
            meta: String::new(),
            note: String::new(),
            poster,
            is_series: hit.is_series,
            seasons: hit.season_subjects.len() as i64,
            progress: 0.0,
        }
    }

    fn card(&self) -> StreamCard {
        StreamCard {
            id: self.id.clone(),
            title: self.title.clone(),
            year: self.line.clone(),
            poster: path_str(self.poster.clone()),
            is_series: self.is_series,
            seasons: self.seasons,
            meta: self.meta.clone(),
            overview: self.note.clone(),
            progress: self.progress,
        }
    }
}

fn cards_of(list: &[CardData]) -> Vec<StreamCard> {
    list.iter().map(CardData::card).collect()
}

pub(crate) fn path_str(p: Option<PathBuf>) -> String {
    p.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The persisted preferences the tab opens with.
pub(crate) fn init_view(view: &mut StreamView) {
    view.resolution = stream::quality::filter();
    view.source = stream::source::load().key().to_string();
    view.sub_scale = stream::subs::scale() as f64;
    view.sub_delay = stream::subs::delay() as f64;
    view.autoplay = stream::autoplay::enabled();
    view.to_library = library_dest();
    view.sub_choice = -1;
    view.current = -1;
    view.history_pages = 1;
    view.bm_sort = "date".into();
    view.fourk_default = stream::fourk::DEFAULT_BASE.to_string();
}

// ---------------------------------------------------------- epoch guards ----

/// Bumped by every user action. Detail and stream fetches overlap — click
/// Episode 5 then Episode 7 and both are in flight — so whichever *finishes*
/// last used to win and paint its streams under the other's highlight.
static EPOCH: AtomicU64 = AtomicU64::new(0);

fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_current(epoch: u64) -> bool {
    EPOCH.load(Ordering::SeqCst) == epoch
}

/// The landing row's own generation, kept apart from [`EPOCH`].
///
/// The two guard unrelated things: `EPOCH` is "the newest thing the user
/// clicked", this is a background refresh. Sharing one counter meant a refresh
/// silently abandoned an in-flight detail load, and the autoplay chain abandoned
/// the refresh right back.
static FEED_EPOCH: AtomicU64 = AtomicU64::new(0);

fn next_feed_epoch() -> u64 {
    FEED_EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_current_feed(epoch: u64) -> bool {
    FEED_EPOCH.load(Ordering::SeqCst) == epoch
}

// ---------------------------------------------------------------- client ----

/// Built once and reused; dropped whenever the host list changes.
fn client_cell() -> &'static Mutex<Option<StreamClient>> {
    static C: OnceLock<Mutex<Option<StreamClient>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// Reuse the live client, or build one over the configured hosts and warm it up
/// so later requests carry a session token.
///
/// A `tokio::sync::OnceCell` would be tidier, but the cell has to be resettable
/// when the host list changes — so this keeps the plain lock and accepts that
/// two simultaneous first calls may both build one. The loser is dropped.
async fn client() -> Result<StreamClient, StreamError> {
    if let Some(c) = client_cell().lock().ok().and_then(|g| g.clone()) {
        return Ok(c);
    }
    // Install any persisted signing key before the first request signs anything.
    stream::sign_key::apply();
    let c = StreamClient::new(stream::hosts::load())?;
    c.init().await?;
    if let Ok(mut g) = client_cell().lock() {
        if g.is_none() {
            *g = Some(c.clone());
        }
        return Ok(g.clone().unwrap_or(c));
    }
    Ok(c)
}

fn invalidate_client() {
    if let Ok(mut g) = client_cell().lock() {
        *g = None;
    }
}

/// Which source is selected. Read from settings once, then kept in memory —
/// this is on the path of every search and every episode click.
fn source_cell() -> &'static Mutex<Option<Source>> {
    static S: OnceLock<Mutex<Option<Source>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn active_source() -> Source {
    if let Ok(mut g) = source_cell().lock() {
        return *g.get_or_insert_with(stream::source::load);
    }
    Source::default()
}

fn set_active_source(s: Source) {
    if let Ok(mut g) = source_cell().lock() {
        *g = Some(s);
    }
    stream::source::store(s);
}

/// The catalogue the tab is searching right now.
///
/// MovieBox reuses the warmed client; 4KHDHub is stateless — no session, no
/// token — so it is built per call, which is a `reqwest::Client` clone and not
/// a connection.
async fn catalogue() -> Result<Catalogue, StreamError> {
    match active_source() {
        Source::MovieBox => Ok(Catalogue::MovieBox(client().await?)),
        Source::FourK => Ok(Catalogue::FourK(stream::fourk::FourKClient::new()?)),
    }
}

/// A file's URL, resolved into something a player, a downloader or a cast
/// target can open. A no-op on MovieBox; on 4KHDHub it follows the resolver
/// chain, done at the moment the URL is used because the links it mints are
/// short-lived and resolving twenty of them to paint a list would spend twenty
/// chains to use one.
async fn playable_url(url: &str) -> Result<String, StreamError> {
    catalogue().await?.playable(url).await
}

/// Plain-language failure text — users see these, not the `Debug` form.
fn explain(e: &StreamError) -> String {
    match e {
        StreamError::NoHosts => "No servers configured — open Hosts and add one.".into(),
        StreamError::HostsExhausted => {
            "No server answered. Check your connection or edit Hosts.".into()
        }
        StreamError::MissingToken => {
            "Servers answered but refused a session. Try different Hosts.".into()
        }
        StreamError::ApiStatus(404) => "Nothing found for that.".into(),
        StreamError::ApiStatus(s) => format!("Servers returned {s}. Try again or edit Hosts."),
        StreamError::Reqwest(_) => "Network error.".into(),
        StreamError::Json(_) => "Unreadable response from the server.".into(),
    }
}

fn set_status(msg: impl Into<String>, busy: bool) {
    let msg = msg.into();
    with(|s| {
        s.ui.stream.status = msg;
        s.ui.stream.busy = busy;
    });
}

// ----------------------------------------------------------- poster cache ----

fn cover_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("videos").join("stream"))
}

/// Extensions a cached poster can have. The decoder picks its format from the
/// file extension, so a mislabelled file is a decode error, not a picture.
const COVER_EXTS: [&str; 3] = ["jpg", "png", "webp"];

/// What these bytes actually are, by their magic number — the catalogue serves
/// PNG and WebP from URLs that end in `.jpg`.
fn image_ext(bytes: &[u8]) -> &'static str {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => "png",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "webp",
        _ => "jpg",
    }
}

/// Rename a cached image whose extension disagrees with its own header, and
/// return the path to use. Anything unreadable is left exactly as it is.
async fn repair_ext(path: PathBuf, ext: &str) -> PathBuf {
    use tokio::io::AsyncReadExt;
    let Ok(mut f) = tokio::fs::File::open(&path).await else { return path };
    let mut head = [0u8; 12];
    let Ok(n) = f.read(&mut head).await else { return path };
    let actual = image_ext(&head[..n]);
    if actual == ext {
        return path;
    }
    let fixed = path.with_extension(actual);
    match tokio::fs::rename(&path, &fixed).await {
        Ok(()) => fixed,
        Err(_) => path,
    }
}

/// Download a remote poster into the cache, keyed by a hash of its URL, and
/// return the local path. Already-cached files are reused untouched.
///
/// Shared with Live TV's channel logos and every Stream Plus shelf, deliberately:
/// a second cache would be the clearest sign that this is four apps in a trench
/// coat rather than one section.
pub(crate) async fn cache_cover(url: &str) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }
    let dir = cover_dir()?;
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let stem = h.finalize().iter().map(|b| format!("{b:02x}")).collect::<String>();
    for ext in COVER_EXTS {
        let path = dir.join(format!("{stem}.{ext}"));
        if path.exists() {
            return Some(repair_ext(path, ext).await);
        }
    }
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let bytes = tulipix_core::net::http()
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    let path = dir.join(format!("{stem}.{}", image_ext(&bytes)));
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// Cache a batch of poster URLs concurrently, preserving input order.
pub(crate) async fn cache_covers(urls: Vec<String>) -> Vec<Option<PathBuf>> {
    let tasks: Vec<_> = urls
        .into_iter()
        .map(|u| tokio::spawn(async move { cache_cover(&u).await }))
        .collect();
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(t.await.ok().flatten());
    }
    out
}

/// Keep at most `max` files in `dir`, dropping the oldest first.
async fn cap_dir(dir: &std::path::Path, max: usize) {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let modified = e
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        entries.push((modified, e.path()));
    }
    if entries.len() <= max {
        return;
    }
    entries.sort_by_key(|(t, _)| *t);
    for (_, path) in entries.iter().take(entries.len() - max) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

// ------------------------------------------------------- small formatters ----

/// "2024 · Drama · USA · TV-MA · 50 min · ★ 8.7" — blank fields drop out.
fn meta_line(d: &Details) -> String {
    let mut parts: Vec<String> = Vec::new();
    for f in [&d.year, &d.genre, &d.country, &d.content_rating, &d.duration] {
        if !f.is_empty() {
            parts.push(f.clone());
        }
    }
    if d.rating > 0.0 {
        parts.push(format!("★ {:.1}", d.rating));
    }
    parts.join("  ·  ")
}

/// "2.1 GB · h265" for a row subtitle; either half may be missing.
fn file_sub(f: &StreamFile) -> String {
    [f.size.as_str(), f.codec.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// First `max` characters of `s`, with an ellipsis when it was cut. Counted by
/// `char`, so it never splits a multibyte glyph.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// One subtitle entry per language across every file of an episode, keeping the
/// first URL seen for each. Languages compare case-insensitively.
fn caption_union(files: &[StreamFile]) -> Vec<Caption> {
    let mut out: Vec<Caption> = Vec::new();
    for f in files {
        for c in &f.captions {
            if c.url.is_empty() {
                continue;
            }
            let dup = out.iter().any(|o| {
                (!c.lang.is_empty() && o.lang.eq_ignore_ascii_case(&c.lang)) || o.url == c.url
            });
            if !dup {
                out.push(c.clone());
            }
        }
    }
    out
}

/// "https://api5.aoneroom.com" → "api5.aoneroom.com", for a narrow row.
fn short_host(host: &str) -> String {
    host.split_once("://").map(|(_, rest)| rest).unwrap_or(host).to_string()
}

/// Is the library the download target? Anything but an explicit `downloads`
/// means yes, so the useful behaviour is what a fresh install gets.
fn library_dest() -> bool {
    tulipix_core::settings::Settings::load()
        .unwrap_or_default()
        .text("stream.dl_dest")
        .trim()
        != "downloads"
}

// -------------------------------------------------------------- dispatch ----

/// Handle one command, or report that it belongs to another page.
pub(crate) async fn dispatch(cmd: &VideosCmd) -> Result<bool> {
    match cmd {
        VideosCmd::StreamSearch { query } => search(query.clone()).await,
        VideosCmd::StreamSearchMore => {
            let (query, page) = with(|s| (s.st.query.clone(), s.st.page));
            if !query.is_empty() && page > 0 {
                run_search(query, page + 1).await;
            }
        }
        VideosCmd::StreamHome => home().await,
        VideosCmd::StreamOpen { id } => open(id.clone()).await,
        VideosCmd::StreamOpenResume { id } => open_resume(id.clone()).await,
        VideosCmd::StreamOpenPick { id } => {
            // Both landing lists are built from MovieBox's feed whatever the
            // provider button says, so their ids only mean something there.
            if !id.is_empty() {
                open_against(id.clone(), Source::MovieBox).await;
            }
        }
        VideosCmd::StreamBack => back(),
        VideosCmd::StreamPlay { index } => play(*index).await,
        VideosCmd::StreamSetDub { id } => set_dub(id.clone()).await,
        VideosCmd::StreamSetSeason { season } => set_season(*season).await,
        VideosCmd::StreamSetEpisode { episode } => set_episode(*episode).await,
        VideosCmd::StreamSetSub { index } => set_sub(*index),
        VideosCmd::StreamSetResolution { res } => set_resolution(res.clone()).await,
        VideosCmd::StreamSuggest { query } => suggest(query.clone()).await,
        VideosCmd::StreamSuggestClear => suggest_clear(),
        VideosCmd::StreamPreview { id } => preview(id.clone()).await,
        VideosCmd::StreamDownload { index } => download(*index).await,
        VideosCmd::StreamDownloadCancel => {
            DL_CANCEL.store(true, Ordering::SeqCst);
            set_status("Cancelling…", false);
        }
        VideosCmd::StreamDownloadDismiss => with(|s| {
            s.ui.stream.dl_label.clear();
            s.ui.stream.dl_active = false;
            s.ui.stream.dl_frac = 0.0;
        }),
        VideosCmd::StreamSetCurrent { index } => set_current(*index),
        VideosCmd::StreamPlayCurrent => {
            let i = current_index();
            if i >= 0 {
                play(i).await;
            }
        }
        VideosCmd::StreamDownloadCurrent => {
            let i = current_index();
            if i >= 0 {
                download(i).await;
            }
        }
        VideosCmd::StreamDownloadSeason => download_season().await,
        VideosCmd::StreamTrailer => trailer(),
        VideosCmd::StreamToggleBookmark => toggle_bookmark().await,
        VideosCmd::StreamBookmarksLoad => bookmarks_load().await,
        VideosCmd::StreamBookmarkRemove { id } => {
            if let Ok(pool) = v::pool().await {
                let _ = stream::bookmarks::remove(pool, id).await;
            }
            bookmarks_load().await;
        }
        VideosCmd::StreamBookmarksSort { key, asc } => {
            with(|s| {
                s.ui.stream.bm_sort = key.clone();
                s.ui.stream.bm_asc = *asc;
            });
            bookmarks_load().await;
        }
        VideosCmd::StreamRecentClear => {
            let list = stream::recent::clear();
            with(|s| s.ui.stream.recent = list);
        }
        VideosCmd::StreamFeedLoad => feed_load().await,
        VideosCmd::StreamHostsLoad => hosts_load(),
        VideosCmd::StreamHostsSave { text } => hosts_save(text.clone()),
        VideosCmd::StreamHostsReset => {
            let text = stream::hosts::defaults().join("\n");
            with(|s| {
                s.ui.stream.hosts_text = text;
                s.ui.stream.hosts_error = "Press Save to apply the default servers.".into();
            });
        }
        VideosCmd::StreamHostsCheck { text } => hosts_check(text.clone()).await,
        VideosCmd::StreamKeySave { key } => key_save(key.clone()),
        VideosCmd::StreamKeyReset => {
            let text = stream::sign_key::default();
            with(|s| {
                s.ui.stream.key_text = text;
                s.ui.stream.key_error = "Press Save to apply the default key.".into();
            });
        }
        VideosCmd::StreamSetSource { name } => set_source(name.clone()),
        VideosCmd::StreamSourceLoad => {
            let base = stream::fourk::base();
            // Shown as a placeholder rather than as text when it is the built-in
            // one, so "unset" stays visibly different from "pinned to today's".
            let stored = if base == stream::fourk::DEFAULT_BASE { String::new() } else { base };
            with(|s| {
                s.ui.stream.fourk_base = stored;
                s.ui.stream.fourk_default = stream::fourk::DEFAULT_BASE.to_string();
                s.ui.stream.fourk_error.clear();
            });
        }
        VideosCmd::StreamFourkSave { url } => match stream::fourk::set_base(url) {
            Ok(()) => {
                let now = stream::fourk::base();
                with(|s| {
                    s.ui.stream.fourk_error.clear();
                    s.ui.stream.status = format!("4KHDHub address set to {now}");
                });
            }
            Err(msg) => with(|s| s.ui.stream.fourk_error = msg),
        },
        VideosCmd::StreamFourkReset => {
            let msg = match stream::fourk::set_base("") {
                Ok(()) => "Back to the built-in 4KHDHub address.".to_string(),
                Err(e) => format!("Could not save: {e}"),
            };
            with(|s| {
                s.ui.stream.fourk_base.clear();
                s.ui.stream.fourk_error.clear();
                s.ui.stream.status = msg;
            });
        }
        VideosCmd::StreamDownloadsLoad => downloads_load().await,
        VideosCmd::StreamDownloadsResume => downloads_resume().await,
        VideosCmd::StreamDownloadPlay { id } => download_play(*id).await,
        VideosCmd::StreamDownloadRetry { id } => {
            if let Ok(pool) = v::pool().await {
                let _ =
                    stream::downloads::set_state(pool, *id, stream::downloads::State::Queued, "")
                        .await;
            }
            downloads_load().await;
            start_runner();
        }
        VideosCmd::StreamDownloadForget { id } => {
            if let Ok(pool) = v::pool().await {
                // Removing the job that is running also stops it.
                if let Some(job) = stream::downloads::get(pool, *id).await {
                    if job.state() == stream::downloads::State::Running {
                        DL_CANCEL.store(true, Ordering::SeqCst);
                    }
                }
                let _ = stream::downloads::remove(pool, *id).await;
            }
            downloads_load().await;
        }
        VideosCmd::StreamDownloadDeleteFile { id } => {
            if let Ok(pool) = v::pool().await {
                if let Some(job) = stream::downloads::get(pool, *id).await {
                    let dest = PathBuf::from(&job.dest);
                    let _ = tokio::fs::remove_file(&dest).await;
                    // The subtitle saved with it is part of the download, not a
                    // file the user put there — it goes too.
                    for ext in SUB_EXTS {
                        let _ = tokio::fs::remove_file(dest.with_extension(ext)).await;
                    }
                }
                let _ = stream::downloads::remove(pool, *id).await;
            }
            downloads_load().await;
        }
        VideosCmd::StreamDownloadReveal { id } => download_reveal(*id).await,
        VideosCmd::StreamDownloadsClear => {
            if let Ok(pool) = v::pool().await {
                let _ = stream::downloads::clear_finished(pool).await;
            }
            downloads_load().await;
        }
        VideosCmd::StreamDownloadsCancelAll => {
            DL_CANCEL.store(true, Ordering::SeqCst);
            if let Ok(pool) = v::pool().await {
                let _ = stream::downloads::cancel_waiting(pool).await;
            }
            downloads_load().await;
        }
        VideosCmd::StreamHistoryLoad { page } => history_load(*page).await,
        VideosCmd::StreamHistoryClear => {
            if let Ok(pool) = v::pool().await {
                let _ = stream::progress::clear(pool).await;
            }
            history_load(0).await;
            feed_load().await;
        }
        VideosCmd::StreamHistoryRemove { id, page } => {
            if let Ok(pool) = v::pool().await {
                let _ = stream::progress::remove(pool, id).await;
            }
            history_load(*page).await;
        }
        VideosCmd::StreamHistoryPlay { id, season, episode } => {
            // The episode has to travel with the request: opening a title
            // otherwise lands on episode 1, so Play on an "S02E04" row used to
            // start the season opener.
            with(|s| {
                s.st.pending_play = Some((id.clone(), (*season).max(0), (*episode).max(0)))
            });
            open(id.clone()).await;
        }
        VideosCmd::StreamCastDiscover => cast_discover().await,
        VideosCmd::StreamCastTo { device } => cast_to(device.clone()).await,
        VideosCmd::StreamCastStop => cast_stop().await,
        VideosCmd::StreamSetSubScale { value } => {
            stream::subs::set_scale(*value as f32);
            let applied = stream::subs::scale() as f64;
            with(|s| s.ui.stream.sub_scale = applied);
        }
        VideosCmd::StreamSetSubDelay { value } => {
            stream::subs::set_delay(*value as f32);
            let applied = stream::subs::delay() as f64;
            with(|s| s.ui.stream.sub_delay = applied);
        }
        VideosCmd::StreamSetAutoplay { on } => {
            stream::autoplay::set(*on);
            with(|s| s.ui.stream.autoplay = *on);
        }
        VideosCmd::StreamSetToLibrary { on } => {
            let mut st = tulipix_core::settings::Settings::load().unwrap_or_default();
            st.advanced.insert(
                "stream.dl_dest".to_string(),
                if *on { "library" } else { "downloads" }.to_string(),
            );
            let _ = st.save();
            with(|s| s.ui.stream.to_library = *on);
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Everything the tab does on entry: the local half first, then the network.
///
/// Search is deliberately not among them. It is search-driven — no catalogue
/// request goes out until the user asks for something.
pub(crate) async fn enter() {
    let recent = stream::recent::load();
    let (res, scale, delay, autoplay) = (
        stream::quality::filter(),
        stream::subs::scale() as f64,
        stream::subs::delay() as f64,
        stream::autoplay::enabled(),
    );
    let to_library = library_dest();
    with(|s| {
        s.ui.stream.recent = recent;
        s.ui.stream.resolution = res.clone();
        s.st.resolution = res;
        s.ui.stream.sub_scale = scale;
        s.ui.stream.sub_delay = delay;
        s.ui.stream.autoplay = autoplay;
        s.ui.stream.to_library = to_library;
        s.ui.stream.source = active_source().key().to_string();
    });
    feed_load().await;
    downloads_resume().await;
    bookmarks_refresh();
    prune_caches();
}

/// Trim the on-disk caches. The stream table ages out past a week, and the
/// poster and subtitle directories are capped by file count so they cannot grow
/// without bound.
fn prune_caches() {
    tokio::spawn(async move {
        if let Ok(pool) = v::pool().await {
            let _ = stream::cache::prune(pool, 7 * 24 * 60 * 60).await;
        }
        if let Some(base) = tulipix_core::paths::cache_dir().map(|d| d.join("videos")) {
            cap_dir(&base.join("stream"), 400).await;
            cap_dir(&base.join("stream-subs"), 60).await;
        }
    });
}

// ---------------------------------------------------------------- search ----

async fn search(query: String) {
    let query = query.trim().to_string();
    // A search always returns to the results view — close any open detail so it
    // does not stay layered over them.
    clear_open_title();
    with(|s| {
        s.ui.stream.detail_open = false;
        s.st.query = query.clone();
        s.st.page = 0;
        s.st.results.clear();
    });
    if query.is_empty() {
        with(|s| {
            s.ui.stream.results.clear();
            s.ui.stream.more = false;
            s.ui.stream.status.clear();
            s.ui.stream.busy = false;
            s.ui.stream.query.clear();
        });
        return;
    }
    with(|s| s.ui.stream.query = query.clone());
    run_search(query, 1).await;
}

/// Back to the landing screen from anywhere in the section.
async fn home() {
    next_epoch(); // abandon any search or detail still in flight
    clear_open_title();
    suggest_clear();
    with(|s| {
        s.st.query.clear();
        s.st.page = 0;
        s.st.results.clear();
        s.ui.stream.detail_open = false;
        s.ui.stream.results.clear();
        s.ui.stream.more = false;
        s.ui.stream.status.clear();
        s.ui.stream.busy = false;
        s.ui.stream.query.clear();
    });
    feed_load().await;
}

/// Page 1 replaces the grid; later pages append to it.
async fn run_search(query: String, page: usize) {
    let epoch = next_epoch();
    set_status(if page > 1 { "Loading more…" } else { "Searching…" }, true);

    let c = match catalogue().await {
        Ok(c) => c,
        Err(e) => return set_status(explain(&e), false),
    };
    // Remember the term only once a server has actually been reached, so a dead
    // host list does not fill the landing screen with junk.
    if page == 1 {
        let list = stream::recent::push(&query);
        with(|s| s.ui.stream.recent = list);
    }
    let hits = match c.search(&query, page).await {
        Ok(h) => h,
        Err(e) => return set_status(explain(&e), false),
    };
    // Search "dune" then "loki" quickly and the slower answer must not win.
    if !is_current(epoch) {
        return;
    }
    if hits.is_empty() && page == 1 {
        with(|s| {
            s.st.results.clear();
            s.ui.stream.results.clear();
            s.ui.stream.more = false;
        });
        return set_status("Nothing found.", false);
    }

    remember_season_maps(&hits);
    // A page that came back short is the last one. Folding split seasons and
    // duplicate dubs shrinks a full page well below `PAGE_LEN`, so the test is
    // deliberately generous — one dead "load more" is a smaller sin than hiding
    // half the catalogue.
    let more = hits.len() >= PAGE_LEN / 2;
    let covers = cache_covers(hits.iter().map(|h| h.cover.clone()).collect()).await;
    let host = c.active_host().await;
    if !is_current(epoch) {
        return; // poster caching is slow; re-check before painting
    }

    let fresh: Vec<CardData> =
        hits.iter().zip(covers).map(|(h, poster)| CardData::of(h, poster)).collect();

    let count = with(|s| {
        if page == 1 {
            s.st.results = fresh.clone();
        } else {
            // The catalogue repeats itself across pages more than it should.
            for card in &fresh {
                if !s.st.results.iter().any(|c| c.id == card.id) {
                    s.st.results.push(card.clone());
                }
            }
        }
        s.st.page = page;
        s.ui.stream.results = cards_of(&s.st.results);
        s.ui.stream.more = more;
        s.ui.stream.results.len()
    });
    set_status(format!("{count} results · {host}"), false);
}

/// Record the season lists that came back with a search, keyed by every subject
/// in each list.
///
/// Entries accumulate rather than replacing the previous search's: this map is
/// what tells an open title which season it is, and the next page of results
/// used to wipe the entry under it, leaving a five-season show looking like a
/// one-season one.
fn remember_season_maps(hits: &[stream::SearchHit]) {
    with(|s| {
        for hit in hits.iter().filter(|h| h.season_subjects.len() > 1) {
            for (_, id) in &hit.season_subjects {
                s.st.season_map.insert(id.clone(), hit.season_subjects.clone());
            }
        }
    })
}

fn siblings_of(subject_id: &str) -> Vec<(i64, String)> {
    with(|s| s.st.season_map.get(subject_id).cloned().unwrap_or_default())
}

fn clear_open_title() {
    with(|s| {
        s.st.open_id.clear();
        s.st.details = None;
        s.st.season_subjects.clear();
        s.st.files.clear();
        s.st.subs.clear();
        s.st.sub_choice = None;
    });
}

fn opened_subject() -> Option<String> {
    with(|s| (!s.st.open_id.is_empty()).then(|| s.st.open_id.clone()))
}

// ----------------------------------------------------------- suggestions ----

/// Debounced autocomplete. Each keystroke bumps its own counter; only the last
/// one still standing after the pause actually asks the servers.
static SUGGEST_SEQ: AtomicU64 = AtomicU64::new(0);

async fn suggest(query: String) {
    let query = query.trim().to_string();
    if query.len() < 2 {
        return with(|s| s.ui.stream.suggestions.clear());
    }
    let seq = SUGGEST_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    if SUGGEST_SEQ.load(Ordering::SeqCst) != seq {
        return; // superseded by a later keystroke
    }
    let Ok(c) = catalogue().await else { return };
    let Ok(names) = c.suggest(&query).await else { return };
    if SUGGEST_SEQ.load(Ordering::SeqCst) != seq {
        return;
    }
    with(|s| s.ui.stream.suggestions = names);
}

fn suggest_clear() {
    SUGGEST_SEQ.fetch_add(1, Ordering::SeqCst);
    with(|s| s.ui.stream.suggestions.clear());
}

// --------------------------------------------------------- hover preview ----

static PREVIEW_SEQ: AtomicU64 = AtomicU64::new(0);

/// Details for the card under the cursor. Dwell-gated and memoised: skimming
/// the grid must not fire a request per card.
async fn preview(subject_id: String) {
    let seq = PREVIEW_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    if subject_id.is_empty() {
        return with(|s| s.ui.stream.preview_open = false);
    }
    if let Some(d) = with(|s| s.st.preview_cache.get(&subject_id).cloned()) {
        let cover = cache_cover(&d.cover).await;
        return push_preview(&d, cover);
    }
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    if PREVIEW_SEQ.load(Ordering::SeqCst) != seq {
        return; // pointer moved on
    }
    let Ok(c) = catalogue().await else { return };
    let Ok(d) = c.details(&subject_id).await else { return };
    with(|s| {
        // Bounded: the grid is at most a few dozen cards.
        if s.st.preview_cache.len() > 60 {
            s.st.preview_cache.clear();
        }
        s.st.preview_cache.insert(subject_id.clone(), d.clone());
    });
    let cover = cache_cover(&d.cover).await;
    if PREVIEW_SEQ.load(Ordering::SeqCst) == seq {
        push_preview(&d, cover);
    }
}

fn push_preview(d: &Details, cover: Option<PathBuf>) {
    // Dub languages as a comma list — one more thing the card does not show.
    let langs: String = stream::prefer::dubs(d.dubs.clone())
        .iter()
        .map(|x| x.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let meta = meta_line(d);
    with(|s| {
        let p = &mut s.ui.stream;
        p.preview_title = d.title.clone();
        p.preview_meta = meta;
        p.preview_overview = d.overview.clone();
        p.preview_seasons = d.seasons.len() as i64;
        p.preview_is_series = d.is_series;
        p.preview_langs = langs;
        p.preview_cover = path_str(cover);
        p.preview_open = true;
    });
}

// ---------------------------------------------------------------- detail ----

/// Open a title against a named catalogue rather than whichever one the
/// provider button is showing.
///
/// The landing screen is the one place showing cards that did not come from the
/// active source, so opening one has to say which catalogue its id belongs to.
/// The two id namespaces are unrelated — a MovieBox subject id means nothing to
/// 4KHDHub — so getting this wrong is an empty detail page, not a wrong one.
async fn open_against(subject_id: String, source: Source) {
    // A no-op when it already matches. It clears the open title, so it has to
    // run before the open, not after.
    if source != active_source() {
        set_source(source.key().to_string());
    }
    open(subject_id).await;
}

/// A Continue Watching card, opened against the catalogue it was played from.
///
/// The resume row mixes both sources — one progress table, and playback records
/// into it whichever catalogue was in use. A row with no recorded source, from
/// before the column existed, opens against the current selection.
async fn open_resume(subject_id: String) {
    if subject_id.is_empty() {
        return;
    }
    let recorded = match v::pool().await {
        Ok(pool) => stream::progress::source_of(pool, &subject_id).await,
        Err(_) => None,
    };
    let source = recorded.map_or_else(active_source, |k| Source::parse(&k));
    open_against(subject_id, source).await;
}

/// Fetch details, then the files for the first episode (or the film itself).
async fn open(subject_id: String) {
    let epoch = next_epoch();
    set_status("Loading…", true);

    let c = match catalogue().await {
        Ok(c) => c,
        Err(e) => return set_status(explain(&e), false),
    };
    let details = match c.details(&subject_id).await {
        Ok(d) => d,
        Err(e) => return set_status(explain(&e), false),
    };
    if !is_current(epoch) {
        return;
    }

    // A split show ("Person of Interest S1".."S5") gets its season list from the
    // search grouping; a normal one from its own payload.
    let mut split = siblings_of(&subject_id);
    if split.is_empty() && details.is_series && details.seasons.len() <= 1 {
        split = regroup_seasons(&c, &details.title, &subject_id).await;
        if !is_current(epoch) {
            return;
        }
    }
    let season = if !split.is_empty() {
        split.iter().find(|(_, id)| *id == subject_id).map(|(n, _)| *n).unwrap_or(1)
    } else if details.is_series {
        details.seasons.first().map(|s| s.number).unwrap_or(1)
    } else {
        0
    };
    // Split-show subjects hold one season each, so the wire query is still se=1
    // for them — the season number is only a label there.
    let mut episode = if details.is_series || !split.is_empty() { 1 } else { 0 };
    let mut wire_season = if split.is_empty() { season } else { 1 };
    // Opened from History's Play button: resume the episode that row was about.
    let pending = take_pending_play(&subject_id);
    if let Some((_, p_season, p_episode)) = &pending {
        if *p_episode > 0 {
            wire_season = (*p_season).max(0);
            episode = *p_episode;
        }
    }
    with(|s| {
        s.st.selection = (wire_season, episode);
        s.st.open_id = subject_id.clone();
        s.st.season_subjects = split.clone();
        s.st.episodes = details.episodes.clone();
    });

    let saved = match v::pool().await {
        Ok(p) => stream::bookmarks::is_saved(p, &subject_id).await,
        Err(_) => false,
    };
    // Opening a saved show is what clears its "new episodes" badge.
    if saved {
        if let Ok(p) = v::pool().await {
            let _ = stream::bookmarks::mark_seen(p, &subject_id).await;
        }
    }
    load_watched(&subject_id).await;
    let cover = cache_cover(&details.cover).await;
    push_details(&details, cover, &subject_id, season);
    with(|s| {
        s.st.details = Some(details.clone());
        s.ui.stream.bookmarked = saved;
        if episode > 1 {
            s.ui.stream.episode = episode;
        }
    });
    load_files(epoch, subject_id, wire_season, episode).await;
}

fn take_pending_play(subject_id: &str) -> Option<(String, i64, i64)> {
    with(|s| match s.st.pending_play.as_ref() {
        Some(p) if p.0 == subject_id => s.st.pending_play.take(),
        // Any request for a different title is stale by definition once
        // something else has been opened.
        Some(_) => {
            s.st.pending_play = None;
            None
        }
        None => None,
    })
}

/// Recover the season grouping for a split show opened from outside search.
///
/// The list tying per-season subjects together is only ever built while parsing
/// search results, so a show opened from Bookmarks, History or the feed arrives
/// with no grouping and reads as a one-season title with the rest unreachable.
/// Searching its own base title rebuilds the set. Only a series claiming a
/// single season gets here, so a genuinely short show costs one request.
async fn regroup_seasons(c: &Catalogue, title: &str, subject_id: &str) -> Vec<(i64, String)> {
    let base = stream::split_season_suffix(title).0;
    if base.is_empty() {
        return Vec::new();
    }
    let Ok(hits) = c.search(&base, 1).await else { return Vec::new() };
    remember_season_maps(&hits);
    siblings_of(subject_id)
}

/// Paint the detail pane — everything except the file list.
fn push_details(d: &Details, cover: Option<PathBuf>, opened_id: &str, active_season: i64) {
    // Focus the dub list on Original/English/Hindi, failing open when a title
    // has none of them, so the picker is short and the choice obvious.
    let dubs: Vec<(String, String)> = stream::prefer::dubs(d.dubs.clone())
        .into_iter()
        .map(|x| (x.subject_id, x.name))
        .collect();
    let split = with(|s| s.st.season_subjects.clone());
    let seasons: Vec<i64> = if split.is_empty() {
        d.seasons.iter().map(|s| s.number).collect()
    } else {
        split.iter().map(|(n, _)| *n).collect()
    };
    // Episode count for the season actually on screen. Taking `first()` here
    // listed season 1's episodes no matter which season was open.
    let count = if split.is_empty() {
        max_ep_for(&d.seasons, active_season)
    } else {
        d.seasons.first().map(|s| s.max_ep).unwrap_or(0)
    };
    // Titles and progress key off the season we query with, which for a split
    // show is always 1 however the chip is labelled.
    let episodes = episode_rows(if split.is_empty() { active_season } else { 1 }, count);
    let is_series = d.is_series || !split.is_empty();
    // Highlight the language cut we are actually on. Prefer the id we opened
    // with — the payload's own is often absent.
    let cur_id = if d.id.is_empty() { opened_id.to_string() } else { d.id.clone() };
    let meta = meta_line(d);

    with(|s| {
        let p = &mut s.ui.stream;
        p.detail_open = true;
        p.title = d.title.clone();
        p.overview = d.overview.clone();
        p.meta = meta;
        p.cover = path_str(cover);
        p.is_series = is_series;
        p.dubs = dubs
            .into_iter()
            .map(|(id, label)| StreamChip { active: id == cur_id, id, label })
            .collect();
        p.seasons = seasons;
        p.episodes = episodes;
        p.season = active_season;
        p.episode = if is_series { 1 } else { 0 };
    });
}

/// This title's per-episode progress, so the strip can show how far through
/// each one you are.
async fn load_watched(subject_id: &str) {
    let Ok(pool) = v::pool().await else { return };
    let seen = stream::progress::for_subject(pool, subject_id).await;
    let map = seen
        .into_iter()
        .map(|e| ((e.season, e.episode), if e.finished { 1.0 } else { e.fraction() as f32 }))
        .collect();
    with(|s| s.st.watched = map);
}

/// The episode strip for one season: number, the catalogue's name where there
/// is one, and how far through it you are.
fn episode_rows(season: i64, count: i64) -> Vec<StreamEpisode> {
    let (titles, watched) = with(|s| (s.st.episodes.clone(), s.st.watched.clone()));
    episode_numbers(count)
        .into_iter()
        .map(|n| StreamEpisode {
            number: n,
            title: stream::episode_title(&titles, season, n),
            progress: watched.get(&(season, n)).copied().unwrap_or(0.0) as f64,
        })
        .collect()
}

/// Episode count for `season`, falling back to the first when the requested one
/// is not listed.
fn max_ep_for(seasons: &[stream::Season], season: i64) -> i64 {
    seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0)
}

/// `1..=max_ep`. Clamped — a bogus episode count from the server must not spin
/// up an unbounded list.
fn episode_numbers(max_ep: i64) -> Vec<i64> {
    (1..=max_ep.clamp(0, 500)).collect()
}

/// Resolve the playable files for one episode and paint the rows.
///
/// Cache first: a stored answer paints immediately and the servers are only
/// asked when nothing is stored or the entry has aged past its TTL. Every write
/// below is gated on `epoch`, so a slow response for episode 5 cannot land on
/// top of episode 7.
async fn load_files(epoch: u64, subject_id: String, season: i64, episode: i64) {
    let res = with(|s| s.st.resolution.clone());
    let key = stream::cache::Key::new(&subject_id, season, episode, &res);
    let pool = v::pool().await.ok();

    let cached = match pool {
        Some(p) => stream::cache::load(p, &key).await,
        None => None,
    };
    let mut painted = false;
    if let Some(cached) = &cached {
        if !is_current(epoch) {
            return;
        }
        paint_files(&cached.files, true).await;
        painted = true;

        // Backfill subs for an entry cached before captions were fetched
        // separately: patch just the captions, no stream refetch, and re-store.
        if !cached.files.is_empty() && cached.files.iter().all(|f| f.captions.is_empty()) {
            if let Ok(c) = catalogue().await {
                let mut patched = cached.files.clone();
                attach_episode_captions(&c, &subject_id, &mut patched).await;
                let gained = patched.iter().any(|f| !f.captions.is_empty());
                if gained && is_current(epoch) {
                    if let Some(p) = pool {
                        let _ = stream::cache::store(p, &key, &patched).await;
                    }
                    paint_files(&patched, true).await;
                }
            }
        }
    } else {
        set_status("Finding streams…", true);
    }

    let needs_fetch = match &cached {
        None => true,
        Some(c) => c.is_stale(v::now_secs()),
    };
    if !needs_fetch || !is_current(epoch) {
        return;
    }

    let c = match catalogue().await {
        Ok(c) => c,
        Err(e) => {
            if is_current(epoch) && !painted {
                set_status(explain(&e), false);
            }
            return;
        }
    };
    let mut found = match c
        .resources(&subject_id, season.max(0) as usize, episode.max(0) as usize, &res)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            if !is_current(epoch) {
                return;
            }
            // A stale-but-present list beats an empty screen.
            if !painted {
                paint_files(&[], false).await;
                set_status(explain(&e), false);
            }
            return;
        }
    };

    // Subtitles are NOT inline on the resource — that field is almost always
    // empty. They come from a separate captions request keyed by resource id.
    attach_episode_captions(&c, &subject_id, &mut found).await;

    let changed = match pool {
        Some(p) => stream::cache::store(p, &key, &found).await.unwrap_or(true),
        None => true,
    };
    if is_current(epoch) && (changed || !painted) {
        paint_files(&found, false).await;
    }
}

/// Fetch this episode's subtitle tracks and attach them to every file.
///
/// Subs for the same episode are the same content whichever bitrate you play,
/// so one fetch covers the whole episode — but a given resource may carry none,
/// so up to three distinct ones are tried concurrently and their tracks unioned.
/// Files that already came with inline captions are left untouched.
async fn attach_episode_captions(c: &Catalogue, subject_id: &str, files: &mut [StreamFile]) {
    if files.is_empty() || files.iter().any(|f| !f.captions.is_empty()) {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    let mut probe: Vec<String> = Vec::new();
    for f in files.iter() {
        if !f.resource_id.is_empty() && seen.insert(f.resource_id.clone()) {
            probe.push(f.resource_id.clone());
            if probe.len() == 3 {
                break;
            }
        }
    }

    let mut handles = Vec::new();
    for rid in probe {
        let c = c.clone();
        let sid = subject_id.to_string();
        handles.push(tokio::spawn(async move { c.captions(&sid, &rid).await.unwrap_or_default() }));
    }
    let mut merged: Vec<Caption> = Vec::new();
    for h in handles {
        let Ok(caps) = h.await else { continue };
        for cap in caps {
            if cap.url.is_empty() {
                continue;
            }
            let dup = merged.iter().any(|o| {
                (!cap.lang.is_empty() && o.lang.eq_ignore_ascii_case(&cap.lang)) || o.url == cap.url
            });
            if !dup {
                merged.push(cap);
            }
        }
    }
    let merged = stream::prefer::captions(merged);
    if merged.is_empty() {
        return;
    }
    for f in files.iter_mut() {
        f.captions = merged.clone();
    }
}

/// Push one resolved list into the view: stream rows, subtitle chips, status.
async fn paint_files(found: &[StreamFile], from_cache: bool) {
    let rows: Vec<StreamFileRow> = found
        .iter()
        .enumerate()
        .map(|(idx, f)| StreamFileRow {
            index: idx as i64,
            label: if f.resolution > 0 { format!("{}p", f.resolution) } else { "Auto".into() },
            sub: file_sub(f),
            uploader: f.uploader.clone(),
            subs: match f.captions.len() {
                0 => "No subs".to_string(),
                1 => "1 sub".to_string(),
                n => format!("{n} subs"),
            },
            has_subs: !f.captions.is_empty(),
        })
        .collect();
    let count = rows.len();

    // One subtitle list for the whole episode: the files are the same content at
    // different bitrates, so their caption sets are near-identical.
    let tracks = caption_union(found);
    // Keep the previously chosen language if this episode also has it.
    let want = with(|s| s.st.sub_lang.clone());
    let choice =
        want.as_deref().and_then(|w| tracks.iter().position(|c| c.lang.eq_ignore_ascii_case(w)));
    // Arm a stream the instant an episode's files land, so Play/Copy/Download
    // work without a row click: the rung the user last played, else 720p.
    let armed = stream::quality::pick(found, stream::quality::sticky());
    let current_label: String = armed
        .and_then(|i| found.get(i))
        .map(|f| {
            if f.resolution > 0 {
                format!("{}p stream", f.resolution)
            } else {
                "stream".into()
            }
        })
        .unwrap_or_default();
    let res = with(|s| s.st.resolution.clone());
    let label = if res.is_empty() { "all qualities".to_string() } else { format!("{res}p") };

    with(|s| {
        s.st.subs = tracks.clone();
        s.st.sub_choice = choice;
        s.st.files = found.to_vec();
        s.st.current = armed.unwrap_or(0);
        let p = &mut s.ui.stream;
        p.subs = tracks
            .iter()
            .enumerate()
            .map(|(idx, c)| StreamChip {
                id: idx.to_string(),
                label: if c.lang.is_empty() {
                    format!("Track {}", idx + 1)
                } else {
                    c.lang.clone()
                },
                active: Some(idx) == choice,
            })
            .collect();
        p.sub_choice = choice.map(|i| i as i64).unwrap_or(-1);
        p.files = rows;
        p.current = armed.map(|i| i as i64).unwrap_or(-1);
        p.current_label = current_label;
        p.status = match (count, from_cache) {
            (0, _) => "No streams at this quality — try another.".to_string(),
            (n, true) => format!("{n} streams · {label} · saved"),
            (n, false) => format!("{n} streams · {label}"),
        };
        p.busy = false;
    });

    // Opened from History's Play button: the streams have only just resolved, so
    // this is the first moment there is anything to play.
    if let Some(index) = armed {
        let open = with(|s| s.st.open_id.clone());
        if take_pending_play(&open).is_some() {
            play(index as i64).await;
        }
    }
}

fn back() {
    next_epoch(); // abandon anything still loading for the closed title
    clear_open_title();
    with(|s| {
        let p = &mut s.ui.stream;
        p.detail_open = false;
        p.subs.clear();
        p.sub_choice = -1;
        p.files.clear();
        p.status.clear();
        p.busy = false;
    });
}

/// Blank the streams panel and show the busy state, so the previous episode's
/// rows never linger while the next one resolves.
fn clear_stream_list() {
    with(|s| {
        let p = &mut s.ui.stream;
        p.files.clear();
        p.current = -1;
        p.current_label.clear();
        p.busy = true;
    });
}

/// Season picked — reset to episode 1 and re-resolve.
///
/// A split show stores each season under its own subject, so switching season
/// there means re-opening that subject; a normal series just re-queries.
async fn set_season(season: i64) {
    let split = with(|s| s.st.season_subjects.clone());
    if let Some((_, subject)) = split.iter().find(|(n, _)| *n == season) {
        // Re-register the list against every subject in it before opening the
        // new one: `open` reads the season map to work out which season the
        // subject represents, and if that entry has gone the show reopens as a
        // one-season title labelled "Season 1".
        let subject = subject.clone();
        with(|s| {
            for (_, id) in &split {
                s.st.season_map.insert(id.clone(), split.clone());
            }
            s.ui.stream.season = season;
        });
        return open(subject).await;
    }

    let Some(d) = with(|s| s.st.details.clone()) else { return };
    let episodes = episode_rows(season, max_ep_for(&d.seasons, season).max(1));
    let Some(id) = opened_subject() else { return };
    let epoch = next_epoch();
    with(|s| {
        s.st.selection = (season, 1);
        s.ui.stream.season = season;
        s.ui.stream.episode = 1;
        s.ui.stream.episodes = episodes;
    });
    clear_stream_list();
    load_files(epoch, id, season, 1).await;
}

async fn set_episode(episode: i64) {
    let Some(id) = opened_subject() else { return };
    let season = with(|s| s.st.selection.0).max(1);
    let epoch = next_epoch();
    with(|s| {
        s.st.selection = (season, episode);
        s.ui.stream.episode = episode;
    });
    clear_stream_list();
    load_files(epoch, id, season, episode).await;
}

/// Language cut picked. Each dub is a separate subject, so this reopens the
/// title under the new id.
///
/// For a split show the dub subject is not in the season map — re-point the
/// currently-open season at it and re-register, otherwise switching language
/// would silently collapse a five-season show down to one.
async fn set_dub(subject_id: String) {
    if let Some(cur) = opened_subject() {
        with(|s| {
            if let Some(slot) = s.st.season_subjects.iter_mut().find(|(_, id)| *id == cur) {
                slot.1 = subject_id.clone();
                let list = s.st.season_subjects.clone();
                for (_, id) in &list {
                    s.st.season_map.insert(id.clone(), list.clone());
                }
            }
        });
    }
    open(subject_id).await;
}

/// Subtitle language picked. `-1` turns subtitles off.
fn set_sub(index: i64) {
    let choice = if index < 0 { None } else { Some(index as usize) };
    with(|s| {
        s.st.sub_choice = choice;
        // Remember the language, not the index: the next episode may list its
        // tracks in a different order.
        s.st.sub_lang =
            choice.and_then(|i| s.st.subs.get(i).map(|c| c.lang.clone())).filter(|l| !l.is_empty());
        s.ui.stream.sub_choice = index;
        for (i, chip) in s.ui.stream.subs.iter_mut().enumerate() {
            chip.active = Some(i) == choice;
        }
    });
}

/// Resolution rung picked — re-resolve the current episode at that quality.
async fn set_resolution(res: String) {
    with(|s| {
        s.st.resolution = res.clone();
        s.ui.stream.resolution = res.clone();
    });
    stream::quality::set_filter(&res); // survives a restart
    let Some(id) = opened_subject() else { return };
    let (season, episode) = with(|s| s.st.selection);
    let epoch = next_epoch();
    load_files(epoch, id, season, episode).await;
}

/// A stream row was clicked — make it the current stream without playing.
fn set_current(index: i64) {
    let idx = index.max(0) as usize;
    with(|s| {
        s.st.current = idx;
        let label = s
            .st
            .files
            .get(idx)
            .map(|f| {
                if f.resolution > 0 {
                    format!("{}p stream", f.resolution)
                } else {
                    "stream".to_string()
                }
            })
            .unwrap_or_default();
        s.ui.stream.current = index;
        s.ui.stream.current_label = label;
    });
}

fn current_index() -> i64 {
    with(|s| {
        if s.st.files.is_empty() {
            -1
        } else {
            s.st.current.min(s.st.files.len() - 1) as i64
        }
    })
}

/// Switch which catalogue the tab searches.
///
/// Every cached thing belongs to the old source and its ids mean nothing to the
/// new one, so the results, the open title, the file list and the preview cache
/// are dropped rather than left on screen under a heading that no longer
/// describes them.
fn set_source(name: String) {
    let source = Source::parse(&name);
    if source == active_source() {
        return;
    }
    set_active_source(source);
    next_epoch();
    with(|s| {
        s.st.results.clear();
        s.st.preview_cache.clear();
        s.st.files.clear();
        s.st.open_id.clear();
        s.st.details = None;
        s.st.query.clear();
        s.st.page = 0;
        let p = &mut s.ui.stream;
        p.source = source.key().to_string();
        p.results.clear();
        p.detail_open = false;
        p.query.clear();
        p.status = format!("Searching {} now.", source.label());
    });
}

// -------------------------------------------------------------- playback ----

/// Hand the chosen file to the windowed mpv, with the episode's subtitle tracks
/// attached.
///
/// Tracks are downloaded to files named after their language rather than handed
/// to mpv as bare URLs, because mpv labels a track with its filename: a raw URL
/// shows up in the track menu as an unreadable string.
async fn play(index: i64) {
    let Some(file) = with(|s| s.st.files.get(index.max(0) as usize).cloned()) else {
        return set_status("That stream is no longer available.", false);
    };
    // This launch is the user's answer to "what quality?" — every later episode
    // arms the same rung.
    stream::quality::remember(file.resolution);
    let label =
        if file.resolution > 0 { format!("{}p", file.resolution) } else { "stream".to_string() };
    set_status(format!("Starting {label}…"), true);

    let url = match playable_url(&file.url).await {
        Ok(u) => u,
        Err(e) => return set_status(format!("That source could not be opened: {e}"), false),
    };
    let (tracks, chosen) = with(|s| (s.st.subs.clone(), s.st.sub_choice));
    let (mut args, named) = subtitle_args(&tracks, chosen).await;
    args.extend(subtitle_style_args());
    // Both catalogues hand back plain HTTP video-on-demand, so seeking should
    // reach the whole file rather than the cached window.
    args.extend(crate::vmpv::network_seek_args());
    let resume = resume_point().await;

    crate::vmpv::play(url, resume, None, args, progress_sink());
    // The next episode is very likely the next thing wanted: resolve it into
    // the cache now so switching to it is instant.
    prefetch_next();

    let mut msg = match (named, chosen.and_then(|i| tracks.get(i))) {
        (0, _) => format!("Playing {label} · no subtitles"),
        (n, Some(c)) => format!("Playing {label} · {n} subtitles · {}", c.lang),
        (n, None) => format!("Playing {label} · {n} subtitles · off"),
    };
    if let Some(at) = resume {
        msg.push_str(&format!(" · resumed at {}", v::fmt_duration(at)));
    }
    set_status(msg, false);
}

/// Where the open episode was left, or `None` to start from the top.
async fn resume_point() -> Option<f64> {
    let (subject_id, (season, episode)) = with(|s| (s.st.open_id.clone(), s.st.selection));
    if subject_id.is_empty() {
        return None;
    }
    let pool = v::pool().await.ok()?;
    stream::progress::get(pool, &subject_id, season, episode).await?.resume_at()
}

/// mpv flags for the subtitle look the user set.
fn subtitle_style_args() -> Vec<String> {
    let mut out = Vec::new();
    let scale = stream::subs::scale();
    if (scale - 1.0).abs() > f32::EPSILON {
        out.push(format!("--sub-scale={scale:.2}"));
    }
    let delay = stream::subs::delay();
    if delay.abs() > f32::EPSILON {
        out.push(format!("--sub-delay={delay:.1}"));
    }
    out
}

/// `(subject, season, episode)` of the episode after the open one, when the
/// title is a series and the season has one.
fn next_episode() -> Option<(String, i64, i64)> {
    let (subject_id, (season, episode), details) =
        with(|s| (s.st.open_id.clone(), s.st.selection, s.st.details.clone()));
    let d = details?;
    if subject_id.is_empty() || !d.is_series || episode <= 0 {
        return None;
    }
    let max_ep = d
        .seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| d.seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0);
    (episode < max_ep).then_some((subject_id, season, episode + 1))
}

/// Resolve the next episode's streams in the background, so choosing it paints
/// from cache. Silent: no status line, no repaint, and any failure is dropped.
fn prefetch_next() {
    let Some((subject_id, season, episode)) = next_episode() else { return };
    let res = with(|s| s.st.resolution.clone());
    tokio::spawn(async move {
        let key = stream::cache::Key::new(&subject_id, season, episode, &res);
        let Ok(pool) = v::pool().await else { return };
        if stream::cache::load(pool, &key).await.is_some() {
            return; // already have it
        }
        let Ok(c) = catalogue().await else { return };
        if let Ok(files) = c
            .resources(&subject_id, season.max(0) as usize, episode.max(0) as usize, &res)
            .await
        {
            if !files.is_empty() {
                let _ = stream::cache::store(pool, &key, &files).await;
            }
        }
    });
}

/// Snapshot what identifies the episode now playing, and hand back a hook that
/// records where it got to.
///
/// The hook fires on mpv's watcher thread, which is a plain `std::thread` with
/// no tokio context — hence the runtime handle captured here, where there is
/// one. `Handle::current()` inside the hook would panic, and with
/// `panic = "abort"` that closes the app.
fn progress_sink() -> Option<crate::vmpv::PlaybackEnd> {
    let (subject_id, (season, episode)) = with(|s| (s.st.open_id.clone(), s.st.selection));
    if subject_id.is_empty() {
        return None;
    }
    let (title, cover_url, is_series) = with(|s| {
        s.st.details
            .as_ref()
            .map(|d| (d.title.clone(), d.cover.clone(), d.is_series))
            .unwrap_or_default()
    });
    // Snapshotted with the title, not read inside the hook: by the time mpv
    // exits the user may have switched catalogues, and the row has to say where
    // this actually came from for Continue Watching to reopen it correctly.
    let source = active_source().key().to_string();
    let rt = tokio::runtime::Handle::current();

    Some(std::sync::Arc::new(move |position_s: f64, duration_s: f64| {
        // mpv never reported a position: the stream failed to open, and a
        // zero-progress history row would be noise.
        if position_s <= 1.0 {
            return;
        }
        let entry = stream::progress::Entry {
            subject_id: subject_id.clone(),
            season,
            episode,
            title: title.clone(),
            cover_url: cover_url.clone(),
            is_series,
            position_s,
            duration_s,
            source: source.clone(),
            ..Default::default()
        };
        let finished = stream::progress::is_finished(position_s, duration_s);
        rt.spawn(async move {
            if let Ok(pool) = v::pool().await {
                let _ = stream::progress::record(pool, &entry).await;
            }
            // Repaint the landing row now rather than on the next tab visit —
            // going back after watching something and finding the old position
            // still on the card reads as the progress not having been kept.
            feed_load().await;
            if finished {
                autoplay_next().await;
            }
            v::emit(VideosEvent::Changed);
        });
    }))
}

/// Move to the next episode and play it, if the user is still on the same title
/// and there is one. Anything else on screen means the moment has passed.
async fn autoplay_next() {
    if !stream::autoplay::enabled() {
        return;
    }
    let Some((subject_id, season, episode)) = next_episode() else { return };
    if opened_subject().as_deref() != Some(subject_id.as_str()) {
        return;
    }
    let epoch = next_epoch();
    with(|s| {
        s.st.selection = (season, episode);
        s.ui.stream.episode = episode;
    });
    load_files(epoch, subject_id, season, episode).await;
    if !is_current(epoch) {
        return;
    }
    // `load_files` armed a stream; play whatever it settled on.
    let index = current_index();
    if index >= 0 {
        set_status(format!("Playing episode {episode}…"), true);
        play(index).await;
    }
}

/// Play the title's trailer in the same external mpv the streams use.
///
/// `ytdl://ytsearch1:…` hands the lookup to mpv's youtube-dl hook, so the first
/// result plays directly — no second video pipeline, and no navigating the user
/// into another section to find it.
fn trailer() {
    let Some(d) = with(|s| s.st.details.clone()) else { return };
    if d.title.is_empty() {
        return;
    }
    let query = [d.title.as_str(), d.year.as_str(), "trailer"]
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    crate::vmpv::play(
        format!("ytdl://ytsearch1:{query}"),
        None,
        None,
        vec!["--ytdl-format=best[height<=1080]".to_string()],
        None,
    );
    set_status("Playing trailer.", false);
}

/// The armed stream's URL, resolved. Called from `videos_stream_link`.
pub(crate) async fn resolve_link(index: i64) -> Result<String> {
    let i = if index < 0 { current_index() } else { index };
    let Some(file) = with(|s| s.st.files.get(i.max(0) as usize).cloned()) else {
        anyhow::bail!("that stream is no longer available");
    };
    // Resolved first: the link on a 4KHDHub row points at a landing page, and
    // pasting that somewhere is not what "copy the stream link" means.
    match playable_url(&file.url).await {
        Ok(u) => Ok(u),
        Err(e) => anyhow::bail!("could not resolve that link: {e}"),
    }
}

// -------------------------------------------------------------- subtitles ----

/// Subtitle formats mpv loads from a `--sub-file`.
const SUB_EXTS: [&str; 5] = ["srt", "vtt", "ass", "ssa", "sub"];

/// Cache one subtitle track under a language-derived filename.
async fn cache_subtitle(c: &Caption, idx: usize) -> Option<PathBuf> {
    let dir = tulipix_core::paths::cache_dir()?.join("videos").join("stream-subs");
    tokio::fs::create_dir_all(&dir).await.ok()?;

    // mpv shows the file stem as the track name, so the stem *is* the label.
    let lang: String = c
        .lang
        .chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == ' ' { ch } else { '_' })
        .collect();
    let stem =
        if lang.trim().is_empty() { format!("Track {}", idx + 1) } else { lang.trim().to_string() };

    let bytes = tulipix_core::net::http()
        .get(&c.url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    let path = dir.join(format!("{stem}.{}", subtitle_ext(&bytes, &c.ext)));
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// What this subtitle actually is. Content wins; the server's own label is used
/// only when it is one of the formats mpv accepts — a track the catalogue calls
/// `"TEXT"` is silently ignored when that label goes straight into the filename.
fn subtitle_ext(bytes: &[u8], declared: &str) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("WEBVTT") {
        return "vtt".into();
    }
    if head.starts_with("[Script Info]") || head.starts_with("[V4+ Styles]") {
        return "ass".into();
    }
    let declared = declared.trim().trim_start_matches('.').to_ascii_lowercase();
    if SUB_EXTS.contains(&declared.as_str()) {
        return declared;
    }
    // SubRip is the catalogue's usual format and the safest default.
    "srt".into()
}

/// mpv flags for the episode's subtitles: the chosen language first, so it is
/// track 1 and can be preselected. Any track that fails to download is left
/// out — subtitles must never be the reason a video does not start.
async fn subtitle_args(tracks: &[Caption], chosen: Option<usize>) -> (Vec<String>, usize) {
    if tracks.is_empty() {
        return (Vec::new(), 0);
    }
    let mut order: Vec<usize> = chosen.into_iter().filter(|i| *i < tracks.len()).collect();
    order.extend((0..tracks.len()).filter(|i| Some(*i) != chosen));

    let mut args = Vec::new();
    let mut attached = 0usize;
    for i in order {
        if let Some(path) = cache_subtitle(&tracks[i], i).await {
            args.push(format!("--sub-file={}", path.display()));
            attached += 1;
        }
    }
    if attached > 0 {
        if chosen.is_some() {
            // Ours are the only external subtitle files in play: without this
            // mpv also picks up anything sitting next to the media, which for a
            // remote URL is unpredictable and shifts the track numbering.
            args.push("--sub-auto=no".to_string());
            args.push("--sid=1".to_string());
            // A user mpv.conf carrying `sub-visibility=no` leaves the track
            // selected but invisible, which looks exactly like broken subtitles.
            args.push("--sub-visibility=yes".to_string());
        } else {
            args.push("--sid=no".to_string());
        }
    }
    (args, attached)
}

/// Save a caption beside a downloaded video, so the file plays with the
/// language that was picked when it was queued.
async fn save_sidecar_sub(dest: &std::path::Path, c: &Caption) -> Option<PathBuf> {
    let bytes = tulipix_core::net::http()
        .get(&c.url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    if let Some(parent) = dest.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let path = dest.with_extension(subtitle_ext(&bytes, &c.ext));
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// mpv flags for a downloaded file's sidecar subtitle, when one was saved with
/// it. Empty when there is none — `--sub-auto` is then free to find whatever
/// else is in the folder, which is what a plain local file should do.
fn sidecar_sub_args(dest: &std::path::Path) -> Vec<String> {
    let Some(path) = SUB_EXTS.iter().map(|e| dest.with_extension(e)).find(|p| p.exists()) else {
        return Vec::new();
    };
    let mut args = vec![
        format!("--sub-file={}", path.display()),
        "--sub-auto=no".to_string(),
        "--sid=1".to_string(),
        "--sub-visibility=yes".to_string(),
    ];
    args.extend(subtitle_style_args());
    args
}

// ------------------------------------------------------------- bookmarks ----

async fn toggle_bookmark() {
    let Some(d) = with(|s| s.st.details.clone()) else { return };
    let Some(subject) = opened_subject() else { return };
    let split = with(|s| s.st.season_subjects.clone());
    // For a split show, save the whole show under its first-season subject so it
    // reopens on season 1, not whichever season happened to be on screen.
    let (subject_id, is_series) = match split.first() {
        Some((_, first)) => (first.clone(), true),
        None => (subject, d.is_series),
    };
    let bm = stream::bookmarks::Bookmark {
        subject_id,
        title: d.title.clone(),
        year: d.year.clone(),
        cover_url: d.cover.clone(),
        is_series,
        meta: meta_line(&d),
        overview: d.overview.clone(),
        // What the show lists today is the baseline; the badge is for what turns
        // up after this.
        seen_max_ep: d.seasons.last().map(|s| s.max_ep).unwrap_or(0),
        latest_max_ep: 0,
    };
    let Ok(pool) = v::pool().await else { return };
    let saved = stream::bookmarks::toggle(pool, &bm).await.unwrap_or(false);
    with(|s| {
        s.ui.stream.bookmarked = saved;
        s.ui.stream.status =
            if saved { "Saved to bookmarks." } else { "Removed from bookmarks." }.to_string();
    });
}

async fn bookmarks_load() {
    let (key, asc) = with(|s| (s.ui.stream.bm_sort.clone(), s.ui.stream.bm_asc));
    let Ok(pool) = v::pool().await else { return };
    let mut list = stream::bookmarks::list(pool).await;
    sort_bookmarks(&mut list, &key, asc);
    let covers = cache_covers(list.iter().map(|b| b.cover_url.clone()).collect()).await;
    let rows: Vec<StreamCard> = list
        .into_iter()
        .zip(covers)
        .map(|(b, poster)| {
            // `seasons` doubles as the new-episode badge here, and has to be
            // read before the rest of `b` moves into the card.
            let badge = if b.has_new() { b.new_count() } else { 0 };
            StreamCard {
                id: b.subject_id,
                title: b.title,
                year: b.year,
                poster: path_str(poster),
                is_series: b.is_series,
                seasons: badge,
                meta: b.meta,
                overview: truncate_chars(&b.overview, 100),
                progress: 0.0,
            }
        })
        .collect();
    with(|s| s.ui.stream.bookmarks = rows);
}

/// `list()` already returns newest-first, so "date" descending is a no-op.
fn sort_bookmarks(list: &mut [stream::bookmarks::Bookmark], key: &str, asc: bool) {
    match key {
        "name" => list.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        // Movies before series (then title), so the two kinds group together.
        "type" => list.sort_by(|a, b| {
            a.is_series.cmp(&b.is_series).then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        _ => {}
    }
    if asc {
        list.reverse();
    }
}

/// Check saved series for episodes added since they were last looked at.
///
/// Background curiosity, not something worth a burst of traffic: at most once a
/// day per show, a few at a time, one after another.
fn bookmarks_refresh() {
    tokio::spawn(async move {
        let Ok(pool) = v::pool().await else { return };
        let due = stream::bookmarks::due_for_check(pool, 24 * 60 * 60).await;
        if due.is_empty() {
            return;
        }
        let Ok(c) = catalogue().await else { return };
        let mut news = false;
        for bm in due.into_iter().take(REFRESH_BATCH) {
            let Ok(d) = c.details(&bm.subject_id).await else { continue };
            let latest = d.seasons.last().map(|s| s.max_ep).unwrap_or(0);
            if latest <= 0 {
                continue;
            }
            news |= stream::bookmarks::note_latest(pool, &bm.subject_id, latest)
                .await
                .unwrap_or(false);
        }
        if news {
            bookmarks_load().await;
            v::emit(VideosEvent::Changed);
        }
    });
}

// --------------------------------------------------------------- history ----

async fn history_load(page: i64) {
    let Ok(pool) = v::pool().await else { return };
    let all = stream::progress::history(pool, HISTORY_MAX).await;
    let pages = all.len().div_ceil(HISTORY_PAGE).max(1);
    // A page that no longer exists falls back to the last one that does.
    let page = (page.max(0) as usize).min(pages - 1);
    let entries: Vec<stream::progress::Entry> =
        all.into_iter().skip(page * HISTORY_PAGE).take(HISTORY_PAGE).collect();
    let covers = cache_covers(entries.iter().map(|e| e.cover_url.clone()).collect()).await;
    let rows: Vec<StreamHistoryRow> = entries
        .into_iter()
        .zip(covers)
        .map(|(e, poster)| StreamHistoryRow {
            kind: history_kind(&e),
            state: history_state(&e),
            ago: relative_time(e.updated),
            id: e.subject_id.clone(),
            title: e.title.clone(),
            poster: path_str(poster),
            progress: if e.finished { 1.0 } else { e.fraction() },
            finished: e.finished,
            season: e.season,
            episode: e.episode,
        })
        .collect();
    with(|s| {
        s.ui.stream.history = rows;
        s.ui.stream.history_page = page as i64;
        s.ui.stream.history_pages = pages as i64;
    });
}

/// "Series · S02E04" / "Movie".
fn history_kind(e: &stream::progress::Entry) -> String {
    if e.is_series && (e.season > 0 || e.episode > 0) {
        format!("Series  ·  S{:02}E{:02}", e.season.max(1), e.episode.max(1))
    } else if e.is_series {
        "Series".to_string()
    } else {
        "Movie".to_string()
    }
}

/// "Watched" / "34% · 48 min left".
fn history_state(e: &stream::progress::Entry) -> String {
    if e.finished {
        return "Watched".to_string();
    }
    let pct = format!("{:.0}%", e.fraction() * 100.0);
    match e.remaining_s() {
        Some(s) if s >= 60.0 => format!("{pct}  ·  {:.0} min left", s / 60.0),
        Some(_) => format!("{pct}  ·  nearly done"),
        None => pct,
    }
}

/// "just now" / "3 hours ago" / "2 days ago".
fn relative_time(then: i64) -> String {
    let secs = (v::now_secs() - then).max(0);
    let plural = |n: i64, unit: &str| format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" });
    match secs {
        s if s < 90 => "just now".to_string(),
        s if s < 3600 => plural(s / 60, "minute"),
        s if s < 86_400 => plural(s / 3600, "hour"),
        s if s < 2_592_000 => plural(s / 86_400, "day"),
        s => plural(s / 2_592_000, "month"),
    }
}

// ------------------------------------------------------------ landing row ----

/// Build the landing row: where you left off, then a few things worth opening.
///
/// The picks come from the DB first and are only refetched once they are half a
/// day old — the browse feed is an editorial front page, so asking for it on
/// every tab visit spends a request and a poster batch to arrive at the same six
/// cards. Both halves degrade on their own: no servers still leaves the resume
/// slider, and a failed refresh keeps showing the stored picks.
async fn feed_load() {
    let epoch = next_feed_epoch();

    // 1. Local first: instant, and independent of the network.
    let resume = continue_watching_cards().await;
    if !is_current_feed(epoch) {
        return;
    }
    with(|s| s.ui.stream.resume = cards_of(&resume));

    let pool = v::pool().await.ok();
    let stored = match pool {
        Some(p) => (
            stream::feed_cache::load(p, KIND_TRENDING).await,
            stream::feed_cache::load(p, KIND_VERTICAL).await,
        ),
        None => (None, None),
    };
    // 2. Whatever is stored paints straight away.
    if let Some(c) = &stored.0 {
        paint_picks(epoch, c.hits.clone(), true).await;
    }
    if let Some(c) = &stored.1 {
        paint_picks(epoch, c.hits.clone(), false).await;
    }

    // 3. Only go to the servers when the picks have aged out.
    let stale = stream::feed_cache::needs_refresh(stored.0.as_ref())
        || stream::feed_cache::needs_refresh(stored.1.as_ref());
    if !stale || !is_current_feed(epoch) {
        return;
    }
    let Ok(c) = client().await else {
        if stored.0.is_none() {
            set_status("No servers configured — open Servers and add one.", false);
        }
        return;
    };
    let rows = c.feed().await.unwrap_or_default();
    if rows.is_empty() || !is_current_feed(epoch) {
        return; // a failed refresh leaves the stored picks on screen
    }
    let picks = trending_picks(&rows);
    let verticals = vertical_picks(&rows, &picks);
    if let Some(p) = pool {
        let _ = stream::feed_cache::store(p, KIND_TRENDING, &picks).await;
        let _ = stream::feed_cache::store(p, KIND_VERTICAL, &verticals).await;
    }
    paint_picks(epoch, picks, true).await;
    paint_picks(epoch, verticals, false).await;
}

async fn paint_picks(epoch: u64, hits: Vec<stream::SearchHit>, trending: bool) {
    if hits.is_empty() {
        return;
    }
    let covers = cache_covers(hits.iter().map(|h| h.cover.clone()).collect()).await;
    if !is_current_feed(epoch) {
        return;
    }
    let cards: Vec<CardData> =
        hits.iter().zip(covers).map(|(hit, poster)| CardData::of(hit, poster)).collect();
    with(|s| {
        if trending {
            s.ui.stream.trending = cards_of(&cards);
        } else {
            s.ui.stream.vertical = cards_of(&cards);
        }
    });
}

/// There is no documented flag for the short, phone-shaped serials, so both the
/// trending row and the vertical slider go by the heading the catalogue files
/// them under. One list, read by both, so the two cannot drift apart.
fn is_short_form(row: &stream::feed::FeedRow) -> bool {
    const MARKERS: [&str; 5] = ["short", "vertical", "mini", "quick", "drama"];
    let heading = row.title.to_lowercase();
    MARKERS.iter().any(|m| heading.contains(m))
}

/// Six titles off the top of the feed, series first.
///
/// Series are preferred because the feed's movie entries come through without
/// usable artwork often enough that a row of them reads as broken; a film only
/// appears when there are not six series to show. Artwork is not optional: a
/// card without it is a grey box with a title, so a short row is the better
/// failure.
fn trending_picks(rows: &[stream::feed::FeedRow]) -> Vec<stream::SearchHit> {
    // Short-form rows are left out: the vertical slider draws from them, and
    // trending padding itself to six would take their titles first.
    let mut pool: Vec<&stream::feed::FeedRow> = rows.iter().filter(|r| !is_short_form(r)).collect();
    if pool.is_empty() {
        pool = rows.iter().collect();
    }
    let flat: Vec<&stream::SearchHit> = pool.iter().flat_map(|r| r.items.iter()).collect();
    let mut out: Vec<stream::SearchHit> = Vec::new();
    let take = |keep: &dyn Fn(&stream::SearchHit) -> bool, out: &mut Vec<stream::SearchHit>| {
        for hit in flat.iter().filter(|h| keep(h)) {
            if out.len() >= TRENDING_LEN {
                break;
            }
            if !out.iter().any(|o| o.id == hit.id) {
                out.push((*hit).clone());
            }
        }
    };
    take(&|h| h.is_series && !h.cover.is_empty(), &mut out);
    take(&|h| !h.cover.is_empty(), &mut out);
    out
}

/// Vertical dramas — the short, phone-shaped serials the catalogue runs as
/// their own strand. When the feed uses none of the marker words this falls
/// back to titles the trending row did not take, which is wrong-but-harmless:
/// the slider still shows something openable.
fn vertical_picks(
    rows: &[stream::feed::FeedRow],
    taken: &[stream::SearchHit],
) -> Vec<stream::SearchHit> {
    let spoken_for = |h: &stream::SearchHit| taken.iter().any(|t| t.id == h.id);
    let mut out: Vec<stream::SearchHit> = Vec::new();
    let push = |hit: &stream::SearchHit, out: &mut Vec<stream::SearchHit>| {
        if out.len() < VERTICAL_LEN
            && !hit.cover.is_empty()
            && !spoken_for(hit)
            && !out.iter().any(|o| o.id == hit.id)
        {
            out.push(hit.clone());
        }
    };
    for row in rows.iter().filter(|r| is_short_form(r)) {
        for hit in &row.items {
            push(hit, &mut out);
        }
    }
    if out.is_empty() {
        for hit in rows.iter().flat_map(|r| r.items.iter()) {
            push(hit, &mut out);
        }
    }
    out
}

/// Continue Watching as cards, newest first.
async fn continue_watching_cards() -> Vec<CardData> {
    let Ok(pool) = v::pool().await else { return Vec::new() };
    let entries = stream::progress::continue_watching(pool, RESUME_LEN).await;
    if entries.is_empty() {
        return Vec::new();
    }
    let covers = cache_covers(entries.iter().map(|e| e.cover_url.clone()).collect()).await;
    entries
        .into_iter()
        .zip(covers)
        .map(|(e, poster)| CardData {
            note: format!("{:.0}% watched", e.fraction() * 100.0),
            line: episode_label(&e),
            meta: left_label(&e),
            progress: e.fraction(),
            id: e.subject_id,
            title: e.title,
            poster,
            is_series: e.is_series,
            seasons: 0,
        })
        .collect()
}

/// Which episode you are on. Blank for a film, which then shows nothing there.
fn episode_label(e: &stream::progress::Entry) -> String {
    if e.is_series && (e.season > 0 || e.episode > 0) {
        format!("Season {}  ·  Episode {}", e.season.max(1), e.episode.max(1))
    } else {
        String::new()
    }
}

/// How much is left: "18 min left".
fn left_label(e: &stream::progress::Entry) -> String {
    match e.remaining_s() {
        Some(s) if s >= 3600.0 => {
            format!("{:.0} h {:02.0} min left", s / 3600.0, (s % 3600.0) / 60.0)
        }
        Some(s) if s >= 60.0 => format!("{:.0} min left", s / 60.0),
        Some(_) => "nearly done".to_string(),
        None => String::new(),
    }
}

// ----------------------------------------------------------------- hosts ----

fn hosts_load() {
    let text = stream::hosts::load().join("\n");
    let key = stream::sign_key::load();
    with(|s| {
        let p = &mut s.ui.stream;
        p.hosts_text = text;
        p.hosts_error.clear();
        p.key_text = key;
        p.key_error.clear();
    });
}

/// Validate and persist the edited host list. A rejected entry leaves the
/// stored list untouched and reports why.
fn hosts_save(text: String) {
    let entries: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    match stream::hosts::save(&entries) {
        Ok(saved) => {
            invalidate_client();
            let n = saved.len();
            let joined = saved.join("\n");
            with(|s| {
                let p = &mut s.ui.stream;
                p.hosts_text = joined;
                p.hosts_error.clear();
                p.status = format!("Saved {n} server{}.", if n == 1 { "" } else { "s" });
            });
        }
        Err(msg) => with(|s| s.ui.stream.hosts_error = msg),
    }
}

/// Validate and persist the signing key. On success the running signer switches
/// to it and the client is rebuilt so `init()` re-runs under the new key.
fn key_save(key: String) {
    match stream::sign_key::save(&key) {
        Ok(saved) => {
            invalidate_client();
            with(|s| {
                let p = &mut s.ui.stream;
                p.key_text = saved;
                p.key_error.clear();
                p.status = "Signing key updated.".into();
            });
        }
        Err(msg) => with(|s| s.ui.stream.key_error = msg),
    }
}

/// Time every host in the editor and report which ones answer.
///
/// Reads the text box, not the saved list, so an edit can be tested before it is
/// committed. Nothing is written — the user decides whether to reorder.
async fn hosts_check(text: String) {
    let hosts: Vec<String> = text.lines().filter_map(|l| stream::hosts::validate(l).ok()).collect();
    if hosts.is_empty() {
        return with(|s| {
            s.ui.stream.hosts_error = "Nothing to test — add a server first.".into()
        });
    }
    with(|s| {
        s.ui.stream.hosts_checking = true;
        s.ui.stream.hosts_error.clear();
    });
    // A dead host is only known to be dead once it has timed out, so this is as
    // slow as the slowest entry — hence its own busy flag.
    let results = stream::probe::probe_all(&hosts).await;
    let summary = stream::probe::summary(&results);
    let ranked = stream::probe::rank(&results).join("\n");
    let rows: Vec<StreamHostHealth> = results
        .iter()
        .map(|r| StreamHostHealth { host: short_host(&r.host), note: r.label(), ok: r.ok })
        .collect();
    with(|s| {
        let p = &mut s.ui.stream;
        p.host_health = rows;
        p.hosts_summary = summary;
        // Offered, not applied: Save is still the only thing that writes.
        p.hosts_ranked = ranked;
        p.hosts_checking = false;
    });
}

// ------------------------------------------------------------------ cast ----

/// One SSDP M-SEARCH round. Blocking, so it runs off the async worker.
/// Both sections ask the same question of the same protocol, so the SSDP loop
/// and the name trimming live in `cast_serve` now rather than twice.
fn discover_renderers() -> Vec<tulipix_music::cast::CastDevice> {
    crate::cast_serve::discover()
}

fn short_name(raw: &str) -> String {
    crate::cast_serve::short_name(raw)
}

async fn cast_discover() {
    set_status("Looking for devices…", true);
    let found = tokio::task::spawn_blocking(discover_renderers).await.unwrap_or_default();
    let names: Vec<String> = found.iter().map(|d| short_name(&d.name)).collect();
    let count = names.len();
    with(|s| {
        s.st.cast_targets = found;
        s.ui.stream.cast_devices = names;
        s.ui.stream.status = match count {
            0 => "No cast devices found on this network.".to_string(),
            1 => "1 device found.".to_string(),
            n => format!("{n} devices found."),
        };
        s.ui.stream.busy = false;
    });
}

/// Send the armed stream to the picked renderer.
async fn cast_to(device: String) {
    use tulipix_music::cast as dlna;
    let index = current_index();
    let Some(file) = with(|s| s.st.files.get(index.max(0) as usize).cloned()) else {
        return set_status("Pick a stream first.", false);
    };
    let Some(dev) = with(|s| {
        s.st.cast_targets.iter().find(|d| short_name(&d.name) == device).cloned()
    }) else {
        return set_status("That device is no longer listed — scan again.", false);
    };
    set_status(format!("Casting to {device}…"), true);

    let client = tulipix_core::net::http().clone();
    // The device description says where its AVTransport control endpoint is.
    let desc = match client.get(&dev.location).send().await {
        Ok(r) => r.text().await.unwrap_or_default(),
        Err(_) => String::new(),
    };
    let Some(ctl) = dlna::parse_control_url(&desc) else {
        return set_status("That device cannot play video (no AVTransport).", false);
    };
    let ctl = dlna::resolve_url(&dev.location, &ctl);

    let url = match playable_url(&file.url).await {
        Ok(u) => u,
        Err(e) => return set_status(format!("That source could not be opened: {e}"), false),
    };
    for (action, body) in
        [("SetAVTransportURI", dlna::soap_set_uri(0, &url)), ("Play", dlna::soap_play(0))]
    {
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
            return set_status(format!("{device} refused the stream."), false);
        }
    }
    with(|s| {
        s.st.cast_session = Some(ctl);
        s.ui.stream.cast_active = true;
        s.ui.stream.cast_target = device.clone();
        s.ui.stream.status = format!("Casting to {device}.");
        s.ui.stream.busy = false;
    });
}

/// Stop the renderer and hand playback back to this machine.
async fn cast_stop() {
    use tulipix_music::cast as dlna;
    let Some(ctl) = with(|s| s.st.cast_session.take()) else {
        return with(|s| s.ui.stream.cast_active = false);
    };
    let _ = tulipix_core::net::http()
        .post(&ctl)
        .header("SOAPACTION", dlna::soap_action_header("Stop"))
        .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
        .body(dlna::soap_stop(0))
        .send()
        .await;
    with(|s| {
        s.ui.stream.cast_active = false;
        s.ui.stream.cast_target.clear();
        s.ui.stream.status = "Cast stopped.".into();
    });
}

// ------------------------------------------------------------- downloads ----

/// Set while a download runs; flipping it true asks the loop to stop.
static DL_CANCEL: AtomicBool = AtomicBool::new(false);
/// Guards against two runners at once.
static DL_BUSY: AtomicBool = AtomicBool::new(false);

/// Where finished downloads land.
///
/// The first configured video library by default, so a downloaded episode joins
/// the local grid with a thumbnail and its own resume instead of disappearing
/// into a folder the app never looks at.
fn download_dir() -> PathBuf {
    if library_dest() {
        if let Some(lib) = first_video_library() {
            return lib.join("Stream");
        }
    }
    if let Ok(d) = std::env::var("XDG_DOWNLOAD_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let d = PathBuf::from(home).join("Downloads");
        if d.is_dir() {
            return d;
        }
    }
    PathBuf::from(".")
}

fn first_video_library() -> Option<PathBuf> {
    use tulipix_core::libraries::Section;
    let s = tulipix_core::settings::Settings::load().ok()?;
    s.libraries
        .libraries
        .iter()
        .find(|l| l.section == Section::Videos)
        .map(|l| l.path.clone())
        .filter(|p| p.is_dir())
}

/// Strip anything that cannot go in a filename, and collapse runs of spaces.
fn safe_filename(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '.' || c == '-' { c } else { ' ' })
        .collect();
    // Drop tokens that are nothing but dots. Separators are already gone, so
    // "../.." could not traverse anywhere, but it still leaves names like
    // ".._.._etc_passwd.mp4". Dots inside a token stay, so "S.W.A.T." survives.
    let joined = cleaned
        .split_whitespace()
        .filter(|t| !t.chars().all(|c| c == '.'))
        .collect::<Vec<_>>()
        .join("_");
    if joined.is_empty() { "stream".to_string() } else { joined }
}

/// `"Person_of_Interest_S01E05_1080p.mp4"` — enough to identify it later.
fn download_name(title: &str, season: i64, episode: i64, resolution: i64) -> String {
    let mut name = safe_filename(title);
    if season > 0 || episode > 0 {
        name.push_str(&format!("_S{season:02}E{episode:02}"));
    }
    if resolution > 0 {
        name.push_str(&format!("_{resolution}p"));
    }
    name.push_str(".mp4");
    name
}

/// Queue the chosen stream. One download runs at a time; the rest wait in the
/// ledger, which survives a restart.
async fn download(index: i64) {
    let Some(file) = with(|s| s.st.files.get(index.max(0) as usize).cloned()) else {
        return set_status("That stream is no longer available.", false);
    };
    let (subject, title, (season, episode)) = with(|s| {
        (
            s.st.open_id.clone(),
            s.st.details.as_ref().map(|d| d.title.clone()).unwrap_or_default(),
            s.st.selection,
        )
    });
    let name = download_name(&title, season, episode, file.resolution);
    let dest = download_dir().join(&name).display().to_string();
    // The language picked in the detail pane travels with the file.
    let caption = with(|s| s.st.sub_choice.and_then(|i| s.st.subs.get(i).cloned()));
    queue_subtitle(&dest, caption);
    let job = stream::downloads::Job {
        subject_id: subject,
        title: if title.is_empty() { name.clone() } else { title },
        season,
        episode,
        resolution: file.resolution,
        url: file.url,
        dest,
        ..Default::default()
    };
    enqueue(job).await;
}

/// Fetch a caption and drop it next to the video being queued. Failure is
/// silent: a missing subtitle must never stop a download from being queued.
fn queue_subtitle(dest: &str, caption: Option<Caption>) {
    let Some(c) = caption else { return };
    let dest = PathBuf::from(dest);
    tokio::spawn(async move {
        let _ = save_sidecar_sub(&dest, &c).await;
    });
}

/// This episode's track in the language the user picked.
fn caption_in(file: &StreamFile, lang: Option<&str>) -> Option<Caption> {
    let want = lang?.trim();
    if want.is_empty() {
        return None;
    }
    file.captions.iter().find(|c| c.lang.eq_ignore_ascii_case(want)).cloned()
}

/// Queue every episode of the open season at the armed quality.
///
/// Each episode has to be resolved before it can be fetched, and resolving all
/// of them up front would stall on the slowest, so this walks the season in the
/// background and appends as it goes.
async fn download_season() {
    let Some(subject_id) = opened_subject() else { return };
    let (season, details) = with(|s| (s.st.selection.0, s.st.details.clone()));
    let Some(d) = details else { return };
    if !d.is_series {
        let i = current_index();
        if i >= 0 {
            download(i).await;
        }
        return;
    }
    let max_ep = d
        .seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| d.seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0);
    if max_ep <= 0 {
        return;
    }
    let title = d.title.clone();
    let res = with(|s| s.st.resolution.clone());
    let sub_lang = with(|s| s.st.sub_lang.clone());
    set_status(format!("Queueing {max_ep} episodes…"), true);

    tokio::spawn(async move {
        let Ok(c) = catalogue().await else { return };
        let sticky = stream::quality::sticky();
        let mut queued = 0;
        for ep in 1..=max_ep {
            let files = c
                .resources(&subject_id, season.max(0) as usize, ep.max(0) as usize, &res)
                .await
                .unwrap_or_default();
            let Some(pick) = stream::quality::pick(&files, sticky) else { continue };
            let Some(file) = files.get(pick) else { continue };
            let name = download_name(&title, season, ep, file.resolution);
            let dest = download_dir().join(&name).display().to_string();
            queue_subtitle(&dest, caption_in(file, sub_lang.as_deref()));
            enqueue(stream::downloads::Job {
                subject_id: subject_id.clone(),
                title: title.clone(),
                season,
                episode: ep,
                resolution: file.resolution,
                url: file.url.clone(),
                dest,
                ..Default::default()
            })
            .await;
            queued += 1;
        }
        set_status(format!("Queued {queued} episodes."), false);
        v::emit(VideosEvent::Changed);
    });
}

/// Record a job and start the runner if it is idle.
async fn enqueue(job: stream::downloads::Job) {
    let Ok(pool) = v::pool().await else { return };
    if stream::downloads::enqueue(pool, &job).await.is_err() {
        return;
    }
    downloads_load().await;
    start_runner();
}

/// Start draining the queue unless something already is.
fn start_runner() {
    if DL_BUSY.swap(true, Ordering::SeqCst) {
        return; // a runner is already going; it will pick the new job up
    }
    DL_CANCEL.store(false, Ordering::SeqCst);
    tokio::spawn(run_queue());
}

/// Drain the queue one job at a time until it is empty or cancelled.
///
/// Writes to a `.part` file and renames on success, so an interrupted download
/// never leaves something that looks complete.
async fn run_queue() {
    use stream::downloads::State;
    let Ok(pool) = v::pool().await else {
        DL_BUSY.store(false, Ordering::SeqCst);
        return;
    };
    while let Some(job) = stream::downloads::next_queued(pool).await {
        if DL_CANCEL.swap(false, Ordering::SeqCst) {
            let _ = stream::downloads::cancel_waiting(pool).await;
            break;
        }
        let _ = stream::downloads::set_state(pool, job.id, State::Running, "").await;
        downloads_load().await;

        let dest = PathBuf::from(&job.dest);
        let part = dest.with_extension("mp4.part");
        let left = stream::downloads::active_count(pool).await.saturating_sub(1);
        let label = job_label(&job);
        set_dl(format!("{label}{}", waiting_suffix(left)), 0.0, true);

        let id = job.id;
        let report = move |text: String, frac: f64, done: u64, total: u64| {
            let header = format!("{text}{}", waiting_suffix(left));
            with(|s| {
                s.ui.stream.dl_label = header.clone();
                s.ui.stream.dl_frac = frac;
                s.ui.stream.dl_active = true;
                if let Some(row) = s.ui.stream.downloads.iter_mut().find(|r| r.id == id) {
                    row.progress = frac;
                    row.detail = text.clone();
                }
            });
            // The row is patched here *and* pushed as an event: a snapshot only
            // reaches Dart when it asks for one, and the Downloads page has to
            // move while nothing is being dispatched.
            v::emit(VideosEvent::DownloadTick {
                id,
                progress: frac,
                detail: text,
                label: header,
                active: true,
            });
            let (d, t) = (done as i64, total as i64);
            tokio::spawn(async move {
                if let Ok(pool) = v::pool().await {
                    let _ = stream::downloads::set_progress(pool, id, d, t).await;
                }
            });
        };

        // Resolved here rather than when the job was queued: a resolver link is
        // minted per request and expires, so a queue that sat for an hour would
        // start every job on a dead URL.
        let source_url = match playable_url(&job.url).await {
            Ok(u) => u,
            Err(e) => {
                let _ = stream::downloads::set_state(
                    pool,
                    job.id,
                    State::Failed,
                    &format!("source could not be resolved: {e}"),
                )
                .await;
                set_dl(format!("{label} — source unavailable"), 0.0, true);
                continue;
            }
        };
        match download_to(&source_url, &part, &report).await {
            Ok(true) => {
                if tokio::fs::rename(&part, &dest).await.is_ok() {
                    let _ = stream::downloads::set_state(pool, job.id, State::Done, "").await;
                    set_dl(format!("Saved {label}"), 1.0, true);
                    rescan_library(&dest);
                } else {
                    let _ = stream::downloads::set_state(
                        pool,
                        job.id,
                        State::Failed,
                        "could not be moved into place",
                    )
                    .await;
                    set_dl("Downloaded, but could not be renamed.".to_string(), 1.0, true);
                }
            }
            Ok(false) => {
                let _ = tokio::fs::remove_file(&part).await;
                let _ = stream::downloads::set_state(pool, job.id, State::Cancelled, "").await;
                let _ = stream::downloads::cancel_waiting(pool).await;
                set_dl("Download cancelled.".to_string(), 0.0, false);
                downloads_load().await;
                break;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                let _ = stream::downloads::set_state(pool, job.id, State::Failed, &e).await;
                set_dl(format!("Download failed: {e}"), 0.0, true);
            }
        }
        downloads_load().await;
        v::emit(VideosEvent::Changed);
    }
    DL_BUSY.store(false, Ordering::SeqCst);

    // Auto-hide the strip a few seconds after the queue empties — unless
    // another download started in the meantime.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    if !DL_BUSY.load(Ordering::SeqCst) {
        with(|s| {
            s.ui.stream.dl_label.clear();
            s.ui.stream.dl_active = false;
            s.ui.stream.dl_frac = 0.0;
        });
        v::emit(VideosEvent::Changed);
    }
}

/// "Severance · S01E04 · 720p" — one line identifying a job.
fn job_label(job: &stream::downloads::Job) -> String {
    let mut out = job.title.clone();
    if job.season > 0 || job.episode > 0 {
        out.push_str(&format!("  ·  S{:02}E{:02}", job.season.max(1), job.episode.max(1)));
    }
    if job.resolution > 0 {
        out.push_str(&format!("  ·  {}p", job.resolution));
    }
    out
}

fn waiting_suffix(left: i64) -> String {
    if left > 0 { format!("  ·  {left} waiting") } else { String::new() }
}

/// `frac < 0` leaves the bar where it is — for queue-length notices that should
/// not disturb a download already in flight.
fn set_dl(label: String, frac: f64, active: bool) {
    with(|s| {
        s.ui.stream.dl_label = label;
        if frac >= 0.0 {
            s.ui.stream.dl_frac = frac;
        }
        s.ui.stream.dl_active = active;
    });
}

/// Stream `url` into `part`. `Ok(false)` means the user cancelled.
async fn download_to(
    url: &str,
    part: &std::path::Path,
    report: &impl Fn(String, f64, u64, u64),
) -> Result<bool, String> {
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = part.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut resp = tulipix_core::net::http_stream()
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let total = resp.content_length().unwrap_or(0);
    let mut out = tokio::fs::File::create(part).await.map_err(|e| e.to_string())?;
    let mut done: u64 = 0;
    let mut last_tick = std::time::Instant::now();

    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if DL_CANCEL.load(Ordering::SeqCst) {
            return Ok(false);
        }
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;

        // Reporting on every chunk would flood the event stream.
        if last_tick.elapsed() >= std::time::Duration::from_millis(200) {
            last_tick = std::time::Instant::now();
            let frac = if total > 0 { done as f64 / total as f64 } else { 0.0 };
            report(progress_label(done, total), frac.clamp(0.0, 1.0), done, total);
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    Ok(true)
}

fn progress_label(done: u64, total: u64) -> String {
    let mb = |b: u64| b as f64 / 1024.0 / 1024.0;
    if total > 0 {
        format!(
            "{:.0} MB of {:.0} MB ({:.0}%)",
            mb(done),
            mb(total),
            done as f64 / total as f64 * 100.0
        )
    } else {
        format!("{:.0} MB", mb(done))
    }
}

/// Rescan the video library a download just landed in, so the file shows up in
/// the local grid — with a thumbnail and its own resume — without being asked.
fn rescan_library(dest: &std::path::Path) {
    use tulipix_core::libraries::Section;
    if !library_dest() {
        return;
    }
    let dest = dest.to_path_buf();
    tokio::spawn(async move {
        let Ok(settings) = tulipix_core::settings::Settings::load() else { return };
        // The download folder is a subfolder of a library, so a prefix match is
        // what identifies which one.
        let Some(lib) = settings
            .libraries
            .libraries
            .iter()
            .find(|l| l.section == Section::Videos && dest.starts_with(&l.path))
        else {
            return;
        };
        if let Ok(pool) = v::pool().await {
            let _ = tulipix_videos::scan::scan_library(pool, lib).await;
        }
        v::emit(VideosEvent::Changed);
    });
}

async fn downloads_load() {
    let Ok(pool) = v::pool().await else { return };
    let jobs = stream::downloads::list(pool, LEDGER_LIMIT).await;
    let active = stream::downloads::active_count(pool).await;
    // `file_present` touches the filesystem, so it is resolved before the lock.
    let rows: Vec<StreamDownloadRow> = jobs
        .into_iter()
        .map(|j| {
            let present = j.file_present();
            StreamDownloadRow {
                id: j.id,
                title: job_label(&j),
                // A finished file since deleted elsewhere is reported as
                // missing rather than offered for playing.
                state: state_label(&j, present),
                detail: state_detail(&j, present),
                dest: j.dest.clone(),
                progress: j.fraction(),
                active: j.state().active(),
                done: j.state() == stream::downloads::State::Done && present,
                failed: matches!(
                    j.state(),
                    stream::downloads::State::Failed | stream::downloads::State::Cancelled
                ) || (j.state() == stream::downloads::State::Done && !present),
            }
        })
        .collect();
    with(|s| {
        s.ui.stream.downloads = rows;
        s.ui.stream.dl_pending = active;
    });
}

fn state_label(job: &stream::downloads::Job, present: bool) -> String {
    use stream::downloads::State;
    match job.state() {
        State::Running => "Downloading".into(),
        State::Queued => "Waiting".into(),
        State::Cancelled => "Cancelled".into(),
        State::Failed => "Failed".into(),
        State::Done if present => "Saved".into(),
        State::Done => "Missing".into(),
    }
}

fn state_detail(job: &stream::downloads::Job, present: bool) -> String {
    use stream::downloads::State;
    match job.state() {
        State::Running => {
            progress_label(job.done_bytes.max(0) as u64, job.total_bytes.max(0) as u64)
        }
        State::Queued => "in the queue".into(),
        State::Failed | State::Cancelled if !job.error.is_empty() => job.error.clone(),
        State::Failed => "download did not finish".into(),
        State::Cancelled => "stopped before it finished".into(),
        State::Done if present => job.dest.clone(),
        State::Done => format!("no longer at {}", job.dest),
    }
}

/// Anything left `running` when the app closed is not running now: put it back
/// in the queue and pick it up.
async fn downloads_resume() {
    let Ok(pool) = v::pool().await else { return };
    let _ = stream::downloads::reset_orphans(pool).await;
    if stream::downloads::active_count(pool).await > 0 {
        start_runner();
    }
    downloads_load().await;
}

/// Play a finished download in the same external mpv the streams use.
async fn download_play(id: i64) {
    let Ok(pool) = v::pool().await else { return };
    let Some(job) = stream::downloads::get(pool, id).await else { return };
    if !job.file_present() {
        return set_status("That file is no longer on disk.", false);
    }
    let dest = PathBuf::from(&job.dest);
    // The subtitle saved beside it when it was queued goes back on.
    let args = sidecar_sub_args(&dest);
    let subbed = !args.is_empty();
    crate::vmpv::play(job.dest.clone(), None, None, args, None);
    let label = job_label(&job);
    set_status(
        if subbed {
            format!("Playing {label} · with subtitles")
        } else {
            format!("Playing {label}")
        },
        false,
    );
}

/// Show the finished file in the desktop file manager.
async fn download_reveal(id: i64) {
    let Ok(pool) = v::pool().await else { return };
    let Some(job) = stream::downloads::get(pool, id).await else { return };
    let dest = PathBuf::from(&job.dest);
    let dir = dest.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dest.clone());
    match tulipix_platform::fm::reveal_in_file_manager(&dest) {
        Ok(()) => set_status(format!("Opened {}", dir.display()), false),
        Err(e) => set_status(format!("Could not open the folder: {e}"), false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_line_skips_blank_fields() {
        let d = Details {
            year: "2024".into(),
            genre: "Drama".into(),
            rating: 8.74,
            ..Default::default()
        };
        assert_eq!(meta_line(&d), "2024  ·  Drama  ·  ★ 8.7");
        assert_eq!(meta_line(&Details::default()), "");
    }

    #[test]
    fn errors_are_plain_language_not_debug() {
        for e in [
            StreamError::NoHosts,
            StreamError::HostsExhausted,
            StreamError::MissingToken,
            StreamError::ApiStatus(403),
        ] {
            let msg = explain(&e);
            assert!(!msg.is_empty());
            assert!(!msg.contains("StreamError"), "leaked Debug form: {msg}");
        }
        assert!(explain(&StreamError::ApiStatus(403)).contains("403"));
    }

    /// The fix for streams landing under the wrong episode: only the newest
    /// action may paint.
    #[test]
    fn the_generation_guard_invalidates_a_superseded_action() {
        let first = next_epoch();
        assert!(is_current(first));
        let second = next_epoch();
        assert!(!is_current(first), "a superseded action must not paint");
        assert!(is_current(second));
    }

    #[test]
    fn truncate_chars_caps_by_char_not_byte() {
        assert_eq!(truncate_chars("short", 100), "short");
        assert_eq!(truncate_chars("abcdef", 3), "abc…");
        assert_eq!(truncate_chars("abc", 3), "abc");
        assert_eq!(truncate_chars("héllo", 2), "hé…");
    }

    #[test]
    fn a_cover_extension_follows_the_bytes_not_the_url() {
        assert_eq!(image_ext(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a]), "png");
        assert_eq!(image_ext(b"RIFF\0\0\0\0WEBPVP8 "), "webp");
        assert_eq!(image_ext(&[0xff, 0xd8, 0xff, 0xe0]), "jpg");
        // Unrecognised bytes stay jpg: a wrong guess must not create a second
        // cache entry for the same URL.
        assert_eq!(image_ext(b""), "jpg");
    }

    #[test]
    fn caption_union_keeps_one_entry_per_language() {
        let cap = |lang: &str, url: &str| Caption {
            url: url.into(),
            lang: lang.into(),
            ext: "srt".into(),
        };
        let files = vec![
            StreamFile {
                captions: vec![cap("English", "https://s/en1"), cap("Hindi", "https://s/hi")],
                ..Default::default()
            },
            StreamFile {
                // same languages on the 480p cut — must not double the picker
                captions: vec![cap("english", "https://s/en2"), cap("", "")],
                ..Default::default()
            },
        ];
        let u = caption_union(&files);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].url, "https://s/en1"); // first URL seen wins
        assert!(caption_union(&[]).is_empty());
    }

    #[test]
    fn download_names_are_filesystem_safe_and_identify_the_episode() {
        assert_eq!(
            download_name("Person of Interest", 1, 5, 1080),
            "Person_of_Interest_S01E05_1080p.mp4"
        );
        assert_eq!(download_name("Dune: Part Two", 0, 0, 720), "Dune_Part_Two_720p.mp4");
        // Path separators and quotes must never survive into a filename.
        assert_eq!(download_name("../../etc/passwd", 0, 0, 0), "etc_passwd.mp4");
        assert_eq!(download_name("..", 0, 0, 0), "stream.mp4");
        // But dots within a word are part of the title.
        assert_eq!(download_name("S.W.A.T.", 1, 1, 0), "S.W.A.T._S01E01.mp4");
        assert!(!download_name("a/b\\c:d", 2, 10, 480).contains(['/', '\\', ':']));
    }

    #[test]
    fn progress_label_handles_an_unknown_length() {
        assert_eq!(progress_label(5 * 1024 * 1024, 10 * 1024 * 1024), "5 MB of 10 MB (50%)");
        assert_eq!(progress_label(3 * 1024 * 1024, 0), "3 MB");
    }

    #[test]
    fn episode_count_follows_the_open_season() {
        let seasons = vec![
            stream::Season { number: 1, max_ep: 9 },
            stream::Season { number: 2, max_ep: 22 },
        ];
        assert_eq!(max_ep_for(&seasons, 2), 22); // not season 1's count
        assert_eq!(max_ep_for(&seasons, 99), 9); // unknown season → first
        assert_eq!(max_ep_for(&[], 1), 0);
    }

    #[test]
    fn episode_numbers_are_one_based_and_bounded() {
        assert_eq!(episode_numbers(3), vec![1, 2, 3]);
        assert!(episode_numbers(0).is_empty());
        assert!(episode_numbers(-5).is_empty()); // server junk must not underflow
        assert_eq!(episode_numbers(10_000).len(), 500);
    }

    #[test]
    fn subtitle_extension_follows_the_content_over_the_label() {
        assert_eq!(subtitle_ext(b"WEBVTT\n\n", "TEXT"), "vtt");
        assert_eq!(subtitle_ext(b"[Script Info]\n", ""), "ass");
        assert_eq!(subtitle_ext(b"1\n00:00", "srt"), "srt");
        // A label mpv would not accept never reaches the filename.
        assert_eq!(subtitle_ext(b"1\n00:00", "TEXT"), "srt");
    }

    #[test]
    fn device_names_drop_the_boilerplate_headers() {
        assert_eq!(short_name("Linux/4.9 UPnP/1.0 Sony/1.0"), "Sony/1.0");
        assert_eq!(short_name("Samsung TV"), "Samsung TV");
        // Nothing left after filtering: keep the original rather than a blank.
        assert_eq!(short_name("UPnP/1.0"), "UPnP/1.0");
    }

    #[test]
    fn history_lines_read_as_where_you_got_to() {
        let entry = |finished: bool, pos: f64, dur: f64, series: bool| stream::progress::Entry {
            subject_id: "s".into(),
            season: 2,
            episode: 4,
            title: "Severance".into(),
            is_series: series,
            position_s: pos,
            duration_s: dur,
            finished,
            ..Default::default()
        };
        assert_eq!(history_kind(&entry(false, 0.0, 0.0, true)), "Series  ·  S02E04");
        assert_eq!(history_kind(&entry(false, 0.0, 0.0, false)), "Movie");
        assert_eq!(history_state(&entry(true, 990.0, 1000.0, true)), "Watched");
        assert_eq!(history_state(&entry(false, 300.0, 1200.0, true)), "25%  ·  15 min left");
        // An unknown length gives a percentage of nothing rather than a lie.
        assert_eq!(history_state(&entry(false, 300.0, 0.0, true)), "0%");
    }

    #[test]
    fn relative_time_rounds_to_the_unit_that_reads_best() {
        let now = v::now_secs();
        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - 600), "10 minutes ago");
        assert_eq!(relative_time(now - 3600), "1 hour ago");
        assert_eq!(relative_time(now - 2 * 86_400), "2 days ago");
        // A clock that went backwards must not print a negative age.
        assert_eq!(relative_time(now + 500), "just now");
    }

    #[test]
    fn trending_prefers_series_and_never_pads_with_artless_cards() {
        let hit = |id: &str, series: bool, cover: bool| stream::SearchHit {
            id: id.into(),
            title: format!("Title {id}"),
            year: "2024".into(),
            cover: if cover { format!("https://img/{id}.jpg") } else { String::new() },
            is_series: series,
            season_subjects: Vec::new(),
        };
        let rows = vec![stream::feed::FeedRow {
            title: "Popular".into(),
            items: vec![
                hit("m1", false, true),
                hit("s1", true, true),
                hit("s2", true, true),
                hit("blank", true, false),
            ],
        }];
        let picks = trending_picks(&rows);
        // Series first, then the film; the artwork-less entry is left out even
        // though the row is short of six.
        assert_eq!(picks.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(), ["s1", "s2", "m1"]);
        // Everything is spoken for, so the slider stays empty rather than
        // repeating the row beside it.
        assert!(vertical_picks(&rows, &picks).is_empty());
    }

    #[test]
    fn bookmark_sort_orders_by_key_and_direction() {
        use stream::bookmarks::Bookmark;
        let mk = |id: &str, title: &str, series: bool| Bookmark {
            subject_id: id.into(),
            title: title.into(),
            year: "2020".into(),
            cover_url: String::new(),
            is_series: series,
            meta: String::new(),
            overview: String::new(),
            seen_max_ep: 0,
            latest_max_ep: 0,
        };
        let base = vec![mk("b", "Zebra", false), mk("a", "Apple", true)];

        let mut by_name = base.clone();
        sort_bookmarks(&mut by_name, "name", false);
        assert_eq!(by_name[0].title, "Apple");
        let mut by_name_desc = base.clone();
        sort_bookmarks(&mut by_name_desc, "name", true);
        assert_eq!(by_name_desc[0].title, "Zebra");
        let mut by_type = base.clone();
        sort_bookmarks(&mut by_type, "type", false); // movies before series
        assert!(!by_type[0].is_series);
        let mut by_date = base.clone();
        sort_bookmarks(&mut by_date, "date", false); // unchanged
        assert_eq!(by_date[0].subject_id, "b");
    }
}
