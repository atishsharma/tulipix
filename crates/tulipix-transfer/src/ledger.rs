//! Recent Transfers, persisted. A transfer is an event, not a file, so this
//! does not use the shared proxy schema.

use anyhow::Result;
use sqlx::SqlitePool;

/// Rows per page in the Recent Transfers list. The pagination test derives its
/// expectations from this rather than repeating a literal.
pub const PAGE_SIZE: usize = 50;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS transfers (
    id        INTEGER PRIMARY KEY AUTOINCREMENT,
    direction TEXT    NOT NULL,
    name      TEXT    NOT NULL,
    abs_path  TEXT,
    bytes     INTEGER NOT NULL,
    peer      TEXT    NOT NULL,
    status    TEXT    NOT NULL,
    at        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS transfers_at_idx ON transfers(at DESC);

CREATE TABLE IF NOT EXISTS devices (
    token     TEXT    PRIMARY KEY,
    label     TEXT    NOT NULL,
    issued    INTEGER NOT NULL,
    last_seen INTEGER NOT NULL,
    expires   INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// Opens `transfers.db` next to the section databases, inheriting WAL mode and
/// the pool settings from the shared handle.
pub async fn open() -> Result<SqlitePool> {
    let handle = tulipix_core::db::DbHandle::open("transfers")?;
    let pool = handle.pool().await?;
    apply_schema(&pool).await?;
    Ok(pool)
}

#[derive(Clone, Debug)]
pub struct Row {
    pub direction: &'static str,
    pub name: String,
    pub abs_path: Option<String>,
    pub bytes: i64,
    pub peer: String,
    pub status: &'static str,
}

impl Row {
    pub fn sent(name: &str, bytes: i64, peer: &str) -> Self {
        Self {
            direction: "out",
            name: name.into(),
            abs_path: None,
            bytes,
            peer: peer.into(),
            status: "ok",
        }
    }

    pub fn received(name: &str, path: &str, bytes: i64, peer: &str) -> Self {
        Self {
            direction: "in",
            name: name.into(),
            abs_path: Some(path.into()),
            bytes,
            peer: peer.into(),
            status: "ok",
        }
    }

    pub fn failed(mut self) -> Self {
        self.status = "failed";
        self
    }
}

pub async fn record(pool: &SqlitePool, row: Row, at: i64) -> Result<()> {
    sqlx::query(
        "INSERT INTO transfers (direction, name, abs_path, bytes, peer, status, at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(row.direction)
    .bind(&row.name)
    .bind(&row.abs_path)
    .bind(row.bytes)
    .bind(&row.peer)
    .bind(row.status)
    .bind(at)
    .execute(pool)
    .await?;
    Ok(())
}

pub struct Entry {
    pub name: String,
    pub abs_path: Option<String>,
    pub direction: String,
    pub bytes: i64,
    pub peer: String,
    pub status: String,
    pub at: i64,
}

pub async fn recent(pool: &SqlitePool, page: usize) -> Result<Vec<Entry>> {
    let rows = sqlx::query_as::<_, (String, Option<String>, String, i64, String, String, i64)>(
        "SELECT direction, abs_path, name, bytes, peer, status, at
         FROM transfers ORDER BY at DESC, id DESC LIMIT ? OFFSET ?",
    )
    .bind(PAGE_SIZE as i64)
    .bind((page * PAGE_SIZE) as i64)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(direction, abs_path, name, bytes, peer, status, at)| Entry {
            direction,
            abs_path,
            name,
            bytes,
            peer,
            status,
            at,
        })
        .collect())
}

pub async fn count(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM transfers").fetch_one(pool).await?)
}

pub async fn clear_transfers(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM transfers").execute(pool).await?;
    Ok(())
}

pub async fn remember_device(
    pool: &SqlitePool,
    token: &str,
    label: &str,
    now: i64,
    expires: i64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO devices (token, label, issued, last_seen, expires) VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(token) DO UPDATE SET last_seen = excluded.last_seen",
    )
    .bind(token)
    .bind(label)
    .bind(now)
    .bind(now)
    .bind(expires)
    .execute(pool)
    .await?;
    Ok(())
}

/// Only devices whose token has not expired. Expired rows are left in place for
/// `forget_expired` to sweep, so a phone that reappears at hour 49 is told to
/// re-pair rather than being silently unknown.
pub async fn devices(pool: &SqlitePool, now: i64) -> Result<Vec<(String, String, i64, i64)>> {
    let rows = sqlx::query_as::<_, (String, String, i64, i64)>(
        "SELECT token, label, last_seen, expires FROM devices WHERE expires > ? ORDER BY last_seen DESC",
    )
    .bind(now)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

pub async fn forget_device(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("DELETE FROM devices WHERE token = ?").bind(token).execute(pool).await?;
    Ok(())
}

pub async fn forget_expired(pool: &SqlitePool, now: i64) -> Result<()> {
    sqlx::query("DELETE FROM devices WHERE expires <= ?").bind(now).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_pool() -> sqlx::SqlitePool {
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        apply_schema(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn rows_come_back_newest_first() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("old.mp3", 10, "1.2.3.4"), 100).await.unwrap();
        record(&pool, Row::sent("new.mp3", 20, "1.2.3.4"), 200).await.unwrap();

        let page = recent(&pool, 0).await.unwrap();
        assert_eq!(
            page.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            ["new.mp3", "old.mp3"]
        );
    }

    #[tokio::test]
    async fn pagination_is_derived_from_the_page_size_constant() {
        let pool = mem_pool().await;
        for n in 0..(PAGE_SIZE + 5) {
            record(&pool, Row::sent(&format!("f{n}"), 1, "1.2.3.4"), 100 + n as i64).await.unwrap();
        }
        assert_eq!(recent(&pool, 0).await.unwrap().len(), PAGE_SIZE);
        assert_eq!(recent(&pool, 1).await.unwrap().len(), 5);
    }

    #[tokio::test]
    async fn clearing_transfers_leaves_paired_devices_alone() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("a", 1, "1.2.3.4"), 100).await.unwrap();
        remember_device(&pool, "tok", "Android · Chrome", 100, 100 + 48 * 3600).await.unwrap();

        clear_transfers(&pool).await.unwrap();

        assert!(recent(&pool, 0).await.unwrap().is_empty());
        assert_eq!(devices(&pool, 200).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn an_expired_device_is_not_listed() {
        let pool = mem_pool().await;
        remember_device(&pool, "tok", "Android · Chrome", 100, 100 + 48 * 3600).await.unwrap();
        assert_eq!(devices(&pool, 100 + 47 * 3600).await.unwrap().len(), 1);
        assert_eq!(devices(&pool, 100 + 49 * 3600).await.unwrap().len(), 0);
    }

    #[tokio::test]
    async fn a_received_row_keeps_the_path_it_landed_at() {
        let pool = mem_pool().await;
        record(&pool, Row::received("a.mp3", "/inbox/a.mp3", 5, "1.2.3.4"), 100).await.unwrap();
        let page = recent(&pool, 0).await.unwrap();
        assert_eq!(page[0].direction, "in");
        assert_eq!(page[0].abs_path.as_deref(), Some("/inbox/a.mp3"));
    }

    #[tokio::test]
    async fn a_failed_transfer_is_recorded_as_failed() {
        let pool = mem_pool().await;
        record(&pool, Row::sent("x", 1, "1.2.3.4").failed(), 100).await.unwrap();
        assert_eq!(recent(&pool, 0).await.unwrap()[0].status, "failed");
    }
}
