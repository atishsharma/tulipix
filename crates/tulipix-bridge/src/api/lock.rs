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
    /// Settings › Security › Unlock with a passkey, and something actually
    /// set up to unlock with. Both, or the lock screen offers nothing it
    /// cannot do.
    pub passkey: bool,
    /// What the passkey button should say: "Use your fingerprint" or
    /// "Touch your security key". Empty when `passkey` is false.
    pub passkey_label: String,
    /// The profiles to choose between, when profiles are on and there is more
    /// than one. Empty otherwise, and then the lock screen shows no picker.
    pub profiles: Vec<LockProfile>,
}

/// One profile on the lock screen's picker.
pub struct LockProfile {
    pub slug: String,
    pub name: String,
    /// The one this window is already using.
    pub active: bool,
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
        passkey: passkey_label(&s).is_some(),
        passkey_label: passkey_label(&s).unwrap_or_default(),
        profiles: lock_profiles(),
    }
}

/// The picker's rows, or nothing at all. Nothing is the ordinary case: one
/// profile, or the switch off.
fn lock_profiles() -> Vec<LockProfile> {
    if !tulipix_core::multi_user::needs_picker() {
        return Vec::new();
    }
    tulipix_core::multi_user::list()
        .into_iter()
        .map(|p| LockProfile { slug: p.slug, name: p.display_name, active: p.active })
        .collect()
}

/// Open another profile from the lock screen. Tulipix starts again as that
/// profile: every path in the app is fixed when the process starts, and this
/// one has its databases open.
///
/// Returns false when the name is not a profile — the slug comes from the
/// page, and this is about to become an environment variable.
pub fn lock_switch_profile(slug: String) -> bool {
    if !tulipix_core::multi_user::list().iter().any(|p| p.slug == slug) {
        return false;
    }
    crate::api::settings::relaunch_as_profile(&slug);
    true
}

/// What the lock screen's passkey button should say, or `None` when there is
/// nothing to offer: the switch is off, or nothing is enrolled.
fn passkey_label(s: &tulipix_core::settings::Settings) -> Option<String> {
    use tulipix_core::sec::passkey::{self, PasskeyKind};
    if !s.flag("passkey", false) {
        return None;
    }
    let have_finger = passkey::has(PasskeyKind::Fingerprint);
    let have_key = passkey::has(PasskeyKind::SecurityKey);
    match (have_finger, have_key) {
        (true, true) => Some("Fingerprint or security key".into()),
        (true, false) => Some("Use your fingerprint".into()),
        (false, true) => Some("Touch your security key".into()),
        (false, false) => None,
    }
}

/// Unlock with whatever is enrolled. Fingerprint first when both are, because
/// it asks nothing of the user but a finger they have already put down.
///
/// Blocking: `fprintd-verify` waits for a finger and `fido2-assert` waits for
/// a touch, and neither has an async form. It runs on a blocking thread so the
/// lock screen keeps drawing while it waits.
pub async fn lock_verify_passkey() -> bool {
    use tulipix_core::sec::passkey::{self, PasskeyKind};
    if passkey_label(&load()).is_none() {
        return false;
    }
    tokio::task::spawn_blocking(|| {
        if passkey::has(PasskeyKind::Fingerprint) && passkey::fingerprint::available() {
            match passkey::fingerprint::verify() {
                Ok(true) => return true,
                Ok(false) => {}
                Err(e) => tracing::warn!(error = %e, "fingerprint unlock"),
            }
        }
        for cred in passkey::list() {
            if cred.kind != PasskeyKind::SecurityKey {
                continue;
            }
            match passkey::security_key::verify(&cred) {
                Ok(true) => return true,
                Ok(false) => {}
                Err(e) => tracing::warn!(error = %e, "security key unlock"),
            }
        }
        false
    })
    .await
    .unwrap_or(false)
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
