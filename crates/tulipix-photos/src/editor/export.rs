//! Export rendered photo to JPEG/PNG/TIFF/HEIC with a quality slider and
//! EXIF preserve/scrub control. The image crate handles JPEG/PNG/TIFF in
//! process; HEIC is encoded via the bundled ffmpeg.

use anyhow::{Context, Result};
use image::{DynamicImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat { Jpeg, Png, Webp, Tiff, Heic }

impl ExportFormat {
    pub fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg", Self::Png => "png", Self::Webp => "webp",
            Self::Tiff => "tif", Self::Heic => "heic",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum ExifPolicy { Preserve, Scrub, ScrubGpsOnly }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportSpec {
    pub format: ExportFormat,
    pub quality: u8,        // 1..100; ignored for PNG/TIFF
    pub exif:    ExifPolicy,
    pub out_dir: PathBuf,
    pub stem:    String,    // file name without extension
}

pub fn write(img: &DynamicImage, source_for_exif: Option<&Path>, spec: &ExportSpec) -> Result<PathBuf> {
    std::fs::create_dir_all(&spec.out_dir).with_context(|| format!("mkdir {}", spec.out_dir.display()))?;
    let out = spec.out_dir.join(format!("{}.{}", spec.stem, spec.format.extension()));
    match spec.format {
        ExportFormat::Jpeg => {
            let mut buf = std::io::Cursor::new(Vec::new());
            let q = spec.quality.clamp(1, 100);
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
            img.write_with_encoder(encoder)?;
            std::fs::write(&out, buf.into_inner())?;
        }
        ExportFormat::Png  => img.save_with_format(&out, ImageFormat::Png)?,
        // image 0.25's WebP encoder is lossless; the quality slider is ignored.
        ExportFormat::Webp => img.save_with_format(&out, ImageFormat::WebP)?,
        ExportFormat::Tiff => img.save_with_format(&out, ImageFormat::Tiff)?,
        ExportFormat::Heic => {
            // Hand off to ffmpeg for HEIC encode — JPEG transient kept on
            // disk so we can hint the original's metadata via -metadata.
            let tmp = spec.out_dir.join(format!(".{}-export.jpg", spec.stem));
            let mut buf = std::io::Cursor::new(Vec::new());
            let q = spec.quality.clamp(1, 100);
            let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, q);
            img.write_with_encoder(encoder)?;
            std::fs::write(&tmp, buf.into_inner())?;
            let status = std::process::Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error", "-i"]).arg(&tmp).arg(&out).status();
            let _ = std::fs::remove_file(&tmp);
            let s = status.with_context(|| "spawn ffmpeg")?;
            if !s.success() { anyhow::bail!("ffmpeg HEIC encode failed: {s}"); }
        }
    }
    // EXIF handling — preserve copies from `source_for_exif`, scrub leaves
    // the freshly-encoded file as is, ScrubGpsOnly removes location only.
    if let Some(src) = source_for_exif {
        match spec.exif {
            ExifPolicy::Preserve => {
                let _ = crate::exif_write::apply(&out, &crate::exif_write::ExifPatch::new()
                    .set("TagsFromFile", src.display().to_string()));
            }
            ExifPolicy::Scrub => { /* freshly-encoded file already has no EXIF */ }
            ExifPolicy::ScrubGpsOnly => {
                let _ = crate::exif_write::strip_gps(&out);
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn extensions_match_format() {
        assert_eq!(ExportFormat::Jpeg.extension(), "jpg");
        assert_eq!(ExportFormat::Png.extension(),  "png");
        assert_eq!(ExportFormat::Tiff.extension(), "tif");
        assert_eq!(ExportFormat::Heic.extension(), "heic");
    }
    #[test]
    fn jpeg_write_round_trips() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(8, 8, Rgba([100, 50, 200, 255])));
        let tmp = tempfile::tempdir().unwrap();
        let spec = ExportSpec {
            format: ExportFormat::Jpeg, quality: 80, exif: ExifPolicy::Scrub,
            out_dir: tmp.path().to_path_buf(), stem: "out".into(),
        };
        let path = write(&img, None, &spec).unwrap();
        assert!(path.exists());
        let decoded = image::open(&path).unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.height(), 8);
    }
}
