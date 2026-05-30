//! Spatial / depth photo detection.
//!
//! Three sources are recognised:
//!  * Apple Portrait — HEIC with an `auxiliaryImageType` of
//!    `urn:com:apple:photo:2018:aux:depth` or `…:hdrgainmap`.
//!  * iPhone / Google portrait JPEGs — XMP `Camera:Container` with a
//!    `Depthmap` Item describing an embedded depth payload (XMP HDR Gain Map
//!    or Camera:DepthMap).
//!  * Vision Pro spatial pairs — MV-HEVC stills with the `hevc1` brand and an
//!    `stvi` track group. We surface them by the `MV-HEVC` brand string the
//!    container carries near the top.
//!
//! Detection is heuristic: we never decode the depth track here, only flag
//! the asset for the viewer to load with parallax/depth shaders.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::io::Read;
use std::path::Path;

const APPLE_DEPTH: &[u8] = b"urn:com:apple:photo:2018:aux:depth";
const APPLE_HDRGAIN: &[u8] = b"urn:com:apple:photo:2018:aux:hdrgainmap";
const GCAM_DEPTH: &[u8] = b"GCamera:DepthMap";
const XMP_DEPTHMAP: &[u8] = b"DepthMap";
const MVHEVC_BRAND: &[u8] = b"hevc";
const MVHEVC_TRACK: &[u8] = b"stvi";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialKind {
    /// HEIC/HEIF auxiliary depth or HDR-gain-map track.
    AppleAux,
    /// Google Camera / Android depth-map XMP container.
    GoogleDepth,
    /// MV-HEVC spatial photo (Apple Vision Pro / iPhone 15+ pair).
    MvHevc,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpatialDetect {
    pub item_id: i64,
    pub kind: SpatialKind,
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() || needle.len() > haystack.len() { return false; }
    haystack.windows(needle.len()).any(|w| w == needle)
}

fn read_head(path: &Path, n: usize) -> Vec<u8> {
    let Ok(mut f) = std::fs::File::open(path) else { return Vec::new(); };
    let mut buf = vec![0u8; n];
    let read = f.read(&mut buf).unwrap_or(0);
    buf.truncate(read);
    buf
}

pub fn probe(path: &Path) -> Option<SpatialKind> {
    let head = read_head(path, 256 * 1024);
    if contains(&head, APPLE_DEPTH) || contains(&head, APPLE_HDRGAIN) {
        return Some(SpatialKind::AppleAux);
    }
    if contains(&head, GCAM_DEPTH) || contains(&head, XMP_DEPTHMAP) {
        return Some(SpatialKind::GoogleDepth);
    }
    if contains(&head, MVHEVC_BRAND) && contains(&head, MVHEVC_TRACK) {
        return Some(SpatialKind::MvHevc);
    }
    None
}

pub async fn scan(pool: &SqlitePool) -> Result<u64> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT items.id, items.abs_path FROM items
         WHERE items.section = 'photos' AND items.missing_since IS NULL",
    ).fetch_all(pool).await?;
    let mut hits = 0u64;
    for (id, path) in rows {
        let flag = probe(Path::new(&path)).is_some();
        let v: i64 = if flag { 1 } else { 0 };
        sqlx::query("UPDATE photo_meta SET spatial = ? WHERE item_id = ?")
            .bind(v).bind(id).execute(pool).await?;
        if flag { hits += 1; }
    }
    Ok(hits)
}

pub async fn list_spatial(pool: &SqlitePool) -> Result<Vec<i64>> {
    let rows: Vec<(i64,)> = sqlx::query_as(
        "SELECT item_id FROM photo_meta WHERE spatial = 1 ORDER BY taken_at DESC NULLS LAST",
    ).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|(x,)| x).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn detects_apple_depth_marker() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("portrait.heic");
        let mut bytes = vec![0u8; 1024];
        bytes.extend_from_slice(APPLE_DEPTH);
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(probe(&p), Some(SpatialKind::AppleAux));
    }

    #[test]
    fn detects_google_depth_xmp() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("g.jpg");
        std::fs::write(&p, b"...xmlns:GCamera=\"...\" GCamera:DepthMap=\"data...\"").unwrap();
        assert_eq!(probe(&p), Some(SpatialKind::GoogleDepth));
    }

    #[test]
    fn detects_mvhevc_stvi() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("avp.heic");
        let mut bytes = b"....ftyphevc1....".to_vec();
        bytes.extend_from_slice(b".......stvi.......");
        std::fs::write(&p, &bytes).unwrap();
        assert_eq!(probe(&p), Some(SpatialKind::MvHevc));
    }

    #[test]
    fn ignores_plain_jpeg() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("plain.jpg");
        std::fs::write(&p, b"no depth here").unwrap();
        assert!(probe(&p).is_none());
    }

    #[tokio::test]
    async fn scan_flags_spatial_in_db() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("a.heic");
        let mut bytes = vec![0u8; 64];
        bytes.extend_from_slice(APPLE_HDRGAIN);
        std::fs::write(&p, &bytes).unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(p.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
            .bind(p.to_string_lossy().as_ref()).fetch_one(&pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id) VALUES (?)").bind(id).execute(&pool).await.unwrap();
        let n = scan(&pool).await.unwrap();
        assert_eq!(n, 1);
        assert_eq!(list_spatial(&pool).await.unwrap(), vec![id]);
    }
}
