//! Pure crop-rect math for the profile cover/avatar cropper, plus the PNG
//! crop+save I/O that consumes it.
//!
//! The Slint `CropDialog` tracks pan/zoom in *display* pixels against a fixed
//! on-screen mask (`mask_w`/`mask_h`). `crop_rect` inverts that same affine
//! transform to find the equivalent rectangle in the *source* image's native
//! pixel space, so the final crop is taken from the full-resolution original
//! rather than a downscaled preview.

use anyhow::Result;
use std::path::Path;

/// Returns `(x, y, w, h)` in source-image pixels. `zoom` is clamped to >= 1.0
/// (the mask is always fully covered, never showing empty edges). `pan_x`/
/// `pan_y` are display-pixel offsets, same units as `mask_w`/`mask_h`.
pub fn crop_rect(
    nat_w: u32, nat_h: u32,
    mask_w: f32, mask_h: f32,
    zoom: f32, pan_x: f32, pan_y: f32,
) -> (u32, u32, u32, u32) {
    let (nat_w_f, nat_h_f) = (nat_w as f32, nat_h as f32);
    let base_scale = (mask_w / nat_w_f).max(mask_h / nat_h_f);
    let scale = base_scale * zoom.max(1.0);
    let crop_w = (mask_w / scale).min(nat_w_f);
    let crop_h = (mask_h / scale).min(nat_h_f);
    let center_x = nat_w_f / 2.0 - pan_x / scale;
    let center_y = nat_h_f / 2.0 - pan_y / scale;
    let x0 = (center_x - crop_w / 2.0).clamp(0.0, nat_w_f - crop_w);
    let y0 = (center_y - crop_h / 2.0).clamp(0.0, nat_h_f - crop_h);
    (x0.round() as u32, y0.round() as u32, crop_w.round() as u32, crop_h.round() as u32)
}

/// Crop `src_path` per `crop_rect`, resize to `(out_w, out_h)`, write PNG to
/// `dest_path` (creating parent dirs as needed).
pub fn save_cropped(
    src_path: &Path, dest_path: &Path,
    mask_w: f32, mask_h: f32, out_w: u32, out_h: u32,
    zoom: f32, pan_x: f32, pan_y: f32,
) -> Result<()> {
    let img = image::ImageReader::open(src_path)?.with_guessed_format()?.decode()?;
    let (nat_w, nat_h) = (img.width(), img.height());
    let (x, y, w, h) = crop_rect(nat_w, nat_h, mask_w, mask_h, zoom, pan_x, pan_y);
    let cropped = img.crop_imm(x, y, w, h);
    let resized = cropped.resize_exact(out_w, out_h, image::imageops::FilterType::Lanczos3);
    if let Some(parent) = dest_path.parent() { std::fs::create_dir_all(parent)?; }
    resized.save_with_format(dest_path, image::ImageFormat::Png)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_1_centers_and_fills_wide_mask() {
        // 2000x1000 source, 1200x300 mask (4:1 cover ratio). base_scale = 0.6
        // (limited by width), so the crop is full-width, vertically centered.
        let (x, y, w, h) = crop_rect(2000, 1000, 1200.0, 300.0, 1.0, 0.0, 0.0);
        assert_eq!((x, y, w, h), (0, 250, 2000, 500));
    }

    #[test]
    fn pan_clamps_to_available_slack() {
        // Same setup, huge downward pan request — must clamp so the crop
        // never runs off the bottom edge of the source image.
        let (_, y, _, h) = crop_rect(2000, 1000, 1200.0, 300.0, 1.0, 0.0, -1000.0);
        assert_eq!(y + h, 1000); // clamped flush to the bottom edge
    }

    #[test]
    fn zoom_in_shrinks_the_crop_window() {
        // 1000x1000 source, 480x480 mask (square avatar), zoom=2.0.
        let (x, y, w, h) = crop_rect(1000, 1000, 480.0, 480.0, 2.0, 0.0, 0.0);
        assert_eq!((x, y, w, h), (250, 250, 500, 500));
    }
}
