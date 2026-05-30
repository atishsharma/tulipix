//! Music-specific schema overlay on top of the shared `items` proxy table.
//!
//! `items` (in tulipix-core) stays the source of truth for "this file exists";
//! `track_meta` hangs off it by `item_id`, while `artists`, `albums`,
//! `playlists`, `podcasts`, etc. carry their own identity. Nothing here ever
//! copies file bytes — analysis (BPM/key/DR), ratings, lyrics and embeddings
//! survive a file move because they key on `item_id`.

use anyhow::Result;
use sqlx::SqlitePool;

pub const MUSIC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS artists (
    id    INTEGER PRIMARY KEY AUTOINCREMENT,
    name  TEXT    NOT NULL UNIQUE,
    mbid  TEXT,
    bio   TEXT,
    image_path TEXT
);

CREATE TABLE IF NOT EXISTS albums (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    title      TEXT    NOT NULL,
    artist_id  INTEGER REFERENCES artists(id) ON DELETE SET NULL,
    year       INTEGER,
    mbid       TEXT,
    cover_path TEXT,
    UNIQUE(title, artist_id)
);
CREATE INDEX IF NOT EXISTS albums_artist_idx ON albums(artist_id);

CREATE TABLE IF NOT EXISTS track_meta (
    item_id        INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    title          TEXT,
    artist_id      INTEGER REFERENCES artists(id) ON DELETE SET NULL,
    album_id       INTEGER REFERENCES albums(id) ON DELETE SET NULL,
    album_artist   TEXT,
    genre          TEXT,
    year           INTEGER,
    track_no        INTEGER,
    disc_no        INTEGER,
    duration_s     REAL,
    bitrate        INTEGER,
    sample_rate    INTEGER,
    channels       INTEGER,
    codec          TEXT,
    container      TEXT,
    bpm            REAL,
    music_key      TEXT,
    dr_score       REAL,
    replaygain_track REAL,
    replaygain_album REAL,
    loved          INTEGER NOT NULL DEFAULT 0,
    rating         INTEGER NOT NULL DEFAULT 0,   -- 0..5
    play_count     INTEGER NOT NULL DEFAULT 0,
    last_played    INTEGER,
    folder         TEXT,
    mbid           TEXT,
    is_audiobook   INTEGER NOT NULL DEFAULT 0,
    is_stream      INTEGER NOT NULL DEFAULT 0,
    stream_url     TEXT,
    cache_path     TEXT,
    cache_bytes    INTEGER
);
CREATE INDEX IF NOT EXISTS track_meta_artist_idx ON track_meta(artist_id);
CREATE INDEX IF NOT EXISTS track_meta_album_idx  ON track_meta(album_id);
CREATE INDEX IF NOT EXISTS track_meta_genre_idx  ON track_meta(genre);
CREATE INDEX IF NOT EXISTS track_meta_year_idx   ON track_meta(year);
CREATE INDEX IF NOT EXISTS track_meta_loved_idx  ON track_meta(loved);
CREATE INDEX IF NOT EXISTS track_meta_rating_idx ON track_meta(rating);
CREATE INDEX IF NOT EXISTS track_meta_dr_idx     ON track_meta(dr_score);
CREATE INDEX IF NOT EXISTS track_meta_played_idx ON track_meta(last_played);
CREATE INDEX IF NOT EXISTS track_meta_folder_idx ON track_meta(folder);
CREATE INDEX IF NOT EXISTS track_meta_cache_idx  ON track_meta(cache_path);

CREATE TABLE IF NOT EXISTS playlists (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT    NOT NULL,
    is_smart  INTEGER NOT NULL DEFAULT 0,
    rule_json TEXT,
    created   INTEGER NOT NULL,
    updated   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS playlist_items (
    playlist_id INTEGER NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
    item_id     INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    position    INTEGER NOT NULL,
    PRIMARY KEY (playlist_id, position)
);
CREATE INDEX IF NOT EXISTS playlist_items_item_idx ON playlist_items(item_id);

CREATE TABLE IF NOT EXISTS lyrics (
    item_id  INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    synced   INTEGER NOT NULL DEFAULT 0,   -- 1 = LRC timestamps present
    content  TEXT NOT NULL,
    source   TEXT,
    updated  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS play_history (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id   INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    played_at INTEGER NOT NULL,
    ms_played INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS play_history_item_idx ON play_history(item_id);
CREATE INDEX IF NOT EXISTS play_history_at_idx   ON play_history(played_at DESC);

CREATE TABLE IF NOT EXISTS track_embeddings (
    item_id INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    dim     INTEGER NOT NULL,
    model   TEXT    NOT NULL,
    vec     BLOB    NOT NULL
);

CREATE TABLE IF NOT EXISTS play_queue (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    position INTEGER NOT NULL UNIQUE,
    source   TEXT,
    added    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audiobook_progress (
    item_id    INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    position_s REAL    NOT NULL DEFAULT 0,
    speed      REAL    NOT NULL DEFAULT 1.0,
    finished   INTEGER NOT NULL DEFAULT 0,
    updated    INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS audiobook_bookmarks (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    position_s REAL    NOT NULL,
    label      TEXT,
    created    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS audiobook_bookmarks_item_idx ON audiobook_bookmarks(item_id);

CREATE TABLE IF NOT EXISTS podcasts (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    feed_url     TEXT    NOT NULL UNIQUE,
    title        TEXT,
    author       TEXT,
    image_url    TEXT,
    last_checked INTEGER
);

CREATE TABLE IF NOT EXISTS podcast_episodes (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    podcast_id      INTEGER NOT NULL REFERENCES podcasts(id) ON DELETE CASCADE,
    guid            TEXT    NOT NULL,
    title           TEXT,
    audio_url       TEXT    NOT NULL,
    published       INTEGER,
    duration_s      REAL,
    downloaded_path TEXT,
    position_s      REAL    NOT NULL DEFAULT 0,
    played          INTEGER NOT NULL DEFAULT 0,
    UNIQUE(podcast_id, guid)
);
CREATE INDEX IF NOT EXISTS podcast_episodes_pod_idx ON podcast_episodes(podcast_id, published DESC);

CREATE TABLE IF NOT EXISTS radio_stations (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    station_uuid TEXT UNIQUE,
    name         TEXT NOT NULL,
    url          TEXT NOT NULL,
    favicon      TEXT,
    country      TEXT,
    tags         TEXT,
    favourite    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS radio_fav_idx ON radio_stations(favourite);

CREATE TABLE IF NOT EXISTS scrobble_queue (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id   INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    service   TEXT    NOT NULL,           -- 'lastfm' | 'listenbrainz'
    played_at INTEGER NOT NULL,
    submitted INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS scrobble_pending_idx ON scrobble_queue(service, submitted);

CREATE TABLE IF NOT EXISTS music_video_link (
    item_id       INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    video_item_id INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS music_video_link_video_idx ON music_video_link(video_item_id);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(MUSIC_SCHEMA).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::{apply_proxy_schema, DbHandle};

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("music.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "music".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply_proxy_schema(&pool, "music").await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    /// Insert a bare `items` row + `track_meta` and return the item id.
    pub(crate) async fn add_track(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'music', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO track_meta (item_id) VALUES (?)").bind(id).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM track_meta").fetch_one(&pool).await.unwrap();
    }
}
