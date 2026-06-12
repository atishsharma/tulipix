//! `np.p4.books.progress` — reading progress + bookmarks + cross-device sync.
//!
//! Persists a per-book locator (EPUB CFI string or page index) plus bookmarks.
//! The sync backend (Account mode) replicates these rows; the merge here is
//! last-writer-wins by `updated`, which is the conflict rule sync expects.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn save(pool: &SqlitePool, item_id: i64, locator: &str, page: i64, total: Option<i64>) -> Result<()> {
    let finished = total.map_or(0, |t| (page + 1 >= t) as i64);
    sqlx::query(
        "INSERT INTO reading_progress (item_id, locator, page, total_pages, finished, updated) VALUES (?,?,?,?,?,?)
         ON CONFLICT(item_id) DO UPDATE SET locator=excluded.locator, page=excluded.page,
            total_pages=excluded.total_pages, finished=excluded.finished, updated=excluded.updated",
    ).bind(item_id).bind(locator).bind(page).bind(total).bind(finished).bind(now()).execute(pool).await?;
    Ok(())
}

/// `(page, total, finished)` or `None` if unread.
pub async fn get(pool: &SqlitePool, item_id: i64) -> Result<Option<(i64, Option<i64>, bool)>> {
    let row: Option<(i64, Option<i64>, i64)> = sqlx::query_as(
        "SELECT page, total_pages, finished FROM reading_progress WHERE item_id = ?",
    ).bind(item_id).fetch_optional(pool).await?;
    Ok(row.map(|(p, t, f)| (p, t, f != 0)))
}

/// Percent read 0..100, given the stored progress.
pub fn percent(page: i64, total: Option<i64>) -> u8 {
    match total {
        Some(t) if t > 0 => (((page + 1).min(t) as f64 / t as f64) * 100.0).round().clamp(0.0, 100.0) as u8,
        _ => 0,
    }
}

pub async fn add_bookmark(
    pool: &SqlitePool, item_id: i64, locator: &str, page: i64,
    note: Option<&str>, color: Option<&str>,
) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO bookmarks (item_id, locator, page, note, created, color) VALUES (?,?,?,?,?,?) RETURNING id",
    ).bind(item_id).bind(locator).bind(page).bind(note).bind(now()).bind(color).fetch_one(pool).await?)
}

/// `(id, page, note, color)` ordered by page — the colour-coded jump list.
pub async fn bookmarks(pool: &SqlitePool, item_id: i64) -> Result<Vec<(i64, i64, Option<String>, Option<String>)>> {
    Ok(sqlx::query_as("SELECT id, page, note, color FROM bookmarks WHERE item_id = ? ORDER BY page")
        .bind(item_id).fetch_all(pool).await?)
}

pub async fn delete_bookmark(pool: &SqlitePool, bookmark_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM bookmarks WHERE id = ?").bind(bookmark_id).execute(pool).await?;
    Ok(())
}

// ── Reading time + streaks + yearly goal (np.p5.books.stats) ────────────────

/// Unix epoch days (UTC) — integer adjacency makes streaks trivial.
fn today() -> i64 {
    now() / 86_400
}

/// Add `seconds` of reading time for `item_id` to today's session row.
pub async fn add_reading_time(pool: &SqlitePool, item_id: i64, seconds: i64) -> Result<()> {
    if seconds <= 0 { return Ok(()); }
    sqlx::query(
        "INSERT INTO reading_sessions (item_id, day, seconds) VALUES (?,?,?)
         ON CONFLICT(item_id, day) DO UPDATE SET seconds = seconds + excluded.seconds",
    ).bind(item_id).bind(today()).bind(seconds).execute(pool).await?;
    Ok(())
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ReadingStats {
    pub total_seconds: i64,
    /// Consecutive days (ending today or yesterday) with any reading time.
    pub streak_days: i64,
    /// Books whose `finished` flag flipped within the current calendar year.
    pub finished_this_year: i64,
    pub year_goal: i64,
}

pub async fn reading_stats(pool: &SqlitePool) -> Result<ReadingStats> {
    let total_seconds: i64 =
        sqlx::query_scalar("SELECT COALESCE(SUM(seconds), 0) FROM reading_sessions")
            .fetch_one(pool).await?;
    // Distinct active days, newest first; streak = run of adjacent days that
    // starts today or yesterday (yesterday keeps an unbroken streak alive
    // before today's first read).
    let days: Vec<i64> =
        sqlx::query_scalar("SELECT DISTINCT day FROM reading_sessions ORDER BY day DESC")
            .fetch_all(pool).await?;
    let t = today();
    let mut streak_days = 0i64;
    if let Some(&first) = days.first() {
        if first >= t - 1 {
            streak_days = 1;
            let mut prev = first;
            for &d in &days[1..] {
                if d == prev - 1 { streak_days += 1; prev = d; } else { break; }
            }
        }
    }
    // Calendar-year boundary in unix seconds, computed from epoch days (UTC).
    // Good enough for a goal counter; avoids a date dependency.
    let year_start = year_start_secs(now());
    let finished_this_year: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_progress WHERE finished = 1 AND updated >= ?",
    ).bind(year_start).fetch_one(pool).await?;
    let year_goal = get_pref(pool, "year_goal").await?.and_then(|v| v.parse().ok()).unwrap_or(12);
    Ok(ReadingStats { total_seconds, streak_days, finished_this_year, year_goal })
}

