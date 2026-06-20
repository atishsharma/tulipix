//! Photos section extracted from tulipix-app: grid/timeline/library
//! population, category filter, selection + clipboard, dedupe/map cards, the
//! non-destructive editor, and the viewer's exif/histogram panel. The photo
//! `window.on_*` callbacks in main.rs reach these via `use tulipix_sec_photos::*`.

#![allow(clippy::too_many_arguments)]

use std::path::PathBuf;
use slint::{ComponentHandle, Model};
use tulipix_ui::*;
use tulipix_common::*;

// Map photo tile index → absolute path so the click handler can pop the
// viewer with the original (not the 320px thumb).
static PHOTO_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
pub fn photo_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    PHOTO_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Full (unfiltered) photo list for the grid: (label, original path, thumb path).
// Lets search rebuild the visible grid without re-scanning.
static PHOTO_FULL: std::sync::OnceLock<std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>>> = std::sync::OnceLock::new();
pub fn photo_full() -> &'static std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>> {
    PHOTO_FULL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Filter for the active photos category. `set` is membership; `order` (when
// non-empty) is the backend's display order so freshly-flagged items land at
// the top (e.g. star/trash sort newest-first). Recent leaves `order` empty so
// the grid keeps the natural scan order. `None` = no filter (show everything).
#[derive(Default, Clone)]
pub struct CatFilter {
    pub set: std::collections::HashSet<String>,
    pub order: Vec<String>,
}
static CATEGORY_PATHS: std::sync::OnceLock<std::sync::Mutex<Option<CatFilter>>> =
    std::sync::OnceLock::new();
pub fn category_paths() -> &'static std::sync::Mutex<Option<CatFilter>> {
    CATEGORY_PATHS.get_or_init(|| std::sync::Mutex::new(None))
}

/// Compute the `abs_path` filter for `category` by querying the photos DB.
/// Returns `None` only on a pool error for recent (so the grid never blanks);
/// otherwise `Some(CatFilter)`. Non-recent categories carry `order` (backend
/// recency) so newly-flagged photos appear at the top of the grid.
pub async fn category_path_set(category: &str) -> Option<CatFilter> {
    let pool = match pool_for("photos").await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(category, error = %e, "category: photos pool open failed");
            // Recent falls back to "show everything" so a DB hiccup never blanks
            // the main grid; the flag tabs fall back to empty.
            return if category == "recent" { None } else { Some(CatFilter::default()) };
        }
    };
    let paths: Vec<String> = match category {
        // Recent = the default library view: everything except trashed/archived.
        "recent" => sqlx::query_scalar(
            "SELECT items.abs_path FROM items \
             LEFT JOIN photo_meta ON photo_meta.item_id = items.id \
             WHERE items.missing_since IS NULL \
               AND photo_meta.deleted_at IS NULL \
               AND (photo_meta.archived IS NULL OR photo_meta.archived = 0)",
        )
        .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "recent query"); Vec::new() }),
        // Photos pinned into any manual album.
        "albums" => sqlx::query_scalar(
            "SELECT DISTINCT items.abs_path FROM album_items \
             JOIN items ON items.id = album_items.item_id",
        )
        .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "albums query"); Vec::new() }),
        // Photos taken on this month/day in any past year.
        "memories" => tulipix_photos::memories::on_this_day(&pool, now_secs())
            .await
            .map(|hits| hits.into_iter().map(|h| h.abs_path).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "memories query"); Vec::new() }),
        // Geotagged photos (EXIF GPS present).
        "places" => sqlx::query_scalar(
            "SELECT items.abs_path FROM items \
             JOIN photo_meta ON photo_meta.item_id = items.id \
             WHERE photo_meta.gps_lat IS NOT NULL AND photo_meta.gps_lon IS NOT NULL",
        )
        .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "places query"); Vec::new() }),
        // Starred / Archive / Trash — backed by the photos flag modules.
        "starred" => tulipix_photos::star::list(&pool, 100_000).await
            .map(|v| v.into_iter().map(|(_, p)| p).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "starred list"); Vec::new() }),
        "archive" => tulipix_photos::archive::list(&pool, 0, 100_000).await
            .map(|v| v.into_iter().map(|(_, p)| p).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "archive list"); Vec::new() }),
        "trash" => tulipix_photos::trash::list(&pool).await
            .map(|v| v.into_iter().map(|e| e.abs_path).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "trash list"); Vec::new() }),
        _ => Vec::new(),
    };
    // Recent keeps natural scan order (empty `order`); every other category
    // displays in the backend's order so flagged items surface at the top.
    let set = paths.iter().cloned().collect();
    let order = if category == "recent" { Vec::new() } else { paths };
    Some(CatFilter { set, order })
}

/// Resolve a photo's `items.id` from its absolute path.
pub async fn item_id_for(pool: &sqlx::SqlitePool, path: &std::path::Path) -> Option<i64> {
    let abs = path.to_string_lossy().into_owned();
    sqlx::query_scalar::<_, i64>("SELECT id FROM items WHERE abs_path = ?")
        .bind(abs).fetch_optional(pool).await.ok().flatten()
}

/// Phase 6 helpers — load auxiliary badge caches from the photos DB.
pub async fn reload_phase6_caches(pool: &sqlx::SqlitePool) {
    // Color labels
    let labels: Vec<(String, String)> = sqlx::query_as(
        "SELECT items.abs_path, COALESCE(pm.color_label, '') FROM items \
         JOIN photo_meta pm ON pm.item_id = items.id \
         WHERE pm.color_label IS NOT NULL AND pm.color_label != ''"
    ).fetch_all(pool).await.unwrap_or_default();
    if let Ok(mut g) = color_labels().lock() {
        g.clear();
        for (path, label) in labels { g.insert(path, label); }
    }
    // path → item_id
    let id_map: Vec<(String, i64)> = sqlx::query_as(
        "SELECT abs_path, id FROM items WHERE section = 'photos' AND missing_since IS NULL"
    ).fetch_all(pool).await.unwrap_or_default();
    if let Ok(mut g) = photo_item_ids().lock() {
        g.clear();
        for (path, id) in id_map { g.insert(path, id); }
    }
    // Live photo IDs
    let live = tulipix_photos::live_photos::detect_in_library(pool).await.unwrap_or_default();
    if let Ok(mut g) = live_ids().lock() {
        g.clear();
        for lp in live { g.insert(lp.still_item_id); }
    }
    // Stack info
    match tulipix_photos::stacks::list(pool).await {
        Ok(stacks) => {
            let hidden = tulipix_photos::stacks::hidden_ids(pool).await.unwrap_or_default();
            // Build cover_path → size and hidden_paths sets
            let id_map_snap = photo_item_ids().lock().map(|g| g.clone()).unwrap_or_default();
            let id_to_path: std::collections::HashMap<i64, String> =
                id_map_snap.iter().map(|(p, id)| (*id, p.clone())).collect();
            let mut cover_map: std::collections::HashMap<String, i32> = Default::default();
            let mut hidden_paths: std::collections::HashSet<String> = Default::default();
            for stack in &stacks {
                if let Some(cover_path) = id_to_path.get(&stack.cover_id) {
                    cover_map.insert(cover_path.clone(), stack.member_ids.len() as i32);
                }
            }
            for hid in hidden {
                if let Some(p) = id_to_path.get(&hid) { hidden_paths.insert(p.clone()); }
            }
            if let Ok(mut g) = stack_info().lock() { *g = (cover_map, hidden_paths); }
        }
        Err(e) => tracing::warn!(error=%e, "stacks::list failed"),
    }
}

/// Phase 6 — memory rail loading.
pub async fn load_memory_rails(pool: &sqlx::SqlitePool) -> Vec<(i32, String, Vec<String>)> {
    let hits = tulipix_photos::memories::on_this_day(pool, now_secs()).await.unwrap_or_default();
    let mut map: std::collections::BTreeMap<i32, Vec<String>> = std::collections::BTreeMap::new();
    for h in hits { map.entry(h.years_ago).or_default().push(h.abs_path); }
    map.into_iter().map(|(y, paths)| {
        let label = if y == 1 { "1 year ago".to_string() } else { format!("{y} years ago") };
        (y, label, paths)
    }).collect()
}

