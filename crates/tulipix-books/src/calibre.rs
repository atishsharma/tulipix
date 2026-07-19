//! Import an existing Calibre library.
//!
//! Calibre keeps its catalogue in a plain SQLite `metadata.db` at the library
//! root, with the actual files under `<root>/<author>/<title> (<id>)/`. Reading
//! it directly means series, tags, ratings and blurbs come across as-is — far
//! better data than re-deriving any of it from the files, and no parsing.

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct CalibreBook {
    pub file: PathBuf,
    pub title: String,
    pub author: String,
    pub series: String,
    pub genre: String,
    pub published: String,
    pub summary: String,
    /// 0–5 stars (Calibre stores 0–10).
    pub rating: f64,
}

/// Does this look like a Calibre library root?
pub fn is_library(root: &Path) -> bool {
    root.join("metadata.db").is_file()
}

/// Open Calibre's catalogue read-only — we never write to the user's library.
async fn open_metadata(root: &Path) -> Result<SqlitePool> {
    let db = root.join("metadata.db");
    anyhow::ensure!(db.is_file(), "no metadata.db in {}", root.display());
    let opts = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(&db)
        .read_only(true)
        .immutable(true);
    Ok(sqlx::SqlitePool::connect_with(opts).await?)
}

/// Every Calibre book whose file exists on disk and is a format we can read.
/// Books with several formats contribute the first supported one.
pub async fn scan(root: &Path) -> Result<Vec<CalibreBook>> {
    let cal = open_metadata(root).await?;
    // One row per (book, format). Calibre's schema has been stable for years;
    // the joins are left joins so a book missing a series or rating still comes.
    let rows: Vec<(i64, String, String, Option<String>, Option<String>, Option<f64>, Option<String>, String, String)> =
        sqlx::query_as(
            "SELECT b.id,
                    b.title,
                    b.path,
                    (SELECT a.name FROM authors a
                      JOIN books_authors_link al ON al.author = a.id
                      WHERE al.book = b.id LIMIT 1),
                    (SELECT s.name FROM series s
                      JOIN books_series_link sl ON sl.series = s.id
                      WHERE sl.book = b.id LIMIT 1),
                    (SELECT r.rating FROM ratings r
                      JOIN books_ratings_link rl ON rl.rating = r.id
                      WHERE rl.book = b.id LIMIT 1),
                    (SELECT c.text FROM comments c WHERE c.book = b.id LIMIT 1),
                    d.format,
                    d.name
             FROM books b JOIN data d ON d.book = b.id",
        )
        .fetch_all(&cal)
        .await
        .context("read Calibre metadata.db")?;

    // Tags → genre, joined per book.
    let tag_rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT tl.book, t.name FROM tags t JOIN books_tags_link tl ON tl.tag = t.id",
    )
    .fetch_all(&cal)
    .await
    .unwrap_or_default();
    let mut tags: std::collections::HashMap<i64, Vec<String>> = std::collections::HashMap::new();
    for (book, name) in tag_rows {
        tags.entry(book).or_default().push(name);
    }

    // Publication year, best-effort (Calibre stores a full timestamp).
    let pub_rows: Vec<(i64, Option<String>)> =
        sqlx::query_as("SELECT id, pubdate FROM books").fetch_all(&cal).await.unwrap_or_default();
    let pubdates: std::collections::HashMap<i64, String> = pub_rows
        .into_iter()
        .filter_map(|(id, d)| d.map(|d| (id, d.chars().take(4).collect())))
        .filter(|(_, y): &(i64, String)| y.chars().all(|c| c.is_ascii_digit()) && y.len() == 4)
        .collect();

    cal.close().await;

    let mut out: Vec<CalibreBook> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    for (id, title, path, author, series, rating, comments, format, name) in rows {
        let ext = format.to_ascii_lowercase();
        let file = root.join(&path).join(format!("{name}.{ext}"));
        // Only formats this app can actually open, and only files really there.
        if crate::format_of(&file).is_none() || !file.is_file() {
            continue;
        }
        // First supported format per book wins — importing the same book twice
        // under two formats would just duplicate it in the library.
        if !seen.insert(id) {
            continue;
        }
        out.push(CalibreBook {
            file,
            title,
            author: author.unwrap_or_default(),
            series: series.unwrap_or_default(),
            genre: tags.get(&id).map(|t| t.join(", ")).unwrap_or_default(),
            published: pubdates.get(&id).cloned().unwrap_or_default(),
            summary: comments.map(|c| crate::epub::html_to_text(&c)).unwrap_or_default(),
            // Calibre rates 0–10 in half-star steps.
            rating: rating.unwrap_or(0.0) / 2.0,
        });
    }
    Ok(out)
}

/// Overlay a Calibre library's catalogue onto books already ingested from the
/// same folders, matched by absolute path.
///
/// Deliberately not an importer of its own: the ordinary folder scan already
/// walks a Calibre library and picks up every file, covers and all. All that's
/// missing is the *curated* data — series, tags, ratings, blurbs — which is
/// exactly what `metadata.db` has and what we'd otherwise be guessing at. So
/// pointing "Add books" at a Calibre root just works, with no separate flow.
///
/// Calibre's values win where it has them; ours stay where it doesn't.
/// Returns how many rows were updated.
pub async fn apply_metadata(pool: &SqlitePool, root: &Path) -> Result<usize> {
    let books = scan(root).await?;
    let mut updated = 0usize;
    for b in books {
        let path = b.file.display().to_string();
        let r = sqlx::query(
            "UPDATE books SET
                title     = CASE WHEN ? != '' THEN ? ELSE title     END,
                author    = CASE WHEN ? != '' THEN ? ELSE author    END,
                series    = CASE WHEN ? != '' THEN ? ELSE series    END,
                genre     = CASE WHEN ? != '' THEN ? ELSE genre     END,
                published = CASE WHEN ? != '' THEN ? ELSE published END,
                summary   = CASE WHEN ? != '' THEN ? ELSE summary   END,
                rating    = CASE WHEN ? > 0  THEN ? ELSE rating    END
             WHERE path = ?",
        )
        .bind(&b.title).bind(&b.title)
        .bind(&b.author).bind(&b.author)
        .bind(&b.series).bind(&b.series)
        .bind(&b.genre).bind(&b.genre)
        .bind(&b.published).bind(&b.published)
        .bind(&b.summary).bind(&b.summary)
        .bind(b.rating).bind(b.rating)
        .bind(&path)
        .execute(pool)
        .await;
        if r.map(|r| r.rows_affected() > 0).unwrap_or(false) {
            updated += 1;
        }
    }
    Ok(updated)
}
