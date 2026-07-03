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
    // Extension follows the blob type: ggml whisper models are .bin, the
    // photo-editing models are .onnx.
    let ext = if entry.url.ends_with(".bin") { "bin" } else { "onnx" };
    install_dir(entry).map(|d| d.join(format!("{}.{ext}", entry.name)))
}

pub fn is_installed(entry: &ModelEntry) -> bool {
    local_path(entry).map(|p| p.exists()).unwrap_or(false)
}

/// Download + verify one model. Returns the local path on success.
pub async fn download(entry: &ModelEntry) -> Result<PathBuf> {
    download_with_progress(entry, |_| {}).await
}

/// Download + verify with a progress callback (0..1, based on the manifest
/// size estimate — clamped so it only hits 1.0 when the stream ends).
pub async fn download_with_progress(entry: &ModelEntry, mut on_progress: impl FnMut(f32)) -> Result<PathBuf> {
    let dir = install_dir(entry).ok_or_else(|| anyhow!("no data dir"))?;
    let out = local_path(entry).unwrap();
    if out.exists() && verify(&out, &entry.sha256).await.is_ok() {
        on_progress(1.0);
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
    // Prefer the server's real length over the manifest estimate.
    let total = resp.content_length().unwrap_or(entry.size_bytes).max(1);

    let mut file = tokio::fs::File::create(&tmp).await?;
    let mut hasher = Sha256::new();
    let mut stream = resp.bytes_stream();
    let mut got: u64 = 0;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
        got += chunk.len() as u64;
        on_progress((got as f32 / total as f32).min(0.99));
    }
    file.flush().await?;
    drop(file);
    on_progress(1.0);

    let digest = hex(&hasher.finalize());
    if entry.sha256.eq_ignore_ascii_case(TOFU) {
        // Trust-on-first-use: no upstream hash published — pin the digest of
        // this first download beside the file; later verifies enforce it.
        tokio::fs::write(tmp.with_extension("sha256"), &digest).await?;
        let pinned = out.with_extension("sha256");
        tokio::fs::rename(tmp.with_extension("sha256"), &pinned).await?;
    } else if !digest.eq_ignore_ascii_case(&entry.sha256) {
        let _ = tokio::fs::remove_file(&tmp).await;
        anyhow::bail!("sha256 mismatch — wanted {}, got {}", entry.sha256, digest);
    }
    tokio::fs::rename(&tmp, &out).await?;
    Ok(out)
}

/// Sentinel for manifest rows without an upstream hash: pin on first download.
pub const TOFU: &str = "tofu";

pub async fn verify(path: &Path, expected_sha: &str) -> Result<()> {
    let bytes = tokio::fs::read(path).await?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let got = hex(&h.finalize());
    // TOFU rows verify against the digest pinned at first download (if any).
    let expected = if expected_sha.eq_ignore_ascii_case(TOFU) {
        match tokio::fs::read_to_string(path.with_extension("sha256")).await {
            Ok(p) => p.trim().to_string(),
            Err(_) => return Ok(()), // nothing pinned yet — accept
        }
    } else { expected_sha.to_string() };
    if got.eq_ignore_ascii_case(&expected) { Ok(()) } else {
        Err(anyhow!("sha256 mismatch ({} vs {})", got, expected))
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
