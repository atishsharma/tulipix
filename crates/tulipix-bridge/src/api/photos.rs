//! Photos section — the whole Dart-facing contract in four symbols.
//!
//! The Slint page it replaces (`ui/page_photos.slint` + `tulipix-sec-photos`)
//! declares 23 properties and ~40 callbacks, because Slint has no store: every
//! value the UI can read has to be a declared property. Flutter has state
//! management, so the ephemeral half — selection, clipboard, which tab is
//! blinking, the album picker's open/closed — stays in Dart and never crosses
//! the FFI boundary. What crosses is one snapshot, one command, one event
//! stream, and a lazy thumbnail resolver.

use anyhow::Result;
use crate::db::photos_pool;
use crate::frb_generated::StreamSink;
use flutter_rust_bridge::frb;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Tiles fetched per page. `ShowMore` grows the window by another one of these.
const PAGE: i64 = 240;
/// Grid thumbnail tier. `tulipix_photos::thumbs::SIZES` also renders 512/1024
/// for the viewer; the grid never needs those.
const GRID_DIM: u32 = 256;

// ---------------------------------------------------------------- state ----

#[derive(Debug, Clone)]
pub struct PhotoTile {
    pub item_id: i64,
    pub path: String,
    /// Absolute path to the 256px JPEG, or "" when it has not been rendered
    /// yet — the tile widget calls `photos_ensure_thumb` when it scrolls into
    /// view, which is the lazy pass `visible-hint` drives in the Slint build.
    pub thumb: String,
    pub label: String,
    /// Unix seconds, 0 = no EXIF date (the "Undated" bucket).
    pub taken_at: i64,
    pub starred: bool,
    pub archived: bool,
    pub trashed: bool,
    pub width: i64,
    pub height: i64,
}

/// One date bucket of the timeline, e.g. "May 2026". Empty when the grid is
/// sorted by something other than date.
#[derive(Debug, Clone)]
pub struct PhotoGroup {
    pub label: String,
    /// Indices into `PhotosState::tiles`, so a tile is never sent twice.
    pub tiles: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct FolderRow {
    pub name: String,
    pub path: String,
    pub count: i64,
}

#[derive(Debug, Clone)]
pub struct AlbumCard {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub cover: String,
    pub smart: bool,
}

#[derive(Debug, Clone)]
pub struct PhotosState {
    pub category: String,
    pub query: String,
    pub sort_mode: String,
    pub sort_dir: String,
    /// Total matching the current category + query, before paging.
    pub item_count: i64,
    /// How many rows remain past the end of `tiles`.
    pub more_count: i64,
    pub folder_count: i64,
    /// 0 when not viewing a single album.
    pub album_id: i64,
    pub tiles: Vec<PhotoTile>,
    pub groups: Vec<PhotoGroup>,
    pub folders: Vec<FolderRow>,
    pub albums: Vec<AlbumCard>,
}

// -------------------------------------------------------------- commands ----

#[derive(Debug, Clone)]
pub enum PhotosCmd {
    /// Re-read the current view. Sent on mount and after an external change.
    Refresh,
    /// One of: recent, starred, archive, trash, places, album, library.
    SetCategory { name: String },
    Search { query: String },
    /// mode: date | name | size. dir: desc | asc.
    SetSort { mode: String, dir: String },
    ShowMore,
    OpenAlbum { album_id: i64 },
    Star { item_id: i64, starred: bool },
    Archive { item_ids: Vec<i64>, archived: bool },
    Trash { item_ids: Vec<i64> },
    Restore { item_ids: Vec<i64> },
    AlbumNew { name: String },
    AlbumRename { album_id: i64, name: String },
    AlbumDelete { album_id: i64 },
    AlbumAdd { album_id: i64, item_ids: Vec<i64> },
    AddFolder { path: String },
    Scan,
}

#[derive(Debug, Clone)]
pub enum PhotosEvent {
    ScanStarted { root: String },
    ScanFinished { scanned: i64, inserted: i64, updated: i64, missing: i64 },
    ScanFailed { message: String },
}

// --------------------------------------------------------------- session ----

/// What the grid is currently showing. Lives here rather than in Dart because
/// every command has to re-query against it, and a round trip per keystroke is
/// exactly the shape this bridge exists to avoid.
///
/// `frb(ignore)`: it is private state, not part of the contract. Without it the
/// generator emits codecs (and a `Session::default()` wire fn) for a private
/// struct with private fields, which cannot compile.
#[frb(ignore)]
#[derive(Debug, Clone)]
struct Session {
    category: String,
    query: String,
    sort_mode: String,
    sort_dir: String,
    limit: i64,
    album_id: i64,
}

impl Default for Session {
    fn default() -> Self {
        Self {
            category: "recent".into(),
            query: String::new(),
            sort_mode: "date".into(),
            sort_dir: "desc".into(),
            limit: PAGE,
            album_id: 0,
        }
    }
}

fn session() -> &'static Mutex<Session> {
    static S: std::sync::OnceLock<Mutex<Session>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(Session::default()))
}