/// Unix seconds at Jan 1 00:00 UTC of the year containing `at_secs`. Walks
/// back day by day (≤365 steps) — trivially correct, no date dependency.
fn year_start_secs(at_secs: i64) -> i64 {
    let days = at_secs / 86_400;
    let y = civil_year(days);
    let mut d0 = days;
    while civil_year(d0 - 1) == y { d0 -= 1; }
    d0 * 86_400
}

/// Calendar year of a unix epoch-day (UTC). Hinnant civil-from-days.
fn civil_year(days: i64) -> i64 {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    if m <= 2 { y + 1 } else { y }
}

pub async fn get_pref(pool: &SqlitePool, key: &str) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT value FROM book_prefs WHERE key = ?")
        .bind(key).fetch_optional(pool).await?)
}

pub async fn set_pref(pool: &SqlitePool, key: &str, value: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO book_prefs (key, value) VALUES (?,?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    ).bind(key).bind(value).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_book};

    #[test]
    fn percent_math() {
        assert_eq!(percent(0, Some(100)), 1);
        assert_eq!(percent(99, Some(100)), 100);
        assert_eq!(percent(5, None), 0);
    }

    #[tokio::test]
    async fn save_marks_finished_at_end() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/x.epub", "epub", false).await;
        assert!(get(&pool, id).await.unwrap().is_none());
        save(&pool, id, "loc1", 5, Some(100)).await.unwrap();
        let (p, t, fin) = get(&pool, id).await.unwrap().unwrap();
        assert_eq!((p, t, fin), (5, Some(100), false));
        save(&pool, id, "loc-end", 99, Some(100)).await.unwrap();
        assert!(get(&pool, id).await.unwrap().unwrap().2);
    }

    #[tokio::test]
    async fn bookmarks_ordered_by_page_with_color() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/x.epub", "epub", false).await;
        add_bookmark(&pool, id, "b", 40, Some("later"), Some("#ffd54f")).await.unwrap();
        let bid = add_bookmark(&pool, id, "a", 10, None, None).await.unwrap();
        let b = bookmarks(&pool, id).await.unwrap();
        assert_eq!(b[0].1, 10);
        assert_eq!(b[1].2.as_deref(), Some("later"));
        assert_eq!(b[1].3.as_deref(), Some("#ffd54f"));
        delete_bookmark(&pool, bid).await.unwrap();
        assert_eq!(bookmarks(&pool, id).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn reading_time_accumulates_and_streaks() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/x.epub", "epub", false).await;
        add_reading_time(&pool, id, 120).await.unwrap();
        add_reading_time(&pool, id, 60).await.unwrap();
        add_reading_time(&pool, id, 0).await.unwrap(); // no-op
        // Backfill yesterday + day before to form a 3-day streak.
        let t = today();
        for d in [t - 1, t - 2] {
            sqlx::query("INSERT INTO reading_sessions (item_id, day, seconds) VALUES (?,?,300)")
                .bind(id).bind(d).execute(&pool).await.unwrap();
        }
        let s = reading_stats(&pool).await.unwrap();
        assert_eq!(s.total_seconds, 180 + 600);
        assert_eq!(s.streak_days, 3);
        assert_eq!(s.year_goal, 12); // default
    }

    #[tokio::test]
    async fn streak_breaks_on_gap() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/x.epub", "epub", false).await;
        let t = today();
        for d in [t, t - 1, t - 3] { // gap at t-2
            sqlx::query("INSERT INTO reading_sessions (item_id, day, seconds) VALUES (?,?,60)")
                .bind(id).bind(d).execute(&pool).await.unwrap();
        }
        assert_eq!(reading_stats(&pool).await.unwrap().streak_days, 2);
    }

    #[tokio::test]
    async fn goal_pref_roundtrip() {
        let (_t, pool) = open_pool().await;
        set_pref(&pool, "year_goal", "20").await.unwrap();
        assert_eq!(get_pref(&pool, "year_goal").await.unwrap().as_deref(), Some("20"));
        let s = reading_stats(&pool).await.unwrap();
        assert_eq!(s.year_goal, 20);
    }

    #[test]
    fn civil_year_sane() {
        assert_eq!(civil_year(0), 1970);
        assert_eq!(civil_year(19_722), 2023); // 2023-12-31
        assert_eq!(civil_year(19_723), 2024); // 2024-01-01
        // Jan 1 2026 00:00 UTC = 1_767_225_600
        assert_eq!(year_start_secs(1_767_225_600 + 5_000_000), 1_767_225_600);
    }
}
