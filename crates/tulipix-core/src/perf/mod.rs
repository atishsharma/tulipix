//! Performance subsystem — frame pacing, jank telemetry, viewport prefetch,
//! IO + GPU + memory budgets. All modules are headless / no-Slint so they
//! can be unit-tested without spinning up a window.

pub mod vrr;
pub mod jank;
pub mod prefetch;
pub mod offthread;
pub mod memory;
pub mod io_throttle;
pub mod gpu_budget;
