//! Folder browse view — disk hierarchy 1:1.
//!
//! Walks the `items` proxy table, groups by parent directory, and exposes the
//! first video in each folder (sorted by name) as the cover candidate. The
//! actual cover frame is rendered separately via ffmpeg, but this module
//! decides *which* file to render for.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderEntry {
    pub path: PathBuf,
    pub item_count: i64,
    pub cover_item_id: Option<i64>,
    pub cover_path: Option<PathBuf>,
}

pub async fn list_folders(pool: &SqlitePool) -> Result<Vec<FolderEntry>> {
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT items.id, items.abs_path FROM items
         WHERE items.section = 'videos' AND items.missing_since IS NULL
         ORDER BY items.abs_path",
    ).fetch_all(pool).await?;

    let mut by_folder: BTreeMap<PathBuf, Vec<(i64, PathBuf)>> = BTreeMap::new();
    for (id, p) in rows {
        let path = PathBuf::from(p);
        if let Some(parent) = path.parent() {
            by_folder.entry(parent.to_path_buf()).or_default().push((id, path));
        }
    }
    let mut out = Vec::with_capacity(by_folder.len());
    for (folder, mut items) in by_folder {
        items.sort_by(|a, b| a.1.cmp(&b.1));
        let cover = items.first().cloned();
        out.push(FolderEntry {
            path: folder,
            item_count: items.len() as i64,
            cover_item_id: cover.as_ref().map(|(id, _)| *id),
            cover_path: cover.map(|(_, p)| p),
        });
    }
    Ok(out)
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

/// Render a per-folder cover by grabbing the cover video's 10% frame.
/// Returns the cached cover path (`<out_dir>/<sha>.jpg`).
pub fn render_cover(src: &Path, duration_s: Option<f64>, out_dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let ts = duration_s.map(|d| d * 0.10).unwrap_or(5.0);
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("cover");
    let out = out_dir.join(format!("{stem}.jpg"));
    let ff = bundled_bin("ffmpeg");
    let status = Command::new(&ff)
        .args(["-y", "-loglevel", "error", "-ss", &format!("{ts:.3}"), "-i"]).arg(src)
        .args(["-vframes", "1", "-q:v", "3"]).arg(&out)
        .status().with_context(|| format!("spawn {}", ff.display()))?;
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    #[tokio::test]
    async fn list_groups_by_parent_directory() {
        let (_t, pool) = open_pool().await;
        for p in ["/movies/a.mkv", "/movies/b.mkv", "/shows/s/e1.mkv"] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'videos', 0, 0)")
                .bind(p).execute(&pool).await.unwrap();
        }
        let f = list_folders(&pool).await.unwrap();
        let counts: BTreeMap<&str, i64> = f.iter()
            .map(|e| (e.path.to_str().unwrap(), e.item_count))
            .collect();
        assert_eq!(counts["/movies"], 2);
        assert_eq!(counts["/shows/s"], 1);
    }

    #[tokio::test]
    async fn cover_picks_first_alphabetical() {
        let (_t, pool) = open_pool().await;
        for p in ["/x/b.mkv", "/x/a.mkv", "/x/c.mkv"] {
            sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, 0, 1, 0, 'videos', 0, 0)")
                .bind(p).execute(&pool).await.unwrap();
        }
        let f = list_folders(&pool).await.unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].cover_path.as_ref().unwrap().to_str().unwrap(), "/x/a.mkv");
    }
}
