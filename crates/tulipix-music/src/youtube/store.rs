//! `youtube.db` schema + queries. Self-contained section DB (no `items` FK).

use anyhow::Result;
use sqlx::SqlitePool;

pub const YOUTUBE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS yt_subs (
    channel_id  TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    avatar_path TEXT,
    video_count INTEGER,
    fetched_at  INTEGER
);

CREATE TABLE IF NOT EXISTS yt_cached (
    video_id   TEXT PRIMARY KEY,
    title      TEXT,
    channel    TEXT,
    thumb_path TEXT,
    media_path TEXT,
    duration   INTEGER,
    cached_at  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS yt_downloaded (
    video_id      TEXT PRIMARY KEY,
    title         TEXT,
    channel       TEXT,
    thumb_path    TEXT,
    media_path    TEXT,
    duration      INTEGER,
    kind          TEXT NOT NULL DEFAULT 'audio',   -- audio | video
    downloaded_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS yt_recent_searches (
    query       TEXT PRIMARY KEY,
    searched_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS yt_playlists (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT NOT NULL,
    created_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS yt_playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES yt_playlists(id) ON DELETE CASCADE,
    video_id    TEXT NOT NULL,
    title       TEXT,
    channel     TEXT,
    thumb_path  TEXT,
    duration    INTEGER,
    position    INTEGER NOT NULL,
    added_at    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, video_id)
);
CREATE INDEX IF NOT EXISTS yt_playlist_items_pl_idx ON yt_playlist_items(playlist_id, position);
"#;

/// Apply the youtube schema to a (youtube.db) pool. Idempotent.
pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(YOUTUBE_SCHEMA).execute(pool).await?;
    Ok(())
}

use crate::youtube::subscriptions::ImportedSub;

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Sub {
    pub channel_id: String,
    pub title: String,
    pub avatar_path: Option<String>,
    pub video_count: Option<i64>,
    pub fetched_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CachedVideo {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub thumb_path: String,
    pub media_path: String,
    pub duration: i64,
    pub at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistRow {
    pub id: i64,
    pub name: String,
    pub count: i64,
    pub cover: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistItem {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub thumb_path: String,
    pub duration: i64,
    pub position: i64,
}

/// Upsert imported subs (title refreshed; avatar/count preserved). Returns total subs.
pub async fn import_subs(pool: &SqlitePool, subs: &[ImportedSub]) -> Result<i64> {
    for s in subs {
        sqlx::query(
            "INSERT INTO yt_subs (channel_id, title) VALUES (?, ?)
             ON CONFLICT(channel_id) DO UPDATE SET title = excluded.title",
        )
        .bind(&s.channel_id)
        .bind(&s.title)
        .execute(pool)
        .await?;
    }
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM yt_subs").fetch_one(pool).await?)
}

pub async fn list_subs(pool: &SqlitePool) -> Result<Vec<Sub>> {
    let rows: Vec<(String, String, Option<String>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT channel_id, title, avatar_path, video_count, fetched_at FROM yt_subs ORDER BY title COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(channel_id, title, avatar_path, video_count, fetched_at)| Sub {
            channel_id,
            title,
            avatar_path,
            video_count,
            fetched_at,
        })
        .collect())
}

pub async fn set_sub_meta(
    pool: &SqlitePool,
    channel_id: &str,
    avatar: Option<&str>,
    count: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE yt_subs SET avatar_path = COALESCE(?, avatar_path), video_count = COALESCE(?, video_count), fetched_at = ? WHERE channel_id = ?",
    )
    .bind(avatar)
    .bind(count)
    .bind(now())
    .bind(channel_id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn unsubscribe(pool: &SqlitePool, channel_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM yt_subs WHERE channel_id = ?")
        .bind(channel_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn push_recent_search(pool: &SqlitePool, query: &str) -> Result<()> {
    sqlx::query(
        "INSERT INTO yt_recent_searches (query, searched_at) VALUES (?, ?)
         ON CONFLICT(query) DO UPDATE SET searched_at = excluded.searched_at",
    )
    .bind(query)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_recent_searches(pool: &SqlitePool, limit: i64) -> Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT query FROM yt_recent_searches ORDER BY searched_at DESC, rowid DESC LIMIT ?")
            .bind(limit)
            .fetch_all(pool)
            .await?,
    )
}

pub async fn record_cached(
    pool: &SqlitePool,
    id: &str,
    title: &str,
    channel: &str,
    thumb: &str,
    media: &str,
    duration: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO yt_cached (video_id,title,channel,thumb_path,media_path,duration,cached_at) VALUES (?,?,?,?,?,?,?)
         ON CONFLICT(video_id) DO UPDATE SET cached_at = excluded.cached_at, media_path = excluded.media_path",
    )
    .bind(id)
    .bind(title)
    .bind(channel)
    .bind(thumb)
    .bind(media)
    .bind(duration)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_cached(pool: &SqlitePool, limit: i64) -> Result<Vec<CachedVideo>> {
    let rows: Vec<(String, String, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,media_path,duration,cached_at FROM yt_cached ORDER BY cached_at DESC, rowid DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(video_id, title, channel, thumb_path, media_path, duration, at)| CachedVideo {
            video_id,
            title,
            channel,
            thumb_path,
            media_path,
            duration,
            at,
        })
        .collect())
}

/// Evict oldest cached rows beyond `keep` newest. Returns (video_id, media_path) removed.
pub async fn evict_cached_over(pool: &SqlitePool, keep: i64) -> Result<Vec<(String, String)>> {
    let victims: Vec<(String, String)> = sqlx::query_as(
        "SELECT video_id, media_path FROM yt_cached ORDER BY cached_at DESC, rowid DESC LIMIT -1 OFFSET ?",
    )
    .bind(keep)
    .fetch_all(pool)
    .await?;
    for (id, _) in &victims {
        sqlx::query("DELETE FROM yt_cached WHERE video_id = ?")
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(victims)
}

pub async fn clear_cached(pool: &SqlitePool) -> Result<Vec<String>> {
    let paths: Vec<String> = sqlx::query_scalar("SELECT media_path FROM yt_cached")
        .fetch_all(pool)
        .await?;
    sqlx::query("DELETE FROM yt_cached").execute(pool).await?;
    Ok(paths)
}

pub async fn record_download(
    pool: &SqlitePool,
    id: &str,
    title: &str,
    channel: &str,
    thumb: &str,
    media: &str,
    duration: i64,
    kind: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO yt_downloaded (video_id,title,channel,thumb_path,media_path,duration,kind,downloaded_at) VALUES (?,?,?,?,?,?,?,?)
         ON CONFLICT(video_id) DO UPDATE SET media_path = excluded.media_path, downloaded_at = excluded.downloaded_at, kind = excluded.kind",
    )
    .bind(id)
    .bind(title)
    .bind(channel)
    .bind(thumb)
    .bind(media)
    .bind(duration)
    .bind(kind)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_downloads(pool: &SqlitePool, limit: i64) -> Result<Vec<CachedVideo>> {
    let rows: Vec<(String, String, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,media_path,duration,downloaded_at FROM yt_downloaded ORDER BY downloaded_at DESC, rowid DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(video_id, title, channel, thumb_path, media_path, duration, at)| CachedVideo {
            video_id,
            title,
            channel,
            thumb_path,
            media_path,
            duration,
            at,
        })
        .collect())
}

pub async fn remove_download(pool: &SqlitePool, id: &str) -> Result<Option<String>> {
    let path: Option<String> = sqlx::query_scalar("SELECT media_path FROM yt_downloaded WHERE video_id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    sqlx::query("DELETE FROM yt_downloaded WHERE video_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(path)
}

pub async fn create_playlist(pool: &SqlitePool, name: &str) -> Result<i64> {
    let r = sqlx::query("INSERT INTO yt_playlists (name, created_at) VALUES (?, ?)")
        .bind(name)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(r.last_insert_rowid())
}

pub async fn list_playlists(pool: &SqlitePool) -> Result<Vec<PlaylistRow>> {
    let rows: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, name FROM yt_playlists ORDER BY created_at DESC")
            .fetch_all(pool)
            .await?;
    let mut out = Vec::new();
    for (id, name) in rows {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM yt_playlist_items WHERE playlist_id = ?")
            .bind(id)
            .fetch_one(pool)
            .await?;
        let cover: Option<String> = sqlx::query_scalar(
            "SELECT thumb_path FROM yt_playlist_items WHERE playlist_id = ? ORDER BY position LIMIT 1",
        )
        .bind(id)
        .fetch_optional(pool)
        .await?
        .flatten();
        out.push(PlaylistRow { id, name, count, cover });
    }
    Ok(out)
}

pub async fn add_to_playlist(
    pool: &SqlitePool,
    playlist_id: i64,
    id: &str,
    title: &str,
    channel: &str,
    thumb: &str,
    duration: i64,
) -> Result<()> {
    let pos: i64 = sqlx::query_scalar(
        "SELECT COALESCE(MAX(position)+1, 0) FROM yt_playlist_items WHERE playlist_id = ?",
    )
    .bind(playlist_id)
    .fetch_one(pool)
    .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO yt_playlist_items (playlist_id,video_id,title,channel,thumb_path,duration,position,added_at) VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(playlist_id)
    .bind(id)
    .bind(title)
    .bind(channel)
    .bind(thumb)
    .bind(duration)
    .bind(pos)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn playlist_items(pool: &SqlitePool, playlist_id: i64) -> Result<Vec<PlaylistItem>> {
    let rows: Vec<(String, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,duration,position FROM yt_playlist_items WHERE playlist_id = ? ORDER BY position",
    )
    .bind(playlist_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(video_id, title, channel, thumb_path, duration, position)| PlaylistItem {
            video_id,
            title,
            channel,
            thumb_path,
            duration,
            position,
        })
        .collect())
}

pub async fn remove_from_playlist(pool: &SqlitePool, playlist_id: i64, video_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM yt_playlist_items WHERE playlist_id = ? AND video_id = ?")
        .bind(playlist_id)
        .bind(video_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn delete_playlist(pool: &SqlitePool, playlist_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM yt_playlists WHERE id = ?")
        .bind(playlist_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn rename_playlist(pool: &SqlitePool, playlist_id: i64, name: &str) -> Result<()> {
    sqlx::query("UPDATE yt_playlists SET name = ? WHERE id = ?")
        .bind(name)
        .bind(playlist_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;
    use crate::youtube::subscriptions::ImportedSub;

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        apply_schema(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM yt_subs")
            .fetch_one(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn imports_and_lists_subs() {
        let (_t, pool) = open_pool().await;
        let n = import_subs(
            &pool,
            &[
                ImportedSub { channel_id: "UC1".into(), title: "A".into() },
                ImportedSub { channel_id: "UC2".into(), title: "B".into() },
                ImportedSub { channel_id: "UC1".into(), title: "A again".into() },
            ],
        )
        .await
        .unwrap();
        assert_eq!(n, 2);
        let subs = list_subs(&pool).await.unwrap();
        assert_eq!(subs.len(), 2);
        set_sub_meta(&pool, "UC1", Some("/a/av.png"), Some(142)).await.unwrap();
        let subs = list_subs(&pool).await.unwrap();
        let one = subs.iter().find(|s| s.channel_id == "UC1").unwrap();
        assert_eq!(one.video_count, Some(142));
        unsubscribe(&pool, "UC2").await.unwrap();
        assert_eq!(list_subs(&pool).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn recent_searches_capped() {
        let (_t, pool) = open_pool().await;
        for i in 0..30 {
            push_recent_search(&pool, &format!("q{i}")).await.unwrap();
        }
        let recents = list_recent_searches(&pool, 6).await.unwrap();
        assert_eq!(recents.len(), 6);
        assert_eq!(recents[0], "q29");
    }

    #[tokio::test]
    async fn cache_records_and_evicts() {
        let (_t, pool) = open_pool().await;
        record_cached(&pool, "v1", "T1", "C", "/t/1.png", "/m/1.opus", 100).await.unwrap();
        record_cached(&pool, "v2", "T2", "C", "/t/2.png", "/m/2.opus", 200).await.unwrap();
        assert_eq!(list_cached(&pool, 10).await.unwrap().len(), 2);
        let removed = evict_cached_over(&pool, 1).await.unwrap();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].0, "v1");
    }

    #[tokio::test]
    async fn downloads_add_and_remove() {
        let (_t, pool) = open_pool().await;
        record_download(&pool, "v1", "T", "C", "/t.png", "/m.opus", 120, "audio").await.unwrap();
        assert_eq!(list_downloads(&pool, 10).await.unwrap().len(), 1);
        let path = remove_download(&pool, "v1").await.unwrap();
        assert_eq!(path.as_deref(), Some("/m.opus"));
        assert!(list_downloads(&pool, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn playlists_crud_and_add() {
        let (_t, pool) = open_pool().await;
        let pid = create_playlist(&pool, "Chill").await.unwrap();
        add_to_playlist(&pool, pid, "v1", "T1", "C", "/t.png", 100).await.unwrap();
        add_to_playlist(&pool, pid, "v2", "T2", "C", "/t.png", 100).await.unwrap();
        add_to_playlist(&pool, pid, "v1", "T1", "C", "/t.png", 100).await.unwrap();
        let pls = list_playlists(&pool).await.unwrap();
        assert_eq!(pls.len(), 1);
        assert_eq!(pls[0].count, 2);
        let items = playlist_items(&pool, pid).await.unwrap();
        assert_eq!(items.len(), 2);
        remove_from_playlist(&pool, pid, "v1").await.unwrap();
        assert_eq!(playlist_items(&pool, pid).await.unwrap().len(), 1);
        delete_playlist(&pool, pid).await.unwrap();
        assert!(list_playlists(&pool).await.unwrap().is_empty());
    }
}
