//! RGB + per-channel tone curves, with a small histogram helper for the
//! curves widget.
//!
//! Curve is a piecewise-linear function from `points` (a sorted list of
//! `(x, y)` in 0..1). Channels: All / R / G / B. Histogram bucketises into
//! 256 bins per channel.

use image::{DynamicImage, ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Channel { All, R, G, B }

pub fn lut_from_points(points: &[(f32, f32)]) -> [u8; 256] {
    let mut sorted: Vec<(f32, f32)> = points.to_vec();
    sorted.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if sorted.is_empty() {
        return identity_lut();
    }
    if sorted[0].0 > 0.0 { sorted.insert(0, (0.0, sorted[0].1)); }
    if sorted.last().unwrap().0 < 1.0 {
        let last_y = sorted.last().unwrap().1;
        sorted.push((1.0, last_y));
    }
    let mut lut = [0u8; 256];
    for (i, slot) in lut.iter_mut().enumerate() {
        let x = i as f32 / 255.0;
        let (mut lo, mut hi) = (&sorted[0], &sorted[sorted.len() - 1]);
        for w in sorted.windows(2) {
            if x >= w[0].0 && x <= w[1].0 { lo = &w[0]; hi = &w[1]; break; }
        }
        let span = (hi.0 - lo.0).max(f32::EPSILON);
        let t = (x - lo.0) / span;
        let y = lo.1 + t * (hi.1 - lo.1);
        *slot = (y.clamp(0.0, 1.0) * 255.0) as u8;
    }
    lut
}

pub fn identity_lut() -> [u8; 256] {
    let mut lut = [0u8; 256];
    for i in 0..256 { lut[i] = i as u8; }
    lut
}

pub fn apply(img: DynamicImage, channel: Channel, points: &[(f32, f32)]) -> DynamicImage {
    let lut = lut_from_points(points);
    let rgba = img.to_rgba8();
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(rgba.width(), rgba.height());
    for (src, dst) in rgba.pixels().zip(out.pixels_mut()) {
        let mut r = src[0]; let mut g = src[1]; let mut b = src[2];
        match channel {
            Channel::All => { r = lut[r as usize]; g = lut[g as usize]; b = lut[b as usize]; }
            Channel::R   => { r = lut[r as usize]; }
            Channel::G   => { g = lut[g as usize]; }
            Channel::B   => { b = lut[b as usize]; }
        }
        *dst = Rgba([r, g, b, src[3]]);
    }
    DynamicImage::ImageRgba8(out)
}

#[derive(Debug, Clone)]
pub struct Histogram { pub r: [u32; 256], pub g: [u32; 256], pub b: [u32; 256], pub luma: [u32; 256] }

impl Default for Histogram { fn default() -> Self { Self { r: [0; 256], g: [0; 256], b: [0; 256], luma: [0; 256] } } }

pub fn histogram(img: &DynamicImage) -> Histogram {
    let rgba = img.to_rgba8();
    let mut h = Histogram::default();
    for p in rgba.pixels() {
        h.r[p[0] as usize] += 1;
        h.g[p[1] as usize] += 1;
        h.b[p[2] as usize] += 1;
        let l = (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as usize;
        h.luma[l.min(255)] += 1;
    }
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_curve_is_identity_lut() {
        let lut = lut_from_points(&[(0.0, 0.0), (1.0, 1.0)]);
        for i in 0..256 {
            assert!((lut[i] as i32 - i as i32).abs() <= 1);
        }
    }
    #[test]
    fn invert_curve_inverts() {
        let lut = lut_from_points(&[(0.0, 1.0), (1.0, 0.0)]);
        assert_eq!(lut[0], 255);
        assert_eq!(lut[255], 0);
    }
    #[test]
    fn histogram_counts_pixels() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([10, 20, 30, 255])));
        let h = histogram(&img);
        assert_eq!(h.r[10], 16);
        assert_eq!(h.g[20], 16);
        assert_eq!(h.b[30], 16);
    }
}
