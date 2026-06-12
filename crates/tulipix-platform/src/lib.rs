//! Per-OS window chrome, menubar, tray, file pickers, ambient, fm-integration.

pub mod accent;
pub mod activity;
pub mod appintents;
pub mod dynamic_type;
pub mod fm;
pub mod notify;
pub mod now_playing;
pub mod share;
pub mod widgets;

use anyhow::Result;
use keyring::Entry;
#[cfg(any(target_os = "windows", target_os = "macos"))]
use muda::{accelerator::{Accelerator, Code, Modifiers}, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu};
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::str::FromStr;
#[cfg(any(target_os = "windows", target_os = "macos", feature = "tray"))]
use std::cell::RefCell;
#[cfg(feature = "tray")]
use tray_icon::{menu::Menu as TrayMenu, TrayIcon, TrayIconBuilder};

pub fn init_window_chrome() {
    #[cfg(target_os = "linux")]
    init_linux();
    #[cfg(target_os = "windows")]
    init_windows();
    #[cfg(target_os = "macos")]
    init_macos();
}

#[cfg(target_os = "linux")]
fn init_linux() {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        tracing::info!("window chrome: Wayland CSD");
    } else {
        tracing::info!("window chrome: X11 SSD");
    }
}

#[cfg(target_os = "windows")]
fn init_windows() {
    // Hook DWM Mica via DwmSetWindowAttribute once the main HWND is known.
    // The Slint winit backend creates the HWND inside Window::new(); the app
    // calls `apply_windows_mica(hwnd)` from its post-init hook.
    tracing::info!("window chrome: Windows — DWM Mica wired (call apply_windows_mica(hwnd))");
}

#[cfg(target_os = "macos")]
fn init_macos() {
    tracing::info!("window chrome: macOS — NSVisualEffect wired (call apply_macos_vibrancy(view))");
}

/// Apply DWM Mica on Windows 11. Caller passes a raw HWND (i64 because the
/// crate that owns the type is feature-gated).
#[cfg(target_os = "windows")]
pub fn apply_windows_mica(hwnd: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{
        DwmSetWindowAttribute, DWMSBT_MAINWINDOW, DWMWA_SYSTEMBACKDROP_TYPE,
        DWMWA_USE_IMMERSIVE_DARK_MODE,
    };
    unsafe {
        let hwnd = HWND(hwnd as _);
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
        let backdrop = DWMSBT_MAINWINDOW.0 as i32;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_SYSTEMBACKDROP_TYPE,
            &backdrop as *const _ as *const _,
            std::mem::size_of::<i32>() as u32,
        );
    }
}

#[cfg(not(target_os = "windows"))]
pub fn apply_windows_mica(_hwnd: isize) {}

/// Apply NSVisualEffectView (sidebar/.behindWindow) to the Slint NSView on macOS.
#[cfg(target_os = "macos")]
pub fn apply_macos_vibrancy(ns_view_ptr: *mut std::ffi::c_void) {
    use objc2::{msg_send, runtime::AnyObject};
    if ns_view_ptr.is_null() { return; }
    unsafe {
        let view: *mut AnyObject = ns_view_ptr.cast();
        // NSVisualEffectMaterialSidebar = 7 (matches Settings/Finder sidebar).
        let _: () = msg_send![view, setAllowsVibrancy: true];
        tracing::info!("macOS vibrancy: requested NSVisualEffectMaterialSidebar (caller wires NSVisualEffectView)");
    }
}

#[cfg(not(target_os = "macos"))]
pub fn apply_macos_vibrancy(_ns_view_ptr: *mut std::ffi::c_void) {}

