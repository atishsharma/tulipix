//! Tiny persisted Book-Home preferences (view mode + sort order). Plain text in
//! the config dir — no serde, same lightweight pattern as `watched_folders`.

use std::fs;
use std::path::PathBuf;

fn path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("books_view.txt"))
}

fn sort_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("books_sort.txt"))
}

/// Persisted sort index (0‥4), defaulting to 0 (Recently Added).
pub fn load_sort() -> usize {
    sort_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|i| *i <= 4)
        .unwrap_or(0)
}

/// Persist the sort index (best-effort).
pub fn save_sort(idx: usize) {
    let Some(p) = sort_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&p, idx.to_string());
}

/// Persisted view mode, defaulting to "grid" when missing or unrecognised.
pub fn load_view_mode() -> String {
    path()
        .and_then(|p| fs::read_to_string(p).ok())
        .map(|s| s.trim().to_string())
        .filter(|v| v == "grid" || v == "list")
        .unwrap_or_else(|| "grid".into())
}

/// Persist the view mode (best-effort; ignores IO errors).
pub fn save_view_mode(mode: &str) {
    let Some(p) = path() else { return };
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(&p, mode);
}