/// Phase 6 — map cluster loading. Returns (lat, lon, count, thumb_path, label) tuples (no slint::Image).
pub async fn load_map_clusters(pool: &sqlx::SqlitePool) -> Vec<(f32, f32, i32, PathBuf, String)> {
    let clusters = tulipix_photos::map::cluster_pins(pool, 1.0).await.unwrap_or_default();
    let mut out = Vec::with_capacity(clusters.len());
    for c in clusters {
        let cover_path: Option<String> = sqlx::query_scalar(
            "SELECT abs_path FROM items WHERE id = ?"
        ).bind(c.cover_item_id).fetch_optional(pool).await.ok().flatten();
        let thumb: PathBuf = cover_path.as_deref()
            .and_then(|p| {
                let full = photo_full().lock().ok()?;
                full.iter().find(|(_, orig, _)| orig.to_string_lossy() == p).map(|(_, _, th)| th.clone())
            })
            .unwrap_or_default();
        let label = format!("{:.1}°N {:.1}°E", c.lat, c.lon);
        out.push((c.lat as f32, c.lon as f32, c.count as i32, thumb, label));
    }
    out
}

/// Phase 6 — dedupe group loading. Returns tuples (no slint::Image) for thread safety.
/// Returns: (cluster_id, kind, left_thumb_path, right_thumb_path, left_label, right_label)
pub async fn load_dedupe_groups(pool: &sqlx::SqlitePool) -> Vec<(i32, String, PathBuf, PathBuf, String, String)> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT dm.cluster_id, dc.kind, dm.item_id \
         FROM dedup_members dm JOIN dedup_clusters dc ON dc.id = dm.cluster_id \
         WHERE (SELECT COUNT(*) FROM dedup_members WHERE cluster_id = dm.cluster_id) >= 2 \
         ORDER BY dm.cluster_id, dm.item_id LIMIT 2000"
    ).fetch_all(pool).await.unwrap_or_default();
    let mut cluster_map: std::collections::BTreeMap<i64, (String, Vec<i64>)> = Default::default();
    for (cid, kind, item_id) in rows {
        let e = cluster_map.entry(cid).or_insert_with(|| (kind, Vec::new()));
        if e.1.len() < 2 { e.1.push(item_id); }
    }
    let by_orig: std::collections::HashMap<i64, PathBuf> = {
        let id_map = photo_item_ids().lock().ok().map(|g| g.clone()).unwrap_or_default();
        let full = photo_full().lock().unwrap();
        let path_to_thumb: std::collections::HashMap<String, PathBuf> =
            full.iter().map(|(_, orig, th)| (orig.to_string_lossy().into_owned(), th.clone())).collect();
        drop(full);
        id_map.into_iter().filter_map(|(path, id)| {
            path_to_thumb.get(&path).map(|th| (id, th.clone()))
        }).collect()
    };
    let abs_paths: std::collections::HashMap<i64, String> =
        photo_item_ids().lock().ok().map(|g| g.iter().map(|(p, &id)| (id, p.clone())).collect()).unwrap_or_default();
    let fname = |p: &str| std::path::Path::new(p).file_name()
        .and_then(|s| s.to_str()).unwrap_or(p).to_string();
    let mut out = Vec::new();
    for (cluster_id, (kind, members)) in cluster_map {
        if members.len() < 2 { continue; }
        let left_id = members[0]; let right_id = members[1];
        let left_path = abs_paths.get(&left_id).cloned().unwrap_or_default();
        let right_path = abs_paths.get(&right_id).cloned().unwrap_or_default();
        let left_thumb = by_orig.get(&left_id).cloned().unwrap_or_default();
        let right_thumb = by_orig.get(&right_id).cloned().unwrap_or_default();
        out.push((cluster_id as i32, kind, left_thumb, right_thumb, fname(&left_path), fname(&right_path)));
    }
    out
}

/// Build `Vec<DedupeGroup>` from path tuples on the UI thread (where slint::Image is safe).
pub fn dedupe_groups_from_paths(data: Vec<(i32, String, PathBuf, PathBuf, String, String)>) -> Vec<DedupeGroup> {
    data.into_iter().map(|(cluster_id, kind, lp, rp, ll, rl)| DedupeGroup {
        cluster_id,
        kind: kind.into(),
        left_thumb: slint::Image::load_from_path(&lp).unwrap_or_default(),
        right_thumb: slint::Image::load_from_path(&rp).unwrap_or_default(),
        left_label: ll.into(),
        right_label: rl.into(),
    }).collect()
}

/// Build `Vec<MapCluster>` from path tuples on the UI thread.
pub fn map_clusters_from_paths(data: Vec<(f32, f32, i32, PathBuf, String)>) -> Vec<MapCluster> {
    data.into_iter().map(|(lat, lon, count, th, label)| MapCluster {
        lat, lon, count,
        cover: slint::Image::load_from_path(&th).unwrap_or_default(),
        label: label.into(),
    }).collect()
}

/// Recompute the allowed-path set for `category` off-thread, then rebuild the
/// photo grid (honouring `query`) on the UI thread. Shared by the category-tab
/// switch, right-click flag actions, and the post-scan refresh.
pub fn kick_category_refresh(weak: slint::Weak<MainWindow>, category: String, query: String) {
    // A rebuild re-orders photo_paths, so any selection indices are now stale.
    selection_clear();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Refresh starred membership so every grid colours its star correctly.
        // Also reload Phase 6 caches (labels, live IDs, stacks) on each grid refresh.
        if let Ok(pool) = pool_for("photos").await {
            if let Ok(list) = tulipix_photos::star::list(&pool, 1_000_000).await {
                if let Ok(mut g) = starred_paths().lock() {
                    *g = list.into_iter().map(|(_, p)| p).collect();
                }
            }
            reload_phase6_caches(&pool).await;
        }
        match category.as_str() {
            // Timeline = date-grouped grid, newest→oldest (the DB supplies the
            // order + month label; thumbs come from photo_full on the UI thread).
            "timeline" => {
                let order = timeline_order(&query).await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_timeline(&w, order, &query));
            }
            // A single library folder opened as a grouped grid, sorted per the
            // active sort mode.
            "folder" => {
                let folder = selected_folder().lock().map(|g| g.clone()).unwrap_or_default();
                let sort = sort_mode().lock().map(|g| g.clone()).unwrap_or_else(|_| "date".into());
                let dir = sort_dir().lock().map(|g| g.clone()).unwrap_or_else(|_| "desc".into());
                let order = folder_order(&folder, &sort, &dir).await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_timeline(&w, order, &query));
            }
            // Library = folder list, derived from the loaded photo grid.
            "library" => {
                let _ = weak.upgrade_in_event_loop(move |w| populate_library(&w, &query));
            }
            // People tab = face-cluster avatar cards (np.p2.ai.face-clusters).
            "people" => {
                let cards = load_people_cards().await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_people(&w, cards));
            }
            // Things tab = object-tag list (np.p2.ai.tags).
            "things" => {
                let things = load_things().await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_things(&w, things));
            }
            // Albums tab = album cover cards (np.p2.albums).
            "albums" => {
                let cards = load_albums().await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_albums(&w, cards));
            }
            // Person / tag / album drill-in — the open handler already stored the
            // path set in category_paths; just rebuild the flat grid from it.
            "facephotos" | "tagphotos" | "albumphotos" => {
                let _ = weak.upgrade_in_event_loop(move |w| apply_photo_filter(&w, &query));
            }
            // Phase 6 — Memories: year-rail view.
            "memories" => {
                if let Ok(pool) = pool_for("photos").await {
                    let rails_raw = load_memory_rails(&pool).await;
                    let weak2 = weak.clone();
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        use slint::{ModelRc, VecModel};
                        let rails: Vec<MemoryRail> = rails_raw.into_iter().map(|(year, ago, paths)| {
                            let starred = starred_snapshot();
                            let tiles: Vec<PhotoTile> = {
                                let full = photo_full().lock().unwrap();
                                let by_path: std::collections::HashMap<String, (String, std::path::PathBuf)> =
                                    full.iter().map(|e| (e.1.to_string_lossy().into_owned(), (e.0.clone(), e.2.clone()))).collect();
                                drop(full);
                                paths.iter().enumerate().filter_map(|(i, p)| {
                                    by_path.get(p).map(|(label, thumb)| {
                                        let color_label = color_labels().lock().ok()
                                            .and_then(|g| g.get(p).cloned())
                                            .unwrap_or_default();
                                        let item_id = photo_item_ids().lock().ok()
                                            .and_then(|g| g.get(p).copied());
                                        let is_live = item_id.map(|id| live_ids().lock().ok()
                                            .map(|g| g.contains(&id)).unwrap_or(false)).unwrap_or(false);
                                        let (stack_count, _hidden) = {
                                            
                                            stack_info().lock().ok()
                                                .map(|g| (g.0.get(p).copied().unwrap_or(0), g.1.contains(p)))
                                                .unwrap_or_default()
                                        };
                                        PhotoTile {
                                            thumb: slint::Image::load_from_path(thumb).unwrap_or_default(),
                                            label: label.clone().into(), col: i as i32, row: 0, index: i as i32,
                                            starred: starred.contains(p), selected: false,
                                            color_label: color_label.into(), is_live, stack_count, count: 0,
                                        }
                                    })
                                }).collect()
                            };
                            MemoryRail {
                                year,
                                ago: ago.into(),
                                tiles: ModelRc::new(VecModel::from(tiles)),
                            }
                        }).collect();
                        w.set_photo_memory_rails(ModelRc::new(VecModel::from(rails)));
                        let _ = weak2;
                    });
                }
            }
            // Phase 6 — Places: cluster grid.
            "places" => {
                if let Ok(pool) = pool_for("photos").await {
                    let cluster_data = load_map_clusters(&pool).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        use slint::{ModelRc, VecModel};
                        let clusters = map_clusters_from_paths(cluster_data);
                        w.set_photo_map_clusters(ModelRc::new(VecModel::from(clusters)));
                    });
                }
            }
            // Phase 6 — Dedupe: load pairs if any exist.
            "dedupe" => {
                if let Ok(pool) = pool_for("photos").await {
                    let group_data = load_dedupe_groups(&pool).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        use slint::{ModelRc, VecModel};
                        let groups = dedupe_groups_from_paths(group_data);
                        w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
                    });
                }
            }
            // Starred / Archive / Trash = flat filtered grid, sorted per the
            // active sort mode/direction (shared with the folder view).
            _ => {
                let mut set = category_path_set(&category).await;
                if let Some(cf) = set.as_mut() {
                    let sort = sort_mode().lock().map(|g| g.clone()).unwrap_or_else(|_| "date".into());
                    let dir = sort_dir().lock().map(|g| g.clone()).unwrap_or_else(|_| "desc".into());
                    sort_paths(&mut cf.order, &sort, &dir).await;
                }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    if let Ok(mut g) = category_paths().lock() { *g = set; }
                    apply_photo_filter(&w, &query);
                });
            }
        }
    });
}

