//! The non-destructive editor.
//!
//! `tulipix_photos::editor` already owns everything that matters: an `EditOp`
//! enum, an `EditStack` with an undo cursor, a deterministic executor, and a
//! `photo_edits` row that persists the stack as JSON. None of that changes
//! here. What this module adds is the half a UI needs and a renderer does not:
//! a decoded source held between keystrokes, a bounded preview written where
//! Flutter can load it, and parameters rather than a stack -- because a
//! brightness slider dragged across its range must not leave 60 ops behind.
//!
//! The stack is derived from the parameters on every render, so what lands in
//! `photo_edits` is an ordinary `EditStack` that the Slint editor reads back
//! unchanged.

use crate::db::photos_pool;
use anyhow::{Context, Result};
use flutter_rust_bridge::frb;
use std::path::PathBuf;
use std::sync::Mutex;
use tulipix_photos::editor::{crop, curves, filters, ops, redeye};

/// Longest edge of the preview. Big enough to judge an edit, small enough that
/// a nine-channel per-pixel pass finishes between two drags of a slider.
const PREVIEW_DIM: u32 = 1400;

/// A full-frame crop: `crop::apply` clamps w/h against the post-rotation size,
/// so u32::MAX means "whatever the image is after the transform".
const FULL_FRAME: u32 = u32::MAX;

// ----------------------------------------------------------------- types ----

/// The nine sliders, in `adjust::AdjustParams` order and range. f64 because
/// Dart has no f32; they narrow on the way into the op.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjust {
    pub exposure: f64,
    pub contrast: f64,
    pub saturation: f64,
    pub temperature: f64,
    pub tint: f64,
    pub highlights: f64,
    pub shadows: f64,
    pub blacks: f64,
    pub whites: f64,
}

impl Default for Adjust {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 0.0,
            saturation: 0.0,
            temperature: 0.0,
            tint: 0.0,
            highlights: 0.0,
            shadows: 0.0,
            blacks: 0.0,
            whites: 0.0,
        }
    }
}

impl Adjust {
    fn is_identity(&self) -> bool {
        *self == Self::default()
    }
}

/// One control point of a tone curve, both axes 0..1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CurvePoint {
    pub x: f64,
    pub y: f64,
}

/// A crop rectangle in source pixels, as `crop::CropSpec` stores it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CropRect {
    pub x: i64,
    pub y: i64,
    pub w: i64,
    pub h: i64,
}

/// One red-eye correction: a circle in source pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RedEyeSpot {
    pub cx: i64,
    pub cy: i64,
    pub radius: i64,
}

/// 256 buckets per channel, computed from the preview. Slint reads the
/// original; at 1400px the distribution is the same shape and the read is
/// free, because the preview is already decoded.
#[derive(Debug, Clone)]
pub struct Histogram {
    pub r: Vec<u32>,
    pub g: Vec<u32>,
    pub b: Vec<u32>,
    pub luma: Vec<u32>,
}

/// Everything the editor screen draws. One value per control, not a stack --
/// a slider needs to know where its handle sits, not how it got there.
#[derive(Debug, Clone)]
pub struct EditorState {
    pub item_id: i64,
    pub source: String,
    /// The rendered preview on disk. The file name carries a counter, so
    /// Flutter's image cache can never hand back the previous render.
    pub preview: String,
    pub width: i64,
    pub height: i64,
    pub adjust: Adjust,
    /// One of: bw, sepia, vintage, drama, hdr, polaroid, faded. Empty = none.
    pub filter: String,
    pub filter_strength: f64,
    pub rotate_deg: f64,
    pub flip_h: bool,
    pub flip_v: bool,
    pub sharpen: f64,
    pub enhance: bool,
    /// None when the whole frame is kept. Coordinates are source pixels, so
    /// the overlay has to scale them against `width`/`height`.
    pub crop: Option<CropRect>,
    /// Per channel, in `all, r, g, b` order. Empty vec = identity.
    pub curve_all: Vec<CurvePoint>,
    pub curve_r: Vec<CurvePoint>,
    pub curve_g: Vec<CurvePoint>,
    pub curve_b: Vec<CurvePoint>,
    pub red_eye: Vec<RedEyeSpot>,
    pub histogram: Histogram,
    pub can_undo: bool,
    pub can_redo: bool,
    /// True when the parameters differ from what `photo_edits` holds.
    pub dirty: bool,
    /// Ops loaded from a stack this build cannot represent -- curves, text,
    /// red-eye, the AI hooks. They are applied first and written back on save,
    /// so editing a photo here never silently discards them.
    pub carried_ops: i64,
    /// Absolute path of the last export, for the confirmation line.
    pub last_export: String,
}

