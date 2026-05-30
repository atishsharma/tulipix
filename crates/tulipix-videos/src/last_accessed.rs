//! Recent rail — `video_meta.last_accessed` index.
//!
//! `touch()` should be called when playback starts. `recent()` returns the
//! top-N most recently touched items, optionally excluding finished.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentItem {
    pub item_id: i64,
    pub abs_path: String,
    pub last_accessed: i64,
    pub position_s: Option<f64>,
    pub duration_s: Option<f64>,
    pub finished: bool,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn touch(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE video_meta SET last_accessed = ? WHERE item_id = ?")
        .bind(now()).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn recent(pool: &SqlitePool, limit: i64, exclude_finished: bool) -> Result<Vec<RecentItem>> {
    let mut sql = String::from(
        "SELECT items.id, items.abs_path, video_meta.last_accessed,
                wp.position_s, wp.duration_s, COALESCE(wp.finished, 0)
         FROM video_meta
         JOIN items ON items.id = video_meta.item_id
         LEFT JOIN watch_progress wp ON wp.item_id = items.id
         WHERE video_meta.last_accessed IS NOT NULL
           AND items.missing_since IS NULL",
    );
    if exclude_finished {
        sql.push_str(" AND COALESCE(wp.finished, 0) = 0");
    }
    sql.push_str(" ORDER BY video_meta.last_accessed DESC LIMIT ?");
    let rows: Vec<(i64, String, i64, Option<f64>, Option<f64>, i64)> = sqlx::query_as(&sql)
        .bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(item_id, abs_path, last_accessed, position_s, duration_s, fin)| RecentItem {
        item_id, abs_path, last_accessed, position_s, duration_s, finished: fin != 0,
    }).collect())
}

/// Drop `last_accessed` entries older than `cutoff_unix`. Useful for a
/// rolling-window Recent rail (e.g. last 90 days).
pub async fn prune(pool: &SqlitePool, cutoff_unix: i64) -> Result<u64> {
    let r = sqlx::query("UPDATE video_meta SET last_accessed = NULL WHERE last_accessed < ?")
        .bind(cutoff_unix).execute(pool).await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn add(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'videos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO video_meta (item_id) VALUES (?)").bind(id).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn touch_orders_recent_desc() {
        let (_t, pool) = open_pool().await;
        let a = add(&pool, "/a.mkv").await;
        let b = add(&pool, "/b.mkv").await;
        touch(&pool, a).await.unwrap();
        // backdate to force ordering
        sqlx::query("UPDATE video_meta SET last_accessed = 100 WHERE item_id = ?").bind(a).execute(&pool).await.unwrap();
        sqlx::query("UPDATE video_meta SET last_accessed = 200 WHERE item_id = ?").bind(b).execute(&pool).await.unwrap();
        let r = recent(&pool, 10, false).await.unwrap();
        assert_eq!(r[0].item_id, b);
        assert_eq!(r[1].item_id, a);
    }

    #[tokio::test]
    async fn exclude_finished_hides_them() {
        let (_t, pool) = open_pool().await;
        let a = add(&pool, "/a.mkv").await;
        let b = add(&pool, "/b.mkv").await;
        sqlx::query("UPDATE video_meta SET last_accessed = 100 WHERE item_id IN (?, ?)").bind(a).bind(b).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO watch_progress (item_id, position_s, finished, updated) VALUES (?, 0, 1, 0)").bind(a).execute(&pool).await.unwrap();
        let r = recent(&pool, 10, true).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].item_id, b);
    }

    #[tokio::test]
    async fn prune_clears_old_rows() {
        let (_t, pool) = open_pool().await;
        let a = add(&pool, "/a.mkv").await;
        sqlx::query("UPDATE video_meta SET last_accessed = 100 WHERE item_id = ?").bind(a).execute(&pool).await.unwrap();
        let n = prune(&pool, 1000).await.unwrap();
        assert_eq!(n, 1);
        assert!(recent(&pool, 10, false).await.unwrap().is_empty());
    }
}
