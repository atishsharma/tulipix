//! ffprobe-driven video metadata extraction.
//!
//! Calls the bundled `ffprobe` (next to ffmpeg in `resources/bin/<os-arch>/`)
//! with `-print_format json -show_format -show_streams` and parses the
//! response into a strongly-typed `VideoFacts`. The full ffprobe payload is
//! kept opaque — only the fields the rest of the section needs are surfaced.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::process::Command;
use tulipix_core::thumbs::tool_bin;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VideoFacts {
    pub duration_s: Option<f64>,
    pub container: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub fps: Option<f64>,
    pub bitrate: Option<i64>,
    pub hdr: Option<String>,
    pub color_primaries: Option<String>,
    pub color_transfer: Option<String>,
    pub audio_channels: Option<i64>,
    pub audio_sample_hz: Option<i64>,
}

/// Run `ffprobe` on `path` and return parsed facts. Returns default-empty
/// facts (rather than erroring) on non-zero exit so the indexer keeps going.
pub fn probe(path: &Path) -> Result<VideoFacts> {
    let bin = tool_bin("ffprobe");
    let out = Command::new(&bin)
        .args([
            "-loglevel", "error",
            "-print_format", "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .with_context(|| format!("spawn {}", bin.display()))?;
    if !out.status.success() {
        anyhow::bail!("ffprobe exit {} — {}", out.status, String::from_utf8_lossy(&out.stderr));
    }
    parse_ffprobe_json(&out.stdout)
}

pub fn parse_ffprobe_json(bytes: &[u8]) -> Result<VideoFacts> {
    let v: serde_json::Value = serde_json::from_slice(bytes).context("decode ffprobe json")?;
    let mut f = VideoFacts::default();

    if let Some(fmt) = v.get("format") {
        f.container = fmt.get("format_name").and_then(|x| x.as_str()).map(str::to_string);
        f.duration_s = fmt.get("duration").and_then(|x| x.as_str()).and_then(|s| s.parse().ok());
        f.bitrate = fmt.get("bit_rate").and_then(|x| x.as_str()).and_then(|s| s.parse().ok());
    }

    if let Some(streams) = v.get("streams").and_then(|x| x.as_array()) {
        for s in streams {
            match s.get("codec_type").and_then(|x| x.as_str()) {
                Some("video") if f.video_codec.is_none() => {
                    f.video_codec = s.get("codec_name").and_then(|x| x.as_str()).map(str::to_string);
                    f.width  = s.get("width").and_then(|x| x.as_i64());
                    f.height = s.get("height").and_then(|x| x.as_i64());
                    f.fps    = parse_rate(s.get("avg_frame_rate"))
                        .or_else(|| parse_rate(s.get("r_frame_rate")));
                    f.color_primaries = s.get("color_primaries").and_then(|x| x.as_str()).map(str::to_string);
                    f.color_transfer  = s.get("color_transfer").and_then(|x| x.as_str()).map(str::to_string);
                    f.hdr = classify_hdr(s);
                }
                Some("audio") if f.audio_codec.is_none() => {
                    f.audio_codec = s.get("codec_name").and_then(|x| x.as_str()).map(str::to_string);
                    f.audio_channels = s.get("channels").and_then(|x| x.as_i64());
                    f.audio_sample_hz = s.get("sample_rate")
                        .and_then(|x| x.as_str())
                        .and_then(|s| s.parse().ok());
                }
                _ => {}
            }
        }
    }
    Ok(f)
}

fn parse_rate(v: Option<&serde_json::Value>) -> Option<f64> {
    let s = v?.as_str()?;
    let (num, den) = s.split_once('/')?;
    let n: f64 = num.parse().ok()?;
    let d: f64 = den.parse().ok()?;
    if d == 0.0 { return None; }
    Some(n / d)
}

fn classify_hdr(stream: &serde_json::Value) -> Option<String> {
    // Dolby Vision is signalled by a side_data entry with side_data_type
    // "DOVI configuration record".
    if let Some(sds) = stream.get("side_data_list").and_then(|x| x.as_array()) {
        for sd in sds {
            if sd.get("side_data_type").and_then(|x| x.as_str()) == Some("DOVI configuration record") {
                return Some("dolby_vision".into());
            }
            // HDR10+ comes through as "HDR Dynamic Metadata SMPTE2094-40 (HDR10+)".
            if sd.get("side_data_type").and_then(|x| x.as_str())
                .map(|s| s.contains("HDR10+")).unwrap_or(false) {
                return Some("hdr10+".into());
            }
        }
    }
    let transfer = stream.get("color_transfer").and_then(|x| x.as_str());
    match transfer {
        Some("smpte2084") => Some("hdr10".into()),
        Some("arib-std-b67") => Some("hlg".into()),
        Some(_) => Some("sdr".into()),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimum_payload() {
        let json = br#"{
            "format": { "format_name": "mov,mp4,m4a", "duration": "12.5", "bit_rate": "8000000" },
            "streams": [
                { "codec_type": "video", "codec_name": "h264",
                  "width": 1920, "height": 1080,
                  "avg_frame_rate": "30000/1001",
                  "color_transfer": "bt709" },
                { "codec_type": "audio", "codec_name": "aac",
                  "channels": 2, "sample_rate": "48000" }
            ]
        }"#;
        let f = parse_ffprobe_json(json).unwrap();
        assert_eq!(f.container.as_deref(), Some("mov,mp4,m4a"));
        assert_eq!(f.duration_s, Some(12.5));
        assert_eq!(f.bitrate, Some(8_000_000));
        assert_eq!(f.video_codec.as_deref(), Some("h264"));
        assert_eq!(f.width, Some(1920));
        assert_eq!(f.height, Some(1080));
        assert!(f.fps.unwrap() > 29.9 && f.fps.unwrap() < 30.0);
        assert_eq!(f.hdr.as_deref(), Some("sdr"));
        assert_eq!(f.audio_codec.as_deref(), Some("aac"));
        assert_eq!(f.audio_channels, Some(2));
        assert_eq!(f.audio_sample_hz, Some(48_000));
    }

    #[test]
    fn classifies_hdr10_from_transfer() {
        let json = br#"{
            "format": {},
            "streams": [
                { "codec_type": "video", "codec_name": "hevc",
                  "width": 3840, "height": 2160, "color_transfer": "smpte2084" }
            ]
        }"#;
        let f = parse_ffprobe_json(json).unwrap();
        assert_eq!(f.hdr.as_deref(), Some("hdr10"));
    }

    #[test]
    fn classifies_dolby_vision_from_side_data() {
        let json = br#"{
            "format": {},
            "streams": [
                { "codec_type": "video", "codec_name": "hevc",
                  "side_data_list": [
                    { "side_data_type": "DOVI configuration record" }
                  ]
                }
            ]
        }"#;
        let f = parse_ffprobe_json(json).unwrap();
        assert_eq!(f.hdr.as_deref(), Some("dolby_vision"));
    }

    #[test]
    fn rate_parses_fraction() {
        assert!((parse_rate(Some(&serde_json::json!("24/1"))).unwrap() - 24.0).abs() < 1e-9);
        assert!(parse_rate(Some(&serde_json::json!("0/0"))).is_none());
    }
}
