//! Jank detection + dropped-frame telemetry.
//!
//! Dev builds: the compositor calls `record_frame(elapsed_ns)` per frame.
//! Frames over the 16 ms budget log a tracing event with the call-tree
//! attached by the caller. Release builds gather a counter exposed in
//! Diagnostics; off by default unless the user opts in.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

static ENABLED: AtomicBool = AtomicBool::new(false);
static BUDGET_NS: AtomicU64 = AtomicU64::new(16_000_000);
static TOTAL_FRAMES: AtomicU64 = AtomicU64::new(0);
static DROPPED_FRAMES: AtomicU64 = AtomicU64::new(0);
static WORST_FRAME_NS: AtomicU64 = AtomicU64::new(0);

pub fn enable(enabled: bool) { ENABLED.store(enabled, Ordering::Relaxed); }
pub fn set_budget(budget: Duration) {
    BUDGET_NS.store(budget.as_nanos() as u64, Ordering::Relaxed);
}

/// Record one composited frame. Caller passes the GPU-side elapsed time.
/// Returns `true` if the frame breached the budget.
pub fn record_frame(elapsed: Duration) -> bool {
    TOTAL_FRAMES.fetch_add(1, Ordering::Relaxed);
    let ns = elapsed.as_nanos() as u64;
    WORST_FRAME_NS.fetch_max(ns, Ordering::Relaxed);
    let budget = BUDGET_NS.load(Ordering::Relaxed);
    let dropped = ns > budget;
    if dropped {
        DROPPED_FRAMES.fetch_add(1, Ordering::Relaxed);
        if ENABLED.load(Ordering::Relaxed) {
            tracing::warn!(elapsed_ns = ns, budget_ns = budget, "jank");
        }
    }
    dropped
}

#[derive(Debug, Clone, Copy)]
pub struct JankStats {
    pub total: u64,
    pub dropped: u64,
    pub worst_ns: u64,
}

impl JankStats {
    pub fn drop_rate(self) -> f32 {
        if self.total == 0 { 0.0 } else { self.dropped as f32 / self.total as f32 }
    }
}

pub fn snapshot() -> JankStats {
    JankStats {
        total: TOTAL_FRAMES.load(Ordering::Relaxed),
        dropped: DROPPED_FRAMES.load(Ordering::Relaxed),
        worst_ns: WORST_FRAME_NS.load(Ordering::Relaxed),
    }
}

pub fn reset() {
    TOTAL_FRAMES.store(0, Ordering::Relaxed);
    DROPPED_FRAMES.store(0, Ordering::Relaxed);
    WORST_FRAME_NS.store(0, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test] fn smooth_frames_no_drops() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        set_budget(Duration::from_millis(16));
        for _ in 0..10 { assert!(!record_frame(Duration::from_millis(8))); }
        let s = snapshot();
        assert_eq!(s.total, 10);
        assert_eq!(s.dropped, 0);
        assert!(s.drop_rate() < 1e-6);
    }
    #[test] fn breached_frames_counted() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        reset();
        set_budget(Duration::from_millis(16));
        for _ in 0..5 { record_frame(Duration::from_millis(25)); }
        let s = snapshot();
        assert_eq!(s.dropped, 5);
        assert!(s.worst_ns >= 25_000_000);
    }
}
