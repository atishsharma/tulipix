//! Tiny persisted Book-Home view preference (grid vs list). One line of plain
//! text in the config dir — no serde, same lightweight pattern as
//! `watched_folders`. Sort order is intentionally not persisted yet (needs the
//! sort ComboBox current-index plumbed through; low value).

use std::fs;
use std::path::PathBuf;

fn path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("books_view.txt"))
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
