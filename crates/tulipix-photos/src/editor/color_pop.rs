//! Color pop / portrait depth — keep the subject in colour, desaturate the
//! rest. The subject mask comes from SAM-tiny (`Segmenter` trait). When the
//! model is unavailable a brightness-based fallback approximates the mask
//! by treating the top-N% brightest pixels as foreground.

use anyhow::{anyhow, Result};
use image::{DynamicImage, GrayImage, ImageBuffer, Rgba};

pub trait Segmenter: Send + Sync {
    fn subject_mask(&self, img: &DynamicImage) -> Result<GrayImage>;
}

pub struct NullSegmenter;
impl Segmenter for NullSegmenter {
    fn subject_mask(&self, _: &DynamicImage) -> Result<GrayImage> {
        Err(anyhow!("SAM-tiny model not installed — Settings → AI Models → Install"))
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ColorPopParams {
    pub background_saturation: f32, // 0..1; 0 = full B&W
    pub feather_px: u32,
}
impl Default for ColorPopParams { fn default() -> Self { Self { background_saturation: 0.0, feather_px: 4 } } }

pub fn apply(img: DynamicImage, mask: &GrayImage, params: ColorPopParams) -> DynamicImage {
    let feathered = feather(mask, params.feather_px);
    let rgba = img.to_rgba8();
    let mut out: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(rgba.width(), rgba.height());
    for (x, y, src) in rgba.enumerate_pixels() {
        let alpha = if feathered.width() == rgba.width() && feathered.height() == rgba.height() {
            feathered.get_pixel(x, y)[0] as f32 / 255.0
        } else {
            0.0
        };
        let (r0, g0, b0) = (src[0] as f32, src[1] as f32, src[2] as f32);
        let l = 0.299 * r0 + 0.587 * g0 + 0.114 * b0;
        let bg_sat = params.background_saturation.clamp(0.0, 1.0);
        let r_bg = l + (r0 - l) * bg_sat;
        let g_bg = l + (g0 - l) * bg_sat;
        let b_bg = l + (b0 - l) * bg_sat;
        let r = r0 * alpha + r_bg * (1.0 - alpha);
        let g = g0 * alpha + g_bg * (1.0 - alpha);
        let b = b0 * alpha + b_bg * (1.0 - alpha);
        out.put_pixel(x, y, Rgba([r as u8, g as u8, b as u8, src[3]]));
    }
    DynamicImage::ImageRgba8(out)
}

fn feather(mask: &GrayImage, radius: u32) -> GrayImage {
    if radius == 0 { return mask.clone(); }
    image::imageops::blur(mask, radius as f32)
}

/// Brightness-based fallback mask — keep pixels whose luma is in the top
/// `keep_fraction`. Cheap, only meaningful for high-key portraits.
pub fn brightness_fallback_mask(img: &DynamicImage, keep_fraction: f32) -> GrayImage {
    let rgba = img.to_rgba8();
    let mut lums: Vec<u8> = rgba
        .pixels()
        .map(|p| (0.299 * p[0] as f32 + 0.587 * p[1] as f32 + 0.114 * p[2] as f32) as u8)
        .collect();
    let mut sorted = lums.clone();
    sorted.sort_unstable();
    let k = ((1.0 - keep_fraction.clamp(0.0, 1.0)) * sorted.len() as f32) as usize;
    let threshold = sorted.get(k.min(sorted.len().saturating_sub(1))).copied().unwrap_or(0);
    for v in &mut lums {
        *v = if *v >= threshold { 255 } else { 0 };
    }
    GrayImage::from_raw(rgba.width(), rgba.height(), lums).unwrap_or_else(|| GrayImage::new(rgba.width(), rgba.height()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Luma;

    #[test]
    fn null_segmenter_errors() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([0, 0, 0, 255])));
        assert!(NullSegmenter.subject_mask(&img).is_err());
    }
    #[test]
    fn background_desaturates_when_mask_excludes() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([200, 50, 50, 255])));
        let mask = GrayImage::from_pixel(4, 4, Luma([0])); // all background
        let out = apply(img, &mask, ColorPopParams::default()).to_rgba8();
        let p = out.get_pixel(0, 0);
        assert!(p[0] == p[1] && p[1] == p[2], "background should be desaturated grey");
    }
    #[test]
    fn fallback_mask_separates_bright_dark() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(4, 4);
        for (x, _, p) in buf.enumerate_pixels_mut() {
            *p = if x < 2 { Rgba([10, 10, 10, 255]) } else { Rgba([240, 240, 240, 255]) };
        }
        let m = brightness_fallback_mask(&DynamicImage::ImageRgba8(buf), 0.5);
        assert_eq!(m.get_pixel(0, 0)[0], 0);
        assert_eq!(m.get_pixel(3, 0)[0], 255);
    }
}
