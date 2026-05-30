//! Auto-pick whisper model tier from a hardware probe.
//!
//! Tier map (matches the plan):
//!   * tiny     — < 4 GB RAM
//!   * base     — 4–8 GB RAM
//!   * small    — 8–16 GB RAM, OR any system with GPU ≥ 2 GB VRAM
//!   * medium   — 16+ GB RAM AND GPU ≥ 4 GB VRAM
//!   * large-v3 — 32+ GB RAM AND GPU ≥ 10 GB VRAM
//!
//! Settings UI shows the auto-pick alongside a manual override toggle; if the
//! override is set the probe is ignored entirely.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WhisperTier { Tiny, Base, Small, Medium, LargeV3 }

impl WhisperTier {
    pub fn model_stem(self) -> &'static str {
        match self {
            Self::Tiny    => "ggml-tiny",
            Self::Base    => "ggml-base",
            Self::Small   => "ggml-small",
            Self::Medium  => "ggml-medium",
            Self::LargeV3 => "ggml-large-v3",
        }
    }

    /// Display name for Settings UI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Tiny => "Tiny", Self::Base => "Base", Self::Small => "Small",
            Self::Medium => "Medium", Self::LargeV3 => "Large v3",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareProbe {
    pub ram_mb: u64,
    pub cpu_threads: u32,
    pub gpu_vram_mb: u64,
    pub gpu_present: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TierPick {
    pub tier: WhisperTier,
    pub reason: String,
}

pub fn pick(probe: &HardwareProbe) -> TierPick {
    let ram_gb = probe.ram_mb / 1024;
    let vram_gb = probe.gpu_vram_mb / 1024;

    // High-end: requires both RAM and VRAM headroom.
    if ram_gb >= 32 && probe.gpu_present && vram_gb >= 10 {
        return TierPick { tier: WhisperTier::LargeV3, reason: format!("{}GB RAM + {}GB VRAM ≥ large-v3 threshold", ram_gb, vram_gb) };
    }
    if ram_gb >= 16 && probe.gpu_present && vram_gb >= 4 {
        return TierPick { tier: WhisperTier::Medium, reason: format!("{}GB RAM + {}GB VRAM ≥ medium threshold", ram_gb, vram_gb) };
    }
    // Mid: GPU shortcut OR RAM ≥ 8 (medium/large already returned above when
    // GPU was beefy enough, so anything that reaches here without GPU stays Small).
    if (probe.gpu_present && vram_gb >= 2) || ram_gb >= 8 {
        return TierPick { tier: WhisperTier::Small, reason: format!("{}GB RAM + {}GB VRAM → small", ram_gb, vram_gb) };
    }
    if (4..8).contains(&ram_gb) {
        return TierPick { tier: WhisperTier::Base, reason: format!("{}GB RAM → base", ram_gb) };
    }
    TierPick { tier: WhisperTier::Tiny, reason: format!("{}GB RAM < 4GB → tiny", ram_gb) }
}

/// Manual override wins over the probe-derived pick.
pub fn resolve(probe: &HardwareProbe, override_tier: Option<WhisperTier>) -> TierPick {
    match override_tier {
        Some(t) => TierPick { tier: t, reason: "manual override from Settings".into() },
        None => pick(probe),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(ram_gb: u64, vram_gb: u64) -> HardwareProbe {
        HardwareProbe {
            ram_mb: ram_gb * 1024, cpu_threads: 8,
            gpu_vram_mb: vram_gb * 1024,
            gpu_present: vram_gb > 0,
        }
    }

    #[test]
    fn low_ram_picks_tiny() {
        assert_eq!(pick(&p(2, 0)).tier, WhisperTier::Tiny);
    }

    #[test]
    fn mid_ram_picks_base() {
        assert_eq!(pick(&p(6, 0)).tier, WhisperTier::Base);
    }

    #[test]
    fn ram_band_picks_small_without_gpu() {
        assert_eq!(pick(&p(12, 0)).tier, WhisperTier::Small);
    }

    #[test]
    fn gpu_2gb_shortcuts_to_small_even_on_low_ram() {
        // 4GB RAM normally → base, but 2GB VRAM bumps to small.
        assert_eq!(pick(&p(4, 2)).tier, WhisperTier::Small);
    }

    #[test]
    fn medium_requires_both_thresholds() {
        assert_eq!(pick(&p(32, 0)).tier, WhisperTier::Small); // RAM only, no GPU
        assert_eq!(pick(&p(16, 4)).tier, WhisperTier::Medium);
    }

    #[test]
    fn large_v3_requires_high_ram_and_vram() {
        assert_eq!(pick(&p(32, 10)).tier, WhisperTier::LargeV3);
        assert_eq!(pick(&p(64, 24)).tier, WhisperTier::LargeV3);
        // missing one half drops a tier.
        assert_eq!(pick(&p(32, 8)).tier, WhisperTier::Medium);
        assert_eq!(pick(&p(16, 10)).tier, WhisperTier::Medium);
    }

    #[test]
    fn override_short_circuits_probe() {
        let r = resolve(&p(2, 0), Some(WhisperTier::LargeV3));
        assert_eq!(r.tier, WhisperTier::LargeV3);
        assert!(r.reason.to_lowercase().contains("manual"));
    }

    #[test]
    fn model_stems_match_ggml_naming() {
        assert_eq!(WhisperTier::Tiny.model_stem(), "ggml-tiny");
        assert_eq!(WhisperTier::LargeV3.model_stem(), "ggml-large-v3");
    }
}
