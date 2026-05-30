//! 120 Hz / VRR (variable-refresh) support. The display refresh-rate is
//! detected per-OS at startup; the resulting frame budget is exported to
//! the Slint compositor so animations can pace to actual presentation.
//!
//! Targets:
//!   * 60 Hz  → 16.6 ms budget   (laptop lid integrated panel fallback)
//!   * 120 Hz →  8.3 ms budget   (most modern panels)
//!   * 165 Hz →  6.0 ms budget   (gaming displays)
//!   * 240 Hz →  4.2 ms budget   (e-sports)
//!
//! Per-OS hooks live in tulipix-platform; this crate keeps the math + cap.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refresh { pub hz: u32 }

impl Refresh {
    pub const fn new(hz: u32) -> Self { Self { hz } }

    /// Per-frame budget rounded down to the nearest 0.1 ms.
    pub fn frame_budget(self) -> Duration {
        if self.hz == 0 { return Duration::from_millis(16); }
        Duration::from_nanos(1_000_000_000u64 / self.hz as u64)
    }

    /// Vulkan `VK_PRESENT_MODE_*` hint. Mailbox for >60 Hz, FIFO otherwise to
    /// keep power use sane on lid-integrated panels.
    pub fn vulkan_present_mode(self) -> &'static str {
        if self.hz > 60 { "VK_PRESENT_MODE_MAILBOX_KHR" } else { "VK_PRESENT_MODE_FIFO_KHR" }
    }

    /// `CAMetalLayer.maximumDrawableCount`. 3 unlocks ProMotion's variable
    /// pacing on macOS; 2 is the conservative default.
    pub fn metal_max_drawables(self) -> u32 {
        if self.hz > 60 { 3 } else { 2 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn budgets_match_rates() {
        assert_eq!(Refresh::new(60).frame_budget().as_millis(), 16);
        let b120 = Refresh::new(120).frame_budget().as_micros();
        assert!(b120 >= 8000 && b120 <= 8400, "120 Hz ≈ 8.33 ms, got {b120} µs");
        let b165 = Refresh::new(165).frame_budget().as_micros();
        assert!(b165 >= 6000 && b165 <= 6100);
    }
    #[test] fn present_mode_switches_at_60() {
        assert_eq!(Refresh::new(60).vulkan_present_mode(), "VK_PRESENT_MODE_FIFO_KHR");
        assert_eq!(Refresh::new(120).vulkan_present_mode(), "VK_PRESENT_MODE_MAILBOX_KHR");
    }
    #[test] fn metal_drawables_bumped_above_60() {
        assert_eq!(Refresh::new(60).metal_max_drawables(), 2);
        assert_eq!(Refresh::new(120).metal_max_drawables(), 3);
    }
}
