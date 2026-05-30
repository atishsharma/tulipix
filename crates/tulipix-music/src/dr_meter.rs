//! `np.p4.music.dr-meter` — Dynamic Range (DR) metering.
//!
//! Implements the TT/Pleasurize "DR" measure: per-channel, DR ≈ the spread
//! between the peak and the loudest-20% RMS. The audio decode happens in the
//! worker; this module owns the DR math (testable against known signals) and
//! the sort/filter queries that surface over-compressed "loudness war" tracks.

use anyhow::Result;
use sqlx::SqlitePool;

/// Compute a DR-style score from per-block RMS values (dBFS) and the track
/// peak (dBFS). DR = peak − (mean of loudest 20% of block RMS).
pub fn dr_score(block_rms_dbfs: &[f64], peak_dbfs: f64) -> Option<f64> {
    if block_rms_dbfs.is_empty() { return None; }
    let mut sorted = block_rms_dbfs.to_vec();
    sorted.sort_by(|a, b| b.partial_cmp(a).unwrap()); // loudest first
    let take = (sorted.len() as f64 * 0.2).ceil().max(1.0) as usize;
    let loud_mean = sorted[..take].iter().sum::<f64>() / take as f64;
    Some((peak_dbfs - loud_mean).max(0.0))
}

/// Linear RMS (0..1) → dBFS, clamped to avoid -inf on silence.
pub fn rms_to_dbfs(rms: f64) -> f64 {
    if rms <= 1e-9 { -120.0 } else { 20.0 * rms.log10() }
}

pub async fn store(pool: &SqlitePool, item_id: i64, dr: f64) -> Result<()> {
    sqlx::query("UPDATE track_meta SET dr_score = ? WHERE item_id = ?")
        .bind(dr).bind(item_id).execute(pool).await?;
    Ok(())
}

/// Tracks at or below `max_dr` — i.e. the most-compressed; the cleanup view.
pub async fn most_compressed(pool: &SqlitePool, max_dr: f64, limit: i64) -> Result<Vec<(i64, f64)>> {
    Ok(sqlx::query_as(
        "SELECT item_id, dr_score FROM track_meta
         WHERE dr_score IS NOT NULL AND dr_score <= ?
         ORDER BY dr_score ASC LIMIT ?",
    ).bind(max_dr).bind(limit).fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn wide_dynamics_score_higher() {
        // peak −1 dB, loud blocks ~ −15 dB → DR ≈ 14
        let blocks = vec![-15.0, -16.0, -40.0, -50.0, -60.0];
        let dr = dr_score(&blocks, -1.0).unwrap();
        assert!((dr - 14.0).abs() < 1.0, "dr was {dr}");
        assert!(dr_score(&[], -1.0).is_none());
    }

    #[test]
    fn dbfs_clamps_silence() {
        assert_eq!(rms_to_dbfs(0.0), -120.0);
        assert!((rms_to_dbfs(1.0)).abs() < 1e-6);
    }

    #[tokio::test]
    async fn filter_returns_compressed_first() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/loud.flac").await;
        let b = add_track(&pool, "/m/dynamic.flac").await;
        store(&pool, a, 4.0).await.unwrap();
        store(&pool, b, 14.0).await.unwrap();
        let r = most_compressed(&pool, 8.0, 10).await.unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].0, a);
    }
}
