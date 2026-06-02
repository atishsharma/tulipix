//! Preset filter chain + sharpen/blur kernels.
//!
//! Each `Preset` is a fixed parameter recipe layered on top of the existing
//! `adjust` + `curves` ops. The `strength` slider scales the recipe linearly
//! (0 = identity, 1 = full effect). Sharpen + blur are exposed separately
//! because they have their own UI sliders.

use image::{imageops, DynamicImage, ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Preset { BW, Sepia, Vintage, Drama, Hdr, Polaroid, Faded }

// Every match arm reassigns r/g/b, so the (r0,g0,b0) seed is intentionally
// overwritten — keeps the bindings definitely-initialised for all presets.
#[allow(unused_assignments)]
pub fn apply(img: DynamicImage, preset: Preset, strength: f32) -> DynamicImage {
    let s = strength.clamp(0.0, 1.0);
    let rgba = img.to_rgba8();
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(rgba.width(), rgba.height());
    for (src, dst) in rgba.pixels().zip(out.pixels_mut()) {
        let (r0, g0, b0) = (src[0] as f32, src[1] as f32, src[2] as f32);
        let (mut r, mut g, mut b) = (r0, g0, b0);
        match preset {
            Preset::BW => {
                let l = 0.299 * r0 + 0.587 * g0 + 0.114 * b0;
                r = l; g = l; b = l;
            }
            Preset::Sepia => {
                r = 0.393*r0 + 0.769*g0 + 0.189*b0;
                g = 0.349*r0 + 0.686*g0 + 0.168*b0;
                b = 0.272*r0 + 0.534*g0 + 0.131*b0;
            }
            Preset::Vintage => {
                r = r0 * 0.9 + 30.0;
                g = g0 * 0.85 + 20.0;
                b = b0 * 0.7 + 10.0;
            }
            Preset::Drama => {
                let l = 0.299 * r0 + 0.587 * g0 + 0.114 * b0;
                r = ((r0 - l) * 1.6 + l).clamp(0.0, 255.0);
                g = ((g0 - l) * 1.6 + l).clamp(0.0, 255.0);
                b = ((b0 - l) * 1.6 + l).clamp(0.0, 255.0);
            }
            Preset::Hdr => {
                r = ((r0 - 128.0) * 1.4 + 128.0).clamp(0.0, 255.0);
                g = ((g0 - 128.0) * 1.4 + 128.0).clamp(0.0, 255.0);
                b = ((b0 - 128.0) * 1.4 + 128.0).clamp(0.0, 255.0);
            }
            Preset::Polaroid => {
                r = r0 * 1.05 + 8.0;
                g = g0 * 1.0  + 14.0;
                b = b0 * 0.9  - 8.0;
            }
            Preset::Faded => {
                let mix = 0.6;
                r = r0 * mix + 255.0 * (1.0 - mix);
                g = g0 * mix + 240.0 * (1.0 - mix);
                b = b0 * mix + 220.0 * (1.0 - mix);
            }
        }
        let mix_r = r0 * (1.0 - s) + r * s;
        let mix_g = g0 * (1.0 - s) + g * s;
        let mix_b = b0 * (1.0 - s) + b * s;
        *dst = Rgba([mix_r.clamp(0.0, 255.0) as u8, mix_g.clamp(0.0, 255.0) as u8, mix_b.clamp(0.0, 255.0) as u8, src[3]]);
    }
    DynamicImage::ImageRgba8(out)
}

/// Unsharp mask: result = src + amount * (src - blurred).
pub fn sharpen(img: DynamicImage, amount: f32, radius: f32) -> DynamicImage {
    let radius = radius.max(0.5);
    let blurred = imageops::blur(&img.to_rgba8(), radius);
    let rgba = img.to_rgba8();
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(rgba.width(), rgba.height());
    for ((src, blur), dst) in rgba.pixels().zip(blurred.pixels()).zip(out.pixels_mut()) {
        let mut p = [0u8; 4];
        for i in 0..3 {
            let v = src[i] as f32 + amount * (src[i] as f32 - blur[i] as f32);
            p[i] = v.clamp(0.0, 255.0) as u8;
        }
        p[3] = src[3];
        *dst = Rgba(p);
    }
    DynamicImage::ImageRgba8(out)
}

pub fn blur(img: DynamicImage, sigma: f32) -> DynamicImage {
    DynamicImage::ImageRgba8(imageops::blur(&img.to_rgba8(), sigma.max(0.0)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bw_strips_chroma() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([200, 50, 50, 255])));
        let out = apply(img, Preset::BW, 1.0).to_rgba8();
        let p = out.get_pixel(0, 0);
        assert_eq!(p[0], p[1]);
        assert_eq!(p[1], p[2]);
    }
    #[test]
    fn strength_zero_is_identity() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([200, 50, 50, 255])));
        let out = apply(img.clone(), Preset::Sepia, 0.0).to_rgba8();
        assert_eq!(out.get_pixel(0, 0)[0], 200);
    }
    #[test]
    fn sharpen_amplifies_contrast() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(8, 8);
        for (x, _, p) in buf.enumerate_pixels_mut() { *p = Rgba([if x < 4 { 50 } else { 200 }, 0, 0, 255]); }
        let img = DynamicImage::ImageRgba8(buf);
        let out = sharpen(img, 1.5, 1.0).to_rgba8();
        // Edge pixel should over/undershoot.
        let left = out.get_pixel(3, 4)[0] as i32;
        let right = out.get_pixel(4, 4)[0] as i32;
        assert!(right - left > 150 || left < 50 || right > 200);
    }
}
