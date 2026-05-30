//! `np.p4.music.mini-player` — floating always-on-top mini-player.
//!
//! 1:1 square window with proportional resize and an OS always-on-top hint.
//! This owns the geometry math (aspect-locked, clamped) and the window-flag
//! set the platform layer applies; the actual window lives in tulipix-app.

use serde::{Deserialize, Serialize};

pub const MIN_SIDE: f64 = 160.0;
pub const MAX_SIDE: f64 = 600.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MiniPlayer {
    /// Side length in logical px (window is square).
    pub side: f64,
    pub always_on_top: bool,
    pub visible: bool,
}

impl Default for MiniPlayer {
    fn default() -> Self { Self { side: 240.0, always_on_top: true, visible: false } }
}

impl MiniPlayer {
    /// Proportional resize from a drag on either edge: take the larger delta so
    /// the square tracks the cursor, then clamp.
    pub fn resize(&mut self, dw: f64, dh: f64) {
        let delta = if dw.abs() >= dh.abs() { dw } else { dh };
        self.side = (self.side + delta).clamp(MIN_SIDE, MAX_SIDE);
    }

    /// Window size — always square.
    pub fn window_size(&self) -> (f64, f64) { (self.side, self.side) }

    /// Platform window flags to apply (order-stable).
    pub fn window_flags(&self) -> Vec<&'static str> {
        let mut f = vec!["no-maximize", "keep-aspect"];
        if self.always_on_top { f.push("always-on-top"); }
        f
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_keeps_square_and_clamps() {
        let mut m = MiniPlayer::default();
        m.resize(80.0, 10.0); // horizontal drag wins
        assert_eq!(m.window_size(), (320.0, 320.0));
        m.resize(9999.0, 0.0);
        assert_eq!(m.side, MAX_SIDE);
        m.resize(-9999.0, 0.0);
        assert_eq!(m.side, MIN_SIDE);
    }

    #[test]
    fn flags_include_on_top_when_set() {
        let m = MiniPlayer::default();
        assert!(m.window_flags().contains(&"always-on-top"));
        let m2 = MiniPlayer { always_on_top: false, ..Default::default() };
        assert!(!m2.window_flags().contains(&"always-on-top"));
    }
}
