//! Library scanner — walks the configured book folders, upserts new files
//! (metadata + cover extracted off the DB path), and flags rows whose file
//! vanished as `missing`.

use crate::{covers, format_of, metadata, schema};
use anyhow::Result;
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug, Default, Clone, Copy)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub missing: usize,
}

/// Configured scan roots ("Add books").
pub async fn folders(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar("SELECT path FROM book_folders ORDER BY added_at")
        .fetch_all(pool)
        .await?)
}

pub async fn add_folder(pool: &SqlitePool, path: &str) -> Result<()> {
    sqlx::query("INSERT OR IGNORE INTO book_folders (path, added_at) VALUES (?, ?)")
        .bind(path)
        .bind(schema::now())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn remove_folder(pool: &SqlitePool, path: &str) -> Result<()> {
    sqlx::query("DELETE FROM book_folders WHERE path = ?")
        .bind(path)
        .execute(pool)
        .await?;
    Ok(())
}

/// Scan every configured folder. Blocking file work (walk, metadata, covers)
/// runs on the calling thread — callers wrap in `spawn_blocking`/task.
pub async fn scan_all(pool: &SqlitePool) -> Result<ScanReport> {
    let mut report = ScanReport::default();
    let roots = folders(pool).await?;
    let mut seen: Vec<String> = Vec::new();

    for root in &roots {
        let files: Vec<PathBuf> = WalkDir::new(root)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_type().is_file())
            .map(|e| e.into_path())
            .filter(|p| format_of(p).is_some())
            .collect();
        for f in files {
            let path_str = f.display().to_string();
            seen.push(path_str.clone());
            let known: Option<i64> =
                sqlx::query_scalar("SELECT id FROM books WHERE path = ?")
                    .bind(&path_str)
                    .fetch_optional(pool)
                    .await?;
            if let Some(id) = known {
                sqlx::query("UPDATE books SET missing = 0 WHERE id = ?")
                    .bind(id)
                    .execute(pool)
                    .await?;
                continue;
            }
            add_one(pool, &f).await?;
            report.added += 1;
        }
    }

    // Flag rows whose file is gone (only those under a configured root —
    // one-off opened files outside the roots keep their row).
    let all: Vec<(i64, String)> = sqlx::query_as("SELECT id, path FROM books WHERE missing = 0")
        .fetch_all(pool)
        .await?;
    for (id, p) in all {
        let under_root = roots.iter().any(|r| p.starts_with(r.as_str()));
        if under_root && !seen.contains(&p) && !Path::new(&p).exists() {
            sqlx::query("UPDATE books SET missing = 1 WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            report.missing += 1;
        }
    }
    Ok(report)
}

/// Index book *contents* for library-wide search (FTS). Walks books that have
/// no `book_fts` row yet, extracts their text (blocking parse, off-thread), and
/// inserts it. Best-effort + resumable — run in a background task after a scan.
/// Image-only formats (PDF/CBZ/CBR) have no reflow text and are skipped.
pub async fn index_contents(pool: &SqlitePool) -> Result<()> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT b.id, b.path, b.format FROM books b
         WHERE b.missing = 0
           AND b.format IN ('epub','mobi','azw3')
           AND b.id NOT IN (SELECT CAST(book_id AS INTEGER) FROM book_fts)",
    )
    .fetch_all(pool)
    .await?;
    for (id, path, format) in rows {
        let p = PathBuf::from(&path);
        let body = tokio::task::spawn_blocking(move || crate::book_text(&p, &format).join("\n"))
            .await
            .unwrap_or_default();
        let _ = crate::library::fts_index(pool, id, &body).await;
    }
    Ok(())
}

/// Upsert a single file (also used by drag-drop / file-picker adds).
pub async fn add_one(pool: &SqlitePool, path: &Path) -> Result<i64> {
    let Some(format) = format_of(path) else { anyhow::bail!("unsupported file") };
    let path_str = path.display().to_string();
    let meta = metadata::extract(path, format);
    let size = std::fs::metadata(path).map(|m| m.len() as i64).unwrap_or(0);
    let cover = covers::extract(path, format)
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (path, format, title, author, genre, series, published,
                            size_bytes, cover_path, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET missing = 0
         RETURNING id",
    )
    .bind(&path_str)
    .bind(format)
    .bind(&meta.title)
    .bind(&meta.author)
    .bind(&meta.genre)
    .bind(&meta.series)
    .bind(&meta.published)
    .bind(size)
    .bind(&cover)
    .bind(schema::now())
    .fetch_one(pool)
    .await?;
    Ok(id)
}
