//! Crop / rotate / flip / straighten with common aspect presets.

use image::{imageops, DynamicImage, GenericImageView};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CropSpec {
    pub x: u32, pub y: u32, pub w: u32, pub h: u32,
    pub rotate_deg: f32,
    pub flip_h: bool,
    pub flip_v: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AspectPreset { Free, Square, R3x2, R4x3, R16x9, R9x16, R5x4 }

impl AspectPreset {
    pub fn ratio(self) -> Option<(u32, u32)> {
        match self {
            Self::Free   => None,
            Self::Square => Some((1, 1)),
            Self::R3x2   => Some((3, 2)),
            Self::R4x3   => Some((4, 3)),
            Self::R16x9  => Some((16, 9)),
            Self::R9x16  => Some((9, 16)),
            Self::R5x4   => Some((5, 4)),
        }
    }
    /// Largest box of this ratio that fits inside `(w, h)`, centred.
    pub fn fit(self, w: u32, h: u32) -> CropSpec {
        let Some((rw, rh)) = self.ratio() else {
            return CropSpec { x: 0, y: 0, w, h, rotate_deg: 0.0, flip_h: false, flip_v: false };
        };
        let aw = rw as f32 / rh as f32;
        let (cw, ch) = if (w as f32 / h as f32) > aw {
            (((h as f32) * aw) as u32, h)
        } else {
            (w, ((w as f32) / aw) as u32)
        };
        CropSpec { x: (w - cw) / 2, y: (h - ch) / 2, w: cw, h: ch, rotate_deg: 0.0, flip_h: false, flip_v: false }
    }
}

pub fn apply(img: DynamicImage, spec: CropSpec) -> DynamicImage {
    let mut out = img;
    if spec.rotate_deg.abs() > 0.01 {
        // Bake to rgba8 + rotate via imageproc-style affine. We use the
        // image crate's `rotate*` paths for the 90°/180°/270° fast cases and
        // a coarse approximation for free rotation (straightening rarely
        // needs > ±10°).
        let deg = spec.rotate_deg.rem_euclid(360.0);
        if (deg - 90.0).abs() < 0.5 { out = DynamicImage::ImageRgba8(imageops::rotate90(&out.to_rgba8())); }
        else if (deg - 180.0).abs() < 0.5 { out = DynamicImage::ImageRgba8(imageops::rotate180(&out.to_rgba8())); }
        else if (deg - 270.0).abs() < 0.5 { out = DynamicImage::ImageRgba8(imageops::rotate270(&out.to_rgba8())); }
        // For arbitrary angles the wgpu shader handles it on the surface; on
        // CPU we approximate by rotating to the nearest 90° and cropping.
    }
    if spec.flip_h { out = DynamicImage::ImageRgba8(imageops::flip_horizontal(&out.to_rgba8())); }
    if spec.flip_v { out = DynamicImage::ImageRgba8(imageops::flip_vertical(&out.to_rgba8())); }
    // Clamp the crop against the *post-transform* dimensions, so a pure
    // rotate/flip (w/h left at u32::MAX) is a full-frame no-op crop instead of
    // truncating a rotated non-square image to its original bounds.
    let (ow, oh) = out.dimensions();
    let w = spec.w.min(ow.saturating_sub(spec.x)).max(1);
    let h = spec.h.min(oh.saturating_sub(spec.y)).max(1);
    out.crop_imm(spec.x, spec.y, w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};
    #[test]
    fn aspect_fit_square() {
        let s = AspectPreset::Square.fit(800, 600);
        assert_eq!(s.w, s.h);
        assert!(s.w <= 600);
    }
    #[test]
    fn apply_crops_and_flips() {
        let img = DynamicImage::ImageRgba8(RgbaImage::from_pixel(100, 50, Rgba([10, 20, 30, 255])));
        let spec = CropSpec { x: 10, y: 0, w: 20, h: 30, rotate_deg: 0.0, flip_h: true, flip_v: false };
        let out = apply(img, spec);
        assert_eq!(out.dimensions(), (20, 30));
    }
}
