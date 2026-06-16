//! Sprite-sheet hover thumbnails under the scrubber.
//!
//! A sheet is one tall PNG with `tiles_per_row` × `rows` frames. ffmpeg
//! samples the source at `interval_s` and tiles them with `-vf
//! fps=…,scale=…,tile=…`. Lookup at runtime maps a scrubber timestamp to
//! the right tile rectangle.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_TILE_W: u32 = 160;
pub const DEFAULT_TILE_H: u32 = 90;
pub const DEFAULT_TILES_PER_ROW: u32 = 10;
pub const DEFAULT_INTERVAL_S: f64 = 5.0;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpriteSheet {
    pub path: PathBuf,
    pub tile_w: u32,
    pub tile_h: u32,
    pub tiles_per_row: u32,
    pub rows: u32,
    pub interval_s: f64,
    pub total_frames: u32,
}

impl SpriteSheet {
    /// Find the tile rectangle (x, y, w, h) for a given timestamp.
    pub fn tile_for(&self, t_seconds: f64) -> (u32, u32, u32, u32) {
        if self.total_frames == 0 {
            return (0, 0, self.tile_w, self.tile_h);
        }
        let idx = ((t_seconds / self.interval_s).max(0.0)) as u32;
        let idx = idx.min(self.total_frames.saturating_sub(1));
        let col = idx % self.tiles_per_row;
        let row = idx / self.tiles_per_row;
        (col * self.tile_w, row * self.tile_h, self.tile_w, self.tile_h)
    }

    pub fn frame_count_for(duration_s: f64, interval_s: f64) -> u32 {
        if interval_s <= 0.0 { return 0; }
        ((duration_s / interval_s).ceil() as i64).max(1) as u32
    }
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

pub fn render_sheet(src: &Path, duration_s: f64, out: &Path) -> Result<SpriteSheet> {
    if let Some(p) = out.parent() { std::fs::create_dir_all(p)?; }
    let interval = DEFAULT_INTERVAL_S;
    let total = SpriteSheet::frame_count_for(duration_s, interval);
    let cols = DEFAULT_TILES_PER_ROW;
    let rows = total.div_ceil(cols);
    let fps_filter = format!(
        "fps=1/{interval},scale={tw}:{th}:force_original_aspect_ratio=decrease,tile={cols}x{rows}",
        interval = interval, tw = DEFAULT_TILE_W, th = DEFAULT_TILE_H, cols = cols, rows = rows,
    );
    let ff = bundled_bin("ffmpeg");
    let mut cmd = Command::new(&ff);
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(src)
        .args(["-vf", &fps_filter, "-frames:v", "1"]).arg(out);
    // Suppress the console window the child would pop on Windows (GUI app).
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let status = cmd.status().with_context(|| format!("spawn {}", ff.display()))?;
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }
    Ok(SpriteSheet {
        path: out.to_path_buf(),
        tile_w: DEFAULT_TILE_W, tile_h: DEFAULT_TILE_H,
        tiles_per_row: cols, rows,
        interval_s: interval,
        total_frames: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(total: u32) -> SpriteSheet {
        SpriteSheet {
            path: PathBuf::from("/x.png"),
            tile_w: DEFAULT_TILE_W, tile_h: DEFAULT_TILE_H,
            tiles_per_row: DEFAULT_TILES_PER_ROW,
            rows: total.div_ceil(DEFAULT_TILES_PER_ROW),
            interval_s: DEFAULT_INTERVAL_S,
            total_frames: total,
        }
    }

    #[test]
    fn frame_count_uses_ceiling() {
        assert_eq!(SpriteSheet::frame_count_for(0.0, 5.0), 1);
        assert_eq!(SpriteSheet::frame_count_for(20.0, 5.0), 4);
        assert_eq!(SpriteSheet::frame_count_for(21.0, 5.0), 5);
    }

    #[test]
    fn tile_for_maps_seconds_to_rect() {
        let s = mk(30);
        let (x0, y0, w, h) = s.tile_for(0.0);
        assert_eq!((x0, y0, w, h), (0, 0, DEFAULT_TILE_W, DEFAULT_TILE_H));
        let (x1, y1, _, _) = s.tile_for(7.5); // interval 5 → idx 1
        assert_eq!(x1, DEFAULT_TILE_W);
        assert_eq!(y1, 0);
        // beyond the last tile clamps to the last frame
        let (xN, yN, _, _) = s.tile_for(1_000.0);
        let expected_idx = 29u32;
        let col = expected_idx % DEFAULT_TILES_PER_ROW;
        let row = expected_idx / DEFAULT_TILES_PER_ROW;
        assert_eq!((xN, yN), (col * DEFAULT_TILE_W, row * DEFAULT_TILE_H));
    }

    #[test]
    fn empty_sheet_is_safe() {
        let s = mk(0);
        let (x, y, _, _) = s.tile_for(100.0);
        assert_eq!((x, y), (0, 0));
    }
}
