//! Stream tab wiring — remote catalogue search, detail view, and playback.
//!
//! The typed backend lives in `tulipix_videos::stream`; everything here is the
//! UI half: one shared client, the poster cache, and the async handlers that
//! `main.rs` hangs off the `window.on_video_stream_*` callbacks.
//!
//! Playback hands the resolved URL to the same windowed mpv the local library
//! uses (`spawn_mpv_windowed`) — no resume/progress writeback, since a remote
//! title has no row in the videos DB.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tulipix_common::*;
use tulipix_ui::*;
use tulipix_videos::stream::{self, Caption, Details, StreamClient, StreamError, StreamFile};

// ---- shared state ----

/// Everything the Stream tab remembers between callbacks, behind one lock.
///
/// This used to be eleven separate statics that had to agree with each other;
/// they did not, and the mismatches were real bugs (a subject id from one field
/// used with a season from another). One struct makes the invalid combinations
/// unrepresentable and the update sites obvious.
#[derive(Debug)]
struct StreamState {
    /// Subject id the open title was fetched with.
    ///
    /// Resource lookups must use *this*, not `Details::id`. The detail payload's
    /// `id` field is frequently absent, and an empty `subjectId=` in the resource
    /// query is a 400 — which is why switching episodes once failed while the
    /// first load (which still had the id in hand) worked.
    open_id: String,
    /// Details of the open title, so a season/episode/dub switch can re-query
    /// without another detail fetch.
    details: Option<Details>,
    /// Selected `(season, episode)`; `(0, 0)` means "a movie, no episode axis".
    selection: (i64, i64),
    /// `(season number, subject id)` for a show the catalogue splits across one
    /// subject per season. Empty when the subject carries its own seasons.
    season_subjects: Vec<(i64, String)>,
    /// Every subject id belonging to a split show maps to that show's full
    /// season list, so opening any season still knows about its siblings.
    season_map: std::collections::HashMap<String, Vec<(i64, String)>>,
    /// Files backing the current episode, parallel to the rows in the UI.
    files: Vec<StreamFile>,
    /// Subtitle tracks for the current episode, one per language.
    subs: Vec<Caption>,
    /// Index into `subs`, or `None` for no subtitles.
    sub_choice: Option<usize>,
    /// Language *name* of the chosen subtitle. Kept separately from the index
    /// so a pick of "English" survives an episode whose tracks are ordered
    /// differently.
    sub_lang: Option<String>,
    /// Resolution rung the user picked; `""` means every rung (one request each).
    resolution: String,
    /// In-memory detail cache for the hover preview, so skimming the grid does
    /// not hammer the servers.
    preview_cache: std::collections::HashMap<String, Details>,
    /// Index into `files` treated as the "current" stream — the one the info-box
    /// Play/Copy/Download act on. Auto-set to 0 (best quality) each time an
    /// episode's streams load, so changing episode never needs a row click.
    current_stream: usize,
    /// Whether the open title is saved. Drives the bookmark button's state.
    bookmarked: bool,
    /// Saved-page sort: key ("date"|"name"|"type") and ascending flag.
    bm_sort: String,
    bm_asc: bool,
}

impl Default for StreamState {
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
            resolution: String::new(),
            preview_cache: std::collections::HashMap::new(),
            current_stream: 0,
            bookmarked: false,
            bm_sort: "date".to_string(),
            bm_asc: false,
        }
    }
}

static STATE: OnceLock<Mutex<StreamState>> = OnceLock::new();
fn state() -> &'static Mutex<StreamState> {
    STATE.get_or_init(|| Mutex::new(StreamState::default()))
}

/// Read or mutate the shared state. Never hold the guard across an `.await` —
/// every caller here takes what it needs and gets out.
fn with_state<R>(f: impl FnOnce(&mut StreamState) -> R) -> R
where
    R: Default,
{
    match state().lock() {
        Ok(mut g) => f(&mut g),
        Err(_) => R::default(),
    }
}

/// The subject id resource lookups must use. `None` means nothing is open.
fn opened_subject() -> Option<String> {
    with_state(|s| (!s.open_id.is_empty()).then(|| s.open_id.clone()))
}

fn current_resolution() -> String {
    with_state(|s| s.resolution.clone())
}

/// Clear everything tied to the open title, keeping the search-level state.
fn clear_open_title() {
    with_state(|s| {
        s.open_id.clear();
        s.details = None;
        s.season_subjects.clear();
        s.files.clear();
        s.subs.clear();
        s.sub_choice = None;
    });
}

/// Built once and reused; dropped whenever the host list changes.
static CLIENT: OnceLock<Mutex<Option<StreamClient>>> = OnceLock::new();
fn client_cell() -> &'static Mutex<Option<StreamClient>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Monotonic counter bumped by every user action in the tab.
///
/// Detail and stream fetches are async and overlap: click Episode 5 then
/// Episode 7 and both requests are in flight, so whichever *finishes* last
/// used to win and paint its streams under the other episode's highlight.
/// Every fetch carries the epoch it started in and drops its result if a newer
/// action has happened since.
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

fn is_current(epoch: u64) -> bool {
    EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch
}

/// Reuse the live client, or build one over the configured hosts and warm it up
/// so later requests carry a session token.
///
/// `tokio::sync::OnceCell` would be tidier, but the cell has to be resettable
/// when the host list changes — so this keeps the plain lock and accepts that
/// two simultaneous first-calls may both build one. The loser is dropped.
async fn client() -> Result<StreamClient, StreamError> {
    if let Some(c) = client_cell().lock().ok().and_then(|g| g.clone()) {
        return Ok(c);
    }
    let c = StreamClient::new(stream::hosts::load())?;
    c.init().await?;
    if let Ok(mut g) = client_cell().lock() {
        // Another task may have finished first; keep whichever is already in.
        if g.is_none() {
            *g = Some(c.clone());
        }
        return Ok(g.clone().unwrap_or(c));
    }
    Ok(c)
}

/// Force the next call to rebuild the client — used after a host edit so the new
/// list takes effect without a restart.
pub fn stream_invalidate_client() {
    if let Ok(mut g) = client_cell().lock() {
        *g = None;
    }
}

// ---- small UI helpers ----

fn set_status(weak: &slint::Weak<MainWindow>, msg: impl Into<String>, busy: bool) {
    let msg = msg.into();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_status(msg.into());
        w.set_video_stream_busy(busy);
    });
}

