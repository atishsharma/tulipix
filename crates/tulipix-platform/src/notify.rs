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

/// Hands the banner to whatever the OS already ships.
///
/// A subprocess rather than `notify-rust`: on Linux that crate pulls the whole
/// zbus stack for what `notify-send` does in one exec, and this app already
/// shells out to ffmpeg, yt-dlp and mpv, so the pattern is not new. Actions and
/// deep links are not carried — `notify-send` cannot express them without a
/// live D-Bus connection to keep the callback alive, so a click opens nothing
/// and the notification is informational only.
pub struct OsSink;

impl NotificationSink for OsSink {
    fn deliver(&self, n: &Notification) -> Result<()> {
        use std::process::{Command, Stdio};

        #[cfg(target_os = "linux")]
        let mut cmd = {
            let mut c = Command::new("notify-send");
            c.args(["-a", "Tulipix", "-i", icon_arg(), n.title.as_str(), n.body.as_str()]);
            c
        };

        #[cfg(target_os = "macos")]
        let mut cmd = {
            // Quotes are escaped rather than interpolated raw: a bill named
            // `Bob"s rent` would otherwise end the AppleScript string early.
            let script = format!(
                "display notification \"{}\" with title \"{}\"",
                esc(&n.body),
                esc(&n.title)
            );
            let mut c = Command::new("osascript");
            c.args(["-e", &script]);
            c
        };

        #[cfg(target_os = "windows")]
        let mut cmd = {
            // WinRT toast through PowerShell. Verbose, but it is the only route
            // that needs nothing installed on a stock Windows 10 or 11.
            let script = format!(
                r#"[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType=WindowsRuntime] > $null
$t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)
$x = $t.GetElementsByTagName('text')
$x.Item(0).AppendChild($t.CreateTextNode('{}')) > $null
$x.Item(1).AppendChild($t.CreateTextNode('{}')) > $null
[Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('Tulipix').Show([Windows.UI.Notifications.ToastNotification]::new($t))"#,
                esc_ps(&n.title),
                esc_ps(&n.body)
            );
            let mut c = Command::new("powershell");
            c.args(["-NoProfile", "-NonInteractive", "-Command", &script]);
            c
        };

        // Fire and forget. Waiting on the notification daemon would block the
        // caller for as long as the banner is on screen on some desktops.
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map(|_| ())
            .map_err(anyhow::Error::from)
    }
}

#[cfg(target_os = "macos")]
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(target_os = "windows")]
fn esc_ps(s: &str) -> String {
    s.replace('\'', "''")
}

static ICON: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// Point the banners at the app's own mark, by file path.
///
/// Without this the Linux sink asks for the icon-theme name `tulipix`, which exists
/// only once the app has been installed into a theme directory — so an uninstalled
/// build shows the theme's broken-image placeholder instead of a logo. The app knows
/// where its icon is on disk; this module does not, and should not have to embed a
/// second copy of it to find out.
pub fn set_icon(path: impl Into<std::path::PathBuf>) {
    let _ = ICON.set(path.into());
}

/// The icon to hand the desktop: the file if one was named, otherwise the theme name
/// an installed build can rely on.
#[cfg(target_os = "linux")]
fn icon_arg() -> &'static str {
    ICON.get().and_then(|p| p.to_str()).unwrap_or("tulipix")
}

static SINK: std::sync::OnceLock<Box<dyn NotificationSink>> = std::sync::OnceLock::new();

/// Install the process-wide deliverer. First call wins; later ones are ignored,
/// which keeps a second call from silently changing behaviour halfway through a
/// run.
pub fn set_sink(sink: Box<dyn NotificationSink>) {
    let _ = SINK.set(sink);
}

/// The installed deliverer, falling back to [`os_sink`].
pub fn sink() -> &'static dyn NotificationSink {
    SINK.get_or_init(os_sink).as_ref()
}

/// Deliver through the installed sink, logging rather than propagating.
///
/// A failed banner must never fail the operation that raised it: a bill is still
/// overdue whether or not the desktop managed to say so.
pub fn notify(n: &Notification) {
    if let Err(e) = sink().deliver(n) {
        tracing::debug!(error = %e, id = %n.id, "notification not delivered");
    }
}

/// Per-OS deliverer. Falls back to the stub where the OS tool is missing —
/// notably a headless CI runner, where `notify-send` exists on no image.
pub fn os_sink() -> Box<dyn NotificationSink> {
    if delivery_available() { Box::new(OsSink) } else { Box::new(StubSink) }
}

fn delivery_available() -> bool {
    // `$DISPLAY`-less Linux has no one to show a banner to, and spawning
    // notify-send there writes an error to a log nobody reads.
    #[cfg(target_os = "linux")]
    {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return false;
        }
        which("notify-send")
    }
    #[cfg(not(target_os = "linux"))]
    {
        true
    }
}

#[cfg(target_os = "linux")]
fn which(bin: &str) -> bool {
    std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(bin).is_file()))
        .unwrap_or(false)
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
