//! `np.p5.music.replaygain` — loudness-scan to COMPUTE gain for untagged files.
//!
//! Playback applies ReplayGain from file tags (mpv `replaygain`); this module
//! fills the gap for untagged libraries: the app worker decodes each track
//! through ffmpeg's `ebur128` filter, this module parses the integrated
//! loudness out of the summary and converts it to a ReplayGain 2 gain
//! (reference −18 LUFS), persisted in `track_meta.replaygain_track/_album`.
//! mpv then picks the value up via `replaygain-fallback` for tagless files.

use anyhow::Result;
use sqlx::SqlitePool;

/// ReplayGain 2 reference loudness (LUFS).
pub const RG2_REFERENCE_LUFS: f64 = -18.0;

/// Integrated loudness → ReplayGain 2 track gain in dB.
pub fn gain_from_lufs(integrated_lufs: f64) -> f64 {
    RG2_REFERENCE_LUFS - integrated_lufs
}

/// Parse the integrated loudness ("I: -23.0 LUFS") out of ffmpeg `ebur128`
/// summary output. Takes the LAST match — the filter prints rolling values,
/// the final one is the whole-track summary.
pub fn parse_ebur128_integrated(out: &str) -> Option<f64> {
    let mut found = None;
    for line in out.lines() {
        let t = line.trim();
        let Some(rest) = t.strip_prefix("I:") else { continue };
        let Some(num) = rest.trim().split_whitespace().next() else { continue };
        if let Ok(v) = num.parse::<f64>() { found = Some(v); }
    }
    found
}

/// Tracks that still need a computed gain: local files (not streams) with no
/// tag-derived or computed `replaygain_track`. Returns `(item_id, path)`.
pub async fn untagged(pool: &SqlitePool) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as(
        // items stores the file location in abs_path; there is no `path` column,
        // so this query failed outright ("no such column: i.path") and the
        // ReplayGain scan could never find a single track to analyse.
        "SELECT tm.item_id, i.abs_path FROM track_meta tm
         JOIN items i ON i.id = tm.item_id
         WHERE tm.replaygain_track IS NULL AND tm.is_stream = 0",
    ).fetch_all(pool).await?)
}

pub async fn store_track_gain(pool: &SqlitePool, item_id: i64, gain_db: f64) -> Result<()> {
    sqlx::query("UPDATE track_meta SET replaygain_track = ? WHERE item_id = ?")
        .bind(gain_db).bind(item_id).execute(pool).await?;
    Ok(())
}

/// Album gain = mean of the album's track gains (simple mean is the accepted
/// approximation when the per-track integrated loudness isn't retained).
pub async fn recompute_album_gains(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "UPDATE track_meta SET replaygain_album =
           (SELECT AVG(t2.replaygain_track) FROM track_meta t2
            WHERE t2.album_id = track_meta.album_id AND t2.replaygain_track IS NOT NULL)
         WHERE album_id IS NOT NULL",
    ).execute(pool).await?;
    Ok(())
}

/// Stored gain for a track: `(replaygain_track, replaygain_album)`.
pub async fn gains_for(pool: &SqlitePool, item_id: i64) -> Result<(Option<f64>, Option<f64>)> {
    let row: Option<(Option<f64>, Option<f64>)> = sqlx::query_as(
        "SELECT replaygain_track, replaygain_album FROM track_meta WHERE item_id = ?",
    ).bind(item_id).fetch_optional(pool).await?;
    Ok(row.unwrap_or((None, None)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn parses_last_integrated_summary() {
        let out = "[Parsed_ebur128_0] t: 1.0  I: -70.0 LUFS\n\
                   [Parsed_ebur128_0] t: 180.2  I: -22.4 LUFS\n\
                   Integrated loudness:\n  I:         -23.1 LUFS\n";
        assert_eq!(parse_ebur128_integrated(out), Some(-23.1));
        assert_eq!(parse_ebur128_integrated("no loudness here"), None);
    }

    #[test]
    fn rg2_gain_reference() {
        // quieter than reference → positive boost
        assert!((gain_from_lufs(-23.0) - 5.0).abs() < 1e-9);
        // hot master → cut
        assert!((gain_from_lufs(-10.0) + 8.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn untagged_then_stored_drops_out() {
        let (_t, pool) = open_pool().await;
        let a = add_track(&pool, "/m/a.flac").await;
        assert_eq!(untagged(&pool).await.unwrap().len(), 1);
        store_track_gain(&pool, a, -3.2).await.unwrap();
        assert!(untagged(&pool).await.unwrap().is_empty());
        let (t, _al) = gains_for(&pool, a).await.unwrap();
        assert!((t.unwrap() + 3.2).abs() < 1e-9);
    }
}
