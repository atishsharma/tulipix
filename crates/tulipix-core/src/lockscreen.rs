//! Lockscreen state machine. Three OS-native biometric providers, all
//! abstracted behind `BiometricProvider`:
//!   * macOS  → LocalAuthentication.framework (Touch ID + Apple Watch)
//!   * Win    → WinHello / WindowsBiometric API
//!   * Linux  → libfprint (D-Bus net.reactivated.Fprint)
//!
//! After 3 consecutive biometric failures we fall back to PIN; another
//! 3 PIN failures locks the app to `Tier::Guest` and surfaces an admin
//! reset prompt. The state machine itself is OS-agnostic so unit tests
//! can drive it without a real authenticator.

use anyhow::Result;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockscreenState {
    Locked,
    PromptingBiometric,
    PromptingPin,
    Unlocked,
    Failed,
}

pub const MAX_BIOMETRIC_ATTEMPTS: u8 = 3;
pub const MAX_PIN_ATTEMPTS: u8       = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthOutcome { Ok, Wrong, Unsupported, Cancelled }

pub trait BiometricProvider: Send + Sync {
    fn supported(&self) -> bool;
    fn prompt(&self, reason: &str) -> AuthOutcome;
}

#[derive(Debug)]
pub struct Lockscreen {
    pub state: LockscreenState,
    pub biometric_attempts: u8,
    pub pin_attempts: u8,
    pub pin_salt: Vec<u8>,
    pub pin_hash: Vec<u8>,
}

impl Lockscreen {
    pub fn new(pin: &str, salt: Vec<u8>) -> Self {
        Self {
            state: LockscreenState::Locked,
            biometric_attempts: 0,
            pin_attempts: 0,
            pin_hash: hash_pin(pin, &salt),
            pin_salt: salt,
        }
    }

    pub fn begin_unlock(&mut self, biometric: &dyn BiometricProvider) -> LockscreenState {
        if biometric.supported() && self.biometric_attempts < MAX_BIOMETRIC_ATTEMPTS {
            self.state = LockscreenState::PromptingBiometric;
            match biometric.prompt("Unlock Tulipix") {
                AuthOutcome::Ok        => { self.state = LockscreenState::Unlocked; }
                AuthOutcome::Wrong     => {
                    self.biometric_attempts += 1;
                    self.state = if self.biometric_attempts >= MAX_BIOMETRIC_ATTEMPTS {
                        LockscreenState::PromptingPin
                    } else { LockscreenState::PromptingBiometric };
                }
                AuthOutcome::Cancelled => { self.state = LockscreenState::Locked; }
                AuthOutcome::Unsupported => { self.state = LockscreenState::PromptingPin; }
            }
        } else {
            self.state = LockscreenState::PromptingPin;
        }
        self.state
    }

    pub fn try_pin(&mut self, pin: &str) -> LockscreenState {
        let candidate = hash_pin(pin, &self.pin_salt);
        if ct_eq(&candidate, &self.pin_hash) {
            self.state = LockscreenState::Unlocked;
            self.pin_attempts = 0;
            self.biometric_attempts = 0;
        } else {
            self.pin_attempts += 1;
            self.state = if self.pin_attempts >= MAX_PIN_ATTEMPTS { LockscreenState::Failed } else { LockscreenState::PromptingPin };
        }
        self.state
    }
}

fn hash_pin(pin: &str, salt: &[u8]) -> Vec<u8> {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(pin.as_bytes());
    h.finalize().to_vec()
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() { return false; }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) { diff |= x ^ y; }
    diff == 0
}

// ─── Stub providers for tests + CI ──────────────────────────────────────

pub struct UnsupportedProvider;
impl BiometricProvider for UnsupportedProvider {
    fn supported(&self) -> bool { false }
    fn prompt(&self, _: &str) -> AuthOutcome { AuthOutcome::Unsupported }
}

pub struct AlwaysOkProvider;
impl BiometricProvider for AlwaysOkProvider {
    fn supported(&self) -> bool { true }
    fn prompt(&self, _: &str) -> AuthOutcome { AuthOutcome::Ok }
}

pub struct AlwaysWrongProvider;
impl BiometricProvider for AlwaysWrongProvider {
    fn supported(&self) -> bool { true }
    fn prompt(&self, _: &str) -> AuthOutcome { AuthOutcome::Wrong }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn salt() -> Vec<u8> { (0..16u8).collect() }

    #[test] fn biometric_ok_unlocks_immediately() {
        let mut ls = Lockscreen::new("1234", salt());
        assert_eq!(ls.begin_unlock(&AlwaysOkProvider), LockscreenState::Unlocked);
    }
    #[test] fn three_biometric_failures_falls_back_to_pin() {
        let mut ls = Lockscreen::new("1234", salt());
        ls.begin_unlock(&AlwaysWrongProvider);
        ls.begin_unlock(&AlwaysWrongProvider);
        let state = ls.begin_unlock(&AlwaysWrongProvider);
        assert_eq!(state, LockscreenState::PromptingPin);
    }
    #[test] fn pin_unlocks_and_resets_counters() {
        let mut ls = Lockscreen::new("1234", salt());
        ls.begin_unlock(&UnsupportedProvider);
        ls.try_pin("9999");
        assert_eq!(ls.pin_attempts, 1);
        ls.try_pin("1234");
        assert_eq!(ls.state, LockscreenState::Unlocked);
        assert_eq!(ls.pin_attempts, 0);
        assert_eq!(ls.biometric_attempts, 0);
    }
    #[test] fn three_pin_failures_locks_out() {
        let mut ls = Lockscreen::new("1234", salt());
        ls.begin_unlock(&UnsupportedProvider);
        ls.try_pin("a"); ls.try_pin("b");
        assert_eq!(ls.try_pin("c"), LockscreenState::Failed);
    }
    #[test] fn ct_eq_handles_length_mismatch() {
        assert!(!ct_eq(b"abc", b"abcd"));
        assert!(ct_eq(b"abc", b"abc"));
    }
    #[test] fn unsupported_biometric_jumps_straight_to_pin() {
        let mut ls = Lockscreen::new("1234", salt());
        let state = ls.begin_unlock(&UnsupportedProvider);
        assert_eq!(state, LockscreenState::PromptingPin);
    }
}
