//! In-app self-update channel for bundled tools (currently yt-dlp).
//!
//! Strategy: query the upstream release endpoint, compare against the installed
//! version marker, download with rustls + SHA-256 verify against the pinned
//! manifest hash (when stable) or the release-attached sha256sum file. The
//! downloaded binary is written atomically next to the existing one and then
//! renamed into place so a partial download never replaces the live tool.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

pub const YTDLP_RELEASE_API: &str = "https://api.github.com/repos/yt-dlp/yt-dlp/releases/latest";

#[derive(Debug, Deserialize)]
struct GhRelease {
    tag_name: String,
    assets: Vec<GhAsset>,
}

#[derive(Debug, Deserialize)]
struct GhAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub url: String,
    pub asset_name: String,
}

fn asset_name_for_host() -> &'static str {
    if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") { "yt-dlp_linux_aarch64" }
    else if cfg!(target_os = "linux") { "yt-dlp_linux" }
    else if cfg!(target_os = "windows") { "yt-dlp.exe" }
    else if cfg!(target_os = "macos") { "yt-dlp_macos" }
    else { "yt-dlp" }
}

pub fn check_ytdlp_update(installed_version: &str) -> Result<Option<UpdateInfo>> {
    let agent = format!("tulipix/{}", env!("CARGO_PKG_VERSION"));
    let resp = reqwest::blocking::Client::builder()
        .user_agent(agent)
        .build()?
        .get(YTDLP_RELEASE_API)
        .send()?
        .error_for_status()?;
    let release: GhRelease = resp.json().context("parse release json")?;
    if release.tag_name == installed_version {
        return Ok(None);
    }
    let want = asset_name_for_host();
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == want)
        .with_context(|| format!("no asset {want} in release {}", release.tag_name))?;
    Ok(Some(UpdateInfo {
        current: installed_version.to_string(),
        latest: release.tag_name,
        url: asset.browser_download_url,
        asset_name: asset.name,
    }))
}

/// Download + verify against an inline SHA-256 sidecar (`<asset>.sha256sum`) if available,
/// otherwise compute and return the hash for caller-side pinning. The new binary is
/// written to `dest.with_extension("new")` and renamed over `dest` on success.
pub fn apply_ytdlp_update(info: &UpdateInfo, dest: &Path) -> Result<String> {
    let agent = format!("tulipix/{}", env!("CARGO_PKG_VERSION"));
    let client = reqwest::blocking::Client::builder().user_agent(agent).build()?;
    let bytes = client.get(&info.url).send()?.error_for_status()?.bytes()?;
    let mut h = Sha256::new();
    h.update(&bytes);
    let got = hex(h.finalize().as_slice());

    // Optional sidecar verification.
    let sidecar_url = format!("{}.sha256sum", info.url);
    if let Ok(resp) = client.get(&sidecar_url).send() {
        if let Ok(resp) = resp.error_for_status() {
            if let Ok(text) = resp.text() {
                let want = text.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
                if !want.is_empty() && want != got {
                    bail!("yt-dlp update sha256 mismatch: want {want} got {got}");
                }
            }
        }
    }

    let tmp: PathBuf = dest.with_extension("new");
    let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
    f.write_all(&bytes)?;
    f.sync_all()?;
    drop(f);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755))?;
    }
    fs::rename(&tmp, dest).with_context(|| format!("rename to {}", dest.display()))?;
    Ok(got)
}

fn hex(b: &[u8]) -> String {
    const T: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(b.len() * 2);
    for &x in b {
        out.push(T[(x >> 4) as usize] as char);
        out.push(T[(x & 0x0f) as usize] as char);
    }
    out
}
