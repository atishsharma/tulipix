//! Modern still codecs — JPEG-XL and AVIF write.
//!
//! Both routes go through bundled ffmpeg with `libjxl` (JPEG-XL) and
//! `libsvtav1`/`libaom-av1` (AVIF) encoders. The BtbN GPL build that ships in
//! `resources/bin/` exposes both. EXIF is forwarded with `-map_metadata 0`
//! so the wide-gamut/HDR tags carry through.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModernCodec {
    JpegXl,
    Avif,
}

impl ModernCodec {
    pub fn extension(self) -> &'static str {
        match self { Self::JpegXl => "jxl", Self::Avif => "avif" }
    }
}

#[derive(Debug, Clone)]
pub struct EncodeOptions {
    /// 0 = lossless. For JPEG-XL: distance 0–15 (0 = lossless, ≈1 ≈ visually
    /// lossless). For AVIF: maps to libaom `crf` 0–63 (0 = lossless).
    pub quality: u8,
    /// Encoder speed/effort knob. JXL accepts 1 (slow, best) … 9 (fast).
    /// AVIF maps to libaom `cpu-used` 0 … 9.
    pub effort: u8,
    /// True keeps EXIF metadata; false strips it.
    pub keep_metadata: bool,
    pub overwrite: bool,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { quality: 75, effort: 7, keep_metadata: true, overwrite: false }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncodeReport {
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

pub fn encode(
    src: &Path,
    out_dir: &Path,
    codec: ModernCodec,
    opts: &EncodeOptions,
) -> Result<EncodeReport> {
    if !src.is_file() { anyhow::bail!("not a file: {}", src.display()); }
    std::fs::create_dir_all(out_dir)?;
    let stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("image");
    let output = out_dir.join(format!("{stem}.{}", codec.extension()));
    if output.exists() && !opts.overwrite {
        anyhow::bail!("output exists: {} (set overwrite=true)", output.display());
    }

    let ff = bundled_bin("ffmpeg");
    let mut c = Command::new(&ff);
    c.args(["-y", "-loglevel", "error", "-i"]);
    c.arg(src);
    if opts.keep_metadata { c.args(["-map_metadata", "0"]); }
    else { c.args(["-map_metadata", "-1"]); }
    match codec {
        ModernCodec::JpegXl => {
            let dist = opts.quality.min(15);
            let effort = opts.effort.clamp(1, 9);
            c.args([
                "-c:v", "libjxl",
                "-distance", &dist.to_string(),
                "-effort", &effort.to_string(),
            ]);
        }
        ModernCodec::Avif => {
            let crf = opts.quality.clamp(0, 63);
            let cpu = opts.effort.clamp(0, 9);
            c.args([
                "-c:v", "libaom-av1",
                "-still-picture", "1",
                "-crf", &crf.to_string(),
                "-cpu-used", &cpu.to_string(),
                "-b:v", "0",
            ]);
        }
    }
    c.arg(&output);
    let status = c.status().with_context(|| format!("spawn {}", ff.display()))?;
    if !status.success() { anyhow::bail!("ffmpeg exit {status}"); }
    let bytes_out = std::fs::metadata(&output).map(|m| m.len()).unwrap_or(0);
    Ok(EncodeReport { source: src.to_path_buf(), output, bytes_out })
}

pub fn encode_batch(
    sources: &[PathBuf],
    out_dir: &Path,
    codec: ModernCodec,
    opts: &EncodeOptions,
) -> (Vec<EncodeReport>, Vec<(PathBuf, String)>) {
    let mut ok = Vec::new();
    let mut err = Vec::new();
    for s in sources {
        match encode(s, out_dir, codec, opts) {
            Ok(r) => ok.push(r),
            Err(e) => err.push((s.clone(), e.to_string())),
        }
    }
    (ok, err)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_for_each_codec() {
        assert_eq!(ModernCodec::JpegXl.extension(), "jxl");
        assert_eq!(ModernCodec::Avif.extension(), "avif");
    }

    #[test]
    fn encode_options_defaults() {
        let o = EncodeOptions::default();
        assert_eq!(o.quality, 75);
        assert_eq!(o.effort, 7);
        assert!(o.keep_metadata);
    }

    #[test]
    fn missing_source_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let r = encode(
            Path::new("/no/such/file.png"),
            tmp.path(),
            ModernCodec::JpegXl,
            &EncodeOptions::default(),
        );
        assert!(r.is_err());
    }
}
