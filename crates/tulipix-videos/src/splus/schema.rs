//! Stream Plus tables. Separate from `stream_*` on purpose: the Stream tab is
//! not modified by this feature, so nothing here alters a table it owns.

use anyhow::Result;
use sqlx::SqlitePool;

pub const SCHEMA: &str = r#"
-- Which source answered, how fast, and how badly it is doing. Drives both the
-- failover order and the health chips in Settings.
CREATE TABLE IF NOT EXISTS splus_source_health (
    source       TEXT    PRIMARY KEY,
    last_ok      INTEGER NOT NULL DEFAULT 0,
    last_fail    INTEGER NOT NULL DEFAULT 0,
    fail_streak  INTEGER NOT NULL DEFAULT 0,
    last_ms      INTEGER NOT NULL DEFAULT 0,
    note         TEXT    NOT NULL DEFAULT ''
);

-- Saved titles.
CREATE TABLE IF NOT EXISTS splus_bookmarks (
    key         TEXT    PRIMARY KEY,
    anilist_id  INTEGER,
    tmdb_id     INTEGER,
    title       TEXT    NOT NULL,
    english     TEXT    NOT NULL DEFAULT '',
    year        INTEGER,
    cover_url   TEXT    NOT NULL DEFAULT '',
    overview    TEXT    NOT NULL DEFAULT '',
    format      TEXT    NOT NULL DEFAULT 'TV',
    episodes    INTEGER NOT NULL DEFAULT 0,
    certification TEXT  NOT NULL DEFAULT '',
    -- What the release-poll last saw, so "new episode" fires once per episode.
    seen_ep     INTEGER NOT NULL DEFAULT 0,
    latest_ep   INTEGER NOT NULL DEFAULT 0,
    checked_at  INTEGER NOT NULL DEFAULT 0,
    added_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS splus_bookmarks_added_idx ON splus_bookmarks(added_at);

-- Where playback got to, one row per episode.
CREATE TABLE IF NOT EXISTS splus_progress (
    key          TEXT    PRIMARY KEY,
    title_key    TEXT    NOT NULL,
    title        TEXT    NOT NULL,
    label        TEXT    NOT NULL DEFAULT '',
    season       INTEGER NOT NULL DEFAULT 1,
    episode      INTEGER NOT NULL DEFAULT 0,
    audio        TEXT    NOT NULL DEFAULT 'sub',
    source       TEXT    NOT NULL DEFAULT '',
    quality      TEXT    NOT NULL DEFAULT '',
    cover_url    TEXT    NOT NULL DEFAULT '',
    position_s   REAL    NOT NULL DEFAULT 0,
    duration_s   REAL    NOT NULL DEFAULT 0,
    finished     INTEGER NOT NULL DEFAULT 0,
    played_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS splus_progress_played_idx ON splus_progress(played_at);
CREATE INDEX IF NOT EXISTS splus_progress_title_idx  ON splus_progress(title_key);

-- The download queue. Survives a restart, which is the whole reason it is a
-- table and not a Vec.
CREATE TABLE IF NOT EXISTS splus_downloads (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    key         TEXT    NOT NULL,
    title       TEXT    NOT NULL,
    label       TEXT    NOT NULL DEFAULT '',
    quality     TEXT    NOT NULL DEFAULT '',
    source      TEXT    NOT NULL DEFAULT '',
    url         TEXT    NOT NULL,
    kind        TEXT    NOT NULL DEFAULT 'mp4',   -- mp4 | m3u8
    dest        TEXT    NOT NULL DEFAULT '',
    sub_url     TEXT    NOT NULL DEFAULT '',
    -- waiting | running | done | failed | missing
    state       TEXT    NOT NULL DEFAULT 'waiting',
    detail      TEXT    NOT NULL DEFAULT '',
    done_bytes  INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER NOT NULL DEFAULT 0,
    -- Set together so a batch can be cancelled or reported as one thing.
    batch       TEXT    NOT NULL DEFAULT '',
    added_at    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS splus_downloads_state_idx ON splus_downloads(state);
CREATE INDEX IF NOT EXISTS splus_downloads_batch_idx ON splus_downloads(batch);

-- TMDB episode-group corrections, filled on demand for the shows that need one.
CREATE TABLE IF NOT EXISTS splus_episode_group (
    tmdb_id       INTEGER NOT NULL,
    season        INTEGER NOT NULL,
    episode       INTEGER NOT NULL,
    mapped_season INTEGER NOT NULL,
    mapped_ep     INTEGER NOT NULL,
    PRIMARY KEY (tmdb_id, season, episode)
);

-- What the "new episode of a saved show" check compares against, so a poll that
-- finds nothing new costs one row read.
CREATE TABLE IF NOT EXISTS splus_release_cache (
    anilist_id INTEGER PRIMARY KEY,
    next_ep    INTEGER NOT NULL DEFAULT 0,
    airs_at    INTEGER NOT NULL DEFAULT 0,
    checked_at INTEGER NOT NULL DEFAULT 0
);

-- Search terms, newest first, for the Recent row.
CREATE TABLE IF NOT EXISTS splus_recent (
    term       TEXT PRIMARY KEY,
    used_at    INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}
