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
/// Upper bound when a drill-down resolves through a domain call that takes a
/// limit rather than a page. Matches the ceiling the Slint glue uses for its
/// starred and archive lists.
const DRILL_MAX: i64 = 100_000;
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
    /// >1 when this tile is the cover of a stack, and how many it stands for.
    pub stack_size: i64,
    /// The stack this tile covers, so the grid can expand or break it.
    pub stack_id: i64,
    /// True when a sibling video or an MP4 trailer makes this a live photo.
    pub live: bool,
}

/// One indexing pass over the library, as the AI tab draws it.
///
/// `stage` is `background::Stage::key()` — the string persisted in
/// `photo_ai_state.stage`, so it is a storage format and Dart passes it back
/// unchanged.
#[derive(Debug, Clone)]
pub struct AiStage {
    pub stage: String,
    pub label: String,
    /// What it produces, in the user's terms rather than the model's.
    pub blurb: String,
    /// Photos that have not been considered for this stage yet.
    pub pending: i64,
    /// Every model this stage needs before it can do anything, by manifest
    /// name. Empty for the two stages that need none.
    pub needs: Vec<String>,
    /// All of `needs` installed. False ⇒ the row offers "Get model", not "Run".
    pub ready: bool,
    /// 0..1 while this stage is the one running; -1 when it is not.
    pub frac: f64,
}

/// One row of `resources/ai-models.toml`, with what it is worth on this
/// machine right now.
#[derive(Debug, Clone)]
pub struct AiModel {
    /// Manifest name — the id every command takes.
    pub name: String,
    /// What it does, not what it is called.
    pub label: String,
    pub purpose: String,
    pub mb: i64,
    pub quant: String,
    pub installed: bool,
    /// 0..1 while downloading, -1 otherwise.
    pub frac: f64,
    /// The stage this model feeds, or "" for an editor tool with no queue —
    /// Magic eraser, Smart select and Upscale are per-photo, so their card
    /// offers the editor rather than a Run.
    pub stage: String,
    /// Photos still waiting on this model's stage; -1 when it has no stage.
    pub pending: i64,
}

