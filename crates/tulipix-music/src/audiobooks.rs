//! `np.p4.music.audiobooks` — chapter nav, position-remember, 0.5×–3× speed
//! without pitch shift.
//!
//! Marks a track as an audiobook, persists resume position + playback speed in
//! `audiobook_progress`, and owns the mpv options that change tempo while
//! preserving pitch (the `scaletempo2` path).

use anyhow::Result;
use sqlx::SqlitePool;

pub const MIN_SPEED: f64 = 0.5;
pub const MAX_SPEED: f64 = 3.0;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub fn clamp_speed(s: f64) -> f64 { s.clamp(MIN_SPEED, MAX_SPEED) }

/// mpv options for pitch-preserving speed change.
pub fn mpv_speed_options(speed: f64) -> Vec<String> {
    vec![
        "--audio-pitch-correction=yes".to_string(),
        format!("--speed={}", clamp_speed(speed)),
    ]
}

pub async fn mark_audiobook(pool: &SqlitePool, item_id: i64) -> Result<()> {
    sqlx::query("UPDATE track_meta SET is_audiobook = 1 WHERE item_id = ?").bind(item_id).execute(pool).await?;
    Ok(())
}

/// Save resume position + speed.
pub async fn save_progress(pool: &SqlitePool, item_id: i64, position_s: f64, speed: f64) -> Result<()> {
    sqlx::query(
        "INSERT INTO audiobook_progress (item_id, position_s, speed, updated) VALUES (?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET position_s=excluded.position_s, speed=excluded.speed, updated=excluded.updated",
    ).bind(item_id).bind(position_s).bind(clamp_speed(speed)).bind(now()).execute(pool).await?;
    Ok(())
}

/// Resume position + speed, defaulting to start / 1.0× if unseen.
pub async fn resume(pool: &SqlitePool, item_id: i64) -> Result<(f64, f64)> {
    let row: Option<(f64, f64)> = sqlx::query_as("SELECT position_s, speed FROM audiobook_progress WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await?;
    Ok(row.unwrap_or((0.0, 1.0)))
}

/// Save a position bookmark for an audiobook (np.p5.music.audiobook-chapters).
pub async fn add_bookmark(pool: &SqlitePool, item_id: i64, position_s: f64, label: &str) -> Result<()> {
    sqlx::query("INSERT INTO audiobook_bookmarks (item_id, position_s, label, created) VALUES (?,?,?,?)")
        .bind(item_id).bind(position_s)
        .bind(if label.is_empty() { None } else { Some(label) })
        .bind(now()).execute(pool).await?;
    Ok(())
}

/// Ordered (position_s, label) bookmarks for an audiobook, earliest first.
pub async fn bookmarks(pool: &SqlitePool, item_id: i64) -> Result<Vec<(f64, String)>> {
    let rows: Vec<(f64, Option<String>)> = sqlx::query_as(
        "SELECT position_s, label FROM audiobook_bookmarks WHERE item_id = ? ORDER BY position_s")
        .bind(item_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(p, l)| (p, l.unwrap_or_default())).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn speed_clamps_and_keeps_pitch() {
        assert_eq!(clamp_speed(9.0), 3.0);
        assert_eq!(clamp_speed(0.1), 0.5);
        let o = mpv_speed_options(1.5);
        assert!(o.contains(&"--audio-pitch-correction=yes".to_string()));
        assert!(o.contains(&"--speed=1.5".to_string()));
    }

    #[tokio::test]
    async fn progress_roundtrip() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/book.m4b").await;
        assert_eq!(resume(&pool, a).await.unwrap(), (0.0, 1.0));
        save_progress(&pool, a, 1234.5, 1.25).await.unwrap();
        let (pos, sp) = resume(&pool, a).await.unwrap();
        assert!((pos - 1234.5).abs() < 1e-6);
        assert!((sp - 1.25).abs() < 1e-6);
    }
}