// Library folder currently opened in the "folder" view + the active sort.
static SELECTED_FOLDER: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn selected_folder() -> &'static std::sync::Mutex<String> {
    SELECTED_FOLDER.get_or_init(|| std::sync::Mutex::new(String::new()))
}
static SORT_MODE: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn sort_mode() -> &'static std::sync::Mutex<String> {
    SORT_MODE.get_or_init(|| std::sync::Mutex::new("date".into()))
}
static SORT_DIR: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
pub fn sort_dir() -> &'static std::sync::Mutex<String> {
    SORT_DIR.get_or_init(|| std::sync::Mutex::new("desc".into()))
}
// Library folder-list sort: mode ("name" | "count") + direction.
static LIB_SORT: std::sync::OnceLock<std::sync::Mutex<(String, String)>> = std::sync::OnceLock::new();
pub fn lib_sort() -> &'static std::sync::Mutex<(String, String)> {
    LIB_SORT.get_or_init(|| std::sync::Mutex::new(("name".into(), "asc".into())))
}
// Starred abs_paths — refreshed before each grid rebuild so tiles can show a
// filled (yellow) vs hollow (white) star.
static STARRED_PATHS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();
pub fn starred_paths() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    STARRED_PATHS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}
pub fn starred_snapshot() -> std::collections::HashSet<String> {
    starred_paths().lock().map(|g| g.clone()).unwrap_or_default()
}

// Phase 6 — Color labels cache: abs_path → label string ("red", "orange", …, "").
static COLOR_LABELS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::OnceLock::new();
pub fn color_labels() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    COLOR_LABELS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

// Phase 6 — Live photo item IDs.
static LIVE_IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::OnceLock::new();
pub fn live_ids() -> &'static std::sync::Mutex<std::collections::HashSet<i64>> {
    LIVE_IDS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

// Phase 6 — Stack info: (cover_path → stack_size, hidden_paths).
static STACK_INFO: std::sync::OnceLock<std::sync::Mutex<(std::collections::HashMap<String, i32>, std::collections::HashSet<String>)>> =
    std::sync::OnceLock::new();
pub fn stack_info() -> &'static std::sync::Mutex<(std::collections::HashMap<String, i32>, std::collections::HashSet<String>)> {
    STACK_INFO.get_or_init(|| std::sync::Mutex::new((Default::default(), Default::default())))
}

// Phase 6 — path → item_id map for badge lookups.
static PHOTO_ITEM_IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
    std::sync::OnceLock::new();
pub fn photo_item_ids() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    PHOTO_ITEM_IDS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

// ── Multi-select (np.p2.multiselect) ───────────────────────────────────────
// Selected photo indices into the active `photo_paths` order, plus the range
// anchor (last toggled tile). Indices are cleared on every grid rebuild since a
// rebuild re-orders `photo_paths`.
#[derive(Default)]
pub struct SelState { pub sel: std::collections::HashSet<i32>, pub anchor: i32 }
static SELECTION: std::sync::OnceLock<std::sync::Mutex<SelState>> = std::sync::OnceLock::new();
pub fn selection() -> &'static std::sync::Mutex<SelState> {
    SELECTION.get_or_init(|| std::sync::Mutex::new(SelState::default()))
}
pub fn selection_clear() {
    if let Ok(mut g) = selection().lock() { g.sel.clear(); g.anchor = 0; }
}
pub fn selection_snapshot() -> std::collections::HashSet<i32> {
    selection().lock().map(|g| g.sel.clone()).unwrap_or_default()
}

// Clipboard for Copy/Cut → Paste. `(source paths, is_cut)`. Survives grid
// rebuilds so you can copy in one folder and paste in another.
static CLIPBOARD: std::sync::OnceLock<std::sync::Mutex<(Vec<PathBuf>, bool)>> = std::sync::OnceLock::new();
pub fn clipboard() -> &'static std::sync::Mutex<(Vec<PathBuf>, bool)> {
    CLIPBOARD.get_or_init(|| std::sync::Mutex::new((Vec::new(), false)))
}
pub fn clipboard_has() -> bool {
    clipboard().lock().map(|g| !g.0.is_empty()).unwrap_or(false)
}

thread_local! {
    // Single-shot timer that clears the tab "blink" confirmation. UI-thread only.
    pub static BLINK_TIMER: std::cell::RefCell<slint::Timer> = std::cell::RefCell::new(slint::Timer::default());
    // VecModels backing the currently-shown photo grids, so selection toggles
    // can repaint the `selected` flag in place (no image reload). UI-thread only.
    static GRID_MODELS: std::cell::RefCell<Vec<std::rc::Rc<slint::VecModel<PhotoTile>>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Reset the registered grid models (called at the start of every grid build).
pub fn grid_models_reset() { GRID_MODELS.with(|m| m.borrow_mut().clear()); }
/// Register a freshly-built grid VecModel for in-place selection repaint.
pub fn grid_models_push(m: std::rc::Rc<slint::VecModel<PhotoTile>>) {
    GRID_MODELS.with(|g| g.borrow_mut().push(m));
}

/// Repaint the `selected` flag on every visible tile from the selection set.
pub fn repaint_selection() {
    let sel = selection_snapshot();
    GRID_MODELS.with(|models| {
        for model in models.borrow().iter() {
            for r in 0..model.row_count() {
                if let Some(mut t) = model.row_data(r) {
                    let want = sel.contains(&t.index);
                    if t.selected != want { t.selected = want; model.set_row_data(r, t); }
                }
            }
        }
    });
}

/// Push selection count / mode / paste-availability to the UI, then repaint.
pub fn refresh_selection_meta(w: &MainWindow) {
    let count = selection().lock().map(|g| g.sel.len()).unwrap_or(0) as i32;
    let can_paste = clipboard_has() && w.get_photos_category() == "folder";
    w.set_photos_select_count(count);
    w.set_photos_selecting(count > 0);
    w.set_photos_can_paste(can_paste);
    repaint_selection();
}

/// Reorder `order` (abs_paths) by `sort`/`dir`. Default (date desc) is a no-op
/// since the lists already arrive newest-first.
pub async fn sort_paths(order: &mut [String], sort: &str, dir: &str) {
    if sort == "date" && dir == "desc" { return; }
    let map: std::collections::HashMap<String, (i64, i64)> =
        photos_meta().await.into_iter().map(|(p, dt, s)| (p, (dt, s))).collect();
    let fname = |p: &str| std::path::Path::new(p).file_name()
        .and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    order.sort_by(|a, b| match sort {
        "name" => fname(a).cmp(&fname(b)),
        "size" => map.get(a).map(|x| x.1).unwrap_or(0).cmp(&map.get(b).map(|x| x.1).unwrap_or(0)),
        _ => map.get(a).map(|x| x.0).unwrap_or(0).cmp(&map.get(b).map(|x| x.0).unwrap_or(0)),
    });
    if dir == "desc" { order.reverse(); }
}

/// (abs_path, date, size) for every live photo (trashed/archived excluded).
/// Date is taken_at, falling back to file mtime.
pub async fn photos_meta() -> Vec<(String, i64, i64)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    let rows: Vec<(String, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT items.abs_path, photo_meta.taken_at, items.mtime, items.size \
         FROM items LEFT JOIN photo_meta ON photo_meta.item_id = items.id \
         WHERE items.section = 'photos' AND items.missing_since IS NULL \
           AND photo_meta.deleted_at IS NULL \
           AND (photo_meta.archived IS NULL OR photo_meta.archived = 0)",
    )
    .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "photos_meta query"); Vec::new() });
    rows.into_iter().map(|(p, taken, mtime, size)| (p, taken.unwrap_or(mtime), size)).collect()
}

