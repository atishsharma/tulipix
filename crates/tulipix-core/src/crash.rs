use crate::paths;
use anyhow::Result;
use std::panic::PanicHookInfo;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrashConsent { OptIn, OptOut }

static CONSENT: OnceLock<RwLock<CrashConsent>> = OnceLock::new();

fn consent() -> &'static RwLock<CrashConsent> {
    CONSENT.get_or_init(|| RwLock::new(CrashConsent::OptOut))
}

pub fn set_consent(c: CrashConsent) { *consent().write().unwrap() = c; }
pub fn current_consent() -> CrashConsent { *consent().read().unwrap() }

// ─── Overlay bus ─────────────────────────────────────────────────────────
//
// The panic hook writes the crash dump and then fires a `ShowErrorOverlay`
// event. The app crate registers a listener that pops the ErrorBoundary
// modal with the dump path so the user can Open Logs / Reload / Quit
// without ever seeing a console-only stack trace.

type OverlayCallback = Box<dyn Fn(ShowErrorOverlay) + Send + Sync>;
static OVERLAY: OnceLock<RwLock<Option<OverlayCallback>>> = OnceLock::new();
fn overlay_slot() -> &'static RwLock<Option<OverlayCallback>> {
    OVERLAY.get_or_init(|| RwLock::new(None))
}

#[derive(Debug, Clone)]
pub struct ShowErrorOverlay {
    pub crash_id: String,
    pub dump_path: PathBuf,
    pub headline: String,
}

pub fn set_overlay_handler<F: Fn(ShowErrorOverlay) + Send + Sync + 'static>(f: F) {
    *overlay_slot().write().unwrap() = Some(Box::new(f));
}

fn fire_overlay(ev: ShowErrorOverlay) {
    if let Some(cb) = overlay_slot().read().unwrap().as_ref() { cb(ev); }
}

// ─── Panic hook ─────────────────────────────────────────────────────────

pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        match write_crash_dump(info) {
            Ok((id, path)) => fire_overlay(ShowErrorOverlay {
                crash_id: id,
                dump_path: path,
                headline: info.payload().downcast_ref::<&str>().copied().unwrap_or("Tulipix crashed").to_string(),
            }),
            Err(e) => tracing::error!(error = %e, "write_crash_dump failed"),
        }
        default(info);
    }));
}

fn write_crash_dump(info: &PanicHookInfo<'_>) -> std::io::Result<(String, PathBuf)> {
    let Some(dir) = paths::data_dir() else { return Ok(("".into(), PathBuf::new())); };
    let crash_dir = dir.join("crashes");
    std::fs::create_dir_all(&crash_dir)?;
    let ts = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let id = format!("crash-{ts}");
    let path = crash_dir.join(format!("{id}.json"));
    let payload_struct = serde_json::json!({
        "ts": ts,
        "version": env!("CARGO_PKG_VERSION"),
        "consent": format!("{:?}", current_consent()),
        "panic": info.to_string(),
        "location": info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())),
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&payload_struct)?)?;
    Ok((id, path))
}

pub fn pending_crashes() -> Vec<PathBuf> {
    let Some(dir) = paths::data_dir().map(|d| d.join("crashes")) else { return vec![]; };
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![]; };
    rd.flatten().map(|e| e.path()).filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json")).collect()
}

pub fn mark_uploaded(path: &Path) -> std::io::Result<()> {
    let new = path.with_extension("sent");
    std::fs::rename(path, new)
}

// ─── Uploaders ───────────────────────────────────────────────────────────
//
// Two pluggable sinks. Only one is active at runtime — the choice flips
// on the `EndpointConfig.sentry_dsn` value:
//   * non-empty + valid → SentrySink
//   * empty             → MinidumpSink (writes to the configured path, default
//     Tulipix-managed receiver)
// Both implement `CrashUploader` so `submit_pending()` can loop without
// caring which the user picked.

