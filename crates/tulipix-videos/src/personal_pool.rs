//! Personal Video Pool — boolean `in_personal_pool` flag on the shared
//! `items` table that lets users co-mingle home videos (Photos section) with
//! the curated TMDB-scraped library (Videos section). Toggling the flag from
//! either side updates the same row so a phone-shot clip surfaces in both
//! Today-in-Past memories AND the Personal rail on the Videos landing page.

use anyhow::Result;
use sqlx::SqlitePool;

pub const PERSONAL_POOL_SCHEMA: &str = r#"
ALTER TABLE items ADD COLUMN in_personal_pool INTEGER NOT NULL DEFAULT 0;
CREATE INDEX IF NOT EXISTS items_personal_pool_idx ON items(in_personal_pool, added);
"#;

/// Idempotent — uses ALTER TABLE which errors on second run, so we swallow
/// the "duplicate column" error specifically (every other error bubbles).
pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    if column_exists(pool).await? {
        // Index might still be missing on an older install.
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS items_personal_pool_idx ON items(in_personal_pool, added)",
        )
        .execute(pool)
        .await?;
        return Ok(());
    }
    sqlx::raw_sql(PERSONAL_POOL_SCHEMA).execute(pool).await?;
    Ok(())
}

async fn column_exists(pool: &SqlitePool) -> Result<bool> {
    let rows: Vec<(i64, String, String, i64, Option<String>, i64)> =
        sqlx::query_as("PRAGMA table_info(items)").fetch_all(pool).await?;
    Ok(rows.iter().any(|(_, name, _, _, _, _)| name == "in_personal_pool"))
}

pub async fn add_to_pool(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE items SET in_personal_pool = 1 WHERE id = ?")
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_from_pool(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE items SET in_personal_pool = 0 WHERE id = ?")
        .bind(item_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolRow {
    pub item_id: i64,
    pub abs_path: String,
    pub section: String,
    pub added: i64,
}

/// Lists every item currently in the personal pool — both sides query this
/// (Videos shows the rail; Photos memories cross-references for "include
/// home-movie clips" toggle).
pub async fn list_pool(pool: &SqlitePool, limit: i64) -> Result<Vec<PoolRow>> {
    let rows = sqlx::query_as::<_, (i64, String, String, i64)>(
        "SELECT id, abs_path, section, added FROM items
         WHERE in_personal_pool = 1 ORDER BY added DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(item_id, abs_path, section, added)| PoolRow {
            item_id,
            abs_path,
            section,
            added,
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn insert(pool: &SqlitePool, path: &str, section: &str, added: i64) -> i64 {
        sqlx::query(
            "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated)
             VALUES (?, 0, 1, 0, ?, ?, 0)",
        )
        .bind(path)
        .bind(section)
        .bind(added)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(path)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn schema_is_idempotent() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        apply_schema(&pool).await.unwrap();
        assert!(column_exists(&pool).await.unwrap());
    }

    #[tokio::test]
    async fn pool_round_trip() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let a = insert(&pool, "/clip1.mp4", "photos", 10).await;
        let b = insert(&pool, "/clip2.mp4", "videos", 20).await;
        let _ = insert(&pool, "/movie.mkv", "videos", 30).await;
        add_to_pool(&pool, a).await.unwrap();
        add_to_pool(&pool, b).await.unwrap();
        let pool_rows = list_pool(&pool, 100).await.unwrap();
        assert_eq!(pool_rows.len(), 2);
        // newest first
        assert_eq!(pool_rows[0].item_id, b);
        assert_eq!(pool_rows[1].item_id, a);
    }

    #[tokio::test]
    async fn remove_flips_back() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let a = insert(&pool, "/clip1.mp4", "photos", 10).await;
        add_to_pool(&pool, a).await.unwrap();
        remove_from_pool(&pool, a).await.unwrap();
        let rows = list_pool(&pool, 100).await.unwrap();
        assert!(rows.is_empty());
    }
}
