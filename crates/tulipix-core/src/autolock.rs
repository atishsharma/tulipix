//! App-wide idle auto-lock. Bridges the idle tracker to the lock action:
//!   * Settings → Security → "Lock after" picks one of the discrete options.
//!   * On threshold crossing we fire the registered lock callback, which
//!     transitions caps to Tier::Guest, stops indexer / transcode / sync
//!     workers, and the UI swaps to the lockscreen.

use crate::idle;
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutoLockTimeout {
    Never,
    OneMinute,
    FiveMinutes,
    FifteenMinutes,
    ThirtyMinutes,
    OneHour,
}

impl AutoLockTimeout {
    pub fn seconds(self) -> Option<u64> {
        match self {
            Self::Never          => None,
            Self::OneMinute      => Some(60),
            Self::FiveMinutes    => Some(300),
            Self::FifteenMinutes => Some(900),
            Self::ThirtyMinutes  => Some(1800),
            Self::OneHour        => Some(3600),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Never => "Never",
            Self::OneMinute => "1 minute",
            Self::FiveMinutes => "5 minutes",
            Self::FifteenMinutes => "15 minutes",
            Self::ThirtyMinutes => "30 minutes",
            Self::OneHour => "1 hour",
        }
    }
}

impl Default for AutoLockTimeout {
    fn default() -> Self { Self::FifteenMinutes }
}

type LockCallback = Box<dyn Fn() + Send + Sync>;
static LOCK_CB: OnceLock<RwLock<Option<LockCallback>>> = OnceLock::new();
fn slot() -> &'static RwLock<Option<LockCallback>> {
    LOCK_CB.get_or_init(|| RwLock::new(None))
}

/// Install the lock-action callback. App layer wires it to:
/// caps tier → Guest, abort indexer + transcode + sync joins, show lockscreen.
pub fn set_lock_handler<F: Fn() + Send + Sync + 'static>(f: F) {
    *slot().write().unwrap() = Some(Box::new(f));
}

/// Apply the timeout. `Never` disables auto-lock (idle threshold pushed
/// to a year so the listener never fires).
pub fn apply(timeout: AutoLockTimeout) {
    match timeout.seconds() {
        Some(s) => idle::set_threshold(s),
        None    => idle::set_threshold(60 * 60 * 24 * 365),
    }
}

/// Subscribe the auto-lock to the idle change stream. Call once at app boot.
pub fn install() {
    idle::on_change(|idle_now| {
        if !idle_now { return; }
        if let Some(cb) = slot().read().unwrap().as_ref() { cb(); }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    #[test] fn seconds_table() {
        assert_eq!(AutoLockTimeout::Never.seconds(), None);
        assert_eq!(AutoLockTimeout::FiveMinutes.seconds(), Some(300));
        assert_eq!(AutoLockTimeout::OneHour.seconds(), Some(3600));
    }
    #[test] fn default_is_15m() {
        assert_eq!(AutoLockTimeout::default(), AutoLockTimeout::FifteenMinutes);
    }
    #[test] fn apply_writes_idle_threshold() {
        apply(AutoLockTimeout::OneMinute);
        assert_eq!(idle::threshold(), 60);
        apply(AutoLockTimeout::Never);
        assert!(idle::threshold() >= 31_000_000);
    }
    #[test] fn lock_handler_round_trip() {
        let n = Arc::new(AtomicU32::new(0));
        let n2 = n.clone();
        set_lock_handler(move || { n2.fetch_add(1, Ordering::Relaxed); });
        if let Some(cb) = slot().read().unwrap().as_ref() { cb(); }
        assert_eq!(n.load(Ordering::Relaxed), 1);
    }
}
