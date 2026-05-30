//! `np.p4.music.opus-cache` — background Opus/WebM cache for streamed tracks.
//!
//! Streamed (YouTube/SoundCloud) tracks cache to `Tulipix/cache/stream/`. This
//! enforces a size cap (default 5 GB) with LRU eviction by `last_played`, and
//! records each cached file's path + byte size on `track_meta`.

use anyhow::Result;
use sqlx::SqlitePool;

pub const DEFAULT_CAP_BYTES: i64 = 5 * 1024 * 1024 * 1024; // 5 GB

pub async fn record(pool: &SqlitePool, item_id: i64, cache_path: &str, bytes: i64) -> Result<()> {
    sqlx::query("UPDATE track_meta SET is_stream = 1, cache_path = ?, cache_bytes = ? WHERE item_id = ?")
        .bind(cache_path).bind(bytes).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn total_bytes(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COALESCE(SUM(cache_bytes), 0) FROM track_meta WHERE cache_path IS NOT NULL")
        .fetch_one(pool).await?)
}

/// Files to evict (LRU, NULL last_played first) so total drops at or below
/// `cap_bytes`. Returns `(item_id, cache_path)` for the caller to unlink.
pub async fn evict_plan(pool: &SqlitePool, cap_bytes: i64) -> Result<Vec<(i64, String)>> {
    let total = total_bytes(pool).await?;
    if total <= cap_bytes { return Ok(vec![]); }
    let mut over = total - cap_bytes;
    // oldest first: NULLs (never played) then ascending last_played
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT item_id, cache_path, cache_bytes FROM track_meta
         WHERE cache_path IS NOT NULL
         ORDER BY (last_played IS NULL) DESC, last_played ASC",
    ).fetch_all(pool).await?;
    let mut plan = Vec::new();
    for (id, path, bytes) in rows {
        if over <= 0 { break; }
        plan.push((id, path));
        over -= bytes;
    }
    Ok(plan)
}

/// Forget a cache entry after the file has been removed from disk.
pub async fn forget(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE track_meta SET cache_path = NULL, cache_bytes = NULL WHERE item_id = ?")
        .bind(item_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn evicts_oldest_until_under_cap() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/c/a.opus").await; // never played
        let b = add_track(&pool, "/c/b.opus").await;
        let c = add_track(&pool, "/c/c.opus").await;
        record(&pool, a, "/c/a.opus", 100).await.unwrap();
        record(&pool, b, "/c/b.opus", 100).await.unwrap();
        record(&pool, c, "/c/c.opus", 100).await.unwrap();
        sqlx::query("UPDATE track_meta SET last_played = 500 WHERE item_id = ?").bind(b).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET last_played = 900 WHERE item_id = ?").bind(c).execute(&pool).await.unwrap();
        assert_eq!(total_bytes(&pool).await.unwrap(), 300);
        // cap 150 → must drop 150 bytes: evict a (null) then b (oldest)
        let plan = evict_plan(&pool, 150).await.unwrap();
        let ids: Vec<i64> = plan.iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, vec![a, b]);
        // under cap → empty plan
        assert!(evict_plan(&pool, 10_000).await.unwrap().is_empty());
    }
}
