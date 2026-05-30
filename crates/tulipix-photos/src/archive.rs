//! Archive flag — hides items from the default timeline without deleting.
//!
//! The timeline query already filters `archived = 0`. This module just owns
//! the toggles + the Archive tab listing.

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn set(pool: &SqlitePool, item_ids: &[i64], archived: bool) -> Result<u64> {
    let val = if archived { 1 } else { 0 };
    let mut total = 0u64;
    for &id in item_ids {
        let r = sqlx::query("UPDATE photo_meta SET archived = ? WHERE item_id = ?")
            .bind(val).bind(id).execute(pool).await?;
        total += r.rows_affected();
    }
    Ok(total)
}

pub async fn list(pool: &SqlitePool, offset: i64, limit: i64) -> Result<Vec<(i64, String)>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT items.id, items.abs_path
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.archived = 1
           AND photo_meta.deleted_at IS NULL
           AND items.missing_since IS NULL
         ORDER BY items.added DESC
         LIMIT ? OFFSET ?",
    ).bind(limit).bind(offset).fetch_all(pool).await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn archive_round_trip() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/p.jpg'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id) VALUES (?)").bind(id).execute(&pool).await.unwrap();
        set(&pool, &[id], true).await.unwrap();
        let rows = list(&pool, 0, 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        set(&pool, &[id], false).await.unwrap();
        let rows = list(&pool, 0, 10).await.unwrap();
        assert!(rows.is_empty());
    }
}
