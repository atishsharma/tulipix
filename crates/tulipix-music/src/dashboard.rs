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

/// Resume, with how far in each one is (0..100). Anything at 0% has not really
/// been started and anything marked finished is done, so neither belongs on a
/// "continue" rail — the SQL filters both. Tracks with no known duration are
/// reported at 0% rather than dropped: the row is still worth resuming, we
/// just cannot draw a bar for it.
pub async fn resume_pct(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, i32)>> {
    let rows: Vec<(i64, f64, Option<f64>)> = sqlx::query_as(
        "SELECT ap.item_id, ap.position_s, tm.duration_s
         FROM audiobook_progress ap
         JOIN items ON items.id = ap.item_id
         LEFT JOIN track_meta tm ON tm.item_id = ap.item_id
         WHERE ap.finished = 0 AND ap.position_s > 0 AND items.missing_since IS NULL
         ORDER BY ap.updated DESC LIMIT ?",
    ).bind(limit).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, pos, dur)| {
        let pct = match dur {
            Some(d) if d > 0.0 => ((pos / d) * 100.0).clamp(0.0, 100.0) as i32,
            _ => 0,
        };
        (id, pct)
    }).collect())
}

/// What the Home listening strip shows. Milliseconds, not minutes, because
/// rounding belongs at the formatting step where the unit is chosen.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Stats {
    pub week_ms: i64,
    pub total_ms: i64,
    pub top_genre: Option<String>,
    /// Consecutive days ending today (or yesterday) that had at least one play.
    pub streak_days: i64,
}

/// One pass over `play_history` for the whole strip.
///
/// The streak counts back from today OR yesterday: at 00:30 you have usually
/// not played anything yet, and zeroing a real streak because the clock rolled
/// over would be wrong. It breaks at the first day with no play.
pub async fn stats(pool: &SqlitePool) -> Result<Stats> {
    let n = now();
    let (total_ms,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(ms_played), 0) FROM play_history")
        .fetch_one(pool).await.unwrap_or((0,));
    let (week_ms,): (i64,) = sqlx::query_as(
        "SELECT COALESCE(SUM(ms_played), 0) FROM play_history WHERE played_at >= ?")
        .bind(n - 7 * 86_400).fetch_one(pool).await.unwrap_or((0,));
    let top_genre: Option<(String,)> = sqlx::query_as(
        "SELECT tm.genre FROM play_history ph
         JOIN track_meta tm ON tm.item_id = ph.item_id
         WHERE tm.genre IS NOT NULL AND tm.genre != ''
         GROUP BY tm.genre ORDER BY COUNT(*) DESC LIMIT 1")
        .fetch_optional(pool).await.unwrap_or(None);
    // Distinct local days with a play, newest first.
    let days: Vec<(i64,)> = sqlx::query_as(
        "SELECT DISTINCT played_at / 86400 FROM play_history ORDER BY 1 DESC LIMIT 400")
        .fetch_all(pool).await.unwrap_or_default();
    let today = n / 86_400;
    let mut streak = 0i64;
    let mut want = match days.first() {
        Some((d,)) if *d == today || *d == today - 1 => *d,
        _ => -1,
    };
    if want >= 0 {
        for (d,) in &days {
            if *d == want { streak += 1; want -= 1; } else if *d < want { break; }
        }
    }
    Ok(Stats { week_ms, total_ms, top_genre: top_genre.map(|(g,)| g), streak_days: streak })
}

/// "12h 30m" / "45m" / "—". Hours only once there is an hour to show.
pub fn fmt_listen(ms: i64) -> String {
    if ms <= 0 { return "—".into(); }
    let mins = ms / 60_000;
    if mins < 1 { return "<1m".into(); }
    let (h, m) = (mins / 60, mins % 60);
    if h == 0 { format!("{m}m") } else if m == 0 { format!("{h}h") } else { format!("{h}h {m}m") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn stats_sum_and_streak() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        sqlx::query("UPDATE track_meta SET genre = 'Ghazal' WHERE item_id = ?")
            .bind(a).execute(&pool).await.unwrap();
        let today = now() / 86_400;
        // Three consecutive days ending today, then a gap, then one more.
        for (day, ms) in [(today, 600_000i64), (today - 1, 300_000), (today - 2, 60_000), (today - 9, 999)] {
            sqlx::query("INSERT INTO play_history (item_id, played_at, ms_played) VALUES (?, ?, ?)")
                .bind(a).bind(day * 86_400 + 100).bind(ms).execute(&pool).await.unwrap();
        }
        let s = stats(&pool).await.unwrap();
        assert_eq!(s.total_ms, 960_999);
        // The 9-day-old play is outside the 7-day window.
        assert_eq!(s.week_ms, 960_000);
        assert_eq!(s.top_genre.as_deref(), Some("Ghazal"));
        // The gap ends it at three, not four.
        assert_eq!(s.streak_days, 3);
    }

    #[tokio::test]
    async fn resume_pct_skips_finished_and_unstarted() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.m4b").await;
        let b = add_track(&pool, "/m/b.m4b").await;
        let c = add_track(&pool, "/m/c.m4b").await;
        sqlx::query("UPDATE track_meta SET duration_s = 1000 WHERE item_id IN (?, ?, ?)")
            .bind(a).bind(b).bind(c).execute(&pool).await.unwrap();
        for (id, pos, fin) in [(a, 250.0f64, 0), (b, 900.0, 1), (c, 0.0, 0)] {
            sqlx::query("INSERT INTO audiobook_progress (item_id, position_s, finished, updated) VALUES (?, ?, ?, 1)")
                .bind(id).bind(pos).bind(fin).execute(&pool).await.unwrap();
        }
        let got = resume_pct(&pool, 10).await.unwrap();
        assert_eq!(got, vec![(a, 25)]);
    }

    #[test]
    fn listen_formatting() {
        assert_eq!(fmt_listen(0), "—");
        assert_eq!(fmt_listen(30_000), "<1m");
        assert_eq!(fmt_listen(45 * 60_000), "45m");
        assert_eq!(fmt_listen(120 * 60_000), "2h");
        assert_eq!(fmt_listen(150 * 60_000), "2h 30m");
    }

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
