//! Periodic rescan scheduler. Realtime notify (`crate::watcher`) is always on;
//! this module fires *full* rescans per library based on the cadence stored in
//! `LibrariesConfig`. NFS/SMB and sleep edges are where this matters — notify
//! events get dropped, so the cadence backstop guarantees eventual consistency.

use crate::libraries::{LibrariesConfig, Library, ScanCadence};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    Fire,
    Skip,
}

fn cadence_interval(c: ScanCadence) -> Option<Duration> {
    match c {
        ScanCadence::Manual => None,
        ScanCadence::Hourly => Some(Duration::from_secs(3_600)),
        ScanCadence::Daily => Some(Duration::from_secs(86_400)),
        ScanCadence::Weekly => Some(Duration::from_secs(86_400 * 7)),
    }
}

pub fn due_now(lib: &Library, default_cadence: ScanCadence, now: SystemTime) -> Tick {
    let cadence = lib.cadence_override.unwrap_or(default_cadence);
    let Some(interval) = cadence_interval(cadence) else { return Tick::Skip; };
    let Some(last) = lib.last_scan else { return Tick::Fire; };
    match now.duration_since(last) {
        Ok(elapsed) if elapsed >= interval => Tick::Fire,
        _ => Tick::Skip,
    }
}

type FireFn = Arc<dyn Fn(&str) + Send + Sync>;
static FIRE: OnceLock<Mutex<Option<FireFn>>> = OnceLock::new();
fn fire_slot() -> &'static Mutex<Option<FireFn>> {
    FIRE.get_or_init(|| Mutex::new(None))
}
pub fn set_fire<F: Fn(&str) + Send + Sync + 'static>(f: F) {
    *fire_slot().lock().unwrap() = Some(Arc::new(f));
}

/// Single tick: enumerate libraries, fire rescan for any due. Caller owns the
/// timer (tokio interval or simple thread sleep) — keeping this side-effect-free
/// makes the scheduler unit-testable.
pub fn run_tick(cfg: &LibrariesConfig, now: SystemTime) -> Vec<String> {
    let mut fired = Vec::new();
    let cb = fire_slot().lock().unwrap().clone();
    for lib in &cfg.libraries {
        if matches!(due_now(lib, cfg.default_cadence, now), Tick::Fire) {
            fired.push(lib.id.clone());
            if let Some(ref f) = cb { f(&lib.id); }
        }
    }
    fired
}

/// In-process tick budget — keeps a per-library `Instant` of last fire so a
/// burst of ticks within the same cadence window does not re-fire.
#[derive(Default)]
pub struct TickBudget {
    last_fire: HashMap<String, Instant>,
}

impl TickBudget {
    pub fn allow(&mut self, id: &str, cadence: ScanCadence) -> bool {
        let Some(interval) = cadence_interval(cadence) else { return false; };
        let now = Instant::now();
        match self.last_fire.get(id) {
            Some(prev) if now.duration_since(*prev) < interval => false,
            _ => { self.last_fire.insert(id.to_string(), now); true }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, SystemTime};

    fn mk(id: &str, last: Option<SystemTime>, ov: Option<ScanCadence>) -> Library {
        Library {
            id: id.into(),
            path: PathBuf::from("/tmp/x"),
            section: crate::libraries::Section::Photos,
            last_scan: last,
            item_count: 0,
            size_bytes: 0,
            exclude_globs: vec![],
            cadence_override: ov,
            realtime_notify: true,
        }
    }

    #[test]
    fn manual_never_fires() {
        let lib = mk("a", None, Some(ScanCadence::Manual));
        assert_eq!(due_now(&lib, ScanCadence::Daily, SystemTime::now()), Tick::Skip);
    }

    #[test]
    fn never_scanned_fires_first_tick() {
        let lib = mk("a", None, None);
        assert_eq!(due_now(&lib, ScanCadence::Daily, SystemTime::now()), Tick::Fire);
    }

    #[test]
    fn daily_holds_within_window() {
        let now = SystemTime::now();
        let lib = mk("a", Some(now - Duration::from_secs(60)), None);
        assert_eq!(due_now(&lib, ScanCadence::Daily, now), Tick::Skip);
    }

    #[test]
    fn daily_fires_after_window() {
        let now = SystemTime::now();
        let lib = mk("a", Some(now - Duration::from_secs(86_401)), None);
        assert_eq!(due_now(&lib, ScanCadence::Daily, now), Tick::Fire);
    }

    #[test]
    fn override_beats_default() {
        let now = SystemTime::now();
        let lib = mk("a", Some(now - Duration::from_secs(3_601)), Some(ScanCadence::Hourly));
        assert_eq!(due_now(&lib, ScanCadence::Weekly, now), Tick::Fire);
    }
}
