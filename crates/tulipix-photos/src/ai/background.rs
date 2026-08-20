//! Idle-only background indexer.
//!
//! Drives EXIF, thumbs, faces, tags, and CLIP embeddings off the same queue,
//! but only ticks while the user is idle (`tulipix_core::idle`). Pauses when
//! the laptop is on battery if `BackgroundPolicy::pause_on_battery` is set.
//! Resumes automatically when state flips back.
//!
//! Implementation note: this module owns the *policy* — the actual worker
//! loop lives in `tulipix-app`'s main runtime so we don't double-spawn
//! tokio tasks in unit tests.

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::time::Duration;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackgroundPolicy {
    pub idle_threshold_secs: u64,
    pub pause_on_battery: bool,
    pub batch_size: usize,
    pub tick: Duration,
}

impl Default for BackgroundPolicy {
    fn default() -> Self {
        Self {
            idle_threshold_secs: 600, // 10 min
            pause_on_battery: true,
            batch_size: 64,
            tick: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunState { Active, IdleWaiting, BatteryPaused, UserActive }

#[derive(Debug, Clone, Copy)]
pub struct SystemSnapshot {
    pub idle_secs: u64,
    pub on_battery: bool,
}

pub fn decide(p: &BackgroundPolicy, s: SystemSnapshot) -> RunState {
    if p.pause_on_battery && s.on_battery {
        return RunState::BatteryPaused;
    }
    if s.idle_secs >= p.idle_threshold_secs {
        RunState::Active
    } else {
        RunState::UserActive
    }
}

/// One kind of per-photo background work.
///
/// The string form is the `photo_ai_state.stage` key and is persisted, so these
/// spellings are a storage format: renaming one silently re-queues every photo
/// for that stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    /// Read EXIF into `photo_meta`.
    Exif,
    /// Index filename/camera/tags/people into the `photo_fts` table.
    Fts,
    /// Detect faces and write crops.
    Faces,
    /// COCO object tags.
    Tags,
    /// CLIP image embedding.
    Clip,
}

impl Stage {
    pub const ALL: [Stage; 5] = [Stage::Exif, Stage::Fts, Stage::Faces, Stage::Tags, Stage::Clip];

    pub fn key(self) -> &'static str {
        match self {
            Stage::Exif => "exif",
            Stage::Fts => "fts",
            Stage::Faces => "faces",
            Stage::Tags => "tags",
            Stage::Clip => "clip",
        }
    }
}

/// Counts of pending work per stage — drives the Settings status pill.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PendingCounts {
    pub exif: i64,
    pub fts: i64,
    pub faces: i64,
    pub clip: i64,
    pub tags: i64,
}

impl PendingCounts {
    pub fn total(&self) -> i64 {
        self.exif + self.fts + self.faces + self.clip + self.tags
    }
}

/// How many live photos have not yet been *considered* for `stage`.
///
/// Counted against `photo_ai_state`, not against whether the stage produced
/// anything. These queries used to ask "which photos have no faces row / no
/// tags row / no EXIF date", which are all satisfiable-by-nothing: a photo with
/// no faces in it, a landscape with no COCO objects, a screenshot with no date.
/// Those items answered "pending" forever, so the queue could never drain and
/// an indexer driven from it would re-process the same photos for as long as
/// the machine stayed idle.
pub async fn pending_for(pool: &SqlitePool, stage: Stage) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM items
         LEFT JOIN photo_ai_state s ON s.item_id = items.id AND s.stage = ?
         WHERE items.section = 'photos'
           AND items.missing_since IS NULL
           AND s.item_id IS NULL",
    )
    .bind(stage.key())
    .fetch_one(pool)
    .await?)
}

pub async fn pending(pool: &SqlitePool) -> anyhow::Result<PendingCounts> {
    Ok(PendingCounts {
        exif: pending_for(pool, Stage::Exif).await?,
        fts: pending_for(pool, Stage::Fts).await?,
        faces: pending_for(pool, Stage::Faces).await?,
        tags: pending_for(pool, Stage::Tags).await?,
        clip: pending_for(pool, Stage::Clip).await?,
    })
}

/// The next `limit` photos awaiting `stage`, as `(item_id, abs_path)`.
///
/// Oldest-added first, so a long-running backfill makes visible progress from
/// one end rather than skipping around the library.
pub async fn next_batch(
    pool: &SqlitePool,
    stage: Stage,
    limit: i64,
) -> anyhow::Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as(
        "SELECT items.id, items.abs_path FROM items
         LEFT JOIN photo_ai_state s ON s.item_id = items.id AND s.stage = ?
         WHERE items.section = 'photos'
           AND items.missing_since IS NULL
           AND s.item_id IS NULL
         ORDER BY items.added ASC, items.id ASC
         LIMIT ?",
    )
    .bind(stage.key())
    .bind(limit)
    .fetch_all(pool)
    .await?)
}

/// Record that `stage` has been attempted for `item_id`.
///
/// Called whether or not the stage produced anything — that is the entire point
/// (see `pending_for`). `model` identifies the weights used, so a later version
/// bump can requeue just that stage.
/// Takes an executor rather than the pool, like `exif::write_facts`, so the
/// library scan can record the EXIF it just read inside its own batch
/// transaction instead of re-opening one — and so a freshly scanned library
/// does not queue every photo for a second, pointless read.
pub async fn mark_done<'e, E>(exec: E, item_id: i64, stage: Stage, model: &str) -> anyhow::Result<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query(
        "INSERT INTO photo_ai_state (item_id, stage, done_at, model) VALUES (?, ?, ?, ?)
         ON CONFLICT(item_id, stage) DO UPDATE SET done_at = excluded.done_at, model = excluded.model",
    )
    .bind(item_id)
    .bind(stage.key())
    .bind(now)
    .bind(model)
    .execute(exec)
    .await?;
    Ok(())
}