fn events() -> &'static Mutex<Vec<StreamSink<PhotosEvent>>> {
    static E: std::sync::OnceLock<Mutex<Vec<StreamSink<PhotosEvent>>>> =
        std::sync::OnceLock::new();
    E.get_or_init(|| Mutex::new(Vec::new()))
}

fn emit(event: PhotosEvent) {
    if let Ok(mut sinks) = events().lock() {
        // `add` fails once Dart has cancelled the subscription. Dropping those
        // sinks here is the only place they get collected — the page cancels on
        // dispose but has no way to say so across the boundary.
        sinks.retain(|s| s.add(event.clone()).is_ok());
    }
}

// ------------------------------------------------------------- exported ----

/// Apply one command and return the resulting snapshot. Every mutation goes
/// through here, so Dart never has to guess what a write did to the view.
pub async fn photos_dispatch(cmd: PhotosCmd) -> Result<PhotosState> {
    let pool = photos_pool().await?;

    match cmd {
        PhotosCmd::Refresh => {}
        PhotosCmd::SetCategory { name } => {
            let mut s = lock();
            // A tab switch is a new view, so the page window resets with it.
            s.limit = PAGE;
            if name != "album" {
                s.album_id = 0;
            }
            s.category = name;
        }
        PhotosCmd::Search { query } => {
            let mut s = lock();
            s.limit = PAGE;
            s.query = query;
        }
        PhotosCmd::SetSort { mode, dir } => {
            let mut s = lock();
            s.sort_mode = mode;
            s.sort_dir = dir;
        }
        PhotosCmd::ShowMore => lock().limit += PAGE,
        PhotosCmd::OpenAlbum { album_id } => {
            let mut s = lock();
            s.limit = PAGE;
            s.category = "album".into();
            s.album_id = album_id;
        }
        PhotosCmd::Star { item_id, starred } => {
            tulipix_photos::star::set(pool, item_id, starred).await?;
        }
        PhotosCmd::Archive { item_ids, archived } => {
            tulipix_photos::archive::set(pool, &item_ids, archived).await?;
        }
        PhotosCmd::Trash { item_ids } => {
            tulipix_photos::trash::soft_delete(pool, &item_ids).await?;
        }
        PhotosCmd::Restore { item_ids } => {
            tulipix_photos::trash::restore(pool, &item_ids).await?;
        }
        PhotosCmd::AlbumNew { name } => {
            tulipix_photos::albums::create(pool, &name).await?;
        }
        PhotosCmd::AlbumRename { album_id, name } => {
            tulipix_photos::albums::rename(pool, album_id, &name).await?;
        }
        PhotosCmd::AlbumDelete { album_id } => {
            tulipix_photos::albums::delete(pool, album_id).await?;
            let mut s = lock();
            if s.album_id == album_id {
                s.album_id = 0;
                s.category = "recent".into();
            }
        }
        PhotosCmd::AlbumAdd { album_id, item_ids } => {
            tulipix_photos::albums::add_items(pool, album_id, &item_ids).await?;
        }
        PhotosCmd::AddFolder { path } => {
            add_watched_folder(Path::new(&path));
            scan_watched(pool).await;
        }
        PhotosCmd::Scan => scan_watched(pool).await,
    }

    snapshot(pool).await
}

/// Scan progress. Registering a second sink is harmless — every sink gets
/// every event.
#[frb(sync)]
pub fn photos_events(sink: StreamSink<PhotosEvent>) {
    if let Ok(mut sinks) = events().lock() {
        sinks.push(sink);
    }
}

