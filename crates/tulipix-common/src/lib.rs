//! Shared app infrastructure for the section-wiring crates.
//!
//! Owns the per-section SQLite pool cache (`pool_for`), the data-directory
//! helpers, and the bundled-binary probes — everything a section crate needs
//! that isn't section-specific, without depending on `tulipix-app`.

use anyhow::Result;
use std::path::PathBuf;
use std::sync::OnceLock;

static PHOTOS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static VIDEOS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static MUSIC_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
static BOOKS_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
static CLOUD_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
// Music sub-sections split out of music.db (no items FK — self-contained).
static PODCASTS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static RADIO_POOL:    OnceLock<sqlx::SqlitePool> = OnceLock::new();
static YOUTUBE_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
// Tools job queue (tools.db) — shared by the GUI Tools section + CLI.
static TOOLS_POOL:    OnceLock<sqlx::SqlitePool> = OnceLock::new();

/// Open (or return the cached) SQLite pool for a section, applying its schema
/// on first open. Both the GUI and CLI go through here so every front-end sees
/// the same DB + migrations.
pub async fn pool_for(section: &str) -> Result<sqlx::SqlitePool> {
    let cache = match section {
        "photos" => &PHOTOS_POOL,
        "videos" => &VIDEOS_POOL,
        "music"  => &MUSIC_POOL,
        "books"  => &BOOKS_POOL,
        "cloud"  => &CLOUD_POOL,
        "podcasts" => &PODCASTS_POOL,
        "radio"    => &RADIO_POOL,
        "youtube"  => &YOUTUBE_POOL,
        "tools"    => &TOOLS_POOL,
        _ => anyhow::bail!("unknown section"),
    };
    if let Some(p) = cache.get() { return Ok(p.clone()); }
    let handle = tulipix_core::db::DbHandle::open(section)?;
    let pool = handle.init_pool().await?;
    match section {
        "photos" => {
            tulipix_photos::schema::apply(&pool).await?;
            // Phase 6 migrations (safe — ALTER TABLE IF NOT EXISTS equivalent).
            let _ = sqlx::query("ALTER TABLE photo_meta ADD COLUMN color_label TEXT").execute(&pool).await;
            // Apply stacks schema (idempotent CREATE TABLE IF NOT EXISTS).
            let _ = tulipix_photos::stacks::apply_schema(&pool).await;
            // Dedup tables (created by build_clusters; ensure they exist).
            let _ = sqlx::query(
                "CREATE TABLE IF NOT EXISTS dedup_clusters (
                    id      INTEGER PRIMARY KEY AUTOINCREMENT,
                    kind    TEXT NOT NULL,
                    key     TEXT NOT NULL,
                    created INTEGER NOT NULL,
                    UNIQUE(kind, key)
                )"
            ).execute(&pool).await;
            let _ = sqlx::query(
                "CREATE TABLE IF NOT EXISTS dedup_members (
                    cluster_id INTEGER NOT NULL REFERENCES dedup_clusters(id) ON DELETE CASCADE,
                    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                    PRIMARY KEY (cluster_id, item_id)
                )"
            ).execute(&pool).await;
        }
        "videos" => tulipix_videos::schema::apply(&pool).await?,
        "music"  => tulipix_music::schema::apply(&pool).await?,
        "podcasts" => {
            tulipix_music::podcasts::apply_schema(&pool).await?;
            migrate_split_from_music(&pool, &[
                ("podcasts",
                 "id, feed_url, title, author, image_url, category, description, last_checked"),
                ("podcast_episodes",
                 "id, podcast_id, guid, title, audio_url, published, duration_s, \
                  description, image_url, downloaded_path, position_s, played"),
            ]).await;
        }
        "radio" => {
            tulipix_music::radio::apply_schema(&pool).await?;
            migrate_split_from_music(&pool, &[
                ("radio_stations",
                 "id, station_uuid, name, url, favicon, country, tags, favourite"),
            ]).await;
        }
        "youtube" => {
            tulipix_music::youtube::store::apply_schema(&pool).await?;
        }
        "books"  => tulipix_books::schema::apply(&pool).await?,
        "cloud"  => tulipix_cloud::schema::apply(&pool).await?,
        "tools"  => tulipix_tools::schema::apply(&pool).await?,
        _ => {}
    }
    let _ = cache.set(pool.clone());
    Ok(pool)
}

