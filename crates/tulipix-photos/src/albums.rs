//! Manual albums — CRUD + drag-drop ordering.
//!
//! Smart-rule albums live in `smart_albums`; this module covers the case
//! where the user pins specific items into a named collection. Sort order
//! is a sparse integer column so drag-drop reorders only re-write the moved
//! row instead of the whole album.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Album {
    pub id: i64,
    pub name: String,
    pub cover_id: Option<i64>,
    pub item_count: i64,
    pub rating: i64,
    pub created: i64,
    pub updated: i64,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn create(pool: &SqlitePool, name: &str) -> Result<i64> {
    let n = now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO albums (name, created, updated) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(name).bind(n).bind(n)
    .fetch_one(pool).await?;
    Ok(id)
}

pub async fn rename(pool: &SqlitePool, id: i64, new_name: &str) -> Result<()> {
    sqlx::query("UPDATE albums SET name = ?, updated = ? WHERE id = ?")
        .bind(new_name).bind(now()).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn delete(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM albums WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn list(pool: &SqlitePool) -> Result<Vec<Album>> {
    let rows: Vec<(i64, String, Option<i64>, i64, i64, i64)> = sqlx::query_as(
        "SELECT a.id, a.name, a.cover_id, a.rating, a.created, a.updated FROM albums a WHERE a.smart_rule IS NULL ORDER BY a.updated DESC",
    ).fetch_all(pool).await?;
    let mut out = Vec::with_capacity(rows.len());
    for (id, name, cover_id, rating, created, updated) in rows {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM album_items WHERE album_id = ?")
            .bind(id).fetch_one(pool).await?;
        out.push(Album { id, name, cover_id, item_count: count, rating, created, updated });
    }
    Ok(out)
}

/// Set the 0-5 star rating for an album. Values outside the range are clamped.
pub async fn set_rating(pool: &SqlitePool, id: i64, rating: i64) -> Result<()> {
    let r = rating.clamp(0, 5);
    sqlx::query("UPDATE albums SET rating = ?, updated = ? WHERE id = ?")
        .bind(r).bind(now()).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn rating(pool: &SqlitePool, id: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT rating FROM albums WHERE id = ?")
        .bind(id).fetch_one(pool).await?)
}

pub async fn add_items(pool: &SqlitePool, album_id: i64, item_ids: &[i64]) -> Result<u64> {
    let mut tx = pool.begin().await?;
    // Find current max sort_key once; append new rows after it.
    let mut next: i64 = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(sort_key) FROM album_items WHERE album_id = ?",
    )
    .bind(album_id)
    .fetch_one(&mut *tx)
    .await?
    .unwrap_or(-1);
    next += 1024;
    let mut inserted = 0u64;
    for &id in item_ids {
        let r = sqlx::query(
            "INSERT OR IGNORE INTO album_items (album_id, item_id, sort_key) VALUES (?, ?, ?)",
        )
        .bind(album_id).bind(id).bind(next)
        .execute(&mut *tx).await?;
        if r.rows_affected() > 0 { inserted += 1; next += 1024; }
    }
    sqlx::query("UPDATE albums SET updated = ?, cover_id = COALESCE(cover_id, ?) WHERE id = ?")
        .bind(now()).bind(item_ids.first().copied()).bind(album_id)
        .execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(inserted)
}

pub async fn remove_items(pool: &SqlitePool, album_id: i64, item_ids: &[i64]) -> Result<u64> {
    let mut removed = 0u64;
    for &id in item_ids {
        let r = sqlx::query(
            "DELETE FROM album_items WHERE album_id = ? AND item_id = ?",
        )
        .bind(album_id).bind(id)
        .execute(pool).await?;
        removed += r.rows_affected();
    }
    Ok(removed)
}

/// Move `item_id` to sit between `before_id` and `after_id` (either may be
/// None for the head/tail). Picks a sort_key midway and re-balances only if
/// the midpoint would collide with an existing key.
pub async fn move_item(
    pool: &SqlitePool,
    album_id: i64,
    item_id: i64,
    before: Option<i64>,
    after: Option<i64>,
) -> Result<()> {
    let lo: i64 = match before {
        Some(b) => sqlx::query_scalar("SELECT sort_key FROM album_items WHERE album_id = ? AND item_id = ?")
            .bind(album_id).bind(b).fetch_one(pool).await?,
        None => {
            let min: Option<i64> = sqlx::query_scalar("SELECT MIN(sort_key) FROM album_items WHERE album_id = ?")
                .bind(album_id).fetch_one(pool).await?;
            min.unwrap_or(0) - 2048
        }
    };
    let hi: i64 = match after {
        Some(a) => sqlx::query_scalar("SELECT sort_key FROM album_items WHERE album_id = ? AND item_id = ?")
            .bind(album_id).bind(a).fetch_one(pool).await?,
        None => {
            let max: Option<i64> = sqlx::query_scalar("SELECT MAX(sort_key) FROM album_items WHERE album_id = ?")
                .bind(album_id).fetch_one(pool).await?;
            max.unwrap_or(0) + 2048
        }
    };
    let mut mid = lo + (hi - lo) / 2;
    if (mid - lo).abs() < 2 || (hi - mid).abs() < 2 {
        rebalance(pool, album_id).await?;
        return Box::pin(move_item(pool, album_id, item_id, before, after)).await;
    }
    if mid == lo { mid += 1; }
    sqlx::query("UPDATE album_items SET sort_key = ? WHERE album_id = ? AND item_id = ?")
        .bind(mid).bind(album_id).bind(item_id)
        .execute(pool).await?;
    Ok(())
}

async fn rebalance(pool: &SqlitePool, album_id: i64) -> Result<()> {
    let ids: Vec<(i64,)> = sqlx::query_as(
        "SELECT item_id FROM album_items WHERE album_id = ? ORDER BY sort_key",
    )
    .bind(album_id).fetch_all(pool).await?;
    let mut k: i64 = 1024;
    let mut tx = pool.begin().await?;
    for (id,) in ids {
        sqlx::query("UPDATE album_items SET sort_key = ? WHERE album_id = ? AND item_id = ?")
            .bind(k).bind(album_id).bind(id).execute(&mut *tx).await?;
        k += 1024;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn seed_items(pool: &SqlitePool, paths: &[&str]) -> Vec<i64> {
        let mut ids = Vec::new();
        for p in paths {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
                .bind(p).execute(pool).await.unwrap();
            ids.push(sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(p).fetch_one(pool).await.unwrap());
        }
        ids
    }

    #[tokio::test]
    async fn rating_clamps_and_persists() {
        let (_t, pool) = open_pool().await;
        let id = create(&pool, "R").await.unwrap();
        assert_eq!(rating(&pool, id).await.unwrap(), 0);
        set_rating(&pool, id, 4).await.unwrap();
        assert_eq!(rating(&pool, id).await.unwrap(), 4);
        set_rating(&pool, id, 99).await.unwrap();
        assert_eq!(rating(&pool, id).await.unwrap(), 5);
        set_rating(&pool, id, -3).await.unwrap();
        assert_eq!(rating(&pool, id).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn create_add_list_reorder() {
        let (_t, pool) = open_pool().await;
        let ids = seed_items(&pool, &["/a.jpg", "/b.jpg", "/c.jpg"]).await;
        let id = create(&pool, "Trip").await.unwrap();
        let n = add_items(&pool, id, &ids).await.unwrap();
        assert_eq!(n, 3);
        let albums = list(&pool).await.unwrap();
        assert_eq!(albums.len(), 1);
        assert_eq!(albums[0].item_count, 3);

        // Move last to head: between None and first
        move_item(&pool, id, ids[2], None, Some(ids[0])).await.unwrap();
        let order: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM album_items WHERE album_id = ? ORDER BY sort_key",
        ).bind(id).fetch_all(&pool).await.unwrap();
        assert_eq!(order[0], ids[2]);
    }
}
