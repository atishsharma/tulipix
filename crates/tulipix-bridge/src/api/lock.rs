//! The lock screen: what it reads in one call, and the PIN check.
//!
//! The PIN itself lives in the core (`account::set_pin` / `verify_pin`, salted
//! and hashed) -- the same one the Slint build's lock screen checks -- so this
//! only hands the Flutter overlay what it needs to decide when to lock and
//! what to show once it has. Settings writes every one of these keys through
//! the Security panel's rows.

use crate::api::shell::load;

pub struct LockConfig {
    /// Settings › Security › Lock when idle.
    pub enabled: bool,
    /// Seconds without input before it locks. Ten minutes when unset.
    pub idle_secs: u32,
    pub has_pin: bool,
    /// How many digits the PIN has, so the pad can check it the moment the
    /// last one goes in. 0 when unknown -- a PIN set before this was kept --
    /// and then Enter checks it, and the first unlock records it.
    pub pin_len: u32,
    pub show_music: bool,
    pub show_lyrics: bool,
    /// Play, pause, skip and love work without unlocking.
    pub media_controls: bool,
    pub show_video: bool,
    pub show_glance: bool,
    /// The moving smoke and the slow wallpaper zoom. Off is one still frame.
    pub motion: bool,
    /// The wallpapers folder, or empty for the gradient.
    pub wallpapers: String,
}

pub fn lock_config() -> LockConfig {
    let s = load();
    LockConfig {
        enabled: s.flag("autolock", false),
        idle_secs: idle_secs(&s).min(u32::MAX as u64) as u32,
        has_pin: tulipix_core::account::has_pin(),
        pin_len: s.text(PIN_LEN_KEY).parse().unwrap_or(0),
        show_music: s.flag("lock.show-music", true),
        show_lyrics: s.flag("lock.show-lyrics", true),
        media_controls: s.flag("lock.media-controls", true),
        show_video: s.flag("lock.show-video", true),
        show_glance: s.flag("lock.show-glance", true),
        motion: s.flag("lock.motion", true),
        wallpapers: s.text("lock.wallpapers"),
    }
}

/// Whether `pin` opens the lock. True when no PIN is set, so a plain click
/// always unlocks.
pub fn lock_verify_pin(pin: String) -> bool {
    let ok = tulipix_core::account::verify_pin(&pin);
    if ok && tulipix_core::account::has_pin() && load().text(PIN_LEN_KEY).is_empty() {
        remember_pin_len(pin.len());
    }
    ok
}

const PIN_LEN_KEY: &str = "lock.pin-len";

/// Keep the PIN's length beside its hash; 0 forgets it. The length is all the
/// pad learns -- the digits never leave the core.
pub(crate) fn remember_pin_len(len: usize) {
    let mut s = load();
    if len == 0 {
        s.advanced.remove(PIN_LEN_KEY);
    } else {
        s.advanced.insert(PIN_LEN_KEY.into(), len.to_string());
    }
    crate::api::shell::save(s);
}

/// The idle timeout in seconds, with the ten-minute default the Slint build
/// uses for an unset value.
pub(crate) fn idle_secs(s: &tulipix_core::settings::Settings) -> u64 {
    if s.idle_lock_secs > 0 { s.idle_lock_secs } else { 600 }
}