/// Render the grid thumbnail for one item if the cache does not already hold
/// it, and return its absolute path. Called by the tile widget when it scrolls
/// into view, so a cold library paints progressively instead of blocking the
/// first snapshot on thousands of JPEG decodes.
pub async fn photos_ensure_thumb(item_id: i64) -> Result<Option<String>> {
    let pool = photos_pool().await?;
    let row: Option<(String, i64, i64)> =
        sqlx::query_as("SELECT abs_path, mtime, size FROM items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;
    let Some((abs, mtime, size)) = row else { return Ok(None) };
    let src = PathBuf::from(&abs);
    let Some(dest) = tulipix_photos::thumbs::thumb_path(&src, mtime, size as u64, GRID_DIM) else {
        return Ok(None);
    };
    if dest.exists() {
        return Ok(Some(dest.to_string_lossy().into_owned()));
    }
    // render_all is CPU-bound (and shells out to ffmpeg for HEIC/RAW), so it
    // must not run on the bridge's async worker.
    let rendered = tokio::task::spawn_blocking(move || tulipix_photos::thumbs::render_all(&src))
        .await?
        .is_ok();
    Ok(if rendered && dest.exists() {
        Some(dest.to_string_lossy().into_owned())
    } else {
        None
    })
}

// ------------------------------------------------------------- internals ----

fn lock() -> std::sync::MutexGuard<'static, Session> {
    // A poisoned session lock means a previous command panicked mid-write. The
    // session is six scalars with no cross-field invariant, so the recovered
    // value is still usable and dropping the grid is the worse outcome.
    session().lock().unwrap_or_else(|e| e.into_inner())
}

async fn snapshot(pool: &sqlx::SqlitePool) -> Result<PhotosState> {
    let s = lock().clone();

    let (tiles, total) = load_tiles(pool, &s).await?;
    let groups = if s.sort_mode == "date" { group_by_month(&tiles) } else { Vec::new() };
    let folders = folders(pool).await?;
    let albums = albums(pool).await?;

    Ok(PhotosState {
        item_count: total,
        more_count: (total - tiles.len() as i64).max(0),
        folder_count: folders.len() as i64,
        category: s.category,
        query: s.query,
        sort_mode: s.sort_mode,
        sort_dir: s.sort_dir,
        album_id: s.album_id,
        tiles,
        groups,
        folders,
        albums,
    })
}

/// Rows the grid needs, plus the unpaged total for the "show more" count.
async fn load_tiles(pool: &sqlx::SqlitePool, s: &Session) -> Result<(Vec<PhotoTile>, i64)> {
    // A search is its own ranking, so it takes the FTS path and the category
    // filter narrows the hits rather than the other way round.
    if !s.query.trim().is_empty() {
        let hits = tulipix_photos::search::query(pool, &s.query, None, None, s.limit + 1).await?;
        let ids: Vec<i64> = hits.iter().map(|h| h.item_id).collect();
        let mut rows = rows_for_ids(pool, &ids).await?;
        // FTS returns by rank; preserve that order.
        rows.sort_by_key(|t| ids.iter().position(|id| *id == t.item_id).unwrap_or(usize::MAX));
        let total = rows.len() as i64;
        rows.truncate(s.limit as usize);
        return Ok((rows, total));
    }

    let (where_sql, bind_album) = category_filter(&s.category);
    let order = order_by(&s.sort_mode, &s.sort_dir);

    let count_sql = format!(
        "SELECT COUNT(*) FROM items LEFT JOIN photo_meta pm ON pm.item_id = items.id WHERE {where_sql}"
    );
    let mut cq = sqlx::query_scalar::<sqlx::Sqlite, i64>(&count_sql);
    if bind_album {
        cq = cq.bind(s.album_id);
    }
    let total = cq.fetch_one(pool).await?;

    let sql = format!("{TILE_SELECT} WHERE {where_sql} ORDER BY {order} LIMIT ?");
    let mut q = sqlx::query_as::<sqlx::Sqlite, TileRow>(&sql);
    if bind_album {
        q = q.bind(s.album_id);
    }
    let rows = q.bind(s.limit).fetch_all(pool).await?;

    Ok((rows.into_iter().map(into_tile).collect(), total))
}

const TILE_SELECT: &str = "SELECT items.id, items.abs_path, items.mtime, items.size, \
     COALESCE(pm.taken_at, 0), COALESCE(pm.starred, 0), COALESCE(pm.archived, 0), \
     CASE WHEN pm.deleted_at IS NULL THEN 0 ELSE 1 END, \
     COALESCE(pm.width, 0), COALESCE(pm.height, 0) \
     FROM items LEFT JOIN photo_meta pm ON pm.item_id = items.id";

