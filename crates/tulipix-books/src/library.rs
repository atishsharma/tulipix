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
    pub summary: String,
    pub summary_fetched_at: i64,
    #[sqlx(default)]
    pub published: String,
    #[sqlx(default)]
    pub net_rating: f64,
    #[sqlx(default)]
    pub trashed: i64,
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
    /// "" = all series.
    pub series: String,
    /// 0 = no collection filter; else a collections.id.
    pub collection: i64,
    /// Search over title/author/series.
    pub query: String,
    /// Restrict to these book ids (content/FTS search). Empty = no restriction;
    /// a single sentinel `-1` forces an empty result (search ran, no matches).
    pub ids: Vec<i64>,
    pub sort: Sort,
}

pub async fn list(pool: &SqlitePool, f: &Filter) -> Result<Vec<BookRow>> {
    // The "missing" status surfaces broken-path books (so the grid badge can
    // render + user can relink); every other view hides them.
    // Trash is its own view; missing surfaces broken paths; everything else
    // hides both trashed and missing books.
    let base_clause = match f.status.as_str() {
        "trashed" => "b.trashed = 1",
        "missing" => "b.missing = 1 AND b.trashed = 0",
        _ => "b.missing = 0 AND b.trashed = 0",
    };
    let mut sql = format!(
        "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
                b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
                b.rating, b.summary, b.summary_fetched_at, b.published, b.net_rating, b.trashed,
                COALESCE(p.percent, 0.0) AS percent,
                COALESCE(p.updated_at, 0) AS last_read
         FROM books b LEFT JOIN progress p ON p.book_id = b.id
         WHERE {base_clause}",
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
    if !f.series.is_empty() {
        sql.push_str(" AND b.series = ?");
        binds.push(f.series.clone());
    }
    if f.collection > 0 {
        sql.push_str(
            " AND b.id IN (SELECT book_id FROM collection_books WHERE collection_id = ?)",
        );
        binds.push(f.collection.to_string());
    }
    if !f.ids.is_empty() {
        let list = f.ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND b.id IN ({list})"));
    }
    match f.status.as_str() {
        "reading" => sql.push_str(" AND b.finished = 0 AND COALESCE(p.percent, 0) > 0"),
        "finished" => sql.push_str(" AND b.finished = 1"),
        "unread" => sql.push_str(" AND b.finished = 0 AND COALESCE(p.percent, 0) = 0"),
        "favorites" => sql.push_str(" AND b.favorite = 1"),
        _ => {}
    }
    if !f.query.is_empty() {
        sql.push_str(
            " AND (b.title LIKE ? ESCAPE '\\' OR b.author LIKE ? ESCAPE '\\' OR b.series LIKE ? ESCAPE '\\')",
        );
        // Escape LIKE metacharacters so a query with % or _ matches literally.
        let esc = f.query.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
        let like = format!("%{esc}%");
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
                b.rating, b.summary, b.summary_fetched_at, b.published, b.net_rating,
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

/// Cache fetched online metadata (P6). Only overwrites a field when the new
/// value is non-empty, so a partial fetch never wipes existing data. Always
/// stamps `summary_fetched_at` so the backfill won't retry this book.
pub async fn set_metadata(
    pool: &SqlitePool,
    id: i64,
    summary: &str,
    published: &str,
    net_rating: f64,
) -> Result<()> {
    sqlx::query(
        "UPDATE books SET
            summary   = CASE WHEN ? != '' THEN ? ELSE summary   END,
            published = CASE WHEN ? != '' THEN ? ELSE published END,
            net_rating = CASE WHEN ? > 0  THEN ? ELSE net_rating END,
            summary_fetched_at = ?
         WHERE id = ?",
    )
    .bind(summary)
    .bind(summary)
    .bind(published)
    .bind(published)
    .bind(net_rating)
    .bind(net_rating)
    .bind(crate::schema::now())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Books that have never had an online-metadata fetch, oldest-added first, up
/// to `limit`. Drives the background backfill so tiles show real publication
/// dates without opening each book.
pub async fn needs_metadata(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, title, author FROM books
         WHERE missing = 0 AND trashed = 0 AND summary_fetched_at = 0
         ORDER BY added_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Point a book at a new file path + clear its missing flag (relink a moved
/// file). Used by the "missing" grid filter's relink action.
pub async fn relink(pool: &SqlitePool, id: i64, new_path: &str) -> Result<()> {
    sqlx::query("UPDATE books SET path = ?, missing = 0 WHERE id = ?")
        .bind(new_path)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Distinct authors for the author filter dropdown.
pub async fn authors(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT author FROM books
         WHERE missing = 0 AND trashed = 0 AND author != '' ORDER BY author COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?)
}

/// Distinct formats present in the library (for the format chips row).
pub async fn formats(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT format FROM books WHERE missing = 0 AND trashed = 0 ORDER BY format",
    )
    .fetch_all(pool)
    .await?)
}

/// Formats with their book counts, most common first (rail + toolbar).
pub async fn format_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT format, COUNT(*) FROM books
         WHERE missing = 0 AND trashed = 0 AND format != '' GROUP BY format ORDER BY COUNT(*) DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Genres with their book counts, most common first (rail).
pub async fn genre_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT genre, COUNT(*) FROM books
         WHERE missing = 0 AND trashed = 0 AND genre != '' GROUP BY genre ORDER BY COUNT(*) DESC",
    )
    .fetch_all(pool)
    .await?)
}

/// Distinct genres present in the library (for the genre chips row).
pub async fn genres(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT genre FROM books
         WHERE missing = 0 AND trashed = 0 AND genre != '' ORDER BY genre COLLATE NOCASE",
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
    for t in ["annotations", "bookmarks", "progress", "collection_books"] {
        sqlx::query(&format!("DELETE FROM {t} WHERE book_id = ?"))
            .bind(id)
            .execute(pool)
            .await?;
    }
    sqlx::query("DELETE FROM book_fts WHERE CAST(book_id AS INTEGER) = ?")
        .bind(id)
        .execute(pool)
        .await
        .ok();
    sqlx::query("DELETE FROM books WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Soft-delete: flag the row trashed. The file on disk is left untouched —
/// the book just moves from the library to the Trash tab.
pub async fn trash(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE books SET trashed = 1, trashed_at = ? WHERE id = ?")
        .bind(crate::schema::now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Restore a trashed book: clear the trashed flag (no file to move back —
/// trashing never touches the disk).
pub async fn restore(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE books SET trashed = 0, trashed_at = 0 WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// All non-empty cover paths (for the background 3D pre-bake sweep).
pub async fn cover_paths(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT cover_path FROM books WHERE cover_path != ''")
            .fetch_all(pool)
            .await?,
    )
}

/// Books with no extracted cover art — input for the placeholder-cover bake
/// (path, title, author).
pub async fn coverless_books(pool: &SqlitePool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT path, title, author FROM books
         WHERE cover_path = '' AND missing = 0 AND trashed = 0",
    )
    .fetch_all(pool)
    .await?)
}

/// Count of books currently in the Trash (for the toolbar chip badge).
pub async fn trashed_count(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE trashed = 1")
        .fetch_one(pool)
        .await?)
}

/// Series with book counts, most-populous first (rail + grouping). Only series
/// with 2+ books (a single-book "series" isn't one).
pub async fn series_counts(pool: &SqlitePool) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT series, COUNT(*) FROM books
         WHERE missing = 0 AND trashed = 0 AND series != ''
         GROUP BY series HAVING COUNT(*) >= 2 ORDER BY COUNT(*) DESC, series COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?)
}

// ── collections ───────────────────────────────────────────────────────────────

/// Collections with their book counts (id, name, count).
pub async fn collections(pool: &SqlitePool) -> Result<Vec<(i64, String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT c.id, c.name, COUNT(cb.book_id)
         FROM collections c LEFT JOIN collection_books cb ON cb.collection_id = c.id
         GROUP BY c.id, c.name ORDER BY c.name COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?)
}

