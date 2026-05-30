//! Pre-roll bumper — user-supplied MP4 plays before every movie. Skippable
//! after `min_visible_ms`. The selection is round-robin across the configured
//! pool so a user with multiple bumpers doesn't get the same intro on every
//! launch.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PreRollConfig {
    pub enabled: bool,
    pub bumpers: Vec<PathBuf>,
    pub min_visible_ms: i64,
    /// Per-section override. `None` means "use the global pool".
    pub section_overrides: std::collections::BTreeMap<String, Vec<PathBuf>>,
}

impl Default for PreRollConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bumpers: Vec::new(),
            min_visible_ms: 1_500,
            section_overrides: Default::default(),
        }
    }
}

#[derive(Debug, Default)]
pub struct PreRollSelector {
    cursor: AtomicUsize,
}

impl PreRollSelector {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn next_for(&self, cfg: &PreRollConfig, section: &str) -> Option<PathBuf> {
        if !cfg.enabled {
            return None;
        }
        let pool = cfg
            .section_overrides
            .get(section)
            .filter(|p| !p.is_empty())
            .unwrap_or(&cfg.bumpers);
        if pool.is_empty() {
            return None;
        }
        let i = self.cursor.fetch_add(1, Ordering::Relaxed) % pool.len();
        Some(pool[i].clone())
    }

    /// Reset the round-robin cursor — used when the bumper pool changes so
    /// the first play after a Settings update isn't stale.
    pub fn reset(&self) {
        self.cursor.store(0, Ordering::Relaxed);
    }
}

/// Returns whether the Skip button should be live at `position_ms`.
pub fn can_skip(min_visible_ms: i64, position_ms: i64) -> bool {
    position_ms >= min_visible_ms
}

/// Validate the bumper paths — drops missing files / non-video extensions and
/// returns a cleaned list the UI can show. Doesn't touch disk beyond
/// `Path::exists()` so it's safe to call from the settings panel.
pub fn validate_pool(paths: &[PathBuf], cwd: &Path) -> Vec<PathBuf> {
    let valid_ext = ["mp4", "mkv", "mov", "m4v", "webm"];
    paths
        .iter()
        .map(|p| {
            if p.is_absolute() {
                p.clone()
            } else {
                cwd.join(p)
            }
        })
        .filter(|p| p.exists())
        .filter(|p| {
            p.extension()
                .and_then(|s| s.to_str())
                .map(|s| valid_ext.contains(&s.to_ascii_lowercase().as_str()))
                .unwrap_or(false)
        })
        .collect()
}

pub fn load_config(path: &Path) -> Result<PreRollConfig> {
    if !path.exists() {
        return Ok(PreRollConfig::default());
    }
    let bytes = std::fs::read(path)?;
    Ok(serde_json::from_slice(&bytes).unwrap_or_default())
}

pub fn save_config(path: &Path, cfg: &PreRollConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let bytes = serde_json::to_vec_pretty(cfg)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn disabled_returns_none() {
        let cfg = PreRollConfig::default();
        let sel = PreRollSelector::new();
        assert!(sel.next_for(&cfg, "videos").is_none());
    }

    #[test]
    fn round_robin_walks_pool() {
        let cfg = PreRollConfig {
            enabled: true,
            bumpers: vec!["a.mp4".into(), "b.mp4".into(), "c.mp4".into()],
            ..Default::default()
        };
        let sel = PreRollSelector::new();
        let a = sel.next_for(&cfg, "videos").unwrap();
        let b = sel.next_for(&cfg, "videos").unwrap();
        let c = sel.next_for(&cfg, "videos").unwrap();
        let d = sel.next_for(&cfg, "videos").unwrap();
        assert_eq!(a, PathBuf::from("a.mp4"));
        assert_eq!(b, PathBuf::from("b.mp4"));
        assert_eq!(c, PathBuf::from("c.mp4"));
        assert_eq!(d, PathBuf::from("a.mp4"), "wraps");
    }

    #[test]
    fn section_override_takes_priority() {
        let mut cfg = PreRollConfig {
            enabled: true,
            bumpers: vec!["global.mp4".into()],
            ..Default::default()
        };
        cfg.section_overrides
            .insert("videos".into(), vec!["videos.mp4".into()]);
        let sel = PreRollSelector::new();
        assert_eq!(
            sel.next_for(&cfg, "videos").unwrap(),
            PathBuf::from("videos.mp4")
        );
        assert_eq!(
            sel.next_for(&cfg, "music").unwrap(),
            PathBuf::from("global.mp4")
        );
    }

    #[test]
    fn empty_override_falls_back_to_global() {
        let mut cfg = PreRollConfig {
            enabled: true,
            bumpers: vec!["global.mp4".into()],
            ..Default::default()
        };
        cfg.section_overrides.insert("videos".into(), vec![]);
        let sel = PreRollSelector::new();
        assert_eq!(
            sel.next_for(&cfg, "videos").unwrap(),
            PathBuf::from("global.mp4")
        );
    }

    #[test]
    fn skip_threshold() {
        assert!(!can_skip(1_500, 1_499));
        assert!(can_skip(1_500, 1_500));
        assert!(can_skip(1_500, 5_000));
    }

    #[test]
    fn validate_drops_missing_and_unknown_ext() {
        let d = tempdir().unwrap();
        let ok = d.path().join("real.mp4");
        std::fs::write(&ok, b"x").unwrap();
        let txt = d.path().join("notes.txt");
        std::fs::write(&txt, b"x").unwrap();
        let pool = validate_pool(
            &[
                ok.clone(),
                txt,
                d.path().join("missing.mp4"),
            ],
            d.path(),
        );
        assert_eq!(pool, vec![ok]);
    }

    #[test]
    fn save_then_load_round_trip() {
        let d = tempdir().unwrap();
        let path = d.path().join("preroll.json");
        let mut cfg = PreRollConfig::default();
        cfg.enabled = true;
        cfg.bumpers.push("/tmp/x.mp4".into());
        save_config(&path, &cfg).unwrap();
        let again = load_config(&path).unwrap();
        assert_eq!(again, cfg);
    }
}
