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

/// Rows per page in the history popups.
pub const PAGE_SIZE: i64 = 20;

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
/// `artists` is already ordered with the chosen main artist first, so
/// `album_artist` = `artists[0]` and the joined `artist` string reads
/// "Main, Second, …". Returns the created `items.id`.
pub async fn ingest_downloaded_track(
    pool: &SqlitePool,
    abs_path: &str,
    title: &str,
    artists: &[String],
    album: Option<&str>,
    duration_ms: Option<u64>,
) -> Result<i64> {
    let item_id = upsert_music_item(pool, abs_path).await?;
    let joined = artists.join(", ");
    let main = artists.first().cloned();
    let container = Path::new(abs_path)
        .extension()
        .and_then(|e| e.to_str())
        .and_then(container_from_ext)
        .map(|s| s.to_string());
    let tags = TrackTags {
        title: Some(title.to_string()),
        artist: if joined.is_empty() { None } else { Some(joined) },
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
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = page.clamp(0, pages - 1);
    let rows = sqlx::query_as::<_, DlHistoryRow>(
        "SELECT id, title, artists, album, provider, abs_path, downloaded_at
         FROM dl_history ORDER BY downloaded_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(PAGE_SIZE)
    .bind(page * PAGE_SIZE)
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
    let pages = ((total + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page = page.clamp(0, pages - 1);
    let rows = sqlx::query_as::<_, DlSearchRow>(
        "SELECT id, url, kind, title, provider, searched_at
         FROM dl_searches ORDER BY searched_at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(PAGE_SIZE)
    .bind(page * PAGE_SIZE)
    .fetch_all(pool)
    .await?;
    Ok((rows, pages))
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
        assert_eq!(artist, "Lead Singer, Producer");
        assert_eq!(album_artist.as_deref(), Some("Lead Singer"));
    }

    #[tokio::test]
    async fn history_paginates_newest_first() {
        let (_t, pool) = open_pool().await;
        for i in 0..25 {
            record_download(&pool, &format!("T{i}"), "A", None, Some("Spotify"), &format!("/m/{i}.opus"))
                .await
                .unwrap();
        }
        let (rows, pages) = history_page(&pool, 0).await.unwrap();
        assert_eq!(rows.len(), 20);
        assert_eq!(pages, 2);
        assert_eq!(rows[0].title, "T24"); // newest first
        let (rows2, _) = history_page(&pool, 1).await.unwrap();
        assert_eq!(rows2.len(), 5);
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
