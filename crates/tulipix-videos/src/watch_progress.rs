//! Local watch_progress — resume-from-position store.
//!
//! Each player update writes the current playhead, and the row flips to
//! `finished = 1` once playback crosses `FINISHED_THRESHOLD` of the duration
//! (95% by default). The Recent rail joins on `updated DESC`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const FINISHED_THRESHOLD: f64 = 0.95;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Progress {
    pub item_id: i64,
    pub position_s: f64,
    pub duration_s: Option<f64>,
    pub finished: bool,
    pub updated: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Write current playback position. If `duration_s` is known and `position_s`
/// is past `FINISHED_THRESHOLD`, the row flips to finished.
pub async fn update(pool: &SqlitePool, item_id: i64, position_s: f64, duration_s: Option<f64>) -> Result<()> {
    let finished: i64 = match duration_s {
        Some(d) if d > 0.0 && position_s / d >= FINISHED_THRESHOLD => 1,
        _ => 0,
    };
    let pos = position_s.max(0.0);
    sqlx::query(
        "INSERT INTO watch_progress (item_id, position_s, duration_s, finished, updated)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            position_s = excluded.position_s,
            duration_s = excluded.duration_s,
            finished   = MAX(watch_progress.finished, excluded.finished),
            updated    = excluded.updated",
    ).bind(item_id).bind(pos).bind(duration_s).bind(finished).bind(now())
    .execute(pool).await?;
    Ok(())
}

/// Mark item explicitly finished/unfinished (UI "Mark as watched").
pub async fn mark_finished(pool: &SqlitePool, item_id: i64, finished: bool) -> Result<()> {
    sqlx::query(
        "INSERT INTO watch_progress (item_id, position_s, finished, updated)
         VALUES (?, 0, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET finished = excluded.finished, updated = excluded.updated",
    ).bind(item_id).bind(if finished { 1 } else { 0 }).bind(now())
    .execute(pool).await?;
    Ok(())
}

pub async fn resume(pool: &SqlitePool, item_id: i64) -> Result<Option<f64>> {
    let row: Option<(f64, i64)> = sqlx::query_as(
        "SELECT position_s, finished FROM watch_progress WHERE item_id = ?",
    ).bind(item_id).fetch_optional(pool).await?;
    Ok(match row {
        Some((pos, fin)) if fin == 0 && pos > 0.0 => Some(pos),
        _ => None,
    })
}

pub async fn get(pool: &SqlitePool, item_id: i64) -> Result<Option<Progress>> {
    let row: Option<(i64, f64, Option<f64>, i64, i64)> = sqlx::query_as(
        "SELECT item_id, position_s, duration_s, finished, updated FROM watch_progress WHERE item_id = ?",
    ).bind(item_id).fetch_optional(pool).await?;
    Ok(row.map(|(item_id, position_s, duration_s, finished, updated)| Progress {
        item_id, position_s, duration_s, finished: finished != 0, updated,
    }))
}

pub async fn clear(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM watch_progress WHERE item_id = ?").bind(item_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn add_item(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'videos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn update_then_resume() {
        let (_t, pool) = open_pool().await;
        let id = add_item(&pool, "/a.mkv").await;
        update(&pool, id, 120.0, Some(7200.0)).await.unwrap();
        assert_eq!(resume(&pool, id).await.unwrap(), Some(120.0));
    }

    #[tokio::test]
    async fn finished_threshold_flips_row() {
        let (_t, pool) = open_pool().await;
        let id = add_item(&pool, "/a.mkv").await;
        update(&pool, id, 6900.0, Some(7200.0)).await.unwrap();
        let p = get(&pool, id).await.unwrap().unwrap();
        assert!(p.finished);
        // resume must return None for finished items so the UI doesn't offer it.
        assert_eq!(resume(&pool, id).await.unwrap(), None);
    }

    #[tokio::test]
    async fn mark_finished_round_trip() {
        let (_t, pool) = open_pool().await;
        let id = add_item(&pool, "/a.mkv").await;
        mark_finished(&pool, id, true).await.unwrap();
        assert!(get(&pool, id).await.unwrap().unwrap().finished);
        mark_finished(&pool, id, false).await.unwrap();
        assert!(!get(&pool, id).await.unwrap().unwrap().finished);
    }
}
