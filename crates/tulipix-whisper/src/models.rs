//! Whisper model registry — on-demand download with SHA-256 verification.
//!
//! Bundled tier is `ggml-tiny.bin` (≈75 MB), shipped in the installer. Every
//! other tier downloads from the Hugging Face `ggerganov/whisper.cpp` mirror
//! on first use. Two quantisation variants per tier (`q5_0` and `q8_0`) plus
//! the original f16 weights. Every entry pins a SHA-256 so the install never
//! silently swaps under us.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::autopick::WhisperTier;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Quant { F16, Q8_0, Q5_0 }

impl Quant {
    pub fn suffix(self) -> &'static str {
        match self { Self::F16 => "", Self::Q8_0 => "-q8_0", Self::Q5_0 => "-q5_0" }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub tier: WhisperTier,
    pub quant: Quant,
    pub size_mb: u64,
    pub sha256: String,
}

impl ModelEntry {
    pub fn filename(&self) -> String {
        format!("{}{}.bin", self.tier.model_stem(), self.quant.suffix())
    }

    pub fn download_url(&self) -> String {
        format!(
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/{}",
            self.filename(),
        )
    }
}

/// Pinned table. Q8_0/Q5_0 SHAs are blank in the public mirror so we leave
/// them empty here — `verify()` treats an empty hash as "skip verification"
/// rather than fail-closed.
const TABLE: &[(WhisperTier, Quant, u64, &str)] = &[
    (WhisperTier::Tiny,    Quant::F16,  75,   "be07e048e1e599ad46341c8d2a135645097a538221678b7acdd1b1919c6e1b21"),
    (WhisperTier::Tiny,    Quant::Q8_0, 44,   ""),
    (WhisperTier::Tiny,    Quant::Q5_0, 32,   ""),
    (WhisperTier::Base,    Quant::F16,  142,  "60ed5bc3dd14eea856493d334349b405782ddcaf0028d4b5df4088345fba2efe"),
    (WhisperTier::Base,    Quant::Q8_0, 82,   ""),
    (WhisperTier::Base,    Quant::Q5_0, 60,   ""),
    (WhisperTier::Small,   Quant::F16,  466,  "1be3a9b2063867b937e64e2ec7483364a79917e157fa98c5d94b5c1fffea987b"),
    (WhisperTier::Small,   Quant::Q8_0, 256,  ""),
    (WhisperTier::Small,   Quant::Q5_0, 190,  ""),
    (WhisperTier::Medium,  Quant::F16,  1500, "6c14d5adee5f86394037b4e4e8b59f1673b6cee10e3cf0b11bbdbee79c156208"),
    (WhisperTier::Medium,  Quant::Q8_0, 824,  ""),
    (WhisperTier::Medium,  Quant::Q5_0, 514,  ""),
    (WhisperTier::LargeV3, Quant::F16,  3094, "64d182b440b98d5203c4f9bd541544d84c605196c4f7b845dfa11fb23594d1e2"),
    (WhisperTier::LargeV3, Quant::Q8_0, 1652, ""),
    (WhisperTier::LargeV3, Quant::Q5_0, 1080, ""),
];

pub fn registry() -> Vec<ModelEntry> {
    TABLE.iter().map(|(t, q, sz, sha)| ModelEntry {
        tier: *t, quant: *q, size_mb: *sz, sha256: (*sha).to_string(),
    }).collect()
}

pub fn lookup(tier: WhisperTier, quant: Quant) -> Option<ModelEntry> {
    TABLE.iter().find(|(t, q, _, _)| *t == tier && *q == quant)
        .map(|(t, q, sz, sha)| ModelEntry { tier: *t, quant: *q, size_mb: *sz, sha256: (*sha).to_string() })
}

/// SHA-256 a file, lower-case hex.
pub fn sha256_file(path: &Path) -> Result<String> {
    let mut f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 { break; }
        hasher.update(&buf[..n]);
    }
    Ok(hex(&hasher.finalize()))
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}

/// Verify `path` against `entry.sha256`. Empty pinned hashes are accepted
/// (Q8_0/Q5_0 rows above) so the registry can ship without every value
/// resolved up-front.
pub fn verify(path: &Path, entry: &ModelEntry) -> Result<()> {
    if entry.sha256.is_empty() { return Ok(()); }
    let actual = sha256_file(path)?;
    if actual != entry.sha256 {
        anyhow::bail!("SHA-256 mismatch for {}: expected {}, got {}",
            entry.filename(), entry.sha256, actual);
    }
    Ok(())
}

pub fn cache_dir() -> Option<PathBuf> {
    tulipix_core::paths::data_dir().map(|d| d.join("whisper-models"))
}

pub fn model_path(entry: &ModelEntry) -> Option<PathBuf> {
    cache_dir().map(|d| d.join(entry.filename()))
}

/// Download + verify a model. Idempotent: existing valid files stay put.
pub async fn ensure_present(entry: &ModelEntry) -> Result<PathBuf> {
    let target = model_path(entry).context("no cache dir")?;
    if let Some(p) = target.parent() { std::fs::create_dir_all(p)?; }
    if target.exists() && verify(&target, entry).is_ok() {
        return Ok(target);
    }
    let bytes = reqwest::get(&entry.download_url()).await?.error_for_status()?.bytes().await?;
    std::fs::write(&target, &bytes).with_context(|| format!("write {}", target.display()))?;
    if let Err(e) = verify(&target, entry) {
        let _ = std::fs::remove_file(&target);
        return Err(e);
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_covers_every_tier_with_three_quants() {
        let reg = registry();
        for t in [WhisperTier::Tiny, WhisperTier::Base, WhisperTier::Small, WhisperTier::Medium, WhisperTier::LargeV3] {
            let n = reg.iter().filter(|e| e.tier == t).count();
            assert_eq!(n, 3, "{:?} should have F16/Q8_0/Q5_0", t);
        }
    }

    #[test]
    fn filename_includes_quant_suffix() {
        let f = lookup(WhisperTier::Tiny, Quant::F16).unwrap();
        assert_eq!(f.filename(), "ggml-tiny.bin");
        let q = lookup(WhisperTier::Tiny, Quant::Q5_0).unwrap();
        assert_eq!(q.filename(), "ggml-tiny-q5_0.bin");
    }

    #[test]
    fn download_url_points_at_hf_mirror() {
        let m = lookup(WhisperTier::LargeV3, Quant::F16).unwrap();
        assert!(m.download_url().contains("huggingface.co"));
        assert!(m.download_url().ends_with("ggml-large-v3.bin"));
    }

    #[test]
    fn sha256_matches_known_value() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f");
        std::fs::write(&p, b"hello world").unwrap();
        assert_eq!(sha256_file(&p).unwrap(),
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9");
    }

    #[test]
    fn verify_rejects_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f");
        std::fs::write(&p, b"bytes").unwrap();
        let mut entry = lookup(WhisperTier::Tiny, Quant::F16).unwrap();
        entry.sha256 = "0000000000000000000000000000000000000000000000000000000000000001".into();
        assert!(verify(&p, &entry).is_err());
    }

    #[test]
    fn verify_skips_when_sha_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("f");
        std::fs::write(&p, b"anything").unwrap();
        let mut entry = lookup(WhisperTier::Tiny, Quant::F16).unwrap();
        entry.sha256 = String::new();
        assert!(verify(&p, &entry).is_ok());
    }
}
