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

/// Funnel every ORT error through this. Generic, because from rc.13 the
/// builder's errors carry the builder back (`Error<SessionBuilder>`), which
/// `?` cannot hand to anyhow.
fn oe<R>(e: ort::Error<R>) -> anyhow::Error {
    anyhow::anyhow!("ort: {e}")
}

/// Build an ORT session for a model file. CPU EP for now (always available);
/// GPU EPs wire in once `ep::auto_pick` is connected to ORT's provider list.
fn build_session(model: &Path) -> Result<Session> {
    let s = Session::builder()
        .map_err(oe)?
        .with_execution_providers([
            ort::ep::CPU::default().build(),
        ])
        .map_err(oe)?
        .commit_from_file(model)
        .map_err(oe)
        .with_context(|| format!("load onnx model {}", model.display()))?;
    Ok(s)
}

/// Resize to fit a square `side`×`side` box preserving aspect, placed at the
/// top-left, remaining pixels filled with `pad`. Returns the image and the
/// scale applied, which the caller needs to map boxes back to source pixels.
///
/// Top-left rather than centred: that is what the YOLOX and SCRFD reference
/// implementations do, and centring would offset every box by half the padding.
fn letterbox(img: &DynamicImage, side: u32, pad: u8) -> (RgbImage, f32) {
    let (w, h) = img.dimensions();
    let scale = (side as f32 / w as f32).min(side as f32 / h as f32);
    let (nw, nh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    let resized = img.resize_exact(nw, nh, image::imageops::FilterType::Triangle).to_rgb8();
    let mut canvas = RgbImage::from_pixel(side, side, image::Rgb([pad, pad, pad]));
    image::imageops::replace(&mut canvas, &resized, 0, 0);
    (canvas, scale)
}

/// One decoded detection in source-image pixels.
#[derive(Debug, Clone, Copy)]
struct Box2D {
    x1: f32,
    y1: f32,
    x2: f32,
    y2: f32,
    score: f32,
    class: usize,
}

fn iou(a: &Box2D, b: &Box2D) -> f32 {
    let x1 = a.x1.max(b.x1);
    let y1 = a.y1.max(b.y1);
    let x2 = a.x2.min(b.x2);
    let y2 = a.y2.min(b.y2);
    let inter = (x2 - x1).max(0.0) * (y2 - y1).max(0.0);
    let area_a = (a.x2 - a.x1).max(0.0) * (a.y2 - a.y1).max(0.0);
    let area_b = (b.x2 - b.x1).max(0.0) * (b.y2 - b.y1).max(0.0);
    let union = area_a + area_b - inter;
    if union <= 0.0 { 0.0 } else { inter / union }
}

/// Greedy non-maximum suppression, highest score first. Boxes of different
/// classes never suppress each other.
fn nms(mut boxes: Vec<Box2D>, iou_thresh: f32) -> Vec<Box2D> {
    boxes.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut keep: Vec<Box2D> = Vec::new();
    'outer: for b in boxes {
        for k in &keep {
            if k.class == b.class && iou(k, &b) > iou_thresh {
                continue 'outer;
            }
        }
        keep.push(b);
    }
    keep
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
        let in_name = session.inputs().first().context("model has no input")?.name().to_owned();
        let out_name = session.outputs().first().context("model has no output")?.name().to_owned();
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
        let in_name = session.inputs().first().context("model has no input")?.name().to_owned();
        let out_name = session.outputs().first().context("model has no output")?.name().to_owned();
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

/// YOLOX-S COCO-80 object detection.
///
/// Verified against the installed blob rather than assumed:
///   IN   `images` f32 [1, 3, 640, 640]
///   OUT  `output` f32 [1, 8400, 85]
/// 8400 = 80² + 40² + 20², the strides 8/16/32 grids already concatenated into
/// one tensor in that order; 85 = cx, cy, w, h, objectness, then 80 class
/// scores.
///
/// YOLOX takes **raw RGB in 0..255** — no `/255`, no ImageNet mean/std. This is
/// the one that fails silently: normalised input yields confident nonsense
/// rather than an error.
pub struct OrtTagger {
    session: Mutex<Session>,
    in_name: String,
    out_name: String,
    side: u32,
    conf_thresh: f32,
    iou_thresh: f32,
}

impl OrtTagger {
    pub fn load(model: &Path) -> Result<Self> {
        let session = build_session(model)?;
        let input = session.inputs().first().context("model has no input")?;
        let in_name = input.name().to_owned();
        let out_name = session.outputs().first().context("model has no output")?.name().to_owned();
        Ok(Self {
            session: Mutex::new(session),
            in_name,
            out_name,
            side: 640,
            conf_thresh: 0.30,
            iou_thresh: 0.45,
        })
    }
}

impl crate::ai::tags::Tagger for OrtTagger {
    fn source(&self) -> &str { "yolox" }

    fn predict(&self, img: &DynamicImage) -> Result<Vec<crate::ai::tags::Detection>> {
        let (ow, oh) = img.dimensions();
        if ow == 0 || oh == 0 { return Ok(Vec::new()); }
        let (canvas, scale) = letterbox(img, self.side, 114);

        // Raw 0..255, CHW. Deliberately NOT `to_nchw`, which divides by 255.
        let s = self.side as usize;
        let mut data = vec![0f32; 3 * s * s];
        for (i, px) in canvas.pixels().enumerate() {
            data[i] = px[0] as f32;
            data[s * s + i] = px[1] as f32;
            data[2 * s * s + i] = px[2] as f32;
        }
        let tensor =
            ort::value::Tensor::from_array((vec![1i64, 3, s as i64, s as i64], data)).map_err(oe)?;

        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs![self.in_name.as_str() => tensor])
            .map_err(oe)?;
        let (shape, out) = outputs[self.out_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(oe)?;
        anyhow::ensure!(shape.len() == 3 && shape[2] >= 6, "unexpected YOLOX output {shape:?}");
        let anchors = shape[1] as usize;
        let stride_len = shape[2] as usize;
        let n_classes = stride_len - 5;

        // Rebuild the anchor grid the same way it was concatenated: all of
        // stride 8, then 16, then 32.
        let mut grid: Vec<(f32, f32, f32)> = Vec::with_capacity(anchors);
        for stride in [8u32, 16, 32] {
            let g = self.side / stride;
            for gy in 0..g {
                for gx in 0..g {
                    grid.push((gx as f32, gy as f32, stride as f32));
                }
            }
        }
        anchors
            .eq(&grid.len())
            .then_some(())
            .with_context(|| format!("anchor count {anchors} != grid {}", grid.len()))?;

        let mut boxes: Vec<Box2D> = Vec::new();
        for (i, &(gx, gy, stride)) in grid.iter().enumerate() {
            let row = &out[i * stride_len..(i + 1) * stride_len];
            let obj = row[4];
            if obj < self.conf_thresh { continue; } // cheap reject before the argmax
            let (class, cls_score) = row[5..5 + n_classes]
                .iter()
                .enumerate()
                .fold((0usize, 0f32), |acc, (c, &v)| if v > acc.1 { (c, v) } else { acc });
            let score = obj * cls_score;
            if score < self.conf_thresh { continue; }

            // Decode, then undo the letterbox scale to land in source pixels.
            let cx = (row[0] + gx) * stride / scale;
            let cy = (row[1] + gy) * stride / scale;
            let bw = row[2].exp() * stride / scale;
            let bh = row[3].exp() * stride / scale;
            boxes.push(Box2D {
                x1: cx - bw / 2.0,
                y1: cy - bh / 2.0,
                x2: cx + bw / 2.0,
                y2: cy + bh / 2.0,
                score,
                class,
            });
        }

        // `Detection.bbox` is documented as x,y,w,h normalised 0..1.
        let (fw, fh) = (ow as f32, oh as f32);
        Ok(nms(boxes, self.iou_thresh)
            .into_iter()
            .map(|b| {
                let x = (b.x1 / fw).clamp(0.0, 1.0);
                let y = (b.y1 / fh).clamp(0.0, 1.0);
                crate::ai::tags::Detection {
                    class: b.class,
                    confidence: b.score,
                    bbox: [x, y, (b.x2 / fw).clamp(0.0, 1.0) - x, (b.y2 / fh).clamp(0.0, 1.0) - y],
                }
            })
            .collect())
    }
}

/// SCRFD face detection (the `detection/` half of the buffalo_s pack).
///
/// Verified against the installed blob:
///   IN   `input.1` f32 [1, 3, ?, ?]  — dynamic, traced at 640×640
///   OUT  nine tensors, positionally grouped and **not** semantically named:
///          [0..3] scores  [12800,1] [3200,1] [800,1]
///          [3..6] bbox    [12800,4] [3200,4] [800,4]
///          [6..9] kps     [12800,10] [3200,10] [800,10]
///
/// 12800 = 80·80·2, 3200 = 40·40·2, 800 = 20·20·2 — strides 8/16/32 with **two
/// anchors per cell**. Both the grouping and the anchor count are invisible
/// from the output names, which are just numbers.
///
/// Normalisation is `(x - 127.5) / 128`, RGB — insightface's convention, and
/// different from both YOLOX (raw) and ArcFace (`/127.5`).
pub struct OrtFaceDetector {
    session: Mutex<Session>,
    in_name: String,
    out_names: Vec<String>,
    side: u32,
    conf_thresh: f32,
    iou_thresh: f32,
}

const SCRFD_STRIDES: [u32; 3] = [8, 16, 32];
const SCRFD_ANCHORS: usize = 2;

impl OrtFaceDetector {
    pub fn load(model: &Path) -> Result<Self> {
        let session = build_session(model)?;
        let in_name = session.inputs().first().context("model has no input")?.name().to_owned();
        let out_names: Vec<String> = session.outputs().iter().map(|o| o.name().to_owned()).collect();
        anyhow::ensure!(
            out_names.len() >= 6,
            "SCRFD expects >=6 outputs (scores+bbox per stride), got {}",
            out_names.len()
        );
        Ok(Self {
            session: Mutex::new(session),
            in_name,
            out_names,
            side: 640,
            conf_thresh: 0.5,
            iou_thresh: 0.4,
        })
    }
}

impl crate::ai::faces::FaceDetector for OrtFaceDetector {
    fn detect(&self, img: &DynamicImage) -> Result<Vec<crate::ai::faces::BBox>> {
        let (ow, oh) = img.dimensions();
        if ow == 0 || oh == 0 { return Ok(Vec::new()); }
        let (canvas, scale) = letterbox(img, self.side, 0);

        let s = self.side as usize;
        let mut data = vec![0f32; 3 * s * s];
        for (i, px) in canvas.pixels().enumerate() {
            data[i] = (px[0] as f32 - 127.5) / 128.0;
            data[s * s + i] = (px[1] as f32 - 127.5) / 128.0;
            data[2 * s * s + i] = (px[2] as f32 - 127.5) / 128.0;
        }
        let tensor =
            ort::value::Tensor::from_array((vec![1i64, 3, s as i64, s as i64], data)).map_err(oe)?;

        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs![self.in_name.as_str() => tensor])
            .map_err(oe)?;

        let n = SCRFD_STRIDES.len();
        let mut boxes: Vec<Box2D> = Vec::new();
        for (k, stride) in SCRFD_STRIDES.iter().copied().enumerate() {
            let (_, scores) = outputs[self.out_names[k].as_str()]
                .try_extract_tensor::<f32>()
                .map_err(oe)?;
            let (bshape, deltas) = outputs[self.out_names[n + k].as_str()]
                .try_extract_tensor::<f32>()
                .map_err(oe)?;
            let width = *bshape.last().unwrap_or(&4) as usize;
            anyhow::ensure!(width == 4, "SCRFD bbox width {width}, want 4");

            let g = (self.side / stride) as usize;
            for idx in 0..scores.len() {
                if scores[idx] < self.conf_thresh { continue; }
                // Anchors are interleaved per cell: idx = (gy*g + gx)*A + a.
                let cell = idx / SCRFD_ANCHORS;
                let (gx, gy) = ((cell % g) as f32, (cell / g) as f32);
                let (ax, ay) = (gx * stride as f32, gy * stride as f32);
                // distance2bbox: the four values are distances from the anchor
                // centre to left/top/right/bottom, in stride units.
                let d = &deltas[idx * 4..idx * 4 + 4];
                boxes.push(Box2D {
                    x1: (ax - d[0] * stride as f32) / scale,
                    y1: (ay - d[1] * stride as f32) / scale,
                    x2: (ax + d[2] * stride as f32) / scale,
                    y2: (ay + d[3] * stride as f32) / scale,
                    score: scores[idx],
                    class: 0,
                });
            }
        }

        Ok(nms(boxes, self.iou_thresh)
            .into_iter()
            .map(|b| {
                let x = b.x1.max(0.0).min(ow as f32);
                let y = b.y1.max(0.0).min(oh as f32);
                crate::ai::faces::BBox {
                    x: x as i32,
                    y: y as i32,
                    w: (b.x2.min(ow as f32) - x).max(1.0) as u32,
                    h: (b.y2.min(oh as f32) - y).max(1.0) as u32,
                    score: b.score,
                }
            })
            .collect())
    }
}

