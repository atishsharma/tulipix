//! AI model version manifest + update pipeline.
//!
//! Pieces:
//!   * `ModelEntry`     — one row in `resources/ai-models.toml` (name +
//!     SemVer + sha256 + url + size + execution-provider compat + quant +
//!     min VRAM + capability gate).
//!   * `Manifest`       — parsed TOML; `load_baseline` reads the shipped
//!     copy, `load_live` overlays a live override (signed manifest fetched
//!     from the appcast URL, see api_endpoints::update_channel).
//!   * `UpdateChecker`  — daily-schedule wrapper that walks the manifest
//!     and emits `PendingUpdate` rows when a higher SemVer is available.
//!     Settings → AI Models reads these for the badge + row state.
//!   * `FirstOpenPrompt` — gate that fires the "Latest AI models
//!     available" dialog on every first launch with internet detected,
//!     with per-version dismiss so the same version never re-prompts.
//!   * `AutoUpdatePolicy` — global toggle + tier default + bandwidth cap.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SemVer { pub major: u32, pub minor: u32, pub patch: u32 }

impl SemVer {
    pub fn parse(s: &str) -> Result<Self> {
        let mut it = s.trim().trim_start_matches('v').split('.');
        let major: u32 = it.next().ok_or_else(|| anyhow!("missing major"))?.parse()?;
        let minor: u32 = it.next().ok_or_else(|| anyhow!("missing minor"))?.parse()?;
        let patch: u32 = it.next().ok_or_else(|| anyhow!("missing patch"))?.parse()?;
        if it.next().is_some() { return Err(anyhow!("extra version component")); }
        Ok(Self { major, minor, patch })
    }
}

impl PartialOrd for SemVer { fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> { Some(self.cmp(other)) } }
impl Ord for SemVer {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch))
    }
}
impl std::fmt::Display for SemVer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EpCompat { CpuOnly, Cuda, Metal, DirectMl, CoreMl, Vulkan, Any }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub version: SemVer,
    pub sha256: String,
    pub url: String,
    pub size_bytes: u64,
    pub ep_compat: Vec<EpCompat>,
    pub quant: String,
    pub min_vram_mb: u32,
    /// Capability gate (e.g. "ai.captions"). Empty = no gate.
    #[serde(default)]
    pub cap: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub models: Vec<ModelEntry>,
}

impl Manifest {
    pub fn from_toml(text: &str) -> Result<Self> {
        #[derive(Deserialize)]
        struct Row {
            name: String,
            version: String,
            sha256: String,
            url: String,
            size_bytes: u64,
            #[serde(default)] ep_compat: Vec<EpCompat>,
            #[serde(default)] quant: String,
            #[serde(default)] min_vram_mb: u32,
            #[serde(default)] cap: String,
        }
        #[derive(Deserialize)]
        struct Doc { #[serde(default)] models: Vec<Row> }
        let doc: Doc = toml::from_str(text)?;
        let mut models = Vec::with_capacity(doc.models.len());
        for r in doc.models {
            models.push(ModelEntry {
                name: r.name, version: SemVer::parse(&r.version)?, sha256: r.sha256, url: r.url,
                size_bytes: r.size_bytes, ep_compat: r.ep_compat, quant: r.quant,
                min_vram_mb: r.min_vram_mb, cap: r.cap,
            });
        }
        Ok(Self { models })
    }
    pub fn find(&self, name: &str) -> Option<&ModelEntry> { self.models.iter().find(|m| m.name == name) }
}

// ─── Update checker ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PendingUpdate {
    pub name: String,
    pub installed: SemVer,
    pub latest: SemVer,
    pub size_bytes: u64,
}

pub type InstalledVersions = BTreeMap<String, SemVer>;

