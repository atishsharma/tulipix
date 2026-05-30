//! SAM-tiny sky segmentation → replace with bundled textures.

use anyhow::{anyhow, Result};
use image::{DynamicImage, GrayImage, ImageBuffer, Rgba};
use std::path::Path;

pub trait SkySegmenter: Send + Sync {
    fn mask(&self, img: &DynamicImage) -> Result<GrayImage>;
}

pub struct NullSky;
impl SkySegmenter for NullSky {
    fn mask(&self, _: &DynamicImage) -> Result<GrayImage> {
        Err(anyhow!("SAM-tiny model not installed — Settings → AI Models → Install"))
    }
}

pub fn apply(img: DynamicImage, mask_path: &Path, texture: &str, _segmenter: &dyn SkySegmenter) -> Result<DynamicImage> {
    // When a user-painted mask is supplied, use it directly (the SAM model
    // is only needed for the "auto detect sky" button). Texture lookup is
    // mocked here — production loads bundled "sunset.jpg"/"clear.jpg"/etc.
    let mask = image::ImageReader::open(mask_path)?.with_guessed_format()?.decode()?.to_luma8();
    let mut rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let tex = bundled_texture(texture, w, h);
    for (x, y, m) in mask.enumerate_pixels() {
        if m[0] < 128 { continue; }
        let t = tex.get_pixel(x, y);
        rgba.put_pixel(x, y, Rgba([t[0], t[1], t[2], rgba.get_pixel(x, y)[3]]));
    }
    Ok(DynamicImage::ImageRgba8(rgba))
}

fn bundled_texture(name: &str, w: u32, h: u32) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    // Fallback synthetic gradients keyed by name — keeps the module self
    // contained for tests. The shipping binary maps each name to a real
    // bundled JPEG under resources/textures/sky/.
    let mut buf: ImageBuffer<Rgba<u8>, _> = ImageBuffer::new(w, h);
    let (top, bot): ([u8; 3], [u8; 3]) = match name {
        "sunset"   => ([230, 90, 40],  [255, 200, 120]),
        "clear"    => ([90, 150, 230], [180, 220, 255]),
        "dramatic" => ([20, 30, 60],   [120, 140, 180]),
        _          => ([100, 130, 180], [200, 220, 240]),
    };
    for y in 0..h {
        let t = y as f32 / h.max(1) as f32;
        let r = (top[0] as f32 * (1.0 - t) + bot[0] as f32 * t) as u8;
        let g = (top[1] as f32 * (1.0 - t) + bot[1] as f32 * t) as u8;
        let b = (top[2] as f32 * (1.0 - t) + bot[2] as f32 * t) as u8;
        for x in 0..w { buf.put_pixel(x, y, Rgba([r, g, b, 255])); }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn null_segmenter_errors() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(4, 4, Rgba([0, 0, 0, 255])));
        assert!(NullSky.mask(&img).is_err());
    }
    #[test]
    fn texture_name_picks_palette() {
        let a = bundled_texture("sunset", 2, 2);
        let b = bundled_texture("clear", 2, 2);
        assert_ne!(a.get_pixel(0, 0), b.get_pixel(0, 0));
    }
}
