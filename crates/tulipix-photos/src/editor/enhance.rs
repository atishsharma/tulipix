//! Auto Enhance one-click — gentle, idempotent-by-design.
//!
//! The previous build ran a per-tile CLAHE that re-scaled RGB by a per-tile
//! luma ratio; with no inter-tile interpolation it produced blocky, blown-out
//! results that got worse every re-apply. This replaces it with the classic,
//! well-behaved auto-enhance recipe:
//!   1. Clamped gray-world white balance — neutralise a colour cast, but cap
//!      the per-channel gain to ±25% so a strongly-tinted scene can't blow up.
//!   2. Percentile auto-levels — stretch luma so the 0.5th/99.5th percentiles
//!      map to black/white (a single global affine on all three channels, so
//!      neutrals stay neutral). Skipped when the image is already full-range.
//! Both stages are bounded and converge: applying once gives a natural lift and
//! applying again is essentially a no-op. (A saturation boost was deliberately
//! dropped — it compounds on every re-apply, which is exactly the "overdoing
//! it" the auto-enhance is meant to avoid.)

use image::{DynamicImage, ImageBuffer, Rgba};

const GAIN_MIN: f32 = 0.75;
const GAIN_MAX: f32 = 1.25;
const CLIP_FRACTION: f32 = 0.005; // ignore the darkest/brightest 0.5%

pub fn apply(img: DynamicImage) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    auto_white_balance(&mut rgba);
    auto_levels(&mut rgba);
    DynamicImage::ImageRgba8(rgba)
}

#[inline]
fn luma(p: &Rgba<u8>) -> f32 {
    0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32
}

fn auto_white_balance(buf: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let n = buf.pixels().len() as f64;
    if n == 0.0 { return; }
    let (mut sr, mut sg, mut sb) = (0f64, 0f64, 0f64);
    for p in buf.pixels() { sr += p[0] as f64; sg += p[1] as f64; sb += p[2] as f64; }
    let mr = (sr / n).max(1.0); let mg = (sg / n).max(1.0); let mb = (sb / n).max(1.0);
    let mean = (mr + mg + mb) / 3.0;
    let clamp = |k: f64| (k as f32).clamp(GAIN_MIN, GAIN_MAX);
    let kr = clamp(mean / mr); let kg = clamp(mean / mg); let kb = clamp(mean / mb);
    for p in buf.pixels_mut() {
        p[0] = (p[0] as f32 * kr).clamp(0.0, 255.0) as u8;
        p[1] = (p[1] as f32 * kg).clamp(0.0, 255.0) as u8;
        p[2] = (p[2] as f32 * kb).clamp(0.0, 255.0) as u8;
    }
}

/// Global contrast stretch: map [lo, hi] luma percentiles → [0, 255] using one
/// affine applied to every channel. No-op when the image already spans the
/// range (so re-applying does nothing).
fn auto_levels(buf: &mut ImageBuffer<Rgba<u8>, Vec<u8>>) {
    let total = buf.pixels().len();
    if total == 0 { return; }
    let mut hist = [0u32; 256];
    for p in buf.pixels() { hist[(luma(p) as usize).min(255)] += 1; }
    let cut = ((total as f32) * CLIP_FRACTION) as u32;

    let mut acc = 0u32;
    let mut lo = 0usize;
    for (i, &c) in hist.iter().enumerate() { acc += c; if acc > cut { lo = i; break; } }
    acc = 0;
    let mut hi = 255usize;
    for i in (0..256).rev() { acc += hist[i]; if acc > cut { hi = i; break; } }

    // Already (near) full-range — leave it alone.
    if hi <= lo + 8 { return; }
    let lo = lo as f32;
    let span = (hi as f32 - lo).max(1.0);
    let scale = 255.0 / span;
    let map = |v: u8| (((v as f32 - lo) * scale).clamp(0.0, 255.0)) as u8;
    for p in buf.pixels_mut() {
        p[0] = map(p[0]); p[1] = map(p[1]); p[2] = map(p[2]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn enhance_runs_on_uniform_image() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(64, 64, Rgba([120, 100, 80, 255])));
        let out = apply(img).to_rgba8();
        assert_eq!(out.dimensions(), (64, 64));
    }
    #[test]
    fn enhance_does_not_panic_on_tiny_image() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([0, 0, 0, 255])));
        let _ = apply(img);
    }
    #[test]
    fn enhance_is_roughly_idempotent() {
        // A normal photo-ish gradient: enhancing twice ≈ enhancing once.
        let mut buf = ImageBuffer::<Rgba<u8>, Vec<u8>>::new(32, 32);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = Rgba([(x * 6) as u8, (y * 6) as u8, 90, 255]);
        }
        let once = apply(DynamicImage::ImageRgba8(buf.clone())).to_rgba8();
        let twice = apply(DynamicImage::ImageRgba8(once.clone())).to_rgba8();
        let mut max_d = 0i32;
        for (a, b) in once.pixels().zip(twice.pixels()) {
            for c in 0..3 { max_d = max_d.max((a[c] as i32 - b[c] as i32).abs()); }
        }
        assert!(max_d < 20, "second enhance shifted pixels by {max_d}");
    }
}
