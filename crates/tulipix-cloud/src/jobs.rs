//! `np.p5.cloud.schedule` + `np.p5.cloud.quota` — saved sync jobs the core
//! scheduler polls, plus a transfer-history log for the usage dashboard.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// A saved, optionally-scheduled sync job.
#[derive(Debug, Clone, PartialEq)]
pub struct SyncJob {
    pub id: i64,
    pub src: String,
    pub dst: String,
    pub direction: String,   // "oneway" | "bisync"
    pub bwlimit: Option<String>,
    pub interval_s: i64,
    pub last_run: i64,
    pub enabled: i64,
}

pub async fn save_job(pool: &SqlitePool, src: &str, dst: &str, direction: &str, bwlimit: Option<&str>, interval_s: i64) -> Result<i64> {
    Ok(sqlx::query_scalar(
        "INSERT INTO sync_jobs (src, dst, direction, bwlimit, interval_s, last_run, enabled)
         VALUES (?,?,?,?,?,0,1) RETURNING id",
    ).bind(src).bind(dst).bind(direction).bind(bwlimit).bind(interval_s).fetch_one(pool).await?)
}

pub async fn list_jobs(pool: &SqlitePool) -> Result<Vec<SyncJob>> {
    let rows: Vec<(i64, String, String, String, Option<String>, i64, i64, i64)> = sqlx::query_as(
        "SELECT id, src, dst, direction, bwlimit, interval_s, last_run, enabled FROM sync_jobs ORDER BY id",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(id, src, dst, direction, bwlimit, interval_s, last_run, enabled)| SyncJob {
        id, src, dst, direction, bwlimit, interval_s, last_run, enabled,
    }).collect())
}

pub async fn delete_job(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM sync_jobs WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

pub async fn set_enabled(pool: &SqlitePool, id: i64, enabled: bool) -> Result<()> {
    sqlx::query("UPDATE sync_jobs SET enabled = ? WHERE id = ?").bind(enabled as i64).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn mark_ran(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE sync_jobs SET last_run = ? WHERE id = ?").bind(now()).bind(id).execute(pool).await?;
    Ok(())
}

/// Jobs whose schedule is due (enabled, interval>0, last_run+interval ≤ now).
pub async fn due_jobs(pool: &SqlitePool, now_unix: i64) -> Result<Vec<SyncJob>> {
    Ok(list_jobs(pool).await?.into_iter()
        .filter(|j| j.enabled != 0 && crate::sync::is_due(j.last_run, j.interval_s, now_unix))
        .collect())
}

/// Append a transfer-history row.
pub async fn log(pool: &SqlitePool, kind: &str, detail: &str, ok: bool) -> Result<()> {
    sqlx::query("INSERT INTO transfer_log (kind, detail, ok, at) VALUES (?,?,?,?)")
        .bind(kind).bind(detail).bind(ok as i64).bind(now()).execute(pool).await?;
    Ok(())
}

/// Recent transfer-history rows (kind, detail, ok, at), newest first.
pub async fn recent_log(pool: &SqlitePool, limit: i64) -> Result<Vec<(i64, String, String, i64, i64)>> {
    Ok(sqlx::query_as("SELECT id, kind, detail, ok, at FROM transfer_log ORDER BY at DESC, id DESC LIMIT ?")
        .bind(limit).fetch_all(pool).await?)
}

/// (kind, detail) for one transfer-history row — used to re-run ("retry") it.
pub async fn log_row(pool: &SqlitePool, id: i64) -> Result<Option<(String, String)>> {
    Ok(sqlx::query_as("SELECT kind, detail FROM transfer_log WHERE id = ?")
        .bind(id).fetch_optional(pool).await?)
}

/// Delete one transfer-history row.
pub async fn delete_log(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM transfer_log WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Wipe the entire transfer history.
pub async fn clear_log(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM transfer_log").execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn job_crud_and_due() {
        let (_t, pool) = open_pool().await;
        let id = save_job(&pool, "gdrive:", "/local", "bisync", Some("2M"), 60).await.unwrap();
        let jobs = list_jobs(&pool).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].direction, "bisync");
        // last_run=0, interval 60 → due at now.
        assert_eq!(due_jobs(&pool, 1000).await.unwrap().len(), 1);
        mark_ran(&pool, id).await.unwrap();
        assert!(due_jobs(&pool, 0).await.unwrap().is_empty());
        set_enabled(&pool, id, false).await.unwrap();
        assert!(due_jobs(&pool, i64::MAX).await.unwrap().is_empty());
        delete_job(&pool, id).await.unwrap();
        assert!(list_jobs(&pool).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn transfer_history() {
        let (_t, pool) = open_pool().await;
        log(&pool, "copy", "a:→b:", true).await.unwrap();
        log(&pool, "verify", "0 differences", false).await.unwrap();
        let rows = recent_log(&pool, 10).await.unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].1, "verify"); // newest first (rows[0].0 is id)
        // retry lookup + single + clear-all delete.
        let top = rows[0].0;
        assert_eq!(log_row(&pool, top).await.unwrap().unwrap().0, "verify");
        delete_log(&pool, top).await.unwrap();
        assert_eq!(recent_log(&pool, 10).await.unwrap().len(), 1);
        clear_log(&pool).await.unwrap();
        assert!(recent_log(&pool, 10).await.unwrap().is_empty());
    }
}
