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

-- `yt-dlp -J`, trimmed to the format panel's rows. The stream URLs inside a
-- real dump expire within hours; the list of formats does not, so six hours is
-- a cache window, not a correctness one.
-- What was played, audio or picture, and whether it was seen to the end.
-- The row carries its own title so History reads without a join, and
-- `fill_video_meta` names the ones started before a title was known.
CREATE TABLE IF NOT EXISTS yt_history (
    video_id   TEXT PRIMARY KEY,
    title      TEXT NOT NULL DEFAULT '',
    channel    TEXT NOT NULL DEFAULT '',
    channel_id TEXT NOT NULL DEFAULT '',
    thumb_path TEXT NOT NULL DEFAULT '',
    duration   INTEGER NOT NULL DEFAULT 0,
    watched_at INTEGER NOT NULL,
    finished   INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS yt_history_at_idx ON yt_history(watched_at);

CREATE TABLE IF NOT EXISTS yt_formats (
    video_id   TEXT PRIMARY KEY,
    json       TEXT NOT NULL,
    fetched_at INTEGER NOT NULL
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
    // After the v1 rebuild, which recreates `yt_downloaded` from a fixed
    // column list and would drop anything added before it. Nullable: rows
    // written before this, and by the Slint build, have no channel id.
    let _ = sqlx::query("ALTER TABLE yt_cached ADD COLUMN channel_id TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE yt_downloaded ADD COLUMN channel_id TEXT").execute(pool).await;
    // Cache rows say what they hold and how big it is, so the Cached page can
    // total them without a stat per row, and eviction can order by use.
    let _ = sqlx::query("ALTER TABLE yt_cached ADD COLUMN kind TEXT NOT NULL DEFAULT 'audio'").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE yt_cached ADD COLUMN bytes INTEGER").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE yt_cached ADD COLUMN played_at INTEGER").execute(pool).await;
    // The newest listed video when the channel was last looked at: "new" is
    // whatever sits above this one, which approximate dates cannot tell.
    let _ = sqlx::query("ALTER TABLE yt_subs ADD COLUMN seen_video TEXT").execute(pool).await;
    // The channel page's banner, a local copy like `avatar_path`.
    let _ = sqlx::query("ALTER TABLE yt_subs ADD COLUMN banner_path TEXT").execute(pool).await;
    // The channel's @handle, from its listing.
    let _ = sqlx::query("ALTER TABLE yt_subs ADD COLUMN handle TEXT").execute(pool).await;
    // When a listed video went up, unix seconds. yt-dlp reads it off the
    // listing's "3 days ago", so it is approximate; NULL for rows written
    // before the column, and by the Slint build.
    let _ = sqlx::query("ALTER TABLE yt_channel_cache ADD COLUMN published INTEGER").execute(pool).await;
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
    /// "@name"; None until a listing has been fetched.
    pub handle: Option<String>,
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
    /// "" when the row predates the column or was written by the Slint build.
    pub channel_id: String,
    /// "audio" | "video".
    pub kind: String,
    /// File size; -1 where it is not tracked (downloads) or the file is gone.
    pub bytes: i64,
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
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, Option<String>, Option<i64>, Option<i64>, Option<i64>, i64, Option<String>)> = sqlx::query_as(
        "SELECT channel_id, title, avatar_path, video_count, sub_count, fetched_at, subscribed, NULLIF(handle, '') FROM yt_subs ORDER BY title COLLATE NOCASE",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(channel_id, title, avatar_path, video_count, sub_count, fetched_at, subscribed, handle)| Sub {
            channel_id,
            title,
            avatar_path,
            video_count,
            sub_count,
            fetched_at,
            subscribed: subscribed != 0,
            handle,
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

/// Save what a channel's listing says about it: art (local paths), follower
/// count, @handle. Stamps `fetched_at`, so a channel refreshed this way is not
/// stale. Channels not in `yt_subs` are left alone; `None` keeps what is stored.
pub async fn set_channel_art(
    pool: &SqlitePool,
    channel_id: &str,
    avatar: Option<&str>,
    banner: Option<&str>,
    followers: Option<i64>,
    handle: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE yt_subs SET avatar_path = COALESCE(?, avatar_path), banner_path = COALESCE(?, banner_path),
             sub_count = COALESCE(?, sub_count), handle = COALESCE(?, handle), fetched_at = ?
         WHERE channel_id = ?",
    )
    .bind(avatar)
    .bind(banner)
    .bind(followers)
    .bind(handle)
    .bind(now())
    .bind(channel_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// `(avatar, banner)` as stored, "" where unknown.
pub async fn channel_art(pool: &SqlitePool, channel_id: &str) -> Result<(String, String)> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(avatar_path, ''), COALESCE(banner_path, '') FROM yt_subs WHERE channel_id = ?",
    )
    .bind(channel_id)
    .fetch_optional(pool)
    .await?
    .unwrap_or_default())
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct VideoMeta {
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    pub thumb_path: String,
    /// Seconds; 0 when unknown.
    pub duration: i64,
}

/// A video's title, channel and thumbnail from whichever table has a real
/// title for it (a bare import is titled with its own id): downloads, the cache, playlists, channel listings.
pub async fn video_meta(pool: &SqlitePool, video_id: &str) -> Result<Option<VideoMeta>> {
    let row: Option<(String, String, String, String, i64)> = sqlx::query_as(
        "SELECT title, COALESCE(channel,''), COALESCE(channel_id,''), COALESCE(thumb_path,''), COALESCE(duration,0) FROM (
             SELECT title, channel, channel_id, thumb_path, duration, 0 AS r FROM yt_downloaded WHERE video_id = ?1
             UNION ALL SELECT title, channel, channel_id, thumb_path, duration, 1 FROM yt_cached WHERE video_id = ?1
             -- A flat channel listing leaves the channel name off its rows.
             UNION ALL SELECT title, COALESCE(NULLIF(channel, ''), (SELECT s.title FROM yt_subs s WHERE s.channel_id = c.channel_id)),
                    channel_id, thumb_path, duration, 2 FROM yt_channel_cache c WHERE video_id = ?1
             UNION ALL SELECT title, COALESCE(NULLIF(channel, ''), (SELECT s.title FROM yt_subs s WHERE s.channel_id = p.channel_id)),
                    channel_id, thumb_path, duration, 3 FROM yt_channel_popular p WHERE video_id = ?1
             UNION ALL SELECT title, channel, '', thumb_path, duration, 4 FROM yt_playlist_items WHERE video_id = ?1
             UNION ALL SELECT title, channel, '', thumb_path, duration, 5 FROM yt_playlist_cache WHERE video_id = ?1
         ) WHERE COALESCE(title,'') NOT IN ('', ?1) ORDER BY r LIMIT 1",
    )
    .bind(video_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(title, channel, channel_id, thumb_path, duration)| VideoMeta {
        title,
        channel,
        channel_id,
        thumb_path,
        duration,
    }))
}

/// Fill in a video's title, channel, thumbnail and length wherever it was
/// stored without them: playlist rows imported as bare ids (titled with the
/// id), cache rows written before the metadata was known. What is there stays.
pub async fn fill_video_meta(pool: &SqlitePool, video_id: &str, m: &VideoMeta) -> Result<()> {
    for table in ["yt_playlist_items", "yt_playlist_cache", "yt_cached", "yt_downloaded", "yt_history"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET
                 title = CASE WHEN COALESCE(title, '') IN ('', video_id) THEN ?1 ELSE title END,
                 channel = COALESCE(NULLIF(channel, ''), ?2),
                 thumb_path = COALESCE(NULLIF(thumb_path, ''), ?3),
                 duration = CASE WHEN COALESCE(duration, 0) = 0 THEN ?5 ELSE duration END
             WHERE video_id = ?4 AND (COALESCE(title, '') IN ('', video_id) OR COALESCE(duration, 0) = 0)"
        )))
        .bind(&m.title)
        .bind(&m.channel)
        .bind(&m.thumb_path)
        .bind(video_id)
        .bind(m.duration)
        .execute(pool)
        .await?;
    }
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

