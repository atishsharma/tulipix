//! `np.p4.cloud.versions` — per-file revision rows + restore-revision UI.
//!
//! Mirrors backend file revisions (Drive, S3 versioning, Dropbox) into the
//! `revisions` table and builds the `rclone copyto --version-at` restore
//! command. Exactly one revision per path is flagged `is_current`.

use anyhow::Result;
use sqlx::SqlitePool;

/// Record a revision; if `is_current`, demote any prior current for that path.
pub async fn record(pool: &SqlitePool, remote_id: i64, path: &str, revision: &str, size: Option<i64>, modified: Option<i64>, is_current: bool) -> Result<()> {
    if is_current {
        sqlx::query("UPDATE revisions SET is_current = 0 WHERE remote_id = ? AND remote_path = ?")
            .bind(remote_id).bind(path).execute(pool).await?;
    }
    sqlx::query(
        "INSERT INTO revisions (remote_id, remote_path, revision, size, modified, is_current) VALUES (?,?,?,?,?,?)
         ON CONFLICT(remote_id, remote_path, revision) DO UPDATE SET size=excluded.size, modified=excluded.modified, is_current=excluded.is_current",
    ).bind(remote_id).bind(path).bind(revision).bind(size).bind(modified).bind(is_current as i64).execute(pool).await?;
    Ok(())
}

/// Revisions for a path, newest modified first.
pub async fn list(pool: &SqlitePool, remote_id: i64, path: &str) -> Result<Vec<(String, Option<i64>, bool)>> {
    let rows: Vec<(String, Option<i64>, i64)> = sqlx::query_as(
        "SELECT revision, modified, is_current FROM revisions WHERE remote_id = ? AND remote_path = ? ORDER BY modified DESC NULLS LAST",
    ).bind(remote_id).bind(path).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(r, m, c)| (r, m, c != 0)).collect())
}

/// `rclone copyto remote:path dest --version-at <when>` to restore a revision.
pub fn restore_args(remote: &str, path: &str, version_at: &str, dest: &str) -> Vec<String> {
    vec![
        "copyto".into(),
        format!("{remote}:{path}"),
        dest.into(),
        "--version-at".into(), version_at.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_remote};

    #[tokio::test]
    async fn only_one_current_per_path() {
        let (_t, pool) = open_pool().await;
        let r = add_remote(&pool, "gdrive", "drive").await;
        record(&pool, r, "doc.txt", "v1", Some(10), Some(100), true).await.unwrap();
        record(&pool, r, "doc.txt", "v2", Some(12), Some(200), true).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM revisions WHERE remote_id = ? AND remote_path = 'doc.txt' AND is_current = 1").bind(r).fetch_one(&pool).await.unwrap();
        assert_eq!(n, 1);
        let list = list(&pool, r, "doc.txt").await.unwrap();
        assert_eq!(list[0].0, "v2");
        assert!(list[0].2);
    }

    #[test]
    fn restore_argv() {
        let a = restore_args("gdrive", "doc.txt", "2024-01-01 00:00:00", "/tmp/doc.txt");
        assert!(a.windows(2).any(|w| w == ["--version-at", "2024-01-01 00:00:00"]));
    }
}
