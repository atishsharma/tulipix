//! Library queries powering the Books grid: filter chips (format / status /
//! author), sort dropdown, and search.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct BookRow {
    pub id: i64,
    pub path: String,
    pub format: String,
    pub title: String,
    pub author: String,
    pub genre: String,
    pub series: String,
    pub cover_path: String,
    pub size_bytes: i64,
    pub added_at: i64,
    pub finished: i64,
    pub favorite: i64,
    pub missing: i64,
    pub rating: f64,
    pub percent: f64,
    pub last_read: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    #[default]
    RecentlyAdded,
    RecentlyRead,
    Title,
    Author,
    Rating,
}

impl Sort {
    pub fn from_index(i: usize) -> Self {
        match i {
            1 => Self::RecentlyRead,
            2 => Self::Title,
            3 => Self::Author,
            4 => Self::Rating,
            _ => Self::RecentlyAdded,
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Self::RecentlyAdded => "b.added_at DESC",
            Self::RecentlyRead => "last_read DESC, b.added_at DESC",
            Self::Title => "b.title COLLATE NOCASE ASC",
            Self::Author => "b.author COLLATE NOCASE ASC, b.title COLLATE NOCASE ASC",
            Self::Rating => "b.rating DESC, b.title COLLATE NOCASE ASC",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// "" = all; else one of epub/pdf/mobi/azw3/cbz/cbr.
    pub format: String,
    /// "" | "reading" | "finished" | "unread".
    pub status: String,
    /// "" = all authors.
    pub author: String,
    /// "" = all genres.
    pub genre: String,
    /// Search over title/author/series.
    pub query: String,
    pub sort: Sort,
}

pub async fn list(pool: &SqlitePool, f: &Filter) -> Result<Vec<BookRow>> {
    let mut sql = String::from(
        "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
                b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
                b.rating,
                COALESCE(p.percent, 0.0) AS percent,
                COALESCE(p.updated_at, 0) AS last_read
         FROM books b LEFT JOIN progress p ON p.book_id = b.id
         WHERE b.missing = 0",
    );
    let mut binds: Vec<String> = Vec::new();
    if !f.format.is_empty() {
        sql.push_str(" AND b.format = ?");
        binds.push(f.format.clone());
    }
    if !f.author.is_empty() {
        sql.push_str(" AND b.author = ?");
        binds.push(f.author.clone());
    }
    if !f.genre.is_empty() {
        sql.push_str(" AND b.genre = ?");
        binds.push(f.genre.clone());
    }
    match f.status.as_str() {
        "reading" => sql.push_str(" AND b.finished = 0 AND COALESCE(p.percent, 0) > 0"),
        "finished" => sql.push_str(" AND b.finished = 1"),
        "unread" => sql.push_str(" AND b.finished = 0 AND COALESCE(p.percent, 0) = 0"),
        "favorites" => sql.push_str(" AND b.favorite = 1"),
        _ => {}
    }
    if !f.query.is_empty() {
        sql.push_str(" AND (b.title LIKE ? OR b.author LIKE ? OR b.series LIKE ?)");
        let like = format!("%{}%", f.query);
        binds.push(like.clone());
        binds.push(like.clone());
        binds.push(like);
    }
    sql.push_str(" ORDER BY ");
    sql.push_str(f.sort.sql());
    let mut q = sqlx::query_as::<_, BookRow>(&sql);
    for b in &binds {
        q = q.bind(b);
    }
    Ok(q.fetch_all(pool).await?)
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<BookRow>> {
    Ok(sqlx::query_as(
        "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
                b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
                b.rating,
                COALESCE(p.percent, 0.0) AS percent,
                COALESCE(p.updated_at, 0) AS last_read
         FROM books b LEFT JOIN progress p ON p.book_id = b.id
         WHERE b.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Set a book's local rating (0–5 stars; 0 = unrated).
pub async fn set_rating(pool: &SqlitePool, id: i64, rating: f64) -> Result<()> {
    sqlx::query("UPDATE books SET rating = ? WHERE id = ?")
        .bind(rating.clamp(0.0, 5.0))
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Distinct authors for the author filter dropdown.
pub async fn authors(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT author FROM books
         WHERE missing = 0 AND author != '' ORDER BY author COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?)
}

/// Distinct formats present in the library (for the format chips row).
pub async fn formats(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT format FROM books WHERE missing = 0 ORDER BY format",
    )
    .fetch_all(pool)
    .await?)
}

/// Formats with their book counts, most common first (rail + toolbar).
pub async fn format_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT format, COUNT(*) FROM books
         WHERE missing = 0 AND format != '' GROUP BY format ORDER BY COUNT(*) DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Genres with their book counts, most common first (rail).
pub async fn genre_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT genre, COUNT(*) FROM books
         WHERE missing = 0 AND genre != '' GROUP BY genre ORDER BY COUNT(*) DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Distinct genres present in the library (for the genre chips row).
pub async fn genres(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT genre FROM books
         WHERE missing = 0 AND genre != '' ORDER BY genre COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?)
}

/// Flip the favorite flag on a book; returns the new state.
pub async fn toggle_favorite(pool: &SqlitePool, id: i64) -> Result<bool> {
    sqlx::query("UPDATE books SET favorite = 1 - favorite WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let fav: i64 = sqlx::query_scalar("SELECT favorite FROM books WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    Ok(fav != 0)
}

/// Remove a book row (file untouched) plus its progress/marks/notes.
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<()> {
    for t in ["annotations", "bookmarks", "progress"] {
        sqlx::query(&format!("DELETE FROM {t} WHERE book_id = ?"))
            .bind(id)
            .execute(pool)
            .await?;
    }
    sqlx::query("DELETE FROM books WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}
