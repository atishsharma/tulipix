//! Native floating Picture-in-Picture window.
//!
//! This is *not* the browser PiP API — it's a full second OS window with the
//! same Slint+mpv render context the main window owns, simply detached and
//! pinned. The platform crate is responsible for the actual window creation;
//! this module owns the layout math + persistence.

use serde::{Deserialize, Serialize};

pub const MIN_PIP_WIDTH:  u32 = 192;
pub const MIN_PIP_HEIGHT: u32 = 108;
pub const DEFAULT_MARGIN: u32 = 24;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipCorner { TopLeft, TopRight, BottomLeft, BottomRight, Custom }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipState {
    pub width:  u32,
    pub height: u32,
    pub x:      i32,
    pub y:      i32,
    pub corner: PipCorner,
    pub always_on_top: bool,
    pub visible: bool,
}

impl Default for PipState {
    fn default() -> Self {
        Self {
            width: 480, height: 270,
            x: 0, y: 0,
            corner: PipCorner::BottomRight,
            always_on_top: true,
            visible: false,
        }
    }
}

impl PipState {
    pub fn resize(&mut self, w: u32, h: u32) {
        self.width  = w.max(MIN_PIP_WIDTH);
        self.height = h.max(MIN_PIP_HEIGHT);
    }

    pub fn show(&mut self) { self.visible = true; }
    pub fn hide(&mut self) { self.visible = false; }
    pub fn toggle(&mut self) { self.visible = !self.visible; }

    pub fn snap(&mut self, corner: PipCorner, screen_w: u32, screen_h: u32) {
        self.corner = corner;
        let w = self.width.min(screen_w);
        let h = self.height.min(screen_h);
        let m = DEFAULT_MARGIN as i32;
        let right = screen_w as i32 - w as i32 - m;
        let bottom = screen_h as i32 - h as i32 - m;
        let (x, y) = match corner {
            PipCorner::TopLeft     => (m, m),
            PipCorner::TopRight    => (right, m),
            PipCorner::BottomLeft  => (m, bottom),
            PipCorner::BottomRight => (right, bottom),
            PipCorner::Custom      => (self.x, self.y),
        };
        self.x = x; self.y = y;
    }

    /// Fit `(src_w, src_h)` aspect into the PiP rectangle, preserving aspect.
    pub fn letterbox(&self, src_w: u32, src_h: u32) -> (u32, u32, u32, u32) {
        if src_w == 0 || src_h == 0 { return (0, 0, self.width, self.height); }
        let src_aspect = src_w as f32 / src_h as f32;
        let dst_aspect = self.width as f32 / self.height as f32;
        let (w, h) = if src_aspect > dst_aspect {
            (self.width, (self.width as f32 / src_aspect) as u32)
        } else {
            ((self.height as f32 * src_aspect) as u32, self.height)
        };
        let x = (self.width - w) / 2;
        let y = (self.height - h) / 2;
        (x, y, w, h)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resize_clamps_to_minimums() {
        let mut p = PipState::default();
        p.resize(10, 10);
        assert_eq!(p.width, MIN_PIP_WIDTH);
        assert_eq!(p.height, MIN_PIP_HEIGHT);
    }

    #[test]
    fn snap_bottom_right_places_in_corner() {
        let mut p = PipState::default();
        p.snap(PipCorner::BottomRight, 1920, 1080);
        let m = DEFAULT_MARGIN as i32;
        assert_eq!(p.x, 1920 - p.width as i32 - m);
        assert_eq!(p.y, 1080 - p.height as i32 - m);
    }

    #[test]
    fn letterbox_wide_source_pillarboxes() {
        let mut p = PipState::default();
        p.resize(400, 400);
        let (_x, _y, w, h) = p.letterbox(1920, 1080);
        assert!(w >= h, "wider source produces wider letterbox");
    }

    #[test]
    fn letterbox_safe_with_zero_source() {
        let p = PipState::default();
        let (x, y, w, h) = p.letterbox(0, 0);
        assert_eq!((x, y, w, h), (0, 0, p.width, p.height));
    }

    #[test]
    fn toggle_visibility() {
        let mut p = PipState::default();
        p.toggle();
        assert!(p.visible);
        p.toggle();
        assert!(!p.visible);
    }
}
