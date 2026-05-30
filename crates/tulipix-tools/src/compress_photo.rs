//! `np.p4.tools.compress.photo` — JPEG (mozjpeg) / WebP / AVIF re-encode with
//! side-by-side quality preview.
//!
//! Picks the encoder + quality flag per target format and estimates the output
//! size from a quality→bytes-per-pixel heuristic so the preview can show an
//! expected file size before encoding.

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PhotoFormat { Jpeg, WebP, Avif }

impl PhotoFormat {
    pub fn ext(self) -> &'static str {
        match self { PhotoFormat::Jpeg => "jpg", PhotoFormat::WebP => "webp", PhotoFormat::Avif => "avif" }
    }
}

/// Encoder argv (cjpeg/mozjpeg style for JPEG; ffmpeg-style for WebP/AVIF). The
/// quality is 1..100.
pub fn args(input: &str, fmt: PhotoFormat, quality: u8, out: &str) -> Vec<String> {
    let q = quality.clamp(1, 100);
    match fmt {
        PhotoFormat::Jpeg => vec!["-quality".into(), q.to_string(), "-optimize".into(), "-outfile".into(), out.into(), input.into()],
        PhotoFormat::WebP => vec!["-i".into(), input.into(), "-c:v".into(), "libwebp".into(), "-quality".into(), q.to_string(), out.into()],
        PhotoFormat::Avif => vec!["-i".into(), input.into(), "-c:v".into(), "libaom-av1".into(), "-crf".into(), avif_crf(q).to_string(), out.into()],
    }
}

/// Map a 1..100 quality to an AV1 CRF (lower = better; inverse of quality).
fn avif_crf(quality: u8) -> u8 {
    let q = quality.clamp(1, 100) as u32;
    (63 - (q * 63 / 100)) as u8
}

/// Rough expected output bytes for a `w*h` image at `quality`, per format —
/// drives the preview's size estimate.
pub fn estimate_bytes(w: u32, h: u32, fmt: PhotoFormat, quality: u8) -> u64 {
    let px = (w as u64) * (h as u64);
    let q = quality.clamp(1, 100) as f64 / 100.0;
    let bpp = match fmt {
        PhotoFormat::Jpeg => 0.25 * q + 0.02,
        PhotoFormat::WebP => 0.18 * q + 0.015,
        PhotoFormat::Avif => 0.12 * q + 0.01,
    };
    (px as f64 * bpp) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_per_format() {
        assert!(args("a.png", PhotoFormat::Jpeg, 85, "o.jpg").contains(&"-optimize".to_string()));
        assert!(args("a.png", PhotoFormat::WebP, 80, "o.webp").contains(&"libwebp".to_string()));
        // higher quality → lower CRF for AVIF
        assert!(avif_crf(100) < avif_crf(10));
    }

    #[test]
    fn avif_smaller_than_jpeg() {
        let j = estimate_bytes(4000, 3000, PhotoFormat::Jpeg, 80);
        let a = estimate_bytes(4000, 3000, PhotoFormat::Avif, 80);
        assert!(a < j);
        assert!(j > 0);
    }
}
