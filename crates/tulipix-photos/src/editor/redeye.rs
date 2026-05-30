//! Auto red-eye removal.
//!
//! Two paths:
//!   * Automatic — InsightFace eye landmarks supply circle centres; we
//!     pick pixels inside each circle where R is dominant (R > 1.5×G and
//!     R > 1.5×B) and desaturate the red channel to the mean of G+B.
//!   * Manual — the user clicks each eye in the editor and we apply the
//!     same kernel inside the user-supplied circle.
//!
//! Either way the op stores the circle list so it replays deterministically.

use image::{DynamicImage, Rgba};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EyeCircle {
    pub cx: i32,
    pub cy: i32,
    pub radius: u32,
}

pub fn apply(img: DynamicImage, circles: &[EyeCircle]) -> DynamicImage {
    let mut rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as i32, rgba.height() as i32);
    for c in circles {
        let r2 = (c.radius * c.radius) as i32;
        for dy in -(c.radius as i32)..=(c.radius as i32) {
            for dx in -(c.radius as i32)..=(c.radius as i32) {
                if dx * dx + dy * dy > r2 { continue; }
                let x = c.cx + dx;
                let y = c.cy + dy;
                if x < 0 || y < 0 || x >= w || y >= h { continue; }
                let p = rgba.get_pixel(x as u32, y as u32);
                let (r, g, b) = (p[0] as f32, p[1] as f32, p[2] as f32);
                if r > g * 1.5 && r > b * 1.5 && r > 80.0 {
                    let target = ((g + b) * 0.5) as u8;
                    rgba.put_pixel(x as u32, y as u32, Rgba([target, p[1], p[2], p[3]]));
                }
            }
        }
    }
    DynamicImage::ImageRgba8(rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};
    #[test]
    fn desaturates_dominant_red_inside_circle() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(20, 20);
        for (_, _, p) in buf.enumerate_pixels_mut() { *p = Rgba([220, 20, 20, 255]); }
        let img = DynamicImage::ImageRgba8(buf);
        let out = apply(img, &[EyeCircle { cx: 10, cy: 10, radius: 4 }]).to_rgba8();
        let inside = out.get_pixel(10, 10);
        assert!(inside[0] < 100, "red should be desaturated");
        let outside = out.get_pixel(0, 0);
        assert_eq!(outside[0], 220, "outside circle untouched");
    }
    #[test]
    fn skips_pixels_not_red_dominant() {
        let mut buf = ImageBuffer::<Rgba<u8>, _>::new(8, 8);
        for (_, _, p) in buf.enumerate_pixels_mut() { *p = Rgba([100, 100, 100, 255]); }
        let img = DynamicImage::ImageRgba8(buf);
        let out = apply(img, &[EyeCircle { cx: 4, cy: 4, radius: 3 }]).to_rgba8();
        assert_eq!(out.get_pixel(4, 4)[0], 100);
    }
}