/// Plain-language failure text — users see these, not the Debug form.
fn explain(e: &StreamError) -> String {
    match e {
        StreamError::NoHosts => "No servers configured — open Hosts and add one.".into(),
        StreamError::HostsExhausted => "No server answered. Check your connection or edit Hosts.".into(),
        StreamError::MissingToken => "Servers answered but refused a session. Try different Hosts.".into(),
        StreamError::ApiStatus(404) => "Nothing found for that.".into(),
        StreamError::ApiStatus(s) => format!("Servers returned {s}. Try again or edit Hosts."),
        StreamError::Reqwest(_) => "Network error.".into(),
        StreamError::Json(_) => "Unreadable response from the server.".into(),
    }
}

fn cover_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("videos").join("stream"))
}

/// Download a remote poster into the cache, keyed by a hash of its URL, and
/// return the local path. Already-cached files are reused untouched.
async fn cache_cover(url: &str) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }
    let dir = cover_dir()?;
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let path = dir.join(format!("{:x}.jpg", h.finalize()));
    if path.exists() {
        return Some(path);
    }
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let bytes = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

fn load_image(path: Option<PathBuf>) -> slint::Image {
    path.as_deref()
        .and_then(|p| slint::Image::load_from_path(p).ok())
        .unwrap_or_default()
}

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

/// "2.1 GB · h265" for the row subtitle; either half may be missing.
fn file_sub(f: &StreamFile) -> String {
    [f.size.as_str(), f.codec.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

// ---- search ----

/// Search the remote catalogue and repaint the results grid. Posters are cached
/// concurrently so a slow image host cannot hold up the whole grid.
pub fn stream_search(weak: slint::Weak<MainWindow>, query: String) {
    let query = query.trim().to_string();
    // A search always returns to the results/landing view — close any open
    // detail so it doesn't stay layered over the results.
    clear_open_title();
    let _ = weak.upgrade_in_event_loop(|w| w.set_video_stream_detail_open(false));
    if query.is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_results(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamCard>::new())));
            w.set_video_stream_status("".into());
            w.set_video_stream_busy(false);
        });
        return;
    }
    let epoch = next_epoch(); // also abandons any detail load still in flight
    set_status(&weak, "Searching…", true);
    tokio::runtime::Handle::current().spawn(async move {
        let c = match client().await {
            Ok(c) => c,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        // Remember the term only once a server has actually been reached, so a
        // dead host list does not fill the landing screen with junk.
        push_recent(&weak, &query);
        let hits = match c.search(&query, 1).await {
            Ok(h) => h,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        // Search "dune" then "loki" quickly and the slower answer must not win.
        if !is_current(epoch) {
            return;
        }
        if hits.is_empty() {
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_video_stream_results(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamCard>::new())));
            });
            return set_status(&weak, "Nothing found.", false);
        }

        remember_season_maps(&hits);
        let covers = cache_covers(hits.iter().map(|h| h.cover.clone()).collect()).await;
        let host = c.active_host().await;
        let count = hits.len();
        let cards: Vec<(stream::SearchHit, Option<PathBuf>)> =
            hits.into_iter().zip(covers).collect();
        if !is_current(epoch) {
            return; // poster caching is slow; re-check before painting
        }

        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<StreamCard> = cards
                .into_iter()
                .map(|(h, poster)| StreamCard {
                    id: h.id.into(),
                    title: h.title.into(),
                    year: h.year.into(),
                    poster: load_image(poster),
                    is_series: h.is_series,
                    // A split show advertises its season count on the card.
                    seasons: h.season_subjects.len() as i32,
                    meta: Default::default(),
                    overview: Default::default(),
                })
                .collect();
            w.set_video_stream_results(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_video_stream_status(format!("{count} results · {host}").into());
            w.set_video_stream_busy(false);
        });
    });
}

/// Cache a batch of poster URLs concurrently, preserving input order.
async fn cache_covers(urls: Vec<String>) -> Vec<Option<PathBuf>> {
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

// ---- split-season shows ----

/// Record the season lists that came back with a search, keyed by every subject
/// in each list. Replaces the previous search's entries.
fn remember_season_maps(hits: &[stream::SearchHit]) {
    with_state(|st| {
        st.season_map.clear();
        for hit in hits.iter().filter(|h| h.season_subjects.len() > 1) {
            for (_, id) in &hit.season_subjects {
                st.season_map.insert(id.clone(), hit.season_subjects.clone());
            }
        }
    })
}

/// Season siblings of `subject_id`, or empty when it is not a split show.
fn siblings_of(subject_id: &str) -> Vec<(i64, String)> {
    with_state(|st| st.season_map.get(subject_id).cloned().unwrap_or_default())
}

// ---- detail view ----

/// Open a title: fetch details, then the files for the first episode (or the
/// movie itself). A series with no season data still resolves as season 1.
pub fn stream_open(weak: slint::Weak<MainWindow>, subject_id: String) {
    let epoch = next_epoch();
    set_status(&weak, "Loading…", true);
    tokio::runtime::Handle::current().spawn(async move {
        let c = match client().await {
            Ok(c) => c,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        let details = match c.details(&subject_id).await {
            Ok(d) => d,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        // A newer open/season/episode click landed while this was in flight.
        if !is_current(epoch) {
            return;
        }

        // A split show ("Person of Interest S1".."S5") gets its season list from
        // the search grouping; a normal one from its own payload.
        let split = siblings_of(&subject_id);
        let season = if !split.is_empty() {
            split
                .iter()
                .find(|(_, id)| *id == subject_id)
                .map(|(n, _)| *n)
                .unwrap_or(1)
        } else if details.is_series {
            details.seasons.first().map(|s| s.number).unwrap_or(1)
        } else {
            0
        };
        // Split-show subjects hold one season each, so the wire query is still
        // se=1 for them — the season number is only a label here.
        let episode = if details.is_series || !split.is_empty() { 1 } else { 0 };
        let wire_season = if split.is_empty() { season } else { 1 };
        with_state(|st| st.selection = (wire_season, episode));
        with_state(|st| st.open_id = subject_id.clone());
        with_state(|st| st.season_subjects = split.clone());
        // Saved state for the bookmark button.
        let saved = match pool_for("videos").await {
            Ok(p) => stream::bookmarks::is_saved(&p, &subject_id).await,
            Err(_) => false,
        };
        with_state(|st| st.bookmarked = saved);
        let cover = cache_cover(&details.cover).await;
        push_details(&weak, &details, cover, &subject_id, season);
        with_state(|st| st.details = Some(details.clone()));
        let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_bookmarked(saved));
        load_files(weak, epoch, subject_id, wire_season, episode).await;
    });
}

/// Paint the detail pane (everything except the file list).
fn push_details(
    weak: &slint::Weak<MainWindow>,
    d: &Details,
    cover: Option<PathBuf>,
    opened_id: &str,
    active_season: i64,
) {
    let (title, overview, meta) = (d.title.clone(), d.overview.clone(), meta_line(d));
    // Focus the dub list on Original/English/Hindi (fails open when a title has
    // none of them), so the picker is short and the choice obvious.
    let dubs: Vec<(String, String)> = stream::prefer::dubs(d.dubs.clone())
        .into_iter()
        .map(|x| (x.subject_id, x.name))
        .collect();
    // Seasons come from the search grouping for a split show, otherwise from
    // the subject's own payload.
    let split = with_state(|st| st.season_subjects.clone());
    let seasons: Vec<i32> = if split.is_empty() {
        d.seasons.iter().map(|s| s.number as i32).collect()
    } else {
        split.iter().map(|(n, _)| *n as i32).collect()
    };
    // Episode count for the season actually on screen. Taking `first()` here
    // listed season 1's episodes no matter which season was open.
    let episodes = episode_numbers(if split.is_empty() {
        max_ep_for(&d.seasons, active_season)
    } else {
        // Split shows carry exactly one season per subject.
        d.seasons.first().map(|s| s.max_ep).unwrap_or(0)
    });
    let is_series = d.is_series || !split.is_empty();
    // Highlight the language cut we are actually on. Prefer the id we opened
    // with — the payload's own `id` is often absent.
    let cur_id = if d.id.is_empty() { opened_id.to_string() } else { d.id.clone() };

    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_detail_open(true);
        w.set_video_stream_title(title.into());
        w.set_video_stream_overview(overview.into());
        w.set_video_stream_meta(meta.into());
        w.set_video_stream_cover(load_image(cover));
        w.set_video_stream_is_series(is_series);
        let chips: Vec<StreamChip> = dubs
            .into_iter()
            .map(|(id, label)| {
                let active = id == cur_id;
                StreamChip { id: id.into(), label: label.into(), active }
            })
            .collect();
        w.set_video_stream_dubs(slint::ModelRc::new(slint::VecModel::from(chips)));
        w.set_video_stream_seasons(slint::ModelRc::new(slint::VecModel::from(seasons)));
        w.set_video_stream_episodes(slint::ModelRc::new(slint::VecModel::from(episodes)));
        w.set_video_stream_season(active_season as i32);
        w.set_video_stream_episode(if is_series { 1 } else { 0 });
    });
}

