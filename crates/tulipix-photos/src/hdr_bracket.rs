//! HDR bracket merge.
//!
//! Detection: scan consecutive shots from the same camera within
//! `BRACKET_WINDOW_SECS`, read `ExposureBiasValue` from EXIF, and emit a set
//! when ≥3 frames have distinct EV stops and span ≥`MIN_EV_SPAN`.
//!
//! Merge: hand the ordered file list to bundled ffmpeg with the `mergeplanes`
//! + `tonemap` chain via the `mertens` exposure-fusion filter — fully local,
//! no extra libraries.

use anyhow::{Context, Result};
use exif::{In, Reader, Tag, Value};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const BRACKET_WINDOW_SECS: i64 = 4;
pub const MIN_BRACKET_FRAMES: usize = 3;
pub const MIN_EV_SPAN: f64 = 1.0; // need ≥1 stop spread to be a meaningful bracket
pub const EV_EQ_EPS: f64 = 0.1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BracketSet {
    pub camera: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub item_ids: Vec<i64>,
    pub item_paths: Vec<String>,
    pub ev: Vec<f64>,
}

fn bundled_bin(name: &str) -> PathBuf {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_ref().and_then(|p| p.parent()).and_then(|p| p.parent());
    let os_arch =
        if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") { "linux-aarch64" }
        else if cfg!(target_os = "linux") { "linux-x86_64" }
        else if cfg!(target_os = "windows") { "windows-x86_64" }
        else if cfg!(target_arch = "aarch64") { "macos-aarch64" }
        else { "macos-x86_64" };
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    dir.map(|d| d.join("resources").join("bin").join(os_arch).join(format!("{name}{ext}")))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

/// Read `ExposureBiasValue` (a signed rational) from an image file. Returns
/// `None` if the tag is missing or the file has no EXIF.
pub fn read_ev(path: &Path) -> Option<f64> {
    let file = std::fs::File::open(path).ok()?;
    let mut bufreader = std::io::BufReader::new(&file);
    let reader = Reader::new();
    let exif = reader.read_from_container(&mut bufreader).ok()?;
    let field = exif.get_field(Tag::ExposureBiasValue, In::PRIMARY)?;
    match &field.value {
        Value::SRational(v) => v.first().map(|r| r.to_f64()),
        Value::Rational(v)  => v.first().map(|r| r.to_f64()),
        _ => None,
    }
}

/// Detect bracket sets across the photos DB. Reads EV from disk for each
/// candidate (filtered by camera + time-window first).
pub async fn detect(pool: &SqlitePool) -> Result<Vec<BracketSet>> {
    let rows: Vec<(i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.taken_at,
                photo_meta.camera_make, photo_meta.camera_model
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.taken_at IS NOT NULL
           AND items.missing_since IS NULL
         ORDER BY photo_meta.taken_at",
    ).fetch_all(pool).await?;

    let mut out: Vec<BracketSet> = Vec::new();
    let mut cur: Option<BracketSet> = None;
    for (id, path, taken, make, model) in rows {
        let cam = format!("{} {}", make.unwrap_or_default(), model.unwrap_or_default()).trim().to_string();
        let ev = read_ev(Path::new(&path)).unwrap_or(0.0);
        let push_new = match &cur {
            None => true,
            Some(b) => taken - b.end_unix > BRACKET_WINDOW_SECS || cam != b.camera,
        };
        if push_new {
            if let Some(b) = cur.take() { if qualifies(&b) { out.push(b); } }
            cur = Some(BracketSet {
                camera: cam,
                start_unix: taken, end_unix: taken,
                item_ids: vec![id], item_paths: vec![path],
                ev: vec![ev],
            });
        } else if let Some(b) = cur.as_mut() {
            b.end_unix = taken;
            b.item_ids.push(id);
            b.item_paths.push(path);
            b.ev.push(ev);
        }
    }
    if let Some(b) = cur { if qualifies(&b) { out.push(b); } }
    Ok(out)
}

fn qualifies(b: &BracketSet) -> bool {
    if b.item_ids.len() < MIN_BRACKET_FRAMES { return false; }
    let min = b.ev.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = b.ev.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    if !(max - min).is_finite() || (max - min) < MIN_EV_SPAN { return false; }
    // need ≥3 distinct EV values
    let mut distinct = 0usize;
    'outer: for (i, e) in b.ev.iter().enumerate() {
        for prev in &b.ev[..i] {
            if (e - prev).abs() < EV_EQ_EPS { continue 'outer; }
        }
        distinct += 1;
    }
    distinct >= MIN_BRACKET_FRAMES
}

/// Tone-map a bracket set into a single image via ffmpeg's `mertens` filter.
/// Reordered by EV ascending so the merge is deterministic.
pub fn merge(set: &BracketSet, out_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let mut order: Vec<usize> = (0..set.ev.len()).collect();
    order.sort_by(|&a, &b| set.ev[a].partial_cmp(&set.ev[b]).unwrap_or(std::cmp::Ordering::Equal));

    let ff = bundled_bin("ffmpeg");
    let mut c = Command::new(&ff);
    c.args(["-y", "-loglevel", "error"]);
    for i in &order { c.arg("-i").arg(&set.item_paths[*i]); }
    let n = order.len();
    let inputs: String = (0..n).map(|i| format!("[{i}:v]")).collect();
    let filter = format!("{inputs}mertens=contrast=1.0:saturation=1.0:exposure=1.0[hdr]");
    c.args(["-filter_complex", &filter, "-map", "[hdr]", "-frames:v", "1", "-q:v", "2"]);
    let out = out_dir.join(format!("hdr-{}.jpg", set.start_unix));
    c.arg(&out);
    let status = c.status().with_context(|| format!("spawn {}", ff.display()))?;
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn seed(pool: &SqlitePool, path: &str, taken: i64, cam: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id, taken_at, camera_make, camera_model) VALUES (?, ?, ?, ?)")
            .bind(id).bind(taken).bind("Tulip").bind(cam).execute(pool).await.unwrap();
        id
    }

    #[test]
    fn qualifies_rejects_uniform_ev() {
        let b = BracketSet {
            camera: "x".into(), start_unix: 0, end_unix: 1,
            item_ids: vec![1,2,3], item_paths: vec!["/a".into(),"/b".into(),"/c".into()],
            ev: vec![0.0, 0.0, 0.0],
        };
        assert!(!qualifies(&b));
    }

    #[test]
    fn qualifies_accepts_classic_bracket() {
        let b = BracketSet {
            camera: "x".into(), start_unix: 0, end_unix: 2,
            item_ids: vec![1,2,3], item_paths: vec!["/a".into(),"/b".into(),"/c".into()],
            ev: vec![-2.0, 0.0, 2.0],
        };
        assert!(qualifies(&b));
    }

    #[tokio::test]
    async fn detect_with_no_exif_returns_empty() {
        // Files don't physically exist with EXIF; detect should not crash and
        // should yield no sets because EV defaults to 0 → span = 0.
        let (_t, pool) = open_pool().await;
        for i in 0..3 { seed(&pool, &format!("/p/{i}.jpg"), 1000 + i, "C1").await; }
        let sets = detect(&pool).await.unwrap();
        assert!(sets.is_empty());
    }
}
