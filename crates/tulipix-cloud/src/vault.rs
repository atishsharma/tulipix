//! `np.p4.cloud.vault` — Personal Vault (AES-256-GCM, extra PIN/biometric,
//! idle auto-relock).
//!
//! The vault is a second factor over cloud files. This owns the lock-state
//! machine: PIN verification (salted SHA-256, constant-time compare), the
//! idle-relock deadline, and the AES-256-GCM parameter contract (32-byte key,
//! 12-byte nonce) the cipher layer must honor. The cipher itself lives behind
//! a core crypto helper; here we gate access and time out.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const KEY_LEN: usize = 32;   // AES-256
pub const NONCE_LEN: usize = 12; // GCM standard

/// Salted PIN hash (hex). Salt is per-vault, stored alongside.
pub fn hash_pin(pin: &str, salt: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(pin.as_bytes());
    let d = h.finalize();
    d.iter().map(|b| format!("{b:02x}")).collect()
}

/// Constant-time-ish equality for hex digests (avoid early-exit timing leak).
pub fn verify_pin(pin: &str, salt: &[u8], expected_hex: &str) -> bool {
    let got = hash_pin(pin, salt);
    if got.len() != expected_hex.len() { return false; }
    got.bytes().zip(expected_hex.bytes()).fold(0u8, |acc, (a, b)| acc | (a ^ b)) == 0
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Vault {
    pub locked: bool,
    /// Auto-relock after this many idle seconds.
    pub idle_timeout_s: f64,
    /// Monotonic time of last activity while unlocked.
    pub last_activity_s: f64,
    pub biometric_ok: bool,
}

impl Vault {
    pub fn new(idle_timeout_s: f64) -> Self {
        Self { locked: true, idle_timeout_s, last_activity_s: 0.0, biometric_ok: false }
    }

    /// Unlock with a verified PIN (caller verified via [`verify_pin`]).
    pub fn unlock(&mut self, now_s: f64) {
        self.locked = false;
        self.last_activity_s = now_s;
    }

    pub fn lock(&mut self) { self.locked = true; }

    /// Record activity to defer the idle relock.
    pub fn touch(&mut self, now_s: f64) { if !self.locked { self.last_activity_s = now_s; } }

    /// Should the vault auto-relock now?
    pub fn should_relock(&self, now_s: f64) -> bool {
        !self.locked && self.idle_timeout_s > 0.0 && now_s - self.last_activity_s >= self.idle_timeout_s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_hash_verifies() {
        let salt = b"abc123";
        let h = hash_pin("4242", salt);
        assert!(verify_pin("4242", salt, &h));
        assert!(!verify_pin("0000", salt, &h));
        assert!(!verify_pin("4242", b"different", &h));
    }

    #[test]
    fn idle_relock_after_timeout() {
        let mut v = Vault::new(60.0);
        v.unlock(100.0);
        assert!(!v.should_relock(150.0));
        assert!(v.should_relock(160.0)); // 60s idle
        v.touch(155.0);
        assert!(!v.should_relock(160.0)); // activity deferred it
    }

    #[test]
    fn gcm_param_contract() {
        assert_eq!(KEY_LEN, 32);
        assert_eq!(NONCE_LEN, 12);
    }
}