#[derive(Debug, Clone)]
pub enum EditCmd {
    Open { item_id: i64 },
    SetAdjust { adjust: Adjust },
    SetFilter { preset: String, strength: f64 },
    SetSharpen { amount: f64 },
    /// Named `enabled`, not `on`: `on` is a reserved word in Dart and frb
    /// escapes it to `on_`, which every call site then has to spell.
    SetEnhance { enabled: bool },
    /// Relative, in quarter turns. Negative turns anticlockwise.
    Rotate { quarter_turns: i64 },
    Flip { horizontal: bool },
    /// Source-pixel rectangle. A zero-area rect clears the crop.
    SetCrop { rect: CropRect },
    /// free | square | 3x2 | 4x3 | 16x9 | 9x16 | 5x4 -- the largest box of
    /// that ratio that fits, centred, exactly as `AspectPreset::fit` computes.
    SetAspect { preset: String },
    /// all | r | g | b. An empty point list restores the identity curve.
    SetCurve { channel: String, points: Vec<CurvePoint> },
    AddRedEye { spot: RedEyeSpot },
    ClearRedEye,
    Undo,
    Redo,
    /// Back to the unedited photo, but still unsaved until `Save`.
    Reset,
    Save,
    /// Drop the saved stack entirely and forget the photo was ever edited.
    Revert,
    /// Render at full resolution and write a copy. Not `Export`: that becomes
    /// `EditCmd.export` in Dart, and `export` is a reserved word there.
    SaveCopy {
        format: String,
        quality: i64,
        keep_exif: bool,
        out_dir: String,
        stem: String,
    },
}

// --------------------------------------------------------------- session ----

/// One editable state of the photo. Undo and redo move between these, rather
/// than between ops, so a slider drag is one step back and not sixty.
/// `frb(ignore)`: private state, not part of the contract. Without it the
/// generator emits codecs for a private struct and the crate stops compiling.
#[frb(ignore)]
#[derive(Debug, Clone, PartialEq, Default)]
struct Params {
    adjust: Adjust,
    filter: String,
    filter_strength: f64,
    rotate_deg: f32,
    flip_h: bool,
    flip_v: bool,
    sharpen: f64,
    enhance: bool,
    crop: Option<CropRect>,
    curve_all: Vec<CurvePoint>,
    curve_r: Vec<CurvePoint>,
    curve_g: Vec<CurvePoint>,
    curve_b: Vec<CurvePoint>,
    red_eye: Vec<RedEyeSpot>,
}

#[frb(ignore)]
struct EditSession {
    item_id: i64,
    source: PathBuf,
    width: i64,
    height: i64,
    /// Ops from the loaded stack that this build cannot represent.
    carried: Vec<ops::EditOp>,
    history: Vec<Params>,
    cursor: usize,
    /// Index in `history` that matches what is in the database.
    saved: usize,
    preview: Option<PathBuf>,
    seq: u64,
    last_export: String,
}

impl EditSession {
    fn current(&self) -> &Params {
        &self.history[self.cursor]
    }

    /// Record a new state, dropping any redo branch. A no-op change is not a
    /// history entry: releasing a slider you did not move should not cost an
    /// undo step.
    fn push(&mut self, p: Params) -> bool {
        if *self.current() == p {
            return false;
        }
        self.history.truncate(self.cursor + 1);
        self.history.push(p);
        self.cursor = self.history.len() - 1;
        if self.saved > self.cursor {
            self.saved = usize::MAX;
        }
        true
    }

