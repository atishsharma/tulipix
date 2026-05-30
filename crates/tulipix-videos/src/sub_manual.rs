//! Manual subtitle file picker — fallback when nothing auto-loaded.
//!
//! Sniffs the chosen file's format from contents (the user may have given a
//! .txt that's actually SRT) and either copies it next to the video as a
//! sidecar (so future autopicks see it) or registers it as an external track.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DetectedFormat { Srt, Vtt, Ass, Sub, Unknown }

impl DetectedFormat {
    pub fn extension(self) -> Option<&'static str> {
        match self {
            Self::Srt => Some("srt"), Self::Vtt => Some("vtt"),
            Self::Ass => Some("ass"), Self::Sub => Some("sub"),
            Self::Unknown => None,
        }
    }
}

/// Read up to 8 KiB and decide which format the bytes resemble. Independent
/// of the file extension on disk — the user may have renamed it.
pub fn sniff(path: &Path) -> Result<DetectedFormat> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buf = [0u8; 8192];
    let n = f.read(&mut buf)?;
    let head = String::from_utf8_lossy(&buf[..n]);
    let trimmed = head.trim_start();
    if trimmed.starts_with("WEBVTT") { return Ok(DetectedFormat::Vtt); }
    if trimmed.starts_with("[Script Info]") || trimmed.starts_with("[V4+ Styles]") {
        return Ok(DetectedFormat::Ass);
    }
    // SRT signature: cue number, then a timecode like 00:00:00,000 --> 00:00:01,000.
    if has_srt_timecode(trimmed) { return Ok(DetectedFormat::Srt); }
    // MicroDVD / SubViewer .sub: {start_frame}{end_frame}text OR HH:MM:SS.FF
    if trimmed.starts_with('{') && trimmed.contains('}') { return Ok(DetectedFormat::Sub); }
    Ok(DetectedFormat::Unknown)
}

fn has_srt_timecode(s: &str) -> bool {
    s.lines().take(8).any(|l| {
        let l = l.trim();
        l.contains(" --> ") && l.chars().filter(|c| *c == ':').count() >= 4
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManualPick {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub format: DetectedFormat,
    pub copied: bool,
}

/// Drop the user's pick next to the video. Renames the extension to the
/// sniffed format if the user gave a `.txt` (or anything else). If the file
/// is already in-place beside the video, returns the existing path without
/// copying.
pub fn install_alongside(video: &Path, picked: &Path, language: Option<&str>) -> Result<ManualPick> {
    let format = sniff(picked)?;
    let ext = format.extension().unwrap_or("srt");
    let stem = video.file_stem().and_then(|s| s.to_str()).unwrap_or("video");
    let parent = video.parent().unwrap_or(Path::new("."));
    let dest_name = match language {
        Some(l) if !l.is_empty() => format!("{stem}.{l}.{ext}"),
        _ => format!("{stem}.{ext}"),
    };
    let destination = parent.join(dest_name);
    let copied = if destination == picked { false } else {
        std::fs::copy(picked, &destination)
            .with_context(|| format!("copy {} → {}", picked.display(), destination.display()))?;
        true
    };
    Ok(ManualPick { source: picked.to_path_buf(), destination, format, copied })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &str) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, body).unwrap();
        p
    }

    #[test]
    fn sniff_recognises_webvtt() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "x.txt", "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHi\n");
        assert_eq!(sniff(&p).unwrap(), DetectedFormat::Vtt);
    }

    #[test]
    fn sniff_recognises_srt() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "x.txt", "1\n00:00:00,000 --> 00:00:01,000\nHi\n");
        assert_eq!(sniff(&p).unwrap(), DetectedFormat::Srt);
    }

    #[test]
    fn sniff_recognises_ass() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "x.txt", "[Script Info]\nTitle: ...\n");
        assert_eq!(sniff(&p).unwrap(), DetectedFormat::Ass);
    }

    #[test]
    fn sniff_unknown_for_random_text() {
        let tmp = tempfile::tempdir().unwrap();
        let p = write(tmp.path(), "x.txt", "just some prose without timecodes");
        assert_eq!(sniff(&p).unwrap(), DetectedFormat::Unknown);
    }

    #[test]
    fn install_copies_with_language_tag() {
        let tmp = tempfile::tempdir().unwrap();
        let video  = write(tmp.path(), "Movie.mkv", "");
        let picked = write(tmp.path(), "external.txt", "1\n00:00:00,000 --> 00:00:01,000\nHi\n");
        let r = install_alongside(&video, &picked, Some("en")).unwrap();
        assert_eq!(r.format, DetectedFormat::Srt);
        assert!(r.destination.file_name().unwrap().to_string_lossy().ends_with("Movie.en.srt"));
        assert!(r.copied);
        assert!(r.destination.exists());
    }

    #[test]
    fn install_no_copy_when_already_in_place() {
        let tmp = tempfile::tempdir().unwrap();
        let video = write(tmp.path(), "Movie.mkv", "");
        let sib   = write(tmp.path(), "Movie.srt", "1\n00:00:00,000 --> 00:00:01,000\nHi\n");
        let r = install_alongside(&video, &sib, None).unwrap();
        assert!(!r.copied);
        assert_eq!(r.destination, sib);
    }
}