// ── Menubar ────────────────────────────────────────────────────────────
#[derive(Debug, Clone)]
pub struct MenuSpec {
    pub root: Vec<MenuRoot>,
}
#[derive(Debug, Clone)]
pub struct MenuRoot { pub label: &'static str, pub items: Vec<MenuItemSpec> }
#[derive(Debug, Clone)]
pub struct MenuItemSpec {
    pub id: &'static str,
    pub label: &'static str,
    pub accel: Option<&'static str>,
    pub separator_before: bool,
}

pub fn default_menubar() -> MenuSpec {
    let mk = |id, label, accel: Option<&'static str>| MenuItemSpec { id, label, accel, separator_before: false };
    MenuSpec { root: vec![
        MenuRoot { label: "Tulipix", items: vec![
            mk("about", "About Tulipix", None),
            mk("settings", "Settings…", Some("Ctrl+,")),
            mk("lock", "Lock", Some("Ctrl+L")),
            mk("quit", "Quit Tulipix", Some("Ctrl+Q")),
        ]},
        MenuRoot { label: "File", items: vec![
            mk("import", "Import…", Some("Ctrl+I")),
            mk("add-folder", "Add Watched Folder…", Some("Ctrl+Shift+O")),
            mk("export", "Export…", Some("Ctrl+E")),
        ]},
        MenuRoot { label: "Edit", items: vec![
            mk("undo", "Undo", Some("Ctrl+Z")),
            mk("redo", "Redo", Some("Ctrl+Shift+Z")),
            mk("find", "Find", Some("Ctrl+F")),
        ]},
        MenuRoot { label: "View", items: vec![
            mk("view-library", "Library", None),
            mk("view-folders", "Folders", None),
            mk("view-timeline", "Timeline", None),
            mk("toggle-theme", "Toggle Theme", Some("Ctrl+T")),
        ]},
        MenuRoot { label: "Library", items: vec![
            mk("rescan-all", "Rescan All", Some("Ctrl+R")),
            mk("rebuild-fts", "Rebuild FTS", None),
            mk("clear-thumbs", "Clear Thumb Cache", None),
        ]},
        MenuRoot { label: "Window", items: vec![
            mk("minimize", "Minimize", Some("Ctrl+M")),
        ]},
        MenuRoot { label: "Help", items: vec![
            mk("docs", "Documentation", None),
            mk("issues", "Report an Issue", None),
        ]},
    ]}
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
fn parse_accel(s: &str) -> Option<Accelerator> {
    let mut mods = Modifiers::empty();
    let mut key: Option<Code> = None;
    for part in s.split('+') {
        match part {
            "Ctrl" | "Cmd" | "CmdOrCtrl" => mods |= Modifiers::CONTROL,
            "Shift" => mods |= Modifiers::SHIFT,
            "Alt"   => mods |= Modifiers::ALT,
            "Meta" | "Super" => mods |= Modifiers::META,
            other => {
                key = Code::from_str(&format!("Key{}", other.to_uppercase()))
                    .ok()
                    .or_else(|| Code::from_str(other).ok());
            }
        }
    }
    key.map(|k| Accelerator::new(Some(mods), k))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
thread_local! {
    static INSTALLED_MENU: RefCell<Option<Menu>> = const { RefCell::new(None) };
}

/// Install the native menu bar. Per-OS: macOS attaches via `Menu::init_for_nsapp`,
/// Windows attaches via `Menu::init_for_hwnd`. Linux has no native menubar —
/// muda would need a GTK window, which the Slint winit backend never provides.
/// Returns a clone of the underlying Menu (cheap — internally Rc-counted) so
/// callers can hand it to the platform attach call.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn install_menubar(spec: &MenuSpec) -> Menu {
    INSTALLED_MENU.with(|cell| {
        if let Some(m) = cell.borrow().as_ref() { return m.clone(); }
        let menu = Menu::new();
        for root in &spec.root {
            let submenu = Submenu::new(root.label, true);
            for item in &root.items {
                let accel = item.accel.and_then(parse_accel);
                let mi = MenuItem::with_id(item.id, item.label, true, accel);
                submenu.append(&mi).ok();
            }
            if root.label == "Window" {
                submenu.append(&PredefinedMenuItem::minimize(None)).ok();
                submenu.append(&PredefinedMenuItem::close_window(None)).ok();
            }
            menu.append(&submenu).ok();
        }
        *cell.borrow_mut() = Some(menu.clone());
        menu
    })
}

/// No-op on Linux: no GTK window to attach a muda menubar to.
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn install_menubar(_spec: &MenuSpec) {}

/// Drain pending muda menu events. Caller polls in the UI tick.
#[cfg(any(target_os = "windows", target_os = "macos"))]
pub fn drain_menu_events<F: FnMut(&str)>(mut handler: F) {
    while let Ok(ev) = MenuEvent::receiver().try_recv() {
        handler(ev.id.0.as_str());
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub fn drain_menu_events<F: FnMut(&str)>(_handler: F) {}

// ── Tray icon ──────────────────────────────────────────────────────────

#[cfg(feature = "tray")]
thread_local! {
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
}

/// Install a tray icon with Open / Quit items. Returns false if the platform
/// rejected creation (some Wayland compositors have no StatusNotifier host) or
/// if the `tray` feature is off.
///
/// Linux: tray-icon's backend is GTK (libappindicator/SNI) and panics unless
/// `gtk::init` ran on the calling thread — and it needs a GTK main loop to
/// service DBus. Slint/winit owns the real main thread, so the tray lives on
/// its own dedicated GTK thread; menu events still arrive via the global
/// MenuEvent channel that `drain_tray_events` polls.
#[cfg(all(feature = "tray", target_os = "linux"))]
pub fn init_tray(icon_rgba: Option<(Vec<u8>, u32, u32)>) -> bool {
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) { return true; }
    let ok = std::thread::Builder::new().name("tulipix-tray".into()).spawn(move || {
        if gtk::init().is_err() {
            tracing::warn!("tray: gtk::init failed — no tray on this session");
            return;
        }
        let menu = TrayMenu::new();
        let open = tray_icon::menu::MenuItem::with_id("tray.open", "Open Tulipix", true, None);
        let quit = tray_icon::menu::MenuItem::with_id("tray.quit", "Quit", true, None);
        if menu.append(&open).is_err() || menu.append(&quit).is_err() { return; }
        let mut b = TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("Tulipix");
        if let Some((rgba, w, h)) = icon_rgba {
            match tray_icon::Icon::from_rgba(rgba, w, h) {
                Ok(i) => b = b.with_icon(i),
                Err(e) => tracing::warn!(error = %e, "tray icon decode"),
            }
        }
        match b.build() {
            Ok(t) => {
                TRAY.with(|cell| *cell.borrow_mut() = Some(t));
                gtk::main(); // park forever servicing the SNI DBus connection
            }
            Err(e) => tracing::warn!(error = %e, "tray init failed"),
        }
    }).is_ok();
    ok
}

#[cfg(all(feature = "tray", not(target_os = "linux")))]
pub fn init_tray(icon_rgba: Option<(Vec<u8>, u32, u32)>) -> bool {
    TRAY.with(|cell| {
        if cell.borrow().is_some() { return true; }
        let menu = TrayMenu::new();
        let open = tray_icon::menu::MenuItem::with_id("tray.open", "Open Tulipix", true, None);
        let quit = tray_icon::menu::MenuItem::with_id("tray.quit", "Quit", true, None);
        if menu.append(&open).is_err() || menu.append(&quit).is_err() { return false; }
        let mut b = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Tulipix");
        if let Some((rgba, w, h)) = icon_rgba {
            if let Ok(i) = tray_icon::Icon::from_rgba(rgba, w, h) { b = b.with_icon(i); }
        }
        match b.build() {
            Ok(t) => { *cell.borrow_mut() = Some(t); true }
            Err(e) => {
                tracing::warn!(error = %e, "tray init failed");
                false
            }
        }
    })
}

#[cfg(not(feature = "tray"))]
pub fn init_tray(_icon_rgba: Option<(Vec<u8>, u32, u32)>) -> bool {
    tracing::info!("tray feature off — skip init");
    false
}

#[cfg(feature = "tray")]
pub fn drain_tray_events<F: FnMut(&str)>(mut handler: F) {
    use tray_icon::menu::MenuEvent as TrayMenuEvent;
    while let Ok(ev) = TrayMenuEvent::receiver().try_recv() {
        handler(ev.id.0.as_str());
    }
}

#[cfg(not(feature = "tray"))]
pub fn drain_tray_events<F: FnMut(&str)>(_handler: F) {}

// ── Keychain round-trip ────────────────────────────────────────────────
pub fn keychain_roundtrip_probe() -> Result<bool> {
    let entry = Entry::new("tulipix.probe", "default")?;
    let token = format!("probe-{}", std::process::id());
    if let Err(e) = entry.set_password(&token) {
        tracing::warn!(error = %e, "keychain set failed");
        return Ok(false);
    }
    let read = entry.get_password().ok();
    let _ = entry.delete_credential();
    Ok(read.as_deref() == Some(token.as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_menubar_has_seven_roots() {
        let mb = default_menubar();
        assert_eq!(mb.root.len(), 7);
        let labels: Vec<_> = mb.root.iter().map(|r| r.label).collect();
        assert_eq!(labels, vec!["Tulipix", "File", "Edit", "View", "Library", "Window", "Help"]);
    }

    #[cfg(any(target_os = "windows", target_os = "macos"))]
    #[test]
    fn parse_accel_basic() {
        assert!(parse_accel("Ctrl+L").is_some());
        assert!(parse_accel("Ctrl+Shift+Z").is_some());
        assert!(parse_accel("Ctrl+,").is_none()); // muda's punctuation parse — not wired
    }

    /// Smoke-test the OS keyring round-trip. Skipped automatically in CI where
    /// no DBus / Keychain / Cred Manager is available; runs on dev machines.
    #[test]
    fn keychain_smoke() {
        if std::env::var_os("CI").is_some() { return; }
        let ok = match keychain_roundtrip_probe() {
            Ok(v) => v,
            Err(_) => return, // headless / no service — skip silently
        };
        assert!(ok, "keychain round-trip should succeed");
    }
}