/// Timeline order — newest first, grouped by month.
pub async fn timeline_order(_query: &str) -> Vec<(String, String)> {
    let mut m = photos_meta().await;
    m.sort_by(|a, b| b.1.cmp(&a.1));
    m.into_iter().map(|(p, dt, _)| (p, month_label(dt))).collect()
}

/// Folder-view order — photos directly inside `folder`, ordered + grouped per
/// `sort` (date → month groups; name/size → a single group) and `dir`.
pub async fn folder_order(folder: &str, sort: &str, dir: &str) -> Vec<(String, String)> {
    let mut m: Vec<(String, i64, i64)> = photos_meta().await.into_iter()
        .filter(|(p, _, _)| {
            std::path::Path::new(p).parent()
                .map(|d| d.to_string_lossy() == folder).unwrap_or(false)
        })
        .collect();
    let fname = |p: &str| std::path::Path::new(p).file_name()
        .and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let asc = dir == "asc";
    let arrow = if asc { "▲" } else { "▼" };
    match sort {
        "name" => {
            m.sort_by_key(|a| fname(&a.0));
            if !asc { m.reverse(); }
            let label = format!("By name {arrow}");
            m.into_iter().map(|(p, _, _)| (p, label.clone())).collect()
        }
        "size" => {
            m.sort_by_key(|a| a.2);
            if !asc { m.reverse(); }
            let label = format!("By size {arrow}");
            m.into_iter().map(|(p, _, _)| (p, label.clone())).collect()
        }
        _ => {
            m.sort_by_key(|a| a.1);
            if !asc { m.reverse(); }
            m.into_iter().map(|(p, dt, _)| (p, month_label(dt))).collect()
        }
    }
}

/// "May 2026" for a unix timestamp (GMT).
pub fn month_label(unix: i64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|d| d.format("%B %Y").to_string())
        .unwrap_or_else(|| "Undated".into())
}

/// Build the timeline date groups from `order` + the cached thumbs, set them
/// on the window, and keep photo_paths in flattened group order for the viewer.
pub fn populate_timeline(w: &MainWindow, order: Vec<(String, String)>, query: &str) {
    use slint::{ModelRc, VecModel};
    let q = query.trim().to_lowercase();
    let cols = 6i32;
    let full = photo_full().lock().unwrap();
    let by_path: std::collections::HashMap<String, &(String, PathBuf, PathBuf)> =
        full.iter().map(|e| (e.1.to_string_lossy().into_owned(), e)).collect();
    let starred = starred_snapshot();

    grid_models_reset();
    let mut groups: Vec<PhotoGroup> = Vec::new();
    let mut cur_label = String::new();
    let mut cur: Vec<PhotoTile> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let flush = |label: &str, tiles: &mut Vec<PhotoTile>, groups: &mut Vec<PhotoGroup>| {
        if !tiles.is_empty() {
            // Hold the VecModel so selection can repaint the group's tiles.
            let model = std::rc::Rc::new(VecModel::from(std::mem::take(tiles)));
            grid_models_push(model.clone());
            groups.push(PhotoGroup { label: label.into(), tiles: ModelRc::from(model) });
        }
    };
    for (path, label) in &order {
        let Some((fname, orig, thumb)) = by_path.get(path).map(|e| (&e.0, &e.1, &e.2)) else { continue; };
        if !q.is_empty() && !fname.to_lowercase().contains(&q) { continue; }
        if *label != cur_label && !cur.is_empty() {
            flush(&cur_label, &mut cur, &mut groups);
        }
        cur_label = label.clone();
        let orig_str = orig.to_string_lossy().into_owned();
        let color_label = color_labels().lock().ok()
            .and_then(|g| g.get(&orig_str).cloned()).unwrap_or_default();
        let item_id_opt = photo_item_ids().lock().ok().and_then(|g| g.get(&orig_str).copied());
        let is_live = item_id_opt.map(|id| live_ids().lock().ok()
            .map(|g| g.contains(&id)).unwrap_or(false)).unwrap_or(false);
        let (stack_count, is_hidden) = stack_info().lock().ok()
            .map(|g| (g.0.get(&orig_str).copied().unwrap_or(0), g.1.contains(&orig_str)))
            .unwrap_or_default();
        if is_hidden { continue; }
        let gi = paths.len() as i32;
        cur.push(PhotoTile {
            thumb: slint::Image::load_from_path(thumb).unwrap_or_default(),
            label: fname.clone().into(),
            col: gi % cols, row: gi / cols, index: gi,
            starred: starred.contains(path),
            selected: false,
            color_label: color_label.into(),
            is_live,
            stack_count,
            count: 0,
        });
        paths.push(orig.clone());
    }
    flush(&cur_label, &mut cur, &mut groups);
    drop(full);
    *photo_paths().lock().unwrap() = paths;
    w.set_photo_groups(ModelRc::new(VecModel::from(groups)));
    refresh_selection_meta(w);
}

/// Build the Library folder list (one row per parent dir of the photo grid),
/// optionally filtered by `query` against the folder name.
pub fn populate_library(w: &MainWindow, query: &str) {
    use slint::{ModelRc, VecModel};
    let q = query.trim().to_lowercase();
    let full = photo_full().lock().unwrap();
    // dir → (count, first thumb path)
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, (i32, PathBuf)> = std::collections::HashMap::new();
    for (_, orig, thumb) in full.iter() {
        let Some(dir) = orig.parent() else { continue; };
        let key = dir.to_string_lossy().into_owned();
        let e = map.entry(key.clone()).or_insert_with(|| { order.push(key.clone()); (0, thumb.clone()) });
        e.0 += 1;
    }
    drop(full);
    let mut rows: Vec<FolderRow> = order.into_iter().filter_map(|path| {
        let (count, thumb) = map.remove(&path).unwrap();
        let name = std::path::Path::new(&path).file_name()
            .and_then(|s| s.to_str()).unwrap_or(&path).to_string();
        if !q.is_empty() && !name.to_lowercase().contains(&q) { return None; }
        Some(FolderRow {
            name: name.into(),
            path: path.into(),
            count,
            cover: slint::Image::load_from_path(&thumb).unwrap_or_default(),
        })
    }).collect();
    // Sort by name or photo count, per the library sort selector.
    let (mode, dir) = lib_sort().lock().map(|g| g.clone()).unwrap_or_else(|_| ("name".into(), "asc".into()));
    if mode == "count" {
        rows.sort_by_key(|a| a.count);
    } else {
        rows.sort_by_key(|a| a.name.to_lowercase());
    }
    if dir == "desc" { rows.reverse(); }
    w.set_photo_folders(ModelRc::new(VecModel::from(rows)));
}

