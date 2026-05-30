//! `np.p4.cloud.share` — share-link issuance via the sync backend.
//!
//! Share links are minted by the optional sync backend (Go service), not
//! rclone, so issuance is greyed out when offline. This owns the issue/revoke
//! bookkeeping and the offline gate.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Can the share button be used? Requires Account mode + an online backend.
pub fn can_share(account_mode: bool, backend_online: bool) -> bool {
    account_mode && backend_online
}

/// Persist an issued share link (URL comes back from the backend).
pub async fn issue(pool: &SqlitePool, remote_id: i64, path: &str, url: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO shares (remote_id, remote_path, url, created, revoked) VALUES (?,?,?,?,0) RETURNING id",
    ).bind(remote_id).bind(path).bind(url).bind(now()).fetch_one(pool).await?)
}

pub async fn revoke(pool: &SqlitePool, share_id: i64) -> Result<()> {
    sqlx::query("UPDATE shares SET revoked = 1 WHERE id = ?").bind(share_id).execute(pool).await?;
    Ok(())
}

/// Active (non-revoked) share URLs for a path.
pub async fn active_for(pool: &SqlitePool, remote_id: i64, path: &str) -> Result<Vec<String>> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "SELECT url FROM shares WHERE remote_id = ? AND remote_path = ? AND revoked = 0 AND url IS NOT NULL",
    ).bind(remote_id).bind(path).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(u,)| u).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[test]
    fn offline_disables_share() {
        assert!(can_share(true, true));
        assert!(!can_share(true, false));
        assert!(!can_share(false, true));
    }

    #[tokio::test]
    async fn issue_then_revoke() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        let id = issue(&pool, r, "pic.jpg", "https://t.lx/abc").await.unwrap();
        assert_eq!(active_for(&pool, r, "pic.jpg").await.unwrap(), vec!["https://t.lx/abc"]);
        revoke(&pool, id).await.unwrap();
        assert!(active_for(&pool, r, "pic.jpg").await.unwrap().is_empty());
    }
}
