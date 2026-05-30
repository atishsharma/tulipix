//! Adjust panel — exposure, contrast, saturation, temperature, tint,
//! highlights/shadows, blacks/whites. Reference CPU impl; the Slint surface
//! mirrors the same parameter set in a wgpu fragment shader.

use image::{DynamicImage, ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq)]
pub struct AdjustParams {
    pub exposure: f32,     // stops, -2..2
    pub contrast: f32,     // -1..1
    pub saturation: f32,   // -1..1
    pub temperature: f32,  // -1..1 (cool/warm)
    pub tint: f32,         // -1..1 (green/magenta)
    pub highlights: f32,   // -1..1
    pub shadows: f32,      // -1..1
    pub blacks: f32,       // -1..1
    pub whites: f32,       // -1..1
}

pub fn apply(img: DynamicImage, p: AdjustParams) -> DynamicImage {
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(w, h);
    let exp_gain = 2f32.powf(p.exposure);
    for (src, dst) in rgba.pixels().zip(out.pixels_mut()) {
        let mut r = src[0] as f32 / 255.0;
        let mut g = src[1] as f32 / 255.0;
        let mut b = src[2] as f32 / 255.0;
        // exposure
        r *= exp_gain; g *= exp_gain; b *= exp_gain;
        // temperature: shifts R↑ / B↓ on warm
        r += p.temperature * 0.15;
        b -= p.temperature * 0.15;
        // tint: green ↔ magenta
        g -= p.tint * 0.10;
        r += p.tint * 0.05;
        b += p.tint * 0.05;
        // contrast around 0.5
        r = ((r - 0.5) * (1.0 + p.contrast)) + 0.5;
        g = ((g - 0.5) * (1.0 + p.contrast)) + 0.5;
        b = ((b - 0.5) * (1.0 + p.contrast)) + 0.5;
        // saturation
        let l = 0.299 * r + 0.587 * g + 0.114 * b;
        r = l + (r - l) * (1.0 + p.saturation);
        g = l + (g - l) * (1.0 + p.saturation);
        b = l + (b - l) * (1.0 + p.saturation);
        // tonal regions
        let region = if l > 0.66 { p.highlights } else if l < 0.33 { p.shadows } else { 0.0 };
        r += region * 0.10;
        g += region * 0.10;
        b += region * 0.10;
        // blacks/whites stretch
        let lo = -p.blacks * 0.05;
        let hi = 1.0 + p.whites * 0.05;
        let stretch = |v: f32| ((v - lo) / (hi - lo)).clamp(0.0, 1.0);
        r = stretch(r);
        g = stretch(g);
        b = stretch(b);
        dst[0] = (r.clamp(0.0, 1.0) * 255.0) as u8;
        dst[1] = (g.clamp(0.0, 1.0) * 255.0) as u8;
        dst[2] = (b.clamp(0.0, 1.0) * 255.0) as u8;
        dst[3] = src[3];
    }
    DynamicImage::ImageRgba8(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn identity_params_are_noop_within_rounding() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(8, 8);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = Rgba([((x * 32) & 0xff) as u8, ((y * 32) & 0xff) as u8, 64, 255]);
        }
        let img = DynamicImage::ImageRgba8(buf.clone());
        let out = apply(img, AdjustParams::default());
        let rgba = out.to_rgba8();
        for (a, b) in buf.pixels().zip(rgba.pixels()) {
            for c in 0..3 {
                assert!((a[c] as i32 - b[c] as i32).abs() <= 2, "drift {}", a[c] as i32 - b[c] as i32);
            }
        }
    }
    #[test]
    fn exposure_brightens() {
        let buf = ImageBuffer::from_pixel(4, 4, Rgba([100, 100, 100, 255]));
        let mut p = AdjustParams::default();
        p.exposure = 1.0; // +1 stop = ×2
        let out = apply(DynamicImage::ImageRgba8(buf), p).to_rgba8();
        assert!(out.get_pixel(0, 0)[0] > 150);
    }
}
