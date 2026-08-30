use std::path::{Path, PathBuf};

pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("APPDATA").map(PathBuf::from)
    };
    base.map(|b| b.join("Tulipix"))
}

pub fn data_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("APPDATA").map(PathBuf::from)
    };
    base.map(|b| b.join("Tulipix"))
}

pub fn cache_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Caches"))
    } else {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    };
    base.map(|b| b.join("Tulipix"))
}

pub fn thumbs_dir() -> Option<PathBuf> { cache_dir().map(|d| d.join("thumbs")) }
pub fn db_path(section: &str) -> Option<PathBuf> { data_dir().map(|d| d.join(format!("{section}.db"))) }
pub fn settings_path() -> Option<PathBuf> { config_dir().map(|d| d.join("settings.json")) }
pub fn logs_dir() -> Option<PathBuf> { data_dir().map(|d| d.join("logs")) }

/// Can we create a file in this directory?
///
/// Asked by trying, because the permission bits lie: a read-only mount, an ACL
/// and a container bind all report a writable mode and then refuse the write.
/// Two callers now — the yt-dlp updater choosing where an update lands, and the
/// subtitle finder choosing whether a download can sit beside the film.
pub fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".tulipix-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}
