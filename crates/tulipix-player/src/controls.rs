//! Controls overlay state machine.
//!
//! The Slint overlay is a thin view over this state struct: every UI
//! interaction maps to a `ControlEvent`, the state advances, and the
//! controller pushes the equivalent mpv command through the IPC bridge.
//! Keeping the model here (free of any Slint type) lets us unit-test the
//! transitions exhaustively without spinning up a window.

use serde::{Deserialize, Serialize};

pub const MIN_SPEED: f32 = 0.25;
pub const MAX_SPEED: f32 = 4.0;
pub const VOLUME_STEP: f32 = 0.05;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayState { Idle, Playing, Paused, Buffering, Ended }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode { Window, Fullscreen, Pip }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerControls {
    pub state: PlayState,
    pub position_s: f64,
    pub duration_s: f64,
    pub volume: f32,        // 0.0-1.0
    pub muted: bool,
    pub speed: f32,         // 0.25-4.0
    pub audio_track: i32,   // mpv track id, -1 = none/auto
    pub sub_track: i32,
    pub display: DisplayMode,
    pub controls_visible: bool,
    /// Seconds since the last user interaction. Auto-hide timer reads it.
    pub idle_ms: u32,
}

impl Default for PlayerControls {
    fn default() -> Self {
        Self {
            state: PlayState::Idle,
            position_s: 0.0, duration_s: 0.0,
            volume: 1.0, muted: false, speed: 1.0,
            audio_track: -1, sub_track: -1,
            display: DisplayMode::Window,
            controls_visible: true, idle_ms: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ControlEvent {
    PlayPause,
    Stop,
    Seek(f64),
    SeekRelative(f64),
    Volume(f32),
    VolumeStep(i32),
    Mute(bool),
    Speed(f32),
    SetAudioTrack(i32),
    SetSubTrack(i32),
    ToggleFullscreen,
    TogglePip,
    UserActivity,
    Tick(u32),               // ms since last tick
    PositionUpdate(f64),
    DurationUpdate(f64),
    StateUpdate(PlayState),
}

pub const AUTOHIDE_MS: u32 = 2_500;

impl PlayerControls {
    pub fn apply(&mut self, ev: ControlEvent) {
        self.idle_ms = match ev {
            ControlEvent::Tick(dt) => self.idle_ms.saturating_add(dt),
            _ => { self.controls_visible = true; 0 }
        };
        match ev {
            ControlEvent::PlayPause => {
                self.state = match self.state {
                    PlayState::Playing => PlayState::Paused,
                    PlayState::Paused | PlayState::Idle | PlayState::Ended => PlayState::Playing,
                    PlayState::Buffering => PlayState::Buffering,
                };
            }
            ControlEvent::Stop => {
                self.state = PlayState::Idle;
                self.position_s = 0.0;
            }
            ControlEvent::Seek(p) => {
                self.position_s = p.clamp(0.0, self.duration_s.max(0.0));
            }
            ControlEvent::SeekRelative(d) => {
                self.position_s = (self.position_s + d).clamp(0.0, self.duration_s.max(0.0));
            }
            ControlEvent::Volume(v) => { self.volume = v.clamp(0.0, 1.0); self.muted = false; }
            ControlEvent::VolumeStep(steps) => {
                let v = self.volume + steps as f32 * VOLUME_STEP;
                self.volume = v.clamp(0.0, 1.0);
                if steps != 0 { self.muted = false; }
            }
            ControlEvent::Mute(m) => self.muted = m,
            ControlEvent::Speed(s) => self.speed = s.clamp(MIN_SPEED, MAX_SPEED),
            ControlEvent::SetAudioTrack(t) => self.audio_track = t,
            ControlEvent::SetSubTrack(t) => self.sub_track = t,
            ControlEvent::ToggleFullscreen => {
                self.display = if matches!(self.display, DisplayMode::Fullscreen) { DisplayMode::Window } else { DisplayMode::Fullscreen };
            }
            ControlEvent::TogglePip => {
                self.display = if matches!(self.display, DisplayMode::Pip) { DisplayMode::Window } else { DisplayMode::Pip };
            }
            ControlEvent::UserActivity => {} // already handled by idle reset above
            ControlEvent::Tick(_) => {
                if self.idle_ms >= AUTOHIDE_MS && self.state == PlayState::Playing {
                    self.controls_visible = false;
                }
            }
            ControlEvent::PositionUpdate(p) => self.position_s = p.max(0.0),
            ControlEvent::DurationUpdate(d) => self.duration_s = d.max(0.0),
            ControlEvent::StateUpdate(s) => self.state = s,
        }
    }

    pub fn is_playing(&self) -> bool { self.state == PlayState::Playing }
    pub fn progress_fraction(&self) -> f64 {
        if self.duration_s <= 0.0 { 0.0 } else { (self.position_s / self.duration_s).clamp(0.0, 1.0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_pause_toggles() {
        let mut c = PlayerControls::default();
        c.apply(ControlEvent::PlayPause);
        assert!(c.is_playing());
        c.apply(ControlEvent::PlayPause);
        assert_eq!(c.state, PlayState::Paused);
    }

    #[test]
    fn seek_clamps_to_duration() {
        let mut c = PlayerControls { duration_s: 60.0, ..Default::default() };
        c.apply(ControlEvent::Seek(100.0));
        assert_eq!(c.position_s, 60.0);
        c.apply(ControlEvent::SeekRelative(-200.0));
        assert_eq!(c.position_s, 0.0);
    }

    #[test]
    fn volume_step_unmutes() {
        let mut c = PlayerControls { muted: true, volume: 0.5, ..Default::default() };
        c.apply(ControlEvent::VolumeStep(1));
        assert!(!c.muted);
        assert!((c.volume - 0.55).abs() < 1e-6);
    }

    #[test]
    fn speed_clamped_to_range() {
        let mut c = PlayerControls::default();
        c.apply(ControlEvent::Speed(10.0));
        assert_eq!(c.speed, MAX_SPEED);
        c.apply(ControlEvent::Speed(0.0));
        assert_eq!(c.speed, MIN_SPEED);
    }

    #[test]
    fn idle_hides_controls_when_playing() {
        let mut c = PlayerControls::default();
        c.apply(ControlEvent::PlayPause); // Playing
        for _ in 0..(AUTOHIDE_MS / 100 + 1) {
            c.apply(ControlEvent::Tick(100));
        }
        assert!(!c.controls_visible);
        c.apply(ControlEvent::UserActivity);
        assert!(c.controls_visible);
        assert_eq!(c.idle_ms, 0);
    }

    #[test]
    fn fullscreen_pip_toggle() {
        let mut c = PlayerControls::default();
        c.apply(ControlEvent::ToggleFullscreen);
        assert_eq!(c.display, DisplayMode::Fullscreen);
        c.apply(ControlEvent::TogglePip);
        assert_eq!(c.display, DisplayMode::Pip);
        c.apply(ControlEvent::TogglePip);
        assert_eq!(c.display, DisplayMode::Window);
    }

    #[test]
    fn progress_fraction_safe_with_zero_duration() {
        let c = PlayerControls::default();
        assert_eq!(c.progress_fraction(), 0.0);
    }
}
