//! Library scanner — walks the configured book folders, upserts new files, and
//! flags rows whose file vanished as `missing`.
//!
//! **The sweep only writes rows.** Cover art (extract or draw, then bake two
//! mockup renditions) and library-wide content indexing are both deferred to
//! background passes — [`build_art`] and [`index_contents`] — that the app
//! drains in batches afterwards. Adding a folder finishes at the speed of
//! parsing metadata, not at the speed of three image pipelines per book, and a
//! rescan of a large library is cheap for the same reason.

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
    // Book roots live in this table, not in `LibrariesConfig`, so the FS watcher
    // — which is built from the libraries list — never saw them. Deleting a book
    // on disk left its row and its tile behind until the next manual rescan.
    // Attaching here covers every way a root arrives: the Books "Add books"
    // picker, a Calibre import, and the Genesis download folder, which registers
    // itself through this same call.
    tulipix_core::watcher::watch_path(Path::new(path));
    Ok(())
}

/// Watch every configured book root on the live FS watcher.
///
/// [`add_folder`] attaches new roots as they arrive, but roots stored in a
/// previous session are only rows in a table — nothing re-attaches them at
/// startup. Call once after the watcher is installed.
pub async fn watch_folders(pool: &SqlitePool) {
    for root in folders(pool).await.unwrap_or_default() {
        tulipix_core::watcher::watch_path(Path::new(&root));
    }
}

/// Flag books whose file has vanished, without walking the roots.
///
/// The full [`scan_all`] ends with this same reconcile, but it costs a complete
/// directory walk plus metadata extraction; an FS delete event only needs the
/// vanished pass. One query plus a `stat` per non-missing row.
///
/// Returns how many rows were newly flagged, so a caller can skip the UI refresh
/// when nothing changed.
pub async fn reconcile_missing(pool: &SqlitePool) -> Result<usize> {
    let roots = folders(pool).await.unwrap_or_default();
    let all: Vec<(i64, String)> = sqlx::query_as("SELECT id, path FROM books WHERE missing = 0")
        .fetch_all(pool)
        .await?;
    let mut gone = 0usize;
    for (id, p) in all {
        // Only rows under a configured root — a one-off opened file outside the
        // roots keeps its row, same rule `scan_all` uses.
        if roots.iter().any(|r| is_under(&p, r)) && !Path::new(&p).exists() {
            sqlx::query("UPDATE books SET missing = 1 WHERE id = ?")
                .bind(id)
                .execute(pool)
                .await?;
            gone += 1;
        }
    }
    Ok(gone)
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
    // On the blocking pool: the walk has no await in it, and a large shelf
    // would otherwise hold an async worker every other section shares.
    let walk_roots = roots.clone();
    let all_files: Vec<PathBuf> = tokio::task::spawn_blocking(move || {
        let mut all_files = Vec::new();
        for root in &walk_roots {
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
        all_files
    })
    .await?;
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
        let mut q = sqlx::query(sqlx::AssertSqlSafe(&*sql));
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

/// How many books one [`build_art`] call may claim. Small on purpose: the
/// caller refreshes the grid between batches, so this is also how often covers
/// appear while a big import is draining.
pub const ART_BATCH: i64 = 6;

/// How many art pipelines run at once inside a batch.
///
/// Two, not four. Each one holds a decoded cover plus two mockup bitmaps in
/// memory while it warps, and this runs *alongside* whatever else the app is
/// doing — the point of moving it off the ingest path was to stop the machine
/// being saturated by adding books, so the builder does not get to saturate it
/// either.
const ART_JOBS: usize = 2;

/// Build the cover art for the next batch of books that have none yet.
///
/// Returns how many rows were processed; `0` means the queue is empty and the
/// caller can stop looping. Every claimed book is stamped done whatever the
/// outcome — a file with no extractable art gets a drawn title card, and one
/// that fails outright must not come back on the next pass forever.
///
/// Three images come out of this per book: the flat cover (extracted from the
/// file, or rendered from the title), and two bakes — the grid hardcover and
/// the flat hero. The UI reads them straight off disk and does not memoise
/// misses, so tiles pick each one up as soon as it lands.
pub async fn build_art(pool: &SqlitePool, limit: i64) -> Result<usize> {
    let batch = crate::library::needs_art(pool, limit).await?;
    if batch.is_empty() {
        return Ok(0);
    }
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(ART_JOBS));
    let mut set = tokio::task::JoinSet::new();
    for (id, path, format, title, author) in batch {
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let cover = tokio::task::spawn_blocking(move || art_for(&path, &format, &title, &author))
                .await
                .unwrap_or_default();
            (id, cover)
        });
    }
    let mut done = 0usize;
    while let Some(res) = set.join_next().await {
        if let Ok((id, cover)) = res {
            crate::library::set_art(pool, id, &cover).await?;
            done += 1;
        }
    }
    Ok(done)
}

