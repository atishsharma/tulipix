//! `np.p4.music.folders` — folder browse view (disk hierarchy as-found).
//!
//! Groups tracks by their on-disk `folder` and resolves a per-folder cover
//! from the conventional sidecar names (folder.jpg / cover.jpg / front.png …),
//! falling back to embedded art. The cover-name precedence is pure logic and
//! unit-tested; the listing query reads `track_meta.folder`.

use anyhow::Result;
use sqlx::SqlitePool;

/// Sidecar cover filenames in precedence order.
pub const COVER_NAMES: &[&str] = &["cover.jpg", "cover.png", "folder.jpg", "folder.png", "front.jpg", "front.png", "album.jpg"];

/// Pick the best sidecar cover from a directory listing, honoring precedence.
/// Case-insensitive. Returns `None` → caller falls back to embedded art.
pub fn pick_cover(files_in_dir: &[String]) -> Option<&String> {
    for want in COVER_NAMES {
        if let Some(f) = files_in_dir.iter().find(|f| {
            std::path::Path::new(f).file_name().and_then(|n| n.to_str())
                .map(|n| n.eq_ignore_ascii_case(want)).unwrap_or(false)
        }) {
            return Some(f);
        }
    }
    None
}

/// Folders that contain tracks, with track counts, alphabetical.
pub async fn list(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT folder, COUNT(*) FROM track_meta
         JOIN items ON items.id = track_meta.item_id
         WHERE items.missing_since IS NULL AND folder IS NOT NULL
         GROUP BY folder ORDER BY folder COLLATE NOCASE",
    ).fetch_all(pool).await?)
}

/// Tracks in one folder, ordered by disc/track then path.
pub async fn tracks_in(pool: &SqlitePool, folder: &str) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT items.id, items.abs_path FROM track_meta
         JOIN items ON items.id = track_meta.item_id
         WHERE track_meta.folder = ? AND items.missing_since IS NULL
         ORDER BY track_meta.disc_no, track_meta.track_no, items.abs_path",
    ).bind(folder).fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use crate::{scan, tags::TrackTags};

    #[test]
    fn cover_precedence() {
        let files = vec!["/a/folder.jpg".to_string(), "/a/cover.png".to_string(), "/a/song.flac".to_string()];
        // cover.png beats folder.jpg by precedence order
        assert_eq!(pick_cover(&files), Some(&"/a/cover.png".to_string()));
        assert!(pick_cover(&["/a/song.flac".to_string()]).is_none());
    }

    #[tokio::test]
    async fn folders_grouped() {
        let (_t, pool) = open_pool().await;
        for p in ["/m/Band/Disc/1.flac", "/m/Band/Disc/2.flac"] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)").bind(p).execute(&pool).await.unwrap();
            let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p).fetch_one(&pool).await.unwrap();
            scan::upsert_track(&pool, id, p, &TrackTags::default()).await.unwrap();
        }
        let f = list(&pool).await.unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0], ("/m/Band/Disc".to_string(), 2));
        assert_eq!(tracks_in(&pool, "/m/Band/Disc").await.unwrap().len(), 2);
    }
}
