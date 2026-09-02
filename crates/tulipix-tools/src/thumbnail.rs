//! `np.p4.tools.thumbnail` — generate thumbnails + contact sheets.
//!
//! Single-frame thumbnails (at a timestamp) and contact sheets (a grid of
//! frames sampled across the duration). Owns the timestamp sampling + the
//! ffmpeg `tile`/`fps` filter string for the sheet.

/// Evenly-spaced sample timestamps for `count` frames over `duration_s`,
/// centered in each slice to avoid black intro/outro frames.
pub fn sample_times(duration_s: f64, count: usize) -> Vec<f64> {
    if count == 0 || duration_s <= 0.0 { return vec![]; }
    let slice = duration_s / count as f64;
    (0..count).map(|i| (i as f64 + 0.5) * slice).collect()
}

/// Scale to `width` at the shape the video is *watched* at, not the shape it
/// is stored at.
///
/// `scale=W:-1` derives the height from the storage pixels, which is only the
/// right answer when they are square. Anamorphic sources — a DVD rip at
/// 720x576 with SAR 16:15, a lot of broadcast footage — store a 4:3 picture in
/// a 5:4 grid, and `-1` faithfully reproduces the squash. `dar` is the display
/// aspect, so `W/dar` is the height the picture is meant to have; `setsar=1`
/// then tells everything downstream that these pixels really are square.
pub fn display_scale(width: u32) -> String {
    format!("scale={width}:trunc({width}/dar/2)*2,setsar=1")
}

/// ffmpeg argv for one thumbnail at `at_s`, scaled to `width` (height auto).
pub fn thumb_args(input: &str, at_s: f64, width: u32, out: &str) -> Vec<String> {
    vec![
        "-ss".into(), format!("{at_s}"),
        "-i".into(), input.into(),
        "-frames:v".into(), "1".into(),
        "-vf".into(), display_scale(width),
        out.into(),
    ]
}

/// ffmpeg `-vf` filter for a `cols`×`rows` contact sheet sampling `count`
/// frames across the whole file.
pub fn contact_sheet_filter(duration_s: f64, cols: u32, rows: u32, thumb_w: u32) -> String {
    let count = (cols * rows).max(1);
    let fps = count as f64 / duration_s.max(1.0);
    format!("fps={fps:.4},{},tile={cols}x{rows}", display_scale(thumb_w))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_centered_and_spaced() {
        let t = sample_times(100.0, 4);
        assert_eq!(t.len(), 4);
        assert!((t[0] - 12.5).abs() < 1e-9);
        assert!((t[3] - 87.5).abs() < 1e-9);
        assert!(sample_times(10.0, 0).is_empty());
    }

    #[test]
    fn sheet_filter_has_tile() {
        let f = contact_sheet_filter(120.0, 4, 3, 160);
        assert!(f.contains("tile=4x3"));
        // Height off the display aspect, not off the storage pixels: `-1`
        // reproduces an anamorphic source's squash into every cell.
        assert!(f.contains("scale=160:trunc(160/dar/2)*2"));
        assert!(f.contains("setsar=1"));
        assert!(thumb_args("in.mp4", 5.0, 320, "t.jpg").windows(2).any(|w| w == ["-frames:v", "1"]));
    }
}
