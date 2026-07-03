//! Live Photos / motion photos.
//!
//! Three input shapes are supported:
//!  * iPhone Live Photo: HEIC + sibling MOV with identical stem.
//!  * iPhone Live Photo (export): JPG + sibling MOV with identical stem.
//!  * Android motion-photo: single JPEG with an embedded MP4 trailer marked
//!    by `MicroVideoOffset` / `MotionPhoto` XMP — the bytes after that offset
//!    are a valid MP4.
//!
//! Detection scans the photos library for any of these and produces a
//! `LivePhoto` record. Export splits the pair into still / video / GIF; the
//! GIF route reuses bundled ffmpeg with palettegen.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;
use tulipix_core::thumbs::tool_bin;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LiveKind {
    /// HEIC or JPG + sibling MOV file.
    SiblingPair,
    /// JPEG with an MP4 trailer at a known offset (Android motion-photo).
    EmbeddedMp4,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LivePhoto {
    pub still_item_id: i64,
    pub still_path: PathBuf,
    /// For SiblingPair: the sibling MOV path. For EmbeddedMp4: the same path
    /// as `still_path` (extraction reads from the byte offset).
    pub video_path: PathBuf,
    pub kind: LiveKind,
    /// Byte offset of the MP4 trailer inside `still_path` (EmbeddedMp4 only).
    pub video_offset: Option<u64>,
}

/// Probe one file path. Returns `Some(LivePhoto)` if it is a live photo of any
/// shape, `None` otherwise. `item_id` is filled in for SQL-backed callers; pass
/// 0 if probing outside the DB.
pub fn probe(path: &Path, item_id: i64) -> Option<LivePhoto> {
    if !path.is_file() { return None; }
    let ext = path.extension().and_then(|e| e.to_str())?.to_ascii_lowercase();

    if matches!(ext.as_str(), "heic" | "heif" | "jpg" | "jpeg") {
        if let Some(mov) = sibling_video(path) {
            return Some(LivePhoto {
                still_item_id: item_id,
                still_path: path.to_path_buf(),
                video_path: mov,
                kind: LiveKind::SiblingPair,
                video_offset: None,
            });
        }
    }
    if matches!(ext.as_str(), "jpg" | "jpeg") {
        if let Some(off) = detect_embedded_mp4(path) {
            return Some(LivePhoto {
                still_item_id: item_id,
                still_path: path.to_path_buf(),
                video_path: path.to_path_buf(),
                kind: LiveKind::EmbeddedMp4,
                video_offset: Some(off),
            });
        }
    }
    None
}

fn sibling_video(path: &Path) -> Option<PathBuf> {
    let stem = path.file_stem()?;
    let parent = path.parent()?;
    for ext in ["mov", "MOV", "mp4", "MP4"] {
        let candidate = parent.join(stem).with_extension(ext);
        if candidate.is_file() { return Some(candidate); }
    }
    None
}

/// Find the offset of an `ftyp` box inside the JPEG. Android motion photos
/// stash a full MP4 directly after the JPEG EOI. We scan the tail and locate
/// the first `ftyp` atom signature (`....ftyp` — four bytes of length followed
/// by ASCII 'ftyp').
fn detect_embedded_mp4(path: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(path).ok()?;
    let meta = f.metadata().ok()?;
    let len = meta.len();
    if len < 1024 { return None; }
    // Read in trailing chunks of 1 MiB; most motion-photo MP4s start within
    // a few MiB of EOF and the whole append is rarely larger than the still.
    let mut buf = Vec::with_capacity(1024 * 1024);
    let mut pos = len.saturating_sub(8 * 1024 * 1024);
    f.seek(SeekFrom::Start(pos)).ok()?;
    buf.clear();
    f.read_to_end(&mut buf).ok()?;
    let needle = b"ftyp";
    for i in 4..buf.len().saturating_sub(4) {
        if &buf[i..i + 4] == needle {
            // ftyp box length is the 4 bytes immediately before.
            // Validate: the byte right after `ftyp` should be an ASCII brand char.
            let after = buf[i + 4];
            if after.is_ascii_alphanumeric() || after == b' ' {
                pos = pos.saturating_add(i as u64).saturating_sub(4);
                return Some(pos);
            }
        }
    }
    None
}

/// Scan the photos DB for all live photos. Returns the new records; caller
/// decides whether to persist them.
pub async fn detect_in_library(pool: &SqlitePool) -> Result<Vec<LivePhoto>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT id, abs_path FROM items
         WHERE section = 'photos' AND missing_since IS NULL",
    ).fetch_all(pool).await?;
    let mut out = Vec::new();
    for (id, p) in rows {
        if let Some(lp) = probe(Path::new(&p), id) { out.push(lp); }
    }
    Ok(out)
}

