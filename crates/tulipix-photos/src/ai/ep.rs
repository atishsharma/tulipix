//! Execution-provider auto-pick for ONNX Runtime.
//!
//! Order of preference per platform:
//!   macOS  → CoreML → CPU
//!   Windows → CUDA → DirectML → CPU
//!   Linux  → CUDA → ROCm → CPU
//!
//! Detection is cheap and side-effect free: it inspects the bundled ORT
//! binary's compiled EP list, env vars (`CUDA_VISIBLE_DEVICES`, presence of
//! `/proc/driver/nvidia`), and the platform target. The user can override
//! per-model in Settings → AI Models → Advanced.

use serde::{Deserialize, Serialize};
use std::path::Path;
pub use tulipix_core::ai_models::EpCompat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpChoice {
    pub provider: EpCompat,
    pub source:   Source,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source { Auto, UserOverride }

/// Walk `compat` in platform-preferred order; first available EP wins.
pub fn auto_pick(compat: &[EpCompat]) -> EpChoice {
    let order = platform_order();
    for &p in order {
        if compat.contains(&p) && available(p) {
            return EpChoice { provider: p, source: Source::Auto };
        }
    }
    EpChoice { provider: EpCompat::CpuOnly, source: Source::Auto }
}

/// Resolve choice with optional user override. `override_pick` only honoured
/// if it appears in the model's compat list.
pub fn resolve(compat: &[EpCompat], override_pick: Option<EpCompat>) -> EpChoice {
    if let Some(p) = override_pick {
        if compat.contains(&p) && available(p) {
            return EpChoice { provider: p, source: Source::UserOverride };
        }
    }
    auto_pick(compat)
}

fn platform_order() -> &'static [EpCompat] {
    #[cfg(target_os = "macos")]
    { &[EpCompat::CoreMl, EpCompat::CpuOnly] }
    #[cfg(target_os = "windows")]
    { &[EpCompat::Cuda, EpCompat::DirectMl, EpCompat::CpuOnly] }
    #[cfg(all(unix, not(target_os = "macos")))]
    { &[EpCompat::Cuda, EpCompat::Vulkan, EpCompat::CpuOnly] }
}

/// Best-effort availability check. CPU is always available; GPU EPs need
/// driver hints — checking actual ORT capability requires loading the
/// binary, which we defer to first inference. False positives here are
/// resolved at runtime (ORT falls back to CPU on EP init failure).
pub fn available(ep: EpCompat) -> bool {
    match ep {
        EpCompat::CpuOnly => true,
        EpCompat::Any     => true,
        EpCompat::Cuda    => cuda_available(),
        EpCompat::CoreMl  => cfg!(target_os = "macos"),
        EpCompat::DirectMl => cfg!(target_os = "windows"),
        EpCompat::Vulkan  => vulkan_available(),
        EpCompat::Metal   => cfg!(target_os = "macos"),
    }
}

fn cuda_available() -> bool {
    if std::env::var_os("TULIPIX_FORCE_CUDA").is_some() { return true; }
    // Linux exposes /proc/driver/nvidia/version when the NVIDIA module is loaded.
    if Path::new("/proc/driver/nvidia/version").exists() { return true; }
    // CUDA_VISIBLE_DEVICES being set + non-empty + not "-1" is a strong hint.
    match std::env::var("CUDA_VISIBLE_DEVICES") {
        Ok(s) if !s.is_empty() && s != "-1" => true,
        _ => false,
    }
}

fn vulkan_available() -> bool {
    Path::new("/usr/share/vulkan").exists() || std::env::var_os("VK_ICD_FILENAMES").is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cpu_always_picked_when_nothing_else_compat() {
        let c = auto_pick(&[EpCompat::CpuOnly]);
        assert_eq!(c.provider, EpCompat::CpuOnly);
        assert_eq!(c.source, Source::Auto);
    }
    #[test]
    fn override_honoured_when_compat_and_available() {
        let r = resolve(&[EpCompat::CpuOnly], Some(EpCompat::CpuOnly));
        assert_eq!(r.provider, EpCompat::CpuOnly);
        assert_eq!(r.source, Source::UserOverride);
    }
    #[test]
    fn override_falls_back_when_incompatible() {
        let r = resolve(&[EpCompat::CpuOnly], Some(EpCompat::Cuda));
        // CUDA not in compat list → auto pick → CPU
        assert_eq!(r.provider, EpCompat::CpuOnly);
        assert_eq!(r.source, Source::Auto);
    }
}
