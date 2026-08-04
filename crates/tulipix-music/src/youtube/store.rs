//! `youtube.db` schema + queries. Self-contained section DB (no `items` FK).

use anyhow::Result;
use sqlx::SqlitePool;
use tulipix_core::util::unix_secs_i64 as now;

pub const YOUTUBE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS yt_subs (
    channel_id  TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    avatar_path TEXT,
    video_count INTEGER,
    sub_count   INTEGER,
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

-- One row per (video, quality): the same video downloaded at different
-- resolutions/formats keeps a distinct entry (audio, best, 1080p, 720p, …).
CREATE TABLE IF NOT EXISTS yt_downloaded (
    video_id      TEXT NOT NULL,
    title         TEXT,
    channel       TEXT,
    thumb_path    TEXT,
    media_path    TEXT,
    duration      INTEGER,
    kind          TEXT NOT NULL DEFAULT 'audio',   -- audio | video
    fmt           TEXT NOT NULL DEFAULT '',        -- container badge (MKV / OPUS)
    quality       TEXT NOT NULL DEFAULT '',        -- Audio | Best | 1080p | 720p …
    downloaded_at INTEGER NOT NULL,
    PRIMARY KEY (video_id, quality)
);

CREATE TABLE IF NOT EXISTS yt_recent_searches (
    query       TEXT PRIMARY KEY,
    searched_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS yt_playlists (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT NOT NULL,
    source_url  TEXT,         -- set for imported YouTube playlists (remote)
    video_count INTEGER,      -- total videos (remote, from import); NULL for local
    created_at  INTEGER NOT NULL
);

-- Lazily-fetched windows of a remote playlist's videos (mirrors yt_channel_cache).
CREATE TABLE IF NOT EXISTS yt_playlist_cache (
    playlist_id INTEGER NOT NULL,
    video_id    TEXT NOT NULL,
    title       TEXT,
    channel     TEXT,
    meta        TEXT,
    info        TEXT,
    thumb_path  TEXT,
    duration    INTEGER,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, video_id)
);
CREATE INDEX IF NOT EXISTS yt_playlist_cache_idx ON yt_playlist_cache(playlist_id, position);

CREATE TABLE IF NOT EXISTS yt_channel_cache (
    channel_id TEXT NOT NULL,
    video_id   TEXT NOT NULL,
    title      TEXT,
    channel    TEXT,
    meta       TEXT,
    info       TEXT,
    thumb_path TEXT,
    duration   INTEGER,
    position   INTEGER NOT NULL,
    PRIMARY KEY (channel_id, video_id)
);
CREATE INDEX IF NOT EXISTS yt_channel_cache_idx ON yt_channel_cache(channel_id, position);

-- Top "most-watched" videos for a channel — fetched + stored only when the user
-- taps the channel-page "Popular" button (mirrors yt_channel_cache).
CREATE TABLE IF NOT EXISTS yt_channel_popular (
    channel_id TEXT NOT NULL,
    video_id   TEXT NOT NULL,
    title      TEXT,
    channel    TEXT,
    meta       TEXT,
    info       TEXT,
    thumb_path TEXT,
    duration   INTEGER,
    position   INTEGER NOT NULL,
    PRIMARY KEY (channel_id, video_id)
);
CREATE INDEX IF NOT EXISTS yt_channel_popular_idx ON yt_channel_popular(channel_id, position);

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

-- Watch position per video, so a half-finished one resumes across restarts.
-- Deliberately NOT keyed to yt_cached: you can leave a video part-watched
-- without it ever being cached, and evicting the cache must not lose the mark.
CREATE TABLE IF NOT EXISTS yt_progress (
    video_id   TEXT PRIMARY KEY,
    position_s REAL    NOT NULL,
    duration_s REAL    NOT NULL DEFAULT 0,
    updated    INTEGER NOT NULL
);
"#;

/// Apply the youtube schema to a (youtube.db) pool. Idempotent.
pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(YOUTUBE_SCHEMA).execute(pool).await?;
    // Migrate pre-existing youtube.db files lacking sub_count. Ignore if present.
    let _ = sqlx::query("ALTER TABLE yt_subs ADD COLUMN sub_count INTEGER").execute(pool).await;
    // Soft-unsubscribe: unsubbed channels stay in the list (subscribed = 0).
    let _ = sqlx::query("ALTER TABLE yt_subs ADD COLUMN subscribed INTEGER NOT NULL DEFAULT 1").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE yt_playlists ADD COLUMN source_url TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE yt_playlists ADD COLUMN video_count INTEGER").execute(pool).await;
    // Per-download container + quality badges (e.g. "MKV" / "1080p", "OPUS" / "Audio").
    let _ = sqlx::query("ALTER TABLE yt_downloaded ADD COLUMN fmt TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE yt_downloaded ADD COLUMN quality TEXT").execute(pool).await;
    // v1: rebuild yt_downloaded with a composite (video_id, quality) primary key so
    // re-downloading a video at a different resolution no longer overwrites the
    // previous file — each resolution keeps its own row. Old single-PK tables (and
    // a freshly-created composite one) both pass through harmlessly once.
    let ver: i64 = sqlx::query_scalar("PRAGMA user_version").fetch_one(pool).await.unwrap_or(0);
    if ver < 1 {
        let _ = sqlx::raw_sql(
            r#"
            CREATE TABLE yt_downloaded_v1 (
                video_id      TEXT NOT NULL,
                title         TEXT,
                channel       TEXT,
                thumb_path    TEXT,
                media_path    TEXT,
                duration      INTEGER,
                kind          TEXT NOT NULL DEFAULT 'audio',
                fmt           TEXT NOT NULL DEFAULT '',
                quality       TEXT NOT NULL DEFAULT '',
                downloaded_at INTEGER NOT NULL,
                PRIMARY KEY (video_id, quality)
            );
            INSERT OR IGNORE INTO yt_downloaded_v1
                SELECT video_id, title, channel, thumb_path, media_path, duration, kind,
                       COALESCE(fmt,''), COALESCE(quality,''), downloaded_at
                FROM yt_downloaded;
            DROP TABLE yt_downloaded;
            ALTER TABLE yt_downloaded_v1 RENAME TO yt_downloaded;
            PRAGMA user_version = 1;
            "#,
        )
        .execute(pool)
        .await;
    }
    Ok(())
}

