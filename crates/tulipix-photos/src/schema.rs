//! Photos-specific schema overlay on top of the shared `items` proxy table.
//!
//! `items` (in tulipix-core) stays the source of truth for "this file exists";
//! these tables hang off it by `item_id` so AI tags, EXIF, faces, and edits
//! never get clobbered when the file moves on disk.

use anyhow::Result;
use sqlx::SqlitePool;

pub const PHOTOS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS photo_meta (
    item_id        INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    taken_at       INTEGER,
    camera_make    TEXT,
    camera_model   TEXT,
    lens           TEXT,
    iso            INTEGER,
    f_number       REAL,
    exposure_s     REAL,
    focal_mm       REAL,
    gps_lat        REAL,
    gps_lon        REAL,
    orientation    INTEGER,
    width          INTEGER,
    height         INTEGER,
    phash          TEXT,
    starred        INTEGER NOT NULL DEFAULT 0,
    archived       INTEGER NOT NULL DEFAULT 0,
    deleted_at     INTEGER,
    purge_after    INTEGER,
    pano           INTEGER NOT NULL DEFAULT 0,
    spatial        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS photo_meta_taken_idx     ON photo_meta(taken_at);
CREATE INDEX IF NOT EXISTS photo_meta_starred_idx   ON photo_meta(starred);
CREATE INDEX IF NOT EXISTS photo_meta_archived_idx  ON photo_meta(archived);
CREATE INDEX IF NOT EXISTS photo_meta_deleted_idx   ON photo_meta(deleted_at);
CREATE INDEX IF NOT EXISTS photo_meta_phash_idx     ON photo_meta(phash);

CREATE TABLE IF NOT EXISTS albums (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    name       TEXT    NOT NULL,
    smart_rule TEXT,            -- JSON expression; NULL = manual album
    cover_id   INTEGER,         -- item_id of cover photo
    rating     INTEGER NOT NULL DEFAULT 0,  -- 0-5; 0 = unrated
    created    INTEGER NOT NULL,
    updated    INTEGER NOT NULL,
    UNIQUE(name)
);

CREATE TABLE IF NOT EXISTS album_items (
    album_id INTEGER NOT NULL REFERENCES albums(id) ON DELETE CASCADE,
    item_id  INTEGER NOT NULL REFERENCES items(id)  ON DELETE CASCADE,
    sort_key INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (album_id, item_id)
);
CREATE INDEX IF NOT EXISTS album_items_item_idx ON album_items(item_id);

CREATE VIRTUAL TABLE IF NOT EXISTS photo_fts USING fts5(
    item_id    UNINDEXED,
    filename,
    camera,
    tags,
    people,
    notes,
    tokenize = 'unicode61 remove_diacritics 2'
);

CREATE TABLE IF NOT EXISTS people (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    name        TEXT,             -- NULL until user names cluster
    cover_face  INTEGER,          -- face_id used as avatar
    created     INTEGER NOT NULL,
    updated     INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS faces (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    item_id      INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    person_id    INTEGER REFERENCES people(id) ON DELETE SET NULL,
    bbox_x       INTEGER NOT NULL,
    bbox_y       INTEGER NOT NULL,
    bbox_w       INTEGER NOT NULL,
    bbox_h       INTEGER NOT NULL,
    embedding    BLOB,             -- 512-d fp32 from arcface, optional
    confidence   REAL,
    crop_path    TEXT,             -- relative path under face_thumbs/
    confirmed    INTEGER NOT NULL DEFAULT 0,
    created      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS faces_item_idx   ON faces(item_id);
CREATE INDEX IF NOT EXISTS faces_person_idx ON faces(person_id);

CREATE TABLE IF NOT EXISTS tags (
    id   INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT    NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS item_tags (
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    tag_id     INTEGER NOT NULL REFERENCES tags(id)  ON DELETE CASCADE,
    confidence REAL    NOT NULL DEFAULT 1.0,
    source     TEXT    NOT NULL DEFAULT 'user', -- 'user' | a Tagger::source() | 'clip'
    PRIMARY KEY (item_id, tag_id)
);
CREATE INDEX IF NOT EXISTS item_tags_tag_idx ON item_tags(tag_id);

CREATE TABLE IF NOT EXISTS dedup_clusters (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    kind       TEXT NOT NULL,  -- 'sha256' | 'phash'
    key        TEXT NOT NULL,
    created    INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS dedup_members (
    cluster_id INTEGER NOT NULL REFERENCES dedup_clusters(id) ON DELETE CASCADE,
    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    PRIMARY KEY (cluster_id, item_id)
);

CREATE TABLE IF NOT EXISTS photo_edits (
    item_id        INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    ops            TEXT    NOT NULL DEFAULT '[]', -- JSON array of EditOp
    undo_idx       INTEGER NOT NULL DEFAULT 0,    -- 0 = no ops applied
    rendered_path  TEXT,                          -- cached preview render
    updated        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS clip_embeddings (
    item_id  INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    model    TEXT NOT NULL,
    vec      BLOB NOT NULL,
    updated  INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS clip_model_idx ON clip_embeddings(model);

CREATE TABLE IF NOT EXISTS edit_clipboard (
    id      INTEGER PRIMARY KEY CHECK (id = 1),  -- singleton
    ops     TEXT NOT NULL,
    copied  INTEGER NOT NULL
);

-- What the background indexer has ALREADY TRIED for an item, per stage.
--
-- Deliberately records the attempt, not the result. "Pending" cannot be
-- "produced no rows": a photo with no faces in it produces no `faces` rows, a
-- photo of scenery produces no `item_tags`, and a screenshot has no EXIF date —
-- so a queue defined by missing results would hand back the same items forever
-- and the indexer would never finish, burning CPU every time the machine went
-- idle. A row here means "considered", which is what makes the queue drain.
--
-- `model` is the model name+version for the stages that use one, so bumping a
-- model can invalidate exactly its own stage by deleting those rows and nothing
-- else.
CREATE TABLE IF NOT EXISTS photo_ai_state (
    item_id  INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    stage    TEXT NOT NULL,
    done_at  INTEGER NOT NULL,
    model    TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (item_id, stage)
);
CREATE INDEX IF NOT EXISTS photo_ai_state_stage_idx ON photo_ai_state(stage);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(PHOTOS_SCHEMA).execute(pool).await?;
    // Tolerant migrations for in-place upgrades — duplicate-column errors are
    // expected when the schema already includes them.
    for stmt in [
        "ALTER TABLE photo_meta ADD COLUMN pano INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE photo_meta ADD COLUMN spatial INTEGER NOT NULL DEFAULT 0",
        "ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0",
    ] {
        let _ = sqlx::query(stmt).execute(pool).await;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::{apply_proxy_schema, DbHandle};

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("photos.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "photos".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply_proxy_schema(&pool, "photos").await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    /// Insert an item + photo_meta row; shared by the stacks / HDR tests.
    pub(crate) async fn seed_photo(pool: &SqlitePool, path: &str, taken: i64, cam: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id, taken_at, camera_make, camera_model) VALUES (?, ?, ?, ?)")
            .bind(id).bind(taken).bind("Tulip").bind(cam).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        // Re-running must not error.
        apply(&pool).await.unwrap();
        // sanity: each declared table is queryable.
        for t in ["photo_meta", "albums", "album_items", "photo_fts", "people", "faces", "tags", "item_tags", "dedup_clusters", "dedup_members", "photo_edits", "clip_embeddings", "edit_clipboard"] {
            let q = format!("SELECT COUNT(*) FROM {t}");
            let _: i64 = sqlx::query_scalar(sqlx::AssertSqlSafe(&*q)).fetch_one(&pool).await.unwrap();
        }
    }
}
