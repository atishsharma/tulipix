//! `np.p4.cloud.recycle` — 30-day recycle bin with restore.
//!
//! Deletes route through here: a row is stamped with a `purge_after` 30 days
//! out instead of removing the file. Restore clears the row; the sweeper
//! returns rows whose grace period has elapsed for hard deletion.

use anyhow::Result;
use sqlx::SqlitePool;

pub const GRACE_SECS: i64 = 30 * 86_400;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Move a remote path into the recycle bin.
pub async fn trash(pool: &SqlitePool, remote_id: i64, path: &str) -> Result<i64> {
    let t = now();
    Ok(sqlx::query_scalar(
        "INSERT INTO recycle_bin (remote_id, remote_path, trashed_at, purge_after, restored) VALUES (?,?,?,?,0) RETURNING id",
    ).bind(remote_id).bind(path).bind(t).bind(t + GRACE_SECS).fetch_one(pool).await?)
}

pub async fn restore(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE recycle_bin SET restored = 1 WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Items still in the bin (not restored, not yet purged).
pub async fn list(pool: &SqlitePool, remote_id: i64) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as("SELECT id, remote_path FROM recycle_bin WHERE remote_id = ? AND restored = 0 ORDER BY trashed_at DESC")
        .bind(remote_id).fetch_all(pool).await?)
}

/// Rows whose grace period elapsed → hard-delete candidates.
pub async fn purgeable(pool: &SqlitePool, now_unix: i64) -> Result<Vec<(i64, i64, String)>> {
    Ok(sqlx::query_as("SELECT id, remote_id, remote_path FROM recycle_bin WHERE restored = 0 AND purge_after <= ?")
        .bind(now_unix).fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[tokio::test]
    async fn trash_restore_purge() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        let id = trash(&pool, r, "old.txt").await.unwrap();
        assert_eq!(list(&pool, r).await.unwrap().len(), 1);
        // not purgeable before grace
        assert!(purgeable(&pool, now()).await.unwrap().is_empty());
        // purgeable far in the future
        assert_eq!(purgeable(&pool, now() + GRACE_SECS + 1).await.unwrap().len(), 1);
        restore(&pool, id).await.unwrap();
        assert!(list(&pool, r).await.unwrap().is_empty());
        assert!(purgeable(&pool, now() + GRACE_SECS + 1).await.unwrap().is_empty());
    }
}
