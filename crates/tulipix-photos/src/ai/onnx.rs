//! Real ONNX inference (feature `onnx`). Loads model blobs via the model
//! registry and runs them through ORT, implementing the editor's `Upscaler`
//! (and, later, `Coloriser`/`Healer`/`SkySegmenter`) traits with real models
//! instead of the Null fallbacks.
//!
//! Tensor I/O names are read from the session at load time so we don't hard-code
//! a single export's naming. Inputs are NCHW float32 RGB in [0,1]; outputs are
//! clamped back to 8-bit RGB.

use anyhow::{Context, Result};
use image::{DynamicImage, GenericImageView, RgbImage};
use ort::session::Session;
use std::path::Path;
use std::sync::Mutex;

use crate::editor::colorize::Coloriser;
use crate::editor::upscale::Upscaler;

/// `ort::Error` doesn't implement `std::error::Error`, so `?`/anyhow can't
/// convert it directly — funnel every ORT call through this.
fn oe(e: ort::Error) -> anyhow::Error {
    anyhow::anyhow!("ort: {e}")
}

/// Build an ORT session for a model file. CPU EP for now (always available);
/// GPU EPs wire in once `ep::auto_pick` is connected to ORT's provider list.
fn build_session(model: &Path) -> Result<Session> {
    let s = Session::builder()
        .map_err(oe)?
        .with_execution_providers([
            ort::execution_providers::CPUExecutionProvider::default().build(),
        ])
        .map_err(oe)?
        .commit_from_file(model)
        .map_err(oe)
        .with_context(|| format!("load onnx model {}", model.display()))?;
    Ok(s)
}

/// CHW float32 [0,1] tensor data from an RGB image.
fn to_nchw(img: &RgbImage) -> (Vec<i64>, Vec<f32>) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut data = vec![0f32; 3 * h * w];
    for (i, px) in img.pixels().enumerate() {
        // pixels() iterate row-major; i = y*w + x
        data[i] = px[0] as f32 / 255.0;
        data[h * w + i] = px[1] as f32 / 255.0;
        data[2 * h * w + i] = px[2] as f32 / 255.0;
    }
    (vec![1, 3, h as i64, w as i64], data)
}

/// Rebuild an RGB image from an NCHW [0,1] output tensor.
fn from_nchw(shape: &[i64], data: &[f32]) -> Result<RgbImage> {
    anyhow::ensure!(shape.len() == 4 && shape[1] == 3, "unexpected output shape {shape:?}");
    let (h, w) = (shape[2] as usize, shape[3] as usize);
    let plane = h * w;
    let mut img = RgbImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let r = (data[i].clamp(0.0, 1.0) * 255.0).round() as u8;
            let g = (data[plane + i].clamp(0.0, 1.0) * 255.0).round() as u8;
            let b = (data[2 * plane + i].clamp(0.0, 1.0) * 255.0).round() as u8;
            img.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
        }
    }
    Ok(img)
}

/// RealESRGAN super-resolution. The model's own scale factor applies (×4 for
/// x4plus); we downscale very large inputs first so a single un-tiled pass
/// stays within memory.
pub struct OrtUpscaler {
    session: Mutex<Session>,
    in_name: String,
    out_name: String,
    max_in_side: u32,
}

impl OrtUpscaler {
    pub fn load(model: &Path) -> Result<Self> {
        let session = build_session(model)?;
        let in_name = session.inputs.first().context("model has no input")?.name.clone();
        let out_name = session.outputs.first().context("model has no output")?.name.clone();
        Ok(Self { session: Mutex::new(session), in_name, out_name, max_in_side: 1024 })
    }

    fn run(&self, rgb: &RgbImage) -> Result<RgbImage> {
        let (shape, data) = to_nchw(rgb);
        let tensor = ort::value::Tensor::from_array((shape, data)).map_err(oe)?;
        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs![self.in_name.as_str() => tensor])
            .map_err(oe)?;
        let (oshape, odata) = outputs[self.out_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(oe)?;
        from_nchw(oshape, odata)
    }
}

