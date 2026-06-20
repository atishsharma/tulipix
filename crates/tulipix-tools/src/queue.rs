//! `np.p4.tools.queue` — job queue UI backend.
//!
//! Parallel worker slider, per-job progress, pause/resume/cancel/retry, all
//! persisted in `tools.db`. Both the GUI and the `tulipix` CLI submit here, so
//! a CLI `--queue` job shows up live in the GUI. Owns the state machine + the
//! "next runnable job respecting the worker cap" claim query.

use anyhow::Result;
use sqlx::SqlitePool;

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

pub const WORKERS_KEY: &str = "worker_slots";
pub const DEFAULT_WORKERS: i64 = 2;

/// Submit a job; returns its id.
pub async fn submit(pool: &SqlitePool, kind: &str, spec_json: &str, priority: i64) -> Result<i64> {
    let t = now();
    Ok(sqlx::query_scalar(
        "INSERT INTO jobs (kind, spec_json, state, priority, created, updated) VALUES (?,?,'queued',?,?,?) RETURNING id",
    ).bind(kind).bind(spec_json).bind(priority).bind(t).bind(t).fetch_one(pool).await?)
}

pub async fn set_worker_slots(pool: &SqlitePool, n: i64) -> Result<()> {
    sqlx::query("INSERT INTO queue_settings (key, value) VALUES (?,?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
        .bind(WORKERS_KEY).bind(n.max(1).to_string()).execute(pool).await?;
    Ok(())
}

pub async fn worker_slots(pool: &SqlitePool) -> Result<i64> {
    let v: Option<String> = sqlx::query_scalar("SELECT value FROM queue_settings WHERE key = ?")
        .bind(WORKERS_KEY).fetch_optional(pool).await?;
    Ok(v.and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_WORKERS))
}

/// Claim the next runnable job if a worker slot is free: marks the highest
/// priority queued job 'running'. Returns the claimed job id, or None.
pub async fn claim_next(pool: &SqlitePool) -> Result<Option<i64>> {
    let mut tx = pool.begin().await?;
    let running: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE state = 'running'").fetch_one(&mut *tx).await?;
    let slots = {
        let v: Option<String> = sqlx::query_scalar("SELECT value FROM queue_settings WHERE key = ?")
            .bind(WORKERS_KEY).fetch_optional(&mut *tx).await?;
        v.and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_WORKERS)
    };
    if running >= slots { tx.commit().await?; return Ok(None); }
    let next: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE state = 'queued' ORDER BY priority DESC, id LIMIT 1",
    ).fetch_optional(&mut *tx).await?;
    if let Some(id) = next {
        sqlx::query("UPDATE jobs SET state = 'running', attempts = attempts + 1, updated = ? WHERE id = ?")
            .bind(now()).bind(id).execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(next)
}

