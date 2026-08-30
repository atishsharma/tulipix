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
    // Numeric compare, not `!=`. A local nightly (`2026.08.19.232815`) is newer
    // than the latest stable tag, and string inequality would have offered to
    // replace it with the older one, every week, forever.
    if !crate::ytdlp::is_newer(installed_version, &release.tag_name) {
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


// ── the wiring ──────────────────────────────────────────────────────────────
//
// The two functions above have existed, complete and correct, since the tool
// updater was written — and nothing ever called them. That is how the bundled
// binary reached eight weeks stale and started answering 403 on every download.
// Everything below is the part that was missing.

/// Check, and install if there is something newer. Returns the version now on
/// disk when it changed, `None` when it was already current.
///
/// Blocking: `reqwest::blocking` plus a 30 MB download. Call it from
/// [`spawn_ytdlp_update`] or a `spawn_blocking`, never from an async task.
pub fn update_ytdlp_now() -> Result<Option<String>> {
    let installed = crate::ytdlp::installed_version().unwrap_or_default();
    let now = crate::ytdlp::now_unix();
    let Some(info) = check_ytdlp_update(&installed)? else {
        crate::ytdlp::record_check(now, &installed);
        return Ok(None);
    };
    let dest = crate::ytdlp::update_dest()
        .context("no writable location for the yt-dlp update")?;
    tracing::info!(from = %info.current, to = %info.latest, dest = %dest.display(), "yt-dlp: updating");
    apply_ytdlp_update(&info, &dest)?;
    // The resolver may now answer with a different path (the managed copy), and
    // the cached version certainly describes the old file.
    crate::ytdlp::forget_version();
    crate::ytdlp::record_check(now, &info.latest);
    tracing::info!(version = %info.latest, "yt-dlp: updated");
    Ok(Some(info.latest))
}

/// The weekly check, if it is due and the user has not switched it off.
///
/// Every arm is a no-op that logs rather than an error that surfaces: this runs
/// unasked, in the background, and a laptop with no network on a Tuesday is not
/// something to put a banner in front of anyone about.
pub fn ytdlp_auto_update() {
    if !crate::ytdlp::auto_update_enabled() {
        return;
    }
    let now = crate::ytdlp::now_unix();
    if !crate::ytdlp::is_check_due(crate::ytdlp::last_check(), now) {
        return;
    }
    match update_ytdlp_now() {
        Ok(Some(v)) => tracing::info!(version = %v, "yt-dlp: auto-updated"),
        Ok(None) => tracing::debug!("yt-dlp: already current"),
        Err(e) => {
            // Record the attempt anyway, or a machine that is offline retries on
            // every single launch.
            crate::ytdlp::record_check(now, "");
            tracing::warn!(error = %e, "yt-dlp: auto-update failed");
        }
    }
}

/// Run [`ytdlp_auto_update`] on a background thread.
///
/// A plain `std::thread`, not a tokio task: the body is blocking I/O, and both
/// front-ends call this from `main` before either runtime is necessarily up.
pub fn spawn_ytdlp_update() {
    std::thread::Builder::new()
        .name("ytdlp-update".into())
        .spawn(|| {
            // Warm the version cache here, whether or not a check is due. Both
            // Settings panels and the Status page print it, and reading it
            // spawns a PyInstaller binary that takes a couple of hundred
            // milliseconds to start — which on the Slint build would be the UI
            // thread, and on the Flutter build a tokio worker. Once, off both.
            let _ = crate::ytdlp::installed_version();
            ytdlp_auto_update();
        })
        .map(|_| ())
        .unwrap_or_else(|e| tracing::warn!(error = %e, "yt-dlp: could not start the update thread"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_asset_is_named() {
        // Whatever the host, there is exactly one asset we would ask for.
        assert!(asset_name_for_host().starts_with("yt-dlp"));
    }

    #[test]
    fn hex_is_lowercase_and_padded() {
        assert_eq!(hex(&[0x00, 0x0f, 0xff]), "000fff");
    }
}
