//! books.db schema — standalone (no cross-DB FKs). Idempotent.

use anyhow::Result;
use sqlx::SqlitePool;

/// books.db schema revision, stored in `PRAGMA user_version`. Bump when adding
/// a one-time data migration below, and gate that migration on the old value.
const SCHEMA_REV: i64 = 1;

/// Bookmarks table. Deliberately NOT `UNIQUE(book_id, page)`: reflow books key
/// their bookmarks by char offset, and after a repagination (font/margin
/// change) two different offsets can land on the same page number — the old
/// constraint turned that into a silent insert failure.
const BOOKMARKS_SQL: &str = "CREATE TABLE IF NOT EXISTS bookmarks (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            book_id     INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            page        INTEGER NOT NULL,
            char_offset INTEGER NOT NULL DEFAULT 0,
            note        TEXT    NOT NULL DEFAULT '',
            color       TEXT    NOT NULL DEFAULT '',
            created_at  INTEGER NOT NULL
        )";

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

    sqlx::query(BOOKMARKS_SQL).execute(pool).await?;

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

    // Distinct days the user read (unix day number = secs/86400) — powers the
    // reading-stats streak + heatmap.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS reading_days (
            day INTEGER PRIMARY KEY
        )",
    )
    .execute(pool)
    .await?;

    // User collections (shelves beyond genre) + membership.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS collections (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT NOT NULL,
            created_at INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS collection_books (
            collection_id INTEGER NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
            book_id       INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE,
            PRIMARY KEY (collection_id, book_id)
        )",
    )
    .execute(pool)
    .await?;

    // Smart collections — a saved filter, not a saved list of books, so the
    // membership stays live as the library grows. Stored as one column per
    // `Filter` field rather than a serialised blob: the shapes match exactly,
    // and it keeps the table queryable and dependency-free.
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS smart_collections (
            id         INTEGER PRIMARY KEY AUTOINCREMENT,
            name       TEXT    NOT NULL,
            format     TEXT    NOT NULL DEFAULT '',
            status     TEXT    NOT NULL DEFAULT '',
            author     TEXT    NOT NULL DEFAULT '',
            genre      TEXT    NOT NULL DEFAULT '',
            series     TEXT    NOT NULL DEFAULT '',
            query      TEXT    NOT NULL DEFAULT '',
            sort       INTEGER NOT NULL DEFAULT 0,
            created_at INTEGER NOT NULL
        )",
    )
    .execute(pool)
    .await?;

    // Full-text search over book contents (FTS5). `book_id` is UNINDEXED so the
    // table maps a match back to a book without duplicating it in the index.
    sqlx::query(
        "CREATE VIRTUAL TABLE IF NOT EXISTS book_fts USING fts5(
            body,
            book_id UNINDEXED
        )",
    )
    .execute(pool)
    .await?;

    // Older DBs may predate these columns — add best-effort (errors when they
    // already exist, which is fine).
    for alter in [
        "ALTER TABLE books ADD COLUMN rating REAL NOT NULL DEFAULT 0",
        // R1 book-detail popup: cached online summary + when it was fetched.
        "ALTER TABLE books ADD COLUMN summary TEXT NOT NULL DEFAULT ''",
        "ALTER TABLE books ADD COLUMN summary_fetched_at INTEGER NOT NULL DEFAULT 0",
        // Soft-delete → Trash: flag, when, and the path to restore the file to.
        "ALTER TABLE books ADD COLUMN trashed INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE books ADD COLUMN trashed_at INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE books ADD COLUMN orig_path TEXT NOT NULL DEFAULT ''",
        // P6: fetched average rating (0 = unknown); shown in the detail popup.
        "ALTER TABLE books ADD COLUMN net_rating REAL NOT NULL DEFAULT 0",
        // Treat-as-magazine flag (detail popup toggle): hides the reader's
        // in-book search for PDFs whose text layer is useless.
        "ALTER TABLE books ADD COLUMN magazine INTEGER NOT NULL DEFAULT 0",
        // rtl predates some DBs (it's in CREATE, but older tables may lack it).
        "ALTER TABLE books ADD COLUMN rtl INTEGER NOT NULL DEFAULT 0",
        // Per-book reader view: -1 unset · 0 odd spreads · 1 even spreads ·
        // 2 single page. Remembered across sessions.
        "ALTER TABLE books ADD COLUMN reader_view INTEGER NOT NULL DEFAULT -1",
        // Last-indexed file mtime (secs). With size_bytes this is the rescan
        // change signal: a book edited/replaced in place gets re-extracted
        // instead of keeping stale metadata + cover forever.
        "ALTER TABLE books ADD COLUMN file_mtime INTEGER NOT NULL DEFAULT 0",
        // Has this book's text been pushed into book_fts? Asking the FTS table
        // directly means a full scan of it — `book_id` is UNINDEXED and FTS5
        // keeps every column in one row, so reading it drags each book's whole
        // body off disk. Tracked here instead, where it's a cheap indexed read.
        "ALTER TABLE books ADD COLUMN fts_indexed INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = sqlx::query(alter).execute(pool).await;
    }

    // Older DBs created `bookmarks` with UNIQUE(book_id, page) — see
    // BOOKMARKS_SQL. Rebuild those without it (the constraint's auto-index is
    // the only way to detect it after the fact).
    let legacy: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master
         WHERE type = 'index' AND tbl_name = 'bookmarks' AND name LIKE 'sqlite_autoindex%'",
    )
    .fetch_optional(pool)
    .await
    .unwrap_or(None);
    if legacy.is_some() {
        for stmt in [
            "ALTER TABLE bookmarks RENAME TO bookmarks_legacy",
            BOOKMARKS_SQL,
            "INSERT INTO bookmarks (id, book_id, page, char_offset, note, color, created_at)
             SELECT id, book_id, page, char_offset, note, color, created_at FROM bookmarks_legacy",
            "DROP TABLE bookmarks_legacy",
        ] {
            sqlx::query(stmt).execute(pool).await?;
        }
    }

    // Sorts are COLLATE NOCASE, so the index has to be too — the old plain
    // title/author indexes were never used by `library::list`.
    for idx in [
        "CREATE INDEX IF NOT EXISTS idx_books_title_nc  ON books(title COLLATE NOCASE)",
        "CREATE INDEX IF NOT EXISTS idx_books_author_nc ON books(author COLLATE NOCASE)",
        // Every list/home query filters on this pair before anything else, then
        // orders by added_at — one covering index serves the default view.
        "CREATE INDEX IF NOT EXISTS idx_books_live ON books(trashed, missing, added_at DESC)",
        // "Recently Read" sort + the Home hero join.
        "CREATE INDEX IF NOT EXISTS idx_progress_updated ON progress(updated_at DESC)",
        "CREATE INDEX IF NOT EXISTS idx_bookmarks_book ON bookmarks(book_id, char_offset)",
        "CREATE INDEX IF NOT EXISTS idx_annots_book ON annotations(book_id)",
        // Drives the background content indexer's "what's left?" query.
        "CREATE INDEX IF NOT EXISTS idx_books_fts_todo ON books(fts_indexed, missing)",
    ] {
        sqlx::query(idx).execute(pool).await?;
    }

    // One-time migrations, gated on the DB's own schema revision.
    //
    // Deliberately NOT inferred from the data: a guard like "has any row got
    // fts_indexed = 1?" never trips on a library where nothing is indexed yet,
    // so the backfill re-ran its full FTS scan at every single startup.
    let rev: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(pool).await.unwrap_or(0);
    if rev < 1 {
        // Backfill `fts_indexed` for DBs predating the column. This is the very
        // scan the column exists to avoid — but now it runs exactly once.
        let _ = sqlx::query(
            "UPDATE books SET fts_indexed = 1
             WHERE id IN (SELECT CAST(book_id AS INTEGER) FROM book_fts)",
        )
        .execute(pool)
        .await;
    }
    if rev < SCHEMA_REV {
        sqlx::query(&format!("PRAGMA user_version = {SCHEMA_REV}")).execute(pool).await?;
    }
    // Superseded by the NOCASE pair above.
    for drop in ["DROP INDEX IF EXISTS idx_books_title", "DROP INDEX IF EXISTS idx_books_author"] {
        let _ = sqlx::query(drop).execute(pool).await;
    }
    Ok(())
}

/// Unix now, seconds.
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
