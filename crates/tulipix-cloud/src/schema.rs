//! Cloud-specific schema. Unlike the media sections, cloud rows describe
//! *remote* objects (rclone remotes, revisions, shares) rather than local
//! files, so most tables stand alone rather than hanging off `items`. The
//! proxy `items` table is still applied for parity (downloaded/cached files).

use anyhow::Result;
use sqlx::SqlitePool;

pub const CLOUD_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS remotes (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    name      TEXT    NOT NULL UNIQUE,     -- rclone remote name
    backend   TEXT    NOT NULL,            -- 'drive' | 's3' | 'dropbox' | 'union' | ...
    encrypted INTEGER NOT NULL DEFAULT 0,  -- config encrypted at rest
    created   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS mounts (
    remote_id     INTEGER PRIMARY KEY REFERENCES remotes(id) ON DELETE CASCADE,
    mount_path    TEXT    NOT NULL,
    fs_kind       TEXT    NOT NULL,        -- 'fuse' | 'winfsp' | 'http'
    auto_remount  INTEGER NOT NULL DEFAULT 1,
    status        TEXT    NOT NULL DEFAULT 'unmounted'  -- 'mounted' | 'unmounted' | 'error'
);

CREATE TABLE IF NOT EXISTS selective_sync (
    remote_id   INTEGER NOT NULL REFERENCES remotes(id) ON DELETE CASCADE,
    folder_path TEXT    NOT NULL,
    mode        TEXT    NOT NULL,          -- 'mount' | 'full' | 'mirror_up' | 'ignore'
    PRIMARY KEY (remote_id, folder_path)
);

CREATE TABLE IF NOT EXISTS revisions (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_id   INTEGER NOT NULL REFERENCES remotes(id) ON DELETE CASCADE,
    remote_path TEXT    NOT NULL,
    revision    TEXT    NOT NULL,          -- backend revision id / version tag
    size        INTEGER,
    modified    INTEGER,
    is_current  INTEGER NOT NULL DEFAULT 0,
    UNIQUE(remote_id, remote_path, revision)
);
CREATE INDEX IF NOT EXISTS revisions_path_idx ON revisions(remote_id, remote_path);

CREATE TABLE IF NOT EXISTS recycle_bin (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_id   INTEGER NOT NULL REFERENCES remotes(id) ON DELETE CASCADE,
    remote_path TEXT    NOT NULL,
    trashed_at  INTEGER NOT NULL,
    purge_after INTEGER NOT NULL,
    restored    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS recycle_purge_idx ON recycle_bin(purge_after);

CREATE TABLE IF NOT EXISTS shares (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_id   INTEGER NOT NULL REFERENCES remotes(id) ON DELETE CASCADE,
    remote_path TEXT    NOT NULL,
    url         TEXT,
    created     INTEGER NOT NULL,
    revoked     INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS snapshots (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_id INTEGER NOT NULL REFERENCES remotes(id) ON DELETE CASCADE,
    taken_at  INTEGER NOT NULL,
    manifest  TEXT    NOT NULL             -- path to local snapshot manifest dir
);
CREATE INDEX IF NOT EXISTS snapshots_time_idx ON snapshots(remote_id, taken_at DESC);

CREATE TABLE IF NOT EXISTS uploads (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    remote_id   INTEGER NOT NULL REFERENCES remotes(id) ON DELETE CASCADE,
    src_path    TEXT    NOT NULL,
    dest_path   TEXT    NOT NULL,
    bytes_total INTEGER NOT NULL DEFAULT 0,
    bytes_done  INTEGER NOT NULL DEFAULT 0,
    state       TEXT    NOT NULL DEFAULT 'queued'  -- 'queued' | 'running' | 'done' | 'error'
);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(CLOUD_SCHEMA).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::{apply_proxy_schema, DbHandle};

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("cloud.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "cloud".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply_proxy_schema(&pool, "cloud").await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    /// Insert a remote and return its id.
    pub(crate) async fn add_remote(pool: &SqlitePool, name: &str, backend: &str) -> i64 {
        sqlx::query_scalar("INSERT INTO remotes (name, backend, created) VALUES (?,?,0) RETURNING id")
            .bind(name).bind(backend).fetch_one(pool).await.unwrap()
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM remotes").fetch_one(&pool).await.unwrap();
    }
}
