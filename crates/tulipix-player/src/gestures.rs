//! Gesture / input mapping for the player.
//!
//! Pure function: input event in → `ControlEvent` out. The controller wires
//! the OS-specific gesture recogniser (NSPanGestureRecognizer on macOS,
//! Slint's scroll events on the cross-platform path) to this module so the
//! key map stays one place, and we keep tests trivial.

use serde::{Deserialize, Serialize};

use crate::controls::ControlEvent;

pub const WHEEL_SECONDS_PER_NOTCH: f64 = 5.0;
pub const TRACKPAD_PIXELS_PER_SECOND: f64 = 8.0;
pub const ARROW_SEEK_SHORT: f64 = 5.0;
pub const ARROW_SEEK_LONG:  f64 = 60.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    Play, Pause, Stop,
    SeekShortBack, SeekShortFwd, SeekLongBack, SeekLongFwd,
    VolUp, VolDown, MuteToggle,
    SpeedUp, SpeedDown, SpeedReset,
    Fullscreen, Pip, ToggleSubs,
}

/// Map an mpv-style keyboard key (already normalised to lower-case ASCII) to
/// a high-level action. Returns `None` if the key isn't bound.
pub fn map_key(key: &str, shift: bool) -> Option<KeyAction> {
    match (key, shift) {
        ("space", _) | ("k", _) => Some(KeyAction::Play),
        ("j", false) => Some(KeyAction::SeekLongBack),
        ("l", false) => Some(KeyAction::SeekLongFwd),
        ("left", false)  => Some(KeyAction::SeekShortBack),
        ("right", false) => Some(KeyAction::SeekShortFwd),
        ("left", true)   => Some(KeyAction::SeekLongBack),
        ("right", true)  => Some(KeyAction::SeekLongFwd),
        ("up", _)        => Some(KeyAction::VolUp),
        ("down", _)      => Some(KeyAction::VolDown),
        ("m", _)         => Some(KeyAction::MuteToggle),
        ("[", _)         => Some(KeyAction::SpeedDown),
        ("]", _)         => Some(KeyAction::SpeedUp),
        ("backspace", _) => Some(KeyAction::SpeedReset),
        ("f", _) | ("escape", _) => Some(KeyAction::Fullscreen),
        ("p", _)         => Some(KeyAction::Pip),
        ("s", _)         => Some(KeyAction::ToggleSubs),
        _ => None,
    }
}

pub fn action_to_event(a: KeyAction, current_subs_enabled: bool) -> ControlEvent {
    use KeyAction::*;
    match a {
        Play | Pause => ControlEvent::PlayPause,
        Stop => ControlEvent::Stop,
        SeekShortBack => ControlEvent::SeekRelative(-ARROW_SEEK_SHORT),
        SeekShortFwd  => ControlEvent::SeekRelative( ARROW_SEEK_SHORT),
        SeekLongBack  => ControlEvent::SeekRelative(-ARROW_SEEK_LONG),
        SeekLongFwd   => ControlEvent::SeekRelative( ARROW_SEEK_LONG),
        VolUp   => ControlEvent::VolumeStep(1),
        VolDown => ControlEvent::VolumeStep(-1),
        MuteToggle => ControlEvent::Mute(true),
        SpeedUp    => ControlEvent::Speed(1.5),
        SpeedDown  => ControlEvent::Speed(0.75),
        SpeedReset => ControlEvent::Speed(1.0),
        Fullscreen => ControlEvent::ToggleFullscreen,
        Pip        => ControlEvent::TogglePip,
        ToggleSubs => ControlEvent::SetSubTrack(if current_subs_enabled { -1 } else { 1 }),
    }
}

/// Vertical scroll wheel notches → seek relative. Positive = scroll up =
/// seek forward (matches macOS natural-scroll convention).
pub fn wheel_to_event(notches: f64) -> ControlEvent {
    ControlEvent::SeekRelative(notches * WHEEL_SECONDS_PER_NOTCH)
}

/// macOS trackpad two-finger horizontal swipe — pan-gesture pixels → seek.
pub fn trackpad_pan_to_event(pixels: f64) -> ControlEvent {
    ControlEvent::SeekRelative(pixels / TRACKPAD_PIXELS_PER_SECOND)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn space_and_k_pause() {
        assert_eq!(map_key("space", false), Some(KeyAction::Play));
        assert_eq!(map_key("k", false), Some(KeyAction::Play));
    }

    #[test]
    fn shift_arrow_is_long_seek() {
        assert_eq!(map_key("left", true), Some(KeyAction::SeekLongBack));
        assert_eq!(map_key("right", true), Some(KeyAction::SeekLongFwd));
    }

    #[test]
    fn mpv_jkl_layout() {
        assert_eq!(map_key("j", false), Some(KeyAction::SeekLongBack));
        assert_eq!(map_key("l", false), Some(KeyAction::SeekLongFwd));
    }

    #[test]
    fn wheel_seeks_correctly() {
        if let ControlEvent::SeekRelative(d) = wheel_to_event(3.0) {
            assert!((d - 15.0).abs() < 1e-9);
        } else { panic!("expected SeekRelative"); }
    }

    #[test]
    fn trackpad_pan_converts() {
        if let ControlEvent::SeekRelative(d) = trackpad_pan_to_event(80.0) {
            assert!((d - 10.0).abs() < 1e-9);
        } else { panic!("expected SeekRelative"); }
    }

    #[test]
    fn unknown_key_returns_none() {
        assert_eq!(map_key("q", false), None);
    }
}