    /// `scale` maps source pixels onto whatever image the stack will run over.
    /// Save and export render the full-resolution original, so they pass 1.0;
    /// the preview runs over a 1400px copy, and a crop rectangle measured in
    /// source pixels would cut a hole out of the middle of it.
    fn stack(&self, scale: f64) -> ops::EditStack {
        let px = |v: i64| (v as f64 * scale).round().max(0.0) as u32;
        let mut st = ops::EditStack::new();
        // Carried ops go through unscaled. Crop and red-eye never reach here
        // -- `open` folds those into parameters -- so what is left is text and
        // the AI ops, which this build cannot edit anyway. In the preview a
        // carried text layer therefore lands at its full-resolution offset;
        // what gets saved is the op it came in as, unchanged.
        for op in &self.carried {
            st.push(op.clone());
        }
        let p = self.current();
        let cropped = p.crop.filter(|c| c.w > 0 && c.h > 0);
        if cropped.is_some() || p.rotate_deg.abs() > 0.01 || p.flip_h || p.flip_v {
            let r = cropped.unwrap_or(CropRect { x: 0, y: 0, w: 0, h: 0 });
            st.push(ops::EditOp::Crop {
                x: px(r.x),
                y: px(r.y),
                // A cleared crop is the full frame, which `crop::apply` reads
                // as u32::MAX and clamps against the post-rotation size.
                w: if cropped.is_some() { px(r.w).max(1) } else { FULL_FRAME },
                h: if cropped.is_some() { px(r.h).max(1) } else { FULL_FRAME },
                rotate_deg: p.rotate_deg,
                flip_h: p.flip_h,
                flip_v: p.flip_v,
            });
        }
        for (channel, points) in [
            (curves::Channel::All, &p.curve_all),
            (curves::Channel::R, &p.curve_r),
            (curves::Channel::G, &p.curve_g),
            (curves::Channel::B, &p.curve_b),
        ] {
            if points.len() >= 2 {
                st.push(ops::EditOp::Curve {
                    channel,
                    points: points.iter().map(|q| (q.x as f32, q.y as f32)).collect(),
                });
            }
        }
        if !p.red_eye.is_empty() {
            st.push(ops::EditOp::RedEye {
                circles: p
                    .red_eye
                    .iter()
                    .map(|c| redeye::EyeCircle {
                        cx: (c.cx as f64 * scale).round() as i32,
                        cy: (c.cy as f64 * scale).round() as i32,
                        radius: px(c.radius).max(1),
                    })
                    .collect(),
            });
        }
        if !p.adjust.is_identity() {
            let a = p.adjust;
            st.push(ops::EditOp::Adjust {
                exposure: a.exposure as f32,
                contrast: a.contrast as f32,
                saturation: a.saturation as f32,
                temperature: a.temperature as f32,
                tint: a.tint as f32,
                highlights: a.highlights as f32,
                shadows: a.shadows as f32,
                blacks: a.blacks as f32,
                whites: a.whites as f32,
            });
        }
        if let Some(preset) = preset_from(&p.filter) {
            st.push(ops::EditOp::Filter { preset, strength: p.filter_strength as f32 });
        }
        if p.sharpen.abs() > f64::EPSILON {
            st.push(ops::EditOp::Sharpen { amount: p.sharpen as f32, radius: 1.0 });
        }
        if p.enhance {
            st.push(ops::EditOp::Enhance);
        }
        st
    }
}

fn preset_from(name: &str) -> Option<filters::Preset> {
    Some(match name {
        "bw" => filters::Preset::BW,
        "sepia" => filters::Preset::Sepia,
        "vintage" => filters::Preset::Vintage,
        "drama" => filters::Preset::Drama,
        "hdr" => filters::Preset::Hdr,
        "polaroid" => filters::Preset::Polaroid,
        "faded" => filters::Preset::Faded,
        _ => return None,
    })
}

/// True for 90 and 270, the two rotations that swap width and height.
fn quarter_turned(deg: f32) -> bool {
    let d = deg.rem_euclid(180.0);
    (d - 90.0).abs() < 45.0
}

fn aspect_from(name: &str) -> crop::AspectPreset {
    match name {
        "square" => crop::AspectPreset::Square,
        "3x2" => crop::AspectPreset::R3x2,
        "4x3" => crop::AspectPreset::R4x3,
        "16x9" => crop::AspectPreset::R16x9,
        "9x16" => crop::AspectPreset::R9x16,
        "5x4" => crop::AspectPreset::R5x4,
        _ => crop::AspectPreset::Free,
    }
}