/// Rebuild the photo grid from the retained full list, filtered by `query`
/// (case-insensitive filename match). Keeps `photo_paths` in sync for the viewer.
pub fn apply_photo_filter(w: &MainWindow, query: &str) {
    use slint::{ModelRc, VecModel};
    let q = query.trim().to_lowercase();
    let cols = 6i32;
    // Active category filter: None = recent fallback (all photos).
    let cat = category_paths().lock().ok().and_then(|g| g.clone());
    let full = photo_full().lock().unwrap();

    // Choose iteration order: the category's `order` (backend recency) when it
    // has one, else the natural scan order from photo_full.
    let ordered: Vec<&(String, PathBuf, PathBuf)> = match &cat {
        Some(c) if !c.order.is_empty() => {
            let by_path: std::collections::HashMap<String, &(String, PathBuf, PathBuf)> =
                full.iter().map(|e| (e.1.to_string_lossy().into_owned(), e)).collect();
            c.order.iter().filter_map(|p| by_path.get(p.as_str()).copied()).collect()
        }
        _ => full.iter().collect(),
    };
    let starred = starred_snapshot();

    let mut tiles: Vec<PhotoTile> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for (label, orig, thumb) in ordered {
        if !q.is_empty() && !label.to_lowercase().contains(&q) { continue; }
        let orig_str = orig.to_string_lossy().into_owned();
        if let Some(c) = &cat {
            if !c.set.contains(&orig_str) { continue; }
        }
        let color_label = color_labels().lock().ok()
            .and_then(|g| g.get(&orig_str).cloned()).unwrap_or_default();
        let item_id_opt = photo_item_ids().lock().ok().and_then(|g| g.get(&orig_str).copied());
        let is_live = item_id_opt.map(|id| live_ids().lock().ok()
            .map(|g| g.contains(&id)).unwrap_or(false)).unwrap_or(false);
        let (stack_count, is_hidden) = stack_info().lock().ok()
            .map(|g| (g.0.get(&orig_str).copied().unwrap_or(0), g.1.contains(&orig_str)))
            .unwrap_or_default();
        if is_hidden { continue; }
        let i = tiles.len() as i32;
        tiles.push(PhotoTile {
            thumb: slint::Image::load_from_path(thumb).unwrap_or_default(),
            label: label.clone().into(),
            col: i % cols, row: i / cols, index: i,
            starred: starred.contains(&orig_str),
            selected: false,
            color_label: color_label.into(),
            is_live,
            stack_count,
            count: 0,
        });
        paths.push(orig.clone());
    }
    drop(full);
    *photo_paths().lock().unwrap() = paths;
    grid_models_reset();
    let model = std::rc::Rc::new(VecModel::from(tiles));
    grid_models_push(model.clone());
    w.set_photo_tiles(ModelRc::from(model));
    refresh_selection_meta(w);
}

// ── People / Things tabs (np.p2.ai.face-clusters / .name / .tags) ──────────

/// Load face-cluster cards: (person_id, name, face_count, cover_thumb_abs_path).
/// Cover = the person's `cover_face` crop, falling back to any face crop.
pub async fn load_people_cards() -> Vec<(i64, Option<String>, i64, Option<String>)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    let people = match tulipix_photos::ai::people::list(&pool).await {
        Ok(p) => p,
        Err(e) => { tracing::warn!(error=%e, "people::list"); return Vec::new(); }
    };
    let dir = tulipix_photos::ai::faces::face_thumbs_dir();
    let mut out = Vec::with_capacity(people.len());
    for p in people {
        // Resolve a cover crop path (relative to face_thumbs_dir).
        let rel: Option<String> = if let Some(fid) = p.cover_face_id {
            sqlx::query_scalar::<_, String>("SELECT crop_path FROM faces WHERE id = ? AND crop_path <> ''")
                .bind(fid).fetch_optional(&pool).await.ok().flatten()
        } else { None };
        let rel = match rel {
            Some(r) => Some(r),
            None => sqlx::query_scalar::<_, String>(
                "SELECT crop_path FROM faces WHERE person_id = ? AND crop_path <> '' LIMIT 1")
                .bind(p.id).fetch_optional(&pool).await.ok().flatten(),
        };
        let cover = rel.and_then(|r| dir.as_ref().map(|d| d.join(r).to_string_lossy().into_owned()));
        out.push((p.id, p.name, p.face_count, cover));
    }
    out
}

/// Build the People tab model on the UI thread (Image loads must run here).
pub fn populate_people(w: &MainWindow, cards: Vec<(i64, Option<String>, i64, Option<String>)>) {
    use slint::{ModelRc, VecModel};
    let rows: Vec<PersonCard> = cards.into_iter().map(|(id, name, count, cover)| {
        let named = name.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
        PersonCard {
            id: id as i32,
            name: name.unwrap_or_default().into(),
            count: count as i32,
            cover: cover.and_then(|p| slint::Image::load_from_path(std::path::Path::new(&p)).ok())
                .unwrap_or_default(),
            named,
        }
    }).collect();
    w.set_photo_people(ModelRc::new(VecModel::from(rows)));
}

/// Build the Things tab model on the UI thread.
pub fn populate_things(w: &MainWindow, things: Vec<(String, i64)>) {
    use slint::{ModelRc, VecModel};
    let rows: Vec<ThingChip> = things.into_iter()
        .map(|(label, count)| ThingChip { label: label.into(), count: count as i32 })
        .collect();
    w.set_photo_things(ModelRc::new(VecModel::from(rows)));
}

/// Albums + cover path (cover_id's photo, else first member). (np.p2.albums)
pub async fn load_albums() -> Vec<(i64, String, i64, Option<String>)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    let albums = tulipix_photos::albums::list(&pool).await.unwrap_or_else(|e| {
        tracing::warn!(error=%e, "albums::list"); Vec::new()
    });
    let mut out = Vec::with_capacity(albums.len());
    for a in albums {
        let cover: Option<String> = match a.cover_id {
            Some(cid) => sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                .bind(cid).fetch_optional(&pool).await.ok().flatten(),
            None => sqlx::query_scalar(
                "SELECT i.abs_path FROM album_items ai JOIN items i ON i.id = ai.item_id \
                 WHERE ai.album_id = ? LIMIT 1")
                .bind(a.id).fetch_optional(&pool).await.ok().flatten(),
        };
        out.push((a.id, a.name, a.item_count, cover));
    }
    out
}

pub fn album_cards(cards: Vec<(i64, String, i64, Option<String>)>) -> Vec<AlbumCard> {
    cards.into_iter().map(|(id, name, count, cover)| AlbumCard {
        id: id as i32,
        name: name.into(),
        count: count as i32,
        cover: cover.and_then(|p| slint::Image::load_from_path(std::path::Path::new(&p)).ok()).unwrap_or_default(),
    }).collect()
}

/// Build the Albums tab model on the UI thread.
pub fn populate_albums(w: &MainWindow, cards: Vec<(i64, String, i64, Option<String>)>) {
    w.set_photo_albums(slint::ModelRc::new(slint::VecModel::from(album_cards(cards))));
}

/// Item ids for the current photo selection (grid index → path → items.id).
pub async fn selected_item_ids(pool: &sqlx::SqlitePool, paths: Vec<PathBuf>) -> Vec<i64> {
    let mut ids = Vec::with_capacity(paths.len());
    for p in paths { if let Some(id) = item_id_for(pool, &p).await { ids.push(id); } }
    ids
}

/// Snapshot the current selection's absolute paths (grid order).
pub fn selected_photo_paths() -> Vec<PathBuf> {
    let sel = selection().lock().map(|g| g.sel.clone()).unwrap_or_default();
    let pp = photo_paths().lock();
    match pp { Ok(g) => sel.iter().filter_map(|&i| g.get(i as usize).cloned()).collect(), Err(_) => Vec::new() }
}

pub async fn load_things() -> Vec<(String, i64)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    tulipix_photos::ai::tags::things(&pool).await
        .unwrap_or_else(|e| { tracing::warn!(error=%e, "tags::things"); Vec::new() })
}

/// Live abs_paths for every photo carrying tag `label` (newest first).
pub async fn tag_photo_paths(label: &str) -> Vec<String> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    sqlx::query_scalar::<_, String>(
        "SELECT i.abs_path FROM items i
         JOIN item_tags it ON it.item_id = i.id
         JOIN tags t ON t.id = it.tag_id
         JOIN photo_meta pm ON pm.item_id = i.id
         WHERE t.name = ? AND i.missing_since IS NULL
           AND pm.deleted_at IS NULL AND pm.archived = 0
         ORDER BY i.added DESC",
    ).bind(label).fetch_all(&pool).await
    .unwrap_or_else(|e| { tracing::warn!(error=%e, "tag_photo_paths"); Vec::new() })
}