/// Create a collection (no-op on empty name); returns its id.
pub async fn collection_create(pool: &SqlitePool, name: &str) -> Result<i64> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "empty collection name");
    let r = sqlx::query("INSERT INTO collections (name, created_at) VALUES (?, ?)")
        .bind(name)
        .bind(crate::schema::now())
        .execute(pool)
        .await?;
    Ok(r.last_insert_rowid())
}

pub async fn collection_rename(pool: &SqlitePool, id: i64, name: &str) -> Result<()> {
    sqlx::query("UPDATE collections SET name = ? WHERE id = ?")
        .bind(name.trim())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn collection_delete(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM collection_books WHERE collection_id = ?").bind(id).execute(pool).await?;
    sqlx::query("DELETE FROM collections WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Add/remove a book to/from a collection (idempotent).
pub async fn collection_add(pool: &SqlitePool, cid: i64, book_id: i64) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO collection_books (collection_id, book_id) VALUES (?, ?)")
        .bind(cid)
        .bind(book_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn collection_remove(pool: &SqlitePool, cid: i64, book_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM collection_books WHERE collection_id = ? AND book_id = ?")
        .bind(cid)
        .bind(book_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Collection ids a book belongs to.
pub async fn book_collections(pool: &SqlitePool, book_id: i64) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT collection_id FROM collection_books WHERE book_id = ?")
        .bind(book_id)
        .fetch_all(pool)
        .await?)
}

// ── full-text search (contents) ───────────────────────────────────────────────

/// Index (or re-index) a book's full text for library-wide search. Cheap when
/// the text is already in hand (e.g. on open). Replaces any prior rows.
pub async fn fts_index(pool: &SqlitePool, book_id: i64, body: &str) -> Result<()> {
    sqlx::query("DELETE FROM book_fts WHERE CAST(book_id AS INTEGER) = ?")
        .bind(book_id)
        .execute(pool)
        .await?;
    if !body.trim().is_empty() {
        sqlx::query("INSERT INTO book_fts (body, book_id) VALUES (?, ?)")
            .bind(body)
            .bind(book_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Is a book already indexed for content search?
pub async fn fts_has(pool: &SqlitePool, book_id: i64) -> Result<bool> {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_fts WHERE CAST(book_id AS INTEGER) = ?",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await?;
    Ok(n > 0)
}

/// Book ids whose contents match `query` (FTS5 MATCH). Empty query → empty.
pub async fn fts_search(pool: &SqlitePool, query: &str) -> Result<Vec<i64>> {
    let q = query.trim();
    if q.is_empty() {
        return Ok(Vec::new());
    }
    // Quote the query as a single FTS phrase so punctuation can't break MATCH.
    let phrase = format!("\"{}\"", q.replace('"', " "));
    // FTS5 stores columns as text — CAST so sqlx decodes i64 cleanly.
    Ok(sqlx::query_scalar(
        "SELECT CAST(book_id AS INTEGER) FROM book_fts WHERE book_fts MATCH ? LIMIT 200",
    )
    .bind(phrase)
    .fetch_all(pool)
    .await?)
}

// ── reading days + stats ──────────────────────────────────────────────────────

/// Record that the user read today (unix day number). Idempotent.
pub async fn mark_read_today(pool: &SqlitePool) -> Result<()> {
    let day = crate::schema::now() / 86400;
    sqlx::query("INSERT OR IGNORE INTO reading_days (day) VALUES (?)")
        .bind(day)
        .execute(pool)
        .await?;
    Ok(())
}

/// All distinct reading days (unix day numbers), for streak + heatmap.
pub async fn reading_days(pool: &SqlitePool) -> Result<Vec<i64>> {
    Ok(sqlx::query_scalar("SELECT day FROM reading_days ORDER BY day")
        .fetch_all(pool)
        .await?)
}

/// Aggregate reading stats: (total_secs, days_read, books_finished, books_started).
pub async fn reading_totals(pool: &SqlitePool) -> Result<(i64, i64, i64, i64)> {
    let secs: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(time_read_secs), 0) FROM progress")
        .fetch_one(pool)
        .await?;
    let days: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_days").fetch_one(pool).await?;
    let finished: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE missing = 0 AND trashed = 0 AND finished = 1")
            .fetch_one(pool)
            .await?;
    let started: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM progress WHERE COALESCE(percent, 0) > 0",
    )
    .fetch_one(pool)
    .await?;
    Ok((secs, days, finished, started))
}

/// Per-book reading time, most first: (title, secs). Top `limit`.
pub async fn time_per_book(pool: &SqlitePool, limit: i64) -> Result<Vec<(String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT b.title, p.time_read_secs
         FROM progress p JOIN books b ON b.id = p.book_id
         WHERE b.missing = 0 AND b.trashed = 0 AND p.time_read_secs > 0
         ORDER BY p.time_read_secs DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}