/// A channel's newest upload, from a check that asked for one video: put on
/// top of the cached listing, the rest moved down, unless the listing already
/// has it. The older rows stay, so a check does not cut a listing to one.
pub async fn prepend_channel_video(
    pool: &SqlitePool,
    channel_id: &str,
    v: &ChannelVid,
    published: Option<i64>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let known: Option<i64> =
        sqlx::query_scalar("SELECT position FROM yt_channel_cache WHERE channel_id = ? AND video_id = ?")
            .bind(channel_id)
            .bind(&v.video_id)
            .fetch_optional(&mut *tx)
            .await?;
    if known.is_none() {
        sqlx::query("UPDATE yt_channel_cache SET position = position + 1 WHERE channel_id = ?")
            .bind(channel_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO yt_channel_cache (channel_id,video_id,title,channel,meta,info,thumb_path,duration,position) VALUES (?,?,?,?,?,?,?,?,0)",
        )
        .bind(channel_id).bind(&v.video_id).bind(&v.title).bind(&v.channel)
        .bind(&v.meta).bind(&v.info).bind(&v.thumb_path).bind(v.duration)
        .execute(&mut *tx).await?;
    }
    if let Some(ts) = published {
        sqlx::query("UPDATE yt_channel_cache SET published = ? WHERE channel_id = ? AND video_id = ?")
            .bind(ts)
            .bind(channel_id)
            .bind(&v.video_id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Stamp a channel's cached listing with upload times, `(video_id, unix
/// seconds)`. Run after `set_channel_cache`, which rewrites the rows.
pub async fn set_listing_dates(pool: &SqlitePool, channel_id: &str, dates: &[(String, i64)]) -> Result<()> {
    for (video_id, ts) in dates {
        sqlx::query("UPDATE yt_channel_cache SET published = ? WHERE channel_id = ? AND video_id = ?")
            .bind(ts)
            .bind(channel_id)
            .bind(video_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// Channel listing rows with `fresh` = listed above the channel's
/// `seen_video`. A seen video that has dropped off the listing makes every row
/// fresh; a channel never looked at has none.
const FEED_ROWS: &str = "SELECT c.channel_id, c.video_id, COALESCE(c.title,'') AS title, s.title AS channel,
        COALESCE(c.thumb_path,'') AS thumb_path, COALESCE(c.duration,0) AS duration, c.position, s.subscribed,
        COALESCE(c.published,0) AS published,
        (s.seen_video IS NOT NULL AND c.position < COALESCE(
            (SELECT m.position FROM yt_channel_cache m WHERE m.channel_id = c.channel_id AND m.video_id = s.seen_video),
            1000000)) AS fresh
     FROM yt_channel_cache c JOIN yt_subs s ON s.channel_id = c.channel_id";

/// Mark the channel's current listing as seen. `only_if_unset` sets a
/// baseline for a channel never looked at, and leaves the rest alone.
pub async fn mark_seen(pool: &SqlitePool, channel_id: &str, only_if_unset: bool) -> Result<()> {
    sqlx::query(
        "UPDATE yt_subs SET seen_video =
            (SELECT video_id FROM yt_channel_cache WHERE channel_id = ?1 ORDER BY position LIMIT 1)
         WHERE channel_id = ?1 AND (?2 = 0 OR seen_video IS NULL)",
    )
    .bind(channel_id)
    .bind(only_if_unset)
    .execute(pool)
    .await?;
    Ok(())
}

/// New videos per channel, for channels that have any.
pub async fn new_counts(pool: &SqlitePool) -> Result<std::collections::HashMap<String, i64>> {
    let sql = format!("SELECT channel_id, SUM(fresh) FROM ({FEED_ROWS}) GROUP BY channel_id HAVING SUM(fresh) > 0");
    let rows: Vec<(String, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(sql)).fetch_all(pool).await?;
    Ok(rows.into_iter().collect())
}

#[derive(Debug, Clone, PartialEq)]
pub struct FeedVid {
    pub channel_id: String,
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub thumb_path: String,
    pub duration: i64,
    pub fresh: bool,
    /// Unix seconds, approximate; 0 when unknown.
    pub published: i64,
}

/// The Subscriptions feed, new videos first. One channel: its 30 newest, in
/// its own order. `""`: every subscribed channel's newest video, newest upload
/// first where the listing had dates.
pub async fn sub_feed(pool: &SqlitePool, channel_id: &str) -> Result<Vec<FeedVid>> {
    let sql = format!(
        "SELECT channel_id, video_id, title, channel, thumb_path, duration, fresh, published FROM ({FEED_ROWS})
         WHERE (?1 = '' AND subscribed = 1 AND position = 0) OR (channel_id = ?1 AND position < 30)
         ORDER BY fresh DESC, CASE WHEN ?1 = '' THEN -published ELSE 0 END, position, channel COLLATE NOCASE
         LIMIT 1000"
    );
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, String, String, i64, bool, i64)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql)).bind(channel_id).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(channel_id, video_id, title, channel, thumb_path, duration, fresh, published)| FeedVid {
            channel_id, video_id, title, channel, thumb_path, duration, fresh, published,
        })
        .collect())
}

/// The newest videos across `channel_ids`, by upload time. Rows with no date
/// (a listing cached before dates were read) follow, every channel's newest,
/// then every one's second newest, ties in the order given.
pub async fn latest_across(pool: &SqlitePool, channel_ids: &[String], limit: i64) -> Result<Vec<FeedVid>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, String, String, i64, i64)> = sqlx::query_as(
        "SELECT c.channel_id, c.video_id, COALESCE(c.title, ''), COALESCE(s.title, c.channel, ''),
                COALESCE(c.thumb_path, ''), COALESCE(c.duration, 0), COALESCE(c.published, 0) AS published
         FROM yt_channel_cache c
         JOIN json_each(?1) j ON j.value = c.channel_id
         LEFT JOIN yt_subs s ON s.channel_id = c.channel_id
         ORDER BY published DESC, c.position, j.key
         LIMIT ?2",
    )
    .bind(serde_json::to_string(channel_ids)?)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(channel_id, video_id, title, channel, thumb_path, duration, published)| FeedVid {
            channel_id, video_id, title, channel, thumb_path, duration, fresh: false, published,
        })
        .collect())
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
    record_cached_as(pool, id, title, channel, thumb, media, duration, "audio").await
}