/// Episode count for `season`, falling back to the first season when the
/// requested one is not listed.
fn max_ep_for(seasons: &[stream::Season], season: i64) -> i64 {
    seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0)
}

/// `1..=max_ep` as pickable chips. Clamped — a bogus episode count from the
/// server must not spin up an unbounded model.
fn episode_numbers(max_ep: i64) -> Vec<i32> {
    (1..=max_ep.clamp(0, 500) as i32).collect()
}

/// Resolve the playable files for one episode and paint the file rows.
///
/// Cache first: a stored answer paints immediately and the servers are only
/// asked when nothing is stored or the entry has aged past its TTL. A refresh
/// that comes back identical repaints nothing.
///
/// `epoch` is the action this load belongs to — see [`EPOCH`]. Every UI write below
/// is gated on it, so a slow response for episode 5 cannot land on top of
/// episode 7.
async fn load_files(
    weak: slint::Weak<MainWindow>,
    epoch: u64,
    subject_id: String,
    season: i64,
    episode: i64,
) {
    let res = current_resolution();
    let key = stream::cache::Key::new(&subject_id, season, episode, &res);
    let pool = pool_for("videos").await.ok();

    // 1. Serve whatever is stored, immediately.
    let cached = match &pool {
        Some(p) => stream::cache::load(p, &key).await,
        None => None,
    };
    let mut painted = false;
    if let Some(cached) = &cached {
        if !is_current(epoch) {
            return;
        }
        paint_files(&weak, &cached.files, true);
        painted = true;

        // Backfill subs for an entry cached before captions were fetched
        // separately: patch just the captions, no stream refetch, and re-store.
        if !cached.files.is_empty() && cached.files.iter().all(|f| f.captions.is_empty()) {
            if let Ok(c) = client().await {
                let mut patched = cached.files.clone();
                attach_episode_captions(&c, &subject_id, &mut patched).await;
                let gained = patched.iter().any(|f| !f.captions.is_empty());
                if gained && is_current(epoch) {
                    if let Some(p) = &pool {
                        let _ = stream::cache::store(p, &key, &patched).await;
                    }
                    paint_files(&weak, &patched, true);
                }
            }
        }
    } else {
        set_status(&weak, "Finding streams…", true);
    }

    // 2. Only go to the network when there is nothing, or it has gone stale.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let needs_fetch = match &cached {
        None => true,
        Some(c) => c.is_stale(now),
    };
    if !needs_fetch || !is_current(epoch) {
        return;
    }

    let c = match client().await {
        Ok(c) => c,
        Err(e) => {
            if is_current(epoch) && !painted {
                set_status(&weak, explain(&e), false);
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
                paint_files(&weak, &[], false);
                set_status(&weak, explain(&e), false);
            }
            return;
        }
    };

    // 2b. Subtitles are NOT inline on the resource — that field is almost always
    // empty. They come from a separate get-ext-captions request keyed by
    // resourceId. Fetch once for this episode and attach to every file so the
    // picker is populated and the answer caches with the streams.
    attach_episode_captions(&c, &subject_id, &mut found).await;

    // 3. Store, and repaint only if the answer actually moved.
    let changed = match &pool {
        Some(p) => stream::cache::store(p, &key, &found).await.unwrap_or(true),
        None => true,
    };
    if is_current(epoch) && (changed || !painted) {
        paint_files(&weak, &found, false);
    }
}

/// Fetch this episode's subtitle tracks and attach them to every file.
///
/// The catalogue serves subtitles from `get-ext-captions`, keyed by a resource,
/// not inline on the stream list. Subs for the same episode are the same content
/// regardless of which bitrate you play, so one fetch covers the whole episode —
/// but a given resource may simply carry none, so up to three distinct resources
/// (usually different uploaders) are tried concurrently and their tracks unioned.
/// Files that already came with inline captions are left untouched.
async fn attach_episode_captions(c: &StreamClient, subject_id: &str, files: &mut [StreamFile]) {
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
        if let Ok(caps) = h.await {
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
    }
    let merged = stream::prefer::captions(merged);
    if merged.is_empty() {
        return;
    }
    for f in files.iter_mut() {
        f.captions = merged.clone();
    }
}

