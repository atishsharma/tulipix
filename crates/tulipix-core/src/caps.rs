use crate::quota::CapabilitiesFile;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};

const ALL_CAPS: &[&str] = &[
    "photos.scan", "videos.scan", "music.scan", "books.scan",
    "cloud.mount", "cloud.vault", "cloud.union-drive",
    "ai.upscale", "ai.stem-separation", "ai.tts", "ai.face-cluster",
    "ai.tag-cluster", "ai.inpaint", "ai.segment", "ai.colorize", "ai.subtitle-gen",
    "photos.nondestructive-edit", "photos.collage", "photos.raw-timeline",
    "player.hw-decode", "player.hdr-tonemap",
    "profiles.managed",
    "plugins.install", "plugins.custom-scraper",
    "tools.bulk-metadata", "tools.ffmpeg-ui", "tools.dedup",
    "photos.ai.captions", "ai.voice", "ai.cloud-offload", "mcp.server", "mcp.server.write",
    "platform.share-sheet",
    "admin.capabilities-override",
];

static TRACE: AtomicBool = AtomicBool::new(false);

pub fn enable_trace() {
    TRACE.store(true, Ordering::Relaxed);
}

pub fn trace_enabled() -> bool {
    TRACE.load(Ordering::Relaxed)
}

// Local-only app: account tiers removed 2026-06-10 — local has full access.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    LocalBasic,
    LocalPro,
    Admin,
    Guest,
    Kid,
}

impl Tier {
    pub fn as_str(self) -> &'static str {
        match self {
            Tier::LocalBasic => "local_basic",
            Tier::LocalPro => "local_pro",
            Tier::Admin => "admin",
            Tier::Guest => "guest",
            Tier::Kid => "kid",
        }
    }
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "local_basic" => Tier::LocalBasic,
            "local_pro" => Tier::LocalPro,
            "admin" => Tier::Admin,
            "guest" => Tier::Guest,
            "kid" => Tier::Kid,
            _ => return None,
        })
    }
}

/// Capability registry — every gateable feature.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cap {
    PhotosScan,
    VideosScan,
    MusicScan,
    BooksScan,
    CloudMount,
    CloudVault,
    CloudUnionDrive,
    AiUpscale,
    AiStemSeparation,
    AiTts,
    AiFaceCluster,
    AiTagCluster,
    AiInpaint,
    AiSegment,
    AiColorize,
    AiSubtitleGen,
    PhotosNondestructiveEdit,
    PhotosCollage,
    PhotosRawTimeline,
    PlayerHwDecode,
    PlayerHdrTonemap,
    // sync.* caps removed 2026-06-10 — local-only app, no sync backend.
    LocalProfiles,
    PluginsInstall,
    PluginsCustomScraper,
    ToolsBulkMetadata,
    ToolsFfmpegUi,
    ToolsDedup,
    PhotosAiCaptions,
    AiVoice,
    AiCloudOffload,
    McpServer,
    McpServerWrite,
    PlatformShareSheet,
    AdminCapabilitiesOverride,
}

impl Cap {
    pub fn as_str(self) -> &'static str {
        match self {
            Cap::PhotosScan => "photos.scan",
            Cap::VideosScan => "videos.scan",
            Cap::MusicScan => "music.scan",
            Cap::BooksScan => "books.scan",
            Cap::CloudMount => "cloud.mount",
            Cap::CloudVault => "cloud.vault",
            Cap::CloudUnionDrive => "cloud.union-drive",
            Cap::AiUpscale => "ai.upscale",
            Cap::AiStemSeparation => "ai.stem-separation",
            Cap::AiTts => "ai.tts",
            Cap::AiFaceCluster => "ai.face-cluster",
            Cap::AiTagCluster => "ai.tag-cluster",
            Cap::AiInpaint => "ai.inpaint",
            Cap::AiSegment => "ai.segment",
            Cap::AiColorize => "ai.colorize",
            Cap::AiSubtitleGen => "ai.subtitle-gen",
            Cap::PhotosNondestructiveEdit => "photos.nondestructive-edit",
            Cap::PhotosCollage => "photos.collage",
            Cap::PhotosRawTimeline => "photos.raw-timeline",
            Cap::PlayerHwDecode => "player.hw-decode",
            Cap::PlayerHdrTonemap => "player.hdr-tonemap",
            Cap::LocalProfiles => "profiles.managed",
            Cap::PluginsInstall => "plugins.install",
            Cap::PluginsCustomScraper => "plugins.custom-scraper",
            Cap::ToolsBulkMetadata => "tools.bulk-metadata",
            Cap::ToolsFfmpegUi => "tools.ffmpeg-ui",
            Cap::ToolsDedup => "tools.dedup",
            Cap::PhotosAiCaptions => "photos.ai.captions",
            Cap::AiVoice => "ai.voice",
            Cap::AiCloudOffload => "ai.cloud-offload",
            Cap::McpServer => "mcp.server",
            Cap::McpServerWrite => "mcp.server.write",
            Cap::PlatformShareSheet => "platform.share-sheet",
            Cap::AdminCapabilitiesOverride => "admin.capabilities-override",
        }
    }
}

