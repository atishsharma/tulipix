//! The pairing QR, rendered straight to a Slint image.
//!
//! Deliberately not `fast_qr`'s own image converter: that one goes through
//! resvg + tiny-skia to rasterise an SVG, which is a large dependency to add
//! for what is a grid of squares. Reading the module matrix and filling pixels
//! is a dozen lines and adds nothing to the tree.

use slint::{Image, Rgb8Pixel, SharedPixelBuffer};

/// Modules of white margin around the code. Four is what the spec requires for
/// a scanner to find the finder patterns against a busy background.
const QUIET: usize = 4;
/// Pixels per module. 8 keeps a 33-module code around 330px — big enough to
/// scan off a laptop screen from arm's length.
const SCALE: usize = 8;

/// Whether a module is printed dark.
///
/// `fast_qr::Module` is a `u8` packing the module's type in the high bits and
/// its dark/light bit in bit 0; `value()` reads that bit. The one line here
/// that depends on that representation.
fn is_dark(m: fast_qr::Module) -> bool {
    m.value()
}

/// A QR for `url`, or `None` when the payload will not fit in a code (which for
/// a `http://192.168.x.x:PORT/?k=…` URL means something has gone wrong upstream).
pub fn render(url: &str) -> Option<Image> {
    if url.is_empty() {
        return None;
    }
    let qr = fast_qr::QRBuilder::new(url).build().ok()?;
    let modules = qr.size;
    let side = (modules + QUIET * 2) * SCALE;

    // White ground, then darken the module squares. A QR must be dark-on-light:
    // inverting it is the single most common way to make one unscannable.
    let mut buf = SharedPixelBuffer::<Rgb8Pixel>::new(side as u32, side as u32);
    let width = buf.width() as usize;
    let pixels = buf.make_mut_slice();
    for p in pixels.iter_mut() {
        *p = Rgb8Pixel { r: 255, g: 255, b: 255 };
    }

    for my in 0..modules {
        for mx in 0..modules {
            if !is_dark(qr.data[my * modules + mx]) {
                continue;
            }
            let x0 = (mx + QUIET) * SCALE;
            let y0 = (my + QUIET) * SCALE;
            for y in y0..y0 + SCALE {
                for x in x0..x0 + SCALE {
                    pixels[y * width + x] = Rgb8Pixel { r: 0, g: 0, b: 0 };
                }
            }
        }
    }

    Some(Image::from_rgb8(buf))
}