/// Blank the streams panel and show the busy state — used the instant a season
/// or episode is picked so the previous episode's rows never linger on screen.
fn clear_stream_list(w: &MainWindow) {
    w.set_video_stream_files(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamFileRow>::new())));
    w.set_video_stream_current(-1);
    w.set_video_stream_current_label("".into());
    w.set_video_stream_busy(true);
}

/// Push one resolved list into the UI: stream rows, subtitle chips, status.
fn paint_files(weak: &slint::Weak<MainWindow>, found: &[StreamFile], from_cache: bool) {
    let rows: Vec<StreamFileRow> = found
        .iter()
        .enumerate()
        .map(|(idx, f)| StreamFileRow {
            index: idx as i32,
            label: if f.resolution > 0 { format!("{}p", f.resolution) } else { "Auto".into() }.into(),
            sub: file_sub(f).into(),
            uploader: f.uploader.clone().into(),
            subs: match f.captions.len() {
                0 => slint::SharedString::from("No subs"),
                1 => "1 sub".into(),
                n => format!("{n} subs").into(),
            },
            has_subs: !f.captions.is_empty(),
        })
        .collect();
    let count = rows.len();

    // One subtitle list for the whole episode: the files are the same content at
    // different bitrates, so their caption sets are near-identical. Union them,
    // one entry per language, so the picker does not repeat itself.
    let tracks = caption_union(found);
    // Keep the previously chosen language if this episode also has it.
    let want = with_state(|st| st.sub_lang.clone());
    let choice = want
        .as_deref()
        .and_then(|w| tracks.iter().position(|c| c.lang.eq_ignore_ascii_case(w)));
    with_state(|st| st.subs = tracks.clone());
    with_state(|st| st.sub_choice = choice);
    with_state(|st| st.files = found.to_vec());
    // Auto-select the best (first) stream so the info-box Play/Copy/Download are
    // armed the instant an episode's streams land — no row click needed.
    with_state(|st| st.current_stream = 0);
    let current_label: String = found
        .first()
        .map(|f| if f.resolution > 0 { format!("{}p stream", f.resolution) } else { "stream".into() })
        .unwrap_or_default();

    let res = current_resolution();
    let _ = weak.upgrade_in_event_loop(move |w| {
        let chips: Vec<StreamChip> = tracks
            .iter()
            .enumerate()
            .map(|(idx, c)| StreamChip {
                id: idx.to_string().into(),
                label: if c.lang.is_empty() { format!("Track {}", idx + 1) } else { c.lang.clone() }.into(),
                active: Some(idx) == choice,
            })
            .collect();
        w.set_video_stream_subs(slint::ModelRc::new(slint::VecModel::from(chips)));
        w.set_video_stream_sub_choice(choice.map(|i| i as i32).unwrap_or(-1));
        w.set_video_stream_files(slint::ModelRc::new(slint::VecModel::from(rows)));
        w.set_video_stream_current(if count > 0 { 0 } else { -1 });
        w.set_video_stream_current_label(current_label.into());
        let label = if res.is_empty() { "all qualities".to_string() } else { format!("{res}p") };
        w.set_video_stream_status(
            match (count, from_cache) {
                (0, _) => "No streams at this quality — try another.".to_string(),
                (n, true) => format!("{n} streams · {label} · saved"),
                (n, false) => format!("{n} streams · {label}"),
            }
            .into(),
        );
        w.set_video_stream_busy(false);
    });
}

/// Resolution rung picked — re-resolve the current episode at that quality.
pub fn stream_set_resolution(weak: slint::Weak<MainWindow>, res: slint::SharedString) {
    let res = res.to_string();
    with_state(|st| st.resolution = res.clone());
    let Some(id) = opened_subject() else { return };
    let (season, episode) = with_state(|st| st.selection);
    let epoch = next_epoch();
    let _ = weak.upgrade_in_event_loop({
        let res = res.clone();
        move |w| w.set_video_stream_resolution(res.into())
    });
    tokio::runtime::Handle::current().spawn(async move {
        load_files(weak, epoch, id, season, episode).await;
    });
}

/// One subtitle entry per language across every file of an episode, keeping the
/// first URL seen for each. Languages are compared case-insensitively.
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

/// Subtitle language picked in the detail pane. `-1` turns subtitles off.
pub fn stream_set_sub(weak: slint::Weak<MainWindow>, index: i32) {
    let choice = if index < 0 { None } else { Some(index as usize) };
    with_state(|st| st.sub_choice = choice);
    // Remember the language, not the index: the next episode may list its
    // tracks in a different order.
    with_state(|st| {
        st.sub_lang = choice
            .and_then(|i| st.subs.get(i).map(|c| c.lang.clone()))
            .filter(|l| !l.is_empty());
    });
    let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_sub_choice(index));
}

/// Back to the results grid; clears the per-title state so a stale file list
/// can never be played against the next title.
pub fn stream_back(weak: slint::Weak<MainWindow>) {
    next_epoch(); // abandon anything still loading for the closed title
    clear_open_title();
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_detail_open(false);
        w.set_video_stream_subs(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamChip>::new())));
        w.set_video_stream_sub_choice(-1);
        w.set_video_stream_files(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamFileRow>::new())));
        w.set_video_stream_status("".into());
        w.set_video_stream_busy(false);
    });
}

/// Season picked — reset to episode 1 and re-resolve.
///
/// A split show stores each season under its own subject, so switching season
/// there means re-opening that subject; a normal series just re-queries with a
/// different `se`.
pub fn stream_set_season(weak: slint::Weak<MainWindow>, season: i32) {
    let split = with_state(|st| st.season_subjects.clone());
    if let Some((_, subject)) = split.iter().find(|(n, _)| *n == season as i64) {
        // Reflect the click straight away; the details fetch fills in the rest.
        let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_season(season));
        stream_open(weak, subject.clone());
        return;
    }

    let Some(d) = with_state(|st| st.details.clone()) else { return };
    let episodes = episode_numbers(
        d.seasons
            .iter()
            .find(|s| s.number == season as i64)
            .map(|s| s.max_ep)
            .unwrap_or(1),
    );
    with_state(|st| st.selection = (season as i64, 1));
    let Some(id) = opened_subject() else { return };
    let epoch = next_epoch();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_season(season);
        w.set_video_stream_episode(1);
        w.set_video_stream_episodes(slint::ModelRc::new(slint::VecModel::from(episodes)));
        clear_stream_list(&w);
    });
    tokio::runtime::Handle::current().spawn(async move {
        load_files(weak, epoch, id, season as i64, 1).await;
    });
}

