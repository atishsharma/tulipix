//! `np.p4.tools.trim` — keyframe-aligned lossless (`-c copy`) or transcode for
//! precision.
//!
//! Lossless trim must start on a keyframe; if the requested start isn't near
//! one, we either snap to the nearest preceding keyframe (lossless) or
//! transcode for frame-accurate cut. This decides which and builds the argv.

/// Snap `start_s` back to the nearest keyframe at or before it.
pub fn snap_to_keyframe(start_s: f64, keyframes_s: &[f64]) -> f64 {
    keyframes_s.iter().copied().filter(|k| *k <= start_s).fold(0.0, f64::max)
}

/// Is the requested start close enough to a keyframe (within `tol_s`) to cut
/// losslessly without a visible jump?
pub fn can_lossless(start_s: f64, keyframes_s: &[f64], tol_s: f64) -> bool {
    keyframes_s.iter().any(|k| (k - start_s).abs() <= tol_s)
}

/// Lossless trim argv (`-ss` before input, stream copy, keyframe-snapped).
pub fn lossless_args(input: &str, start_s: f64, end_s: f64, out: &str) -> Vec<String> {
    vec![
        "-ss".into(), format!("{start_s}"),
        "-to".into(), format!("{end_s}"),
        "-i".into(), input.into(),
        "-c".into(), "copy".into(),
        "-avoid_negative_ts".into(), "make_zero".into(),
        out.into(),
    ]
}

/// Frame-accurate transcode trim (`-ss` after input → decode-accurate).
pub fn precise_args(input: &str, start_s: f64, end_s: f64, out: &str) -> Vec<String> {
    vec![
        "-i".into(), input.into(),
        "-ss".into(), format!("{start_s}"),
        "-to".into(), format!("{end_s}"),
        "-c:v".into(), "libx264".into(), "-c:a".into(), "aac".into(),
        out.into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframe_snap_and_choice() {
        let kf = [0.0, 5.0, 10.0, 15.0];
        assert_eq!(snap_to_keyframe(7.3, &kf), 5.0);
        assert!(can_lossless(10.1, &kf, 0.2));
        assert!(!can_lossless(7.5, &kf, 0.2));
    }

    #[test]
    fn argv_ss_placement() {
        // lossless: -ss before -i
        let l = lossless_args("in.mp4", 5.0, 10.0, "o.mp4");
        let ss = l.iter().position(|x| x == "-ss").unwrap();
        let i = l.iter().position(|x| x == "-i").unwrap();
        assert!(ss < i);
        // precise: -ss after -i
        let p = precise_args("in.mp4", 5.0, 10.0, "o.mp4");
        let ss = p.iter().position(|x| x == "-ss").unwrap();
        let i = p.iter().position(|x| x == "-i").unwrap();
        assert!(ss > i);
    }
}