use crate::youtube::subscriptions::ImportedSub;

#[derive(Debug, Clone, PartialEq)]
pub struct Sub {
    pub channel_id: String,
    pub title: String,
    pub avatar_path: Option<String>,
    pub video_count: Option<i64>,
    pub sub_count: Option<i64>,
    pub fetched_at: Option<i64>,
    pub subscribed: bool,
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
    pub fmt: String,      // container badge, e.g. "MKV" / "OPUS" ("" for cached)
    pub quality: String,  // "1080p" / "Best" / "Audio" ("" for cached)
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlaylistRow {
    pub id: i64,
    pub name: String,
    pub count: i64,          // total videos (remote video_count, else item count)
    pub cover: Option<String>,
    pub source_url: Option<String>,
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
            "INSERT INTO yt_subs (channel_id, title, subscribed) VALUES (?, ?, 1)
             ON CONFLICT(channel_id) DO UPDATE SET title = excluded.title, subscribed = 1",
        )
        .bind(&s.channel_id)
        .bind(&s.title)
        .execute(pool)
        .await?;
    }
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM yt_subs").fetch_one(pool).await?)
}

pub async fn list_subs(pool: &SqlitePool) -> Result<Vec<Sub>> {
    let rows: Vec<(String, String, Option<String>, Option<i64>, Option<i64>, Option<i64>, i64)> = sqlx::query_as(
        "SELECT channel_id, title, avatar_path, video_count, sub_count, fetched_at, subscribed FROM yt_subs ORDER BY title COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(channel_id, title, avatar_path, video_count, sub_count, fetched_at, subscribed)| Sub {
            channel_id,
            title,
            avatar_path,
            video_count,
            sub_count,
            fetched_at,
            subscribed: subscribed != 0,
        })
        .collect())
}

