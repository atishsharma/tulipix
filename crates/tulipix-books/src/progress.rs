//! Reading progress + bookmarks. Saved on every page turn and on reader
//! close; feeds the Home hero card and the In Progress / Completed stats.

use crate::schema::now;
use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Default, Clone, sqlx::FromRow)]
pub struct Progress {
    pub book_id: i64,
    pub page: i64,
    pub total_pages: i64,
    pub char_offset: i64,
    pub percent: f64,
    pub time_read_secs: i64,
}

pub async fn get(pool: &SqlitePool, book_id: i64) -> Result<Option<Progress>> {
    Ok(sqlx::query_as(
        "SELECT book_id, page, total_pages, char_offset, percent, time_read_secs
         FROM progress WHERE book_id = ?",
    )
    .bind(book_id)
    .fetch_optional(pool)
    .await?)
}

/// Upsert position. `session_secs` is added to the running total.
/// Reaching the last page flips `books.finished`.
pub async fn save(
    pool: &SqlitePool,
    book_id: i64,
    page: i64,
    total_pages: i64,
    char_offset: i64,
    session_secs: i64,
) -> Result<()> {
    let percent = if total_pages > 0 {
        ((page + 1) as f64 / total_pages as f64 * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    };
    sqlx::query(
        "INSERT INTO progress (book_id, page, total_pages, char_offset, percent,
                               time_read_secs, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(book_id) DO UPDATE SET
            page = excluded.page, total_pages = excluded.total_pages,
            char_offset = excluded.char_offset, percent = excluded.percent,
            time_read_secs = progress.time_read_secs + excluded.time_read_secs,
            updated_at = excluded.updated_at",
    )
    .bind(book_id)
    .bind(page)
    .bind(total_pages)
    .bind(char_offset)
    .bind(percent)
    .bind(session_secs)
    .bind(now())
    .execute(pool)
    .await?;
    if total_pages > 0 && page + 1 >= total_pages {
        sqlx::query("UPDATE books SET finished = 1 WHERE id = ?")
            .bind(book_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

// ── bookmarks ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Bookmark {
    pub id: i64,
    pub page: i64,
    pub char_offset: i64,
    pub note: String,
    pub color: String,
    pub created_at: i64,
}

pub async fn bookmarks(pool: &SqlitePool, book_id: i64) -> Result<Vec<Bookmark>> {
    Ok(sqlx::query_as(
        "SELECT id, page, char_offset, note, color, created_at
         FROM bookmarks WHERE book_id = ? ORDER BY page",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await?)
}

pub async fn is_bookmarked(pool: &SqlitePool, book_id: i64, page: i64) -> Result<bool> {
    let n: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM bookmarks WHERE book_id = ? AND page = ?")
            .bind(book_id)
            .bind(page)
            .fetch_one(pool)
            .await?;
    Ok(n > 0)
}

/// Reflow-book bookmark check: any bookmark whose char offset falls inside the
/// current page's [start, end) range. Page indexes shift on repagination
/// (font/margin changes); char offsets don't.
pub async fn is_bookmarked_range(
    pool: &SqlitePool,
    book_id: i64,
    start: i64,
    end: i64,
) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM bookmarks
         WHERE book_id = ? AND char_offset >= ? AND char_offset < ?",
    )
    .bind(book_id)
    .bind(start)
    .bind(end)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

/// Reflow-book bookmark toggle keyed by char-offset range; returns new state.
/// `page` is stored for display only.
pub async fn toggle_bookmark_range(
    pool: &SqlitePool,
    book_id: i64,
    start: i64,
    end: i64,
    page: i64,
) -> Result<bool> {
    if is_bookmarked_range(pool, book_id, start, end).await? {
        sqlx::query(
            "DELETE FROM bookmarks
             WHERE book_id = ? AND char_offset >= ? AND char_offset < ?",
        )
        .bind(book_id)
        .bind(start)
        .bind(end)
        .execute(pool)
        .await?;
        Ok(false)
    } else {
        sqlx::query(
            "INSERT INTO bookmarks (book_id, page, char_offset, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(book_id)
        .bind(page)
        .bind(start)
        .bind(now())
        .execute(pool)
        .await?;
        Ok(true)
    }
}

/// Remove one bookmark by row id (bookmarks-panel trash button).
pub async fn remove_bookmark(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM bookmarks WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Toggle bookmark on a page; returns the new state.
pub async fn toggle_bookmark(
    pool: &SqlitePool,
    book_id: i64,
    page: i64,
    char_offset: i64,
) -> Result<bool> {
    if is_bookmarked(pool, book_id, page).await? {
        sqlx::query("DELETE FROM bookmarks WHERE book_id = ? AND page = ?")
            .bind(book_id)
            .bind(page)
            .execute(pool)
            .await?;
        Ok(false)
    } else {
        sqlx::query(
            "INSERT INTO bookmarks (book_id, page, char_offset, created_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(book_id)
        .bind(page)
        .bind(char_offset)
        .bind(now())
        .execute(pool)
        .await?;
        Ok(true)
    }
}
