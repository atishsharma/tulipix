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
    /// Treat-as-magazine flag (hides reader search; user toggle).
    #[sqlx(default)]
    pub magazine: i64,
    /// Right-to-left page order (manga CBZ/CBR; user toggle).
    #[sqlx(default)]
    pub rtl: i64,
    /// Remembered reader view: -1 unset · 0 odd · 1 even · 2 single.
    #[sqlx(default)]
    pub reader_view: i64,
    /// Total seconds read (progress join; 0 when never opened).
    #[sqlx(default)]
    pub time_read: i64,
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

/// Build the shared `FROM … WHERE …` tail for a filter, plus its bind values.
/// Both the count and the page query run off this so they can never disagree.
fn where_tail(f: &Filter) -> (String, Vec<String>) {
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
        "FROM books b LEFT JOIN progress p ON p.book_id = b.id
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
    (sql, binds)
}

/// One page of the filtered library, plus the unpaged total (for the pager).
/// Paging happens in SQL — the grid shows 6 tiles, so fetching the whole table
/// and slicing in Rust meant a full scan per pager click.
pub async fn list_page(
    pool: &SqlitePool,
    f: &Filter,
    limit: i64,
    offset: i64,
) -> Result<(Vec<BookRow>, i64)> {
    let (tail, binds) = where_tail(f);

    let count_sql = format!("SELECT COUNT(*) {tail}");
    let mut count_q = sqlx::query_scalar::<_, i64>(sqlx::AssertSqlSafe(&*count_sql));
    for b in &binds {
        count_q = count_q.bind(b);
    }
    let total: i64 = count_q.fetch_one(pool).await?;

    let sql = format!(
        "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
                b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
                b.rating, b.summary, b.summary_fetched_at, b.published, b.net_rating, b.trashed,
                COALESCE(p.percent, 0.0) AS percent,
                COALESCE(p.updated_at, 0) AS last_read
         {tail} ORDER BY {} LIMIT ? OFFSET ?",
        f.sort.sql(),
    );
    let mut q = sqlx::query_as::<_, BookRow>(sqlx::AssertSqlSafe(&*sql));
    for b in &binds {
        q = q.bind(b);
    }
    let rows = q.bind(limit).bind(offset.max(0)).fetch_all(pool).await?;
    Ok((rows, total))
}