/// Write the motion-video portion of a live photo to `out_dir/<stem>.mp4`.
/// For EmbeddedMp4 this reads bytes from `video_offset` to EOF.
pub fn export_video(lp: &LivePhoto, out_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let stem = lp.still_path.file_stem().and_then(|s| s.to_str()).unwrap_or("live");
    let out = out_dir.join(format!("{stem}.mp4"));
    match lp.kind {
        LiveKind::SiblingPair => {
            std::fs::copy(&lp.video_path, &out)
                .with_context(|| format!("copy {} → {}", lp.video_path.display(), out.display()))?;
        }
        LiveKind::EmbeddedMp4 => {
            let off = lp.video_offset.unwrap_or(0);
            let mut src = std::fs::File::open(&lp.still_path)?;
            src.seek(SeekFrom::Start(off))?;
            let mut dst = std::fs::File::create(&out)?;
            std::io::copy(&mut src, &mut dst)?;
        }
    }
    Ok(out)
}

/// Encode the live video to an animated GIF using bundled ffmpeg with a
/// proper palettegen → paletteuse pass.
pub fn export_gif(lp: &LivePhoto, out_dir: &Path, width: u32, fps: u32) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let stem = lp.still_path.file_stem().and_then(|s| s.to_str()).unwrap_or("live");
    let out = out_dir.join(format!("{stem}.gif"));
    let mp4 = match lp.kind {
        LiveKind::SiblingPair => lp.video_path.clone(),
        LiveKind::EmbeddedMp4 => export_video(lp, out_dir)?,
    };
    let ff = tool_bin("ffmpeg");
    let filter = format!(
        "fps={fps},scale={w}:-2:flags=lanczos,split [a][b];[a]palettegen[p];[b][p]paletteuse",
        w = width.max(64)
    );
    let status = Command::new(&ff)
        .args(["-y", "-loglevel", "error", "-i"]).arg(&mp4)
        .args(["-vf", &filter]).arg(&out)
        .status().with_context(|| format!("spawn {}", ff.display()))?;
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }
    if matches!(lp.kind, LiveKind::EmbeddedMp4) { let _ = std::fs::remove_file(&mp4); }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[test]
    fn sibling_pair_detected_for_heic_plus_mov() {
        let tmp = tempfile::tempdir().unwrap();
        let heic = tmp.path().join("IMG_0001.HEIC");
        let mov  = tmp.path().join("IMG_0001.MOV");
        std::fs::write(&heic, b"fake heic").unwrap();
        std::fs::write(&mov,  b"fake mov").unwrap();
        let lp = probe(&heic, 7).unwrap();
        assert_eq!(lp.kind, LiveKind::SiblingPair);
        assert_eq!(lp.video_path, mov);
        assert_eq!(lp.still_item_id, 7);
    }

    #[test]
    fn lone_heic_is_not_live() {
        let tmp = tempfile::tempdir().unwrap();
        let heic = tmp.path().join("only.heic");
        std::fs::write(&heic, b"x").unwrap();
        assert!(probe(&heic, 0).is_none());
    }

    #[test]
    fn embedded_mp4_offset_finds_ftyp() {
        let tmp = tempfile::tempdir().unwrap();
        let jpg = tmp.path().join("motion.jpg");
        // Synthesise a JPEG header + filler + an ftyp box near the end.
        let mut data = vec![0xFFu8, 0xD8]; // SOI
        data.extend(std::iter::repeat(0u8).take(2048));
        // Box: length=0x20, type='ftyp', brand='mp42', ...
        data.extend_from_slice(&[0, 0, 0, 0x20]);
        data.extend_from_slice(b"ftyp");
        data.extend_from_slice(b"mp42");
        data.extend(std::iter::repeat(0u8).take(20));
        std::fs::write(&jpg, &data).unwrap();
        let off = detect_embedded_mp4(&jpg).expect("ftyp located");
        let lp = probe(&jpg, 0).unwrap();
        assert_eq!(lp.kind, LiveKind::EmbeddedMp4);
        assert_eq!(lp.video_offset, Some(off));
    }

    #[tokio::test]
    async fn detect_in_library_returns_pair() {
        let (_t, pool) = open_pool().await;
        let tmp = tempfile::tempdir().unwrap();
        let heic = tmp.path().join("A.heic");
        let mov  = tmp.path().join("A.mov");
        std::fs::write(&heic, b"x").unwrap();
        std::fs::write(&mov,  b"y").unwrap();
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(heic.to_string_lossy().as_ref()).execute(&pool).await.unwrap();
        let v = detect_in_library(&pool).await.unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].kind, LiveKind::SiblingPair);
    }
}
