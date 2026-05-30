//! Live Activities — long-running jobs that surface progress outside the
//! main window. Targets:
//!   * macOS: ActivityKit live activity pinned in the menubar + Control
//!     Centre. Requires app-group entitlement; the actual ActivityKit
//!     bridge ships under `target_os = "macos"` and is feature-gated.
//!   * Windows 11: `TaskbarManager.SetProgressValue` + jump-list progress
//!     pip. Provided by the WinRT `Windows.UI.Shell` namespace.
//!   * Linux (GNOME/KDE): "background activity" notification stack via
//!     `org.freedesktop.Notifications` + the optional XDG-portal background
//!     interface.
//!
//! This module owns the activity registry + transport-agnostic update
//! pipeline; the per-OS sinks live in the app layer.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivityKind {
    LibraryScan,
    Transcode,
    AiInference,
    Download,
    SyncUpload,
    SyncDownload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityState {
    pub id: String,
    pub kind: ActivityKind,
    pub title: String,
    pub progress: f32, // 0.0..=1.0; <0 = indeterminate
    pub eta_seconds: Option<u32>,
    pub cancellable: bool,
}

#[derive(Debug)]
struct Tracked {
    state: ActivityState,
    started: Instant,
}

#[derive(Default)]
pub struct ActivityRegistry {
    inner: RwLock<Vec<Tracked>>,
}

impl ActivityRegistry {
    pub fn new() -> Arc<Self> { Arc::new(Self::default()) }

    pub fn start(&self, state: ActivityState) {
        let mut g = self.inner.write().unwrap();
        if let Some(t) = g.iter_mut().find(|t| t.state.id == state.id) {
            t.state = state; return;
        }
        g.push(Tracked { state, started: Instant::now() });
    }

    pub fn update<F: FnOnce(&mut ActivityState)>(&self, id: &str, f: F) -> Result<()> {
        let mut g = self.inner.write().unwrap();
        let t = g.iter_mut().find(|t| t.state.id == id)
            .ok_or_else(|| anyhow::anyhow!("activity not found: {id}"))?;
        f(&mut t.state);
        Ok(())
    }

    pub fn finish(&self, id: &str) {
        self.inner.write().unwrap().retain(|t| t.state.id != id);
    }

    pub fn snapshot(&self) -> Vec<ActivityState> {
        self.inner.read().unwrap().iter().map(|t| t.state.clone()).collect()
    }

    pub fn elapsed(&self, id: &str) -> Option<std::time::Duration> {
        self.inner.read().unwrap().iter().find(|t| t.state.id == id).map(|t| t.started.elapsed())
    }
}

pub trait ActivitySink: Send + Sync {
    fn push(&self, state: &ActivityState) -> Result<()>;
    fn drop(&self, id: &str) -> Result<()>;
}

pub struct StubSink;
impl ActivitySink for StubSink {
    fn push(&self, s: &ActivityState) -> Result<()> {
        tracing::info!(id = %s.id, kind = ?s.kind, progress = s.progress, "activity push (stub)");
        Ok(())
    }
    fn drop(&self, id: &str) -> Result<()> {
        tracing::info!(id, "activity drop (stub)"); Ok(())
    }
}

pub fn os_sink() -> Box<dyn ActivitySink> { Box::new(StubSink) }

#[cfg(test)]
mod tests {
    use super::*;
    fn st(id: &str, p: f32) -> ActivityState {
        ActivityState { id: id.into(), kind: ActivityKind::Transcode, title: "T".into(),
                        progress: p, eta_seconds: Some(30), cancellable: true }
    }
    #[test] fn start_update_finish_lifecycle() {
        let r = ActivityRegistry::new();
        r.start(st("a", 0.0));
        r.update("a", |s| s.progress = 0.5).unwrap();
        assert_eq!(r.snapshot()[0].progress, 0.5);
        r.finish("a");
        assert!(r.snapshot().is_empty());
    }
    #[test] fn start_replaces_same_id() {
        let r = ActivityRegistry::new();
        r.start(st("a", 0.0));
        r.start(st("a", 0.9));
        assert_eq!(r.snapshot().len(), 1);
        assert!((r.snapshot()[0].progress - 0.9).abs() < 1e-6);
    }
    #[test] fn missing_update_errors() {
        let r = ActivityRegistry::new();
        assert!(r.update("nope", |_| {}).is_err());
    }
}
