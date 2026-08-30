//! Music section extracted from tulipix-app: My Music / Podcasts / Radio /
//! Audiobooks + the YouTube tabs, all driven by one unified out-of-process
//! player. The shared playback core + now-playing surface live in
//! tulipix-common; the music `window.on_*` callbacks in main reach these via
//! `use tulipix_sec_music::*`. (A few book-reader bookmark helpers ride along
//! because they were physically nested in the music region; they relocate when
//! Books is extracted.)

#![allow(clippy::too_many_arguments)]

pub mod analysis;
pub mod mdl;

use std::sync::OnceLock;
use std::path::PathBuf;
use anyhow::Result;
use slint::{ComponentHandle, Model};
use tulipix_core::proc::NoWindow;
use tulipix_ui::*;
use tulipix_common::*;

// Map music tile index → absolute path for playback.
static MUSIC_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
pub fn music_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    MUSIC_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Accumulated music tracks across ALL watched folders: (label, abs, thumb).
// Like photo_full/video_full, this is the source of truth the tiles + paths are
// rebuilt from, so adding a second folder ACCUMULATES instead of replacing the
// first. Deduped by abs path.
static MUSIC_FULL: std::sync::OnceLock<std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>>> = std::sync::OnceLock::new();
pub fn music_full() -> &'static std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>> {
    MUSIC_FULL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// Rebuild the music tiles + playback-path list from the accumulated
/// `music_full` set. Each tile's `index` is its playback position.
///
/// `music_tiles` stays position-aligned and COMPLETE (audiobook chapters
/// included) because rails/albums/audiobook cards resolve labels and thumbs by
/// playback position. It carries **no images**: see [`music_thumb_at`].
///
/// The My Music Tracks grid used to bind a second, filtered copy of this model
/// (`music_tracks_grid`) — every track outside a non-"My Music" folder, each
/// with its own decoded thumbnail. Nothing ever iterated it: `page_music.slint`
/// only read `tiles.length` to pick between the library and the empty state,
/// and the Songs list renders from `music_song_rows`, 35 at a time. The model
/// is gone; `music_track_count` is that length.
pub fn rebuild_music_tiles(w: &MainWindow) {
    shuffle_reset(); // playback indices shift with the tiles — stale order dies here
    let full = music_full().lock().map(|g| g.clone()).unwrap_or_default();
    let sections = load_folder_sections();
    let excluded: Vec<PathBuf> = sections.iter()
        .filter(|(_, key)| key.as_str() != "mymusic")
        .map(|(folder, _)| PathBuf::from(folder))
        .collect();
    let mut paths: Vec<PathBuf> = Vec::with_capacity(full.len());
    let mut tiles: Vec<PhotoTile> = Vec::with_capacity(full.len());
    let mut in_grid = 0i32;
    for (i, (label, orig, _thumb)) in full.iter().enumerate() {
        if !excluded.iter().any(|e| orig.starts_with(e)) {
            in_grid += 1;
        }
        tiles.push(PhotoTile {
            label: label.clone().into(),
            index: i as i32,
            ..Default::default()
        });
        paths.push(orig.clone());
    }
    // Positions just moved; anything memoised against the old ones is wrong.
    clear_thumb_cache();
    if let Ok(mut g) = music_paths().lock() { *g = paths; }
    w.set_music_track_count(in_grid);
    w.set_music_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
}

// Decoded track thumbnails, keyed by playback position, bounded.
//
// `rebuild_music_tiles` used to call `Image::load_from_path` for every track in
// the library and keep the result in two root-level Slint models. That is an
// eager full decode — ~300 KB per 320×320 thumb — held for the life of the
// process, twice, for a library that renders at most a few dozen thumbs at a
// time. On a 10k-track library it was the largest single allocation in the app.
// Same LRU shape as the Live TV logo and book cover caches, but `thread_local!`
// rather than a static: `slint::Image` is neither `Send` nor `Sync` (its
// `ImageInner` holds a `VRc`), so it cannot live in a `static` at all. Same
// reason `MUSIC_BROWSE` above is a thread-local. Every caller here runs on the
// Slint event-loop thread — the fns take `&MainWindow`, or sit inside an
// `upgrade_in_event_loop` closure — so one thread's copy is the only copy.
const THUMB_CACHE_MAX: usize = 256;
type ThumbCache = (std::collections::HashMap<usize, slint::Image>, std::collections::VecDeque<usize>);
thread_local! {
    static THUMB_CACHE: std::cell::RefCell<ThumbCache> = std::cell::RefCell::new(Default::default());
}

/// Drop every memoised thumb. Called whenever playback positions shift (the
/// cache is keyed by position) and when the section is unloaded.
pub fn clear_thumb_cache() {
    THUMB_CACHE.with(|c| { let mut c = c.borrow_mut(); c.0.clear(); c.1.clear(); });
}

/// Thumbnail for a playback position, decoded on first ask.
///
/// The path comes from `music_full`, which is position-aligned with
/// `music_paths` and `music_tiles` by construction — one push per entry in
/// `rebuild_music_tiles`. Out-of-range and un-decodable both give the default
/// image, which is what the old `tiles.row_data(pos).thumb` returned too.
pub fn music_thumb_at(pos: i32) -> slint::Image {
    if pos < 0 { return slint::Image::default(); }
    let pos = pos as usize;
    // Borrow ends before the decode — `load_from_path` must never run while the
    // cache is borrowed, or a re-entrant ask would panic the RefCell.
    if let Some(img) = THUMB_CACHE.with(|c| c.borrow().0.get(&pos).cloned()) {
        return img;
    }
    let path = music_full().lock().ok().and_then(|g| g.get(pos).map(|(_, _, t)| t.clone()));
    let Some(path) = path else { return slint::Image::default() };
    let img = slint::Image::load_from_path(&path).unwrap_or_default();
    THUMB_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if c.0.insert(pos, img.clone()).is_none() {
            c.1.push_back(pos);
            while c.1.len() > THUMB_CACHE_MAX {
                if let Some(old) = c.1.pop_front() { c.0.remove(&old); }
            }
        }
    });
    img
}
/// Drop every accumulated music track whose abs path is under `dir` (used when
/// a folder is removed from the library so its tiles disappear without a rescan).
pub fn prune_music_full_under(dir: &std::path::Path) {
    if let Ok(mut g) = music_full().lock() {
        g.retain(|(_, orig, _)| !orig.starts_with(dir));
    }
}
// Parallel to music_paths: the music.db item_id at each playback position, so
// item_id-keyed features (dashboard / browse / rating) map back to a position.
static MUSIC_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
pub fn music_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    MUSIC_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// item_id of the track at the current now-playing position (None if unknown).
pub fn current_music_id(w: &MainWindow) -> Option<i64> {
    music_id_at(w.get_music_np_index())
}

/// item_id at an arbitrary playback position.
pub fn music_id_at(idx: i32) -> Option<i64> {
    if idx < 0 { return None; }
    music_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).filter(|id| *id >= 0)
}

// ── Context queue (np.p6.music.context-queue) ───────────────────────────────
// Playing a track from a list means playing THAT list: the persisted play_queue
// is replaced with the rest of the list, in the order the page showed it, and
// every advance — auto or Next — pops from it. Set while a context play is in
// flight so `play_music_at` does not seed its sonic-similar mix over the top of
// a queue that is about to be written.
static CTX_QUEUE_PENDING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Arm the flag for callers that start the track themselves and hand the list
/// to `set_context_queue` separately.
pub fn set_ctx_pending() { CTX_QUEUE_PENDING.store(true, std::sync::atomic::Ordering::SeqCst); }

/// Replace the queue with `order` (item ids) starting after `from_id`, wrapping
/// round to the top of the list. `from_id` missing from the list = the whole
/// list, minus nothing.
pub fn set_context_queue(w: &MainWindow, order: Vec<i64>, from_id: Option<i64>, source: &'static str) {
    let start = from_id.and_then(|id| order.iter().position(|x| *x == id));
    let rest: Vec<i64> = match start {
        Some(s) => (1..order.len()).map(|off| order[(s + off) % order.len()]).collect(),
        None => order,
    };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("music").await {
            let _ = tulipix_music::queue::replace(&pool, &rest, source).await;
        }
        CTX_QUEUE_PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = weak.upgrade_in_event_loop(|w| build_music_queue(&w));
    });
}

/// Drop the queue entirely: "Play all" / "Shuffle all" walk the whole library,
/// and a queue left over from an album would hijack the very first advance.
/// Arms the same flag as a context play, so the track started right after this
/// does not seed a sonic-similar mix into the queue being emptied.
pub fn clear_context_queue(w: &MainWindow) {
    set_ctx_pending();
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("music").await {
            let _ = tulipix_music::queue::clear(&pool).await;
        }
        CTX_QUEUE_PENDING.store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = weak.upgrade_in_event_loop(|w| build_music_queue(&w));
    });
}

/// Play library position `pos` as part of `order` (library positions, in the
/// order the page lists them) and make the queue that list's tail.
pub fn play_in_context(w: &MainWindow, order: &[i32], pos: i32, source: &'static str) {
    let ids: Vec<i64> = {
        let Ok(g) = music_ids().lock() else { return play_music_at(w, pos) };
        order.iter().filter_map(|p| g.get(*p as usize).copied()).filter(|id| *id >= 0).collect()
    };
    let from = music_id_at(pos);
    if ids.is_empty() || from.is_none() { return play_music_at(w, pos); }
    CTX_QUEUE_PENDING.store(true, std::sync::atomic::Ordering::SeqCst);
    play_music_at(w, pos);
    set_context_queue(w, ids, from, source);
}

/// The Songs page as it currently reads — same audiobook/search filter and the
/// live sort — as library positions. This is the list a click in the Songs page
/// plays from, so the queue has to be built from exactly it.
pub fn songs_view_order() -> Vec<i32> {
    let q = music_query_filter().lock().map(|s| s.to_lowercase()).unwrap_or_default();
    let Ok(g) = music_songs().lock() else { return Vec::new(); };
    g.iter().filter(|s| {
        !s.is_audiobook
            && (q.is_empty() || s.title.to_lowercase().contains(&q)
                || s.artist.to_lowercase().contains(&q) || s.album.to_lowercase().contains(&q))
    }).map(|s| s.pos).collect()
}

/// The inverse: playback position of an item_id, for "play THAT chapter".
pub fn music_pos_of(item_id: i64) -> Option<i32> {
    music_ids().lock().ok()
        .and_then(|g| g.iter().position(|id| *id == item_id))
        .map(|p| p as i32)
}

/// One track's metadata for the detailed, sortable Songs list.
#[derive(Clone)]
pub struct SongMeta { pub pos: i32, pub item_id: i64, pub title: String, pub artist: String, pub album: String, pub duration_s: f64, pub added: i64, pub plays: i64, pub loved: bool, pub stars: i32, pub synced: bool, pub release_date: String, pub is_audiobook: bool }
static MUSIC_SONGS: std::sync::OnceLock<std::sync::Mutex<Vec<SongMeta>>> = std::sync::OnceLock::new();
pub fn music_songs() -> &'static std::sync::Mutex<Vec<SongMeta>> {
    MUSIC_SONGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
const SONG_PAGE: usize = 32;
const RECENT_PAGE: usize = 6;
static MUSIC_QUERY: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn music_query_filter() -> &'static std::sync::Mutex<String> {
    MUSIC_QUERY.get_or_init(|| std::sync::Mutex::new(String::new()))
}
// Recently-played pool (up to 18) for the Home 6-per-page × 3-page pager.
static MUSIC_RECENT: std::sync::OnceLock<std::sync::Mutex<Vec<SongMeta>>> = std::sync::OnceLock::new();
pub fn music_recent() -> &'static std::sync::Mutex<Vec<SongMeta>> {
    MUSIC_RECENT.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Browse sources (tile + sort-count) per tab. PhotoTile holds a slint Image
// (not Send/Sync) so this lives in a UI-thread thread_local, not a static.
thread_local! {
    static MUSIC_BROWSE: std::cell::RefCell<std::collections::HashMap<&'static str, Vec<(PhotoTile, i64)>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    // Full track list of the open album/artist/genre detail (display paginated 25/page).
    pub static DETAIL_ROWS: std::cell::RefCell<Vec<MusicSongRow>> = const { std::cell::RefCell::new(Vec::new()) };
    // Artist detail — the artist's albums column (2/row, 8/page, follows the track page).
    static DETAIL_ARTIST_ALBUMS: std::cell::RefCell<Vec<PhotoTile>> = const { std::cell::RefCell::new(Vec::new()) };
    // Metadata manager — every song's row (display paginated 30/page).
    static META_ROWS: std::cell::RefCell<Vec<MetaMgrRow>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Split a filename stem on " - " into (artist, title, album?) per the user's
/// "Artist - Title - Album" naming convention (np.p5.atmusic.metadata-manager).
pub fn parse_music_filename(stem: &str) -> (String, String, Option<String>) {
    let parts: Vec<&str> = stem.split(" - ").map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 => (String::new(), stem.trim().to_string(), None),
        1 => (String::new(), parts[0].to_string(), None),
        2 => (parts[0].to_string(), parts[1].to_string(), None),
        _ => (parts[0].to_string(), parts[1].to_string(), Some(parts[2..].join(" - "))),
    }
}

/// Look the filename up on MusicBrainz and write the best match's title /
/// artist / album / year / genre back to `track_meta` (creating artist/album
/// rows as needed). Returns the fetched metadata for UI feedback.
pub async fn fetch_and_store_meta(
    pool: &sqlx::SqlitePool, client: &reqwest::Client, item_id: i64, stem: &str,
) -> anyhow::Result<Option<tulipix_music::musicbrainz::FetchedMeta>> {
    // Make sure the extra columns exist (idempotent).
    let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(pool).await;
    // Respect the user-lock — manually edited songs are never auto-overwritten.
    let locked: i64 = sqlx::query_scalar("SELECT COALESCE(user_locked, 0) FROM track_meta WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await.ok().flatten().unwrap_or(0);
    if locked != 0 { return Ok(None); }
    let (artist_q, title_q, album_q) = parse_music_filename(stem);
    if title_q.is_empty() { return Ok(None); }
    let search = tulipix_music::musicbrainz::lookup_recording(client, &artist_q, &title_q).await?;
    let Some(meta) = tulipix_music::musicbrainz::best_metadata(&search) else { return Ok(None); };
    // Prefer MB values, fall back to the parsed filename where MB is blank.
    let f_title  = if meta.title.is_empty()  { title_q.clone() }  else { meta.title.clone() };
    let f_artist = if meta.artist.is_empty() { artist_q.clone() } else { meta.artist.clone() };
    let f_album  = if meta.album.is_empty()  { album_q.unwrap_or_default() } else { meta.album.clone() };
    let artist_id = if f_artist.is_empty() { None }
        else { tulipix_music::scan::get_or_create_artist(pool, &f_artist).await.ok() };
    let album_id = if f_album.is_empty() { None }
        else { tulipix_music::scan::get_or_create_album(pool, &f_album, artist_id, meta.year).await.ok() };
    let credits = if meta.credits.is_empty() { None } else { Some(meta.credits.clone()) };
    sqlx::query(
        "UPDATE track_meta SET title = ?, artist_id = COALESCE(?, artist_id),
            album_id = COALESCE(?, album_id), genre = COALESCE(?, genre), year = COALESCE(?, year),
            release_date = COALESCE(?, release_date), credits = COALESCE(?, credits)
         WHERE item_id = ?")
        .bind(&f_title).bind(artist_id).bind(album_id)
        .bind(&meta.genre).bind(meta.year)
        .bind(&meta.release_date).bind(&credits).bind(item_id)
        .execute(pool).await?;
    Ok(Some(meta))
}

/// Build the metadata-manager row list from the live song + path tables.
pub fn build_meta_rows(w: &MainWindow) {
    let songs = music_songs().lock().map(|g| g.clone()).unwrap_or_default();
    let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
    // My Music only — audiobook chapters have their own section and would
    // flood the manager with untagged rows.
    let rows: Vec<MetaMgrRow> = songs.iter().filter(|s| !s.is_audiobook).map(|s| {
        let file = paths.get(s.pos as usize)
            .and_then(|p| p.file_stem()).and_then(|x| x.to_str()).unwrap_or("").to_string();
        // A song counts as "Tagged" when it already carries both artist + album.
        let tagged = !s.artist.trim().is_empty() && !s.album.trim().is_empty();
        MetaMgrRow {
            file: file.into(), title: s.title.clone().into(), artist: s.artist.clone().into(),
            album: s.album.clone().into(), status: if tagged { "Tagged".into() } else { "Pending".into() }, index: s.pos,
        }
    }).collect();
    w.set_music_meta_mgr_total(rows.len() as i32);
    META_ROWS.with(|r| *r.borrow_mut() = rows);
}

/// True when a metadata row counts as already-tagged (matched online or carries tags).
pub fn meta_row_tagged(status: &str) -> bool { status == "Tagged" || status == "Matched" }

/// Publish the current metadata-manager page (30 rows) + counts + page count,
/// filtered by the active stat-card filter (all | tagged | missing).
pub fn publish_meta_page(w: &MainWindow) {
    const PER: usize = 30;
    META_ROWS.with(|r| {
        let all = r.borrow();
        let total = all.len();
        let tagged = all.iter().filter(|x| meta_row_tagged(&x.status)).count();
        w.set_music_meta_mgr_total(total as i32);
        w.set_music_meta_mgr_tagged(tagged as i32);
        w.set_music_meta_mgr_missing((total - tagged) as i32);
        let filter = w.get_music_meta_mgr_filter().to_string();
        let filtered: Vec<MetaMgrRow> = all.iter().filter(|x| match filter.as_str() {
            "tagged" => meta_row_tagged(&x.status),
            "missing" => !meta_row_tagged(&x.status),
            _ => true,
        }).cloned().collect();
        let pages = filtered.len().div_ceil(PER).max(1);
        let page = (w.get_music_meta_mgr_page().max(0) as usize).min(pages - 1);
        w.set_music_meta_mgr_pages(pages as i32);
        w.set_music_meta_mgr_page(page as i32);
        let slice: Vec<MetaMgrRow> = filtered.iter().skip(page * PER).take(PER).cloned().collect();
        w.set_music_meta_mgr_rows(slint::ModelRc::new(slint::VecModel::from(slice)));
    });
}

/// Update one metadata row's status (and tags, when a match was found), then
/// republish the visible page.
pub fn update_meta_row(w: &MainWindow, pos: i32, status: &str, fetched: Option<&tulipix_music::musicbrainz::FetchedMeta>) {
    META_ROWS.with(|r| {
        let mut all = r.borrow_mut();
        if let Some(row) = all.iter_mut().find(|x| x.index == pos) {
            row.status = status.into();
            if let Some(f) = fetched {
                if !f.title.is_empty()  { row.title  = f.title.clone().into(); }
                if !f.artist.is_empty() { row.artist = f.artist.clone().into(); }
                if !f.album.is_empty()  { row.album  = f.album.clone().into(); }
            }
        }
    });
    publish_meta_page(w);
}
/// Store the full artist-albums list and publish the first 8-per-page slice.
pub fn set_detail_artist_albums(w: &MainWindow, albums: Vec<PhotoTile>) {
    DETAIL_ARTIST_ALBUMS.with(|r| *r.borrow_mut() = albums);
    publish_detail_artist_albums(w);
}
/// Publish the current 8-album page (keyed to the track page index).
pub fn publish_detail_artist_albums(w: &MainWindow) {
    const PER: usize = 8;
    DETAIL_ARTIST_ALBUMS.with(|r| {
        let all = r.borrow();
        let page = w.get_music_detail_page().max(0) as usize;
        let slice: Vec<PhotoTile> = all.iter().skip(page * PER).take(PER).cloned().collect();
        w.set_music_detail_artist_albums(slint::ModelRc::new(slint::VecModel::from(slice)));
    });
}
/// Store the detail's full rows and publish the first 25-track page.
pub fn set_detail_rows(w: &MainWindow, rows: Vec<MusicSongRow>) {
    DETAIL_ROWS.with(|r| *r.borrow_mut() = rows);
    w.set_music_detail_page(0);
    publish_detail_page(w);
}
/// Publish the current detail page (25 tracks) + page count.
pub fn publish_detail_page(w: &MainWindow) {
    const PER: usize = 25;
    DETAIL_ROWS.with(|r| {
        let rows = r.borrow();
        let pages = rows.len().div_ceil(PER).max(1);
        let page = (w.get_music_detail_page().max(0) as usize).min(pages - 1);
        let slice: Vec<MusicSongRow> = rows.iter().skip(page * PER).take(PER).cloned().collect();
        w.set_music_detail_pages(pages as i32);
        w.set_music_detail_page(page as i32);
        w.set_music_detail_tracks(slint::ModelRc::new(slint::VecModel::from(slice)));
    });
    // Keep the artist-albums column in step with the track page.
    publish_detail_artist_albums(w);
}
pub fn browse_tab_key(tab: &str) -> Option<&'static str> {
    ["albums", "artists", "genres", "folders", "playlists"].into_iter().find(|k| *k == tab)
}

/// Filter the cached Artists/Albums browse tiles by the search query for the
/// unified grouped-search header (np.p5.atmusic.lib-grouped-search).
pub fn rebuild_grouped_search(w: &MainWindow, q: &str) {
    let q = q.trim().to_lowercase();
    let pick = |tab: &'static str, limit: usize| -> Vec<PhotoTile> {
        if q.is_empty() { return Vec::new(); }
        MUSIC_BROWSE.with(|m| m.borrow().get(tab).cloned().unwrap_or_default())
            .into_iter().filter(|(t, _)| t.label.to_lowercase().contains(&q))
            .map(|(t, _)| t).take(limit).collect()
    };
    // Artists: 9 per line × 2 lines = 18. Albums: 7 per line × 2 lines = 14.
    w.set_music_search_artists(slint::ModelRc::new(slint::VecModel::from(pick("artists", 18))));
    w.set_music_search_albums(slint::ModelRc::new(slint::VecModel::from(pick("albums", 14))));
}
pub fn set_browse_src(tab: &'static str, src: Vec<(PhotoTile, i64)>) {
    MUSIC_BROWSE.with(|m| { m.borrow_mut().insert(tab, src); });
}

/// Rebuild the Home "Recently played" page (6 rows) from the recent pool.
pub fn rebuild_recent_page(w: &MainWindow) {
    let g = match music_recent().lock() { Ok(g) => g, Err(_) => return };
    let pages = g.len().div_ceil(RECENT_PAGE).clamp(1, 3);
    let page = (w.get_music_recent_page() as usize).min(pages - 1);
    let rows: Vec<MusicSongRow> = g.iter().skip(page * RECENT_PAGE).take(RECENT_PAGE).map(|s| MusicSongRow {
        thumb: music_thumb_at(s.pos),
        title: s.title.clone().into(), artist: s.artist.clone().into(),
        duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
        index: s.pos,
    }).collect();
    w.set_music_recent_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_recent_pages(pages as i32);
    w.set_music_recent_page(page as i32);
}

/// Re-sort + publish the active browse tab's tiles by name/count (np.p4 sort).
pub fn rebuild_browse_tab(w: &MainWindow, tab: &str) {
    let Some(tab) = browse_tab_key(tab) else { return; };
    let mut v: Vec<(PhotoTile, i64)> = MUSIC_BROWSE.with(|m| m.borrow().get(tab).cloned().unwrap_or_default());
    // Context search — filter the current browse tab by the search box query.
    {
        let q = music_query_filter().lock().map(|s| s.trim().to_lowercase()).unwrap_or_default();
        if !q.is_empty() {
            if tab == "folders" {
                // On the Folders tab, search looks INSIDE each folder, not just at
                // the folder name: keep a folder if its name matches OR it holds a
                // track whose title/artist/album matches — so songs stay findable
                // by folder (np.p5.atmusic.folder-content-search).
                let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
                let hit_folders: std::collections::HashSet<std::path::PathBuf> = music_songs().lock()
                    .map(|g| g.iter()
                        .filter(|s| s.title.to_lowercase().contains(&q)
                            || s.artist.to_lowercase().contains(&q)
                            || s.album.to_lowercase().contains(&q))
                        .filter_map(|s| paths.get(s.pos as usize).and_then(|p| p.parent().map(|d| d.to_path_buf())))
                        .collect())
                    .unwrap_or_default();
                v.retain(|(t, _)| t.label.to_lowercase().contains(&q)
                    || paths.get(t.index as usize)
                        .and_then(|p| p.parent())
                        .map(|d| hit_folders.contains(d))
                        .unwrap_or(false));
            } else {
                v.retain(|(t, _)| t.label.to_lowercase().contains(&q));
            }
        }
    }
    let sort = w.get_music_browse_sort().to_string();
    let asc = w.get_music_browse_dir() == "asc";
    v.sort_by(|a, b| {
        let o = match sort.as_str() {
            "count"  => a.1.cmp(&b.1),
            "rating" => a.0.stack_count.cmp(&b.0.stack_count),
            _        => a.0.label.to_lowercase().cmp(&b.0.label.to_lowercase()),
        };
        if asc { o } else { o.reverse() }
    });
    // Albums show their track count next to the name (easier visual sorting).
    let tiles: Vec<PhotoTile> = v.into_iter().map(|(mut t, c)| {
        if tab == "albums" || tab == "genres" { t.label = format!("{}  ·  {} tracks", t.label, c).into(); }
        t
    }).collect();
    // Albums/Artists are paginated 21/page (Songs-grid style); the rest show all.
    const BROWSE_PER: usize = 21;
    let page_slice = |w: &MainWindow, tiles: Vec<PhotoTile>| -> Vec<PhotoTile> {
        let pages = tiles.len().div_ceil(BROWSE_PER).max(1);
        let page = (w.get_music_browse_page().max(0) as usize).min(pages - 1);
        w.set_music_browse_pages(pages as i32);
        w.set_music_browse_page(page as i32);
        tiles.into_iter().skip(page * BROWSE_PER).take(BROWSE_PER).collect()
    };
    match tab {
        "albums"    => w.set_music_albums(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        "artists"   => w.set_music_artists(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        "genres"    => w.set_music_genres(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        // Folders is paginated on the same 21/page rule as the three above. It
        // used to render every folder in one pass, which on a deep library is
        // thousands of tiles built and laid out for one visible screen.
        "folders"   => w.set_music_folders(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        "playlists" => w.set_music_playlists(slint::ModelRc::new(slint::VecModel::from(tiles))),
        _ => {}
    }
}

/// Build the "Up next" queue panel. Prefers the persisted `play_queue` (so a
/// reordered/explicit queue survives relaunch — np.p5.music.queue-persist) and
/// falls back to the tracks after the current position (np.p4.music.queue).
pub fn build_music_queue(w: &MainWindow) {
    // YouTube playback owns the Up-next panel while active.
    if YT_QUEUE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) { build_yt_queue_panel(w); return; }
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids = tulipix_music::queue::list(&pool).await.unwrap_or_default();
        if ids.is_empty() { let _ = weak.upgrade_in_event_loop(build_sequential_queue); return; }
        let _ = weak.upgrade_in_event_loop(move |w| set_instant_mix_queue(&w, &ids));
    });
}

/// The classic sequential "Up next" (tracks after the current position).
/// Audiobook chapter playing → the queue is the BOOK's remaining chapters
/// only, never the My Music library that happens to follow it positionally.
pub fn build_sequential_queue(w: MainWindow) {
    let total = w.get_music_np_total();
    if total <= 0 { return; }
    let cur = w.get_music_np_index();
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
        .unwrap_or_default();
    let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
    let cur_dir = paths.get(cur as usize).and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let is_book = cur_dir.as_ref()
        .map(|d| ab_cover_cache().lock().ok().map(|g| g.contains_key(&d.display().to_string())).unwrap_or(false)
            || load_folder_sections().get(&d.display().to_string()).map(|k| k == "audiobooks").unwrap_or(false))
        .unwrap_or(false);
    let next_positions: Vec<i32> = if is_book {
        // Remaining chapters of this book, in order, no wrap.
        ((cur + 1)..total)
            .filter(|&pos| paths.get(pos as usize).and_then(|p| p.parent()) == cur_dir.as_deref())
            .take(30)
            .collect()
    } else {
        (1..=30.min(total - 1)).map(|off| (cur + off).rem_euclid(total)).collect()
    };
    // Audiobook chapters have no per-track art — use the book cover as each
    // queue row's thumb so rows read like a normal queue (▶ overlay on art).
    let book_thumb: Option<slint::Image> = if is_book {
        cur_dir.as_ref().and_then(|d| ab_cover_cache().lock().ok()
            .and_then(|g| g.get(&d.display().to_string()).cloned()))
            .map(|px| art_image(&Some(px)))
    } else { None };
    let rows: Vec<MusicSongRow> = next_positions.into_iter().map(|pos| {
        let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        MusicSongRow {
            // Chapters always show the book cover (incl. user-picked custom
            // art) — their scan thumbs are generic waveform placeholders.
            thumb: match &book_thumb {
                Some(cover) => cover.clone(),
                None => music_thumb_at(pos),
            },
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
        }
    }).collect();
    w.set_music_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Build an "Up next" queue panel from explicit item_ids (used by Instant Mix).
/// item_ids are mapped back to their playback positions via `music_ids`.
pub fn set_instant_mix_queue(w: &MainWindow, ids: &[i64]) {
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect())
        .unwrap_or_default();
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
        .unwrap_or_default();
    // Up-next is a PEEK at what plays next, never the whole queue table.
    //
    // `queue::list` hands back everything that was ever queued, and a "play all"
    // on a real library is thousands of ids. Every row built here calls
    // `music_thumb_at`, which decodes a PNG on the event loop when the row is
    // not already in the thumbnail cache — so opening the queue drawer after a
    // play-all was thousands of synchronous decodes, which is the several-second
    // hang on that button. Forty is what the sequential and up-next builders
    // already show; this one is now the same.
    const UPNEXT_MAX: usize = 40;
    let rows: Vec<MusicSongRow> = ids.iter().filter_map(|id| pos_of.get(id).copied())
        .take(UPNEXT_MAX).map(|pos| {
        let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        MusicSongRow {
            thumb: music_thumb_at(pos),
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
        }
    }).collect();
    w.set_music_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Plain "Up next" — the following tracks in the current library order. Cheap,
/// always available; used as the fallback when the sonic index isn't built.
pub fn set_upnext_queue(w: &MainWindow, pos: i32) {
    let total = music_ids().lock().map(|g| g.len() as i32).unwrap_or(0);
    if total <= 0 { return; }
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
        .unwrap_or_default();
    let rows: Vec<MusicSongRow> = (1..=40i32).map(|k| pos + k).filter(|p| *p >= 0 && *p < total).map(|p| {
        let (title, artist, dur) = by_pos.get(&p).cloned().unwrap_or_default();
        MusicSongRow {
            thumb: music_thumb_at(p),
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: p,
        }
    }).collect();
    w.set_music_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Default Up-next queue for a fresh My Music session: sonic-similar tracks when
/// the embedding index is ready (silent — no toasts, no on-demand embedding),
/// otherwise a plain up-next list. Keeps the queue from ever being empty.
pub fn build_default_music_queue(w: &MainWindow, pos: i32) {
    let Some(seed) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
    let weak = w.as_weak();
    let rt = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let ids: Vec<i64> = rt.block_on(async {
            let Ok(pool) = pool_for("music").await else { return vec![]; };
            let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_embeddings")
                .fetch_one(&pool).await.unwrap_or(0);
            if indexed <= 0 { return vec![]; }
            // Only use sonic when the seed is already embedded — never embed
            // on-demand here (that's the slow, toast-worthy path).
            let have = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM track_embeddings WHERE item_id = ?")
                .bind(seed).fetch_one(&pool).await.map(|n| n > 0).unwrap_or(false);
            if !have { return vec![]; }
            tulipix_music::embeddings::similar(&pool, seed, 40).await
                .unwrap_or_default().into_iter().map(|(id, _)| id).collect()
        });
        let _ = weak.upgrade_in_event_loop(move |w| {
            if ids.is_empty() { set_upnext_queue(&w, pos); }
            else { set_instant_mix_queue(&w, &ids); }
        });
    });
}

// ── Sonic Similar (np.p4.music.embeddings, dsp-v1) ─────────────────────────
// Embedding-based "more like this": each track gets a 43-dim DSP descriptor
// (analysis::dsp_embedding — no model download needed); nearest-by-cosine
// fills the Up-next queue. A future CLAP/PANNs ONNX path stores under its own
// `model` tag and silently takes over (the similarity query never mixes
// embedding spaces).

/// True while the background indexer walks the library.
pub static SONIC_INDEXING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Compute + store the dsp-v1 embedding for one track. Blocking (ffmpeg
/// decode + DSP ~2-4 s) — call from a worker thread.
pub fn sonic_embed_one(rt: &tokio::runtime::Handle, item_id: i64, path: &std::path::Path) -> bool {
    let Some(samples) = analysis::decode_mono(path) else { return false; };
    let Some(vec) = analysis::dsp_embedding(&samples) else { return false; };
    rt.block_on(async move {
        let Ok(pool) = pool_for("music").await else { return false; };
        tulipix_music::embeddings::store(&pool, item_id, "dsp-v1", &vec).await.is_ok()
    })
}

/// Context-menu "Sonic similar": ensure the seed is embedded, rank the
/// indexed library by cosine, fill the Up-next queue. Tracks not yet indexed
/// simply can't rank — the background indexer (below) closes that gap.
pub fn sonic_similar_queue(w: &MainWindow, pos: i32) {
    let Some(seed) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
    let Some(path) = music_paths().lock().ok().and_then(|g| g.get(pos as usize).cloned()) else { return; };
    let title = music_songs().lock().ok()
        .and_then(|g| g.iter().find(|s| s.pos == pos).map(|s| s.title.clone()))
        .unwrap_or_else(|| "track".into());
    w.set_caps_nudge(format!("Finding tracks that sound like “{title}”…").into());
    let weak = w.as_weak();
    let rt = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        // Seed embedding on demand (skips instantly when already stored).
        let have = rt.block_on(async {
            let Ok(pool) = pool_for("music").await else { return false; };
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM track_embeddings WHERE item_id = ?")
                .bind(seed).fetch_one(&pool).await.map(|n| n > 0).unwrap_or(false)
        });
        if !have && !sonic_embed_one(&rt, seed, &path) {
            let _ = weak.upgrade_in_event_loop(|w| w.set_caps_nudge("Sonic similar failed — could not analyze the file.".into()));
            return;
        }
        rt.block_on(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let ranked = tulipix_music::embeddings::similar(&pool, seed, 40).await.unwrap_or_default();
            let indexed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_embeddings")
                .fetch_one(&pool).await.unwrap_or(0);
            let ids: Vec<i64> = ranked.into_iter().map(|(id, _)| id).collect();
            let _ = tulipix_music::queue::clear(&pool).await;
            for id in &ids { let _ = tulipix_music::queue::enqueue(&pool, *id, "sonic").await; }
            let n = ids.len();
            let _ = weak.upgrade_in_event_loop(move |w| {
                set_instant_mix_queue(&w, &ids);
                w.set_music_player_panel("queue".into());
                w.set_caps_nudge(match (n, indexed) {
                    (0, _) => "No sonic matches yet — run “Build sonic index” in Music settings first.".to_string(),
                    (n, i) => format!("Queued {n} sonically similar tracks ({i} indexed)."),
                }.into());
            });
        });
    });
}

/// Settings-card "Build sonic index": walk every library track without an
/// embedding, one at a time (each ~2-4 s of ffmpeg + DSP), progress in the
/// settings status line. Idempotent; re-run picks up only new tracks.
pub fn sonic_index_all(w: &MainWindow) {
    use std::sync::atomic::Ordering;
    if SONIC_INDEXING.swap(true, Ordering::SeqCst) { return; }  // already running
    let items: Vec<(i64, std::path::PathBuf)> = {
        let ids = music_ids().lock().map(|g| g.clone()).unwrap_or_default();
        let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
        ids.into_iter().zip(paths).collect()
    };
    let weak = w.as_weak();
    let rt = tokio::runtime::Handle::current();
    std::thread::spawn(move || {
        let todo: Vec<(i64, std::path::PathBuf)> = rt.block_on(async {
            let Ok(pool) = pool_for("music").await else { return Vec::new(); };
            let done: std::collections::HashSet<i64> =
                sqlx::query_scalar::<_, i64>("SELECT item_id FROM track_embeddings")
                    .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
            items.into_iter().filter(|(id, _)| !done.contains(id)).collect()
        });
        let total = todo.len();
        if total == 0 {
            SONIC_INDEXING.store(false, Ordering::SeqCst);
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_sonic_status("Sonic index is up to date.".into()));
            return;
        }
        let mut ok = 0usize;
        for (i, (id, path)) in todo.into_iter().enumerate() {
            if sonic_embed_one(&rt, id, &path) { ok += 1; }
            if i % 3 == 0 || i + 1 == total {
                let msg = format!("Indexing… {} / {total}", i + 1);
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_sonic_status(msg.into()));
            }
        }
        SONIC_INDEXING.store(false, Ordering::SeqCst);
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_sonic_status(format!("Sonic index ready — {ok} of {total} tracks analyzed.").into());
        });
    });
}

// Parsed synced-lyric lines for the current track: (ms, text), driving the
// active-line highlight during playback (np.p5.music.lyrics-synced).
static MUSIC_LYRICS_LINES: std::sync::OnceLock<std::sync::Mutex<Vec<(i64, String)>>> = std::sync::OnceLock::new();
pub fn music_lyrics_lines() -> &'static std::sync::Mutex<Vec<(i64, String)>> {
    MUSIC_LYRICS_LINES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Last cast-discovery results, indexed by the names shown in the UI.
static CAST_TARGETS: std::sync::OnceLock<std::sync::Mutex<Vec<tulipix_music::cast::CastDevice>>> = std::sync::OnceLock::new();
pub fn cast_targets() -> &'static std::sync::Mutex<Vec<tulipix_music::cast::CastDevice>> {
    CAST_TARGETS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// The currently-open playlist detail: (playlist_id, ordered item_ids) — lets the
// remove button map a row index back to an item (np.p5.music.playlists-builder).
static CURRENT_PLAYLIST: std::sync::OnceLock<std::sync::Mutex<(i64, Vec<i64>)>> = std::sync::OnceLock::new();
pub fn current_playlist() -> &'static std::sync::Mutex<(i64, Vec<i64>)> {
    CURRENT_PLAYLIST.get_or_init(|| std::sync::Mutex::new((-1, Vec::new())))
}
// Playlist ids backing the add-to-playlist picker (parallel to the shown names).
static PICK_PLAYLIST_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
pub fn pick_playlist_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    PICK_PLAYLIST_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Map a list of item_ids (in order) to MusicSongRows, resolving each to its
/// playback position so the existing `play-music`/`tile-clicked` path plays it.
/// Ids not in the current library are skipped. Shared by Favorites + History.
pub fn song_rows_for_ids(ids: &[i64]) -> Vec<MusicSongRow> {
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect()).unwrap_or_default();
    ids.iter().filter_map(|id| pos_of.get(id).copied()).map(|pos| {
        let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        MusicSongRow {
            thumb: music_thumb_at(pos),
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
        }
    }).collect()
}

// LRCLIB manual-search results for the current pick (content, is_synced).
static LYRICS_SEARCH: std::sync::OnceLock<std::sync::Mutex<Vec<(String, bool)>>> = std::sync::OnceLock::new();
pub fn lyrics_search_store() -> &'static std::sync::Mutex<Vec<(String, bool)>> {
    LYRICS_SEARCH.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Scanned-roots list with per-root music track counts (np.p5.atmusic.lib-folder-mgmt).
pub fn populate_folder_roots(w: &MainWindow) {
    let roots = load_watched_folders();
    // Folders assigned to other music sections (Audiobooks etc.) belong to
    // their own section's UI — never to the My Music Folders tab.
    let sections = load_folder_sections();
    let excluded: Vec<PathBuf> = sections.iter()
        .filter(|(_, key)| key.as_str() != "mymusic")
        .map(|(folder, _)| PathBuf::from(folder))
        .collect();
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let mut labels: Vec<slint::SharedString> = Vec::new();
        for r in &roots {
            if excluded.iter().any(|e| r.starts_with(e)) { continue; }
            let prefix = format!("{}%", r.to_string_lossy());
            let n: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM items i JOIN track_meta tm ON tm.item_id = i.id \
                 WHERE i.section = 'music' AND i.missing_since IS NULL \
                   AND COALESCE(tm.is_audiobook, 0) = 0 AND i.abs_path LIKE ?")
                .bind(&prefix).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
            let name = r.file_name().and_then(|s| s.to_str()).unwrap_or(".").to_string();
            labels.push(format!("{name}   ·   {n} tracks   —   {}", r.display()).into());
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_folder_roots(slint::ModelRc::new(slint::VecModel::from(labels)));
        });
    });
}

/// Favorites page (np.p5.atmusic.favorites-page) — all loved tracks.
/// All loved track ids (favorites), paginated 20/page in the UI.
static FAV_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
pub fn fav_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    FAV_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// Filter item_ids by the current search query (title/artist/album substring),
/// resolving each via the cached `music_songs` metadata. Empty query = all.
pub fn filter_ids_by_query(ids: &[i64]) -> Vec<i64> {
    let q = music_query_filter().lock().map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if q.is_empty() { return ids.to_vec(); }
    let by_id: std::collections::HashMap<i64, (String, String, String)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.item_id, (s.title.to_lowercase(), s.artist.to_lowercase(), s.album.to_lowercase()))).collect())
        .unwrap_or_default();
    ids.iter().copied().filter(|id| by_id.get(id)
        .map(|(t, a, al)| t.contains(&q) || a.contains(&q) || al.contains(&q)).unwrap_or(false)).collect()
}
/// Publish one 20-track page of favorites + the page count.
pub fn rebuild_fav_page(w: &MainWindow) {
    const PER: usize = 20;
    let all = fav_ids().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = filter_ids_by_query(&all);
    let pages = ids.len().div_ceil(PER).max(1);
    let page = (w.get_music_fav_page().max(0) as usize).min(pages - 1);
    let slice: Vec<i64> = ids.iter().skip(page * PER).take(PER).copied().collect();
    let rows = song_rows_for_ids(&slice);
    w.set_music_fav_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_fav_pages(pages as i32);
    w.set_music_fav_page(page as i32);
}
pub fn populate_favorites(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // Music only: a loved book chapter belongs on the Audiobooks shelf.
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta \
             WHERE loved = 1 AND COALESCE(is_audiobook, 0) = 0 \
             ORDER BY title COLLATE NOCASE")
            .fetch_all(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Ok(mut g) = fav_ids().lock() { *g = ids; }
            w.set_music_fav_page(0);
            rebuild_fav_page(&w);
        });
    });
}

/// Playback History page (np.p5.atmusic.history-page) — most-recent plays first.
/// Cached ids so the search box can filter the list live.
static HISTORY_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
pub fn history_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    HISTORY_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// Re-publish history rows, applying the current search query filter and paging
/// 30 plays per page (up to 5 pages = 150 most-recent plays).
pub fn rebuild_history_page(w: &MainWindow) {
    const PER: usize = 30;
    const MAX_PAGES: usize = 5;
    let all = history_ids().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = filter_ids_by_query(&all);
    let pages = ids.len().div_ceil(PER).clamp(1, MAX_PAGES);
    w.set_music_history_pages(pages as i32);
    let page = (w.get_music_history_page().max(1) as usize).min(pages);
    w.set_music_history_page(page as i32);
    let start = (page - 1) * PER;
    let slice: Vec<i64> = ids.iter().skip(start).take(PER).copied().collect();
    let rows = song_rows_for_ids(&slice);
    w.set_music_history_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}
pub fn populate_history(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // Music only — the History tab lives in My Music, and a book chapter
        // played from the Audiobooks shelf writes the same play_history row a
        // song does. Books have their own progress and their own shelf.
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT ph.item_id FROM play_history ph \
             JOIN track_meta tm ON tm.item_id = ph.item_id \
             WHERE COALESCE(tm.is_audiobook, 0) = 0 \
             ORDER BY ph.played_at DESC LIMIT 150")
            .fetch_all(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Ok(mut g) = history_ids().lock() { *g = ids; }
            rebuild_history_page(&w);
        });
    });
}

// Current Album/Artist detail context: (kind, db id, ordered track item_ids).
static MUSIC_DETAIL: std::sync::OnceLock<std::sync::Mutex<(String, i64, Vec<i64>)>> = std::sync::OnceLock::new();
pub fn music_detail() -> &'static std::sync::Mutex<(String, i64, Vec<i64>)> {
    MUSIC_DETAIL.get_or_init(|| std::sync::Mutex::new((String::new(), -1, Vec::new())))
}

/// Open the Album detail overlay (np.p5.atmusic.album-detail): header (cover,
/// title, album-artist · year) + full tracklist ordered by disc/track number.
pub fn open_album_detail(w: &MainWindow, album_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let hdr: Option<(String, Option<i64>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT al.title, al.year, al.cover_path, ar.name
             FROM albums al LEFT JOIN artists ar ON ar.id = al.artist_id WHERE al.id = ?")
            .bind(album_id).fetch_optional(&pool).await.ok().flatten();
        let Some((title, year, cover, artist)) = hdr else { return; };
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let al_loved: i64 = sqlx::query_scalar("SELECT COALESCE(loved,0) FROM albums WHERE id = ?").bind(album_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let al_rating: i64 = sqlx::query_scalar("SELECT COALESCE(rating,0) FROM albums WHERE id = ?").bind(album_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE album_id = ? ORDER BY COALESCE(disc_no,0), COALESCE(track_no,0), title")
            .bind(album_id).fetch_all(&pool).await.unwrap_or_default();
        let sub = {
            let mut s = artist.unwrap_or_default();
            if let Some(y) = year { if y > 0 { if !s.is_empty() { s.push_str("  ·  "); } s.push_str(&y.to_string()); } }
            s
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows = song_rows_for_ids(&ids);
            let art = rows.first().map(|r| r.thumb.clone())
                .or_else(|| cover.as_ref().filter(|p| std::path::Path::new(p).exists())
                    .map(|p| slint::Image::load_from_path(std::path::Path::new(p)).unwrap_or_default()))
                .unwrap_or_default();
            if let Ok(mut g) = music_detail().lock() { *g = ("album".into(), album_id, ids.clone()); }
            w.set_music_detail_kind("album".into());
            w.set_music_detail_title(title.into());
            w.set_music_detail_subtitle(sub.into());
            w.set_music_detail_art(art);
            w.set_music_detail_bio("".into());
            w.set_music_detail_status("".into());
            w.set_music_detail_loved(al_loved != 0);
            w.set_music_detail_stars(al_rating as i32);
            set_detail_rows(&w, rows);
            w.set_music_detail_open(true);
        });
    });
}

/// Open the Artist detail overlay (np.p5.atmusic.artist-detail): header (image,
/// name, track/album counts), all tracks, and a MusicBrainz bio fetched async.
pub fn open_artist_detail(w: &MainWindow, artist_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let hdr: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT name, image_path, bio FROM artists WHERE id = ?")
            .bind(artist_id).fetch_optional(&pool).await.ok().flatten();
        let Some((name, image, bio)) = hdr else { return; };
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE artist_id = ? ORDER BY COALESCE(album_id,0), COALESCE(track_no,0), title")
            .bind(artist_id).fetch_all(&pool).await.unwrap_or_default();
        let album_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT album_id) FROM track_meta WHERE artist_id = ? AND album_id IS NOT NULL")
            .bind(artist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        // This artist's own favourite / rating state (for the info-box buttons).
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let ar_loved: i64 = sqlx::query_scalar("SELECT COALESCE(loved,0) FROM artists WHERE id = ?").bind(artist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let ar_rating: i64 = sqlx::query_scalar("SELECT COALESCE(rating,0) FROM artists WHERE id = ?").bind(artist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        // Albums by this artist (cover + first track) for the 70/30 right column.
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let alb: Vec<(i64, String, Option<String>, i64, i64, i64)> = sqlx::query_as(
            "SELECT al.id, al.title, al.cover_path, MIN(tm.item_id), COALESCE(al.loved,0), COALESCE(al.rating,0) \
             FROM track_meta tm JOIN albums al ON al.id = tm.album_id \
             WHERE tm.artist_id = ? GROUP BY al.id ORDER BY al.title COLLATE NOCASE")
            .bind(artist_id).fetch_all(&pool).await.unwrap_or_default();
        let sub = format!("{} track{}  ·  {} album{}",
            ids.len(), if ids.len() == 1 { "" } else { "s" },
            album_count, if album_count == 1 { "" } else { "s" });
        let cached_bio = bio.clone().unwrap_or_default();
        let _ = weak.upgrade_in_event_loop({
            let name = name.clone();
            move |w| {
                let rows = song_rows_for_ids(&ids);
                let art = rows.first().map(|r| r.thumb.clone())
                    .or_else(|| image.as_ref().filter(|p| std::path::Path::new(p).exists())
                        .map(|p| slint::Image::load_from_path(std::path::Path::new(p)).unwrap_or_default()))
                    .unwrap_or_default();
                if let Ok(mut g) = music_detail().lock() { *g = ("artist".into(), artist_id, ids.clone()); }
                w.set_music_detail_kind("artist".into());
                w.set_music_detail_title(name.into());
                w.set_music_detail_subtitle(sub.into());
                w.set_music_detail_art(art);
                w.set_music_detail_bio(cached_bio.into());
                w.set_music_detail_loved(ar_loved != 0);
                w.set_music_detail_stars(ar_rating as i32);
                w.set_music_detail_status("".into());
                set_detail_rows(&w, rows);
                // Build the artist-albums column tiles (cover → first-track thumb).
                let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                    .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
                let album_tiles: Vec<PhotoTile> = alb.iter().map(|(_aid, title, cover, first, loved, rating)| {
                    let pos = pos_of.get(first).copied().unwrap_or(-1);
                    let thumb = cover.as_ref().filter(|p| std::path::Path::new(p).exists())
                        .map(|p| slint::Image::load_from_path(std::path::Path::new(p)).unwrap_or_default())
                        .unwrap_or_else(|| music_thumb_at(pos));
                    PhotoTile { thumb, label: title.clone().into(), index: pos,
                        starred: *loved != 0, stack_count: *rating as i32, ..Default::default() }
                }).collect();
                set_detail_artist_albums(&w, album_tiles);
                w.set_music_detail_open(true);
            }
        });
        // Background MusicBrainz bio fetch when we don't have one cached.
        if bio.as_deref().unwrap_or("").is_empty() {
            let client = tulipix_core::net::http().clone();
            if let Ok(s) = tulipix_music::musicbrainz::lookup_artist(&client, &name).await {
                if let Some(blurb) = s.artists.iter().max_by_key(|a| a.score)
                    .map(tulipix_music::musicbrainz::artist_blurb) {
                    let _ = sqlx::query("UPDATE artists SET bio = ? WHERE id = ?")
                        .bind(&blurb).bind(artist_id).execute(&pool).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        if w.get_music_detail_open() && w.get_music_detail_kind() == "artist" {
                            w.set_music_detail_bio(blurb.into());
                        }
                    });
                }
            }
        }
    });
}

/// Open a Genre detail overlay (np.p4.music.browse) — all tracks in the genre,
/// shown before playback like album/artist pages.
pub fn open_genre_detail(w: &MainWindow, genre: String) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE genre = ? ORDER BY COALESCE(artist_id,0), COALESCE(album_id,0), title")
            .bind(&genre).fetch_all(&pool).await.unwrap_or_default();
        let sub = format!("{} track{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows = song_rows_for_ids(&ids);
            // Genre cover override wins over the first track's album art.
            let art = load_music_pref(&format!("music.genre.cover.{genre}"))
                .map(|c| slint::Image::load_from_path(std::path::Path::new(&c)).unwrap_or_default())
                .filter(|im| im.size().width > 0)
                .unwrap_or_else(|| rows.first().map(|r| r.thumb.clone()).unwrap_or_default());
            if let Ok(mut g) = music_detail().lock() { *g = ("genre".into(), -1, ids.clone()); }
            w.set_music_detail_kind("genre".into());
            w.set_music_detail_title(genre.into());
            w.set_music_detail_subtitle(sub.into());
            w.set_music_detail_art(art);
            w.set_music_detail_bio("".into());
            w.set_music_detail_status("".into());
            set_detail_rows(&w, rows);
            w.set_music_detail_open(true);
        });
    });
}

/// Re-open whatever album/artist/genre detail overlay is currently showing, so a
/// metadata edit reflects in the overlay without navigating away.
pub fn refresh_open_detail(w: &MainWindow) {
    if !w.get_music_detail_open() { return; }
    let (kind, id, _) = music_detail().lock().map(|g| g.clone()).unwrap_or_default();
    match kind.as_str() {
        "album" if id >= 0 => open_album_detail(w, id),
        "artist" if id >= 0 => open_artist_detail(w, id),
        "genre" => open_genre_detail(w, w.get_music_detail_title().to_string()),
        _ => {}
    }
}

/// Item ids whose file lives directly in `dir` (one folder level), by playback
/// position — using the in-memory paths/ids so no path SQL is needed.
pub fn folder_track_ids(dir: &std::path::Path) -> Vec<i64> {
    let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = music_ids().lock().map(|g| g.clone()).unwrap_or_default();
    paths.iter().zip(ids.iter())
        .filter(|(p, _)| p.parent() == Some(dir))
        .map(|(_, id)| *id).collect()
}
/// Open a folder's own songs page (detail overlay) — its directly-contained tracks.
pub fn open_folder_detail(w: &MainWindow, pos: i32) {
    let Some(dir) = music_paths().lock().ok()
        .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.to_path_buf()))) else { return; };
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let ids = folder_track_ids(&dir);
    let title = dir.file_name().and_then(|s| s.to_str()).unwrap_or("Folder").to_string();
    let sub = format!("{} track{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
    let rows = song_rows_for_ids(&ids);
    let art = rows.first().map(|r| r.thumb.clone()).unwrap_or_default();
    if let Ok(mut g) = music_detail().lock() { *g = ("folder".into(), -1, ids.clone()); }
    w.set_music_detail_kind("folder".into());
    w.set_music_detail_title(title.into());
    w.set_music_detail_subtitle(sub.into());
    w.set_music_detail_art(art);
    w.set_music_detail_bio("".into());
    w.set_music_detail_status("".into());
    set_detail_rows(w, rows);
    w.set_music_detail_open(true);
    let _ = pos_of;
}

/// Queue all of the open detail's tracks and start playback (Play All / Shuffle).
pub fn detail_play(w: &MainWindow, shuffle: bool) {
    let ids = music_detail().lock().map(|g| g.2.clone()).unwrap_or_default();
    if ids.is_empty() { return; }
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let mut positions: Vec<i32> = ids.iter().filter_map(|id| pos_of.get(id).copied()).collect();
    if positions.is_empty() { return; }
    w.set_music_shuffle(shuffle);
    let first = if shuffle {
        let n = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos()).unwrap_or(0) as usize) % positions.len();
        positions.swap(0, n);
        positions[0]
    } else { positions[0] };
    // The queue IS the detail, in the order it was just laid out (shuffle moved
    // the pick to the front, so the tail follows from position 1).
    let order: Vec<i64> = positions.iter().filter_map(|p| {
        music_ids().lock().ok().and_then(|g| g.get(*p as usize).copied())
    }).collect();
    set_ctx_pending();
    play_music_at(w, first);
    set_context_queue(w, order, music_id_at(first), "detail");
}

/// Extract a vivid dominant colour from an album-art file for the now-playing
/// gradient wash (np.p5.atmusic.art-gradient — a small color-thief-style
/// quantiser over a downscaled copy, biased to saturated buckets).
pub fn dominant_color(path: &std::path::Path) -> Option<slint::Color> {
    let img = image::open(path).ok()?.thumbnail(48, 48).to_rgb8();
    // 4×4×4 colour histogram, weighted by saturation × a mid-luma preference.
    let mut buckets = [(0.0f64, 0u64, 0u64, 0u64, 0u64); 64];
    for p in img.pixels() {
        let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let max = r.max(g).max(b); let min = r.min(g).min(b);
        let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
        let luma = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        let weight = sat * (1.0 - (luma - 0.55).abs()); // favour vivid, mid-bright
        let idx = ((p[0] >> 6) as usize) * 16 + ((p[1] >> 6) as usize) * 4 + (p[2] >> 6) as usize;
        let e = &mut buckets[idx];
        e.0 += weight; e.1 += 1; e.2 += p[0] as u64; e.3 += p[1] as u64; e.4 += p[2] as u64;
    }
    let best = buckets.iter().filter(|e| e.1 > 0).max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))?;
    if best.1 == 0 { return None; }
    let (r, g, b) = ((best.2 / best.1) as u8, (best.3 / best.1) as u8, (best.4 / best.1) as u8);
    // Washed-out result (white / near-grey covers): accent-tinted pills and
    // bars painted with it disappear on light surfaces (user report
    // 2026-07-12: white art made the home player's volume bar vanish). Treat
    // as "no usable accent" — callers fall back to the brand pink.
    {
        let (rf, gf, bf) = (r as f64, g as f64, b as f64);
        let max = rf.max(gf).max(bf);
        let sat = if max > 0.0 { (max - rf.min(gf).min(bf)) / max } else { 0.0 };
        let luma = (0.299 * rf + 0.587 * gf + 0.114 * bf) / 255.0;
        if luma > 0.82 || sat < 0.12 {
            return None;
        }
    }
    Some(slint::Color::from_rgb_u8(r, g, b))
}

/// Load a playlist's tracks into the detail view: rows (mapped to playback
/// positions for play) + the ordered item ids (for remove).
/// Custom cover image path for a playlist (stored in Settings, not the DB).
pub fn playlist_cover_path(id: i64) -> Option<String> {
    let s = tulipix_core::settings::Settings::load().ok()?;
    s.advanced.get(&format!("music.playlist.cover.{id}")).cloned()
        .filter(|p| std::path::Path::new(p).exists())
}
pub fn build_playlist_detail(w: &MainWindow, playlist_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let name: String = sqlx::query_scalar("SELECT name FROM playlists WHERE id = ?")
            .bind(playlist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or_default();
        let smart: i64 = sqlx::query_scalar("SELECT COALESCE(is_smart,0) FROM playlists WHERE id = ?")
            .bind(playlist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let item_ids: Vec<i64> = if smart != 0 {
            let rule_json: Option<String> = sqlx::query_scalar("SELECT rule_json FROM playlists WHERE id = ?")
                .bind(playlist_id).fetch_optional(&pool).await.ok().flatten();
            match rule_json.and_then(|j| serde_json::from_str::<tulipix_music::playlists::SmartRule>(&j).ok()) {
                Some(rule) => tulipix_music::playlists::evaluate(&pool, &rule).await.unwrap_or_default(),
                None => Vec::new(),
            }
        } else {
            tulipix_music::playlists::items(&pool, playlist_id).await.unwrap_or_default()
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect())
                .unwrap_or_default();
            let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
                .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
                .unwrap_or_default();
            let mut rows: Vec<MusicSongRow> = item_ids.iter().map(|id| {
                let pos = pos_of.get(id).copied().unwrap_or(-1);
                let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
                MusicSongRow {
                    thumb: music_thumb_at(pos),
                    title: if title.is_empty() { "Track".into() } else { title.into() },
                    artist: artist.into(),
                    duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
                    index: pos,
                }
            }).collect();
            // Apply the playlist sort — Custom keeps the saved/manual order.
            let psort = w.get_music_playlist_sort().to_string();
            match psort.as_str() {
                "title"  => rows.sort_by_key(|a| a.title.to_lowercase()),
                "artist" => rows.sort_by_key(|a| a.artist.to_lowercase()),
                _ => {}
            }
            if psort != "custom" && w.get_music_playlist_sort_dir() == "desc" { rows.reverse(); }
            // Cover: custom art (Settings) → else the first track's thumb.
            let cover = playlist_cover_path(playlist_id)
                .map(|p| slint::Image::load_from_path(std::path::Path::new(&p)).unwrap_or_default())
                .or_else(|| rows.first().map(|r| r.thumb.clone()))
                .unwrap_or_default();
            // Persist the playlist in its *displayed* order so playback follows it.
            let id_of_pos: std::collections::HashMap<i32, i64> = pos_of.iter().map(|(id, p)| (*p, *id)).collect();
            let ordered_ids: Vec<i64> = rows.iter().filter_map(|r| id_of_pos.get(&r.index).copied()).collect();
            if let Ok(mut g) = current_playlist().lock() { *g = (playlist_id, ordered_ids); }
            w.set_music_playlist_name(if name.is_empty() { "Playlist".into() } else { name.into() });
            w.set_music_playlist_cover(cover);
            w.set_music_playlist_tracks(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

// id of the podcast whose detail page is currently open.
static CUR_PODCAST_ID: std::sync::OnceLock<std::sync::Mutex<i64>> = std::sync::OnceLock::new();
pub fn cur_podcast_id() -> &'static std::sync::Mutex<i64> {
    CUR_PODCAST_ID.get_or_init(|| std::sync::Mutex::new(-1))
}
// Podcast/station artwork: a local path is used as-is, a URL is downloaded
// once into the shared cache. Both spellings kept because callers pass either;
// the download itself is `pod_trends::cache_art`, so the Trends grid, the show
// pages and the Flutter build all read the one directory.
pub async fn cache_artwork(client: &reqwest::Client, key: &str, url: &str) -> Option<std::path::PathBuf> {
    if !url.starts_with("http") { return None; }
    tulipix_music::pod_trends::cache_art(client, key, url).await
}

pub async fn resolve_artwork(client: &reqwest::Client, key: &str, src: &str) -> Option<std::path::PathBuf> {
    tulipix_music::pod_trends::cache_art(client, key, src).await
}

// ── Off-UI-thread artwork decode ─────────────────────────────────────────────
// `slint::Image::load_from_path` decodes on the event loop; podcast/show art
// is routinely 1400×1400 JPEG, so a grid of covers froze input for seconds on
// every section switch. Workers decode into SharedPixelBuffers (memoised by
// path) and the event-loop closure only wraps them — wrapping is O(1).
type ArtPx = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
pub fn art_px_cache() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, ArtPx>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, ArtPx>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Decode `path` (downscaled to ≤512px — card size) off the UI thread.
pub async fn decode_art_px(path: Option<PathBuf>) -> Option<ArtPx> {
    let path = path?;
    if let Some(hit) = art_px_cache().lock().ok().and_then(|g| g.get(&path).cloned()) {
        return Some(hit);
    }
    let key = path.clone();
    let px = tokio::task::spawn_blocking(move || {
        let img = image::open(&path).ok()?;
        let img = if img.width() > 512 || img.height() > 512 { img.thumbnail(512, 512) } else { img };
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Some(ArtPx::clone_from_slice(rgba.as_raw(), w, h))
    }).await.ok().flatten()?;
    if let Ok(mut g) = art_px_cache().lock() { g.insert(key, px.clone()); }
    Some(px)
}

/// Wrap a pre-decoded buffer for display. Cheap; safe on the UI thread.
pub fn art_image(px: &Option<ArtPx>) -> slint::Image {
    px.as_ref().map(|b| slint::Image::from_rgba8(b.clone())).unwrap_or_default()
}

/// Fill the Podcasts grid with subscribed feeds, filtered by the active
/// category, plus the category-chip list (np.p5.music.podcast-feeds).
// Send-safe subscription summary (no slint::Image).
#[derive(Clone)]
pub struct PodAllData {
    pub id: i64,
    pub title: String,
    pub author: String,
    pub category: String,
    pub art: Option<ArtPx>,
    pub unplayed: i64,
    pub episodes: i64,
    pub latest: i64,        // MAX(published) across this show's episodes (Home sort)
    pub pinned: bool,       // home_pinned — shown on Home "Your shows"
}
const PODCAST_SUB_PAGE: usize = 21;   // Subscribed grid: 3 rows × 7
const PODCAST_HOME_PAGE: usize = 14;  // Home "Your shows": 2 rows × 7
const PODCAST_HOME_MAX_PAGES: usize = 2;  // Home caps at 2 pages; rest live on Subscribed

// ── Trends (baked podcast directory) ──────────────────────────────────────
// The directory, its metadata cache and the fetch all live in
// `tulipix_music::pod_trends` so both front ends show the same grid off the
// same `podcast_trends` rows. Edit resources/podcast-feeds.txt (one feed URL
// per line; '#'/blank lines ignored) and rebuild to add more —
// crates/tulipix-app/build.rs watches it. What stays here is the Slint half:
// decoding art into `Image`, sorting, paging and pushing the model.
pub use tulipix_music::pod_trends::{feed_urls as trend_feed_urls, TrendMeta};

pub fn trend_cache() -> &'static std::sync::Mutex<Vec<TrendMeta>> {
    static C: OnceLock<std::sync::Mutex<Vec<TrendMeta>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

const PODCAST_TREND_PAGE: usize = 21;   // Trends grid: 3 rows × 7

pub fn cur_trend_idx() -> &'static std::sync::Mutex<i32> {
    static C: OnceLock<std::sync::Mutex<i32>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(-1))
}

// Feed URL of the podcast whose info card is open (Trends or Subscribed) — drives
// Save-category / Update-thumb / Subscribe regardless of how it was opened.
pub fn cur_info_feed() -> &'static std::sync::Mutex<String> {
    static C: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(String::new()))
}

// Per-tab podcast search filters — each tab (and the single-podcast page) keeps
// its own query so search works independently across Home/Trends/Subscribed/
// Downloads and the detail page.
#[derive(Default, Clone)]
pub struct PodFilters { pub home: String, pub trends: String, pub subs: String, pub downloads: String, pub detail: String }
pub fn pod_filters() -> &'static std::sync::Mutex<PodFilters> {
    static C: OnceLock<std::sync::Mutex<PodFilters>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(PodFilters::default()))
}
pub fn pod_filter(which: &str) -> String {
    let g = pod_filters().lock().map(|f| f.clone()).unwrap_or_default();
    match which { "home" => g.home, "trends" => g.trends, "subs" => g.subs, "downloads" => g.downloads, "detail" => g.detail, _ => String::new() }
}

/// Render the Trends grid from the session cache: sort (name/category), paginate
/// (21/page), and flag which feeds are already subscribed. No network.
pub fn render_trends(w: &MainWindow) {
    let weak = w.as_weak();
    let sort = w.get_music_podcast_trends_sort().to_string();
    let page = w.get_music_podcast_trends_page().max(0) as usize;
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let subs: Vec<(String,)> = sqlx::query_as("SELECT feed_url FROM podcasts").fetch_all(&pool).await.unwrap_or_default();
        let subset: std::collections::HashSet<String> = subs.into_iter().map(|(u,)| u).collect();
        let metas = trend_cache().lock().map(|g| g.clone()).unwrap_or_default();
        // Keep the original (feed-order) index for subscribe/info-by-index.
        let mut indexed: Vec<(usize, TrendMeta)> = metas.into_iter().enumerate().collect();
        // Filter (Trends search) — title/author/category.
        let filter = pod_filter("trends");
        let needle = filter.trim().to_lowercase();
        if !needle.is_empty() {
            indexed.retain(|(_, m)| m.title.to_lowercase().contains(&needle)
                || m.author.to_lowercase().contains(&needle)
                || m.category.to_lowercase().contains(&needle));
        }
        match sort.as_str() {
            "category" => indexed.sort_by(|a, b| a.1.category.to_lowercase().cmp(&b.1.category.to_lowercase())
                .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
            "subscribed" => indexed.sort_by(|a, b| subset.contains(&b.1.feed_url).cmp(&subset.contains(&a.1.feed_url))
                .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
            "unsubscribed" => indexed.sort_by(|a, b| subset.contains(&a.1.feed_url).cmp(&subset.contains(&b.1.feed_url))
                .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
            _ => indexed.sort_by_key(|a| a.1.title.to_lowercase()),
        }
        let total = indexed.len();
        let pages = total.div_ceil(PODCAST_TREND_PAGE).max(1);
        let page = page.min(pages - 1);
        let slice: Vec<(usize, TrendMeta)> = indexed.into_iter().skip(page * PODCAST_TREND_PAGE).take(PODCAST_TREND_PAGE).collect();
        // Decode the visible page's art off-thread before touching the UI.
        let mut slice_px: Vec<(usize, TrendMeta, Option<ArtPx>)> = Vec::with_capacity(slice.len());
        for (i, m) in slice {
            let px = decode_art_px(m.art.clone()).await;
            slice_px.push((i, m, px));
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<PodcastTrendCard> = slice_px.iter().map(|(i, m, px)| PodcastTrendCard {
                title: m.title.clone().into(),
                author: m.author.clone().into(),
                category: m.category.clone().into(),
                image: art_image(px),
                feed_url: m.feed_url.clone().into(),
                subscribed: subset.contains(&m.feed_url),
                index: *i as i32,
            }).collect();
            w.set_music_podcast_trends(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_music_podcast_trends_pages(pages as i32);
            w.set_music_podcast_trends_page(page as i32);
        });
    });
}

/// Subscribe to `url`, driving the subscribe progress bar (Trends button /
/// info-card Subscribe) as episodes are stored, then refresh the grids.
pub fn subscribe_feed_with_progress(weak: slint::Weak<MainWindow>, url: String) {
    tokio::runtime::Handle::current().spawn(async move {
        let finish = |weak: slint::Weak<MainWindow>| { let _ = weak.upgrade_in_event_loop(|w| { w.set_music_podcast_subscribing(false); }); };
        let Ok(pool) = pool_for("podcasts").await else { finish(weak); return; };
        let client = tulipix_core::net::http().clone();
        let resp = match client.get(&url).header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA).send().await {
            Ok(r) => r, Err(_) => { finish(weak); return; } };
        let xml = match resp.text().await { Ok(x) => x, Err(_) => { finish(weak); return; } };
        let feed = tulipix_music::podcasts::parse_feed(&xml);
        if feed.title.is_none() && feed.episodes.is_empty() { finish(weak); return; }
        let cbw = weak.clone();
        let _ = tulipix_music::podcasts::subscribe_with_progress(&pool, &url, &feed, move |done, total| {
            // Throttle UI updates (~40 steps max) so we don't flood the event loop.
            let step = (total / 40).max(1);
            if total > 0 && (done % step == 0 || done >= total) {
                let frac = done as f32 / total as f32;
                let status = format!("Adding… {done}/{total}");
                let _ = cbw.upgrade_in_event_loop(move |w| {
                    w.set_music_podcast_subscribe_frac(frac);
                    w.set_music_podcast_subscribe_status(status.into());
                });
            }
        }).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_subscribe_frac(1.0);
            w.set_music_podcast_subscribing(false);
            w.set_music_podcast_info_subscribed(true);
            render_trends(&w);             // flip the card to ✓ Subscribed
            populate_podcasts(&w);
            populate_podcast_latest(&w);
        });
    });
}

/// Populate the Trends grid. The directory + metadata cache is
/// `tulipix_music::pod_trends::build`; this only session-caches the result and
/// renders it.
pub fn populate_podcast_trends(w: &MainWindow) {
    let weak = w.as_weak();
    let feeds = trend_feed_urls();
    let cached_now = trend_cache().lock().map(|g| g.len()).unwrap_or(0);
    if cached_now == feeds.len() { render_trends(w); return; }
    w.set_music_podcast_trends_loading(true);
    tokio::runtime::Handle::current().spawn(async move {
        let metas = match pool_for("podcasts").await {
            Ok(pool) => tulipix_music::pod_trends::build(&pool, tulipix_core::net::http()).await,
            Err(_) => Vec::new(),
        };
        if let Ok(mut g) = trend_cache().lock() { *g = metas; }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_trends_loading(false);
            render_trends(&w);
        });
    });
}

pub fn populate_podcasts(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let filter = pod_filter("subs");
        let like = format!("%{}%", filter.trim());
        let base = "SELECT p.id, COALESCE(p.title, p.feed_url), COALESCE(p.author,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), COALESCE(p.category,''),
                    (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id AND COALESCE(e.played,0) = 0),
                    (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id),
                    (SELECT COALESCE(MAX(e.published),0) FROM podcast_episodes e WHERE e.podcast_id = p.id),
                    COALESCE(p.home_pinned,0)
             FROM podcasts p";
        let rows: Vec<(i64, String, String, String, String, i64, i64, i64, i64)> = if filter.trim().is_empty() {
            sqlx::query_as(&format!("{base} ORDER BY p.title COLLATE NOCASE")).fetch_all(&pool).await.unwrap_or_default()
        } else {
            sqlx::query_as(&format!("{base} WHERE p.title LIKE ?1 OR p.author LIKE ?1 OR p.category LIKE ?1 OR p.feed_url LIKE ?1
                    OR EXISTS (SELECT 1 FROM podcast_episodes e WHERE e.podcast_id = p.id AND e.title LIKE ?1)
                 ORDER BY p.title COLLATE NOCASE"))
                .bind(&like).fetch_all(&pool).await.unwrap_or_default()
        };
        let client = tulipix_core::net::http().clone();
        let mut all: Vec<PodAllData> = Vec::with_capacity(rows.len());
        for (id, title, author, img, category, unplayed, episodes, latest, pinned) in rows {
            let art_path = resolve_artwork(&client, &format!("pod-{id}"), &img).await;
            let art = decode_art_px(art_path).await;
            all.push(PodAllData { id, title, author, category, art, unplayed, episodes, latest, pinned: pinned != 0 });
        }
        let _ = weak.upgrade_in_event_loop(move |w| render_podcast_cards(&w, &all));
    });
}

/// Build the Subscribed (filtered + 21/page) and Home (14/page) card models
/// from the full subscription list on the UI thread.
pub fn render_podcast_cards(w: &MainWindow, all: &[PodAllData]) {
    let to_card = |i: usize, d: &PodAllData| PodcastCard {
        id: d.id as i32,
        title: d.title.clone().into(),
        author: d.author.clone().into(),
        category: d.category.clone().into(),
        image: art_image(&d.art),
        unplayed: d.unplayed as i32,
        episodes: d.episodes as i32,
        index: i as i32,
        home_pinned: d.pinned,
    };
    w.set_music_podcast_total(all.len() as i32);
    // Categories.
    let mut cats: Vec<String> = vec!["All".into()];
    for d in all { if !d.category.is_empty() && !cats.contains(&d.category) { cats.push(d.category.clone()); } }
    let cat_models: Vec<slint::SharedString> = cats.iter().map(|c| c.clone().into()).collect();
    w.set_music_podcast_categories(slint::ModelRc::new(slint::VecModel::from(cat_models)));
    // Subscribed — category filter + 21/page.
    let active = w.get_music_podcast_cat().to_string();
    let filtered: Vec<(usize, &PodAllData)> = all.iter().enumerate()
        .filter(|(_, d)| active == "All" || d.category == active).collect();
    let sub_pages = filtered.len().div_ceil(PODCAST_SUB_PAGE).max(1);
    let sub_page = (w.get_music_podcast_sub_page().max(0) as usize).min(sub_pages - 1);
    let sub_cards: Vec<PodcastCard> = filtered.iter().skip(sub_page * PODCAST_SUB_PAGE).take(PODCAST_SUB_PAGE)
        .map(|(i, d)| to_card(*i, d)).collect();
    w.set_music_podcast_sub_pages(sub_pages as i32);
    w.set_music_podcast_sub_page(sub_page as i32);
    w.set_music_podcast_cards(slint::ModelRc::new(slint::VecModel::from(sub_cards)));
    // Home "Your shows" — only shows the user pinned (home_pinned), capped at 2
    // pages; the rest live on the Subscribed tab. 14/page, sortable.
    let home_sort = w.get_music_podcast_home_sort().to_string();
    let mut home_order: Vec<(usize, &PodAllData)> = all.iter().enumerate().filter(|(_, d)| d.pinned).collect();
    match home_sort.as_str() {
        "category" => home_order.sort_by(|a, b| a.1.category.to_lowercase().cmp(&b.1.category.to_lowercase())
            .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
        "latest" => home_order.sort_by(|a, b| b.1.latest.cmp(&a.1.latest)
            .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
        _ => home_order.sort_by_key(|a| a.1.title.to_lowercase()),
    }
    let home_total = home_order.len().min(PODCAST_HOME_PAGE * PODCAST_HOME_MAX_PAGES);  // cap to 2 pages
    let home_pages = home_total.div_ceil(PODCAST_HOME_PAGE).max(1);
    let home_page = (w.get_music_podcast_home_page().max(0) as usize).min(home_pages - 1);
    let home_cards: Vec<PodcastCard> = home_order.iter().take(home_total)
        .skip(home_page * PODCAST_HOME_PAGE).take(PODCAST_HOME_PAGE)
        .map(|(i, d)| to_card(*i, d)).collect();
    w.set_music_podcast_home_pages(home_pages as i32);
    w.set_music_podcast_home_page(home_page as i32);
    w.set_music_podcast_home_cards(slint::ModelRc::new(slint::VecModel::from(home_cards)));
}

/// Open a feed's detail page by podcast id — header + episode list.
pub fn open_podcast(w: &MainWindow, pid_i32: i32) {
    let pid = pid_i32 as i64;
    if pid < 0 { return; }
    if let Ok(mut g) = cur_podcast_id().lock() { *g = pid; }
    // Clear the previous feed's header + episodes synchronously so opening a
    // different podcast never flashes the old one while the new page loads.
    w.set_music_podcast_d_title("Loading…".into());
    w.set_music_podcast_d_author("".into());
    w.set_music_podcast_d_category("".into());
    w.set_music_podcast_d_desc("".into());
    w.set_music_podcast_d_desc_short("".into());
    w.set_music_podcast_d_image(slint::Image::default());
    w.set_music_podcast_d_episodes(slint::ModelRc::new(slint::VecModel::from(Vec::<PodcastEpisodeRow>::new())));
    w.set_music_podcast_detail_open(true);
    w.set_music_podcast_d_page(0);
    w.set_music_podcast_d_id(pid_i32);
    load_podcast_detail(w, pid);
}

const PODCAST_PAGE: i64 = 15;

/// (Re)load the header + a 20-episode page for podcast `pid` into the detail
/// view, ordered by the chosen sort (newest/oldest first).
pub fn load_podcast_detail(w: &MainWindow, pid: i64) {
    let sort = w.get_music_podcast_d_sort().to_string();
    let page = w.get_music_podcast_d_page().max(0) as i64;
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let head: Option<(String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT COALESCE(title, feed_url), COALESCE(author,''), COALESCE(category,''), COALESCE(description,''), COALESCE(NULLIF(custom_image,''), image_url, ''), COALESCE(home_pinned,0)
             FROM podcasts WHERE id = ?").bind(pid).fetch_optional(&pool).await.ok().flatten();
        let filter = pod_filter("detail");
        let like = format!("%{}%", filter.trim());
        let has_f = !filter.trim().is_empty();
        let extra = if has_f { " AND title LIKE ?".to_string() } else { String::new() };
        let total: i64 = {
            let cq = format!("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?{extra}");
            let mut qb = sqlx::query_scalar(&cq).bind(pid);
            if has_f { qb = qb.bind(like.clone()); }
            qb.fetch_one(&pool).await.unwrap_or(0)
        };
        let pages = ((total + PODCAST_PAGE - 1) / PODCAST_PAGE).max(1);
        let page = page.min(pages - 1);
        let order = if sort == "old" { "ASC" } else { "DESC" };
        let q = format!(
            "SELECT id, COALESCE(title,''), audio_url, published, duration_s, COALESCE(image_url,''), downloaded_path, COALESCE(played,0)
                    , COALESCE(position_s, 0)
             FROM podcast_episodes WHERE podcast_id = ?{extra}
             ORDER BY COALESCE(published,0) {order}, id {order} LIMIT ? OFFSET ?");
        let mut qb = sqlx::query_as(&q).bind(pid);
        if has_f { qb = qb.bind(like); }
        let eps: Vec<(i64, String, String, Option<i64>, Option<f64>, String, Option<String>, i64, f64)> =
            qb.bind(PODCAST_PAGE).bind(page * PODCAST_PAGE)
                .fetch_all(&pool).await.unwrap_or_default();
        let client = tulipix_core::net::http().clone();
        // Only the show artwork is fetched here (usually already cached from the
        // grid). Per-episode thumbs are NOT fetched on the detail page — fetching
        // 20 images serially was the main cause of the slow open; every row falls
        // back to the show art, which is what most podcast apps show anyway.
        let head_art = if let Some((_, _, _, _, img, _)) = &head { resolve_artwork(&client, &format!("pod-{pid}"), img).await } else { None };
        let head_px = decode_art_px(head_art).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Some((title, author, category, desc, _, pinned)) = &head {
                w.set_music_podcast_d_title(title.clone().into());
                w.set_music_podcast_d_author(author.clone().into());
                w.set_music_podcast_d_category(category.clone().into());
                w.set_music_podcast_d_desc(desc.clone().into());
                // Info-card copy caps at 200 chars — the ⓘ Info popup keeps the
                // full text (char-boundary safe for multi-byte scripts).
                let short = if desc.chars().count() > 200 {
                    let mut s: String = desc.chars().take(200).collect();
                    s.push('…');
                    s
                } else { desc.clone() };
                w.set_music_podcast_d_desc_short(short.into());
                w.set_music_podcast_d_pinned(*pinned != 0);
            }
            w.set_music_podcast_d_image(art_image(&head_px));
            w.set_music_podcast_d_total(total as i32);
            w.set_music_podcast_d_pages(pages as i32);
            w.set_music_podcast_d_page(page as i32);
            let queued = podcast_dl_queue().lock().map(|g| g.clone()).unwrap_or_default();
            let up_next = podcast_queue().lock().map(|g| g.clone()).unwrap_or_default();
            let rows: Vec<PodcastEpisodeRow> = eps.iter().enumerate().map(|(i, (id, title, _url, pub_, dur, _img, dl, played, pos))| PodcastEpisodeRow {
                id: *id as i32,
                title: title.clone().into(),
                show: slint::SharedString::new(),
                date: pub_.map(fmt_date).unwrap_or_default().into(),
                duration: dur.map(|d| fmt_clock(d).into()).unwrap_or_default(),
                // Show artwork for every row (per-episode thumbs skipped for speed).
                image: art_image(&head_px),
                played: *played != 0,
                downloaded: dl.is_some(),
                queued: queued.contains(&(*id as i32)),
                dlinfo: slint::SharedString::new(),
                index: i as i32,
                // Needs a duration to be a fraction of; an episode whose feed
                // omitted one shows no bar rather than a wrong one.
                progress: match dur {
                    Some(d) if *d > 0.0 && *pos > 0.0 => (*pos / *d).clamp(0.0, 1.0) as f32,
                    _ => 0.0,
                },
                up_next: up_next.contains(&(*id as i32)),
            }).collect();
            w.set_music_podcast_d_episodes(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

// Send-safe intermediate (no slint::Image) for cross-thread episode rows.
#[derive(Clone)]
pub struct EpRowData {
    pub id: i32,
    pub title: String,
    pub show: String,
    pub date: String,
    pub duration: String,
    pub art: Option<ArtPx>,
    pub played: bool,
    pub downloaded: bool,
    pub dlinfo: String,    // "12 Jun · 14:32" — when the offline copy was stored
    /// 0..1 through the episode. Playback has always written `position_s`, and
    /// resume has always read it — but `played` is a boolean, so a 90-minute
    /// episode abandoned at minute 70 looked exactly like an untouched one.
    pub progress: f32,
}

/// Build the Home (Latest) feed — newest 14, one episode per show. Episode art
/// falls back to the show's feed artwork.
pub fn populate_podcast_latest(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let filter = pod_filter("home");
        let like = format!("%{}%", filter.trim());
        let extra = if filter.trim().is_empty() { "" } else { " AND (e.title LIKE ?1 OR p.title LIKE ?1)" };
        // Rank once per episode, then keep the top of each partition — NOT a
        // correlated subquery. The previous form asked "is this row the newest
        // of its show?" by re-running an ORDER BY ... LIMIT 1 over that show's
        // episodes for EVERY episode in the table, and `COALESCE(published,0)`
        // made that inner sort unindexable, so each one was a fresh scan. It
        // logged a 1.05 s slow-statement warning to return 7 rows.
        //
        // The COALESCE was also unnecessary. SQLite sorts NULL below every
        // value, so `published DESC` already puts undated episodes last —
        // exactly what mapping them to 0 achieved — and dropping it lets
        // `podcast_episodes_pod_idx (podcast_id, published DESC)` serve the
        // window's PARTITION BY / ORDER BY. (The two differ only for an episode
        // published exactly at the epoch, which would tie with undated ones
        // instead of sorting just above them.)
        let q = format!(
            "SELECT e.id, COALESCE(e.title,''), e.audio_url, e.published, e.duration_s,
                    COALESCE(e.image_url,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), p.id, e.downloaded_path, COALESCE(e.played,0), COALESCE(p.title,''), e.downloaded_at
             FROM (SELECT *, ROW_NUMBER() OVER (
                       PARTITION BY podcast_id ORDER BY published DESC, id DESC) AS rn
                   FROM podcast_episodes) e
             JOIN podcasts p ON p.id = e.podcast_id
             WHERE e.rn = 1{extra}
             ORDER BY e.published DESC, e.id DESC LIMIT 14");
        let mut qb = sqlx::query_as(&q);
        if !filter.trim().is_empty() { qb = qb.bind(like); }
        let eps: Vec<EpQueryRow> = qb.fetch_all(&pool).await.unwrap_or_default();
        let data = build_episode_data(eps).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_latest(slint::ModelRc::new(slint::VecModel::from(rows_from_data(&data))));
        });
    });
}

const PODCAST_DL_PAGE: i64 = 20;

/// Build the Downloads tab — cached episodes, sorted + paginated (20/page).
/// Pending episode downloads (FIFO) — clicks beyond the active one wait here.
pub fn podcast_dl_queue() -> &'static std::sync::Mutex<std::collections::VecDeque<i32>> {
    static C: OnceLock<std::sync::Mutex<std::collections::VecDeque<i32>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// True while a drain worker is alive (one worker, sequential downloads).
pub static PODCAST_DL_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Download ONE episode to the offline cache, driving the header progress bar
/// (dl-id / dl-frac / dl-title). Returns when the file is stored or failed.
pub async fn podcast_download_one(weak: slint::Weak<MainWindow>, id: i32) {
    let Ok(pool) = pool_for("podcasts").await else { return; };
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT audio_url, downloaded_path, COALESCE(title,'') FROM podcast_episodes WHERE id = ?")
        .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
    let Some((url, dl, title)) = row else { return; };
    if url.is_empty() || dl.is_some() { return; }
    let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("podcast_offline");
    let _ = std::fs::create_dir_all(&dir);
    let ext = url.split('?').next().unwrap_or(&url).rsplit('.').next()
        .filter(|e| e.len() <= 4 && !e.contains('/')).unwrap_or("mp3").to_string();
    let dest = dir.join(format!("ep-{id}.{ext}"));
    // Mark this row as the in-flight download.
    let wk = weak.clone();
    let _ = wk.upgrade_in_event_loop(move |w| {
        w.set_music_podcast_dl_id(id);
        w.set_music_podcast_dl_frac(0.0);
        w.set_music_podcast_dl_title(title.into());
    });
    let client = tulipix_core::net::http_stream().clone();
    let ok = {
        use std::io::Write;
        let mut got: u64 = 0;
        let mut last_pct: i32 = -1;
        let result: Option<()> = async {
            let mut resp = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_core::net::BROWSER_UA)
                .send().await.ok()?;
            let total = resp.content_length();
            let mut file = std::fs::File::create(&dest).ok()?;
            while let Some(chunk) = resp.chunk().await.ok()? {
                file.write_all(&chunk).ok()?;
                got += chunk.len() as u64;
                if let Some(t) = total {
                    if t > 0 {
                        let pct = ((got as f64 / t as f64) * 100.0) as i32;
                        if pct != last_pct {
                            last_pct = pct;
                            let frac = (got as f64 / t as f64) as f32;
                            let wk = weak.clone();
                            let _ = wk.upgrade_in_event_loop(move |w| w.set_music_podcast_dl_frac(frac));
                        }
                    }
                }
            }
            Some(())
        }.await;
        result.is_some()
    };
    if ok {
        let _ = tulipix_music::podcasts::mark_downloaded(&pool, id as i64, dest.to_string_lossy().as_ref()).await;
    } else {
        let _ = std::fs::remove_file(&dest);
    }
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_podcast_dl_id(-1);
        w.set_music_podcast_dl_frac(0.0);
        w.set_music_podcast_dl_title("".into());
        refresh_podcast_views(&w);
    });
}

pub fn populate_podcast_downloads(w: &MainWindow) {
    let weak = w.as_weak();
    let sort = w.get_music_podcast_dl_sort().to_string();
    let page = w.get_music_podcast_dl_page().max(0) as i64;
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let filter = pod_filter("downloads");
        let like = format!("%{}%", filter.trim());
        let has_f = !filter.trim().is_empty();
        let extra = if has_f { " AND (e.title LIKE ? OR p.title LIKE ?)".to_string() } else { String::new() };
        let total: i64 = {
            let cq = format!("SELECT COUNT(*) FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id WHERE e.downloaded_path IS NOT NULL{extra}");
            let mut qb = sqlx::query_scalar(&cq);
            if has_f { qb = qb.bind(like.clone()).bind(like.clone()); }
            qb.fetch_one(&pool).await.unwrap_or(0)
        };
        let pages = ((total + PODCAST_DL_PAGE - 1) / PODCAST_DL_PAGE).max(1);
        let page = page.min(pages - 1);
        // "dl" (default) = most recently downloaded first ("dl-asc" flips it);
        // new/old = by publish date.
        let order_by = match sort.as_str() {
            "old" => "COALESCE(e.published,0) ASC, e.id ASC",
            "new" => "COALESCE(e.published,0) DESC, e.id DESC",
            "dl-asc" => "COALESCE(e.downloaded_at, e.published, 0) ASC, e.id ASC",
            _ => "COALESCE(e.downloaded_at, e.published, 0) DESC, e.id DESC",
        };
        let q = format!(
            "SELECT e.id, COALESCE(e.title,''), e.audio_url, e.published, e.duration_s,
                    COALESCE(e.image_url,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), p.id, e.downloaded_path, COALESCE(e.played,0), COALESCE(p.title,''), e.downloaded_at
             FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id
             WHERE e.downloaded_path IS NOT NULL{extra}
             ORDER BY {order_by} LIMIT ? OFFSET ?");
        let mut qb = sqlx::query_as(&q);
        if has_f { qb = qb.bind(like.clone()).bind(like); }
        let eps: Vec<EpQueryRow> =
            qb.bind(PODCAST_DL_PAGE).bind(page * PODCAST_DL_PAGE).fetch_all(&pool).await.unwrap_or_default();
        let data = build_episode_data(eps).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_dl_pages(pages as i32);
            w.set_music_podcast_dl_page(page as i32);
            w.set_music_podcast_downloads(slint::ModelRc::new(slint::VecModel::from(rows_from_data(&data))));
        });
    });
}

// Cross-show episode query row: id, title, audio_url, published, duration_s,
// episode_image, show_image, podcast_id, downloaded_path, played, show_title,
// downloaded_at (offline-copy timestamp).
type EpQueryRow = (i64, String, String, Option<i64>, Option<f64>, String, String, i64, Option<String>, i64, String, Option<i64>);

/// Off-thread: cache artwork + flatten a cross-show episode query into Send data.
pub async fn build_episode_data(eps: Vec<EpQueryRow>) -> Vec<EpRowData> {
    let client = tulipix_core::net::http().clone();
    // One extra query for the whole batch rather than a column added to each
    // of the several differently-shaped episode SELECTs that feed this.
    let progress: std::collections::HashMap<i64, f32> = if eps.is_empty() {
        Default::default()
    } else {
        let ids = eps.iter().map(|e| e.0.to_string()).collect::<Vec<_>>().join(",");
        match pool_for("podcasts").await {
            Ok(pool) => sqlx::query_as::<_, (i64, f64, Option<f64>)>(&format!(
                "SELECT id, COALESCE(position_s, 0), duration_s FROM podcast_episodes WHERE id IN ({ids})"))
                .fetch_all(&pool).await.unwrap_or_default()
                .into_iter()
                .filter_map(|(id, pos, dur)| match dur {
                    Some(d) if d > 0.0 && pos > 0.0 => Some((id, (pos / d).clamp(0.0, 1.0) as f32)),
                    _ => None,
                }).collect(),
            Err(_) => Default::default(),
        }
    };
    let mut out = Vec::with_capacity(eps.len());
    for (id, title, _url, pub_, dur, ep_img, show_img, pid, dl, played, show, dl_at) in eps {
        // Prefer the episode's own image; else the show artwork — keyed `pod-{id}`
        // so it reuses the file the grid already cached (instant, never blank).
        let art = if !ep_img.is_empty() { cache_artwork(&client, &format!("ep-{id}"), &ep_img).await } else { None };
        let art = match art {
            Some(p) => Some(p),
            None if !show_img.is_empty() => resolve_artwork(&client, &format!("pod-{pid}"), &show_img).await,
            None => None,
        };
        let art = decode_art_px(art).await;
        out.push(EpRowData {
            id: id as i32,
            title,
            show,
            date: pub_.map(fmt_date).unwrap_or_default(),
            duration: dur.map(fmt_clock).unwrap_or_default(),
            art,
            played: played != 0,
            downloaded: dl.is_some(),
            dlinfo: dl_at.map(fmt_dl_stamp).unwrap_or_default(),
            progress: progress.get(&id).copied().unwrap_or(0.0),
        });
    }
    out
}

/// Short local download stamp — "12 Jun · 14:32".
pub fn fmt_dl_stamp(epoch: i64) -> String {
    use chrono::{Local, TimeZone, Datelike, Timelike};
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match Local.timestamp_opt(epoch, 0).single() {
        Some(dt) => format!("{} {} · {:02}:{:02}", dt.day(), MON[(dt.month0() as usize).min(11)], dt.hour(), dt.minute()),
        None => String::new(),
    }
}

/// On the UI thread: turn Send data into UI rows (loads slint::Image).
pub fn rows_from_data(data: &[EpRowData]) -> Vec<PodcastEpisodeRow> {
    let queued = podcast_dl_queue().lock().map(|g| g.clone()).unwrap_or_default();
    // Two different queues share this row: `queued` is waiting to DOWNLOAD,
    // `up_next` is waiting to PLAY.
    let up_next = podcast_queue().lock().map(|g| g.clone()).unwrap_or_default();
    data.iter().enumerate().map(|(i, d)| PodcastEpisodeRow {
        id: d.id,
        title: d.title.clone().into(),
        show: d.show.clone().into(),
        date: d.date.clone().into(),
        duration: d.duration.clone().into(),
        image: art_image(&d.art),
        played: d.played,
        downloaded: d.downloaded,
        queued: queued.contains(&d.id),
        dlinfo: d.dlinfo.clone().into(),
        index: i as i32,
        progress: d.progress,
        up_next: up_next.contains(&d.id),
    }).collect()
}

/// Repopulate whichever podcast surface is currently visible.
pub fn refresh_podcast_views(w: &MainWindow) {
    match w.get_music_podcast_tab().as_str() {
        "home" => { populate_podcasts(w); populate_podcast_latest(w); }
        "downloads" => populate_podcast_downloads(w),
        _ => populate_podcasts(w),
    }
    if w.get_music_podcast_detail_open() {
        let pid = cur_podcast_id().lock().map(|g| *g).unwrap_or(-1);
        if pid >= 0 { load_podcast_detail(w, pid); }
    }
}

/// Fill the audiobooks view with `is_audiobook` library tracks (np.p5.music.audiobook-chapters).
/// Folder basename → book title. The Flutter build shows the same name for the
/// same folder, so the rule lives in `tulipix_music::ab_meta`.
pub use tulipix_music::ab_meta::{book_query, book_title};

/// Seconds → "8h 12m" / "47m" pretty duration for book cards.
pub fn fmt_hm(secs: f64) -> String {
    let total = secs.max(0.0) as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    if h > 0 { format!("{}h {}m", h, m) } else { format!("{}m", m) }
}

/// In-memory maps used to turn library positions into title/artist/duration +
/// cover art (shared by the audiobook card + detail builders).
pub fn music_pos_maps() -> (std::collections::HashMap<i64, i32>, std::collections::HashMap<i32, (String, String, f64)>) {
    let pos_of = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let by_pos = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect()).unwrap_or_default();
    (pos_of, by_pos)
}

/// Per-book decoded cover, keyed by folder — filled by `populate_audiobooks`,
/// read synchronously by the detail hero.
pub fn ab_cover_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, ArtPx>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<String, ArtPx>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Audiobook cover (np.p5.music.audiobook-chapters), decoded for Slint.
///
/// Which FILE is the cover — stored choice, sidecar image, art extracted from
/// the first chapter — is `tulipix_music::ab_meta::cover_path`, shared with the
/// Flutter build so both read the same cache. This adds the decode and the
/// per-folder memo; `None` keeps the 📚 monogram.
pub async fn audiobook_cover_px(folder: &str, custom: Option<PathBuf>, first_chapter: Option<PathBuf>) -> Option<ArtPx> {
    // A user-chosen cover outranks everything and bypasses the memo, so a
    // change shows immediately.
    if let Some(c) = custom.filter(|p| p.is_file()) {
        let px = decode_art_px(Some(c)).await?;
        if let Ok(mut g) = ab_cover_cache().lock() { g.insert(folder.to_string(), px.clone()); }
        return Some(px);
    }
    if let Some(hit) = ab_cover_cache().lock().ok().and_then(|g| g.get(folder).cloned()) {
        return Some(hit);
    }
    let found = tulipix_music::ab_meta::cover_path(folder, None, first_chapter.as_deref()).await;
    let px = decode_art_px(found).await?;
    if let Ok(mut g) = ab_cover_cache().lock() { g.insert(folder.to_string(), px.clone()); }
    Some(px)
}

// ── Audiobook identity: title + author + cover (np.p7.music.audiobook-net) ──
// The five-method resolution chain lives in `tulipix_music::ab_meta`; what is
// left here is the session dedup and the Slint repaint it triggers.

/// Folders already looked up online this session (hit or miss) — populate
/// never re-hits the network or loops on books the internet doesn't know.
fn ab_net_tried() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

/// (title, author) per book folder (from `audiobook_meta`), for cards + detail.
pub fn ab_meta_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, String)>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, String)>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Book display title: resolved real title when known, else folder basename.
pub fn book_display_title(folder: &str) -> String {
    ab_meta_cache().lock().ok()
        .and_then(|g| g.get(folder).map(|(t, _)| t.clone()))
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| book_title(folder))
}

/// Background pass: resolve title + author + cover for each folder, persist,
/// then re-populate the cards once. Session-deduped per folder; the resolution
/// chain itself is `tulipix_music::ab_meta`, shared with the Flutter build so
/// one library does not resolve two different titles for the same book.
fn kick_ab_net_lookup(weak: slint::Weak<MainWindow>, folders: Vec<String>) {
    tokio::runtime::Handle::current().spawn(async move {
        let todo: Vec<String> = {
            let Ok(mut g) = ab_net_tried().lock() else { return; };
            folders.into_iter().filter(|f| g.insert(f.clone())).collect()
        };
        if todo.is_empty() { return; }
        let Ok(pool) = pool_for("music").await else { return; };
        if tulipix_music::ab_meta::resolve_and_store(&pool, &todo).await {
            // A better cover may have replaced the one already decoded.
            if let Ok(mut g) = ab_cover_cache().lock() {
                for f in &todo { g.remove(f); }
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_audiobooks(&w));
        }
    });
}

/// Fill the audiobook detail hero + chapter list for one folder.
/// Build the ChapterRow list for one book's ordered chapter ids — shared by
/// the detail page and the BookMini chapter queue. Returns
/// `(rows, first_pos, resume_pos, total_s)`; `resume_pos` is the CURRENT
/// chapter's playback position (-1 when the book was never touched).
pub fn chapter_rows_for(
    ids: &[i64], listened: &[i64], current: Option<i64>,
) -> (Vec<ChapterRow>, i32, i32, f64) {
    let (pos_of, by_pos) = music_pos_maps();
    let mut total = 0.0;
    let mut first_pos = -1;
    let mut resume_pos = -1;
    let mut rows: Vec<ChapterRow> = Vec::new();
    for id in ids {
        let Some(&pos) = pos_of.get(id) else { continue; };
        if first_pos < 0 { first_pos = pos; }
        let is_cur = current == Some(*id);
        if is_cur { resume_pos = pos; }
        let (title, _artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        total += dur;
        let n = rows.len() + 1;
        rows.push(ChapterRow {
            title: if title.is_empty() { format!("Chapter {}", n).into() } else { title.into() },
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
            played: !is_cur && listened.contains(id),
            current: is_cur,
        });
    }
    (rows, first_pos, resume_pos, total)
}

pub fn fill_book_detail(
    w: &MainWindow, folder: &str, ids: &[i64], listened: &[i64], current: Option<i64>,
) {
    let (rows, first_pos, resume_pos, total) = chapter_rows_for(ids, listened, current);
    // Real book cover (folder image / embedded art) decoded by the cards
    // populate; tile thumb only as the last resort.
    let cover = ab_cover_cache().lock().ok()
        .and_then(|g| g.get(folder).cloned())
        .map(slint::Image::from_rgba8)
        .unwrap_or_else(|| music_thumb_at(first_pos));
    w.set_music_ab_d_title(book_display_title(folder).into());
    w.set_music_ab_d_author(ab_meta_cache().lock().ok()
        .and_then(|g| g.get(folder).map(|(_, a)| a.clone())).unwrap_or_default().into());
    w.set_music_ab_d_cover(cover);
    w.set_music_ab_d_total(fmt_hm(total).into());
    w.set_music_ab_d_chapters(slint::ModelRc::new(slint::VecModel::from(rows)));
    // Resume = start of the CURRENT chapter (never a mid-chapter seek).
    w.set_music_ab_d_resume_index(if resume_pos >= 0 { resume_pos } else { first_pos });
    w.set_music_audiobook_detail_open(true);
    load_book_detail_bookmarks(w, ids.to_vec());
}

/// The open book's chapter ids, so a bookmark action knows what it belongs to
/// without re-deriving it from the folder.
pub static AB_OPEN_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
pub fn ab_open_ids() -> &'static std::sync::Mutex<Vec<i64>> { AB_OPEN_IDS.get_or_init(Default::default) }
/// Parallel to the label/time models: which `(item_id, position_s)` each row is.
pub static AB_OPEN_MARKS: std::sync::OnceLock<std::sync::Mutex<Vec<(i64, f64)>>> = std::sync::OnceLock::new();
pub fn ab_open_marks() -> &'static std::sync::Mutex<Vec<(i64, f64)>> { AB_OPEN_MARKS.get_or_init(Default::default) }

/// Load every bookmark in the open book into the detail page's three parallel
/// models, plus the finished flag the Mark-finished button reflects.
pub fn load_book_detail_bookmarks(w: &MainWindow, ids: Vec<i64>) {
    if let Ok(mut g) = ab_open_ids().lock() { *g = ids.clone(); }
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let marks = tulipix_music::audiobooks::book_bookmarks(&pool, &ids).await.unwrap_or_default();
        // This book's remembered narration speed, so opening it restores the
        // pace it was last listened at rather than the previous book's.
        let speed = match ids.first() {
            Some(first) => tulipix_music::audiobooks::book_speed(&pool, *first).await.unwrap_or(1.0),
            None => 1.0,
        };
        // Finished when every chapter is. A part-finished book is in progress.
        let finished = if ids.is_empty() { false } else {
            let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
            let done: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM audiobook_progress WHERE finished = 1 AND item_id IN ({list})"))
                .fetch_one(&pool).await.unwrap_or(0);
            done as usize == ids.len()
        };
        // Chapter number comes from the position in `ids`, which is book order.
        let rank: std::collections::HashMap<i64, usize> =
            ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let labels: Vec<slint::SharedString> =
                marks.iter().map(|(_, _, l)| l.clone().into()).collect();
            let times: Vec<slint::SharedString> = marks.iter().map(|(id, p, _)| {
                let ch = rank.get(id).map(|i| i + 1).unwrap_or(0);
                let s = if ch > 0 { format!("Ch {ch} · {}", fmt_clock(*p)) } else { fmt_clock(*p) };
                s.into()
            }).collect();
            if let Ok(mut g) = ab_open_marks().lock() {
                *g = marks.iter().map(|(id, p, _)| (*id, *p)).collect();
            }
            w.set_music_ab_d_bm_labels(slint::ModelRc::new(slint::VecModel::from(labels)));
            w.set_music_ab_d_bm_times(slint::ModelRc::new(slint::VecModel::from(times)));
            w.set_music_ab_d_finished(finished);
            w.set_music_ab_d_bm_edit(-1);
            w.set_music_book_speed(speed as f32);
        });
    });
}

pub fn populate_audiobooks(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // Reflect folder→section assignments: flag every audiobook-section
        // folder so its (now-scanned) tracks show up grouped below. Idempotent,
        // and the only thing that makes the tag mean anything once the scan has
        // finally produced the rows it applies to.
        let sections = load_folder_sections();
        let _ = tulipix_music::audiobooks::apply_folder_sections(&pool, &sections).await;
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE is_audiobook = 1")
            .fetch_all(&pool).await.unwrap_or_default();
        // User-chosen + net-resolved covers, and the resolved title/author
        // (np.p5.music.audiobook-chapters / np.p7.music.audiobook-net).
        tulipix_music::ab_meta::ensure_tables(&pool).await;
        let custom_covers = tulipix_music::ab_meta::load_covers(&pool).await;
        let metas = tulipix_music::ab_meta::load_meta(&pool).await;
        if let Ok(mut g) = ab_meta_cache().lock() { *g = metas.clone(); }
        // Per-chapter resume positions → book status (in-progress / finished).
        let progress: std::collections::HashMap<i64, f64> = sqlx::query_as(
            "SELECT item_id, position_s FROM audiobook_progress")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        // One (folder, ordered chapter ids) entry per book card. Covers come
        // from custom art / a folder image / embedded album art, decoded off
        // the UI thread.
        let books = tulipix_music::audiobooks::book_folders(&pool).await.unwrap_or_default();
        let (pos_of, by_pos) = music_pos_maps();
        let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
        // (folder, chapter ids, total seconds, cover, resume 0..1, finished)
        let mut book_data: Vec<(String, Vec<i64>, f64, Option<ArtPx>, f32, bool)> =
            Vec::with_capacity(books.len());
        for (folder, _n) in &books {
            let cids = tulipix_music::audiobooks::book_chapters(&pool, folder).await.unwrap_or_default();
            let total: f64 = cids.iter()
                .filter_map(|id| pos_of.get(id))
                .filter_map(|pos| by_pos.get(pos).map(|(_, _, d)| *d))
                .sum();
            let first_path = cids.first()
                .and_then(|id| pos_of.get(id))
                .and_then(|&pos| paths.get(pos as usize).cloned());
            let custom = custom_covers.get(folder).map(PathBuf::from);
            let cover = audiobook_cover_px(folder, custom, first_path).await;
            // Furthest chapter with a saved position drives the resume bar;
            // "finished" = saved position ≥90% through the LAST chapter.
            let n = cids.len().max(1);
            let mut resume = 0.0f32;
            let mut finished = false;
            for (i, id) in cids.iter().enumerate() {
                let Some(&pos_s) = progress.get(id) else { continue; };
                let dur = pos_of.get(id).and_then(|p| by_pos.get(p)).map(|(_, _, d)| *d).unwrap_or(0.0);
                let frac_in = if dur > 1.0 { (pos_s / dur).clamp(0.0, 1.0) } else { 0.0 };
                resume = resume.max((i as f64 + frac_in) as f32 / n as f32);
                if i == n - 1 && frac_in >= 0.9 { finished = true; }
            }
            book_data.push((folder.clone(), cids, total, cover, resume, finished));
        }
        // Resolution pass targets: books with no art anywhere, plus books whose
        // cover came from an EARLIER net lookup that didn't yet resolve a real
        // title (the improved chain re-resolves those once). User-chosen covers
        // are never touched. Session-deduped inside the kick.
        let with_art: std::collections::HashSet<String> = book_data.iter()
            .filter(|(_, _, _, c, ..)| c.is_some()).map(|(f, ..)| f.clone()).collect();
        let folders: Vec<String> = book_data.iter().map(|(f, ..)| f.clone()).collect();
        let missing = tulipix_music::ab_meta::needs_lookup(
            &folders, &custom_covers, &metas, |f| with_art.contains(f));
        if !missing.is_empty() { kick_ab_net_lookup(weak.clone(), missing); }
        let tab = ab_tab().lock().map(|g| g.clone()).unwrap_or_default();
        // Folders view rows: "name · N chapters — /path".
        let folder_rows: Vec<String> = book_data.iter()
            .map(|(f, c, ..)| format!("{}   ·   {} chapters   —   {}", book_display_title(f), c.len(), f))
            .collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<MusicSongRow> = ids.iter().filter_map(|id| pos_of.get(id).copied()).map(|pos| {
                let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
                MusicSongRow {
                    thumb: music_thumb_at(pos),
                    title: if title.is_empty() { "Track".into() } else { title.into() },
                    artist: artist.into(),
                    duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
                    index: pos,
                }
            }).collect();
            w.set_music_audiobooks(slint::ModelRc::new(slint::VecModel::from(rows)));
            let cards: Vec<BookCard> = book_data.iter()
                .filter(|(.., resume, finished)| match tab.as_str() {
                    "progress" => *resume > 0.0 && !finished,
                    "finished" => *finished,
                    _ => true,
                })
                .map(|(folder, cids, total, cover, resume, _)| BookCard {
                    id: folder.clone().into(),
                    // Real book title once resolved; folder basename until then.
                    title: metas.get(folder).map(|(t, _)| t.clone())
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| book_title(folder)).into(),
                    author: metas.get(folder).map(|(_, a)| a.clone()).unwrap_or_default().into(),
                    cover: art_image(cover),
                    chapters: cids.len() as i32,
                    total_time: fmt_hm(*total).into(),
                    resume_frac: *resume,
                }).collect();
            w.set_music_audiobook_cards(slint::ModelRc::new(slint::VecModel::from(cards)));
            let frows: Vec<slint::SharedString> = folder_rows.into_iter().map(Into::into).collect();
            w.set_music_ab_folders(slint::ModelRc::new(slint::VecModel::from(frows)));
        });
    });
}

/// Active Audiobooks sub-tab: all | progress | finished | folders.
pub fn ab_tab() -> &'static std::sync::Mutex<String> {
    static C: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new("all".to_string()))
}

/// Folder of the audiobook whose detail page is open (custom-cover target).
pub fn cur_book_folder() -> &'static std::sync::Mutex<String> {
    static C: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// File-picker → persist a custom cover for `folder` in audiobook_covers, then
/// refresh the cards, the open detail hero, and the now-playing art if a
/// chapter of this book is on the vinyl right now.
pub fn audiobook_pick_cover(weak: slint::Weak<MainWindow>, folder: String) {
    if folder.is_empty() { return; }
    tokio::runtime::Handle::current().spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
            .set_title("Choose audiobook cover")
            .pick_file().await else { return; };
        let path = file.path().to_path_buf();
        let Ok(pool) = pool_for("music").await else { return; };
        let _ = tulipix_music::ab_meta::set_cover(
            &pool, &folder, path.to_string_lossy().as_ref()).await;
        // Drop the stale decode and rebuild cards + the open detail hero.
        if let Ok(mut g) = ab_cover_cache().lock() { g.remove(&folder); }
        let ids = tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default();
        let (listened, current) = tulipix_music::audiobooks::chapter_states(&pool, &ids).await
            .unwrap_or_default();
        let px = audiobook_cover_px(&folder, Some(path), None).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            populate_audiobooks(&w);
            if w.get_music_audiobook_detail_open() {
                fill_book_detail(&w, &folder, &ids, &listened, current);
            }
            // Live vinyl art swap if this book is currently playing.
            let np = w.get_music_np_index();
            let np_folder = music_paths().lock().ok()
                .and_then(|g| g.get(np as usize).and_then(|p| p.parent().map(|d| d.display().to_string())));
            if np_folder.as_deref() == Some(folder.as_str()) {
                if let Some(px) = px { w.set_music_np_art(slint::Image::from_rgba8(px)); }
            }
        });
    });
}

/// Stream an arbitrary audio URL via a fresh headless mpv (podcast episodes).
/// Mirrors `play_music_at` minus the library-position bookkeeping.
/// Episode ids queued to play next, across shows.
///
/// Podcasts previously played one episode at a time and inherited whichever
/// list you started from; a queue you build yourself is the loop every podcast
/// app is actually used through. Held in memory and mirrored to settings, not
/// a new table — it is a short list of ids and losing it on a crash costs
/// nothing.
pub static PODCAST_QUEUE: std::sync::OnceLock<std::sync::Mutex<Vec<i32>>> = std::sync::OnceLock::new();
pub fn podcast_queue() -> &'static std::sync::Mutex<Vec<i32>> { PODCAST_QUEUE.get_or_init(Default::default) }

/// Push the queue into the UI (count + the id list the panel renders).
pub fn podcast_queue_sync(w: &MainWindow) {
    let ids = podcast_queue().lock().map(|g| g.clone()).unwrap_or_default();
    w.set_music_podcast_queue_len(ids.len() as i32);
    // Every other episode list draws the same toggle, so they have to be
    // rebuilt or the button lies about what is queued.
    refresh_podcast_views(w);
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert("music.podcast_queue".into(),
        ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(","));
    let _ = s.save();
    populate_podcast_queue(w);
}

/// Restore the queue saved last session.
pub fn podcast_queue_load(w: &MainWindow) {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    let ids: Vec<i32> = s.advanced.get("music.podcast_queue").map(|v| v.split(',')
        .filter_map(|t| t.trim().parse().ok()).collect()).unwrap_or_default();
    if let Ok(mut g) = podcast_queue().lock() { *g = ids; }
    podcast_queue_sync(w);
}

/// Pop the head of the queue and play it. No-op on an empty queue, which is
/// what makes this safe to call from every EOF.
pub fn podcast_queue_advance(w: &MainWindow) {
    let next = podcast_queue().lock().ok().and_then(|mut g| if g.is_empty() { None } else { Some(g.remove(0)) });
    let Some(id) = next else { return; };
    podcast_queue_sync(w);
    w.invoke_music_podcast_play(id);
}

/// Rows for the queue panel, in queue order. Reuses the episode-row builder so
/// a queued episode looks exactly like it does in any other list.
pub fn populate_podcast_queue(w: &MainWindow) {
    let ids = podcast_queue().lock().map(|g| g.clone()).unwrap_or_default();
    if ids.is_empty() {
        w.set_music_podcast_queue_rows(slint::ModelRc::new(slint::VecModel::<PodcastEpisodeRow>::default()));
        return;
    }
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        let eps: Vec<EpQueryRow> = sqlx::query_as(&format!(
            "SELECT e.id, COALESCE(e.title,''), e.audio_url, e.published, e.duration_s,
                    COALESCE(e.image_url,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''),
                    p.id, e.downloaded_path, COALESCE(e.played,0), COALESCE(p.title,''), e.downloaded_at
             FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id
             WHERE e.id IN ({list})")).fetch_all(&pool).await.unwrap_or_default();
        let data = build_episode_data(eps).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            // The SQL returns them in whatever order it likes; the queue's
            // order is the whole point, so reorder to match.
            let rank: std::collections::HashMap<i32, usize> =
                ids.iter().enumerate().map(|(i, id)| (*id, i)).collect();
            let mut rows = rows_from_data(&data);
            rows.sort_by_key(|r| rank.get(&r.id).copied().unwrap_or(usize::MAX));
            w.set_music_podcast_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

pub fn play_music_url(w: &MainWindow, url: &str, title: &str) {
    play_music_file(w, url, title, "Podcast");
}

/// Play an arbitrary local file / URL through the music player with an explicit
/// second-line label (`sub`). Backs both podcast streams and the Downloader's
/// "Play in app" from history.
pub fn play_music_file(w: &MainWindow, url: &str, title: &str, sub: &str) {
    if let Ok(mut g) = yt_cur_audio().lock() { g.clear(); }
    w.set_music_yt_now_video(false);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
    let mut pre_args = vec![format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32)];
    if w.get_music_muted() { pre_args.push("--mute=yes".into()); }
    pre_args.push(format!("--af={}", music_full_af(&music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10])))));
    pre_args.extend(music_device_args());
    // Episode audio is served from the same CDNs that bot-filter the feed, and
    // they answer mpv's default agent with 403 — see `net::BROWSER_UA`.
    if url.starts_with("http") {
        pre_args.push(format!("--user-agent={}", tulipix_core::net::BROWSER_UA));
    }
    let weak = w.as_weak();
    let on_prop = move |name: &str, data: &serde_json::Value| {
        let name = name.to_string();
        let data = data.clone();
        let wk = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = wk.upgrade() else { return; };
            match name.as_str() {
                "time-pos" => if let Some(d) = data.as_f64() { w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into()); }
                "duration" => if let Some(d) = data.as_f64() { w.set_music_dur(d as f32); w.set_music_dur_label(fmt_clock(d).into()); }
                "pause" => if let Some(p) = data.as_bool() { w.set_music_playing(!p); }
                _ => {}
            }
        });
    };
    let eof_weak = w.as_weak();
    // Reaching the end hands off to the podcast queue if anything is waiting;
    // otherwise playback simply stops, as before.
    let on_eof = move || { let _ = eof_weak.upgrade_in_event_loop(|w| {
        w.set_music_playing(false);
        if w.get_music_player_mode().as_str() == "podcast" { podcast_queue_advance(&w); }
    }); };
    if let Err(e) = player::spawn_audio(player::AudioLaunch {
        prefix: "tulipix-music",
        mpv_bin: tulipix_core::thumbs::tool_bin("mpv"),
        src: std::path::Path::new(url),
        pre_args,
        observe: &[(1, "time-pos"), (2, "duration"), (3, "pause")],
        generation: my_gen,
    }, on_prop, on_eof) {
        tracing::error!(error = %e, "mpv stream launch failed"); return;
    }
    w.set_music_np_title(title.into());
    w.set_music_np_sub(sub.into());
    w.set_music_np_album("".into());      // no stale artist·album on the second line
    // Local files carry embedded cover art — extract it so the player isn't blank
    // (podcast/radio URLs have no thumb and fall back to the default).
    let art = std::path::Path::new(url)
        .exists()
        .then(|| tulipix_core::thumbs::render_or_cache(
            std::path::Path::new(url),
            tulipix_core::thumbs::ThumbSpec {
                kind: tulipix_core::thumbs::ThumbKind::Audio, width: 320, height: 320 })
            .ok().flatten().map(|t| t.path))
        .flatten()
        .and_then(|p| slint::Image::load_from_path(&p).ok())
        .unwrap_or_default();
    w.set_music_np_art(art);
    w.set_music_radio_np_uuid("".into()); // a non-radio stream ends any LIVE state
    // A stream is not a library track: nothing here has lyrics, and the last
    // song's would otherwise keep scrolling behind it.
    clear_music_lyrics(w);
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("0:00".into());
}

// ── Internet radio (np.p4.music.radio) ──────────────────────────────────────
// Curated India-first presets + radio-browser search; favourites/recents live
// in radio.db; playback is a headless mpv whose ICY `media-title` becomes the
// live now-playing line.

/// The full station list behind the open radio page. Row `index` fields point
/// into THIS Vec, so sorting/paging the view never breaks play/fav targets.
pub fn radio_list() -> &'static std::sync::Mutex<Vec<tulipix_music::radio::Station>> {
    static C: OnceLock<std::sync::Mutex<Vec<tulipix_music::radio::Station>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Favourite uuids backing the heart flags of the current list.
pub fn radio_favs() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// Downloaded favicon paths by station uuid — survives re-renders (sort/page).
pub fn radio_icons() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// Monotonic fetch generation — a slow response from a previous genre/search
/// can never overwrite a newer list (nor can its favicon updates).
static RADIO_FETCH_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn radio_quality(codec: &str, bitrate: u32) -> String {
    let codec_ok = !codec.is_empty() && codec != "UNKNOWN";
    match (codec_ok, bitrate) {
        (false, 0) => String::new(),
        (false, b) => format!("{b}k"),
        (true, 0) => codec.to_uppercase(),
        (true, b) => format!("{} · {b}k", codec.to_uppercase()),
    }
}

/// Install a NEW station list (resets pagination) and render it.
pub fn radio_render(w: &MainWindow, stations: Vec<tulipix_music::radio::Station>, favs: &std::collections::HashSet<String>) {
    if let Ok(mut g) = radio_list().lock() { *g = stations; }
    if let Ok(mut g) = radio_favs().lock() { *g = favs.clone(); }
    w.set_music_radio_fav_page(0);
    radio_rerender(w);
}

/// Re-render the current list through the active sort (+ direction) and the
/// 20/page pagination on category pages and Favourites. Row `index` stays the
/// position in `radio_list`.
pub fn radio_rerender(w: &MainWindow) {
    let list = radio_list().lock().map(|g| g.clone()).unwrap_or_default();
    let favs = radio_favs().lock().map(|g| g.clone()).unwrap_or_default();
    let icons = radio_icons().lock().map(|g| g.clone()).unwrap_or_default();
    let mut order: Vec<usize> = (0..list.len()).collect();
    // ▲ asc / ▼ desc are literal: A→Z / Z→A for Name, low→high / high→low for
    // Bitrate; for Top, desc = most-voted first (the fetch order).
    let asc = w.get_music_radio_sort_dir().as_str() == "asc";
    match w.get_music_radio_sort().as_str() {
        "name" => { order.sort_by_key(|&i| list[i].name.trim().to_lowercase()); if !asc { order.reverse(); } }
        "bitrate" => { order.sort_by_key(|&i| list[i].bitrate); if !asc { order.reverse(); } }
        _ => { if asc { order.reverse(); } }
    }
    let paged = w.get_music_radio_cat_open()
        || w.get_music_radio_tab().as_str() == "favourites";
    let (pages, page) = if paged {
        let pages = order.len().div_ceil(20).max(1);
        (pages, (w.get_music_radio_fav_page().max(0) as usize).min(pages - 1))
    } else { (1, 0) };
    w.set_music_radio_fav_pages(pages as i32);
    w.set_music_radio_fav_page(page as i32);
    let slice: &[usize] = if paged { &order[page * 20..((page + 1) * 20).min(order.len())] } else { &order };
    let rows: Vec<RadioStation> = slice.iter().map(|&oi| {
        let s = &list[oi];
        let (icon, has_icon) = icons.get(&s.stationuuid)
            .and_then(|p| slint::Image::load_from_path(p).ok())
            .map_or((slint::Image::default(), false), |im| (im, true));
        RadioStation {
            uuid: s.stationuuid.clone().into(),
            name: s.name.trim().into(),
            initial: s.name.trim().chars().next()
                .map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "♪".into()).into(),
            tags: s.tags.split(',').map(str::trim).filter(|t| !t.is_empty())
                .take(3).collect::<Vec<_>>().join(" · ").into(),
            country: s.country.clone().into(),
            quality: radio_quality(&s.codec, s.bitrate).into(),
            icon, has_icon,
            fav: favs.contains(&s.stationuuid),
            index: oi as i32,
        }
    }).collect();
    w.set_music_radio_stations(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Background favicon pass — fills tiles in as each icon lands. Reuses the
/// podcast artwork cache; updates are dropped once a newer fetch supersedes.
pub fn radio_fetch_icons(weak: slint::Weak<MainWindow>, stations: Vec<tulipix_music::radio::Station>, fgen: u64) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).build() else { return; };
        for s in stations.iter() {
            if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
            // Slint has no ICO decoder — skip those favicons.
            if s.favicon.is_empty() || s.favicon.to_lowercase().ends_with(".ico") { continue; }
            let key = format!("radio-{}", s.stationuuid.replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
            let Some(path) = cache_artwork(&client, &key, &s.favicon).await else { continue; };
            // Stations lie about favicon formats (ICO bytes behind a .png URL) —
            // sniff the magic and drop undecodable files instead of letting the
            // image loader error on every render.
            let magic_ok = std::fs::read(&path).map(|b|
                b.starts_with(&[0x89, b'P', b'N', b'G']) || b.starts_with(&[0xFF, 0xD8])
                || b.starts_with(b"GIF8") || (b.len() > 11 && &b[8..12] == b"WEBP")
                || b.starts_with(b"<?xml") || b.starts_with(b"<svg")).unwrap_or(false);
            if !magic_ok { let _ = std::fs::remove_file(&path); continue; }
            if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
            let uuid = s.stationuuid.clone();
            let _ = weak.upgrade_in_event_loop(move |w| {
                // Remember the path (survives sort/page re-renders), then patch
                // the visible row by uuid — the view may be sorted/paged, so
                // model position ≠ fetch position.
                if let Ok(mut g) = radio_icons().lock() { g.insert(uuid.clone(), path.clone()); }
                let model = w.get_music_radio_stations();
                for r in 0..model.row_count() {
                    let Some(mut row) = model.row_data(r) else { continue; };
                    if row.uuid == uuid.as_str() {
                        if let Ok(img) = slint::Image::load_from_path(&path) {
                            row.icon = img; row.has_icon = true;
                            model.set_row_data(r, row);
                        }
                        break;
                    }
                }
            });
        }
    });
}

/// Fetch a radio-browser station list (browse preset or free-text search).
pub fn radio_fetch(weak: slint::Weak<MainWindow>, url: String) {
    let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _ = weak.upgrade_in_event_loop(|w| { w.set_music_radio_busy(true); w.set_music_radio_status("".into()); });
    tokio::runtime::Handle::current().spawn(async move {
        let favs: std::collections::HashSet<String> = match pool_for("radio").await {
            Ok(pool) => tulipix_music::radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect(),
            Err(_) => Default::default(),
        };
        let res = async {
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build()?;
            let list: Vec<tulipix_music::radio::Station> = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                .send().await?.error_for_status()?.json().await?;
            anyhow::Ok(list)
        }.await;
        if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
        match res {
            Ok(mut list) => {
                // radio-browser carries many duplicate registrations of the same
                // stream — keep the top-voted copy (the list arrives votes-desc).
                let mut seen = std::collections::HashSet::new();
                list.retain(|s| seen.insert(s.name.trim().to_lowercase()));
                let icons = list.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |w| {
                    w.set_music_radio_busy(false);
                    radio_render(&w, list, &favs);
                });
                radio_fetch_icons(weak, icons, fgen);
            }
            Err(e) => {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_music_radio_busy(false);
                    radio_render(&w, Vec::new(), &Default::default());
                    w.set_music_radio_status(format!("radio-browser unreachable — {e}").into());
                });
            }
        }
    });
}

/// Fill the grid from radio.db — `which` is "favourites" or "recent".
pub fn radio_show_saved(weak: slint::Weak<MainWindow>, which: &'static str) {
    let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _ = weak.upgrade_in_event_loop(|w| { w.set_music_radio_busy(true); w.set_music_radio_status("".into()); });
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("radio").await else { return; };
        let list = match which {
            "recent" => tulipix_music::radio::recents(&pool, 50).await.unwrap_or_default(),
            _ => tulipix_music::radio::favourites(&pool).await.unwrap_or_default(),
        };
        let favs: std::collections::HashSet<String> =
            tulipix_music::radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect();
        if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
        let icons = list.clone();
        let _ = weak.clone().upgrade_in_event_loop(move |w| {
            w.set_music_radio_busy(false);
            radio_render(&w, list, &favs);
        });
        radio_fetch_icons(weak, icons, fgen);
    });
}

/// Re-populate the open page for whichever radio tab is active. Home shows the
/// category tiles (no list); categories load from the radio.db cache.
pub fn radio_reload_tab(w: &MainWindow) {
    match w.get_music_radio_tab().as_str() {
        "favourites" => radio_show_saved(w.as_weak(), "favourites"),
        "recent" => radio_show_saved(w.as_weak(), "recent"),
        _ => {
            if w.get_music_radio_cat_open() {
                radio_open_category(w, w.get_music_radio_genre().max(0) as usize);
            } else {
                radio_render(w, Vec::new(), &Default::default());
            }
        }
    }
}

/// Refresh the per-category station counts on the Home tiles. When the cache
/// is completely empty (first run) and `auto_refresh` is set, kicks off a full
/// Refresh so the section self-populates on first open.
pub fn radio_load_counts(weak: slint::Weak<MainWindow>, auto_refresh: bool) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("radio").await else { return; };
        let counts = tulipix_music::radio::cache_counts(&pool).await.unwrap_or_default();
        let total: i64 = counts.iter().map(|(_, n)| n).sum();
        let by_preset: std::collections::HashMap<String, i64> = counts.into_iter().collect();
        let row: Vec<i32> = tulipix_music::radio::PRESETS.iter()
            .map(|(_, q)| *by_preset.get(*q).unwrap_or(&0) as i32).collect();
        let _ = weak.clone().upgrade_in_event_loop(move |w| {
            w.set_music_radio_genre_counts(slint::ModelRc::new(slint::VecModel::from(row)));
            // Header pill on Home — every cached station across all categories.
            w.set_music_radio_total(total as i32);
            if total == 0 && auto_refresh && !w.get_music_radio_refresh_busy() {
                radio_refresh_all(&w);
            }
        });
    });
}

/// Open one curated category as a station list — cache-first; falls back to a
/// one-off network fetch (which seeds the cache) when the preset was never
/// refreshed.
pub fn radio_open_category(w: &MainWindow, i: usize) {
    let Some(&(label, q)) = tulipix_music::radio::PRESETS.get(i) else { return; };
    w.set_music_radio_genre(i as i32);
    w.set_music_radio_cat_open(true);
    w.set_music_radio_cat_title(label.into());
    let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    w.set_music_radio_busy(true);
    w.set_music_radio_status("".into());
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("radio").await else { return; };
        let mut list = tulipix_music::radio::load_cache(&pool, q).await.unwrap_or_default();
        if list.is_empty() {
            // Never refreshed — one network fetch seeds this preset's cache.
            if let Ok(fetched) = radio_fetch_preset(q).await {
                let _ = tulipix_music::radio::save_cache(&pool, q, &fetched).await;
                list = fetched;
            }
        }
        let favs: std::collections::HashSet<String> =
            tulipix_music::radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect();
        if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
        let icons = list.clone();
        let empty = list.is_empty();
        let _ = weak.clone().upgrade_in_event_loop(move |w| {
            w.set_music_radio_busy(false);
            if empty { w.set_music_radio_status("Nothing cached for this category — hit ↻ Refresh stations.".into()); }
            radio_render(&w, list, &favs);
        });
        radio_fetch_icons(weak, icons, fgen);
    });
}

/// One preset fetch: votes-ordered, hidebroken, name-deduped.
pub async fn radio_fetch_preset(q: &str) -> Result<Vec<tulipix_music::radio::Station>> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
    let mut list: Vec<tulipix_music::radio::Station> = client
        .get(tulipix_music::radio::browse_url(q, 60))
        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
        .send().await?.error_for_status()?.json().await?;
    let mut seen = std::collections::HashSet::new();
    list.retain(|s| seen.insert(s.name.trim().to_lowercase()));
    Ok(list)
}

/// "Refresh stations" — re-fetch EVERY curated category from radio-browser in
/// parallel, replacing the whole cache. The progress pill fills per preset.
pub fn radio_refresh_all(w: &MainWindow) {
    if w.get_music_radio_refresh_busy() { return; }
    w.set_music_radio_refresh_busy(true);
    w.set_music_radio_refresh_frac(0.0);
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let total = tulipix_music::radio::PRESETS.len();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(total);
        for &(_, q) in tulipix_music::radio::PRESETS {
            let done = done.clone();
            let weak = weak.clone();
            handles.push(tokio::spawn(async move {
                if let Ok(list) = radio_fetch_preset(q).await {
                    if let Ok(pool) = pool_for("radio").await {
                        let _ = tulipix_music::radio::save_cache(&pool, q, &list).await;
                    }
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let frac = n as f32 / total as f32;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_radio_refresh_frac(frac));
            }));
        }
        for h in handles { let _ = h.await; }
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_music_radio_refresh_busy(false);
            w.set_music_radio_refresh_frac(0.0);
            radio_load_counts(w.as_weak(), false);
            // An open category page re-reads its freshly replaced cache.
            if w.get_music_radio_cat_open() { radio_reload_tab(&w); }
        });
    });
}

/// Play a live radio stream — same headless-mpv path as [`play_music_url`] but
/// observing `media-title`: Shoutcast/Icecast ICY metadata carries
/// "Artist - Song" for most stations and becomes the now-playing title live.
pub fn play_radio(w: &MainWindow, st: &tulipix_music::radio::Station) {
    if let Ok(mut g) = yt_cur_audio().lock() { g.clear(); }
    w.set_music_yt_now_video(false);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
    let mut pre_args = vec![format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32)];
    if w.get_music_muted() { pre_args.push("--mute=yes".into()); }
    pre_args.push(format!("--af={}", music_full_af(&music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10])))));
    pre_args.extend(music_device_args());
    // Live cushion: buffer ~10s before starting so transient network dips eat
    // the cache instead of stuttering (we run ~10s behind the live edge).
    pre_args.extend([
        "--cache=yes".into(), "--cache-secs=30".into(),
        "--cache-pause-initial=yes".into(), "--cache-pause-wait=10".into(),
        "--demuxer-readahead-secs=30".into(),
        // Most Icecast/Shoutcast servers and the CDNs in front of them answer
        // mpv's default agent with 403, or drop the connection during the TLS
        // handshake. Ask as a browser — see `net::BROWSER_UA`.
        format!("--user-agent={}", tulipix_core::net::BROWSER_UA),
    ]);
    let station_name = st.name.trim().to_string();
    let stream_url = st.url.clone();
    let weak = w.as_weak();
    let on_prop = move |name: &str, data: &serde_json::Value| {
        let name = name.to_string();
        let data = data.clone();
        let wk = weak.clone();
        let su = stream_url.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = wk.upgrade() else { return; };
            match name.as_str() {
                // Live stream: no duration — the position label shows time on air.
                "time-pos" => if let Some(d) = data.as_f64() { w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into()); }
                "pause" => if let Some(p) = data.as_bool() { w.set_music_playing(!p); }
                "media-title" => if let Some(t) = data.as_str() {
                    let t = t.trim();
                    // The title stays the STATION name (from the channel list) so it's
                    // static across the bottom + zen players. The ICY now-playing track
                    // (when present) rides the second line instead. mpv reports the URL
                    // until the first ICY update — ignore those.
                    if !t.is_empty() && t != su && !t.starts_with("http") {
                        w.set_music_np_sub(t.into());
                        // Also surface it inside the Radio tab itself, and keep
                        // a short log of what the station has played. Without
                        // this the answer to "what was that song" lives only on
                        // the player bar and is gone the moment it changes.
                        w.set_music_radio_np_track(t.into());
                        let mut log: Vec<slint::SharedString> =
                            w.get_music_radio_track_log().iter().collect();
                        if log.first().map(|f| f.as_str()) != Some(t) {
                            log.insert(0, t.into());
                            log.truncate(12);
                            w.set_music_radio_track_log(
                                slint::ModelRc::new(slint::VecModel::from(log)));
                        }
                    }
                }
                _ => {}
            }
        });
    };
    let eof_weak = w.as_weak();
    let on_eof = move || { let _ = eof_weak.upgrade_in_event_loop(|w| {
        w.set_music_playing(false);
        w.set_music_radio_np_uuid("".into());
        w.set_music_radio_np_track("".into());
        // A recording belongs to the stream that was playing; the new one has
        // to be started deliberately, not inherited.
        w.set_music_radio_recording(false);
        w.set_music_radio_rec_file("".into());
    }); };
    if let Err(e) = player::spawn_audio(player::AudioLaunch {
        prefix: "tulipix-music",
        mpv_bin: tulipix_core::thumbs::tool_bin("mpv"),
        src: std::path::Path::new(&st.url),
        pre_args,
        observe: &[(1, "time-pos"), (3, "pause"), (5, "media-title")],
        generation: my_gen,
    }, on_prop, on_eof) {
        tracing::error!(error = %e, "mpv radio launch failed"); return;
    }
    w.set_music_player_mode("radio".into());
    w.set_music_radio_np_uuid(st.stationuuid.clone().into());
    w.set_music_radio_np_initial(station_name.chars().next()
        .map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "♪".into()).into());
    w.set_music_np_title(station_name.into());
    w.set_music_np_sub("📻 Internet Radio · LIVE".into());
    w.set_music_np_album("".into());      // no stale artist·album on the second line
    // No favicon = a real generated tile, not an empty frame. The caller
    // overwrites this the moment a station icon exists.
    w.set_music_np_art(radio_placeholder_art(w.get_music_radio_np_initial().as_str()));
    w.set_music_np_accent(slint::Color::from_rgb_u8(0x14, 0xb8, 0xa6));
    clear_music_lyrics(w);
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("LIVE".into());
    w.invoke_music_center_mini();
}

/// The station-initial tile, baked as an image.
///
/// The now-playing panel and the floating bubble already draw this by hand when
/// the art is empty, but the mini widget, the bottom bars, the home layouts and
/// the lock screen do not — and a station without a favicon is common. Baking
/// it into `music-np-art` gives every one of them the tile for free, and keeps
/// the two hand-drawn ones looking exactly the same as before.
fn radio_placeholder_art(initial: &str) -> slint::Image {
    use ab_glyph::{Font, ScaleFont};
    const FONT_SORA: &[u8] = include_bytes!("../../../resources/fonts/Sora[wght].ttf");
    const S: u32 = 256;
    // #14b8a6 → #0ea5e9 on the 135° diagonal, the same wash as the UI tiles.
    const A: [f32; 3] = [0x14 as f32, 0xb8 as f32, 0xa6 as f32];
    const B: [f32; 3] = [0x0e as f32, 0xa5 as f32, 0xe9 as f32];
    let mut img = image::RgbaImage::new(S, S);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let t = (x + y) as f32 / (2 * (S - 1)) as f32;
        *p = image::Rgba([
            (A[0] + (B[0] - A[0]) * t) as u8,
            (A[1] + (B[1] - A[1]) * t) as u8,
            (A[2] + (B[2] - A[2]) * t) as u8,
            255,
        ]);
    }
    // One glyph, centred on its own ink box rather than on the font metrics —
    // a letter centred by baseline sits visibly high in a square tile.
    let ch = initial.chars().next().unwrap_or('R');
    if let Ok(font) = ab_glyph::FontRef::try_from_slice(FONT_SORA) {
        let sf = font.as_scaled(132.0);
        let mut g = sf.scaled_glyph(ch);
        g.position = ab_glyph::point(0.0, 0.0);
        if let Some(outline) = font.outline_glyph(g) {
            let bb = outline.px_bounds();
            let ox = (S as f32 - (bb.max.x - bb.min.x)) / 2.0 - bb.min.x;
            let oy = (S as f32 - (bb.max.y - bb.min.y)) / 2.0 - bb.min.y;
            outline.draw(|gx, gy, c| {
                let x = bb.min.x + ox + gx as f32;
                let y = bb.min.y + oy + gy as f32;
                if x < 0.0 || y < 0.0 || x >= S as f32 || y >= S as f32 {
                    return;
                }
                let p = img.get_pixel_mut(x as u32, y as u32);
                let a = c * 0.82;
                for i in 0..3 {
                    p[i] = (255.0 * a + p[i] as f32 * (1.0 - a)) as u8;
                }
            });
        }
    }
    slint::Image::from_rgba8(slint::SharedPixelBuffer::clone_from_slice(img.as_raw(), S, S))
}

/// Register every radio callback + the curated genre chips.
pub fn wire_radio(window: &MainWindow) {
    use tulipix_music::radio;
    // Tile labels split into emoji + name — the Home cards show the icon big
    // above the name (the full label stays the category page title).
    let icons: Vec<slint::SharedString> = radio::PRESETS.iter()
        .map(|(l, _)| l.split_whitespace().next().unwrap_or("📻").into()).collect();
    let names: Vec<slint::SharedString> = radio::PRESETS.iter()
        .map(|(l, _)| l.split_once(' ').map_or(*l, |(_, n)| n).trim().into()).collect();
    window.set_music_radio_genres(slint::ModelRc::new(slint::VecModel::from(names)));
    window.set_music_radio_genre_icons(slint::ModelRc::new(slint::VecModel::from(icons)));

    let w = window.as_weak();
    window.on_music_radio_set_tab(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_radio_cat_open(false); // tabs always leave any open list page
        w0.set_music_radio_tab(t);
        radio_reload_tab(&w0);
        if w0.get_music_radio_tab().as_str() == "home" { radio_load_counts(w.clone(), false); }
    });
    let w = window.as_weak();
    window.on_music_radio_set_genre(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        radio_open_category(&w0, i.max(0) as usize);
    });
    let w = window.as_weak();
    window.on_music_radio_back(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_radio_cat_open(false);
        w0.set_music_radio_tab("home".into());
        radio_render(&w0, Vec::new(), &Default::default());
        radio_load_counts(w.clone(), false);
    });
    let w = window.as_weak();
    window.on_music_radio_refresh(move || {
        let Some(w0) = w.upgrade() else { return; };
        radio_refresh_all(&w0);
    });
    let w = window.as_weak();
    window.on_music_radio_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        // Re-tapping the active sort flips its direction; switching sorts
        // resets to that sort's natural face (Name A→Z, Bitrate/Top high first).
        if w0.get_music_radio_sort() == s {
            let flipped = if w0.get_music_radio_sort_dir().as_str() == "asc" { "desc" } else { "asc" };
            w0.set_music_radio_sort_dir(flipped.into());
        } else {
            w0.set_music_radio_sort_dir(if s.as_str() == "name" { "asc" } else { "desc" }.into());
            w0.set_music_radio_sort(s);
        }
        w0.set_music_radio_fav_page(0);
        radio_rerender(&w0);
    });
    let w = window.as_weak();
    window.on_music_radio_fav_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_radio_fav_page() + d).clamp(0, (w0.get_music_radio_fav_pages() - 1).max(0));
        w0.set_music_radio_fav_page(next);
        radio_rerender(&w0);
    });
    let w = window.as_weak();
    window.on_music_radio_clear_recent(move || {
        if w.upgrade().is_none() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("radio").await {
                let _ = tulipix_music::radio::clear_recents(&pool).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                if w.get_music_radio_tab().as_str() == "recent" { radio_reload_tab(&w); }
            });
        });
    });
    let w = window.as_weak();
    window.on_music_radio_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        let q = q.trim().to_string();
        if q.is_empty() {
            w0.set_music_radio_cat_open(false);
            radio_reload_tab(&w0);
        } else if q.len() >= 2 {
            // Universal search — every cached category + saved stations in one
            // pass (offline); empty cache results fall back to radio-browser.
            w0.set_music_radio_tab("home".into());
            w0.set_music_radio_cat_open(true);
            w0.set_music_radio_cat_title(format!("🔍 “{q}” — all stations").into());
            let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            w0.set_music_radio_busy(true);
            w0.set_music_radio_status("".into());
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("radio").await else { return; };
                let list = radio::search_cache(&pool, &q).await.unwrap_or_default();
                if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
                if list.is_empty() {
                    // Nothing cached matches — go to the network.
                    let _ = weak.clone().upgrade_in_event_loop(move |w| {
                        radio_fetch(w.as_weak(), radio::search_url(&q, 60));
                    });
                    return;
                }
                let favs: std::collections::HashSet<String> =
                    radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect();
                let icons = list.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |w| {
                    w.set_music_radio_busy(false);
                    radio_render(&w, list, &favs);
                });
                radio_fetch_icons(weak, icons, fgen);
            });
        }
    });
    let w = window.as_weak();
    window.on_music_radio_play(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let idx = i.max(0) as usize;
        let Some(st) = radio_list().lock().ok().and_then(|g| g.get(idx).cloned()) else { return; };
        play_radio(&w0, &st);
        // The grid favicon doubles as now-playing art.
        if let Some(row) = w0.get_music_radio_stations().row_data(idx) {
            if row.has_icon { w0.set_music_np_art(row.icon); }
        }
        tokio::runtime::Handle::current().spawn(async move {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            if let Ok(pool) = pool_for("radio").await {
                let _ = radio::touch_played(&pool, &st, now).await;
            }
            // Popularity click-ping (radio-browser etiquette) — fire and forget.
            if !st.stationuuid.starts_with("custom:") {
                if let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).build() {
                    let _ = client.post(radio::click_url(&st.stationuuid))
                        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                        .send().await;
                }
            }
        });
    });
    // Record the live stream to disk while it keeps playing. mpv's
    // `stream-record` is settable over IPC, so this toggles mid-listen instead
    // of needing a relaunch — which for a live stream would mean losing the
    // buffer and rejoining a few seconds later.
    let w = window.as_weak();
    window.on_music_radio_record(move || {
        let Some(w0) = w.upgrade() else { return; };
        if w0.get_music_radio_np_uuid().is_empty() { return; }
        if w0.get_music_radio_recording() {
            // Empty string is mpv's "stop recording".
            tulipix_common::music_ipc(&["set_property", "stream-record", ""]);
            w0.set_music_radio_recording(false);
            return;
        }
        let dir = tulipix_core::paths::data_dir()
            .unwrap_or_else(std::env::temp_dir).join("Recordings");
        if std::fs::create_dir_all(&dir).is_err() { return; }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M").to_string();
        let name = radio::record_filename(w0.get_music_np_title().as_str(), &stamp);
        let out = dir.join(&name);
        tulipix_common::music_ipc(&["set_property", "stream-record", &out.to_string_lossy()]);
        w0.set_music_radio_recording(true);
        w0.set_music_radio_rec_file(name.into());
    });
    // "Find track" on an ICY title. The library is the honest first answer —
    // if you already own it, nothing needs downloading — so this drops the
    // title into My Music's search rather than straight into YouTube.
    let w = window.as_weak();
    window.on_music_radio_track_search(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        let q = t.trim().to_string();
        if q.is_empty() { return; }
        w0.set_music_view("mymusic".into());
        w0.set_music_lib_tab("songs".into());
        w0.set_music_query(q.clone().into());
        w0.invoke_music_search(q.into());
    });
    let w = window.as_weak();
    window.on_music_radio_fav(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let idx = i.max(0) as usize;
        let Some(st) = radio_list().lock().ok().and_then(|g| g.get(idx).cloned()) else { return; };
        // `i` indexes radio_list, NOT the (possibly sorted/paged) model — flip
        // the heart in the favs set and re-render so the right row updates.
        let now_fav = radio_favs().lock().map(|mut g| {
            if g.contains(&st.stationuuid) { g.remove(&st.stationuuid); false }
            else { g.insert(st.stationuuid.clone()); true }
        }).unwrap_or(false);
        radio_rerender(&w0);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("radio").await else { return; };
            let _ = if now_fav { radio::add_favourite(&pool, &st).await }
                    else { radio::remove_favourite(&pool, &st.stationuuid).await };
            // Unhearting while ON the favourites tab removes the card.
            let _ = weak.upgrade_in_event_loop(|w| {
                if w.get_music_radio_tab().as_str() == "favourites" { radio_reload_tab(&w); }
            });
        });
    });
    // Home Quick Action — play a random Bollywood/Punjabi station in the home
    // player. Coin-flips between the two curated presets, cache-first; a cold
    // cache goes to radio-browser once and saves the list so the next click is
    // instant. If the flipped preset comes up empty, the other one is tried.
    let w = window.as_weak();
    window.on_home_random_radio(move || {
        if w.upgrade().is_none() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            const SOURCES: [&str; 2] = ["tag=bollywood", "language=punjabi"];
            // Clock-nanos randomness — good enough for "surprise me", no rand dep.
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as usize)
                .unwrap_or(0);
            let mut list = Vec::new();
            for k in 0..SOURCES.len() {
                let q = SOURCES[(nanos + k) % SOURCES.len()];
                list = match pool_for("radio").await {
                    Ok(pool) => radio::load_cache(&pool, q).await.unwrap_or_default(),
                    Err(_) => Vec::new(),
                };
                if list.is_empty() {
                    list = radio_fetch_preset(q).await.unwrap_or_default();
                    if !list.is_empty() {
                        if let Ok(pool) = pool_for("radio").await {
                            let _ = radio::save_cache(&pool, q, &list).await;
                        }
                    }
                }
                if !list.is_empty() { break; }
            }
            if list.is_empty() { return; }
            let pick = (nanos / 7) % list.len();
            let st = list[pick].clone();
            let st2 = st.clone();
            let _ = weak.upgrade_in_event_loop(move |w| play_radio(&w, &st2));
            // Recents + popularity click-ping — same etiquette as a grid click.
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            if let Ok(pool) = pool_for("radio").await {
                let _ = radio::touch_played(&pool, &st, now).await;
            }
            if !st.stationuuid.starts_with("custom:") {
                if let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).build() {
                    let _ = client.post(radio::click_url(&st.stationuuid))
                        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                        .send().await;
                }
            }
        });
    });
    let w = window.as_weak();
    window.on_music_radio_add_save(move || {
        let Some(w0) = w.upgrade() else { return; };
        let name = w0.get_music_radio_add_name().trim().to_string();
        let url = w0.get_music_radio_add_url().trim().to_string();
        if url.is_empty() { return; }
        let st = radio::custom_station(if name.is_empty() { &url } else { &name }, &url);
        w0.set_music_radio_add_open(false);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("radio").await {
                let _ = radio::add_favourite(&pool, &st).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_music_radio_tab("favourites".into());
                radio_reload_tab(&w);
            });
        });
    });
}

/// Persist one `music.*` preference into Settings.advanced.
pub fn save_music_pref(key: &str, val: &str) {
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert(key.into(), val.into());
    let _ = s.save();
}

/// Read one `music.*` preference from Settings.advanced.
pub fn load_music_pref(key: &str) -> Option<String> {
    tulipix_core::settings::Settings::load().ok().and_then(|s| s.advanced.get(key).cloned())
}

/// The item id the tag editor targets — explicit context-menu target, else the
/// now-playing track.
pub fn editing_target(w: &MainWindow) -> Option<i64> {
    tag_edit_target().lock().ok().and_then(|g| *g).or_else(|| current_music_id(w))
}

/// Fill the tag-editor dropdown + stored fields (release date / genre / album
/// artist / credits / lock) for an item, async (np.p5.music.tag-editor).
pub fn prefill_tag_editor(weak: &slint::Weak<MainWindow>, id: i64) {
    let weak = weak.clone();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(&pool).await;
        let row: Option<(Option<String>, Option<String>, Option<String>, Option<String>, i64)> = sqlx::query_as(
            "SELECT release_date, genre, album_artist, credits, COALESCE(user_locked,0) FROM track_meta WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        let nums: Option<(Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT track_no, disc_no FROM track_meta WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        let opts: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT genre FROM track_meta WHERE genre IS NOT NULL AND genre != '' ORDER BY genre")
            .fetch_all(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Some((d, g, aa, cr, lk)) = row {
                if let Some(d) = d { w.set_music_tag_date(d.into()); }
                if let Some(g) = g { w.set_music_tag_genre(g.into()); }
                if let Some(aa) = aa { w.set_music_tag_album_artist(aa.into()); }
                if let Some(cr) = cr { w.set_music_tag_credits(cr.into()); }
                w.set_music_tag_locked(lk != 0);
            }
            if let Some((tn, dn)) = nums {
                w.set_music_tag_track(tn.map(|n| n.to_string()).unwrap_or_default().into());
                w.set_music_tag_disc(dn.map(|n| n.to_string()).unwrap_or_default().into());
            }
            let opts: Vec<slint::SharedString> = opts.into_iter().map(|s| s.into()).collect();
            w.set_music_genre_options(slint::ModelRc::new(slint::VecModel::from(opts)));
        });
    });
}

/// Recompute the highlighted lyric line for the current playhead + offset.
pub fn update_lyrics_active(w: &MainWindow) {
    let t_ms = (w.get_music_pos() as f64 * 1000.0) as i64 - w.get_music_lyrics_offset_ms() as i64;
    let (active, prog) = music_lyrics_lines().lock().ok().map(|g| {
        let a = tulipix_music::lyrics::active_line(&g, t_ms);
        let p = a.map(|i| {
            let start = g[i].0;
            let end = g.get(i + 1).map(|l| l.0).unwrap_or(start + 4000);
            (((t_ms - start) as f64) / ((end - start).max(1) as f64)).clamp(0.0, 1.0) as f32
        }).unwrap_or(0.0);
        (a.map(|i| i as i32).unwrap_or(-1), p)
    }).unwrap_or((-1, 0.0));
    if w.get_music_lyrics_active() != active { w.set_music_lyrics_active(active); }
    w.set_music_lyrics_line_progress(prog); // karaoke wipe within the current line
}

/// mpv `--key=value` args for the persisted audio config (device / exclusive /
/// gapless / replaygain). Applied at launch in `play_music_at`.
/// Output-device flags (device + exclusive) shared by every audio spawn path —
/// streams (radio / podcasts / YT audio) honour the chosen output device too,
/// not just library playback.
pub fn music_device_args() -> Vec<String> {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    let device = s.advanced.get("music.device").cloned().unwrap_or_else(|| "auto".into());
    let exclusive = s.advanced.get("music.exclusive").map(|v| v == "1").unwrap_or(false);
    tulipix_music::output_device::device_options(&device, exclusive)
}

pub fn music_audio_args(s: &tulipix_core::settings::Settings) -> Vec<String> {
    use tulipix_music::player::{AudioConfig, ReplayGainMode};
    let cfg = AudioConfig {
        gapless: s.advanced.get("music.gapless").map(|v| v != "0").unwrap_or(true),
        crossfade_s: s.advanced.get("music.crossfade").and_then(|v| v.parse().ok()).unwrap_or(0.0),
        replaygain: match s.advanced.get("music.replaygain").map(|v| v.as_str()) {
            Some("track") => ReplayGainMode::Track,
            Some("album") => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        },
        preamp_db: s.advanced.get("music.preamp").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).clamp(-12.0, 12.0),
    };
    let mut args = cfg.mpv_options();
    let device = s.advanced.get("music.device").cloned().unwrap_or_else(|| "auto".into());
    let exclusive = s.advanced.get("music.exclusive").map(|v| v == "1").unwrap_or(false);
    args.extend(tulipix_music::output_device::device_options(&device, exclusive));
    args
}

/// Enumerate audio output devices via `mpv --audio-device=help` (np.p5.music.output).
pub fn enumerate_audio_devices() -> Vec<tulipix_music::output_device::AudioDevice> {
    let out = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv")).arg("--audio-device=help").no_window().output();
    match out {
        Ok(o) => {
            let txt = String::from_utf8_lossy(&o.stdout);
            let mut d = tulipix_music::output_device::parse_device_list(&txt);
            if d.is_empty() { d.push(tulipix_music::output_device::default_device()); }
            d
        }
        Err(_) => vec![tulipix_music::output_device::default_device()],
    }
}

/// SSDP M-SEARCH for UPnP MediaRenderers; collects responses for ~2 s
/// (np.p5.music.cast). Runs on a worker thread (blocking socket).
/// One-track HTTP server for the cast handoff: serves exactly `path` on an
/// ephemeral port so the renderer can pull the bytes. Each cast replaces the
/// served file; the listener thread lives for the app's lifetime. Returns the
/// URL the renderer should fetch. Range requests supported (renderer seeks).
static CAST_SERVE: std::sync::OnceLock<std::sync::Mutex<Option<(u16, std::path::PathBuf)>>> = std::sync::OnceLock::new();
pub fn cast_serve_url(path: &std::path::Path) -> Option<String> {
    let state = CAST_SERVE.get_or_init(|| std::sync::Mutex::new(None));
    let mut g = state.lock().ok()?;
    let port = match &*g {
        Some((port, _)) => *port,
        None => {
            let listener = std::net::TcpListener::bind("0.0.0.0:0").ok()?;
            let port = listener.local_addr().ok()?.port();
            std::thread::Builder::new().name("tulipix-cast-http".into()).spawn(move || {
                for stream in listener.incoming().flatten() {
                    let served = CAST_SERVE.get().and_then(|s| s.lock().ok().and_then(|g| g.clone()));
                    let Some((_, path)) = served else { continue };
                    let mut stream = stream;
                    std::thread::spawn(move || {
                        use std::io::{Read, Write};
                        let mut req = [0u8; 1024];
                        let n = stream.read(&mut req).unwrap_or(0);
                        let head = String::from_utf8_lossy(&req[..n]);
                        let head_only = head.starts_with("HEAD");
                        let Ok(bytes) = std::fs::read(&path) else {
                            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                            return;
                        };
                        let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                            "mp3" => "audio/mpeg", "flac" => "audio/flac", "ogg" | "oga" => "audio/ogg",
                            "m4a" | "aac" => "audio/mp4", "wav" => "audio/wav", "opus" => "audio/opus",
                            _ => "application/octet-stream",
                        };
                        // HTTP Range (np.b2.music.cast-v2): renderers seek by
                        // re-requesting `bytes=start-[end]`; suffix form
                        // `bytes=-N` asks for the trailing N bytes.
                        let total = bytes.len() as u64;
                        let range = head.lines().find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            if !k.trim().eq_ignore_ascii_case("range") { return None; }
                            let (a, b) = v.trim().strip_prefix("bytes=")?.split_once('-')?;
                            match (a.trim(), b.trim()) {
                                ("", suf) => {
                                    let n: u64 = suf.parse().ok()?;
                                    Some((total.saturating_sub(n), total.saturating_sub(1)))
                                }
                                (st, "") => Some((st.parse().ok()?, total.saturating_sub(1))),
                                (st, en) => Some((st.parse().ok()?, en.parse().ok()?)),
                            }
                        });
                        match range {
                            Some((start, end)) if start < total => {
                                let end = end.min(total - 1);
                                let hdr = format!(
                                    "HTTP/1.1 206 Partial Content\r\nContent-Type: {mime}\r\n\
                                     Accept-Ranges: bytes\r\nContent-Range: bytes {start}-{end}/{total}\r\n\
                                     Content-Length: {}\r\nConnection: close\r\n\r\n",
                                    end - start + 1);
                                let _ = stream.write_all(hdr.as_bytes());
                                if !head_only {
                                    let _ = stream.write_all(&bytes[start as usize..=end as usize]);
                                }
                            }
                            Some(_) => {
                                let _ = stream.write_all(format!(
                                    "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{total}\r\n\
                                     Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes());
                            }
                            None => {
                                let hdr = format!(
                                    "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nAccept-Ranges: bytes\r\n\
                                     Content-Length: {total}\r\nConnection: close\r\n\r\n");
                                let _ = stream.write_all(hdr.as_bytes());
                                if !head_only { let _ = stream.write_all(&bytes); }
                            }
                        }
                    });
                }
            }).ok()?;
            port
        }
    };
    *g = Some((port, path.to_path_buf()));
    // LAN-reachable local address: route-probe via UDP connect (no packets sent).
    let ip = std::net::UdpSocket::bind("0.0.0.0:0").ok()
        .and_then(|s| s.connect("8.8.8.8:80").ok().and_then(|_| s.local_addr().ok()))
        .map(|a| a.ip().to_string())?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("track");
    let enc: String = name.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect();
    Some(format!("http://{ip}:{port}/{enc}"))
}

/// The actual cast handoff (np.p5.music.cast): serve the current track over
/// HTTP, then SOAP SetAVTransportURI + Play at the renderer's AVTransport
/// control URL. Local mpv stops — the renderer owns playback.
pub fn cast_current_track(w: &MainWindow, device_name: &str) {
    let idx = w.get_music_np_index();
    let Some(path) = music_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else {
        w.set_music_cast_status("Play a track first, then pick a renderer.".into());
        return;
    };
    let Some(dev) = cast_targets().lock().ok()
        .and_then(|g| g.iter().find(|d| d.name == device_name).cloned()) else { return; };
    let Some(media_url) = cast_serve_url(std::path::Path::new(&path)) else {
        w.set_music_cast_status("Could not start the local stream server.".into());
        return;
    };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let client = tulipix_core::net::http().clone();
        let desc = match client.get(&dev.location).send().await {
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(e) => { tracing::warn!(error = %e, "cast: description fetch failed"); String::new() }
        };
        let Some(ctl) = tulipix_music::cast::parse_control_url(&desc) else {
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_cast_status("Renderer has no AVTransport service.".into()));
            return;
        };
        let ctl = tulipix_music::cast::resolve_url(&dev.location, &ctl);
        for (action, body) in [
            ("SetAVTransportURI", tulipix_music::cast::soap_set_uri(0, &media_url)),
            ("Play", tulipix_music::cast::soap_play(0)),
        ] {
            let ok = client.post(&ctl)
                .header("SOAPACTION", tulipix_music::cast::soap_action_header(action))
                .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
                .body(body).send().await
                .map(|r| r.status().is_success()).unwrap_or(false);
            if !ok {
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_cast_status(format!("Renderer refused {action}.").into()));
                return;
            }
        }
        // Remember the control URL so Pause/Stop chips can drive the session
        // (np.b2.music.cast-v2).
        if let Ok(mut g) = cast_session().lock() { *g = Some(ctl.clone()); }
        let _ = weak.upgrade_in_event_loop(move |w| {
            // Renderer owns playback now — stop the local pipeline.
            stop_music(&w);
            w.set_music_cast_active(true);
            w.set_music_cast_paused(false);
            w.set_music_cast_status(format!("Casting to {}.", dev.name).into());
        });
    });
}

/// Active cast session's AVTransport control URL (np.b2.music.cast-v2).
static CAST_SESSION: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
pub fn cast_session() -> &'static std::sync::Mutex<Option<String>> {
    CAST_SESSION.get_or_init(|| std::sync::Mutex::new(None))
}

/// One SOAP action at the stored control URL; `on_done(ok)` hops back to the
/// event loop.
fn cast_soap(w: &MainWindow, action: &'static str, body: String, on_done: impl Fn(&MainWindow, bool) + Send + 'static) {
    let Some(ctl) = cast_session().lock().ok().and_then(|g| g.clone()) else { return; };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let ok = tulipix_core::net::http().post(&ctl)
            .header("SOAPACTION", tulipix_music::cast::soap_action_header(action))
            .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
            .body(body).send().await
            .map(|r| r.status().is_success()).unwrap_or(false);
        let _ = weak.upgrade_in_event_loop(move |w| on_done(&w, ok));
    });
}

/// Pause ⇄ resume the renderer (np.b2.music.cast-v2).
pub fn cast_pause_toggle(w: &MainWindow) {
    let pause = !w.get_music_cast_paused();
    let (action, body) = if pause {
        ("Pause", tulipix_music::cast::soap_pause(0))
    } else {
        ("Play", tulipix_music::cast::soap_play(0))
    };
    cast_soap(w, action, body, move |w, ok| {
        if ok { w.set_music_cast_paused(pause); }
        else { w.set_music_cast_status(format!("Renderer refused {action}.").into()); }
    });
}

/// Stop the renderer and end the cast session (np.b2.music.cast-v2).
pub fn cast_stop(w: &MainWindow) {
    cast_soap(w, "Stop", tulipix_music::cast::soap_stop(0), |w, _| {
        // Even a refused Stop ends the session locally — the renderer keeps
        // its own state; we just stop steering it.
        if let Ok(mut g) = cast_session().lock() { *g = None; }
        w.set_music_cast_active(false);
        w.set_music_cast_paused(false);
        w.set_music_cast_target("".into());
        w.set_music_cast_status("Cast stopped.".into());
    });
}

pub fn discover_cast_devices() -> Vec<tulipix_music::cast::CastDevice> {
    use std::net::UdpSocket;
    let mut out: Vec<tulipix_music::cast::CastDevice> = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return out; };
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(600)));
    let msg = tulipix_music::cast::ssdp_msearch();
    let _ = sock.send_to(msg.as_bytes(), "239.255.255.250:1900");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                let raw = String::from_utf8_lossy(&buf[..n]);
                if let Some(dev) = tulipix_music::cast::parse_ssdp_response(&raw) {
                    if !out.iter().any(|d| d.location == dev.location) { out.push(dev); }
                }
            }
            Err(_) => continue,
        }
    }
    out
}

/// Stop playback immediately (kill mpv, suppress auto-advance). Used by the
/// cast handoff (renderer owns playback) and the lyrics-only "check" path.
pub fn stop_music(w: &MainWindow) {
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
    w.set_music_playing(false);
}

/// The track id currently shown in the read-only lyrics-view popup.
static MUSIC_VIEW_LYRICS_ID: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
pub fn music_view_lyrics_id() -> &'static std::sync::Mutex<Option<i64>> {
    MUSIC_VIEW_LYRICS_ID.get_or_init(|| std::sync::Mutex::new(None))
}
/// Target track for the tag editor when opened from a song's context menu
/// (Edit media info). `None` ⇒ the editor targets the now-playing track.
static TAG_EDIT_TARGET: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
pub fn tag_edit_target() -> &'static std::sync::Mutex<Option<i64>> {
    TAG_EDIT_TARGET.get_or_init(|| std::sync::Mutex::new(None))
}
/// Previewed-but-unsaved lyrics from a popup re-search: (synced, content).
static PENDING_VIEW_LYRICS: std::sync::OnceLock<std::sync::Mutex<Option<(bool, String)>>> = std::sync::OnceLock::new();
pub fn pending_view_lyrics() -> &'static std::sync::Mutex<Option<(bool, String)>> {
    PENDING_VIEW_LYRICS.get_or_init(|| std::sync::Mutex::new(None))
}
/// Last.fm request token awaiting browser approval (np.p5.music.scrobble).
static LASTFM_PENDING_TOKEN: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
pub fn lastfm_pending_token() -> &'static std::sync::Mutex<Option<String>> {
    LASTFM_PENDING_TOKEN.get_or_init(|| std::sync::Mutex::new(None))
}

/// Populate the lyrics-view popup for a specific track without touching playback.
/// Loads stored lyrics (DB) or fetches from LRCLIB, then fills the viewer rows +
/// prefills the re-search fields with the track's name/artist/album.
pub fn view_lyrics_load(w: &MainWindow, id: i64) {
    w.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::<MusicLyricLine>::default()));
    w.set_music_lyrics_view_plain("Loading…".into());
    w.set_music_lyrics_view_title("".into());
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let sig: Option<(Option<String>, Option<String>, Option<String>, f64)> = sqlx::query_as(
            "SELECT tm.title, ar.name, al.title, COALESCE(tm.duration_s, 0)
             FROM track_meta tm
             LEFT JOIN artists ar ON ar.id = tm.artist_id
             LEFT JOIN albums  al ON al.id = tm.album_id
             WHERE tm.item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        let (title, artist, album, dur) = match sig {
            Some((t, ar, al, d)) => (t.unwrap_or_default(), ar.unwrap_or_default(), al.unwrap_or_default(), d),
            None => (String::new(), String::new(), String::new(), 0.0),
        };
        let mut row: Option<(i64, String)> = sqlx::query_as(
            "SELECT synced, content FROM lyrics WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        if row.as_ref().map(|(_, c)| c.is_empty()).unwrap_or(true) && !title.is_empty() {
            let url = tulipix_music::lyrics::get_url(&artist, &title, &album, dur);
            let client = tulipix_core::net::http().clone();
            if let Ok(resp) = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                .send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    let synced = json["syncedLyrics"].as_str().unwrap_or("");
                    let plain = json["plainLyrics"].as_str().unwrap_or("");
                    if !synced.is_empty() { let _ = tulipix_music::lyrics::store(&pool, id, synced, true, "lrclib").await; row = Some((1, synced.to_string())); }
                    else if !plain.is_empty() { let _ = tulipix_music::lyrics::store(&pool, id, plain, false, "lrclib").await; row = Some((0, plain.to_string())); }
                }
            }
        }
        let (synced, content) = row.unwrap_or((0, String::new()));
        let lines = if synced != 0 { tulipix_music::lyrics::parse_lrc(&content) } else { Vec::new() };
        let status = if synced != 0 { "Synced" } else if content.is_empty() { "Not Found" } else { "Normal" };
        let sub = if album.is_empty() { artist.clone() } else if artist.is_empty() { album.clone() } else { format!("{artist} · {album}") };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_lyrics_view_mode("lyrics".into());
            w.set_music_lyrics_view_results(slint::ModelRc::new(slint::VecModel::<LyricsResult>::default()));
            w.set_music_lyrics_view_title(title.clone().into());
            w.set_music_lyrics_view_sub(sub.into());
            w.set_music_lyrics_view_status(status.into());
            w.set_music_lyrics_view_q_name(title.into());
            w.set_music_lyrics_view_q_artist(artist.into());
            w.set_music_lyrics_view_q_album(album.into());
            let rows: Vec<MusicLyricLine> = lines.iter().map(|(ms, t)| MusicLyricLine {
                time: fmt_clock(*ms as f64 / 1000.0).into(), text: t.clone().into() }).collect();
            w.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_music_lyrics_view_plain(if synced == 0 {
                if content.is_empty() { "No lyrics found — try Search below.".to_string() } else { content }
            } else { String::new() }.into());
        });
    });
}

/// LRCLIB search candidates for the lyrics popup: (synced, content) per result.
static LYRICS_SEARCH_RESULTS: std::sync::OnceLock<std::sync::Mutex<Vec<(bool, String)>>> = std::sync::OnceLock::new();
pub fn lyrics_search_results() -> &'static std::sync::Mutex<Vec<(bool, String)>> {
    LYRICS_SEARCH_RESULTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

thread_local! {
    /// Full library lyrics-manager list (one row per song), paged 20/screen.
    static LYRICS_MGR_ROWS: std::cell::RefCell<Vec<LyricsMgrRow>> = const { std::cell::RefCell::new(Vec::new()) };
}
const LYRICS_MGR_PAGE: usize = 20;

/// Publish the current lyrics-manager page (20 rows) + counters to the UI.
pub fn publish_lyrics_mgr_page(w: &MainWindow) {
    LYRICS_MGR_ROWS.with(|r| {
        let all = r.borrow();
        let total = all.len();
        let synced = all.iter().filter(|x| x.status == "Synced").count();
        let missing = all.iter().filter(|x| x.status == "Missing").count();
        // Filter the visible rows by the active stat-card filter (counts stay full).
        let filter = w.get_music_lyrics_mgr_filter().to_string();
        let filtered: Vec<LyricsMgrRow> = all.iter().filter(|x| match filter.as_str() {
            "synced" => x.status == "Synced",
            "normal" => x.status == "Normal",
            "missing" => x.status == "Missing",
            _ => true,
        }).cloned().collect();
        let pages = filtered.len().div_ceil(LYRICS_MGR_PAGE).max(1);
        let page = (w.get_music_lyrics_mgr_page() as usize).min(pages - 1);
        let slice: Vec<LyricsMgrRow> = filtered.iter().skip(page * LYRICS_MGR_PAGE).take(LYRICS_MGR_PAGE).cloned().collect();
        w.set_music_lyrics_mgr_rows(slint::ModelRc::new(slint::VecModel::from(slice)));
        w.set_music_lyrics_mgr_total(total as i32);
        w.set_music_lyrics_mgr_synced(synced as i32);
        w.set_music_lyrics_mgr_missing(missing as i32);
        w.set_music_lyrics_mgr_page(page as i32);
        w.set_music_lyrics_mgr_pages(pages as i32);
        w.set_music_lyrics_sync_progress(if total > 0 { (total - missing) as f32 / total as f32 } else { 0.0 });
    });
}

/// Rebuild the lyrics-manager list from the song library + the lyrics table.
/// My Music only — audiobook chapters never have lyrics and would drown the
/// "Missing" stats (np.p4.music.lyrics).
pub fn rebuild_lyrics_manager(w: &MainWindow) {
    let songs: Vec<(i32, i64, String, String)> = match music_songs().lock() {
        Ok(g) => g.iter().filter(|s| !s.is_audiobook)
            .map(|s| (s.pos, s.item_id, s.title.clone(), s.artist.clone())).collect(),
        Err(_) => return,
    };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // item_id → synced flag, for tracks that actually have lyric content.
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT item_id, synced FROM lyrics WHERE content IS NOT NULL AND content <> ''")
            .fetch_all(&pool).await.unwrap_or_default();
        let map: std::collections::HashMap<i64, i64> = rows.into_iter().collect();
        let list: Vec<LyricsMgrRow> = songs.iter().map(|(pos, id, title, artist)| {
            let status = match map.get(id) {
                Some(s) if *s != 0 => "Synced",
                Some(_) => "Normal",
                None => "Missing",
            };
            LyricsMgrRow { title: title.clone().into(), artist: artist.clone().into(),
                status: status.into(), index: *pos }
        }).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            LYRICS_MGR_ROWS.with(|r| *r.borrow_mut() = list);
            publish_lyrics_mgr_page(&w);
        });
    });
}

/// Take whatever lyrics are on screen down.
///
/// Called at the top of a load, and — the reason it is public — whenever
/// playback moves to something that cannot have lyrics at all: a podcast
/// episode, a radio station, a YouTube track. Every consumer (the Home layouts'
/// lyric panel, the square widget, the zen middle) keys off the row list, so
/// without this the last song's lyrics stayed up, scrolling, over a podcast.
///
/// `music-lyrics-live` is the flag those consumers gate on rather than the rows
/// themselves: a lyrics fetch for the previous track can still be in the air
/// when this runs, and an empty list is not proof that nothing is coming.
pub fn clear_music_lyrics(w: &MainWindow) {
    w.set_music_lyrics_live(false);
    w.set_music_lyrics_offset_ms(0);
    w.set_music_lyrics_active(-1);
    w.set_music_lyrics_text("".into());
    if let Ok(mut g) = music_lyrics_lines().lock() { g.clear(); }
    w.set_music_lyrics_rows(slint::ModelRc::new(slint::VecModel::<MusicLyricLine>::default()));
}

/// Load synced/plain lyrics for the current track from the lyrics table (empty
/// when none — LRCLIB fetch needs network). Parses synced LRC into rows for the
/// scrolling highlight (np.p4.music.lyrics / np.p5.music.lyrics-synced).
pub fn load_music_lyrics(w: &MainWindow) {
    clear_music_lyrics(w);
    let Some(id) = current_music_id(w) else { return; };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let mut row: Option<(i64, String)> = sqlx::query_as(
            "SELECT synced, content FROM lyrics WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        // DB miss → live LRCLIB fetch by track signature, then cache it
        // (np.p5.music.lyrics-synced — was DB-only).
        if row.as_ref().map(|(_, c)| c.is_empty()).unwrap_or(true) {
            let sig: Option<(Option<String>, Option<String>, f64)> = sqlx::query_as(
                "SELECT tm.title, ar.name, COALESCE(tm.duration_s, 0)
                 FROM track_meta tm LEFT JOIN artists ar ON ar.id = tm.artist_id
                 WHERE tm.item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            if let Some((Some(title), artist, dur)) = sig {
                let artist = artist.unwrap_or_default();
                let url = tulipix_music::lyrics::get_url(&artist, &title, "", dur);
                let client = tulipix_core::net::http().clone();
                if let Ok(resp) = client.get(&url)
                    .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                    .send().await {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        let synced = json["syncedLyrics"].as_str().unwrap_or("");
                        let plain = json["plainLyrics"].as_str().unwrap_or("");
                        if !synced.is_empty() {
                            let _ = tulipix_music::lyrics::store(&pool, id, synced, true, "lrclib").await;
                            row = Some((1, synced.to_string()));
                        } else if !plain.is_empty() {
                            let _ = tulipix_music::lyrics::store(&pool, id, plain, false, "lrclib").await;
                            row = Some((0, plain.to_string()));
                        }
                    }
                }
            }
        }
        let (synced, content) = row.unwrap_or((0, String::new()));
        let lines = if synced != 0 { tulipix_music::lyrics::parse_lrc(&content) } else { Vec::new() };
        let _ = weak.upgrade_in_event_loop(move |w| {
            // Drop stale results: the track may have changed while we fetched, which
            // would otherwise show the previous song's lyrics (sync bug).
            if current_music_id(&w) != Some(id) { return; }
            w.set_music_lyrics_text(content.into());
            if !lines.is_empty() {
                let rows: Vec<MusicLyricLine> = lines.iter().map(|(ms, t)| MusicLyricLine {
                    time: fmt_clock(*ms as f64 / 1000.0).into(),
                    text: t.clone().into(),
                }).collect();
                w.set_music_lyrics_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
                // These belong to the track playing now, and everything that
                // draws them is waiting on this to say so.
                w.set_music_lyrics_live(true);
                if let Ok(mut g) = music_lyrics_lines().lock() { *g = lines; }
                update_lyrics_active(&w);
            }
        });
    });
}

/// Submit pending scrobbles (np.p5.music.scrobble) — ListenBrainz (token from
/// the keychain) and Last.fm (signed calls with the session key from the
/// connect flow). Drains oldest-first batches, marking rows submitted on 2xx.
pub fn submit_scrobbles() {
    submit_scrobbles_lastfm();
    let Some(token) = tulipix_core::api_keys::fetch("listenbrainz").ok().flatten() else { return; };
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let pending: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT id, item_id, played_at FROM scrobble_queue
             WHERE service = 'listenbrainz' AND submitted = 0 ORDER BY played_at ASC LIMIT 50")
            .fetch_all(&pool).await.unwrap_or_default();
        if pending.is_empty() { return; }
        let client = tulipix_core::net::http().clone();
        for (row_id, item_id, played_at) in pending {
            let meta: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT tm.title, ar.name, al.title
                 FROM track_meta tm
                 LEFT JOIN artists ar ON ar.id = tm.artist_id
                 LEFT JOIN albums  al ON al.id = tm.album_id
                 WHERE tm.item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten();
            let Some((Some(title), artist, album)) = meta else { continue; };
            let artist = artist.unwrap_or_default();
            if artist.is_empty() { continue; }
            let payload = tulipix_music::listenbrainz::single_listen(
                &artist, &title, album.as_deref(), played_at);
            let ok = client.post(tulipix_music::listenbrainz::SUBMIT_URL)
                .header(reqwest::header::AUTHORIZATION, tulipix_music::listenbrainz::auth_header(&token))
                .json(&payload).send().await
                .map(|r| r.status().is_success()).unwrap_or(false);
            if ok {
                let _ = sqlx::query("UPDATE scrobble_queue SET submitted = 1 WHERE id = ?")
                    .bind(row_id).execute(&pool).await;
            } else { break; } // network/auth issue — retry the rest next time
        }
    });
}

/// Drain the Last.fm half of the scrobble queue with signed `track.scrobble`
/// calls. Needs both the API creds (Settings → API Keys, "KEY:SECRET") and the
/// session key minted by the connect flow; otherwise rows stay queued.
pub fn submit_scrobbles_lastfm() {
    let creds = tulipix_core::api_keys::fetch("lastfm").ok().flatten()
        .and_then(|v| tulipix_music::scrobble::parse_key_secret(&v));
    let Some((api_key, secret)) = creds else { return; };
    let Some(sk) = tulipix_core::api_keys::fetch("lastfm.session").ok().flatten() else { return; };
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let pending = tulipix_music::scrobble::pending(&pool, 50).await.unwrap_or_default();
        if pending.is_empty() { return; }
        let client = tulipix_core::net::http().clone();
        for (row_id, item_id, played_at) in pending {
            let meta: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT tm.title, ar.name, al.title
                 FROM track_meta tm
                 LEFT JOIN artists ar ON ar.id = tm.artist_id
                 LEFT JOIN albums  al ON al.id = tm.album_id
                 WHERE tm.item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten();
            let Some((Some(title), artist, album)) = meta else { continue; };
            let artist = artist.unwrap_or_default();
            if artist.is_empty() { continue; }
            let ts = played_at.to_string();
            let mut params: Vec<(&str, &str)> = vec![
                ("method", "track.scrobble"), ("api_key", &api_key), ("sk", &sk),
                ("artist", &artist), ("track", &title), ("timestamp", &ts),
            ];
            let album = album.unwrap_or_default();
            if !album.is_empty() { params.push(("album", &album)); }
            let form = tulipix_music::scrobble::signed_params(&params, &secret);
            let ok = client.post(tulipix_music::scrobble::API_ROOT)
                .form(&form).send().await
                .map(|r| r.status().is_success()).unwrap_or(false);
            if ok {
                let _ = tulipix_music::scrobble::mark_submitted(&pool, &[row_id]).await;
            } else { break; } // auth/network issue — retry next drain
        }
    });
}

/// Sort the in-memory Songs list by the active mode/direction.
pub fn sort_music_songs(sort: &str, dir: &str) {
    let Ok(mut g) = music_songs().lock() else { return; };
    g.sort_by(|a, b| {
        let o = match sort {
            "title"  => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            "artist" => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
            "plays"  => a.plays.cmp(&b.plays),
            "rating" => a.stars.cmp(&b.stars).then(a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            // Release date — songs without one sort last (treated as far-future).
            "release" => {
                let key = |s: &str| if s.trim().is_empty() { "9999".to_string() } else { s.to_string() };
                key(&a.release_date).cmp(&key(&b.release_date)).then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            },
            _        => a.added.cmp(&b.added),
        };
        if dir == "asc" { o } else { o.reverse() }
    });
}

/// Build the current Songs page (30 rows) into the UI model + page counters.
pub fn rebuild_music_songs_page(w: &MainWindow) {
    let g = match music_songs().lock() { Ok(g) => g, Err(_) => return };
    let q = music_query_filter().lock().map(|s| s.to_lowercase()).unwrap_or_default();
    // Filter by the search box (title/artist/album substring) before paginating.
    // Audiobook-flagged tracks are excluded — they live in the Audiobooks
    // section, not the My Music Songs list (np.p5.music.audiobook-detect).
    let view: Vec<&SongMeta> = g.iter().filter(|s| {
        !s.is_audiobook
            && (q.is_empty() || s.title.to_lowercase().contains(&q) || s.artist.to_lowercase().contains(&q)
                || s.album.to_lowercase().contains(&q))
    }).collect();
    let total = view.len();
    // Smaller pages while searching so songs stay above the artists/albums rows.
    let per = if q.is_empty() { SONG_PAGE } else { 14 };
    let pages = total.div_ceil(per).max(1);
    let page = (w.get_music_song_page() as usize).min(pages - 1);
    let thumb_at = music_thumb_at;
    let page_rows: Vec<&SongMeta> = view.iter().skip(page * per).take(per).copied().collect();
    let rows: Vec<MusicSongRow> = page_rows.iter().map(|s| MusicSongRow {
        thumb: thumb_at(s.pos),
        title: s.title.clone().into(),
        artist: s.artist.clone().into(),
        duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
        index: s.pos,
    }).collect();
    // Rich list rows with the lyrics / fav / star columns.
    let rows_ex: Vec<SongRowEx> = page_rows.iter().map(|s| SongRowEx {
        thumb: thumb_at(s.pos),
        title: s.title.clone().into(),
        artist: s.artist.clone().into(),
        album: s.album.clone().into(),
        duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
        index: s.pos,
        loved: s.loved,
        stars: s.stars,
        synced: s.synced,
    }).collect();
    w.set_music_songs(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_songs_ex(slint::ModelRc::new(slint::VecModel::from(rows_ex)));
    w.set_music_song_total(total as i32);
    w.set_music_song_pages(pages as i32);
    w.set_music_song_page(page as i32);
}

/// Read audio tags from a file via the bundled (or PATH) ffprobe — title /
/// artist / album / genre / year / track / disc + duration & stream info — so
/// the music browse views have real metadata (np.p4.music.tags).
pub fn ffprobe_tags(path: &std::path::Path) -> tulipix_music::tags::TrackTags {
    use tulipix_music::tags::{self, TrackTags};
    let mut t = TrackTags::default();
    let ff = tulipix_core::thumbs::tool_bin("ffprobe");
    let out = std::process::Command::new(ff)
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path).no_window().output();
    let Ok(out) = out else { return fallback_title(t, path); };
    let json: serde_json::Value = match serde_json::from_slice(&out.stdout) { Ok(v) => v, Err(_) => return fallback_title(t, path) };
    let fmt = &json["format"];
    let tagv = &fmt["tags"];
    // Case-insensitive tag lookup (ffmpeg emits TITLE vs title per container).
    let get = |k: &str| -> Option<String> {
        tagv.as_object().and_then(|o| o.iter()
            .find(|(kk, _)| kk.to_lowercase() == k)
            .and_then(|(_, v)| v.as_str()).map(|s| s.to_string()))
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
    t.container = path.extension().and_then(|e| e.to_str()).and_then(tags::container_from_ext).map(|s| s.to_string());
    fallback_title(t, path)
}

/// Write title/artist/album tags back into the audio file via the bundled
/// ffmpeg (np.p5.music.tag-editor). Stream-copies to a sibling temp file, then
/// atomically renames over the original — so a decode failure leaves the source
/// untouched. Empty fields are left as-is on the file.
pub struct FileTags<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub album_artist: &'a str,
    pub genre: &'a str,
    pub date: &'a str,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
}

pub fn write_audio_tags(path: &str, tags: &FileTags) {
    let src = std::path::Path::new(path);
    let Some(ext) = src.extension().and_then(|e| e.to_str()) else { return; };
    let tmp = src.with_extension(format!("tulipix-tmp.{ext}"));
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let mut cmd = std::process::Command::new(ff);
    cmd.no_window();
    cmd.arg("-v").arg("error").arg("-y").arg("-i").arg(src)
        .arg("-map_metadata").arg("0").arg("-c").arg("copy");
    for (key, val) in [
        ("title", tags.title), ("artist", tags.artist), ("album", tags.album),
        ("album_artist", tags.album_artist), ("genre", tags.genre), ("date", tags.date),
    ] {
        if !val.is_empty() { cmd.arg("-metadata").arg(format!("{key}={val}")); }
    }
    if let Some(n) = tags.track_no { cmd.arg("-metadata").arg(format!("track={n}")); }
    if let Some(n) = tags.disc_no  { cmd.arg("-metadata").arg(format!("disc={n}")); }
    cmd.arg(&tmp);
    match cmd.status() {
        Ok(st) if st.success() && tmp.exists() && std::fs::metadata(&tmp).map(|m| m.len() > 0).unwrap_or(false) => {
            if let Err(e) = std::fs::rename(&tmp, src) {
                tracing::warn!(error = %e, "tag write-back rename failed");
                let _ = std::fs::remove_file(&tmp);
            }
        }
        _ => { let _ = std::fs::remove_file(&tmp); }
    }
}

/// Fall back to the filename stem when the file carries no title tag.
pub fn fallback_title(mut t: tulipix_music::tags::TrackTags, path: &std::path::Path) -> tulipix_music::tags::TrackTags {
    if t.title.is_none() {
        t.title = path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string());
    }
    t
}

/// Extract tags for every music item missing metadata, upsert into track_meta /
/// artists / albums, then refresh the Music views (np.p4.music.tags + scan).
pub fn ingest_music_tags(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, abs_path FROM items WHERE section = 'music' AND missing_since IS NULL")
            .fetch_all(&pool).await.unwrap_or_default();
        let mut tagged = 0u32;
        for (id, path) in rows {
            // Idempotent: skip items already carrying a title.
            let done: Option<(i64,)> = sqlx::query_as(
                "SELECT item_id FROM track_meta WHERE item_id = ? AND title IS NOT NULL")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            if done.is_some() { continue; }
            let p = std::path::PathBuf::from(&path);
            if !p.exists() { continue; }
            let tags = tokio::task::spawn_blocking(move || ffprobe_tags(&p)).await.unwrap_or_default();
            if tulipix_music::scan::upsert_track(&pool, id, &path, &tags).await.is_ok() { tagged += 1; }
        }
        tracing::info!(tagged, "music tag ingest complete");
        let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
    });
}

/// Build music_ids (item_id per playback position) from the music.db `items`
/// table, then populate the dashboard rails + browse groups (Albums/Artists/
/// Genres). Each browse/dashboard tile carries its playback position in `index`
/// so the existing `play-music` path plays it. (np.p4.music.dashboard / browse)
pub fn populate_music_views(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // Re-assert audiobook-section flags BEFORE building any view, so
        // chapters never leak into songs/albums/artists even when the user
        // hasn't opened the Audiobooks tab yet this session (subtree-aware —
        // a parent dir of per-book folders flags everything under it).
        for (folder, key) in load_folder_sections() {
            if key == "audiobooks" {
                let _ = tulipix_music::audiobooks::flag_audiobook_folder(&pool, &folder).await;
            }
        }
        // abs_path → item_id for every present music item.
        let id_rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, abs_path FROM items WHERE section = 'music' AND missing_since IS NULL")
            .fetch_all(&pool).await.unwrap_or_default();
        let recent = tulipix_music::dashboard::recently_played(&pool, 20).await.unwrap_or_default();
        // Seven, because the rail draws seven across and lays the rest out
        // past the right edge of the page where nothing can reach them.
        let most   = tulipix_music::dashboard::most_played(&pool, 7).await.unwrap_or_default();
        let loved  = tulipix_music::rating::loved(&pool, 20).await.unwrap_or_default();
        let fresh  = tulipix_music::dashboard::new_this_week(&pool, 20).await.unwrap_or_default();
        // Home — Continue listening + the listening strip. Both are reads over
        // tables playback already writes; nothing new is recorded for them.
        let resume_rows = tulipix_music::dashboard::resume_pct(&pool, 14).await.unwrap_or_default();
        let listen_stats = tulipix_music::dashboard::stats(&pool).await.unwrap_or_default();
        // Per-folder sidecar covers (folder.jpg / cover.png / …). Reading one
        // directory listing per folder beats borrowing a track's embedded art,
        // which is what the folder tiles used to show.
        let folder_covers: std::collections::HashMap<String, String> = {
            let dirs: Vec<String> = sqlx::query_scalar(
                "SELECT DISTINCT folder FROM track_meta \
                 WHERE folder IS NOT NULL AND folder != '' AND is_audiobook = 0")
                .fetch_all(&pool).await.unwrap_or_default();
            dirs.into_iter().filter_map(|d| {
                // Real filenames, not lowercased — `pick_cover` compares
                // case-insensitively and the winner is joined back onto the
                // path, so a folder holding `Cover.JPG` must stay spelled that
                // way or the load fails on a case-sensitive filesystem.
                let names: Vec<String> = std::fs::read_dir(&d).ok()?
                    .flatten()
                    .filter_map(|e| e.file_name().to_str().map(str::to_owned))
                    .collect();
                let pick = tulipix_music::folders::pick_cover(&names)?;
                Some((d.clone(), std::path::Path::new(&d).join(pick).to_string_lossy().into_owned()))
            }).collect()
        };
        let albums = tulipix_music::browse::albums(&pool).await.unwrap_or_default();
        let artists = tulipix_music::browse::artists(&pool).await.unwrap_or_default();
        let genres = tulipix_music::browse::genres(&pool).await.unwrap_or_default();
        // Favorited albums/artists (np.p5.music.fav-collections). The `loved`
        // columns are added on demand; ignore the error when they already exist.
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let loved_albums: std::collections::HashSet<i64> = sqlx::query_scalar("SELECT id FROM albums WHERE loved = 1")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        let loved_artists: std::collections::HashSet<i64> = sqlx::query_scalar("SELECT id FROM artists WHERE loved = 1")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        let album_rating: std::collections::HashMap<i64, i64> = sqlx::query_as("SELECT id, rating FROM albums WHERE rating > 0")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        let artist_rating: std::collections::HashMap<i64, i64> = sqlx::query_as("SELECT id, rating FROM artists WHERE rating > 0")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        // Per-track grouping keys, to resolve a group's first playback position.
        let meta: Vec<(i64, Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT item_id, album_id, artist_id, genre FROM track_meta WHERE is_audiobook = 0").fetch_all(&pool).await.unwrap_or_default();
        // Detailed Songs list rows (np.p4.music.browse list view).
        // On-demand columns (idempotent) so the SELECT below never fails on older DBs.
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(&pool).await;
        // music_songs stays complete (incl. audiobooks) so the Audiobooks section
        // can resolve each chapter's title/duration by position; the My Music
        // Songs *list* filters audiobooks out at render in rebuild_music_songs_page.
        let song_rows: Vec<(i64, Option<String>, Option<String>, Option<f64>, i64, i64, i64, i64, Option<i64>, Option<String>, Option<String>, i64)> = sqlx::query_as(
            "SELECT tm.item_id, tm.title, ar.name, tm.duration_s, it.added, tm.play_count, \
                    COALESCE(tm.loved,0), COALESCE(tm.rating,0), \
                    (SELECT ly.synced FROM lyrics ly WHERE ly.item_id = tm.item_id), al.title, tm.release_date, \
                    tm.is_audiobook \
             FROM track_meta tm JOIN items it ON it.id = tm.item_id AND it.missing_since IS NULL \
             LEFT JOIN artists ar ON ar.id = tm.artist_id \
             LEFT JOIN albums al ON al.id = tm.album_id").fetch_all(&pool).await.unwrap_or_default();
        // Folder hierarchy (np.p4.music.folders) — folder path per track.
        let folder_rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT item_id, folder FROM track_meta WHERE folder IS NOT NULL AND folder != '' AND is_audiobook = 0")
            .fetch_all(&pool).await.unwrap_or_default();
        // Playlists (np.p4.music.playlists) — name + first track. Smart playlists
        // (Loved / Recently Added) compute their tracks from a rule, so the LEFT
        // JOIN count is 0 — resolve those via the rule so counts/covers are real.
        let playlist_raw: Vec<(i64, String, i64, Option<i64>, i64, Option<String>)> = sqlx::query_as(
            "SELECT p.id, p.name, COUNT(pi.item_id), MIN(pi.item_id), COALESCE(p.is_smart,0), p.rule_json \
             FROM playlists p LEFT JOIN playlist_items pi ON pi.playlist_id = p.id GROUP BY p.id")
            .fetch_all(&pool).await.unwrap_or_default();
        let mut playlist_rows: Vec<(i64, String, i64, Option<i64>)> = Vec::with_capacity(playlist_raw.len());
        for (pid, name, n, first, is_smart, rule_json) in playlist_raw {
            if is_smart != 0 {
                let ids = match rule_json.as_ref().and_then(|j| serde_json::from_str::<tulipix_music::playlists::SmartRule>(j).ok()) {
                    Some(rule) => tulipix_music::playlists::evaluate(&pool, &rule).await.unwrap_or_default(),
                    None => Vec::new(),
                };
                playlist_rows.push((pid, name, ids.len() as i64, ids.first().copied()));
            } else {
                playlist_rows.push((pid, name, n, first));
            }
        }

        let _ = weak.upgrade_in_event_loop(move |w| {
            let id_of: std::collections::HashMap<String, i64> = id_rows.into_iter()
                .map(|(id, p)| (p, id)).collect();
            // music_ids aligned to music_paths order.
            let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
            let ids: Vec<i64> = paths.iter()
                .map(|p| id_of.get(&p.to_string_lossy().into_owned()).copied().unwrap_or(-1))
                .collect();
            // pos_of[item_id] = playback position.
            let mut pos_of: std::collections::HashMap<i64, i32> = std::collections::HashMap::new();
            for (i, id) in ids.iter().enumerate() { if *id >= 0 { pos_of.entry(*id).or_insert(i as i32); } }
            if let Ok(mut g) = music_ids().lock() { *g = ids.clone(); }

            // Snapshot the scanned song tiles by position to reuse labels. The
            // model carries no image, so the thumb is decoded here — rails are a
            // row or two, not the library.
            let songs = w.get_music_tiles();
            let tile_at = |pos: i32| -> Option<PhotoTile> {
                if pos < 0 || pos as usize >= songs.row_count() { return None; }
                let mut t = songs.row_data(pos as usize)?;
                t.thumb = music_thumb_at(pos);
                Some(t)
            };
            // Tagged title by item_id. The scanned tile is labelled with the
            // file stem — right for the Songs grid, which has to name a track
            // with no tags at all — but a rail that has the item_id in hand can
            // show the real title, so it does whenever one is stored.
            let title_of: std::collections::HashMap<i64, slint::SharedString> = song_rows.iter()
                .filter_map(|r| r.1.as_ref()
                    .filter(|t| !t.trim().is_empty())
                    .map(|t| (r.0, t.as_str().into())))
                .collect();
            let rail = |id_list: &[i64]| -> Vec<PhotoTile> {
                id_list.iter().filter_map(|id| {
                    let pos = pos_of.get(id).copied()?;
                    let mut t = tile_at(pos)?;
                    t.index = pos;
                    if let Some(title) = title_of.get(id) { t.label = title.clone(); }
                    Some(t)
                }).collect()
            };
            // First playback position for each grouping key.
            let mut first_album: std::collections::HashMap<i64, i32> = std::collections::HashMap::new();
            let mut first_artist: std::collections::HashMap<i64, i32> = std::collections::HashMap::new();
            let mut first_genre: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
            for (item_id, alb, art, genre) in &meta {
                let Some(&pos) = pos_of.get(item_id) else { continue; };
                if let Some(a) = alb { first_album.entry(*a).or_insert(pos); }
                if let Some(a) = art { first_artist.entry(*a).or_insert(pos); }
                if let Some(g) = genre { if !g.is_empty() { first_genre.entry(g.clone()).or_insert(pos); } }
            }

            w.set_music_recent(slint::ModelRc::new(slint::VecModel::from(rail(&recent))));
            w.set_music_most(slint::ModelRc::new(slint::VecModel::from(rail(&most))));
            w.set_music_loved(slint::ModelRc::new(slint::VecModel::from(rail(&loved))));
            // Seven, and no more: the rail is one row of seven cells and lays
            // anything past that out beyond the card's right edge, where it
            // shows as a stray tile outside the outline. Truncated here rather
            // than in the query because `rail` drops rows whose file is not in
            // the current walk — asking SQL for seven can hand back four.
            let mut fresh_tiles = rail(&fresh);
            fresh_tiles.truncate(7);
            w.set_music_fresh(slint::ModelRc::new(slint::VecModel::from(fresh_tiles)));
            // Continue listening — `count` carries percent-complete, which is
            // the only spare integer on PhotoTile and what the card's progress
            // hairline reads.
            let cont: Vec<PhotoTile> = resume_rows.iter().filter_map(|(id, pct)| {
                let pos = pos_of.get(id).copied()?;
                tile_at(pos).map(|mut t| {
                    t.index = pos; t.count = *pct;
                    if let Some(title) = title_of.get(id) { t.label = title.clone(); }
                    t
                })
            }).take(7).collect();
            w.set_music_continue_rows(slint::ModelRc::new(slint::VecModel::from(cont)));
            // Listening strip. An empty total hides the whole strip rather
            // than showing four em dashes on a library nobody has played yet.
            use tulipix_music::dashboard::fmt_listen;
            w.set_music_stats_total(if listen_stats.total_ms > 0 {
                fmt_listen(listen_stats.total_ms).into() } else { slint::SharedString::new() });
            w.set_music_stats_week(fmt_listen(listen_stats.week_ms).into());
            w.set_music_stats_genre(listen_stats.top_genre.clone().unwrap_or_default().into());
            w.set_music_stats_streak(match listen_stats.streak_days {
                0 => "—".to_string(),
                1 => "1 day".to_string(),
                n => format!("{n} days"),
            }.into());

            // Albums — cover from cover_path (else the first track's thumb).
            let album_tiles_src: Vec<(PhotoTile, i64)> = albums.iter().map(|a| {
                let pos = first_album.get(&a.album_id).copied().unwrap_or(-1);
                let thumb = a.cover_path.as_ref()
                    .map(|c| slint::Image::load_from_path(std::path::Path::new(c)).unwrap_or_default())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                let label = match &a.artist { Some(ar) => format!("{} · {}", a.title, ar), None => a.title.clone() };
                (PhotoTile { thumb, label: label.into(), index: pos, starred: loved_albums.contains(&a.album_id),
                    stack_count: album_rating.get(&a.album_id).copied().unwrap_or(0) as i32,
                    count: a.track_count as i32, ..Default::default() }, a.track_count)
            }).collect();
            set_browse_src("albums", album_tiles_src.clone());
            rebuild_browse_tab(&w, "albums");
            // Favorited albums → the categorized Favorites page.
            let fav_album_tiles: Vec<PhotoTile> = album_tiles_src.iter().filter(|(t, _)| t.starred).map(|(t, _)| t.clone()).collect();
            w.set_music_fav_albums(slint::ModelRc::new(slint::VecModel::from(fav_album_tiles)));

            // Artists — cover from the artist's first track (album art); Genres stay label-only.
            let artist_src: Vec<(PhotoTile, i64)> = artists.iter().map(|(id, name, n)| {
                let pos = first_artist.get(id).copied().unwrap_or(-1);
                (PhotoTile {
                    thumb: tile_at(pos).map(|t| t.thumb).unwrap_or_default(),
                    label: name.clone().into(), index: pos, starred: loved_artists.contains(id),
                    stack_count: artist_rating.get(id).copied().unwrap_or(0) as i32,
                    count: *n as i32, ..Default::default()
                }, *n)
            }).collect();
            set_browse_src("artists", artist_src.clone());
            rebuild_browse_tab(&w, "artists");
            // Favorited artists → the categorized Favorites page.
            let fav_artist_tiles: Vec<PhotoTile> = artist_src.iter().filter(|(t, _)| t.starred).map(|(t, _)| t.clone()).collect();
            w.set_music_fav_artists(slint::ModelRc::new(slint::VecModel::from(fav_artist_tiles)));
            // Per-genre cover override (music.genre.cover.<genre>) wins over the
            // first-track album art (np.p4.music.browse / genre-art).
            let gcovers = tulipix_core::settings::Settings::load().map(|s| s.advanced).unwrap_or_default();
            let genre_src: Vec<(PhotoTile, i64)> = genres.iter().map(|(g, n)| {
                let pos = first_genre.get(g).copied().unwrap_or(-1);
                let thumb = gcovers.get(&format!("music.genre.cover.{g}"))
                    .map(|c| slint::Image::load_from_path(std::path::Path::new(c)).unwrap_or_default())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                (PhotoTile { thumb, label: g.clone().into(), index: pos, ..Default::default() }, *n)
            }).collect();
            set_browse_src("genres", genre_src);
            rebuild_browse_tab(&w, "genres");

            // Detailed Songs list — map each track to its playback position, with
            // a filename fallback for missing titles.
            let metas: Vec<SongMeta> = song_rows.into_iter().filter_map(|(id, title, artist, dur, added, plays, loved, rating, synced, album, release, is_audiobook)| {
                let pos = *pos_of.get(&id)?;
                let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
                    paths.get(pos as usize)
                        .and_then(|p| p.file_stem()).and_then(|s| s.to_str())
                        .unwrap_or("Unknown").to_string()
                });
                Some(SongMeta {
                    pos, item_id: id, title,
                    artist: artist.unwrap_or_default(),
                    album: album.unwrap_or_default(),
                    duration_s: dur.unwrap_or(0.0),
                    added, plays,
                    loved: loved != 0, stars: rating as i32, synced: synced.is_some(),
                    release_date: release.unwrap_or_default(),
                    is_audiobook: is_audiobook != 0,
                })
            }).collect();
            // Auto-fill the Songs-row pills with current coverage (lyrics synced %,
            // tags present %) so they reflect library state without opening either
            // manager (np.p5.atmusic.songs-pills).
            {
                let total = metas.len();
                if total > 0 {
                    let synced_n = metas.iter().filter(|m| m.synced).count();
                    let tagged_n = metas.iter()
                        .filter(|m| !m.artist.trim().is_empty() && !m.album.trim().is_empty()).count();
                    w.set_music_lyrics_sync_progress(synced_n as f32 / total as f32);
                    w.set_music_meta_fetch_progress(tagged_n as f32 / total as f32);
                }
            }
            // title/artist/duration by item_id, for the Home "Recently played" list.
            let info: std::collections::HashMap<i64, (String, String, f64)> = metas.iter()
                .map(|m| (ids[m.pos as usize], (m.title.clone(), m.artist.clone(), m.duration_s)))
                .collect();
            // Recently-played pool (≤18) → Home pager (6/page, 3 pages).
            let recent_pool: Vec<SongMeta> = recent.iter().take(18).filter_map(|id| {
                let pos = *pos_of.get(id)?;
                let (title, artist, dur) = info.get(id).cloned().unwrap_or_default();
                Some(SongMeta { pos, item_id: *id, title, artist, album: String::new(), duration_s: dur, added: 0, plays: 0, loved: false, stars: 0, synced: false, release_date: String::new(), is_audiobook: false })
            }).collect();
            if let Ok(mut g) = music_recent().lock() { *g = recent_pool; }
            w.set_music_recent_page(0);
            rebuild_recent_page(&w);

            // Newest 4 by `added` → the Welcome home layout's Recently Added
            // card, Music tab (one row of four). Recently PLAYED (the pool
            // above) is a different question and already has its own pager.
            // Audiobook chapters are dropped for the same reason the Songs grid
            // drops them: a book lands as 40 "new songs" and would be the whole
            // tab.
            {
                let mut newest: Vec<&SongMeta> = metas.iter().filter(|m| !m.is_audiobook).collect();
                newest.sort_by(|a, b| b.added.cmp(&a.added));
                let rows: Vec<MusicSongRow> = newest.iter().take(4).map(|s| MusicSongRow {
                    thumb: music_thumb_at(s.pos),
                    title: s.title.clone().into(), artist: s.artist.clone().into(),
                    duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
                    index: s.pos,
                }).collect();
                w.set_home_recent_songs(slint::ModelRc::new(slint::VecModel::from(rows)));
            }

            // Top artists / albums (most tracks first, capped) for the Home grids.
            let mut top_artist_src = artists.clone();
            top_artist_src.sort_by(|a, b| b.2.cmp(&a.2));
            // Eight, for the Home grid's four columns by two rows.
            let top_artist_tiles: Vec<PhotoTile> = top_artist_src.iter().take(8).map(|(id, name, _)| {
                let pos = first_artist.get(id).copied().unwrap_or(-1);
                PhotoTile { thumb: tile_at(pos).map(|t| t.thumb).unwrap_or_default(),
                    label: name.clone().into(), index: pos, ..Default::default() }
            }).collect();
            w.set_music_top_artists(slint::ModelRc::new(slint::VecModel::from(top_artist_tiles)));
            let mut top_album_src: Vec<_> = album_tiles_src.clone();
            top_album_src.sort_by(|a, b| b.1.cmp(&a.1)); // by track_count
            let top_album_tiles: Vec<PhotoTile> = top_album_src.iter().take(7).map(|(t, _)| t.clone()).collect();
            w.set_music_top_albums(slint::ModelRc::new(slint::VecModel::from(top_album_tiles)));

            // Folders — first playback position per folder, basename label.
            let mut first_folder: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
            for (item_id, folder) in &folder_rows {
                if let Some(&pos) = pos_of.get(item_id) { first_folder.entry(folder.clone()).or_insert(pos); }
            }
            let mut folder_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            for (_, folder) in &folder_rows { *folder_counts.entry(folder.clone()).or_insert(0) += 1; }
            let mut folder_keys: Vec<&String> = first_folder.keys().collect();
            folder_keys.sort();
            // Section tag per folder (np.p5.atmusic.folder-sections) — carried in
            // `color_label` so the Folders grid can render the assignment chip.
            let sec_map = load_folder_sections();
            let folder_src: Vec<(PhotoTile, i64)> = folder_keys.iter().map(|folder| {
                let base = std::path::Path::new(folder.as_str()).file_name()
                    .and_then(|s| s.to_str()).unwrap_or(folder.as_str());
                let n = folder_counts.get(*folder).copied().unwrap_or(0);
                let sec = sec_map.get(*folder).cloned().unwrap_or_else(|| "mymusic".to_string());
                let pos = first_folder.get(*folder).copied().unwrap_or(-1);
                // Sidecar cover first (that is the art the folder was given),
                // then the first track's embedded art, then nothing.
                let thumb = folder_covers.get(*folder)
                    .and_then(|c| slint::Image::load_from_path(std::path::Path::new(c)).ok())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                (PhotoTile {
                    thumb,
                    label: format!("🗂 {base} · {n}").into(),
                    color_label: sec.into(),
                    index: pos, ..Default::default()
                }, n)
            }).collect();
            set_browse_src("folders", folder_src);
            rebuild_browse_tab(&w, "folders");

            // Playlists — `index` carries the playlist DB id so playlist-open can
            // load its tracks (np.p5.music.playlists-builder).
            let playlist_src: Vec<(PhotoTile, i64)> = playlist_rows.iter().map(|(pid, name, n, first)| {
                // Cover: custom playlist art → else the first track's album thumb.
                let pos = first.and_then(|fid| pos_of.get(&fid).copied()).unwrap_or(-1);
                let thumb = playlist_cover_path(*pid)
                    .map(|p| slint::Image::load_from_path(std::path::Path::new(&p)).unwrap_or_default())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                (PhotoTile {
                    label: format!("{name} · {n}").into(), thumb,
                    index: *pid as i32, ..Default::default()
                }, *n)
            }).collect();
            set_browse_src("playlists", playlist_src);
            rebuild_browse_tab(&w, "playlists");

            if let Ok(mut g) = music_songs().lock() { *g = metas; }
            sort_music_songs(&w.get_music_song_sort(), &w.get_music_song_dir());
            w.set_music_song_page(0);
            rebuild_music_songs_page(&w);

            // Cold-start resume (np.p5.music.resume): point the transport at the
            // last-played track; the saved position applies on the next play.
            if music_warm_once("resume") {
                if let Some((rid, rpos)) = load_music_pref("music.resume")
                    .and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse::<i64>().ok()?, b.parse::<f64>().ok()?)))) {
                    let pos = music_songs().lock().ok()
                        .and_then(|g| g.iter().find(|s| s.item_id == rid).map(|s| s.pos));
                    if let Some(p) = pos.filter(|p| *p >= 0) {
                        w.set_music_np_index(p);
                        w.set_music_np_total(music_ids().lock().map(|g| g.len() as i32).unwrap_or(0));
                        if let Ok(mut g) = resume_pending().lock() { *g = Some((rid, rpos)); }
                    }
                }
            }
        });
    });
}

/// Pending "resume here" request — consumed by the next `play_music_at` spawn
/// of the matching track (applied as mpv `--start`).
static RESUME_PENDING: std::sync::OnceLock<std::sync::Mutex<Option<(i64, f64)>>> = std::sync::OnceLock::new();
pub fn resume_pending() -> &'static std::sync::Mutex<Option<(i64, f64)>> {
    RESUME_PENDING.get_or_init(|| std::sync::Mutex::new(None))
}

/// Throttle stamp for the periodic now-playing position save.
static RESUME_SAVED_AT: std::sync::OnceLock<std::sync::Mutex<std::time::Instant>> = std::sync::OnceLock::new();
pub fn resume_maybe_save(w: &MainWindow, pos_s: f64) {
    // Audiobooks never write the global song-resume: a book position leaking
    // into music.resume made the next SONG cold-start mid-file (user report
    // 2026-07-12, songs cut at 1:26). Book progress has its own chapter-level
    // store (audiobook_progress).
    if w.get_music_player_mode().as_str() == "book" { return; }
    let stamp = RESUME_SAVED_AT.get_or_init(|| std::sync::Mutex::new(std::time::Instant::now() - std::time::Duration::from_secs(60)));
    let due = stamp.lock().map(|g| g.elapsed().as_secs() >= 15).unwrap_or(false);
    if !due || pos_s < 5.0 { return; }
    let Some(id) = current_music_id(w) else { return; };
    if let Ok(mut g) = stamp.lock() { *g = std::time::Instant::now(); }
    save_music_pref("music.resume", &format!("{id},{pos_s:.0}"));
}

// Single audio player instance — replacing it on each play avoids stacking
// overlapping mpv processes the way video (separate windows) can tolerate.
// MUSIC_PROC / VIDEO_PID / mpv_die_with_parent / stop_video / kill_music_proc / kill_all_mpv moved to tulipix_common.
/// Generation counter for the music sleep timer; bumped on each cycle so a
/// pending timer task knows it was superseded.
pub static SLEEP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Volume-fade flags. While any is set, mpv "volume" property-change events
/// are NOT mirrored into the UI slider — the fades move mpv's volume, the
/// user's chosen volume stays put and is restored afterwards.
pub static FADE_TAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static FADE_IN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub static FADE_SLEEP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn fade_active() -> bool {
    use std::sync::atomic::Ordering::Relaxed;
    FADE_TAIL.load(Relaxed) || FADE_IN.load(Relaxed) || FADE_SLEEP.load(Relaxed)
}

/// Arm (or clear) the sleep timer. `min > 0` = stop after N minutes with a
/// 10-second volume fade-out at the tail (tulipix_music::sleep_timer's ramp);
/// `min == -1` = stop when the current track ends; `min == 0` = off.
pub fn arm_sleep_timer(weak: &slint::Weak<MainWindow>, min: i32) {
    use std::sync::atomic::Ordering;
    let Some(w) = weak.upgrade() else { return; };
    w.set_music_sleep_min(min);
    let tok = SLEEP_GEN.fetch_add(1, Ordering::SeqCst) + 1; // cancels any prior timer
    SLEEP_STOP_EOT.store(min == -1, Ordering::SeqCst);
    if min <= 0 {
        // Off (or EoT, which needs no task): make sure a mid-fade cancel
        // doesn't leave the live track quiet.
        FADE_SLEEP.store(false, Ordering::Relaxed);
        let vol = w.get_music_volume().clamp(0.0, 130.0);
        music_ipc(&["set_property", "volume", &format!("{}", vol as i32)]);
        return;
    }
    const FADE_S: f64 = 10.0;
    let timer = tulipix_music::sleep_timer::SleepTimer {
        mode: tulipix_music::sleep_timer::SleepMode::Timed { after_s: min as f64 * 60.0, fade_s: FADE_S },
        armed_at_s: 0.0,
    };
    let weak = weak.clone();
    tokio::runtime::Handle::current().spawn(async move {
        let t0 = std::time::Instant::now();
        let head = (min as u64 * 60).saturating_sub(FADE_S as u64);
        tokio::time::sleep(std::time::Duration::from_secs(head)).await;
        if SLEEP_GEN.load(Ordering::SeqCst) != tok { return; } // superseded
        // Snapshot the user's volume once, then ramp it down over the tail.
        let (txv, rxv) = std::sync::mpsc::channel();
        let _ = weak.upgrade_in_event_loop(move |w| { let _ = txv.send(w.get_music_volume()); });
        let base = rxv.recv_timeout(std::time::Duration::from_secs(1)).unwrap_or(100.0).clamp(0.0, 130.0) as f64;
        FADE_SLEEP.store(true, Ordering::Relaxed);
        loop {
            let now = t0.elapsed().as_secs_f64();
            if SLEEP_GEN.load(Ordering::SeqCst) != tok { FADE_SLEEP.store(false, Ordering::Relaxed); return; }
            if timer.should_stop(now, false) { break; }
            let v = base * timer.volume_at(now);
            music_ipc(&["set_property", "volume", &format!("{}", v as i32)]);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        FADE_SLEEP.store(false, Ordering::Relaxed);
        let _ = weak.upgrade_in_event_loop(|w| {
            stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
            w.set_music_playing(false);
            w.set_music_sleep_min(0);
            tracing::info!("music sleep timer fired (faded)");
        });
    });
}
// MUSIC_SOCK / music_sock / MUSIC_GEN moved to tulipix_common.
/// Live 10-band equalizer gains (dB), shared by presets + per-band drags.
static MUSIC_EQ: std::sync::OnceLock<std::sync::Mutex<[f64; 10]>> = std::sync::OnceLock::new();
pub fn music_eq() -> &'static std::sync::Mutex<[f64; 10]> {
    MUSIC_EQ.get_or_init(|| std::sync::Mutex::new([0.0; 10]))
}
/// Build an mpv `af` value for the 10-band EQ. mpv's option parser treats
/// `|`/`=`/space specially, so the `anequalizer` params are length-quoted
/// (`%N%…`) — the format that survives both `--af=` and IPC `set_property af`.
/// Empty when flat (clears the filter).
pub fn music_eq_af(gains: &[f64; 10]) -> String {
    if gains.iter().all(|g| *g == 0.0) { return String::new(); }
    const F: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
    let entries = F.iter().zip(gains.iter()).map(|(f, g)| {
        let w = (*f as f64 * 0.7) as u32;
        format!("c0 f={f} w={w} g={g}|c1 f={f} w={w} g={g}")
    }).collect::<Vec<_>>().join("|");
    format!("anequalizer=params=%{}%{}", entries.len(), entries)
}

/// Live momentary loudness (LUFS) from the ebur128 visualizer filter, stored as
/// a normalized 0..1000 amplitude so the visualizer pulses to the actual audio.
pub static MUSIC_LOUDNESS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// LUFS → 0..1 envelope (≈ -45 dB silence .. -6 dB loud).
pub fn loudness_to_amp(lufs: f64) -> f32 { (((lufs + 45.0) / 39.0).clamp(0.0, 1.0)) as f32 }
/// Headphone-correction af chunk (AutoEq preset → anequalizer + preamp),
/// empty when off. Composed into every audio spawn via `music_full_af`.
static HEADPHONE_AF: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn headphone_af() -> &'static std::sync::Mutex<String> {
    HEADPHONE_AF.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// AutoEq ParametricEQ → mpv `af` chunk: one stereo anequalizer with every PK
/// filter (bandwidth ≈ fc/Q) plus a preamp volume stage so boosts don't clip.
pub fn hp_af_from_eq(eq: &tulipix_music::headphone_eq::ParametricEq) -> String {
    if eq.filters.is_empty() { return String::new(); }
    let entries = eq.filters.iter().map(|f| {
        let w = (f.fc_hz / f.q.max(0.1)).max(1.0) as u32;
        let (fc, g) = (f.fc_hz as u32, f.gain_db);
        format!("c0 f={fc} w={w} g={g:.1}|c1 f={fc} w={w} g={g:.1}")
    }).collect::<Vec<_>>().join("|");
    let aneq = format!("anequalizer=params=%{}%{}", entries.len(), entries);
    if eq.preamp_db.abs() > 0.05 { format!("{aneq},volume={:.1}", eq.preamp_db) } else { aneq }
}

/// Karaoke mode (np.p4.music.stem, DSP tier): live centre-channel
/// cancellation in the filter chain — removes centre-panned vocals without a
/// model download. 0.85 (not 1.0) keeps a hint of the centre so bass/kick
/// that's also centre-mixed doesn't fully vanish. The MDX-Net ONNX model
/// (`mdx-vocal` in the registry) is the learned upgrade path.
pub static KARAOKE_ON: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
const KARAOKE_AF: &str = "lavfi=[pan=stereo|c0=c0-0.85*c1|c1=c1-0.85*c0]";

/// Append karaoke + the headphone-correction chain + the ebur128 metering
/// filter (labelled `vis`) to the EQ `af`. Empty EQ → hp + meter (or meter).
pub fn music_full_af(eq_af: &str) -> String {
    const VIS: &str = "@vis:ebur128=metadata=1:video=0";
    let hp = headphone_af().lock().map(|g| g.clone()).unwrap_or_default();
    let mut parts: Vec<&str> = Vec::new();
    if KARAOKE_ON.load(std::sync::atomic::Ordering::Relaxed) { parts.push(KARAOKE_AF); }
    if !eq_af.is_empty() { parts.push(eq_af); }
    if !hp.is_empty() { parts.push(&hp); }
    parts.push(VIS);
    parts.join(",")
}

/// Toggle karaoke and push the rebuilt chain to the live player. The session
/// arg on the persistent process is now stale, but the next play compares
/// against the *newly built* af and respawns — settings still win.
pub fn karaoke_toggle(w: &MainWindow) {
    use std::sync::atomic::Ordering;
    let on = !KARAOKE_ON.load(Ordering::Relaxed);
    KARAOKE_ON.store(on, Ordering::Relaxed);
    w.set_music_karaoke_on(on);
    let gains = music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]);
    music_ipc(&["set_property", "af", &music_full_af(&music_eq_af(&gains))]);
    w.set_caps_nudge(if on { "Karaoke on — centre vocals reduced (works best on centre-mixed tracks)." }
                     else { "Karaoke off." }.into());
}

/// Default AutoEq mirror (overridable via the `api.autoeq` setting).
pub const AUTOEQ_DEFAULT: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results";

pub fn autoeq_base() -> String {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.get("api.autoeq").cloned().filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| AUTOEQ_DEFAULT.to_string())
        .trim_end_matches('/').to_string()
}

/// `(name, relpath)` preset index parsed out of AutoEq's INDEX.md — markdown
/// link lines like `- [Sony WH-1000XM4](./oratory1990/over-ear/Sony WH-1000XM4)`.
/// Cached on disk for a week; the file is ~1 MB.
pub fn hp_parse_index(md: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in md.lines() {
        let Some(s) = line.find("[") else { continue };
        let Some(m) = line[s..].find("](") else { continue };
        let name = &line[s + 1..s + m];
        let rest = &line[s + m + 2..];
        let Some(e) = rest.find(')') else { continue };
        let rel = rest[..e].trim_start_matches("./").to_string();
        if !name.is_empty() && !rel.is_empty() && !rel.starts_with("http") {
            out.push((name.to_string(), rel));
        }
    }
    out
}

fn hp_index_cache_path() -> Option<std::path::PathBuf> {
    tulipix_core::paths::data_dir().map(|d| d.join("autoeq_index.md"))
}

/// Fetch (or reuse) the AutoEq index; ~7-day disk cache next to the DBs.
pub async fn hp_index(client: &reqwest::Client) -> Vec<(String, String)> {
    if let Some(p) = hp_index_cache_path() {
        let fresh = std::fs::metadata(&p).and_then(|m| m.modified()).ok()
            .and_then(|t| t.elapsed().ok()).map(|e| e.as_secs() < 7 * 86_400).unwrap_or(false);
        if fresh {
            if let Ok(md) = std::fs::read_to_string(&p) { return hp_parse_index(&md); }
        }
    }
    let url = format!("{}/INDEX.md", autoeq_base());
    let Ok(resp) = client.get(&url)
        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
        .send().await else { return Vec::new(); };
    let Ok(md) = resp.text().await else { return Vec::new(); };
    if let Some(p) = hp_index_cache_path() { let _ = std::fs::write(&p, &md); }
    hp_parse_index(&md)
}

/// Apply (or clear, with `None`) the headphone preset: fetch the ParametricEQ
/// file, convert, store, persist, and hot-swap the live mpv `af`.
pub fn hp_apply(weak: slint::Weak<MainWindow>, preset: Option<(String, String)>) {
    tokio::runtime::Handle::current().spawn(async move {
        let status: String;
        match preset {
            None => {
                if let Ok(mut g) = headphone_af().lock() { g.clear(); }
                save_music_pref("music.hp_preset", "");
                save_music_pref("music.hp_af", "");
                status = "Headphone EQ off.".into();
            }
            Some((name, rel)) => {
                let client = tulipix_core::net::http().clone();
                // AutoEq file layout: {rel}/{basename} ParametricEQ.txt
                let base = rel.rsplit('/').next().unwrap_or(&rel).to_string();
                let url = format!("{}/{}/{} ParametricEQ.txt", autoeq_base(), rel, base);
                let txt = match client.get(&url)
                    .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                    .send().await {
                    Ok(r) if r.status().is_success() => r.text().await.unwrap_or_default(),
                    _ => String::new(),
                };
                let eq = tulipix_music::headphone_eq::parse_autoeq(&txt);
                if eq.filters.is_empty() {
                    status = format!("No AutoEq profile found for {name}.");
                } else {
                    let af = hp_af_from_eq(&eq);
                    if let Ok(mut g) = headphone_af().lock() { *g = af.clone(); }
                    save_music_pref("music.hp_preset", &name);
                    save_music_pref("music.hp_af", &af);
                    status = format!("Headphone EQ: {name} ({} filters).", eq.filters.len());
                }
            }
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_hp_status(status.into());
            apply_music_eq(&w); // hot-swap the live af chain
        });
    });
}

/// Apply the current EQ gains to the live track + reflect them in the UI bands.
pub fn apply_music_eq(w: &MainWindow) {
    let gains = music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]);
    music_ipc(&["set_property", "af", &music_full_af(&music_eq_af(&gains))]);
    let bands: Vec<f32> = gains.iter().map(|g| *g as f32).collect();
    w.set_music_eq_bands(slint::ModelRc::new(slint::VecModel::from(bands)));
}

/// Saved custom EQ profiles, persisted as JSON in Settings.advanced.
pub fn load_eq_customs() -> Vec<(String, Vec<f64>)> {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.get("music.eq.custom")
        .and_then(|j| serde_json::from_str::<Vec<(String, Vec<f64>)>>(j).ok())
        .unwrap_or_default()
}
pub fn save_eq_customs(list: &[(String, Vec<f64>)]) {
    if let Ok(j) = serde_json::to_string(list) { save_music_pref("music.eq.custom", &j); }
}
/// Publish the custom-profile names into the EQ popup.
pub fn populate_eq_customs(w: &MainWindow) {
    let names: Vec<slint::SharedString> = load_eq_customs().into_iter().map(|(n, _)| n.into()).collect();
    w.set_music_eq_custom_names(slint::ModelRc::new(slint::VecModel::from(names)));
}
// music_ipc / video_ipc moved to tulipix_common (playback core).

/// Advance to the next track honoring shuffle + repeat (off/all/one). Called on
/// natural end-of-file and by the Next button.
/// Advance to the next track. If a play_queue has upcoming tracks (Instant Mix
/// or a persisted/manual queue), follow it — pop the front and play it; only
/// fall back to sequential/shuffle/repeat order when the queue is empty
/// (np.p5.music.instant-mix follow + np.p5.music.queue-persist).
/// Sleep-timer "stop after current track" flag — armed from the sleep popup,
/// consumed (one-shot) by the auto-advance paths on natural EOF.
pub static SLEEP_STOP_EOT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// One-shot check: if the end-of-track sleep timer is armed, stop instead of
/// advancing. Returns true when playback was stopped.
pub fn sleep_eot_fired(w: &MainWindow) -> bool {
    if !SLEEP_STOP_EOT.swap(false, std::sync::atomic::Ordering::SeqCst) { return false; }
    w.set_music_playing(false);
    w.set_music_sleep_min(0);
    tracing::info!("music sleep timer: stopped at track end");
    true
}

pub fn advance_music(w: &MainWindow) {
    if sleep_eot_fired(w) { return; }
    // Book mode follows the BOOK: the next chapter in book order from the
    // chapter-queue model — never the shuffle bag or the music queue (user
    // report 2026-07-12: chapter EOF jumped to a random My Music song). Going
    // through the audiobook-play callback keeps the chapter bookkeeping
    // (current-chapter row, progress registration) intact. End of book = stop.
    if w.get_music_player_mode().as_str() == "book" {
        let cur = w.get_music_np_index();
        let rows = w.get_music_book_chapter_rows();
        let mut next = None;
        let mut seen = false;
        for i in 0..rows.row_count() {
            if let Some(r) = rows.row_data(i) {
                if seen { next = Some(r.index); break; }
                if r.index == cur { seen = true; }
            }
        }
        match next {
            Some(p) => w.invoke_music_audiobook_play(p),
            None => w.set_music_playing(false), // book finished
        }
        return;
    }
    // "Repeat one" always re-plays the current track, ignoring the queue.
    if w.get_music_repeat() == "one" { advance_sequential(w); return; }
    queue_advance(w, false);
}

/// The library walk behind an empty queue. `wrap` is the difference between a
/// track ending (repeat "off" means stop at the end of the list) and Next being
/// pressed (which always has to produce a track).
fn advance_or_wrap(w: &MainWindow, wrap: bool) {
    if wrap && !w.get_music_shuffle() && w.get_music_repeat() == "off" {
        let total = w.get_music_np_total();
        if let Some(p) = step_music_pos(w.get_music_np_index(), total, 1) {
            play_music_at(w, p);
        }
        return;
    }
    advance_sequential(w);
}

/// Pop the queue and play what comes out; an empty queue falls back to the
/// library walk. Shuffle pops a RANDOM entry instead of the front, so it
/// shuffles the list you started — the album, playlist or Songs page the queue
/// was built from — rather than escaping into the whole library. Shuffle used
/// to skip the queue entirely, which is why a 40-track mix was built on the
/// first play and then never followed (user report 2026-08-10).
pub fn queue_advance(w: &MainWindow, wrap: bool) {
    let shuffle = w.get_music_shuffle();
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else {
            let _ = weak.upgrade_in_event_loop(move |w| advance_or_wrap(&w, wrap)); return; };
        let popped = if shuffle {
            tulipix_music::queue::pop_random(&pool).await
        } else {
            tulipix_music::queue::pop_first(&pool).await
        };
        match popped {
            Ok(Some(next_id)) => {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    let pos = music_ids().lock().ok()
                        .and_then(|g| g.iter().position(|id| *id == next_id)).map(|p| p as i32);
                    match pos {
                        Some(p) => { play_music_at(&w, p); build_music_queue(&w); }
                        None => advance_or_wrap(&w, wrap),
                    }
                });
            }
            _ => { let _ = weak.upgrade_in_event_loop(move |w| advance_or_wrap(&w, wrap)); }
        }
    });
}

/// Shuffle order state: upcoming indices in shuffled order (`bag`, drained one
/// per advance and refilled with a fresh permutation once empty — every track
/// plays exactly once per cycle) + the indices we came from (`hist`, so Prev
/// under shuffle walks the real played order).
static SHUFFLE_STATE: std::sync::OnceLock<std::sync::Mutex<(Vec<i32>, Vec<i32>)>> = std::sync::OnceLock::new();
pub fn shuffle_state() -> &'static std::sync::Mutex<(Vec<i32>, Vec<i32>)> {
    SHUFFLE_STATE.get_or_init(|| std::sync::Mutex::new((Vec::new(), Vec::new())))
}

/// Next shuffled index. Pops the bag (refilling it with a Fisher–Yates
/// permutation of the library minus `cur` when empty). The history is no longer
/// pushed here — `play_music_at` records every play, shuffled or not.
pub fn shuffle_next(total: i32, cur: i32) -> i32 {
    let Ok(mut g) = shuffle_state().lock() else { return rand_index(total) };
    let (bag, _hist) = &mut *g;
    // A stale bag (library shrank / re-sorted) is rebuilt from scratch.
    if bag.iter().any(|i| *i >= total) { bag.clear(); }
    if bag.is_empty() {
        let mut v: Vec<i32> = (0..total).filter(|i| *i != cur).collect();
        let mut x = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
        for i in (1..v.len()).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            let j = (x % (i as u64 + 1)) as usize;
            v.swap(i, j);
        }
        *bag = v;
    }
    bag.pop().unwrap_or(cur)
}

/// Set while Prev is walking back, so the play it triggers does not push the
/// track it is leaving and trap Prev between the same two tracks forever.
static STEPPING_BACK: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Play `idx` as a step BACKWARDS through the played order.
pub fn play_previous_at(w: &MainWindow, idx: i32) {
    STEPPING_BACK.store(true, std::sync::atomic::Ordering::SeqCst);
    play_music_at(w, idx);
}

/// The previously played index (true played order), if any.
pub fn shuffle_prev_index(total: i32) -> Option<i32> {
    shuffle_state().lock().ok().and_then(|mut g| {
        while let Some(i) = g.1.pop() {
            if i >= 0 && i < total { return Some(i); }
        }
        None
    })
}

/// Reset shuffle order + history (shuffle toggled, or the library rebuilt).
pub fn shuffle_reset() {
    if let Ok(mut g) = shuffle_state().lock() { g.0.clear(); g.1.clear(); }
}

/// True when this library position is an audiobook chapter.
///
/// `music_paths` / `music_ids` cover the whole music SECTION, books included —
/// the Audiobooks tab resolves its chapters by position out of the same list —
/// so every walk over that index space has to filter, or My Music's Next lands
/// in the middle of a book. (User report: the queue "sometimes" played
/// audiobook tracks; sometimes = whenever the walk stepped past the last song
/// before a book folder.)
pub fn is_audiobook_pos(idx: i32) -> bool {
    music_songs().lock().ok()
        .map(|g| g.iter().any(|s| s.pos == idx && s.is_audiobook))
        .unwrap_or(false)
}

/// The next MUSIC position from `idx`, walking `step` (+1 / -1) and wrapping.
/// Returns `None` when the library holds nothing but audiobooks.
pub fn step_music_pos(idx: i32, total: i32, step: i32) -> Option<i32> {
    if total <= 0 { return None; }
    let mut p = idx;
    for _ in 0..total {
        p = (p + step).rem_euclid(total);
        if !is_audiobook_pos(p) { return Some(p); }
    }
    None
}

/// Sequential / shuffle / repeat advance over the library list (the fallback
/// when no queue is active).
pub fn advance_sequential(w: &MainWindow) {
    let total = w.get_music_np_total();
    if total <= 0 { return; }
    let idx = w.get_music_np_index();
    let next = match w.get_music_repeat().as_str() {
        "one" => idx,
        _ if w.get_music_shuffle() && total > 1 => {
            // The bag is over the whole index space, so a book chapter can come
            // out of it — draw again until it does not. Bounded: after `total`
            // tries the library is all books and there is nothing to play.
            let mut pick = shuffle_next(total, idx);
            let mut tries = 0;
            while is_audiobook_pos(pick) && tries < total {
                pick = shuffle_next(total, idx);
                tries += 1;
            }
            if is_audiobook_pos(pick) { w.set_music_playing(false); return; }
            pick
        }
        "all" => match step_music_pos(idx, total, 1) {
            Some(p) => p,
            None => { w.set_music_playing(false); return; }
        },
        _ => { // off: stop at the end of the list
            match step_music_pos(idx, total, 1) {
                // Wrapping past the end means there is no later song left.
                Some(p) if p > idx => p,
                _ => { w.set_music_playing(false); return; }
            }
        }
    };
    play_music_at(w, next);
}

/// Now-playing cover art + accent, resolved off the event loop.
///
/// Both halves of this used to run inline in `play_music_at`:
/// `render_or_cache` shells out to ffmpeg whenever a track's art is not in the
/// thumbnail cache, and `dominant_color` decodes the result. On a cold track
/// that is a third of a second or more of the UI thread doing file work — the
/// pause between one song and the next that reads as the app hanging.
///
/// The previous cover stays on screen until this lands, so the swap fades
/// rather than blanking. Generation-checked on both ends: a quick skip must not
/// paint a stale cover over the track that is actually playing.
///
/// `keep_art` is for audiobook chapters, whose cover comes from the already
/// decoded book cache — only the accent is wanted from here.
fn spawn_np_art(weak: slint::Weak<MainWindow>, path: std::path::PathBuf, gen_id: u64, keep_art: bool) {
    std::thread::spawn(move || {
        let thumb = tulipix_core::thumbs::render_or_cache(
            &path, tulipix_core::thumbs::ThumbSpec {
                kind: tulipix_core::thumbs::ThumbKind::Audio, width: 320, height: 320 })
            .ok().flatten().map(|t| t.path);
        // Dynamic accent from the cover (np.p5.atmusic.art-gradient).
        let accent = thumb.as_deref().and_then(dominant_color)
            .unwrap_or(slint::Color::from_rgb_u8(0xec, 0x48, 0x99));
        // `slint::Image` is not Send — a SharedPixelBuffer is, so the decode
        // happens here and only the buffer crosses to the event loop.
        let px = if keep_art { None } else {
            thumb.as_deref().and_then(|p| image::open(p).ok()).map(|img| {
                let rgba = img.to_rgba8();
                let (w, h) = (rgba.width(), rgba.height());
                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba.as_raw(), w, h)
            })
        };
        if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) != gen_id { return; }
        let _ = weak.upgrade_in_event_loop(move |w| {
            if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) != gen_id { return; }
            w.set_music_np_accent(accent);
            if !keep_art {
                w.set_music_np_art(px.map(slint::Image::from_rgba8).unwrap_or_default());
            }
        });
    });
}

/// Play the music track at `idx`: stop the previous one, spawn a headless mpv
/// with a live IPC control socket, wire a reader thread that streams position /
/// duration / pause / volume into the now-playing bar, and auto-advances on EOF.
pub fn play_music_at(w: &MainWindow, idx: i32) {
    let (path, total) = {
        let Ok(g) = music_paths().lock() else { return; };
        let total = g.len() as i32;
        let Some(p) = g.get(idx as usize).cloned() else { return; };
        (p, total)
    };
    // Record what we are leaving. Every play funnels through here — queue pops,
    // shuffle, a click on a row — so this is the one place that sees the real
    // played order, which is what Prev has to walk. Prev used to step to
    // `index - 1` in the LIBRARY: play an album through the queue and Prev left
    // it for whatever track happened to sort before the current one.
    let stepping_back = STEPPING_BACK.swap(false, std::sync::atomic::Ordering::SeqCst);
    let leaving = w.get_music_np_index();
    if !stepping_back && leaving >= 0 && leaving != idx {
        if let Ok(mut g) = shuffle_state().lock() {
            g.1.push(leaving);
            if g.1.len() > 500 { g.1.remove(0); }
        }
    }
    // Library track — not a YouTube video.
    if let Ok(mut g) = yt_cur_audio().lock() { g.clear(); }
    YT_QUEUE_ACTIVE.store(false, std::sync::atomic::Ordering::Relaxed);
    w.set_music_yt_now_video(false);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    // Session-level mpv flags: the ebur128 meter (+ EQ) so the visualizer
    // pulses to real loudness, then persisted audio config (device / exclusive
    // / gapless / replaygain). A change in any of these respawns the
    // persistent process (np.b2.music.persistent).
    let eq_af = music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]));
    let fade_s = w.get_music_crossfade().clamp(0.0, 12.0) as f64;
    let user_vol = w.get_music_volume().clamp(0.0, 130.0) as f64;
    let mut session_args = vec![format!("--af={}", music_full_af(&eq_af))];
    session_args.extend(music_audio_args(&s));
    // Per-track knobs — IPC properties on reuse, `--flag=value` on spawn.
    // Fade-tracks: launch silent and ramp up below; otherwise start at volume.
    let load_props = vec![
        ("volume".to_string(), format!("{}", if fade_s > 0.0 { 0 } else { user_vol as i32 })),
        ("mute".to_string(), (if w.get_music_muted() { "yes" } else { "no" }).to_string()),
        // Speed never carries across content types: a 2× podcast/audiobook
        // session left the persistent process fast, so the next SONG played
        // at 2× (user report 2026-07-12). Audiobook plays re-apply the book
        // speed over IPC right after this.
        ("speed".to_string(), "1".to_string()),
    ];
    // Cold-start resume: this exact track was mid-play last session → pick up
    // where it left off (one-shot; any other track clears the request).
    let this = music_songs().lock().ok()
        .and_then(|g| g.iter().find(|s| s.pos == idx).map(|s| (s.item_id, s.is_audiobook)));
    let this_id = this.map(|(id, _)| id);
    let mut start_s = None;
    if let Some((rid, rpos)) = resume_pending().lock().ok().and_then(|mut g| g.take()) {
        // Never for audiobook chapters — chapters restart from 0:00 by design
        // (chapter-level resume; a stale pre-decree music.resume could still
        // carry a chapter id + mid-file position).
        if this_id == Some(rid) && rpos > 5.0 && !this.map(|(_, ab)| ab).unwrap_or(false) {
            start_s = Some(rpos);
        }
    }

    // Reader-thread handler — loudness stays in-thread (atomic, no per-frame
    // event-loop hop); everything else drives the now-playing bar.
    let weak = w.as_weak();
    let on_prop = move |name: &str, data: &serde_json::Value| {
        if name == "af-metadata/vis/lavfi.r128.M" {
            if let Some(l) = data.as_str().and_then(|s| s.parse::<f64>().ok()) {
                MUSIC_LOUDNESS.store((loudness_to_amp(l) * 1000.0) as i32, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        let name = name.to_string();
        let data = data.clone();
        let wk = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = wk.upgrade() else { return; };
            match name.as_str() {
                "time-pos" => if let Some(d) = data.as_f64() {
                    w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into());
                    update_lyrics_active(&w);
                    resume_maybe_save(&w, d); // ~15s cadence, np.p5.music.resume
                    // Fade-tracks tail (np.p5.music.fade-tracks): inside the last
                    // `crossfade` seconds ramp mpv's volume to zero; the next
                    // track fades back in, so the transition is smooth even
                    // with one mpv process per track.
                    let cf = w.get_music_crossfade() as f64;
                    let dur = w.get_music_dur() as f64;
                    if cf > 0.0 && dur > cf {
                        use std::sync::atomic::Ordering::Relaxed;
                        let rem = dur - d;
                        if (0.0..=cf).contains(&rem) {
                            FADE_TAIL.store(true, Relaxed);
                            let v = w.get_music_volume().clamp(0.0, 130.0) as f64 * (rem / cf);
                            music_ipc(&["set_property", "volume", &format!("{}", v as i32)]);
                        } else if FADE_TAIL.swap(false, Relaxed) {
                            // Seeked back out of the tail — restore the user volume.
                            let v = w.get_music_volume().clamp(0.0, 130.0) as i32;
                            music_ipc(&["set_property", "volume", &v.to_string()]);
                        }
                    } }
                "duration" => if let Some(d) = data.as_f64() {
                    w.set_music_dur(d as f32); w.set_music_dur_label(fmt_clock(d).into()); }
                "pause"  => if let Some(p) = data.as_bool() { w.set_music_playing(!p); media_set_playing(!p); }
                "volume" => if let Some(d) = data.as_f64() {
                    if !fade_active() { w.set_music_volume(d as f32); } }
                "mute"   => if let Some(m) = data.as_bool() { w.set_music_muted(m); }
                _ => {}
            }
        });
    };
    let eof_weak = w.as_weak();
    let on_eof = move || { let _ = eof_weak.upgrade_in_event_loop(|w| advance_music(&w)); };
    const OBSERVE: &[(u64, &str)] = &[(1, "time-pos"), (2, "duration"), (3, "pause"),
                                      (4, "volume"), (5, "mute"), (6, "af-metadata/vis/lavfi.r128.M")];

    // Persistent transport (np.b2.music.persistent) — default on; set
    // music.persistent=0 to restore spawn-per-track.
    let persist_on = s.advanced.get("music.persistent").map(|v| v != "0").unwrap_or(true);
    if persist_on {
        if let Err(e) = player::play_persistent(player::PersistentLaunch {
            prefix: "tulipix-music",
            mpv_bin: tulipix_core::thumbs::tool_bin("mpv"),
            src: &path,
            session_args,
            load_props,
            start_s,
            observe: OBSERVE,
            generation: my_gen,
        }, on_prop, on_eof) {
            tracing::error!(error = %e, "mpv audio launch failed"); return;
        }
    } else {
        // Legacy spawn-per-track: stop the previous track, spawn fresh.
        stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
        let mut pre_args: Vec<String> = load_props.iter().map(|(k, v)| format!("--{k}={v}")).collect();
        pre_args.extend(session_args);
        if let Some(rp) = start_s { pre_args.push(format!("--start={rp:.0}")); }
        if let Err(e) = player::spawn_audio(player::AudioLaunch {
            prefix: "tulipix-music",
            mpv_bin: tulipix_core::thumbs::tool_bin("mpv"),
            src: &path,
            pre_args,
            observe: OBSERVE,
            generation: my_gen,
        }, on_prop, on_eof) {
            tracing::error!(error = %e, "mpv audio launch failed"); return;
        }
    }

    // Fade-tracks head: ramp 0 → user volume over the configured seconds
    // (np.p5.music.fade-tracks). Gen-checked so a quick skip cancels the ramp.
    if fade_s > 0.0 {
        use std::sync::atomic::Ordering::Relaxed;
        FADE_TAIL.store(false, Relaxed);
        FADE_IN.store(true, Relaxed);
        let fade_gen = my_gen;
        tokio::runtime::Handle::current().spawn(async move {
            const STEPS: u32 = 20;
            let step_ms = ((fade_s * 1000.0) / STEPS as f64) as u64;
            for i in 1..=STEPS {
                if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) != fade_gen { FADE_IN.store(false, Relaxed); return; }
                let v = user_vol * (i as f64 / STEPS as f64);
                music_ipc(&["set_property", "volume", &format!("{}", v as i32)]);
                tokio::time::sleep(std::time::Duration::from_millis(step_ms)).await;
            }
            music_ipc(&["set_property", "volume", &format!("{}", user_vol as i32)]);
            FADE_IN.store(false, Relaxed);
        });
    }

    // Untagged-file ReplayGain (np.p5.music.replaygain): mpv only honours RG
    // tags inside the file; feed the DB-computed gain through
    // `replaygain-fallback` so scanned-but-tagless tracks normalize too.
    {
        let mode = w.get_music_replaygain().to_string();
        let item_id = music_songs().lock().ok()
            .and_then(|g| g.iter().find(|s| s.pos == idx).map(|s| s.item_id));
        if let (Some(id), false) = (item_id, mode == "off") {
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("music").await else { return; };
                let Ok((track, album)) = tulipix_music::replaygain::gains_for(&pool, id).await else { return; };
                let gain = if mode == "album" { album.or(track) } else { track.or(album) };
                if let Some(g) = gain {
                    music_ipc(&["set_property", "replaygain-fallback", &format!("{g:.2}")]);
                }
            });
        }
    }

    // Now-playing metadata — prefer real tags from the Songs store.
    let (title, artist) = music_songs().lock().ok()
        .and_then(|g| g.iter().find(|s| s.pos == idx).map(|s| (s.title.clone(), s.artist.clone())))
        .unwrap_or_else(|| (path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(), String::new()));
    // Audiobook chapter → the book's (possibly custom) cover is the vinyl art,
    // and the folder name drives book mode (second line = book title).
    let book_folder = path.parent().map(|d| d.display().to_string());
    let book_px = book_folder.as_ref()
        .and_then(|f| ab_cover_cache().lock().ok().and_then(|g| g.get(f).cloned()));
    // Cover art + accent land later, off-thread — see `spawn_np_art`. A book
    // chapter already has its cover decoded in the cache, so that one is free.
    if let Some(px) = &book_px { w.set_music_np_art(slint::Image::from_rgba8(px.clone())); }
    spawn_np_art(w.as_weak(), path.clone(), my_gen, book_px.is_some());
    w.set_music_np_title(title.into());
    // Audiobook chapters carry the book title on the second line (and survive
    // chapter auto-advance, which re-enters here); plain tracks show the artist.
    // Book detection uses the DB flag from the songs store — the old cover-
    // cache probe missed whenever the Audiobooks tab hadn't populated yet,
    // which dropped the player back to plain music mode mid-book.
    let is_book = music_songs().lock().ok()
        .and_then(|g| g.iter().find(|s| s.pos == idx).map(|s| s.is_audiobook))
        .unwrap_or(false)
        || book_px.is_some();
    w.set_music_np_sub(match (is_book, &book_folder) {
        (true, Some(f)) => book_display_title(f).into(),
        _ if artist.is_empty() => "Playing from your library".into(),
        _ => artist.into(),
    });
    w.set_music_np_index(idx);
    w.set_music_np_total(total);
    w.set_music_player_mode(if is_book { "book" } else { "music" }.into());
    w.set_music_radio_np_uuid("".into());    // a library track ends any radio LIVE state
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("0:00".into());
    w.set_music_np_loved(false);
    w.set_music_np_stars(0);
    w.set_music_np_album("".into());
    if let Some(id) = current_music_id(w) {
        let weak = w.as_weak();
        let scrobble = w.get_music_scrobble_on();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::queue::record_play(&pool, id, 0).await;
            // Refresh the Home rails so "Recently played" reflects this play
            // immediately (it used to stay stale until app restart).
            let wk2 = weak.clone();
            let _ = wk2.upgrade_in_event_loop(move |w| populate_music_views(w.as_weak()));
            // Scrobble now-playing (np.p5.music.scrobble) — opt-in; the submitter
            // drains the pending queue to Last.fm / ListenBrainz.
            if scrobble {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64).unwrap_or(0);
                let _ = tulipix_music::scrobble::enqueue(&pool, id, now).await;
                // Mirror to ListenBrainz when enabled, then drain its queue.
                let lbz = tulipix_core::settings::Settings::load().ok()
                    .map(|s| s.flags.get("api.listenbrainz").copied().unwrap_or(false)).unwrap_or(false);
                if lbz {
                    let _ = tulipix_music::listenbrainz::enqueue(&pool, id, now).await;
                    submit_scrobbles();
                }
            }
            let row: Option<(i64, i64, Option<String>)> = sqlx::query_as(
                "SELECT COALESCE(tm.loved,0), COALESCE(tm.rating,0), al.title
                 FROM track_meta tm LEFT JOIN albums al ON al.id = tm.album_id
                 WHERE tm.item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            let (loved, stars, album) = row.unwrap_or((0, 0, None));
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_np_loved(loved != 0);
                w.set_music_np_stars(stars as i32);
                // Book mode keeps the second line as the bare book title.
                if w.get_music_player_mode().as_str() != "book" {
                    w.set_music_np_album(album.unwrap_or_default().into());
                }
            });
        });
    }
    // Always refresh lyrics for the new track so the now-playing lyric line
    // (above the seekbar) syncs automatically — independent of the side panel.
    load_music_lyrics(w);

    // Seed the Up-next queue for a fresh My Music session (queue currently empty
    // — i.e. not a playlist/instant-mix/YT/audiobook queue). Sonic-similar when
    // the embedding index is ready, else a plain up-next list. Once per session:
    // subsequent next/prev keep the now-populated queue. My Music only — audiobook
    // (book mode) builds its own chapter queue.
    // A context play (a track picked out of a list) writes the real queue a
    // moment later on the pool thread — seeding a mix here would be overwritten
    // anyway, and for the instant in between it is the wrong queue on screen.
    if w.get_music_player_mode().as_str() == "music"
        && w.get_music_queue_rows().row_count() == 0
        && !CTX_QUEUE_PENDING.load(std::sync::atomic::Ordering::SeqCst)
    {
        build_default_music_queue(w, idx);
    }
}

/// Load the photo at `idx` from the current library list into the viewer,
/// updating the image, label, index and total. Shared by open/next/prev.
// Photo viewer exif/histogram (show_photo_at, exif_rows, format_exif) -> tulipix_sec_photos.

/// Pick a random index in 0..total using a time-seeded xorshift (no rand dep).
pub fn rand_index(total: i32) -> i32 {
    if total <= 1 { return 0; }
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    (x % total as u64) as i32
}

// ── Settings + overlay helpers ─────────────────────────────────────────────

/// Wall-clock HH:MM in the OS-local timezone (chrono::Local reads the system
/// TZ — e.g. Asia/Kolkata → IST +05:30), not UTC.
pub fn clock_now() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

// ════════════════════════════════════════════════════════════════════════════
// YouTube section (np.p4.music.youtube) — helpers, in-memory state, populators.
// ════════════════════════════════════════════════════════════════════════════

const YT_CACHE_KEEP: i64 = 60;   // newest N auto-cached videos kept; rest evicted
/// Hard disk bound on the same cache. 60 audio-only entries land well under
/// this; 60 video ones would not, which is the case the count rule misses.
const YT_CACHE_CAP_BYTES: i64 = 5 * 1024 * 1024 * 1024; // 5 GB
pub const YT_PAGE: usize = 5;        // Home search results revealed per "Load more"

/// Send-safe video row gathered off the UI thread (thumb is a file path).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct YtVidData {
    pub id: String,
    pub channel_id: String, // owning channel ("" = unknown; set for recommended)
    pub title: String,
    pub channel: String,
    pub meta: String,      // "312K views · 8 years ago"
    pub info: String,      // ≤100-char blurb
    pub duration: String,  // "4:12"
    pub dur_s: i64,
    pub thumb: String,     // cached PNG path
    #[serde(default)] pub fmt: String,      // download container badge ("" = none)
    #[serde(default)] pub quality: String,  // download quality badge ("" = none)
    #[serde(default)] pub path: String,     // downloaded file path (per-resolution row id; "" = not a download)
}

#[derive(Default)]
pub struct YtSearchState {
    pub all: Vec<YtVidData>,
    pub nextpage: Option<String>,
    pub shown: usize,
}

pub fn yt_search_state() -> &'static std::sync::Mutex<YtSearchState> {
    static S: OnceLock<std::sync::Mutex<YtSearchState>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(YtSearchState::default()))
}
pub fn yt_vids() -> &'static std::sync::Mutex<std::collections::HashMap<String, YtVidData>> {
    static S: OnceLock<std::sync::Mutex<std::collections::HashMap<String, YtVidData>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
pub fn yt_remember(rows: &[YtVidData]) {
    if let Ok(mut m) = yt_vids().lock() { for r in rows { m.insert(r.id.clone(), r.clone()); } }
}
pub fn yt_lookup(id: &str) -> Option<YtVidData> {
    yt_vids().lock().ok().and_then(|m| m.get(id).cloned())
}

#[allow(dead_code)] // kept: api.piped-instance setting still surfaced; reserved for an optional Piped backend
pub fn piped_instance() -> String {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.get("api.piped-instance").map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://pipedapi.kavin.rocks".to_string())
}
// YouTube preferences live in `tulipix_music::yt_prefs`: same settings file,
// same keys, same meaning of the `-999` sentinel and the nine-entry pin list,
// so the two builds cannot drift on them.
use tulipix_music::yt_prefs::HOME_MAX as YT_HOME_MAX;
pub use tulipix_music::yt_prefs::{
    add_home_channel as yt_add_home_channel, default_res as yt_default_res,
    home_channels as yt_home_channels, remove_home_channel as yt_remove_home_channel,
    store_default_res as yt_store_default_res, store_subs_filter as yt_set_subs_filter,
    subs_filter as yt_subs_filter,
};

pub fn yt_thumb_dir() -> std::path::PathBuf {
    tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("youtube_thumbs")
}
pub fn yt_media_dir() -> std::path::PathBuf {
    tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("youtube_cache")
}
pub fn yt_dl_dir() -> std::path::PathBuf {
    tulipix_core::paths::data_dir().unwrap_or_else(std::env::temp_dir).join("youtube_downloads")
}

pub fn yt_fmt_count(n: i64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1e6) }
    else if n >= 1_000 { format!("{:.0}K", n as f64 / 1e3) }
    else { n.to_string() }
}
pub fn yt_fmt_dur(secs: i64) -> String {
    if secs <= 0 { return String::new(); }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}
pub fn yt_fmt_meta(views: i64, uploaded: &str) -> String {
    let mut parts = Vec::new();
    if views > 0 { parts.push(format!("{} views", yt_fmt_count(views))); }
    if !uploaded.is_empty() { parts.push(uploaded.to_string()); }
    parts.join(" · ")
}
pub fn yt_trunc100(s: &str) -> String {
    if s.chars().count() > 100 { s.chars().take(100).collect::<String>() + "…" } else { s.to_string() }
}

/// Convert a list of Piped videos into rows, fetching thumbnails concurrently.
///
/// Every caller used to be `for v in &videos { rows.push(yt_vid_data(..).await) }`
/// — twenty search results meant twenty HTTP round trips end to end, each
/// waiting on the last. Bounded at [`YT_DECODE_JOBS`], the same cap the decode
/// side uses, and the input order is kept by awaiting the handles in order.
pub async fn yt_vid_data_all(
    client: &reqwest::Client,
    dir: &std::path::Path,
    videos: &[tulipix_music::youtube::piped::Video],
) -> Vec<YtVidData> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(YT_DECODE_JOBS));
    let handles: Vec<_> = videos.iter().map(|v| {
        let (sem, client, dir, v) = (sem.clone(), client.clone(), dir.to_path_buf(), v.clone());
        tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            yt_vid_data(&client, &dir, &v).await
        })
    }).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(r) = h.await { out.push(r); }
    }
    out
}

/// Convert a Piped video into a Send-safe row, fetching its thumbnail.
pub async fn yt_vid_data(client: &reqwest::Client, dir: &std::path::Path,
                     v: &tulipix_music::youtube::piped::Video) -> YtVidData {
    let thumb = if v.thumbnail.is_empty() { String::new() }
        else { tulipix_music::youtube::thumbs::fetch_thumb(client, dir, &v.thumbnail).await.unwrap_or_default() };
    YtVidData {
        id: v.id.clone(), channel_id: String::new(), title: v.title.clone(), channel: v.channel.clone(),
        meta: yt_fmt_meta(v.views, &v.uploaded), info: yt_trunc100(&v.blurb),
        duration: yt_fmt_dur(v.duration), dur_s: v.duration, thumb,
        ..Default::default()
    }
}

pub fn yt_img(path: &str) -> slint::Image {
    // Routes through the shared decode cache (`yt_decode_thumb`), so a thumbnail
    // pre-decoded off the UI thread is just a cheap refcounted wrap here.
    match yt_decode_thumb(path) {
        Some(buf) => slint::Image::from_rgba8(buf),
        None => Default::default(),
    }
}

/// A decoded thumbnail as raw pixels. `slint::Image` is NOT `Send`, but a
/// `SharedPixelBuffer` is — so the expensive PNG decode runs on the tokio worker
/// and only the cheap `Image::from_rgba8` wrap happens on the UI thread.
type YtPixels = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

/// How many bytes of decoded thumbnails the cache may hold.
///
/// **Bytes, not entries.** The cap used to be "512 images", which says nothing
/// about memory: YouTube Music serves square album art up to 4153×4153, so one
/// entry could be 69 MB of RGBA and 512 of them a 5.6 GB ceiling the cache would
/// never notice it had hit. A measured session held 313 of them — 2.4 GB of
/// heap, most of it swapped out. `thumbs::MAX_EDGE` now caps a single image at
/// 4 MB, and this caps the pile.
const YT_THUMB_BUDGET: usize = 192 * 1024 * 1024;

/// Decoded-thumbnail cache: path → pixels, plus insertion order and the running
/// byte total so eviction is oldest-first instead of a wholesale clear.
type YtThumbCache = (
    std::collections::HashMap<String, YtPixels>,
    std::collections::VecDeque<String>,
    usize,
);

/// Bounded decode cache (path → pixels). A given video's thumbnail appears in
/// several lists (home rail, recommended, search, its tab) and survives
/// refreshes; decoding the PNG once and cloning the refcounted buffer avoids
/// repeat decode work.
///
/// A `static` here — rather than the `thread_local!` the track-thumb memo
/// needs — because `SharedPixelBuffer` *is* `Send`. That is the whole point of
/// this type: the decode runs on a tokio worker and only the cheap
/// `Image::from_rgba8` wrap happens on the UI thread.
pub fn yt_thumb_cache() -> &'static std::sync::Mutex<YtThumbCache> {
    static S: OnceLock<std::sync::Mutex<YtThumbCache>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// Bytes one decoded buffer occupies.
fn yt_thumb_bytes(buf: &YtPixels) -> usize {
    buf.width() as usize * buf.height() as usize * 4
}

/// Decode a thumbnail PNG into a Send pixel buffer. Call OFF the UI thread.
///
/// Goes through `thumbs::open_capped`, so an oversized file left by an older
/// build is shrunk on disk the first time it is read and never costs full size
/// again.
pub fn yt_decode_thumb(path: &str) -> Option<YtPixels> {
    if path.is_empty() { return None; }
    if let Ok(c) = yt_thumb_cache().lock() {
        if let Some(buf) = c.0.get(path) { return Some(buf.clone()); }
    }
    let img = tulipix_music::youtube::thumbs::open_capped(std::path::Path::new(path)).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    let mut buf = YtPixels::new(w, h);
    buf.make_mut_bytes().copy_from_slice(img.as_raw());
    if let Ok(mut c) = yt_thumb_cache().lock() {
        let bytes = yt_thumb_bytes(&buf);
        if c.0.insert(path.to_string(), buf.clone()).is_none() {
            c.1.push_back(path.to_string());
            c.2 += bytes;
        }
        while c.2 > YT_THUMB_BUDGET && c.1.len() > 1 {
            let Some(old) = c.1.pop_front() else { break };
            if let Some(dead) = c.0.remove(&old) { c.2 -= yt_thumb_bytes(&dead); }
        }
    }
    Some(buf)
}

/// A Send-able pre-decoded row: every field is `Send`, so the whole `Vec` can
/// cross into `upgrade_in_event_loop`. Built by `yt_videos` on the worker.
pub struct YtRow { pub d: YtVidData, pub index: i32, pub pixels: Option<YtPixels> }

/// Decode rows + their thumbnails into Send-able structs, one at a time, on the
/// calling thread. Only sensible for a handful of rows — see [`yt_videos`].
pub fn yt_videos_blocking(rows: &[YtVidData]) -> Vec<YtRow> {
    rows.iter().enumerate()
        .map(|(i, d)| YtRow { d: d.clone(), index: i as i32, pixels: yt_decode_thumb(&d.thumb) })
        .collect()
}

/// How many thumbnails decode at once.
///
/// PNG decode is CPU-bound, so this is a parallelism cap, not a queue depth. Four
/// matches `home_thumbs_parallel` in the app crate and leaves headroom on a box
/// that has to cap its own build parallelism; at `thumbs::MAX_EDGE` the transient
/// cost is bounded at 4 × 4 MB.
const YT_DECODE_JOBS: usize = 4;

/// Decode rows + their thumbnails into Send-able structs, in parallel.
///
/// This used to be a plain `.map()` over the whole list: one PNG decoded, then
/// the next, on a single thread — and for the `yt_video_model` callers, that
/// single thread was the UI thread, so a channel page froze the window until
/// every thumbnail in it had been decoded. Now each decode is a `spawn_blocking`
/// behind a semaphore, and order is preserved by awaiting the handles in order
/// rather than by finishing in order.
///
/// Already-cached thumbnails short-circuit inside `yt_decode_thumb`, so a
/// re-publish of a list the user has already seen costs a lock and a clone.
pub async fn yt_videos(rows: &[YtVidData]) -> Vec<YtRow> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(YT_DECODE_JOBS));
    let handles: Vec<_> = rows.iter().enumerate().map(|(i, d)| {
        let (sem, d, i) = (sem.clone(), d.clone(), i as i32);
        tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let thumb = d.thumb.clone();
            let pixels = tokio::task::spawn_blocking(move || yt_decode_thumb(&thumb)).await.ok().flatten();
            YtRow { d, index: i, pixels }
        })
    }).collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(r) = h.await { out.push(r); }
    }
    out
}

/// Decode `rows` off the UI thread, then hand the finished model to `set`.
///
/// The alternative — `w.set_x(yt_video_model(&rows))` inside an event-loop
/// closure — decodes every thumbnail on the UI thread before the first pixel is
/// drawn. `set` is a plain `fn`, so a non-capturing closure like
/// `|w, m| w.set_music_yt_results(m)` coerces straight into it.
pub fn yt_publish(
    weak: slint::Weak<MainWindow>,
    rows: Vec<YtVidData>,
    set: fn(&MainWindow, slint::ModelRc<YtVideo>),
) {
    tokio::runtime::Handle::current().spawn(async move {
        let decoded = yt_videos(&rows).await;
        let _ = weak.upgrade_in_event_loop(move |w| set(&w, yt_model(decoded)));
    });
}

/// Watched-fraction per video id, mirrored from `yt_progress` so the (sync, UI
/// thread) model builder can read it without a query. Refreshed by
/// `refresh_yt_progress`; a stale entry only means a bar is a few seconds off.
pub static YT_PROGRESS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f32>>>
    = std::sync::OnceLock::new();
pub fn yt_progress_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, f32>> {
    YT_PROGRESS.get_or_init(Default::default)
}

/// Reload the watched-fraction cache from disk. Cheap (one small table) and
/// safe to call whenever a YouTube surface is about to be rebuilt.
pub async fn refresh_yt_progress() {
    let Ok(pool) = pool_for("youtube").await else { return; };
    let Ok(rows) = tulipix_music::youtube::store::progress_map(&pool).await else { return; };
    if let Ok(mut g) = yt_progress_cache().lock() { *g = rows.into_iter().collect(); }
}

/// Wrap pre-decoded rows into a model — cheap, safe on the UI thread (no decode).
pub fn yt_model(rows: Vec<YtRow>) -> slint::ModelRc<YtVideo> {
    let prog = yt_progress_cache().lock().map(|g| g.clone()).unwrap_or_default();
    let v: Vec<YtVideo> = rows.into_iter().map(|r| {
        let d = r.d;
        let progress = prog.get(&d.id).copied().unwrap_or(0.0);
        YtVideo {
            id: d.id.into(), channel_id: d.channel_id.into(),
            title: d.title.into(), channel: d.channel.into(),
            meta: d.meta.into(), info: d.info.into(), duration: d.duration.into(),
            thumb: r.pixels.map(slint::Image::from_rgba8).unwrap_or_default(), index: r.index,
            fmt: d.fmt.into(), quality: d.quality.into(),
            path: d.path.into(),
            progress,
        }
    }).collect();
    slint::ModelRc::new(slint::VecModel::from(v))
}

/// Convenience: decode + wrap in one call, **on the calling thread**.
///
/// Fine for clearing a list (`&[]`) or a couple of rows. For anything the user
/// will wait on, use [`yt_publish`] — this one blocks whoever calls it, and the
/// callers were all on the UI thread.
pub fn yt_video_model(rows: &[YtVidData]) -> slint::ModelRc<YtVideo> {
    yt_model(yt_videos_blocking(rows))
}

pub fn yt_cached_to_data(c: &tulipix_music::youtube::store::CachedVideo) -> YtVidData {
    YtVidData {
        id: c.video_id.clone(), channel_id: String::new(), title: c.title.clone(), channel: c.channel.clone(),
        meta: String::new(), info: String::new(), duration: yt_fmt_dur(c.duration),
        dur_s: c.duration, thumb: c.thumb_path.clone(),
        fmt: c.fmt.clone(), quality: c.quality.clone(),
        path: c.media_path.clone(),
    }
}

/// One-time YouTube warm-up. The six populate_yt_* calls each decode their rows'
/// thumbnails from disk on the UI thread; re-running them on every tab switch is
/// what made YouTube feel slow. We warm them once (on entering the Music section
/// or first YouTube open) and rely on the per-action refreshers afterwards, so
/// switching in/out of the YouTube tab is instant — like the other tabs.
static YT_WARMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
pub fn warm_youtube(w: &MainWindow) {
    if YT_WARMED.swap(true, std::sync::atomic::Ordering::Relaxed) { return; }
    populate_yt_subs(w);
    populate_yt_cached(w);
    populate_yt_downloads(w);
    populate_yt_recent(w);
    populate_yt_recommended(w);
    populate_yt_playlists(w);
}

/// One-shot guard for the populate bundles that fire on Music-section entry and
/// sub-view switches. Returns true the FIRST time a key is seen (caller should
/// populate), false afterwards. Slint models persist across switches and every
/// data mutation (subscribe/refresh/download/category) calls its own populate
/// directly — bypassing this guard — so re-querying on each switch was pure
/// UI-thread waste. Same idea as warm_youtube, applied to podcasts/audiobooks.
pub fn music_warm_once(key: &str) -> bool {
    static S: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    let set = S.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
    set.lock().map(|mut g| g.insert(key.to_string())).unwrap_or(true)
}

/// Collect EVERY video in a playlist (local items or remote flat list), remember
/// their metadata so the queue panel shows real titles/thumbs, queue them, and
/// start playback. Shared by the playlist card + detail "Play all" buttons.
pub fn yt_play_all_playlist(weak: slint::Weak<MainWindow>, pl_id: i64) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else {
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_pl_playall_busy(false)); return;
        };
        let (source_url, count) = match tulipix_music::youtube::store::get_playlist(&pool, pl_id).await {
            Ok(Some((_, su, vc))) => (su, vc.unwrap_or(0)),
            _ => (None, 0),
        };
        let mut datas: Vec<YtVidData> = if let Some(url) = &source_url {
            let vids = ytdlp_playlist_window(url, 1, count.max(1)).await;
            if !vids.is_empty() {
                vids.iter().map(|v| YtVidData {
                    id: v.id.clone(), title: v.title.clone(), channel: v.channel.clone(),
                    meta: yt_fmt_meta(v.views, &v.uploaded), duration: yt_fmt_dur(v.duration),
                    dur_s: v.duration, ..Default::default()
                }).collect()
            } else {
                tulipix_music::youtube::store::get_playlist_cache(&pool, pl_id).await.unwrap_or_default()
                    .iter().map(yt_channelvid_to_data).collect()
            }
        } else {
            tulipix_music::youtube::store::playlist_items_page(&pool, pl_id, 0, 100000).await.unwrap_or_default()
                .iter().map(|it| YtVidData {
                    id: it.video_id.clone(), title: it.title.clone(), channel: it.channel.clone(),
                    duration: yt_fmt_dur(it.duration), dur_s: it.duration, thumb: it.thumb_path.clone(),
                    ..Default::default()
                }).collect()
        };
        if datas.is_empty() {
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_pl_playall_busy(false)); return;
        }
        // Mark this playlist as the playing one → its card shows a pill sweep.
        let _ = weak.upgrade_in_event_loop(move |w| w.set_music_yt_playing_pl_id(pl_id as i32));
        yt_remember(&datas);
        let ids: Vec<String> = datas.iter().map(|d| d.id.clone()).collect();
        if let Ok(mut g) = yt_queue().lock() { *g = (ids.clone(), 0); }
        // Fetch the FIRST track's thumbnail BEFORE playback so the now-playing
        // card, the cached row, and the queue panel all show art immediately —
        // otherwise it loads after the fact and the first song appears thumbless.
        if datas[0].thumb.is_empty() || !std::path::Path::new(&datas[0].thumb).exists() {
            let client = tulipix_core::net::http().clone();
            let dir = yt_thumb_dir();
            let url = format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", datas[0].id);
            if let Ok(path) = tulipix_music::youtube::thumbs::fetch_thumb(&client, &dir, &url).await {
                if !path.is_empty() {
                    datas[0].thumb = path.clone();
                    if source_url.is_none() {
                        if let Ok(p) = pool_for("youtube").await {
                            let _ = tulipix_music::youtube::store::update_playlist_item_meta(
                                &p, pl_id, &datas[0].id, &datas[0].title, &datas[0].channel, &path, datas[0].dur_s).await;
                        }
                    }
                    yt_remember(&datas);
                }
            }
        }
        yt_play_audio(weak.clone(), ids[0].clone());
        // Background: fetch + cache thumbnails for every queued item that lacks
        // one (YouTube's deterministic hqdefault URL — no yt-dlp call), persist
        // local-playlist rows to the DB, then refresh the queue panel.
        let is_local = source_url.is_none();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let dir = yt_thumb_dir();
            let pool2 = pool_for("youtube").await.ok();
            let mut changed = false;
            for d in datas.iter_mut() {
                if !d.thumb.is_empty() && std::path::Path::new(&d.thumb).exists() { continue; }
                let url = format!("https://i.ytimg.com/vi/{}/hqdefault.jpg", d.id);
                if let Ok(path) = tulipix_music::youtube::thumbs::fetch_thumb(&client, &dir, &url).await {
                    if path.is_empty() { continue; }
                    d.thumb = path.clone();
                    changed = true;
                    if is_local {
                        if let Some(p) = &pool2 {
                            let _ = tulipix_music::youtube::store::update_playlist_item_meta(
                                p, pl_id, &d.id, &d.title, &d.channel, &path, d.dur_s).await;
                        }
                    }
                }
            }
            if changed {
                yt_remember(&datas);
                let _ = weak.upgrade_in_event_loop(|w| if YT_QUEUE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) { build_yt_queue_panel(&w); });
            }
        });
    });
}

/// Drop the cached Recommended picks (memory + disk) so the next populate
/// refetches — used when the Home rail's pinned channels change.
pub fn yt_reco_clear() {
    if let Ok(mut g) = yt_reco().lock() { g.clear(); }
    let _ = std::fs::remove_file(yt_reco_cache_path());
}

pub fn yt_reco() -> &'static std::sync::Mutex<Vec<YtVidData>> {
    static S: OnceLock<std::sync::Mutex<Vec<YtVidData>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Home "Recommended" — latest video from up to 10 subscribed channels. Cached
/// in-memory for the session so it's built once.
/// Home recommendations refresh at most once a day. Order: in-memory (this
/// session) → on-disk cache if <24h old (no network) → otherwise fetch the
/// latest from subs and persist with a timestamp.
const YT_RECO_TTL: u64 = 24 * 60 * 60;

pub fn yt_now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0)
}
pub fn yt_reco_cache_path() -> std::path::PathBuf {
    tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("youtube_reco.json")
}
#[derive(serde::Serialize, serde::Deserialize, Default)]
pub struct YtRecoCache {
    pub fetched: u64,
    pub items: Vec<YtVidData>,
    // Source channel ids the picks were built from. When the Home rail changes,
    // this no longer matches → the cache is ignored and Recommended refetches.
    #[serde(default)] pub sources: Vec<String>,
}

pub fn yt_reco_load() -> Option<YtRecoCache> {
    serde_json::from_str(&std::fs::read_to_string(yt_reco_cache_path()).ok()?).ok()
}
pub fn yt_reco_save(items: &[YtVidData], sources: &[String]) {
    let c = YtRecoCache { fetched: yt_now_secs(), items: items.to_vec(), sources: sources.to_vec() };
    if let Ok(j) = serde_json::to_string(&c) {
        let p = yt_reco_cache_path();
        if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
        let _ = std::fs::write(p, j);
    }
}

pub fn populate_yt_recommended(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let all_subs = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
        // Mirror the Home right rail: pinned channels if any are pinned, else the
        // subscribed channels. The exact set is the cache key.
        let pins = yt_home_channels();
        let chans: Vec<_> = if !pins.is_empty() {
            pins.iter().filter_map(|pid| all_subs.iter().find(|s| &s.channel_id == pid).cloned()).collect()
        } else {
            all_subs.into_iter().filter(|s| s.subscribed).collect()
        };
        let want: Vec<String> = chans.iter().take(10).map(|s| s.channel_id.clone()).collect();
        if want.is_empty() { return; }
        // Reuse the cache only when it was built from the SAME channels and is
        // still fresh; a changed Home rail forces an immediate refetch.
        if let Some(c) = yt_reco_load() {
            if !c.items.is_empty() && c.sources == want && yt_now_secs().saturating_sub(c.fetched) < YT_RECO_TTL {
                let items = c.items.clone();
                yt_remember(&items);
                let items = yt_videos(&items).await;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_yt_recommended(yt_model(items)));
                return;
            }
        }
        let client = tulipix_core::net::http().clone();
        let dir = yt_thumb_dir();
        // Fetch every channel's latest video concurrently instead of serially —
        // 10 yt-dlp spawns + thumbnail downloads in parallel collapse the Home
        // rail load from ~sum to ~max latency. Order is preserved by awaiting the
        // handles in spawn order.
        let handles: Vec<_> = chans.iter().take(10).cloned().map(|s| {
            let client = client.clone();
            let dir = dir.clone();
            tokio::spawn(async move {
                let vids = ytdlp_channel_latest(&s.channel_id, 1).await;
                let v = vids.into_iter().next()?;
                let mut d = yt_vid_data(&client, &dir, &v).await;
                d.channel_id = s.channel_id.clone();
                if d.channel.is_empty() { d.channel = s.title.clone(); }
                Some(d)
            })
        }).collect();
        let mut out: Vec<YtVidData> = Vec::new();
        for h in handles {
            if let Ok(Some(d)) = h.await { out.push(d); }
        }
        if out.is_empty() { return; }
        yt_remember(&out);
        yt_reco_save(&out, &want);
        let out = yt_videos(&out).await;
        let _ = weak.upgrade_in_event_loop(move |w| w.set_music_yt_recommended(yt_model(out)));
    });
}

pub fn populate_yt_recent(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let recents = tulipix_music::youtube::store::list_recent_searches(&pool, 6).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let m: Vec<slint::SharedString> = recents.iter().map(|q| q.clone().into()).collect();
            w.set_music_yt_recent(slint::ModelRc::new(slint::VecModel::from(m)));
        });
    });
}

// Per-page text filters for the context-aware top search (np.p4.music.youtube).
// Set by the `yt-filter` callback, read by the matching populate fn.
pub fn yt_cached_q() -> &'static std::sync::Mutex<String> { static S: OnceLock<std::sync::Mutex<String>> = OnceLock::new(); S.get_or_init(|| std::sync::Mutex::new(String::new())) }
pub fn yt_dls_q()    -> &'static std::sync::Mutex<String> { static S: OnceLock<std::sync::Mutex<String>> = OnceLock::new(); S.get_or_init(|| std::sync::Mutex::new(String::new())) }
pub fn yt_subs_q()   -> &'static std::sync::Mutex<String> { static S: OnceLock<std::sync::Mutex<String>> = OnceLock::new(); S.get_or_init(|| std::sync::Mutex::new(String::new())) }
pub fn yt_pls_q()    -> &'static std::sync::Mutex<String> { static S: OnceLock<std::sync::Mutex<String>> = OnceLock::new(); S.get_or_init(|| std::sync::Mutex::new(String::new())) }
pub fn yt_q_get(m: &std::sync::Mutex<String>) -> String { m.lock().map(|g| g.to_lowercase()).unwrap_or_default() }

pub fn populate_yt_cached(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        // Downloaded videos are permanent + live in the Downloads tab; never list
        // them under Cached even if an earlier stream cached their audio.
        let dl_ids: std::collections::HashSet<String> = tulipix_music::youtube::store::list_downloads(&pool, 100000).await
            .unwrap_or_default().into_iter().map(|d| d.video_id).collect();
        let rows: Vec<YtVidData> = tulipix_music::youtube::store::list_cached(&pool, 200).await
            .unwrap_or_default().iter().map(yt_cached_to_data)
            .filter(|d| !dl_ids.contains(&d.id)).collect();
        yt_remember(&rows);
        // Decode thumbnails here (worker thread), not in the event loop.
        let home = yt_videos(&rows.iter().take(3).cloned().collect::<Vec<_>>()).await;
        let q = yt_q_get(yt_cached_q());
        let shown: Vec<YtVidData> = if q.is_empty() { rows }
            else { rows.into_iter().filter(|d| d.title.to_lowercase().contains(&q) || d.channel.to_lowercase().contains(&q)).collect() };
        let shown = yt_videos(&shown).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_yt_cached(yt_model(shown));
            w.set_music_yt_home_cached(yt_model(home));
        });
    });
}

const YT_DL_LIST_PAGE: i64 = 10;

pub fn populate_yt_downloads(w: &MainWindow) {
    let weak = w.as_weak();
    let sort = w.get_music_yt_downloads_sort().to_string();
    let page = w.get_music_yt_downloads_page().max(0) as i64;
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        // DB returns newest-downloaded first; reorder client-side per the sort pill.
        let mut rows: Vec<YtVidData> = tulipix_music::youtube::store::list_downloads(&pool, 10000).await
            .unwrap_or_default().iter().map(yt_cached_to_data).collect();
        match sort.as_str() {
            "old" => rows.reverse(),
            "az" => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            _ => {} // "new" = DB order (newest first)
        }
        yt_remember(&rows);
        let home: Vec<YtVidData> = rows.iter().take(3).cloned().collect();
        let q = yt_q_get(yt_dls_q());
        let rows: Vec<YtVidData> = if q.is_empty() { rows }
            else { rows.into_iter().filter(|d| d.title.to_lowercase().contains(&q) || d.channel.to_lowercase().contains(&q)).collect() };
        let total = rows.len() as i32;
        let pages = ((rows.len() as i64 + YT_DL_LIST_PAGE - 1) / YT_DL_LIST_PAGE).max(1);
        let page = page.min(pages - 1);
        let start = (page * YT_DL_LIST_PAGE) as usize;
        let pagerows: Vec<YtVidData> = rows.into_iter().skip(start).take(YT_DL_LIST_PAGE as usize).collect();
        // Decode thumbnails here (worker thread), not in the event loop.
        let home = yt_videos(&home).await;
        let pagerows = yt_videos(&pagerows).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_yt_downloads_count(total);
            w.set_music_yt_downloads_pages(pages as i32);
            w.set_music_yt_downloads_page(page as i32);
            w.set_music_yt_downloads(yt_model(pagerows));
            w.set_music_yt_home_downloads(yt_model(home));
        });
    });
}

const YT_PL_PER_PAGE: i64 = 10;

#[derive(Default)]
pub struct YtPlOpen { pub id: i64, pub source_url: Option<String>, pub total: i64 }
pub fn yt_pl_open() -> &'static std::sync::Mutex<YtPlOpen> {
    static S: OnceLock<std::sync::Mutex<YtPlOpen>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(YtPlOpen::default()))
}

pub fn yt_item_to_data(it: &tulipix_music::youtube::store::PlaylistItem) -> YtVidData {
    YtVidData {
        id: it.video_id.clone(), channel_id: String::new(), title: it.title.clone(), channel: it.channel.clone(),
        meta: String::new(), info: String::new(), duration: yt_fmt_dur(it.duration),
        dur_s: it.duration, thumb: it.thumb_path.clone(),
        ..Default::default()
    }
}

/// Load one page (10) of the open playlist into the detail view. Remote playlists
/// fetch windows via yt-dlp (cached); local playlists page the DB and lazily fill
/// missing metadata for just the shown items. Sort/search apply to the loaded page.
pub fn yt_playlist_load(weak: slint::Weak<MainWindow>, page: i64, sort: String, query: String) {
    tokio::runtime::Handle::current().spawn(async move {
        let (id, source_url, total) = { let g = yt_pl_open().lock().unwrap(); (g.id, g.source_url.clone(), g.total) };
        let Ok(pool) = pool_for("youtube").await else { return; };
        let page = page.max(0);
        let offset = page * YT_PL_PER_PAGE;
        let client = tulipix_core::net::http().clone();
        let dir = yt_thumb_dir();
        let mut rows: Vec<YtVidData> = if let Some(url) = &source_url {
            let cache = tulipix_music::youtube::store::get_playlist_cache(&pool, id).await.unwrap_or_default();
            let have: Vec<_> = cache.iter().skip(offset as usize).take(YT_PL_PER_PAGE as usize).cloned().collect();
            if !have.is_empty() {
                have.iter().map(yt_channelvid_to_data).collect()
            } else {
                let vids = ytdlp_playlist_window(url, offset + 1, offset + YT_PL_PER_PAGE).await;
                let mut cv = Vec::with_capacity(vids.len());
                for v in &vids { cv.push(yt_data_to_channelvid(&yt_vid_data(&client, &dir, v).await)); }
                let _ = tulipix_music::youtube::store::set_playlist_cache_window(&pool, id, offset, &cv).await;
                cv.iter().map(yt_channelvid_to_data).collect()
            }
        } else {
            let items = tulipix_music::youtube::store::playlist_items_page(&pool, id, offset, YT_PL_PER_PAGE).await.unwrap_or_default();
            let mut out = Vec::with_capacity(items.len());
            for it in &items {
                if it.title == it.video_id || it.thumb_path.is_empty() {
                    if let Some(v) = ytdlp_video_meta(&it.video_id).await {
                        let d = yt_vid_data(&client, &dir, &v).await;
                        let _ = tulipix_music::youtube::store::update_playlist_item_meta(&pool, id, &it.video_id, &d.title, &d.channel, &d.thumb, d.dur_s).await;
                        out.push(d);
                    } else { out.push(yt_item_to_data(it)); }
                } else { out.push(yt_item_to_data(it)); }
            }
            out
        };
        // Page-local sort + filter.
        let q = query.trim().to_lowercase();
        if !q.is_empty() { rows.retain(|r| r.title.to_lowercase().contains(&q)); }
        match sort.as_str() {
            "title" => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            "duration" => rows.sort_by(|a, b| b.dur_s.cmp(&a.dur_s)),
            _ => {}
        }
        yt_remember(&rows);
        let has_next = (offset + YT_PL_PER_PAGE) < total;
        let sub = format!("{total} videos");
        let decoded = yt_videos(&rows).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_yt_playlist_videos(yt_model(decoded));
            w.set_music_yt_playlist_page((page + 1) as i32);
            w.set_music_yt_playlist_has_next(has_next);
            w.set_music_yt_playlist_sub(sub.into());
        });
    });
}

pub fn populate_yt_playlists(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let pls = tulipix_music::youtube::store::list_playlists(&pool).await.unwrap_or_default();
        let q = yt_q_get(yt_pls_q());
        let rows: Vec<(i64, String, String, i64)> = pls.iter()
            .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q))
            .map(|p| (p.id, p.name.clone(), p.cover.clone().unwrap_or_default(), p.count)).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let m: Vec<YtPlaylist> = rows.iter().enumerate().map(|(i, (id, name, cover, count))| YtPlaylist {
                id: *id as i32, name: name.clone().into(), cover: yt_img(cover),
                sub: format!("{} video{}", count, if *count == 1 { "" } else { "s" }).into(),
                index: i as i32,
            }).collect();
            w.set_music_yt_playlists(slint::ModelRc::new(slint::VecModel::from(m)));
        });
    });
}

const YT_SUBS_PER_PAGE: usize = 15;

pub fn yt_sub_to_model(s: &tulipix_music::youtube::store::Sub, i: usize, in_home: bool) -> YtSub {
    let videos = match s.video_count { Some(n) if n > 0 => format!("{n} videos"), _ => "—".to_string() };
    let subs = match s.sub_count { Some(n) if n > 0 => format!("{} subscribers", yt_fmt_count(n)), _ => String::new() };
    YtSub {
        channel_id: s.channel_id.clone().into(),
        title: s.title.clone().into(),
        avatar: yt_img(&s.avatar_path.clone().unwrap_or_default()),
        count: videos.into(),
        subs: subs.into(),
        subscribed: s.subscribed,
        in_home,
        index: i as i32,
    }
}

/// Sort + paginate the subscriptions and push the page slice (30) + Home rail (10).
pub fn yt_set_subs(w: &MainWindow, subs: &[tulipix_music::youtube::store::Sub]) {
    let sort = w.get_music_yt_subs_sort().to_string();
    let asc = w.get_music_yt_subs_dir() != "desc";
    // Persisted page filter — "sub" shows subscribed channels, "unsub" the rest.
    let filter = yt_subs_filter();
    w.set_music_yt_subs_filter(filter.clone().into());
    let pin_set: std::collections::HashSet<String> = yt_home_channels().into_iter().collect();
    let q = yt_q_get(yt_subs_q());
    let mut sorted: Vec<&tulipix_music::youtube::store::Sub> = subs.iter()
        .filter(|s| if filter == "unsub" { !s.subscribed } else { s.subscribed })
        .filter(|s| q.is_empty() || s.title.to_lowercase().contains(&q))
        .collect();
    // Sort ascending by the chosen key (title as tiebreaker), then reverse for desc.
    match sort.as_str() {
        "videos" => sorted.sort_by(|a, b| a.video_count.unwrap_or(0).cmp(&b.video_count.unwrap_or(0))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
        "subscribers" => sorted.sort_by(|a, b| a.sub_count.unwrap_or(0).cmp(&b.sub_count.unwrap_or(0))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
        // Subscribed first (asc), then unsubscribed; title tiebreaker.
        "status" => sorted.sort_by(|a, b| b.subscribed.cmp(&a.subscribed)
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
        _ => sorted.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }
    if !asc { sorted.reverse(); }
    // Header count reflects only channels you're actually subscribed to (full set).
    w.set_music_yt_sub_count(subs.iter().filter(|s| s.subscribed).count() as i32);
    // Home rail = user-pinned channels (newest first, max 9); default to the
    // most-followed subscriptions when nothing's pinned yet. Built from the full
    // set so the page filter never empties it.
    let pins = yt_home_channels();
    let home_subs: Vec<&tulipix_music::youtube::store::Sub> = if pins.is_empty() {
        subs.iter().filter(|s| s.subscribed).take(YT_HOME_MAX).collect()
    } else {
        pins.iter().filter_map(|pid| subs.iter().find(|s| &s.channel_id == pid)).take(YT_HOME_MAX).collect()
    };
    let home: Vec<YtSub> = home_subs.iter().enumerate().map(|(i, s)| yt_sub_to_model(s, i, true)).collect();
    w.set_music_yt_home_subs(slint::ModelRc::new(slint::VecModel::from(home)));
    let pages = sorted.len().div_ceil(YT_SUBS_PER_PAGE).max(1);
    let page = (w.get_music_yt_subs_page().max(0) as usize).min(pages - 1);
    w.set_music_yt_subs_pages(pages as i32);
    w.set_music_yt_subs_page(page as i32);
    // Warm avatars for this page + the immediate neighbours off the UI thread, so
    // paging stays instant without decoding every one of the (20+) pages at once.
    let win_start = page.saturating_sub(1) * YT_SUBS_PER_PAGE;
    let window: Vec<String> = sorted.iter().skip(win_start).take(YT_SUBS_PER_PAGE * 3)
        .filter_map(|s| s.avatar_path.clone()).collect();
    if !window.is_empty() {
        tokio::runtime::Handle::current().spawn_blocking(move || { for p in &window { yt_decode_thumb(p); } });
    }
    let slice: Vec<YtSub> = sorted.iter().skip(page * YT_SUBS_PER_PAGE).take(YT_SUBS_PER_PAGE)
        .enumerate().map(|(i, s)| yt_sub_to_model(s, i, pin_set.contains(&s.channel_id))).collect();
    w.set_music_yt_subs(slint::ModelRc::new(slint::VecModel::from(slice)));
}

// Subscriptions page: list + display only. No network — counts come from DB
// (filled once at import time by `yt_fetch_sub_meta`).
pub fn populate_yt_subs(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let subs = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| yt_set_subs(&w, &subs));
    });
}

// Channel metadata fetch (avatar + video/subscriber count) with a live progress
// bar. `force = false` fills only channels lacking meta — runs at import time so
// the Subscriptions page stays cheap. `force = true` re-fetches every subscribed
// channel — wired to the page's Refresh button (counts never auto-refresh on
// open). Channels are fetched concurrently (bounded) instead of serially.
pub fn yt_fetch_sub_meta(weak: slint::Weak<MainWindow>, force: bool) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let subs = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
        let need: Vec<String> = if force {
            subs.iter().filter(|s| s.subscribed).map(|s| s.channel_id.clone()).collect()
        } else {
            subs.iter().filter(|s| s.fetched_at.is_none()).map(|s| s.channel_id.clone()).collect()
        };
        if need.is_empty() { return; }
        let total = need.len();
        let client = tulipix_core::net::http().clone();
        let dir = yt_thumb_dir();
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_music_yt_fetch_busy(true); w.set_music_yt_fetch_frac(0.0);
            w.set_music_yt_fetch_msg(format!("Fetching 0 / {total} channels…").into()); });
        // Cap parallel yt-dlp spawns so we don't fork dozens of processes at once.
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(6));
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut set = tokio::task::JoinSet::new();
        for cid in need {
            let (pool, client, dir) = (pool.clone(), client.clone(), dir.clone());
            let (sem, done, weak) = (sem.clone(), done.clone(), weak.clone());
            set.spawn(async move {
                let _permit = sem.acquire().await;
                if let Some((avatar_url, followers, video_count)) = ytdlp_channel_meta(&cid).await {
                    let avatar = if avatar_url.is_empty() { None }
                        else { tulipix_music::youtube::thumbs::fetch_thumb(&client, &dir, &avatar_url).await.ok() };
                    let _ = tulipix_music::youtube::store::set_sub_meta(&pool, &cid, avatar.as_deref(), Some(video_count), Some(followers)).await;
                } else if !force {
                    // First-time fill: stamp 0/0 so we don't retry forever. A forced
                    // refresh that fails leaves the previous counts untouched.
                    let _ = tulipix_music::youtube::store::set_sub_meta(&pool, &cid, None, Some(0), Some(0)).await;
                }
                let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                let frac = d as f32 / total as f32;
                if d % 3 == 0 || d == total {
                    let subs_now = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        yt_set_subs(&w, &subs_now);
                        w.set_music_yt_fetch_frac(frac);
                        w.set_music_yt_fetch_msg(format!("Fetching {d} / {total} channels…").into());
                    });
                }
            });
        }
        while set.join_next().await.is_some() {}
        let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_fetch_busy(false));
    });
}

/// Render the current search stash (first `shown` items) into the Home center.
pub fn yt_render_search(w: &MainWindow) {
    let (rows, more): (Vec<YtVidData>, bool) = {
        let st = yt_search_state().lock().unwrap();
        let shown = st.shown.min(st.all.len());
        (st.all[..shown].to_vec(), shown < st.all.len() || st.nextpage.is_some())
    };
    yt_publish(w.as_weak(), rows, |w, m| w.set_music_yt_results(m));
    w.set_music_yt_results_more(more);
    w.set_music_yt_busy(false);
}

// Channel-scoped search stash — separate from the cached "latest" list so the
// latest videos are never clobbered; results page 10 at a time up to 20.
#[derive(Default)]
pub struct YtChSearch { pub all: Vec<YtVidData>, pub shown: usize }
pub fn yt_ch_search_state() -> &'static std::sync::Mutex<YtChSearch> {
    static S: OnceLock<std::sync::Mutex<YtChSearch>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(YtChSearch::default()))
}
pub fn yt_render_channel_search(w: &MainWindow) {
    let (rows, more): (Vec<YtVidData>, bool) = {
        let st = yt_ch_search_state().lock().unwrap();
        let shown = st.shown.min(st.all.len());
        (st.all[..shown].to_vec(), shown < st.all.len())
    };
    yt_publish(w.as_weak(), rows, |w, m| w.set_music_yt_channel_results(m));
    w.set_music_yt_channel_results_more(more);
    w.set_music_yt_busy(false);
}

pub async fn yt_dlp_fetch_audio(id: &str, dir: &std::path::Path) -> Option<String> {
    let _ = std::fs::create_dir_all(dir);
    let out = dir.join(format!("{id}.opus"));
    if out.exists() { return Some(out.to_string_lossy().into_owned()); }
    let url = format!("https://www.youtube.com/watch?v={id}");
    let tmpl = dir.join(format!("{id}.%(ext)s"));
    let _ = tokio::process::Command::new(tulipix_core::ytdlp::bin())
        .arg("-f").arg("bestaudio").arg("-x").arg("--audio-format").arg("opus")
        .arg("--no-playlist").args(tulipix_core::ytdlp::common_args())
        .arg("-o").arg(&tmpl).arg(&url).no_window().status().await;
    if out.exists() { Some(out.to_string_lossy().into_owned()) } else { None }
}

/// Resolve a direct best-audio stream URL (no download) so playback can start
/// instantly — mpv streams the googlevideo URL while we cache the file in the
/// background (np.p4.music.youtube — instant audio, video-style streaming).
pub async fn yt_dlp_stream_url(id: &str) -> Option<String> {
    let url = format!("https://www.youtube.com/watch?v={id}");
    let out = tokio::process::Command::new(tulipix_core::ytdlp::bin())
        .arg("-g").arg("-f").arg("bestaudio/best").arg("--no-playlist")
        .args(tulipix_core::ytdlp::common_args()).arg(&url)
        .no_window()
        .output().await.ok()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if tulipix_core::ytdlp::is_access_error(&stderr) {
            tracing::warn!(%stderr, "youtube: yt-dlp was refused — update it, or set cookies");
        }
        return None;
    }
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
}

pub fn yt_data_to_channelvid(d: &YtVidData) -> tulipix_music::youtube::store::ChannelVid {
    tulipix_music::youtube::store::ChannelVid {
        video_id: d.id.clone(), title: d.title.clone(), channel: d.channel.clone(),
        meta: d.meta.clone(), info: d.info.clone(), thumb_path: d.thumb.clone(), duration: d.dur_s,
    }
}
pub fn yt_channelvid_to_data(c: &tulipix_music::youtube::store::ChannelVid) -> YtVidData {
    YtVidData {
        id: c.video_id.clone(), channel_id: String::new(), title: c.title.clone(), channel: c.channel.clone(),
        meta: c.meta.clone(), info: c.info.clone(), duration: yt_fmt_dur(c.duration),
        dur_s: c.duration, thumb: c.thumb_path.clone(),
        ..Default::default()
    }
}

// ── Download queue (sequential, with progress) ──────────────────────────────
#[derive(Clone)]
pub struct YtDlJobData {
    pub id: String, pub title: String, pub channel: String, pub thumb: String,
    pub height: i64, pub frac: f32, pub status: String,
}
pub fn yt_dl_jobs() -> &'static std::sync::Mutex<Vec<YtDlJobData>> {
    static S: OnceLock<std::sync::Mutex<Vec<YtDlJobData>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
static YT_DL_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn yt_dl_refresh(weak: &slint::Weak<MainWindow>) {
    let rows: Vec<YtDlJobData> = yt_dl_jobs().lock().map(|g| g.clone()).unwrap_or_default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        // The in-flight download (first row) drives the per-card Save progress bar.
        let (aid, afrac) = rows.first().map(|j| (j.id.clone(), j.frac)).unwrap_or_default();
        w.set_music_yt_dl_active_id(aid.into());
        w.set_music_yt_dl_active_frac(afrac);
        let m: Vec<YtDlJob> = rows.iter().map(|j| YtDlJob {
            title: j.title.clone().into(), thumb: yt_img(&j.thumb),
            status: j.status.clone().into(), frac: j.frac,
        }).collect();
        w.set_music_yt_dl_jobs(slint::ModelRc::new(slint::VecModel::from(m)));
    });
}

pub fn parse_ytdlp_pct(line: &str) -> Option<f32> {
    let l = line.trim();
    if !l.starts_with("[download]") { return None; }
    let p = l.find('%')?;
    let start = l[..p].rfind(' ')?;
    l[start..p].trim().parse::<f32>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
}

pub fn yt_dl_set(id: &str, frac: f32, status: &str) {
    if let Ok(mut g) = yt_dl_jobs().lock() {
        if let Some(j) = g.iter_mut().find(|j| j.id == id) { j.frac = frac; j.status = status.to_string(); }
    }
}

pub fn yt_dl_enqueue(weak: slint::Weak<MainWindow>, job: YtDlJobData) {
    if let Ok(mut g) = yt_dl_jobs().lock() {
        // Keyed by (video, resolution) so the same video can queue at several
        // qualities at once; only an identical resolution is a no-op duplicate.
        if g.iter().any(|j| j.id == job.id && j.height == job.height) { return; }
        g.push(job);
    }
    yt_dl_refresh(&weak);
    if YT_DL_ACTIVE.swap(true, std::sync::atomic::Ordering::AcqRel) { return; }
    tokio::runtime::Handle::current().spawn(async move {
        loop {
            let job = { yt_dl_jobs().lock().ok().and_then(|g| g.first().cloned()) };
            let Some(job) = job else { break; };
            let dir = yt_dl_dir();
            let _ = std::fs::create_dir_all(&dir);
            yt_dl_set(&job.id, 0.0, "Starting…");
            yt_dl_refresh(&weak);
            let path = yt_dl_run(&job, &dir, &weak).await;
            if let Some(path) = path {
                if let Ok(pool) = pool_for("youtube").await {
                    let kind = if job.height < 0 { "audio" } else { "video" };
                    let (fmt, quality) = if job.height < 0 { ("OPUS", "Audio".to_string()) }
                        else if job.height == 0 { ("MKV", "Best".to_string()) }
                        else { ("MKV", format!("{}p", job.height)) };
                    let _ = tulipix_music::youtube::store::record_download(
                        &pool, &job.id, &job.title, &job.channel, &job.thumb, &path, 0, kind, fmt, &quality).await;
                }
            }
            if let Ok(mut g) = yt_dl_jobs().lock() { g.retain(|j| !(j.id == job.id && j.height == job.height)); }
            yt_dl_refresh(&weak);
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_downloads(&w));
        }
        YT_DL_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    });
}

pub async fn yt_dl_run(job: &YtDlJobData, dir: &std::path::Path, weak: &slint::Weak<MainWindow>) -> Option<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let url = format!("https://www.youtube.com/watch?v={}", job.id);
    // Per-resolution stem so different qualities of one video are distinct files
    // (e.g. `<id>.audio.opus`, `<id>.best.mkv`, `<id>.720p.mkv`) instead of one
    // shared `<id>.<ext>` that each new download would overwrite.
    let suffix = if job.height < 0 { "audio".to_string() }
        else if job.height == 0 { "best".to_string() }
        else { format!("{}p", job.height) };
    let stem = format!("{}.{}", job.id, suffix);
    let tmpl = dir.join(format!("{}.%(ext)s", stem));
    let mut cmd = tokio::process::Command::new(tulipix_core::ytdlp::bin());
    cmd.no_window();
    cmd.args(tulipix_core::ytdlp::common_args());
    let expected = if job.height < 0 {
        cmd.arg("-f").arg("bestaudio").arg("-x").arg("--audio-format").arg("opus");
        dir.join(format!("{}.opus", stem))
    } else {
        let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
        let fmt = if job.height == 0 { "bestvideo+bestaudio/best".to_string() }
            else { format!("bestvideo[height<=?{0}][vcodec^=vp9]+bestaudio/bestvideo[height<=?{0}]+bestaudio/best[height<=?{0}]", job.height) };
        cmd.arg("-f").arg(fmt).arg("--merge-output-format").arg("mkv");
        // Only pin --ffmpeg-location to an absolute, existing binary; a bare name
        // ("ffmpeg") is NOT resolved against PATH by yt-dlp and silently breaks the
        // merge, leaving split .f###.m4a/.mp4 fragments and no .mkv → never recorded.
        if ff.is_absolute() && ff.exists() { cmd.arg("--ffmpeg-location").arg(&ff); }
        dir.join(format!("{}.mkv", stem))
    };
    cmd.arg("--newline").arg("--no-playlist").arg("-o").arg(&tmpl).arg(&url)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        let mut last = -1i32;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(frac) = parse_ytdlp_pct(&line) {
                let pct = (frac * 100.0) as i32;
                if pct != last { last = pct; yt_dl_set(&job.id, frac, &format!("Downloading {pct}%")); yt_dl_refresh(weak); }
            } else if line.contains("[Merger]") || line.contains("Merging") {
                yt_dl_set(&job.id, 0.99, "Merging…"); yt_dl_refresh(weak);
            }
        }
    }
    let _ = child.wait().await;
    if expected.exists() { return Some(expected.to_string_lossy().into_owned()); }
    // Fallback: the merged file should be `{stem}.<ext>`. yt-dlp names split
    // streams `{stem}.f###.<ext>`, so match by this resolution's stem first;
    // failing that, pick the largest `{stem}.*` file that isn't an in-progress
    // fragment. Scoping to `stem` keeps one resolution from grabbing another's file.
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut best: Option<(u64, std::path::PathBuf)> = None;
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.file_stem().and_then(|s| s.to_str()) == Some(stem.as_str()) {
                return Some(p.to_string_lossy().into_owned());
            }
            if name.starts_with(&format!("{}.", stem)) && !name.ends_with(".part") && !name.ends_with(".ytdl") {
                let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
                if best.as_ref().map(|(b, _)| sz > *b).unwrap_or(true) { best = Some((sz, p)); }
            }
        }
        if let Some((_, p)) = best { return Some(p.to_string_lossy().into_owned()); }
    }
    None
}

// ── yt-dlp browsing backend (reliable, replaces flaky Piped at runtime) ──────
pub async fn ytdlp_json(args: Vec<String>) -> Option<serde_json::Value> {
    let out = tokio::process::Command::new(tulipix_core::ytdlp::bin())
        .args(tulipix_core::ytdlp::common_args())
        .args(&args)
        .no_window().output().await.ok()?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if tulipix_core::ytdlp::is_access_error(&stderr) {
            tracing::warn!(%stderr, "youtube: yt-dlp was refused — update it, or set cookies");
        }
    }
    serde_json::from_slice(&out.stdout).ok()
}

pub fn yt_pick_thumb(v: &serde_json::Value) -> String {
    if let Some(t) = v.get("thumbnail").and_then(|x| x.as_str()) { if !t.is_empty() { return t.to_string(); } }
    if let Some(arr) = v.get("thumbnails").and_then(|x| x.as_array()) {
        for t in arr.iter().rev() { if let Some(u) = t.get("url").and_then(|x| x.as_str()) { return u.to_string(); } }
    }
    String::new()
}

/// Channel avatar: prefer a thumbnail whose id mentions "avatar", else first.
pub fn yt_pick_avatar(v: &serde_json::Value) -> String {
    if let Some(arr) = v.get("thumbnails").and_then(|x| x.as_array()) {
        for t in arr { if t.get("id").and_then(|x| x.as_str()).map(|s| s.contains("avatar")).unwrap_or(false) {
            if let Some(u) = t.get("url").and_then(|x| x.as_str()) { return u.to_string(); } } }
        if let Some(u) = arr.first().and_then(|t| t.get("url")).and_then(|x| x.as_str()) { return u.to_string(); }
    }
    yt_pick_thumb(v)
}

pub fn yt_entry_to_video(e: &serde_json::Value) -> tulipix_music::youtube::piped::Video {
    use tulipix_music::youtube::piped::Video;
    Video {
        id: e.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        title: e.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        channel: e.get("channel").or_else(|| e.get("uploader")).and_then(|x| x.as_str()).unwrap_or("").to_string(),
        duration: e.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64,
        views: e.get("view_count").and_then(|x| x.as_i64()).unwrap_or(0),
        uploaded: String::new(),
        blurb: e.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        thumbnail: yt_pick_thumb(e),
        is_short: false,
    }
}

/// Which backend fetches YouTube listings — Settings → Music → YouTube.
///
/// `auto` (the default, and what the section always did) asks Piped first and
/// falls through to yt-dlp when it does not answer. The other two pin the
/// backend: Piped instances are fast but go down, yt-dlp always works but is a
/// process spawn per query, and which trade you want depends on your network.
pub fn yt_fetcher() -> String {
    tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("music.yt.fetcher"))
        .filter(|v| v == "piped" || v == "ytdlp")
        .unwrap_or_else(|| "auto".into())
}

/// Search YouTube through whichever backend the fetcher setting names.
pub async fn yt_fetch_search(query: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    use tulipix_music::youtube::piped;
    let mut vids = match yt_fetcher().as_str() {
        "ytdlp" => return ytdlp_search(query, n).await,
        "piped" => {
            let client = tulipix_core::net::http().clone();
            piped::search_strict(&client, &piped_instance(), query).await
                .map(|p| p.videos).unwrap_or_default()
        }
        _ => {
            let client = tulipix_core::net::http().clone();
            piped::search(&client, &piped_instance(), query).await
                .map(|p| p.videos).unwrap_or_default()
        }
    };
    vids.truncate(n);
    vids
}

pub async fn ytdlp_search(query: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    let q = format!("ytsearch{n}:{query}");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(), q]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    tulipix_music::youtube::piped::without_shorts(vids)
}

/// Latest `n` videos of a channel via yt-dlp (cheap — flat, windowed to n).
pub async fn ytdlp_channel_latest(id: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    let url = format!("https://www.youtube.com/channel/{id}/videos");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-end".into(), n.to_string(), url]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    tulipix_music::youtube::piped::without_shorts(vids)
}

/// Top `n` most-watched videos for a channel. Pulls a flat window of recent
/// uploads (which carry `view_count`), drops Shorts, sorts by views desc.
pub async fn ytdlp_channel_popular(id: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    // YouTube's "popular" sort (legacy `sort=p`), which yt-dlp's channel-tab
    // extractor honours — the server returns entries already ordered by views.
    // Flat mode omits `view_count`, so the earlier local `sort_by(views)` was a
    // no-op (all zeros) and Popular came back identical to Latest; trust the
    // server order here and only re-sort when counts are actually present.
    let url = format!("https://www.youtube.com/channel/{id}/videos?view=0&sort=p&flow=grid");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-end".into(), (n * 3).to_string(), url]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    let mut vids = tulipix_music::youtube::piped::without_shorts(vids);
    // Stable sort: keeps the server's popularity order when counts are missing,
    // sharpens it when yt-dlp does surface view_count.
    if vids.iter().any(|v| v.views > 0) { vids.sort_by(|a, b| b.views.cmp(&a.views)); }
    vids.truncate(n);
    vids
}

/// Search within a single channel.
pub async fn ytdlp_channel_search(id: &str, query: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    let q = query.trim().replace(' ', "%20");
    let url = format!("https://www.youtube.com/channel/{id}/search?query={q}");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-end".into(), n.to_string(), url]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    tulipix_music::youtube::piped::without_shorts(vids)
}

/// Remote playlist metadata: (title, video_count).
pub async fn ytdlp_playlist_meta(url: &str) -> Option<(String, i64)> {
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-items".into(), "0".into(), url.to_string()]).await?;
    let title = j.get("title").and_then(|x| x.as_str()).unwrap_or("Imported playlist").to_string();
    let count = j.get("playlist_count").and_then(|x| x.as_i64()).unwrap_or(0);
    Some((title, count))
}

/// One window [start..=end] (1-based) of a remote playlist's videos.
pub async fn ytdlp_playlist_window(url: &str, start: i64, end: i64) -> Vec<tulipix_music::youtube::piped::Video> {
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-start".into(), start.to_string(), "--playlist-end".into(), end.to_string(), url.to_string()]).await
        else { return vec![]; };
    j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default()
}

/// Flat metadata for a single video id.
pub async fn ytdlp_video_meta(id: &str) -> Option<tulipix_music::youtube::piped::Video> {
    let url = format!("https://www.youtube.com/watch?v={id}");
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(), url]).await?;
    Some(yt_entry_to_video(&j))
}

/// Channel metadata: (avatar_url, follower_count, video_count).
pub async fn ytdlp_channel_meta(id: &str) -> Option<(String, i64, i64)> {
    let url = format!("https://www.youtube.com/channel/{id}");
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-items".into(), "0".into(), url]).await?;
    let followers = j.get("channel_follower_count").and_then(|x| x.as_i64()).unwrap_or(0);
    let video_count = j.get("playlist_count").and_then(|x| x.as_i64()).unwrap_or(0);
    Some((yt_pick_avatar(&j), followers, video_count))
}

/// Resolve any channel URL (/@handle, /channel/UC…, /c/…, /user/…, or a video
/// URL) into (channel_id, title) via yt-dlp.
pub async fn ytdlp_resolve_channel(url: &str) -> Option<(String, String)> {
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-items".into(), "0".into(), url.to_string()]).await?;
    let cid = j.get("channel_id").and_then(|x| x.as_str())
        .or_else(|| j.get("uploader_id").and_then(|x| x.as_str()))
        .or_else(|| j.get("id").and_then(|x| x.as_str()))
        .unwrap_or("").to_string();
    if !cid.starts_with("UC") { return None; }
    let title = j.get("channel").and_then(|x| x.as_str())
        .or_else(|| j.get("uploader").and_then(|x| x.as_str()))
        .or_else(|| j.get("title").and_then(|x| x.as_str()))
        .unwrap_or("").to_string();
    Some((cid, title))
}

/// Play a downloaded file through the in-app mpv player (bottom/mini/zen),
/// setting now-playing metadata from the given strings.
pub fn yt_cur_audio() -> &'static std::sync::Mutex<String> {
    static S: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// Active YouTube audio play-queue (video ids + current index). Empty = no queue;
/// a single tap clears it, Play-all fills it, EOF auto-advances to the next.
pub fn yt_queue() -> &'static std::sync::Mutex<(Vec<String>, usize)> {
    static S: OnceLock<std::sync::Mutex<(Vec<String>, usize)>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new((Vec::new(), 0)))
}

/// True while the visible Up-next panel reflects the YouTube queue (set on yt
/// playback, cleared when a library track plays). Lets the shared queue panel +
/// row-click route to YouTube instead of the music library.
pub static YT_QUEUE_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Fill the player's Up-next panel from the current YouTube queue (titles +
/// channel + thumb from the in-memory video cache). `index` = queue position.
pub fn build_yt_queue_panel(w: &MainWindow) {
    let (ids, _cur) = yt_queue().lock().map(|g| (g.0.clone(), g.1)).unwrap_or_default();
    let cur_id = yt_cur_audio().lock().map(|g| g.clone()).unwrap_or_default();
    let ids: Vec<String> = if ids.is_empty() { if cur_id.is_empty() { vec![] } else { vec![cur_id] } } else { ids };
    let rows: Vec<MusicSongRow> = ids.iter().enumerate().map(|(i, id)| {
        let m = yt_lookup(id).unwrap_or_default();
        MusicSongRow {
            thumb: yt_img(&m.thumb),
            title: if m.title.is_empty() { "Video".into() } else { m.title.into() },
            artist: m.channel.into(),
            duration: m.duration.into(),
            index: i as i32,
        }
    }).collect();
    w.set_music_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Stream + play one YouTube video's audio (cached file preferred), caching the
/// opus in the background. Shared by single taps and queue advance.
pub fn yt_play_audio(weak: slint::Weak<MainWindow>, id: String) {
    tokio::runtime::Handle::current().spawn(async move {
        let dir = yt_media_dir();
        let meta = yt_lookup(&id).unwrap_or_default();
        let (title, channel, thumb) = (meta.title.clone(), meta.channel.clone(), meta.thumb.clone());
        // Pick up where this video was left, if it was left anywhere.
        let resume = match pool_for("youtube").await {
            Ok(pool) => tulipix_music::youtube::store::progress_of(&pool, &id).await.unwrap_or(0.0),
            Err(_) => 0.0,
        };
        let cached = dir.join(format!("{id}.opus"));
        if cached.exists() {
            let (p2, id2, t, c, th) = (cached.to_string_lossy().into_owned(), id.clone(), title.clone(), channel.clone(), thumb.clone());
            let _ = weak.upgrade_in_event_loop(move |w| yt_play_inapp(&w, id2, p2, t, c, th, resume));
            return;
        }
        if let Some(stream_url) = yt_dlp_stream_url(&id).await {
            let (u2, id2, t, c, th) = (stream_url, id.clone(), title.clone(), channel.clone(), thumb.clone());
            let _ = weak.upgrade_in_event_loop(move |w| yt_play_inapp(&w, id2, u2, t, c, th, resume));
        }
        let weak2 = weak.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Some(path) = yt_dlp_fetch_audio(&id, &dir).await {
                if let Ok(pool) = pool_for("youtube").await {
                    let _ = tulipix_music::youtube::store::record_cached(
                        &pool, &id, &meta.title, &meta.channel, &meta.thumb, &path, meta.dur_s).await;
                    // Two caps, because one of them alone is not a bound: the
                    // count keeps the list short, the byte cap keeps the disk
                    // honest when the cached files are video rather than audio.
                    for (_v, p) in tulipix_music::youtube::store::evict_cached_over(&pool, YT_CACHE_KEEP).await.unwrap_or_default() {
                        let _ = std::fs::remove_file(&p);
                    }
                    for (_v, p) in tulipix_music::youtube::store::evict_cached_over_bytes(&pool, YT_CACHE_CAP_BYTES).await.unwrap_or_default() {
                        let _ = std::fs::remove_file(&p);
                    }
                }
                let _ = weak2.upgrade_in_event_loop(|w| populate_yt_cached(&w));
            }
        });
    });
}

/// EOF reached on a queued track → play the next one (no-op if no queue/at end).
pub fn yt_rand_nanos() -> u32 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos()).unwrap_or(0)
}

/// Manual transport skip inside the YouTube queue (wraps at both ends).
/// `forward` = next track, else previous; `shuffle` picks a random next.
pub fn yt_queue_jump(weak: slint::Weak<MainWindow>, forward: bool, shuffle: bool) {
    let id = {
        let mut g = match yt_queue().lock() { Ok(g) => g, Err(_) => return };
        let len = g.0.len();
        if len == 0 { return; }
        let idx = if forward && shuffle && len > 1 {
            let mut n = (yt_rand_nanos() as usize) % len;
            if n == g.1 { n = (n + 1) % len; }
            n
        } else if forward {
            (g.1 + 1) % len
        } else {
            (g.1 + len - 1) % len
        };
        g.1 = idx;
        g.0[idx].clone()
    };
    yt_play_audio(weak, id);
}

/// EOF reached on a queued track → play the next one. Honours shuffle and
/// repeat-all (wrap); repeat-one is handled by mpv `loop-file` so EOF never
/// fires there. Returns false (→ stop) only at a hard end with repeat off.
pub fn yt_queue_advance(weak: slint::Weak<MainWindow>) -> bool {
    if let Some(w) = weak.upgrade() {
        if sleep_eot_fired(&w) { return false; }
    }
    let (shuffle, repeat) = weak.upgrade()
        .map(|w| (w.get_music_shuffle(), w.get_music_repeat().to_string()))
        .unwrap_or((false, "off".to_string()));
    let next = {
        let mut g = match yt_queue().lock() { Ok(g) => g, Err(_) => return false };
        let len = g.0.len();
        if len == 0 { return false; }
        if shuffle && len > 1 {
            let mut n = (yt_rand_nanos() as usize) % len;
            if n == g.1 { n = (n + 1) % len; }
            g.1 = n;
        } else if g.1 + 1 < len {
            g.1 += 1;
        } else if repeat == "all" {
            g.1 = 0;
        } else {
            return false;
        }
        g.0[g.1].clone()
    };
    yt_play_audio(weak, next);
    true
}

static YT_VID_POS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Watch a video fullscreen: stops the in-app audio, plays the muxed stream via
/// mpv (starting at `start` secs), tracks position, and on close resumes the
/// in-app audio for the same video at the position the video stopped.
pub fn yt_watch_video(weak: slint::Weak<MainWindow>, id: String, height: i64, start: f64) {
    // Stop in-app audio (kill the music mpv, invalidate its reader, clear UI).
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
    let _ = weak.upgrade_in_event_loop(|w| w.set_music_playing(false));
    tokio::runtime::Handle::current().spawn(async move {
        let fmt = if height <= 0 { "best".to_string() } else { format!("best[height<=?{height}]/best") };
        let url = format!("https://www.youtube.com/watch?v={id}");
        let stream = match tokio::process::Command::new(tulipix_core::ytdlp::bin())
            .arg("-g").arg("-f").arg(&fmt).arg("--no-playlist")
            .args(tulipix_core::ytdlp::common_args()).arg(&url)
            .no_window().output().await {
            Ok(o) => String::from_utf8_lossy(&o.stdout).lines().next().map(|l| l.to_string()).filter(|l| !l.is_empty()),
            Err(_) => None,
        };
        let Some(stream) = stream else { return; };
        let sock = mpv_ipc::endpoint("tulipix-yt-video");
        mpv_ipc::cleanup(&sock);
        YT_VID_POS.store(start as i64, std::sync::atomic::Ordering::Relaxed);
        let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
        cmd.no_window();
        // Maximized normal window (keeps the WM top bar), not borderless fullscreen.
        cmd.arg("--window-maximized=yes").arg("--force-window=immediate").arg("--title=Tulipix — YouTube")
            .arg(format!("--input-ipc-server={}", sock.display()));
        if start > 1.0 { cmd.arg(format!("--start={}", start as i64)); }
        cmd.arg(&stream);
        mpv_die_with_parent(&mut cmd);
        let Ok(_child) = cmd.spawn() else { return; };
        let weak2 = weak.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let mut conn = None;
            for _ in 0..50 {
                if let Ok(s) = mpv_ipc::connect(&sock) { conn = Some(s); break; }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if let Some(mut stream) = conn {
                let _ = stream.write_all(b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n");
                let rd = BufReader::new(stream);
                for line in rd.lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        if v["event"] == "property-change" && v["name"] == "time-pos" {
                            if let Some(d) = v["data"].as_f64() { YT_VID_POS.store(d as i64, std::sync::atomic::Ordering::Relaxed); }
                        }
                    }
                }
            }
            // mpv exited → resume in-app audio at the last video position.
            let pos = YT_VID_POS.load(std::sync::atomic::Ordering::Relaxed) as f64;
            let id2 = id.clone();
            let _ = weak2.upgrade_in_event_loop(move |w| {
                let weak3 = w.as_weak();
                tokio::runtime::Handle::current().spawn(async move {
                    let dir = yt_media_dir();
                    if let Some(path) = yt_dlp_fetch_audio(&id2, &dir).await {
                        let m = yt_lookup(&id2).unwrap_or_default();
                        // A watched video also lands in the Cached tab (parity with
                        // audio playback — record the cached opus + its metadata).
                        if let Ok(pool) = pool_for("youtube").await {
                            let _ = tulipix_music::youtube::store::record_cached(
                                &pool, &id2, &m.title, &m.channel, &m.thumb, &path, m.dur_s).await;
                        }
                        let (t, c, th, idd) = (m.title, m.channel, m.thumb, id2.clone());
                        let _ = weak3.upgrade_in_event_loop(move |w| { yt_play_inapp(&w, idd, path, t, c, th, pos); populate_yt_cached(&w); });
                    }
                });
            });
        });
    });
}

/// Play a locally-downloaded video file in a windowed mpv. No yt-dlp, no network,
/// no audio caching — the file is already permanent in the Downloads tab.
pub fn yt_play_local_video(weak: slint::Weak<MainWindow>, path: String) {
    // Stop the in-app audio first (same as yt_watch_video).
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
    let _ = weak.upgrade_in_event_loop(|w| w.set_music_playing(false));
    let sock = mpv_ipc::endpoint("tulipix-yt-video");
    mpv_ipc::cleanup(&sock);
    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
    cmd.no_window();
    cmd.arg("--window-maximized=yes").arg("--force-window=immediate").arg("--title=Tulipix — YouTube")
        .arg(format!("--input-ipc-server={}", sock.display()))
        .arg(&path);
    mpv_die_with_parent(&mut cmd);
    let _ = cmd.spawn();
}

pub fn yt_play_inapp(w: &MainWindow, id: String, path: String, title: String, sub: String, thumb: String, start: f64) {
    // The cards need to know which video is current, and only this function
    // knows — the id is moved into the global on the next line.
    w.set_music_yt_np_id(id.clone().into());
    if let Ok(mut g) = yt_cur_audio().lock() { *g = id; }
    w.set_music_yt_now_video(true);
    // Reflect the YouTube queue in the shared Up-next panel + route row clicks.
    YT_QUEUE_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
    w.set_music_yt_pl_playall_busy(false); // playback started → clear the loading fill
    build_yt_queue_panel(w);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
    let mut pre_args = vec![format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32)];
    if start > 1.0 { pre_args.push(format!("--start={}", start as i64)); }
    if w.get_music_muted() { pre_args.push("--mute=yes".into()); }
    let eq_af = music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]));
    pre_args.push(format!("--af={}", music_full_af(&eq_af)));
    pre_args.extend(music_device_args());
    // Property reader → transport UI; on EOF advance the play-queue or stop.
    let weak = w.as_weak();
    let on_prop = move |name: &str, data: &serde_json::Value| {
        if name == "af-metadata/vis/lavfi.r128.M" {
            if let Some(l) = data.as_str().and_then(|s| s.parse::<f64>().ok()) {
                MUSIC_LOUDNESS.store((loudness_to_amp(l) * 1000.0) as i32, std::sync::atomic::Ordering::Relaxed);
            }
            return;
        }
        let name = name.to_string();
        let data = data.clone();
        let wk = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = wk.upgrade() else { return; };
            match name.as_str() {
                "time-pos" => if let Some(d) = data.as_f64() {
                    w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into());
                    // Persist the watch position roughly every 10s of playback
                    // rather than on every tick — mpv reports time-pos about
                    // once a second and this is a disk write.
                    let dur = w.get_music_dur() as f64;
                    if d as i64 % 10 == 0 && dur > 0.0 {
                        let vid = w.get_music_yt_np_id().to_string();
                        if !vid.is_empty() {
                            tokio::runtime::Handle::current().spawn(async move {
                                if let Ok(pool) = pool_for("youtube").await {
                                    let _ = tulipix_music::youtube::store::save_progress(&pool, &vid, d, dur).await;
                                }
                                refresh_yt_progress().await;
                            });
                        }
                    }
                }
                "duration" => if let Some(d) = data.as_f64() { w.set_music_dur(d as f32); w.set_music_dur_label(fmt_clock(d).into()); }
                "pause"  => if let Some(p) = data.as_bool() { w.set_music_playing(!p); }
                "volume" => if let Some(d) = data.as_f64() { w.set_music_volume(d as f32); }
                "mute"   => if let Some(m) = data.as_bool() { w.set_music_muted(m); }
                _ => {}
            }
        });
    };
    let eof_weak = w.as_weak();
    let on_eof = move || {
        let wk = eof_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if !yt_queue_advance(wk.clone()) {
                if let Some(w) = wk.upgrade() { w.set_music_playing(false); }
            }
        });
    };
    if let Err(e) = player::spawn_audio(player::AudioLaunch {
        prefix: "tulipix-music",
        mpv_bin: tulipix_core::thumbs::tool_bin("mpv"),
        src: std::path::Path::new(&path),
        pre_args,
        observe: &[(1, "time-pos"), (2, "duration"), (3, "pause"),
                   (4, "volume"), (5, "mute"), (6, "af-metadata/vis/lavfi.r128.M")],
        generation: my_gen,
    }, on_prop, on_eof) {
        tracing::error!(error = %e, "yt mpv launch failed"); return;
    }
    w.set_music_np_accent(thumb.is_empty().then(|| slint::Color::from_rgb_u8(0xef, 0x44, 0x44))
        .unwrap_or_else(|| dominant_color(std::path::Path::new(&thumb)).unwrap_or(slint::Color::from_rgb_u8(0xef, 0x44, 0x44))));
    w.set_music_np_title(title.into());
    w.set_music_np_sub(if sub.is_empty() { "YouTube".into() } else { sub.into() });
    w.set_music_np_art(yt_img(&thumb));
    w.set_music_player_mode("music".into());
    // A YouTube track is played in music mode but is not in the library, so it
    // has no lyrics row of its own — and must not inherit the last song's.
    clear_music_lyrics(w);
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("0:00".into());
}
