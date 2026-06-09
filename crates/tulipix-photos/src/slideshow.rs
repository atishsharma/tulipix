//! Slideshow / movie-from-photos via bundled ffmpeg.
//!
//! Builds a Ken-Burns + crossfade timeline:
//!   * each photo gets `slide_secs` on screen with a slow `zoompan` push,
//!   * adjacent slides crossfade for `xfade_secs`,
//!   * optional music track is overlaid at half volume.
//!
//! This module produces the ffmpeg command + concat manifest; running it
//! is the worker's job. Tests verify the manifest structure.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideshowSpec {
    pub photos: Vec<PathBuf>,
    pub out_path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub slide_secs: f32,
    pub xfade_secs: f32,
    pub music: Option<PathBuf>,
}

impl Default for SlideshowSpec {
    fn default() -> Self {
        Self {
            photos: Vec::new(), out_path: PathBuf::from("slideshow.mp4"),
            width: 1920, height: 1080, fps: 30, slide_secs: 4.0, xfade_secs: 0.8, music: None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FfmpegInvocation {
    pub args: Vec<String>,
}

/// Build the ffmpeg argv. Each photo becomes its own input with `-loop 1
/// -t slide_secs`; the filter graph chains zoompan into xfade. Music, when
/// provided, is the final input mixed at half volume.
pub fn plan(spec: &SlideshowSpec) -> Result<FfmpegInvocation> {
    if spec.photos.is_empty() { anyhow::bail!("slideshow needs at least one photo"); }
    if spec.slide_secs <= 0.0 || spec.xfade_secs < 0.0 { anyhow::bail!("invalid timings"); }
    let mut args: Vec<String> = vec!["-y".into(), "-loglevel".into(), "error".into()];
    for p in &spec.photos {
        args.extend(["-loop".into(), "1".into(), "-t".into(), format!("{:.3}", spec.slide_secs), "-i".into(), p.to_string_lossy().into_owned()]);
    }
    if let Some(m) = &spec.music { args.extend(["-i".into(), m.to_string_lossy().into_owned()]); }

    // Filter graph: zoompan per stream → xfade pairwise.
    let mut graph = String::new();
    for (i, _) in spec.photos.iter().enumerate() {
        let _ = graph.write_fmt(format_args!(
            "[{i}:v]zoompan=z='min(zoom+0.0008,1.15)':d={d}:s={w}x{h}[v{i}];",
            i = i, d = (spec.slide_secs * spec.fps as f32) as u32, w = spec.width, h = spec.height,
        ));
    }
    // Chain xfade.
    if spec.photos.len() == 1 {
        let _ = graph.write_fmt(format_args!("[v0]format=yuv420p[vout]"));
    } else {
        let mut prev = "v0".to_string();
        let mut offset = spec.slide_secs - spec.xfade_secs;
        for i in 1..spec.photos.len() {
            let cur = format!("v{i}");
            let next = if i == spec.photos.len() - 1 { "vout".to_string() } else { format!("x{i}") };
            let _ = graph.write_fmt(format_args!(
                "[{prev}][{cur}]xfade=transition=fade:duration={d}:offset={o:.3}[{next}];",
                prev = prev, cur = cur, d = spec.xfade_secs, o = offset, next = next,
            ));
            prev = next.clone();
            offset += spec.slide_secs - spec.xfade_secs;
        }
    }

    args.push("-filter_complex".into());
    args.push(graph.trim_end_matches(';').to_string());
    args.extend(["-map".into(), "[vout]".into()]);
    if let Some(_) = &spec.music {
        args.extend(["-map".into(), format!("{}:a", spec.photos.len()), "-shortest".into(), "-af".into(), "volume=0.5".into()]);
    }
    args.extend(["-c:v".into(), "libx264".into(), "-pix_fmt".into(), "yuv420p".into(), "-r".into(), spec.fps.to_string()]);
    args.push(spec.out_path.to_string_lossy().into_owned());
    Ok(FfmpegInvocation { args })
}

/// Convenience: run the planned ffmpeg invocation.
pub fn render(spec: &SlideshowSpec) -> Result<PathBuf> {
    let inv = plan(spec)?;
    let status = std::process::Command::new("ffmpeg").args(&inv.args).status()?;
    if !status.success() { anyhow::bail!("ffmpeg slideshow exit {status}"); }
    Ok(spec.out_path.clone())
}

// Use std::fmt::Write so write_fmt is in scope on String.
use std::fmt::Write as _;
fn _ensure_write_in_scope() {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn spec_with(n: usize) -> SlideshowSpec {
        let photos = (0..n).map(|i| PathBuf::from(format!("/p/{i}.jpg"))).collect();
        SlideshowSpec { photos, ..SlideshowSpec::default() }
    }

    #[test]
    fn empty_input_errors() {
        let s = SlideshowSpec::default();
        assert!(plan(&s).is_err());
    }

    #[test]
    fn three_photos_emit_pairwise_xfade() {
        let s = spec_with(3);
        let inv = plan(&s).unwrap();
        let graph = inv.args.iter().find(|a| a.contains("xfade")).unwrap();
        // Two crossfades between three slides.
        let count = graph.matches("xfade=transition=fade").count();
        assert_eq!(count, 2);
        assert!(graph.contains("[vout]"));
    }

    #[test]
    fn music_track_adds_audio_map() {
        let mut s = spec_with(2);
        s.music = Some(PathBuf::from("/tmp/music.mp3"));
        let inv = plan(&s).unwrap();
        let mapped_audio = inv.args.windows(2).any(|w| w[0] == "-map" && w[1].ends_with(":a"));
        assert!(mapped_audio);
    }
}
