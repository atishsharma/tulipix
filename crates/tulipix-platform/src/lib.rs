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
#[cfg(any(target_os = "windows", target_os = "macos"))]
use std::cell::RefCell;
// tray-icon is a Windows/macOS dependency only — Linux uses ksni (see below).
#[cfg(all(feature = "tray", not(target_os = "linux")))]
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

/// What the tray context menu says about playback.
///
/// Pushed from the app with [`update_tray`] whenever the track or transport
/// state changes; the tray re-renders its menu from this. Deliberately without
/// a playback position: the menu would have to be re-published over D-Bus every
/// second to keep a clock honest, and the popup window (the other tray style)
/// already shows one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrayNowPlaying {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    pub has_track: bool,
    pub shuffle: bool,
    /// "off" · "all" · "one"
    pub repeat: String,
    /// Album art as PNG bytes — what a StatusNotifierItem menu row takes
    /// (`icon_data`), already scaled down to menu-icon size by the caller.
    /// Empty means no art, and the row draws without one.
    pub art_png: Vec<u8>,
    /// Tray style is "popup": a left click should open the popup window rather
    /// than raising the main one.
    pub popup: bool,
}

#[cfg(all(feature = "tray", not(target_os = "linux")))]
thread_local! {
    static TRAY: RefCell<Option<TrayIcon>> = const { RefCell::new(None) };
    /// Menu items whose text changes with playback, kept so they can be
    /// relabelled in place — rebuilding the menu would drop it while open.
    static TRAY_ITEMS: RefCell<Option<TrayItems>> = const { RefCell::new(None) };
}

#[cfg(all(feature = "tray", not(target_os = "linux")))]
struct TrayItems {
    now: tray_icon::menu::MenuItem,
    playpause: tray_icon::menu::MenuItem,
    shuffle: tray_icon::menu::MenuItem,
    repeat: tray_icon::menu::MenuItem,
}

// ── Linux tray: StatusNotifierItem over D-Bus ────────────────────────────────
//
// This used to be tray-icon, whose Linux backend is libappindicator and therefore
// GTK: it needed `gtk::init` plus a `gtk::main` loop parked on a dedicated thread
// just to service D-Bus, and it pulled 21 gtk-rs crates into the build (eight of
// them unmaintained) along with a libgtk-3-dev build dependency. ksni speaks the
// same StatusNotifierItem protocol directly over zbus, which the tree already has
// via ashpd, so none of that is needed.
//
// Menu clicks are pushed onto a channel so `drain_tray_events` keeps the exact
// interface the app polls, unchanged.
#[cfg(all(feature = "tray", target_os = "linux"))]
mod linux_tray {
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::sync::{Mutex, OnceLock};

    pub struct TulipixTray {
        pub icon: Vec<ksni::Icon>,
        pub tx: Sender<&'static str>,
        pub np: crate::TrayNowPlaying,
    }

    /// Menu labels have to fit a menu, not a window.
    fn clip(s: &str, max: usize) -> String {
        if s.chars().count() <= max {
            return s.to_string();
        }
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", head.trim_end())
    }

    impl ksni::Tray for TulipixTray {
        /// Any click opens the menu, not just the right one. The host draws the
        /// menu itself — an app cannot ask it to — so this property is the only
        /// way to say "left click means menu too". It replaces `activate`
        /// entirely, which is why the header row below carries what a left
        /// click used to do.
        const MENU_ON_ACTIVATE: bool = true;

