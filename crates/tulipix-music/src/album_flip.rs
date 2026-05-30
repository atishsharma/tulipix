//! `np.p4.music.album-flip` — interactive album-art flip.
//!
//! Click the art → a 3D Y-axis flip reveals synced lyrics on the back; click
//! again to flip home. Works in both the mini-player and the main view. This
//! owns the flip state machine + the per-frame rotation/visible-face math the
//! renderer reads (it just maps a 0..1 progress to degrees).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Face { Art, Lyrics }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct FlipState {
    /// Animation progress 0..1 (0 = fully Art, 1 = fully Lyrics).
    pub progress: f64,
    /// Direction the flip is currently heading.
    pub target: Face,
}

impl Default for FlipState {
    fn default() -> Self { Self { progress: 0.0, target: Face::Art } }
}

impl FlipState {
    /// Toggle which face we're heading toward (called on click / Cmd+L).
    pub fn toggle(&mut self) {
        self.target = match self.target { Face::Art => Face::Lyrics, Face::Lyrics => Face::Art };
    }

    /// Advance the animation by `dp` (already eased upstream), clamping 0..1.
    pub fn advance(&mut self, dp: f64) {
        let goal = match self.target { Face::Art => 0.0, Face::Lyrics => 1.0 };
        if self.progress < goal { self.progress = (self.progress + dp).min(goal); }
        else { self.progress = (self.progress - dp).max(goal); }
    }

    /// Y-rotation in degrees (0..180) for the renderer.
    pub fn rotation_deg(&self) -> f64 { self.progress.clamp(0.0, 1.0) * 180.0 }

    /// The face currently presented to the camera (back half shows lyrics).
    pub fn visible_face(&self) -> Face {
        if self.rotation_deg() >= 90.0 { Face::Lyrics } else { Face::Art }
    }

    pub fn is_settled(&self) -> bool {
        let goal = match self.target { Face::Art => 0.0, Face::Lyrics => 1.0 };
        (self.progress - goal).abs() < 1e-6
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flip_runs_to_lyrics_and_back() {
        let mut s = FlipState::default();
        s.toggle();
        assert_eq!(s.target, Face::Lyrics);
        for _ in 0..20 { s.advance(0.1); }
        assert!(s.is_settled());
        assert_eq!(s.visible_face(), Face::Lyrics);
        assert_eq!(s.rotation_deg(), 180.0);
        s.toggle();
        for _ in 0..20 { s.advance(0.1); }
        assert_eq!(s.visible_face(), Face::Art);
    }

    #[test]
    fn back_half_shows_lyrics() {
        let s = FlipState { progress: 0.5, target: Face::Lyrics };
        assert_eq!(s.rotation_deg(), 90.0);
        assert_eq!(s.visible_face(), Face::Lyrics);
    }
}
