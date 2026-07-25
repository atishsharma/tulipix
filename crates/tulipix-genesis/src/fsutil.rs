//! The two filesystem helpers the download path needs, and the location of the
//! mirror-ranking cache.
//!
//! Upstream these lived in a `config` module that also parsed a TOML file and
//! owned an XDG layout. The app already answers both of those questions —
//! `Settings.advanced` for configuration, [`tulipix_core::paths`] for
//! directories — so only the file primitives came across.

use std::path::{Path, PathBuf};

use crate::error::{Context, Result};

pub fn ensure_dir(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_context(|| format!("could not create directory {}", path.display()))
}

/// Create (or truncate) a file only this user can read.
///
/// The mode matters: a partial download sits in a directory the user chose, and
/// on a shared machine the default umask would leave it world-readable.
pub fn create_private_file(path: &Path) -> Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path).with_context(|| format!("could not create {}", path.display()))
}

/// Where downloads land: `<Downloads>/Genesis`.
///
/// Deliberately *not* an existing library folder. A curated shelf is something
/// the user arranged; dropping fetched files into it mixes the two and there is
/// no way to tell them apart afterwards. Its own folder under the system
/// Downloads directory keeps the boundary obvious, and the app registers it as
/// a library root so the books still show up in My Library.
pub fn default_dest() -> PathBuf {
    downloads_dir().unwrap_or_else(|| PathBuf::from(".")).join("Genesis")
}

/// The system Downloads folder.
///
/// `XDG_DOWNLOAD_DIR` first so a relocated folder is honoured; otherwise the
/// conventional spot under the home directory. Same shape as the transfer
/// crate's inbox lookup — no `dirs` dependency for two `env::var_os` calls.
fn downloads_dir() -> Option<PathBuf> {
    if let Some(x) = std::env::var_os("XDG_DOWNLOAD_DIR") {
        let p = PathBuf::from(x);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Downloads"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads"))
    }
}

/// Where the ranked mirror list is remembered between runs.
///
/// In the app's own cache directory, so clearing caches clears this too. A
/// missing directory is not an error here — every caller treats a cache miss
/// as "probe again", which is always correct, just slower.
pub fn mirror_cache_path() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("genesis-mirrors.tsv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_land_in_their_own_folder_under_downloads() {
        let dest = default_dest();
        assert_eq!(dest.file_name().and_then(|n| n.to_str()), Some("Genesis"));
        // Never the bare Downloads folder: fetched books stay separable from
        // everything else that lands there.
        assert!(dest.parent().is_some());
    }
}