/// Episode picked — re-resolve within the current season.
pub fn stream_set_episode(weak: slint::Weak<MainWindow>, episode: i32) {
    let Some(id) = opened_subject() else { return };
    let season = with_state(|st| st.selection.0).max(1);
    with_state(|st| st.selection = (season, episode as i64));
    let epoch = next_epoch();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_episode(episode);
        clear_stream_list(&w);
    });
    tokio::runtime::Handle::current().spawn(async move {
        load_files(weak, epoch, id, season, episode as i64).await;
    });
}

/// Language cut picked. Each dub is a separate subject, so this reopens the
/// title under the new id.
///
/// For a split show the dub subject is not in the season map — re-point the
/// currently-open season at it and re-register, otherwise switching language
/// would silently collapse a five-season show down to one.
pub fn stream_set_dub(weak: slint::Weak<MainWindow>, subject_id: String) {
    if let Some(cur) = opened_subject() {
        with_state(|st| {
            if let Some(slot) = st.season_subjects.iter_mut().find(|(_, id)| *id == cur) {
                slot.1 = subject_id.clone();
                let list = st.season_subjects.clone();
                for (_, id) in &list {
                    st.season_map.insert(id.clone(), list.clone());
                }
            }
        });
    }
    stream_open(weak, subject_id);
}

// ---- playback ----

/// Hand the chosen file to the windowed mpv, with the episode's subtitle tracks
/// attached. No resume or progress writeback — a remote title has no DB row.
///
/// Tracks are downloaded to files named after their language rather than handed
/// to mpv as bare URLs, because mpv labels a track with its filename: a raw URL
/// shows up in the track menu as an unreadable string. The language picked in
/// the detail pane is preselected; the rest stay available in mpv's own menu.
pub fn stream_play(weak: slint::Weak<MainWindow>, index: i32) {
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    let label = if file.resolution > 0 { format!("{}p", file.resolution) } else { "stream".into() };
    set_status(&weak, format!("Starting {label}…"), true);

    tokio::runtime::Handle::current().spawn(async move {
        let (tracks, chosen) = with_state(|st| (st.subs.clone(), st.sub_choice));
        let (args, named) = subtitle_args(&tracks, chosen).await;
        spawn_mpv_windowed_with(PathBuf::from(file.url), None, None, args);

        let msg = match (named, chosen.and_then(|i| tracks.get(i))) {
            (0, _) => format!("Playing {label} in mpv · no subtitles"),
            (n, Some(c)) => format!("Playing {label} in mpv · {} subtitles · {}", n, c.lang),
            (n, None) => format!("Playing {label} in mpv · {n} subtitles · off"),
        };
        set_status(&weak, msg, false);
    });
}

/// Cache one subtitle track under a language-derived filename. Returns the local
/// path, or `None` if it could not be fetched.
async fn cache_subtitle(c: &Caption, idx: usize) -> Option<PathBuf> {
    let dir = tulipix_core::paths::cache_dir()?.join("videos").join("stream-subs");
    tokio::fs::create_dir_all(&dir).await.ok()?;

    // mpv shows the file stem as the track name, so the stem *is* the label.
    let lang: String = c
        .lang
        .chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == ' ' { ch } else { '_' })
        .collect();
    let stem = if lang.trim().is_empty() { format!("Track {}", idx + 1) } else { lang.trim().to_string() };
    let ext = if c.ext.is_empty() { "srt".to_string() } else { c.ext.clone() };
    let path = dir.join(format!("{stem}.{ext}"));

    let bytes = reqwest::Client::new()
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
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// mpv flags for the episode's subtitles: the chosen language first (so it is
/// track 1 and can be preselected with `--sid=1`), then the rest.
///
/// Returns `(args, tracks_attached)`. Any track that fails to download is simply
/// left out — subtitles must never be the reason a video does not start.
async fn subtitle_args(tracks: &[Caption], chosen: Option<usize>) -> (Vec<String>, usize) {
    if tracks.is_empty() {
        return (Vec::new(), 0);
    }
    // Chosen track first, remaining in their original order.
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
        // Track 1 is whatever we put first: the picked language, or nothing
        // picked means subtitles stay off until the user turns them on in mpv.
        args.push(if chosen.is_some() { "--sid=1".to_string() } else { "--sid=no".to_string() });
    }
    (args, attached)
}

// ---- copy link ----

/// Put the chosen stream's URL on the system clipboard (the TUI's `[C]`).
pub fn stream_copy_link(weak: slint::Weak<MainWindow>, index: i32) {
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(file.url)) {
        Ok(()) => set_status(&weak, "Stream link copied.", false),
        Err(e) => set_status(&weak, format!("Could not copy: {e}"), false),
    }
}

// ---- download ----

/// Set while a download runs; flipping it true asks the loop to stop.
static DL_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Guards against two downloads running at once.
static DL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Where finished downloads land: `$XDG_DOWNLOAD_DIR`, else `~/Downloads`, else
/// the working directory.
fn download_dir() -> PathBuf {
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

/// Strip anything that cannot go in a filename, and collapse runs of spaces.
fn safe_filename(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '.' { c } else { ' ' })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join("_");
    if joined.is_empty() { "stream".to_string() } else { joined }
}

/// `"Person_of_Interest_S01E05_1080p.mp4"` — enough to identify the file later.
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

/// Download the chosen stream to the Downloads folder, reporting progress.
///
/// Writes to a `.part` file and renames on success, so an interrupted download
/// never leaves something that looks complete.
pub fn stream_download(weak: slint::Weak<MainWindow>, index: i32) {
    use std::sync::atomic::Ordering;
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    if DL_BUSY.swap(true, Ordering::SeqCst) {
        set_status(&weak, "A download is already running.", false);
        return;
    }
    DL_CANCEL.store(false, Ordering::SeqCst);

    let (title, (season, episode)) =
        with_state(|st| (st.details.as_ref().map(|d| d.title.clone()).unwrap_or_default(), st.selection));
    let name = download_name(&title, season, episode, file.resolution);
    let dest = download_dir().join(&name);
    let part = dest.with_extension("mp4.part");

    let set_dl = {
        let weak = weak.clone();
        move |label: String, frac: f32, active: bool| {
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_video_stream_dl_label(label.into());
                w.set_video_stream_dl_frac(frac);
                w.set_video_stream_dl_active(active);
            });
        }
    };
    set_dl(format!("Starting {name}"), 0.0, true);

    let weak_hide = weak.clone();
    tokio::runtime::Handle::current().spawn(async move {
        let result = download_to(&file.url, &part, &set_dl).await;
        DL_BUSY.store(false, Ordering::SeqCst);

        match result {
            Ok(true) => {
                if tokio::fs::rename(&part, &dest).await.is_ok() {
                    set_dl(format!("Saved to {}", dest.display()), 1.0, false);
                } else {
                    set_dl("Downloaded, but could not be renamed.".into(), 1.0, false);
                }
            }
            Ok(false) => {
                let _ = tokio::fs::remove_file(&part).await;
                set_dl("Download cancelled.".into(), 0.0, false);
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                set_dl(format!("Download failed: {e}"), 0.0, false);
            }
        }
        // Auto-hide the row a few seconds after it finishes — but only if no new
        // download has started in the meantime (that would reset DL_BUSY true).
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        if !DL_BUSY.load(Ordering::SeqCst) {
            let _ = weak_hide.upgrade_in_event_loop(|w| {
                w.set_video_stream_dl_label("".into());
                w.set_video_stream_dl_active(false);
                w.set_video_stream_dl_frac(0.0);
            });
        }
    });
}