/// ArcFace/w600k recognition (the `recognition/` half of buffalo_s).
///
/// Verified against the installed blob:
///   IN   `input.1` f32 [batch, 3, 112, 112]
///   OUT  f32 [1, 512]
///
/// **112×112, not 160.** `faces::CROP_SIZE` is 160 and its comment calls that
/// "the arcface input", which is wrong — the crops are 160 because that is a
/// nicer face thumbnail for the People tab, and this resizes them down. Feeding
/// 160 would not error; ORT would reject the shape, but a dynamic-axis export
/// would silently produce a meaningless vector.
///
/// Normalisation is `(x - 127.5) / 127.5`, RGB — note the denominator differs
/// from the detector's 128.
pub struct OrtFaceEmbedder {
    session: Mutex<Session>,
    in_name: String,
    out_name: String,
    side: u32,
    model: String,
}

impl OrtFaceEmbedder {
    pub fn load(model: &Path) -> Result<Self> {
        let session = build_session(model)?;
        let in_name = session.inputs().first().context("model has no input")?.name().to_owned();
        let out_name = session.outputs().first().context("model has no output")?.name().to_owned();
        Ok(Self {
            session: Mutex::new(session),
            in_name,
            out_name,
            side: 112,
            model: model
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("arcface")
                .to_string(),
        })
    }
}

