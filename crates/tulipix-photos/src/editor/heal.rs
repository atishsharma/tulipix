//! LaMa inpainting (~50 MB ONNX) brush-driven magic eraser.
//!
//! The actual ONNX session lives behind the `Healer` trait so the rest of
//! the editor doesn't depend on ORT being compiled in. When the model is
//! missing the `NullHealer` returns Err so `EditOp::Heal` short-circuits
//! and the UI flips the row into "needs model" state.

use anyhow::{anyhow, Result};
use image::{DynamicImage, GrayImage, ImageBuffer, Rgba};
use std::path::Path;

pub trait Healer: Send + Sync {
    fn inpaint(&self, img: &DynamicImage, mask: &GrayImage) -> Result<DynamicImage>;
}

pub struct NullHealer;
impl Healer for NullHealer {
    fn inpaint(&self, _: &DynamicImage, _: &GrayImage) -> Result<DynamicImage> {
        Err(anyhow!("LaMa model not installed — Settings → AI Models → Install"))
    }
}

pub fn load_mask(path: &Path) -> Result<GrayImage> {
    let img = image::ImageReader::open(path)?.with_guessed_format()?.decode()?;
    Ok(img.to_luma8())
}

pub fn apply(img: DynamicImage, mask_path: &Path, healer: &dyn Healer) -> Result<DynamicImage> {
    let mask = load_mask(mask_path)?;
    healer.inpaint(&img, &mask)
}

/// Fallback "median-fill" when no model is available — paints masked pixels
/// with the surrounding region's median colour. Not photorealistic, but
/// gives the editor *some* output for tiny smudges so the op can still be
/// previewed in the no-model state. Opt-in via Settings.
pub fn median_fill(img: DynamicImage, mask: &GrayImage) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let mut samples = Vec::new();
    for (x, y, m) in mask.enumerate_pixels() {
        if m[0] < 128 { continue; }
        samples.clear();
        for dy in -2i32..=2 {
            for dx in -2i32..=2 {
                let nx = x as i32 + dx;
                let ny = y as i32 + dy;
                if nx < 0 || ny < 0 || nx >= w as i32 || ny >= h as i32 { continue; }
                let mp = mask.get_pixel(nx as u32, ny as u32);
                if mp[0] >= 128 { continue; } // skip masked-out neighbours
                let p = rgba.get_pixel(nx as u32, ny as u32);
                samples.push(*p);
            }
        }
        if samples.is_empty() { continue; }
        let mut chans = [Vec::<u8>::new(), Vec::<u8>::new(), Vec::<u8>::new()];
        for px in &samples {
            for c in 0..3 { chans[c].push(px[c]); }
        }
        let mut filled = [0u8; 4];
        for c in 0..3 {
            chans[c].sort_unstable();
            filled[c] = chans[c][chans[c].len() / 2];
        }
        filled[3] = rgba.get_pixel(x, y)[3];
        rgba.put_pixel(x, y, Rgba(filled));
    }
    DynamicImage::ImageRgba8(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    #[test]
    fn null_healer_errors() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([0, 0, 0, 255])));
        let mask = GrayImage::from_pixel(4, 4, image::Luma([255]));
        assert!(NullHealer.inpaint(&img, &mask).is_err());
    }
    #[test]
    fn median_fill_replaces_masked_pixels() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(5, 5);
        for (_, _, p) in buf.enumerate_pixels_mut() { *p = Rgba([100, 150, 200, 255]); }
        // Black sentinel at (2,2)
        buf.put_pixel(2, 2, Rgba([0, 0, 0, 255]));
        let mut mask = GrayImage::from_pixel(5, 5, image::Luma([0]));
        mask.put_pixel(2, 2, image::Luma([255]));
        let out = median_fill(DynamicImage::ImageRgba8(buf), &mask).to_rgba8();
        let p = out.get_pixel(2, 2);
        assert_eq!(p[0], 100);
    }
}