#[derive(Default)]
struct Registry {
    tier_caps: HashMap<Tier, HashSet<String>>,
    file: Option<CapabilitiesFile>,
}

static REGISTRY: OnceLock<RwLock<Registry>> = OnceLock::new();
static CURRENT_TIER: RwLock<Tier> = RwLock::new(Tier::LocalBasic);
static HIT_COUNT: AtomicU64 = AtomicU64::new(0);
static DENIED_COUNT: AtomicU64 = AtomicU64::new(0);

type DeniedListener = Box<dyn Fn(Cap, Tier) + Send + Sync>;
static DENIED_LISTENERS: OnceLock<RwLock<Vec<DeniedListener>>> = OnceLock::new();
static DENIED_NUDGED: OnceLock<RwLock<HashSet<&'static str>>> = OnceLock::new();

fn listeners() -> &'static RwLock<Vec<DeniedListener>> {
    DENIED_LISTENERS.get_or_init(|| RwLock::new(Vec::new()))
}
fn nudged() -> &'static RwLock<HashSet<&'static str>> {
    DENIED_NUDGED.get_or_init(|| RwLock::new(HashSet::new()))
}

pub fn on_denied<F: Fn(Cap, Tier) + Send + Sync + 'static>(f: F) {
    listeners().write().unwrap().push(Box::new(f));
}

/// Reset the per-session de-dup set (called on tier switch).
pub fn reset_nudge_dedup() {
    nudged().write().unwrap().clear();
}

fn emit_denied(cap: Cap, tier: Tier) {
    let mut n = nudged().write().unwrap();
    if n.contains(cap.as_str()) { return; }
    n.insert(cap.as_str());
    drop(n);
    for cb in listeners().read().unwrap().iter() {
        cb(cap, tier);
    }
}

fn reg() -> &'static RwLock<Registry> {
    REGISTRY.get_or_init(|| RwLock::new(Registry::default()))
}

/// Load capabilities.toml (default + optional local override). Idempotent.
/// Schema-validates: every cap string in [tiers] must be `*` or a known Cap.
pub fn load_from_toml(default_toml: &str, override_toml: Option<&str>) -> anyhow::Result<()> {
    let mut file = CapabilitiesFile::parse(default_toml)?;
    if let Some(o) = override_toml {
        let ov = CapabilitiesFile::parse(o)?;
        for (k, v) in ov.tiers { file.tiers.insert(k, v); }
        for (k, v) in ov.quota { file.quota.insert(k, v); }
    }
    let known: HashSet<&str> = ALL_CAPS.iter().copied().collect();
    let mut tier_caps: HashMap<Tier, HashSet<String>> = HashMap::new();
    for (tier_name, caps) in &file.tiers {
        let Some(t) = Tier::from_str(tier_name) else {
            tracing::warn!(tier = %tier_name, "unknown tier in capabilities.toml — ignored");
            continue;
        };
        for c in caps {
            if c == "*" { continue; }
            if !known.contains(c.as_str()) {
                anyhow::bail!("capabilities.toml: tier {tier_name} references unknown cap `{c}`");
            }
        }
        tier_caps.insert(t, caps.iter().cloned().collect());
    }
    let mut r = reg().write().unwrap();
    r.tier_caps = tier_caps;
    r.file = Some(file);
    drop(r);
    for cb in reload_listeners().read().unwrap().iter() { cb(); }
    Ok(())
}

