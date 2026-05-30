//! Local `sync.db` mirror of Account-mode state. The Go backend is the source
//! of truth; these tables cache invitations, activity, shared albums/members,
//! TOTP enrollment, audit events, and the sync watermark so the client renders
//! and queues offline without a round-trip.

use anyhow::Result;
use sqlx::SqlitePool;

pub const SYNC_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sync_state (
    key   TEXT PRIMARY KEY,
    value TEXT
);

CREATE TABLE IF NOT EXISTS invitations (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    email     TEXT    NOT NULL,
    role      TEXT    NOT NULL DEFAULT 'member',  -- 'admin' | 'member' | 'viewer'
    token     TEXT    NOT NULL UNIQUE,
    state     TEXT    NOT NULL DEFAULT 'pending', -- 'pending'|'accepted'|'revoked'|'expired'
    invited_by TEXT,
    created   INTEGER NOT NULL,
    expires   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS invitations_state_idx ON invitations(state);

CREATE TABLE IF NOT EXISTS activity (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    actor     TEXT    NOT NULL,
    verb      TEXT    NOT NULL,            -- 'uploaded' | 'shared' | 'commented' | ...
    object    TEXT,
    at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS activity_at_idx ON activity(at DESC);

CREATE TABLE IF NOT EXISTS shared_albums (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    name     TEXT    NOT NULL,
    owner    TEXT    NOT NULL,
    created  INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS shared_album_members (
    album_id INTEGER NOT NULL REFERENCES shared_albums(id) ON DELETE CASCADE,
    member   TEXT    NOT NULL,
    role     TEXT    NOT NULL DEFAULT 'viewer', -- 'editor' | 'viewer'
    PRIMARY KEY (album_id, member)
);

CREATE TABLE IF NOT EXISTS totp_enrollment (
    user_id      TEXT PRIMARY KEY,
    secret_b32   TEXT NOT NULL,
    confirmed    INTEGER NOT NULL DEFAULT 0,
    algorithm    TEXT NOT NULL DEFAULT 'SHA256',
    digits       INTEGER NOT NULL DEFAULT 6,
    period_s     INTEGER NOT NULL DEFAULT 30
);

CREATE TABLE IF NOT EXISTS totp_backup_codes (
    id       INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id  TEXT NOT NULL,
    code_hash TEXT NOT NULL,
    used     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS backup_codes_user_idx ON totp_backup_codes(user_id, used);

CREATE TABLE IF NOT EXISTS audit_log (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    actor   TEXT    NOT NULL,
    event   TEXT    NOT NULL,   -- 'invite'|'role-change'|'share'|'delete'|'tier-flip'|'config'
    target  TEXT,
    detail  TEXT,
    at      INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS audit_event_idx ON audit_log(event);
CREATE INDEX IF NOT EXISTS audit_at_idx    ON audit_log(at DESC);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SYNC_SCHEMA).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::DbHandle;

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("sync.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "sync".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM invitations").fetch_one(&pool).await.unwrap();
    }
}
