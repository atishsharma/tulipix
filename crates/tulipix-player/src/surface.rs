//! Render surface that bridges the Slint GPU swapchain to libmpv's
//! `mpv_render_context`.
//!
//! The surface is the contract between the UI layer (which owns the
//! window's Vulkan/Metal swapchain) and the player runtime (which feeds
//! frames into that swapchain). This module exposes the descriptor types,
//! frame-size negotiation, and the lifecycle states the controller has to
//! transition through. The actual FFI lives behind the `embed-mpv` feature
//! flag in a sibling module.

use serde::{Deserialize, Serialize};

use crate::mpv_probe::GpuApi;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceState {
    Idle,
    Initialising,
    Ready,
    Resizing,
    Lost,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SurfaceDescriptor {
    pub api: GpuApi,
    /// Opaque pointer to the underlying GPU object (VkImage on Vulkan,
    /// CAMetalLayer on Metal). Stored as a usize so the struct stays
    /// `Send + Sync`.
    pub native_handle: usize,
    pub framebuffer_w: u32,
    pub framebuffer_h: u32,
    /// HiDPI scale factor reported by the windowing layer.
    pub scale_factor: f32,
}

impl SurfaceDescriptor {
    pub fn new(native_handle: usize, w: u32, h: u32, scale: f32) -> Self {
        Self {
            api: GpuApi::for_target(),
            native_handle,
            framebuffer_w: w,
            framebuffer_h: h,
            scale_factor: scale.max(0.5),
        }
    }

    pub fn aspect(&self) -> f32 {
        if self.framebuffer_h == 0 { return 0.0; }
        self.framebuffer_w as f32 / self.framebuffer_h as f32
    }
}

pub struct Surface {
    descriptor: SurfaceDescriptor,
    state: SurfaceState,
    frames: u64,
}

impl Surface {
    pub fn new(descriptor: SurfaceDescriptor) -> Self {
        Self { descriptor, state: SurfaceState::Idle, frames: 0 }
    }

    pub fn state(&self) -> SurfaceState { self.state }
    pub fn descriptor(&self) -> &SurfaceDescriptor { &self.descriptor }
    pub fn frames(&self) -> u64 { self.frames }

    /// Transition Idle → Initialising → Ready. Idempotent on Ready.
    pub fn initialise(&mut self) -> SurfaceState {
        match self.state {
            SurfaceState::Idle | SurfaceState::Lost => {
                self.state = SurfaceState::Initialising;
                self.state = SurfaceState::Ready;
            }
            _ => {}
        }
        self.state
    }

    pub fn resize(&mut self, w: u32, h: u32) -> SurfaceState {
        if w == 0 || h == 0 { return self.state; }
        self.state = SurfaceState::Resizing;
        self.descriptor.framebuffer_w = w;
        self.descriptor.framebuffer_h = h;
        self.state = SurfaceState::Ready;
        self.state
    }

    /// Called every render tick. Returns false when the surface is not in a
    /// renderable state and the caller should skip the mpv render pass.
    pub fn tick(&mut self) -> bool {
        if self.state == SurfaceState::Ready { self.frames += 1; true } else { false }
    }

    /// Mark the surface lost (e.g. after a Vulkan VK_ERROR_OUT_OF_DATE).
    pub fn invalidate(&mut self) { self.state = SurfaceState::Lost; }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptor_aspect_matches() {
        let d = SurfaceDescriptor::new(0, 1920, 1080, 1.0);
        assert!((d.aspect() - 16.0 / 9.0).abs() < 1e-6);
    }

    #[test]
    fn lifecycle_idle_to_ready() {
        let d = SurfaceDescriptor::new(0, 100, 100, 1.0);
        let mut s = Surface::new(d);
        assert_eq!(s.state(), SurfaceState::Idle);
        assert_eq!(s.initialise(), SurfaceState::Ready);
        assert!(s.tick());
        assert_eq!(s.frames(), 1);
    }

    #[test]
    fn invalidate_then_reinitialise() {
        let mut s = Surface::new(SurfaceDescriptor::new(0, 100, 100, 1.0));
        s.initialise();
        s.invalidate();
        assert_eq!(s.state(), SurfaceState::Lost);
        assert!(!s.tick());
        s.initialise();
        assert_eq!(s.state(), SurfaceState::Ready);
    }

    #[test]
    fn resize_keeps_state_ready() {
        let mut s = Surface::new(SurfaceDescriptor::new(0, 100, 100, 1.0));
        s.initialise();
        s.resize(1920, 1080);
        assert_eq!(s.state(), SurfaceState::Ready);
        assert_eq!(s.descriptor().framebuffer_w, 1920);
    }

    #[test]
    fn zero_resize_is_ignored() {
        let mut s = Surface::new(SurfaceDescriptor::new(0, 100, 100, 1.0));
        s.initialise();
        s.resize(0, 1080);
        assert_eq!(s.descriptor().framebuffer_w, 100);
    }
}
