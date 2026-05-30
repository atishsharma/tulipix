use crate::caps::{self, Tier};
use serde::{Deserialize, Serialize};
use std::sync::{OnceLock, RwLock};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AppMode {
    Local,
    Account,
    Locked,
}

impl Default for AppMode {
    fn default() -> Self { Self::Local }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AccountPlan {
    Free,
    Plus,
    Pro,
}

static MODE: RwLock<AppMode> = RwLock::new(AppMode::Local);
static PLAN: RwLock<AccountPlan> = RwLock::new(AccountPlan::Free);
static PREV_NON_LOCKED: RwLock<AppMode> = RwLock::new(AppMode::Local);

type ModeListener = Box<dyn Fn(AppMode) + Send + Sync>;
static LISTENERS: OnceLock<RwLock<Vec<ModeListener>>> = OnceLock::new();
fn listeners() -> &'static RwLock<Vec<ModeListener>> {
    LISTENERS.get_or_init(|| RwLock::new(Vec::new()))
}

pub fn on_mode_changed<F: Fn(AppMode) + Send + Sync + 'static>(f: F) {
    listeners().write().unwrap().push(Box::new(f));
}

pub fn current() -> AppMode { *MODE.read().unwrap() }
pub fn current_plan() -> AccountPlan { *PLAN.read().unwrap() }

pub fn set_plan(plan: AccountPlan) {
    *PLAN.write().unwrap() = plan;
    apply_tier(current(), plan);
}

fn apply_tier(mode: AppMode, plan: AccountPlan) {
    let tier = match mode {
        AppMode::Locked => Tier::Guest,
        AppMode::Local => Tier::LocalPro, // local pro by default — basic only via explicit downgrade
        AppMode::Account => match plan {
            AccountPlan::Free => Tier::AccountFree,
            AccountPlan::Plus => Tier::AccountPlus,
            AccountPlan::Pro => Tier::AccountPro,
        },
    };
    caps::set_current_tier(tier);
    caps::reset_nudge_dedup();
}

/// Switching never deletes user files — proxy model. Caller responsibility:
/// pause scanners → swap mode → resume scanners. Returns previous mode.
pub fn set(next: AppMode) -> AppMode {
    let mut m = MODE.write().unwrap();
    let prev = *m;
    if prev != AppMode::Locked { *PREV_NON_LOCKED.write().unwrap() = prev; }
    *m = next;
    drop(m);
    tracing::info!(?prev, ?next, "app mode switched");
    apply_tier(next, current_plan());
    for cb in listeners().read().unwrap().iter() { cb(next); }
    prev
}

pub fn lock() { set(AppMode::Locked); }

/// Unlock restores the last non-locked mode (Local or Account).
pub fn unlock() -> AppMode {
    let prev = *PREV_NON_LOCKED.read().unwrap();
    set(prev)
}

/// Toggle between Local and Account (no-op while Locked).
pub fn toggle_local_account() -> AppMode {
    let cur = current();
    let next = match cur {
        AppMode::Local => AppMode::Account,
        AppMode::Account => AppMode::Local,
        AppMode::Locked => return cur,
    };
    set(next);
    next
}
