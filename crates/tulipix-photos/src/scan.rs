//! Photo-section recursive scan + notify-based live watcher.
//!
//! Builds on `tulipix_core::populator` (the common file-proxy walker) and
//! `tulipix_core::watcher` (notify event stream). This module adds the
//! photos-specific extension filter, a default exclude pattern for sidecar
//! caches, and a stat→insert into `photo_meta` so the rest of the section can
//! query a single join.

use anyhow::Result;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::Path;
use tulipix_core::libraries::Library;
use tulipix_core::populator::{populate, PopulateStats};
use tulipix_core::watcher::{apply_event, FsEvent};

pub const PHOTO_EXTS: &[&str] = &[
    "jpg", "jpeg", "png", "webp", "gif", "heic", "heif", "avif", "jxl",
    "tif", "tiff", "bmp",
    "raw", "cr2", "cr3", "nef", "arw", "dng", "raf", "rw2", "orf", "pef",
];

pub fn is_photo(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            PHOTO_EXTS.iter().any(|x| **x == e)
        })
        .unwrap_or(false)
}

/// Scan one library, return PopulateStats. After the populator inserts rows
/// for everything in the directory, this prunes the non-photo `items` rows
/// from the photos DB (they survive elsewhere) and seeds an empty `photo_meta`
/// row for each surviving photo so the join in the timeline view is cheap.
pub async fn scan_library(pool: &SqlitePool, lib: &Library) -> Result<PopulateStats> {
    let stats = populate(pool, lib).await?;

    // Drop non-photo items from the photos DB.
    let prefix = format!("{}%", lib.path.to_string_lossy());
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, abs_path FROM items WHERE section = 'photos' AND abs_path LIKE ?",
    )
    .bind(&prefix)
    .fetch_all(pool)
    .await?;
    for (id, p) in rows {
        if !is_photo(Path::new(&p)) {
            sqlx::query("DELETE FROM items WHERE id = ?").bind(id).execute(pool).await?;
        } else {
            sqlx::query(
                "INSERT OR IGNORE INTO photo_meta (item_id) VALUES (?)",
            ).bind(id).execute(pool).await?;
        }
    }
    Ok(stats)
}

/// Apply one FS event coming from the realtime watcher. Renames update the
/// `items.abs_path` in place (preserving the photo_meta row); deletes flip
/// `missing_since` but do not drop the metadata. Created/Modified files only
/// get a `photo_meta` row if they have a photo extension.
pub async fn apply_fs_event(pool: &SqlitePool, event: &FsEvent) -> Result<()> {
    apply_event(pool, event).await?;
    match event {
        FsEvent::Created(p) | FsEvent::Modified(p)
            if is_photo(p) => {
                let abs = p.to_string_lossy().into_owned();
                let id: Option<i64> = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
                    .bind(&abs)
                    .fetch_optional(pool)
                    .await?;
                if let Some(id) = id {
                    sqlx::query("INSERT OR IGNORE INTO photo_meta (item_id) VALUES (?)")
                        .bind(id).execute(pool).await?;
                }
            }
        _ => {}
    }
    Ok(())
}

/// Resolve which item IDs in this library still need EXIF extraction (i.e.,
/// `photo_meta.taken_at IS NULL`). Used by the background indexer.
pub async fn unprocessed_ids(pool: &SqlitePool, limit: i64) -> Result<HashSet<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT items.id
         FROM items
         JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.section = 'photos'
           AND items.missing_since IS NULL
           AND photo_meta.taken_at IS NULL
         LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use std::path::PathBuf;
    use tulipix_core::libraries::{ScanCadence, Section};

    fn mk_lib(path: PathBuf) -> Library {
        Library {
            id: "lib1".into(),
            path, section: Section::Photos,
            last_scan: None, item_count: 0, size_bytes: 0,
            exclude_globs: vec![],
            cadence_override: Some(ScanCadence::Manual),
            realtime_notify: false,
        }
    }

    #[tokio::test]
    async fn scan_keeps_photos_drops_other() {
        let (_t, pool) = open_pool().await;
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("a.jpg"), b"x").unwrap();
        std::fs::write(root.path().join("b.txt"), b"y").unwrap();
        let lib = mk_lib(root.path().to_path_buf());
        let _ = scan_library(&pool, &lib).await.unwrap();
        let photos: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM items").fetch_one(&pool).await.unwrap();
        assert_eq!(photos, 1, "non-photo rows pruned from photos DB");
        let meta: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM photo_meta").fetch_one(&pool).await.unwrap();
        assert_eq!(meta, 1);
    }

    #[tokio::test]
    async fn ext_filter_covers_common_extensions() {
        for e in ["jpg", "JPG", "heic", "ARW", "dng"] {
            let p = PathBuf::from(format!("/tmp/x.{e}"));
            assert!(is_photo(&p), "{e} should be a photo");
        }
        assert!(!is_photo(Path::new("/tmp/x.txt")));
    }
}
