//! Auto-caption + alt-text generation. Pipeline:
//!   1. CLIP zero-shot scores image against a candidate label vocabulary.
//!   2. Top-k labels (+ optional EXIF hints) get folded into an LLM prompt.
//!   3. LLM streams a one-sentence caption + concise alt-text.
//!
//! Caller persists the two strings into `photos.caption` / `photos.alt_text`,
//! which the FTS index already covers — so the new fields become searchable
//! and screen-reader-readable for free. Cap-gated `photos.ai.captions`.

use crate::llm::{ChatMessage, LlmBackend};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use tulipix_core::caps::{is_allowed, Cap};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionRequest {
    pub photo_id: i64,
    pub abs_path: String,
    pub clip_labels: Vec<ScoredLabel>,
    pub exif_hint: Option<ExifHint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredLabel {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExifHint {
    pub camera: Option<String>,
    pub date: Option<String>,
    pub gps_place: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptionResult {
    pub caption: String,
    pub alt_text: String,
}

pub trait CaptionGenerator: Send + Sync {
    fn generate(&self, req: &CaptionRequest) -> Result<CaptionResult>;
}

/// LLM-backed generator. Composes top-k CLIP labels + EXIF into a strict
/// system prompt and asks the LLM for `{caption, alt_text}` JSON; falls back
/// to a deterministic template if the cap is denied or the LLM stream errors.
pub struct LlmCaptionGenerator<B: LlmBackend> {
    pub backend: B,
    pub top_k: usize,
}

impl<B: LlmBackend> LlmCaptionGenerator<B> {
    pub fn new(backend: B) -> Self { Self { backend, top_k: 5 } }
}

impl<B: LlmBackend> CaptionGenerator for LlmCaptionGenerator<B> {
    fn generate(&self, req: &CaptionRequest) -> Result<CaptionResult> {
        if !is_allowed(Cap::PhotosAiCaptions) {
            anyhow::bail!("photos.ai.captions denied");
        }
        let labels: Vec<&ScoredLabel> = req.clip_labels.iter().take(self.top_k).collect();
        let label_line = labels
            .iter()
            .map(|s| format!("{} ({:.2})", s.label, s.score))
            .collect::<Vec<_>>()
            .join(", ");
        let exif_line = req
            .exif_hint
            .as_ref()
            .map(|h| {
                let mut bits = vec![];
                if let Some(c) = &h.camera { bits.push(format!("camera={c}")); }
                if let Some(d) = &h.date { bits.push(format!("date={d}")); }
                if let Some(g) = &h.gps_place { bits.push(format!("place={g}")); }
                bits.join(", ")
            })
            .unwrap_or_default();
        let sys = "You write concise photo captions. Output strict JSON \
                   {\"caption\":\"...\",\"alt_text\":\"...\"}. \
                   Caption ≤ 80 chars. Alt-text ≤ 120 chars, screen-reader friendly.";
        let user = format!("Labels: {label_line}\nMeta: {exif_line}");
        let prompt = vec![
            ChatMessage { role: "system".into(), content: sys.into() },
            ChatMessage { role: "user".into(), content: user },
        ];
        let body: String = self.backend.stream(&prompt).collect();
        match serde_json::from_str::<CaptionResult>(&body) {
            Ok(r) => Ok(r),
            Err(_) => Ok(template_fallback(req)),
        }
    }
}

fn template_fallback(req: &CaptionRequest) -> CaptionResult {
    let label = req
        .clip_labels
        .first()
        .map(|s| s.label.clone())
        .unwrap_or_else(|| "photo".into());
    let place = req
        .exif_hint
        .as_ref()
        .and_then(|h| h.gps_place.clone())
        .unwrap_or_default();
    let date = req
        .exif_hint
        .as_ref()
        .and_then(|h| h.date.clone())
        .unwrap_or_default();
    let caption = match (place.is_empty(), date.is_empty()) {
        (true, true) => label.clone(),
        (false, true) => format!("{label} in {place}"),
        (true, false) => format!("{label} on {date}"),
        (false, false) => format!("{label} in {place}, {date}"),
    };
    CaptionResult {
        alt_text: format!("Photograph showing {label}."),
        caption,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::StubBackend;
    use std::sync::Mutex;
    use tulipix_core::caps::{load_from_toml, set_current_tier, Tier};

    static SERIAL: Mutex<()> = Mutex::new(());

    const CAPS: &str = r#"
[tiers]
local_basic = []
local_pro   = ["photos.ai.captions"]
admin       = ["*"]
"#;

    fn req() -> CaptionRequest {
        CaptionRequest {
            photo_id: 1,
            abs_path: "/x/a.jpg".into(),
            clip_labels: vec![
                ScoredLabel { label: "sunset over mountains".into(), score: 0.81 },
                ScoredLabel { label: "lake".into(), score: 0.54 },
            ],
            exif_hint: Some(ExifHint {
                camera: Some("Pixel 8".into()),
                date: Some("2025-09-12".into()),
                gps_place: Some("Bend, OR".into()),
            }),
        }
    }

    #[test]
    fn denied_when_cap_off() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalBasic);
        let g = LlmCaptionGenerator::new(StubBackend);
        assert!(g.generate(&req()).is_err());
    }

    #[test]
    fn fallback_when_llm_emits_non_json() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalPro);
        let g = LlmCaptionGenerator::new(StubBackend);
        let out = g.generate(&req()).unwrap();
        // StubBackend emits "(stub LLM response)" — not JSON — falls back.
        assert!(out.caption.contains("sunset over mountains"));
        assert!(out.caption.contains("Bend, OR"));
        assert!(out.alt_text.starts_with("Photograph"));
    }
}