/// Update only a job's message (e.g. the resolved download title) without
/// touching its progress.
pub async fn set_message(pool: &SqlitePool, id: i64, message: &str) -> Result<()> {
    sqlx::query("UPDATE jobs SET message = ?, updated = ? WHERE id = ?")
        .bind(message).bind(now()).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn set_progress(pool: &SqlitePool, id: i64, progress: f64, message: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE jobs SET progress = ?, message = COALESCE(?, message), updated = ? WHERE id = ?")
        .bind(progress.clamp(0.0, 1.0)).bind(message).bind(now()).bind(id).execute(pool).await?;
    Ok(())
}

/// Transition helpers. Pause/resume only affect queued/running; cancel is
/// terminal; retry re-queues a failed job.
pub async fn pause(pool: &SqlitePool, id: i64) -> Result<()> { transition(pool, id, "running", "paused").await }
pub async fn resume(pool: &SqlitePool, id: i64) -> Result<()> { transition(pool, id, "paused", "queued").await }

pub async fn cancel(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = 'canceled', updated = ? WHERE id = ? AND state IN ('queued','running','paused')")
        .bind(now()).bind(id).execute(pool).await?;
    Ok(())
}

pub async fn complete(pool: &SqlitePool, id: i64, ok: bool, message: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = ?, progress = ?, message = COALESCE(?, message), updated = ? WHERE id = ?")
        .bind(if ok { "done" } else { "error" }).bind(if ok { 1.0 } else { 0.0 }).bind(message).bind(now()).bind(id)
        .execute(pool).await?;
    Ok(())
}

pub async fn retry(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = 'queued', progress = 0.0, updated = ? WHERE id = ? AND state = 'error'")
        .bind(now()).bind(id).execute(pool).await?;
    Ok(())
}

/// Delete a single job (not while it's running — its process is live).
pub async fn remove(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = ? AND state != 'running'").bind(id).execute(pool).await?;
    Ok(())
}

/// Clear every job except the ones currently running.
pub async fn clear_all(pool: &SqlitePool) -> Result<()> {
    sqlx::query("DELETE FROM jobs WHERE state != 'running'").execute(pool).await?;
    Ok(())
}

/// Move a queued job up (sooner) or down in run order. Rewrites all queued
/// priorities to distinct descending values so the new order is deterministic
/// (claim_next reads priority DESC, id).
pub async fn reorder(pool: &SqlitePool, id: i64, up: bool) -> Result<()> {
    let mut ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM jobs WHERE state = 'queued' ORDER BY priority DESC, id").fetch_all(pool).await?;
    let Some(pos) = ids.iter().position(|r| *r == id) else { return Ok(()); };
    let tgt = if up { pos.checked_sub(1) } else if pos + 1 < ids.len() { Some(pos + 1) } else { None };
    let Some(tgt) = tgt else { return Ok(()); };
    ids.swap(pos, tgt);
    let n = ids.len() as i64;
    let t = now();
    for (i, rid) in ids.iter().enumerate() {
        sqlx::query("UPDATE jobs SET priority = ?, updated = ? WHERE id = ?")
            .bind(n - i as i64).bind(t).bind(rid).execute(pool).await?;
    }
    Ok(())
}

async fn transition(pool: &SqlitePool, id: i64, from: &str, to: &str) -> Result<()> {
    sqlx::query("UPDATE jobs SET state = ?, updated = ? WHERE id = ? AND state = ?")
        .bind(to).bind(now()).bind(id).bind(from).execute(pool).await?;
    Ok(())
}

pub async fn state_of(pool: &SqlitePool, id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT state FROM jobs WHERE id = ?").bind(id).fetch_optional(pool).await?)
}

/// The job's stored spec JSON — used to resolve its output path (Open button).
pub async fn job_spec(pool: &SqlitePool, id: i64) -> Result<Option<String>> {
    Ok(sqlx::query_scalar("SELECT spec_json FROM jobs WHERE id = ?").bind(id).fetch_optional(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn worker_cap_limits_concurrency() {
        let (_t, pool) = open_pool().await;
        set_worker_slots(&pool, 1).await.unwrap();
        submit(&pool, "rename", "{}", 0).await.unwrap();
        submit(&pool, "rename", "{}", 5).await.unwrap(); // higher priority
        let first = claim_next(&pool).await.unwrap().unwrap();
        // priority 5 job claimed first
        let kind_pri: i64 = sqlx::query_scalar("SELECT priority FROM jobs WHERE id = ?").bind(first).fetch_one(&pool).await.unwrap();
        assert_eq!(kind_pri, 5);
        // worker cap = 1, one already running → no second claim
        assert!(claim_next(&pool).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn lifecycle_transitions() {
        let (_t, pool) = open_pool().await;
        let id = submit(&pool, "convert", "{}", 0).await.unwrap();
        claim_next(&pool).await.unwrap();
        pause(&pool, id).await.unwrap();
        assert_eq!(state_of(&pool, id).await.unwrap().as_deref(), Some("paused"));
        resume(&pool, id).await.unwrap();
        assert_eq!(state_of(&pool, id).await.unwrap().as_deref(), Some("queued"));
        claim_next(&pool).await.unwrap();
        complete(&pool, id, false, Some("boom")).await.unwrap();
        assert_eq!(state_of(&pool, id).await.unwrap().as_deref(), Some("error"));
        retry(&pool, id).await.unwrap();
        assert_eq!(state_of(&pool, id).await.unwrap().as_deref(), Some("queued"));
    }
}
