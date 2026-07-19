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

/// Persisted sort index (0‥4), defaulting to 2 (Title / name).
pub fn load_sort() -> usize {
    sort_path()
        .and_then(|p| fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<usize>().ok())
        .filter(|i| *i <= 4)
        .unwrap_or(2)
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

fn reader_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("books_reader.txt"))
}

/// Persisted reader typography/appearance prefs, one space-separated line:
/// `font_px line_idx margin_idx typeface align bold theme brightness`.
pub fn load_reader_prefs() -> Option<(f32, u8, u8, u8, u8, bool, u8, f32)> {
    let s = reader_path().and_then(|p| fs::read_to_string(p).ok())?;
    let v: Vec<&str> = s.split_whitespace().collect();
    if v.len() != 8 {
        return None;
    }
    Some((
        v[0].parse().ok()?,
        v[1].parse().ok()?,
        v[2].parse().ok()?,
        v[3].parse().ok()?,
        v[4].parse().ok()?,
        v[5] == "1",
        v[6].parse().ok()?,
        v[7].parse().ok()?,
    ))
}

/// Persist the reader prefs (best-effort).
#[allow(clippy::too_many_arguments)]
pub fn save_reader_prefs(
    font_px: f32,
    line_idx: u8,
    margin_idx: u8,
    typeface: u8,
    align: u8,
    bold: bool,
    theme: u8,
    brightness: f32,
) {
    let Some(p) = reader_path() else { return };
    if let Some(dir) = p.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::write(
        &p,
        format!(
            "{font_px} {line_idx} {margin_idx} {typeface} {align} {} {theme} {brightness}",
            if bold { 1 } else { 0 }
        ),
    );
}
