use crate::libraries::LibrariesConfig;
use crate::paths;
use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default)]
    pub libraries: LibrariesConfig,
    #[serde(default = "default_theme")]
    pub theme: String, // "system" | "light" | "extra-dark"
    #[serde(default)]
    pub view_mode_per_section: std::collections::HashMap<String, ViewMode>,
    #[serde(default = "default_true")]
    pub reduce_motion: bool,
    /// Idle auto-lock timeout in seconds (0 = never).
    #[serde(default)]
    pub idle_lock_secs: u64,
    /// Generic on/off feature flags surfaced in the Settings panels
    /// (notifications, passkey, db-encrypt, provider toggles, …). Keyed by a
    /// stable setting id so new toggles need no schema change.
    #[serde(default)]
    pub flags: std::collections::HashMap<String, bool>,
    /// Generic free-text settings (custom endpoint URLs, model channels, …).
    #[serde(default)]
    pub advanced: std::collections::HashMap<String, String>,
}

fn default_theme() -> String { "system".into() }
fn default_true() -> bool { false }

impl Settings {
    /// Read a feature flag with a default for first-run.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        self.flags.get(key).copied().unwrap_or(default)
    }
    /// Read a free-text advanced setting (empty string if unset).
    pub fn text(&self, key: &str) -> String {
        self.advanced.get(key).cloned().unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ViewMode { #[default]
Library, Folders, Timeline }


/// The settings file, parsed once (np.perf.settings-cache).
///
/// `Settings::load()` is called from 89 places, many of them UI callbacks and a
/// few inside loops, and each call was a `read_to_string` + a full serde parse
/// on whatever thread asked — including the event-loop thread. The file is
/// small, but blocking disk I/O in a click handler is the wrong shape.
///
/// `save()` refreshes this, so the app always reads back its own writes. The
/// cache is only wrong if *another process* rewrites settings.json underneath a
/// running app — the CLI can, and this trades that (already racy) case away.
/// `invalidate()` is the escape hatch.
static CACHE: std::sync::OnceLock<std::sync::RwLock<Option<Settings>>> = std::sync::OnceLock::new();
fn cache() -> &'static std::sync::RwLock<Option<Settings>> {
    CACHE.get_or_init(|| std::sync::RwLock::new(None))
}

/// Forget the parsed settings; the next `load` re-reads the file. Call after
/// anything outside `Settings::save` has written it (factory reset, import).
pub fn invalidate() {
    if let Ok(mut g) = cache().write() {
        *g = None;
    }
}

impl Settings {
    pub fn load() -> Result<Self> {
        if let Ok(g) = cache().read() {
            if let Some(s) = g.as_ref() {
                return Ok(s.clone());
            }
        }
        let parsed = Self::read_file()?;
        if let Ok(mut g) = cache().write() {
            *g = Some(parsed.clone());
        }
        Ok(parsed)
    }

    /// The uncached read. Kept separate so `load` stays a cache lookup and the
    /// parse lives in one place.
    fn read_file() -> Result<Self> {
        let Some(path) = paths::settings_path() else { return Ok(Settings::default()); };
        if !path.exists() { return Ok(Settings::default()); }
        let text = std::fs::read_to_string(&path)?;
        if text.trim().is_empty() { return Ok(Settings::default()); }
        Ok(serde_json::from_str(&text)?)
    }

    pub fn save(&self) -> Result<()> {
        let Some(path) = paths::settings_path() else { return Ok(()); };
        if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;
        // Publish AFTER the rename, so a failed write leaves the cache holding
        // what is actually on disk rather than what we hoped to put there.
        if let Ok(mut g) = cache().write() {
            *g = Some(self.clone());
        }
        Ok(())
    }
}
