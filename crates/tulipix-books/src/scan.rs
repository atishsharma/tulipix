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

/// Is `path` inside `root`? Plain `starts_with` on the strings would put
/// `/books2/x.epub` under root `/books`, so the boundary has to land on a
/// separator (or be an exact match).
fn is_under(path: &str, root: &str) -> bool {
    let root = root.trim_end_matches(std::path::MAIN_SEPARATOR);
    path == root
        || (path.starts_with(root)
            && path[root.len()..].starts_with(std::path::MAIN_SEPARATOR))
}

/// `(size, mtime_secs)` identity of a file — the change signal for rescans.
/// `(0, 0)` when the file can't be stat'd.
fn file_stamp(path: &Path) -> (i64, i64) {
    let Ok(m) = std::fs::metadata(path) else { return (0, 0) };
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (m.len() as i64, mtime)
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
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();

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

    // Known files: path -> (size, mtime) as last indexed. A file whose size or
    // mtime moved gets its metadata + cover re-extracted; the rest are just
    // un-flagged as present, in batched UPDATEs rather than one per file.
    let known: std::collections::HashMap<String, (i64, i64)> =
        sqlx::query_as::<_, (String, i64, i64)>("SELECT path, size_bytes, file_mtime FROM books")
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(|(p, s, m)| (p, (s, m)))
            .collect();
    let mut done = 0u32;
    let mut fresh: Vec<PathBuf> = Vec::new();
    let mut stale: Vec<PathBuf> = Vec::new();
    let mut present: Vec<String> = Vec::new();
    let mut backfill: Vec<(String, i64, i64)> = Vec::new();
    for f in &all_files {
        let path_str = f.display().to_string();
        match known.get(&path_str) {
            Some(&(size, mtime)) => {
                let stamp = file_stamp(f);
                if mtime == 0 {
                    // Row predates the file_mtime column: adopt the file's
                    // current stamp instead of reading it as "changed", or the
                    // first scan after the upgrade would re-extract the entire
                    // library. Runs once per pre-existing book, ever.
                    backfill.push((path_str.clone(), stamp.0, stamp.1));
                } else if stamp != (size, mtime) {
                    stale.push(f.clone());
                } else {
                    done += 1;
                    on_progress(total, done, f.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
                }
                present.push(path_str.clone());
            }
            None => fresh.push(f.clone()),
        }
        seen.insert(path_str);
    }
    for (path, size, mtime) in backfill {
        sqlx::query("UPDATE books SET size_bytes = ?, file_mtime = ? WHERE path = ?")
            .bind(size)
            .bind(mtime)
            .bind(&path)
            .execute(pool)
            .await?;
        done += 1;
        on_progress(total, done, "");
    }
    // Batched un-missing: one statement per 400 paths instead of per file.
    for chunk in present.chunks(400) {
        let holes = vec!["?"; chunk.len()].join(",");
        let sql = format!("UPDATE books SET missing = 0 WHERE missing = 1 AND path IN ({holes})");
        let mut q = sqlx::query(&sql);
        for p in chunk {
            q = q.bind(p);
        }
        q.execute(pool).await?;
    }

    // Changed-on-disk files: re-extract metadata + cover, sequentially (there
    // are usually a handful, and each one is already blocking-offloaded).
    for f in stale {
        done += 1;
        on_progress(total, done, f.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
        if refresh_one(pool, &f).await.is_ok() {
            report.updated += 1;
        }
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

    // Any scan root that is a Calibre library: overlay its curated catalogue
    // (series, tags, ratings, blurbs) onto the books just ingested from it.
    // Runs after ingest so every row exists to be matched by path.
    for root in &roots {
        let p = Path::new(root);
        if crate::calibre::is_library(p) {
            match crate::calibre::apply_metadata(pool, p).await {
                Ok(n) if n > 0 => tracing::info!("calibre: enriched {n} book(s) from {root}"),
                Err(e) => tracing::warn!("calibre: {root}: {e:#}"),
                _ => {}
            }
        }
    }

    // Flag rows whose file is gone (only those under a configured root —
    // one-off opened files outside the roots keep their row).
    let all: Vec<(i64, String)> = sqlx::query_as("SELECT id, path FROM books WHERE missing = 0")
        .fetch_all(pool)
        .await?;
    for (id, p) in all {
        let under_root = roots.iter().any(|r| is_under(&p, r));
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
    // Indexed-ness comes off the `books` row, never from the FTS table: its
    // `book_id` is UNINDEXED, so a NOT IN against it full-scans every stored
    // book body (measured at 2.6 s on a small library).
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT b.id, b.path, b.format FROM books b
         WHERE b.missing = 0
           AND b.fts_indexed = 0
           AND b.format IN ('epub','mobi','azw3','fb2')",
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

#[cfg(test)]
mod tests {
    use super::is_under;

    #[test]
    fn root_match_respects_path_boundaries() {
        assert!(is_under("/books/a.epub", "/books"));
        assert!(is_under("/books/sub/a.epub", "/books"));
        assert!(is_under("/books", "/books"));
        // Trailing slash on the configured root must not change the answer.
        assert!(is_under("/books/a.epub", "/books/"));
        // The bug this guards: a sibling folder sharing the root's prefix.
        assert!(!is_under("/books2/a.epub", "/books"));
        assert!(!is_under("/booksold/a.epub", "/books"));
        assert!(!is_under("/other/a.epub", "/books"));
    }
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
    let (_, mtime) = file_stamp(path);
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (path, format, title, author, genre, series, published,
                            size_bytes, file_mtime, cover_path, rtl, summary, added_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(path) DO UPDATE SET
            missing = 0, size_bytes = excluded.size_bytes, file_mtime = excluded.file_mtime
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
    .bind(mtime)
    .bind(&cover)
    .bind(i64::from(meta.rtl))
    .bind(&meta.summary)
    .bind(schema::now())
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Re-index a book whose file changed on disk (size/mtime moved): drop the
/// stale cached cover art, re-extract metadata + cover, and update the row.
/// User-owned fields (rating, favorite, collections, progress) are untouched.
async fn refresh_one(pool: &SqlitePool, path: &Path) -> Result<()> {
    let Some(format) = format_of(path) else { anyhow::bail!("unsupported file") };
    let path_str = path.display().to_string();
    let p = path.to_path_buf();
    let (meta, size, mtime, cover) = tokio::task::spawn_blocking(move || {
        // The cover cache is keyed by book path, which hasn't changed — so the
        // stale art has to be evicted or `extract` would just serve it back.
        covers::purge_cached(&p);
        let meta = metadata::extract(&p, format);
        let (size, mtime) = file_stamp(&p);
        let cover = covers::extract(&p, format)
            .map(|c| c.display().to_string())
            .unwrap_or_default();
        let flat = if cover.is_empty() {
            covers::placeholder_flat(&p, &meta.title, &meta.author, crate::cover_hue_rgb(&meta.title)).ok()
        } else {
            Some(PathBuf::from(&cover))
        };
        if let Some(cp) = flat {
            let _ = covers::bake_book(&cp);
            let _ = covers::bake_hero(&cp);
        }
        (meta, size, mtime, cover)
    })
    .await
    .map_err(|e| anyhow::anyhow!("join: {e}"))?;
    // `rtl` is deliberately NOT overwritten here: the user may have toggled it
    // by hand, and a re-index shouldn't undo that. It's set once, on insert.
    sqlx::query(
        "UPDATE books SET format = ?, title = ?, author = ?, genre = ?, series = ?,
                          published = ?, size_bytes = ?, file_mtime = ?, cover_path = ?,
                          missing = 0
         WHERE path = ?",
    )
    .bind(format)
    .bind(&meta.title)
    .bind(&meta.author)
    .bind(&meta.genre)
    .bind(&meta.series)
    .bind(&meta.published)
    .bind(size)
    .bind(mtime)
    .bind(&cover)
    .bind(&path_str)
    .execute(pool)
    .await?;
    // Contents changed → the old FTS body is wrong. Clearing the flag is what
    // re-queues the book; the DELETE only runs when there's really a row, since
    // scanning book_fts is expensive (see `fts_indexed` in the schema).
    let indexed: Option<i64> = sqlx::query_scalar("SELECT fts_indexed FROM books WHERE path = ?")
        .bind(&path_str)
        .fetch_optional(pool)
        .await
        .unwrap_or(None);
    if indexed.unwrap_or(0) != 0 {
        sqlx::query(
            "DELETE FROM book_fts
             WHERE CAST(book_id AS INTEGER) = (SELECT id FROM books WHERE path = ?)",
        )
        .bind(&path_str)
        .execute(pool)
        .await
        .ok();
        sqlx::query("UPDATE books SET fts_indexed = 0 WHERE path = ?")
            .bind(&path_str)
            .execute(pool)
            .await?;
    }
    Ok(())
}
