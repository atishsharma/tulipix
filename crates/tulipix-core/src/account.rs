//! Local-only app modes. There is no account mode — the app is local at
//! heart: no sign-in, no sync backend, full access on this device. The only
//! modes are Local (normal use) and Locked (lockscreen).

use crate::caps::{self, Tier};
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AppMode {
    #[default]
    Local,
    Locked,
}


static MODE: RwLock<AppMode> = RwLock::new(AppMode::Local);

type ModeListener = Box<dyn Fn(AppMode) + Send + Sync>;
static LISTENERS: OnceLock<RwLock<Vec<ModeListener>>> = OnceLock::new();
fn listeners() -> &'static RwLock<Vec<ModeListener>> {
    LISTENERS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn on_mode_changed<F: Fn(AppMode) + Send + Sync + 'static>(f: F) {
    listeners().write().unwrap().push(Box::new(f));
}

pub fn current() -> AppMode { *MODE.read().unwrap() }

fn apply_tier(mode: AppMode) {
    let tier = match mode {
        AppMode::Locked => Tier::Guest,
        AppMode::Local => Tier::LocalPro, // local = full access by default
    };
    caps::set_current_tier(tier);
    caps::reset_nudge_dedup();
}

/// Switching never deletes user files. Returns the previous mode.
pub fn set(next: AppMode) -> AppMode {
    let mut m = MODE.write().unwrap();
    let prev = *m;
    *m = next;
    drop(m);
    tracing::info!(?prev, ?next, "app mode switched");
    apply_tier(next);
    for cb in listeners().read().unwrap().iter() { cb(next); }
    prev
}

pub fn lock() { set(AppMode::Locked); }

/// Unlock returns to Local — the only non-locked mode.
pub fn unlock() -> AppMode {
    set(AppMode::Local);
    AppMode::Local
}

// ── Lock-screen PIN ────────────────────────────────────────────────────────
// A single device PIN gates the lock screen. The hash + salt live in the
// generic settings KV (`lock.pin_hash` / `lock.pin_salt`) so no schema change
// is needed. When no PIN is set, unlocking is a plain click.

const PIN_HASH_KEY: &str = "lock.pin_hash";
const PIN_SALT_KEY: &str = "lock.pin_salt";

fn hash_pin(salt: &str, pin: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(pin.as_bytes());
    let out = h.finalize();
    let mut s = String::with_capacity(out.len() * 2);
    for b in out.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn random_salt() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut nano = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut buf = [0u8; 16];
    for b in &mut buf {
        nano = nano
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *b = (nano >> 64) as u8 ^ (nano as u8);
    }
    let mut s = String::with_capacity(buf.len() * 2);
    for b in buf.iter() {
        s.push_str(&format!("{:02x}", b));
    }
    s
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Is a lock-screen PIN configured?
pub fn has_pin() -> bool {
    crate::settings::Settings::load()
        .map(|s| !s.text(PIN_HASH_KEY).is_empty() && !s.text(PIN_SALT_KEY).is_empty())
        .unwrap_or(false)
}

/// Set (or, with an empty string, clear) the lock-screen PIN. Persisted.
pub fn set_pin(pin: &str) {
    let mut s = crate::settings::Settings::load().unwrap_or_default();
    if pin.is_empty() {
        s.advanced.remove(PIN_HASH_KEY);
        s.advanced.remove(PIN_SALT_KEY);
    } else {
        let salt = random_salt();
        let hash = hash_pin(&salt, pin);
        s.advanced.insert(PIN_HASH_KEY.into(), hash);
        s.advanced.insert(PIN_SALT_KEY.into(), salt);
    }
    if let Err(e) = s.save() {
        tracing::warn!(error = %e, "save settings (pin)");
    }
}

/// Verify a candidate PIN against the stored hash. When no PIN is configured
/// this returns `true` so a plain click always unlocks.
pub fn verify_pin(pin: &str) -> bool {
    let Ok(s) = crate::settings::Settings::load() else { return true; };
    let (stored, salt) = (s.text(PIN_HASH_KEY), s.text(PIN_SALT_KEY));
    if stored.is_empty() || salt.is_empty() {
        return true;
    }
    let candidate = hash_pin(&salt, pin);
    constant_time_eq(stored.as_bytes(), candidate.as_bytes())
}

#[cfg(test)]
mod pin_tests {
    // Note: these exercise only the pure hashing helpers, not the persisted
    // settings round-trip (which needs a real config dir).
    use super::{constant_time_eq, hash_pin};

    #[test]
    fn hash_is_salt_sensitive() {
        assert_ne!(hash_pin("a", "1234"), hash_pin("b", "1234"));
        assert_eq!(hash_pin("a", "1234"), hash_pin("a", "1234"));
        assert_ne!(hash_pin("a", "1234"), hash_pin("a", "4242"));
    }

    #[test]
    fn ct_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
    }
}