        fn id(&self) -> String {
            "tulipix".into()
        }
        fn title(&self) -> String {
            "Tulipix".into()
        }
        // Falls back to the themed name when no pixmap was decoded.
        fn icon_name(&self) -> String {
            if self.icon.is_empty() { "tulipix".into() } else { String::new() }
        }
        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            self.icon.clone()
        }
        /// Hovering the icon says what is playing without opening anything.
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: "Tulipix".into(),
                description: if self.np.has_track {
                    format!("{} — {}", self.np.title, self.np.artist)
                } else {
                    String::new()
                },
                ..Default::default()
            }
        }
        /// Left click. With the popup tray style this opens the popup window;
        /// otherwise it raises the app, which is what it always did.
        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.tx.send(if self.np.popup { "tray.popup" } else { "tray.open" });
        }
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::{CheckmarkItem, StandardItem, SubMenu};
            let mut items: Vec<ksni::MenuItem<Self>> = Vec::new();

            // Header — art + track. A StatusNotifierItem menu row takes an icon
            // and a label and nothing else, so this is the whole "thumbnail":
            // one ~22px pixmap beside the title. The popup tray style exists for
            // when that is not enough.
            if self.np.has_track {
                items.push(
                    StandardItem {
                        label: clip(&self.np.title, 34),
                        icon_data: self.np.art_png.clone(),
                        // The popup tray style's window has no click of its own
                        // left (see MENU_ON_ACTIVATE) — the track row is where
                        // it lives now.
                        activate: Box::new(|t: &mut Self| {
                            let _ = t.tx.send(if t.np.popup { "tray.popup" } else { "tray.open" });
                        }),
                        ..Default::default()
                    }
                    .into(),
                );
                if !self.np.artist.is_empty() {
                    items.push(
                        StandardItem {
                            label: clip(&self.np.artist, 38),
                            enabled: false,
                            ..Default::default()
                        }
                        .into(),
                    );
                }
                items.push(ksni::MenuItem::Separator);
                items.push(
                    StandardItem {
                        label: "Previous".into(),
                        activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.prev"); }),
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: if self.np.playing { "Pause".into() } else { "Play".to_string() },
                        activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.playpause"); }),
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    StandardItem {
                        label: "Next".into(),
                        activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.next"); }),
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(ksni::MenuItem::Separator);
                items.push(
                    CheckmarkItem {
                        label: "Shuffle".into(),
                        checked: self.np.shuffle,
                        activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.shuffle"); }),
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(
                    SubMenu {
                        label: "Repeat".into(),
                        submenu: vec![
                            CheckmarkItem {
                                label: "Off".into(),
                                checked: self.np.repeat == "off",
                                activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.repeat.off"); }),
                                ..Default::default()
                            }
                            .into(),
                            CheckmarkItem {
                                label: "All".into(),
                                checked: self.np.repeat == "all",
                                activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.repeat.all"); }),
                                ..Default::default()
                            }
                            .into(),
                            CheckmarkItem {
                                label: "This track".into(),
                                checked: self.np.repeat == "one",
                                activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.repeat.one"); }),
                                ..Default::default()
                            }
                            .into(),
                        ],
                        ..Default::default()
                    }
                    .into(),
                );
                items.push(ksni::MenuItem::Separator);
            }

            items.push(
                StandardItem {
                    label: "Open Tulipix".into(),
                    activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.open"); }),
                    ..Default::default()
                }
                .into(),
            );
            items.push(
                StandardItem {
                    label: "Mini Player".into(),
                    enabled: self.np.has_track,
                    activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.mini"); }),
                    ..Default::default()
                }
                .into(),
            );
            items.push(ksni::MenuItem::Separator);
            items.push(
                StandardItem {
                    label: "Quit".into(),
                    activate: Box::new(|t: &mut Self| { let _ = t.tx.send("tray.quit"); }),
                    ..Default::default()
                }
                .into(),
            );
            items
        }
    }

    /// Receiver end of the menu-click channel, read by `drain_tray_events`.
    pub fn events() -> &'static Mutex<Option<Receiver<&'static str>>> {
        static EV: OnceLock<Mutex<Option<Receiver<&'static str>>>> = OnceLock::new();
        EV.get_or_init(|| Mutex::new(None))
    }

    pub fn new_channel() -> Sender<&'static str> {
        let (tx, rx) = channel();
        if let Ok(mut g) = events().lock() {
            *g = Some(rx);
        }
        tx
    }

    /// App → tray-thread state pushes. Unbounded and lossy-by-latest: the tray
    /// only ever draws the newest state, and `update_tray` drops a push that
    /// equals the last one, so this cannot back up behind a slow D-Bus round
    /// trip. Tokio's channel rather than std's because the receiving side lives
    /// in the tray thread's async block, next to `Handle::update`, which is
    /// itself async.
    pub fn state_channel() -> &'static Mutex<Option<tokio::sync::mpsc::UnboundedSender<crate::TrayNowPlaying>>> {
        static ST: OnceLock<Mutex<Option<tokio::sync::mpsc::UnboundedSender<crate::TrayNowPlaying>>>> =
            OnceLock::new();
        ST.get_or_init(|| Mutex::new(None))
    }

    /// RGBA (what `image` produces) → ARGB32 network byte order (what the SNI
    /// spec wants). Rotating each pixel right by one byte moves A into front.
    pub fn to_argb(mut rgba: Vec<u8>, w: u32, h: u32) -> Option<ksni::Icon> {
        if rgba.len() != (w as usize) * (h as usize) * 4 {
            return None;
        }
        for px in rgba.chunks_exact_mut(4) {
            px.rotate_right(1);
        }
        Some(ksni::Icon { width: w as i32, height: h as i32, data: rgba })
    }
}

