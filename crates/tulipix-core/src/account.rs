//! Local-only app modes. There is no account mode — the app is local at
//! heart: no sign-in, no sync backend, full access on this device. The only
//! modes are Local (normal use) and Locked (lockscreen).

use crate::caps::{self, Tier};
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum AppMode {
    #[default]
    Local,
    Locked,
}


static MODE: RwLock<AppMode> = RwLock::new(AppMode::Local);

type ModeListener = Box<dyn Fn(AppMode) + Send + Sync>;
static LISTENERS: OnceLock<RwLock<Vec<ModeListener>>> = OnceLock::new();
fn listeners() -> &'static RwLock<Vec<ModeListener>> {
    LISTENERS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn on_mode_changed<F: Fn(AppMode) + Send + Sync + 'static>(f: F) {
    listeners().write().unwrap().push(Box::new(f));
}

pub fn current() -> AppMode { *MODE.read().unwrap() }

fn apply_tier(mode: AppMode) {
    let tier = match mode {
        AppMode::Locked => Tier::Guest,
        AppMode::Local => Tier::LocalPro, // local = full access by default
    };
    caps::set_current_tier(tier);
    caps::reset_nudge_dedup();
}

/// Switching never deletes user files. Returns the previous mode.
pub fn set(next: AppMode) -> AppMode {
    let mut m = MODE.write().unwrap();
    let prev = *m;
    *m = next;
    drop(m);
    tracing::info!(?prev, ?next, "app mode switched");
    apply_tier(next);
    for cb in listeners().read().unwrap().iter() { cb(next); }
    prev
}

pub fn lock() { set(AppMode::Locked); }

/// Unlock returns to Local — the only non-locked mode.
pub fn unlock() -> AppMode {
    set(AppMode::Local);
    AppMode::Local
}
