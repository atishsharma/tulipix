//! Cross-device handoff (Account mode) — resume a photo / playback / chat
//! session on another logged-in device. Backed by the realtime sync channel:
//! one HandoffSnapshot per active context, expired after 5 minutes idle.
//!
//! Cap-gated `sync.enabled` (only Account-tier seats can publish/resume).

use crate::caps::{is_allowed, Cap};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::RwLock;
use std::time::{SystemTime, UNIX_EPOCH};

const TTL_SECONDS: u64 = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HandoffContext {
    PhotoViewer,
    VideoPlayback,
    MusicPlayback,
    BookReading,
    ChatThread,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffSnapshot {
    pub context: HandoffContext,
    pub device_id: String,
    pub device_label: String,
    pub deep_link: String,
    pub position_ms: Option<u64>,
    pub updated_at: u64,
}

#[derive(Debug, Default)]
pub struct HandoffRegistry {
    inner: RwLock<Vec<HandoffSnapshot>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl HandoffRegistry {
    pub fn new() -> Self { Self::default() }

    pub fn publish(&self, mut snap: HandoffSnapshot) -> Result<()> {
        if !is_allowed(Cap::SyncEnabled) { anyhow::bail!("sync.enabled denied"); }
        snap.updated_at = now();
        let mut g = self.inner.write().unwrap();
        g.retain(|s| !(s.context == snap.context && s.device_id == snap.device_id));
        g.push(snap);
        Ok(())
    }

    /// Snapshots from *other* devices for the given context, freshest first.
    pub fn available(&self, context: HandoffContext, self_device_id: &str) -> Vec<HandoffSnapshot> {
        let cutoff = now().saturating_sub(TTL_SECONDS);
        let mut out: Vec<_> = self
            .inner.read().unwrap().iter()
            .filter(|s| s.context == context && s.device_id != self_device_id && s.updated_at >= cutoff)
            .cloned().collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        out
    }

    /// Mark a snapshot consumed so the publisher device clears its picker entry.
    pub fn resume(&self, context: HandoffContext, device_id: &str) -> Result<HandoffSnapshot> {
        if !is_allowed(Cap::SyncEnabled) { anyhow::bail!("sync.enabled denied"); }
        let mut g = self.inner.write().unwrap();
        let pos = g.iter().position(|s| s.context == context && s.device_id == device_id)
            .ok_or_else(|| anyhow::anyhow!("no snapshot for {device_id}"))?;
        Ok(g.remove(pos))
    }

    pub fn gc_expired(&self) {
        let cutoff = now().saturating_sub(TTL_SECONDS);
        self.inner.write().unwrap().retain(|s| s.updated_at >= cutoff);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::caps::{load_from_toml, set_current_tier, Tier};
    use std::sync::Mutex;
    static SERIAL: Mutex<()> = Mutex::new(());
    const CAPS: &str = r#"
[tiers]
local_basic  = []
account_pro  = ["sync.enabled"]
"#;
    fn snap(dev: &str, ctx: HandoffContext) -> HandoffSnapshot {
        HandoffSnapshot {
            context: ctx, device_id: dev.into(), device_label: dev.into(),
            deep_link: format!("tulipix://x/{dev}"), position_ms: Some(42000), updated_at: 0,
        }
    }
    #[test] fn cap_gates_publish() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::LocalBasic);
        let r = HandoffRegistry::new();
        assert!(r.publish(snap("phone", HandoffContext::ChatThread)).is_err());
    }
    #[test] fn other_device_visible() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::AccountPro);
        let r = HandoffRegistry::new();
        r.publish(snap("phone", HandoffContext::MusicPlayback)).unwrap();
        let av = r.available(HandoffContext::MusicPlayback, "laptop");
        assert_eq!(av.len(), 1);
        assert_eq!(av[0].device_id, "phone");
    }
    #[test] fn resume_removes_entry() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        load_from_toml(CAPS, None).unwrap();
        set_current_tier(Tier::AccountPro);
        let r = HandoffRegistry::new();
        r.publish(snap("phone", HandoffContext::MusicPlayback)).unwrap();
        let _resumed = r.resume(HandoffContext::MusicPlayback, "phone").unwrap();
        assert!(r.available(HandoffContext::MusicPlayback, "laptop").is_empty());
    }
}
