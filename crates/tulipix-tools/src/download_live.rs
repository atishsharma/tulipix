//! `np.p4.tools.download.live` — live-stream record with scheduled start/stop.
//!
//! Records a live stream from its start (`--live-from-start`) and auto-segments
//! into fixed-duration files. Owns the argv and the schedule gate (is it time
//! to start? should we stop?).

/// yt-dlp argv to record a live stream from the beginning, segmenting every
/// `segment_s` seconds via an ffmpeg output template.
pub fn record_args(url: &str, out_template: &str, segment_s: u32) -> Vec<String> {
    vec![
        "--live-from-start".into(),
        "--no-part".into(),
        "-o".into(), out_template.into(),
        // hand ffmpeg segmenting through yt-dlp's downloader args
        "--downloader".into(), "ffmpeg".into(),
        "--downloader-args".into(),
        format!("ffmpeg_o:-f segment -segment_time {segment_s} -reset_timestamps 1"),
        url.into(),
    ]
}

/// Is it time to start? `now ≥ start`.
pub fn should_start(start_unix: i64, now_unix: i64) -> bool { now_unix >= start_unix }

/// Should recording stop? `stop > 0 && now ≥ stop`.
pub fn should_stop(stop_unix: i64, now_unix: i64) -> bool { stop_unix > 0 && now_unix >= stop_unix }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schedule_gates() {
        assert!(should_start(1000, 1000));
        assert!(!should_start(1000, 999));
        assert!(should_stop(2000, 2001));
        assert!(!should_stop(0, 999_999)); // 0 = run indefinitely
    }

    #[test]
    fn argv_segments() {
        let a = record_args("https://y/live", "%(title)s-%(autonumber)d.%(ext)s", 600);
        assert!(a.contains(&"--live-from-start".to_string()));
        assert!(a.iter().any(|s| s.contains("segment_time 600")));
    }
}
