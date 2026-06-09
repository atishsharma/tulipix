//! `np.p4.music.dashboard` — music landing dashboard rails.
//!
//! Recently Played / Most Played / Loved / Resume / New This Week — each a
//! small query over `track_meta` + `audiobook_progress`. The hero card's
//! "For You" seed is just the most-played track id.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

async fn ids(pool: &SqlitePool, sql: &str, limit: i64) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(sql).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn recently_played(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    ids(pool,
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE last_played IS NOT NULL AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY last_played DESC LIMIT ?", limit).await
}

pub async fn most_played(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    ids(pool,
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE play_count > 0 AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY play_count DESC LIMIT ?", limit).await
}

/// New This Week — items added in the last 7 days.
pub async fn new_this_week(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    let cutoff = now() - 7 * 86_400;
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT track_meta.item_id FROM track_meta JOIN items ON items.id = track_meta.item_id
         WHERE items.added >= ? AND items.missing_since IS NULL AND track_meta.is_audiobook = 0
         ORDER BY items.added DESC LIMIT ?",
    ).bind(cutoff).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Resume — audiobooks/podcasts with a saved, unfinished position.
pub async fn resume(pool: &SqlitePool, limit: i64) -> Result<Vec<i64>> {
    ids(pool,
        "SELECT ap.item_id FROM audiobook_progress ap JOIN items ON items.id = ap.item_id
         WHERE ap.finished = 0 AND ap.position_s > 0 AND items.missing_since IS NULL
         ORDER BY ap.updated DESC LIMIT ?", limit).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn rails_reflect_state() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        sqlx::query("UPDATE track_meta SET play_count = 10, last_played = 500 WHERE item_id = ?").bind(a).execute(&pool).await.unwrap();
        sqlx::query("UPDATE track_meta SET play_count = 3, last_played = 200 WHERE item_id = ?").bind(b).execute(&pool).await.unwrap();
        assert_eq!(most_played(&pool, 10).await.unwrap(), vec![a, b]);
        assert_eq!(recently_played(&pool, 10).await.unwrap(), vec![a, b]);
        // freshly added => new this week
        sqlx::query("UPDATE items SET added = ? WHERE id = ?").bind(now()).bind(a).execute(&pool).await.unwrap();
        assert!(new_this_week(&pool, 10).await.unwrap().contains(&a));
    }
}
