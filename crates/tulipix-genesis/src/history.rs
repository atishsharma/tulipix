//! What has already been downloaded, so a result row can say so.
//!
//! Upstream kept this as a JSONL file it re-read on every launch. The app has a
//! database layer, so this is a table — which also makes `has()` an indexed
//! lookup rather than a linear scan of the file for every row on screen.

use anyhow::Result;
use sqlx::SqlitePool;

use crate::model::Book;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS downloads (
    md5        TEXT PRIMARY KEY,
    title      TEXT NOT NULL,
    authors    TEXT,
    year       TEXT,
    extension  TEXT,
    size_bytes INTEGER,
    path       TEXT NOT NULL,
    mirror     TEXT,
    at         INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS downloads_at_idx ON downloads (at DESC);

-- What has been searched for, newest first, so a query can be run again
-- without retyping it. Keyed on the terms plus the filters they ran under:
-- the same words with a different format filter is a different search.
CREATE TABLE IF NOT EXISTS searches (
    terms    TEXT NOT NULL,
    field    TEXT NOT NULL DEFAULT 'Any',
    format   TEXT NOT NULL DEFAULT 'Any',
    language TEXT NOT NULL DEFAULT 'Any',
    results  INTEGER NOT NULL DEFAULT 0,
    at       INTEGER NOT NULL,
    PRIMARY KEY (terms, field, format, language)
);
CREATE INDEX IF NOT EXISTS searches_at_idx ON searches (at DESC);

-- Record-page metadata, cached so opening the same book twice costs one
-- request rather than two. `at` is when it was fetched, which is what a
-- refresh checks against.
CREATE TABLE IF NOT EXISTS details (
    md5         TEXT PRIMARY KEY,
    title       TEXT NOT NULL DEFAULT '',
    series      TEXT NOT NULL DEFAULT '',
    authors     TEXT NOT NULL DEFAULT '',
    publisher   TEXT NOT NULL DEFAULT '',
    year        TEXT NOT NULL DEFAULT '',
    isbn        TEXT NOT NULL DEFAULT '',
    language    TEXT NOT NULL DEFAULT '',
    pages       TEXT NOT NULL DEFAULT '',
    size        TEXT NOT NULL DEFAULT '',
    extension   TEXT NOT NULL DEFAULT '',
    description TEXT NOT NULL DEFAULT '',
    cover_src   TEXT NOT NULL DEFAULT '',
    source_url  TEXT NOT NULL DEFAULT '',
    at          INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// Opens `genesis.db` next to the other section databases, inheriting WAL mode
/// and the pool settings from the shared handle.
pub async fn open() -> Result<SqlitePool> {
    let handle = tulipix_core::db::DbHandle::open("genesis")?;
    let pool = handle.pool().await?;
    apply_schema(&pool).await?;
    Ok(pool)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub md5: String,
    pub title: String,
    pub authors: Option<String>,
    pub year: Option<String>,
    pub extension: Option<String>,
    pub size_bytes: Option<i64>,
    pub path: String,
    pub mirror: Option<String>,
    pub at: i64,
}

/// Record a completed download.
///
/// Keyed on MD5, which is the catalogue's own identity for a file, so
/// re-downloading the same book to a new location updates the row rather than
/// leaving two rows claiming to be the same thing.
pub async fn record(
    pool: &SqlitePool,
    book: &Book,
    path: &str,
    mirror: &str,
    at: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO downloads (md5, title, authors, year, extension, size_bytes, path, mirror, at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(md5) DO UPDATE SET
             path = excluded.path, mirror = excluded.mirror, at = excluded.at",
    )
    .bind(&book.md5)
    .bind(&book.title)
    .bind(&book.authors)
    .bind(&book.year)
    .bind(&book.extension)
    .bind(book.size_bytes.map(|b| b as i64))
    .bind(path)
    .bind(mirror)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

/// The MD5s already downloaded, out of the ones on screen.
///
/// A set query rather than one `has()` per row: a page of results asks this
/// once, and a per-row round trip would be 25 queries to paint one table.
pub async fn known(pool: &SqlitePool, md5s: &[String]) -> Result<Vec<String>> {
    if md5s.is_empty() {
        return Ok(Vec::new());
    }
    // One `?` per id. The count is bounded by the page size, and the values are
    // still bound rather than interpolated.
    let holes = std::iter::repeat_n("?", md5s.len()).collect::<Vec<_>>().join(",");
    let sql = format!("SELECT md5 FROM downloads WHERE md5 IN ({holes})");
    let mut q = sqlx::query_scalar::<_, String>(&sql);
    for id in md5s {
        q = q.bind(id);
    }
    Ok(q.fetch_all(pool).await?)
}

pub async fn recent(pool: &SqlitePool, limit: i64) -> Result<Vec<Entry>> {
    let rows = sqlx::query_as::<
        _,
        (String, String, Option<String>, Option<String>, Option<String>, Option<i64>, String, Option<String>, i64),
    >(
        "SELECT md5, title, authors, year, extension, size_bytes, path, mirror, at
         FROM downloads ORDER BY at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(md5, title, authors, year, extension, size_bytes, path, mirror, at)| Entry {
            md5,
            title,
            authors,
            year,
            extension,
            size_bytes,
            path,
            mirror,
            at,
        })
        .collect())
}

pub async fn clear(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM downloads").execute(pool).await?;
    Ok(())
}

// ── search history ──────────────────────────────────────────────────────────

/// One past search, enough to run it again exactly as it was.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Search {
    pub terms: String,
    pub field: String,
    pub format: String,
    pub language: String,
    pub results: i64,
    pub at: i64,
}

/// Remember a search. Repeating one moves it back to the top rather than
/// filling the list with the same words over and over.
pub async fn record_search(
    pool: &SqlitePool,
    terms: &str,
    field: &str,
    format: &str,
    language: &str,
    results: i64,
    at: i64,
) -> Result<()> {
    if terms.trim().is_empty() {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO searches (terms, field, format, language, results, at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(terms, field, format, language)
         DO UPDATE SET results = excluded.results, at = excluded.at",
    )
    .bind(terms.trim())
    .bind(field)
    .bind(format)
    .bind(language)
    .bind(results)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

/// The most recent searches, newest first.
pub async fn recent_searches(pool: &SqlitePool, limit: i64) -> Result<Vec<Search>> {
    let rows = sqlx::query_as::<_, (String, String, String, String, i64, i64)>(
        "SELECT terms, field, format, language, results, at
         FROM searches ORDER BY at DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(terms, field, format, language, results, at)| Search {
            terms,
            field,
            format,
            language,
            results,
            at,
        })
        .collect())
}

pub async fn clear_searches(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM searches").execute(pool).await?;
    Ok(())
}

// ── record-page details ─────────────────────────────────────────────────────

/// The cached record page for this MD5, if one was fetched before.
pub async fn cached_details(pool: &SqlitePool, md5: &str) -> Result<Option<crate::details::Details>> {
    let row = sqlx::query_as::<
        _,
        (String, String, String, String, String, String, String, String, String, String, String, String, String, String),
    >(
        "SELECT md5, title, series, authors, publisher, year, isbn, language, pages, size,
                extension, description, cover_src, source_url
         FROM details WHERE md5 = ?",
    )
    .bind(md5.to_ascii_lowercase())
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| crate::details::Details {
        md5: r.0,
        title: r.1,
        series: r.2,
        authors: r.3,
        publisher: r.4,
        year: r.5,
        isbn: r.6,
        language: r.7,
        pages: r.8,
        size: r.9,
        extension: r.10,
        description: r.11,
        cover_src: r.12,
        source_url: r.13,
    }))
}