/// Record a cached file as `kind` ("audio" | "video"), sized from disk. One row
/// per video: a video cache replaces an audio one, because the video file
/// carries the sound and the audio deck plays it as well.
#[allow(clippy::too_many_arguments)]
pub async fn record_cached_as(
    pool: &SqlitePool,
    id: &str,
    title: &str,
    channel: &str,
    thumb: &str,
    media: &str,
    duration: i64,
    kind: &str,
) -> Result<()> {
    let bytes = std::fs::metadata(media).map(|m| m.len() as i64).ok();
    sqlx::query(
        "INSERT INTO yt_cached (video_id,title,channel,thumb_path,media_path,duration,cached_at,kind,bytes) VALUES (?,?,?,?,?,?,?,?,?)
         ON CONFLICT(video_id) DO UPDATE SET cached_at = excluded.cached_at, media_path = excluded.media_path,
             kind = excluded.kind, bytes = excluded.bytes",
    )
    .bind(id)
    .bind(title)
    .bind(channel)
    .bind(thumb)
    .bind(media)
    .bind(duration)
    .bind(now())
    .bind(kind)
    .bind(bytes)
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a cached video as just played, for least-recently-played eviction.
pub async fn touch_cached(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("UPDATE yt_cached SET played_at = ? WHERE video_id = ?")
        .bind(now())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn list_cached(pool: &SqlitePool, limit: i64) -> Result<Vec<CachedVideo>> {
    let rows: Vec<(String, String, String, String, String, i64, i64, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,media_path,duration,cached_at,COALESCE(channel_id,''),kind,bytes FROM yt_cached ORDER BY cached_at DESC, rowid DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    let mut out = Vec::with_capacity(rows.len());
    for (video_id, title, channel, thumb_path, media_path, duration, at, channel_id, kind, bytes) in rows {
        // Rows written before the column (or by the Slint build) are sized
        // once, here, and remembered.
        let bytes = match bytes {
            Some(b) => b,
            None => {
                let b = std::fs::metadata(&media_path).map(|m| m.len() as i64).unwrap_or(-1);
                if b >= 0 {
                    sqlx::query("UPDATE yt_cached SET bytes = ? WHERE video_id = ?")
                        .bind(b)
                        .bind(&video_id)
                        .execute(pool)
                        .await?;
                }
                b
            }
        };
        out.push(CachedVideo {
            video_id,
            title,
            channel,
            thumb_path,
            media_path,
            duration,
            at,
            fmt: String::new(),
            quality: String::new(),
            channel_id,
            kind,
            bytes,
        });
    }
    Ok(out)
}

/// Remember how far into a video you got, so closing the app and coming back
/// resumes instead of restarting. Positions within the first or last few
/// seconds are not worth keeping — those are "started" and "finished", and
/// resuming either one is worse than not.
pub async fn save_progress(pool: &SqlitePool, id: &str, position_s: f64, duration_s: f64) -> Result<()> {
    let done = duration_s > 0.0 && position_s >= duration_s - 15.0;
    if done {
        sqlx::query("UPDATE yt_history SET finished = 1 WHERE video_id = ?").bind(id).execute(pool).await?;
    }
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
    let rows: Vec<(String, String, String, String, String, i64, i64, Option<String>, Option<String>, String, String)> = sqlx::query_as(
        "SELECT video_id,title,channel,thumb_path,media_path,duration,downloaded_at,fmt,quality,COALESCE(channel_id,''),kind FROM yt_downloaded ORDER BY downloaded_at DESC, rowid DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(video_id, title, channel, thumb_path, media_path, duration, at, fmt, quality, channel_id, kind)| CachedVideo {
            video_id,
            title,
            channel,
            thumb_path,
            media_path,
            duration,
            at,
            fmt: fmt.unwrap_or_default(),
            quality: quality.unwrap_or_default(),
            channel_id,
            kind,
            bytes: -1,
        })
        .collect())
}

pub async fn remove_cached(pool: &SqlitePool, id: &str) -> Result<Option<String>> {
    let path: Option<String> = sqlx::query_scalar("SELECT media_path FROM yt_cached WHERE video_id = ?")
        .bind(id).fetch_optional(pool).await?;
    sqlx::query("DELETE FROM yt_cached WHERE video_id = ?").bind(id).execute(pool).await?;
    Ok(path)
}

/// A video's format list, if one was fetched within `max_age_s`. A row that no
/// longer deserialises (an older shape) is a miss, not an error.
pub async fn get_formats(
    pool: &SqlitePool,
    video_id: &str,
    max_age_s: i64,
) -> Result<Option<crate::youtube::formats::StreamInfo>> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT json, fetched_at FROM yt_formats WHERE video_id = ?")
            .bind(video_id)
            .fetch_optional(pool)
            .await?;
    Ok(match row {
        Some((json, at)) if now() - at <= max_age_s => serde_json::from_str(&json).ok(),
        _ => None,
    })
}

pub async fn put_formats(
    pool: &SqlitePool,
    video_id: &str,
    info: &crate::youtube::formats::StreamInfo,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO yt_formats (video_id, json, fetched_at) VALUES (?,?,?)
         ON CONFLICT(video_id) DO UPDATE SET json = excluded.json, fetched_at = excluded.fetched_at",
    )
    .bind(video_id)
    .bind(serde_json::to_string(info)?)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Attach a channel id to a video's cache and download rows. Kept apart from
/// `record_cached` / `record_download` so the Slint build's calls to those
/// stay as they are. Never overwrites an id already there.
pub async fn set_channel_id(pool: &SqlitePool, video_id: &str, channel_id: &str) -> Result<()> {
    if channel_id.is_empty() {
        return Ok(());
    }
    for table in ["yt_cached", "yt_downloaded"] {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            "UPDATE {table} SET channel_id = ? WHERE video_id = ? AND COALESCE(channel_id, '') = ''"
        )))
        .bind(channel_id)
        .bind(video_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

/// `video_id → "audio" | "video"` for everything that plays without the
/// network, `"audio:dl" | "video:dl"` where that copy is a download.
pub async fn offline_kinds(pool: &SqlitePool) -> Result<std::collections::HashMap<String, String>> {
    let mut out = std::collections::HashMap::new();
    let cached: Vec<(String, String)> =
        sqlx::query_as("SELECT video_id, kind FROM yt_cached").fetch_all(pool).await?;
    for (id, kind) in cached {
        out.insert(id, kind);
    }
    let downloaded: Vec<(String, String)> =
        sqlx::query_as("SELECT video_id, kind FROM yt_downloaded").fetch_all(pool).await?;
    for (id, kind) in downloaded {
        // A download says so (`:dl`), and beats a cached copy of the same kind
        // or less: a video download wins over anything, an audio one over
        // cached audio.
        if kind == "video" || out.get(&id).is_none_or(|k| k == "audio") {
            out.insert(id, format!("{kind}:dl"));
        }
    }
    Ok(out)
}

/// `q` as a LIKE pattern that matches it anywhere, with `%`, `_` and the
/// escape character itself taken literally. Pair with `ESCAPE '\'`.
fn like_anywhere(q: &str) -> String {
    let escaped = q.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
    format!("%{escaped}%")
}

/// Videos the library knows whose title or channel contains `q`: downloads
/// first, then the cache, then (unless `offline_only`) channel listings. One
/// row per video, the best-ranked store's.
pub async fn find_videos(
    pool: &SqlitePool,
    q: &str,
    offline_only: bool,
    limit: usize,
) -> Result<Vec<CachedVideo>> {
    let listings = if offline_only {
        ""
    } else {
        "UNION ALL SELECT video_id, title, channel, thumb_path, '', duration, 0, '', '', channel_id, 2, ''
         FROM yt_channel_cache"
    };
    let sql = format!(
        "SELECT video_id, COALESCE(title,''), COALESCE(channel,''), COALESCE(thumb_path,''),
                COALESCE(media_path,''), COALESCE(duration,0), COALESCE(at,0), COALESCE(fmt,''),
                COALESCE(quality,''), COALESCE(channel_id,''), COALESCE(kind,'')
         FROM (
             SELECT video_id, title, channel, thumb_path, media_path, duration, downloaded_at AS at,
                    fmt, quality, channel_id, 0 AS rank, kind FROM yt_downloaded
             UNION ALL SELECT video_id, title, channel, thumb_path, media_path, duration, cached_at,
                    '', '', channel_id, 1, kind FROM yt_cached
             {listings}
         )
         WHERE title LIKE ?1 ESCAPE '\\' OR channel LIKE ?1 ESCAPE '\\'
         ORDER BY rank, at DESC"
    );
    let rows: Vec<(String, String, String, String, String, i64, i64, String, String, String, String)> =
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(like_anywhere(q.trim()))
            .fetch_all(pool)
            .await?;
    let mut out: Vec<CachedVideo> = Vec::new();
    for (video_id, title, channel, thumb_path, media_path, duration, at, fmt, quality, channel_id, kind) in rows {
        if out.iter().any(|v| v.video_id == video_id) {
            continue;
        }
        out.push(CachedVideo {
            video_id, title, channel, thumb_path, media_path, duration, at, fmt, quality, channel_id, kind, bytes: -1,
        });
        if out.len() == limit {
            break;
        }
    }
    Ok(out)
}

/// Which cached files go first when the cache is over its budget.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evict {
    /// Not played for longest (never-played counts from when it was cached).
    LeastRecentlyPlayed,
    Oldest,
    Largest,
}