/// What the machine brings, for the strip above the stage list.
#[derive(Debug, Clone)]
pub struct AiMachine {
    pub ram_mb: i64,
    /// Whether this build has the ONNX runtime compiled in at all. Without it
    /// every model stage is permanently a no-op, and saying so beats a Run
    /// button that does nothing.
    pub onnx: bool,
    pub installed: i64,
    pub total: i64,
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

/// A face cluster. `name` is empty until the user names it -- the whole point
/// of the tab is turning strangers into names, so unnamed clusters are shown,
/// not hidden.
#[derive(Debug, Clone)]
pub struct PersonCard {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub cover: String,
}

/// A detected object tag and how many live photos carry it.
#[derive(Debug, Clone)]
pub struct TagCard {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub cover: String,
}

/// Two members of one duplicate cluster, side by side. Clusters can hold more
/// than two; the Slint tab shows the first two and so does this.
#[derive(Debug, Clone)]
pub struct DedupePair {
    pub cluster_id: i64,
    /// sha256 (byte-identical) or phash (visually near-identical).
    pub kind: String,
    pub left: PhotoTile,
    pub right: PhotoTile,
}

/// A map pin: one bucket of geotagged photos.
#[derive(Debug, Clone)]
pub struct MapPin {
    pub lat: f64,
    pub lon: f64,
    pub count: i64,
    pub cover: String,
}

/// The area every geotagged photo falls inside -- the map's opening viewport.
#[derive(Debug, Clone)]
pub struct MapBounds {
    pub min_lat: f64,
    pub min_lon: f64,
    pub max_lat: f64,
    pub max_lon: f64,
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
    /// 0 when not drilled into one person.
    pub person_id: i64,
    /// Empty when not drilled into one tag. Carried so the header can name the
    /// tag without Dart having to search `things` for the id.
    pub tag_name: String,
    pub tiles: Vec<PhotoTile>,
    pub groups: Vec<PhotoGroup>,
    pub folders: Vec<FolderRow>,
    pub albums: Vec<AlbumCard>,
    pub people: Vec<PersonCard>,
    pub things: Vec<TagCard>,
    pub dedupe: Vec<DedupePair>,
    pub pins: Vec<MapPin>,
    /// None when nothing carries GPS, which is also when the map has nothing
    /// to open onto.
    pub bounds: Option<MapBounds>,
    /// True when an MBTiles basemap was found. Without one the Places tab has
    /// pins and nothing to draw them over, so it stays a grid.
    pub has_basemap: bool,
    /// name | count | size, for the Library tab's own sort.
    pub lib_sort: String,
    pub lib_sort_dir: String,
    // --- the AI tab ---
    pub ai_stages: Vec<AiStage>,
    pub ai_models: Vec<AiModel>,
    pub ai_machine: AiMachine,
    /// The stage running now, or "" — one at a time, because they contend for
    /// the same disk and the same cores.
    pub ai_running: String,
    /// Photos waiting on at least one runnable pass. Distinct photos, not the
    /// sum of the per-stage queues: a photo owing three passes is one photo
    /// waiting, and adding the queues up trebled the headline.
    pub ai_waiting: i64,
    /// "idle" | "plugged" | "always". When the background indexer is allowed to
    /// pick the queue up by itself.
    pub ai_when: String,
    /// Per-category counts for the chip row, keyed by category id. Counted once
    /// per snapshot rather than once per chip.
    pub counts: Vec<CategoryCount>,
}

/// A chip and how many photos are behind it.
#[derive(Debug, Clone)]
pub struct CategoryCount {
    pub id: String,
    pub count: i64,
}

// -------------------------------------------------------------- commands ----

#[derive(Debug, Clone)]
pub enum PhotosCmd {
    /// Re-read the current view. Sent on mount and after an external change.
    Refresh,
    /// One of: recent, starred, archive, trash, places, album, library,
    /// memories, people, facephotos, things, tagphotos.
    SetCategory { name: String },
    Search { query: String },
    /// mode: date | name | size. dir: desc | asc.
    SetSort { mode: String, dir: String },
    ShowMore,
    OpenAlbum { album_id: i64 },
    OpenPerson { person_id: i64 },
    OpenTag { tag_id: i64, name: String },
    /// Empty name un-names the cluster, matching `people::set_name`.
    RenamePerson { person_id: i64, name: String },
    Star { item_id: i64, starred: bool },
    Archive { item_ids: Vec<i64>, archived: bool },
    Trash { item_ids: Vec<i64> },
    Restore { item_ids: Vec<i64> },
    AlbumNew { name: String },
    /// A smart album: every photo its rule matches, read afresh each time the
    /// album is. Empty text and 0 years are left out; every rule given must
    /// hold. At least one is required.
    AlbumNewSmart {
        name: String,
        tag: String,
        person: String,
        camera: String,
        year_from: i64,
        year_to: i64,
        starred: bool,
    },
    AlbumRename { album_id: i64, name: String },
    AlbumDelete { album_id: i64 },
    AlbumAdd { album_id: i64, item_ids: Vec<i64> },
    AddFolder { path: String },
    /// The folder list sorts independently of the photo grid: one sorts
    /// folders, the other photos, and "name" means a different thing to each.
    SetLibSort { mode: String, dir: String },
    /// Keep one of a duplicate pair and trash the other.
    DedupeResolve { keep_item_id: i64, trash_item_id: i64 },
    /// Show or hide a stack's members in the grid.
    ToggleStack { stack_id: i64, expanded: bool },
    /// Break a stack up; its members become ordinary tiles again.
    Unstack { stack_id: i64 },
    Scan,
    /// Run one indexing pass over everything still waiting for it. Detached —
    /// the command returns at once and progress arrives as `Stale`.
    AiRun { stage: String },
    /// The first stage that has work and a model, then the next, and so on.
    AiRunAll,
    /// Stop after the batch in flight. Progress already written is kept.
    AiStop,
    /// Forget every attempt at this stage so the whole library is reconsidered,
    /// then run it. "Run again" on a stage that has drained.
    AiRerun { stage: String },
    /// "idle" | "plugged" | "always" — when the background indexer may run
    /// without being asked.
    AiSetWhen { mode: String },
    /// Fetch and verify one manifest model.
    AiDownload { name: String },
    /// Re-check an installed model against its pinned digest.
    AiVerify { name: String },
    /// Delete a model's folder, pinned digest included.
    AiRemove { name: String },
}

#[derive(Debug, Clone)]
pub enum PhotosEvent {
    ScanStarted { root: String },
    ScanFinished { scanned: i64, inserted: i64, updated: i64, missing: i64 },
    ScanFailed { message: String },
    /// Something behind the page changed on its own — an indexing pass moved,
    /// a model finished downloading. Dart answers with a plain re-read.
    Stale,
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
    person_id: i64,
    tag_id: i64,
    tag_name: String,
    lib_sort: String,
    lib_sort_dir: String,
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
            person_id: 0,
            tag_id: 0,
            tag_name: String::new(),
            lib_sort: "name".into(),
            lib_sort_dir: "asc".into(),
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

pub(crate) fn emit(event: PhotosEvent) {
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
            // Leaving a drill-down clears what it was drilled into, so a
            // later Refresh cannot resurrect a stale album, person or tag.
            if name != "album" {
                s.album_id = 0;
            }
            if name != "facephotos" {
                s.person_id = 0;
            }
            if name != "tagphotos" {
                s.tag_id = 0;
                s.tag_name = String::new();
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
        PhotosCmd::OpenPerson { person_id } => {
            let mut s = lock();
            s.limit = PAGE;
            s.category = "facephotos".into();
            s.person_id = person_id;
        }
        PhotosCmd::OpenTag { tag_id, name } => {
            let mut s = lock();
            s.limit = PAGE;
            s.category = "tagphotos".into();
            s.tag_id = tag_id;
            s.tag_name = name;
        }
        PhotosCmd::RenamePerson { person_id, name } => {
            tulipix_photos::ai::people::set_name(pool, person_id, &name).await?;
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
        PhotosCmd::AlbumNewSmart { name, tag, person, camera, year_from, year_to, starred } => {
            let rule = smart_rule(&tag, &person, &camera, year_from, year_to, starred)?;
            let id = tulipix_photos::albums::create(pool, &name).await?;
            tulipix_photos::smart_albums::save_rule(pool, id, &rule).await?;
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
        PhotosCmd::SetLibSort { mode, dir } => {
            let mut s = lock();
            s.lib_sort = mode;
            s.lib_sort_dir = dir;
        }
        PhotosCmd::DedupeResolve { keep_item_id, trash_item_id } => {
            // Trash, never delete: the whole point of a soft delete is that a
            // wrong call about which of two near-identical photos to keep is
            // recoverable from the Trash tab.
            let _ = keep_item_id;
            tulipix_photos::trash::soft_delete(pool, &[trash_item_id]).await?;
        }
        PhotosCmd::ToggleStack { stack_id, expanded } => {
            tulipix_photos::stacks::set_expanded(pool, stack_id, expanded).await?;
            invalidate_badges();
        }
        PhotosCmd::Unstack { stack_id } => {
            tulipix_photos::stacks::unstack(pool, stack_id).await?;
            invalidate_badges();
        }
        PhotosCmd::Scan => {
            scan_watched(pool).await;
            invalidate_badges();
        }
        PhotosCmd::AiRun { stage } => {
            if let Some(st) = stage_of(&stage) {
                ai_spawn(vec![st]);
            }
        }
        PhotosCmd::AiRunAll => {
            // In queue order, skipping what has no model: EXIF and the search
            // index first because everything else reads what they write.
            ai_spawn(
                tulipix_photos::ai::background::Stage::ALL
                    .into_iter()
                    .filter(|s| tulipix_photos::ai::load::stage_ready(*s))
                    .collect(),
            );
        }
        PhotosCmd::AiStop => {
            AI_STOP.store(true, std::sync::atomic::Ordering::Relaxed);
            emit(PhotosEvent::Stale);
        }
        PhotosCmd::AiRerun { stage } => {
            if let Some(st) = stage_of(&stage) {
                tulipix_photos::ai::background::reset_stage(pool, st).await?;
                ai_spawn(vec![st]);
            }
        }
        PhotosCmd::AiSetWhen { mode } => set_ai_when(&mode),
        PhotosCmd::AiDownload { name } => {
            let Some(entry) = manifest().models.iter().find(|m| m.name == name) else {
                anyhow::bail!("no model called \u{201c}{name}\u{201d} is in the manifest");
            };
            // Detached, like the indexing run and for the same reason: an
            // unanswered press is a press the user makes four more times.
            // Claimed before the spawn so the snapshot this command returns
            // already says "downloading". The guard is taken and given back
            // inside this `let` rather than held over the `await` below: a std
            // MutexGuard alive across an await point makes the whole dispatch
            // future !Send, and frb spawns it.
            let already = {
                let mut g = match ai_dl().lock() {
                    Ok(g) => g,
                    Err(e) => e.into_inner(),
                };
                let had = g.contains_key(&name);
                if !had {
                    g.insert(name.clone(), 0.0);
                }
                had
            };
            if already {
                return snapshot(pool).await;
            }
            let entry = entry.clone();
            tokio::spawn(async move {
                let key = entry.name.clone();
                let res = tulipix_photos::ai::models::download_with_progress(&entry, |f| {
                    let mut g = match ai_dl().lock() {
                        Ok(g) => g,
                        Err(e) => e.into_inner(),
                    };
                    let prev = g.get(&key).copied().unwrap_or(0.0);
                    g.insert(key.clone(), f as f64);
                    drop(g);
                    // Per percent, not per chunk: a 205MB model is thousands of
                    // chunks and every event costs Dart a whole snapshot.
                    if (f as f64 * 100.0) as i64 != (prev * 100.0) as i64 {
                        emit(PhotosEvent::Stale);
                    }
                })
                .await;
                if let Ok(mut g) = ai_dl().lock() {
                    g.remove(&entry.name);
                }
                match res {
                    Ok(_) => tracing::info!(name = %entry.name, "model installed"),
                    Err(e) => tracing::error!(name = %entry.name, error = %e, "model download"),
                }
                emit(PhotosEvent::Stale);
            });
        }
        PhotosCmd::AiVerify { name } => {
            let Some(entry) = manifest().models.iter().find(|m| m.name == name) else {
                anyhow::bail!("no model called \u{201c}{name}\u{201d} is in the manifest");
            };
            let Some(path) = tulipix_photos::ai::models::local_path(entry) else {
                anyhow::bail!("{name} is not installed");
            };
            tulipix_photos::ai::models::verify(&path, &entry.sha256).await?;
        }
        PhotosCmd::AiRemove { name } => {
            let Some(entry) = manifest().models.iter().find(|m| m.name == name) else {
                anyhow::bail!("no model called \u{201c}{name}\u{201d} is in the manifest");
            };
            // The whole folder, pinned digest included, so a later download
            // re-pins rather than checking against a hash for a file that is
            // no longer there.
            if let Some(dir) = tulipix_photos::ai::models::install_dir(entry) {
                std::fs::remove_dir_all(&dir).ok();
            }
        }
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

/// One basemap tile as PNG bytes, or None when there is no basemap or the
/// tile is past the edge of it.
///
/// MBTiles stores tiles in TMS order, where y counts from the bottom; every
/// slippy-map widget ever written counts from the top. The flip lives here so
/// the Dart side can think in ordinary XYZ.
pub async fn photos_map_tile(z: u32, x: u32, y: u32) -> Result<Option<Vec<u8>>> {
    let Some(pool) = basemap_pool().await else { return Ok(None) };
    let flipped = (1u32 << z).saturating_sub(1).saturating_sub(y);
    tulipix_photos::map::tile(pool, z, x, flipped).await
}

async fn basemap_pool() -> Option<&'static sqlx::SqlitePool> {
    static P: tokio::sync::OnceCell<Option<sqlx::SqlitePool>> = tokio::sync::OnceCell::const_new();
    P.get_or_init(|| async {
        let path = basemap_path()?;
        match tulipix_photos::map::open_mbtiles(&path).await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(error = %e, "basemap open failed");
                None
            }
        }
    })
    .await
    .as_ref()
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

// ---------------------------------------------------------------- viewer ----

/// One line of the viewer's info panel.
#[derive(Debug, Clone)]
pub struct ExifRow {
    pub label: String,
    pub value: String,
}

/// Everything the full-size viewer needs for one photo. `tile` carries the
/// path, dimensions and flags the grid already has; `exif` is the part that is
/// far too expensive to put in a snapshot of 240 tiles.
#[derive(Debug, Clone)]
pub struct PhotoDetail {
    pub tile: PhotoTile,
    pub size: i64,
    pub exif: Vec<ExifRow>,
}

/// Describe one photo. The viewer's cursor -- which photo is current, what
/// next and prev mean -- stays in Dart, which already holds the ordered tile
/// list the last snapshot returned. Putting it in Rust would mean re-running
/// the whole grid query on every arrow key to answer a question Dart can
/// already answer, so this asks Rust exactly one thing: describe this id.
pub async fn photos_item_detail(item_id: i64) -> Result<Option<PhotoDetail>> {
    let pool = photos_pool().await?;
    let sql = format!("{TILE_SELECT} WHERE items.id = ?");
    let row = sqlx::query_as::<sqlx::Sqlite, TileRow>(sqlx::AssertSqlSafe(&*sql))
        .bind(item_id)
        .fetch_optional(pool)
        .await?;
    let Some(row) = row else { return Ok(None) };
    let size = row.3;
    let tile = into_tile(row);
    let path = PathBuf::from(&tile.path);
    // Opening the file and walking its EXIF segment is blocking IO on a path
    // that may be a slow disk or a network mount.
    let exif = tokio::task::spawn_blocking(move || exif_rows(&path)).await?;
    Ok(Some(PhotoDetail { tile, size, exif }))
}

/// The same 24 attributes `tulipix_sec_photos::exif_rows` shows, in the same
/// order, with the same em-dash for a missing tag. Duplicated rather than
/// imported because that crate links slint, and nothing the bridge links may.
/// If a field is added there, add it here -- the two lists drifting apart is a
/// parity bug that no compiler will catch.
fn exif_rows(path: &Path) -> Vec<ExifRow> {
    use exif::{In, Reader, Tag};
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let dims = image::image_dimensions(path).ok();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_uppercase();
    let meta = std::fs::File::open(path).ok().and_then(|f| {
        let mut br = std::io::BufReader::new(f);
        Reader::new().read_from_container(&mut br).ok()
    });
    let g = |tag: Tag| -> String {
        meta.as_ref()
            .and_then(|e| {
                e.get_field(tag, In::PRIMARY)
                    .map(|f| f.display_value().with_unit(e).to_string())
            })
            .unwrap_or_else(|| MISSING.into())
    };
    let row = |label: &str, value: String| ExifRow { label: label.into(), value };
    vec![
        row(
            "File",
            path.file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(MISSING)
                .to_string(),
        ),
        row("Format", if ext.is_empty() { MISSING.into() } else { ext }),
        row("Size", human_size(size)),
        row(
            "Dimensions",
            dims.map(|(w, h)| format!("{w} \u{d7} {h}"))
                .unwrap_or_else(|| MISSING.into()),
        ),
        row("Date taken", g(Tag::DateTimeOriginal)),
        row("Date digit.", g(Tag::DateTimeDigitized)),
        row("Camera make", g(Tag::Make)),
        row("Camera model", g(Tag::Model)),
        row("Lens", g(Tag::LensModel)),
        row("ISO", g(Tag::PhotographicSensitivity)),
        row("Aperture", g(Tag::FNumber)),
        row("Shutter", g(Tag::ExposureTime)),
        row("Exp. program", g(Tag::ExposureProgram)),
        row("Exp. comp.", g(Tag::ExposureBiasValue)),
        row("Metering", g(Tag::MeteringMode)),
        row("Flash", g(Tag::Flash)),
        row("Focal length", g(Tag::FocalLength)),
        row("Focal 35mm", g(Tag::FocalLengthIn35mmFilm)),
        row("White bal.", g(Tag::WhiteBalance)),
        row("Color space", g(Tag::ColorSpace)),
        row("Orientation", g(Tag::Orientation)),
        row("GPS", {
            let lat = g(Tag::GPSLatitude);
            let lon = g(Tag::GPSLongitude);
            if lat == MISSING && lon == MISSING {
                MISSING.into()
            } else {
                format!("{lat}, {lon}")
            }
        }),
        row("Software", g(Tag::Software)),
        row("Artist", g(Tag::Artist)),
    ]
}

/// Placeholder for a tag the file does not carry. Matches the Slint panel.
const MISSING: &str = "\u{2014}";

/// Scaled unit plus the thousands-separated raw count, exactly as
/// `tulipix_common::human_size` renders it. That crate links slint too.
pub(crate) fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    let (val, unit) = if b >= 1_073_741_824.0 {
        (b / 1_073_741_824.0, "GB")
    } else if b >= 1_048_576.0 {
        (b / 1_048_576.0, "MB")
    } else if b >= 1024.0 {
        (b / 1024.0, "KB")
    } else {
        (b, "bytes")
    };
    let digits = bytes.to_string();
    let mut raw = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            raw.push(',');
        }
        raw.push(c);
    }
    if unit == "bytes" {
        format!("{raw} bytes")
    } else {
        format!("{val:.2} {unit} ({raw} bytes)")
    }
}

// -------------------------------------------------------------------- ai ----

/// The indexing job in flight: `(stage key, 0..1)`. Empty stage = nothing
/// running. One slot, because one stage runs at a time.
fn ai_job() -> &'static Mutex<(String, f64)> {
    static J: std::sync::OnceLock<Mutex<(String, f64)>> = std::sync::OnceLock::new();
    J.get_or_init(|| Mutex::new((String::new(), 0.0)))
}

