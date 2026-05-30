//! RealESRGAN ONNX upscale (×2 / ×4).
//!
//! Production loads the RealESRGAN_x4plus.onnx blob via the model registry.
//! `NullUpscaler` falls back to high-quality Lanczos resampling so the op
//! still produces a result for users without the model — clearly labelled
//! "preview only, not AI" in the editor surface.

use anyhow::Result;
use image::{imageops::FilterType, DynamicImage, GenericImageView};

pub trait Upscaler: Send + Sync {
    fn upscale(&self, img: &DynamicImage, factor: u32) -> Result<DynamicImage>;
}

pub struct NullUpscaler;
impl Upscaler for NullUpscaler {
    fn upscale(&self, img: &DynamicImage, factor: u32) -> Result<DynamicImage> {
        // Pure-Rust fallback — Lanczos3 resample. Not the same as the AI model
        // but at least gives the user a non-trivial preview.
        let factor = factor.clamp(1, 8);
        let (w, h) = img.dimensions();
        Ok(img.resize_exact(w * factor, h * factor, FilterType::Lanczos3))
    }
}

pub fn apply(img: DynamicImage, factor: u32, upscaler: &dyn Upscaler) -> Result<DynamicImage> {
    upscaler.upscale(&img, factor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn null_upscaler_doubles_dimensions() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(16, 16, Rgba([10, 20, 30, 255])));
        let out = apply(img, 2, &NullUpscaler).unwrap();
        assert_eq!(out.dimensions(), (32, 32));
    }
    #[test]
    fn factor_one_is_identity_dimensions() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(8, 8, Rgba([0, 0, 0, 255])));
        let out = apply(img, 1, &NullUpscaler).unwrap();
        assert_eq!(out.dimensions(), (8, 8));
    }
}
