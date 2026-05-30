//! Non-destructive photo editor.
//!
//! Edits are an ordered list of `EditOp` values, persisted as JSON in
//! `photo_edits.ops`. Re-running the list deterministically reproduces the
//! current rendered photo from the original on disk. Undo/redo walk the
//! `undo_idx` cursor along the same list — old ops above the cursor are
//! kept until a new op truncates them.
//!
//! Implementation notes:
//!   * Pure-Rust ops (adjust, curves, crop, filters, tint, text, redeye,
//!     enhance, sharpen, export) operate on `image::DynamicImage` so they
//!     run anywhere.
//!   * GPU ops (the same adjust/curves/sharpen kernels rebound to wgpu
//!     shaders) ship with the Slint surface; this module owns the CPU
//!     reference impl used by export + headless tests.
//!   * AI ops (heal, sky, upscale, colorize) declare their need for an
//!     ONNX model — the executor short-circuits with a clear error when
//!     the model is missing.

pub mod adjust;
pub mod color_pop;
pub mod colorize;
pub mod copy_paste;
pub mod crop;
pub mod curves;
pub mod enhance;
pub mod export;
pub mod filters;
pub mod heal;
pub mod ops;
pub mod redeye;
pub mod sharpen;
pub mod sky;
pub mod text;
pub mod tint;
pub mod upscale;

pub use ops::{EditOp, EditStack};