/// One-time migration: move `tables` (each `(name, explicit_column_list)`) out
/// of the legacy shared `music.db` into the freshly-opened section `dest` pool.
/// Idempotent: no-ops once the source tables are gone.
async fn migrate_split_from_music(dest: &sqlx::SqlitePool, tables: &[(&str, &str)]) {
    let Some(music_path) = tulipix_core::paths::db_path("music") else { return; };
    if !music_path.exists() { return; }
    let Ok(mut conn) = dest.acquire().await else { return; };
    if sqlx::query(&format!("ATTACH DATABASE '{}' AS legacy", music_path.display()))
        .execute(&mut *conn).await.is_err() { return; }
    let mut moved_any = false;
    for (table, cols) in tables {
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM legacy.sqlite_master WHERE type='table' AND name = ?")
            .bind(*table).fetch_optional(&mut *conn).await.ok().flatten();
        if exists.is_none() { continue; }
        moved_any = true;
        let _ = sqlx::query(&format!(
            "INSERT OR IGNORE INTO {table} ({cols}) SELECT {cols} FROM legacy.{table}"))
            .execute(&mut *conn).await;
    }
    if moved_any {
        for (table, _) in tables.iter().rev() {
            let _ = sqlx::query(&format!("DROP TABLE IF EXISTS legacy.{table}")).execute(&mut *conn).await;
        }
        tracing::info!("migrated {} table(s) out of music.db into a split section DB", tables.len());
    }
    let _ = sqlx::query("DETACH DATABASE legacy").execute(&mut *conn).await;
}

/// App data dir (`<config>/Tulipix`), per OS.
pub fn dirs_default() -> Option<std::path::PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    };
    base.map(|b| b.join("Tulipix"))
}

/// The user's Documents folder (best effort), home as the fallback.
pub fn dirs_default_documents() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let docs = home.join("Documents");
    if docs.is_dir() { docs } else { home }
}

/// Directory holding the per-OS bundled binaries (dev layout).
pub fn bundled_bin_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/bin/linux-x86_64"))
}

/// Is `file` present in the bundled-binary dir?
pub fn bundled_present(file: &str) -> bool { bundled_bin_dir().join(file).exists() }

/// Is `name` resolvable on PATH?
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").map(|paths| {
        std::env::split_paths(&paths).any(|d| d.join(name).exists())
    }).unwrap_or(false)
}

// ---- Cross-section image helpers ------------------------------------------
// Shared by the Cloud preview, the Photos viewer/properties panels, and the
// editor curve grid — so they live here rather than in any one section crate.

/// Human-readable byte count: scaled unit + thousands-separated raw bytes.
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    let (val, unit) = if b >= 1_073_741_824.0 { (b / 1_073_741_824.0, "GB") }
        else if b >= 1_048_576.0 { (b / 1_048_576.0, "MB") }
        else if b >= 1024.0 { (b / 1024.0, "KB") }
        else { (b, "bytes") };
    // Thousands-separated raw byte count.
    let mut raw = String::new();
    let digits = bytes.to_string();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { raw.push(','); }
        raw.push(c);
    }
    if unit == "bytes" { format!("{raw} bytes") } else { format!("{val:.2} {unit} ({raw} bytes)") }
}

/// Decode `path` and render its histogram; an empty 256×100 buffer on failure.
pub fn histogram_image(path: &std::path::Path) -> slint::Image {
    let Ok(img) = image::open(path) else {
        use slint::{Rgba8Pixel, SharedPixelBuffer};
        return slint::Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::new(256, 100));
    };
    slint::Image::from_rgba8(histogram_buf(&img))
}

/// Render a 256×100 additive RGB histogram buffer from an already-decoded
/// image. Shared by the viewer/properties panels and the editor curve grid.
pub fn histogram_buf(img: &image::DynamicImage) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    use slint::{Rgba8Pixel, SharedPixelBuffer};
    const W: usize = 256;
    const H: usize = 100;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(W as u32, H as u32);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() { *p = Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 }; }

    let small = img.thumbnail(256, 256).to_rgb8();
    let (mut rh, mut gh, mut bh) = ([0u32; 256], [0u32; 256], [0u32; 256]);
    for p in small.pixels() {
        rh[p[0] as usize] += 1; gh[p[1] as usize] += 1; bh[p[2] as usize] += 1;
    }
    let maxv = rh.iter().chain(&gh).chain(&bh).copied().max().unwrap_or(1).max(1);
    for x in 0..W {
        for (count, (cr, cg, cb)) in [
            (rh[x], (210u16, 40, 40)),
            (gh[x], (40, 200, 90)),
            (bh[x], (50, 120, 230)),
        ] {
            let bar = ((count as f64 / maxv as f64) * (H as f64 - 1.0)).round() as usize;
            for y in (H - bar)..H {
                let idx = y * W + x;
                let c = px[idx];
                px[idx] = Rgba8Pixel {
                    r: (c.r as u16 + cr).min(255) as u8,
                    g: (c.g as u16 + cg).min(255) as u8,
                    b: (c.b as u16 + cb).min(255) as u8,
                    a: 235,
                };
            }
        }
    }
    buf
}

// ---- Watched folders + time (shared by every media section) ----------------
/// JSON file holding the list of watched root folders, so libraries survive
/// restarts (the grids re-scan from these on launch).
pub fn watched_folders_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

pub fn load_watched_folders() -> Vec<PathBuf> {
    let Some(p) = watched_folders_path() else { return Vec::new(); };
    let Ok(body) = std::fs::read_to_string(&p) else { return Vec::new(); };
    serde_json::from_str::<Vec<String>>(&body)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