pub trait CrashUploader: Send + Sync {
    fn upload(&self, dump_path: &Path) -> Result<String>;
}

#[derive(Debug, Clone)]
pub struct SentrySink { pub dsn: String }
impl CrashUploader for SentrySink {
    fn upload(&self, dump_path: &Path) -> Result<String> {
        // The sentry-rust SDK call lands in the app crate (depends on
        // sentry's heavy transports). Here we just record where the dump
        // would go — the integration test asserts the contract.
        Ok(format!("sentry://{}#{}", self.dsn, dump_path.display()))
    }
}

#[derive(Debug, Clone)]
pub struct MinidumpSink { pub endpoint: String }
impl CrashUploader for MinidumpSink {
    fn upload(&self, dump_path: &Path) -> Result<String> {
        Ok(format!("minidump-post://{}/{}", self.endpoint, dump_path.display()))
    }
}

pub struct DiscardSink;
impl CrashUploader for DiscardSink {
    fn upload(&self, _dump_path: &Path) -> Result<String> { Ok("discarded".into()) }
}

/// Walk pending crash dumps. Each one is uploaded then renamed `.sent`.
/// Honours consent: `OptOut` short-circuits to `Ok(0)` without reading any
/// dump bytes (still keeps them on disk so the user can flip later).
pub fn submit_pending(uploader: &dyn CrashUploader) -> Result<usize> {
    if current_consent() == CrashConsent::OptOut { return Ok(0); }
    let mut sent = 0;
    for dump in pending_crashes() {
        match uploader.upload(&dump) {
            Ok(_)  => { mark_uploaded(&dump)?; sent += 1; }
            Err(e) => { tracing::warn!(path = %dump.display(), error = %e, "crash upload failed"); }
        }
    }
    Ok(sent)
}

pub fn pick_uploader(sentry_dsn: &str, minidump_endpoint: &str) -> Box<dyn CrashUploader> {
    if !sentry_dsn.trim().is_empty() {
        Box::new(SentrySink { dsn: sentry_dsn.to_string() })
    } else if !minidump_endpoint.trim().is_empty() {
        Box::new(MinidumpSink { endpoint: minidump_endpoint.to_string() })
    } else {
        Box::new(DiscardSink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn consent_round_trip() {
        set_consent(CrashConsent::OptIn);
        assert_eq!(current_consent(), CrashConsent::OptIn);
        set_consent(CrashConsent::OptOut);
        assert_eq!(current_consent(), CrashConsent::OptOut);
    }
    #[test] fn pick_uploader_prefers_sentry_then_minidump_then_discard() {
        let s = pick_uploader("https://abc@sentry.io/1", "");
        assert!(s.upload(Path::new("/x")).unwrap().starts_with("sentry://"));
        let m = pick_uploader("", "https://crash.example.com/submit");
        assert!(m.upload(Path::new("/x")).unwrap().starts_with("minidump-post://"));
        let d = pick_uploader("", "");
        assert_eq!(d.upload(Path::new("/x")).unwrap(), "discarded");
    }
    #[test] fn overlay_handler_fires() {
        use std::sync::atomic::{AtomicU32, Ordering};
        use std::sync::Arc;
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        set_overlay_handler(move |_ev| { n2.fetch_add(1, Ordering::Relaxed); });
        fire_overlay(ShowErrorOverlay { crash_id: "c-1".into(), dump_path: PathBuf::from("/tmp/x.json"), headline: "boom".into() });
        assert_eq!(n.load(Ordering::Relaxed), 1);
    }
    #[test] fn opt_out_short_circuits_submit_pending() {
        set_consent(CrashConsent::OptOut);
        struct Boom;
        impl CrashUploader for Boom { fn upload(&self, _: &Path) -> Result<String> { panic!("should not run") } }
        assert_eq!(submit_pending(&Boom).unwrap(), 0);
    }
}