impl Evict {
    pub fn key(&self) -> &'static str {
        match self {
            Evict::LeastRecentlyPlayed => "lru",
            Evict::Oldest => "oldest",
            Evict::Largest => "largest",
        }
    }

    pub fn parse(key: &str) -> Evict {
        match key {
            "oldest" => Evict::Oldest,
            "largest" => Evict::Largest,
            _ => Evict::LeastRecentlyPlayed,
        }
    }
}

/// Bring the cache inside `cap_bytes`, and drop anything not played for
/// `idle_days`. `except` is never removed: it is what is playing. Returns
/// `(video_id, media_path)` for each row removed; the caller deletes files.
pub async fn evict_cached(
    pool: &SqlitePool,
    cap_bytes: i64,
    order: Evict,
    idle_days: Option<u32>,
    except: Option<&str>,
) -> Result<Vec<(String, String)>> {
    let by = match order {
        Evict::LeastRecentlyPlayed => "COALESCE(played_at, cached_at) ASC",
        Evict::Oldest => "cached_at ASC",
        Evict::Largest => "COALESCE(bytes, 0) DESC",
    };
    let rows: Vec<(String, String, i64, i64)> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT video_id, COALESCE(media_path,''), COALESCE(bytes, 0), COALESCE(played_at, cached_at)
         FROM yt_cached ORDER BY {by}, rowid ASC"
    )))
    .fetch_all(pool)
    .await?;
    let idle_before = idle_days.filter(|d| *d > 0).map(|d| now() - d as i64 * 86_400);
    let mut total: i64 = rows.iter().map(|r| r.2).sum();
    let mut victims = Vec::new();
    for (id, path, bytes, last_use) in rows {
        if except == Some(id.as_str()) {
            continue;
        }
        let idle = idle_before.is_some_and(|t| last_use < t);
        if idle || total > cap_bytes {
            total -= bytes;
            victims.push((id, path));
        }
    }
    for (id, _) in &victims {
        sqlx::query("DELETE FROM yt_cached WHERE video_id = ?").bind(id).execute(pool).await?;
    }
    Ok(victims)
}

