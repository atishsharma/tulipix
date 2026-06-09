//! `np.p4.music.rating` — Loved + 1–5 star rating per track.
//!
//! Feeds rating-driven smart playlists ([`crate::playlists`]). Stars clamp to
//! 0..5 (0 = unrated); `loved` is an independent boolean heart.

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn set_stars(pool: &SqlitePool, item_id: i64, stars: u8) -> Result<u8> {
    let s = stars.min(5);
    sqlx::query("UPDATE track_meta SET rating = ? WHERE item_id = ?").bind(s as i64).bind(item_id).execute(pool).await?;
    Ok(s)
}

pub async fn set_loved(pool: &SqlitePool, item_id: i64, loved: bool) -> Result<()> {
    sqlx::query("UPDATE track_meta SET loved = ? WHERE item_id = ?").bind(loved as i64).bind(item_id).execute(pool).await?;
    Ok(())
}

pub async fn toggle_loved(pool: &SqlitePool, item_id: i64) -> Result<bool> {
    let cur: i64 = sqlx::query_scalar("SELECT loved FROM track_meta WHERE item_id = ?").bind(item_id).fetch_one(pool).await?;
    let next = cur == 0;
    set_loved(pool, item_id, next).await?;
    Ok(next)
}

/// Loved tracks, most-recently-played first.
pub async fn loved(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE track_meta.loved = 1 AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY track_meta.last_played DESC NULLS LAST LIMIT ?",
    ).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn stars_clamp_and_love_toggles() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        assert_eq!(set_stars(&pool, a, 9).await.unwrap(), 5);
        assert!(toggle_loved(&pool, a).await.unwrap());
        assert_eq!(loved(&pool, 10).await.unwrap(), vec![a]);
        assert!(!toggle_loved(&pool, a).await.unwrap());
        assert!(loved(&pool, 10).await.unwrap().is_empty());
    }
}