/// id, abs_path, mtime, size, taken_at, starred, archived, trashed, w, h —
/// in the column order `TILE_SELECT` declares.
type TileRow = (i64, String, i64, i64, i64, i64, i64, i64, i64, i64);

fn into_tile(row: TileRow) -> PhotoTile {
    let (id, abs, mtime, size, taken, starred, archived, trashed, w, h) = row;
    let src = Path::new(&abs);
    // Only report a thumb the cache already holds. Rendering here would
    // make the first snapshot of a cold library take minutes.
    let thumb = tulipix_photos::thumbs::thumb_path(src, mtime, size as u64, GRID_DIM)
        .filter(|p| p.exists())
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    PhotoTile {
        item_id: id,
        label: src
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        path: abs,
        thumb,
        taken_at: taken,
        starred: starred != 0,
        archived: archived != 0,
        trashed: trashed != 0,
        width: w,
        height: h,
    }
}

async fn rows_for_ids(pool: &sqlx::SqlitePool, ids: &[i64]) -> Result<Vec<PhotoTile>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    // sqlx has no array binding for SQLite; the ids come from our own FTS query
    // and are i64, so formatting them in is not an injection surface.
    let list = ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    let sql = format!("{TILE_SELECT} WHERE items.id IN ({list})");
    let rows = sqlx::query_as::<sqlx::Sqlite, TileRow>(&sql).fetch_all(pool).await?;
    Ok(rows.into_iter().map(into_tile).collect())
}

/// The WHERE clause per tab, and whether it carries an `album_id` placeholder.
/// Mirrors `tulipix_sec_photos::category_path_set`.
fn category_filter(category: &str) -> (&'static str, bool) {
    match category {
        "starred" => (
            "items.missing_since IS NULL AND pm.deleted_at IS NULL AND pm.starred = 1",
            false,
        ),
        "archive" => (
            "items.missing_since IS NULL AND pm.deleted_at IS NULL AND pm.archived = 1",
            false,
        ),
        "trash" => ("pm.deleted_at IS NOT NULL", false),
        "places" => (
            "items.missing_since IS NULL AND pm.deleted_at IS NULL \
             AND pm.gps_lat IS NOT NULL AND pm.gps_lon IS NOT NULL",
            false,
        ),
        "album" => (
            "items.missing_since IS NULL AND pm.deleted_at IS NULL \
             AND items.id IN (SELECT item_id FROM album_items WHERE album_id = ?)",
            true,
        ),
        // recent, and anything unrecognised: the default library view.
        _ => (
            "items.missing_since IS NULL AND pm.deleted_at IS NULL \
             AND COALESCE(pm.archived, 0) = 0",
            false,
        ),
    }
}

fn order_by(mode: &str, dir: &str) -> String {
    let dir = if dir == "asc" { "ASC" } else { "DESC" };
    match mode {
        "name" => format!("items.abs_path {dir}"),
        "size" => format!("items.size {dir}"),
        // Undated photos sort last in both directions rather than clumping at
        // the top of an ascending list, which is what a raw NULL would do.
        _ => format!("pm.taken_at {dir} NULLS LAST, items.mtime {dir}"),
    }
}

fn group_by_month(tiles: &[PhotoTile]) -> Vec<PhotoGroup> {
    let mut out: Vec<PhotoGroup> = Vec::new();
    for (i, t) in tiles.iter().enumerate() {
        let label = month_label(t.taken_at);
        match out.last_mut() {
            Some(g) if g.label == label => g.tiles.push(i as u32),
            _ => out.push(PhotoGroup { label, tiles: vec![i as u32] }),
        }
    }
    out
}

fn month_label(unix: i64) -> String {
    if unix <= 0 {
        return "Undated".into();
    }
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0)
        .map(|d| d.format("%B %Y").to_string())
        .unwrap_or_else(|| "Undated".into())
}

async fn albums(pool: &sqlx::SqlitePool) -> Result<Vec<AlbumCard>> {
    let rows: Vec<(i64, String, Option<String>, i64, Option<String>)> = sqlx::query_as(
        "SELECT a.id, a.name, a.smart_rule, \
                (SELECT COUNT(*) FROM album_items ai WHERE ai.album_id = a.id), \
                (SELECT items.abs_path FROM album_items ai \
                   JOIN items ON items.id = ai.item_id \
                  WHERE ai.album_id = a.id ORDER BY ai.sort_key LIMIT 1) \
         FROM albums a ORDER BY a.updated DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, smart, count, cover)| AlbumCard {
            id,
            name,
            count,
            cover: cover.unwrap_or_default(),
            smart: smart.is_some(),
        })
        .collect())
}

