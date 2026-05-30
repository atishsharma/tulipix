//! `np.p4.sync.engine` — batch upsert + watermark pull every 60s.
//!
//! The engine pushes local changes in batches and pulls server changes since a
//! monotonic `watermark` cursor every 60 s. This owns the watermark persistence
//! (in `sync_state`), the pull-due timer, and batching of the local outbox into
//! fixed-size upsert payloads.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

pub const PULL_INTERVAL_S: i64 = 60;
pub const WATERMARK_KEY: &str = "sync_watermark";
pub const MAX_BATCH: usize = 500;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChangeRecord {
    pub section: String,
    pub item_id: i64,
    pub op: String,    // "upsert" | "delete"
    pub rev: i64,
}

/// Is a watermark pull due? (`last_pull + 60s ≤ now`.)
pub fn is_pull_due(last_pull_unix: i64, now_unix: i64) -> bool {
    now_unix >= last_pull_unix + PULL_INTERVAL_S
}

/// Chunk the outbox into upsert batches of at most [`MAX_BATCH`].
pub fn build_batches(outbox: &[ChangeRecord]) -> Vec<&[ChangeRecord]> {
    outbox.chunks(MAX_BATCH).collect()
}

pub async fn get_watermark(pool: &SqlitePool) -> Result<i64> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM sync_state WHERE key = ?")
        .bind(WATERMARK_KEY).fetch_optional(pool).await?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(0))
}

/// Advance the watermark; never moves backward (server cursor is monotonic).
pub async fn set_watermark(pool: &SqlitePool, watermark: i64) -> Result<i64> {
    let cur = get_watermark(pool).await?;
    let next = watermark.max(cur);
    sqlx::query("INSERT INTO sync_state (key, value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(WATERMARK_KEY).bind(next.to_string()).execute(pool).await?;
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn pull_timer_and_batching() {
        assert!(is_pull_due(1000, 1060));
        assert!(!is_pull_due(1000, 1059));
        let outbox: Vec<ChangeRecord> = (0..1100).map(|i| ChangeRecord {
            section: "photos".into(), item_id: i, op: "upsert".into(), rev: i,
        }).collect();
        let b = build_batches(&outbox);
        assert_eq!(b.len(), 3);            // 500 + 500 + 100
        assert_eq!(b[0].len(), 500);
        assert_eq!(b[2].len(), 100);
    }

    #[tokio::test]
    async fn watermark_is_monotonic() {
        let (_t, pool) = open_pool().await;
        assert_eq!(get_watermark(&pool).await.unwrap(), 0);
        assert_eq!(set_watermark(&pool, 100).await.unwrap(), 100);
        // a stale lower value can't roll it back
        assert_eq!(set_watermark(&pool, 50).await.unwrap(), 100);
        assert_eq!(get_watermark(&pool).await.unwrap(), 100);
    }
}
