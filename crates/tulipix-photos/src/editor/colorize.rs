//! DeOldify ONNX colorisation.
//!
//! Production loads `deoldify.onnx` via the model registry. `NullColoriser`
//! returns an Err so the UI surfaces a clear "install model" prompt — there
//! is no good non-AI fallback for colorisation.

use anyhow::{anyhow, Result};
use image::DynamicImage;

pub trait Coloriser: Send + Sync {
    fn colorise(&self, img: &DynamicImage) -> Result<DynamicImage>;
}

pub struct NullColoriser;
impl Coloriser for NullColoriser {
    fn colorise(&self, _: &DynamicImage) -> Result<DynamicImage> {
        Err(anyhow!("DeOldify model not installed — Settings → AI Models → Install"))
    }
}

pub fn apply(img: DynamicImage, c: &dyn Coloriser) -> Result<DynamicImage> {
    c.colorise(&img)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn null_coloriser_errors_clearly() {
        let img = DynamicImage::ImageRgba8(ImageBuffer::from_pixel(2, 2, Rgba([0, 0, 0, 255])));
        let r = apply(img, &NullColoriser);
        assert!(r.is_err());
        let msg = r.err().unwrap().to_string();
        assert!(msg.contains("DeOldify"));
    }
}
