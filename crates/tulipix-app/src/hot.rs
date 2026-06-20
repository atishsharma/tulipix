//! Dev-only Rust hot-patch host glue (feature = "hot").
//!
//! `hot_module` builds a proxy module whose `wire_*` fns dispatch into the
//! `tulipix-hot` dylib (`crates/tulipix-hot`). When `just hot-lib` rebuilds that
//! dylib, hot-lib-reloader reloads the new `.so` and `subscribe()` fires; we
//! then re-run the `wire_*` fns on the Slint event-loop thread, which REPLACES
//! the section's callback closures with the freshly compiled code — no app
//! restart, and tulipix-app/main.rs (the 9.5k-line monolith) never recompiles.
//!
//! `lib_dir` matches CARGO_TARGET_DIR=target-dev from `just hot`; the running
//! exe and the dylib both land in `target-dev/debug/`.

#[hot_lib_reloader::hot_module(dylib = "tulipix_hot", lib_dir = "target-dev/debug")]
pub mod hot {
    // The proxy signatures are regenerated from the dylib's source, so the
    // Slint `MainWindow` type must resolve inside this module too.
    pub use tulipix_ui::MainWindow;
    hot_functions_from_file!("../tulipix-hot/src/lib.rs");

    #[lib_change_subscription]
    pub fn subscribe() -> hot_lib_reloader::LibReloadObserver {}
}

use slint::ComponentHandle;
use tulipix_ui::MainWindow;

/// Wire the hot sections once, then watch for dylib rebuilds and re-wire on each.
pub fn wire_and_watch(window: &MainWindow) {
    hot::wire_tools(window);

    let weak = window.as_weak();
    std::thread::Builder::new()
        .name("tulipix-hot-reload".into())
        .spawn(move || {
            let observer = hot::subscribe();
            loop {
                // Blocks until the rebuilt dylib is loaded; then bounce the
                // re-wire onto the UI thread (Slint callbacks must be set there).
                observer.wait_for_reload();
                let weak = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(win) = weak.upgrade() {
                        hot::wire_tools(&win);
                        tracing::info!("hot: re-wired section callbacks");
                    }
                });
            }
        })
        .expect("spawn tulipix-hot-reload watcher");
}
