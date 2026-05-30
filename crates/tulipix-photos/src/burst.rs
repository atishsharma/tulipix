//! Burst-set detection + auto GIF/MP4 export.
//!
//! Heuristic: ≥3 photos within `BURST_WINDOW_SECS` of each other, from the
//! same camera (`photo_meta.camera_make` + `camera_model`). Detection
//! returns groups of item IDs ordered by `taken_at`. Encoding hands the
//! list to bundled ffmpeg via a generated concat-demuxer manifest.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::path::{Path, PathBuf};

pub const BURST_WINDOW_SECS: i64 = 1;
pub const MIN_BURST_FRAMES: usize = 3;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurstSet {
    pub camera: String,
    pub start_unix: i64,
    pub end_unix: i64,
    pub item_ids: Vec<i64>,
    pub item_paths: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
pub enum BurstOutput { Gif, Mp4 }
impl BurstOutput {
    pub fn extension(self) -> &'static str { match self { Self::Gif => "gif", Self::Mp4 => "mp4" } }
}

pub async fn detect(pool: &SqlitePool) -> Result<Vec<BurstSet>> {
    let rows: Vec<(i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT items.id, items.abs_path, photo_meta.taken_at,
                photo_meta.camera_make, photo_meta.camera_model
         FROM items JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE photo_meta.taken_at IS NOT NULL
           AND items.missing_since IS NULL
         ORDER BY photo_meta.taken_at",
    ).fetch_all(pool).await?;

    let mut bursts: Vec<BurstSet> = Vec::new();
    let mut cur: Option<BurstSet> = None;
    for (id, path, taken, make, model) in rows {
        let cam = format!("{} {}", make.unwrap_or_default(), model.unwrap_or_default()).trim().to_string();
        let push_new = match &cur {
            None => true,
            Some(b) => {
                taken - b.end_unix > BURST_WINDOW_SECS || cam != b.camera
            }
        };
        if push_new {
            if let Some(b) = cur.take() {
                if b.item_ids.len() >= MIN_BURST_FRAMES { bursts.push(b); }
            }
            cur = Some(BurstSet {
                camera: cam,
                start_unix: taken, end_unix: taken,
                item_ids: vec![id], item_paths: vec![path],
            });
        } else if let Some(b) = cur.as_mut() {
            b.end_unix = taken;
            b.item_ids.push(id);
            b.item_paths.push(path);
        }
    }
    if let Some(b) = cur {
        if b.item_ids.len() >= MIN_BURST_FRAMES { bursts.push(b); }
    }
    Ok(bursts)
}

/// Write a concat-demuxer manifest + invoke bundled ffmpeg.
pub fn encode(burst: &BurstSet, output: BurstOutput, fps: u32, out_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let manifest = out_dir.join(format!("burst-{}.concat", burst.start_unix));
    let mut body = String::from("ffconcat version 1.0\n");
    let dur = 1.0 / fps.max(1) as f64;
    for p in &burst.item_paths {
        body.push_str(&format!("file '{}'\nduration {:.3}\n", p.replace('\'', "'\\''"), dur));
    }
    if let Some(last) = burst.item_paths.last() {
        body.push_str(&format!("file '{}'\n", last.replace('\'', "'\\''")));
    }
    std::fs::write(&manifest, body)?;
    let out = out_dir.join(format!("burst-{}.{}", burst.start_unix, output.extension()));
    let status = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-f", "concat", "-safe", "0", "-i"]).arg(&manifest)
        .args(match output {
            BurstOutput::Gif => &["-vf", "fps=10,scale=720:-2:flags=lanczos,split [a][b];[a]palettegen[p];[b][p]paletteuse"][..],
            BurstOutput::Mp4 => &["-c:v", "libx264", "-pix_fmt", "yuv420p", "-crf", "23"][..],
        })
        .arg(&out)
        .status().with_context(|| "spawn ffmpeg")?;
    let _ = std::fs::remove_file(&manifest);
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    async fn add(pool: &SqlitePool, path: &str, taken: i64, cam: &str) -> i64 {
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'photos', 0, 0)")
            .bind(path).execute(pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?").bind(path).fetch_one(pool).await.unwrap();
        sqlx::query("INSERT INTO photo_meta (item_id, taken_at, camera_make, camera_model) VALUES (?, ?, ?, ?)")
            .bind(id).bind(taken).bind("Make").bind(cam).execute(pool).await.unwrap();
        id
    }

    #[tokio::test]
    async fn detect_groups_three_consecutive() {
        let (_t, pool) = open_pool().await;
        for i in 0..3 { add(&pool, &format!("/p/{i}.jpg"), 1000 + i, "Camera1").await; }
        add(&pool, "/q.jpg", 2000, "Camera1").await; // lone shot
        let bursts = detect(&pool).await.unwrap();
        assert_eq!(bursts.len(), 1);
        assert_eq!(bursts[0].item_ids.len(), 3);
    }

    #[tokio::test]
    async fn camera_change_breaks_burst() {
        let (_t, pool) = open_pool().await;
        add(&pool, "/a.jpg", 1000, "Camera1").await;
        add(&pool, "/b.jpg", 1000, "Camera2").await;
        add(&pool, "/c.jpg", 1000, "Camera1").await;
        let bursts = detect(&pool).await.unwrap();
        assert!(bursts.is_empty(), "different cameras shouldn't merge");
    }
}
