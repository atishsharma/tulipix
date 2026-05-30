//! Collage maker — fixed template layouts (2×2, 3×3, mosaic, custom grid)
//! and the freeform "picture pile" submodule.
//!
//! Each template produces a `Layout` (list of pixel-space rectangles); the
//! renderer composites every source photo into its slot via `image` crate
//! resize + paste. Output is a single JPEG/PNG.

pub mod picture_pile;

use anyhow::Result;
use image::{imageops::FilterType, DynamicImage, GenericImageView, ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Template { Grid2x2, Grid3x3, Mosaic, Strip }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Slot { pub x: u32, pub y: u32, pub w: u32, pub h: u32 }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub padding_px: u32,
    pub slots: Vec<Slot>,
}

impl Layout {
    pub fn for_template(template: Template, canvas_w: u32, canvas_h: u32, padding_px: u32) -> Self {
        let (cols, rows) = match template {
            Template::Grid2x2 => (2, 2),
            Template::Grid3x3 => (3, 3),
            Template::Mosaic  => (3, 2),
            Template::Strip   => (4, 1),
        };
        let cw = canvas_w.saturating_sub(padding_px * (cols + 1)) / cols.max(1);
        let ch = canvas_h.saturating_sub(padding_px * (rows + 1)) / rows.max(1);
        let mut slots = Vec::with_capacity((cols * rows) as usize);
        for r in 0..rows {
            for c in 0..cols {
                slots.push(Slot {
                    x: padding_px + c * (cw + padding_px),
                    y: padding_px + r * (ch + padding_px),
                    w: cw, h: ch,
                });
            }
        }
        // Mosaic has one extra-wide cell at the top.
        if template == Template::Mosaic && !slots.is_empty() {
            let merged = Slot { x: slots[0].x, y: slots[0].y, w: cw * 3 + padding_px * 2, h: ch };
            slots[0] = merged;
            slots.remove(1);
            slots.remove(1);
        }
        Self { canvas_w, canvas_h, padding_px, slots }
    }
}

pub fn render(sources: &[DynamicImage], layout: &Layout) -> DynamicImage {
    let mut canvas: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_pixel(
        layout.canvas_w, layout.canvas_h, Rgba([255, 255, 255, 255]),
    );
    for (img, slot) in sources.iter().zip(layout.slots.iter()) {
        if slot.w == 0 || slot.h == 0 { continue; }
        let (iw, ih) = img.dimensions();
        let (sw, sh) = fill_dims(iw, ih, slot.w, slot.h);
        let scaled = img.resize(sw, sh, FilterType::Triangle);
        // Centre-crop to exactly slot.w × slot.h
        let (xo, yo) = (sw.saturating_sub(slot.w) / 2, sh.saturating_sub(slot.h) / 2);
        let cropped = scaled.crop_imm(xo, yo, slot.w.min(sw), slot.h.min(sh));
        image::imageops::overlay(&mut canvas, &cropped, slot.x as i64, slot.y as i64);
    }
    DynamicImage::ImageRgba8(canvas)
}

/// Compute scale-to-fill dimensions (larger of the two ratios wins so the
/// slot is fully covered before centre-crop).
fn fill_dims(iw: u32, ih: u32, sw: u32, sh: u32) -> (u32, u32) {
    let rw = sw as f32 / iw.max(1) as f32;
    let rh = sh as f32 / ih.max(1) as f32;
    let r = rw.max(rh);
    ((iw as f32 * r).ceil() as u32, (ih as f32 * r).ceil() as u32)
}

pub fn write_jpeg(img: &DynamicImage, out_dir: &Path, stem: &str, quality: u8) -> Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join(format!("{stem}.jpg"));
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
    img.write_with_encoder(encoder)?;
    std::fs::write(&path, buf.into_inner())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid3x3_yields_9_slots() {
        let l = Layout::for_template(Template::Grid3x3, 1200, 900, 12);
        assert_eq!(l.slots.len(), 9);
    }
    #[test]
    fn mosaic_has_wide_top_slot() {
        let l = Layout::for_template(Template::Mosaic, 1200, 800, 8);
        assert!(l.slots[0].w > l.slots[1].w);
    }
    #[test]
    fn render_paints_canvas() {
        let imgs = vec![
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(50, 50, Rgba([255, 0, 0, 255]))),
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(50, 50, Rgba([0, 255, 0, 255]))),
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(50, 50, Rgba([0, 0, 255, 255]))),
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(50, 50, Rgba([255, 255, 0, 255]))),
        ];
        let l = Layout::for_template(Template::Grid2x2, 200, 200, 4);
        let out = render(&imgs, &l).to_rgba8();
        assert_eq!(out.dimensions(), (200, 200));
        // Padding pixel should still be white.
        assert_eq!(out.get_pixel(0, 0), &Rgba([255, 255, 255, 255]));
    }
}
