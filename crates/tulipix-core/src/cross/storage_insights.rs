//! `np.p4.storage` — Storage Insights (largest N / near-dup cohorts / cleanup).
//!
//! Reads the section `items` proxy table: the largest files, exact-duplicate
//! cohorts (same `sha256`, count > 1), and the reclaimable bytes if every
//! cohort were collapsed to one copy. Feeds the cleanup wizard.

use anyhow::Result;
use sqlx::SqlitePool;

/// Largest `n` present files: `(item_id, abs_path, size)`.
pub async fn largest(pool: &SqlitePool, n: i64) -> Result<Vec<(i64, String, i64)>> {
    Ok(sqlx::query_as(
        "SELECT id, abs_path, size FROM items WHERE missing_since IS NULL ORDER BY size DESC LIMIT ?",
    ).bind(n).fetch_all(pool).await?)
}

/// Exact-duplicate cohorts keyed by sha256: `(sha256, count, size_each)` where
/// count > 1.
pub async fn dup_cohorts(pool: &SqlitePool) -> Result<Vec<(String, i64, i64)>> {
    Ok(sqlx::query_as(
        "SELECT sha256, COUNT(*) c, MAX(size) FROM items
         WHERE sha256 IS NOT NULL AND missing_since IS NULL
         GROUP BY sha256 HAVING c > 1 ORDER BY MAX(size) * (c - 1) DESC",
    ).fetch_all(pool).await?)
}

/// Bytes reclaimable by collapsing every exact-dup cohort to one copy.
pub async fn reclaimable_bytes(pool: &SqlitePool) -> Result<i64> {
    let cohorts = dup_cohorts(pool).await?;
    Ok(cohorts.iter().map(|(_, count, size_each)| size_each * (count - 1)).sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{apply_proxy_schema, DbHandle};

    async fn pool() -> (tempfile::TempDir, SqlitePool) {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("photos.db");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let h = DbHandle { section: "photos".into(), path, url };
        let p = h.pool().await.unwrap();
        apply_proxy_schema(&p, "photos").await.unwrap();
        (tmp, p)
    }

    async fn add(p: &SqlitePool, path: &str, size: i64, sha: Option<&str>) {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, sha256, section, added, updated) VALUES (?,0,?,0,?,'photos',0,0)")
            .bind(path).bind(size).bind(sha).execute(p).await.unwrap();
    }

    #[tokio::test]
    async fn largest_and_dups() {
        let (_t, p) = pool().await;
        add(&p, "/a.jpg", 100, Some("h1")).await;
        add(&p, "/b.jpg", 100, Some("h1")).await; // dup of a
        add(&p, "/c.jpg", 500, Some("h2")).await;
        let big = largest(&p, 2).await.unwrap();
        assert_eq!(big[0].2, 500);
        let cohorts = dup_cohorts(&p).await.unwrap();
        assert_eq!(cohorts.len(), 1);
        assert_eq!(cohorts[0].1, 2); // count
        assert_eq!(reclaimable_bytes(&p).await.unwrap(), 100); // one redundant copy
    }
}
