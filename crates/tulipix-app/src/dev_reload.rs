//! Dev-only Slint hot-reload watcher.
//!
//! Watches the workspace `ui/` tree for `.slint` changes and emits a tracing
//! event tagged `slint_reload` so any external runner (`cargo watch -x run`,
//! `bacon`) can act on it. Slint embeds `.slint` files at compile time via
//! `slint::include_modules!()`, so a live in-process swap is not possible — the
//! watcher's job is to make the dev loop observable.

use notify::{Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub fn spawn() {
    thread::Builder::new()
        .name("tulipix-dev-reload".into())
        .spawn(run)
        .expect("spawn dev-reload watcher");
}

fn workspace_root() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest.join("..").join("..").canonicalize().ok()
}

fn run() {
    let Some(root) = workspace_root() else {
        tracing::warn!("dev-reload: workspace root not found; watcher idle");
        return;
    };
    let ui = root.join("ui");
    let caps_path = root.join("resources/capabilities.toml");
    let caps_override = crate::dirs_default().map(|d| d.join("capabilities.local.toml"));

    let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
    let mut watcher = match RecommendedWatcher::new(tx, notify::Config::default()) {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(error = %e, "dev-reload: watcher create failed");
            return;
        }
    };
    if ui.exists() {
        if let Err(e) = watcher.watch(&ui, RecursiveMode::Recursive) {
            tracing::warn!(error = %e, path = %ui.display(), "dev-reload: ui watch failed");
        } else {
            tracing::info!(path = %ui.display(), "dev-reload: watching ui/");
        }
    }
    if caps_path.exists() {
        if let Err(e) = watcher.watch(&caps_path, RecursiveMode::NonRecursive) {
            tracing::warn!(error = %e, path = %caps_path.display(), "dev-reload: caps watch failed");
        } else {
            tracing::info!(path = %caps_path.display(), "dev-reload: watching capabilities.toml");
        }
    }

    let debounce = Duration::from_millis(150);
    let mut last_fire = Instant::now() - debounce;
    while let Ok(evt) = rx.recv() {
        let Ok(evt) = evt else { continue };
        if !matches!(evt.kind, EventKind::Modify(_) | EventKind::Create(_) | EventKind::Remove(_)) { continue; }
        if last_fire.elapsed() < debounce { continue; }
        last_fire = Instant::now();

        let touched_caps = evt.paths.iter().any(|p| same_file(p, &caps_path));
        if touched_caps {
            match tulipix_core::caps::reload_from_disk(&caps_path, caps_override.as_deref()) {
                Ok(()) => tracing::info!(target: "caps_reload", "capabilities.toml reloaded"),
                Err(e) => tracing::warn!(error = %e, "caps reload failed"),
            }
            continue;
        }
        if evt.paths.iter().any(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("slint"))) {
            let names: Vec<String> = evt
                .paths
                .iter()
                .filter_map(|p| p.file_name().and_then(|n| n.to_str()).map(|s| s.to_string()))
                .collect();
            tracing::info!(target: "slint_reload", files = ?names, "ui changed — rerun cargo to pick up");
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.canonicalize().ok() == b.canonicalize().ok()
}
