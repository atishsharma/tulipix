//! `np.p4.music.downloader` — persistence for the Music Downloader (mdl):
//! direct ingest of a finished download into the library, plus the Download-
//! History and Search-History tables backing the two downloader popups.
//!
//! Direct ingest is the fix for "downloaded tracks don't show up": instead of
//! re-scanning the folder by filename and re-probing tags, we write the `items`
//! + `track_meta` rows straight from the provider metadata we already hold.
//! Per-track thumbnails then come for free — the cover is embedded in the file
//! by the downloader, and the tile thumb path extracts it.

use anyhow::Result;
use sqlx::SqlitePool;
use std::path::Path;

use crate::scan;
use crate::tags::{container_from_ext, TrackTags};

/// Rows per page in the Download-History popup.
pub const HISTORY_PAGE_SIZE: i64 = 15;
/// Rows per page in the Search-History popup.
pub const SEARCH_PAGE_SIZE: i64 = 10;

/// True if a track with this `title` credited to `artist` already lives in the
/// music library (present file, not tombstoned). Drives the downloader's
/// "In Library" skip so a track already owned isn't fetched again.
pub async fn track_in_library(pool: &SqlitePool, title: &str, artist: &str) -> bool {
    let n: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM track_meta tm
         JOIN artists a ON a.id = tm.artist_id
         JOIN items it ON it.id = tm.item_id
         WHERE tm.title = ? COLLATE NOCASE AND a.name = ? COLLATE NOCASE
           AND it.section = 'music' AND it.missing_since IS NULL",
    )
    .bind(title)
    .bind(artist)
    .fetch_one(pool)
    .await
    .unwrap_or(0);
    n > 0
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Insert (or refresh) the `items` row for `abs_path`, returning its id. Fills
/// real size/mtime so the file-watcher treats it like any scanned file.
pub async fn upsert_music_item(pool: &SqlitePool, abs_path: &str) -> Result<i64> {
    let (size, mtime) = std::fs::metadata(abs_path)
        .map(|m| {
            let mt = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            (m.len() as i64, mt)
        })
        .unwrap_or((0, 0));
    let now = now();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated)
         VALUES (?, 0, ?, ?, 'music', ?, ?)
         ON CONFLICT(abs_path) DO UPDATE SET
            size = excluded.size, mtime = excluded.mtime,
            missing_since = NULL, updated = excluded.updated
         RETURNING id",
    )
    .bind(abs_path)
    .bind(size)
    .bind(mtime)
    .bind(now)
    .bind(now)
    .fetch_one(pool)
    .await?;
    Ok(id)
}