/// Set by Stop, cleared when a run ends. The run checks it between batches,
/// which is also its yield point — stopping mid-batch would throw away work
/// already paid for.
static AI_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Model downloads in flight, by manifest name.
fn ai_dl() -> &'static Mutex<std::collections::HashMap<String, f64>> {
    static D: std::sync::OnceLock<Mutex<std::collections::HashMap<String, f64>>> =
        std::sync::OnceLock::new();
    D.get_or_init(Default::default)
}

/// Progress events are answered by a whole fresh snapshot on the Dart side, so
/// they are rationed rather than sent per item. Four a second is faster than a
/// progress bar reads.
fn ai_progress(stage: &str, frac: f64) {
    static LAST: std::sync::Mutex<Option<std::time::Instant>> = std::sync::Mutex::new(None);
    let changed = {
        let mut g = match ai_job().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        let changed = g.0 != stage;
        *g = (stage.to_string(), frac);
        changed
    };
    let due = changed
        || LAST
            .lock()
            .map(|mut last| {
                let now = std::time::Instant::now();
                let due = last.is_none_or(|t| now.duration_since(t).as_millis() >= 250);
                if due {
                    *last = Some(now);
                }
                due
            })
            .unwrap_or(true);
    if due {
        emit(PhotosEvent::Stale);
    }
}