/// Forget every attempt at `stage`, so the whole library is reconsidered.
/// For "re-run detection with the new model" in Settings.
pub async fn reset_stage(pool: &SqlitePool, stage: Stage) -> anyhow::Result<u64> {
    let r = sqlx::query("DELETE FROM photo_ai_state WHERE stage = ?")
        .bind(stage.key())
        .execute(pool)
        .await?;
    Ok(r.rows_affected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn seed(pool: &SqlitePool, path: &str) -> i64 {
        sqlx::query(
            "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated)
             VALUES (?, 0, 1, 0, 'photos', 0, 0)",
        )
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(path)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// The property the whole `photo_ai_state` table exists for: a stage that
    /// finds nothing still counts as done, so the queue drains. Under the old
    /// "no faces row ⇒ pending" definition this photo stayed pending forever.
    #[tokio::test]
    async fn a_stage_that_produced_nothing_still_drains() {
        let (_t, pool) = open_pool().await;
        let id = seed(&pool, "/tmp/scenery.jpg").await;
        assert_eq!(pending_for(&pool, Stage::Faces).await.unwrap(), 1);

        // No faces detected — nothing written to `faces` — but it was tried.
        mark_done(&pool, id, Stage::Faces, "scrfd-1.0").await.unwrap();

        assert_eq!(pending_for(&pool, Stage::Faces).await.unwrap(), 0);
        assert!(next_batch(&pool, Stage::Faces, 10).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn stages_are_tracked_independently() {
        let (_t, pool) = open_pool().await;
        let id = seed(&pool, "/tmp/a.jpg").await;
        mark_done(&pool, id, Stage::Exif, "").await.unwrap();

        let p = pending(&pool).await.unwrap();
        assert_eq!(p.exif, 0);
        assert_eq!(p.faces, 1);
        assert_eq!(p.tags, 1);
        assert_eq!(p.clip, 1);
        assert_eq!(p.fts, 1);
        assert_eq!(p.total(), 4);
    }

    #[tokio::test]
    async fn next_batch_respects_limit_and_marking() {
        let (_t, pool) = open_pool().await;
        for i in 0..5 {
            seed(&pool, &format!("/tmp/{i}.jpg")).await;
        }
        let first = next_batch(&pool, Stage::Fts, 2).await.unwrap();
        assert_eq!(first.len(), 2);
        for (id, _) in &first {
            mark_done(&pool, *id, Stage::Fts, "").await.unwrap();
        }
        // The marked two are gone from the queue rather than handed back.
        let second = next_batch(&pool, Stage::Fts, 2).await.unwrap();
        assert_eq!(second.len(), 2);
        assert!(second.iter().all(|(id, _)| !first.iter().any(|(f, _)| f == id)));
        assert_eq!(pending_for(&pool, Stage::Fts).await.unwrap(), 3);
    }

    #[tokio::test]
    async fn reset_stage_requeues_only_its_own() {
        let (_t, pool) = open_pool().await;
        let id = seed(&pool, "/tmp/a.jpg").await;
        mark_done(&pool, id, Stage::Tags, "yolov8n-1.0").await.unwrap();
        mark_done(&pool, id, Stage::Clip, "clip-vit-b32").await.unwrap();

        assert_eq!(reset_stage(&pool, Stage::Tags).await.unwrap(), 1);
        assert_eq!(pending_for(&pool, Stage::Tags).await.unwrap(), 1);
        assert_eq!(pending_for(&pool, Stage::Clip).await.unwrap(), 0);
    }

    /// A file that has gone missing is not work — it would fail every pass.
    #[tokio::test]
    async fn missing_items_are_not_queued() {
        let (_t, pool) = open_pool().await;
        let id = seed(&pool, "/tmp/gone.jpg").await;
        sqlx::query("UPDATE items SET missing_since = 1 WHERE id = ?")
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
        assert_eq!(pending_for(&pool, Stage::Exif).await.unwrap(), 0);
    }

    #[test]
    fn battery_pauses_when_policy_says_so() {
        let p = BackgroundPolicy::default();
        let s = decide(&p, SystemSnapshot { idle_secs: 9999, on_battery: true });
        assert_eq!(s, RunState::BatteryPaused);
    }
    #[test]
    fn battery_ignored_when_policy_off() {
        let mut p = BackgroundPolicy::default();
        p.pause_on_battery = false;
        let s = decide(&p, SystemSnapshot { idle_secs: 9999, on_battery: true });
        assert_eq!(s, RunState::Active);
    }
    #[test]
    fn idle_threshold_gates_activity() {
        let p = BackgroundPolicy::default();
        let a = decide(&p, SystemSnapshot { idle_secs: 0, on_battery: false });
        assert_eq!(a, RunState::UserActive);
        let b = decide(&p, SystemSnapshot { idle_secs: 700, on_battery: false });
        assert_eq!(b, RunState::Active);
    }
}