// ── Photo editor state (np.p2.edit.*) ──────────────────────────────────────
static EDITOR_ITEM: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
pub fn editor_item() -> &'static std::sync::Mutex<Option<i64>> {
    EDITOR_ITEM.get_or_init(|| std::sync::Mutex::new(None))
}
static EDITOR_SRC: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
pub fn editor_src() -> &'static std::sync::Mutex<Option<PathBuf>> {
    EDITOR_SRC.get_or_init(|| std::sync::Mutex::new(None))
}
// Full-resolution working copy of the original. Ops apply here so geometric
// ops (crop/resize) use real pixel coordinates and export matches the preview;
// the preview frame is downscaled only for display.
static EDITOR_ORIG: std::sync::OnceLock<std::sync::Mutex<Option<image::DynamicImage>>> = std::sync::OnceLock::new();
pub fn editor_orig() -> &'static std::sync::Mutex<Option<image::DynamicImage>> {
    EDITOR_ORIG.get_or_init(|| std::sync::Mutex::new(None))
}
// Dimensions of the *current* edited result (after the active stack). Crop maps
// UI fractions against these.
static EDITOR_CUR_DIMS: std::sync::OnceLock<std::sync::Mutex<(u32, u32)>> = std::sync::OnceLock::new();
pub fn editor_cur_dims() -> &'static std::sync::Mutex<(u32, u32)> {
    EDITOR_CUR_DIMS.get_or_init(|| std::sync::Mutex::new((0, 0)))
}

/// Build a display-sized RGBA buffer (≤1600px long edge) from a result image,
/// returning the buffer plus the result's true full-res dimensions.
pub fn to_display_buffer(out: &image::DynamicImage) -> (slint::SharedPixelBuffer<slint::Rgba8Pixel>, u32, u32) {
    let (fw, fh) = (out.width(), out.height());
    let disp = if fw.max(fh) > 1600 { out.thumbnail(1600, 1600) } else { out.clone() };
    let rgba = disp.to_rgba8();
    let (dw, dh) = (rgba.width(), rgba.height());
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(dw, dh);
    buf.make_mut_bytes().copy_from_slice(rgba.as_raw());
    (buf, fw, fh)
}
static EDITOR_STACK: std::sync::OnceLock<std::sync::Mutex<tulipix_photos::editor::ops::EditStack>> = std::sync::OnceLock::new();
pub fn editor_stack() -> &'static std::sync::Mutex<tulipix_photos::editor::ops::EditStack> {
    EDITOR_STACK.get_or_init(|| std::sync::Mutex::new(Default::default()))
}
// True while the trailing Adjust op is the one the sliders are live-editing
// (so slider moves replace it instead of stacking a new op each tick).
pub static EDITOR_ADJUST_LIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn parse_preset(s: &str) -> Option<tulipix_photos::editor::filters::Preset> {
    use tulipix_photos::editor::filters::Preset;
    Some(match s {
        "bw" => Preset::BW, "sepia" => Preset::Sepia, "vintage" => Preset::Vintage,
        "drama" => Preset::Drama, "hdr" => Preset::Hdr, "polaroid" => Preset::Polaroid,
        "faded" => Preset::Faded, _ => return None,
    })
}

pub fn editor_reset_sliders(w: &MainWindow) {
    w.set_editor_exposure(0.0);
    w.set_editor_contrast(0.0);
    w.set_editor_saturation(0.0);
    w.set_editor_temperature(0.0);
    w.set_editor_highlights(0.0);
    w.set_editor_shadows(0.0);
}

/// Open the editor on the photo at `idx` (path already resolved): show the
/// overlay immediately, then load the saved edit stack + decode a preview.
pub fn open_editor(weak: slint::Weak<MainWindow>, _idx: i32, path: &std::path::Path) {
    let path = path.to_path_buf();
    if let Some(w) = weak.upgrade() {
        editor_reset_sliders(&w);
        w.set_editor_active_filter("".into());
        w.set_editor_status("".into());
        w.set_editor_filename(
            path.file_name().and_then(|s| s.to_str()).unwrap_or("").into());
        w.set_editor_busy(true);
        w.set_editor_open(true);
    }
    let weak2 = weak.clone();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let item_id = match pool_for("photos").await {
            Ok(pool) => item_id_for(&pool, &path).await,
            Err(_) => None,
        };
        let stack = match (item_id, pool_for("photos").await) {
            (Some(id), Ok(pool)) => tulipix_photos::editor::ops::load(&pool, id).await.unwrap_or_default(),
            _ => Default::default(),
        };
        let path2 = path.clone();
        // Decode at full resolution + build the "original" display frame.
        let decoded = tokio::task::spawn_blocking(move || {
            image::open(&path2).ok().map(|im| {
                let (buf, fw, fh) = to_display_buffer(&im);
                (im, buf, fw, fh)
            })
        }).await.ok().flatten();
        if let Ok(mut g) = editor_item().lock() { *g = item_id; }
        if let Ok(mut g) = editor_src().lock() { *g = Some(path); }
        if let Ok(mut g) = editor_stack().lock() { *g = stack; }
        if let Some((img, _, fw, fh)) = &decoded {
            if let Ok(mut g) = editor_orig().lock() { *g = Some(img.clone()); }
            if let Ok(mut g) = editor_cur_dims().lock() { *g = (*fw, *fh); }
        }
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        // Read EXIF for the Info viewer + Meta editor panels (off the UI thread).
        let exif_path = editor_src().lock().ok().and_then(|g| g.clone());
        let (dump, fields) = match exif_path {
            Some(p) => tokio::task::spawn_blocking(move || (format_exif(&p), exif_edit_fields(&p)))
                .await.unwrap_or_default(),
            None => (String::new(), Default::default()),
        };
        let orig_frame = decoded.map(|(_, buf, fw, fh)| (buf, fw, fh));
        let _ = weak2.upgrade_in_event_loop(move |w| {
            if let Some((buf, fw, fh)) = orig_frame {
                w.set_editor_original(slint::Image::from_rgba8(buf));
                w.set_editor_nat_w(fw as i32);
                w.set_editor_nat_h(fh as i32);
            }
            let (a, c, d, m, dt) = fields;
            w.set_editor_exif(dump.into());
            w.set_editor_exif_artist(a.into());
            w.set_editor_exif_copyright(c.into());
            w.set_editor_exif_description(d.into());
            w.set_editor_exif_comment(m.into());
            w.set_editor_exif_date(dt.into());
        });
        editor_render(weak2);
    });
}

/// Re-render the editor preview off-thread by applying the current stack to the
/// cached preview-scaled original, then push the frame + undo/redo flags.
pub fn editor_render(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = tokio::task::spawn_blocking(|| -> Option<(slint::SharedPixelBuffer<slint::Rgba8Pixel>, bool, bool, i32, u32, u32, slint::SharedPixelBuffer<slint::Rgba8Pixel>)> {
            let stack = editor_stack().lock().ok()?.clone();
            let orig = editor_orig().lock().ok()?.clone()?;
            let out = tulipix_photos::editor::ops::apply(orig.clone(), &stack).unwrap_or(orig);
            let (buf, fw, fh) = to_display_buffer(&out);
            let hist = histogram_buf(&out);
            if let Ok(mut g) = editor_cur_dims().lock() { *g = (fw, fh); }
            let can_undo = stack.undo_idx > 0;
            let can_redo = stack.undo_idx < stack.ops.len();
            Some((buf, can_undo, can_redo, stack.undo_idx as i32, fw, fh, hist))
        }).await.ok().flatten();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Some((buf, cu, cr, n, fw, fh, hist)) = res {
                w.set_editor_preview(slint::Image::from_rgba8(buf));
                w.set_editor_histogram(slint::Image::from_rgba8(hist));
                w.set_editor_can_undo(cu);
                w.set_editor_can_redo(cr);
                w.set_editor_op_count(n);
                // Crop/aspect math in the UI maps against the current result dims.
                w.set_editor_nat_w(fw as i32);
                w.set_editor_nat_h(fh as i32);
            }
            w.set_editor_busy(false);
        });
    });
}

/// Map an extension to an export format (for save-to-original).
pub fn format_for_ext(ext: &str) -> tulipix_photos::editor::export::ExportFormat {
    use tulipix_photos::editor::export::ExportFormat;
    match ext.to_ascii_lowercase().as_str() {
        "png" => ExportFormat::Png,
        "webp" => ExportFormat::Webp,
        "tif" | "tiff" => ExportFormat::Tiff,
        "heic" | "heif" => ExportFormat::Heic,
        _ => ExportFormat::Jpeg,
    }
}