impl Upscaler for OrtUpscaler {
    fn upscale(&self, img: &DynamicImage, _factor: u32) -> Result<DynamicImage> {
        let (w, h) = img.dimensions();
        // Cap the input side so a single pass fits in memory; the model's own
        // scale still applies on top.
        let src = if w.max(h) > self.max_in_side {
            let s = self.max_in_side as f32 / w.max(h) as f32;
            img.resize_exact((w as f32 * s) as u32, (h as f32 * s) as u32, image::imageops::FilterType::Lanczos3)
        } else {
            img.clone()
        };
        // Transformer SR models (swin2SR) want the input padded to a multiple
        // of the window size (8). Pad by edge-replication, run, then crop the
        // output back to scale·original.
        let mut rgb = src.to_rgb8();
        let (sw, sh) = (rgb.width(), rgb.height());
        let pad = |v: u32| (v + 7) / 8 * 8;
        let (pw, ph) = (pad(sw), pad(sh));
        if pw != sw || ph != sh {
            let mut padded = image::RgbImage::new(pw, ph);
            for y in 0..ph {
                for x in 0..pw {
                    let p = rgb.get_pixel(x.min(sw - 1), y.min(sh - 1));
                    padded.put_pixel(x, y, *p);
                }
            }
            rgb = padded;
        }
        let out = self.run(&rgb)?;
        // Infer the model scale from the padded round-trip, crop off padding.
        let scale = out.width() / pw;
        let want = DynamicImage::ImageRgb8(out).crop_imm(0, 0, sw * scale, sh * scale);
        Ok(want)
    }
}

/// DeOldify colourisation. The model takes a 0-255 (un-normalised) grayscale
/// image as 3-channel RGB at a fixed square size and returns BGR colour. We
/// recombine the model's chroma with the *original* full-res luminance (YCbCr)
/// so detail is preserved — the standard DeOldify post-process.
pub struct OrtColoriser {
    session: Mutex<Session>,
    in_name: String,
    out_name: String,
    size: u32,
}

impl OrtColoriser {
    pub fn load(model: &Path) -> Result<Self> {
        let session = build_session(model)?;
        let in_name = session.inputs.first().context("model has no input")?.name.clone();
        let out_name = session.outputs.first().context("model has no output")?.name.clone();
        Ok(Self { session: Mutex::new(session), in_name, out_name, size: 256 })
    }

    /// Run the model on a `size`×`size` grayscale-as-RGB image; return the raw
    /// colourised RGB (channels swapped from the model's BGR), still `size`².
    fn run(&self, gray_rgb: &RgbImage) -> Result<RgbImage> {
        let s = self.size as usize;
        // CHW, 0-255 float (NO normalisation — matches DeOldify export).
        let mut data = vec![0f32; 3 * s * s];
        for (i, px) in gray_rgb.pixels().enumerate() {
            data[i] = px[0] as f32;
            data[s * s + i] = px[1] as f32;
            data[2 * s * s + i] = px[2] as f32;
        }
        let tensor = ort::value::Tensor::from_array((vec![1i64, 3, s as i64, s as i64], data)).map_err(oe)?;
        let mut session = self.session.lock().unwrap();
        let outputs = session.run(ort::inputs![self.in_name.as_str() => tensor]).map_err(oe)?;
        let (_oshape, od) = outputs[self.out_name.as_str()].try_extract_tensor::<f32>().map_err(oe)?;
        let plane = s * s;
        let mut out = RgbImage::new(self.size, self.size);
        for y in 0..s {
            for x in 0..s {
                let i = y * s + x;
                // Model emits BGR → swap to RGB.
                let r = od[2 * plane + i].clamp(0.0, 255.0) as u8;
                let g = od[plane + i].clamp(0.0, 255.0) as u8;
                let b = od[i].clamp(0.0, 255.0) as u8;
                out.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
            }
        }
        Ok(out)
    }
}