impl crate::ai::faces::FaceEmbedder for OrtFaceEmbedder {
    fn model_name(&self) -> &str { &self.model }

    fn embed(&self, crop: &DynamicImage) -> Result<Vec<f32>> {
        let s = self.side as usize;
        let rgb = crop
            .resize_exact(self.side, self.side, image::imageops::FilterType::Triangle)
            .to_rgb8();
        let mut data = vec![0f32; 3 * s * s];
        for (i, px) in rgb.pixels().enumerate() {
            data[i] = (px[0] as f32 - 127.5) / 127.5;
            data[s * s + i] = (px[1] as f32 - 127.5) / 127.5;
            data[2 * s * s + i] = (px[2] as f32 - 127.5) / 127.5;
        }
        let tensor =
            ort::value::Tensor::from_array((vec![1i64, 3, s as i64, s as i64], data)).map_err(oe)?;

        let mut session = self.session.lock().unwrap();
        let outputs = session
            .run(ort::inputs![self.in_name.as_str() => tensor])
            .map_err(oe)?;
        let (shape, out) = outputs[self.out_name.as_str()]
            .try_extract_tensor::<f32>()
            .map_err(oe)?;
        let dim = crate::ai::face_clusters::EMBED_DIM;
        anyhow::ensure!(out.len() == dim, "arcface returned {} dims (shape {shape:?}), want {dim}", out.len());
        // Clustering compares by cosine; normalising here means it can use a
        // plain dot product and every stored vector is directly comparable.
        let mut v = out.to_vec();
        crate::ai::clip::l2_normalise(&mut v);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolve a model for the smoke tests: `$env` override first, else the
    /// copy the app itself installed. `None` means skip — CI has no blobs, and
    /// a test that cannot run should pass rather than fail.
    fn test_model(env: &str, name: &str, version: &str) -> Option<std::path::PathBuf> {
        if let Ok(p) = std::env::var(env) {
            let p = std::path::PathBuf::from(p);
            if p.exists() { return Some(p); }
        }
        let p = crate::ai::models::models_root()?
            .join(format!("{name}-{version}"))
            .join(format!("{name}.onnx"));
        p.exists().then_some(p)
    }

    /// YOLOX end-to-end. Asserts the shape contract and that decoding lands
    /// inside the image — a letterbox or stride mistake shows up as boxes
    /// outside 0..1 or as an anchor-count mismatch, both of which this catches
    /// without needing a labelled photo.
    #[test]
    fn ort_tagger_decodes_within_bounds() {
        use crate::ai::tags::Tagger;
        let Some(path) = test_model("TULIPIX_YOLOX_MODEL", "yolox-s", "1.0.0") else { return };
        let t = OrtTagger::load(&path).expect("load yolox");

        // Non-square on purpose: a letterbox that forgets the scale, or centres
        // instead of top-lefting, misplaces boxes only when w != h.
        let mut buf = image::RgbImage::new(900, 500);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = image::Rgb([(x % 256) as u8, (y % 256) as u8, 128]);
        }
        let dets = t.predict(&DynamicImage::ImageRgb8(buf)).expect("run yolox");
        eprintln!("yolox: {} detections on noise", dets.len());
        for d in &dets {
            assert!(d.class < 80, "class {} outside COCO-80", d.class);
            assert!((0.0..=1.0).contains(&d.confidence), "confidence {}", d.confidence);
            let [x, y, w, h] = d.bbox;
            assert!((0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y), "origin {x},{y}");
            assert!(w >= 0.0 && h >= 0.0 && x + w <= 1.001 && y + h <= 1.001, "box {:?}", d.bbox);
        }
    }