pub fn check(installed: &InstalledVersions, live: &Manifest) -> Vec<PendingUpdate> {
    let mut out = Vec::new();
    for entry in &live.models {
        let Some(have) = installed.get(&entry.name) else { continue; };
        if entry.version > *have {
            out.push(PendingUpdate { name: entry.name.clone(), installed: *have, latest: entry.version, size_bytes: entry.size_bytes });
        }
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckCadence { OnLaunch, Daily }

#[derive(Debug, Clone)]
pub struct CheckSchedule { pub last_check_unix: u64, pub cadence: CheckCadence }

impl CheckSchedule {
    pub fn due(&self, now_unix: u64) -> bool {
        match self.cadence {
            CheckCadence::OnLaunch => true,
            CheckCadence::Daily    => now_unix.saturating_sub(self.last_check_unix) >= 86_400,
        }
    }
}

// ─── First-open prompt ───────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DismissLedger {
    /// Map of model-name → highest dismissed SemVer. A higher published
    /// version re-opens the prompt for that model.
    pub dismissed: BTreeMap<String, SemVer>,
}

impl DismissLedger {
    pub fn dismiss(&mut self, name: &str, version: SemVer) {
        let cur = self.dismissed.get(name).copied();
        if cur.map(|c| version > c).unwrap_or(true) { self.dismissed.insert(name.into(), version); }
    }
    pub fn is_dismissed(&self, name: &str, version: SemVer) -> bool {
        self.dismissed.get(name).map(|c| *c >= version).unwrap_or(false)
    }
}

pub fn promptable(updates: &[PendingUpdate], ledger: &DismissLedger, has_internet: bool) -> Vec<PendingUpdate> {
    if !has_internet { return Vec::new(); }
    updates.iter().filter(|u| !ledger.is_dismissed(&u.name, u.latest)).cloned().collect()
}

// ─── Auto-update + bandwidth policy ──────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoUpdateMode { Off, AskFirst, Auto }

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct AutoUpdatePolicy {
    pub mode: AutoUpdateMode,
    /// 0 = unlimited.
    pub bandwidth_kbps: u32,
}

impl AutoUpdatePolicy {
    /// Tier default: account.free+ gets `Auto`, anything else gets `AskFirst`.
    pub fn default_for_tier(tier: &str) -> Self {
        let auto = matches!(tier, "account.free" | "account.plus" | "account.pro" | "admin");
        Self { mode: if auto { AutoUpdateMode::Auto } else { AutoUpdateMode::AskFirst }, bandwidth_kbps: 0 }
    }
    pub fn should_download_now(&self) -> bool { matches!(self.mode, AutoUpdateMode::Auto) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toml_doc() -> &'static str {
        r#"
        [[models]]
        name = "clip-vit-b-32"
        version = "1.2.3"
        sha256 = "deadbeef"
        url = "https://x/clip.onnx"
        size_bytes = 340000000
        ep_compat = ["cpu-only", "core-ml", "cuda"]
        quant = "fp16"
        min_vram_mb = 0
        cap = "photos.ai.clip"

        [[models]]
        name = "scrfd"
        version = "0.4.0"
        sha256 = "feedface"
        url = "https://x/scrfd.onnx"
        size_bytes = 17000000
        "#
    }

    #[test] fn semver_orders() {
        let a = SemVer::parse("1.2.3").unwrap();
        let b = SemVer::parse("v1.2.4").unwrap();
        assert!(b > a);
        assert!(SemVer::parse("1.2").is_err());
    }
    #[test] fn manifest_parses_with_defaults() {
        let m = Manifest::from_toml(toml_doc()).unwrap();
        assert_eq!(m.models.len(), 2);
        assert_eq!(m.find("clip-vit-b-32").unwrap().version, SemVer { major: 1, minor: 2, patch: 3 });
        assert_eq!(m.find("scrfd").unwrap().cap, "");
    }
    #[test] fn check_flags_higher_versions_only() {
        let live = Manifest::from_toml(toml_doc()).unwrap();
        let mut installed = InstalledVersions::new();
        installed.insert("clip-vit-b-32".into(), SemVer::parse("1.2.0").unwrap());
        installed.insert("scrfd".into(), SemVer::parse("0.4.0").unwrap());
        let updates = check(&installed, &live);
        assert_eq!(updates.len(), 1);
        assert_eq!(updates[0].name, "clip-vit-b-32");
        assert_eq!(updates[0].latest, SemVer { major: 1, minor: 2, patch: 3 });
    }
    #[test] fn schedule_daily_gates_24h() {
        let s = CheckSchedule { last_check_unix: 1_000_000, cadence: CheckCadence::Daily };
        assert!(!s.due(1_000_000 + 86_399));
        assert!(s.due(1_000_000 + 86_400));
        let l = CheckSchedule { last_check_unix: 1_000_000, cadence: CheckCadence::OnLaunch };
        assert!(l.due(1_000_001));
    }
    #[test] fn dismiss_persists_until_higher_version() {
        let mut ledger = DismissLedger::default();
        let v1 = SemVer::parse("1.0.0").unwrap();
        let v2 = SemVer::parse("1.1.0").unwrap();
        ledger.dismiss("clip", v1);
        assert!(ledger.is_dismissed("clip", v1));
        assert!(!ledger.is_dismissed("clip", v2));
        let updates = vec![PendingUpdate { name: "clip".into(), installed: SemVer { major: 0, minor: 9, patch: 0 }, latest: v1, size_bytes: 100 }];
        assert!(promptable(&updates, &ledger, true).is_empty());
        // Offline never prompts.
        let v_new = vec![PendingUpdate { name: "clip".into(), installed: SemVer { major: 0, minor: 9, patch: 0 }, latest: v2, size_bytes: 100 }];
        assert_eq!(promptable(&v_new, &ledger, true).len(), 1);
        assert!(promptable(&v_new, &ledger, false).is_empty());
    }
    #[test] fn auto_update_default_per_tier() {
        let basic = AutoUpdatePolicy::default_for_tier("local.basic");
        assert_eq!(basic.mode, AutoUpdateMode::AskFirst);
        let pro = AutoUpdatePolicy::default_for_tier("account.pro");
        assert_eq!(pro.mode, AutoUpdateMode::Auto);
        assert!(pro.should_download_now());
        assert!(!basic.should_download_now());
    }
}
