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

/// Replace the whole queue with `ids`, in that order. One statement per row in
/// a single transaction: playing from a list has to leave NOTHING of the list
/// you were on before, or the old context leaks into the new one one track at a
/// time (np.p6.music.context-queue).
pub async fn replace(pool: &SqlitePool, ids: &[i64], source: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM play_queue").execute(&mut *tx).await?;
    let t = now();
    for (pos, id) in ids.iter().enumerate() {
        sqlx::query("INSERT INTO play_queue (item_id, position, source, added) VALUES (?,?,?,?)")
            .bind(id).bind(pos as i64).bind(source).bind(t).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Drop everything up to and including the first entry for `item_id`. Playing a
/// row out of the queue panel consumes the rows above it — otherwise the track
/// you jumped to is still sitting in the queue and plays itself again.
pub async fn drop_through(pool: &SqlitePool, item_id: i64) -> Result<()> {
    let cut: Option<(i64,)> = sqlx::query_as(
        "SELECT position FROM play_queue WHERE item_id = ? ORDER BY position LIMIT 1")
        .bind(item_id).fetch_optional(pool).await?;
    let Some((position,)) = cut else { return Ok(()); };
    sqlx::query("DELETE FROM play_queue WHERE position <= ?").bind(position).execute(pool).await?;
    Ok(())
}

/// Drop every entry that came from `source`, leaving the rest in order.
///
/// What "stop the station" is: the tracks nobody chose go, and anything queued
/// by hand stays. Positions are re-densified afterwards so the queue keeps the
/// contiguous 0..n that `move_item` walks — and each survivor keeps its own
/// source, so clearing the station does not relabel what a user queued.
pub async fn clear_source(pool: &SqlitePool, source: &str) -> Result<i64> {
    let hit = sqlx::query("DELETE FROM play_queue WHERE source = ?")
        .bind(source).execute(pool).await?.rows_affected() as i64;
    if hit == 0 { return Ok(0); }
    // Renumbering in place would collide with the UNIQUE on `position` halfway
    // through, so the survivors are read out, cleared and written back.
    let rows: Vec<(i64, Option<String>, i64)> = sqlx::query_as(
        "SELECT item_id, source, added FROM play_queue ORDER BY position").fetch_all(pool).await?;
    clear(pool).await?;
    for (position, (item_id, src, added)) in rows.into_iter().enumerate() {
        sqlx::query("INSERT INTO play_queue (item_id, position, source, added) VALUES (?,?,?,?)")
            .bind(item_id).bind(position as i64).bind(src).bind(added).execute(pool).await?;
    }
    Ok(hit)
}


/// What the shuffle draw compares. Everything it needs about one candidate,
/// so the scoring is pure and the query lives with the caller.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Draw {
    pub item_id: i64,
    pub artist_id: Option<i64>,
    pub album_id: Option<i64>,
    pub loved: bool,
    /// When it was last played, or `None` if never.
    pub last_played: Option<i64>,
}

/// How far back down the played order a repeat still counts against a track.
const SPACING: usize = 12;
/// Penalty for sharing an artist with something in that window, at the top of
/// it. Falls off linearly with distance.
const ARTIST_WEIGHT: f64 = 6.0;
/// Same, for sharing an album. Lower: a shuffle within one record is a normal
/// thing to be doing, and two songs from the same album are less jarring than
/// two by the same artist back to back.
const ALBUM_WEIGHT: f64 = 3.0;
/// What a loved track is worth. Small on purpose: this nudges the order, it
/// does not turn shuffle into a favourites playlist.
const LOVED_BONUS: f64 = 1.5;
/// What being played recently costs, decaying over this many days.
const RECENCY_WEIGHT: f64 = 2.5;
const RECENCY_DAYS: f64 = 14.0;

/// Pick the next track to play, weighted so a shuffle feels shuffled.
///
/// True randomness clusters, and the clustering is what makes shuffle feel
/// broken: three songs by the same artist in a row reads as a bug even though
/// it is exactly what random does. This pushes the same artist and album apart
/// from what was just played, sinks tracks heard recently, and floats loved
/// ones — the reason Spotify rebuilt theirs.
///
/// `recent` is the played order, newest first. `now` is a unix timestamp;
/// taking it as an argument rather than reading the clock is what lets the
/// recency term be tested.
///
/// Returns an index into `pool_`, or `None` when there is nothing to pick.
pub fn weighted_pick(pool_: &[Draw], recent: &[i64], now: i64, roll: f64) -> Option<usize> {
    if pool_.is_empty() {
        return None;
    }
    if pool_.len() == 1 {
        return Some(0);
    }

    // What was just played, and how long ago in track counts.
    let window: Vec<&Draw> = recent
        .iter()
        .take(SPACING)
        .filter_map(|id| pool_.iter().find(|d| d.item_id == *id))
        .collect();

    let mut best: Vec<(usize, f64)> = Vec::with_capacity(pool_.len());
    for (i, cand) in pool_.iter().enumerate() {
        // Never the track that is playing.
        if recent.first() == Some(&cand.item_id) {
            continue;
        }
        let mut score = 10.0f64;
        for (back, seen) in window.iter().enumerate() {
            // Nearest in the window hurts most; the twelfth barely at all.
            let closeness = 1.0 - (back as f64 / SPACING as f64);
            if cand.artist_id.is_some() && cand.artist_id == seen.artist_id {
                score -= ARTIST_WEIGHT * closeness;
            }
            if cand.album_id.is_some() && cand.album_id == seen.album_id {
                score -= ALBUM_WEIGHT * closeness;
            }
        }
        if cand.loved {
            score += LOVED_BONUS;
        }
        if let Some(at) = cand.last_played {
            let days = (now - at).max(0) as f64 / 86_400.0;
            if days < RECENCY_DAYS {
                score -= RECENCY_WEIGHT * (1.0 - days / RECENCY_DAYS);
            }
        }
        // A floor rather than a hard exclusion: on a one-artist library every
        // candidate is penalised, and a shuffle that refuses to play anything
        // is worse than one that repeats.
        best.push((i, score.max(0.25)));
    }
    if best.is_empty() {
        return Some(0);
    }

    // Weighted draw, not "take the highest": always picking the best score is
    // not a shuffle, it is a sort, and it would play the same order every time.
    let total: f64 = best.iter().map(|(_, w)| w).sum();
    let mut target = roll.clamp(0.0, 1.0) * total;
    for (i, w) in &best {
        target -= w;
        if target <= 0.0 {
            return Some(*i);
        }
    }
    Some(best.last().expect("checked non-empty").0)
}

/// Pop a random entry rather than the front — shuffle stays inside the queue
/// (the album/playlist you started) instead of escaping to the whole library.
pub async fn pop_random(pool: &SqlitePool) -> Result<Option<i64>> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT position, item_id FROM play_queue ORDER BY RANDOM() LIMIT 1")
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

    fn draw(id: i64, artist: i64, album: i64) -> Draw {
        Draw { item_id: id, artist_id: Some(artist), album_id: Some(album), ..Default::default() }
    }

    #[test]
    fn shuffle_pushes_the_same_artist_away_from_what_just_played() {
        // Four tracks: two by artist 1 (one of them just played), two by others.
        let pool_ = vec![draw(1, 1, 1), draw(2, 1, 1), draw(3, 2, 2), draw(4, 3, 3)];
        let recent = vec![1i64];

        // Sweep the whole roll range and count what comes out. The same-artist
        // track should still be reachable -- this is a weighting, not a ban --
        // but it must be the rarest of the three.
        let mut counts = [0usize; 5];
        for k in 0..1000 {
            let i = weighted_pick(&pool_, &recent, 0, k as f64 / 1000.0).unwrap();
            counts[pool_[i].item_id as usize] += 1;
        }
        assert_eq!(counts[1], 0, "never the track that is playing");
        assert!(counts[2] > 0, "a weighting, not a ban");
        assert!(counts[3] > counts[2], "a different artist should win more often");
        assert!(counts[4] > counts[2]);
    }

    #[test]
    fn loved_floats_and_recent_sinks() {
        let now = 1_700_000_000i64;
        let mut loved = draw(1, 1, 1);
        loved.loved = true;
        let plain = draw(2, 2, 2);
        let mut heard = draw(3, 3, 3);
        heard.last_played = Some(now - 3_600); // an hour ago
        let pool_ = vec![loved, plain, heard];

        let mut counts = [0usize; 4];
        for k in 0..1000 {
            let i = weighted_pick(&pool_, &[], now, k as f64 / 1000.0).unwrap();
            counts[pool_[i].item_id as usize] += 1;
        }
        assert!(counts[1] > counts[2], "loved floats above plain");
        assert!(counts[2] > counts[3], "played an hour ago sinks below plain");
    }

    #[test]
    fn a_one_artist_library_still_plays_something() {
        // Every candidate penalised. The floor is what stops this returning
        // nothing at all.
        let pool_ = vec![draw(1, 1, 1), draw(2, 1, 1), draw(3, 1, 1)];
        let pick = weighted_pick(&pool_, &[1, 2, 3], 0, 0.5).unwrap();
        assert_ne!(pool_[pick].item_id, 1, "still not the one playing");
        assert!(weighted_pick(&[], &[], 0, 0.5).is_none());
        assert_eq!(weighted_pick(&pool_[..1], &[], 0, 0.5), Some(0));
    }

    #[tokio::test]
    async fn clearing_one_source_leaves_the_rest_intact() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        let c = add_track(&pool, "/m/c.flac").await;
        enqueue(&pool, a, "manual").await.unwrap();
        enqueue(&pool, b, "station").await.unwrap();
        enqueue(&pool, c, "sonic").await.unwrap();

        assert_eq!(clear_source(&pool, "station").await.unwrap(), 1);
        assert_eq!(list(&pool).await.unwrap(), vec![a, c]);

        // Positions are contiguous again, and a survivor keeps its own source.
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT position, source FROM play_queue ORDER BY position")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(rows, vec![(0, "manual".into()), (1, "sonic".into())]);
        assert_eq!(clear_source(&pool, "station").await.unwrap(), 0, "nothing left to clear");
    }

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
    async fn replace_and_drop_through() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        let b = add_track(&pool, "/m/b.flac").await;
        let c = add_track(&pool, "/m/c.flac").await;
        enqueue(&pool, a, "old").await.unwrap();
        // Replacing leaves nothing of the previous context behind.
        replace(&pool, &[c, b, a], "ctx").await.unwrap();
        assert_eq!(list(&pool).await.unwrap(), vec![c, b, a]);
        // Jumping to `b` consumes `c` above it as well as `b` itself.
        drop_through(&pool, b).await.unwrap();
        assert_eq!(list(&pool).await.unwrap(), vec![a]);
        // An id that is not queued changes nothing.
        drop_through(&pool, c).await.unwrap();
        assert_eq!(list(&pool).await.unwrap(), vec![a]);
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