fn ai_clear() {
    if let Ok(mut g) = ai_job().lock() {
        *g = (String::new(), 0.0);
    }
    AI_STOP.store(false, std::sync::atomic::Ordering::Relaxed);
    emit(PhotosEvent::Stale);
}

fn stage_of(key: &str) -> Option<tulipix_photos::ai::background::Stage> {
    use tulipix_photos::ai::background::Stage;
    Stage::ALL.into_iter().find(|s| s.key() == key)
}

/// Drain one stage, a batch at a time, until it is empty or Stop is pressed.
///
/// Detached: `photos_dispatch` must come back the moment the button is pressed
/// or the first press reads as dropped and the next four start four more runs.
/// Guarded by the job slot, so a second press while one runs is a no-op rather
/// than two passes fighting over the same rows.
fn ai_spawn(stages: Vec<tulipix_photos::ai::background::Stage>) {
    {
        let g = match ai_job().lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        if !g.0.is_empty() {
            return;
        }
    }
    AI_STOP.store(false, std::sync::atomic::Ordering::Relaxed);
    // Claim the slot before the task is spawned: the snapshot this command
    // returns has to already say "running", or the button springs back.
    if let Some(first) = stages.first() {
        ai_progress(first.key(), 0.0);
    }
    tokio::spawn(async move {
        let Ok(pool) = photos_pool().await else {
            ai_clear();
            return;
        };
        for stage in stages {
            if AI_STOP.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            if !tulipix_photos::ai::load::stage_ready(stage) {
                continue;
            }
            let total = tulipix_photos::ai::background::pending_for(pool, stage)
                .await
                .unwrap_or(0)
                .max(1);
            let mut done = 0i64;
            loop {
                if AI_STOP.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                match tulipix_photos::ai::indexer::run_batch(pool, stage, AI_BATCH).await {
                    // 0 means drained, or a model stage with no model. Either
                    // way there is nothing more this pass can do.
                    Ok(0) => break,
                    Ok(n) => {
                        done += n as i64;
                        ai_progress(stage.key(), (done as f64 / total as f64).clamp(0.0, 1.0));
                    }
                    Err(e) => {
                        tracing::warn!(stage = stage.key(), error = %e, "index run");
                        break;
                    }
                }
            }
            // Faces and tags write rows the grid reads; the badge cache has to
            // let go of what it counted before them.
            invalidate_badges();
        }
        ai_clear();
    });
}

/// Photos per batch. Small enough that Stop feels immediate and a stage that
/// turns out to be slow does not hold the pool for a minute.
const AI_BATCH: i64 = 48;

/// The manifest, parsed once. Same file `api/settings.rs` reads, by
/// `include_str!` rather than a copy.
fn manifest() -> &'static tulipix_core::ai_models::Manifest {
    static M: std::sync::OnceLock<tulipix_core::ai_models::Manifest> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        tulipix_core::ai_models::Manifest::from_toml(include_str!(
            "../../../../resources/ai-models.toml"
        ))
        .unwrap_or_default()
    })
}

/// Name, purpose and which stage it feeds, per manifest row. Keyed on the name
/// rather than the capability because two files share a capability twice over
/// (the face pair, CLIP and its tokenizer) and the cards are per file.
fn model_display(name: &str) -> Option<(&'static str, &'static str, &'static str)> {
    Some(match name {
        "face-det-500m" => ("Face detection", "Finds faces in photos so people can be grouped.", "faces"),
        "face-rec-500m" => ("Face recognition", "Tells faces apart so each person is grouped once.", "faces"),
        "clip-vit-b32-q8" => ("Photo search", "Finds photos by what is in them \u{2014} type \u{201c}beach at sunset\u{201d}.", "clip"),
        "clip-vit-b32-tokenizer" => ("Search words", "Turns what you type into something photo search understands.", "clip"),
        "yolox-s" => ("Object tags", "Names the things in your photos, for the Things tab.", "tags"),
        // The three editor tools. Empty stage = no queue, so the card offers
        // the editor rather than a Run button that would have nothing to run.
        "lama-inpaint" => ("Magic eraser", "Removes unwanted objects in the photo editor.", ""),
        "mobile-sam" => ("Smart select", "Selects sky or an object for one-tap edits.", ""),
        "realesrgan-x4" => ("Upscale", "Enlarges a photo four times in the editor, keeping detail.", ""),
        // Whisper, CLAP, MDX and Kokoro are in the same manifest and belong to
        // other sections. The Photos tab shows Photos' models.
        _ => return None,
    })
}

fn stage_blurb(stage: tulipix_photos::ai::background::Stage) -> (&'static str, &'static str) {
    use tulipix_photos::ai::background::Stage;
    match stage {
        Stage::Exif => ("Read EXIF", "Date taken, camera, lens and GPS. Everything else sorts by it."),
        Stage::Fts => ("Search index", "Filenames, camera, tags and people, into the full-text table."),
        Stage::Faces => ("Find faces", "Detect, crop and embed, then cluster. Fills the People tab."),
        Stage::Tags => ("Tag objects", "COCO-80 objects. Fills the Things tab and sharpens search."),
        Stage::Clip => ("Photo search index", "One embedding per photo, so \u{201c}beach at sunset\u{201d} finds the photo."),
    }
}

/// Which models a stage needs before it can do anything.
fn stage_needs(stage: tulipix_photos::ai::background::Stage) -> Vec<String> {
    use tulipix_photos::ai::background::Stage;
    let v: &[&str] = match stage {
        Stage::Exif | Stage::Fts => &[],
        Stage::Faces => &["face-det-500m", "face-rec-500m"],
        Stage::Tags => &["yolox-s"],
        Stage::Clip => &["clip-vit-b32-q8", "clip-vit-b32-tokenizer"],
    };
    v.iter().map(|s| (*s).to_string()).collect()
}

/// The key `Settings.advanced` stores the run policy under. Shared with the
/// Slint build, which reads the same store.
const AI_WHEN_KEY: &str = "photos.ai.when";

fn ai_when() -> String {
    tulipix_core::settings::Settings::load()
        .ok()
        .and_then(|s| s.advanced.get(AI_WHEN_KEY).cloned())
        .unwrap_or_else(|| "idle".into())
}

