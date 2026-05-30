//! Star / Archive / Trash for videos — parity with the photos section.
//!
//! All three flags live on `video_meta`. Trash is soft-delete with a
//! `purge_after` cliff that the maintenance pass uses to actually wipe the
//! row (the file on disk is left alone — the section is a proxy view).

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const DEFAULT_PURGE_DAYS: i64 = 30;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Counts {
    pub starred: i64,
    pub archived: i64,
    pub trashed: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn set_starred(pool: &SqlitePool, item_id: i64, starred: bool) -> Result<()> {
    sqlx::query("UPDATE video_meta SET starred = ? WHERE item_id = ?")
        .bind(if starred { 1 } else { 0 }).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn set_archived(pool: &SqlitePool, item_id: i64, archived: bool) -> Result<()> {
    sqlx::query("UPDATE video_meta SET archived = ? WHERE item_id = ?")
        .bind(if archived { 1 } else { 0 }).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn trash(pool: &SqlitePool, item_id: i64, retain_days: i64) -> Result<()> {
    let now = now();
    let purge = now + retain_days.max(1) * 86_400;
    sqlx::query("UPDATE video_meta SET deleted_at = ? WHERE item_id = ?")
        .bind(now).bind(item_id).execute(pool).await?;
    // store the purge cliff alongside; we re-use deleted_at + retention for it.
    sqlx::query("UPDATE items SET updated = ? WHERE id = ?")
        .bind(purge).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn restore(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE video_meta SET deleted_at = NULL WHERE item_id = ?")
        .bind(item_id).execute(pool).await?;
    Ok(())
}

/// Drop video_meta rows whose `deleted_at + retain_days * day` is in the past.
/// Returns the number of rows hard-removed from the index (files on disk are
/// untouched — we never wrote them, never destroy them).
pub async fn purge_expired(pool: &SqlitePool, retain_days: i64) -> Result<u64> {
    let cutoff = now() - retain_days.max(0) * 86_400;
    let r = sqlx::query(
        "DELETE FROM video_meta WHERE deleted_at IS NOT NULL AND deleted_at < ?",
    ).bind(cutoff).execute(pool).await?;
    Ok(r.rows_affected())
}

pub async fn counts(pool: &SqlitePool) -> Result<Counts> {
    let s: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM video_meta WHERE starred = 1 AND deleted_at IS NULL")
        .fetch_one(pool).await?;
    let a: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM video_meta WHERE archived = 1 AND deleted_at IS NULL")
        .fetch_one(pool).await?;
    let t: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM video_meta WHERE deleted_at IS NOT NULL")
        .fetch_one(pool).await?;
    Ok(Counts { starred: s, archived: a, trashed: t })
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
    async fn star_archive_round_trip() {
        let (_t, pool) = open_pool().await;
        let id = add(&pool, "/a.mkv").await;
        set_starred(&pool, id, true).await.unwrap();
        set_archived(&pool, id, true).await.unwrap();
        let c = counts(&pool).await.unwrap();
        assert_eq!(c.starred, 1);
        assert_eq!(c.archived, 1);
        set_starred(&pool, id, false).await.unwrap();
        assert_eq!(counts(&pool).await.unwrap().starred, 0);
    }

    #[tokio::test]
    async fn trash_then_restore() {
        let (_t, pool) = open_pool().await;
        let id = add(&pool, "/a.mkv").await;
        trash(&pool, id, DEFAULT_PURGE_DAYS).await.unwrap();
        assert_eq!(counts(&pool).await.unwrap().trashed, 1);
        restore(&pool, id).await.unwrap();
        assert_eq!(counts(&pool).await.unwrap().trashed, 0);
    }

    #[tokio::test]
    async fn purge_only_drops_expired() {
        let (_t, pool) = open_pool().await;
        let id = add(&pool, "/a.mkv").await;
        trash(&pool, id, 30).await.unwrap();
        // backdate deleted_at far in the past
        sqlx::query("UPDATE video_meta SET deleted_at = 0 WHERE item_id = ?").bind(id).execute(&pool).await.unwrap();
        let n = purge_expired(&pool, 30).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(counts(&pool).await.unwrap().trashed, 0);
    }
}