type ReloadListener = Box<dyn Fn() + Send + Sync>;
static RELOAD_LISTENERS: OnceLock<RwLock<Vec<ReloadListener>>> = OnceLock::new();
fn reload_listeners() -> &'static RwLock<Vec<ReloadListener>> {
    RELOAD_LISTENERS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn on_reload<F: Fn() + Send + Sync + 'static>(f: F) {
    reload_listeners().write().unwrap().push(Box::new(f));
}

/// Reload capabilities from disk, merging an optional override file. Used by
/// the dev-reload watcher to pick up edits without restart.
pub fn reload_from_disk(
    default_path: &std::path::Path,
    override_path: Option<&std::path::Path>,
) -> anyhow::Result<()> {
    let default = std::fs::read_to_string(default_path)?;
    let ov = match override_path {
        Some(p) if p.exists() => Some(std::fs::read_to_string(p)?),
        _ => None,
    };
    load_from_toml(&default, ov.as_deref())
}

/// Per-service per-day quota for current tier. None = service not declared.
pub fn quota_for_service(service: &str) -> Option<i64> {
    let tier = current_tier().as_str();
    let r = reg().read().unwrap();
    r.file.as_ref()?.quota_for(service, tier)
}

pub fn set_current_tier(t: Tier) {
    *CURRENT_TIER.write().unwrap() = t;
}

pub fn current_tier() -> Tier {
    *CURRENT_TIER.read().unwrap()
}

pub fn is_allowed(cap: Cap) -> bool {
    HIT_COUNT.fetch_add(1, Ordering::Relaxed);
    let tier = current_tier();
    let r = reg().read().unwrap();
    let allowed = match r.tier_caps.get(&tier) {
        Some(set) => set.contains("*") || set.contains(cap.as_str()),
        None => false,
    };
    drop(r);
    if !allowed {
        DENIED_COUNT.fetch_add(1, Ordering::Relaxed);
        emit_denied(cap, tier);
    }
    if trace_enabled() {
        tracing::info!(tier = tier.as_str(), cap = cap.as_str(), allowed, "caps::is_allowed");
    }
    allowed
}

pub fn hits() -> u64 { HIT_COUNT.load(Ordering::Relaxed) }
pub fn denials() -> u64 { DENIED_COUNT.load(Ordering::Relaxed) }

/// Persist hit counter to cache/caps-hits.json.
pub fn persist_hit_counts(cache_dir: &std::path::Path) -> anyhow::Result<()> {
    std::fs::create_dir_all(cache_dir)?;
    let path = cache_dir.join("caps-hits.json");
    let body = serde_json::json!({
        "hits": hits(),
        "denials": denials(),
        "tier": current_tier().as_str(),
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&body)?)?;
    Ok(())
}

#[macro_export]
macro_rules! require {
    ($cap:expr) => {{
        if !$crate::caps::is_allowed($cap) {
            anyhow::bail!("capability denied: {:?}", $cap);
        }
    }};
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[tiers]
local_basic = []
local_pro = ["photos.scan", "videos.scan"]
admin = ["*"]
"#;

    #[test]
    fn deny_by_default() {
        load_from_toml(SAMPLE, None).unwrap();
        set_current_tier(Tier::LocalBasic);
        assert!(!is_allowed(Cap::PhotosScan));
    }

    #[test]
    fn pro_unlocks() {
        load_from_toml(SAMPLE, None).unwrap();
        set_current_tier(Tier::LocalPro);
        assert!(is_allowed(Cap::PhotosScan));
        assert!(!is_allowed(Cap::AiUpscale));
    }

    #[test]
    fn admin_wildcard() {
        load_from_toml(SAMPLE, None).unwrap();
        set_current_tier(Tier::Admin);
        assert!(is_allowed(Cap::AiUpscale));
    }
}