pub async fn set_sub_meta(
    pool: &SqlitePool,
    channel_id: &str,
    avatar: Option<&str>,
    video_count: Option<i64>,
    sub_count: Option<i64>,
) -> Result<()> {
    sqlx::query(
        "UPDATE yt_subs SET avatar_path = COALESCE(?, avatar_path), video_count = COALESCE(?, video_count), sub_count = COALESCE(?, sub_count), fetched_at = ? WHERE channel_id = ?",
    )
    .bind(avatar)
    .bind(video_count)
    .bind(sub_count)
    .bind(now())
    .bind(channel_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Soft unsubscribe — keep the channel in the list (subscribed = 0) so it can be
/// re-subscribed later. Avatar / counts are preserved.
pub async fn unsubscribe(pool: &SqlitePool, channel_id: &str) -> Result<()> {
    sqlx::query("UPDATE yt_subs SET subscribed = 0 WHERE channel_id = ?")
        .bind(channel_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn clear_subs(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM yt_subs").execute(pool).await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelVid {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub meta: String,
    pub info: String,
    pub thumb_path: String,
    pub duration: i64,
}

pub async fn get_channel_cache(pool: &SqlitePool, channel_id: &str) -> Result<Vec<ChannelVid>> {
    let rows: Vec<(String, String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,meta,info,thumb_path,duration FROM yt_channel_cache WHERE channel_id = ? ORDER BY position",
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(video_id, title, channel, meta, info, thumb_path, duration)| ChannelVid {
        video_id, title, channel, meta, info, thumb_path, duration,
    }).collect())
}

/// Replace the cached latest-videos list for a channel.
pub async fn set_channel_cache(pool: &SqlitePool, channel_id: &str, vids: &[ChannelVid]) -> Result<()> {
    sqlx::query("DELETE FROM yt_channel_cache WHERE channel_id = ?").bind(channel_id).execute(pool).await?;
    for (i, v) in vids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO yt_channel_cache (channel_id,video_id,title,channel,meta,info,thumb_path,duration,position) VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(channel_id).bind(&v.video_id).bind(&v.title).bind(&v.channel)
        .bind(&v.meta).bind(&v.info).bind(&v.thumb_path).bind(v.duration).bind(i as i64)
        .execute(pool).await?;
    }
    Ok(())
}

pub async fn get_channel_popular(pool: &SqlitePool, channel_id: &str) -> Result<Vec<ChannelVid>> {
    let rows: Vec<(String, String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,meta,info,thumb_path,duration FROM yt_channel_popular WHERE channel_id = ? ORDER BY position",
    )
    .bind(channel_id)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(video_id, title, channel, meta, info, thumb_path, duration)| ChannelVid {
        video_id, title, channel, meta, info, thumb_path, duration,
    }).collect())
}

/// Replace the cached most-watched list for a channel.
pub async fn set_channel_popular(pool: &SqlitePool, channel_id: &str, vids: &[ChannelVid]) -> Result<()> {
    sqlx::query("DELETE FROM yt_channel_popular WHERE channel_id = ?").bind(channel_id).execute(pool).await?;
    for (i, v) in vids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO yt_channel_popular (channel_id,video_id,title,channel,meta,info,thumb_path,duration,position) VALUES (?,?,?,?,?,?,?,?,?)",
        )
        .bind(channel_id).bind(&v.video_id).bind(&v.title).bind(&v.channel)
        .bind(&v.meta).bind(&v.info).bind(&v.thumb_path).bind(v.duration).bind(i as i64)
        .execute(pool).await?;
    }
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

