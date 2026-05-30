//! Audio normalisation via ffmpeg `loudnorm` two-pass.
//!
//! Pass 1 measures integrated LUFS / LRA / true-peak / target offset by
//! parsing the JSON `loudnorm` emits on stderr. Pass 2 re-encodes with the
//! measured values bolted onto the filter so the result is exact, not the
//! ~0.5 LU drift you get from a single-pass run.
//!
//! Target gain per track is persisted so the player can apply the same
//! attenuation when streaming the original file (e.g. casting), without
//! re-encoding.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LoudnormMeasure {
    pub input_i: f64,           // integrated loudness in LUFS
    pub input_lra: f64,         // loudness range
    pub input_tp: f64,          // true-peak (dBTP)
    pub input_thresh: f64,
    pub target_offset: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct LoudnormTarget {
    pub integrated_lufs: f64, // -16.0 default (streaming spec)
    pub lra: f64,             // 11.0
    pub true_peak_db: f64,    // -1.5
}

impl Default for LoudnormTarget {
    fn default() -> Self {
        Self {
            integrated_lufs: -16.0,
            lra: 11.0,
            true_peak_db: -1.5,
        }
    }
}

/// Parse the JSON blob ffmpeg's `loudnorm=print_format=json` prints. The
/// printer mixes a single JSON object into the regular stderr stream so we
/// scan for the opening `{` and the matching `}` rather than json-parsing
/// the entire stderr.
pub fn parse_pass1(stderr: &str) -> Result<LoudnormMeasure> {
    let start = stderr.find('{').ok_or_else(|| anyhow!("no JSON in loudnorm output"))?;
    let end = stderr[start..]
        .rfind('}')
        .ok_or_else(|| anyhow!("unterminated JSON"))?
        + start;
    let v: serde_json::Value = serde_json::from_str(&stderr[start..=end])?;
    Ok(LoudnormMeasure {
        input_i: read_f(&v, "input_i")?,
        input_lra: read_f(&v, "input_lra")?,
        input_tp: read_f(&v, "input_tp")?,
        input_thresh: read_f(&v, "input_thresh")?,
        target_offset: read_f(&v, "target_offset")?,
    })
}

fn read_f(v: &serde_json::Value, key: &str) -> Result<f64> {
    let raw = v.get(key).and_then(|x| x.as_str())
        .ok_or_else(|| anyhow!("missing {key}"))?;
    raw.trim().parse::<f64>().map_err(|e| anyhow!("parse {key}: {e}"))
}

/// Build the pass-1 filter arg. Use this on a probe run that goes to
/// `-f null -`.
pub fn pass1_filter(target: LoudnormTarget) -> String {
    format!(
        "loudnorm=I={i}:LRA={lra}:TP={tp}:print_format=json",
        i = target.integrated_lufs,
        lra = target.lra,
        tp = target.true_peak_db,
    )
}

/// Build the pass-2 filter arg using the measured values from pass 1. This
/// applies the exact gain offset rather than re-measuring during the
/// re-encode (which is what makes loudnorm two-pass deterministic).
pub fn pass2_filter(target: LoudnormTarget, measured: &LoudnormMeasure) -> String {
    format!(
        "loudnorm=I={i}:LRA={lra}:TP={tp}:measured_I={mi}:measured_LRA={mlra}:measured_TP={mtp}:measured_thresh={mth}:offset={off}:linear=true:print_format=summary",
        i = target.integrated_lufs,
        lra = target.lra,
        tp = target.true_peak_db,
        mi = measured.input_i,
        mlra = measured.input_lra,
        mtp = measured.input_tp,
        mth = measured.input_thresh,
        off = measured.target_offset,
    )
}

/// dB the player should apply to hit the target LUFS without re-encoding.
/// Clamped to ±20 dB so a corrupt measurement can't blow speakers.
pub fn replaygain_for(target: LoudnormTarget, measured: &LoudnormMeasure) -> f64 {
    let raw = target.integrated_lufs - measured.input_i;
    raw.clamp(-20.0, 20.0)
}

