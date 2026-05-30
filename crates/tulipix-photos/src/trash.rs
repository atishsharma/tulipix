//! Trash — soft-delete with a 30-day purge horizon.
//!
//! "Delete" never touches disk. It stamps `photo_meta.deleted_at` + an
//! absolute `purge_after` (30 days later by default). The Trash tab filters
//! `WHERE deleted_at IS NOT NULL AND purge_after > now`. A scheduled
//! `purge_due` sweep moves expired items to OS trash and drops the row.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const RETENTION_SECS: i64 = 30 * 24 * 3600;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrashEntry {
    pub item_id: i64,
    pub abs_path: String,
    pub deleted_at: i64,
    pub purge_after: i64,
}

pub async fn soft_delete(pool: &SqlitePool, item_ids: &[i64]) -> Result<u64> {
    let n = now();
    let p = n + RETENTION_SECS;
    let mut total = 0u64;
    for &id in item_ids {
        let r = sqlx::query(
            "UPDATE photo_meta SET deleted_at = ?, purge_after = ? WHERE item_id = ?",
        )
        .bind(n).bind(p).bind(id)
        .execute(pool).await?;
        total += r.rows_affected();
    }
    Ok(total)
}

pub async fn restore(pool: &SqlitePool, item_ids: &[i64]) -> Result<u64> {
    let mut total = 0u64;
    for &id in item_ids {
        let r = sqlx::query(
            "UPDATE photo_meta SET deleted_at = NULL, purge_after = NULL WHERE item_id = ?",
        )
        .bind(id).execute(pool).await?;
        total += r.rows_affected();
    }
    Ok(total)
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<TrashEntry>> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.deleted_at, photo_meta.purge_after
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.deleted_at IS NOT NULL
         ORDER BY photo_meta.deleted_at DESC",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(item_id, abs_path, deleted_at, purge_after)| TrashEntry {
        item_id, abs_path, deleted_at, purge_after,
    }).collect())
}

/// Rows whose `purge_after` has elapsed. Caller does the actual OS-trash move
/// and then `purge_row` to drop the DB record.
pub async fn purge_due(pool: &SqlitePool, now_unix: i64) -> Result<Vec<TrashEntry>> {
    let rows: Vec<(i64, String, i64, i64)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.deleted_at, photo_meta.purge_after
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.purge_after IS NOT NULL AND photo_meta.purge_after <= ?",
    ).bind(now_unix).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(item_id, abs_path, deleted_at, purge_after)| TrashEntry {
        item_id, abs_path, deleted_at, purge_after,
    }).collect())
}

pub async fn purge_row(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM items WHERE id = ?").bind(item_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn seed(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id) VALUES (?)").bind(id).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn soft_delete_then_restore() {
        let (_t, pool) = open_pool().await;
        let id = seed(&pool, "/a.jpg").await;
        soft_delete(&pool, &[id]).await.unwrap();
        let trash = list(&pool).await.unwrap();
        assert_eq!(trash.len(), 1);
        restore(&pool, &[id]).await.unwrap();
        let trash = list(&pool).await.unwrap();
        assert!(trash.is_empty());
    }
    #[tokio::test]
    async fn purge_due_returns_expired_only() {
        let (_t, pool) = open_pool().await;
        let a = seed(&pool, "/a.jpg").await;
        let b = seed(&pool, "/b.jpg").await;
        soft_delete(&pool, &[a, b]).await.unwrap();
        // backdate b's purge_after into the past
        sqlx::query("UPDATE photo_meta SET purge_after = 1 WHERE item_id = ?")
            .bind(b).execute(&pool).await.unwrap();
        let due = purge_due(&pool, now()).await.unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].item_id, b);
    }
}