/// First free `<stem>-NN.<ext>` (zero-padded) in `dir`, starting at 01.
pub fn incremented_stem(dir: &std::path::Path, base: &str, ext: &str) -> String {
    for n in 1..10_000 {
        let stem = format!("{base}-{n:02}");
        if !dir.join(format!("{stem}.{ext}")).exists() { return stem; }
    }
    format!("{base}-edit")
}

/// Resolve the super-resolution model path: `TULIPIX_SR_MODEL` env override,
/// else `<data>/models/swin2sr-x4.onnx`. None ⇒ no real model installed.
#[allow(dead_code)] // wired for the super-resolution feature (not yet on a UI path)
pub fn sr_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TULIPIX_SR_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() { return Some(p); }
    }
    let p = tulipix_photos::ai::models::models_root()?.join("swin2sr-x4.onnx");
    p.exists().then_some(p)
}

/// Build an upscaler: real ORT model when the `ai-onnx` feature is on and the
/// model is installed, else the Lanczos fallback. Returns `(impl, is_ai)`.
pub fn make_upscaler() -> (Box<dyn tulipix_photos::editor::upscale::Upscaler>, bool) {
    #[cfg(feature = "ai-onnx")]
    {
        if let Some(p) = sr_model_path() {
            match tulipix_photos::ai::onnx::OrtUpscaler::load(&p) {
                Ok(u) => return (Box::new(u), true),
                Err(e) => tracing::error!(error = %e, "OrtUpscaler load failed; using Lanczos"),
            }
        }
    }
    (Box::new(tulipix_photos::editor::upscale::NullUpscaler), false)
}

/// AI super-resolution (np.p2.edit.upscale). Applies the current edit stack on
/// the full-res original, runs the model (or Lanczos fallback), writes the
/// result next to the source, then rebases the editor onto that file so
/// further edits + export operate on the upscaled pixels.
pub fn editor_upscale(weak: slint::Weak<MainWindow>) {
    use image::GenericImageView;
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, image::DynamicImage, u32, u32, bool)> {
            let orig = editor_orig().lock().ok().and_then(|g| g.clone())
                .ok_or_else(|| anyhow::anyhow!("no image open"))?;
            let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
            let base = tulipix_photos::editor::ops::apply(orig.clone(), &stack).unwrap_or(orig);
            let (up, is_ai) = make_upscaler();
            let out = tulipix_photos::editor::upscale::apply(base, 4, up.as_ref())?;
            let (w, h) = out.dimensions();
            // Write next to the source as a PNG (lossless preserves the SR detail).
            let src = src.ok_or_else(|| anyhow::anyhow!("no source path"))?;
            let dir = src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base_stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
            let stem = incremented_stem(&dir, &format!("{base_stem}-upscaled"), "png");
            let dest = dir.join(format!("{stem}.png"));
            out.save(&dest)?;
            Ok((dest, out, w, h, is_ai))
        }).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            match result {
                Ok(Ok((dest, img, ow, oh, is_ai))) => {
                    // Rebase the editor onto the upscaled file.
                    if let Ok(mut g) = editor_src().lock() { *g = Some(dest.clone()); }
                    if let Ok(mut g) = editor_orig().lock() { *g = Some(img); }
                    if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
                    let tag = if is_ai { "AI upscaled" } else { "Lanczos upscaled (install model for AI)" };
                    w.set_editor_status(format!("{tag} → {ow}×{oh}").into());
                    editor_render(w.as_weak());
                    w.invoke_refresh_library();
                }
                Ok(Err(e)) => { tracing::error!(error = %e, "upscale"); w.set_editor_status(format!("Upscale failed: {e}").into()); w.set_editor_busy(false); }
                Err(e) => { tracing::error!(error = %e, "upscale join"); w.set_editor_status("Upscale failed.".into()); w.set_editor_busy(false); }
            }
        });
    });
}

/// DeOldify colourise model path: `TULIPIX_COLORIZE_MODEL` env override, else
/// `<data>/models/deoldify.onnx`.
#[allow(dead_code)] // wired for the colourise feature (not yet on a UI path)
pub fn colorize_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TULIPIX_COLORIZE_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() { return Some(p); }
    }
    let p = tulipix_photos::ai::models::models_root()?.join("deoldify.onnx");
    p.exists().then_some(p)
}

#[cfg(feature = "ai-onnx")]
pub fn make_coloriser() -> Option<Box<dyn tulipix_photos::editor::colorize::Coloriser>> {
    let p = colorize_model_path()?;
    match tulipix_photos::ai::onnx::OrtColoriser::load(&p) {
        Ok(c) => Some(Box::new(c)),
        Err(e) => { tracing::error!(error = %e, "OrtColoriser load failed"); None }
    }
}
#[cfg(not(feature = "ai-onnx"))]
pub fn make_coloriser() -> Option<Box<dyn tulipix_photos::editor::colorize::Coloriser>> { None }

/// AI colourisation (np.p2.edit.colorize). Applies the stack, runs DeOldify,
/// writes `<stem>-colorized-NN.png` next to the source, rebases the editor.
pub fn editor_colorize(weak: slint::Weak<MainWindow>) {
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, image::DynamicImage)> {
            let coloriser = make_coloriser().ok_or_else(|| anyhow::anyhow!(
                "DeOldify model not installed — put deoldify.onnx in <data>/models"))?;
            let orig = editor_orig().lock().ok().and_then(|g| g.clone())
                .ok_or_else(|| anyhow::anyhow!("no image open"))?;
            let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
            let base = tulipix_photos::editor::ops::apply(orig.clone(), &stack).unwrap_or(orig);
            let out = tulipix_photos::editor::colorize::apply(base, coloriser.as_ref())?;
            let src = src.ok_or_else(|| anyhow::anyhow!("no source path"))?;
            let dir = src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base_stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
            let stem = incremented_stem(&dir, &format!("{base_stem}-colorized"), "png");
            let dest = dir.join(format!("{stem}.png"));
            out.save(&dest)?;
            Ok((dest, out))
        }).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            match result {
                Ok(Ok((dest, img))) => {
                    if let Ok(mut g) = editor_src().lock() { *g = Some(dest); }
                    if let Ok(mut g) = editor_orig().lock() { *g = Some(img); }
                    if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
                    w.set_editor_status("Colorized".into());
                    editor_render(w.as_weak());
                    w.invoke_refresh_library();
                }
                Ok(Err(e)) => { tracing::error!(error = %e, "colorize"); w.set_editor_status(format!("Colorize failed: {e}").into()); w.set_editor_busy(false); }
                Err(e) => { tracing::error!(error = %e, "colorize join"); w.set_editor_status("Colorize failed.".into()); w.set_editor_busy(false); }
            }
        });
    });
}

/// "Save a copy" — decode the full-res original, apply the stack, and write a
/// new file *into the original's own folder* named `<stem>-NN.<ext>`.
pub fn editor_export(weak: slint::Weak<MainWindow>, format: String, quality: u8) {
    use tulipix_photos::editor::export::{ExportFormat, ExifPolicy, ExportSpec};
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
    let Some(src) = src else { return; };
    let fmt = match format.as_str() {
        "PNG" => ExportFormat::Png, "WEBP" => ExportFormat::Webp,
        "TIFF" => ExportFormat::Tiff, "HEIC" => ExportFormat::Heic,
        _ => ExportFormat::Jpeg,
    };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let src2 = src.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<PathBuf> {
            let img = image::open(&src2)?;
            let edited = tulipix_photos::editor::ops::apply(img, &stack)?;
            let out_dir = src2.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base = src2.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
            let stem = incremented_stem(&out_dir, &base, fmt.extension());
            let spec = ExportSpec {
                format: fmt, quality: quality.clamp(1, 100), exif: ExifPolicy::Preserve,
                out_dir, stem,
            };
            tulipix_photos::editor::export::write(&edited, Some(&src2), &spec)
        }).await;
        let ok = matches!(result, Ok(Ok(_)));
        let msg = match result {
            Ok(Ok(p)) => format!("Saved copy → {}", p.file_name().and_then(|s| s.to_str()).unwrap_or("")),
            Ok(Err(e)) => { tracing::error!(error=%e, "save copy"); "Save failed — see logs.".to_string() }
            Err(e) => { tracing::error!(error=%e, "save copy join"); "Save failed.".to_string() }
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_editor_status(msg.into());
            if ok { w.invoke_refresh_library(); } // background, no scan popup
        });
    });
}