/// The background indexing loop, on this side at last.
///
/// It lived in `tulipix-app`'s runtime, so on the Flutter build nothing ever
/// picked the queue up by itself — a model downloaded and then waited for a
/// button. Started once, from the shell's first snapshot, like the auto-rescan.
///
/// Policy is `photos.ai.when`, which did not exist either: indexing was
/// idle-only and hard-coded, so a machine that is never idle never indexed.
pub(crate) fn start_ai_indexer() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async {
        // Launch has enough to do.
        tokio::time::sleep(std::time::Duration::from_secs(90)).await;
        // A drained library is the steady state -- most of the time this loop
        // exists to do nothing -- so it backs off rather than running five
        // counting queries a minute forever.
        let mut quiet: u32 = 0;
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(20) * (1u32 << quiet.min(4))).await;
            if !ai_policy_allows() {
                // Not a quiet pass: there may be plenty of work and the machine
                // is simply in use. Keep the short tick.
                quiet = 0;
                continue;
            }
            // A run started from the AI tab owns the slot; this loop never
            // competes with the user's own Run.
            if ai_job().lock().map(|g| !g.0.is_empty()).unwrap_or(true) {
                continue;
            }
            let Ok(pool) = photos_pool().await else {
                quiet = quiet.saturating_add(1);
                continue;
            };
            let mut worked = false;
            for stage in tulipix_photos::ai::background::Stage::ALL {
                if !tulipix_photos::ai::load::stage_ready(stage) {
                    continue;
                }
                match tulipix_photos::ai::indexer::run_batch(pool, stage, AI_BATCH).await {
                    Ok(0) => continue,
                    Ok(_) => {
                        worked = true;
                        emit(PhotosEvent::Stale);
                    }
                    Err(e) => tracing::warn!(stage = stage.key(), error = %e, "background index"),
                }
                // One batch per pass. The policy check above is the yield
                // point, and someone who picks the machine up mid-drain should
                // get it back at the next batch boundary.
                break;
            }
            quiet = if worked { 0 } else { quiet.saturating_add(1) };
        }
    });
}

/// Whether the background loop may run right now.
///
/// Three things, in the order they can say no: the section is switched off in
/// Settings → Sections, the machine is on battery, the user is at the keyboard.
fn ai_policy_allows() -> bool {
    let Ok(s) = tulipix_core::settings::Settings::load() else { return false };
    if !tulipix_core::sections::mode_of(&s, "photos").works() {
        return false;
    }
    match s
        .advanced
        .get(AI_WHEN_KEY)
        .map(String::as_str)
        .unwrap_or("idle")
    {
        "always" => true,
        "plugged" => !matches!(
            tulipix_core::power_aware::power_source(),
            tulipix_core::power_aware::PowerSource::Battery
        ),
        // The default. `idle_secs` is weaker than it looks on this build --
        // nothing reports ordinary input -- so the battery gate and the small
        // batch are what actually keep it out of the way.
        _ => {
            !matches!(
                tulipix_core::power_aware::power_source(),
                tulipix_core::power_aware::PowerSource::Battery
            ) && tulipix_core::idle::idle_secs() >= 600
        }
    }
}

fn set_ai_when(mode: &str) {
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert(AI_WHEN_KEY.to_string(), mode.to_string());
    let _ = s.save();
}

async fn ai_panel(
    pool: &sqlx::SqlitePool,
) -> (Vec<AiStage>, Vec<AiModel>, AiMachine, String, i64) {
    use tulipix_photos::ai::background::Stage;
    let (running, frac) = match ai_job().lock() {
        Ok(g) => g.clone(),
        Err(e) => e.into_inner().clone(),
    };
    let dl = ai_dl().lock().map(|g| g.clone()).unwrap_or_default();

    let mut stages = Vec::with_capacity(Stage::ALL.len());
    let mut pending_by_stage = std::collections::HashMap::new();
    for stage in Stage::ALL {
        let (label, blurb) = stage_blurb(stage);
        let pending = tulipix_photos::ai::background::pending_for(pool, stage)
            .await
            .unwrap_or(0);
        pending_by_stage.insert(stage.key(), pending);
        stages.push(AiStage {
            stage: stage.key().to_string(),
            label: label.into(),
            blurb: blurb.into(),
            pending,
            needs: stage_needs(stage),
            ready: tulipix_photos::ai::load::stage_ready(stage),
            frac: if running == stage.key() { frac } else { -1.0 },
        });
    }

    let mut models = Vec::new();
    for m in &manifest().models {
        let Some((label, purpose, stage)) = model_display(&m.name) else { continue };
        models.push(AiModel {
            name: m.name.clone(),
            label: label.into(),
            purpose: purpose.into(),
            mb: (m.size_bytes / 1_000_000) as i64,
            quant: m.quant.clone(),
            installed: tulipix_photos::ai::models::is_installed(m),
            frac: dl.get(&m.name).copied().unwrap_or(-1.0),
            stage: stage.into(),
            pending: pending_by_stage.get(stage).copied().unwrap_or(-1),
        });
    }
    let installed = models.iter().filter(|m| m.installed).count() as i64;
    let total = models.len() as i64;

    let machine = AiMachine {
        ram_mb: total_ram_mb(),
        onnx: cfg!(feature = "ai-onnx"),
        installed,
        total,
    };
    // What the headline says. Only stages that can actually run count: a photo
    // is not "waiting" on a model that is not installed.
    let ready: Vec<Stage> = Stage::ALL
        .into_iter()
        .filter(|st| tulipix_photos::ai::load::stage_ready(*st))
        .collect();
    let waiting = tulipix_photos::ai::background::pending_photos(pool, &ready)
        .await
        .unwrap_or(0);
    (stages, models, machine, running, waiting)
}

/// Installed memory in MB, 0 when it cannot be read. Linux only by design:
/// every other platform gets the honest 0 and the strip says "Unknown" rather
/// than a number invented from nothing.
fn total_ram_mb() -> i64 {
    let Ok(s) = std::fs::read_to_string("/proc/meminfo") else { return 0 };
    s.lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|v| v.split_whitespace().next())
        .and_then(|kb| kb.parse::<i64>().ok())
        .map(|kb| kb / 1024)
        .unwrap_or(0)
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
    let mut folders = folders(pool).await?;
    sort_folders(&mut folders, &s.lib_sort, &s.lib_sort_dir);
    let albums = albums(pool).await?;
    // Cards for the two tabs that show a grid of clusters rather than photos.
    // Both are cheap enough to carry in every snapshot: `people` is one row per
    // cluster and `things` is one row per tag, not one per photo.
    let people = people(pool).await?;
    let things = things(pool).await?;
    // Only the tab that shows them pays for them: a duplicate scan reads every
    // cluster, and pin clustering walks every geotagged photo.
    let dedupe = if s.category == "dedupe" { dedupe_pairs(pool).await? } else { Vec::new() };
    let (pins, bounds) = if s.category == "places" {
        map_view(pool).await?
    } else {
        (Vec::new(), None)
    };
    // Five COUNTs over `photo_ai_state` plus the manifest walk. Only the tab
    // that shows them pays -- except while a pass is running, when the chip
    // has to keep counting down wherever you are.
    let running_now = ai_job().lock().map(|g| !g.0.is_empty()).unwrap_or(false);
    let (ai_stages, ai_models, ai_machine, ai_running, ai_waiting) =
        if s.category == "ai" || running_now {
            ai_panel(pool).await
        } else {
            (Vec::new(), Vec::new(), AiMachine { ram_mb: 0, onnx: false, installed: 0, total: 0 }, String::new(), 0)
        };
    let counts = category_counts(pool, &s).await;

    Ok(PhotosState {
        item_count: total,
        more_count: (total - tiles.len() as i64).max(0),
        folder_count: folders.len() as i64,
        category: s.category,
        query: s.query,
        sort_mode: s.sort_mode,
        sort_dir: s.sort_dir,
        album_id: s.album_id,
        person_id: s.person_id,
        tag_name: s.tag_name,
        tiles,
        groups,
        folders,
        albums,
        people,
        things,
        dedupe,
        pins,
        bounds,
        has_basemap: basemap_path().is_some(),
        lib_sort: s.lib_sort,
        lib_sort_dir: s.lib_sort_dir,
        ai_stages,
        ai_models,
        ai_machine,
        ai_running,
        ai_waiting,
        ai_when: ai_when(),
        counts,
    })
}