    /// SCRFD shape contract + in-bounds decode. Without a labelled face photo
    /// the useful assertion is that the nine outputs group the way the layout
    /// says and that boxes land inside the source image.
    #[test]
    fn ort_face_detector_decodes_within_bounds() {
        use crate::ai::faces::FaceDetector;
        let Some(path) = test_model("TULIPIX_SCRFD_MODEL", "face-det-500m", "1.0.0") else { return };
        let d = OrtFaceDetector::load(&path).expect("load scrfd");
        let mut buf = image::RgbImage::new(800, 600);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = image::Rgb([(x % 256) as u8, (y % 256) as u8, 90]);
        }
        let faces = d.detect(&DynamicImage::ImageRgb8(buf)).expect("run scrfd");
        eprintln!("scrfd: {} faces on noise", faces.len());
        for f in &faces {
            assert!(f.x >= 0 && f.y >= 0, "origin {},{}", f.x, f.y);
            assert!(f.x as u32 + f.w <= 800 && f.y as u32 + f.h <= 600, "box escapes image");
            assert!(f.score >= 0.5, "below threshold: {}", f.score);
        }
    }

    /// ArcFace: 512-d, unit length, and it must accept the 160×160 crops
    /// `extract_crops` writes by resizing them to its own 112.
    #[test]
    fn ort_face_embedder_returns_unit_512() {
        use crate::ai::faces::FaceEmbedder;
        let Some(path) = test_model("TULIPIX_ARCFACE_MODEL", "face-rec-500m", "1.0.0") else { return };
        let e = OrtFaceEmbedder::load(&path).expect("load arcface");
        let crop = DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
            crate::ai::faces::CROP_SIZE,
            crate::ai::faces::CROP_SIZE,
            image::Rgb([120, 100, 90]),
        ));
        let v = e.embed(&crop).expect("run arcface");
        assert_eq!(v.len(), crate::ai::face_clusters::EMBED_DIM);
        let norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "not unit length: {norm}");
    }

    #[test]
    fn nms_drops_overlaps_but_keeps_other_classes() {
        let b = |x: f32, s: f32, c: usize| Box2D { x1: x, y1: 0.0, x2: x + 10.0, y2: 10.0, score: s, class: c };
        // Two heavily-overlapping same-class boxes collapse to the better one.
        let keep = nms(vec![b(0.0, 0.9, 0), b(1.0, 0.5, 0)], 0.45);
        assert_eq!(keep.len(), 1);
        assert_eq!(keep[0].score, 0.9);
        // The same overlap in a different class is a different object.
        let keep = nms(vec![b(0.0, 0.9, 0), b(1.0, 0.5, 7)], 0.45);
        assert_eq!(keep.len(), 2);
    }

    #[test]
    fn letterbox_preserves_aspect_and_reports_scale() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(900, 500));
        let (canvas, scale) = letterbox(&img, 640, 114);
        assert_eq!((canvas.width(), canvas.height()), (640, 640));
        // Long side fills the box; short side scales by the same factor.
        assert!((scale - 640.0 / 900.0).abs() < 1e-6, "scale {scale}");
        assert_eq!(*canvas.get_pixel(639, 639), image::Rgb([114, 114, 114]), "pad colour");
    }

    /// Inspect a model's I/O — set `TULIPIX_INSPECT=/path/to/model.onnx`.
    #[test]
    fn inspect_model_io() {
        let Ok(path) = std::env::var("TULIPIX_INSPECT") else { return; };
        let s = build_session(std::path::Path::new(&path)).expect("load");
        for i in s.inputs() {
            eprintln!("IN  {} :: {:?}", i.name(), i.dtype());
        }
        for o in s.outputs() {
            eprintln!("OUT {} :: {:?}", o.name(), o.dtype());
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
