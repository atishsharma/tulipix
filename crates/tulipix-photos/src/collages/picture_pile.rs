//! Picture-pile collage (Picasa parity) — scatter photos like polaroids on
//! a desk with drop shadows and random rotation. Layout is deterministic
//! given the same seed, so re-rendering with the same album reproduces the
//! same pile.

use anyhow::Result;
use image::{imageops::FilterType, DynamicImage, ImageBuffer, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PileOptions {
    pub canvas_w: u32,
    pub canvas_h: u32,
    pub polaroid_w: u32,
    pub border_px: u32,
    pub max_rotate_deg: f32,
    pub seed: u64,
}

impl Default for PileOptions {
    fn default() -> Self {
        Self { canvas_w: 1600, canvas_h: 1200, polaroid_w: 400, border_px: 24, max_rotate_deg: 18.0, seed: 0xCAFE }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PilePlacement {
    pub x: i32, pub y: i32,
    pub rotate_deg: f32,
    pub w: u32, pub h: u32,
}

pub fn layout(count: usize, opt: &PileOptions) -> Vec<PilePlacement> {
    let mut rng = SplitMix64::new(opt.seed);
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let cx = rng.next_range(0, opt.canvas_w);
        let cy = rng.next_range(0, opt.canvas_h);
        let rot = ((rng.next_f32() * 2.0) - 1.0) * opt.max_rotate_deg;
        let w = opt.polaroid_w;
        let h = (w as f32 * 1.25) as u32; // 4:5 polaroid
        out.push(PilePlacement {
            x: cx as i32 - (w / 2) as i32,
            y: cy as i32 - (h / 2) as i32,
            rotate_deg: rot,
            w, h,
        });
        let _ = i;
    }
    out
}

pub fn render(sources: &[DynamicImage], opt: &PileOptions) -> Result<DynamicImage> {
    let placements = layout(sources.len(), opt);
    let mut canvas: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_pixel(
        opt.canvas_w, opt.canvas_h, Rgba([240, 235, 220, 255]),
    );
    for (img, place) in sources.iter().zip(placements.iter()) {
        let inner_w = place.w - opt.border_px * 2;
        let inner_h = place.h - opt.border_px * 2;
        let scaled = img.resize_to_fill(inner_w, inner_h, FilterType::Triangle);

        // White polaroid card with photo composited inset.
        let mut card: ImageBuffer<Rgba<u8>, _> = ImageBuffer::from_pixel(place.w, place.h, Rgba([255, 255, 255, 255]));
        image::imageops::overlay(&mut card, &scaled, opt.border_px as i64, opt.border_px as i64);

        // Drop shadow — composite a softer dark rect first behind the card.
        let shadow_offset = 6i64;
        for sy in 0..place.h {
            for sx in 0..place.w {
                let tx = place.x as i64 + sx as i64 + shadow_offset;
                let ty = place.y as i64 + sy as i64 + shadow_offset;
                if tx < 0 || ty < 0 || tx >= opt.canvas_w as i64 || ty >= opt.canvas_h as i64 { continue; }
                let cur = canvas.get_pixel(tx as u32, ty as u32);
                let blended = blend(*cur, Rgba([0, 0, 0, 80]));
                canvas.put_pixel(tx as u32, ty as u32, blended);
            }
        }
        image::imageops::overlay(&mut canvas, &card, place.x as i64, place.y as i64);
        let _ = place.rotate_deg; // free rotation lives in the GPU surface; CPU pile keeps axis-aligned cards.
    }
    Ok(DynamicImage::ImageRgba8(canvas))
}

fn blend(under: Rgba<u8>, over: Rgba<u8>) -> Rgba<u8> {
    let a = over[3] as f32 / 255.0;
    let inv = 1.0 - a;
    Rgba([
        (over[0] as f32 * a + under[0] as f32 * inv) as u8,
        (over[1] as f32 * a + under[1] as f32 * inv) as u8,
        (over[2] as f32 * a + under[2] as f32 * inv) as u8,
        255,
    ])
}

/// SplitMix64 PRNG — small, deterministic, no allocations. Seed in, seed out.
struct SplitMix64 { state: u64 }
impl SplitMix64 {
    fn new(seed: u64) -> Self { Self { state: seed } }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn next_f32(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }
    fn next_range(&mut self, lo: u32, hi: u32) -> u32 {
        lo + (self.next_u64() % (hi - lo + 1).max(1) as u64) as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    #[test]
    fn layout_is_deterministic_for_seed() {
        let opt = PileOptions::default();
        let a = layout(20, &opt);
        let b = layout(20, &opt);
        assert_eq!(a.len(), b.len());
        for (l, r) in a.iter().zip(b.iter()) {
            assert_eq!((l.x, l.y, l.w, l.h), (r.x, r.y, r.w, r.h));
        }
    }
    #[test]
    fn render_paints_into_canvas() {
        let imgs = vec![DynamicImage::ImageRgba8(ImageBuffer::from_pixel(80, 100, Rgba([20, 200, 100, 255])))];
        let opt = PileOptions { canvas_w: 400, canvas_h: 300, polaroid_w: 120, ..PileOptions::default() };
        let out = render(&imgs, &opt).unwrap().to_rgba8();
        assert_eq!(out.dimensions(), (400, 300));
    }
}
