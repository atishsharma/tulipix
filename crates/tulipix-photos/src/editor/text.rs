//! Text tool (Picasa parity) — draw text layers directly on the photo
//! using a bundled font with font/colour/outline picker. CPU rasteriser
//! draws solid rectangles per glyph; the wgpu surface uses an SDF cache.
//! For the tests here we only check that the layer is round-trippable
//! through JSON and that placement bounds clamp into the image.

use image::{DynamicImage, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TextLayer {
    pub text:     String,
    pub x:        i32,
    pub y:        i32,
    pub font:     String,             // family name; "Sora" by default
    pub size_px:  u32,
    pub colour:   [u8; 4],
    pub outline:  Option<Outline>,
    pub rotate:   f32,                 // degrees
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Outline {
    pub colour: [u8; 4],
    pub width_px: u32,
}

/// Stamp the text layer onto a copy of `img`. The CPU rasteriser paints one
/// rectangle per glyph (no actual glyph shapes) — enough for tests and as a
/// fallback when the wgpu surface isn't available. Production renders via
/// the same Slint surface that hosts the live edit preview.
pub fn draw(img: DynamicImage, layer: &TextLayer) -> DynamicImage {
    let mut rgba: RgbaImage = img.to_rgba8();
    let (iw, ih) = (rgba.width() as i32, rgba.height() as i32);
    let glyph_w = (layer.size_px as i32 * 6) / 10; // monospace approx
    let glyph_h = layer.size_px as i32;
    for (idx, _ch) in layer.text.chars().enumerate() {
        let gx = layer.x + idx as i32 * (glyph_w + 1);
        let gy = layer.y;
        fill_rect(&mut rgba, gx, gy, glyph_w, glyph_h, layer.colour, iw, ih);
        if let Some(o) = &layer.outline {
            stroke_rect(&mut rgba, gx, gy, glyph_w, glyph_h, o.colour, o.width_px as i32, iw, ih);
        }
    }
    DynamicImage::ImageRgba8(rgba)
}

fn fill_rect(buf: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4], iw: i32, ih: i32) {
    let x0 = x.max(0).min(iw); let y0 = y.max(0).min(ih);
    let x1 = (x + w).max(0).min(iw); let y1 = (y + h).max(0).min(ih);
    for yy in y0..y1 {
        for xx in x0..x1 {
            buf.put_pixel(xx as u32, yy as u32, Rgba(c));
        }
    }
}

fn stroke_rect(buf: &mut RgbaImage, x: i32, y: i32, w: i32, h: i32, c: [u8; 4], thickness: i32, iw: i32, ih: i32) {
    for t in 0..thickness {
        // top + bottom
        fill_rect(buf, x - t, y - t, w + 2 * t, 1, c, iw, ih);
        fill_rect(buf, x - t, y + h - 1 + t, w + 2 * t, 1, c, iw, ih);
        // left + right
        fill_rect(buf, x - t, y - t, 1, h + 2 * t, c, iw, ih);
        fill_rect(buf, x + w - 1 + t, y - t, 1, h + 2 * t, c, iw, ih);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageBuffer;
    #[test]
    fn layer_round_trips_via_json() {
        let layer = TextLayer {
            text: "Hi!".into(), x: 10, y: 20, font: "Sora".into(),
            size_px: 32, colour: [255, 0, 0, 255],
            outline: Some(Outline { colour: [0,0,0,255], width_px: 2 }), rotate: 0.0,
        };
        let s = serde_json::to_string(&layer).unwrap();
        let back: TextLayer = serde_json::from_str(&s).unwrap();
        assert_eq!(back, layer);
    }
    #[test]
    fn draw_clamps_off_image_text() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(40, 40, Rgba([255, 255, 255, 255])));
        let layer = TextLayer {
            text: "WAYOFF".into(), x: 9000, y: 9000, font: "Sora".into(),
            size_px: 20, colour: [0, 0, 0, 255], outline: None, rotate: 0.0,
        };
        let out = draw(img, &layer).to_rgba8();
        // Nothing drawn — all pixels still white.
        assert_eq!(out.get_pixel(0, 0)[0], 255);
    }
}
