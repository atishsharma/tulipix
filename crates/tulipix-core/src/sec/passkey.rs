//! WebAuthn / FIDO2 passkey unlock. Provides a roaming-passkey path
//! (YubiKey / Titan) and a platform-passkey path (Touch ID / Windows Hello /
//! GNOME passkey daemon).
//!
//! Full webauthn-rs wiring sits behind a feature flag in tulipix-app; this
//! crate ships the credential bookkeeping (storing credential IDs, RP info)
//! so the upper layer just hands us the assertion.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PasskeyKind {
    /// Hardware security key (YubiKey, Titan, Solo, Feitian).
    Roaming,
    /// Built-in platform authenticator (Touch ID, Windows Hello, GNOME passkey).
    Platform,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PasskeyCredential {
    pub id: String,            // base64url-encoded credential id
    pub kind: PasskeyKind,
    pub label: String,         // user-friendly name shown in Security settings
    pub created_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RelyingParty {
    pub id: &'static str,      // "tulipix.local" — never touches the network
    pub name: &'static str,
}

pub const RP: RelyingParty = RelyingParty { id: "tulipix.local", name: "Tulipix" };

#[derive(Debug, Default)]
pub struct PasskeyStore {
    credentials: std::sync::RwLock<Vec<PasskeyCredential>>,
}

impl PasskeyStore {
    pub fn new() -> Self { Self::default() }

    pub fn register(&self, cred: PasskeyCredential) -> Result<()> {
        let mut g = self.credentials.write().unwrap();
        if g.iter().any(|c| c.id == cred.id) {
            anyhow::bail!("credential already enrolled: {}", cred.id);
        }
        g.push(cred);
        Ok(())
    }

    pub fn revoke(&self, id: &str) -> Result<()> {
        let mut g = self.credentials.write().unwrap();
        let before = g.len();
        g.retain(|c| c.id != id);
        if g.len() == before { anyhow::bail!("credential not found: {id}"); }
        Ok(())
    }

    pub fn list(&self) -> Vec<PasskeyCredential> {
        self.credentials.read().unwrap().clone()
    }

    /// Caller hands us the assertion id returned by the platform's
    /// `navigator.credentials.get`. We confirm the credential is enrolled.
    pub fn verify(&self, asserted_id: &str) -> bool {
        self.credentials.read().unwrap().iter().any(|c| c.id == asserted_id)
    }

    /// At least one platform authenticator is enrolled — used by the
    /// onboarding wizard to decide whether to nudge the user to add Touch ID.
    pub fn has_platform_authenticator(&self) -> bool {
        self.credentials.read().unwrap().iter().any(|c| matches!(c.kind, PasskeyKind::Platform))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn cred(id: &str, kind: PasskeyKind) -> PasskeyCredential {
        PasskeyCredential { id: id.into(), kind, label: id.into(), created_at: 0 }
    }
    #[test] fn register_then_verify() {
        let s = PasskeyStore::new();
        s.register(cred("abc", PasskeyKind::Platform)).unwrap();
        assert!(s.verify("abc"));
        assert!(!s.verify("xyz"));
    }
    #[test] fn duplicate_registration_fails() {
        let s = PasskeyStore::new();
        s.register(cred("k", PasskeyKind::Roaming)).unwrap();
        assert!(s.register(cred("k", PasskeyKind::Roaming)).is_err());
    }
    #[test] fn platform_detection() {
        let s = PasskeyStore::new();
        s.register(cred("yubi", PasskeyKind::Roaming)).unwrap();
        assert!(!s.has_platform_authenticator());
        s.register(cred("touch", PasskeyKind::Platform)).unwrap();
        assert!(s.has_platform_authenticator());
    }
    #[test] fn revoke_removes() {
        let s = PasskeyStore::new();
        s.register(cred("a", PasskeyKind::Platform)).unwrap();
        s.revoke("a").unwrap();
        assert!(s.list().is_empty());
        assert!(s.revoke("a").is_err());
    }
}
