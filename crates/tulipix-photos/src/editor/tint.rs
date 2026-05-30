//! Colorify / duo-tone tint — picks a target color and tints the image
//! while preserving luminance (Picasa "Tint" parity).

use image::{DynamicImage, ImageBuffer, Rgba};

pub fn apply(img: DynamicImage, rgb: [u8; 3], strength: f32) -> DynamicImage {
    let s = strength.clamp(0.0, 1.0);
    let (tr, tg, tb) = (rgb[0] as f32, rgb[1] as f32, rgb[2] as f32);
    let rgba = img.to_rgba8();
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(rgba.width(), rgba.height());
    for (src, dst) in rgba.pixels().zip(out.pixels_mut()) {
        let l_orig = 0.299 * src[0] as f32 + 0.587 * src[1] as f32 + 0.114 * src[2] as f32;
        // Blend target colour into original at strength `s`.
        let mut r = src[0] as f32 * (1.0 - s) + tr * s;
        let mut g = src[1] as f32 * (1.0 - s) + tg * s;
        let mut b = src[2] as f32 * (1.0 - s) + tb * s;
        // Rescale to preserve luminance of the original pixel.
        let l_new = (0.299 * r + 0.587 * g + 0.114 * b).max(1.0);
        let k = l_orig / l_new;
        r *= k; g *= k; b *= k;
        *dst = Rgba([r.clamp(0.0, 255.0) as u8, g.clamp(0.0, 255.0) as u8, b.clamp(0.0, 255.0) as u8, src[3]]);
    }
    DynamicImage::ImageRgba8(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_luminance_within_tolerance() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([180, 90, 60, 255])));
        let out = apply(img, [80, 50, 200], 1.0).to_rgba8();
        let p = out.get_pixel(0, 0);
        let l_orig = 0.299*180.0 + 0.587*90.0 + 0.114*60.0;
        let l_out  = 0.299*p[0] as f32 + 0.587*p[1] as f32 + 0.114*p[2] as f32;
        assert!((l_orig - l_out).abs() < 30.0, "luma drifted too far: {} -> {}", l_orig, l_out);
    }
    #[test]
    fn strength_zero_is_identity() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([180, 90, 60, 255])));
        let out = apply(img, [10, 200, 30], 0.0).to_rgba8();
        let p = out.get_pixel(0, 0);
        assert_eq!(p[0], 180); assert_eq!(p[1], 90); assert_eq!(p[2], 60);
    }
}
