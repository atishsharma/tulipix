//! Highlights + margin notes for reflowable text. Anchored by page and the
//! (start, end) char range within the paginated stream, plus a snippet of the
//! selected text so anchors survive repagination drift (best-effort re-match).

use crate::schema::now;
use anyhow::Result;
use sqlx::SqlitePool;

/// Highlight palette keys understood by the reader UI.
pub const COLORS: [&str; 4] = ["amber", "violet", "green", "rose"];

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Annotation {
    pub id: i64,
    pub book_id: i64,
    pub page: i64,
    pub start_off: i64,
    pub end_off: i64,
    pub snippet: String,
    pub note: String,
    pub color: String,
    pub created_at: i64,
}

pub async fn for_book(pool: &SqlitePool, book_id: i64) -> Result<Vec<Annotation>> {
    Ok(sqlx::query_as(
        "SELECT id, book_id, page, start_off, end_off, snippet, note, color, created_at
         FROM annotations WHERE book_id = ? ORDER BY start_off",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await?)
}

/// Annotations overlapping a page (for paint on page turn).
pub async fn for_page(pool: &SqlitePool, book_id: i64, page: i64) -> Result<Vec<Annotation>> {
    Ok(sqlx::query_as(
        "SELECT id, book_id, page, start_off, end_off, snippet, note, color, created_at
         FROM annotations WHERE book_id = ? AND page = ? ORDER BY start_off",
    )
    .bind(book_id)
    .bind(page)
    .fetch_all(pool)
    .await?)
}

pub async fn add(
    pool: &SqlitePool,
    book_id: i64,
    page: i64,
    start_off: i64,
    end_off: i64,
    snippet: &str,
    color: &str,
) -> Result<i64> {
    let color = if COLORS.contains(&color) { color } else { COLORS[0] };
    let id = sqlx::query(
        "INSERT INTO annotations (book_id, page, start_off, end_off, snippet, note, color, created_at)
         VALUES (?, ?, ?, ?, ?, '', ?, ?)",
    )
    .bind(book_id)
    .bind(page)
    .bind(start_off)
    .bind(end_off)
    .bind(snippet)
    .bind(color)
    .bind(now())
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

pub async fn set_note(pool: &SqlitePool, id: i64, note: &str) -> Result<()> {
    sqlx::query("UPDATE annotations SET note = ? WHERE id = ?")
        .bind(note)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn set_color(pool: &SqlitePool, id: i64, color: &str) -> Result<()> {
    if !COLORS.contains(&color) {
        return Ok(());
    }
    sqlx::query("UPDATE annotations SET color = ? WHERE id = ?")
        .bind(color)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM annotations WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}