async fn folders(pool: &sqlx::SqlitePool) -> Result<Vec<FolderRow>> {
    let mut out = Vec::new();
    for dir in load_watched_folders() {
        let prefix = format!("{}%", dir.to_string_lossy());
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM items LEFT JOIN photo_meta pm ON pm.item_id = items.id \
             WHERE items.abs_path LIKE ? AND items.missing_since IS NULL AND pm.deleted_at IS NULL",
        )
        .bind(&prefix)
        .fetch_one(pool)
        .await
        .unwrap_or(0);
        out.push(FolderRow {
            name: dir
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| dir.to_string_lossy().into_owned()),
            path: dir.to_string_lossy().into_owned(),
            count,
        });
    }
    Ok(out)
}

async fn scan_watched(pool: &sqlx::SqlitePool) {
    for dir in load_watched_folders() {
        emit(PhotosEvent::ScanStarted { root: dir.to_string_lossy().into_owned() });
        let lib = tulipix_core::libraries::Library {
            id: dir.to_string_lossy().into_owned(),
            path: dir.clone(),
            section: tulipix_core::libraries::Section::Photos,
            last_scan: None,
            item_count: 0,
            size_bytes: 0,
            exclude_globs: Vec::new(),
            cadence_override: Some(tulipix_core::libraries::ScanCadence::Manual),
            realtime_notify: false,
        };
        match tulipix_photos::scan::scan_library(pool, &lib).await {
            Ok(st) => emit(PhotosEvent::ScanFinished {
                scanned: st.scanned as i64,
                inserted: st.inserted as i64,
                updated: st.updated as i64,
                missing: st.missing as i64,
            }),
            Err(e) => emit(PhotosEvent::ScanFailed { message: e.to_string() }),
        }
    }
}

// The watched-folder list is a JSON array of paths in the config dir. This is
// `tulipix_common::{load,add}_watched_folder` minus the slint dependency that
// crate carries — same file, same format, so both builds see one list.
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
    fn month_label_buckets_undated_separately() {
        assert_eq!(month_label(0), "Undated");
        assert_eq!(month_label(-1), "Undated");
        // 2026-05-15T00:00:00Z
        assert_eq!(month_label(1_778_803_200), "May 2026");
    }

    #[test]
    fn groups_are_contiguous_runs_not_a_regroup() {
        let mk = |taken: i64| PhotoTile {
            item_id: taken,
            path: String::new(),
            thumb: String::new(),
            label: String::new(),
            taken_at: taken,
            starred: false,
            archived: false,
            trashed: false,
            width: 0,
            height: 0,
        };
        // May, May, April, then May again (out of order): four tiles, three
        // groups — a date-sorted grid never produces the fourth case, but a
        // regrouping implementation would silently reorder the grid if it did.
        let tiles = vec![mk(1_778_803_200), mk(1_778_889_600), mk(1_776_211_200), mk(1_778_803_200)];
        let groups = group_by_month(&tiles);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].tiles, vec![0, 1]);
        assert_eq!(groups[1].label, "April 2026");
        assert_eq!(groups[2].tiles, vec![3]);
        // Every tile is referenced exactly once.
        let total: usize = groups.iter().map(|g| g.tiles.len()).sum();
        assert_eq!(total, tiles.len());
    }

    #[test]
    fn undated_sorts_last_in_both_directions() {
        assert!(order_by("date", "desc").contains("NULLS LAST"));
        assert!(order_by("date", "asc").contains("NULLS LAST"));
        assert_eq!(order_by("name", "asc"), "items.abs_path ASC");
    }

    #[test]
    fn only_album_filter_takes_a_bind() {
        for cat in ["recent", "starred", "archive", "trash", "places", "nonsense"] {
            let (sql, bind) = category_filter(cat);
            assert!(!bind, "{cat} should not bind");
            assert!(!sql.contains('?'), "{cat} placeholder count must match binds");
        }
        let (sql, bind) = category_filter("album");
        assert!(bind);
        assert_eq!(sql.matches('?').count(), 1);
    }
}
