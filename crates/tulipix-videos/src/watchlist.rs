//! Watchlist — per-user queue of TMDB movies/shows ("want to watch later").
//!
//! Rows live in their own `watchlist` table so they survive even when the
//! underlying movie hasn't been ripped to disk yet. Local-only: `device_uuid`
//! is stamped from the local machine id and rows never leave this device.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaKind {
    Movie,
    Show,
}

impl MediaKind {
    pub fn as_str(self) -> &'static str {
        match self {
            MediaKind::Movie => "movie",
            MediaKind::Show => "show",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "movie" => Some(MediaKind::Movie),
            "show" => Some(MediaKind::Show),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistEntry {
    pub tmdb_id: i64,
    pub kind: MediaKind,
    pub title: String,
    pub year: Option<i64>,
    pub poster_path: Option<String>,
    pub added_at: i64,
    pub updated_at: i64,
    pub device_uuid: String,
    /// Tombstone for last-write-wins removal. Sync emits the row with
    /// `removed = 1` instead of physically deleting it so other devices can
    /// converge.
    pub removed: bool,
}

pub const WATCHLIST_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS watchlist (
    tmdb_id      INTEGER NOT NULL,
    kind         TEXT    NOT NULL,
    title        TEXT    NOT NULL,
    year         INTEGER,
    poster_path  TEXT,
    added_at     INTEGER NOT NULL,
    updated_at   INTEGER NOT NULL,
    device_uuid  TEXT    NOT NULL,
    removed      INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (tmdb_id, kind)
);
CREATE INDEX IF NOT EXISTS watchlist_updated_idx ON watchlist(updated_at DESC);
CREATE INDEX IF NOT EXISTS watchlist_removed_idx ON watchlist(removed, updated_at DESC);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(WATCHLIST_SCHEMA).execute(pool).await?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn add(pool: &SqlitePool, entry: &WatchlistEntry) -> Result<()> {
    sqlx::query(
        "INSERT INTO watchlist
         (tmdb_id, kind, title, year, poster_path, added_at, updated_at, device_uuid, removed)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0)
         ON CONFLICT(tmdb_id, kind) DO UPDATE SET
            title = excluded.title,
            year = excluded.year,
            poster_path = excluded.poster_path,
            updated_at = excluded.updated_at,
            device_uuid = excluded.device_uuid,
            removed = 0",
    )
    .bind(entry.tmdb_id)
    .bind(entry.kind.as_str())
    .bind(&entry.title)
    .bind(entry.year)
    .bind(&entry.poster_path)
    .bind(entry.added_at)
    .bind(entry.updated_at)
    .bind(&entry.device_uuid)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, tmdb_id: i64, kind: MediaKind, device_uuid: &str) -> Result<()> {
    sqlx::query(
        "UPDATE watchlist SET removed = 1, updated_at = ?, device_uuid = ?
         WHERE tmdb_id = ? AND kind = ?",
    )
    .bind(now())
    .bind(device_uuid)
    .bind(tmdb_id)
    .bind(kind.as_str())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_active(pool: &SqlitePool) -> Result<Vec<WatchlistEntry>> {
    let rows = sqlx::query_as::<_, (i64, String, String, Option<i64>, Option<String>, i64, i64, String, i64)>(
        "SELECT tmdb_id, kind, title, year, poster_path, added_at, updated_at, device_uuid, removed
         FROM watchlist WHERE removed = 0 ORDER BY added_at DESC",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| {
            Some(WatchlistEntry {
                tmdb_id: r.0,
                kind: MediaKind::parse(&r.1)?,
                title: r.2,
                year: r.3,
                poster_path: r.4,
                added_at: r.5,
                updated_at: r.6,
                device_uuid: r.7,
                removed: r.8 != 0,
            })
        })
        .collect())
}