pub async fn clear_recent_searches(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM yt_recent_searches").execute(pool).await?;
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
            fmt: String::new(),
            quality: String::new(),
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

/// Remember how far into a video you got, so closing the app and coming back
/// resumes instead of restarting. Positions within the first or last few
/// seconds are not worth keeping — those are "started" and "finished", and
/// resuming either one is worse than not.
pub async fn save_progress(pool: &SqlitePool, id: &str, position_s: f64, duration_s: f64) -> Result<()> {
    let done = duration_s > 0.0 && position_s >= duration_s - 15.0;
    if position_s < 15.0 || done {
        sqlx::query("DELETE FROM yt_progress WHERE video_id = ?").bind(id).execute(pool).await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO yt_progress (video_id, position_s, duration_s, updated) VALUES (?,?,?,?)
         ON CONFLICT(video_id) DO UPDATE SET position_s = excluded.position_s,
             duration_s = excluded.duration_s, updated = excluded.updated",
    ).bind(id).bind(position_s).bind(duration_s).bind(now()).execute(pool).await?;
    Ok(())
}

/// Stored position for one video, or 0.
pub async fn progress_of(pool: &SqlitePool, id: &str) -> Result<f64> {
    Ok(sqlx::query_scalar("SELECT position_s FROM yt_progress WHERE video_id = ?")
        .bind(id).fetch_optional(pool).await?.unwrap_or(0.0))
}

/// Every stored position, as `video_id → fraction watched (0..1)`, for painting
/// the bar on cards. Rows with no known duration are dropped: a bar needs a
/// denominator.
pub async fn progress_map(pool: &SqlitePool) -> Result<Vec<(String, f32)>> {
    let rows: Vec<(String, f64, f64)> = sqlx::query_as(
        "SELECT video_id, position_s, duration_s FROM yt_progress WHERE duration_s > 0")
        .fetch_all(pool).await?;
    Ok(rows.into_iter()
        .map(|(id, pos, dur)| (id, (pos / dur).clamp(0.0, 1.0) as f32))
        .collect())
}

/// Evict oldest cached rows until the cache is at or below `cap_bytes` on
/// disk. Returns `(video_id, media_path)` removed.
///
/// The count rule above bounds how many files are kept, not how big they are —
/// sixty audio-only cache entries are a few hundred megabytes, but the same
/// sixty at 1080p are not. Sizes come from `stat`, not from a stored column:
/// nothing records a byte count at cache time, and a file the user deleted
/// underneath us must count as zero rather than as its remembered size.
pub async fn evict_cached_over_bytes(pool: &SqlitePool, cap_bytes: i64) -> Result<Vec<(String, String)>> {
    // Newest first — we walk forward spending the budget, and evict the tail.
    let rows: Vec<(String, String)> = sqlx::query_as(
        "SELECT video_id, media_path FROM yt_cached ORDER BY cached_at DESC, rowid DESC",
    ).fetch_all(pool).await?;
    let mut budget = cap_bytes;
    let mut victims = Vec::new();
    for (id, path) in rows {
        if budget < 0 {
            victims.push((id, path));
            continue;
        }
        let bytes = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
        budget -= bytes;
        if budget < 0 { victims.push((id, path)); }
    }
    for (id, _) in &victims {
        sqlx::query("DELETE FROM yt_cached WHERE video_id = ?").bind(id).execute(pool).await?;
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
    fmt: &str,
    quality: &str,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO yt_downloaded (video_id,title,channel,thumb_path,media_path,duration,kind,fmt,quality,downloaded_at) VALUES (?,?,?,?,?,?,?,?,?,?)
         ON CONFLICT(video_id,quality) DO UPDATE SET media_path = excluded.media_path, downloaded_at = excluded.downloaded_at, kind = excluded.kind, fmt = excluded.fmt, title = excluded.title, channel = excluded.channel, thumb_path = excluded.thumb_path",
    )
    .bind(id)
    .bind(title)
    .bind(channel)
    .bind(thumb)
    .bind(media)
    .bind(duration)
    .bind(kind)
    .bind(fmt)
    .bind(quality)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_downloads(pool: &SqlitePool, limit: i64) -> Result<Vec<CachedVideo>> {
    let rows: Vec<(String, String, String, String, String, i64, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,media_path,duration,downloaded_at,fmt,quality FROM yt_downloaded ORDER BY downloaded_at DESC, rowid DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(video_id, title, channel, thumb_path, media_path, duration, at, fmt, quality)| CachedVideo {
            video_id,
            title,
            channel,
            thumb_path,
            media_path,
            duration,
            at,
            fmt: fmt.unwrap_or_default(),
            quality: quality.unwrap_or_default(),
        })
        .collect())
}

pub async fn remove_cached(pool: &SqlitePool, id: &str) -> Result<Option<String>> {
    let path: Option<String> = sqlx::query_scalar("SELECT media_path FROM yt_cached WHERE video_id = ?")
        .bind(id).fetch_optional(pool).await?;
    sqlx::query("DELETE FROM yt_cached WHERE video_id = ?").bind(id).execute(pool).await?;
    Ok(path)
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

/// Remove a single download identified by its (unique) media file path — the way
/// the Downloads grid targets one specific resolution row without touching the
/// video's other downloaded resolutions.
pub async fn remove_download_by_path(pool: &SqlitePool, media_path: &str) -> Result<Option<String>> {
    sqlx::query("DELETE FROM yt_downloaded WHERE media_path = ?")
        .bind(media_path)
        .execute(pool)
        .await?;
    Ok(Some(media_path.to_string()))
}

/// Wipe every download row; returns the media paths so the caller can delete the
/// files on disk.
pub async fn clear_downloads(pool: &SqlitePool) -> Result<Vec<String>> {
    let paths: Vec<String> = sqlx::query_scalar("SELECT media_path FROM yt_downloaded")
        .fetch_all(pool)
        .await?;
    sqlx::query("DELETE FROM yt_downloaded").execute(pool).await?;
    Ok(paths)
}

pub async fn create_playlist(pool: &SqlitePool, name: &str) -> Result<i64> {
    let r = sqlx::query("INSERT INTO yt_playlists (name, created_at) VALUES (?, ?)")
        .bind(name)
        .bind(now())
        .execute(pool)
        .await?;
    Ok(r.last_insert_rowid())
}

pub async fn create_remote_playlist(pool: &SqlitePool, name: &str, source_url: &str, video_count: i64) -> Result<i64> {
    let r = sqlx::query("INSERT INTO yt_playlists (name, source_url, video_count, created_at) VALUES (?, ?, ?, ?)")
        .bind(name).bind(source_url).bind(video_count).bind(now()).execute(pool).await?;
    Ok(r.last_insert_rowid())
}

/// (name, source_url, video_count) for one playlist.
pub async fn get_playlist(pool: &SqlitePool, id: i64) -> Result<Option<(String, Option<String>, Option<i64>)>> {
    Ok(sqlx::query_as("SELECT name, source_url, video_count FROM yt_playlists WHERE id = ?")
        .bind(id).fetch_optional(pool).await?)
}

pub async fn list_playlists(pool: &SqlitePool) -> Result<Vec<PlaylistRow>> {
    let rows: Vec<(i64, String, Option<String>, Option<i64>)> =
        sqlx::query_as("SELECT id, name, source_url, video_count FROM yt_playlists ORDER BY created_at DESC")
            .fetch_all(pool)
            .await?;
    let mut out = Vec::new();
    for (id, name, source_url, video_count) in rows {
        let item_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM yt_playlist_items WHERE playlist_id = ?")
            .bind(id).fetch_one(pool).await?;
        let count = video_count.filter(|n| *n > 0).unwrap_or(item_count);
        // Cover: first local item thumb, else first cached thumb.
        let cover: Option<String> = sqlx::query_scalar(
            "SELECT thumb_path FROM yt_playlist_items WHERE playlist_id = ? ORDER BY position LIMIT 1")
            .bind(id).fetch_optional(pool).await?.flatten()
            .or(sqlx::query_scalar("SELECT thumb_path FROM yt_playlist_cache WHERE playlist_id = ? ORDER BY position LIMIT 1")
                .bind(id).fetch_optional(pool).await?.flatten());
        out.push(PlaylistRow { id, name, count, cover, source_url });
    }
    Ok(out)
}

/// Add bare video ids to a local playlist (file import — metadata filled lazily).
pub async fn add_playlist_ids(pool: &SqlitePool, playlist_id: i64, ids: &[String]) -> Result<()> {
    let mut pos: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(position)+1, 0) FROM yt_playlist_items WHERE playlist_id = ?")
        .bind(playlist_id).fetch_one(pool).await?;
    for id in ids {
        sqlx::query("INSERT OR IGNORE INTO yt_playlist_items (playlist_id,video_id,title,channel,thumb_path,duration,position,added_at) VALUES (?,?,?,?,?,?,?,?)")
            .bind(playlist_id).bind(id).bind(id).bind("").bind("").bind(0i64).bind(pos).bind(now())
            .execute(pool).await?;
        pos += 1;
    }
    Ok(())
}

pub async fn update_playlist_item_meta(pool: &SqlitePool, playlist_id: i64, video_id: &str, title: &str, channel: &str, thumb: &str, duration: i64) -> Result<()> {
    sqlx::query("UPDATE yt_playlist_items SET title=?, channel=?, thumb_path=?, duration=? WHERE playlist_id=? AND video_id=?")
        .bind(title).bind(channel).bind(thumb).bind(duration).bind(playlist_id).bind(video_id)
        .execute(pool).await?;
    Ok(())
}

pub async fn get_playlist_cache(pool: &SqlitePool, playlist_id: i64) -> Result<Vec<ChannelVid>> {
    let rows: Vec<(String, String, String, String, String, String, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,meta,info,thumb_path,duration FROM yt_playlist_cache WHERE playlist_id = ? ORDER BY position",
    ).bind(playlist_id).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(video_id, title, channel, meta, info, thumb_path, duration)| ChannelVid {
        video_id, title, channel, meta, info, thumb_path, duration,
    }).collect())
}

