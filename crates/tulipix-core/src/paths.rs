use std::path::{Path, PathBuf};

/// The environment variable that picks a profile. Empty or unset is the one
/// everybody has had until now, whose folders keep their old names.
pub const PROFILE_ENV: &str = "TULIPIX_PROFILE";

/// What goes on the end of every folder name: `-mom` for the "mom" profile,
/// nothing for the default one.
///
/// Read once. Every path in the app is built from these three functions, so a
/// value that changed mid-run would leave half the process looking at one
/// library and half at another; switching profiles restarts instead.
pub fn profile_suffix() -> &'static str {
    static S: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    S.get_or_init(|| match std::env::var(PROFILE_ENV) {
        Ok(v) => {
            let slug = slugify_profile(&v);
            if slug.is_empty() { String::new() } else { format!("-{slug}") }
        }
        Err(_) => String::new(),
    })
}

/// A profile name as a folder-safe slug. The name comes from a text box, and
/// it becomes a directory: anything that is not a letter, a digit or a dash
/// has no business in one.
pub fn slugify_profile(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = true;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash {
            out.push('-');
            dash = true;
        }
    }
    out.trim_matches('-').chars().take(40).collect()
}

/// The folder name for this profile: "Tulipix", or "Tulipix-mom".
fn app_dir_name() -> String {
    format!("Tulipix{}", profile_suffix())
}

pub fn config_dir() -> Option<PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("APPDATA").map(PathBuf::from)
    };
    base.map(|b| b.join(app_dir_name()))
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
    base.map(|b| b.join(app_dir_name()))
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
    base.map(|b| b.join(app_dir_name()))
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
