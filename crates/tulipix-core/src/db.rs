use crate::paths;
use anyhow::{Context, Result};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use sqlx::SqlitePool;
use std::path::PathBuf;
use std::str::FromStr;

pub const SECTIONS: &[&str] = &["photos", "videos", "music", "books", "cloud"];

/// Common per-section proxy schema. Stored once per section DB by `init_pool`.
///
/// Proxy model: every row references a real file by absolute path; the row
/// stores file identity (inode + size + mtime + optional content hash) so we
/// can detect renames/moves/deletes without ever copying bytes.
pub const PROXY_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS items (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    abs_path      TEXT    NOT NULL UNIQUE,
    inode         INTEGER NOT NULL,
    size          INTEGER NOT NULL,
    mtime         INTEGER NOT NULL,
    sha256        TEXT,
    section       TEXT    NOT NULL,
    missing_since INTEGER,
    added         INTEGER NOT NULL,
    updated       INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS items_section_idx ON items(section);
CREATE INDEX IF NOT EXISTS items_sha_idx     ON items(sha256);
CREATE INDEX IF NOT EXISTS items_inode_idx   ON items(inode, size);
CREATE INDEX IF NOT EXISTS items_missing_idx ON items(missing_since);

CREATE TABLE IF NOT EXISTS schema_migrations (
    id          INTEGER PRIMARY KEY,
    applied_at  INTEGER NOT NULL,
    description TEXT    NOT NULL
);
"#;

#[derive(Debug, Clone)]
pub struct DbHandle {
    pub section: String,
    pub path: PathBuf,
    pub url: String, // sqlite:///abs/path?mode=rwc
}

impl DbHandle {
    pub fn open(section: &str) -> Result<Self> {
        let path = paths::db_path(section).context("no data dir")?;
        if let Some(p) = path.parent() { std::fs::create_dir_all(p)?; }
        let url = format!("sqlite://{}?mode=rwc", path.display());
        Ok(Self { section: section.to_string(), path, url })
    }

    /// Build a WAL-mode pool with normal-sync, foreign-keys on. Idempotent.
    pub async fn pool(&self) -> Result<SqlitePool> {
        let opts = SqliteConnectOptions::from_str(&self.url)?
            .journal_mode(SqliteJournalMode::Wal)
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(std::time::Duration::from_secs(5))
            // Read-path tuning: memory-mapped I/O + a page cache cut the
            // syscalls/copies behind list queries (faster section loads), and
            // temp tables/indexes for sorts stay in RAM. cache_size is negative
            // = KiB; mmap_size is bytes (256 MiB ceiling, virtual — it does not
            // add to RSS, though it does inflate what `btop`/smaps report).
            //
            // The page cache is per CONNECTION, and there are ten of these pools
            // (one per section DB). At the old 16 MiB × 8 connections × 10 pools
            // the ceiling was 1.28 GB of anonymous memory living inside SQLite,
            // invisible to every Rust profiler. These queries are list reads over
            // tables of thousands of rows, not analytics — 4 MiB holds the hot
            // b-tree pages of any of them. Raise it for one section (not all ten)
            // if a specific library measurably regresses.
            .pragma("mmap_size", "268435456")
            .pragma("cache_size", "-4000")
            .pragma("temp_store", "MEMORY")
            .create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            // Writers serialise on the WAL lock anyway; the readers that
            // actually run concurrently are the section populate calls, and
            // there are never eight of those in flight against one DB.
            .max_connections(4)
            .acquire_timeout(std::time::Duration::from_secs(10))
            .connect_with(opts)
            .await
            .with_context(|| format!("open sqlite pool {}", self.path.display()))?;
        Ok(pool)
    }

    pub async fn init_pool(&self) -> Result<SqlitePool> {
        let pool = self.pool().await?;
        apply_proxy_schema(&pool, &self.section).await?;
        Ok(pool)
    }
}

pub async fn apply_proxy_schema(pool: &SqlitePool, section: &str) -> Result<()> {
    sqlx::query(PROXY_SCHEMA).execute(pool).await.context("create proxy schema")?;
    let applied: Option<i64> = sqlx::query_scalar("SELECT id FROM schema_migrations WHERE id = 1")
        .fetch_optional(pool)
        .await?;
    if applied.is_none() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        sqlx::query("INSERT INTO schema_migrations (id, applied_at, description) VALUES (?, ?, ?)")
            .bind(1i64)
            .bind(now)
            .bind(format!("{section}.proxy.v1"))
            .execute(pool)
            .await
            .context("record migration")?;
    }
    Ok(())
}

/// Open all per-section DBs (idempotent). Section crates (tulipix-photos, …)
/// extend the schema with their own metadata tables.
pub fn open_all_sections() -> Vec<Result<DbHandle>> {
    SECTIONS.iter().map(|s| DbHandle::open(s)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn pool_opens_in_wal_and_applies_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("t.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let handle = DbHandle { section: "photos".into(), path: path.clone(), url };
        let pool = handle.init_pool().await.unwrap();
        let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
        // re-init is idempotent
        let _ = handle.init_pool().await.unwrap();
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM schema_migrations")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(rows, 1);
    }
}