/// Ingest a just-downloaded track into the library from provider metadata.
///
/// `artists` is already ordered with the chosen main artist first. The library
/// (DB) is credited to the **main artist only** — `artist` = `album_artist` =
/// `artists[0]` — so the now-playing line and library grouping show just the
/// singer the user picked. The full credit list stays in the file's embedded
/// tags (written by the downloader), so nothing is lost. Returns the `items.id`.
pub async fn ingest_downloaded_track(
    pool: &SqlitePool,
    abs_path: &str,
    title: &str,
    artists: &[String],
    album: Option<&str>,
    duration_ms: Option<u64>,
) -> Result<i64> {
    let item_id = upsert_music_item(pool, abs_path).await?;
    let main = artists.first().cloned();
    let container = Path::new(abs_path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(container_from_ext)
        .map(|s| s.to_string());
    let tags = TrackTags {
        title: Some(title.to_string()),
        artist: main.clone(),
        album: album.map(|s| s.to_string()),
        album_artist: main,
        duration_s: duration_ms.map(|ms| ms as f64 / 1000.0),
        container,
        ..Default::default()
    };
    scan::upsert_track(pool, item_id, abs_path, &tags).await?;
    Ok(item_id)
}

// ---------------------------------------------------------------------------
// Download history
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DlHistoryRow {
    pub id: i64,
    pub title: String,
    pub artists: String,
    pub album: Option<String>,
    pub provider: Option<String>,
    pub abs_path: String,
    pub downloaded_at: i64,
}

/// Append one finished download to the history log.
pub async fn record_download(
    pool: &SqlitePool,
    title: &str,
    artists: &str,
    album: Option<&str>,
    provider: Option<&str>,
    abs_path: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO dl_history (title, artists, album, provider, abs_path, downloaded_at)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(title)
    .bind(artists)
    .bind(album)
    .bind(provider)
    .bind(abs_path)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// One page (20) of download history, newest first, plus the total page count.
pub async fn history_page(pool: &SqlitePool, page: i64) -> Result<(Vec<DlHistoryRow>, i64)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dl_history")
        .fetch_one(pool)
        .await?;
    let pages = ((total + HISTORY_PAGE_SIZE - 1) / HISTORY_PAGE_SIZE).max(1);
    let page = page.clamp(0, pages - 1);
    let rows = sqlx::query_as::<_, DlHistoryRow>(
        "SELECT id, title, artists, album, provider, abs_path, downloaded_at
         FROM dl_history ORDER BY downloaded_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(HISTORY_PAGE_SIZE)
    .bind(page * HISTORY_PAGE_SIZE)
    .fetch_all(pool)
    .await?;
    Ok((rows, pages))
}

// ---------------------------------------------------------------------------
// Search history
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct DlSearchRow {
    pub id: i64,
    pub url: String,
    pub kind: String,
    pub title: Option<String>,
    pub provider: Option<String>,
    pub searched_at: i64,
}

/// Record a resolved URL so it can be replayed into the URL field. De-dups the
/// most recent identical URL (moves it to the top instead of piling up).
pub async fn record_search(
    pool: &SqlitePool,
    url: &str,
    kind: &str,
    title: Option<&str>,
    provider: Option<&str>,
) -> Result<()> {
    sqlx::query("DELETE FROM dl_searches WHERE url = ?")
        .bind(url)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO dl_searches (url, kind, title, provider, searched_at)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(url)
    .bind(kind)
    .bind(title)
    .bind(provider)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// One page (20) of search history, newest first, plus the total page count.
pub async fn searches_page(pool: &SqlitePool, page: i64) -> Result<(Vec<DlSearchRow>, i64)> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM dl_searches")
        .fetch_one(pool)
        .await?;
    let pages = ((total + SEARCH_PAGE_SIZE - 1) / SEARCH_PAGE_SIZE).max(1);
    let page = page.clamp(0, pages - 1);
    let rows = sqlx::query_as::<_, DlSearchRow>(
        "SELECT id, url, kind, title, provider, searched_at
         FROM dl_searches ORDER BY searched_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(SEARCH_PAGE_SIZE)
    .bind(page * SEARCH_PAGE_SIZE)
    .fetch_all(pool)
    .await?;
    Ok((rows, pages))
}

/// Wipe the download-history log. Only clears the history *records* — the
/// downloaded audio files on disk (and their library entries) are untouched.
pub async fn clear_history(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM dl_history").execute(pool).await?;
    Ok(())
}

/// Wipe the search-history log (the replayable resolved-URL list).
pub async fn clear_searches(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM dl_searches").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn ingest_orders_main_artist_first() {
        let (_t, pool) = open_pool().await;
        let artists = vec!["Lead Singer".to_string(), "Producer".to_string()];
        let id = ingest_downloaded_track(&pool, "/m/song.opus", "Hit", &artists, Some("Album"), Some(210_000))
            .await
            .unwrap();
        let (artist, album_artist): (String, Option<String>) = sqlx::query_as(
            "SELECT a.name, tm.album_artist FROM track_meta tm
             JOIN artists a ON a.id = tm.artist_id WHERE tm.item_id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        // Library credits the chosen main artist only (others stay in file tags).
        assert_eq!(artist, "Lead Singer");
        assert_eq!(album_artist.as_deref(), Some("Lead Singer"));
    }

    #[tokio::test]
    async fn history_paginates_newest_first() {
        let (_t, pool) = open_pool().await;
        // Derived from HISTORY_PAGE_SIZE rather than hardcoded: this test asserted
        // 20 rows a page, which stopped being true when the constant arrived with
        // the v4 pagination work, and nothing ran it to say so.
        let n = HISTORY_PAGE_SIZE + 10; // one full page plus a partial one
        for i in 0..n {
            record_download(&pool, &format!("T{i}"), "A", None, Some("Spotify"), &format!("/m/{i}.opus"))
                .await
                .unwrap();
        }
        let (rows, pages) = history_page(&pool, 0).await.unwrap();
        assert_eq!(rows.len() as i64, HISTORY_PAGE_SIZE);
        assert_eq!(pages, 2);
        assert_eq!(rows[0].title, format!("T{}", n - 1)); // newest first
        let (rows2, _) = history_page(&pool, 1).await.unwrap();
        assert_eq!(rows2.len() as i64, n - HISTORY_PAGE_SIZE);
    }

    #[tokio::test]
    async fn search_dedups_same_url() {
        let (_t, pool) = open_pool().await;
        record_search(&pool, "https://x/pl/1", "playlist", Some("Mix"), Some("Spotify")).await.unwrap();
        record_search(&pool, "https://x/pl/1", "playlist", Some("Mix"), Some("Spotify")).await.unwrap();
        let (rows, _) = searches_page(&pool, 0).await.unwrap();
        assert_eq!(rows.len(), 1);
    }
}
