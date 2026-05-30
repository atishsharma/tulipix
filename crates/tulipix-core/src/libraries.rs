use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum Section {
    Photos,
    Videos,
    Music,
    Books,
    Cloud,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScanCadence {
    Manual,
    Hourly,
    Daily,
    Weekly,
}

impl Default for ScanCadence {
    fn default() -> Self { Self::Daily }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Library {
    pub id: String,
    pub path: PathBuf,
    pub section: Section,
    pub last_scan: Option<SystemTime>,
    pub item_count: u64,
    pub size_bytes: u64,
    #[serde(default)]
    pub exclude_globs: Vec<String>,
    #[serde(default)]
    pub cadence_override: Option<ScanCadence>,
    #[serde(default = "default_true")]
    pub realtime_notify: bool,
}

fn default_true() -> bool { true }

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LibrariesConfig {
    #[serde(default)]
    pub libraries: Vec<Library>,
    #[serde(default)]
    pub default_cadence: ScanCadence,
}

impl LibrariesConfig {
    pub fn add(&mut self, lib: Library) { self.libraries.push(lib); }
    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.libraries.len();
        self.libraries.retain(|l| l.id != id);
        before != self.libraries.len()
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut Library> {
        self.libraries.iter_mut().find(|l| l.id == id)
    }
    pub fn effective_cadence(&self, id: &str) -> ScanCadence {
        self.libraries
            .iter()
            .find(|l| l.id == id)
            .and_then(|l| l.cadence_override)
            .unwrap_or(self.default_cadence)
    }
}