/// Store a fetched window of a remote playlist (positions start at `start`).
pub async fn set_playlist_cache_window(pool: &SqlitePool, playlist_id: i64, start: i64, vids: &[ChannelVid]) -> Result<()> {
    for (i, v) in vids.iter().enumerate() {
        sqlx::query(
            "INSERT INTO yt_playlist_cache (playlist_id,video_id,title,channel,meta,info,thumb_path,duration,position) VALUES (?,?,?,?,?,?,?,?,?)
             ON CONFLICT(playlist_id,video_id) DO UPDATE SET position=excluded.position, title=excluded.title, channel=excluded.channel, meta=excluded.meta, info=excluded.info, thumb_path=excluded.thumb_path, duration=excluded.duration",
        )
        .bind(playlist_id).bind(&v.video_id).bind(&v.title).bind(&v.channel).bind(&v.meta)
        .bind(&v.info).bind(&v.thumb_path).bind(v.duration).bind(start + i as i64)
        .execute(pool).await?;
    }
    Ok(())
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

pub async fn playlist_items_page(pool: &SqlitePool, playlist_id: i64, offset: i64, limit: i64) -> Result<Vec<PlaylistItem>> {
    let rows: Vec<(String, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,duration,position FROM yt_playlist_items WHERE playlist_id = ? ORDER BY position LIMIT ? OFFSET ?",
    )
    .bind(playlist_id).bind(limit).bind(offset)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|(video_id, title, channel, thumb_path, duration, position)| PlaylistItem {
        video_id, title, channel, thumb_path, duration, position,
    }).collect())
}