pub async fn get(pool: &SqlitePool, id: i64) -> Result<Option<BookRow>> {
    Ok(sqlx::query_as(
        "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
                b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
                b.rating, b.summary, b.summary_fetched_at, b.published, b.net_rating,
                b.magazine, b.rtl, b.reader_view,
                COALESCE(p.time_read_secs, 0) AS time_read,
                COALESCE(p.percent, 0.0) AS percent,
                COALESCE(p.updated_at, 0) AS last_read
         FROM books b LEFT JOIN progress p ON p.book_id = b.id
         WHERE b.id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// One run of a natural-order sort key: digit runs compare as numbers, the
/// rest case-insensitively as text.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum NatPart {
    Num(u64),
    Text(String),
}

/// Split a title into a natural-order key, so "Vol. 2" sorts before "Vol. 10"
/// (plain lexicographic order gets series volumes backwards past 9).
fn natural_key(s: &str) -> Vec<NatPart> {
    let mut out = Vec::new();
    let mut it = s.chars().peekable();
    while let Some(&c) = it.peek() {
        if c.is_ascii_digit() {
            let mut n = String::new();
            while let Some(&d) = it.peek() {
                if !d.is_ascii_digit() {
                    break;
                }
                n.push(d);
                it.next();
            }
            // A run too long for u64 sorts last rather than panicking.
            out.push(NatPart::Num(n.parse().unwrap_or(u64::MAX)));
        } else {
            let mut t = String::new();
            while let Some(&d) = it.peek() {
                if d.is_ascii_digit() {
                    break;
                }
                t.extend(d.to_lowercase());
                it.next();
            }
            out.push(NatPart::Text(t));
        }
    }
    out
}

/// The next book of the same series after `id`, in natural volume order.
/// Prefers the first unfinished entry; if every later volume is finished,
/// returns the immediate successor (a deliberate re-read). `None` when the
/// book has no series or is the last entry.
pub async fn next_in_series(pool: &SqlitePool, id: i64) -> Result<Option<BookRow>> {
    let Some(cur) = get(pool, id).await? else { return Ok(None) };
    if cur.series.trim().is_empty() {
        return Ok(None);
    }
    let mut rows: Vec<BookRow> = sqlx::query_as(
        "SELECT b.id, b.path, b.format, b.title, b.author, b.genre, b.series,
                b.cover_path, b.size_bytes, b.added_at, b.finished, b.favorite, b.missing,
                b.rating, b.summary, b.summary_fetched_at, b.published, b.net_rating, b.trashed,
                COALESCE(p.percent, 0.0) AS percent,
                COALESCE(p.updated_at, 0) AS last_read
         FROM books b LEFT JOIN progress p ON p.book_id = b.id
         WHERE b.series = ? AND b.missing = 0 AND b.trashed = 0",
    )
    .bind(&cur.series)
    .fetch_all(pool)
    .await?;
    rows.sort_by(|a, b| natural_key(&a.title).cmp(&natural_key(&b.title)));
    let Some(i) = rows.iter().position(|b| b.id == id) else { return Ok(None) };
    let rest: Vec<BookRow> = rows.into_iter().skip(i + 1).collect();
    Ok(rest
        .iter()
        .find(|b| b.finished == 0)
        .cloned()
        .or_else(|| rest.into_iter().next()))
}

#[cfg(test)]
mod tests {
    use super::natural_key;

    #[test]
    fn volumes_sort_numerically_not_lexicographically() {
        let mut v = vec!["Berserk Vol. 10", "Berserk Vol. 2", "Berserk Vol. 1"];
        v.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));
        assert_eq!(v, ["Berserk Vol. 1", "Berserk Vol. 2", "Berserk Vol. 10"]);
    }

    #[test]
    fn ordering_is_case_insensitive_and_handles_bare_numbers() {
        let mut v = vec!["c03", "C1", "c20", "c2"];
        v.sort_by(|a, b| natural_key(a).cmp(&natural_key(b)));
        assert_eq!(v, ["C1", "c2", "c03", "c20"]);
    }
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

/// Flip the treat-as-magazine flag; returns the new state.
pub async fn toggle_magazine(pool: &SqlitePool, id: i64) -> Result<bool> {
    sqlx::query("UPDATE books SET magazine = 1 - magazine WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let on: i64 = sqlx::query_scalar("SELECT magazine FROM books WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    Ok(on != 0)
}

/// Flip the right-to-left (manga) flag; returns the new state.
pub async fn toggle_rtl(pool: &SqlitePool, id: i64) -> Result<bool> {
    sqlx::query("UPDATE books SET rtl = 1 - rtl WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    let on: i64 = sqlx::query_scalar("SELECT rtl FROM books WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    Ok(on != 0)
}

/// Remember the reader view for a book (0 odd · 1 even · 2 single).
pub async fn set_reader_view(pool: &SqlitePool, id: i64, view: i64) -> Result<()> {
    sqlx::query("UPDATE books SET reader_view = ? WHERE id = ?")
        .bind(view)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Remove a book row (file untouched) plus its progress/marks/notes.
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<()> {
    for t in ["annotations", "bookmarks", "progress", "collection_books"] {
        sqlx::query(sqlx::AssertSqlSafe(format!("DELETE FROM {t} WHERE book_id = ?")))
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

/// Repoint books at re-encoded cover files (the one-shot PNG→JPEG flat-cover
/// migration). Pairs are `(old_path, new_path)`.
pub async fn repoint_covers(pool: &SqlitePool, pairs: &[(String, String)]) -> Result<()> {
    for (old, new) in pairs {
        sqlx::query("UPDATE books SET cover_path = ? WHERE cover_path = ?")
            .bind(new)
            .bind(old)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Books with no extracted cover art — input for the placeholder-cover bake
/// (path, title, author).
pub async fn coverless_books(pool: &SqlitePool) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as(
        // `art_state = 1` matters: a book waiting on the background art builder
        // also has an empty `cover_path`, and it may well have real embedded
        // art. Drawing it a title-card placeholder here would beat the builder
        // to the punch and the generated card would win.
        "SELECT path, title, author FROM books
         WHERE cover_path = '' AND art_state = 1 AND missing = 0 AND trashed = 0",
    )
    .fetch_all(pool)
    .await?)
}

/// The next `limit` books whose cover art has not been built yet, oldest first.
///
/// Returns `(id, path, format, title, author)` — everything the blocking art
/// pipeline needs, so it never has to come back to the DB mid-batch.
pub async fn needs_art(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, String, String, String, String)>> {
    Ok(sqlx::query_as(
        "SELECT id, path, format, title, author FROM books
         WHERE art_state = 0 AND missing = 0 AND trashed = 0
         ORDER BY added_at ASC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Record the outcome of building one book's art. `cover_path` is empty when
/// the file had no extractable art — the row is still stamped done, because a
/// placeholder was drawn and baked for it and re-trying would find nothing.
pub async fn set_art(pool: &SqlitePool, id: i64, cover_path: &str) -> Result<()> {
    sqlx::query("UPDATE books SET cover_path = ?, art_state = 1 WHERE id = ?")
        .bind(cover_path)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// How many books are still waiting on their art (progress display / tests).
pub async fn pending_art(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM books WHERE art_state = 0 AND missing = 0 AND trashed = 0",
    )
    .fetch_one(pool)
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

// ── smart collections (saved filters) ─────────────────────────────────────────

/// Save the current view as a named smart collection. Unlike a collection,
/// this stores the *filter*, so its membership tracks the library.
pub async fn smart_create(pool: &SqlitePool, name: &str, f: &Filter) -> Result<i64> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "empty smart collection name");
    let r = sqlx::query(
        "INSERT INTO smart_collections
            (name, format, status, author, genre, series, query, sort, created_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(name)
    .bind(&f.format)
    .bind(&f.status)
    .bind(&f.author)
    .bind(&f.genre)
    .bind(&f.series)
    .bind(&f.query)
    .bind(f.sort as i64)
    .bind(crate::schema::now())
    .execute(pool)
    .await?;
    Ok(r.last_insert_rowid())
}

/// Every smart collection as `(id, name, filter)`.
pub async fn smart_list(pool: &SqlitePool) -> Result<Vec<(i64, String, Filter)>> {
    let rows: Vec<(i64, String, String, String, String, String, String, String, i64)> =
        sqlx::query_as(
            "SELECT id, name, format, status, author, genre, series, query, sort
             FROM smart_collections ORDER BY name COLLATE NOCASE",
        )
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, format, status, author, genre, series, query, sort)| {
            (
                id,
                name,
                Filter {
                    format,
                    status,
                    author,
                    genre,
                    series,
                    query,
                    sort: Sort::from_index(sort as usize),
                    ..Default::default()
                },
            )
        })
        .collect())
}

pub async fn smart_delete(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM smart_collections WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

// ── duplicate detection ───────────────────────────────────────────────────────

/// Collapse a title/author to a comparison key: case, punctuation, articles and
/// spacing all vary between sources for what is plainly the same book.
fn dup_key(title: &str, author: &str) -> String {
    let norm = |s: &str| -> String {
        let lower = s.to_lowercase();
        let mut out: String = lower
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { ' ' })
            .collect::<String>()
            .split_whitespace()
            .filter(|w| !matches!(*w, "the" | "a" | "an"))
            .collect::<Vec<_>>()
            .join(" ");
        out.truncate(80);
        out
    };
    format!("{}|{}", norm(title), norm(author))
}

/// Ids of books that have at least one duplicate.
///
/// Two signals, unioned: the same normalised title+author (the same book from
/// two sources, possibly in different formats), and the same size+format (the
/// literal same file filed twice). Size alone is not enough — plenty of
/// distinct books share a byte count.
pub async fn duplicate_ids(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows: Vec<(i64, String, String, i64, String)> = sqlx::query_as(
        "SELECT id, title, author, size_bytes, format FROM books
         WHERE missing = 0 AND trashed = 0",
    )
    .fetch_all(pool)
    .await?;

    let mut by_name: std::collections::HashMap<String, Vec<i64>> = std::collections::HashMap::new();
    let mut by_file: std::collections::HashMap<(i64, String), Vec<i64>> =
        std::collections::HashMap::new();
    for (id, title, author, size, format) in rows {
        let key = dup_key(&title, &author);
        if !key.trim_matches('|').trim().is_empty() {
            by_name.entry(key).or_default().push(id);
        }
        if size > 0 {
            by_file.entry((size, format)).or_default().push(id);
        }
    }
    let mut ids: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for group in by_name.values().chain(by_file.values()) {
        if group.len() > 1 {
            ids.extend(group);
        }
    }
    Ok(ids.into_iter().collect())
}

#[cfg(test)]
mod dup_tests {
    use super::dup_key;

    #[test]
    fn same_book_different_punctuation_matches() {
        assert_eq!(
            dup_key("The Hobbit", "J.R.R. Tolkien"),
            dup_key("hobbit", "J R R  Tolkien")
        );
    }

    #[test]
    fn different_books_do_not_match() {
        assert_ne!(dup_key("Dune", "Herbert"), dup_key("Dune Messiah", "Herbert"));
        assert_ne!(dup_key("Dune", "Herbert"), dup_key("Dune", "Someone Else"));
    }
}

// ── full-text search (contents) ───────────────────────────────────────────────

/// Index (or re-index) a book's full text for library-wide search. Cheap when
/// the text is already in hand (e.g. on open). Replaces any prior rows.
pub async fn fts_index(pool: &SqlitePool, book_id: i64, body: &str) -> Result<()> {
    // Only pay for the delete scan when there's actually a row to remove —
    // on a first index (the common case) this skips it entirely.
    if fts_has(pool, book_id).await.unwrap_or(false) {
        sqlx::query("DELETE FROM book_fts WHERE CAST(book_id AS INTEGER) = ?")
            .bind(book_id)
            .execute(pool)
            .await?;
    }
    if !body.trim().is_empty() {
        sqlx::query("INSERT INTO book_fts (body, book_id) VALUES (?, ?)")
            .bind(body)
            .bind(book_id)
            .execute(pool)
            .await?;
    }
    // The flag means "we have processed this book", not "it produced a row" —
    // otherwise a book with no extractable text would be retried on every
    // scan, forever.
    fts_mark(pool, book_id, true).await
}

/// Record whether a book has been through content indexing.
pub async fn fts_mark(pool: &SqlitePool, book_id: i64, indexed: bool) -> Result<()> {
    sqlx::query("UPDATE books SET fts_indexed = ? WHERE id = ?")
        .bind(i64::from(indexed))
        .bind(book_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Is a book already indexed for content search? Reads the flag on `books`,
/// not the FTS table — see `fts_indexed` in the schema for why that matters.
pub async fn fts_has(pool: &SqlitePool, book_id: i64) -> Result<bool> {
    let n: i64 = sqlx::query_scalar("SELECT fts_indexed FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await?
        .unwrap_or(0);
    Ok(n != 0)
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
