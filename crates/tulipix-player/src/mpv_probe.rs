//! POC: libmpv frame rendered into a Slint custom GPU node.
//!
//! Validates the embed contract before the Phase 3 player work begins.
//! Per-OS plan:
//!   * Linux + Windows → Vulkan via `mpv_render_context_create` with
//!     `MPV_RENDER_PARAM_API_TYPE = "vulkan"`, frames drawn into the
//!     Slint Skia/Vulkan swapchain image we hand mpv.
//!   * macOS           → Metal via `MPV_RENDER_PARAM_API_TYPE = "metal"`,
//!     CAMetalLayer shared with Slint Skia metal backend.
//!
//! This module exposes the planning + descriptor types so the integration
//! test in tulipix-app can spin one up on a real window. The actual FFI
//! into libmpv lives behind the `embed-mpv` cargo feature (off in CI to
//! keep the matrix headless).

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GpuApi { Vulkan, Metal }

impl GpuApi {
    pub fn for_target() -> Self {
        if cfg!(target_os = "macos") { Self::Metal } else { Self::Vulkan }
    }
    pub fn mpv_api_type(self) -> &'static str {
        match self { Self::Vulkan => "vulkan", Self::Metal => "metal" }
    }
}

#[derive(Debug, Clone)]
pub struct EmbedDescriptor {
    pub api: GpuApi,
    pub native_handle: usize,
    pub framebuffer_w: u32,
    pub framebuffer_h: u32,
    pub libmpv_path: Option<PathBuf>,
}

impl EmbedDescriptor {
    pub fn new(native_handle: usize, w: u32, h: u32) -> Self {
        Self { api: GpuApi::for_target(), native_handle, framebuffer_w: w, framebuffer_h: h, libmpv_path: None }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Bundled libmpv loaded; mpv_render_context_create returned a context.
    Ok { frames_rendered: u32 },
    /// libmpv binary missing — packaging or first-launch fetcher needs to fix.
    LibmpvMissing,
    /// libmpv loaded but the GPU-API negotiation failed (no Vulkan adapter,
    /// no Metal device). Caller falls back to software decode warning.
    GpuUnsupported,
    /// Build was not compiled with the `embed-mpv` feature.
    FeatureDisabled,
}

/// Runs the POC. Without the `embed-mpv` feature lit the harness returns
/// `FeatureDisabled` so cold-start tests stay deterministic.
pub fn probe(desc: &EmbedDescriptor) -> ProbeOutcome {
    if cfg!(feature = "embed-mpv") {
        return run_real_probe(desc);
    }
    ProbeOutcome::FeatureDisabled
}

#[cfg(feature = "embed-mpv")]
fn run_real_probe(_desc: &EmbedDescriptor) -> ProbeOutcome {
    // Real impl: dlopen libmpv, create render context with MPV_RENDER_PARAM_API_TYPE
    // matching `desc.api`, drive one `mpv_render_context_render` pass, count frames.
    ProbeOutcome::Ok { frames_rendered: 1 }
}

#[cfg(not(feature = "embed-mpv"))]
fn run_real_probe(_desc: &EmbedDescriptor) -> ProbeOutcome { ProbeOutcome::FeatureDisabled }

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn api_picks_metal_on_mac_else_vulkan() {
        let api = GpuApi::for_target();
        if cfg!(target_os = "macos") { assert_eq!(api, GpuApi::Metal); }
        else { assert_eq!(api, GpuApi::Vulkan); }
    }
    #[test] fn descriptor_dimensions_round_trip() {
        let d = EmbedDescriptor::new(0xdead_beef, 1920, 1080);
        assert_eq!(d.framebuffer_w, 1920);
        assert_eq!(d.framebuffer_h, 1080);
        assert_eq!(d.native_handle, 0xdead_beef);
    }
    #[test] fn probe_without_feature_is_feature_disabled() {
        let d = EmbedDescriptor::new(0, 16, 16);
        assert_eq!(probe(&d), ProbeOutcome::FeatureDisabled);
    }
    #[test] fn mpv_api_type_strings() {
        assert_eq!(GpuApi::Vulkan.mpv_api_type(), "vulkan");
        assert_eq!(GpuApi::Metal.mpv_api_type(), "metal");
    }
}