pub async fn playlist_item_count(pool: &SqlitePool, playlist_id: i64) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM yt_playlist_items WHERE playlist_id = ?")
        .bind(playlist_id).fetch_one(pool).await?)
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
    async fn progress_keeps_only_the_middle() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        // Barely started — not worth a mark.
        save_progress(&pool, "a", 4.0, 600.0).await.unwrap();
        assert_eq!(progress_of(&pool, "a").await.unwrap(), 0.0);
        // Genuinely mid-way.
        save_progress(&pool, "b", 300.0, 600.0).await.unwrap();
        assert_eq!(progress_of(&pool, "b").await.unwrap(), 300.0);
        // Reaching the end clears it, so it does not resume at the credits.
        save_progress(&pool, "b", 595.0, 600.0).await.unwrap();
        assert_eq!(progress_of(&pool, "b").await.unwrap(), 0.0);
        save_progress(&pool, "c", 150.0, 600.0).await.unwrap();
        assert_eq!(progress_map(&pool).await.unwrap(), vec![("c".to_string(), 0.25)]);
    }

    #[tokio::test]
    async fn byte_cap_evicts_when_files_are_large() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        // Three 1 KB files, newest last.
        for (i, id) in ["a", "b", "c"].iter().enumerate() {
            let f = dir.path().join(format!("{id}.opus"));
            std::fs::write(&f, vec![0u8; 1024]).unwrap();
            record_cached(&pool, id, "t", "c", "", f.to_str().unwrap(), 1).await.unwrap();
            // cached_at has one-second resolution — order them explicitly.
            sqlx::query("UPDATE yt_cached SET cached_at = ? WHERE video_id = ?")
                .bind(1000 + i as i64).bind(id).execute(&pool).await.unwrap();
        }
        // Budget for two → the oldest goes.
        let gone = evict_cached_over_bytes(&pool, 2048).await.unwrap();
        let ids: Vec<String> = gone.into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, vec!["a".to_string()]);
        assert!(evict_cached_over_bytes(&pool, 1_000_000).await.unwrap().is_empty());
    }

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
        set_sub_meta(&pool, "UC1", Some("/a/av.png"), Some(142), Some(5000)).await.unwrap();
        let subs = list_subs(&pool).await.unwrap();
        let one = subs.iter().find(|s| s.channel_id == "UC1").unwrap();
        assert_eq!(one.video_count, Some(142));
        assert_eq!(one.sub_count, Some(5000));
        // Soft unsubscribe: channel stays in the list, just flagged subscribed=false.
        unsubscribe(&pool, "UC2").await.unwrap();
        let subs = list_subs(&pool).await.unwrap();
        assert_eq!(subs.len(), 2);
        assert!(!subs.iter().find(|s| s.channel_id == "UC2").unwrap().subscribed);
        assert!(subs.iter().find(|s| s.channel_id == "UC1").unwrap().subscribed);
        // Re-subscribe via import restores the flag.
        import_subs(&pool, &[ImportedSub { channel_id: "UC2".into(), title: "B".into() }]).await.unwrap();
        assert!(list_subs(&pool).await.unwrap().iter().find(|s| s.channel_id == "UC2").unwrap().subscribed);
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
        record_download(&pool, "v1", "T", "C", "/t.png", "/m.opus", 120, "audio", "OPUS", "Audio").await.unwrap();
        assert_eq!(list_downloads(&pool, 10).await.unwrap().len(), 1);
        let path = remove_download(&pool, "v1").await.unwrap();
        assert_eq!(path.as_deref(), Some("/m.opus"));
        assert!(list_downloads(&pool, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn downloads_keep_each_resolution() {
        let (_t, pool) = open_pool().await;
        // Same video, three qualities → three distinct rows (not overwritten).
        record_download(&pool, "v1", "T", "C", "/t.png", "/v1.audio.opus", 120, "audio", "OPUS", "Audio").await.unwrap();
        record_download(&pool, "v1", "T", "C", "/t.png", "/v1.1080p.mkv", 120, "video", "MKV", "1080p").await.unwrap();
        record_download(&pool, "v1", "T", "C", "/t.png", "/v1.720p.mkv", 120, "video", "MKV", "720p").await.unwrap();
        assert_eq!(list_downloads(&pool, 10).await.unwrap().len(), 3);
        // Re-downloading the same quality updates in place (no duplicate).
        record_download(&pool, "v1", "T", "C", "/t.png", "/v1.720p.mkv", 120, "video", "MKV", "720p").await.unwrap();
        assert_eq!(list_downloads(&pool, 10).await.unwrap().len(), 3);
        // Path-targeted removal drops only that resolution.
        remove_download_by_path(&pool, "/v1.720p.mkv").await.unwrap();
        let left = list_downloads(&pool, 10).await.unwrap();
        assert_eq!(left.len(), 2);
        assert!(left.iter().all(|d| d.media_path != "/v1.720p.mkv"));
        // Clear removes the rest + returns paths.
        let paths = clear_downloads(&pool).await.unwrap();
        assert_eq!(paths.len(), 2);
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
