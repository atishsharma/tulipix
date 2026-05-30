//! In-place EXIF edit via the bundled exiftool binary.
//!
//! exiftool ships in `resources/bin/<os-arch>/exiftool[.exe]` (the same pin
//! the core thumb pipeline already uses for camera-ingest). All writes go
//! through `-overwrite_original_in_place` so the file stays at the same
//! inode/path; mtime is touched so the populator picks the change up.

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Default)]
pub struct ExifPatch {
    /// Map of EXIF tag name → new value (string form, exiftool decides type).
    pub set: BTreeMap<String, String>,
    /// Tags whose value should be removed (`-TAG=` in exiftool).
    pub clear: Vec<String>,
}

impl ExifPatch {
    pub fn new() -> Self { Self::default() }
    pub fn set<K: Into<String>, V: Into<String>>(mut self, k: K, v: V) -> Self {
        self.set.insert(k.into(), v.into()); self
    }
    pub fn clear<K: Into<String>>(mut self, k: K) -> Self {
        self.clear.push(k.into()); self
    }
}

fn bundled_exiftool() -> PathBuf {
    let exe = std::env::current_exe().ok();
    let dir = exe.as_ref().and_then(|p| p.parent()).and_then(|p| p.parent());
    let os_arch =
        if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") { "linux-aarch64" }
        else if cfg!(target_os = "linux") { "linux-x86_64" }
        else if cfg!(target_os = "windows") { "windows-x86_64" }
        else if cfg!(target_arch = "aarch64") { "macos-aarch64" }
        else { "macos-x86_64" };
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let candidate = dir.map(|d| d.join("resources").join("bin").join(os_arch).join(format!("exiftool{ext}")));
    candidate
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from("exiftool"))
}

/// Apply `patch` to `path`. Returns the binary's stdout for logging.
pub fn apply(path: &Path, patch: &ExifPatch) -> Result<String> {
    if patch.set.is_empty() && patch.clear.is_empty() {
        return Ok(String::new());
    }
    let mut cmd = Command::new(bundled_exiftool());
    cmd.arg("-overwrite_original_in_place");
    cmd.arg("-preserve");
    cmd.arg("-charset");
    cmd.arg("utf8");
    for (k, v) in &patch.set {
        cmd.arg(format!("-{}={}", k, v));
    }
    for k in &patch.clear {
        cmd.arg(format!("-{}=", k));
    }
    cmd.arg(path);
    let out = cmd.output().with_context(|| "spawn exiftool")?;
    if !out.status.success() {
        anyhow::bail!(
            "exiftool exited {} — {}",
            out.status,
            String::from_utf8_lossy(&out.stderr),
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Strip all GPS tags — used by the "remove location" privacy action.
pub fn strip_gps(path: &Path) -> Result<()> {
    let mut p = ExifPatch::new();
    p.clear.extend([
        "GPSLatitude", "GPSLatitudeRef",
        "GPSLongitude", "GPSLongitudeRef",
        "GPSAltitude", "GPSAltitudeRef",
        "GPSTimeStamp", "GPSDateStamp",
        "GPSPosition", "GPSCoordinates",
    ].into_iter().map(String::from));
    apply(path, &p).map(|_| ())
}

/// Convenience: rewrite Artist + Copyright in one call.
pub fn set_attribution(path: &Path, artist: &str, copyright: &str) -> Result<()> {
    let p = ExifPatch::new()
        .set("Artist", artist)
        .set("Copyright", copyright);
    apply(path, &p).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn patch_builder_chains() {
        let p = ExifPatch::new().set("Artist", "Tulip").clear("GPSLatitude");
        assert_eq!(p.set.get("Artist").map(String::as_str), Some("Tulip"));
        assert_eq!(p.clear, vec!["GPSLatitude"]);
    }
    #[test]
    fn empty_patch_is_noop() {
        // No file needed because apply() short-circuits before spawning.
        let r = apply(Path::new("/dev/null"), &ExifPatch::new()).unwrap();
        assert!(r.is_empty());
    }
}
