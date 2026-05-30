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

/// Counts of pending work per stage — drives the Settings status pill.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct PendingCounts {
    pub exif: i64,
    pub thumbs: i64,
    pub faces: i64,
    pub clip: i64,
    pub tags: i64,
}

pub async fn pending(pool: &SqlitePool) -> anyhow::Result<PendingCounts> {
    let exif: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM photo_meta WHERE taken_at IS NULL"
    ).fetch_one(pool).await?;
    let faces: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM items
         LEFT JOIN faces ON faces.item_id = items.id
         WHERE items.section = 'photos'
           AND items.missing_since IS NULL
           AND faces.id IS NULL"
    ).fetch_one(pool).await?;
    let clip: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM items
         LEFT JOIN clip_embeddings ON clip_embeddings.item_id = items.id
         WHERE items.section = 'photos'
           AND items.missing_since IS NULL
           AND clip_embeddings.item_id IS NULL"
    ).fetch_one(pool).await?;
    let tags: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM items
         LEFT JOIN item_tags ON item_tags.item_id = items.id AND item_tags.source = 'yolov8'
         WHERE items.section = 'photos'
           AND items.missing_since IS NULL
           AND item_tags.item_id IS NULL"
    ).fetch_one(pool).await?;
    Ok(PendingCounts { exif, thumbs: 0, faces, clip, tags })
}

#[cfg(test)]
mod tests {
    use super::*;

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
