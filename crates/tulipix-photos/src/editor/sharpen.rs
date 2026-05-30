//! Sharpen (unsharp mask) + gaussian + motion blur kernels.
//!
//! The sharpen primitive lives in `filters::sharpen`; this module wraps it
//! for direct UI use and adds a CPU-side motion blur (line kernel) so the
//! Slint wgpu shader has a CPU reference for tests + headless export.

use image::{imageops, DynamicImage, ImageBuffer, Rgba};

pub use super::filters::sharpen as unsharp_mask;
pub use super::filters::blur as gaussian_blur;

/// Motion blur — convolves an N-tap line oriented at `angle_deg` (0° =
/// horizontal). `length_px` is the line length in pixels (will be clamped
/// to odd so the kernel has a centre). Simple separable accumulator —
/// roughly O(W*H*length).
pub fn motion_blur(img: DynamicImage, length_px: u32, angle_deg: f32) -> DynamicImage {
    let len = (length_px.max(3) | 1) as i32; // force odd
    let half = len / 2;
    let rad = angle_deg.to_radians();
    let (dx, dy) = (rad.cos(), rad.sin());
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(rgba.width(), rgba.height());
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 4];
            let mut weight = 0f32;
            for t in -half..=half {
                let sx = x as f32 + t as f32 * dx;
                let sy = y as f32 + t as f32 * dy;
                let ix = sx.round() as i32;
                let iy = sy.round() as i32;
                if ix < 0 || iy < 0 || ix >= w || iy >= h { continue; }
                let p = rgba.get_pixel(ix as u32, iy as u32);
                for c in 0..4 { acc[c] += p[c] as f32; }
                weight += 1.0;
            }
            if weight == 0.0 { weight = 1.0; }
            let mut px = [0u8; 4];
            for c in 0..4 { px[c] = (acc[c] / weight).clamp(0.0, 255.0) as u8; }
            out.put_pixel(x as u32, y as u32, Rgba(px));
        }
    }
    DynamicImage::ImageRgba8(out)
}

/// Box blur — fast separable, used for thumbnail previews.
pub fn box_blur(img: DynamicImage, radius: u32) -> DynamicImage {
    if radius == 0 { return img; }
    DynamicImage::ImageRgba8(imageops::blur(&img.to_rgba8(), radius as f32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn motion_blur_smears_vertical_edge() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(20, 20);
        for (x, _, p) in buf.enumerate_pixels_mut() {
            *p = if x < 10 { Rgba([0, 0, 0, 255]) } else { Rgba([255, 255, 255, 255]) };
        }
        let img = DynamicImage::ImageRgba8(buf);
        let out = motion_blur(img, 9, 0.0).to_rgba8(); // horizontal blur
        let edge = out.get_pixel(10, 10)[0];
        assert!(edge > 50 && edge < 240, "edge should be partially blended: {edge}");
    }

    #[test]
    fn zero_radius_box_blur_is_identity() {
        let buf = ImageBuffer::from_pixel(4, 4, Rgba([10, 20, 30, 255]));
        let img = DynamicImage::ImageRgba8(buf.clone());
        let out = box_blur(img, 0).to_rgba8();
        for (a, b) in buf.pixels().zip(out.pixels()) { assert_eq!(a, b); }
    }
}