/// How many photos are behind each chip.
///
/// One pass per snapshot rather than one per chip, and the numbers a tab owns
/// (People, Things, Albums, Dedupe, Places) come from the lists already built
/// for them, so only the photo categories cost a query.
async fn category_counts(pool: &sqlx::SqlitePool, s: &Session) -> Vec<CategoryCount> {
    let mut out = Vec::new();
    let mut push = |id: &str, n: i64| out.push(CategoryCount { id: id.into(), count: n });
    // `category_filter` builds the same WHERE the grid uses, so a chip can
    // never disagree with the page it opens.
    for id in ["recent", "starred", "archive", "trash"] {
        let probe = Session { category: id.into(), query: String::new(), ..s.clone() };
        let Ok(where_sql) = category_filter(pool, &probe).await else { continue };
        let sql = format!("SELECT COUNT(*) FROM items LEFT JOIN photo_meta pm ON pm.item_id = items.id WHERE {where_sql}");
        let n: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(&*sql))
            .fetch_one(pool)
            .await
            .unwrap_or(0);
        push(id, n);
    }
    out
}

/// Rows the grid needs, plus the unpaged total for the "show more" count.
async fn load_tiles(pool: &sqlx::SqlitePool, s: &Session) -> Result<(Vec<PhotoTile>, i64)> {
    let b = badges(pool).await;
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

    let mut where_sql = category_filter(pool, s).await?;
    // Collapsed stacks show one tile, not forty. Trash is exempt: a member
    // hidden behind a cover must still be findable after it is deleted.
    if !b.hidden.is_empty() && s.category != "trash" {
        let list = b.hidden.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        where_sql = format!("{where_sql} AND items.id NOT IN ({list})");
    }
    let order = order_by(&s.sort_mode, &s.sort_dir);

    let count_sql = format!(
        "SELECT COUNT(*) FROM items LEFT JOIN photo_meta pm ON pm.item_id = items.id WHERE {where_sql}"
    );
    let total = sqlx::query_scalar::<sqlx::Sqlite, i64>(sqlx::AssertSqlSafe(&*count_sql))
        .fetch_one(pool)
        .await?;

    let sql = format!("{TILE_SELECT} WHERE {where_sql} ORDER BY {order} LIMIT ?");
    let rows = sqlx::query_as::<sqlx::Sqlite, TileRow>(sqlx::AssertSqlSafe(&*sql))
        .bind(s.limit)
        .fetch_all(pool)
        .await?;

    Ok((rows.into_iter().map(|r| into_tile_with(r, &b)).collect(), total))
}

/// Stack membership and live-photo detection for the whole library.
///
/// Cached because `live_photos::detect_in_library` opens every candidate file
/// to look for an MP4 trailer -- fine once, ruinous per snapshot, and a
/// snapshot happens on every keystroke. Invalidated by anything that can move
/// a photo in or out of a stack.
#[frb(ignore)]
#[derive(Default)]
struct Badges {
    /// item_id of a stack cover -> how many photos it stands for.
    stack_size: std::collections::HashMap<i64, i64>,
    stack_id: std::collections::HashMap<i64, i64>,
    /// Members that are not the cover of a collapsed stack, hidden from the
    /// grid so a burst of 40 frames reads as one photo.
    hidden: Vec<i64>,
    live: std::collections::HashSet<i64>,
}

fn badge_cache() -> &'static Mutex<Option<std::sync::Arc<Badges>>> {
    static B: std::sync::OnceLock<Mutex<Option<std::sync::Arc<Badges>>>> =
        std::sync::OnceLock::new();
    B.get_or_init(|| Mutex::new(None))
}

fn invalidate_badges() {
    if let Ok(mut g) = badge_cache().lock() {
        *g = None;
    }
}

async fn badges(pool: &sqlx::SqlitePool) -> std::sync::Arc<Badges> {
    if let Ok(g) = badge_cache().lock() {
        if let Some(b) = g.as_ref() {
            return b.clone();
        }
    }
    let mut b = Badges::default();
    if let Ok(stacks) = tulipix_photos::stacks::list(pool).await {
        for st in stacks {
            b.stack_size.insert(st.cover_id, st.member_ids.len() as i64);
            b.stack_id.insert(st.cover_id, st.id);
        }
    }
    b.hidden = tulipix_photos::stacks::hidden_ids(pool)
        .await
        .map(|h| h.into_iter().collect())
        .unwrap_or_default();
    // A failure here means no badges, not no photos: a library on a network
    // mount should still show a grid.
    if let Ok(live) = tulipix_photos::live_photos::detect_in_library(pool).await {
        b.live = live.into_iter().map(|l| l.still_item_id).collect();
    }
    let arc = std::sync::Arc::new(b);
    if let Ok(mut g) = badge_cache().lock() {
        *g = Some(arc.clone());
    }
    arc
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
    into_tile_with(row, &Badges::default())
}

fn into_tile_with(row: TileRow, badges: &Badges) -> PhotoTile {
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
        stack_size: badges.stack_size.get(&id).copied().unwrap_or(0),
        stack_id: badges.stack_id.get(&id).copied().unwrap_or(0),
        live: badges.live.contains(&id),
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
    let rows = sqlx::query_as::<sqlx::Sqlite, TileRow>(sqlx::AssertSqlSafe(&*sql)).fetch_all(pool).await?;
    Ok(rows.into_iter().map(into_tile).collect())
}

/// The WHERE clause per tab, and whether it carries an `album_id` placeholder.
/// Mirrors `tulipix_sec_photos::category_path_set`.
/// The WHERE clause for one category.
///
/// Async because three of these resolve through the domain crates before they
/// are a filter at all: Memories is a date computation, People is a face join,
/// and both hand back an id list rather than a predicate. Ids are i64 read out
/// of our own tables, so they format straight into the SQL -- nothing the user
/// typed is ever interpolated, which is why the search path stays separate.
async fn category_filter(pool: &sqlx::SqlitePool, s: &Session) -> Result<String> {
    const LIVE: &str = "items.missing_since IS NULL AND pm.deleted_at IS NULL";
    Ok(match s.category.as_str() {
        "starred" => format!("{LIVE} AND pm.starred = 1"),
        "archive" => format!("{LIVE} AND pm.archived = 1"),
        "trash" => "pm.deleted_at IS NOT NULL".into(),
        "places" => format!("{LIVE} AND pm.gps_lat IS NOT NULL AND pm.gps_lon IS NOT NULL"),
        "album" => match smart_rule_of(pool, s.album_id).await? {
            Some(rule) => id_in(LIVE, smart_match(pool, &rule).await?.into_iter()),
            None => format!(
                "{LIVE} AND items.id IN \
                 (SELECT item_id FROM album_items WHERE album_id = {})",
                s.album_id
            ),
        },
        "memories" => {
            let hits = tulipix_photos::memories::on_this_day(pool, now_secs()).await?;
            id_in(LIVE, hits.into_iter().map(|h| h.item_id))
        }
        "facephotos" => {
            let rows =
                tulipix_photos::ai::people::photos_of(pool, s.person_id, DRILL_MAX).await?;
            id_in(LIVE, rows.into_iter().map(|(id, _)| id))
        }
        // Both tabs draw cards, not photos. Without this they would run a
        // 240-row grid query whose result nothing reads.
        "people" | "things" | "dedupe" => "0 = 1".into(),
        "tagphotos" => format!(
            "{LIVE} AND items.id IN (SELECT item_id FROM item_tags WHERE tag_id = {})",
            s.tag_id
        ),
        // recent, and anything unrecognised: the default library view.
        _ => format!("{LIVE} AND COALESCE(pm.archived, 0) = 0"),
    })
}