/// Install a tray icon with Open / Quit items. Returns false if the platform
/// rejected creation (some Wayland compositors have no StatusNotifier host) or
/// if the `tray` feature is off.
///
/// Linux: served by ksni on the app's existing tokio runtime — no GTK, no
/// dedicated main loop. The returned handle is kept for the process lifetime
/// because dropping it withdraws the tray item.
#[cfg(all(feature = "tray", target_os = "linux"))]
pub fn init_tray(icon_rgba: Option<(Vec<u8>, u32, u32)>) -> bool {
    use ksni::TrayMethods;
    static STARTED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if STARTED.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return true;
    }
    let icon = icon_rgba
        .and_then(|(rgba, w, h)| linux_tray::to_argb(rgba, w, h))
        .map(|i| vec![i])
        .unwrap_or_default();
    let tx = linux_tray::new_channel();
    let (state_tx, mut state_rx) = tokio::sync::mpsc::unbounded_channel::<TrayNowPlaying>();
    if let Ok(mut g) = linux_tray::state_channel().lock() {
        *g = Some(state_tx);
    }
    let handle = std::thread::Builder::new().name("tulipix-tray".into()).spawn(move || {
        // Its own current-thread runtime: init_tray is called during startup on
        // the UI thread, and the tray's D-Bus service should not depend on the
        // shared runtime's scheduling for something that lives this long.
        let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
            Ok(rt) => rt,
            Err(e) => {
                tracing::warn!(error = %e, "tray: runtime build failed");
                return;
            }
        };
        rt.block_on(async move {
            let tray = linux_tray::TulipixTray { icon, tx, np: TrayNowPlaying::default() };
            match tray.spawn().await {
                Ok(handle) => {
                    // Park on the state channel instead of on `pending`: the
                    // handle has to outlive the service (dropping it withdraws
                    // the icon) and it is also the only way to re-publish the
                    // menu, so this task owns it for the process lifetime.
                    while let Some(np) = state_rx.recv().await {
                        let _ = handle.update(move |t| t.np = np).await;
                    }
                    std::future::pending::<()>().await;
                }
                Err(e) => tracing::warn!(error = %e, "tray: no StatusNotifier host"),
            }
        });
    });
    handle.is_ok()
}

/// Push now-playing state into the tray menu.
///
/// Cheap to call on a tick: a push that matches the last one is dropped here,
/// so the menu is only re-published over D-Bus when something a user could see
/// has actually changed.
#[cfg(all(feature = "tray", target_os = "linux"))]
pub fn update_tray(np: TrayNowPlaying) {
    use std::sync::Mutex as StdMutex;
    static LAST: StdMutex<Option<TrayNowPlaying>> = StdMutex::new(None);
    if let Ok(mut last) = LAST.lock() {
        if last.as_ref() == Some(&np) {
            return;
        }
        *last = Some(np.clone());
    }
    if let Ok(g) = linux_tray::state_channel().lock() {
        if let Some(tx) = g.as_ref() {
            let _ = tx.send(np);
        }
    }
}

