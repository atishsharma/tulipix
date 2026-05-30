//! 360 / equirectangular photo detection.
//!
//! Two heuristics, run in order:
//!  1. XMP `GPano:ProjectionType == "equirectangular"` — the Google Photo
//!     Sphere standard. Detected via a literal string scan of the file's
//!     leading bytes; we never load the full image.
//!  2. Aspect ratio 2:1 (±2%) with both dimensions ≥ 4096 — Insta360 and
//!     stitched-action-cam panoramas usually omit GPano.
//!
//! Detection writes the boolean back to `photo_meta.pano`; viewer code reads
//! that column to decide whether to load the Slint GPU-sphere shader.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::io::Read;
use std::path::Path;

const GPANO_NEEDLE: &[u8] = b"ProjectionType";
const EQUIRECT_NEEDLE: &[u8] = b"equirectangular";
const RATIO_TARGET: f64 = 2.0;
const RATIO_TOL: f64 = 0.04;
const MIN_PANO_WIDTH: i64 = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PanoSource {
    Xmp,
    AspectRatio,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PanoDetect {
    pub item_id: i64,
    pub source: PanoSource,
}

/// Read up to the first 64 KiB and look for the GPano XMP markers.
pub fn has_gpano_xmp(path: &Path) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false; };
    let mut buf = [0u8; 64 * 1024];
    let n = f.read(&mut buf).unwrap_or(0);
    if n == 0 { return false; }
    let slice = &buf[..n];
    contains(slice, GPANO_NEEDLE) && contains(slice, EQUIRECT_NEEDLE)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() { return false; }
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Pure aspect-ratio check given width/height stored in `photo_meta`.
pub fn looks_equirectangular(width: i64, height: i64) -> bool {
    if width < MIN_PANO_WIDTH || height <= 0 { return false; }
    let r = width as f64 / height as f64;
    (r - RATIO_TARGET).abs() < RATIO_TARGET * RATIO_TOL
}

/// Probe one photo (path + dimensions). Returns the strongest detection
/// source, or `None` if the photo is not a panorama.
pub fn probe(path: &Path, width: Option<i64>, height: Option<i64>) -> Option<PanoSource> {
    if has_gpano_xmp(path) { return Some(PanoSource::Xmp); }
    if let (Some(w), Some(h)) = (width, height) {
        if looks_equirectangular(w, h) { return Some(PanoSource::AspectRatio); }
    }
    None
}

/// Run detection across the library and write `photo_meta.pano = 1` for the
/// hits. Returns the count of items flagged.
pub async fn scan(pool: &SqlitePool) -> Result<u64> {
    let rows: Vec<(i64, String, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.width, photo_meta.height
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.section = 'photos' AND items.missing_since IS NULL",
    ).fetch_all(pool).await?;
    let mut hits = 0u64;
    for (id, path, w, h) in rows {
        let p = Path::new(&path);
        let flag = probe(p, w, h).is_some();
        let v: i64 = if flag { 1 } else { 0 };
        sqlx::query("UPDATE photo_meta SET pano = ? WHERE item_id = ?")
            .bind(v).bind(id).execute(pool).await?;
        if flag { hits += 1; }
    }
    Ok(hits)
}

/// IDs of panoramas in the library, suitable for an "Immersive" smart album.
pub async fn list_panoramas(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT item_id FROM photo_meta WHERE pano = 1 ORDER BY taken_at DESC NULLS LAST",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(x,)| x).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn aspect_ratio_check() {
        assert!(looks_equirectangular(8192, 4096));
        assert!(looks_equirectangular(7680, 3840));
        assert!(!looks_equirectangular(4096, 4096));
        assert!(!looks_equirectangular(2000, 1000)); // below MIN_PANO_WIDTH
        assert!(!looks_equirectangular(8192, 0));
    }

    #[test]
    fn xmp_marker_found_in_synthetic_bytes() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("pano.jpg");
        let mut bytes = vec![0xFFu8; 2048];
        bytes.extend_from_slice(b"<rdf:Description xmlns:GPano=...>GPano:ProjectionType=\"equirectangular\"</rdf:Description>");
        std::fs::write(&p, &bytes).unwrap();
        assert!(has_gpano_xmp(&p));
    }

    #[test]
    fn probe_picks_xmp_over_aspect() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("p.jpg");
        let mut bytes = vec![0u8; 256];
        bytes.extend_from_slice(b"GPano:ProjectionType=equirectangular");
        std::fs::write(&p, &bytes).unwrap();
        // Dimensions that would *not* trigger the aspect heuristic.
        assert_eq!(probe(&p, Some(4000), Some(3000)), Some(PanoSource::Xmp));
    }

    #[tokio::test]
    async fn scan_writes_pano_flag() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("p.jpg");
        std::fs::write(&p, b"x").unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(p.to_string_lossy().as_ref()).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id, width, height) VALUES (?, 8192, 4096)")
            .bind(id).execute(&pool).await.unwrap();
        let n = scan(&pool).await.unwrap();
        assert_eq!(n, 1);
        let listed = list_panoramas(&pool).await.unwrap();
        assert_eq!(listed, vec![id]);
    }
}