/// `... AND items.id IN (...)`, or a clause matching nothing when the list is
/// empty -- SQLite rejects `IN ()` outright, so an empty People cluster would
/// be a query error rather than an empty grid.
fn id_in(live: &str, ids: impl Iterator<Item = i64>) -> String {
    let list = ids.map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    if list.is_empty() {
        "0 = 1".into()
    } else {
        format!("{live} AND items.id IN ({list})")
    }
}

fn now_secs() -> i64 {
    chrono::Utc::now().timestamp()
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

/// The rule a smart album is stored with; None for an album filled by hand.
async fn smart_rule_of(pool: &sqlx::SqlitePool, album_id: i64) -> Result<Option<String>> {
    let rule: Option<Option<String>> =
        sqlx::query_scalar("SELECT smart_rule FROM albums WHERE id = ?")
            .bind(album_id)
            .fetch_optional(pool)
            .await?;
    Ok(rule.flatten())
}

/// The photos a stored rule matches right now.
async fn smart_match(pool: &sqlx::SqlitePool, rule: &str) -> Result<Vec<i64>> {
    let rule: serde_json::Value = serde_json::from_str(rule)?;
    let compiled = tulipix_photos::smart_albums::compile(&rule)?;
    tulipix_photos::smart_albums::matches(pool, &compiled).await
}

/// The rule for a new smart album, from the dialog's fields.
fn smart_rule(
    tag: &str,
    person: &str,
    camera: &str,
    year_from: i64,
    year_to: i64,
    starred: bool,
) -> Result<serde_json::Value> {
    use serde_json::json;
    let mut all = Vec::new();
    if !tag.trim().is_empty() {
        all.push(json!({ "tag": tag.trim() }));
    }
    if !person.trim().is_empty() {
        all.push(json!({ "person": person.trim() }));
    }
    if !camera.trim().is_empty() {
        all.push(json!({ "camera": camera.trim() }));
    }
    if year_from > 0 {
        all.push(json!({ "year": { ">=": year_from } }));
    }
    if year_to > 0 {
        all.push(json!({ "year": { "<=": year_to } }));
    }
    if starred {
        all.push(json!({ "starred": true }));
    }
    if all.is_empty() {
        anyhow::bail!("Give the smart album at least one rule");
    }
    let rule = json!({ "and": all });
    // Refused here rather than stored and failing every time it is opened.
    tulipix_photos::smart_albums::compile(&rule)?;
    Ok(rule)
}

async fn albums(pool: &sqlx::SqlitePool) -> Result<Vec<AlbumCard>> {
    let rows: Vec<(i64, String, Option<String>, i64, Option<i64>)> = sqlx::query_as(
        "SELECT a.id, a.name, a.smart_rule, \
                (SELECT COUNT(*) FROM album_items ai WHERE ai.album_id = a.id), \
                (SELECT ai.item_id FROM album_items ai \
                  WHERE ai.album_id = a.id ORDER BY ai.sort_key LIMIT 1) \
         FROM albums a ORDER BY a.updated DESC",
    )
    .fetch_all(pool)
    .await?;
    let mut rows = rows;
    // A smart album holds no album_items: its count and cover are whatever the
    // rule matches now.
    for r in rows.iter_mut() {
        if let Some(rule) = &r.2 {
            let ids = smart_match(pool, rule).await.unwrap_or_default();
            r.3 = ids.len() as i64;
            r.4 = ids.first().copied();
        }
    }
    let covers = covers(pool, rows.iter().filter_map(|r| r.4)).await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, smart, count, cover)| AlbumCard {
            id,
            name,
            count,
            cover: cover.and_then(|c| covers.get(&c).cloned()).unwrap_or_default(),
            smart: smart.is_some(),
        })
        .collect())
}

/// One row per face cluster. Prefers the cluster's cover face crop -- a People
/// grid made of whole photos is a grid of scenes, not of faces -- and falls
/// back to the person's newest photo when no crop has been rendered yet.
/// Order matches `ai::people::list`: named clusters first, then by id.
/// item_id -> the cheapest image on disk for a card cover: the cached grid
/// thumb when it has been rendered, else the original. One query for every
/// card on the page, rather than one per card.
///
/// A cover is a 150px circle or a 200px rectangle. Handing Dart the original
/// means decoding a 12 MB file to fill it, once per card, on every scroll --
/// which is exactly the stutter the thumb cache exists to prevent.
async fn covers(
    pool: &sqlx::SqlitePool,
    ids: impl Iterator<Item = i64>,
) -> Result<std::collections::HashMap<i64, String>> {
    let list = ids.map(|i| i.to_string()).collect::<Vec<_>>().join(",");
    if list.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let sql = format!("SELECT id, abs_path, mtime, size FROM items WHERE id IN ({list})");
    let rows = sqlx::query_as::<sqlx::Sqlite, (i64, String, i64, i64)>(sqlx::AssertSqlSafe(&*sql))
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, abs, mtime, size)| {
            let thumb =
                tulipix_photos::thumbs::thumb_path(Path::new(&abs), mtime, size as u64, GRID_DIM)
                    .filter(|p| p.exists())
                    .map(|p| p.to_string_lossy().into_owned());
            (id, thumb.unwrap_or(abs))
        })
        .collect())
}

/// Duplicate clusters, two members each. Mirrors the query
/// `tulipix_sec_photos::load_dedupe_groups` runs, including its 2,000-row cap:
/// a library with tens of thousands of near-duplicates should not be able to
/// turn one tab into a full-table scan.
async fn dedupe_pairs(pool: &sqlx::SqlitePool) -> Result<Vec<DedupePair>> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT dm.cluster_id, dc.kind, dm.item_id \
         FROM dedup_members dm JOIN dedup_clusters dc ON dc.id = dm.cluster_id \
         WHERE (SELECT COUNT(*) FROM dedup_members WHERE cluster_id = dm.cluster_id) >= 2 \
         ORDER BY dm.cluster_id, dm.item_id LIMIT 2000",
    )
    .fetch_all(pool)
    .await?;

    let mut grouped: std::collections::BTreeMap<i64, (String, Vec<i64>)> = Default::default();
    for (cid, kind, item_id) in rows {
        let e = grouped.entry(cid).or_insert_with(|| (kind, Vec::new()));
        if e.1.len() < 2 {
            e.1.push(item_id);
        }
    }
    let ids: Vec<i64> = grouped.values().flat_map(|(_, m)| m.iter().copied()).collect();
    let tiles = rows_for_ids(pool, &ids).await?;
    let by_id: std::collections::HashMap<i64, PhotoTile> =
        tiles.into_iter().map(|t| (t.item_id, t)).collect();

    Ok(grouped
        .into_iter()
        .filter_map(|(cluster_id, (kind, members))| {
            // A cluster whose second member has since been trashed is no longer
            // a decision to make, so it stops being a row.
            let left = by_id.get(members.first()?)?.clone();
            let right = by_id.get(members.get(1)?)?.clone();
            Some(DedupePair { cluster_id, kind, left, right })
        })
        .collect())
}