/// Move a cached file to `dest` and make it a download: the row moves from
/// `yt_cached` to `yt_downloaded`, so it survives eviction. `label` is the
/// Downloads badge ("Opus · from cache").
pub async fn promote_cached(
    pool: &SqlitePool,
    id: &str,
    dest: &std::path::Path,
    label: &str,
) -> Result<std::path::PathBuf> {
    let row: Option<(String, String, String, String, i64, String, String)> = sqlx::query_as(
        "SELECT COALESCE(title,''), COALESCE(channel,''), COALESCE(thumb_path,''), COALESCE(media_path,''),
                COALESCE(duration,0), COALESCE(channel_id,''), kind
         FROM yt_cached WHERE video_id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    let Some((title, channel, thumb, media, duration, channel_id, kind)) = row else {
        anyhow::bail!("that video is no longer in the cache");
    };
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    // A rename cannot cross filesystems, and the cache and ~/Music often sit
    // on different ones.
    if std::fs::rename(&media, dest).is_err() {
        std::fs::copy(&media, dest)?;
        let _ = std::fs::remove_file(&media);
    }
    let fmt = dest
        .extension()
        .map(|e| e.to_string_lossy().to_uppercase())
        .unwrap_or_default();
    record_download(pool, id, &title, &channel, &thumb, &dest.to_string_lossy(), duration, &kind, &fmt, label)
        .await?;
    set_channel_id(pool, id, &channel_id).await?;
    sqlx::query("DELETE FROM yt_cached WHERE video_id = ?").bind(id).execute(pool).await?;
    Ok(dest.to_path_buf())
}

/// Bytes on disk by kind: `(cache_audio, cache_video, downloads_audio,
/// downloads_video)`. Downloads are sized from disk, so a file deleted behind
/// the app's back counts as nothing.
/// Every thumbnail and avatar the YouTube tables point at, as stored: a local
/// path or the URL the art cache keys on.
pub async fn art_sources(pool: &SqlitePool) -> Result<Vec<String>> {
    Ok(sqlx::query_scalar(
        "SELECT thumb_path FROM yt_cached WHERE thumb_path <> ''
         UNION SELECT thumb_path FROM yt_downloaded WHERE thumb_path <> ''
         UNION SELECT thumb_path FROM yt_channel_cache WHERE thumb_path <> ''
         UNION SELECT thumb_path FROM yt_channel_popular WHERE thumb_path <> ''
         UNION SELECT thumb_path FROM yt_playlist_cache WHERE thumb_path <> ''
         UNION SELECT thumb_path FROM yt_playlist_items WHERE thumb_path <> ''
         UNION SELECT avatar_path FROM yt_subs WHERE avatar_path <> ''
         UNION SELECT banner_path FROM yt_subs WHERE banner_path <> ''",
    )
    .fetch_all(pool)
    .await?)
}

pub async fn storage_totals(pool: &SqlitePool) -> Result<(i64, i64, i64, i64)> {
    let cached: Vec<(String, i64)> =
        sqlx::query_as("SELECT kind, COALESCE(bytes, 0) FROM yt_cached").fetch_all(pool).await?;
    let (mut ca, mut cv) = (0, 0);
    for (kind, b) in cached {
        if kind == "video" { cv += b } else { ca += b }
    }
    let downloads: Vec<(String, String)> =
        sqlx::query_as("SELECT kind, COALESCE(media_path,'') FROM yt_downloaded").fetch_all(pool).await?;
    let (mut da, mut dv) = (0, 0);
    for (kind, path) in downloads {
        let b = std::fs::metadata(&path).map(|m| m.len() as i64).unwrap_or(0);
        if kind == "video" { dv += b } else { da += b }
    }
    Ok((ca, cv, da, dv))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ContinueRow {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    pub thumb_path: String,
    pub position_s: f64,
    pub duration_s: f64,
}

/// Part-watched videos, most recently touched first, with whatever the stores
/// know about them. A position with no title anywhere (a search hit that was
/// never cached) is left out: a card with no name is not worth resuming.
pub async fn continue_list(pool: &SqlitePool, limit: usize) -> Result<Vec<ContinueRow>> {
    let rows: Vec<(String, f64, f64, String, String, String, String)> = sqlx::query_as(
        "SELECT p.video_id, p.position_s, p.duration_s,
                COALESCE(m.title, ''), COALESCE(m.channel, ''), COALESCE(m.channel_id, ''), COALESCE(m.thumb_path, '')
         FROM yt_progress p
         JOIN (
             SELECT video_id, title, channel, channel_id, thumb_path, 0 AS rank FROM yt_downloaded
             UNION ALL SELECT video_id, title, channel, channel_id, thumb_path, 1 FROM yt_cached
             UNION ALL SELECT video_id, title, channel, channel_id, thumb_path, 2 FROM yt_channel_cache
             UNION ALL SELECT video_id, title, channel, NULL, thumb_path, 3 FROM yt_playlist_items
         ) m ON m.video_id = p.video_id AND COALESCE(m.title, '') <> ''
         WHERE p.duration_s > 0
         ORDER BY p.updated DESC, p.video_id, m.rank",
    )
    .fetch_all(pool)
    .await?;
    let mut out: Vec<ContinueRow> = Vec::new();
    for (video_id, position_s, duration_s, title, channel, channel_id, thumb_path) in rows {
        // Several stores can know one video; the best-ranked row came first.
        if out.iter().any(|r| r.video_id == video_id) {
            continue;
        }
        out.push(ContinueRow { video_id, title, channel, channel_id, thumb_path, position_s, duration_s });
        if out.len() == limit {
            break;
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, PartialEq)]
pub struct HistoryRow {
    pub video_id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    pub thumb_path: String,
    pub duration: i64,
    pub watched_at: i64,
    pub finished: bool,
}

/// Note a play: the row moves to the top, and what is known about the video
/// fills in whatever the row lacks.
pub async fn record_history(pool: &SqlitePool, id: &str, m: &VideoMeta) -> Result<()> {
    sqlx::query(
        "INSERT INTO yt_history (video_id, title, channel, channel_id, thumb_path, duration, watched_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
         ON CONFLICT(video_id) DO UPDATE SET watched_at = excluded.watched_at,
             title = CASE WHEN excluded.title <> '' THEN excluded.title ELSE title END,
             channel = CASE WHEN excluded.channel <> '' THEN excluded.channel ELSE channel END,
             channel_id = CASE WHEN excluded.channel_id <> '' THEN excluded.channel_id ELSE channel_id END,
             thumb_path = CASE WHEN excluded.thumb_path <> '' THEN excluded.thumb_path ELSE thumb_path END,
             duration = CASE WHEN excluded.duration > 0 THEN excluded.duration ELSE duration END",
    )
    .bind(id)
    .bind(&m.title)
    .bind(&m.channel)
    .bind(&m.channel_id)
    .bind(&m.thumb_path)
    .bind(m.duration)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

/// Mark a video watched or not. Marking one never played adds it to History,
/// so the mark has somewhere to live.
pub async fn mark_watched(pool: &SqlitePool, id: &str, watched: bool, m: &VideoMeta) -> Result<()> {
    if watched {
        sqlx::query(
            "INSERT OR IGNORE INTO yt_history (video_id, title, channel, channel_id, thumb_path, duration, watched_at)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(&m.title)
        .bind(&m.channel)
        .bind(&m.channel_id)
        .bind(&m.thumb_path)
        .bind(m.duration)
        .bind(now())
        .execute(pool)
        .await?;
    }
    sqlx::query("UPDATE yt_history SET finished = ? WHERE video_id = ?")
        .bind(watched)
        .bind(id)
        .execute(pool)
        .await?;
    if watched {
        sqlx::query("DELETE FROM yt_progress WHERE video_id = ?").bind(id).execute(pool).await?;
    }
    Ok(())
}

pub async fn list_history(pool: &SqlitePool, limit: i64) -> Result<Vec<HistoryRow>> {
    #[allow(clippy::type_complexity)]
    let rows: Vec<(String, String, String, String, String, i64, i64, bool)> = sqlx::query_as(
        "SELECT video_id, title, channel, channel_id, thumb_path, duration, watched_at, finished
         FROM yt_history ORDER BY watched_at DESC, rowid DESC LIMIT ?",
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(video_id, title, channel, channel_id, thumb_path, duration, watched_at, finished)| HistoryRow {
            video_id, title, channel, channel_id, thumb_path, duration, watched_at, finished,
        })
        .collect())
}

/// Every video marked or played to the end.
pub async fn watched_ids(pool: &SqlitePool) -> Result<std::collections::HashSet<String>> {
    let ids: Vec<String> = sqlx::query_scalar("SELECT video_id FROM yt_history WHERE finished = 1")
        .fetch_all(pool)
        .await?;
    Ok(ids.into_iter().collect())
}

pub async fn remove_history(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM yt_history WHERE video_id = ?").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn clear_history(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM yt_history").execute(pool).await?;
    Ok(())
}

/// Add rows after a channel's cached listing, from `from` on, skipping videos
/// it already holds: a Load more block. `popular` picks the Popular list.
pub async fn append_channel_listing(
    pool: &SqlitePool,
    popular: bool,
    channel_id: &str,
    vids: &[ChannelVid],
    dates: &[(String, i64)],
) -> Result<()> {
    let table = if popular { "yt_channel_popular" } else { "yt_channel_cache" };
    let mut pos: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM {table} WHERE channel_id = ?"
    )))
    .bind(channel_id)
    .fetch_one(pool)
    .await?;
    for v in vids {
        let added = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT OR IGNORE INTO {table} (channel_id,video_id,title,channel,meta,info,thumb_path,duration,position) VALUES (?,?,?,?,?,?,?,?,?)"
        )))
        .bind(channel_id).bind(&v.video_id).bind(&v.title).bind(&v.channel)
        .bind(&v.meta).bind(&v.info).bind(&v.thumb_path).bind(v.duration).bind(pos)
        .execute(pool)
        .await?
        .rows_affected();
        pos += added as i64;
    }
    if !popular {
        set_listing_dates(pool, channel_id, dates).await?;
    }
    Ok(())
}

