//! `np.p4.cloud.restore-snapshot` — restore-by-date.
//!
//! Pick a date → restore the remote to its state then, using either backend
//! version history (`rclone --version-at`) or a local snapshot manifest dir.
//! This records snapshots, finds the best snapshot at-or-before a target time,
//! and builds the restore argv.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub async fn record(pool: &SqlitePool, remote_id: i64, manifest: &str) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO snapshots (remote_id, taken_at, manifest) VALUES (?,?,?) RETURNING id",
    ).bind(remote_id).bind(now()).bind(manifest).fetch_one(pool).await?)
}

/// Latest snapshot taken at or before `target_unix` — what "restore to this
/// date" actually uses.
pub async fn snapshot_at(pool: &SqlitePool, remote_id: i64, target_unix: i64) -> Result<Option<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT taken_at, manifest FROM snapshots WHERE remote_id = ? AND taken_at <= ? ORDER BY taken_at DESC LIMIT 1",
    ).bind(remote_id).bind(target_unix).fetch_optional(pool).await?)
}

/// `rclone sync --version-at <iso> remote: dest` argv to roll a tree back.
pub fn restore_args(remote: &str, version_at_iso: &str, dest: &str) -> Vec<String> {
    vec![
        "sync".into(), format!("{remote}:"), dest.into(),
        "--version-at".into(), version_at_iso.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[tokio::test]
    async fn picks_snapshot_at_or_before() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        sqlx::query("INSERT INTO snapshots (remote_id, taken_at, manifest) VALUES (?, 100, 'm1')").bind(r).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO snapshots (remote_id, taken_at, manifest) VALUES (?, 300, 'm3')").bind(r).execute(&pool).await.unwrap();
        assert_eq!(snapshot_at(&pool, r, 250).await.unwrap().unwrap().1, "m1");
        assert_eq!(snapshot_at(&pool, r, 999).await.unwrap().unwrap().1, "m3");
        assert!(snapshot_at(&pool, r, 50).await.unwrap().is_none());
    }

    #[test]
    fn restore_argv() {
        let a = restore_args("gdrive", "2024-01-01T00:00:00Z", "/restore");
        assert!(a.windows(2).any(|w| w == ["--version-at", "2024-01-01T00:00:00Z"]));
    }
}
