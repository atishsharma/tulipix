//! `np.p4.tools.split` — split by chapter / time / size / silence.
//!
//! Computes split points and the per-segment `ffmpeg -ss/-t` argv. Chapter and
//! silence boundaries come from probe/detection upstream; time and size splits
//! are computed here from duration/bitrate.

/// Split a `total_s` duration into segments of `every_s`; returns `(start, len)`
/// pairs (last segment may be shorter).
pub fn by_time(total_s: f64, every_s: f64) -> Vec<(f64, f64)> {
    if every_s <= 0.0 || total_s <= 0.0 { return vec![]; }
    let mut segs = Vec::new();
    let mut start = 0.0;
    while start < total_s {
        let len = (total_s - start).min(every_s);
        segs.push((start, len));
        start += every_s;
    }
    segs
}

/// Approximate split duration so each piece is ≤ `target_bytes`, given an
/// average `bitrate_bps`. Returns seconds per segment.
pub fn seconds_for_size(target_bytes: i64, bitrate_bps: i64) -> f64 {
    if bitrate_bps <= 0 { return 0.0; }
    (target_bytes as f64 * 8.0) / bitrate_bps as f64
}

/// ffmpeg argv for one segment (stream-copy, keyframe-aligned by `-ss` before
/// input).
pub fn segment_args(input: &str, start_s: f64, len_s: f64, out: &str) -> Vec<String> {
    vec![
        "-ss".into(), format!("{start_s}"),
        "-i".into(), input.into(),
        "-t".into(), format!("{len_s}"),
        "-c".into(), "copy".into(),
        out.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_split_covers_whole() {
        let segs = by_time(25.0, 10.0);
        assert_eq!(segs.len(), 3);
        assert_eq!(segs[0], (0.0, 10.0));
        assert!((segs[2].1 - 5.0).abs() < 1e-9); // tail
        assert!(by_time(10.0, 0.0).is_empty());
    }

    #[test]
    fn size_to_seconds() {
        // 10 MB target at 1 Mbps → 80 s
        let s = seconds_for_size(10_000_000, 1_000_000);
        assert!((s - 80.0).abs() < 0.01);
    }

    #[test]
    fn segment_argv() {
        let a = segment_args("in.mp4", 10.0, 5.0, "p1.mp4");
        assert!(a.windows(2).any(|w| w == ["-ss", "10"]));
        assert!(a.contains(&"copy".to_string()));
    }
}
