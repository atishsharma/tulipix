//! Videos-specific schema overlay on top of the shared `items` proxy table.

use anyhow::Result;
use sqlx::SqlitePool;

pub const VIDEOS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS video_meta (
    item_id         INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    duration_s      REAL,
    container       TEXT,
    video_codec     TEXT,
    audio_codec     TEXT,
    width           INTEGER,
    height          INTEGER,
    fps             REAL,
    bitrate         INTEGER,
    hdr             TEXT,            -- 'sdr' | 'hdr10' | 'hdr10+' | 'dolby_vision'
    color_primaries TEXT,
    color_transfer  TEXT,
    audio_channels  INTEGER,
    audio_sample_hz INTEGER,
    starred         INTEGER NOT NULL DEFAULT 0,
    archived        INTEGER NOT NULL DEFAULT 0,
    deleted_at      INTEGER,
    last_accessed   INTEGER
);
CREATE INDEX IF NOT EXISTS video_meta_starred_idx       ON video_meta(starred);
CREATE INDEX IF NOT EXISTS video_meta_archived_idx      ON video_meta(archived);
CREATE INDEX IF NOT EXISTS video_meta_last_accessed_idx ON video_meta(last_accessed);
CREATE INDEX IF NOT EXISTS video_meta_duration_idx      ON video_meta(duration_s);

CREATE TABLE IF NOT EXISTS shows (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    tmdb_id      INTEGER UNIQUE,
    tvdb_id      INTEGER UNIQUE,
    title        TEXT    NOT NULL,
    year         INTEGER,
    overview     TEXT,
    poster_path  TEXT,
    backdrop_path TEXT,
    updated      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS movies (
    item_id      INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    tmdb_id      INTEGER,
    title        TEXT    NOT NULL,
    year         INTEGER,
    overview     TEXT,
    poster_path  TEXT,
    backdrop_path TEXT,
    runtime_min  INTEGER,
    updated      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS movies_tmdb_idx ON movies(tmdb_id);

CREATE TABLE IF NOT EXISTS episodes (
    item_id     INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    show_id     INTEGER NOT NULL REFERENCES shows(id) ON DELETE CASCADE,
    season      INTEGER NOT NULL,
    episode     INTEGER NOT NULL,
    title       TEXT,
    overview    TEXT,
    air_date    INTEGER,
    still_path  TEXT,
    runtime_min INTEGER,
    updated     INTEGER NOT NULL,
    UNIQUE(show_id, season, episode)
);
CREATE INDEX IF NOT EXISTS episodes_show_idx ON episodes(show_id, season, episode);

CREATE TABLE IF NOT EXISTS watch_progress (
    item_id    INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    position_s REAL    NOT NULL,
    duration_s REAL,
    finished   INTEGER NOT NULL DEFAULT 0,
    updated    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS watch_progress_updated_idx ON watch_progress(updated DESC);

CREATE TABLE IF NOT EXISTS chapters (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    idx      INTEGER NOT NULL,
    title    TEXT,
    start_s  REAL    NOT NULL,
    end_s    REAL,
    UNIQUE(item_id, idx)
);
CREATE INDEX IF NOT EXISTS chapters_item_idx ON chapters(item_id);

CREATE VIRTUAL TABLE IF NOT EXISTS sub_words USING fts5(
    item_id  UNINDEXED,
    start_ms UNINDEXED,
    end_ms   UNINDEXED,
    word,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(VIDEOS_SCHEMA).execute(pool).await?;
    crate::discover::apply_schema(pool).await?;
    crate::stream::cache::apply_schema(pool).await?;
    crate::stream::bookmarks::apply_schema(pool).await?;
    crate::stream::progress::apply_schema(pool).await?;
    crate::stream::feed_cache::apply_schema(pool).await?;
    crate::stream::downloads::apply_schema(pool).await?;
    // Idempotent migrations — SQLite ignores duplicate-column errors.
    let _ = sqlx::query("ALTER TABLE movies ADD COLUMN poster_local TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE shows  ADD COLUMN poster_local TEXT").execute(pool).await;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::{apply_proxy_schema, DbHandle};

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("videos.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "videos".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply_proxy_schema(&pool, "videos").await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM video_meta").fetch_one(&pool).await.unwrap();
    }
}
