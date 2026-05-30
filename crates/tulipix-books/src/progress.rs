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

pub async fn add_bookmark(pool: &SqlitePool, item_id: i64, locator: &str, page: i64, note: Option<&str>) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO bookmarks (item_id, locator, page, note, created) VALUES (?,?,?,?,?) RETURNING id",
    ).bind(item_id).bind(locator).bind(page).bind(note).bind(now()).fetch_one(pool).await?)
}

pub async fn bookmarks(pool: &SqlitePool, item_id: i64) -> Result<Vec<(i64, i64, Option<String>)>> {
    Ok(sqlx::query_as("SELECT id, page, note FROM bookmarks WHERE item_id = ? ORDER BY page")
        .bind(item_id).fetch_all(pool).await?)
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
    async fn bookmarks_ordered_by_page() {
        let (_t, pool) = open_pool().await;
        let id = add_book(&pool, "/b/x.epub", "epub", false).await;
        add_bookmark(&pool, id, "b", 40, Some("later")).await.unwrap();
        add_bookmark(&pool, id, "a", 10, None).await.unwrap();
        let b = bookmarks(&pool, id).await.unwrap();
        assert_eq!(b[0].1, 10);
        assert_eq!(b[1].2.as_deref(), Some("later"));
    }
}