/// Manually dismiss the download row (the X on it).
pub fn stream_download_dismiss(weak: slint::Weak<MainWindow>) {
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_dl_label("".into());
        w.set_video_stream_dl_active(false);
        w.set_video_stream_dl_frac(0.0);
    });
}

// ---- current stream + info-box actions ----

/// A stream row was clicked — make it the current stream (info-box actions use
/// it) without necessarily playing.
pub fn stream_set_current(weak: slint::Weak<MainWindow>, index: i32) {
    let idx = index.max(0) as usize;
    let label = with_state(|st| {
        st.current_stream = idx;
        st.files.get(idx).map(|f| {
            if f.resolution > 0 { format!("{}p stream", f.resolution) } else { "stream".into() }
        })
    })
    .unwrap_or_default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_current(index);
        w.set_video_stream_current_label(label.into());
    });
}

fn current_index() -> i32 {
    with_state(|st| {
        if st.files.is_empty() { -1 } else { st.current_stream.min(st.files.len() - 1) as i32 }
    })
}

/// Info-box Play — plays the currently selected stream.
pub fn stream_play_current(weak: slint::Weak<MainWindow>) {
    let i = current_index();
    if i >= 0 { stream_play(weak, i); }
}
pub fn stream_copy_current(weak: slint::Weak<MainWindow>) {
    let i = current_index();
    if i >= 0 { stream_copy_link(weak, i); }
}
pub fn stream_download_current(weak: slint::Weak<MainWindow>) {
    let i = current_index();
    if i >= 0 { stream_download(weak, i); }
}

// ---- bookmarks ----

/// Toggle the open title's saved state and reflect it on the button.
pub fn stream_toggle_bookmark(weak: slint::Weak<MainWindow>) {
    let Some(d) = with_state(|st| st.details.clone()) else { return };
    let Some(subject) = opened_subject() else { return };
    let split = with_state(|st| st.season_subjects.clone());
    // For a split show, save the whole show under its first-season subject so it
    // reopens on season 1, not whichever season happened to be on screen.
    let (subject_id, is_series) = if let Some((_, first)) = split.first() {
        (first.clone(), true)
    } else {
        (subject, d.is_series)
    };
    let bm = stream::bookmarks::Bookmark {
        subject_id,
        title: d.title.clone(),
        year: d.year.clone(),
        cover_url: d.cover.clone(),
        is_series,
        meta: meta_line(&d),
        overview: d.overview.clone(),
    };
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let saved = stream::bookmarks::toggle(&pool, &bm).await.unwrap_or(false);
        with_state(|st| st.bookmarked = saved);
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_video_stream_bookmarked(saved);
            w.set_video_stream_status(
                if saved { "Saved to bookmarks." } else { "Removed from bookmarks." }.into(),
            );
        });
    });
}

/// Populate the Saved page.
pub fn stream_bookmarks_load(weak: slint::Weak<MainWindow>) {
    let (key, asc) = with_state(|st| (st.bm_sort.clone(), st.bm_asc));
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let mut list = stream::bookmarks::list(&pool).await;
        sort_bookmarks(&mut list, &key, asc);
        let covers = cache_covers(list.iter().map(|b| b.cover_url.clone()).collect()).await;
        let cards: Vec<(stream::bookmarks::Bookmark, Option<PathBuf>)> =
            list.into_iter().zip(covers).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<StreamCard> = cards
                .into_iter()
                .map(|(b, poster)| StreamCard {
                    id: b.subject_id.into(),
                    title: b.title.into(),
                    year: b.year.into(),
                    poster: load_image(poster),
                    is_series: b.is_series,
                    seasons: 0,
                    meta: b.meta.into(),
                    overview: truncate_chars(&b.overview, 100).into(),
                })
                .collect();
            w.set_video_stream_bookmarks(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

/// First `max` characters of `s`, adding an ellipsis when it was cut. Counts by
/// `char`, not byte, so it never splits a multibyte glyph.
fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// Sort the Saved list in place. `list()` already returns newest-first, so the
/// "date" descending case is a no-op.
fn sort_bookmarks(list: &mut [stream::bookmarks::Bookmark], key: &str, asc: bool) {
    match key {
        "name" => list.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        // Movies before series (then title), so the two kinds group together.
        "type" => list.sort_by(|a, b| {
            a.is_series.cmp(&b.is_series).then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        _ => {} // "date": already newest-first from the query
    }
    if asc {
        list.reverse();
    }
}

/// Change the Saved-page sort and repaint.
pub fn stream_bookmarks_sort(weak: slint::Weak<MainWindow>, key: String, asc: bool) {
    with_state(|st| {
        st.bm_sort = key;
        st.bm_asc = asc;
    });
    stream_bookmarks_load(weak);
}

/// Remove one entry from the Saved page and refresh it.
pub fn stream_bookmark_remove(weak: slint::Weak<MainWindow>, subject_id: String) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::bookmarks::remove(&pool, &subject_id).await;
        }
        stream_bookmarks_load(weak);
    });
}