/// "Save to original" — decode the full-res original, apply the stack, and
/// overwrite the source file in place (destructive). Clears the edit stack
/// since the pixels are now baked into the file.
pub fn editor_save_original(weak: slint::Weak<MainWindow>) {
    use tulipix_photos::editor::export::{ExifPolicy, ExportSpec};
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
    let Some(src) = src else { return; };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let src2 = src.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let img = image::open(&src2)?;
            let edited = tulipix_photos::editor::ops::apply(img, &stack)?;
            let ext = src2.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
            let out_dir = src2.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base = src2.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
            // export::write builds `<stem>.<fmt-ext>` in out_dir; matching the
            // format to the source extension makes it overwrite the original.
            let spec = ExportSpec {
                format: format_for_ext(ext), quality: 95, exif: ExifPolicy::Preserve,
                out_dir, stem: base,
            };
            tulipix_photos::editor::export::write(&edited, Some(&src2), &spec)?;
            Ok(())
        }).await;
        let item = editor_item().lock().map(|g| *g).unwrap_or(None);
        match result {
            Ok(Ok(())) => {
                // Pixels baked in → reset the stack + clear the DB edit row.
                if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
                if let Some(id) = item {
                    if let Ok(pool) = pool_for("photos").await {
                        let _ = tulipix_photos::editor::ops::save(&pool, id, &Default::default()).await;
                    }
                }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    editor_reset_sliders(&w);
                    w.set_editor_active_filter("".into());
                    w.set_editor_op_count(0);
                    w.set_editor_can_undo(false);
                    w.set_editor_can_redo(false);
                    w.set_editor_status("Saved to original.".into());
                    w.invoke_refresh_library(); // background refresh (no scan popup)
                });
            }
            Ok(Err(e)) => {
                tracing::error!(error=%e, "save original");
                let _ = weak.upgrade_in_event_loop(move |w| w.set_editor_status("Save failed — see logs.".into()));
            }
            Err(e) => {
                tracing::error!(error=%e, "save original join");
                let _ = weak.upgrade_in_event_loop(move |w| w.set_editor_status("Save failed.".into()));
            }
        }
    });
}

/// Distinct parent directories across the full photo list (= "N folders").
pub fn photo_folder_count() -> i32 {
    let full = photo_full().lock().unwrap();
    let mut parents: Vec<&std::path::Path> = full.iter().filter_map(|(_, o, _)| o.parent()).collect();
    parents.sort();
    parents.dedup();
    parents.len() as i32
}

/// Copy (or move, when `is_cut`) `srcs` into `target`, resolving name
/// collisions. Returns the number that landed. Cross-device moves fall back to
/// copy-then-delete. (np.p2.multiselect paste)
pub fn paste_into(srcs: &[PathBuf], target: &std::path::Path, is_cut: bool) -> usize {
    let mut done = 0usize;
    for src in srcs {
        // Skip a no-op paste into the file's own directory on copy.
        let Some(name) = src.file_name() else { continue; };
        let dest = unique_dest(&target.join(name));
        if src == &dest { continue; }
        let ok = if is_cut {
            std::fs::rename(src, &dest).is_ok()
                || (std::fs::copy(src, &dest).is_ok() && std::fs::remove_file(src).is_ok())
        } else {
            std::fs::copy(src, &dest).is_ok()
        };
        if ok { done += 1; } else { tracing::warn!(src = %src.display(), "paste failed"); }
    }
    done
}

/// First free path of the form `stem.ext`, `stem (1).ext`, `stem (2).ext`, …
pub fn unique_dest(dest: &std::path::Path) -> PathBuf {
    if !dest.exists() { return dest.to_path_buf(); }
    let parent = dest.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = dest.extension().and_then(|s| s.to_str());
    for n in 1..10_000 {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let cand = parent.join(name);
        if !cand.exists() { return cand; }
    }
    dest.to_path_buf()
}

/// The watched-folder root that contains `path` (so a paste rescans the right
/// library), falling back to the path's own directory.
pub fn library_root_for(_w: &MainWindow, path: &std::path::Path) -> Option<PathBuf> {
    for root in load_watched_folders() {
        if path.starts_with(&root) { return Some(root); }
    }
    Some(path.to_path_buf())
}

pub fn show_photo_at(w: &MainWindow, idx: i32) {
    let (path, total) = {
        let Ok(g) = photo_paths().lock() else { return; };
        let total = g.len() as i32;
        let Some(p) = g.get(idx as usize).cloned() else { return; };
        (p, total)
    };
    let img = slint::Image::load_from_path(&path).unwrap_or_default();
    let sz = img.size();
    w.set_viewer_image(img);
    w.set_viewer_nat_w(sz.width as i32);
    w.set_viewer_nat_h(sz.height as i32);
    w.set_viewer_label(path.file_name().and_then(|s| s.to_str()).unwrap_or("").into());
    w.set_viewer_index(idx);
    w.set_viewer_total(total);
    w.set_viewer_zoom(1.0); // reset zoom/pan on every photo change
    w.set_viewer_exif(format_exif(&path).into());
    w.set_viewer_histogram(histogram_image(&path));
}

/// Render a 256×100 RGB histogram for `path` into a Slint image. Channels are
/// drawn additively so overlapping bins brighten — the usual histogram look.
/// A decode failure yields a transparent image (the panel just shows empty).
// histogram_image / histogram_buf moved to tulipix_common (shared with Cloud).

/// Build the viewer's Info panel — an exiftool-style readout of ~24 common
/// attributes. Missing tags render as an empty "—" so the layout is stable.
/// ~24 common photo attributes as (label, value) pairs. Missing tags render
/// as "—". Shared by the viewer info panel (joined string) and the Properties
/// window (structured rows).
pub fn exif_rows(path: &std::path::Path) -> Vec<(&'static str, String)> {
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
            .and_then(|e| e.get_field(tag, In::PRIMARY).map(|f| f.display_value().with_unit(e).to_string()))
            .unwrap_or_else(|| "—".into())
    };
    let dim_str = dims.map(|(w, h)| format!("{w} × {h}")).unwrap_or_else(|| "—".into());
    vec![
        ("File",        path.file_name().and_then(|s| s.to_str()).unwrap_or("—").to_string()),
        ("Format",      if ext.is_empty() { "—".into() } else { ext }),
        ("Size",        human_size(size)),
        ("Dimensions",  dim_str),
        ("Date taken",  g(Tag::DateTimeOriginal)),
        ("Date digit.", g(Tag::DateTimeDigitized)),
        ("Camera make", g(Tag::Make)),
        ("Camera model",g(Tag::Model)),
        ("Lens",        g(Tag::LensModel)),
        ("ISO",         g(Tag::PhotographicSensitivity)),
        ("Aperture",    g(Tag::FNumber)),
        ("Shutter",     g(Tag::ExposureTime)),
        ("Exp. program",g(Tag::ExposureProgram)),
        ("Exp. comp.",  g(Tag::ExposureBiasValue)),
        ("Metering",    g(Tag::MeteringMode)),
        ("Flash",       g(Tag::Flash)),
        ("Focal length",g(Tag::FocalLength)),
        ("Focal 35mm",  g(Tag::FocalLengthIn35mmFilm)),
        ("White bal.",  g(Tag::WhiteBalance)),
        ("Color space", g(Tag::ColorSpace)),
        ("Orientation", g(Tag::Orientation)),
        ("GPS",         {
            let lat = g(Tag::GPSLatitude); let lon = g(Tag::GPSLongitude);
            if lat == "—" && lon == "—" { "—".into() } else { format!("{lat}, {lon}") }
        }),
        ("Software",    g(Tag::Software)),
        ("Artist",      g(Tag::Artist)),
    ]
}

pub fn format_exif(path: &std::path::Path) -> String {
    exif_rows(path).iter().map(|(k, v)| format!("{k:<13}{v}")).collect::<Vec<_>>().join("\n")
}

/// Read the five editable metadata fields for the editor's Meta panel.
/// Returns (artist, copyright, description, comment, date). Missing → "".
pub fn exif_edit_fields(path: &std::path::Path) -> (String, String, String, String, String) {
    use exif::{In, Reader, Tag};
    let meta = std::fs::File::open(path).ok().and_then(|f| {
        let mut br = std::io::BufReader::new(f);
        Reader::new().read_from_container(&mut br).ok()
    });
    let g = |tag: Tag| -> String {
        meta.as_ref()
            .and_then(|e| e.get_field(tag, In::PRIMARY).map(|f| f.display_value().to_string()))
            .map(|s| s.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    (
        g(Tag::Artist),
        g(Tag::Copyright),
        g(Tag::ImageDescription),
        g(Tag::UserComment),
        g(Tag::DateTimeOriginal),
    )
}
