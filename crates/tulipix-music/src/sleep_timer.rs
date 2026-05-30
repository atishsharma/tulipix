//! `np.p4.music.sleep-timer` — fade-out at N minutes; stop-at-end-of-track.
//!
//! Pure timing/volume logic the player polls. The fade ramps volume linearly
//! to zero over the final `fade_s` seconds before the deadline; "end of track"
//! mode defers the stop to the next track boundary.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SleepMode {
    /// Hard stop `after_s` from arming, with a fade tail.
    Timed { after_s: f64, fade_s: f64 },
    /// Stop when the currently-playing track finishes.
    EndOfTrack,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SleepTimer {
    pub mode: SleepMode,
    /// Monotonic seconds when the timer was armed.
    pub armed_at_s: f64,
}

impl SleepTimer {
    /// Volume multiplier (0..1) at monotonic time `now_s`. 1.0 until the fade
    /// window, ramping to 0 at the deadline. Always 1.0 for EndOfTrack.
    pub fn volume_at(&self, now_s: f64) -> f64 {
        match self.mode {
            SleepMode::EndOfTrack => 1.0,
            SleepMode::Timed { after_s, fade_s } => {
                let deadline = self.armed_at_s + after_s;
                let fade_start = deadline - fade_s.max(0.0);
                if now_s <= fade_start { 1.0 }
                else if now_s >= deadline { 0.0 }
                else { ((deadline - now_s) / fade_s).clamp(0.0, 1.0) }
            }
        }
    }

    /// Should playback stop now? (`track_ended` only matters for EndOfTrack.)
    pub fn should_stop(&self, now_s: f64, track_ended: bool) -> bool {
        match self.mode {
            SleepMode::EndOfTrack => track_ended,
            SleepMode::Timed { after_s, .. } => now_s >= self.armed_at_s + after_s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fade_ramps_down() {
        let t = SleepTimer { mode: SleepMode::Timed { after_s: 100.0, fade_s: 10.0 }, armed_at_s: 0.0 };
        assert_eq!(t.volume_at(50.0), 1.0);   // before fade
        assert!((t.volume_at(95.0) - 0.5).abs() < 1e-6); // mid fade
        assert_eq!(t.volume_at(100.0), 0.0);  // deadline
        assert!(!t.should_stop(99.0, false));
        assert!(t.should_stop(100.0, false));
    }

    #[test]
    fn end_of_track_waits_for_boundary() {
        let t = SleepTimer { mode: SleepMode::EndOfTrack, armed_at_s: 0.0 };
        assert_eq!(t.volume_at(9999.0), 1.0);
        assert!(!t.should_stop(9999.0, false));
        assert!(t.should_stop(9999.0, true));
    }
}
