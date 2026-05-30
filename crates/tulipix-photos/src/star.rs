//! Starred flag — pinned items in the Starred rail.

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn set(pool: &SqlitePool, item_id: i64, starred: bool) -> Result<()> {
    let val = if starred { 1 } else { 0 };
    sqlx::query("UPDATE photo_meta SET starred = ? WHERE item_id = ?")
        .bind(val).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn toggle(pool: &SqlitePool, item_id: i64) -> Result<bool> {
    let cur: Option<i64> = sqlx::query_scalar("SELECT starred FROM photo_meta WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await?;
    let next = matches!(cur, Some(0) | None);
    set(pool, item_id, next).await?;
    Ok(next)
}

pub async fn list(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, String)>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT items.id, items.abs_path
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.starred = 1
           AND photo_meta.deleted_at IS NULL
           AND photo_meta.archived = 0
           AND items.missing_since IS NULL
         ORDER BY items.updated DESC
         LIMIT ?",
    ).bind(limit).fetch_all(pool).await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn toggle_persists() {
        let (_t, pool) = open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/p.jpg'").fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id) VALUES (?)").bind(id).execute(&pool).await.unwrap();
        assert!(toggle(&pool, id).await.unwrap());
        assert_eq!(list(&pool, 10).await.unwrap().len(), 1);
        assert!(!toggle(&pool, id).await.unwrap());
        assert!(list(&pool, 10).await.unwrap().is_empty());
    }
}