impl Coloriser for OrtColoriser {
    fn colorise(&self, img: &DynamicImage) -> Result<DynamicImage> {
        let orig = img.to_rgb8();
        let (w, h) = (orig.width(), orig.height());
        // Grayscale → 3-channel → model size.
        let gray = DynamicImage::ImageRgb8(orig.clone()).grayscale().to_rgb8();
        let gray_small = image::imageops::resize(&gray, self.size, self.size, image::imageops::FilterType::CatmullRom);
        let colored_small = self.run(&gray_small)?;
        let colored = image::imageops::resize(&colored_small, w, h, image::imageops::FilterType::CatmullRom);
        // Recombine: original luminance (Y) + model chroma (Cb/Cr).
        let mut out = RgbImage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let o = orig.get_pixel(x, y);
                let c = colored.get_pixel(x, y);
                let yv = 0.299 * o[0] as f32 + 0.587 * o[1] as f32 + 0.114 * o[2] as f32;
                let (cr, cg, cb) = (c[0] as f32, c[1] as f32, c[2] as f32);
                let cb_ = -0.168736 * cr - 0.331264 * cg + 0.5 * cb + 128.0;
                let crr = 0.5 * cr - 0.418688 * cg - 0.081312 * cb + 128.0;
                let r = (yv + 1.402 * (crr - 128.0)).clamp(0.0, 255.0) as u8;
                let g = (yv - 0.344136 * (cb_ - 128.0) - 0.714136 * (crr - 128.0)).clamp(0.0, 255.0) as u8;
                let b = (yv + 1.772 * (cb_ - 128.0)).clamp(0.0, 255.0) as u8;
                out.put_pixel(x, y, image::Rgb([r, g, b]));
            }
        }
        Ok(DynamicImage::ImageRgb8(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inspect a model's I/O — set `TULIPIX_INSPECT=/path/to/model.onnx`.
    #[test]
    fn inspect_model_io() {
        let Ok(path) = std::env::var("TULIPIX_INSPECT") else { return; };
        let s = build_session(std::path::Path::new(&path)).expect("load");
        for i in &s.inputs {
            eprintln!("IN  {} :: {:?}", i.name, i.input_type);
        }
        for o in &s.outputs {
            eprintln!("OUT {} :: {:?}", o.name, o.output_type);
        }
    }

    /// DeOldify smoke test — set `TULIPIX_COLORIZE_MODEL=/path/to/deoldify.onnx`.
    #[test]
    fn ort_coloriser_runs_on_real_model() {
        let Ok(path) = std::env::var("TULIPIX_COLORIZE_MODEL") else { return; };
        let c = OrtColoriser::load(std::path::Path::new(&path)).expect("load model");
        // A non-trivial gray ramp so the model has something to colourise.
        let mut buf = image::RgbImage::new(128, 128);
        for (x, _y, p) in buf.enumerate_pixels_mut() { let v = (x * 2) as u8; *p = image::Rgb([v, v, v]); }
        let out = c.colorise(&DynamicImage::ImageRgb8(buf)).expect("run");
        let (w, h) = out.dimensions();
        eprintln!("colorize 128x128 -> {w}x{h}");
        assert_eq!((w, h), (128, 128));
    }

    /// Real-model smoke test — set `TULIPIX_TEST_MODEL=/path/to/sr.onnx`.
    /// Skipped (passes) when the env var is absent so CI without the blob is green.
    #[test]
    fn ort_upscaler_runs_on_real_model() {
        let Ok(path) = std::env::var("TULIPIX_TEST_MODEL") else { return; };
        let up = OrtUpscaler::load(std::path::Path::new(&path)).expect("load model");
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(48, 48, image::Rgb([120, 80, 40])));
        let out = up.upscale(&img, 4).expect("run");
        let (w, h) = out.dimensions();
        eprintln!("in 48x48 -> out {w}x{h}");
        assert!(w >= 96 && h >= 96, "expected upscaled output, got {w}x{h}");
    }
}
