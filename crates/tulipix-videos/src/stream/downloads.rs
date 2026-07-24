//! The Stream tab's download ledger.
//!
//! The queue used to live only in memory, so closing the app lost whatever was
//! waiting and there was no record of what had already been fetched. This is the
//! table behind the Downloads page: one row per job, from queued through to
//! done, failed or cancelled.
//!
//! The file on disk is the real artefact — a row whose file has been deleted
//! elsewhere is reported as `Missing` rather than quietly re-listed as present.

use anyhow::Result;
use sqlx::SqlitePool;

pub const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS stream_downloads (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    subject_id  TEXT    NOT NULL DEFAULT '',
    title       TEXT    NOT NULL DEFAULT '',
    season      INTEGER NOT NULL DEFAULT 0,
    episode     INTEGER NOT NULL DEFAULT 0,
    resolution  INTEGER NOT NULL DEFAULT 0,
    url         TEXT    NOT NULL,
    dest        TEXT    NOT NULL,
    state       TEXT    NOT NULL,
    done_bytes  INTEGER NOT NULL DEFAULT 0,
    total_bytes INTEGER NOT NULL DEFAULT 0,
    error       TEXT    NOT NULL DEFAULT '',
    queued_at   INTEGER NOT NULL,
    updated     INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS stream_downloads_state_idx ON stream_downloads(state, queued_at);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl State {
    pub fn as_str(self) -> &'static str {
        match self {
            State::Queued => "queued",
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }

    pub fn parse(raw: &str) -> Self {
        match raw {
            "running" => State::Running,
            "done" => State::Done,
            "failed" => State::Failed,
            "cancelled" => State::Cancelled,
            _ => State::Queued,
        }
    }

    /// Still on its way — the Downloads page groups these at the top.
    pub fn active(self) -> bool {
        matches!(self, State::Queued | State::Running)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Job {
    pub id: i64,
    pub subject_id: String,
    pub title: String,
    pub season: i64,
    pub episode: i64,
    pub resolution: i64,
    pub url: String,
    pub dest: String,
    pub state: String,
    pub done_bytes: i64,
    pub total_bytes: i64,
    pub error: String,
    pub queued_at: i64,
    pub updated: i64,
}

impl Job {
    pub fn state(&self) -> State {
        State::parse(&self.state)
    }

    /// 0.0..=1.0 of the way through, 0 when the size is not known yet.
    pub fn fraction(&self) -> f64 {
        if self.total_bytes <= 0 {
            return 0.0;
        }
        (self.done_bytes as f64 / self.total_bytes as f64).clamp(0.0, 1.0)
    }

    /// Does the finished file still exist where it was put?
    pub fn file_present(&self) -> bool {
        !self.dest.is_empty() && std::path::Path::new(&self.dest).exists()
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

type Row = (i64, String, String, i64, i64, i64, String, String, String, i64, i64, String, i64, i64);

const COLUMNS: &str = "id, subject_id, title, season, episode, resolution, url, dest, state, \
                       done_bytes, total_bytes, error, queued_at, updated";

fn job_of(r: Row) -> Job {
    Job {
        id: r.0,
        subject_id: r.1,
        title: r.2,
        season: r.3,
        episode: r.4,
        resolution: r.5,
        url: r.6,
        dest: r.7,
        state: r.8,
        done_bytes: r.9,
        total_bytes: r.10,
        error: r.11,
        queued_at: r.12,
        updated: r.13,
    }
}

/// Add a job in the `Queued` state. Returns its id.
pub async fn enqueue(pool: &SqlitePool, job: &Job) -> Result<i64> {
    let now = now_secs();
    let id = sqlx::query(
        "INSERT INTO stream_downloads
         (subject_id, title, season, episode, resolution, url, dest, state,
          done_bytes, total_bytes, error, queued_at, updated)
         VALUES (?, ?, ?, ?, ?, ?, ?, 'queued', 0, 0, '', ?, ?)",
    )
    .bind(&job.subject_id)
    .bind(&job.title)
    .bind(job.season)
    .bind(job.episode)
    .bind(job.resolution)
    .bind(&job.url)
    .bind(&job.dest)
    .bind(now)
    .bind(now)
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// Everything, newest first, with the unfinished jobs ahead of the rest.
pub async fn list(pool: &SqlitePool, limit: i64) -> Vec<Job> {
    let rows: Vec<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_downloads
         ORDER BY (state IN ('queued','running')) DESC, queued_at DESC LIMIT ?"
    ))
    .bind(limit.max(0))
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    rows.into_iter().map(job_of).collect()
}

/// The next job to run, oldest first.
pub async fn next_queued(pool: &SqlitePool) -> Option<Job> {
    let row: Option<Row> = sqlx::query_as(&format!(
        "SELECT {COLUMNS} FROM stream_downloads WHERE state = 'queued'
         ORDER BY queued_at LIMIT 1"
    ))
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();
    row.map(job_of)
}

pub async fn get(pool: &SqlitePool, id: i64) -> Option<Job> {
    let row: Option<Row> =
        sqlx::query_as(&format!("SELECT {COLUMNS} FROM stream_downloads WHERE id = ?"))
            .bind(id)
            .fetch_optional(pool)
            .await
            .ok()
            .flatten();
    row.map(job_of)
}

pub async fn set_state(pool: &SqlitePool, id: i64, state: State, error: &str) -> Result<()> {
    sqlx::query("UPDATE stream_downloads SET state = ?, error = ?, updated = ? WHERE id = ?")
        .bind(state.as_str())
        .bind(error)
        .bind(now_secs())
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Record progress. Called on a tick, so it writes only the two counters.
pub async fn set_progress(pool: &SqlitePool, id: i64, done: i64, total: i64) -> Result<()> {
    sqlx::query(
        "UPDATE stream_downloads SET done_bytes = ?, total_bytes = ?, updated = ? WHERE id = ?",
    )
    .bind(done.max(0))
    .bind(total.max(0))
    .bind(now_secs())
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn remove(pool: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM stream_downloads WHERE id = ?").bind(id).execute(pool).await?;
    Ok(())
}

/// Drop every finished row (done, failed, cancelled), keeping the queue.
pub async fn clear_finished(pool: &SqlitePool) -> Result<u64> {
    let r = sqlx::query("DELETE FROM stream_downloads WHERE state NOT IN ('queued','running')")
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

/// Cancel everything still waiting. The job in flight is stopped by the runner.
pub async fn cancel_waiting(pool: &SqlitePool) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE stream_downloads SET state = 'cancelled', updated = ? WHERE state = 'queued'",
    )
    .bind(now_secs())
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// A job left `running` when the app closed never finished. Called at startup so
/// the page does not show a download that no process is working on.
pub async fn reset_orphans(pool: &SqlitePool) -> Result<u64> {
    let r = sqlx::query(
        "UPDATE stream_downloads SET state = 'queued', updated = ? WHERE state = 'running'",
    )
    .bind(now_secs())
    .execute(pool)
    .await?;
    Ok(r.rows_affected())
}

/// How many are waiting or in flight — drives the tab's badge.
pub async fn active_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM stream_downloads WHERE state IN ('queued','running')")
        .fetch_one(pool)
        .await
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn job(title: &str, ep: i64) -> Job {
        Job {
            subject_id: "s1".into(),
            title: title.into(),
            season: 1,
            episode: ep,
            resolution: 720,
            url: format!("https://v/{ep}"),
            dest: format!("/tmp/{title}_{ep}.mp4"),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn jobs_run_oldest_first_and_list_active_first() {
        let (_t, pool) = open_pool().await;
        let a = enqueue(&pool, &job("Show", 1)).await.unwrap();
        let b = enqueue(&pool, &job("Show", 2)).await.unwrap();

        assert_eq!(next_queued(&pool).await.unwrap().id, a, "oldest first");
        set_state(&pool, a, State::Done, "").await.unwrap();
        assert_eq!(next_queued(&pool).await.unwrap().id, b);

        // Finished rows sink below whatever is still going.
        let listed = list(&pool, 10).await;
        assert_eq!(listed[0].id, b);
        assert_eq!(listed[1].id, a);
        assert_eq!(active_count(&pool).await, 1);
    }

    #[tokio::test]
    async fn progress_and_states_round_trip() {
        let (_t, pool) = open_pool().await;
        let id = enqueue(&pool, &job("Show", 1)).await.unwrap();
        assert_eq!(get(&pool, id).await.unwrap().state(), State::Queued);

        set_progress(&pool, id, 50, 200).await.unwrap();
        let j = get(&pool, id).await.unwrap();
        assert_eq!(j.fraction(), 0.25);
        // An unknown total reads as no progress rather than a divide by zero.
        set_progress(&pool, id, 50, 0).await.unwrap();
        assert_eq!(get(&pool, id).await.unwrap().fraction(), 0.0);

        set_state(&pool, id, State::Failed, "network error").await.unwrap();
        let j = get(&pool, id).await.unwrap();
        assert_eq!(j.state(), State::Failed);
        assert_eq!(j.error, "network error");
        assert!(!j.state().active());
    }

    #[tokio::test]
    async fn housekeeping_clears_the_right_rows() {
        let (_t, pool) = open_pool().await;
        let done = enqueue(&pool, &job("A", 1)).await.unwrap();
        let waiting = enqueue(&pool, &job("B", 2)).await.unwrap();
        let running = enqueue(&pool, &job("C", 3)).await.unwrap();
        set_state(&pool, done, State::Done, "").await.unwrap();
        set_state(&pool, running, State::Running, "").await.unwrap();

        assert_eq!(clear_finished(&pool).await.unwrap(), 1, "only the done one");
        assert_eq!(list(&pool, 10).await.len(), 2);

        // A crash leaves a job "running" with nothing running it.
        assert_eq!(reset_orphans(&pool).await.unwrap(), 1);
        assert_eq!(get(&pool, running).await.unwrap().state(), State::Queued);

        assert_eq!(cancel_waiting(&pool).await.unwrap(), 2);
        assert_eq!(get(&pool, waiting).await.unwrap().state(), State::Cancelled);
        assert_eq!(active_count(&pool).await, 0);

        remove(&pool, waiting).await.unwrap();
        assert_eq!(list(&pool, 10).await.len(), 1);
    }
}
