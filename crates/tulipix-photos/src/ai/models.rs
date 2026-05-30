//! Photos-section model registry — fetches model blobs over HTTPS and
//! verifies them against a SHA-256 pinned in `resources/ai-models.toml` (the
//! shared `tulipix_core::ai_models` manifest).
//!
//! Storage: `<data_dir>/models/<name>-<version>/<name>.onnx`. Verification is
//! mandatory — a hash mismatch deletes the file before returning Err.

use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;
use tulipix_core::ai_models::{Manifest, ModelEntry, SemVer};
use tulipix_core::paths;

#[derive(Debug, Clone)]
pub struct InstalledModel {
    pub name: String,
    pub version: SemVer,
    pub path: PathBuf,
}

pub fn models_root() -> Option<PathBuf> {
    paths::data_dir().map(|d| d.join("models"))
}

pub fn install_dir(entry: &ModelEntry) -> Option<PathBuf> {
    models_root().map(|d| d.join(format!("{}-{}", entry.name, entry.version)))
}

pub fn local_path(entry: &ModelEntry) -> Option<PathBuf> {
    install_dir(entry).map(|d| d.join(format!("{}.onnx", entry.name)))
}

pub fn is_installed(entry: &ModelEntry) -> bool {
    local_path(entry).map(|p| p.exists()).unwrap_or(false)
}

/// Download + verify one model. Returns the local path on success.
pub async fn download(entry: &ModelEntry) -> Result<PathBuf> {
    let dir = install_dir(entry).ok_or_else(|| anyhow!("no data dir"))?;
    let out = local_path(entry).unwrap();
    if out.exists() && verify(&out, &entry.sha256).await.is_ok() {
        return Ok(out);
    }
    tokio::fs::create_dir_all(&dir).await?;
    let tmp = dir.join(format!("{}.part", entry.name));

    let client = reqwest::Client::builder()
        .https_only(true)
        .use_rustls_tls()
        .timeout(std::time::Duration::from_secs(60 * 60))
        .build()?;
    let resp = client.get(&entry.url).send().await.with_context(|| format!("GET {}", entry.url))?;
    if !resp.status().is_success() {
        anyhow::bail!("model fetch returned {}", resp.status());
    }

    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);

    let digest = hex(&hasher.finalize());
    if !digest.eq_ignore_ascii_case(&entry.sha256) {
        let _ = tokio::fs::remove_file(&tmp).await;
        anyhow::bail!("sha256 mismatch — wanted {}, got {}", entry.sha256, digest);
    }
    tokio::fs::rename(&tmp, &out).await?;
    Ok(out)
}

pub async fn verify(path: &Path, expected_sha: &str) -> Result<()> {
    let bytes = tokio::fs::read(path).await?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let got = hex(&h.finalize());
    if got.eq_ignore_ascii_case(expected_sha) { Ok(()) } else {
        Err(anyhow!("sha256 mismatch ({} vs {})", got, expected_sha))
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes { s.push_str(&format!("{:02x}", b)); }
    s
}

/// Walk the manifest and return everything currently installed locally.
pub fn installed(manifest: &Manifest) -> Vec<InstalledModel> {
    let mut out = Vec::new();
    for m in &manifest.models {
        if is_installed(m) {
            out.push(InstalledModel { name: m.name.clone(), version: m.version, path: local_path(m).unwrap() });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tulipix_core::ai_models::{EpCompat, SemVer};

    fn dummy_entry() -> ModelEntry {
        ModelEntry {
            name: "scrfd".into(),
            version: SemVer { major: 0, minor: 1, patch: 0 },
            sha256: "00".into(),
            url: "https://example.invalid/scrfd.onnx".into(),
            size_bytes: 0,
            ep_compat: vec![EpCompat::CpuOnly],
            quant: "fp32".into(),
            min_vram_mb: 0,
            cap: "".into(),
        }
    }

    #[tokio::test]
    async fn verify_rejects_wrong_hash() {
        let tmp = tempfile::tempdir().unwrap();
        let p = tmp.path().join("x");
        std::fs::write(&p, b"hi").unwrap();
        assert!(verify(&p, "deadbeef").await.is_err());
    }

    #[test]
    fn install_paths_compose_from_data_dir() {
        let e = dummy_entry();
        if let Some(dir) = install_dir(&e) {
            assert!(dir.to_string_lossy().contains("scrfd-0.1.0"));
        }
    }
}
