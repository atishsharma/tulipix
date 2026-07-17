//! books.db schema — standalone (no cross-DB FKs). Idempotent.

use anyhow::Result;
use sqlx::SqlitePool;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS books (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            path        TEXT    NOT NULL UNIQUE,
            format      TEXT    NOT NULL,
            title       TEXT    NOT NULL,
            author      TEXT    NOT NULL DEFAULT '',
            genre       TEXT    NOT NULL DEFAULT '',
            series      TEXT    NOT NULL DEFAULT '',
            published   TEXT    NOT NULL DEFAULT '',
            rating      REAL    NOT NULL DEFAULT 0,
            size_bytes  INTEGER NOT NULL DEFAULT 0,
            cover_path  TEXT    NOT NULL DEFAULT '',
            added_at    INTEGER NOT NULL,
            favorite    INTEGER NOT NULL DEFAULT 0,
            finished    INTEGER NOT NULL DEFAULT 0,
            rtl         INTEGER NOT NULL DEFAULT 0,
            missing     INTEGER NOT NULL DEFAULT 0
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS progress (
            book_id        INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
            page           INTEGER NOT NULL DEFAULT 0,
            total_pages    INTEGER NOT NULL DEFAULT 0,
            char_offset    INTEGER NOT NULL DEFAULT 0,
            percent        REAL    NOT NULL DEFAULT 0,
            time_read_secs INTEGER NOT NULL DEFAULT 0,
            updated_at     INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS bookmarks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            page        INTEGER NOT NULL,
            char_offset INTEGER NOT NULL DEFAULT 0,
            note        TEXT    NOT NULL DEFAULT '',
            color       TEXT    NOT NULL DEFAULT '',
            created_at  INTEGER NOT NULL,
            UNIQUE(book_id, page)
        )",
    )
    .execute(pool)
    .await?;

    sqlx::query(
        "CREATE TABLE IF NOT EXISTS annotations (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            page        INTEGER NOT NULL,
            char_offset INTEGER NOT NULL DEFAULT 0,
            selected    TEXT    NOT NULL DEFAULT '',
            note        TEXT    NOT NULL DEFAULT '',
            color       TEXT    NOT NULL DEFAULT '#6c4df6',
            created_at  INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Configured scan roots for the Books section ("Add books" folders).
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS book_folders (
            path     TEXT PRIMARY KEY,
            added_at INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Older DBs may predate the rating column — add it best-effort (errors when
    // it already exists, which is fine).
    let _ = sqlx::query("ALTER TABLE books ADD COLUMN rating REAL NOT NULL DEFAULT 0")
        .execute(pool)
        .await;

    sqlx::query("CREATE INDEX IF NOT EXISTS idx_books_title  ON books(title)")
        .execute(pool)
        .await?;
    sqlx::query("CREATE INDEX IF NOT EXISTS idx_books_author ON books(author)")
        .execute(pool)
        .await?;
    Ok(())
}

/// Unix now, seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
