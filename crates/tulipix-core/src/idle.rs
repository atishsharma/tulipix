//! Idle detector for Ambient Mode (screensaver).
//!
//! Tulipix doesn't poll the OS for input — instead the UI side calls
//! `mark_active()` on every pointer/key event, and a separate thread checks
//! whether enough wall-clock time has passed without one of those calls. When
//! the threshold is crossed, registered listeners fire (the app then shows
//! `AmbientScreensaver`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};
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

/// Spawn the idle tracker thread. Caller passes a clock-tick interval (1s is
/// fine — cheap), and we just poll the wall clock.
pub fn spawn_tracker() {
    std::thread::Builder::new()
        .name("tulipix-idle".into())
        .spawn(|| {
            let interval = Duration::from_secs(1);
            let mut last_tick = Instant::now();
            loop {
                let now = now_secs();
                tick(now);
                let elapsed = last_tick.elapsed();
                if elapsed < interval { std::thread::sleep(interval - elapsed); }
                last_tick = Instant::now();
            }
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