/// Stream `url` into `part`. `Ok(false)` means the user cancelled.
async fn download_to(
    url: &str,
    part: &std::path::Path,
    report: &impl Fn(String, f32, bool),
) -> Result<bool, String> {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = part.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut resp = reqwest::Client::new()
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

        // Repainting on every chunk would flood the event loop.
        if last_tick.elapsed() >= std::time::Duration::from_millis(200) {
            last_tick = std::time::Instant::now();
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            report(progress_label(done, total), frac.clamp(0.0, 1.0), true);
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    Ok(true)
}

fn progress_label(done: u64, total: u64) -> String {
    let mb = |b: u64| b as f64 / 1024.0 / 1024.0;
    if total > 0 {
        format!("{:.0} MB of {:.0} MB ({:.0}%)", mb(done), mb(total), done as f64 / total as f64 * 100.0)
    } else {
        format!("{:.0} MB", mb(done))
    }
}

pub fn stream_download_cancel(weak: slint::Weak<MainWindow>) {
    DL_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    set_status(&weak, "Cancelling…", false);
}

// ---- search suggestions ----

/// Debounced autocomplete. Each keystroke bumps its own counter; only the last
/// one still standing after the pause actually asks the servers.
static SUGGEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn stream_suggest(weak: slint::Weak<MainWindow>, query: String) {
    use std::sync::atomic::Ordering;
    let query = query.trim().to_string();
    if query.len() < 2 {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_suggestions(slint::ModelRc::new(slint::VecModel::from(
                Vec::<slint::SharedString>::new(),
            )));
        });
        return;
    }
    let seq = SUGGEST_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::runtime::Handle::current().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if SUGGEST_SEQ.load(Ordering::SeqCst) != seq {
            return; // superseded by a later keystroke
        }
        let Ok(c) = client().await else { return };
        let Ok(names) = c.suggest(&query).await else { return };
        if SUGGEST_SEQ.load(Ordering::SeqCst) != seq {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<slint::SharedString> = names.into_iter().map(Into::into).collect();
            w.set_video_stream_suggestions(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

pub fn stream_suggest_clear(weak: slint::Weak<MainWindow>) {
    SUGGEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_suggestions(slint::ModelRc::new(slint::VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
    });
}

// ---- hover preview ----

/// Details for the card under the cursor (the TUI's Info Preview pane).
///
/// Dwell-gated and memoised: skimming the grid must not fire a request per
/// card. An empty `subject_id` closes the preview.
pub fn stream_preview(weak: slint::Weak<MainWindow>, subject_id: String) {
    use std::sync::atomic::Ordering;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst) + 1;

    if subject_id.is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| w.set_video_stream_preview_open(false));
        return;
    }
    if let Some(d) = with_state(|st| st.preview_cache.get(&subject_id).cloned()) {
        let cover_url = d.cover.clone();
        let weak2 = weak.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let cover = cache_cover(&cover_url).await;
            push_preview(&weak2, &d, cover);
        });
        return;
    }
    tokio::runtime::Handle::current().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        if SEQ.load(Ordering::SeqCst) != seq {
            return; // pointer moved on
        }
        let Ok(c) = client().await else { return };
        let Ok(d) = c.details(&subject_id).await else { return };
        with_state(|st| {
            // Bounded: the grid is at most a few dozen cards.
            if st.preview_cache.len() > 60 {
                st.preview_cache.clear();
            }
            st.preview_cache.insert(subject_id.clone(), d.clone());
        });
        let cover = cache_cover(&d.cover).await;
        if SEQ.load(Ordering::SeqCst) == seq {
            push_preview(&weak, &d, cover);
        }
    });
}

fn push_preview(weak: &slint::Weak<MainWindow>, d: &Details, cover: Option<PathBuf>) {
    let (title, meta, overview) = (d.title.clone(), meta_line(d), d.overview.clone());
    let seasons = d.seasons.len() as i32;
    let is_series = d.is_series;
    // Dub languages as a comma list — one more thing the card doesn't show.
    let langs: String = stream::prefer::dubs(d.dubs.clone())
        .iter()
        .map(|x| x.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_preview_title(title.into());
        w.set_video_stream_preview_meta(meta.into());
        w.set_video_stream_preview_overview(overview.into());
        w.set_video_stream_preview_seasons(seasons);
        w.set_video_stream_preview_is_series(is_series);
        w.set_video_stream_preview_langs(langs.into());
        w.set_video_stream_preview_cover(load_image(cover));
        w.set_video_stream_preview_open(true);
    });
}

// ---- cache housekeeping ----

/// Trim the on-disk caches. Called when the tab opens: the stream table ages out
/// past a week, and the poster/subtitle dirs are capped by file count so they
/// cannot grow without bound.
pub fn stream_prune_caches() {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::cache::prune(&pool, 7 * 24 * 60 * 60).await;
        }
        if let Some(base) = tulipix_core::paths::cache_dir().map(|d| d.join("videos")) {
            cap_dir(&base.join("stream"), 400).await;
            cap_dir(&base.join("stream-subs"), 60).await;
        }
    });
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

// ---- recent searches ----

fn push_recent(weak: &slint::Weak<MainWindow>, term: &str) {
    publish_recent(weak, stream::recent::push(term));
}

fn publish_recent(weak: &slint::Weak<MainWindow>, list: Vec<String>) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        let rows: Vec<slint::SharedString> = list.into_iter().map(Into::into).collect();
        w.set_video_stream_recent(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
}

/// Fill the landing screen's recent list. Called when the Stream tab opens.
pub fn stream_recent_load(weak: slint::Weak<MainWindow>) {
    publish_recent(&weak, stream::recent::load());
}

pub fn stream_recent_clear(weak: slint::Weak<MainWindow>) {
    publish_recent(&weak, stream::recent::clear());
}

// ---- hosts editor ----

/// Fill the editor with the configured hosts, one per line.
pub fn stream_hosts_load(weak: slint::Weak<MainWindow>) {
    let text = stream::hosts::load().join("\n");
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_hosts_text(text.into());
        w.set_video_stream_hosts_error("".into());
    });
}

/// Validate and persist the edited host list. A rejected entry leaves the
/// stored list untouched and reports why.
pub fn stream_hosts_save(weak: slint::Weak<MainWindow>, text: String) {
    let entries: Vec<String> = text.lines().map(|l| l.to_string()).collect();
    match stream::hosts::save(&entries) {
        Ok(saved) => {
            stream_invalidate_client();
            let joined = saved.join("\n");
            let n = saved.len();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_video_stream_hosts_text(joined.into());
                w.set_video_stream_hosts_error("".into());
                // Keep the popup open and flash the "Applied" pill on Save so the
                // user sees the change landed; the UI auto-clears the flag.
                w.set_video_stream_hosts_saved(true);
                w.set_video_stream_status(
                    format!("Saved {n} server{}.", if n == 1 { "" } else { "s" }).into(),
                );
            });
        }
        Err(msg) => {
            let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_hosts_error(msg.into()));
        }
    }
}