/// The blocking half of [`build_art`] for one book: extract or draw the flat
/// cover, then bake both renditions from it. Returns the flat cover path, or
/// an empty string when the book had no art of its own (the placeholder was
/// still drawn and baked — it just is not something to point `cover_path` at,
/// since the UI derives the placeholder path from the book path).
fn art_for(path: &str, format: &str, title: &str, author: &str) -> String {
    let p = Path::new(path);
    let cover = covers::extract(p, format).map(|c| c.display().to_string()).unwrap_or_default();
    let flat = if cover.is_empty() {
        covers::placeholder_flat(p, title, author, crate::cover_hue_rgb(title)).ok()
    } else {
        Some(PathBuf::from(&cover))
    };
    if let Some(cp) = flat {
        let _ = covers::bake_book(&cp);
        let _ = covers::bake_hero(&cp);
    }
    cover
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
///
/// Metadata only. Cover extraction and the three image renditions that follow
/// it are **not** done here — they are left to [`build_art`], which a caller
/// drains in the background. Ingest used to run at the speed of that pipeline:
/// per book it decoded the embedded art (or rendered a title card with a text
/// rasteriser), then warped it onto two separate mockups with their lighting
/// multiplied back over it. That is minutes of work for a folder of 500 books,
/// all of it in front of the row appearing in the grid, and none of it needed
/// until a tile is actually on screen.
///
/// The parse still runs on a blocking worker so concurrent `add_one`s
/// parallelise.
pub async fn add_one(pool: &SqlitePool, path: &Path) -> Result<i64> {
    let Some(format) = format_of(path) else { anyhow::bail!("unsupported file") };
    let path_str = path.display().to_string();
    let p = path.to_path_buf();
    let (meta, size) = tokio::task::spawn_blocking(move || {
        let meta = metadata::extract(&p, format);
        let size = std::fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(0);
        (meta, size)
    })
    .await
    .map_err(|e| anyhow::anyhow!("join: {e}"))?;
    // Empty until the art builder fills it in; `art_state` defaults to 0, which
    // is what puts this row in that queue.
    let cover = String::new();
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
/// stale cached cover art, re-extract metadata, and update the row.
/// User-owned fields (rating, favorite, collections, progress) are untouched.
///
/// Art is re-queued rather than rebuilt here, for the same reason [`add_one`]
/// defers it: a rescan that finds fifty touched files should not sit through a
/// hundred bakes before the grid updates. Clearing `cover_path` and resetting
/// `art_state` is what puts the book back in [`build_art`]'s queue.
async fn refresh_one(pool: &SqlitePool, path: &Path) -> Result<()> {
    let Some(format) = format_of(path) else { anyhow::bail!("unsupported file") };
    let path_str = path.display().to_string();
    let p = path.to_path_buf();
    let (meta, size, mtime) = tokio::task::spawn_blocking(move || {
        // The cover cache is keyed by book path, which hasn't changed — so the
        // stale art has to be evicted or `extract` would just serve it back.
        covers::purge_cached(&p);
        let meta = metadata::extract(&p, format);
        let (size, mtime) = file_stamp(&p);
        (meta, size, mtime)
    })
    .await
    .map_err(|e| anyhow::anyhow!("join: {e}"))?;
    // `rtl` is deliberately NOT overwritten here: the user may have toggled it
    // by hand, and a re-index shouldn't undo that. It's set once, on insert.
    sqlx::query(
        "UPDATE books SET format = ?, title = ?, author = ?, genre = ?, series = ?,
                          published = ?, size_bytes = ?, file_mtime = ?,
                          cover_path = '', art_state = 0, missing = 0
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
