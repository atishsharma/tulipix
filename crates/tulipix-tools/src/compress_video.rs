//! `np.p4.tools.compress.video` — target size/bitrate/CRF; H.264/H.265/AV1;
//! two-pass.
//!
//! Builds the ffmpeg argv for either quality mode (CRF) or target-size mode
//! (computes the bitrate from target size / duration, optionally two-pass).

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Codec { H264, H265, Av1 }

impl Codec {
    pub fn encoder(self) -> &'static str {
        match self { Codec::H264 => "libx264", Codec::H265 => "libx265", Codec::Av1 => "libaom-av1" }
    }
}

/// Video bitrate (bps) to hit `target_bytes` over `duration_s`, reserving
/// `audio_bps` for audio.
pub fn target_video_bitrate(target_bytes: i64, duration_s: f64, audio_bps: i64) -> i64 {
    if duration_s <= 0.0 { return 0; }
    let total_bps = (target_bytes as f64 * 8.0 / duration_s) as i64;
    (total_bps - audio_bps).max(100_000)
}

/// CRF (quality) mode argv.
pub fn crf_args(input: &str, codec: Codec, crf: u8, out: &str) -> Vec<String> {
    vec![
        "-i".into(), input.into(),
        "-c:v".into(), codec.encoder().into(),
        "-crf".into(), crf.min(63).to_string(),
        "-c:a".into(), "copy".into(),
        out.into(),
    ]
}

/// Two-pass target-bitrate argv (returns both passes). Pass 1 → null muxer.
pub fn two_pass_args(input: &str, codec: Codec, video_bps: i64, out: &str) -> (Vec<String>, Vec<String>) {
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let pass1 = vec![
        "-y".into(), "-i".into(), input.into(),
        "-c:v".into(), codec.encoder().into(),
        "-b:v".into(), video_bps.to_string(),
        "-pass".into(), "1".into(), "-an".into(), "-f".into(), "null".into(),
        null.into(),
    ];
    let pass2 = vec![
        "-i".into(), input.into(),
        "-c:v".into(), codec.encoder().into(),
        "-b:v".into(), video_bps.to_string(),
        "-pass".into(), "2".into(), "-c:a".into(), "aac".into(),
        out.into(),
    ];
    (pass1, pass2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_from_target() {
        // 50 MB over 100 s, 128 kbps audio
        let bps = target_video_bitrate(50_000_000, 100.0, 128_000);
        assert!(bps > 3_000_000 && bps < 4_000_000);
        assert_eq!(target_video_bitrate(1, 0.0, 0), 0);
    }

    #[test]
    fn crf_and_two_pass() {
        let c = crf_args("in.mp4", Codec::Av1, 30, "o.mkv");
        assert!(c.contains(&"libaom-av1".to_string()));
        let (p1, p2) = two_pass_args("in.mp4", Codec::H265, 2_000_000, "o.mp4");
        assert!(p1.contains(&"1".to_string()) && p1.contains(&"-pass".to_string()));
        assert!(p2.windows(2).any(|w| w == ["-pass", "2"]));
        assert!(p1.contains(&"libx265".to_string()));
    }
}
