//! Books-specific schema overlay on top of the shared `items` proxy table.
//!
//! `book_meta` hangs off `items` by `item_id`; `series` / `collections` /
//! `bookmarks` / `toc` carry their own identity. Reading progress, bookmarks
//! and scraped metadata survive a file move because they key on `item_id`.

use anyhow::Result;
use sqlx::SqlitePool;

pub const BOOKS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS series (
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    name  TEXT NOT NULL UNIQUE,
    sort_name TEXT
);

CREATE TABLE IF NOT EXISTS book_meta (
    item_id      INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    format       TEXT    NOT NULL,        -- 'epub' | 'cbz' | 'cbr' | 'pdf'
    title        TEXT,
    author       TEXT,
    series_id    INTEGER REFERENCES series(id) ON DELETE SET NULL,
    series_index REAL,
    description  TEXT,
    cover_path   TEXT,
    publisher    TEXT,
    published    INTEGER,
    language     TEXT,
    page_count   INTEGER,
    is_comic     INTEGER NOT NULL DEFAULT 0,
    rtl          INTEGER NOT NULL DEFAULT 0,   -- manga right-to-left
    isbn         TEXT,
    comicvine_id INTEGER,
    added        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS book_meta_series_idx ON book_meta(series_id, series_index);
CREATE INDEX IF NOT EXISTS book_meta_author_idx ON book_meta(author);
CREATE INDEX IF NOT EXISTS book_meta_comic_idx  ON book_meta(is_comic);

CREATE TABLE IF NOT EXISTS collections (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS collection_items (
    collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
    item_id       INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY (collection_id, item_id)
);

CREATE TABLE IF NOT EXISTS toc (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    idx      INTEGER NOT NULL,
    title    TEXT    NOT NULL,
    href     TEXT,            -- EPUB spine href / anchor
    page     INTEGER,         -- PDF/CBZ page index
    UNIQUE(item_id, idx)
);
CREATE INDEX IF NOT EXISTS toc_item_idx ON toc(item_id, idx);

CREATE TABLE IF NOT EXISTS reading_progress (
    item_id     INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    -- EPUB: CFI/locator string; comics/PDF: page index as text.
    locator     TEXT,
    page        INTEGER NOT NULL DEFAULT 0,
    total_pages INTEGER,
    finished    INTEGER NOT NULL DEFAULT 0,
    updated     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS reading_progress_updated_idx ON reading_progress(updated DESC);

CREATE TABLE IF NOT EXISTS bookmarks (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    locator  TEXT,
    page     INTEGER,
    note     TEXT,
    created  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS bookmarks_item_idx ON bookmarks(item_id);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(BOOKS_SCHEMA).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::{apply_proxy_schema, DbHandle};

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("books.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "books".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply_proxy_schema(&pool, "books").await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    /// Insert an `items` row + `book_meta` of a given format; return item id.
    pub(crate) async fn add_book(pool: &SqlitePool, path: &str, format: &str, comic: bool) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'books', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO book_meta (item_id, format, is_comic) VALUES (?,?,?)")
            .bind(id).bind(format).bind(comic as i64).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_meta").fetch_one(&pool).await.unwrap();
    }
}
