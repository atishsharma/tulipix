//! Managed profiles within a single account — Netflix-style sub-profiles
//! (Kid, Family, Mum) that share storage but expose a filtered library and
//! optional per-profile lock. Distinct from `crate::multi_user`, which gives
//! each profile its own directory tree for offline / local-mode households.
//!
//! Cap-gated `sync.profiles` — only available in Account mode tiers.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LockKind {
    None,
    Pin,
    /// Biometric (TouchID/WindowsHello/PolKit) — actual challenge happens in
    /// `tulipix-platform::biometric`; this enum just records the user's pick.
    Biometric,
}

/// Library-filter rule — a profile sees an item iff every rule passes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryFilter {
    /// Sections the profile can access (Photos/Videos/Music/Books/Cloud/Tools).
    pub sections: BTreeSet<String>,
    /// TMDB rating ceiling — items above are hidden. `None` = no limit.
    pub max_rating: Option<String>,
    /// Tags that, if present on an item, hide it from this profile.
    pub blocked_tags: BTreeSet<String>,
    /// Libraries (by id) explicitly carved out.
    pub allowed_library_ids: Option<BTreeSet<i64>>,
}

impl Default for LibraryFilter {
    fn default() -> Self {
        Self {
            sections: ["photos", "videos", "music", "books"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            max_rating: None,
            blocked_tags: BTreeSet::new(),
            allowed_library_ids: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManagedProfile {
    pub id: String,
    pub display_name: String,
    pub avatar_emoji: String,
    /// One-way SHA-256 of `salt + pin` so we never store the raw PIN.
    pub lock: LockKind,
    pub pin_hash: Option<String>,
    pub salt: Option<String>,
    pub filter: LibraryFilter,
}

impl ManagedProfile {
    pub fn unlocked(&self) -> bool {
        matches!(self.lock, LockKind::None)
    }

    /// Set a PIN. Empty PIN clears the lock.
    pub fn set_pin(&mut self, pin: &str) {
        if pin.is_empty() {
            self.lock = LockKind::None;
            self.pin_hash = None;
            self.salt = None;
            return;
        }
        let salt = random_salt();
        let hash = hash_pin(&salt, pin);
        self.lock = LockKind::Pin;
        self.salt = Some(salt);
        self.pin_hash = Some(hash);
    }

    pub fn verify_pin(&self, pin: &str) -> bool {
        let (Some(stored), Some(salt)) = (&self.pin_hash, &self.salt) else {
            return matches!(self.lock, LockKind::None);
        };
        let candidate = hash_pin(salt, pin);
        constant_time_eq(stored.as_bytes(), candidate.as_bytes())
    }
}

fn random_salt() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let mut nano = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let mut buf = [0u8; 16];
    for b in &mut buf {
        nano = nano.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *b = (nano >> 64) as u8 ^ (nano as u8);
    }
    hex(&buf)
}

fn hash_pin(salt: &str, pin: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(salt.as_bytes());
    h.update(b":");
    h.update(pin.as_bytes());
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
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

#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProfilesFile {
    pub profiles: Vec<ManagedProfile>,
    pub default_id: Option<String>,
}

impl ProfilesFile {
    pub fn upsert(&mut self, p: ManagedProfile) {
        match self.profiles.iter_mut().find(|x| x.id == p.id) {
            Some(slot) => *slot = p,
            None => self.profiles.push(p),
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.profiles.retain(|p| p.id != id);
        if self.default_id.as_deref() == Some(id) {
            self.default_id = self.profiles.first().map(|p| p.id.clone());
        }
    }

    pub fn by_id(&self, id: &str) -> Option<&ManagedProfile> {
        self.profiles.iter().find(|p| p.id == id)
    }
}

pub fn load(path: &Path) -> Result<ProfilesFile> {
    if !path.exists() {
        return Ok(ProfilesFile::default());
    }
    let bytes = std::fs::read(path).with_context(|| format!("read {}", path.display()))?;
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

pub fn save(path: &Path, file: &ProfilesFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let bytes = serde_json::to_vec_pretty(file)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fixture(id: &str, name: &str) -> ManagedProfile {
        ManagedProfile {
            id: id.into(),
            display_name: name.into(),
            avatar_emoji: "🙂".into(),
            lock: LockKind::None,
            pin_hash: None,
            salt: None,
            filter: LibraryFilter::default(),
        }
    }

    #[test]
    fn set_pin_and_verify() {
        let mut p = fixture("a", "Mum");
        p.set_pin("4242");
        assert!(matches!(p.lock, LockKind::Pin));
        assert!(p.verify_pin("4242"));
        assert!(!p.verify_pin("0000"));
        assert!(!p.unlocked());
    }

    #[test]
    fn empty_pin_clears_lock() {
        let mut p = fixture("a", "Mum");
        p.set_pin("1234");
        p.set_pin("");
        assert!(matches!(p.lock, LockKind::None));
        assert!(p.unlocked());
        assert!(p.verify_pin(""));
    }

    #[test]
    fn pin_salt_makes_hash_unique() {
        let mut a = fixture("a", "A");
        let mut b = fixture("b", "B");
        a.set_pin("1111");
        b.set_pin("1111");
        assert_ne!(a.pin_hash, b.pin_hash, "different salts → different hashes");
    }

    #[test]
    fn upsert_replaces_by_id() {
        let mut f = ProfilesFile::default();
        f.upsert(fixture("a", "First"));
        f.upsert(fixture("a", "Second"));
        f.upsert(fixture("b", "Other"));
        assert_eq!(f.profiles.len(), 2);
        assert_eq!(f.by_id("a").unwrap().display_name, "Second");
    }

    #[test]
    fn remove_reassigns_default() {
        let mut f = ProfilesFile::default();
        f.upsert(fixture("a", "A"));
        f.upsert(fixture("b", "B"));
        f.default_id = Some("a".into());
        f.remove("a");
        assert_eq!(f.default_id.as_deref(), Some("b"));
    }

    #[test]
    fn load_returns_default_on_missing() {
        let d = tempdir().unwrap();
        let file = load(&d.path().join("missing.json")).unwrap();
        assert!(file.profiles.is_empty());
    }

    #[test]
    fn save_then_load_round_trip() {
        let d = tempdir().unwrap();
        let path = d.path().join("profiles.json");
        let mut file = ProfilesFile::default();
        let mut p = fixture("k", "Kid");
        p.filter.sections = ["videos", "books"].iter().map(|s| s.to_string()).collect();
        file.upsert(p);
        save(&path, &file).unwrap();
        let again = load(&path).unwrap();
        assert_eq!(again, file);
    }

    #[test]
    fn default_filter_excludes_cloud_and_tools() {
        let f = LibraryFilter::default();
        assert!(f.sections.contains("videos"));
        assert!(!f.sections.contains("cloud"));
        assert!(!f.sections.contains("tools"));
    }
}
