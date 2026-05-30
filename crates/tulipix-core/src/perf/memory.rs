//! Memory budget tracker. Each major mode (idle, scan, playback) has a
//! per-platform RSS budget. The watchdog runs once per second; sustained
//! breach over 30 s lights up Settings → Diagnostics → Performance with a
//! warning + suggested action.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode { Idle, Scan, Playback }

impl Mode {
    /// Budget in bytes.
    pub fn budget_bytes(self) -> u64 {
        match self {
            Mode::Idle     =>   300 * 1024 * 1024,
            Mode::Scan     => 1_024 * 1024 * 1024,
            Mode::Playback =>   500 * 1024 * 1024,
        }
    }
}

static CURRENT_MODE: AtomicU64 = AtomicU64::new(0); // 0 = Idle, 1 = Scan, 2 = Playback
static LAST_OK_AT_S: AtomicU64 = AtomicU64::new(0);
static SUSTAINED_BREACH_S: AtomicU64 = AtomicU64::new(0);

pub fn set_mode(m: Mode) {
    CURRENT_MODE.store(match m { Mode::Idle => 0, Mode::Scan => 1, Mode::Playback => 2 }, Ordering::Relaxed);
}
pub fn mode() -> Mode {
    match CURRENT_MODE.load(Ordering::Relaxed) { 1 => Mode::Scan, 2 => Mode::Playback, _ => Mode::Idle }
}

/// Caller probes process RSS (sysinfo / mach_task_info / GetProcessMemoryInfo)
/// and calls this once per second. Returns the current sustained-breach
/// duration in seconds (0 = within budget).
pub fn tick(now: Instant, start: Instant, rss_bytes: u64) -> u64 {
    let budget = mode().budget_bytes();
    let now_s = now.duration_since(start).as_secs();
    if rss_bytes <= budget {
        LAST_OK_AT_S.store(now_s, Ordering::Relaxed);
        SUSTAINED_BREACH_S.store(0, Ordering::Relaxed);
        return 0;
    }
    let last_ok = LAST_OK_AT_S.load(Ordering::Relaxed);
    let breach = now_s.saturating_sub(last_ok);
    SUSTAINED_BREACH_S.store(breach, Ordering::Relaxed);
    breach
}

pub fn breach_threshold() -> Duration { Duration::from_secs(30) }
pub fn sustained_breach() -> u64 { SUSTAINED_BREACH_S.load(Ordering::Relaxed) }

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    static SERIAL: Mutex<()> = Mutex::new(());

    #[test] fn budgets_match() {
        assert_eq!(Mode::Idle.budget_bytes(), 300 * 1024 * 1024);
        assert_eq!(Mode::Playback.budget_bytes(), 500 * 1024 * 1024);
        assert_eq!(Mode::Scan.budget_bytes(), 1024 * 1024 * 1024);
    }
    #[test] fn within_budget_no_breach() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_mode(Mode::Idle);
        let start = Instant::now();
        for i in 0..5 {
            let now = start + Duration::from_secs(i);
            assert_eq!(tick(now, start, 100 * 1024 * 1024), 0);
        }
    }
    #[test] fn sustained_breach_climbs() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        set_mode(Mode::Playback);
        let start = Instant::now();
        let _ = tick(start, start, 100 * 1024 * 1024); // reset last_ok
        let b5 = tick(start + Duration::from_secs(5), start, 600 * 1024 * 1024);
        let b10 = tick(start + Duration::from_secs(10), start, 600 * 1024 * 1024);
        assert!(b5 >= 5);
        assert!(b10 >= 10);
    }
}
