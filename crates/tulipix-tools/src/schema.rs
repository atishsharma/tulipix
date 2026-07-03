//! `tools.db` schema — the shared job queue both the GUI and CLI submit to.
//!
//! A `jobs` row captures one Tools operation: its kind, the JSON spec, state,
//! progress, and worker assignment. Persisting here means a queued/running job
//! survives relaunch and the CLI's `--queue` jobs are visible in the GUI.

use anyhow::Result;
use sqlx::SqlitePool;

pub const TOOLS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS jobs (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    kind      TEXT    NOT NULL,            -- 'rename' | 'compress_video' | ...
    spec_json TEXT    NOT NULL,            -- operation parameters
    state     TEXT    NOT NULL DEFAULT 'queued', -- queued|running|paused|done|error|canceled
    progress  REAL    NOT NULL DEFAULT 0.0,      -- 0..1
    message   TEXT,
    attempts  INTEGER NOT NULL DEFAULT 0,
    priority  INTEGER NOT NULL DEFAULT 0,
    created   INTEGER NOT NULL,
    updated   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_state_idx ON jobs(state, priority DESC, id);

CREATE TABLE IF NOT EXISTS queue_settings (
    key   TEXT PRIMARY KEY,
    value TEXT
);

-- Files the download tools actually wrote (the "Downloaded" list shows ONLY
-- these — not everything that happens to sit in ~/Downloads).
CREATE TABLE IF NOT EXISTS downloads (
    id      INTEGER PRIMARY KEY AUTOINCREMENT,
    job_id  INTEGER,
    path    TEXT    NOT NULL UNIQUE,
    created INTEGER NOT NULL
);
"#;

pub async fn apply(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(TOOLS_SCHEMA).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use tulipix_core::db::DbHandle;

    pub(crate) async fn open_pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("tools.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "tools".into(), path, url };
        let pool = h.pool().await.unwrap();
        apply(&pool).await.unwrap();
        (tmp, pool)
    }

    #[tokio::test]
    async fn schema_applies_idempotently() {
        let (_t, pool) = open_pool().await;
        apply(&pool).await.unwrap();
        let _: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs").fetch_one(&pool).await.unwrap();
    }
}
