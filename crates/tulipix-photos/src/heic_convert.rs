//! Batch HEIC → JPEG/PNG conversion.
//!
//! Decodes via bundled ffmpeg (BtbN GPL build ships libheif) into the target
//! raster format, then copies every readable EXIF tag from the source with
//! `exiftool -tagsFromFile` so dates, GPS, camera identity and orientation all
//! survive the round-trip. Quality knob applies to JPEG output; PNG is lossless.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeicTarget {
    Jpeg,
    Png,
}

impl HeicTarget {
    pub fn extension(self) -> &'static str {
        match self { Self::Jpeg => "jpg", Self::Png => "png" }
    }
}

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    /// JPEG quality (1-100); ignored for PNG.
    pub jpeg_quality: u8,
    /// If true, overwrite any pre-existing output file.
    pub overwrite: bool,
    /// Copy EXIF from source via exiftool. Set false to skip exiftool.
    pub copy_exif: bool,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        Self { jpeg_quality: 92, overwrite: false, copy_exif: true }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConvertReport {
    pub source: PathBuf,
    pub output: PathBuf,
    pub bytes_out: u64,
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

pub fn is_heic(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| {
            let e = e.to_ascii_lowercase();
            e == "heic" || e == "heif"
        })
        .unwrap_or(false)
}

/// Convert one HEIC into `out_dir` using `target` format. Output filename is
/// `<stem>.<ext>`. Returns the resulting path + size.
pub fn convert_one(
    src: &Path,
    out_dir: &Path,
    target: HeicTarget,
    opts: &ConvertOptions,
) -> Result<ConvertReport> {
    if !src.is_file() { anyhow::bail!("not a file: {}", src.display()); }
    std::fs::create_dir_all(out_dir)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
    let output = out_dir.join(format!("{stem}.{}", target.extension()));
    if output.exists() && !opts.overwrite {
        anyhow::bail!("output exists: {} (set overwrite=true)", output.display());
    }

    let ff = bundled_bin("ffmpeg");
    let mut c = Command::new(&ff);
    c.args(["-y", "-loglevel", "error", "-i"]);
    c.arg(src);
    match target {
        HeicTarget::Jpeg => {
            let q = opts.jpeg_quality.clamp(1, 100);
            // ffmpeg JPEG qscale: 2 (best) .. 31 (worst). Map 100→2, 1→31.
            let qscale = (((100 - q as i32) * 29) / 99 + 2).clamp(2, 31);
            c.args(["-q:v", &qscale.to_string()]);
        }
        HeicTarget::Png => {
            c.args(["-compression_level", "6"]);
        }
    }
    c.arg(&output);
    let status = c.status().with_context(|| format!("spawn {}", ff.display()))?;
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }

    if opts.copy_exif {
        let _ = copy_exif(src, &output);
    }
    let bytes_out = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    Ok(ConvertReport { source: src.to_path_buf(), output, bytes_out })
}

/// Convert many HEIC sources. Errors per-file are collected; the batch never
/// aborts on the first failure.
pub fn convert_batch(
    sources: &[PathBuf],
    out_dir: &Path,
    target: HeicTarget,
    opts: &ConvertOptions,
) -> (Vec<ConvertReport>, Vec<(PathBuf, String)>) {
    let mut ok = Vec::new();
    let mut err = Vec::new();
    for s in sources {
        if !is_heic(s) {
            err.push((s.clone(), "not a HEIC/HEIF file".into()));
            continue;
        }
        match convert_one(s, out_dir, target, opts) {
            Ok(r) => ok.push(r),
            Err(e) => err.push((s.clone(), e.to_string())),
        }
    }
    (ok, err)
}

fn copy_exif(src: &Path, dst: &Path) -> Result<()> {
    let exiftool = bundled_bin("exiftool");
    let mut c = Command::new(&exiftool);
    c.arg("-overwrite_original_in_place");
    c.arg("-tagsFromFile");
    c.arg(src);
    c.arg("-all:all");
    c.arg("-orientation=1");
    c.arg(dst);
    let out = c.output().with_context(|| format!("spawn {}", exiftool.display()))?;
    if !out.status.success() {
        anyhow::bail!("exiftool exit {} — {}", out.status, String::from_utf8_lossy(&out.stderr));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_dispatch() {
        assert_eq!(HeicTarget::Jpeg.extension(), "jpg");
        assert_eq!(HeicTarget::Png.extension(), "png");
    }

    #[test]
    fn is_heic_recognises_both_extensions() {
        assert!(is_heic(Path::new("/x.heic")));
        assert!(is_heic(Path::new("/x.HEIF")));
        assert!(!is_heic(Path::new("/x.jpg")));
    }

    #[test]
    fn defaults_are_sensible() {
        let d = ConvertOptions::default();
        assert_eq!(d.jpeg_quality, 92);
        assert!(!d.overwrite);
        assert!(d.copy_exif);
    }

    #[test]
    fn batch_rejects_non_heic_inputs() {
        let tmp = tempfile::tempdir().unwrap();
        let bogus = tmp.path().join("a.jpg");
        std::fs::write(&bogus, b"not heic").unwrap();
        let (ok, err) = convert_batch(
            &[bogus.clone()],
            tmp.path(),
            HeicTarget::Jpeg,
            &ConvertOptions::default(),
        );
        assert!(ok.is_empty());
        assert_eq!(err.len(), 1);
        assert_eq!(err[0].0, bogus);
    }
}