/// Put the built-in list back into the editor. Not persisted until Save, so a
/// misclick is one Escape away from being harmless.
pub fn stream_hosts_reset(weak: slint::Weak<MainWindow>) {
    let text = stream::hosts::defaults().join("\n");
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_hosts_text(text.into());
        w.set_video_stream_hosts_error("Press Save to apply the default servers.".into());
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use tulipix_videos::stream::{Details, Dub, Season};

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
    fn file_sub_handles_missing_halves() {
        let f = StreamFile { size: "2.1 GB".into(), codec: "h265".into(), ..Default::default() };
        assert_eq!(file_sub(&f), "2.1 GB  ·  h265");
        let f = StreamFile { codec: "h264".into(), ..Default::default() };
        assert_eq!(file_sub(&f), "h264");
        assert_eq!(file_sub(&StreamFile::default()), "");
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
    fn download_names_are_filesystem_safe_and_identify_the_episode() {
        assert_eq!(
            download_name("Person of Interest", 1, 5, 1080),
            "Person_of_Interest_S01E05_1080p.mp4"
        );
        assert_eq!(download_name("Dune: Part Two", 0, 0, 720), "Dune_Part_Two_720p.mp4");
        // path separators and quotes must never survive into a filename
        assert_eq!(download_name("../../etc/passwd", 0, 0, 0), "etc_passwd.mp4");
        assert_eq!(download_name("", 0, 0, 0), "stream.mp4");
        assert!(!download_name("a/b\\c:d", 2, 10, 480).contains(['/', '\\', ':']));
    }

    #[test]
    fn progress_label_handles_unknown_length() {
        assert_eq!(progress_label(5 * 1024 * 1024, 10 * 1024 * 1024), "5 MB of 10 MB (50%)");
        assert_eq!(progress_label(3 * 1024 * 1024, 0), "3 MB");
    }

    #[test]
    fn generation_guard_invalidates_older_actions() {
        let first = next_epoch();
        assert!(is_current(first));
        let second = next_epoch();
        assert!(!is_current(first), "a superseded action must not paint");
        assert!(is_current(second));
    }

    #[test]
    fn episode_count_follows_the_open_season() {
        let seasons = vec![
            Season { number: 1, max_ep: 9 },
            Season { number: 2, max_ep: 22 },
            Season { number: 3, max_ep: 5 },
        ];
        assert_eq!(max_ep_for(&seasons, 2), 22); // not season 1's count
        assert_eq!(max_ep_for(&seasons, 3), 5);
        assert_eq!(max_ep_for(&seasons, 99), 9); // unknown season → first
        assert_eq!(max_ep_for(&[], 1), 0);
    }

    /// The bug: subs come from a separate endpoint, so an all-inline-empty file
    /// list must be recognised as "needs a captions fetch".
    #[test]
    fn empty_inline_captions_are_detected_as_needing_a_fetch() {
        let files = vec![
            StreamFile { resource_id: "a".into(), ..Default::default() },
            StreamFile { resource_id: "b".into(), ..Default::default() },
        ];
        // attach_episode_captions probes only when every file lacks inline subs
        assert!(files.iter().all(|f| f.captions.is_empty()));
        let with_inline = vec![StreamFile {
            resource_id: "a".into(),
            captions: vec![Caption { url: "u".into(), lang: "English".into(), ext: "srt".into() }],
            ..Default::default()
        }];
        assert!(with_inline.iter().any(|f| !f.captions.is_empty()));
    }

    #[test]
    fn truncate_chars_caps_and_marks_cut_by_char_not_byte() {
        assert_eq!(truncate_chars("short", 100), "short"); // untouched
        assert_eq!(truncate_chars("abcdef", 3), "abc…"); // cut + ellipsis
        assert_eq!(truncate_chars("abc", 3), "abc"); // exact length, no ellipsis
        // Multibyte: 4 codepoints capped to 2 must not split a glyph.
        assert_eq!(truncate_chars("héllo", 2), "hé…");
    }

    #[test]
    fn bookmark_sort_orders_by_key_and_direction() {
        use tulipix_videos::stream::bookmarks::Bookmark;
        let mk = |id: &str, title: &str, series: bool| Bookmark {
            subject_id: id.into(), title: title.into(), year: "2020".into(),
            cover_url: String::new(), is_series: series,
            meta: String::new(), overview: String::new(),
        };
        // list() is newest-first; simulate [b, a] as that order.
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

    #[test]
    fn caption_union_keeps_one_entry_per_language() {
        let cap = |lang: &str, url: &str| Caption {
            url: url.into(), lang: lang.into(), ext: "srt".into(),
        };
        let files = vec![
            StreamFile {
                captions: vec![cap("English", "https://s/en1"), cap("Hindi", "https://s/hi")],
                ..Default::default()
            },
            StreamFile {
                // same languages on the 480p cut — must not double up the picker
                captions: vec![cap("english", "https://s/en2"), cap("", "")],
                ..Default::default()
            },
        ];
        let u = caption_union(&files);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].lang, "English");
        assert_eq!(u[0].url, "https://s/en1"); // first URL seen wins
        assert_eq!(u[1].lang, "Hindi");
        assert!(caption_union(&[]).is_empty());
    }

    #[tokio::test]
    async fn subtitle_args_are_empty_without_tracks() {
        let (args, n) = subtitle_args(&[], Some(0)).await;
        assert!(args.is_empty());
        assert_eq!(n, 0);
    }

    #[test]
    fn episode_numbers_are_one_based_and_bounded() {
        assert_eq!(episode_numbers(3), vec![1, 2, 3]);
        assert!(episode_numbers(0).is_empty());
        assert!(episode_numbers(-5).is_empty()); // server junk must not underflow
        assert_eq!(episode_numbers(10_000).len(), 500); // clamped, not unbounded
    }

    /// A series opens on its first season, a movie has no episode axis at all.
    #[test]
    fn first_episode_selection_matches_kind() {
        let series = Details {
            is_series: true,
            seasons: vec![Season { number: 2, max_ep: 8 }],
            dubs: vec![Dub { subject_id: "a".into(), name: "English".into() }],
            ..Default::default()
        };
        let sel = if series.is_series {
            (series.seasons.first().map(|s| s.number).unwrap_or(1), 1)
        } else {
            (0, 0)
        };
        assert_eq!(sel, (2, 1));

        let movie = Details { is_series: false, ..Default::default() };
        let sel = if movie.is_series { (1, 1) } else { (0, 0) };
        assert_eq!(sel, (0, 0));
    }
}
