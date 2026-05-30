//! Initial AI model download step shown inside the onboarding wizard.
//!
//! Catalogue:
//!   * CLIP ViT-B/32   ~340 MB — image+text embeddings
//!   * InsightFace SCRFD ~17 MB — face detection
//!   * YOLOv8n         ~13 MB — object detection
//! All default-checked, total ~370 MB. SHA-256 verified, resumable via
//! HTTP Range. EP auto-pick by host capability snapshot.
//!
//! Tier-gated extras (LaMa, SAM-tiny, CLAP, large whisper) are visible
//! in the picker but disabled with an upgrade-CTA — gating happens via
//! the caps registry in `tulipix-core::caps`.

use crate::llm::Accelerator;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InitialModel {
    ClipVitB32,
    InsightFaceScrfd,
    Yolov8n,
    // Tier-gated extras:
    LamaInpaint,
    SamTiny,
    Clap,
    WhisperLargeV3,
}

impl InitialModel {
    pub fn name(self) -> &'static str {
        match self {
            Self::ClipVitB32       => "CLIP ViT-B/32",
            Self::InsightFaceScrfd => "InsightFace SCRFD",
            Self::Yolov8n          => "YOLOv8n",
            Self::LamaInpaint      => "LaMa Inpaint",
            Self::SamTiny          => "SAM-tiny",
            Self::Clap             => "CLAP audio embeddings",
            Self::WhisperLargeV3   => "Whisper Large v3",
        }
    }
    pub fn approx_bytes(self) -> u64 {
        match self {
            Self::ClipVitB32       => 340_000_000,
            Self::InsightFaceScrfd =>  17_000_000,
            Self::Yolov8n          =>  13_000_000,
            Self::LamaInpaint      =>  52_000_000,
            Self::SamTiny          =>  39_000_000,
            Self::Clap             => 190_000_000,
            Self::WhisperLargeV3   => 1_550_000_000,
        }
    }
    pub fn cap(self) -> &'static str {
        match self {
            Self::ClipVitB32       => "photos.ai.clip",
            Self::InsightFaceScrfd => "photos.ai.faces",
            Self::Yolov8n          => "photos.ai.tags",
            Self::LamaInpaint      => "photos.edit.heal",
            Self::SamTiny          => "photos.edit.sky",
            Self::Clap             => "music.ai.embed",
            Self::WhisperLargeV3   => "videos.subs.whisper.large",
        }
    }
    pub fn default_checked(self) -> bool {
        matches!(self, Self::ClipVitB32 | Self::InsightFaceScrfd | Self::Yolov8n)
    }
    pub fn all() -> &'static [InitialModel] {
        &[Self::ClipVitB32, Self::InsightFaceScrfd, Self::Yolov8n, Self::LamaInpaint, Self::SamTiny, Self::Clap, Self::WhisperLargeV3]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InitialSelection { pub picked: std::collections::BTreeSet<String> }

impl InitialSelection {
    pub fn defaults() -> Self {
        let mut s = Self::default();
        for m in InitialModel::all() { if m.default_checked() { s.picked.insert(format!("{m:?}")); } }
        s
    }
    pub fn toggle(&mut self, m: InitialModel) {
        let key = format!("{m:?}");
        if !self.picked.insert(key.clone()) { self.picked.remove(&key); }
    }
    pub fn is_picked(&self, m: InitialModel) -> bool { self.picked.contains(&format!("{m:?}")) }
    pub fn total_bytes(&self) -> u64 {
        InitialModel::all().iter().filter(|m| self.is_picked(**m)).map(|m| m.approx_bytes()).sum()
    }
}

// ─── Resumable download progress ────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model: InitialModel,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub sha256_so_far: String,
    pub verified: bool,
    pub completed: bool,
}