/// The MBTiles basemap, if the user has one. Looked for where the Slint build
/// looks: `<data>/maps/basemap.mbtiles`.
fn basemap_path() -> Option<PathBuf> {
    let p = tulipix_core::paths::data_dir()?.join("maps").join("basemap.mbtiles");
    p.exists().then_some(p)
}

/// Pins and the viewport that holds them. `precision` is the bucket size in
/// degrees; 0.5 is roughly a city, which is what the "1042 photos here" pin
/// pattern wants at an opening zoom.
async fn map_view(pool: &sqlx::SqlitePool) -> Result<(Vec<MapPin>, Option<MapBounds>)> {
    const PIN_PRECISION: f64 = 0.5;
    let clusters = tulipix_photos::map::cluster_pins(pool, PIN_PRECISION).await?;
    let covers = covers(pool, clusters.iter().map(|c| c.cover_item_id)).await?;
    let pins = clusters
        .into_iter()
        .map(|c| MapPin {
            lat: c.lat,
            lon: c.lon,
            count: c.count as i64,
            cover: covers.get(&c.cover_item_id).cloned().unwrap_or_default(),
        })
        .collect();
    let bounds = tulipix_photos::map::bbox_for_photos(pool).await?.map(|b| MapBounds {
        min_lat: b.min_lat,
        min_lon: b.min_lon,
        max_lat: b.max_lat,
        max_lon: b.max_lon,
    });
    Ok((pins, bounds))
}

/// The Library tab sorts folders, not photos. `name` there is a directory
/// name and `size` is the photo count's weight on disk, so it cannot share the
/// grid's comparator.
fn sort_folders(rows: &mut [FolderRow], mode: &str, dir: &str) {
    match mode {
        "count" => rows.sort_by_key(|f| f.count),
        "path" => rows.sort_by(|a, b| a.path.cmp(&b.path)),
        _ => rows.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase())),
    }
    if dir == "desc" {
        rows.reverse();
    }
}

async fn people(pool: &sqlx::SqlitePool) -> Result<Vec<PersonCard>> {
    let rows: Vec<(i64, Option<String>, i64, Option<String>, Option<i64>)> = sqlx::query_as(
        "SELECT p.id, p.name, \
                (SELECT COUNT(*) FROM faces f WHERE f.person_id = p.id), \
                (SELECT f.crop_path FROM faces f WHERE f.id = p.cover_face), \
                (SELECT f.item_id FROM faces f \
                   JOIN items ON items.id = f.item_id \
                  WHERE f.person_id = p.id ORDER BY items.added DESC LIMIT 1) \
         FROM people p ORDER BY (p.name IS NULL), p.name, p.id",
    )
    .fetch_all(pool)
    .await?;
    let crop_dir = tulipix_photos::ai::faces::face_thumbs_dir();
    let covers = covers(pool, rows.iter().filter_map(|r| r.4)).await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, count, crop, newest)| PersonCard {
            id,
            name: name.unwrap_or_default(),
            count,
            cover: crop
                .filter(|c| !c.is_empty())
                .and_then(|c| crop_dir.as_ref().map(|d| d.join(c)))
                .filter(|p| p.exists())
                .map(|p| p.to_string_lossy().into_owned())
                .or_else(|| newest.and_then(|n| covers.get(&n).cloned()))
                .unwrap_or_default(),
        })
        .collect())
}

/// One row per object tag. The filter mirrors `ai::tags::things` exactly --
/// live, untrashed, unarchived -- with the tag id and a cover added, which
/// that function does not return. If its WHERE clause changes, change this.
async fn things(pool: &sqlx::SqlitePool) -> Result<Vec<TagCard>> {
    let rows: Vec<(i64, String, i64, Option<i64>)> = sqlx::query_as(
        "SELECT t.id, t.name, COUNT(it.item_id) AS c, \
                (SELECT it2.item_id FROM item_tags it2 \
                   JOIN items ON items.id = it2.item_id \
                  WHERE it2.tag_id = t.id ORDER BY items.added DESC LIMIT 1) \
         FROM tags t \
         JOIN item_tags it ON it.tag_id = t.id \
         JOIN items i ON i.id = it.item_id \
         JOIN photo_meta pm ON pm.item_id = i.id \
         WHERE i.missing_since IS NULL AND pm.deleted_at IS NULL AND pm.archived = 0 \
         GROUP BY t.id HAVING c > 0 ORDER BY c DESC",
    )
    .fetch_all(pool)
    .await?;
    let covers = covers(pool, rows.iter().filter_map(|r| r.3)).await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, count, cover)| TagCard {
            id,
            name,
            count,
            cover: cover.and_then(|c| covers.get(&c).cloned()).unwrap_or_default(),
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

/// `pub(crate)` for Transfer: an upload landing in the inbox is what makes the
/// library look again, exactly as `refresh_library` does in the Slint build.
/// The fan-out to every section belongs to the shell (phase 4); Photos is the
/// only section this build has.
pub(crate) async fn scan_watched(pool: &sqlx::SqlitePool) {
    for dir in load_watched_folders() {
        scan_one(pool, &dir).await;
    }
}

/// One folder, read on its own -- a watched root, from Settings' per-folder
/// Rescan as well as from the loop above.
pub(crate) async fn scan_one(pool: &sqlx::SqlitePool, dir: &std::path::Path) {
    {
        emit(PhotosEvent::ScanStarted { root: dir.to_string_lossy().into_owned() });
        let lib = tulipix_core::libraries::Library {
            id: dir.to_string_lossy().into_owned(),
            path: dir.to_path_buf(),
            section: tulipix_core::libraries::Section::Photos,
            last_scan: None,
            item_count: 0,
            size_bytes: 0,
            exclude_globs: crate::api::maintenance::exclusions(),
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

// The watched-folder list is `tulipix_core::watched`'s; see there for why
// there is only one of it.
fn load_watched_folders() -> Vec<PathBuf> {
    tulipix_core::watched::load()
}

fn add_watched_folder(dir: &Path) -> bool {
    tulipix_common::add_watched_folder(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smart_rule_keeps_only_the_fields_given() {
        let r = smart_rule(" beach ", "", "", 2020, 0, true).unwrap();
        assert_eq!(
            r,
            serde_json::json!({ "and": [
                { "tag": "beach" },
                { "year": { ">=": 2020 } },
                { "starred": true },
            ] })
        );
        assert!(smart_rule("", " ", "", 0, 0, false).is_err(), "no rule at all is refused");
    }

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
            stack_size: 0,
            stack_id: 0,
            live: false,
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
    fn an_empty_id_list_is_a_clause_that_matches_nothing() {
        // SQLite rejects `IN ()` outright, so a person with no live photos
        // has to become a false predicate rather than a syntax error.
        assert_eq!(id_in("live", std::iter::empty()), "0 = 1");
        assert_eq!(id_in("live", [7, 9].into_iter()), "live AND items.id IN (7,9)");
    }
}
