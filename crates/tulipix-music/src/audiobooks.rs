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

/// Set (or clear) `is_audiobook` for every track whose `folder` matches.
/// Returns the number of track_meta rows updated.
pub async fn set_folder_flag(pool: &SqlitePool, folder: &str, on: bool) -> Result<u64> {
    let res = sqlx::query("UPDATE track_meta SET is_audiobook = ? WHERE folder = ?")
        .bind(if on { 1 } else { 0 })
        .bind(folder)
        .execute(pool)
        .await?;
    Ok(res.rows_affected())
}

/// Distinct audiobook folders with chapter counts, ordered by folder path.
/// One row per book card (np.p5.music.audiobook-chapters).
pub async fn book_folders(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT folder, COUNT(*) AS n FROM track_meta \
         WHERE is_audiobook = 1 AND folder IS NOT NULL AND folder <> '' \
         GROUP BY folder ORDER BY folder")
        .fetch_all(pool).await?;
    Ok(rows)
}

/// item_ids of one book's chapters, ordered by track_no then item_id.
pub async fn book_chapters(pool: &SqlitePool, folder: &str) -> Result<Vec<i64>> {
    let rows: Vec<i64> = sqlx::query_scalar(
        "SELECT item_id FROM track_meta \
         WHERE is_audiobook = 1 AND folder = ? \
         ORDER BY COALESCE(track_no, 1000000), item_id")
        .bind(folder).fetch_all(pool).await?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[tokio::test]
    async fn folder_flag_scopes_to_one_folder() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/books/dune/ch01.mp3").await;
        let b = add_track(&pool, "/books/dune/ch02.mp3").await;
        let c = add_track(&pool, "/music/song.mp3").await;
        for (id, folder) in [(a, "/books/dune"), (b, "/books/dune"), (c, "/music")] {
            sqlx::query("UPDATE track_meta SET folder = ? WHERE item_id = ?")
                .bind(folder).bind(id).execute(&pool).await.unwrap();
        }
        let n = set_folder_flag(&pool, "/books/dune", true).await.unwrap();
        assert_eq!(n, 2);
        let flagged: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE is_audiobook = 1 ORDER BY item_id")
            .fetch_all(&pool).await.unwrap();
        assert_eq!(flagged, vec![a, b]);
        let n2 = set_folder_flag(&pool, "/books/dune", false).await.unwrap();
        assert_eq!(n2, 2);
        let still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_meta WHERE is_audiobook = 1")
            .fetch_one(&pool).await.unwrap();
        assert_eq!(still, 0);
    }

    #[tokio::test]
    async fn book_folders_groups_and_counts() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/books/dune/ch01.mp3").await;
        let b = add_track(&pool, "/books/dune/ch02.mp3").await;
        let c = add_track(&pool, "/books/hobbit/all.m4b").await;
        for (id, folder) in [(a, "/books/dune"), (b, "/books/dune"), (c, "/books/hobbit")] {
            sqlx::query("UPDATE track_meta SET folder = ?, is_audiobook = 1 WHERE item_id = ?")
                .bind(folder).bind(id).execute(&pool).await.unwrap();
        }
        let books = book_folders(&pool).await.unwrap();
        assert_eq!(books, vec![
            ("/books/dune".to_string(), 2),
            ("/books/hobbit".to_string(), 1),
        ]);
        let dune = book_chapters(&pool, "/books/dune").await.unwrap();
        assert_eq!(dune, vec![a, b]);
    }

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
