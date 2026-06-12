//! Hover-to-preview trailer state machine. Lives outside the player so we
//! can drive a tiny mpv child OR a future first-class libmpv embed without
//! changing the UI contract.
//!
//! Rules baked in (matches the plan card):
//!  - Hover for ≥ HOVER_GRACE_MS before we even start the player.
//!  - Audio fades in over FADE_MS once playback starts.
//!  - Mute toggle persists across hovers.
//!  - On hover-out, fade audio out and stop after FADE_MS.
//!  - Re-hover within RESUME_WINDOW_MS resumes from last position.

pub const HOVER_GRACE_MS: i64 = 600;
pub const FADE_MS: i64 = 350;
pub const RESUME_WINDOW_MS: i64 = 3_000;

#[derive(Debug, Clone, PartialEq)]
pub enum PreviewAction {
    Idle,
    StartLoad { item_id: i64, position_ms: i64 },
    FadeIn { ms: i64 },
    FadeOut { ms: i64 },
    Stop,
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[derive(Default)]
enum Phase {
    #[default]
    Idle,
    Hovering { since_ms: i64 },
    Playing,
    Cooling { since_ms: i64 },
}

#[derive(Debug, Clone, Default)]
pub struct PreviewState {
    /// Hovered card (set via `hover_enter`/`hover_leave`).
    pub focused: Option<i64>,
    /// Last persisted playback position per card — fed back on resume.
    pub last_position_ms: std::collections::HashMap<i64, i64>,
    pub muted: bool,
    phase: Phase,
}


impl PreviewState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn hover_enter(&mut self, item_id: i64, now_ms: i64) {
        self.focused = Some(item_id);
        self.phase = match self.phase {
            Phase::Cooling { since_ms } if now_ms - since_ms < RESUME_WINDOW_MS => Phase::Playing,
            _ => Phase::Hovering { since_ms: now_ms },
        };
    }

    pub fn hover_leave(&mut self, now_ms: i64) {
        if matches!(self.phase, Phase::Playing) {
            self.phase = Phase::Cooling { since_ms: now_ms };
        } else {
            self.phase = Phase::Idle;
            self.focused = None;
        }
    }

    /// Advance the state machine by `now_ms` (called per frame). Returns the
    /// player action to take this frame.
    pub fn tick(&mut self, now_ms: i64) -> PreviewAction {
        match self.phase {
            Phase::Idle => PreviewAction::Idle,
            Phase::Hovering { since_ms } => {
                if now_ms - since_ms >= HOVER_GRACE_MS {
                    let id = self.focused.unwrap();
                    let pos = *self.last_position_ms.get(&id).unwrap_or(&0);
                    self.phase = Phase::Playing;
                    return PreviewAction::StartLoad {
                        item_id: id,
                        position_ms: pos,
                    };
                }
                PreviewAction::Idle
            }
            Phase::Playing => {
                if self.muted {
                    PreviewAction::Idle
                } else {
                    PreviewAction::FadeIn { ms: FADE_MS }
                }
            }
            Phase::Cooling { since_ms } => {
                if now_ms - since_ms >= FADE_MS {
                    let id = self.focused.take();
                    if let Some(id) = id {
                        self.last_position_ms.entry(id).or_insert(0);
                    }
                    self.phase = Phase::Idle;
                    PreviewAction::Stop
                } else {
                    PreviewAction::FadeOut { ms: FADE_MS }
                }
            }
        }
    }

    pub fn report_position(&mut self, item_id: i64, position_ms: i64) {
        self.last_position_ms.insert(item_id, position_ms);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hover_grace_delays_playback() {
        let mut s = PreviewState::new();
        s.hover_enter(42, 0);
        assert_eq!(s.tick(100), PreviewAction::Idle);
        assert_eq!(s.tick(500), PreviewAction::Idle);
        let act = s.tick(700);
        assert!(matches!(act, PreviewAction::StartLoad { item_id: 42, position_ms: 0 }));
    }

    #[test]
    fn fade_in_after_start_unless_muted() {
        let mut s = PreviewState::new();
        s.hover_enter(42, 0);
        let _ = s.tick(700);
        assert_eq!(s.tick(800), PreviewAction::FadeIn { ms: FADE_MS });
        s.set_muted(true);
        assert_eq!(s.tick(900), PreviewAction::Idle);
    }

    #[test]
    fn leave_fades_out_then_stops() {
        let mut s = PreviewState::new();
        s.hover_enter(42, 0);
        let _ = s.tick(700);
        s.hover_leave(800);
        assert_eq!(s.tick(900), PreviewAction::FadeOut { ms: FADE_MS });
        assert_eq!(s.tick(1_200), PreviewAction::Stop);
        assert_eq!(s.tick(1_400), PreviewAction::Idle);
    }

    #[test]
    fn quick_re_hover_resumes_playback() {
        let mut s = PreviewState::new();
        s.hover_enter(42, 0);
        let _ = s.tick(700);
        s.report_position(42, 12_500);
        s.hover_leave(800);
        s.hover_enter(42, 1_000); // within resume window
        let act = s.tick(1_001);
        assert_eq!(act, PreviewAction::FadeIn { ms: FADE_MS });
    }

    #[test]
    fn cold_re_hover_restarts_with_persisted_position() {
        let mut s = PreviewState::new();
        s.hover_enter(42, 0);
        let _ = s.tick(700);
        s.report_position(42, 12_500);
        s.hover_leave(800);
        let _ = s.tick(1_200); // exits cooling
        s.hover_enter(42, 10_000);
        match s.tick(10_600) {
            PreviewAction::StartLoad { item_id, position_ms } => {
                assert_eq!(item_id, 42);
                assert_eq!(position_ms, 12_500);
            }
            other => panic!("expected start with resume position, got {other:?}"),
        }
    }
}