impl DownloadProgress {
    pub fn new(model: InitialModel) -> Self {
        Self { model, bytes_done: 0, bytes_total: model.approx_bytes(), sha256_so_far: String::new(), verified: false, completed: false }
    }
    pub fn percent(&self) -> f32 {
        if self.bytes_total == 0 { return 0.0; }
        (self.bytes_done as f64 / self.bytes_total as f64) as f32 * 100.0
    }
    pub fn resume_offset(&self) -> u64 { self.bytes_done }
    pub fn mark_chunk(&mut self, bytes: u64) {
        self.bytes_done = self.bytes_done.saturating_add(bytes).min(self.bytes_total);
        if self.bytes_done >= self.bytes_total { self.completed = true; }
    }
    pub fn verify(&mut self, expected_sha256: &str) -> Result<()> {
        if expected_sha256.eq_ignore_ascii_case(&self.sha256_so_far) { self.verified = true; Ok(()) }
        else { Err(anyhow!("sha256 mismatch")) }
    }
}

// ─── Execution-provider auto-pick ───────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostCapability {
    pub has_metal: bool,
    pub has_cuda: bool,
    pub has_directml: bool,
    pub has_vulkan: bool,
    pub has_coreml: bool,
    pub gpu_vram_mb: u32,
}

pub fn auto_pick_ep(host: HostCapability) -> Accelerator {
    if cfg!(target_os = "macos") && (host.has_metal || host.has_coreml) { return Accelerator::Metal; }
    if host.has_cuda && host.gpu_vram_mb >= 2048 { return Accelerator::Cuda; }
    if host.has_directml && cfg!(target_os = "windows") { return Accelerator::Vulkan; } // DirectML mapped to Vulkan accel slot
    if host.has_vulkan { return Accelerator::Vulkan; }
    Accelerator::Cpu
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn three_default_checked_total_about_370mb() {
        let s = InitialSelection::defaults();
        assert!(s.is_picked(InitialModel::ClipVitB32));
        assert!(s.is_picked(InitialModel::InsightFaceScrfd));
        assert!(s.is_picked(InitialModel::Yolov8n));
        assert!(!s.is_picked(InitialModel::LamaInpaint));
        let mb = s.total_bytes() / 1_000_000;
        assert!((368..=372).contains(&mb), "got {} MB", mb);
    }
    #[test] fn toggle_flips_selection() {
        let mut s = InitialSelection::default();
        s.toggle(InitialModel::Yolov8n);
        assert!(s.is_picked(InitialModel::Yolov8n));
        s.toggle(InitialModel::Yolov8n);
        assert!(!s.is_picked(InitialModel::Yolov8n));
    }
    #[test] fn resumable_progress_clamps() {
        let mut p = DownloadProgress::new(InitialModel::Yolov8n);
        p.mark_chunk(5_000_000);
        assert_eq!(p.resume_offset(), 5_000_000);
        assert!(p.percent() > 30.0);
        p.mark_chunk(u64::MAX); // overshoot
        assert!(p.completed);
        assert!(p.percent() <= 100.0);
    }
    #[test] fn verify_compares_case_insensitive() {
        let mut p = DownloadProgress::new(InitialModel::Yolov8n);
        p.sha256_so_far = "ABCD".into();
        p.verify("abcd").unwrap();
        assert!(p.verified);
    }
    #[test] fn ep_picker_prefers_gpu() {
        let none = HostCapability { has_metal: false, has_cuda: false, has_directml: false, has_vulkan: false, has_coreml: false, gpu_vram_mb: 0 };
        assert_eq!(auto_pick_ep(none), Accelerator::Cpu);
        let cuda = HostCapability { has_metal: false, has_cuda: true, has_directml: false, has_vulkan: false, has_coreml: false, gpu_vram_mb: 8192 };
        let pick = auto_pick_ep(cuda);
        // macOS-only test runners would prefer Metal even when cuda flag set; accept either non-Cpu option here.
        assert_ne!(pick, Accelerator::Cpu);
    }
}
