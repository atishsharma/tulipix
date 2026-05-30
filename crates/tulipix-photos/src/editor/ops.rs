//! `EditOp` enum + the `EditStack` undo/redo cursor.
//!
//! Every editor module pushes a typed variant of `EditOp` onto the stack;
//! the executor walks the stack from index 0 up to `undo_idx` and applies
//! each op in turn. Re-rendering is deterministic — same ops, same input,
//! same output bytes.

use anyhow::Result;
use image::DynamicImage;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EditOp {
    Adjust  { exposure: f32, contrast: f32, saturation: f32, temperature: f32, tint: f32, highlights: f32, shadows: f32, blacks: f32, whites: f32 },
    Curve   { channel: super::curves::Channel, points: Vec<(f32, f32)> },
    Crop    { x: u32, y: u32, w: u32, h: u32, rotate_deg: f32, flip_h: bool, flip_v: bool },
    Filter  { preset: super::filters::Preset, strength: f32 },
    Tint    { rgb: [u8; 3], strength: f32 },
    Text    { layer: super::text::TextLayer },
    RedEye  { circles: Vec<super::redeye::EyeCircle> },
    Enhance,
    Sharpen { amount: f32, radius: f32 },
    Resize  { w: u32, h: u32 },        // Lanczos3 resample to exact dimensions
    Heal    { mask_path: String },     // brushstroke mask serialized to disk
    Sky     { mask_path: String, texture: String },
    Upscale { factor: u32 },
    Colorize,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EditStack {
    pub ops:      Vec<EditOp>,
    pub undo_idx: usize, // count of ops currently applied; len(ops) == undo_idx unless redo-able
}

impl EditStack {
    pub fn new() -> Self { Self::default() }

    /// Push a fresh op. Truncates anything above the cursor (no branching).
    pub fn push(&mut self, op: EditOp) {
        self.ops.truncate(self.undo_idx);
        self.ops.push(op);
        self.undo_idx = self.ops.len();
    }

    pub fn undo(&mut self) -> bool {
        if self.undo_idx == 0 { false } else { self.undo_idx -= 1; true }
    }

    pub fn redo(&mut self) -> bool {
        if self.undo_idx == self.ops.len() { false } else { self.undo_idx += 1; true }
    }

    pub fn active_ops(&self) -> &[EditOp] {
        &self.ops[..self.undo_idx]
    }

    pub fn from_json(s: &str) -> Result<Self> {
        if s.trim().is_empty() { return Ok(Self::default()); }
        Ok(serde_json::from_str(s)?)
    }
    pub fn to_json(&self) -> String { serde_json::to_string(self).unwrap_or_default() }
}

/// Apply every op in `stack.active_ops()` to `img` and return the result.
/// AI ops short-circuit when their hook returns Err (model missing).
pub fn apply(mut img: DynamicImage, stack: &EditStack) -> Result<DynamicImage> {
    use super::*;
    for op in stack.active_ops() {
        img = match op {
            EditOp::Adjust { exposure, contrast, saturation, temperature, tint, highlights, shadows, blacks, whites } => {
                adjust::apply(img, adjust::AdjustParams {
                    exposure: *exposure, contrast: *contrast, saturation: *saturation,
                    temperature: *temperature, tint: *tint,
                    highlights: *highlights, shadows: *shadows,
                    blacks: *blacks, whites: *whites,
                })
            }
            EditOp::Curve { channel, points } => curves::apply(img, *channel, points),
            EditOp::Crop  { x, y, w, h, rotate_deg, flip_h, flip_v } => {
                crop::apply(img, crop::CropSpec { x: *x, y: *y, w: *w, h: *h, rotate_deg: *rotate_deg, flip_h: *flip_h, flip_v: *flip_v })
            }
            EditOp::Filter { preset, strength } => filters::apply(img, *preset, *strength),
            EditOp::Tint   { rgb, strength } => tint::apply(img, *rgb, *strength),
            EditOp::Text   { layer } => text::draw(img, layer),
            EditOp::RedEye { circles } => redeye::apply(img, circles),
            EditOp::Enhance => enhance::apply(img),
            EditOp::Sharpen { amount, radius } => filters::sharpen(img, *amount, *radius),
            EditOp::Resize { w, h } => {
                let (tw, th) = ((*w).max(1), (*h).max(1));
                img.resize_exact(tw, th, image::imageops::FilterType::Lanczos3)
            }
            EditOp::Heal { mask_path } => heal::apply(img, std::path::Path::new(mask_path), &heal::NullHealer)?,
            EditOp::Sky  { mask_path, texture } => sky::apply(img, std::path::Path::new(mask_path), texture, &sky::NullSky)?,
            EditOp::Upscale { factor } => upscale::apply(img, *factor, &upscale::NullUpscaler)?,
            EditOp::Colorize => colorize::apply(img, &colorize::NullColoriser)?,
        };
    }
    Ok(img)
}

/// Load stack from DB. Returns an empty stack if the row is absent.
pub async fn load(pool: &SqlitePool, item_id: i64) -> Result<EditStack> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT ops, undo_idx FROM photo_edits WHERE item_id = ?",
    ).bind(item_id).fetch_optional(pool).await?;
    let Some((ops_json, undo_idx)) = row else { return Ok(EditStack::default()); };
    let ops: Vec<EditOp> = serde_json::from_str(&ops_json).unwrap_or_default();
    let undo_idx = (undo_idx as usize).min(ops.len());
    Ok(EditStack { ops, undo_idx })
}

pub async fn save(pool: &SqlitePool, item_id: i64, stack: &EditStack) -> Result<()> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    sqlx::query(
        "INSERT INTO photo_edits (item_id, ops, undo_idx, updated) VALUES (?, ?, ?, ?)
         ON CONFLICT(item_id) DO UPDATE SET ops = excluded.ops, undo_idx = excluded.undo_idx, updated = excluded.updated",
    )
    .bind(item_id)
    .bind(serde_json::to_string(&stack.ops)?)
    .bind(stack.undo_idx as i64)
    .bind(now)
    .execute(pool).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_round_trip() {
        let mut s = EditStack::new();
        s.push(EditOp::Enhance);
        s.push(EditOp::Sharpen { amount: 1.0, radius: 1.0 });
        assert_eq!(s.active_ops().len(), 2);
        assert!(s.undo());
        assert_eq!(s.active_ops().len(), 1);
        assert!(s.redo());
        assert_eq!(s.active_ops().len(), 2);
        // Push truncates redo tail.
        s.undo();
        s.push(EditOp::Enhance);
        assert!(!s.redo());
    }

    #[tokio::test]
    async fn save_load_round_trip() {
        let (_t, pool) = crate::schema::tests::open_pool().await;
        sqlx::query("INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES ('/p.jpg', 0, 1, 0, 'photos', 0, 0)").execute(&pool).await.unwrap();
        let id: i64 = sqlx::query_scalar("SELECT id FROM items").fetch_one(&pool).await.unwrap();
        let mut s = EditStack::new();
        s.push(EditOp::Enhance);
        save(&pool, id, &s).await.unwrap();
        let r = load(&pool, id).await.unwrap();
        assert_eq!(r.active_ops().len(), 1);
    }
}