#[cfg(all(feature = "tray", not(target_os = "linux")))]
pub fn init_tray(icon_rgba: Option<(Vec<u8>, u32, u32)>) -> bool {
    use tray_icon::menu::{MenuItem, PredefinedMenuItem as Pre};
    TRAY.with(|cell| {
        if cell.borrow().is_some() { return true; }
        let menu = TrayMenu::new();
        // Built once and relabelled in place: rebuilding the menu would drop it
        // out from under a user who has it open. Win32 menus take no artwork —
        // the track row is text, and the thumbnail lives on the taskbar
        // thumbnail toolbar instead.
        // Own id, not "tray.open": ids address menu items, and two rows sharing
        // one makes the click ambiguous. The app routes it to the same handler.
        let now = MenuItem::with_id("tray.now", "Nothing playing", false, None);
        let prev = MenuItem::with_id("tray.prev", "Previous", true, None);
        let playpause = MenuItem::with_id("tray.playpause", "Play", true, None);
        let next = MenuItem::with_id("tray.next", "Next", true, None);
        let shuffle = MenuItem::with_id("tray.shuffle", "Shuffle", true, None);
        let repeat = MenuItem::with_id("tray.repeat", "Repeat: off", true, None);
        let open = MenuItem::with_id("tray.open", "Open Tulipix", true, None);
        let mini = MenuItem::with_id("tray.mini", "Mini Player", true, None);
        let quit = MenuItem::with_id("tray.quit", "Quit", true, None);
        let ok = menu.append(&now).is_ok()
            && menu.append(&Pre::separator()).is_ok()
            && menu.append(&prev).is_ok()
            && menu.append(&playpause).is_ok()
            && menu.append(&next).is_ok()
            && menu.append(&Pre::separator()).is_ok()
            && menu.append(&shuffle).is_ok()
            && menu.append(&repeat).is_ok()
            && menu.append(&Pre::separator()).is_ok()
            && menu.append(&open).is_ok()
            && menu.append(&mini).is_ok()
            && menu.append(&Pre::separator()).is_ok()
            && menu.append(&quit).is_ok();
        if !ok { return false; }
        let items = TrayItems { now, playpause, shuffle, repeat };
        let mut b = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("Tulipix");
        if let Some((rgba, w, h)) = icon_rgba {
            if let Ok(i) = tray_icon::Icon::from_rgba(rgba, w, h) { b = b.with_icon(i); }
        }
        match b.build() {
            Ok(t) => {
                *cell.borrow_mut() = Some(t);
                TRAY_ITEMS.with(|c| *c.borrow_mut() = Some(items));
                true
            }
            Err(e) => {
                tracing::warn!(error = %e, "tray init failed");
                false
            }
        }
    })
}

/// Push now-playing state into the tray menu (Windows / macOS).
///
/// Must be called on the thread that built the tray — the items are
/// thread-local, and both platforms want menu mutation on the UI thread anyway.
#[cfg(all(feature = "tray", not(target_os = "linux")))]
pub fn update_tray(np: TrayNowPlaying) {
    TRAY_ITEMS.with(|c| {
        let b = c.borrow();
        let Some(items) = b.as_ref() else { return };
        items.now.set_text(if np.has_track {
            if np.artist.is_empty() { np.title.clone() } else { format!("{} — {}", np.title, np.artist) }
        } else {
            "Nothing playing".to_string()
        });
        items.playpause.set_text(if np.playing { "Pause" } else { "Play" });
        items.playpause.set_enabled(np.has_track);
        items.shuffle.set_text(if np.shuffle { "Shuffle: on" } else { "Shuffle: off" });
        let rep = if np.repeat.is_empty() { "off" } else { np.repeat.as_str() };
        items.repeat.set_text(format!("Repeat: {rep}"));
    });
    TRAY.with(|c| {
        if let Some(t) = c.borrow().as_ref() {
            let _ = t.set_tooltip(Some(if np.has_track {
                format!("Tulipix — {}", np.title)
            } else {
                "Tulipix".to_string()
            }));
        }
    });
}

#[cfg(not(feature = "tray"))]
pub fn update_tray(_np: TrayNowPlaying) {}

#[cfg(not(feature = "tray"))]
pub fn init_tray(_icon_rgba: Option<(Vec<u8>, u32, u32)>) -> bool {
    tracing::info!("tray feature off — skip init");
    false
}

/// Deliver any pending tray menu clicks as ids ("tray.open" / "tray.quit").
/// Same contract on every OS; only the source differs — a channel fed by ksni's
/// activate callbacks on Linux, tray-icon's global MenuEvent queue elsewhere.
#[cfg(all(feature = "tray", target_os = "linux"))]
pub fn drain_tray_events<F: FnMut(&str)>(mut handler: F) {
    let Ok(guard) = linux_tray::events().lock() else { return };
    let Some(rx) = guard.as_ref() else { return };
    while let Ok(id) = rx.try_recv() {
        handler(id);
    }
}

#[cfg(all(feature = "tray", not(target_os = "linux")))]
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