fn preset_name(p: &filters::Preset) -> &'static str {
    match p {
        filters::Preset::BW => "bw",
        filters::Preset::Sepia => "sepia",
        filters::Preset::Vintage => "vintage",
        filters::Preset::Drama => "drama",
        filters::Preset::Hdr => "hdr",
        filters::Preset::Polaroid => "polaroid",
        filters::Preset::Faded => "faded",
    }
}

fn session() -> &'static Mutex<Option<EditSession>> {
    static S: std::sync::OnceLock<Mutex<Option<EditSession>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

/// The decoded, downscaled source. Decoding a 12 MB original on every slider
/// release would dominate the render; this keeps exactly one photo, because
/// only one is ever open.
fn source_cache() -> &'static Mutex<Option<(i64, image::DynamicImage)>> {
    static C: std::sync::OnceLock<Mutex<Option<(i64, image::DynamicImage)>>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn lock<T>(m: &'static Mutex<T>) -> std::sync::MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// -------------------------------------------------------------- exported ----

/// Apply one editor command and return the screen that results from it.
///
/// Every arm ends in a re-render, which is the point: the preview is the only
/// honest answer to "what did that do", and computing it in Dart would mean a
/// second implementation of nine adjustment curves that has to agree with this
/// one forever.
pub async fn photos_edit(cmd: EditCmd) -> Result<EditorState> {
    let pool = photos_pool().await?;

    if let EditCmd::Open { item_id } = &cmd {
        return open(pool, *item_id).await;
    }

    let mut next = {
        let g = lock(session());
        let s = g.as_ref().context("no photo is open in the editor")?;
        s.current().clone()
    };

    match cmd {
        EditCmd::Open { .. } => unreachable!("handled above"),
        EditCmd::SetAdjust { adjust } => next.adjust = adjust,
        EditCmd::SetFilter { preset, strength } => {
            next.filter = if preset_from(&preset).is_some() { preset } else { String::new() };
            next.filter_strength = strength.clamp(0.0, 1.0);
        }
        EditCmd::SetSharpen { amount } => next.sharpen = amount.clamp(0.0, 4.0),
        EditCmd::SetEnhance { enabled } => next.enhance = enabled,
        EditCmd::Rotate { quarter_turns } => {
            let deg = next.rotate_deg + (quarter_turns as f32) * 90.0;
            next.rotate_deg = deg.rem_euclid(360.0);
        }
        EditCmd::Flip { horizontal } => {
            if horizontal {
                next.flip_h = !next.flip_h;
            } else {
                next.flip_v = !next.flip_v;
            }
        }
        EditCmd::SetCrop { rect } => {
            next.crop = (rect.w > 0 && rect.h > 0).then_some(rect);
        }
        EditCmd::SetAspect { preset } => {
            let (w, h) = {
                let g = lock(session());
                let s = g.as_ref().context("no photo is open in the editor")?;
                let (w, h) = (s.width.max(1) as u32, s.height.max(1) as u32);
                // Crop is applied after the rotation, so a quarter turn swaps
                // the frame the preset has to fit inside.
                if quarter_turned(next.rotate_deg) { (h, w) } else { (w, h) }
            };
            let spec = aspect_from(&preset).fit(w, h);
            next.crop = (preset != "free").then_some(CropRect {
                x: spec.x as i64,
                y: spec.y as i64,
                w: spec.w as i64,
                h: spec.h as i64,
            });
        }
        EditCmd::SetCurve { channel, points } => {
            // Fewer than two points is not a curve; treat it as "remove it"
            // rather than as an op that silently does nothing.
            let pts = if points.len() >= 2 { points } else { Vec::new() };
            match channel.as_str() {
                "r" => next.curve_r = pts,
                "g" => next.curve_g = pts,
                "b" => next.curve_b = pts,
                _ => next.curve_all = pts,
            }
        }
        EditCmd::AddRedEye { spot } => next.red_eye.push(spot),
        EditCmd::ClearRedEye => next.red_eye.clear(),
        EditCmd::Reset => next = Params::default(),
        // Every guard here lives in its own block. `drop(g)` is not enough:
        // the future still counts it as live across the await, and a
        // MutexGuard is not Send, so the whole task stops being spawnable.
        EditCmd::Undo => {
            {
                let mut g = lock(session());
                let s = g.as_mut().context("no photo is open in the editor")?;
                if s.cursor > 0 {
                    s.cursor -= 1;
                }
            }
            return render().await;
        }
        EditCmd::Redo => {
            {
                let mut g = lock(session());
                let s = g.as_mut().context("no photo is open in the editor")?;
                if s.cursor + 1 < s.history.len() {
                    s.cursor += 1;
                }
            }
            return render().await;
        }
        EditCmd::Save => {
            let (item_id, stack) = {
                let g = lock(session());
                let s = g.as_ref().context("no photo is open in the editor")?;
                (s.item_id, s.stack(1.0))
            };
            ops::save(pool, item_id, &stack).await?;
            {
                let mut g = lock(session());
                if let Some(s) = g.as_mut() {
                    s.saved = s.cursor;
                }
            }
            return render().await;
        }
        EditCmd::Revert => {
            let item_id = {
                let g = lock(session());
                g.as_ref().context("no photo is open in the editor")?.item_id
            };
            sqlx::query("DELETE FROM photo_edits WHERE item_id = ?")
                .bind(item_id)
                .execute(pool)
                .await?;
            {
                let mut g = lock(session());
                if let Some(s) = g.as_mut() {
                    s.carried.clear();
                    s.history = vec![Params::default()];
                    s.cursor = 0;
                    s.saved = 0;
                }
            }
            return render().await;
        }
        EditCmd::SaveCopy { format, quality, keep_exif, out_dir, stem } => {
            return export(format, quality, keep_exif, out_dir, stem).await;
        }
    }

    {
        let mut g = lock(session());
        let s = g.as_mut().context("no photo is open in the editor")?;
        s.push(next);
    }
    render().await
}

// ------------------------------------------------------------- internals ----

async fn open(pool: &sqlx::SqlitePool, item_id: i64) -> Result<EditorState> {
    let row: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT items.abs_path, COALESCE(pm.width, 0), COALESCE(pm.height, 0) \
         FROM items LEFT JOIN photo_meta pm ON pm.item_id = items.id \
         WHERE items.id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    let (abs, width, height) = row.context("no such photo")?;

    // Fold the saved stack back into parameters. Anything this build cannot
    // represent is carried untouched rather than dropped -- a photo edited in
    // the Slint build must survive being opened here.
    let stack = ops::load(pool, item_id).await?;
    let mut params = Params::default();
    let mut carried = Vec::new();
    for op in stack.active_ops() {
        match op {
            ops::EditOp::Adjust {
                exposure,
                contrast,
                saturation,
                temperature,
                tint,
                highlights,
                shadows,
                blacks,
                whites,
            } => {
                params.adjust = Adjust {
                    exposure: *exposure as f64,
                    contrast: *contrast as f64,
                    saturation: *saturation as f64,
                    temperature: *temperature as f64,
                    tint: *tint as f64,
                    highlights: *highlights as f64,
                    shadows: *shadows as f64,
                    blacks: *blacks as f64,
                    whites: *whites as f64,
                };
            }
            ops::EditOp::Filter { preset, strength } => {
                params.filter = preset_name(preset).into();
                params.filter_strength = *strength as f64;
            }
            ops::EditOp::Sharpen { amount, .. } => params.sharpen = *amount as f64,
            ops::EditOp::Enhance => params.enhance = true,
            ops::EditOp::Curve { channel, points } => {
                let pts: Vec<CurvePoint> = points
                    .iter()
                    .map(|(x, y)| CurvePoint { x: *x as f64, y: *y as f64 })
                    .collect();
                match channel {
                    curves::Channel::R => params.curve_r = pts,
                    curves::Channel::G => params.curve_g = pts,
                    curves::Channel::B => params.curve_b = pts,
                    curves::Channel::All => params.curve_all = pts,
                }
            }
            ops::EditOp::RedEye { circles } => {
                params.red_eye = circles
                    .iter()
                    .map(|c| RedEyeSpot {
                        cx: c.cx as i64,
                        cy: c.cy as i64,
                        radius: c.radius as i64,
                    })
                    .collect();
            }
            // Only a full-frame crop is a rotate/flip this editor can show. A
            // real crop rectangle is carried, because losing it would change
            // the photo's framing behind the user's back.
            ops::EditOp::Crop { x, y, w, h, rotate_deg, flip_h, flip_v } => {
                params.rotate_deg = *rotate_deg;
                params.flip_h = *flip_h;
                params.flip_v = *flip_v;
                params.crop = (*w != FULL_FRAME && *h != FULL_FRAME).then_some(CropRect {
                    x: *x as i64,
                    y: *y as i64,
                    w: *w as i64,
                    h: *h as i64,
                });
            }
            other => carried.push(other.clone()),
        }
    }

    let source = PathBuf::from(&abs);
    let decode_src = source.clone();
    // The decode is the only thing that knows the real dimensions. photo_meta
    // usually has them, but a row scanned before the EXIF pass has zeros, and
    // every geometric op is measured against these -- a zero would put the
    // preview scale at infinity.
    let (decoded, real_w, real_h) =
        tokio::task::spawn_blocking(move || -> Result<(image::DynamicImage, i64, i64)> {
            let img = image::open(&decode_src)
                .with_context(|| format!("decode {}", decode_src.display()))?;
            let (w, h) = (img.width(), img.height());
            let small = img.resize(PREVIEW_DIM, PREVIEW_DIM, image::imageops::FilterType::Triangle);
            Ok((small, w as i64, h as i64))
        })
        .await??;
    let (width, height) = if width > 0 && height > 0 { (width, height) } else { (real_w, real_h) };
    *lock(source_cache()) = Some((item_id, decoded));

    *lock(session()) = Some(EditSession {
        item_id,
        source,
        width,
        height,
        carried,
        history: vec![params],
        cursor: 0,
        saved: 0,
        preview: None,
        seq: 0,
        last_export: String::new(),
    });

    render().await
}

/// Render the current parameters into a preview file and describe the result.
async fn render() -> Result<EditorState> {
    let (item_id, source_w, seq, previous) = {
        let mut g = lock(session());
        let s = g.as_mut().context("no photo is open in the editor")?;
        s.seq += 1;
        (s.item_id, s.width.max(1), s.seq, s.preview.take())
    };

    let base = {
        let c = lock(source_cache());
        match c.as_ref() {
            Some((id, img)) if *id == item_id => img.clone(),
            // The cache is dropped when another photo opens; nothing to render.
            _ => anyhow::bail!("the editor's source image is no longer loaded"),
        }
    };

    // The preview is the source resized to PREVIEW_DIM; every op measured in
    // source pixels has to come down with it.
    let scale = base.width() as f64 / source_w as f64;
    let stack = {
        let g = lock(session());
        g.as_ref().context("no photo is open in the editor")?.stack(scale)
    };

    let out = preview_path(item_id, seq).context("no cache dir")?;
    let write_to = out.clone();
    // Nine per-pixel passes over two megapixels is not something to do on the
    // runtime that is also answering the UI. The histogram is computed here
    // too, off the rendered image, so it costs one pass and not a second
    // decode.
    let hist = tokio::task::spawn_blocking(move || -> Result<curves::Histogram> {
        let img = ops::apply(base, &stack)?;
        let h = curves::histogram(&img);
        if let Some(p) = write_to.parent() {
            std::fs::create_dir_all(p)?;
        }
        img.into_rgb8().save_with_format(&write_to, image::ImageFormat::Jpeg)?;
        Ok(h)
    })
    .await??;

    // One preview file at a time; the previous render is dead the moment the
    // next one lands.
    if let Some(p) = previous {
        let _ = std::fs::remove_file(p);
    }

    let mut g = lock(session());
    let s = g.as_mut().context("no photo is open in the editor")?;
    s.preview = Some(out.clone());
    let p = s.current().clone();
    Ok(EditorState {
        item_id: s.item_id,
        source: s.source.to_string_lossy().into_owned(),
        preview: out.to_string_lossy().into_owned(),
        width: s.width,
        height: s.height,
        adjust: p.adjust,
        filter: p.filter,
        filter_strength: p.filter_strength,
        rotate_deg: p.rotate_deg as f64,
        flip_h: p.flip_h,
        flip_v: p.flip_v,
        sharpen: p.sharpen,
        enhance: p.enhance,
        crop: p.crop,
        curve_all: p.curve_all,
        curve_r: p.curve_r,
        curve_g: p.curve_g,
        curve_b: p.curve_b,
        red_eye: p.red_eye,
        histogram: Histogram {
            r: hist.r.to_vec(),
            g: hist.g.to_vec(),
            b: hist.b.to_vec(),
            luma: hist.luma.to_vec(),
        },
        can_undo: s.cursor > 0,
        can_redo: s.cursor + 1 < s.history.len(),
        dirty: s.saved != s.cursor,
        carried_ops: s.carried.len() as i64,
        last_export: s.last_export.clone(),
    })
}

fn preview_path(item_id: i64, seq: u64) -> Option<PathBuf> {
    tulipix_core::paths::cache_dir()
        .map(|d| d.join("editor").join(format!("{item_id}-{seq}.jpg")))
}

/// Export renders at full resolution, not from the preview -- the preview is
/// 1400px and exporting it would quietly downscale the photo.
async fn export(
    format: String,
    quality: i64,
    keep_exif: bool,
    out_dir: String,
    stem: String,
) -> Result<EditorState> {
    use tulipix_photos::editor::export as ex;

    let (source, stack) = {
        let g = lock(session());
        let s = g.as_ref().context("no photo is open in the editor")?;
        (s.source.clone(), s.stack(1.0))
    };

    let spec = ex::ExportSpec {
        format: match format.as_str() {
            "png" => ex::ExportFormat::Png,
            "webp" => ex::ExportFormat::Webp,
            "tiff" => ex::ExportFormat::Tiff,
            "heic" => ex::ExportFormat::Heic,
            _ => ex::ExportFormat::Jpeg,
        },
        quality: quality.clamp(1, 100) as u8,
        exif: if keep_exif { ex::ExifPolicy::Preserve } else { ex::ExifPolicy::Scrub },
        out_dir: PathBuf::from(out_dir),
        stem,
    };

    let src = source.clone();
    let written = tokio::task::spawn_blocking(move || -> Result<PathBuf> {
        let full = image::open(&src).with_context(|| format!("decode {}", src.display()))?;
        let rendered = ops::apply(full, &stack)?;
        ex::write(&rendered, Some(&src), &spec)
    })
    .await??;

    {
        let mut g = lock(session());
        if let Some(s) = g.as_mut() {
            s.last_export = written.to_string_lossy().into_owned();
        }
    }
    render().await
}

/// Drop the open photo and its decoded source. Called when the editor closes;
/// without it a 1400px RGBA buffer stays resident for the rest of the session.
#[frb(sync)]
pub fn photos_edit_close() {
    if let Some(s) = lock(session()).take() {
        if let Some(p) = s.preview {
            let _ = std::fs::remove_file(p);
        }
    }
    *lock(source_cache()) = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session_for(p: Params) -> EditSession {
        EditSession {
            item_id: 1,
            source: PathBuf::from("/x.jpg"),
            width: 100,
            height: 100,
            carried: Vec::new(),
            history: vec![p],
            cursor: 0,
            saved: 0,
            preview: None,
            seq: 0,
            last_export: String::new(),
        }
    }

    #[test]
    fn identity_params_produce_an_empty_stack() {
        // An untouched photo must not write a Crop op with rotate 0 -- that
        // would mark every opened photo as edited.
        let s = session_for(Params::default());
        assert!(s.stack(1.0).ops.is_empty());
    }

    #[test]
    fn a_curve_needs_two_points_to_be_an_op() {
        // One dragged point is a gesture in progress, not an edit; emitting an
        // op for it would put a no-op Curve in everyone's saved stack.
        let one = session_for(Params {
            curve_all: vec![CurvePoint { x: 0.5, y: 0.5 }],
            ..Params::default()
        });
        assert!(one.stack(1.0).ops.is_empty());
        let two = session_for(Params {
            curve_all: vec![CurvePoint { x: 0.0, y: 0.0 }, CurvePoint { x: 1.0, y: 1.0 }],
            ..Params::default()
        });
        assert_eq!(two.stack(1.0).ops.len(), 1);
    }

    #[test]
    fn a_cleared_crop_is_a_full_frame_op_not_a_zero_rect() {
        // w/h of 0 would crop the photo to nothing; crop::apply reads
        // u32::MAX as "the whole frame after the transform".
        let s = session_for(Params { rotate_deg: 90.0, crop: None, ..Params::default() });
        match &s.stack(1.0).ops[0] {
            ops::EditOp::Crop { w, h, .. } => {
                assert_eq!((*w, *h), (FULL_FRAME, FULL_FRAME));
            }
            other => panic!("expected a crop op, got {other:?}"),
        }
    }

    #[test]
    fn the_preview_stack_scales_geometry_and_the_saved_one_does_not() {
        // The saved crop is in source pixels. The preview runs over a 1400px
        // copy of a 4000px photo, so the same rectangle has to come down with
        // it -- otherwise the preview crops a corner out of the middle.
        let s = session_for(Params {
            crop: Some(CropRect { x: 400, y: 200, w: 2000, h: 1000 }),
            red_eye: vec![RedEyeSpot { cx: 800, cy: 600, radius: 40 }],
            ..Params::default()
        });
        let saved = s.stack(1.0);
        let preview = s.stack(0.25);
        match (&saved.ops[0], &preview.ops[0]) {
            (
                ops::EditOp::Crop { x: sx, w: sw, .. },
                ops::EditOp::Crop { x: px, w: pw, .. },
            ) => {
                assert_eq!((*sx, *sw), (400, 2000));
                assert_eq!((*px, *pw), (100, 500));
            }
            other => panic!("expected two crop ops, got {other:?}"),
        }
        match (&saved.ops[1], &preview.ops[1]) {
            (ops::EditOp::RedEye { circles: sc }, ops::EditOp::RedEye { circles: pc }) => {
                assert_eq!((sc[0].cx, sc[0].radius), (800, 40));
                assert_eq!((pc[0].cx, pc[0].radius), (200, 10));
            }
            other => panic!("expected two red-eye ops, got {other:?}"),
        }
    }

    #[test]
    fn a_scaled_crop_never_collapses_to_nothing() {
        // A 3px rectangle on a photo scaled down 40x rounds to zero, and a
        // zero-width crop is a panic in image::crop_imm.
        let s = session_for(Params {
            crop: Some(CropRect { x: 0, y: 0, w: 3, h: 3 }),
            ..Params::default()
        });
        match &s.stack(0.025).ops[0] {
            ops::EditOp::Crop { w, h, .. } => assert_eq!((*w, *h), (1, 1)),
            other => panic!("expected a crop op, got {other:?}"),
        }
    }

    #[test]
    fn ops_are_ordered_geometry_then_tone_then_effect() {
        let s = session_for(Params {
            adjust: Adjust { exposure: 0.5, ..Adjust::default() },
            filter: "sepia".into(),
            filter_strength: 0.8,
            rotate_deg: 90.0,
            sharpen: 1.0,
            enhance: true,
            ..Params::default()
        });
        let kinds: Vec<_> = s
            .stack(1.0)
            .ops
            .iter()
            .map(|o| match o {
                ops::EditOp::Crop { .. } => "crop",
                ops::EditOp::Adjust { .. } => "adjust",
                ops::EditOp::Filter { .. } => "filter",
                ops::EditOp::Sharpen { .. } => "sharpen",
                ops::EditOp::Enhance => "enhance",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, ["crop", "adjust", "filter", "sharpen", "enhance"]);
    }

    #[test]
    fn a_no_op_change_is_not_an_undo_step() {
        let mut s = session_for(Params::default());
        assert!(!s.push(Params::default()));
        assert_eq!(s.history.len(), 1);
        assert!(s.push(Params { enhance: true, ..Params::default() }));
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn redo_is_dropped_once_a_new_edit_lands() {
        let mut s = session_for(Params::default());
        s.push(Params { enhance: true, ..Params::default() });
        s.cursor = 0;
        s.push(Params { sharpen: 2.0, ..Params::default() });
        assert_eq!(s.history.len(), 2);
        assert_eq!(s.cursor, 1);
    }
}
