//! `np.p4.music.queue` — Now Playing queue + history + drag-reorder.
//!
//! Persisted in `play_queue` (so the queue survives relaunch) with a dense,
//! gap-free `position` sequence. `move_item` re-packs positions for drag
//! reorder; `record_play` appends to `play_history` for the history rail.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn clear(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM play_queue").execute(pool).await?;
    Ok(())
}

pub async fn enqueue(pool: &SqlitePool, item_id: i64, source: &str) -> Result<()> {
    let next: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position)+1, 0) FROM play_queue").fetch_one(pool).await?;
    sqlx::query("INSERT INTO play_queue (item_id, position, source, added) VALUES (?,?,?,?)")
        .bind(item_id).bind(next).bind(source).bind(now()).execute(pool).await?;
    Ok(())
}

/// Ordered item ids in the queue.
pub async fn list(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as("SELECT item_id FROM play_queue ORDER BY position").fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Drag-reorder: move the entry at `from` to `to`, re-densifying positions.
pub async fn move_item(pool: &SqlitePool, from: usize, to: usize) -> Result<()> {
    let mut ids = list(pool).await?;
    if from >= ids.len() || to >= ids.len() { return Ok(()); }
    let id = ids.remove(from);
    ids.insert(to, id);
    // rewrite positions atomically
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM play_queue").execute(&mut *tx).await?;
    let t = now();
    for (pos, id) in ids.iter().enumerate() {
        sqlx::query("INSERT INTO play_queue (item_id, position, source, added) VALUES (?,?,'reorder',?)")
            .bind(id).bind(pos as i64).bind(t).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Pop the front of the queue: return its item id and delete the row, so
/// playback can follow the queue (np.p5.music.instant-mix / .queue-persist).
pub async fn pop_first(pool: &SqlitePool) -> Result<Option<i64>> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT position, item_id FROM play_queue ORDER BY position LIMIT 1")
        .fetch_optional(pool).await?;
    let Some((position, item_id)) = row else { return Ok(None); };
    sqlx::query("DELETE FROM play_queue WHERE position = ?").bind(position).execute(pool).await?;
    Ok(Some(item_id))
}

/// Append a finished play to history and bump the track's play_count.
pub async fn record_play(pool: &SqlitePool, item_id: i64, ms_played: i64) -> Result<()> {
    let t = now();
    sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?,?,?)")
        .bind(item_id).bind(t).bind(ms_played).execute(pool).await?;
    sqlx::query("UPDATE track_meta SET play_count = play_count + 1, last_played = ? WHERE item_id = ?")
        .bind(t).bind(item_id).execute(pool).await?;
    // Keep only the most recent 50 plays (np.p5.atmusic.history-page).
    sqlx::query(
        "DELETE FROM play_history WHERE id NOT IN
         (SELECT id FROM play_history ORDER BY played_at DESC LIMIT 50)")
        .execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn enqueue_and_reorder() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        let c = add_track(&pool, "/m/c.flac").await;
        enqueue(&pool, a, "x").await.unwrap();
        enqueue(&pool, b, "x").await.unwrap();
        enqueue(&pool, c, "x").await.unwrap();
        assert_eq!(list(&pool).await.unwrap(), vec![a, b, c]);
        move_item(&pool, 2, 0).await.unwrap(); // c to front
        assert_eq!(list(&pool).await.unwrap(), vec![c, a, b]);
    }

    #[tokio::test]
    async fn record_play_bumps_count() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        record_play(&pool, a, 200_000).await.unwrap();
        let pc: i64 = sqlx::query_scalar("SELECT play_count FROM track_meta WHERE item_id = ?").bind(a).fetch_one(&pool).await.unwrap();
        assert_eq!(pc, 1);
    }
}
