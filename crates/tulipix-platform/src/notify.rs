//! Actionable notifications — per-OS toast / banner with click-through
//! actions. Snooze / Mark-played / Dismiss / Open are the four primary
//! action verbs; callers add more via `Notification::action()`.
//!
//! macOS: `UNNotificationAction` + `UNNotificationCategory` registered at
//! launch; click delivered via `UNUserNotificationCenterDelegate`.
//! Windows: WinRT `ToastNotification` with `ToastActivatedEventArgs` query
//! params; the `tulipix:` URI scheme routes back into the app.
//! Linux: `libnotify` actions (`g_signal_connect "action-invoked"`).
//!
//! This module owns the wire shape + per-OS dispatcher; the actual mic-up
//! to `notify-rust` / `winrt-notification` happens at the app layer to
//! keep build matrices small.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub title: String,
    pub body: String,
    pub category: NotificationCategory,
    pub actions: Vec<Action>,
    pub deep_link: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NotificationCategory {
    NowPlaying,
    ScanComplete,
    JobDone,
    SyncConflict,
    Generic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub title: String,
    pub destructive: bool,
}

impl Notification {
    pub fn new(id: impl Into<String>, title: impl Into<String>, body: impl Into<String>) -> Self {
        Self {
            id: id.into(), title: title.into(), body: body.into(),
            category: NotificationCategory::Generic,
            actions: vec![], deep_link: None,
        }
    }
    pub fn category(mut self, c: NotificationCategory) -> Self { self.category = c; self }
    pub fn action(mut self, id: &str, title: &str) -> Self {
        self.actions.push(Action { id: id.into(), title: title.into(), destructive: false }); self
    }
    pub fn destructive(mut self, id: &str, title: &str) -> Self {
        self.actions.push(Action { id: id.into(), title: title.into(), destructive: true }); self
    }
    pub fn deep_link(mut self, url: impl Into<String>) -> Self {
        self.deep_link = Some(url.into()); self
    }
}

pub const SNOOZE_ID: &str = "snooze";
pub const MARK_PLAYED_ID: &str = "played";
pub const DISMISS_ID: &str = "dismiss";
pub const OPEN_ID: &str = "open";

pub trait NotificationSink: Send + Sync {
    fn deliver(&self, n: &Notification) -> Result<()>;
}

pub struct StubSink;
impl NotificationSink for StubSink {
    fn deliver(&self, n: &Notification) -> Result<()> {
        tracing::info!(id = %n.id, category = ?n.category, "notify (stub)");
        Ok(())
    }
}

/// Per-OS deliverer. `tulipix-app` swaps the impl at startup; CI uses StubSink.
pub fn os_sink() -> Box<dyn NotificationSink> {
    Box::new(StubSink)
}

/// Parse a tulipix:// deep-link into (action_id, key=value pairs). Used by
/// the Windows ToastActivatedEventArgs handler and by libnotify action callbacks.
pub fn parse_action_uri(uri: &str) -> Option<(String, Vec<(String, String)>)> {
    let rest = uri.strip_prefix("tulipix://action/")?;
    let (action, query) = rest.split_once('?').unwrap_or((rest, ""));
    let params = query
        .split('&')
        .filter(|s| !s.is_empty())
        .filter_map(|kv| kv.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect();
    Some((action.to_string(), params))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn builder_assembles_actions() {
        let n = Notification::new("nowplaying-1", "Now Playing", "Track")
            .category(NotificationCategory::NowPlaying)
            .action(MARK_PLAYED_ID, "Mark Played")
            .action(SNOOZE_ID, "Snooze")
            .deep_link("tulipix://music/track/42");
        assert_eq!(n.actions.len(), 2);
        assert_eq!(n.deep_link.as_deref(), Some("tulipix://music/track/42"));
    }
    #[test] fn uri_parse_round_trip() {
        let (a, p) = parse_action_uri("tulipix://action/snooze?id=42&minutes=10").unwrap();
        assert_eq!(a, "snooze");
        assert!(p.iter().any(|(k,v)| k == "id" && v == "42"));
        assert!(p.iter().any(|(k,v)| k == "minutes" && v == "10"));
    }
    #[test] fn stub_delivers_ok() {
        StubSink.deliver(&Notification::new("x", "t", "b")).unwrap();
    }
}