/// Cache a record page, replacing whatever was there.
pub async fn store_details(
    pool: &SqlitePool,
    d: &crate::details::Details,
    at: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO details
            (md5, title, series, authors, publisher, year, isbn, language, pages, size,
             extension, description, cover_src, source_url, at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(md5) DO UPDATE SET
            title = excluded.title, series = excluded.series, authors = excluded.authors,
            publisher = excluded.publisher, year = excluded.year, isbn = excluded.isbn,
            language = excluded.language, pages = excluded.pages, size = excluded.size,
            extension = excluded.extension, description = excluded.description,
            cover_src = excluded.cover_src, source_url = excluded.source_url,
            at = excluded.at",
    )
    .bind(d.md5.to_ascii_lowercase())
    .bind(&d.title)
    .bind(&d.series)
    .bind(&d.authors)
    .bind(&d.publisher)
    .bind(&d.year)
    .bind(&d.isbn)
    .bind(&d.language)
    .bind(&d.pages)
    .bind(&d.size)
    .bind(&d.extension)
    .bind(&d.description)
    .bind(&d.cover_src)
    .bind(&d.source_url)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        apply_schema(&pool).await.unwrap();
        pool
    }

    fn book(md5: &str, title: &str) -> Book {
        Book {
            md5: md5.into(),
            title: title.into(),
            extension: Some("epub".into()),
            size_bytes: Some(4096),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn a_recorded_download_is_known_by_its_md5() {
        let pool = mem_pool().await;
        record(&pool, &book("aa", "Some Book"), "/books/a.epub", "libgen.is", 100).await.unwrap();

        let known = known(&pool, &["aa".into(), "bb".into()]).await.unwrap();
        assert_eq!(known, ["aa"]);
    }

    #[tokio::test]
    async fn asking_about_nothing_does_not_hit_the_database() {
        let pool = mem_pool().await;
        assert!(known(&pool, &[]).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn downloading_the_same_book_again_moves_the_row_rather_than_duplicating_it() {
        let pool = mem_pool().await;
        record(&pool, &book("aa", "Some Book"), "/old/a.epub", "libgen.is", 100).await.unwrap();
        record(&pool, &book("aa", "Some Book"), "/new/a.epub", "libgen.rs", 200).await.unwrap();

        let rows = recent(&pool, 10).await.unwrap();
        assert_eq!(rows.len(), 1, "md5 is the identity, so this is one book");
        assert_eq!(rows[0].path, "/new/a.epub");
        assert_eq!(rows[0].mirror.as_deref(), Some("libgen.rs"));
    }

    #[tokio::test]
    async fn recent_is_newest_first_and_respects_the_limit() {
        let pool = mem_pool().await;
        record(&pool, &book("aa", "Old"), "/a", "m", 100).await.unwrap();
        record(&pool, &book("bb", "New"), "/b", "m", 300).await.unwrap();
        record(&pool, &book("cc", "Mid"), "/c", "m", 200).await.unwrap();

        let rows = recent(&pool, 2).await.unwrap();
        assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["New", "Mid"]);
    }

    #[tokio::test]
    async fn clearing_empties_the_table() {
        let pool = mem_pool().await;
        record(&pool, &book("aa", "Some Book"), "/a", "m", 100).await.unwrap();
        clear(&pool).await.unwrap();
        assert!(recent(&pool, 10).await.unwrap().is_empty());
    }
}