pub const LOUDNORM_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS audio_loudness (
    item_id          INTEGER PRIMARY KEY REFERENCES items(id) ON DELETE CASCADE,
    integrated_lufs  REAL NOT NULL,
    lra              REAL NOT NULL,
    true_peak_db     REAL NOT NULL,
    threshold_lufs   REAL NOT NULL,
    target_offset    REAL NOT NULL,
    target_lufs      REAL NOT NULL,
    measured_at      INTEGER NOT NULL
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(LOUDNORM_SCHEMA).execute(pool).await?;
    Ok(())
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

pub async fn persist(
    pool: &SqlitePool,
    item_id: i64,
    target: LoudnormTarget,
    measured: &LoudnormMeasure,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO audio_loudness
         (item_id, integrated_lufs, lra, true_peak_db, threshold_lufs, target_offset, target_lufs, measured_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET
            integrated_lufs = excluded.integrated_lufs,
            lra = excluded.lra,
            true_peak_db = excluded.true_peak_db,
            threshold_lufs = excluded.threshold_lufs,
            target_offset = excluded.target_offset,
            target_lufs = excluded.target_lufs,
            measured_at = excluded.measured_at",
    )
    .bind(item_id)
    .bind(measured.input_i)
    .bind(measured.input_lra)
    .bind(measured.input_tp)
    .bind(measured.input_thresh)
    .bind(measured.target_offset)
    .bind(target.integrated_lufs)
    .bind(now())
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    const SAMPLE_STDERR: &str = r#"
[Parsed_loudnorm_0 @ 0x55] processing
[Parsed_loudnorm_0 @ 0x55]
{
    "input_i" : "-23.45",
    "input_tp" : "-2.20",
    "input_lra" : "7.80",
    "input_thresh" : "-33.40",
    "output_i" : "-16.10",
    "output_tp" : "-1.50",
    "output_lra" : "8.30",
    "output_thresh" : "-25.90",
    "normalization_type" : "dynamic",
    "target_offset" : "0.10"
}
[out#0/null @ 0x55] video:0kB audio:0kB
"#;

    #[test]
    fn parse_pass1_pulls_fields() {
        let m = parse_pass1(SAMPLE_STDERR).unwrap();
        assert!((m.input_i + 23.45).abs() < 1e-9);
        assert!((m.input_tp + 2.20).abs() < 1e-9);
        assert!((m.input_lra - 7.80).abs() < 1e-9);
        assert!((m.target_offset - 0.10).abs() < 1e-9);
    }

    #[test]
    fn parse_pass1_errors_on_missing_field() {
        let bad = r#"{"input_i":"-1","input_tp":"-1"}"#;
        assert!(parse_pass1(bad).is_err());
    }

    #[test]
    fn pass1_filter_emits_target_only() {
        let f = pass1_filter(LoudnormTarget::default());
        assert!(f.contains("I=-16"));
        assert!(f.contains("LRA=11"));
        assert!(f.contains("TP=-1.5"));
        assert!(f.contains("print_format=json"));
        assert!(!f.contains("measured_I"));
    }

    #[test]
    fn pass2_filter_includes_measured() {
        let m = parse_pass1(SAMPLE_STDERR).unwrap();
        let f = pass2_filter(LoudnormTarget::default(), &m);
        assert!(f.contains("measured_I=-23.45"));
        assert!(f.contains("linear=true"));
        assert!(f.contains("offset=0.1"));
    }

    #[test]
    fn replaygain_is_target_minus_measured_clamped() {
        let m = parse_pass1(SAMPLE_STDERR).unwrap();
        let g = replaygain_for(LoudnormTarget::default(), &m);
        assert!((g - 7.45).abs() < 1e-9);

        let cooked = LoudnormMeasure { input_i: -100.0, ..m };
        assert_eq!(replaygain_for(LoudnormTarget::default(), &cooked), 20.0);
        let blown = LoudnormMeasure { input_i: 100.0, ..parse_pass1(SAMPLE_STDERR).unwrap() };
        assert_eq!(replaygain_for(LoudnormTarget::default(), &blown), -20.0);
    }

    #[tokio::test]
    async fn persist_round_trip() {
        let (_t, pool) = open_pool().await;
        apply_schema(&pool).await.unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/a.mkv', 0, 1, 0, 'videos', 0, 0)")
            .execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = '/a.mkv'")
            .fetch_one(&pool).await.unwrap();
        let m = parse_pass1(SAMPLE_STDERR).unwrap();
        persist(&pool, id, LoudnormTarget::default(), &m).await.unwrap();
        let stored: f64 =
            sqlx::query_scalar("SELECT integrated_lufs FROM audio_loudness WHERE item_id = ?")
                .bind(id).fetch_one(&pool).await.unwrap();
        assert!((stored + 23.45).abs() < 1e-9);
    }
}
