//! Idle detector for Ambient Mode (screensaver).
//!
//! Tulipix doesn't poll the OS for input — instead the UI side calls
//! `mark_active()` on every pointer/key event, and a separate thread checks
//! whether enough wall-clock time has passed without one of those calls. When
//! the threshold is crossed, registered listeners fire (the app then shows
//! `AmbientScreensaver`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::Duration;
use crate::util::unix_secs as now_secs;

const DEFAULT_IDLE_SECS: u64 = 600; // 10 min

static LAST_ACTIVE_EPOCH: AtomicU64 = AtomicU64::new(0);
static THRESHOLD_SECS: AtomicU64 = AtomicU64::new(DEFAULT_IDLE_SECS);
static IDLE_NOW: AtomicU64 = AtomicU64::new(0); // 1 once threshold first crossed

type IdleListener = Box<dyn Fn(bool) + Send + Sync>;
static LISTENERS: OnceLock<RwLock<Vec<IdleListener>>> = OnceLock::new();
fn listeners() -> &'static RwLock<Vec<IdleListener>> {
    LISTENERS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn set_threshold(secs: u64) {
    THRESHOLD_SECS.store(secs.max(5), Ordering::Relaxed);
}

pub fn threshold() -> u64 { THRESHOLD_SECS.load(Ordering::Relaxed) }

pub fn mark_active() {
    LAST_ACTIVE_EPOCH.store(now_secs(), Ordering::Relaxed);
    if IDLE_NOW.swap(0, Ordering::Relaxed) == 1 {
        for cb in listeners().read().unwrap().iter() { cb(false); }
    }
}

pub fn on_change<F: Fn(bool) + Send + Sync + 'static>(f: F) {
    listeners().write().unwrap().push(Box::new(f));
}

pub fn is_idle() -> bool { IDLE_NOW.load(Ordering::Relaxed) == 1 }

/// Seconds since the last input, regardless of the auto-lock threshold.
///
/// `is_idle()` answers "has the *auto-lock* threshold been crossed", and that
/// threshold is a user setting which defaults to off — parked a year out. A
/// consumer with its own idea of idle (the background indexer waits ten
/// minutes) has to measure the interval itself rather than ask a question
/// phrased in terms of somebody else's deadline.
///
/// Zero until the first `mark_active`, so a session that has seen no input yet
/// reads as busy rather than as idle since the epoch.
pub fn idle_secs() -> u64 {
    let last = LAST_ACTIVE_EPOCH.load(Ordering::Relaxed);
    if last == 0 { return 0; }
    now_secs().saturating_sub(last)
}

/// Pure tick — exposed for tests + caller-owned timers. Returns the new state.
pub fn tick(now: u64) -> bool {
    let last = LAST_ACTIVE_EPOCH.load(Ordering::Relaxed);
    if last == 0 { LAST_ACTIVE_EPOCH.store(now, Ordering::Relaxed); return false; }
    let elapsed = now.saturating_sub(last);
    let cross = elapsed >= threshold();
    let prev = IDLE_NOW.load(Ordering::Relaxed) == 1;
    if cross != prev {
        IDLE_NOW.store(if cross { 1 } else { 0 }, Ordering::Relaxed);
        for cb in listeners().read().unwrap().iter() { cb(cross); }
    }
    cross
}

/// Longest this thread will sleep. The threshold can change under it —
/// Settings › Security toggles auto-lock at runtime — so it must come back
/// often enough to notice a *shortened* deadline. Half a minute is well inside
/// the 5 s floor `set_threshold` enforces being meaningful.
const MAX_SLEEP: Duration = Duration::from_secs(30);

/// Spawn the idle tracker thread.
///
/// It sleeps until the next moment the state could actually change rather than
/// waking once a second. `mark_active` only ever pushes the deadline *later*,
/// so sleeping the whole remaining time can never miss a crossing; and the
/// idle→active edge is fired by `mark_active` itself, not detected here.
///
/// This matters because auto-lock is off by default, which parks the threshold
/// a year out — the old 1 Hz loop then woke 86,400 times a day to compare
/// against a deadline in 2027.
pub fn spawn_tracker() {
    std::thread::Builder::new()
        .name("tulipix-idle".into())
        .spawn(|| loop {
            let now = now_secs();
            let crossed = tick(now);
            // Already idle: nothing further to detect until input arrives, and
            // input comes in through `mark_active`. Otherwise wait out whatever
            // is left of the threshold.
            let sleep = if crossed {
                MAX_SLEEP
            } else {
                let last = LAST_ACTIVE_EPOCH.load(Ordering::Relaxed);
                let elapsed = now.saturating_sub(last);
                Duration::from_secs(threshold().saturating_sub(elapsed).max(1))
            };
            std::thread::sleep(sleep.min(MAX_SLEEP));
        })
        .expect("spawn idle tracker");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reset() {
        LAST_ACTIVE_EPOCH.store(0, Ordering::Relaxed);
        IDLE_NOW.store(0, Ordering::Relaxed);
        THRESHOLD_SECS.store(DEFAULT_IDLE_SECS, Ordering::Relaxed);
    }

    #[test]
    fn idle_after_threshold() {
        reset();
        set_threshold(60);
        mark_active();
        // simulate 61s later
        let last = LAST_ACTIVE_EPOCH.load(Ordering::Relaxed);
        assert!(!tick(last + 30));
        assert!(tick(last + 61));
        assert!(is_idle());
    }

    #[test]
    fn mark_active_clears_idle() {
        reset();
        set_threshold(10);
        mark_active();
        let last = LAST_ACTIVE_EPOCH.load(Ordering::Relaxed);
        tick(last + 11);
        assert!(is_idle());
        mark_active();
        assert!(!is_idle());
    }
}
