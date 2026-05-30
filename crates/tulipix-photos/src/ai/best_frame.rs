//! Best-Frame extraction.
//!
//! Given a burst (set of candidate frames), score each by:
//!   • sharpness  — variance of Laplacian on the luma channel
//!   • exposure   — penalty for clipped pixels above 250 / below 5
//!   • face score — sum of (open-eye + smile) signals from the detector
//!                  (defaults to 0 when the detector returns nothing —
//!                  sharpness alone wins for non-portrait bursts)
//!
//! Weights are tunable via `BestFrameWeights`. Returns the winning index +
//! per-candidate breakdown so the UI can show "why this frame".

use anyhow::Result;
use image::{DynamicImage, GenericImageView, GrayImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct BestFrameWeights {
    pub sharpness: f32,
    pub exposure: f32,
    pub face: f32,
}

impl Default for BestFrameWeights {
    fn default() -> Self { Self { sharpness: 0.6, exposure: 0.2, face: 0.2 } }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameScore {
    pub index: usize,
    pub sharpness: f32,
    pub exposure: f32,
    pub face: f32,
    pub total: f32,
}

/// Per-frame face inputs from SCRFD + landmark/smile model. `None` means no
/// face data available — face component drops out of the final score.
#[derive(Debug, Clone, Copy, Default)]
pub struct FaceSignal {
    pub open_eyes: f32,   // 0..1, 1 = both eyes open
    pub smile: f32,       // 0..1, 1 = smiling
}

pub fn variance_of_laplacian(luma: &GrayImage) -> f32 {
    let (w, h) = (luma.width() as i32, luma.height() as i32);
    if w < 3 || h < 3 { return 0.0; }
    let mut sum: f64 = 0.0;
    let mut sumsq: f64 = 0.0;
    let mut count: u64 = 0;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let c = luma.get_pixel(x as u32, y as u32)[0] as i32;
            let n = luma.get_pixel(x as u32, (y - 1) as u32)[0] as i32;
            let s = luma.get_pixel(x as u32, (y + 1) as u32)[0] as i32;
            let e = luma.get_pixel((x + 1) as u32, y as u32)[0] as i32;
            let w_ = luma.get_pixel((x - 1) as u32, y as u32)[0] as i32;
            let lap = (4 * c - n - s - e - w_) as f64;
            sum += lap;
            sumsq += lap * lap;
            count += 1;
        }
    }
    if count == 0 { return 0.0; }
    let mean = sum / count as f64;
    let var = (sumsq / count as f64 - mean * mean).max(0.0);
    var as f32
}

pub fn exposure_score(luma: &GrayImage) -> f32 {
    let total = (luma.width() * luma.height()) as f32;
    if total == 0.0 { return 0.0; }
    let mut bad = 0u32;
    for p in luma.pixels() {
        let v = p[0];
        if v <= 5 || v >= 250 { bad += 1; }
    }
    1.0 - (bad as f32 / total).min(1.0)
}

pub fn face_score(signal: FaceSignal) -> f32 {
    (signal.open_eyes + signal.smile) / 2.0
}

pub fn score_frame(
    img: &DynamicImage,
    face: Option<FaceSignal>,
    weights: &BestFrameWeights,
    index: usize,
) -> FrameScore {
    let luma = img.to_luma8();
    let s = variance_of_laplacian(&luma);
    let e = exposure_score(&luma);
    let f = face.map(face_score).unwrap_or(0.0);
    // Normalise sharpness with log scale — Laplacian variance grows fast.
    let s_norm = (s.ln().max(0.0) / 12.0).min(1.0);
    let total = weights.sharpness * s_norm + weights.exposure * e + weights.face * f;
    FrameScore { index, sharpness: s_norm, exposure: e, face: f, total }
}

pub fn pick_best(
    frames: &[DynamicImage],
    faces: &[Option<FaceSignal>],
    weights: &BestFrameWeights,
) -> Result<(usize, Vec<FrameScore>)> {
    if frames.is_empty() { anyhow::bail!("empty burst"); }
    let mut scores = Vec::with_capacity(frames.len());
    for (i, img) in frames.iter().enumerate() {
        let face = faces.get(i).copied().flatten();
        scores.push(score_frame(img, face, weights, i));
    }
    let best = scores
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total.partial_cmp(&b.1.total).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(i, _)| i)
        .unwrap();
    Ok((best, scores))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn flat(color: [u8; 3]) -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_pixel(64, 64, Rgb(color)))
    }

    fn checker() -> DynamicImage {
        let mut img = RgbImage::new(64, 64);
        for (x, y, p) in img.enumerate_pixels_mut() {
            let v = if ((x ^ y) & 1) == 0 { 0u8 } else { 255u8 };
            *p = Rgb([v, v, v]);
        }
        DynamicImage::ImageRgb8(img)
    }

    #[test]
    fn sharp_image_outscores_flat() {
        let flat = flat([128, 128, 128]);
        let detailed = checker();
        let w = BestFrameWeights::default();
        let (best, scores) = pick_best(&[flat, detailed], &[None, None], &w).unwrap();
        assert_eq!(best, 1);
        assert!(scores[1].sharpness > scores[0].sharpness);
    }

    #[test]
    fn face_signal_breaks_sharpness_tie() {
        let a = checker();
        let b = checker();
        let w = BestFrameWeights::default();
        let (best, _) = pick_best(&[a, b], &[Some(FaceSignal { open_eyes: 0.0, smile: 0.0 }), Some(FaceSignal { open_eyes: 1.0, smile: 1.0 })], &w).unwrap();
        assert_eq!(best, 1);
    }
}