/// Move the playlist's `from`th video (in playlist order) to `to`.
pub async fn move_playlist_item(pool: &SqlitePool, playlist_id: i64, from: usize, to: usize) -> Result<()> {
    let mut ids: Vec<String> =
        sqlx::query_scalar("SELECT video_id FROM yt_playlist_items WHERE playlist_id = ? ORDER BY position")
            .bind(playlist_id)
            .fetch_all(pool)
            .await?;
    if from >= ids.len() {
        return Ok(());
    }
    let id = ids.remove(from);
    ids.insert(to.min(ids.len()), id);
    let mut tx = pool.begin().await?;
    for (i, id) in ids.iter().enumerate() {
        sqlx::query("UPDATE yt_playlist_items SET position = ? WHERE playlist_id = ? AND video_id = ?")
            .bind(i as i64)
            .bind(playlist_id)
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

pub async fn set_playlist_count(pool: &SqlitePool, playlist_id: i64, count: i64) -> Result<()> {
    sqlx::query("UPDATE yt_playlists SET video_count = ? WHERE id = ?")
        .bind(count)
        .bind(playlist_id)
        .execute(pool)
        .await?;
    Ok(())
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
        // Cover: the first video's thumbnail, YouTube's own still where the row
        // has none (a playlist imported as bare ids).
        const COVER: &str = "COALESCE(NULLIF(thumb_path, ''), 'https://i.ytimg.com/vi/' || video_id || '/hqdefault.jpg')";
        let cover: Option<String> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
            "SELECT {COVER} FROM yt_playlist_items WHERE playlist_id = ? ORDER BY position LIMIT 1")))
            .bind(id).fetch_optional(pool).await?
            .or(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
                "SELECT {COVER} FROM yt_playlist_cache WHERE playlist_id = ? ORDER BY position LIMIT 1")))
                .bind(id).fetch_optional(pool).await?);
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
    async fn art_sources_are_distinct_across_tables() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        record_cached(&pool, "a", "t", "c", "https://i.ytimg.com/a.jpg", "/x/a.opus", 1).await.unwrap();
        let listing = ChannelVid {
            video_id: "a".into(),
            title: "t".into(),
            channel: String::new(),
            meta: String::new(),
            info: String::new(),
            thumb_path: "https://i.ytimg.com/a.jpg".into(),
            duration: 1,
        };
        set_channel_cache(&pool, "ch", &[listing]).await.unwrap();
        record_cached(&pool, "b", "t", "c", "", "/x/b.opus", 1).await.unwrap();
        assert_eq!(art_sources(&pool).await.unwrap(), vec!["https://i.ytimg.com/a.jpg".to_string()]);
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
    async fn cache_records() {
        let (_t, pool) = open_pool().await;
        record_cached(&pool, "v1", "T1", "C", "/t/1.png", "/m/1.opus", 100).await.unwrap();
        record_cached(&pool, "v2", "T2", "C", "/t/2.png", "/m/2.opus", 200).await.unwrap();
        assert_eq!(list_cached(&pool, 10).await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn downloads_add_and_remove() {
        let (_t, pool) = open_pool().await;
        record_download(&pool, "v1", "T", "C", "/t.png", "/m.opus", 120, "audio", "OPUS", "Audio").await.unwrap();
        assert_eq!(list_downloads(&pool, 10).await.unwrap().len(), 1);
        let path = remove_download_by_path(&pool, "/m.opus").await.unwrap();
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

    #[tokio::test]
    async fn channel_id_fills_once_and_the_lists_carry_it() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        record_cached(&pool, "a", "A", "Chan", "", "/c/a.opus", 60).await.unwrap();
        record_download(&pool, "a", "A", "Chan", "", "/d/a.mp3", 60, "audio", "MP3", "MP3 · 320 kbps").await.unwrap();
        assert_eq!(list_cached(&pool, 10).await.unwrap()[0].channel_id, "");
        set_channel_id(&pool, "a", "UC1").await.unwrap();
        // A second, different id does not replace the first.
        set_channel_id(&pool, "a", "UC2").await.unwrap();
        assert_eq!(list_cached(&pool, 10).await.unwrap()[0].channel_id, "UC1");
        assert_eq!(list_downloads(&pool, 10).await.unwrap()[0].channel_id, "UC1");
        // Applying the schema again (every start) is harmless.
        apply_schema(&pool).await.unwrap();
        assert_eq!(list_cached(&pool, 10).await.unwrap()[0].channel_id, "UC1");
    }

    #[tokio::test]
    async fn offline_kinds_let_a_video_download_win() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        record_cached(&pool, "a", "A", "", "", "/c/a.opus", 60).await.unwrap();
        record_cached(&pool, "b", "B", "", "", "/c/b.opus", 60).await.unwrap();
        record_download(&pool, "b", "B", "", "", "/d/b.mp4", 60, "video", "MP4", "MP4 · 1080p · H.264").await.unwrap();
        record_download(&pool, "c", "C", "", "", "/d/c.mp3", 60, "audio", "MP3", "MP3 · 320 kbps").await.unwrap();
        let k = offline_kinds(&pool).await.unwrap();
        assert_eq!(k.get("a").map(String::as_str), Some("audio"));
        assert_eq!(k.get("b").map(String::as_str), Some("video:dl"));
        assert_eq!(k.get("c").map(String::as_str), Some("audio:dl"));
        assert_eq!(k.get("z"), None);
    }

    #[tokio::test]
    async fn continue_list_is_newest_first_named_and_unique() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        // "a" is known to two stores; the download's row is the one used.
        record_cached(&pool, "a", "A cached", "Chan", "", "/c/a.opus", 600).await.unwrap();
        record_download(&pool, "a", "A downloaded", "Chan", "", "/d/a.mp3", 600, "audio", "MP3", "MP3 · 320 kbps").await.unwrap();
        set_channel_cache(&pool, "UCb", &[ChannelVid {
            video_id: "b".into(), title: "B".into(), channel: "Bee".into(), meta: String::new(),
            info: String::new(), thumb_path: String::new(), duration: 600,
        }]).await.unwrap();
        save_progress(&pool, "a", 100.0, 600.0).await.unwrap();
        save_progress(&pool, "b", 200.0, 600.0).await.unwrap();
        // Watched but never stored anywhere: no title, so no card.
        save_progress(&pool, "ghost", 300.0, 600.0).await.unwrap();
        sqlx::query("UPDATE yt_progress SET updated = 100 WHERE video_id = 'a'").execute(&pool).await.unwrap();
        sqlx::query("UPDATE yt_progress SET updated = 200 WHERE video_id = 'b'").execute(&pool).await.unwrap();

        let rows = continue_list(&pool, 10).await.unwrap();
        let ids: Vec<&str> = rows.iter().map(|r| r.video_id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
        assert_eq!((rows[0].channel_id.as_str(), rows[0].position_s), ("UCb", 200.0));
        assert_eq!(rows[1].title, "A downloaded");
        assert_eq!(continue_list(&pool, 1).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn find_videos_ranks_stores_and_takes_wildcards_literally() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        record_download(&pool, "a", "100% Lofi", "Moss", "", "/d/a.mp3", 60, "audio", "MP3", "MP3 · 320 kbps").await.unwrap();
        record_cached(&pool, "b", "lofi rain", "Moss", "", "/c/b.opus", 60).await.unwrap();
        // Also cached: must appear once, as the download.
        record_cached(&pool, "a", "100% Lofi", "Moss", "", "/c/a.opus", 60).await.unwrap();
        set_channel_cache(&pool, "UCm", &[ChannelVid {
            video_id: "c".into(), title: "Beats".into(), channel: "LOFI Harbour".into(), meta: String::new(),
            info: String::new(), thumb_path: String::new(), duration: 60,
        }]).await.unwrap();

        let ids = |v: Vec<CachedVideo>| v.into_iter().map(|x| x.video_id).collect::<Vec<_>>();
        assert_eq!(ids(find_videos(&pool, "lofi", false, 10).await.unwrap()), ["a", "b", "c"]);
        assert_eq!(ids(find_videos(&pool, "lofi", true, 10).await.unwrap()), ["a", "b"]);
        assert_eq!(ids(find_videos(&pool, "lofi", false, 1).await.unwrap()), ["a"]);
        assert_eq!(find_videos(&pool, "lofi", false, 10).await.unwrap()[0].media_path, "/d/a.mp3");
        // `%` and `_` are characters, not wildcards.
        assert_eq!(ids(find_videos(&pool, "0%", false, 10).await.unwrap()), ["a"]);
        assert!(find_videos(&pool, "_", false, 10).await.unwrap().is_empty());
        assert!(find_videos(&pool, "jazz", false, 10).await.unwrap().is_empty());
    }

    /// Cache rows with real files of the given sizes, cached in order.
    async fn cache_with_files(pool: &SqlitePool, dir: &std::path::Path, rows: &[(&str, usize)]) {
        for (i, (id, size)) in rows.iter().enumerate() {
            let path = dir.join(format!("{id}.opus"));
            std::fs::write(&path, vec![0u8; *size]).unwrap();
            record_cached(pool, id, id, "", "", &path.to_string_lossy(), 60).await.unwrap();
            sqlx::query("UPDATE yt_cached SET cached_at = ? WHERE video_id = ?")
                .bind(1_000 + i as i64).bind(id).execute(pool).await.unwrap();
        }
    }

    #[tokio::test]
    async fn evict_cached_follows_the_order_and_spares_what_is_playing() {
        let dir = tempfile::tempdir().unwrap();
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        cache_with_files(&pool, dir.path(), &[("a", 300), ("b", 100), ("c", 200)]).await;
        let ids = |v: Vec<(String, String)>| v.into_iter().map(|x| x.0).collect::<Vec<_>>();

        // Under budget: nothing goes.
        assert!(evict_cached(&pool, 600, Evict::Oldest, None, None).await.unwrap().is_empty());
        // Largest first: dropping a (300) brings 600 under 400.
        assert_eq!(ids(evict_cached(&pool, 400, Evict::Largest, None, None).await.unwrap()), ["a"]);

        cache_with_files(&pool, dir.path(), &[("a", 300)]).await;
        // b was played most recently of the three, a never: a, then c.
        touch_cached(&pool, "b").await.unwrap();
        assert_eq!(ids(evict_cached(&pool, 150, Evict::LeastRecentlyPlayed, None, None).await.unwrap()), ["a", "c"]);

        cache_with_files(&pool, dir.path(), &[("a", 300), ("c", 200)]).await;
        // Oldest first, but a is playing: b goes, then c.
        assert_eq!(ids(evict_cached(&pool, 350, Evict::Oldest, None, Some("a")).await.unwrap()), ["b", "c"]);
    }

    #[tokio::test]
    async fn evict_cached_drops_idle_rows_whatever_the_budget() {
        let dir = tempfile::tempdir().unwrap();
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        cache_with_files(&pool, dir.path(), &[("old", 10), ("new", 10)]).await;
        // `cache_with_files` dates rows to 1970; bring "new" to now.
        touch_cached(&pool, "new").await.unwrap();
        let gone = evict_cached(&pool, i64::MAX, Evict::Oldest, Some(30), None).await.unwrap();
        assert_eq!(gone.iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["old"]);
        // 0 days means never.
        assert!(evict_cached(&pool, i64::MAX, Evict::Oldest, Some(0), None).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn sizes_are_recorded_backfilled_and_totalled() {
        let dir = tempfile::tempdir().unwrap();
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        cache_with_files(&pool, dir.path(), &[("a", 120)]).await;
        let v = dir.path().join("v.video.mkv");
        std::fs::write(&v, vec![0u8; 500]).unwrap();
        record_cached_as(&pool, "v", "V", "", "", &v.to_string_lossy(), 60, "video").await.unwrap();
        // A row from before the column: no size until listed.
        sqlx::query("UPDATE yt_cached SET bytes = NULL WHERE video_id = 'a'").execute(&pool).await.unwrap();
        let listed = list_cached(&pool, 10).await.unwrap();
        let a = listed.iter().find(|x| x.video_id == "a").unwrap();
        assert_eq!((a.kind.as_str(), a.bytes), ("audio", 120));
        let d = dir.path().join("d.mp3");
        std::fs::write(&d, vec![0u8; 70]).unwrap();
        record_download(&pool, "d", "D", "", "", &d.to_string_lossy(), 60, "audio", "MP3", "MP3 · 320 kbps").await.unwrap();
        assert_eq!(storage_totals(&pool).await.unwrap(), (120, 500, 70, 0));
        assert_eq!(offline_kinds(&pool).await.unwrap().get("v").map(String::as_str), Some("video"));
    }

    #[tokio::test]
    async fn promote_moves_the_file_and_the_row() {
        let dir = tempfile::tempdir().unwrap();
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        cache_with_files(&pool, dir.path(), &[("a", 40)]).await;
        set_channel_id(&pool, "a", "UCa").await.unwrap();
        let dest = dir.path().join("Music/YouTube/Chan/A.opus");
        let moved = promote_cached(&pool, "a", &dest, "Opus · from cache").await.unwrap();
        assert!(moved.exists());
        assert!(!dir.path().join("a.opus").exists());
        assert!(list_cached(&pool, 10).await.unwrap().is_empty());
        let d = &list_downloads(&pool, 10).await.unwrap()[0];
        assert_eq!((d.fmt.as_str(), d.quality.as_str(), d.channel_id.as_str()), ("OPUS", "Opus · from cache", "UCa"));
        assert!(promote_cached(&pool, "a", &dest, "x").await.is_err());
    }

    #[tokio::test]
    async fn formats_cache_expires_and_survives_bad_rows() {
        use crate::youtube::formats::{AudioFormat, StreamInfo};
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let info = StreamInfo {
            audio: vec![AudioFormat {
                id: "251".into(),
                acodec: "opus".into(),
                ext: "webm".into(),
                abr_kbps: 129,
                rate_hz: 48_000,
                bytes: None,
            }],
            duration_s: 213.0,
            ..Default::default()
        };
        assert_eq!(get_formats(&pool, "v", 3600).await.unwrap(), None);
        put_formats(&pool, "v", &info).await.unwrap();
        assert_eq!(get_formats(&pool, "v", 3600).await.unwrap(), Some(info.clone()));
        // Re-putting replaces rather than duplicating.
        put_formats(&pool, "v", &info).await.unwrap();
        // Seven hours old is stale for a six-hour window.
        sqlx::query("UPDATE yt_formats SET fetched_at = fetched_at - 25200")
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(get_formats(&pool, "v", 6 * 3600).await.unwrap(), None);
        // A row from an older shape is a miss, not an error.
        sqlx::query("INSERT INTO yt_formats (video_id, json, fetched_at) VALUES ('w', '{\"nope\":1', ?)")
            .bind(now())
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(get_formats(&pool, "w", 3600).await.unwrap(), None);
    }

    #[tokio::test]
    async fn new_videos_are_the_ones_above_the_seen_marker() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        import_subs(&pool, &[ImportedSub { channel_id: "ch".into(), title: "Chan".into() }]).await.unwrap();
        let vid = |id: &str| ChannelVid {
            video_id: id.into(),
            title: id.into(),
            channel: String::new(),
            meta: String::new(),
            info: String::new(),
            thumb_path: String::new(),
            duration: 1,
        };
        let listing = |ids: &[&str]| ids.iter().map(|i| vid(i)).collect::<Vec<_>>();

        // Never looked at: nothing is new, and the baseline is the top video.
        set_channel_cache(&pool, "ch", &listing(&["c", "b", "a"])).await.unwrap();
        assert!(new_counts(&pool).await.unwrap().is_empty());
        mark_seen(&pool, "ch", true).await.unwrap();

        // Two uploads later.
        set_channel_cache(&pool, "ch", &listing(&["e", "d", "c", "b", "a"])).await.unwrap();
        mark_seen(&pool, "ch", true).await.unwrap(); // baseline kept
        assert_eq!(new_counts(&pool).await.unwrap().get("ch"), Some(&2));
        let feed = sub_feed(&pool, "").await.unwrap();
        let fresh: Vec<_> = feed.iter().filter(|f| f.fresh).map(|f| f.video_id.as_str()).collect();
        assert_eq!(fresh, ["e", "d"]);
        // All channels: each channel's newest.
        assert_eq!(feed.iter().map(|f| f.video_id.as_str()).collect::<Vec<_>>(), ["e"]);

        // Looking clears it.
        mark_seen(&pool, "ch", false).await.unwrap();
        assert!(new_counts(&pool).await.unwrap().is_empty());
        assert_eq!(sub_feed(&pool, "ch").await.unwrap().len(), 5);

        // A one-video check puts the newest on top and keeps the rest.
        prepend_channel_video(&pool, "ch", &vid("f"), Some(500)).await.unwrap();
        prepend_channel_video(&pool, "ch", &vid("f"), None).await.unwrap(); // known: no-op
        let ids: Vec<_> = get_channel_cache(&pool, "ch").await.unwrap().into_iter().map(|v| v.video_id).collect();
        assert_eq!(ids, ["f", "e", "d", "c", "b", "a"]);
        assert_eq!(new_counts(&pool).await.unwrap().get("ch"), Some(&1));

        // The seen video fell off the listing: all of it is new.
        set_channel_cache(&pool, "ch", &listing(&["h", "g", "f"])).await.unwrap();
        assert_eq!(new_counts(&pool).await.unwrap().get("ch"), Some(&3));
    }

    #[tokio::test]
    async fn video_meta_is_found_and_filled_in() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let pl = create_playlist(&pool, "p").await.unwrap();
        add_playlist_ids(&pool, pl, &["v1".to_string()]).await.unwrap();
        assert_eq!(video_meta(&pool, "v1").await.unwrap(), None, "a bare id has no title");
        assert!(list_playlists(&pool).await.unwrap()[0]
            .cover
            .as_deref()
            .is_some_and(|c| c.ends_with("/vi/v1/hqdefault.jpg")));

        let m = VideoMeta {
            title: "Song".into(),
            channel: "Chan".into(),
            channel_id: String::new(),
            thumb_path: "https://i.ytimg.com/vi/v1/x.jpg".into(),
            duration: 212,
        };
        fill_video_meta(&pool, "v1", &m).await.unwrap();
        let got = video_meta(&pool, "v1").await.unwrap().unwrap();
        assert_eq!((got.title.as_str(), got.channel.as_str(), got.duration), ("Song", "Chan", 212));
        // A second answer does not rename what is already named.
        fill_video_meta(&pool, "v1", &VideoMeta { title: "Other".into(), ..m.clone() }).await.unwrap();
        assert_eq!(video_meta(&pool, "v1").await.unwrap().unwrap().title, "Song");

        import_subs(&pool, &[ImportedSub { channel_id: "ch".into(), title: "C".into() }]).await.unwrap();
        set_channel_art(&pool, "ch", Some("/a.jpg"), None, Some(5), Some("@c")).await.unwrap();
        set_channel_art(&pool, "ch", None, Some("/b.jpg"), None, None).await.unwrap();
        assert_eq!(channel_art(&pool, "ch").await.unwrap(), ("/a.jpg".into(), "/b.jpg".into()));
        let sub = list_subs(&pool).await.unwrap().remove(0);
        assert_eq!((sub.handle.as_deref(), sub.sub_count), (Some("@c"), Some(5)));
        assert!(sub.fetched_at.is_some(), "a listing counts as a refresh");
        assert!(art_sources(&pool).await.unwrap().contains(&"/b.jpg".to_string()), "banners are kept by the sweep");
    }

    #[tokio::test]
    async fn history_records_marks_and_finishes() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let m = VideoMeta { title: "T".into(), duration: 600, ..Default::default() };
        record_history(&pool, "a", &m).await.unwrap();
        record_history(&pool, "b", &VideoMeta::default()).await.unwrap();
        assert_eq!(list_history(&pool, 10).await.unwrap().len(), 2);
        assert!(watched_ids(&pool).await.unwrap().is_empty());

        // Played to the end.
        save_progress(&pool, "a", 595.0, 600.0).await.unwrap();
        assert!(watched_ids(&pool).await.unwrap().contains("a"));
        // A later play without a title keeps the one it had.
        record_history(&pool, "a", &VideoMeta::default()).await.unwrap();
        assert_eq!(list_history(&pool, 10).await.unwrap()[0].title, "T");

        // Marked by hand, including one never played.
        mark_watched(&pool, "c", true, &VideoMeta::default()).await.unwrap();
        mark_watched(&pool, "a", false, &m).await.unwrap();
        let w = watched_ids(&pool).await.unwrap();
        assert!(w.contains("c") && !w.contains("a"));

        // Named later by a metadata fill.
        fill_video_meta(&pool, "b", &VideoMeta { title: "Named".into(), ..Default::default() }).await.unwrap();
        assert!(list_history(&pool, 10).await.unwrap().iter().any(|r| r.video_id == "b" && r.title == "Named"));

        remove_history(&pool, "c").await.unwrap();
        assert_eq!(list_history(&pool, 10).await.unwrap().len(), 2);
        clear_history(&pool).await.unwrap();
        assert!(list_history(&pool, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn playlist_items_move_and_listings_append() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let pl = create_playlist(&pool, "p").await.unwrap();
        add_playlist_ids(&pool, pl, &["a".into(), "b".into(), "c".into()]).await.unwrap();
        move_playlist_item(&pool, pl, 0, 2).await.unwrap();
        let order: Vec<String> = playlist_items(&pool, pl).await.unwrap().into_iter().map(|i| i.video_id).collect();
        assert_eq!(order, ["b", "c", "a"]);

        let vid = |id: &str| ChannelVid {
            video_id: id.into(),
            title: id.into(),
            channel: String::new(),
            meta: String::new(),
            info: String::new(),
            thumb_path: String::new(),
            duration: 1,
        };
        set_channel_cache(&pool, "ch", &[vid("1"), vid("2")]).await.unwrap();
        // "2" shifted into the next block when a video went up in between.
        append_channel_listing(&pool, false, "ch", &[vid("2"), vid("3")], &[("3".into(), 50)]).await.unwrap();
        let ids: Vec<String> = get_channel_cache(&pool, "ch").await.unwrap().into_iter().map(|v| v.video_id).collect();
        assert_eq!(ids, ["1", "2", "3"]);
    }

    #[tokio::test]
    async fn latest_across_takes_each_channels_newest_first() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        let listing = |ids: &[&str]| {
            ids.iter()
                .map(|id| ChannelVid {
                    video_id: id.to_string(),
                    title: id.to_string(),
                    channel: String::new(),
                    meta: String::new(),
                    info: String::new(),
                    thumb_path: String::new(),
                    duration: 1,
                })
                .collect::<Vec<_>>()
        };
        set_channel_cache(&pool, "a", &listing(&["a1", "a2", "a3"])).await.unwrap();
        set_channel_cache(&pool, "b", &listing(&["b1", "b2"])).await.unwrap();
        set_channel_cache(&pool, "c", &listing(&["c1"])).await.unwrap();
        let ids = |v: Vec<FeedVid>| v.into_iter().map(|f| f.video_id).collect::<Vec<_>>();
        let order = ["b".to_string(), "a".to_string()];
        assert_eq!(ids(latest_across(&pool, &order, 4).await.unwrap()), ["b1", "a1", "b2", "a2"]);
        assert!(latest_across(&pool, &[], 12).await.unwrap().is_empty());

        // Dated rows go by date, ahead of undated ones.
        set_listing_dates(&pool, "a", &[("a1".into(), 100), ("a2".into(), 300)]).await.unwrap();
        set_listing_dates(&pool, "b", &[("b1".into(), 200)]).await.unwrap();
        assert_eq!(ids(latest_across(&pool, &order, 4).await.unwrap()), ["a2", "b1", "a1", "b2"]);
    }
}
