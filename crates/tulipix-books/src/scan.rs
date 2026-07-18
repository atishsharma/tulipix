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
    scan_all_progress(pool, |_, _, _| {}).await
}

/// Like [`scan_all`] but reports progress: `on_progress(total, done, title)` is
/// called once per candidate file (title = the file being processed), so the UI
/// can show a real bar + the entries as they land instead of a 0→100 jump.
pub async fn scan_all_progress<F>(pool: &SqlitePool, on_progress: F) -> Result<ScanReport>
where
    F: Fn(u32, u32, &str) + Send,
{
    let mut report = ScanReport::default();
    let roots = folders(pool).await?;
    let mut seen: Vec<String> = Vec::new();

    // Collect every candidate file up front so `total` is known for the bar.
    let mut all_files: Vec<PathBuf> = Vec::new();
    for root in &roots {
        all_files.extend(
            WalkDir::new(root)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().is_file())
                .map(|e| e.into_path())
                .filter(|p| format_of(p).is_some()),
        );
    }
    let total = all_files.len() as u32;

    // Known files first — one cheap un-missing UPDATE each, no file parsing.
    let known: std::collections::HashSet<String> =
        sqlx::query_scalar::<_, String>("SELECT path FROM books")
            .fetch_all(pool)
            .await?
            .into_iter()
            .collect();
    let mut done = 0u32;
    let mut fresh: Vec<PathBuf> = Vec::new();
    for f in &all_files {
        let path_str = f.display().to_string();
        if known.contains(&path_str) {
            sqlx::query("UPDATE books SET missing = 0 WHERE path = ?")
                .bind(&path_str)
                .execute(pool)
                .await?;
            done += 1;
            on_progress(total, done, f.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
        } else {
            fresh.push(f.clone());
        }
        seen.push(path_str);
    }

    // New files: ingest 4 at a time — per-file metadata/cover/bake work is
    // blocking-heavy, so a small pool ~4x's the add speed on multi-core.
    // ponytail: fixed 4 workers (matches the -j 1 build box); make it a
    // setting if a beefier machine ever wants more.
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let mut set = tokio::task::JoinSet::new();
    for f in fresh {
        let sem = sem.clone();
        let pool = pool.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let title = f.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
            (title, add_one(&pool, &f).await.is_ok())
        });
    }
    while let Some(res) = set.join_next().await {
        if let Ok((title, ok)) = res {
            done += 1;
            if ok {
                report.added += 1;
            }
            on_progress(total, done, &title);
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
/// The blocking file work (metadata parse, cover extract, 3D bake) runs on a
/// blocking worker so concurrent `add_one`s actually parallelise.
pub async fn add_one(pool: &SqlitePool, path: &Path) -> Result<i64> {
    let Some(format) = format_of(path) else { anyhow::bail!("unsupported file") };
    let path_str = path.display().to_string();
    let p = path.to_path_buf();
    let (meta, size, cover) = tokio::task::spawn_blocking(move || {
        let meta = metadata::extract(&p, format);
        let size = std::fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(0);
        let cover = covers::extract(&p, format)
            .map(|c| c.display().to_string())
            .unwrap_or_default();
        // Pre-bake the 3D renditions here (blocking worker) so the library
        // grid never has to bake during a page flip. Coverless books get a
        // generated title-card placeholder baked through the same pipeline.
        let flat = if cover.is_empty() {
            covers::placeholder_flat(&p, &meta.title, &meta.author, crate::cover_hue_rgb(&meta.title)).ok()
        } else {
            Some(PathBuf::from(&cover))
        };
        if let Some(cp) = flat {
            let _ = covers::bake_book(&cp);
            let _ = covers::bake_hero(&cp);
        }
        (meta, size, cover)
    })
    .await
    .map_err(|e| anyhow::anyhow!("join: {e}"))?;
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
