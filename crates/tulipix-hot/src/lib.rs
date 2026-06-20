//! Hot-reloadable section-wiring shim (dev only).
//!
//! Each `#[no_mangle] pub fn wire_*` simply forwards to the matching section
//! crate's `wire(&MainWindow)`. hot-lib-reloader looks these symbols up in the
//! freshly built `.so` on every rebuild, so editing section callback logic and
//! rebuilding *this* dylib re-runs `wire()` in the live app. Slint callback
//! slots are single-occupancy, so re-running `wire()` cleanly REPLACES the old
//! closures with the new code — that is the whole hot-reload trick.
//!
//! Keep these wrappers trivial: no state lives here (the dylib is swapped out
//! wholesale on reload, so any `static` in it would reset). Section state lives
//! in the section crates' own statics / the on-disk DBs, which survive.

use tulipix_ui::MainWindow;

/// Re-register every `window.on_tools_*` callback from tulipix-sec-tools.
#[unsafe(no_mangle)]
pub fn wire_tools(window: &MainWindow) {
    tulipix_sec_tools::wire(window);
}
