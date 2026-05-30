//! Skip Intro — detect repeated audio segments at the start of show episodes
//! and persist `intro_start`/`intro_end` so the player can render a Skip
//! button. The detector is fingerprint-based (Chromaprint-style MFCC peaks)
//! but the algorithm here is intentionally fingerprint-agnostic — we accept
//! any pre-computed fingerprint stream and look for the longest common
//! prefix-aligned subsequence across episodes of the same season.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IntroMarker {
    pub item_id: i64,
    pub intro_start_s: f64,
    pub intro_end_s: f64,
}

pub const INTRO_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS intro_markers (
    item_id        INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    intro_start_s  REAL NOT NULL,
    intro_end_s    REAL NOT NULL,
    detector       TEXT NOT NULL,    -- 'audio-fp' | 'manual' | 'tmdb-chapter'
    confidence     REAL NOT NULL,
    updated        INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS intro_markers_updated_idx ON intro_markers(updated DESC);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(INTRO_SCHEMA).execute(pool).await?;
    Ok(())
}

/// Per-episode pre-computed fingerprint — `frames` is one u32 per ~125 ms
/// audio frame. Treat the contents as opaque; the detector only does equality.
#[derive(Debug, Clone)]
pub struct Fingerprint {
    pub item_id: i64,
    pub frames: Vec<u32>,
}

impl Fingerprint {
    pub const FRAME_HZ: f64 = 8.0;
}

/// Detect the longest shared prefix-aligned window of fingerprint frames
/// across a season's episodes. Returns one marker per episode where the
/// detection ran above `min_run_frames`.
///
/// `tolerance_frames` allows a small per-frame mismatch budget — themes have
/// minor encoding drift between episode rips so a strict equality requirement
/// would miss real intros.
pub fn detect_season(
    fps: &[Fingerprint],
    min_run_frames: usize,
    tolerance_frames: usize,
) -> Vec<IntroMarker> {
    if fps.len() < 2 {
        return Vec::new();
    }
    let mut best_run = 0usize;
    'outer: for end in (min_run_frames..=fps[0].frames.len()).rev() {
        let head = &fps[0].frames[..end];
        for ep in &fps[1..] {
            if !matches_prefix(head, &ep.frames, tolerance_frames) {
                continue 'outer;
            }
        }
        best_run = end;
        break;
    }
    if best_run == 0 {
        return Vec::new();
    }
    fps.iter()
        .map(|fp| IntroMarker {
            item_id: fp.item_id,
            intro_start_s: 0.0,
            intro_end_s: best_run as f64 / Fingerprint::FRAME_HZ,
        })
        .collect()
}

fn matches_prefix(head: &[u32], ep: &[u32], tolerance: usize) -> bool {
    if ep.len() < head.len() {
        return false;
    }
    let mut miss = 0usize;
    for i in 0..head.len() {
        if head[i] != ep[i] {
            miss += 1;
            if miss > tolerance {
                return false;
            }
        }
    }
    true
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn save_marker(
    pool: &SqlitePool,
    marker: &IntroMarker,
    detector: &str,
    confidence: f64,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO intro_markers (item_id, intro_start_s, intro_end_s, detector, confidence, updated)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            intro_start_s = excluded.intro_start_s,
            intro_end_s = excluded.intro_end_s,
            detector = excluded.detector,
            confidence = excluded.confidence,
            updated = excluded.updated",
    )
    .bind(marker.item_id)
    .bind(marker.intro_start_s)
    .bind(marker.intro_end_s)
    .bind(detector)
    .bind(confidence)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn marker_for(pool: &SqlitePool, item_id: i64) -> Result<Option<IntroMarker>> {
    let row: Option<(i64, f64, f64)> = sqlx::query_as(
        "SELECT item_id, intro_start_s, intro_end_s FROM intro_markers WHERE item_id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(item_id, intro_start_s, intro_end_s)| IntroMarker {
        item_id,
        intro_start_s,
        intro_end_s,
    }))
}

/// Overlay decision — should the player render the Skip button right now? We
/// gate on a 1 s lead-in so the button doesn't pop in mid-frame, and clamp
/// the visible window to `[start, end + 5s]` so it disappears just after the
/// intro ends.
pub fn should_show_skip(marker: &IntroMarker, position_s: f64) -> bool {
    let lead_in = (marker.intro_start_s - 1.0).max(0.0);
    let tail = marker.intro_end_s + 5.0;
    position_s >= lead_in && position_s <= tail
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    fn fp(item_id: i64, frames: &[u32]) -> Fingerprint {
        Fingerprint {
            item_id,
            frames: frames.to_vec(),
        }
    }

    #[test]
    fn detect_finds_common_prefix() {
        let prefix = vec![1u32; 200];
        let mut e1 = prefix.clone();
        e1.extend([10, 20, 30]);
        let mut e2 = prefix.clone();
        e2.extend([40, 50, 60]);
        let mut e3 = prefix.clone();
        e3.extend([70, 80, 90]);
        let out = detect_season(&[fp(1, &e1), fp(2, &e2), fp(3, &e3)], 50, 0);
        assert_eq!(out.len(), 3);
        assert!((out[0].intro_end_s - 25.0).abs() < 1e-9, "200 frames / 8 Hz = 25 s");
    }

    #[test]
    fn detect_skips_short_overlap() {
        let e1 = vec![1u32; 30];
        let e2 = vec![1u32; 30];
        let out = detect_season(&[fp(1, &e1), fp(2, &e2)], 200, 0);
        assert!(out.is_empty(), "30 < min_run 200");
    }

    #[test]
    fn detect_tolerates_drift() {
        let mut e1 = vec![1u32; 100];
        let mut e2 = vec![1u32; 100];
        e2[5] = 99; // 1 frame of drift
        e1.extend([5, 6]);
        e2.extend([7, 8]);
        let strict = detect_season(&[fp(1, &e1), fp(2, &e2)], 50, 0);
        assert!(strict.is_empty());
        let tol = detect_season(&[fp(1, &e1), fp(2, &e2)], 50, 2);
        assert!(!tol.is_empty());
    }

    #[test]
    fn detect_single_episode_returns_empty() {
        assert!(detect_season(&[fp(1, &[1, 2, 3])], 1, 0).is_empty());
    }

    #[test]
    fn skip_visible_window() {
        let m = IntroMarker {
            item_id: 1,
            intro_start_s: 10.0,
            intro_end_s: 70.0,
        };
        assert!(should_show_skip(&m, 9.0)); // lead-in
        assert!(should_show_skip(&m, 30.0));
        assert!(should_show_skip(&m, 74.9)); // tail
        assert!(!should_show_skip(&m, 80.0));
        assert!(!should_show_skip(&m, 1.0));
    }

    #[tokio::test]
    async fn save_then_load() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/e.mkv', 0, 1, 0, 'videos', 0, 0)")
            .execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/e.mkv'").fetch_one(&pool).await.unwrap();
        let m = IntroMarker {
            item_id: id,
            intro_start_s: 5.0,
            intro_end_s: 65.0,
        };
        save_marker(&pool, &m, "audio-fp", 0.9).await.unwrap();
        let got = marker_for(&pool, id).await.unwrap().unwrap();
        assert_eq!(got, m);
    }
}
