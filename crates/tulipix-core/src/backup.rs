//! Backup & restore of the Tulipix data directory.
//!
//! Bundles settings + DBs + edits + face_thumbs into a single archive.
//! Optional AES-256-GCM passphrase wraps the archive bytes. Schedule lives
//! in settings (off / weekly / monthly). Restore verifies an integrity
//! footer (SHA-256 of the inner archive) and offers rollback to the
//! pre-restore data dir under <data>/.rollback-<ts>/.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const MAGIC: &[u8; 8] = b"TULIPBKP";
pub const FORMAT_VERSION: u32 = 1;

/// Sections included in every backup archive.
pub const INCLUDED_DIRS: &[&str] = &[
    "settings",
    "photos.db",
    "videos.db",
    "music.db",
    "podcasts.db",
    "radio.db",
    "books.db",
    "cloud.db",
    "edits",
    "face_thumbs",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Schedule { Off, Weekly, Monthly }

impl Schedule {
    pub fn interval_secs(self) -> Option<u64> {
        match self {
            Self::Off => None,
            Self::Weekly  => Some(7  * 86_400),
            Self::Monthly => Some(30 * 86_400),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupHeader {
    pub magic: [u8; 8],
    pub version: u32,
    pub created_at: u64,
    pub encrypted: bool,
    pub inner_sha256: [u8; 32],
}

impl BackupHeader {
    pub fn new(encrypted: bool, inner: &[u8], created_at: u64) -> Self {
        let mut h = Sha256::new(); h.update(inner);
        Self { magic: *MAGIC, version: FORMAT_VERSION, created_at, encrypted, inner_sha256: h.finalize().into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyOutcome { Ok, BadMagic, BadVersion, IntegrityMismatch }

/// Verify the inner-archive sha against the header. Caller has already
/// decrypted (if needed) and stripped the framing.
pub fn verify(header: &BackupHeader, inner: &[u8]) -> VerifyOutcome {
    if &header.magic != MAGIC { return VerifyOutcome::BadMagic; }
    if header.version != FORMAT_VERSION { return VerifyOutcome::BadVersion; }
    let mut h = Sha256::new(); h.update(inner);
    let got: [u8; 32] = h.finalize().into();
    if got != header.inner_sha256 { return VerifyOutcome::IntegrityMismatch; }
    VerifyOutcome::Ok
}

/// Path the restore step moves the existing data dir to before unpacking
/// the backup. If the restore later fails, the rollback dir is swapped back.
pub fn rollback_path(data_dir: &std::path::Path, now_secs: u64) -> std::path::PathBuf {
    data_dir.join(format!(".rollback-{now_secs}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn schedule_intervals() {
        assert_eq!(Schedule::Off.interval_secs(), None);
        assert_eq!(Schedule::Weekly.interval_secs(), Some(7 * 86_400));
        assert_eq!(Schedule::Monthly.interval_secs(), Some(30 * 86_400));
    }
    #[test] fn verify_ok_round_trip() {
        let body = b"hello, tulipix!".to_vec();
        let h = BackupHeader::new(false, &body, 1234);
        assert_eq!(verify(&h, &body), VerifyOutcome::Ok);
    }
    #[test] fn verify_detects_tamper() {
        let body = b"abc".to_vec();
        let h = BackupHeader::new(false, &body, 0);
        let tampered = b"abcd";
        assert_eq!(verify(&h, tampered), VerifyOutcome::IntegrityMismatch);
    }
    #[test] fn verify_bad_magic() {
        let mut h = BackupHeader::new(false, b"x", 0);
        h.magic = *b"BADMAGIC";
        assert_eq!(verify(&h, b"x"), VerifyOutcome::BadMagic);
    }
    #[test] fn rollback_path_uses_ts() {
        let p = rollback_path(std::path::Path::new("/data"), 99);
        assert_eq!(p.file_name().unwrap(), ".rollback-99");
    }
}
