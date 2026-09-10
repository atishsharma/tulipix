//! `np.p4.music.bpm-key` — BPM & musical-key detection.
//!
//! Audio analysis (autocorrelation / chroma) runs in the Tools worker; this
//! module owns the result type, the persisted columns, and two pieces of pure
//! logic worth testing: BPM-from-beat-times and the Krumhansl key→Camelot
//! ("1A".."12B") mapping DJs sort by.

use anyhow::Result;
use sqlx::SqlitePool;

/// Estimate BPM from detected beat timestamps (seconds). Uses the median
/// inter-beat interval to resist outliers from missed/extra beats.
pub fn bpm_from_beats(beat_times_s: &[f64]) -> Option<f64> {
    if beat_times_s.len() < 2 { return None; }
    let mut iv: Vec<f64> = beat_times_s.windows(2).map(|w| w[1] - w[0]).filter(|d| *d > 0.0).collect();
    if iv.is_empty() { return None; }
    // The `> 0.0` filter above already drops NaN, so this cannot panic today —
    // total_cmp keeps it that way if the filter ever changes.
    iv.sort_by(|a, b| a.total_cmp(b));
    let median = iv[iv.len() / 2];
    Some(60.0 / median)
}

const KEYS: [&str; 12] = ["C","C#","D","D#","E","F","F#","G","G#","A","A#","B"];

/// `(pitch_class 0..11, is_major)` → Camelot wheel code, e.g. (9, false)=Am=8A.
pub fn camelot(pitch_class: u8, major: bool) -> Option<String> {
    if pitch_class > 11 { return None; }
    // Camelot order around the wheel; index = position 1..12.
    const MAJ: [u8; 12] = [8,3,10,5,12,7,2,9,4,11,6,1]; // by pitch class C..B
    const MIN: [u8; 12] = [5,12,7,2,9,4,11,6,1,8,3,10];
    let n = if major { MAJ[pitch_class as usize] } else { MIN[pitch_class as usize] };
    Some(format!("{n}{}", if major { 'B' } else { 'A' }))
}

/// Human key label, e.g. (9,false)="Am".
pub fn key_label(pitch_class: u8, major: bool) -> Option<String> {
    if pitch_class > 11 { return None; }
    Some(format!("{}{}", KEYS[pitch_class as usize], if major { "" } else { "m" }))
}

/// Split a Camelot code into its wheel position and mode, e.g. "8A" -> (8, false).
pub fn parse_camelot(code: &str) -> Option<(u8, bool)> {
    let code = code.trim();
    let (num, letter) = code.split_at(code.len().checked_sub(1)?);
    let major = match letter {
        "B" | "b" => true,
        "A" | "a" => false,
        _ => return None,
    };
    let n: u8 = num.parse().ok()?;
    (1..=12).contains(&n).then_some((n, major))
}

/// Whether two Camelot codes mix without clashing.
///
/// The DJ rule, unchanged since the wheel was drawn: stay where you are, step
/// one position around the ring, or switch mode in place (8A <-> 8B, the
/// relative minor/major). Position 12 is adjacent to 1 — it is a wheel, so the
/// arithmetic wraps; comparing the numbers directly would call 12A and 1A a
/// clash when they are neighbours.
pub fn harmonic(a: &str, b: &str) -> bool {
    let (Some((na, ma)), Some((nb, mb))) = (parse_camelot(a), parse_camelot(b)) else {
        return false;
    };
    if na == nb {
        return true; // same position, either mode
    }
    ma == mb && (na % 12 + 1 == nb || nb % 12 + 1 == na)
}

pub async fn store(pool: &SqlitePool, item_id: i64, bpm: Option<f64>, key: Option<&str>) -> Result<()> {
    sqlx::query("UPDATE track_meta SET bpm = ?, music_key = ? WHERE item_id = ?")
        .bind(bpm).bind(key).bind(item_id).execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::{open_pool, add_track};

    #[test]
    fn bpm_120_from_half_second_beats() {
        let beats: Vec<f64> = (0..9).map(|i| i as f64 * 0.5).collect();
        let bpm = bpm_from_beats(&beats).unwrap();
        assert!((bpm - 120.0).abs() < 0.5);
        assert!(bpm_from_beats(&[1.0]).is_none());
    }

    #[test]
    fn camelot_a_minor_is_8a() {
        assert_eq!(camelot(9, false).unwrap(), "8A");
        assert_eq!(camelot(0, true).unwrap(), "8B"); // C major
        assert!(camelot(12, true).is_none());
        assert_eq!(key_label(9, false).unwrap(), "Am");
    }

    #[test]
    fn the_wheel_wraps_at_twelve() {
        assert!(harmonic("8A", "8A"));
        assert!(harmonic("8A", "8B"), "relative major mixes in place");
        assert!(harmonic("8A", "9A"));
        assert!(harmonic("12A", "1A"), "12 and 1 are neighbours on a wheel");
        assert!(harmonic("1A", "12A"));
        assert!(!harmonic("8A", "10A"), "two steps is a clash");
        assert!(!harmonic("8A", "9B"), "step and switch mode at once is a clash");
        assert!(!harmonic("", "8A"));
        assert!(!harmonic("13A", "1A"), "13 is off the wheel");
        assert_eq!(parse_camelot("8A"), Some((8, false)));
        assert_eq!(parse_camelot("12B"), Some((12, true)));
        assert_eq!(parse_camelot("8C"), None);
    }

    #[tokio::test]
    async fn store_persists() {
        let (_t, pool) = open_pool().await;
        let id = add_track(&pool, "/m/a.flac").await;
        store(&pool, id, Some(128.0), Some("8A")).await.unwrap();
        let bpm: Option<f64> = sqlx::query_scalar("SELECT bpm FROM track_meta WHERE item_id = ?").bind(id).fetch_one(&pool).await.unwrap();
        assert_eq!(bpm, Some(128.0));
    }
}