/// LWW merge: keep whichever side has the newer `updated_at`. Equal timestamps
/// resolve to the existing local row so a no-op sync run is idempotent.
pub async fn merge_remote(pool: &SqlitePool, remote: &WatchlistEntry) -> Result<bool> {
    let local: Option<(i64,)> = sqlx::query_as(
        "SELECT updated_at FROM watchlist WHERE tmdb_id = ? AND kind = ?",
    )
    .bind(remote.tmdb_id)
    .bind(remote.kind.as_str())
    .fetch_optional(pool)
    .await?;
    let should_apply = match local {
        Some((ts,)) => remote.updated_at > ts,
        None => true,
    };
    if !should_apply {
        return Ok(false);
    }
    sqlx::query(
        "INSERT INTO watchlist
         (tmdb_id, kind, title, year, poster_path, added_at, updated_at, device_uuid, removed)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(tmdb_id, kind) DO UPDATE SET
            title = excluded.title, year = excluded.year, poster_path = excluded.poster_path,
            updated_at = excluded.updated_at, device_uuid = excluded.device_uuid,
            removed = excluded.removed",
    )
    .bind(remote.tmdb_id)
    .bind(remote.kind.as_str())
    .bind(&remote.title)
    .bind(remote.year)
    .bind(&remote.poster_path)
    .bind(remote.added_at)
    .bind(remote.updated_at)
    .bind(&remote.device_uuid)
    .bind(i64::from(remote.removed))
    .execute(pool)
    .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn sample(id: i64, title: &str) -> WatchlistEntry {
        WatchlistEntry {
            tmdb_id: id,
            kind: MediaKind::Movie,
            title: title.into(),
            year: Some(2024),
            poster_path: Some(format!("/p{id}.jpg")),
            added_at: 100,
            updated_at: 100,
            device_uuid: "dev-a".into(),
            removed: false,
        }
    }

    #[tokio::test]
    async fn add_then_list_active() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        add(&pool, &sample(1, "A")).await.unwrap();
        add(&pool, &sample(2, "B")).await.unwrap();
        let active = list_active(&pool).await.unwrap();
        assert_eq!(active.len(), 2);
    }

    #[tokio::test]
    async fn remove_tombstones_not_deletes() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        add(&pool, &sample(1, "A")).await.unwrap();
        remove(&pool, 1, MediaKind::Movie, "dev-a").await.unwrap();
        let active = list_active(&pool).await.unwrap();
        assert!(active.is_empty());
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM watchlist")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn merge_newer_wins() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        add(&pool, &sample(1, "Local")).await.unwrap();
        let mut remote = sample(1, "Remote");
        remote.updated_at = 200;
        remote.device_uuid = "dev-b".into();
        let applied = merge_remote(&pool, &remote).await.unwrap();
        assert!(applied);
        let active = list_active(&pool).await.unwrap();
        assert_eq!(active[0].title, "Remote");
    }

    #[tokio::test]
    async fn merge_older_is_skipped() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let mut local = sample(1, "Local");
        local.updated_at = 500;
        add(&pool, &local).await.unwrap();
        let mut remote = sample(1, "Stale");
        remote.updated_at = 100;
        let applied = merge_remote(&pool, &remote).await.unwrap();
        assert!(!applied);
        let active = list_active(&pool).await.unwrap();
        assert_eq!(active[0].title, "Local");
    }

    #[tokio::test]
    async fn merge_remote_tombstone_hides_row() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        add(&pool, &sample(1, "A")).await.unwrap();
        let mut remote = sample(1, "A");
        remote.updated_at = 500;
        remote.removed = true;
        merge_remote(&pool, &remote).await.unwrap();
        let active = list_active(&pool).await.unwrap();
        assert!(active.is_empty());
    }

    #[test]
    fn media_kind_round_trip() {
        assert_eq!(MediaKind::parse("movie"), Some(MediaKind::Movie));
        assert_eq!(MediaKind::parse("show"), Some(MediaKind::Show));
        assert_eq!(MediaKind::parse("other"), None);
    }
}
