//! `np.p4.cloud.shared-inbox` — shared-with-me inbox (accept/decline).
//!
//! A cross-account list of albums/files others shared. Items arrive `pending`;
//! the user accepts (mounts/links into their library) or declines. This owns
//! the state transitions and the pending-count badge query.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn receive(pool: &SqlitePool, from_account: &str, remote_path: &str, kind: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO shared_inbox (from_account, remote_path, kind, state, received) VALUES (?,?,?,'pending',?) RETURNING id",
    ).bind(from_account).bind(remote_path).bind(kind).bind(now()).fetch_one(pool).await?)
}

pub async fn accept(pool: &SqlitePool, id: i64) -> Result<()> { set_state(pool, id, "accepted").await }
pub async fn decline(pool: &SqlitePool, id: i64) -> Result<()> { set_state(pool, id, "declined").await }

async fn set_state(pool: &SqlitePool, id: i64, state: &str) -> Result<()> {
    sqlx::query("UPDATE shared_inbox SET state = ? WHERE id = ? AND state = 'pending'")
        .bind(state).bind(id).execute(pool).await?;
    Ok(())
}

/// Pending items, newest first.
pub async fn pending(pool: &SqlitePool) -> Result<Vec<(i64, String, String)>> {
    Ok(sqlx::query_as("SELECT id, from_account, remote_path FROM shared_inbox WHERE state = 'pending' ORDER BY received DESC")
        .fetch_all(pool).await?)
}

/// Count for the sidebar badge.
pub async fn pending_count(pool: &SqlitePool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT COUNT(*) FROM shared_inbox WHERE state = 'pending'").fetch_one(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn accept_decline_flow() {
        let (_t, pool) = open_pool().await;
        let a = receive(&pool, "alice@x", "Shared/Album1", "album").await.unwrap();
        let b = receive(&pool, "bob@x", "Shared/file.pdf", "file").await.unwrap();
        assert_eq!(pending_count(&pool).await.unwrap(), 2);
        accept(&pool, a).await.unwrap();
        decline(&pool, b).await.unwrap();
        assert_eq!(pending_count(&pool).await.unwrap(), 0);
        // accepting an already-decided item is a no-op
        accept(&pool, b).await.unwrap();
        let st: String = sqlx::query_scalar("SELECT state FROM shared_inbox WHERE id = ?").bind(b).fetch_one(&pool).await.unwrap();
        assert_eq!(st, "declined");
    }
}
