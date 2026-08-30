// Desktop GUI app: never spawn/keep a console window on Windows (all builds).
// Logs still land in the file sink (tulipix_core::logging::LOG); only the
// stdout `tracing` fmt layer goes silent on Windows.
#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use anyhow::{Context, Result};
use slint::{ComponentHandle, Model};
use std::path::PathBuf;
use std::sync::OnceLock;
use tracing_subscriber::EnvFilter;
use tulipix_core::libraries::{ScanCadence, Section as LibrarySection};
use tulipix_core::proc::NoWindow;

#[cfg(all(feature = "alloc-mimalloc", any(target_os = "linux", target_os = "windows")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "alloc-jemalloc", any(target_os = "linux", target_os = "windows"), not(feature = "alloc-mimalloc")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

// The generated Slint UI now lives in the tulipix-ui crate (its own rustc
// unit). Re-export it at the crate root so existing `crate::MainWindow` /
// `ToolField` / … paths in this crate and its submodules keep resolving.
pub use tulipix_ui::*;
// Shared infra (pool cache, data-dir + bundled-binary helpers) now lives in
// tulipix-common; re-export so existing `crate::pool_for` / `dirs_default` /
// `bundled_bin_dir` / … paths in this crate + submodules keep resolving.
pub(crate) use tulipix_common::{
    anime4k_shader_args, bundled_present, cycle_folder_section, dirs_default,
    fmt_clock, fmt_date, fuzzy_score, histogram_image,
    human_size, kill_all_mpv, load_folder_sections, load_watched_folders,
    music_ipc, music_proc, music_section_key, music_section_label,
    now_secs, on_path, pool_for, set_folder_section, spawn_mpv_windowed,
    stop_music_child, watched_folders_path, MUSIC_GEN,
};
// MPRIS/SMTC handle storage now lives in common; setup_media_controls in main
// writes to it.
pub(crate) use tulipix_common::MEDIA_CONTROLS;
// Mini-player size classes — shared with the widget window (see miniwin).
use tulipix_music::mini_player::MiniStyle;
// Photos section (grid/library/editor/viewer helpers) lives in
// tulipix-sec-photos; glob-import so the photo `window.on_*` callbacks that
// stay in main keep calling `show_photo_at` / `open_editor` / … unqualified.
use tulipix_sec_photos::*;
// Music section (My Music/Podcasts/Radio/Audiobooks + YouTube tabs) lives in
// tulipix-sec-music; glob-import so the music `window.on_*` callbacks that stay
// in main keep calling its helpers unqualified.
use tulipix_sec_music::*;
// Videos library (grid/show models, season parsing, TMDB, discover) lives in
// tulipix-sec-videos; main owns the wire-up and launches mpv for playback.
use tulipix_sec_videos::*;

/// Idle threshold meaning "never" — pushed a year out so the idle listener never
/// fires while auto-lock is disabled (the default).
const IDLE_NEVER_SECS: u64 = 60 * 60 * 24 * 365;

#[cfg(feature = "dev-reload")]
mod dev_reload;
#[cfg(feature = "hot")]
mod hot;
mod miniwin;
mod profile_image;

/// Linux: read the XDG settings portal directly, through the `ashpd` that rfd's
/// file dialogs already put in the tree. dark-light v2 did exactly this read but
/// carried its own `ashpd 0.10` — and with it `async-std` plus the whole
/// smol stack (`async-io`/`async-fs`/`async-net`/`async-global-executor`) — into
/// every Linux build, because ashpd 0.10 turns on `zbus/async-io` while ours
/// runs on `zbus/tokio`. Two async runtimes for one boolean.
///
/// Needs a Tokio reactor, and the caller may be the Slint thread, so the probe
/// runs in a one-shot current-thread runtime on its own thread.
#[cfg(target_os = "linux")]
fn detect_dark() -> bool {
    use ashpd::desktop::settings::{ColorScheme, Settings as PortalSettings};
    std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        rt.block_on(async { PortalSettings::new().await.ok()?.color_scheme().await.ok() })
    })
    .join()
    .ok()
    .flatten()
    // No portal, or no preference recorded → dark. Same default as before.
    .map(|scheme| scheme != ColorScheme::PreferLight)
    .unwrap_or(true)
}

/// Windows/macOS keep dark-light: the registry read and the `AppleInterfaceStyle`
/// lookup are its whole value, and neither is testable from here.
#[cfg(not(target_os = "linux"))]
fn detect_dark() -> bool {
    let mode = std::thread::spawn(|| {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().ok()?;
        rt.block_on(async { dark_light::detect() }).ok()
    })
    .join()
    .ok()
    .flatten()
    .unwrap_or(dark_light::Mode::Dark);
    matches!(mode, dark_light::Mode::Dark | dark_light::Mode::Unspecified)
}

fn apply_theme_choice(window: &MainWindow, choice: ThemeChoice) {
    // OLED is the extra-dark tier (pure black surfaces); MusicTheme derives from
    // these so the whole app + the Home MusicMini follow one theme (2026-07-05).
    let (dark, oled) = match choice {
        ThemeChoice::Light => (false, false),
        ThemeChoice::ExtraDark => (true, false),
        ThemeChoice::Oled => (true, true),
        ThemeChoice::System => (detect_dark(), false),
    };
    window.set_dark(dark);
    window.set_oled(oled);
}

/// The stored design language as the index `Surface.lang` wants.
///
/// Names rather than numbers in settings.json: the file is hand-editable, and
/// `"skeuo"` survives a reordering of the picker where `2` would silently
/// become something else. Anything unrecognised — including the empty string a
/// first run gives back — is Standard, which is the look the app already had.
fn design_lang_index(name: &str) -> i32 {
    match name {
        "clay" => 1,
        "skeuo" => 2,
        _ => 0,
    }
}

/// Drive the Settings → Libraries maintenance progress bar. `frac` is 0..1, or
/// negative for an indeterminate "working…" bar. Safe to call from any thread.
fn set_lib_busy(weak: &slint::Weak<MainWindow>, task: &str, frac: f32) {
    let task = task.to_string();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_lib_busy_task(task.into());
        w.set_lib_busy_frac(frac);
    });
}
/// Clear the maintenance progress bar (idle).
fn clear_lib_busy(weak: &slint::Weak<MainWindow>) {
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_lib_busy_task("".into());
        w.set_lib_busy_frac(0.0);
    });
}

/// The section that was on screen before the current one (np.perf.section-release).
static LAST_SECTION: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
fn last_section() -> &'static std::sync::Mutex<String> {
    LAST_SECTION.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// Drop the models a section was drawing.
///
/// Only sections with a single known rebuild entry point appear here: a release
/// without a matching `enter_section` leaves a permanently empty page. Music is
/// excluded on purpose — `music-prewarmed` keeps MusicPage alive so the player
/// survives navigation.
pub fn release_section(w: &MainWindow, name: &str) {
    match name {
        "photos" => tulipix_sec_photos::release(w),
        "videos" => tulipix_sec_videos::release(w),
        // Only the memo. The rails keep their own clones of whatever is on
        // screen, so nothing blanks — the next ask just decodes again.
        "music" => tulipix_sec_music::clear_thumb_cache(),
        _ => {}
    }
}

/// Rebuild what a section draws. The counterpart of [`release_section`], and
/// the reason it is safe to free anything at all.
///
/// Both refreshes are async and land back on the event loop, so the page paints
/// empty for a frame or two — the same path a post-scan refresh already takes.
pub fn enter_section(w: &MainWindow, name: &str) {
    match name {
        "photos" => {
            let cat = w.get_photos_category().to_string();
            let q = w.get_photos_query().to_string();
            kick_category_refresh(w.as_weak(), cat, q);
        }
        "videos" => {
            let cat = w.get_video_category().to_string();
            kick_video_refresh(w.as_weak(), cat);
        }
        _ => {}
    }
}

/// Free the section being navigated away from, and rebuild the one being
/// navigated into. `on_section_changed` only ever told us where we are going;
/// the section we are leaving is remembered here.
fn release_previous_section(w: &MainWindow, entering: &str) {
    let leaving = {
        let Ok(mut g) = last_section().lock() else { return };
        std::mem::replace(&mut *g, entering.to_string())
    };
    if leaving == entering {
        return;
    }
    release_section(w, &leaving);
    enter_section(w, entering);
}

/// Everything the app can put down while it is *only* a music widget.
///
/// Minimising used to be one `window.hide()`: the whole MainWindow stayed
/// resident — every grid model with its decoded thumbnails, every cache — so
/// the "minimal music mode" cost the same RAM as the full app with a section
/// open. The widget draws now-playing state and nothing else, so the section
/// the user happened to be on is pure ballast until they come back.
///
/// Playback is untouched, and that is the whole reason this is safe: audio is
/// an out-of-process mpv driven over its IPC socket, so it does not care what
/// the UI process is holding.
///
/// NOT done here, deliberately: closing the SQLite pools. `tokio::sync::OnceCell`
/// has no reset, so it would need a resettable cell plus an interlock against a
/// running scan — and after the page-cache right-sizing the whole pool ceiling
/// is ~160 MB rather than the 1.28 GB it was. Not worth that risk yet.
pub fn release_for_widget(w: &MainWindow) {
    // The window's own property, not LAST_SECTION: the app opens on Home
    // without ever firing `section-changed`, so LAST_SECTION is empty until the
    // first navigation.
    release_section(w, &w.get_active_section().to_string());
    // Cheap to refill and never load-bearing: a pure memo in front of decoded
    // book covers that are all sitting on local disk.
    tulipix_sec_books::release_covers();
}

/// Undo [`release_for_widget`] — rebuild whatever section the user left open.
pub fn restore_from_widget(w: &MainWindow) {
    let current = w.get_active_section().to_string();
    enter_section(w, &current);
    // Home is not in `enter_section` (the section switch refreshes it inline),
    // but its rails went stale while the window was down.
    if current == "home" {
        kick_home_stats(w);
        kick_home_photos(w);
        kick_home_videos(w);
        kick_home_books(w);
        kick_home_cloud(w);
        kick_home_continue(w);
    }
}

static APP_START: OnceLock<std::time::Instant> = OnceLock::new();
/// Milliseconds from process start to the first turn of the event loop, frozen
/// once. Without this the "Startup to ready" row measured `APP_START.elapsed()`
/// at the moment the panel was built, so it reported how long the app had been
/// running — an hour-old session claimed a 3,600,000 ms startup and lit the
/// warning light every time.
static READY_MS: OnceLock<u64> = OnceLock::new();

fn main() -> Result<()> {
    let _ = APP_START.set(std::time::Instant::now());
    // Name the backend that matches the renderer feature, unless the user
    // overrode it. Slint would pick one on its own; saying it here keeps a box
    // with several GL stacks from choosing a different one run to run.
    if std::env::var_os("SLINT_BACKEND").is_none() {
        // Safety: set at the very top of main, before any threads spawn.
        #[cfg(feature = "renderer-skia")]
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-skia-opengl"); }
        #[cfg(all(not(feature = "renderer-skia"), feature = "renderer-femtovg"))]
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-femtovg"); }
    }
    // Desktop environments export GTK_MODULES=appmenu-gtk-module; the GTK file
    // dialogs (rfd) then print "Failed to load module appmenu-gtk-module" when
    // the module isn't installed. Strip it before anything touches GTK.
    if let Ok(m) = std::env::var("GTK_MODULES") {
        let kept: Vec<&str> =
            m.split(':').filter(|s| !s.is_empty() && !s.contains("appmenu-gtk-module")).collect();
        // Safety: top of main, before any threads spawn.
        unsafe {
            if kept.is_empty() {
                std::env::remove_var("GTK_MODULES");
            } else {
                std::env::set_var("GTK_MODULES", kept.join(":"));
            }
        }
    }
    // `icu_provider=error` mutes one warning we cannot act on: parley (via
    // i-slint-core) builds its word segmenter with
    // `WordSegmenter::new_for_non_complex_scripts`, which loads no complex-script
    // data, so every CJK or Southeast Asian string that reaches a text element
    // logs "No segmentation model for language: ja". The consequence is that the
    // run is treated as one word — word selection and word-wise cursor movement
    // inside it, nothing about rendering or line wrapping. It fires per string,
    // which is what makes it console noise rather than information.
    //
    // Scoped to the default only: RUST_LOG still wins, so `RUST_LOG=icu_provider=warn`
    // brings it back without a rebuild.
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                // sctk_adwaita: the client-side-decoration crate warns
                // "Ignoring unknown button type: icon" for every button in the
                // desktop's `button-layout` setting it has no drawing for. We
                // draw our own caption row, so its opinion of that layout is
                // noise about a frame nobody sees.
                .unwrap_or_else(|_| {
                    EnvFilter::new("info,icu_provider=error,sctk_adwaita=error")
                }),
        )
        .init();
    tulipix_core::crash::install_panic_hook();
    let _ = tulipix_core::logging::LOG.write_event("info", "tulipix", "startup");

    // zbus (transitive via rfd/xdg-portal + ashpd) requires a
    // Tokio reactor in scope for the calling thread. Enter a multi-thread
    // runtime so any sync zbus call has a reactor regardless of caller.
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let _rt_guard = rt.enter();

    if std::env::args().any(|a| a == "--trace-caps") {
        tulipix_core::caps::enable_trace();
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "tulipix starting");

    // Factory reset (relaunched by the in-app Settings → "Reset app" button):
    // wipe every config/data/cache tree BEFORE any DB pool or settings file is
    // opened, so nothing is locked and the app comes up truly fresh — no
    // settings.json → onboarding shows automatically.
    if std::env::args().any(|a| a == "--factory-reset") {
        for d in [
            tulipix_core::paths::config_dir(),
            tulipix_core::paths::data_dir(),
            tulipix_core::paths::cache_dir(),
        ] {
            if let Some(dir) = d {
                let _ = std::fs::remove_dir_all(&dir);
                let _ = std::fs::create_dir_all(&dir);
            }
        }
        // The settings file was just deleted out from under the cache.
        tulipix_core::settings::invalidate();
        tracing::info!("factory reset: all app data cleared on startup");
    }

    // yt-dlp goes stale on its own schedule: sites change, and a binary a few
    // weeks old starts answering 403 on downloads that worked yesterday. This
    // is the weekly check — background thread, at most one network call a week,
    // and every failure is a log line rather than something in the user's way.
    //
    // After the factory reset, not before: that arm deletes the data directory
    // this writes its update into and the settings file it keeps its timer in,
    // and a background thread racing an `rm -rf` of its own working directory
    // is not a race worth having.
    tulipix_core::updater::spawn_ytdlp_update();

    tulipix_platform::init_window_chrome();

    #[cfg(feature = "dev-reload")]
    dev_reload::spawn();

    // Load capability registry
    let default_caps = include_str!("../../../resources/capabilities.toml");
    let override_path = std::env::var_os("TULIPIX_CAPS_OVERRIDE")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs_default().map(|d| d.join("capabilities.local.toml")));
    let override_body = override_path.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
    if let Err(e) = tulipix_core::caps::load_from_toml(default_caps, override_body.as_deref()) {
        tracing::warn!(error = %e, "capabilities load failed; default-deny");
    }

    // Wayland/X11 window identity. Neither protocol carries a window icon the
    // app can push: the compositor matches the toplevel's app_id against an
    // installed `<app_id>.desktop` (or its StartupWMClass) and takes the icon
    // from there. Without this the app_id is empty, nothing matches, and the
    // title bar plus the dock fall back to the generic Wayland mark. Must be
    // set before the window is created — winit reads it when building the
    // window attributes, not on show.
    // `set_xdg_app_id` only writes into an already-live platform context — its
    // own factory is `Err(NoPlatform)`, so calling it first fails with "No
    // default Slint platform was selected". Selecting the backend here creates
    // that context; MainWindow::new() below then reuses it instead of building
    // its own.
    #[cfg(target_os = "linux")]
    {
        if let Err(e) = slint::BackendSelector::new().select() {
            tracing::warn!(error = %e, "slint backend select failed");
        }
        if let Err(e) = slint::set_xdg_app_id("tulipix") {
            tracing::warn!(error = %e, "xdg app id not set — desktop icon will fall back");
        }
    }
    // BEFORE the window, not after. Building the component tree lays out every
    // Text in it, and a Text's preferred width comes from the font it will be
    // drawn in. Registering Sora afterwards meant that first layout ran against
    // whatever the system fallback is — wider, so rows of chips and buttons
    // came out over-subscribed and elided ("Podca…", "Audiobo…"). The metrics
    // are cached, so they stayed wrong until something dirtied the layout, which
    // is why picking any design language in Settings appeared to "fix" it.
    register_bundled_fonts();
    let window = MainWindow::new()?;
    tulipix_platform::install_menubar(&tulipix_platform::default_menubar());
    // Seasonal India branding (Aug 1–31 + Jan 15–31, every year): the tray
    // icon switches for the whole window, and in-app logos DEFAULT to the
    // India mark (the user can still pick another sidebar logo in Profile).
    let festival = festival_season();
    window.set_festival_logo(festival);
    // Tray icon — the compiled-in mark (RGBA-decoded here so the platform
    // crate needs no image dependency); India mark during the festival window.
    let icon_bytes: &[u8] = if festival {
        include_bytes!("../../../resources/appicons/logo_india.png")
    } else {
        include_bytes!("../../../resources/icons/tulipix-64.png")
    };
    let tray_icon_rgba = image::load_from_memory(icon_bytes)
        .ok().map(|img| { let rgba = img.thumbnail(64, 64).to_rgba8(); let (w, h) = rgba.dimensions(); (rgba.into_raw(), w, h) });
    let tray_ok = tulipix_platform::init_tray(tray_icon_rgba);
    TRAY_ACTIVE.store(tray_ok, std::sync::atomic::Ordering::Relaxed);
    // Desktop banners want the icon as a file: `notify-send -i tulipix` goes through
    // the icon theme, which has no such entry until the app is installed, so a run
    // from the build directory shows a broken image. Same bytes as the tray, written
    // once into the cache.
    if let Some(dir) = tulipix_core::paths::cache_dir() {
        let path = dir.join(if festival { "notify-india.png" } else { "notify.png" });
        if !path.exists() {
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(&path, icon_bytes);
        }
        if path.exists() {
            tulipix_platform::notify::set_icon(path);
        }
    }

    // Idle threshold: an env override wins (test hook); otherwise the
    // Settings → Security auto-lock timeout when enabled, else the 600 s
    // ambient default.
    let boot_settings = tulipix_core::settings::Settings::load().unwrap_or_default();
    // Idle behaviour (ambient screensaver + optional lock) stays OFF until the
    // user enables "Auto-lock when idle" in Settings → Security. With the flag
    // off we push the threshold a year out so nothing fires on idle.
    let idle_secs = std::env::var("TULIPIX_IDLE_SECS").ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or_else(|| {
            if boot_settings.flag("autolock", false) {
                let s = boot_settings.idle_lock_secs;
                if s > 0 { s } else { 600 }
            } else {
                IDLE_NEVER_SECS
            }
        });
    tulipix_core::idle::set_threshold(idle_secs);
    tulipix_core::idle::spawn_tracker();

    // Ambient screensaver — idle tracker fades the overlay in/out. When the
    // Security auto-lock flag is on, going idle also locks the app.
    let w = window.as_weak();
    tulipix_core::idle::on_change(move |idle| {
        let w = w.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = w.upgrade() else { return; };
            let autolock = tulipix_core::settings::Settings::load()
                .map(|s| s.flag("autolock", false)).unwrap_or(false);
            // Idle only counts while the app is actually on screen. Minimised
            // or tucked away as the desktop widget, there is nothing to lock in
            // front of and no keyboard to catch — locking there just meant a PIN
            // prompt waiting the next time the window came back.
            if idle && (!w.window().is_visible() || w.window().is_minimized()) {
                tulipix_core::idle::mark_active();
                return;
            }
            if idle {
                w.set_ambient_clock(clock_now().into());
                if autolock {
                    tulipix_core::account::lock();
                    let mut u = w.get_user(); u.mode = Mode::Locked; w.set_user(u);
                    w.set_ambient_caption("Locked — click to unlock".into());
                } else {
                    w.set_ambient_caption("Tulipix — click to resume".into());
                }
            }
            w.set_ambient_active(idle);
            tracing::info!(idle, autolock, "ambient screensaver toggled");
        });
    });
    window.on_ambient_dismissed(tulipix_core::idle::mark_active);
    wire_lock_slideshow(&window);

    // Initial theme — start light (overridden below by persisted setting).
    apply_theme_choice(&window, ThemeChoice::Light);

    // Singleton 80 ms flush ticker — drains atomic counters into the UI in
    // one coalesced post per tick so per-file scan throughput doesn't spam
    // the main thread.
    spawn_progress_flusher(window.as_weak());

    // Idle-only background indexer (np.p2.ai.background) — backfills EXIF and
    // the photo full-text index while the machine is unused. Gated on idle and
    // mains power, and it backs off once the queue drains, so on a settled
    // library it costs a wakeup every couple of minutes.
    spawn_background_indexer();

    // Theme picker callback — apply + persist to settings.json.
    let w = window.as_weak();
    window.on_theme_changed(move |choice| {
        if let Some(w) = w.upgrade() {
            apply_theme_choice(&w, choice);
        }
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.theme = theme_choice_str(choice).to_string();
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save settings (theme)"); }
    });

    // Music-section zen theme button — theme is now unified with the app, so the
    // button flips Theme.dark/oled directly; here we re-sync theme-choice and
    // persist it through the app theme path (single source of truth).
    let w = window.as_weak();
    window.on_music_theme_changed(move || {
        let Some(w) = w.upgrade() else { return; };
        let choice = if !w.get_dark() { ThemeChoice::Light }
                     else if w.get_oled() { ThemeChoice::Oled }
                     else { ThemeChoice::ExtraDark };
        w.set_theme_choice(choice);
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.theme = theme_choice_str(choice).to_string();
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save settings (theme)"); }
    });

    // Folder picker — used by every "Add a folder" / "+ Add Location"
    // surface. Walks the picked folder, classifies every file by extension
    // into photos/videos/music/books, appends one LibraryRow per section
    // that actually has matching files, and kicks off a per-section scan
    // that renders thumbs into the matching tile model.
    fn pick_and_append(window: &MainWindow, _section_hint: &str) {
        let Some(path) = rfd::FileDialog::new()
            .set_title("Add watched folder")
            .pick_folder()
        else { return; };
        tracing::info!(path = %path.display(), "folder picked");

        // Remember the folder so the next launch can restore the grids.
        persist_watched_folder(&path);
        set_scan_silent(false); // an explicit add *does* show the scan popup
        add_folder_path(window, path);
    }

    // Silent background refresh of every watched folder — used after the editor
    // saves a file so the new/updated photo shows without a scan popup.
    fn refresh_library_silent(window: &MainWindow) {
        set_scan_silent(true);
        for path in load_watched_folders() {
            if path.exists() { add_folder_path(window, path); }
        }
    }

    /// Rescan the MUSIC library only — what the Downloader's Rescan button does.
    ///
    /// It used to call `refresh_library_silent`, which re-adds every watched
    /// folder: a "did my download land?" check that rescanned photos, videos
    /// and books along with it. The folder walk stays (one folder can hold both
    /// tracks and cover art), but every non-music count is dropped before the
    /// scan is scheduled, so nothing outside Music is touched.
    fn refresh_music_library_silent(window: &MainWindow) {
        set_scan_silent(true);
        for path in load_watched_folders() {
            if !path.exists() { continue; }
            let mut counts = classify_folder(&path);
            counts.retain(|section, _| *section == "music");
            if counts.is_empty() { continue; }
            add_folder_counted(window, path, counts);
        }
    }



    // Add a freshly-picked folder straight into one music sub-section
    // (mymusic|podcasts|audiobooks|radio|youtube). The Add button is
    // section-scoped — whatever music tab is open is where the folder lands; no
    // universal "which section?" popup.
    //
    // The flag below is a best-effort head start for a folder that was already
    // scanned once: on a first add there are no `track_meta` rows yet, and the
    // pass that actually lands it is `apply_music_folder_sections`, at the end
    // of the scan this kicks off.
    fn music_add_folder_to_section(window: &MainWindow, path: PathBuf, key: &str) {
        let path_str = path.display().to_string();
        set_folder_section(&path_str, key);
        persist_watched_folder(&path);
        set_scan_silent(false);
        add_folder_path(window, path);
        let aud = key == "audiobooks";
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("music").await {
                if aud {
                    let _ = tulipix_music::audiobooks::flag_audiobook_folder(&pool, &path_str).await;
                } else {
                    // Re-adding a folder to a non-audiobook section clears any
                    // stale audiobook flag so its tracks return to My Music.
                    let _ = tulipix_music::audiobooks::set_folder_flag(&pool, &path_str, false).await;
                }
            }
        });
    }

    let w = window.as_weak();
    window.on_pick_folder(move || {
        let Some(w) = w.upgrade() else { return; };
        let section = w.get_active_section().to_string();
        // On the music page the Add button is section-scoped: the folder lands
        // in whichever music sub-section is currently open (music-view), with no
        // universal "which section?" popup. Other sections add directly.
        if section == "music" {
            let Some(path) = rfd::FileDialog::new().set_title("Add music folder").pick_folder() else { return; };
            let key = w.get_music_view().to_string();
            music_add_folder_to_section(&w, path, &key);
            if w.get_library_rows().row_count() > 0 { w.set_onboarding_lib_added(true); }
            return;
        }
        pick_and_append(&w, &section);
        // Reflect a successful add into the onboarding wizard step.
        if w.get_library_rows().row_count() > 0 { w.set_onboarding_lib_added(true); }
    });

    // Confirm the music sub-section for the just-picked folder: persist the
    // section tag, classify+scan the folder, and flag/unflag its audiobook
    // tracks (np.p5.music.audiobook-chapters).
    let w = window.as_weak();
    window.on_music_confirm_add_section(move |key| {
        let Some(w) = w.upgrade() else { return; };
        let path_str = w.get_music_pending_add_path().to_string();
        if path_str.is_empty() { return; }
        music_add_folder_to_section(&w, PathBuf::from(&path_str), key.as_ref());
        if w.get_library_rows().row_count() > 0 { w.set_onboarding_lib_added(true); }
    });

    let w = window.as_weak();
    window.on_lib_add(move || {
        let Some(w) = w.upgrade() else { return; };
        let section = w.get_active_section().to_string();
        pick_and_append(&w, if section == "settings" { "photos" } else { &section });
    });

    // Silent library refresh (editor save → show the new/updated photo).
    let w = window.as_weak();
    window.on_refresh_library(move || {
        if let Some(w) = w.upgrade() { refresh_library_silent(&w); }
    });

    // ── Cloud: rclone remotes CRUD + remote-tree browse (np.p4.cloud.*) ──
    tulipix_sec_cloud::wire(&window);

    // ── Live TV: iptv-org playlists, the channel grid, and mpv (Videos tab). ──
    tulipix_sec_videos::livetv::wire(&window);

    // ── Stream Plus: the anime lane and the second provider stack (Videos tab).
    // Its schema and download queue come up on a background task so a cold start
    // is not held behind a DB open.
    tulipix_sec_videos::splus::wire(&window);
    {
        let w = window.as_weak();
        // The download folder picker lives here because this is where the file
        // dialog already is — the section crate has no business owning one.
        window.on_splus_pick_download_dir(move || {
            let Some(w) = w.upgrade() else { return };
            let weak = w.as_weak();
            std::thread::spawn(move || {
                let Some(dir) = rfd::FileDialog::new().pick_folder() else { return };
                let dir = dir.to_string_lossy().to_string();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = weak.upgrade() {
                        tulipix_sec_videos::splus::settings::set_dir(&w, &dir);
                    }
                });
            });
        });
    }
    tokio::spawn(async { tulipix_sec_videos::splus::init().await });

    // ── Books: Book Home page (Continue Reading hero, stats, library grid,
    // filter chips) + the reader. Callbacks land on `window.on_books_*`.
    tulipix_sec_books::wire(&window);

    // Tools section wiring lives in the tulipix-sec-tools crate. Under the `hot`
    // feature it is routed through the tulipix-hot dylib so the callbacks can be
    // re-wired into the running app on every dylib rebuild (no restart).
    #[cfg(not(feature = "hot"))]
    tulipix_sec_tools::wire(&window);
    #[cfg(feature = "hot")]
    crate::hot::wire_and_watch(&window);

    // ── Transfer: the LAN web server, the share tray and the inbox. The port
    // itself is opened and closed by `section_changed` below, not here.
    tulipix_sec_transfer::wire(&window);

    // ── Genesis: book search and download, a sub-page of Books. Gated from the
    // first commit rather than retrofitted, so a build that should not carry it
    // drops the crate entirely instead of shipping it disabled.
    #[cfg(feature = "genesis")]
    tulipix_sec_genesis::wire(&window);

    // ── Finances: accounts, the ledger, bills, subscriptions, dues, budgets.
    // Wired at startup rather than on first open because the sidebar badge has to
    // be right before the section is ever visited — a bill due today that only
    // appears once you look is a reminder that does not work.
    #[cfg(feature = "finances")]
    tulipix_sec_finances::wire(&window);

    // Photo viewer — click a tile to open the original in a full-screen modal,
    // then navigate the library with prev/next (and the slideshow timer).
    let w = window.as_weak();
    window.on_open_photo(move |idx| {
        let Some(w) = w.upgrade() else { return; };
        show_photo_at(&w, idx);
        w.set_viewer_open(true);
    });
    let w = window.as_weak();
    window.on_viewer_next(move || {
        let Some(w) = w.upgrade() else { return; };
        let total = w.get_viewer_total();
        if total <= 0 { return; }
        let next = if w.get_viewer_shuffle() {
            rand_index(total)
        } else {
            (w.get_viewer_index() + 1).rem_euclid(total)
        };
        show_photo_at(&w, next);
    });
    let w = window.as_weak();
    window.on_viewer_prev(move || {
        let Some(w) = w.upgrade() else { return; };
        let total = w.get_viewer_total();
        if total <= 0 { return; }
        let prev = if w.get_viewer_shuffle() {
            rand_index(total)
        } else {
            (w.get_viewer_index() - 1).rem_euclid(total)
        };
        show_photo_at(&w, prev);
    });
    let w = window.as_weak();
    window.on_close_viewer(move || {
        if let Some(w) = w.upgrade() {
            w.set_viewer_open(false);
            w.set_viewer_slideshow(false); // stop the timer on close (also drops fullscreen via changed-handler)
            w.set_viewer_info_open(false); // reset the EXIF panel
            w.set_viewer_zoom(1.0);
        }
    });
    // Viewer → Edit: open the editor on the photo currently shown.
    let w = window.as_weak();
    window.on_viewer_edit(move || {
        let Some(w0) = w.upgrade() else { return; };
        let idx = w0.get_viewer_index();
        let Some(path) = photo_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else { return; };
        w0.set_viewer_open(false);
        w0.set_viewer_slideshow(false);
        w0.set_viewer_info_open(false);
        open_editor(w.clone(), idx, &path);
    });
    // Real OS-level fullscreen for the slideshow (not just the app window
    // filling its frame). Driven by the viewer-slideshow changed handler.
    let w = window.as_weak();
    window.on_request_fullscreen(move |on| {
        if let Some(w) = w.upgrade() { w.window().set_fullscreen(on); }
    });
    // Slideshow setup auto-start: 5 s after the last interaction in the setup
    // row, start the slideshow as configured. Each arm() restarts the timer.
    let arm_timer = std::rc::Rc::new(slint::Timer::default());
    let w = window.as_weak();
    window.on_slideshow_arm(move || {
        let weak = w.clone();
        arm_timer.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_secs(5),
            move || {
                if let Some(w) = weak.upgrade() {
                    if w.get_viewer_slideshow_setup() {
                        w.set_viewer_slideshow_setup(false);
                        w.set_viewer_slideshow(true);
                    }
                }
            },
        );
    });

    // Video playback — hand the file to mpv (out-of-process) with resume +
    // watch-progress writeback (np.p3.watch-progress / last-accessed).
    let w = window.as_weak();
    window.on_play_video(move |idx| { play_video_at(w.clone(), idx); });

    // Videos library tab switch (Library/Continue/Starred/Archive/Trash).
    let w = window.as_weak();
    window.on_set_video_category(move |c| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_video_category(c.clone());
        kick_video_refresh(w.clone(), c.to_string());
    });

    // Top-level kind tab (TV / Movies / Local).
    let w = window.as_weak();
    window.on_set_video_kind(move |k| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_video_kind(k.clone());
        if let Ok(mut g) = video_kind().lock() { *g = k.to_string(); }
        if let Ok(mut g) = video_show().lock() { *g = None; } // leave any drilled show
        w0.set_video_show_title("".into());
        match k.as_str() {
            "discover" => kick_discover_refresh(w.clone()),
            // Stream is search-driven: no catalogue requests until the user
            // asks for something. Only the local recent-search list loads.
            "stream" => {
                stream_recent_load(w.clone());
                stream_prefs_load(w.clone());
                // Landing screen: Continue Watching from the local DB, then the
                // catalogue's own rows.
                stream_feed_load(w.clone());
                // Quiet daily check for new episodes of saved shows.
                stream_bookmarks_refresh(w.clone());
                // Pick up anything the last session left mid-download.
                stream_downloads_resume(w.clone());
                stream_prune_caches();
            }
            // Stream Plus lands on its home view, and only a click on the Home
            // sub-tab fires `home-load` — so entering the tab has to fire it.
            "splus" => w0.invoke_splus_home_load(),
            _ => kick_video_refresh(w.clone(), w0.get_video_category().to_string()),
        }
    });

    // Stream tab (remote catalogue) — all handlers live in tulipix_sec_videos::stream.
    let w = window.as_weak();
    window.on_video_stream_search(move |q| stream_search(w.clone(), q.to_string()));
    let w = window.as_weak();
    window.on_video_stream_open(move |id| stream_open(w.clone(), id.to_string()));
    let w = window.as_weak();
    window.on_video_stream_open_resume(move |id| stream_open_resume(w.clone(), id.to_string()));
    let w = window.as_weak();
    window.on_video_stream_open_pick(move |id| stream_open_pick(w.clone(), id.to_string()));
    let w = window.as_weak();
    window.on_video_stream_back(move || stream_back(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_home(move || stream_home(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_play(move |i| stream_play(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_set_dub(move |id| stream_set_dub(w.clone(), id.to_string()));
    let w = window.as_weak();
    window.on_video_stream_set_season(move |s| stream_set_season(w.clone(), s));
    let w = window.as_weak();
    window.on_video_stream_set_episode(move |e| stream_set_episode(w.clone(), e));
    let w = window.as_weak();
    window.on_video_stream_set_sub(move |i| stream_set_sub(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_set_resolution(move |r| stream_set_resolution(w.clone(), r));
    let w = window.as_weak();
    window.on_video_stream_suggest(move |q| stream_suggest(w.clone(), q.to_string()));
    let w = window.as_weak();
    window.on_video_stream_suggest_clear(move || stream_suggest_clear(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_preview(move |id| stream_preview(w.clone(), id.to_string()));
    let w = window.as_weak();
    window.on_video_stream_copy_link(move |i| stream_copy_link(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_download(move |i| stream_download(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_download_cancel(move || stream_download_cancel(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_download_dismiss(move || stream_download_dismiss(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_set_current(move |i| stream_set_current(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_play_current(move || stream_play_current(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_copy_current(move || stream_copy_current(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_download_current(move || stream_download_current(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_toggle_bookmark(move || stream_toggle_bookmark(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_bookmarks_load(move || stream_bookmarks_load(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_bookmark_remove(move |id| stream_bookmark_remove(w.clone(), id.to_string()));
    let w = window.as_weak();
    window.on_video_stream_bookmarks_sort(move |k, a| stream_bookmarks_sort(w.clone(), k.to_string(), a));
    let w = window.as_weak();
    window.on_video_stream_recent_clear(move || stream_recent_clear(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_hosts_load(move || stream_hosts_load(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_hosts_save(move |t| stream_hosts_save(w.clone(), t.to_string()));
    let w = window.as_weak();
    window.on_video_stream_hosts_reset(move || stream_hosts_reset(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_key_save(move |k| stream_key_save(w.clone(), k.to_string()));
    let w = window.as_weak();
    window.on_video_stream_key_reset(move || stream_key_reset(w.clone()));
    // Which catalogue Stream searches, and the scraped source's own address.
    window.set_video_stream_source(stream_active_source().key().into());
    let w = window.as_weak();
    window.on_video_stream_set_source(move |s| stream_set_source(w.clone(), s.to_string()));
    let w = window.as_weak();
    window.on_video_stream_source_opened(move || stream_source_load(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_fourk_save(move |u| stream_fourk_save(w.clone(), u.to_string()));
    let w = window.as_weak();
    window.on_video_stream_fourk_reset(move || stream_fourk_reset(w.clone()));
    // Landing row — resume slider + trending picks.
    let w = window.as_weak();
    window.on_video_stream_feed_load(move || stream_feed_load(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_search_more(move || stream_search_more(w.clone()));
    // Downloads page.
    let w = window.as_weak();
    window.on_video_stream_downloads_load(move || stream_downloads_load(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_downloads_resume(move || stream_downloads_resume(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_download_play(move |i| stream_download_play(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_download_retry(move |i| stream_download_retry(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_download_forget(move |i| stream_download_forget(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_download_delete_file(move |i| stream_download_delete_file(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_download_reveal(move |i| stream_download_reveal(w.clone(), i));
    let w = window.as_weak();
    window.on_video_stream_downloads_clear(move || stream_downloads_clear(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_downloads_cancel_all(move || stream_downloads_cancel_all(w.clone()));
    // Watch history.
    let w = window.as_weak();
    window.on_video_stream_history_load(move |p| stream_history_load(w.clone(), p));
    let w = window.as_weak();
    window.on_video_stream_history_clear(move || stream_history_clear(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_history_remove(move |id, p| {
        stream_history_remove(w.clone(), id.to_string(), p)
    });
    let w = window.as_weak();
    window.on_video_stream_history_play(move |id, se, ep| {
        stream_history_play(w.clone(), id.to_string(), se, ep)
    });
    // Acquisition + playback extras.
    let w = window.as_weak();
    window.on_video_stream_download_season(move || stream_download_season(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_trailer(move || stream_trailer(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_cast_discover(move || stream_cast_discover(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_cast_to(move |d| stream_cast_to(w.clone(), d.to_string()));
    let w = window.as_weak();
    window.on_video_stream_cast_stop(move || stream_cast_stop(w.clone()));
    let w = window.as_weak();
    window.on_video_stream_hosts_check(move |t| stream_hosts_check(w.clone(), t.to_string()));
    // Preferences that only take effect at the next launch.
    let w = window.as_weak();
    window.on_video_stream_set_sub_scale(move |v| stream_set_sub_scale(w.clone(), v));
    let w = window.as_weak();
    window.on_video_stream_set_sub_delay(move |v| stream_set_sub_delay(w.clone(), v));
    let w = window.as_weak();
    window.on_video_stream_set_autoplay(move |on| stream_set_autoplay(w.clone(), on));
    let w = window.as_weak();
    window.on_video_stream_set_to_library(move |on| stream_set_to_library(w.clone(), on));

    let w = window.as_weak();
    window.on_video_refresh_discover(move || {
        kick_discover_refresh(w.clone());
    });

    // Drill into a TV show's episodes / back to the show cards.
    let w = window.as_weak();
    window.on_video_show_drill(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        if let Ok(mut g) = video_show().lock() { *g = Some(id as i64); }
        // Look up the show title for the drill-in header.
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let title = match pool_for("videos").await {
                Ok(pool) => sqlx::query_scalar::<_, String>("SELECT title FROM shows WHERE id = ?")
                    .bind(id as i64).fetch_optional(&pool).await.ok().flatten().unwrap_or_default(),
                Err(_) => String::new(),
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_video_show_title(title.into());
                kick_video_refresh(w.as_weak(), w.get_video_category().to_string());
            });
        });
    });
    let w = window.as_weak();
    window.on_video_show_back(move || {
        let Some(w0) = w.upgrade() else { return; };
        if let Ok(mut g) = video_show().lock() { *g = None; }
        w0.set_video_show_title("".into());
        kick_video_refresh(w.clone(), w0.get_video_category().to_string());
    });

    // Filename search within the videos section.
    let w = window.as_weak();
    window.on_video_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        if let Ok(mut g) = video_query().lock() { *g = q.to_string(); }
        kick_video_refresh(w.clone(), w0.get_video_category().to_string());
    });

    // Right-click menu for a video poster — actions vary by tab.
    let w = window.as_weak();
    window.on_open_video_context(move |idx, mx, my| {
        let Some(w0) = w.upgrade() else { return; };
        let mk = |id: &str, label: &str, danger: bool| ContextItem {
            id: id.into(), label: label.into(), shortcut: "".into(), danger,
        };
        let category = w0.get_video_category().to_string();
        let items: Vec<ContextItem> = if category == "trash" {
            vec![
                mk("play", "Play", false),
                mk("restore", "Restore", false),
                mk("reveal", "Reveal In File System", false),
                mk("delete-forever", "Remove from library", true),
            ]
        } else {
            vec![
                mk("play", "Play", false),
                mk("mark-watched", "Toggle Watched", false),
                mk("star", if category == "starred" { "Unstar" } else { "Star" }, false),
                mk("archive", if category == "archive" { "Unarchive" } else { "Archive" }, false),
                mk("reveal", "Reveal In File System", false),
                mk("trash", "Move to Trash", true),
            ]
        };
        w0.set_vctx_items(slint::ModelRc::new(slint::VecModel::from(items)));
        w0.set_vctx_index(idx);
        w0.set_vctx_x(mx);
        w0.set_vctx_y(my);
        w0.set_vctx_open(true);
    });

    // Video poster context actions — flag changes run off-thread, then the tab
    // is rebuilt. Mirrors the photos section's parity actions.
    let w = window.as_weak();
    window.on_video_context_action(move |idx, action| {
        let Some(w0) = w.upgrade() else { return; };
        let action = action.to_string();
        if action == "play" { play_video_at(w.clone(), idx); return; }
        let path = video_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned());
        if action == "reveal" {
            if let Some(p) = path {
                if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&p) {
                    tracing::error!(error = %e, "reveal failed");
                }
            }
            return;
        }
        let Some(id) = video_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()) else { return; };
        let category = w0.get_video_category().to_string();
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("videos").await else { return; };
            use tulipix_videos::{star_archive_trash as sat, watch_progress as wp};
            let res = match action.as_str() {
                "star"    => sat::set_starred(&pool, id, category != "starred").await,
                "archive" => sat::set_archived(&pool, id, category != "archive").await,
                "trash"   => sat::trash(&pool, id, sat::DEFAULT_PURGE_DAYS).await,
                "restore" => sat::restore(&pool, id).await,
                // Drop just this row from the index (the file on disk is a proxy
                // we never wrote, so it is left untouched).
                "delete-forever" => sqlx::query("DELETE FROM video_meta WHERE item_id = ?")
                    .bind(id).execute(&pool).await.map(|_| ()).map_err(Into::into),
                "mark-watched" => {
                    let now = wp::get(&pool, id).await.ok().flatten().map(|p| p.finished).unwrap_or(false);
                    wp::mark_finished(&pool, id, !now).await
                }
                _ => Ok(()),
            };
            if let Err(e) = res { tracing::error!(error = %e, %action, "video flag action failed"); }
            kick_video_refresh(weak, category);
        });
    });

    // Clear cached thumbnails for the videos section, then rescan to rebuild
    // them fresh (np.p3 — "remove past thumb cache from video page").
    let w = window.as_weak();
    window.on_video_clear_thumbs(move || {
        let Some(w0) = w.upgrade() else { return; };
        let paths: Vec<PathBuf> = video_full().lock().ok()
            .map(|g| g.iter().map(|(_, abs, _)| abs.clone()).collect())
            .unwrap_or_default();
        let mut removed = 0u32;
        for abs in &paths {
            if let Ok(meta) = std::fs::metadata(abs) {
                let mtime = meta.modified().ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64).unwrap_or(0);
                let k = tulipix_core::thumbs::key(abs, mtime, meta.len());
                if let Some(p) = tulipix_core::thumbs::thumb_path(&k) {
                    if std::fs::remove_file(&p).is_ok() { removed += 1; }
                }
            }
        }
        tracing::info!(removed, "cleared video thumbnail cache");
        // Drop the accumulator so the rescan rebuilds tiles with fresh thumbs.
        if let Ok(mut g) = video_full().lock() { g.clear(); }
        w0.set_video_tiles(slint::ModelRc::new(slint::VecModel::from(Vec::<VideoTile>::new())));
        w0.invoke_refresh_library();
    });

    // Music playback — single headless mpv instance; now-playing bar reflects it.
    let w = window.as_weak();
    window.on_play_music(move |idx| {
        let Some(w) = w.upgrade() else { return; };
        // A track picked out of the Songs page plays THAT page: the queue
        // becomes the rest of the list as it is sorted and filtered right now.
        // A tile from anywhere else (no such row) is a one-off and leaves the
        // queue it jumped in front of alone (np.p6.music.context-queue).
        let order = songs_view_order();
        if order.contains(&idx) { play_in_context(&w, &order, idx, "songs"); }
        else { play_music_at(&w, idx); }
    });
    let w = window.as_weak();
    window.on_music_next(move || {
        let Some(w) = w.upgrade() else { return; };
        // YouTube queue active → skip within it (honours shuffle); else library.
        if YT_QUEUE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            yt_queue_jump(w.as_weak(), true, w.get_music_shuffle());
            return;
        }
        if w.get_music_np_total() <= 0 { return; }
        // Next follows the QUEUE — the album, playlist or page you started from
        // — and only walks the library when the queue has run dry. It used to
        // go straight to the library index, which is why a built queue was
        // ignored and Next landed on unrelated tracks.
        queue_advance(&w, true);
    });
    let w = window.as_weak();
    window.on_music_prev(move || {
        let Some(w) = w.upgrade() else { return; };
        // YouTube queue active → step back within it; else library.
        if YT_QUEUE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            yt_queue_jump(w.as_weak(), false, false);
            return;
        }
        let total = w.get_music_np_total();
        if total <= 0 { return; }
        // A few seconds in, Prev restarts the current track (player convention);
        // right at the start it goes to the previous one.
        if w.get_music_pos() > 3.0 {
            music_ipc(&["seek", "0", "absolute"]);
            return;
        }
        // Walk the real played order back, shuffled or not. Next pops the QUEUE
        // — the album or playlist you started from — so index−1 in the library
        // was never the track you actually came from; only an empty history
        // falls back to it.
        if let Some(prev) = shuffle_prev_index(total) {
            play_previous_at(&w, prev);
            return;
        }
        // Same audiobook filter the forward walk uses: index−1 in the library
        // can be a book chapter, and My Music never plays those.
        if let Some(p) = step_music_pos(w.get_music_np_index(), total, -1) {
            play_music_at(&w, p);
        }
    });
    let w = window.as_weak();
    window.on_music_stop(move || {
        MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); // suppress auto-advance
        stop_music_child(); // graceful quit → kill fallback (WirePlumber-safe)
        if let Some(w) = w.upgrade() { w.set_music_playing(false); }
    });
    // Play/pause toggle — resume the live track via IPC, or start one if idle.
    let w = window.as_weak();
    window.on_music_toggle_pause(move || {
        let Some(w) = w.upgrade() else { return; };
        let alive = music_proc().lock().map(|g| g.is_some()).unwrap_or(false);
        if alive { music_ipc(&["cycle", "pause"]); }
        else if w.get_music_np_total() > 0 { play_music_at(&w, w.get_music_np_index().max(0)); }
    });
    // Seekbar — absolute seconds.
    window.on_music_seek(|secs| music_ipc(&["seek", &(secs as f64).to_string(), "absolute"]));
    // Volume (0–130) + mute.
    let w = window.as_weak();
    window.on_music_set_volume(move |v| {
        if let Some(w) = w.upgrade() { w.set_music_volume(v); }
        music_ipc(&["set_property", "volume", &(v as f64).to_string()]);
    });
    window.on_music_toggle_mute(|| music_ipc(&["cycle", "mute"]));
    // Repeat off → all → one (IPC loop-file for one).
    let w = window.as_weak();
    window.on_music_cycle_repeat(move || {
        let Some(w) = w.upgrade() else { return; };
        let next = match w.get_music_repeat().as_str() { "off" => "all", "all" => "one", _ => "off" };
        w.set_music_repeat(next.into());
        music_ipc(&["set_property", "loop-file", if next == "one" { "inf" } else { "no" }]);
    });
    // Player panel toggle (queue / eq / lyrics) — re-click closes.
    let w = window.as_weak();
    window.on_music_toggle_panel(move |p| {
        let Some(w) = w.upgrade() else { return; };
        let p = p.to_string();
        let open = if w.get_music_player_panel() == p.as_str() { String::new() } else { p };
        w.set_music_player_panel(open.clone().into());
        match open.as_str() {
            "queue"  => build_music_queue(&w),
            "lyrics" => load_music_lyrics(&w),
            _ => {}
        }
    });
    // Equalizer preset — load its 10 band gains, persist, apply live.
    let w = window.as_weak();
    window.on_music_set_eq(move |preset| {
        let Some(w) = w.upgrade() else { return; };
        let preset = preset.to_string();
        if let Some(eq) = tulipix_music::eq::Equalizer::preset(&preset) {
            if let Ok(mut g) = music_eq().lock() { *g = eq.gains_db; }
        }
        w.set_music_eq_preset(preset.clone().into());
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.advanced.insert("music.eq-preset".into(), preset);
        let _ = s.save();
        apply_music_eq(&w);
    });
    // Rebuild the Up-next model on demand. The Music section rebuilds it when
    // one of its panels opens; the Home players have no panel of their own, so
    // without this their queue was whatever the last panel open had left behind
    // — empty on a fresh session, and stale once the track advanced.
    // Home can be a whole session on its own: the My Music caches (music_ids,
    // and the id→position map every queue row is built from) are only filled
    // when the Music section loads. Asked from Home before that, the queue came
    // back either empty or full of positions that point at nothing — which is
    // what "it plays whatever it likes" was. Warm the caches, then build again.
    let warm_q: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
    let w = window.as_weak();
    window.on_music_build_queue(move || {
        let Some(w) = w.upgrade() else { return; };
        if music_ids().lock().map(|g| g.is_empty()).unwrap_or(true) {
            populate_music_views(w.as_weak());
            // ponytail: a fixed wait, because populate_music_views is
            // fire-and-forget; give it a completion signal if this proves flaky.
            let weak = w.as_weak();
            warm_q.start(
                slint::TimerMode::SingleShot,
                std::time::Duration::from_millis(600),
                move || { if let Some(w) = weak.upgrade() { build_music_queue(&w); } },
            );
        }
        build_music_queue(&w);
    });
    // Play a track from the queue panel (index = playback position).
    let w = window.as_weak();
    window.on_music_play_queue(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        // YouTube queue active → jump to that video; else a library position.
        if YT_QUEUE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            let id = { let mut g = match yt_queue().lock() { Ok(g) => g, Err(_) => return };
                let i = i.max(0) as usize;
                if i >= g.0.len() { return; }
                g.1 = i; g.0[i].clone() };
            if let Ok(mut g) = yt_cur_audio().lock() { *g = id.clone(); }
            yt_play_audio(w.clone(), id);
            return;
        }
        play_music_at(&w0, i);
        // Jumping down the queue consumes everything above the row you picked,
        // itself included — otherwise it is still queued and plays again the
        // moment it ends. Rows from the favourites / history rails share this
        // callback and simply are not in the queue, so nothing is dropped.
        let Some(id) = music_id_at(i) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::queue::drop_through(&pool, id).await;
            let _ = weak.upgrade_in_event_loop(|w| build_music_queue(&w));
        });
    });
    // Add current track to a playlist — open a picker listing manual playlists
    // (np.p5.music.playlists-builder).
    let w = window.as_weak();
    window.on_music_add_to_playlist(move || {
        let Some(w0) = w.upgrade() else { return; };
        if current_music_id(&w0).is_none() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let rows: Vec<(i64, String)> = sqlx::query_as(
                "SELECT id, name FROM playlists WHERE is_smart = 0 ORDER BY name COLLATE NOCASE")
                .fetch_all(&pool).await.unwrap_or_default();
            let _ = weak.upgrade_in_event_loop(move |w| {
                let names: Vec<slint::SharedString> = rows.iter().map(|(_, n)| n.clone().into()).collect();
                if let Ok(mut g) = pick_playlist_ids().lock() { *g = rows.iter().map(|(id, _)| *id).collect(); }
                w.set_music_playlist_picker(slint::ModelRc::new(slint::VecModel::from(names)));
                w.set_music_add_pick_open(true);
            });
        });
    });
    // Equalizer band drag — set one band, apply the 10-band filter live.
    let w = window.as_weak();
    window.on_music_set_eq_band(move |i, v| {
        let Some(w) = w.upgrade() else { return; };
        if !(0..10).contains(&i) { return; }
        if let Ok(mut g) = music_eq().lock() { g[i as usize] = v.clamp(-12.0, 12.0) as f64; }
        w.set_music_eq_preset("custom".into());
        apply_music_eq(&w);
    });
    // EQ — save the current bands as a named custom profile.
    let w = window.as_weak();
    window.on_music_eq_save_profile(move || {
        let Some(w0) = w.upgrade() else { return; };
        let gains: Vec<f64> = music_eq().lock().map(|g| g.to_vec()).unwrap_or_else(|_| vec![0.0; 10]);
        let mut list = load_eq_customs();
        if list.len() >= 3 { return; } // only 3 custom profiles allowed
        // First free "Custom N" slot (1..=3).
        let n = (1..=3).find(|n| !list.iter().any(|(name, _)| name == &format!("Custom {n}"))).unwrap_or(list.len() + 1);
        let name = format!("Custom {n}");
        list.push((name.clone(), gains));
        save_eq_customs(&list);
        populate_eq_customs(&w0);
        w0.set_music_eq_preset(name.into());
    });
    // EQ — delete a saved custom profile (right-click in the EQ popup).
    let w = window.as_weak();
    window.on_music_eq_delete_profile(move |name| {
        let Some(w0) = w.upgrade() else { return; };
        let name = name.to_string();
        let mut list = load_eq_customs();
        list.retain(|(n, _)| *n != name);
        save_eq_customs(&list);
        populate_eq_customs(&w0);
    });
    // EQ — load a saved custom profile by name.
    let w = window.as_weak();
    window.on_music_eq_load_profile(move |name| {
        let Some(w0) = w.upgrade() else { return; };
        let name = name.to_string();
        if let Some((_, gains)) = load_eq_customs().into_iter().find(|(n, _)| *n == name) {
            if let Ok(mut g) = music_eq().lock() {
                for (i, v) in gains.iter().take(10).enumerate() { g[i] = v.clamp(-12.0, 12.0); }
            }
            w0.set_music_eq_preset(name.into());
            apply_music_eq(&w0);
        }
    });
    // Secondary-sidebar view switch (np.p4.music.sidebar-collapse). Populate the
    // podcasts / audiobooks views on demand (np.p5.music.podcast-feeds / .audiobook-chapters).
    let w = window.as_weak();
    window.on_music_set_view(move |v| {
        if let Some(w) = w.upgrade() {
            w.set_music_detail_open(false); // leave any album/artist detail when switching section
            w.set_music_view(v.clone());
            match v.as_str() {
                "podcasts" => {
                    w.set_music_podcast_detail_open(false);
                    // First switch only — models persist; mutations refresh their own view.
                    if music_warm_once("podcasts") { populate_podcasts(&w); }
                    if music_warm_once("podcast_latest") { populate_podcast_latest(&w); }
                    if music_warm_once("podcast_downloads") { populate_podcast_downloads(&w); }
                }
                "audiobooks" => { if music_warm_once("audiobooks") { populate_audiobooks(&w); } }
                // Counts feed the Home tiles; an empty cache (first ever open)
                // auto-triggers one full Refresh so the section self-populates.
                "radio" => radio_load_counts(w.as_weak(), true),
                "youtube" => {
                    w.set_music_yt_channel_open(false);
                    w.set_music_yt_playlist_open(false);
                    // Warmed once (here or on Music-section enter); switching in/out
                    // reuses the loaded models instead of re-decoding every thumb.
                    warm_youtube(&w);
                }
                _ => {}
            }
        }
    });
    wire_radio(&window);
    // Sidebar section click — warm every podcast sub-page so the podcast section
    // never shows empty on first open (data loads while the user is elsewhere).
    let w = window.as_weak();
    // Home Photos card → open the centred photo in the in-app viewer.
    let wp = window.as_weak();
    window.on_home_open_photo_item(move |idx| {
        let Some(w0) = wp.upgrade() else { return; };
        let path = recent_home_photos().lock().ok().and_then(|g| g.get(idx as usize).cloned());
        if let Some(p) = path { show_photo_path(&w0, &p); w0.set_viewer_open(true); }
    });
    // Home Videos card → play the centred video with the external mpv window
    // (the video section's default playback path). The library id is looked up
    // by path so resume + watch-progress + last-accessed all track this playback
    // — an id-less spawn never reached the CONTINUE strip.
    let wvh = window.as_weak();
    window.on_home_open_video_item(move |idx| {
        let path = recent_home_videos().lock().ok().and_then(|g| g.get(idx as usize).cloned());
        let Some(p) = path else { return; };
        let weak = wvh.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let (item_id, resume) = match pool_for("videos").await {
                Ok(pool) => {
                    let id = sqlx::query_scalar::<_, i64>("SELECT id FROM items WHERE abs_path = ?")
                        .bind(p.to_string_lossy().as_ref())
                        .fetch_optional(&pool).await.ok().flatten();
                    let resume = match id {
                        Some(id) => {
                            let _ = tulipix_videos::last_accessed::touch(&pool, id).await;
                            let r = tulipix_videos::watch_progress::resume(&pool, id).await.ok().flatten();
                            // First watch: seed a 1s progress row so the CONTINUE
                            // strip picks the video up the moment playback starts
                            // (the real position overwrites this on mpv exit).
                            if r.is_none() {
                                let _ = tulipix_videos::watch_progress::update(&pool, id, 1.0, None).await;
                            }
                            r
                        }
                        None => None,
                    };
                    (id, resume)
                }
                Err(_) => (None, None),
            };
            spawn_mpv_windowed(p, resume, item_id);
            let _ = weak.upgrade_in_event_loop(|w| kick_home_continue(&w));
        });
    });
    // Home Books card → open that book in the reader by its library id (the
    // row index maps to the id stashed when the covers were loaded). Routing
    // through the existing books-open-details callback reuses the reader's
    // parse/paginate/resume path — no id-less re-open.
    let wbh = window.as_weak();
    window.on_home_open_book_item(move |idx| {
        let Some(w0) = wbh.upgrade() else { return; };
        let id = recent_home_books().lock().ok()
            .and_then(|g| g.get(idx as usize).copied());
        if let Some(id) = id { w0.invoke_books_open_details(id as i32); }
    });
    // Home Tools card → select that tool category in the Tools page.
    let wt = window.as_weak();
    window.on_home_open_tool(move |cat| {
        if let Some(w0) = wt.upgrade() { w0.set_tools_category(cat); }
    });
    // CONTINUE strip — chip filter re-pushes the cached rows
    // (podcast/audiobook routing lives in .slint).
    let wcf = window.as_weak();
    window.on_home_continue_filter(move |f| {
        if let Ok(mut g) = home_cont_filter().lock() { *g = f.to_string(); }
        push_home_continue(&wcf);
    });
    // CONTINUE video card → resume in the external mpv window at the saved
    // position (item_id keeps watch_progress tracking across the resume, the
    // last-accessed touch keeps it at the head of the strip).
    let wcv = window.as_weak();
    window.on_home_continue_video(move |id, path| {
        let id = id as i64;
        let path = PathBuf::from(path.to_string());
        let weak = wcv.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let resume = match pool_for("videos").await {
                Ok(pool) => {
                    let _ = tulipix_videos::last_accessed::touch(&pool, id).await;
                    tulipix_videos::watch_progress::resume(&pool, id).await.ok().flatten()
                }
                Err(_) => None,
            };
            spawn_mpv_windowed(path, resume, Some(id));
            let _ = weak.upgrade_in_event_loop(|w| kick_home_continue(&w));
        });
    });
    // ⋮ → Remove: persist the dismissal + drop the card immediately.
    let wcr = window.as_weak();
    window.on_home_continue_remove(move |kind, id, path| {
        let key = home_cont_dismiss_key(kind.as_str(), id as i64, path.as_str());
        if let Ok(mut g) = home_cont_dismissed().lock() { g.insert(key.clone()); }
        save_home_cont_dismissed();
        if let Ok(mut rows) = home_cont_rows().lock() {
            rows.retain(|r| home_cont_dismiss_key(r.kind, r.id, &r.path) != key);
        }
        push_home_continue(&wcr);
    });
    // Home auto-refresh — while Home is visible, re-pull every card feed on a
    // slow tick so library changes (scans, new books, cloud remotes, progress)
    // show up without leaving the page. Slint caches images by path, so the
    // repeated thumb loads are cheap. Timer leaked on purpose: one per app life.
    // Tray menu (np.p1.tray) — poll the global MenuEvent channel. Without this
    // drain, the tray's "Open Tulipix" / "Quit" items were dead ends.
    let wtr = window.as_weak();
    let tray_tick: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
    tray_tick.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_millis(500),
        move || {
            let weak = wtr.clone();
            tulipix_platform::drain_tray_events(move |id| {
                let Some(w) = weak.upgrade() else { return };
                match id {
                    // "tray.now" is the header row carrying the artwork; it
                    // raises the app like the Open item, it just looks different.
                    "tray.open" | "tray.now" => miniwin::restore(&w),
                    "tray.popup" => miniwin::toggle_popup(&w),
                    "tray.mini" => miniwin::open_mini(&w),
                    "tray.playpause" => w.invoke_music_toggle_pause(),
                    "tray.next" => w.invoke_music_next(),
                    "tray.prev" => w.invoke_music_prev(),
                    "tray.shuffle" => w.invoke_music_toggle_shuffle(),
                    // The submenu names the target state; cycle until it matches
                    // rather than adding a setter the rest of the app has no use
                    // for. Three states, so this lands in at most two steps.
                    "tray.repeat.off" | "tray.repeat.all" | "tray.repeat.one" => {
                        let want = id.rsplit('.').next().unwrap_or("off");
                        for _ in 0..3 {
                            if w.get_music_repeat() == want { break; }
                            w.invoke_music_cycle_repeat();
                        }
                    }
                    "tray.quit" => { let _ = slint::quit_event_loop(); }
                    _ => {}
                }
            });
            // Same tick feeds the widget/popup windows and the tray menu — both
            // only need to be right to the second, and update_tray drops a push
            // that has not changed.
            if let Some(w) = wtr.upgrade() {
                miniwin::sync(&w);
            }
        },
    );
    // Playback-progress ticker (5s) — persists the live podcast/audiobook
    // position while playing (podcast position_s had NO writer at all) and
    // live-refreshes the CONTINUE strip whenever Home is visible.
    let wpt = window.as_weak();
    let pos_tick: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
    pos_tick.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(5),
        move || {
            let Some(w0) = wpt.upgrade() else { return; };
            if !w0.get_music_playing() { return; }
            let pos = w0.get_music_pos() as f64;
            let on_home = w0.get_active_section() == "home";
            let weak = w0.as_weak();
            match w0.get_music_player_mode().as_str() {
                "podcast" => {
                    let id = PODCAST_NOW_ID.load(std::sync::atomic::Ordering::Relaxed);
                    if id > 0 && pos > 1.0 {
                        tokio::runtime::Handle::current().spawn(async move {
                            if let Ok(pool) = pool_for("podcasts").await {
                                let _ = sqlx::query("UPDATE podcast_episodes SET position_s = ? WHERE id = ?")
                                    .bind(pos).bind(id).execute(&pool).await;
                            }
                            if on_home {
                                let _ = weak.upgrade_in_event_loop(|w| kick_home_continue(&w));
                            }
                        });
                    }
                }
                "book" => {
                    if let Some(id) = current_music_id(&w0) {
                        let speed = w0.get_music_book_speed() as f64;
                        tokio::runtime::Handle::current().spawn(async move {
                            if let Ok(pool) = pool_for("music").await {
                                let _ = tulipix_music::audiobooks::save_progress(&pool, id, pos.max(1.0), speed).await;
                            }
                            if on_home {
                                let _ = weak.upgrade_in_event_loop(|w| kick_home_continue(&w));
                            }
                        });
                    }
                }
                _ => {}
            }
        },
    );
    let wh = window.as_weak();
    let home_tick: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
    home_tick.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(20),
        move || {
            let Some(w0) = wh.upgrade() else { return; };
            // Six queries' worth of work for a page nobody can see — folded down
            // to the music widget, the main window is hidden but its section is
            // still "home". `restore` refreshes on the way back in.
            if w0.get_active_section() == "home" && w0.window().is_visible() {
                kick_home_stats(&w0);
                kick_home_photos(&w0);
                kick_home_videos(&w0);
                kick_home_books(&w0);
                kick_home_cloud(&w0);
                kick_home_continue(&w0);
            }
        },
    );

    // Pre-instantiate the Music page (the biggest component tree by far) a
    // moment after startup, while the user is still looking at Home. The first
    // click on Music then only flips visibility instead of paying the whole
    // build cost on the UI thread. The same shot pre-warms the music data
    // models (guarded by music_warm_once, so a real visit never double-loads).
    let wpre = window.as_weak();
    let prewarm: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
    prewarm.start(
        slint::TimerMode::SingleShot,
        std::time::Duration::from_millis(2500),
        move || {
            let Some(w0) = wpre.upgrade() else { return; };
            if music_warm_once("podcasts") { populate_podcasts(&w0); }
            if music_warm_once("podcast_latest") { populate_podcast_latest(&w0); }
            if music_warm_once("podcast_downloads") { populate_podcast_downloads(&w0); }
            if music_warm_once("podcast_trends") { populate_podcast_trends(&w0); }
            // Audiobooks too: warms book covers AND kicks the title/author/cover
            // resolution chain without the user having to open the tab first.
            if music_warm_once("audiobooks") { populate_audiobooks(&w0); }
            warm_youtube(&w0);
            w0.set_music_prewarmed(true);
        },
    );

    window.on_section_changed(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        // Free the section being LEFT, then rebuild the one being entered.
        //
        // The `if active-section == …` gates in main.slint already destroy each
        // page's elements, but the models hang off MainWindow, which outlives
        // every page — and each grid tile owns a decoded thumbnail. Without
        // this, RSS was a high-water mark of every section visited this
        // session and never came back down.
        //
        // Only Photos and Videos are released: that is where the bitmaps are,
        // and both have a single rebuild entry point to pair with. Music is
        // deliberately excluded — `music-prewarmed` keeps MusicPage alive on
        // purpose so the player survives navigation.
        release_previous_section(&w0, s.as_str());
        // Transfer binds its port on the way in and drops it on the way out, so
        // it needs to hear about every section change, not just its own.
        tulipix_sec_transfer::section_changed(&w0, s.as_str());
        // Finances re-runs the recurrence sweep on entry: the app may have been
        // left running across midnight, or across a month end.
        #[cfg(feature = "finances")]
        tulipix_sec_finances::section_changed(&w0, s.as_str());
        if s.as_str() == "home" {
            // Every layout but Classic shows the money somewhere, and all of it
            // comes from what the Finances section loads. One pass on entry.
            #[cfg(feature = "finances")]
            {
                if home_layout_wants_money(w0.get_home_layout().as_str()) {
                    tulipix_sec_finances::refresh(&w0);
                }
            }
            push_home_cinema_extras(&w0);
            // Stream's feed is eight queries, so it only runs for the layout that
            // draws it.
            if w0.get_home_layout().as_str() == "stream" { kick_home_events(&w0); }
            // Fresh greeting (time of day) + counts on every Home landing.
            set_home_greeting_now(&w0);
            kick_home_stats(&w0);
            kick_home_photos(&w0);
            kick_home_videos(&w0);
            kick_home_books(&w0);
            kick_home_cloud(&w0);
            kick_home_continue(&w0);
        }
        if s.as_str() == "music" {
            // Warm each sub-page once; re-entering the section reuses loaded models.
            if music_warm_once("podcasts") { populate_podcasts(&w0); }
            if music_warm_once("podcast_latest") { populate_podcast_latest(&w0); }
            if music_warm_once("podcast_downloads") { populate_podcast_downloads(&w0); }
            if music_warm_once("podcast_trends") { populate_podcast_trends(&w0); }
            // Warm YouTube in the background too, so its first open is instant.
            warm_youtube(&w0);
        }
    });
    let w = window.as_weak();
    window.on_music_set_lib_tab(move |t| {
        if let Some(w) = w.upgrade() {
            w.set_music_detail_open(false); // leave any album/artist detail when switching tab
            w.set_music_playlist_name("".into()); // leave playlist detail too
            w.set_music_lib_tab(t.clone());
            match t.as_str() {
                "favorites" => populate_favorites(&w),
                "history" => populate_history(&w),
                // Re-apply the current search filter to the tab's cached tiles on
                // every entry. Without this, a prior search that filtered folders/
                // playlists to empty stayed empty until an app restart, because the
                // folders case only refreshed the *roots* list and playlists had no
                // case at all (np.p5.atmusic.lib-context-search).
                "folders" => { populate_folder_roots(&w); rebuild_browse_tab(&w, "folders"); }
                "playlists" => rebuild_browse_tab(&w, "playlists"),
                "albums" | "artists" | "genres" => { w.set_music_browse_page(0); rebuild_browse_tab(&w, t.as_str()); }
                _ => {}
            }
        }
    });

    // Downloader tab (mdl) — resolve a streaming URL, download into the library.
    window.set_music_dl_dest(mdl::default_music_dir().display().to_string().into());
    window.on_music_dl_url_changed({
        let w = window.as_weak();
        move |url| { if let Some(win) = w.upgrade() { win.set_music_dl_provider_badge(mdl::detect(&url).into()); } }
    });
    window.on_music_dl_pick_folder({
        let w = window.as_weak();
        move || {
            if let Some(path) = rfd::FileDialog::new().set_title("Choose download folder").pick_folder() {
                if let Some(win) = w.upgrade() { win.set_music_dl_dest(path.display().to_string().into()); }
            }
        }
    });
    window.on_music_dl_resolve({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() { mdl::start_resolve(win.as_weak(), win.get_music_dl_url().to_string()); } }
    });
    window.on_music_dl_cancel_resolve({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() { mdl::cancel_resolve(win.as_weak()); } }
    });
    window.on_music_dl_search({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() {
            mdl::start_search(win.as_weak(), win.get_music_dl_url().to_string(),
                              win.get_music_dl_search_provider().to_string());
        } }
    });
    window.on_music_dl_download({
        let w = window.as_weak();
        move || {
            if let Some(win) = w.upgrade() {
                let url = win.get_music_dl_url().to_string();
                let dest_s = win.get_music_dl_dest().to_string();
                let dest = if dest_s.is_empty() { mdl::default_music_dir() } else { std::path::PathBuf::from(dest_s) };
                let format = win.get_music_dl_format().to_string();
                let name_method = win.get_music_dl_name_method().to_string();
                let parallel = win.get_music_dl_parallel();
                let threads = win.get_music_dl_threads();
                let bitrate = win.get_music_dl_bitrate();
                mdl::start_download(win.as_weak(), url, dest, format, name_method, parallel, threads, bitrate);
            }
        }
    });
    window.on_music_dl_cancel(move || { mdl::cancel(); });
    window.on_music_dl_toggle_row({
        let w = window.as_weak();
        move |i| { if let Some(win) = w.upgrade() { mdl::toggle_row(win.as_weak(), i); } }
    });
    window.on_music_dl_select_all({
        let w = window.as_weak();
        move |all| { if let Some(win) = w.upgrade() { mdl::select_all(win.as_weak(), all); } }
    });
    window.on_music_dl_set_format({
        let w = window.as_weak();
        move |v| { if let Some(win) = w.upgrade() { win.set_music_dl_format(v); } }
    });
    window.on_music_dl_set_bitrate({
        let w = window.as_weak();
        move |v| { if let Some(win) = w.upgrade() { win.set_music_dl_bitrate(v.trim().parse::<i32>().unwrap_or(128).clamp(0, 320)); } }
    });
    window.on_music_dl_clear_all({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() { mdl::clear_all(win.as_weak()); } }
    });
    window.on_music_dl_set_queue_page({
        let w = window.as_weak();
        move |p| { if let Some(win) = w.upgrade() { mdl::set_queue_page(win.as_weak(), p); } }
    });
    window.on_music_dl_sort_by({
        let w = window.as_weak();
        move |k| { if let Some(win) = w.upgrade() { mdl::sort_queue(win.as_weak(), k.to_string()); } }
    });
    window.on_music_dl_set_name_method({
        let w = window.as_weak();
        move |v| { if let Some(win) = w.upgrade() { win.set_music_dl_name_method(v); } }
    });
    window.on_music_dl_refresh_library({
        let w = window.as_weak();
        move || {
            if let Some(win) = w.upgrade() {
                // Light the pill immediately: the classify walk below runs
                // before any counter exists, and a button that looks idle for a
                // second after a click reads as a button that did nothing.
                win.set_music_dl_rescan_busy(true);
                win.set_music_dl_rescan_frac(0.0);
                refresh_music_library_silent(&win);
            }
        }
    });
    window.on_music_dl_set_parallel({
        let w = window.as_weak();
        move |v| { if let Some(win) = w.upgrade() { win.set_music_dl_parallel(v.clamp(1, 4)); } }
    });
    window.on_music_dl_set_threads({
        let w = window.as_weak();
        move |v| { if let Some(win) = w.upgrade() { win.set_music_dl_threads(v.clamp(1, 8)); } }
    });
    window.on_music_dl_set_main_artist({
        let w = window.as_weak();
        move |i, a| { if let Some(win) = w.upgrade() { mdl::set_main_artist(win.as_weak(), i, a.to_string()); } }
    });
    window.on_music_dl_bulk_main_artist({
        let w = window.as_weak();
        move |scope, a| { if let Some(win) = w.upgrade() { mdl::bulk_main_artist(win.as_weak(), scope.to_string(), a.to_string()); } }
    });
    window.on_music_dl_open_history({
        let w = window.as_weak();
        move |page| { if let Some(win) = w.upgrade() { mdl::load_history(win.as_weak(), page); } }
    });
    window.on_music_dl_open_searches({
        let w = window.as_weak();
        move |page| { if let Some(win) = w.upgrade() { mdl::load_searches(win.as_weak(), page); } }
    });
    window.on_music_dl_history_play({
        let w = window.as_weak();
        move |path, title, sub| { if let Some(win) = w.upgrade() { mdl::play_history(&win, path.to_string(), title.to_string(), sub.to_string()); } }
    });
    window.on_music_dl_history_reveal(move |path| {
        let p = std::path::PathBuf::from(path.to_string());
        if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&p) {
            tracing::error!(error = %e, "mdl: reveal failed");
        }
    });
    window.on_music_dl_use_search({
        let w = window.as_weak();
        move |url| { if let Some(win) = w.upgrade() {
            win.set_music_dl_url(url.clone());
            win.set_music_dl_provider_badge(mdl::detect(&url).into());
        } }
    });
    window.on_music_dl_cli_clear({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() { mdl::clear_cli(win.as_weak()); } }
    });
    window.on_music_dl_clear_history({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() { mdl::clear_history(win.as_weak()); } }
    });
    window.on_music_dl_clear_searches({
        let w = window.as_weak();
        move || { if let Some(win) = w.upgrade() { mdl::clear_searches(win.as_weak()); } }
    });
    window.on_music_dl_retry({
        let w = window.as_weak();
        move |i| { if let Some(win) = w.upgrade() { mdl::retry_track(win.as_weak(), i); } }
    });
    // Rating (np.p4.music.rating) — loved + 1–5 stars on the current track.
    let w = window.as_weak();
    window.on_music_love(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let loved = tulipix_music::rating::toggle_loved(&pool, id).await.unwrap_or(false);
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_np_loved(loved);
                populate_music_views(w.as_weak()); // refresh the Loved rail
            });
        });
    });
    let w = window.as_weak();
    window.on_music_rate(move |n| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        let stars = n.clamp(0, 5) as u8;
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::rating::set_stars(&pool, id, stars).await;
            let _ = weak.upgrade_in_event_loop(move |w| w.set_music_np_stars(stars as i32));
        });
    });
    // Songs-list per-row fav / star / find-lyrics (np.p5.atmusic.* list columns).
    let w = window.as_weak();
    window.on_music_row_fav(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let cur = w0.get_music_np_index();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let loved = tulipix_music::rating::toggle_loved(&pool, id).await.unwrap_or(false);
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Ok(mut g) = music_songs().lock() { if let Some(s) = g.iter_mut().find(|s| s.pos == pos) { s.loved = loved; } }
                rebuild_music_songs_page(&w);
                if pos == cur { w.set_music_np_loved(loved); }
            });
        });
    });
    let w = window.as_weak();
    window.on_music_row_rate(move |pos, n| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let stars = n.clamp(0, 5) as u8;
        let cur = w0.get_music_np_index();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::rating::set_stars(&pool, id, stars).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Ok(mut g) = music_songs().lock() { if let Some(s) = g.iter_mut().find(|s| s.pos == pos) { s.stars = stars as i32; } }
                rebuild_music_songs_page(&w);
                if pos == cur { w.set_music_np_stars(stars as i32); }
            });
        });
    });
    // Find lyrics for a list row — play it (so the lyrics save to the right track),
    // the slint side opens the Find panel seeded with this row's name/artist.
    let w = window.as_weak();
    window.on_music_row_find_lyrics(move |pos| {
        if let Some(w0) = w.upgrade() { play_music_at(&w0, pos); }
    });
    // Context menu — open the track in the OS default player (xdg-open / open / start).
    let w = window.as_weak();
    window.on_music_song_play_default(move |pos| {
        let _ = w;
        let Some(path) = music_paths().lock().ok().and_then(|g| g.get(pos as usize).cloned()) else { return; };
        open_in_default_app(&path);
    });
    // Context menu — Properties: gather media info into the dialog props.
    let w = window.as_weak();
    window.on_music_song_properties(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let path = music_paths().lock().ok().and_then(|g| g.get(pos as usize).cloned());
        let meta = music_songs().lock().ok().and_then(|g| g.iter().find(|s| s.pos == pos).cloned());
        let item_id = meta.as_ref().map(|m| m.item_id).unwrap_or(-1);
        w0.set_music_props_pos(pos);
        if let Some(m) = &meta {
            w0.set_music_props_title(m.title.clone().into());
            w0.set_music_props_artist(m.artist.clone().into());
            w0.set_music_props_album(m.album.clone().into());
            w0.set_music_props_duration(if m.duration_s > 0.0 { fmt_clock(m.duration_s).into() } else { "—".into() });
        }
        let (fmt, size) = path.as_ref().map(|p| {
            let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_uppercase();
            let bytes = std::fs::metadata(p).map(|md| md.len()).unwrap_or(0);
            (ext, human_size(bytes))
        }).unwrap_or_default();
        w0.set_music_props_format(fmt.into());
        w0.set_music_props_size(size.into());
        w0.set_music_props_path(path.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default().into());
        w0.set_music_props_genre("".into());
        w0.set_music_props_release("".into());
        w0.set_music_props_credits("".into());
        w0.set_music_props_analysis("".into());
        w0.set_music_song_props_open(true);
        // Fill genre / release date / credits + analysis from the DB asynchronously.
        if item_id >= 0 {
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("music").await else { return; };
                let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
                let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
                let row: Option<(Option<String>, Option<String>, Option<String>, Option<f64>, Option<String>, Option<f64>)> = sqlx::query_as(
                    "SELECT genre, release_date, credits, bpm, music_key, dr_score FROM track_meta WHERE item_id = ?")
                    .bind(item_id).fetch_optional(&pool).await.ok().flatten();
                if let Some((g, rd, cr, bpm, key, dr)) = row {
                    let mut parts: Vec<String> = Vec::new();
                    if let Some(b) = bpm { parts.push(format!("{b:.0} BPM")); }
                    if let Some(k) = key { parts.push(k); }
                    if let Some(d) = dr { parts.push(format!("DR {d:.0}")); }
                    let analysis = parts.join(" · ");
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_music_props_genre(g.unwrap_or_default().into());
                        w.set_music_props_release(rd.unwrap_or_default().into());
                        w.set_music_props_credits(cr.unwrap_or_default().into());
                        w.set_music_props_analysis(analysis.into());
                    });
                }
            });
        }
    });
    // Context menu — Edit media info: open the tag editor targeting this song.
    let w = window.as_weak();
    window.on_music_song_edit_media(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(m) = music_songs().lock().ok().and_then(|g| g.iter().find(|s| s.pos == pos).cloned()) else { return; };
        if let Ok(mut g) = tag_edit_target().lock() { *g = Some(m.item_id); }
        w0.set_music_tag_title(m.title.clone().into());
        w0.set_music_tag_artist(m.artist.clone().into());
        w0.set_music_tag_album(m.album.clone().into());
        w0.set_music_tag_album_artist("".into());
        w0.set_music_tag_track("".into());
        w0.set_music_tag_disc("".into());
        w0.set_music_tag_date("".into());
        w0.set_music_tag_genre("".into());
        w0.set_music_tag_credits("".into());
        w0.set_music_tag_locked(false);
        w0.set_music_tag_fetch_status("".into());
        // Cover preview — the song's tile art.
        w0.set_music_tag_art(music_thumb_at(pos));
        w0.set_music_tag_open(true);
        prefill_tag_editor(&w, m.item_id);
    });
    // Context menu — music-video link (np.p4.music.video-link): pick a video
    // file for this song; playback goes through the windowed mpv player. The
    // videos-library item id is stored when the file is inside that library,
    // plus the absolute path (playback key) in a sibling column.
    let w = window.as_weak();
    window.on_music_song_link_video(move |pos| {
        let Some(m) = music_songs().lock().ok().and_then(|g| g.iter().find(|s| s.pos == pos).cloned()) else { return; };
        let weak = w.clone();
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let Some(file) = rfd::FileDialog::new().set_title("Pick the music video for this song")
                .add_filter("Videos", &["mp4", "mkv", "mov", "webm", "avi", "m4v"]).pick_file() else { return; };
            let path = file.display().to_string();
            let title = m.title.clone();
            rt.spawn(async move {
                let Ok(pool) = pool_for("music").await else { return; };
                let _ = sqlx::query("ALTER TABLE music_video_link ADD COLUMN video_path TEXT").execute(&pool).await;
                // Resolve the videos-library id when the picked file is indexed there.
                let vid: i64 = match pool_for("videos").await {
                    Ok(vp) => sqlx::query_scalar("SELECT id FROM items WHERE abs_path = ?")
                        .bind(&path).fetch_optional(&vp).await.ok().flatten().unwrap_or(0),
                    Err(_) => 0,
                };
                let _ = tulipix_music::video_link::link(&pool, m.item_id, vid).await;
                let _ = sqlx::query("UPDATE music_video_link SET video_path = ? WHERE item_id = ?")
                    .bind(&path).bind(m.item_id).execute(&pool).await;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_caps_nudge(
                    format!("Linked music video to “{title}”.").into()));
            });
        });
    });
    let w = window.as_weak();
    window.on_music_song_play_video(move |pos| {
        let Some(m) = music_songs().lock().ok().and_then(|g| g.iter().find(|s| s.pos == pos).cloned()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE music_video_link ADD COLUMN video_path TEXT").execute(&pool).await;
            let path: Option<String> = sqlx::query_scalar(
                "SELECT video_path FROM music_video_link WHERE item_id = ? AND video_path IS NOT NULL")
                .bind(m.item_id).fetch_optional(&pool).await.ok().flatten();
            match path.filter(|p| std::path::Path::new(p).exists()) {
                Some(p) => yt_play_local_video(weak, p),
                None => { let _ = weak.upgrade_in_event_loop(|w| w.set_caps_nudge(
                    "No music video linked — use “Link music video…” first.".into())); }
            }
        });
    });

    // Context menu — Analyze: BPM + musical key + DR on a worker thread
    // (np.p4.music.bpm-key / .dr-meter); result lands in the nudge line and
    // on track_meta for the Properties dialog.
    let w = window.as_weak();
    window.on_music_song_analyze(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(m) = music_songs().lock().ok().and_then(|g| g.iter().find(|s| s.pos == pos).cloned()) else { return; };
        let Some(path) = music_paths().lock().ok().and_then(|g| g.get(pos as usize).cloned()) else { return; };
        w0.set_caps_nudge(format!("Analyzing “{}”…", m.title).into());
        let weak = w.clone();
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let Some(a) = tulipix_sec_music::analysis::analyze_file(&path) else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_caps_nudge("Analysis failed — could not decode the file.".into()));
                return;
            };
            let key = a.key_pc.and_then(|(pc, maj)| tulipix_music::bpm_key::key_label(pc, maj));
            let camelot = a.key_pc.and_then(|(pc, maj)| tulipix_music::bpm_key::camelot(pc, maj));
            let (bpm, dr, id) = (a.bpm, a.dr, m.item_id);
            let key_db = key.clone();
            rt.spawn(async move {
                if let Ok(pool) = pool_for("music").await {
                    let _ = tulipix_music::bpm_key::store(&pool, id, bpm, key_db.as_deref()).await;
                    if let Some(dr) = dr { let _ = tulipix_music::dr_meter::store(&pool, id, dr).await; }
                }
            });
            let mut parts: Vec<String> = Vec::new();
            if let Some(b) = bpm { parts.push(format!("{b:.0} BPM")); }
            match (key, camelot) {
                (Some(k), Some(c)) => parts.push(format!("{k} ({c})")),
                (Some(k), None) => parts.push(k),
                _ => {}
            }
            if let Some(d) = dr { parts.push(format!("DR {d:.0}")); }
            let msg = if parts.is_empty() { format!("“{}” — nothing detectable.", m.title) }
                      else { format!("“{}” — {}.", m.title, parts.join(" · ")) };
            let _ = weak.upgrade_in_event_loop(move |w| w.set_caps_nudge(msg.into()));
        });
    });
    // Sonic similar (np.p4.music.embeddings) — DSP-embedding nearest-neighbour
    // queue + the whole-library indexer chip in Music settings.
    let w = window.as_weak();
    window.on_music_song_sonic(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        sonic_similar_queue(&w0, pos);
    });
    let w = window.as_weak();
    window.on_music_sonic_index(move || {
        let Some(w0) = w.upgrade() else { return; };
        sonic_index_all(&w0);
    });
    // Karaoke (np.p4.music.stem, DSP tier) — live centre-vocal cancellation.
    let w = window.as_weak();
    window.on_music_toggle_karaoke(move || {
        let Some(w0) = w.upgrade() else { return; };
        karaoke_toggle(&w0);
    });
    let w = window.as_weak();
    window.on_music_song_delete(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let path = music_paths().lock().ok().and_then(|g| g.get(pos as usize).cloned());
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        // Move the file to the OS recycle bin (recoverable) — never a hard delete.
        if let Some(p) = &path {
            if let Err(e) = tulipix_platform::fm::move_to_trash(p) {
                tracing::warn!(path = %p.display(), error = %e, "move song to trash");
            }
            // Drop it from the in-memory accumulator so the tile vanishes too.
            if let Ok(mut g) = music_full().lock() {
                g.retain(|(_, orig, _)| orig != p);
            }
        }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("DELETE FROM track_meta WHERE item_id = ?").bind(item_id).execute(&pool).await;
            let _ = sqlx::query("DELETE FROM lyrics WHERE item_id = ?").bind(item_id).execute(&pool).await;
            let _ = sqlx::query("DELETE FROM items WHERE id = ?").bind(item_id).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| { rebuild_music_tiles(&w); populate_music_views(w.as_weak()); });
        });
        let _ = w0;
    });
    // View a synced track's lyrics in a popup WITHOUT starting playback
    // (np.p5.music.lyrics-synced — read-only viewer + re-search).
    let w = window.as_weak();
    window.on_music_row_view_lyrics(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let item_id = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()).unwrap_or(-1);
        tracing::info!(pos, item_id, "lyrics view requested");
        // Keep playing — viewing lyrics no longer interrupts playback.
        if let Ok(mut g) = music_view_lyrics_id().lock() { *g = Some(item_id); }
        if let Ok(mut g) = pending_view_lyrics().lock() { *g = None; }
        // Open in lyrics mode (not the stale results list from a prior search).
        w0.set_music_lyrics_view_mode("lyrics".into());
        w0.set_music_lyrics_view_results(slint::ModelRc::new(slint::VecModel::<LyricsResult>::default()));
        w0.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::<MusicLyricLine>::default()));
        w0.set_music_lyrics_view_plain("Loading…".into());
        w0.set_music_lyrics_view_dirty(false);
        w0.set_music_lyrics_view_open(true);
        view_lyrics_load(&w0, item_id);
    });
    // Re-search lyrics for the viewed track from the popup's name/artist/album.
    let w = window.as_weak();
    window.on_music_lyrics_view_search(move || {
        let Some(w0) = w.upgrade() else { return; };
        let name = w0.get_music_lyrics_view_q_name().to_string();
        let artist = w0.get_music_lyrics_view_q_artist().to_string();
        let album = w0.get_music_lyrics_view_q_album().to_string();
        // Switch the window to the results list and show the searching state.
        w0.set_music_lyrics_view_mode("results".into());
        w0.set_music_lyrics_view_searching(true);
        w0.set_music_lyrics_view_results(slint::ModelRc::new(slint::VecModel::<LyricsResult>::default()));
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            // LRCLIB /api/search returns an array of candidates.
            let q = |s: &str| urlencoding(s);
            let mut url = format!("https://lrclib.net/api/search?track_name={}&artist_name={}", q(&name), q(&artist));
            if !album.is_empty() { url.push_str(&format!("&album_name={}", q(&album))); }
            let client = tulipix_core::net::http().clone();
            let mut cands: Vec<(bool, String, String, String)> = Vec::new(); // (synced, content, title, sub)
            if let Ok(resp) = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                .send().await {
                if let Ok(arr) = resp.json::<Vec<serde_json::Value>>().await {
                    for j in arr.into_iter().take(30) {
                        let synced = j["syncedLyrics"].as_str().unwrap_or("").to_string();
                        let plain = j["plainLyrics"].as_str().unwrap_or("").to_string();
                        if synced.is_empty() && plain.is_empty() { continue; }
                        let is_synced = !synced.is_empty();
                        let content = if is_synced { synced } else { plain };
                        let t = j["trackName"].as_str().unwrap_or("").to_string();
                        let a = j["artistName"].as_str().unwrap_or("").to_string();
                        let al = j["albumName"].as_str().unwrap_or("").to_string();
                        let d = j["duration"].as_f64().unwrap_or(0.0);
                        let mut sub = a;
                        if !al.is_empty() { if !sub.is_empty() { sub.push_str(" · "); } sub.push_str(&al); }
                        if d > 0.0 { sub.push_str(&format!(" · {}", fmt_clock(d))); }
                        cands.push((is_synced, content, t, sub));
                    }
                }
            }
            // Stash full contents for pick(), build the display model.
            if let Ok(mut g) = lyrics_search_results().lock() {
                *g = cands.iter().map(|(s, c, _, _)| (*s, c.clone())).collect();
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                let rows: Vec<LyricsResult> = cands.iter().map(|(s, _, t, sub)| LyricsResult {
                    title: t.clone().into(), sub: sub.clone().into(),
                    kind: if *s { "Synced".into() } else { "Normal".into() } }).collect();
                w.set_music_lyrics_view_results(slint::ModelRc::new(slint::VecModel::from(rows)));
                w.set_music_lyrics_view_searching(false);
            });
        });
    });
    // Pick a search result — load its content into the box (preview, unsaved).
    let w = window.as_weak();
    window.on_music_lyrics_view_pick(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let Some((synced, content)) = lyrics_search_results().lock().ok().and_then(|g| g.get(i as usize).cloned()) else { return; };
        if let Ok(mut g) = pending_view_lyrics().lock() { *g = Some((synced, content.clone())); }
        let lines = if synced { tulipix_music::lyrics::parse_lrc(&content) } else { Vec::new() };
        let rows: Vec<MusicLyricLine> = lines.iter().map(|(ms, t)| MusicLyricLine {
            time: fmt_clock(*ms as f64 / 1000.0).into(), text: t.clone().into() }).collect();
        w0.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
        w0.set_music_lyrics_view_plain(if synced { String::new() } else { content }.into());
        w0.set_music_lyrics_view_status(if synced { "Synced" } else { "Normal" }.into());
        w0.set_music_lyrics_view_mode("lyrics".into());
        w0.set_music_lyrics_view_dirty(true); // previewed, not yet saved
    });
    // Back — return from a previewed result to the results list.
    let w = window.as_weak();
    window.on_music_lyrics_view_back(move || {
        if let Some(w0) = w.upgrade() { w0.set_music_lyrics_view_mode("results".into()); }
    });
    // Save the previewed lyrics to the library DB (np.p5.music.lyrics-synced).
    let w = window.as_weak();
    window.on_music_lyrics_view_save(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(id) = music_view_lyrics_id().lock().ok().and_then(|g| *g) else { return; };
        let Some((synced, content)) = pending_view_lyrics().lock().ok().and_then(|g| g.clone()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::lyrics::store(&pool, id, &content, synced, "lrclib").await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_lyrics_view_dirty(false);
                w.set_music_lyrics_view_status(if synced { "Synced" } else { "Normal" }.into());
                // Flip the cached song's synced flag so the grid pill updates live,
                // with no button press (np.p5.music.lyrics-pill-auto).
                if let Ok(mut g) = music_songs().lock() {
                    if let Some(s) = g.iter_mut().find(|s| s.item_id == id) { s.synced = synced; }
                }
                rebuild_music_songs_page(&w); // reflect the synced badge across the app
                // If we just edited the now-playing track, refresh its live lyrics line.
                if current_music_id(&w) == Some(id) { load_music_lyrics(&w); }
            });
        });
    });
    // Open the library lyrics manager — build the list + counts.
    let w = window.as_weak();
    window.on_music_open_lyrics_manager(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_lyrics_mgr_page(0);
        w0.set_music_lyrics_mgr_mode("list".into());
        w0.set_music_lyrics_mgr_open(true);
        rebuild_lyrics_manager(&w0);
    });
    // Lyrics manager pagination.
    let w = window.as_weak();
    window.on_music_lyrics_mgr_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let pages = w0.get_music_lyrics_mgr_pages().max(1);
        let cur = w0.get_music_lyrics_mgr_page();
        let next = (cur + d).clamp(0, pages - 1);
        if next != cur { w0.set_music_lyrics_mgr_page(next); publish_lyrics_mgr_page(&w0); }
    });
    // Manager: click a stat card to filter the list (all / synced / missing).
    let w = window.as_weak();
    window.on_music_lyrics_mgr_set_filter(move |f| {
        let Some(w0) = w.upgrade() else { return; };
        let f = f.to_string();
        // Toggle off when re-clicking the active filter.
        let next = if w0.get_music_lyrics_mgr_filter() == f.as_str() { "all".to_string() } else { f };
        w0.set_music_lyrics_mgr_filter(next.into());
        w0.set_music_lyrics_mgr_page(0);
        publish_lyrics_mgr_page(&w0);
    });
    // Manager: view a song's stored lyrics inside the popup (no playback).
    let w = window.as_weak();
    window.on_music_lyrics_mgr_view(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let item_id = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()).unwrap_or(-1);
        if let Ok(mut g) = music_view_lyrics_id().lock() { *g = Some(item_id); }
        w0.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::<MusicLyricLine>::default()));
        w0.set_music_lyrics_view_plain("Loading…".into());
        w0.set_music_lyrics_mgr_mode("lyrics".into());
        view_lyrics_load(&w0, item_id);
    });
    // Manager: open the manual search page for a song (prefill from its tags).
    let w = window.as_weak();
    window.on_music_lyrics_mgr_search_open(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let item_id = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()).unwrap_or(-1);
        if let Ok(mut g) = music_view_lyrics_id().lock() { *g = Some(item_id); }
        if let Ok(mut g) = pending_view_lyrics().lock() { *g = None; }
        w0.set_music_lyrics_view_dirty(false);
        w0.set_music_lyrics_view_mode("results".into()); // start on the search/browse sub-view
        w0.set_music_lyrics_view_results(slint::ModelRc::new(slint::VecModel::<LyricsResult>::default()));
        // Prefill the search fields from this song's metadata.
        if let Some((t, a)) = music_songs().lock().ok().and_then(|g| g.iter().find(|s| s.pos == pos).map(|s| (s.title.clone(), s.artist.clone()))) {
            w0.set_music_lyrics_view_q_name(t.into());
            w0.set_music_lyrics_view_q_artist(a.into());
            w0.set_music_lyrics_view_q_album("".into());
        }
        w0.set_music_lyrics_mgr_mode("search".into());
    });
    // Manager: back to the list (refresh counts/statuses).
    let w = window.as_weak();
    window.on_music_lyrics_mgr_back(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_lyrics_mgr_mode("list".into());
        rebuild_lyrics_manager(&w0);
    });
    // ── Metadata manager (np.p5.atmusic.metadata-manager) ── — see wire_music_metadata()
    wire_music_metadata(&window);

    // ── Phase 5 music — synced lyrics / audio output / visualizer / cast / tags ── — see wire_music_p5a()
    wire_music_p5a(&window);

    // ── Phase 5 music — podcasts / audiobooks / artist bio ────────────────── — see wire_music_podcasts()
    wire_music_podcasts(&window);
    // ── YouTube section wiring (np.p4.music.youtube) — see wire_youtube() ──
    wire_youtube(&window);


    // ── Phase 5 music — playlist builder / M3U / queue reorder ────────────── — see wire_music_playlist()
    wire_music_playlist(&window);

    // ── Phase 6 callbacks ──────────────────────────────────────────────────── — see wire_music_p6()
    wire_music_p6(&window);

    // ── Status: sidebar lamp + the browser dashboard ──────── — see wire_status()
    wire_status(&window);
    wire_home_layout(&window);
    wire_home_stream(&window);
    // ── Mini Player widget + our own caption row ─────────── — see miniwin::wire()
    miniwin::wire(&window);

    // ── Settings panels: load persisted settings, seed the UI models ───────
    {
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        // Restore the saved profile identity into the sidebar user card.
        {
            let mut u = window.get_user();
            u.display_name = clamp_profile_name(&s.text("profile.name")).into();
            u.avatar_emoji = s.text("profile.emoji").into();
            // Custom cover/avatar images persist as fixed-name PNGs.
            let dir = tulipix_core::paths::data_dir().map(|d| d.join("profile"));
            if let Some(p) = dir.as_ref().map(|d| d.join("cover.png")).filter(|p| p.exists()) {
                u.cover_image = slint::Image::load_from_path(&p).unwrap_or_default();
            }
            if let Some(p) = dir.as_ref().map(|d| d.join("avatar.png")).filter(|p| p.exists()) {
                u.avatar_image = slint::Image::load_from_path(&p).unwrap_or_default();
            }
            window.set_user(u);
            // Sidebar logo pick — 0 (default) when unset/unparsable.
            window.set_app_logo_choice(s.text("profile.logo").parse().unwrap_or(0));
        }
        // Home command center (np.p6.home): greeting + date line + live stats.
        window.set_home_music_left(s.flag("home.music-left", false));
        // Which of the five Home compositions is in use, and which of its cards
        // are switched off (docs/home-layouts/README.md §3).
        window.set_home_layout(home_layout_valid(&s.text("home.layout")).into());
        apply_home_cards(&window);
        window.set_design_lang(design_lang_index(&s.text("ui.design-language")));
        set_home_greeting_now(&window);
        kick_home_stats(&window);
        kick_home_photos(&window);
        kick_home_videos(&window);
        kick_home_books(&window);
        kick_home_cloud(&window);
        kick_home_continue(&window);
        // Home is the section the app opens on, so Stream needs its feed at boot
        // too — `on_section_changed` never fires for the section already showing.
        if window.get_home_layout().as_str() == "stream" { kick_home_events(&window); }
        // Restore the last-used app theme and keep it until the user changes it.
        let choice = match s.theme.as_str() {
            "extra-dark" => ThemeChoice::ExtraDark,
            "oled"       => ThemeChoice::Oled,
            "system"     => ThemeChoice::System,
            _             => ThemeChoice::Light,
        };
        window.set_theme_choice(choice);
        apply_theme_choice(&window, choice);
        window.set_reduce_motion(s.reduce_motion);
        window.set_default_cadence(cadence_str(s.libraries.default_cadence).into());
        // First-run onboarding (np.p1.auth-shell) — show until completed once.
        window.set_onboarding_open(!s.flag("onboarded", false));
        if let Some(vm) = s.view_mode_per_section.get("photos") {
            window.set_photos_view_mode(match vm {
                tulipix_core::settings::ViewMode::Folders => "folders",
                tulipix_core::settings::ViewMode::Timeline => "timeline",
                tulipix_core::settings::ViewMode::Library => "library",
            }.into());
        }
    }
    // Onboarding finished → persist the flag + close the wizard.
    let w = window.as_weak();
    window.on_onboarding_finished(move || {
        let Some(w) = w.upgrade() else { return; };
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.flags.insert("onboarded".into(), true);
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save onboarded flag"); }
        w.set_onboarding_open(false);
        tracing::info!("onboarding complete");
    });

    // Photo search — filter the grid by filename (rebuild from the full list).
    let w = window.as_weak();
    window.on_photo_search(move |q| {
        // Re-render the current section filtered by the query (timeline/folder
        // groups, flat grids, and the library folder list all honour it).
        let Some(w0) = w.upgrade() else { return; };
        let cat = w0.get_photos_category().to_string();
        kick_category_refresh(w.clone(), cat, q.to_string());
    });
    // Category tab switch — recompute the allowed-path set from the DB, then
    // rebuild the grid (honouring the current search query) on the UI thread.
    let w = window.as_weak();
    window.on_set_photos_category(move |c| {
        tracing::info!(category = %c, "photos category");
        let Some(w0) = w.upgrade() else { return; };
        let q = w0.get_photos_query().to_string();
        kick_category_refresh(w.clone(), c.to_string(), q);
    });

    wire_photo_tabs(&window);

    wire_photo_editor(&window);

    wire_settings_panels(&window);

    wire_library_panel(&window);


    // Restore watched folders from previous sessions so the section grids are
    // populated on launch instead of starting empty. Re-scan is cheap because
    // thumbnails are cached.
    {
        let folders = load_watched_folders();
        if !folders.is_empty() {
            tracing::info!(count = folders.len(), "restoring watched folders");
        }
        set_scan_silent(true); // startup restore never shows the scan popup
        for path in folders {
            if path.exists() {
                add_folder_path_deferred(&window, path);
            } else {
                tracing::warn!(path = %path.display(), "watched folder gone — skipping");
            }
        }
    }

    // OS media-key support (XF86Audio Play/Pause/Next/Prev) via MPRIS on Linux,
    // SMTC on Windows, MediaPlayer on macOS. Held alive for the app's lifetime.
    // Deferred to a single-shot timer so the native window (and its HWND, which
    // Windows SMTC requires) is realized before registration runs.
    {
        let weak = window.as_weak();
        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
            if let Some(w) = weak.upgrade() { setup_media_controls(&w); }
        });
    }

    // Freeze "startup to ready" on the first turn of the event loop: every
    // blocking setup call above has returned by then and the window is about to
    // paint, which is what the number is supposed to describe.
    slint::Timer::single_shot(std::time::Duration::ZERO, || {
        let ms = APP_START.get().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0);
        let _ = READY_MS.set(ms);
        tracing::info!(startup_ms = ms, "ready");
    });

    // Launch filling the screen rather than as a small floating window. The
    // caption row reads the state back off the `maximized` Window builtin, so
    // there is nothing to mirror here.
    window.window().set_maximized(true);
    if TRAY_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        // With a tray icon there is a way back in, so closing the window hides
        // it instead of ending the process — which is also what makes the Mini
        // Player widget possible: it hides the main window, and dismissing the
        // widget would otherwise take the last visible window down with the app.
        window.window().on_close_requested(|| slint::CloseRequestResponse::HideWindow);
        window.show()?;
        slint::run_event_loop_until_quit()?;
    } else {
        // No tray host: last window closed has to mean quit, or the process
        // would keep running with nothing on screen and no way to reach it.
        window.run()?;
    }

    // Stop all playback so nothing keeps playing after the window closes.
    kill_all_mpv();
    // Tear down any rclone mounts spun up for the cloud section.
    tulipix_sec_cloud::cloud_unmount_all();
    // Close the transfer port if the window was shut while the section was open.
    tulipix_sec_transfer::shutdown();

    // Persist gate-hit counter on shutdown
    if let Some(cache) = dirs_default().map(|d| d.join("cache")) {
        let _ = tulipix_core::caps::persist_hit_counts(&cache);
    }
    // Bounded runtime shutdown — the implicit `Drop` waits FOREVER for running
    // blocking tasks (a wedged rclone/network call, a mid-walk scan), which left
    // a windowless zombie process after close. Cap it, then return.
    drop(_rt_guard);
    rt.shutdown_timeout(std::time::Duration::from_secs(2));
    Ok(())
}

/// Photos tabs: multi-select, People, Things and Albums.
///
/// Carved out of `main()` verbatim: same statements, same order, no captured
/// state beyond `window`. Splitting main() is the peak-compile-RAM lever in
/// this crate — one 4,000-line function is one enormous MIR body.
fn wire_photo_tabs(window: &MainWindow) {
    // ── Multi-select (np.p2.multiselect) ──────────────────────────────────
    // Tile-level: ctrl-click / circle = toggle; shift-click = range from anchor.
    let w = window.as_weak();
    window.on_photo_select(move |idx, kind| {
        let Some(w0) = w.upgrade() else { return; };
        let n = photo_paths().lock().map(|g| g.len() as i32).unwrap_or(0);
        if let Ok(mut g) = selection().lock() {
            match kind.as_str() {
                "range" => {
                    let (a, b) = if g.anchor <= idx { (g.anchor, idx) } else { (idx, g.anchor) };
                    for i in a..=b { if i >= 0 && i < n { g.sel.insert(i); } }
                }
                // "toggle" (default)
                _ => {
                    if !g.sel.remove(&idx) { g.sel.insert(idx); }
                    g.anchor = idx;
                }
            }
        }
        refresh_selection_meta(&w0);
    });

    // Action-bar / keyboard: clear · all · star · archive · trash · copy · cut · paste.
    let w = window.as_weak();
    window.on_photo_sel_action(move |action| {
        let Some(w0) = w.upgrade() else { return; };
        let action = action.to_string();
        match action.as_str() {
            "clear" => { selection_clear(); refresh_selection_meta(&w0); return; }
            "all" => {
                let n = photo_paths().lock().map(|g| g.len() as i32).unwrap_or(0);
                if let Ok(mut g) = selection().lock() {
                    g.sel = (0..n).collect();
                    g.anchor = 0;
                }
                refresh_selection_meta(&w0);
                return;
            }
            _ => {}
        }

        // Resolve the selected indices → absolute paths (current grid order).
        let paths: Vec<PathBuf> = {
            let sel = selection().lock().map(|g| g.sel.clone()).unwrap_or_default();
            let pp = photo_paths().lock();
            match pp {
                Ok(g) => sel.iter().filter_map(|&i| g.get(i as usize).cloned()).collect(),
                Err(_) => Vec::new(),
            }
        };
        if paths.is_empty() { return; }

        // Clipboard ops are synchronous.
        if action == "copy" || action == "cut" {
            if let Ok(mut g) = clipboard().lock() { *g = (paths, action == "cut"); }
            refresh_selection_meta(&w0);
            return;
        }

        // Paste — filesystem copy/move into the open folder, then rescan it.
        if action == "paste" {
            if w0.get_photos_category() != "folder" { return; }
            let target = selected_folder().lock().map(|g| g.clone()).unwrap_or_default();
            if target.is_empty() { return; }
            let (srcs, is_cut) = clipboard().lock().map(|g| g.clone()).unwrap_or_default();
            if srcs.is_empty() { return; }
            let weak = w.clone();
            let handle = tokio::runtime::Handle::current();
            handle.spawn(async move {
                let target = PathBuf::from(&target);
                let target_for_blk = target.clone();
                let moved = tokio::task::spawn_blocking(move || paste_into(&srcs, &target_for_blk, is_cut))
                    .await.unwrap_or(0);
                tracing::info!(count = moved, is_cut, "paste complete");
                if is_cut { if let Ok(mut g) = clipboard().lock() { *g = (Vec::new(), false); } }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    // Rescan the target's library root so the DB + grid pick up the
                    // pasted files, then refresh the open folder view.
                    if let Some(root) = library_root_for(&w, std::path::Path::new(&target)) {
                        add_folder_path(&w, root);
                    }
                    selection_clear();
                    refresh_selection_meta(&w);
                });
            });
            return;
        }

        // DB flag batch actions (star / archive / trash) — run off-thread.
        let category = w0.get_photos_category().to_string();
        let query = w0.get_photos_query().to_string();
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let mut ids: Vec<i64> = Vec::with_capacity(paths.len());
            for p in &paths {
                if let Some(id) = item_id_for(&pool, p).await { ids.push(id); }
            }
            if ids.is_empty() { return; }
            let res = match action.as_str() {
                "star" => {
                    let mut ok = true;
                    for id in &ids { ok &= tulipix_photos::star::set(&pool, *id, true).await.is_ok(); }
                    if ok { Ok(()) } else { Err(anyhow::anyhow!("batch star")) }
                }
                "archive" => tulipix_photos::archive::set(&pool, &ids, category != "archive").await.map(|_| ()),
                "trash" => tulipix_photos::trash::soft_delete(&pool, &ids).await.map(|_| ()),
                _ => Ok(()),
            };
            match res {
                Ok(()) => tracing::info!(%action, count = ids.len(), "batch photo action"),
                Err(e) => tracing::error!(error = %e, %action, "batch photo action failed"),
            }
            let (blink, dur_ms) = match action.as_str() {
                "star" => ("starred", 800u64),
                "archive" => ("archive", 2000u64),
                "trash" => ("trash", 1000u64),
                _ => ("", 0u64),
            };
            selection_clear();
            let bweak = weak.clone();
            kick_category_refresh(weak, category, query);
            if !blink.is_empty() {
                let b = blink.to_string();
                let wk = bweak.clone();
                let _ = bweak.upgrade_in_event_loop(move |w| {
                    w.set_photos_blink(b.into());
                    let wk2 = wk.clone();
                    BLINK_TIMER.with(|t| {
                        t.borrow().start(
                            slint::TimerMode::SingleShot,
                            std::time::Duration::from_millis(dur_ms),
                            move || { if let Some(w) = wk2.upgrade() { w.set_photos_blink("".into()); } },
                        );
                    });
                });
            }
        });
    });

    // Library tab — click a folder row to open it inside the app as a grouped,
    // sortable photo grid (the "folder" view).
    let w = window.as_weak();
    window.on_photo_folder_open(move |path| {
        let Some(w0) = w.upgrade() else { return; };
        let p = path.to_string();
        let name = std::path::Path::new(&p).file_name()
            .and_then(|s| s.to_str()).unwrap_or(&p).to_string();
        if let Ok(mut g) = selected_folder().lock() { *g = p; }
        // Open folders on the default sort (newest first).
        if let Ok(mut g) = sort_mode().lock() { *g = "date".into(); }
        if let Ok(mut g) = sort_dir().lock() { *g = "desc".into(); }
        w0.set_photos_sort("date".into());
        w0.set_photos_sort_dir("desc".into());
        w0.set_photos_folder_name(name.into());
        w0.set_photos_category("folder".into());
        let q = w0.get_photos_query().to_string();
        kick_category_refresh(w.clone(), "folder".into(), q);
    });

    // ── People tab (np.p2.ai.face-clusters / .name) ───────────────────────
    // Open a cluster → show its photos in the flat grid (category "facephotos").
    let w = window.as_weak();
    window.on_photo_person_open(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        let my_gen = next_refresh_gen();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let photos = tulipix_photos::ai::people::photos_of(&pool, id as i64, 100_000).await
                .unwrap_or_else(|e| { tracing::warn!(error=%e, "people::photos_of"); Vec::new() });
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM people WHERE id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            let title = name.unwrap_or_else(|| format!("Person {id}"));
            let paths: Vec<String> = photos.into_iter().map(|(_, p)| p).collect();
            let set: std::collections::HashSet<String> = paths.iter().cloned().collect();
            // Stored before the refresh, not inside its event-loop hop: the
            // plan reads `category_paths`, and it now runs off the UI thread.
            if let Ok(mut g) = category_paths().lock() { *g = Some(CatFilter { set, order: paths }); }
            selection_clear();
            refresh_photo_filter(
                weak,
                String::new(),
                Some(GridHeader { title, category: "facephotos".into() }),
                my_gen,
            ).await;
        });
    });
    // Inline rename a cluster, then reload the People list.
    let w = window.as_weak();
    window.on_photo_person_rename(move |id, name| {
        let weak = w.clone();
        let name = name.to_string();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            if let Err(e) = tulipix_photos::ai::people::set_name(&pool, id as i64, &name).await {
                tracing::error!(error=%e, "people::set_name"); return;
            }
            tracing::info!(person = id, %name, "person renamed");
            kick_category_refresh(weak, "people".into(), String::new());
        });
    });

    // ── Things tab (np.p2.ai.tags) ────────────────────────────────────────
    // Open a tag → show its photos in the flat grid (category "tagphotos").
    let w = window.as_weak();
    window.on_photo_thing_open(move |label| {
        let Some(_w0) = w.upgrade() else { return; };
        let label = label.to_string();
        let weak = w.clone();
        let my_gen = next_refresh_gen();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let paths = tag_photo_paths(&label).await;
            let set: std::collections::HashSet<String> = paths.iter().cloned().collect();
            if let Ok(mut g) = category_paths().lock() { *g = Some(CatFilter { set, order: paths }); }
            selection_clear();
            refresh_photo_filter(
                weak,
                String::new(),
                Some(GridHeader { title: label, category: "tagphotos".into() }),
                my_gen,
            ).await;
        });
    });

    // ── Albums tab (np.p2.albums) ─────────────────────────────────────────
    // Open an album → its photos in the flat grid (category "albumphotos").
    let w = window.as_weak();
    window.on_photo_album_open(move |id| {
        let weak = w.clone();
        let my_gen = next_refresh_gen();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let paths: Vec<String> = sqlx::query_scalar(
                "SELECT i.abs_path FROM album_items ai JOIN items i ON i.id = ai.item_id \
                 WHERE ai.album_id = ? AND i.missing_since IS NULL")
                .bind(id as i64).fetch_all(&pool).await.unwrap_or_default();
            let name: Option<String> = sqlx::query_scalar("SELECT name FROM albums WHERE id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            let title = name.unwrap_or_else(|| "Album".into());
            let set: std::collections::HashSet<String> = paths.iter().cloned().collect();
            if let Ok(mut g) = category_paths().lock() { *g = Some(CatFilter { set, order: paths }); }
            selection_clear();
            refresh_photo_filter(
                weak,
                String::new(),
                Some(GridHeader { title, category: "albumphotos".into() }),
                my_gen,
            ).await;
        });
    });
    let w = window.as_weak();
    window.on_photo_album_new(move || {
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("photos").await {
                let _ = tulipix_photos::albums::create(&pool, "Untitled album").await;
            }
            kick_category_refresh(weak, "albums".into(), String::new());
        });
    });
    let w = window.as_weak();
    window.on_photo_album_rename(move |id, name| {
        let weak = w.clone();
        let name = name.to_string();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("photos").await {
                let _ = tulipix_photos::albums::rename(&pool, id as i64, &name).await;
            }
            kick_category_refresh(weak, "albums".into(), String::new());
        });
    });
    let w = window.as_weak();
    window.on_photo_album_delete(move |id| {
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("photos").await {
                let _ = tulipix_photos::albums::delete(&pool, id as i64).await;
            }
            kick_category_refresh(weak, "albums".into(), String::new());
        });
    });
    // "Add to album" picker: load albums into the picker model + open it.
    let w = window.as_weak();
    window.on_photo_album_add_open(move || {
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let cards = load_albums().await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_album_picker_albums(slint::ModelRc::new(slint::VecModel::from(album_cards(cards))));
                w.set_album_picker_open(true);
            });
        });
    });
    let w = window.as_weak();
    window.on_photo_album_add(move |id| {
        let paths = selected_photo_paths();
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("photos").await {
                let ids = selected_item_ids(&pool, paths).await;
                let n = tulipix_photos::albums::add_items(&pool, id as i64, &ids).await.unwrap_or(0);
                tracing::info!(album = id, added = n, "added to album");
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_album_picker_open(false);
                selection_clear();
                refresh_selection_meta(&w);
            });
        });
    });
    let w = window.as_weak();
    window.on_photo_album_add_new(move || {
        let paths = selected_photo_paths();
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("photos").await {
                if let Ok(id) = tulipix_photos::albums::create(&pool, "Untitled album").await {
                    let ids = selected_item_ids(&pool, paths).await;
                    let _ = tulipix_photos::albums::add_items(&pool, id, &ids).await;
                }
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_album_picker_open(false);
                selection_clear();
                refresh_selection_meta(&w);
            });
        });
    });

}

/// The photo editor (np.p2.edit.*) — adjust, crop, AI ops, export.
///
/// Carved out of `main()` verbatim: same statements, same order, no captured
/// state beyond `window`. Splitting main() is the peak-compile-RAM lever in
/// this crate — one 4,000-line function is one enormous MIR body.
fn wire_photo_editor(window: &MainWindow) {
    // ── Photo editor (np.p2.edit.*) ───────────────────────────────────────
    let w = window.as_weak();
    window.on_editor_adjust(move |e, c, s, t, hi, sh| {
        let p = tulipix_photos::editor::adjust::AdjustParams {
            exposure: e, contrast: c, saturation: s, temperature: t, tint: 0.0,
            highlights: hi, shadows: sh, blacks: 0.0, whites: 0.0,
        };
        let op = tulipix_photos::editor::ops::EditOp::Adjust {
            exposure: p.exposure, contrast: p.contrast, saturation: p.saturation,
            temperature: p.temperature, tint: p.tint, highlights: p.highlights,
            shadows: p.shadows, blacks: p.blacks, whites: p.whites,
        };
        if let Ok(mut g) = editor_stack().lock() {
            let live = EDITOR_ADJUST_LIVE.load(std::sync::atomic::Ordering::Relaxed);
            let top_is_adjust = g.undo_idx > 0
                && matches!(g.ops.get(g.undo_idx - 1), Some(tulipix_photos::editor::ops::EditOp::Adjust { .. }));
            if live && top_is_adjust {
                let i = g.undo_idx - 1; g.ops[i] = op;
            } else {
                g.push(op);
                EDITOR_ADJUST_LIVE.store(true, std::sync::atomic::Ordering::Relaxed);
            }
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_curve(move |ch, y0, y1, y2, y3, y4| {
        use tulipix_photos::editor::{curves::Channel, ops::EditOp};
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let channel = match ch.as_str() {
            "r" => Channel::R, "g" => Channel::G, "b" => Channel::B, _ => Channel::All,
        };
        let points = vec![(0.0, y0), (0.25, y1), (0.5, y2), (0.75, y3), (1.0, y4)];
        if let Ok(mut g) = editor_stack().lock() {
            // Merge a drag (and consecutive same-channel edits) into one op.
            let top_same = g.undo_idx > 0
                && matches!(g.ops.get(g.undo_idx - 1), Some(EditOp::Curve { channel: c, .. }) if *c == channel);
            let op = EditOp::Curve { channel, points };
            if top_same { let i = g.undo_idx - 1; g.ops[i] = op; } else { g.push(op); }
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_exif_save(move |a, c, d, m, dt| {
        let Some(src) = editor_src().lock().ok().and_then(|g| g.clone()) else { return; };
        let (a, c, d, m, dt) = (a.to_string(), c.to_string(), d.to_string(), m.to_string(), dt.to_string());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let src2 = src.clone();
            let res = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
                let mut p = tulipix_photos::exif_write::ExifPatch::new();
                for (tag, val) in [
                    ("Artist", &a), ("Copyright", &c), ("ImageDescription", &d),
                    ("UserComment", &m), ("DateTimeOriginal", &dt),
                ] {
                    if val.trim().is_empty() { p = p.clear(tag); } else { p = p.set(tag, val.clone()); }
                }
                tulipix_photos::exif_write::apply(&src2, &p).map(|_| ())
            }).await;
            let ok = matches!(res, Ok(Ok(())));
            let dump = if ok { Some(format_exif(&src)) } else { None };
            let msg = match res {
                Ok(Ok(())) => "Metadata written.".to_string(),
                Ok(Err(e)) => { tracing::error!(error=%e, "exif write"); "Write failed — see logs.".to_string() }
                Err(e) => { tracing::error!(error=%e, "exif write join"); "Write failed.".to_string() }
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_editor_status(msg.into());
                if let Some(dump) = dump { w.set_editor_exif(dump.into()); }
            });
        });
    });
    let w = window.as_weak();
    window.on_editor_exif_clear_gps(move || {
        let Some(src) = editor_src().lock().ok().and_then(|g| g.clone()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let src2 = src.clone();
            let res = tokio::task::spawn_blocking(move || tulipix_photos::exif_write::strip_gps(&src2)).await;
            let ok = matches!(res, Ok(Ok(())));
            let dump = if ok { Some(format_exif(&src)) } else { None };
            let msg = if ok { "GPS location cleared." } else { "Clear GPS failed — see logs." };
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_editor_status(msg.into());
                if let Some(dump) = dump { w.set_editor_exif(dump.into()); }
            });
        });
    });
    let w = window.as_weak();
    window.on_editor_filter(move |preset| {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let preset = preset.to_string();
        if let Ok(mut g) = editor_stack().lock() {
            // Drop a trailing Filter op (so presets are mutually exclusive / "None").
            if g.undo_idx > 0
                && matches!(g.ops.get(g.undo_idx - 1), Some(tulipix_photos::editor::ops::EditOp::Filter { .. })) {
                let keep = g.undo_idx - 1;
                g.ops.truncate(keep);
                g.undo_idx = keep;
            }
            if let Some(p) = parse_preset(&preset) {
                g.push(tulipix_photos::editor::ops::EditOp::Filter { preset: p, strength: 1.0 });
            }
        }
        if let Some(w0) = w.upgrade() {
            w0.set_editor_active_filter(if preset == "none" { "".into() } else { preset.clone().into() });
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_transform(move |kind| {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let (rot, fh, fv) = match kind.as_str() {
            "rot-left"  => (270.0_f32, false, false),
            "rot-right" => (90.0_f32, false, false),
            "flip-h"    => (0.0_f32, true, false),
            "flip-v"    => (0.0_f32, false, true),
            _ => (0.0_f32, false, false),
        };
        if let Ok(mut g) = editor_stack().lock() {
            g.push(tulipix_photos::editor::ops::EditOp::Crop {
                x: 0, y: 0, w: u32::MAX, h: u32::MAX, rotate_deg: rot, flip_h: fh, flip_v: fv,
            });
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_enhance(move || {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = editor_stack().lock() {
            // One-time: never stack Enhance — it's already idempotent, and a
            // second op would just re-process. No-op if one is already active.
            let already = g.active_ops().iter()
                .any(|op| matches!(op, tulipix_photos::editor::ops::EditOp::Enhance));
            if !already { g.push(tulipix_photos::editor::ops::EditOp::Enhance); }
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_sharpen(move |amount| {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = editor_stack().lock() {
            g.push(tulipix_photos::editor::ops::EditOp::Sharpen { amount, radius: 1.0 });
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_ai_op(move |op| {
        let Some(w0) = w.upgrade() else { return; };
        let op = op.to_string();
        if op == "upscale" {
            w0.set_editor_busy(true);
            w0.set_editor_status("Upscaling…".into());
            editor_upscale(w.clone());
            return;
        }
        if op == "colorize" {
            w0.set_editor_busy(true);
            w0.set_editor_status("Colorizing…".into());
            editor_colorize(w.clone());
            return;
        }
        // heal/sky (np.p2.edit.heal/.sky) — model-state-aware guidance: the
        // download flow lives in Settings → AI Models; inference needs ai-onnx.
        let model = match op.as_str() { "heal" => "lama-inpaint", "sky" => "mobile-sam", _ => "" };
        let installed = ai_manifest().find(model)
            .map(tulipix_photos::ai::models::is_installed).unwrap_or(false);
        let msg = match (op.as_str(), installed, cfg!(feature = "ai-onnx")) {
            ("heal", false, _) => "Magic Eraser: download the lama-inpaint model in Settings → AI Models first.",
            ("sky",  false, _) => "Sky replace: download the mobile-sam model in Settings → AI Models first.",
            ("heal", true, false) | ("sky", true, false) =>
                "Model installed — rebuild with --features ai-onnx to enable on-device inference.",
            ("heal", true, true) => "LaMa ready — brush a mask over the object to erase (coming next).",
            ("sky",  true, true) => "SAM ready — brush the sky region to replace (coming next).",
            _ => "This AI tool needs a model installed in Settings → AI Models.",
        };
        w0.set_editor_status(msg.into());
    });
    let w = window.as_weak();
    window.on_editor_undo(move || {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = editor_stack().lock() { g.undo(); }
        if let Some(w0) = w.upgrade() { editor_reset_sliders(&w0); }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_redo(move || {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = editor_stack().lock() { g.redo(); }
        if let Some(w0) = w.upgrade() { editor_reset_sliders(&w0); }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_reset(move || {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
        if let Some(w0) = w.upgrade() {
            editor_reset_sliders(&w0);
            w0.set_editor_active_filter("".into());
            w0.set_editor_status("".into());
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_export(move |format, quality| {
        editor_export(w.clone(), format.to_string(), quality as u8);
    });
    let w = window.as_weak();
    window.on_editor_save_original(move || {
        editor_save_original(w.clone());
    });
    // Crop — fractions of the currently-displayed image → absolute pixels.
    let w = window.as_weak();
    window.on_editor_crop(move |fx, fy, fw, fh| {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        let (cw, ch) = editor_cur_dims().lock().map(|g| *g).unwrap_or((0, 0));
        if cw == 0 || ch == 0 { return; }
        let x = (fx.clamp(0.0, 1.0) * cw as f32) as u32;
        let y = (fy.clamp(0.0, 1.0) * ch as f32) as u32;
        let cw2 = (fw.clamp(0.0, 1.0) * cw as f32) as u32;
        let ch2 = (fh.clamp(0.0, 1.0) * ch as f32) as u32;
        if cw2 < 2 || ch2 < 2 { return; }
        if let Ok(mut g) = editor_stack().lock() {
            g.push(tulipix_photos::editor::ops::EditOp::Crop {
                x, y, w: cw2, h: ch2, rotate_deg: 0.0, flip_h: false, flip_v: false,
            });
        }
        editor_render(w.clone());
    });
    // Resize — exact target dimensions.
    let w = window.as_weak();
    window.on_editor_resize(move |tw, th| {
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        if tw < 1 || th < 1 { return; }
        if let Ok(mut g) = editor_stack().lock() {
            g.push(tulipix_photos::editor::ops::EditOp::Resize { w: tw as u32, h: th as u32 });
        }
        editor_render(w.clone());
    });
    let w = window.as_weak();
    window.on_editor_close(move || {
        // Persist the edit stack to photo_edits on close.
        let item = editor_item().lock().map(|g| *g).unwrap_or(None);
        let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
        // Read the stack first, then drop the decoded full-res original — the
        // save below only needs the stack, and the image was being kept for the
        // rest of the session.
        editor_release();
        let _ = w;
        if let Some(id) = item {
            let handle = tokio::runtime::Handle::current();
            handle.spawn(async move {
                if let Ok(pool) = pool_for("photos").await {
                    if let Err(e) = tulipix_photos::editor::ops::save(&pool, id, &stack).await {
                        tracing::error!(error=%e, "editor save");
                    }
                }
            });
        }
    });

    // Folder-view sort selector. Clicking the active mode flips asc/desc;
    // switching mode resets to that mode's natural default (name = A→Z asc,
    // date/size = desc).
    let w = window.as_weak();
    window.on_set_photos_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let mode = s.to_string();
        let cur_mode = sort_mode().lock().map(|g| g.clone()).unwrap_or_default();
        let cur_dir = sort_dir().lock().map(|g| g.clone()).unwrap_or_else(|_| "desc".into());
        let dir = if mode == cur_mode {
            if cur_dir == "asc" { "desc" } else { "asc" }
        } else if mode == "name" { "asc" } else { "desc" };
        if let Ok(mut g) = sort_mode().lock() { *g = mode.clone(); }
        if let Ok(mut g) = sort_dir().lock() { *g = dir.to_string(); }
        w0.set_photos_sort(mode.into());
        w0.set_photos_sort_dir(dir.into());
        // Sort applies to the folder view + the flat flag tabs.
        let cat = w0.get_photos_category().to_string();
        if matches!(cat.as_str(), "folder" | "starred" | "archive" | "trash") {
            let q = w0.get_photos_query().to_string();
            kick_category_refresh(w.clone(), cat, q);
        }
    });

    // Library folder-list sort (Name / Photos count). Same toggle convention:
    // re-click flips direction; switching resets (name = A→Z, count = most first).
    let w = window.as_weak();
    window.on_set_photos_library_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let mode = s.to_string();
        let (cur_mode, cur_dir) = lib_sort().lock().map(|g| g.clone()).unwrap_or_else(|_| ("name".into(), "asc".into()));
        let dir = if mode == cur_mode {
            if cur_dir == "asc" { "desc" } else { "asc" }
        } else if mode == "count" { "desc" } else { "asc" };
        if let Ok(mut g) = lib_sort().lock() { *g = (mode.clone(), dir.to_string()); }
        w0.set_photos_lib_sort(mode.into());
        w0.set_photos_lib_sort_dir(dir.into());
        if w0.get_photos_category() == "library" {
            let q = w0.get_photos_query().to_string();
            kick_category_refresh(w.clone(), "library".into(), q);
        }
    });

    // Persist the photos view-mode (np.p1.lib.folder-view).
    window.on_set_photos_view_mode(move |m| {
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        let vm = match m.as_str() {
            "folders" => tulipix_core::settings::ViewMode::Folders,
            "timeline" => tulipix_core::settings::ViewMode::Timeline,
            _ => tulipix_core::settings::ViewMode::Library,
        };
        s.view_mode_per_section.insert("photos".into(), vm);
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save view mode"); }
        tracing::info!(mode = %m, "photos view mode set");
    });
    let w = window.as_weak();
    window.on_reduce_motion_changed(move |v| {
        let Some(w) = w.upgrade() else { return; };
        w.set_reduce_motion(v);
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.reduce_motion = v;
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save settings (reduce-motion)"); }
        tracing::info!(reduce_motion = v, "reduce-motion toggled");
    });
    seed_api_rows(&window);
    seed_settings_panels(&window);
    // Viewport hint for the lazy thumb queue (np.p1.thumbs.lazy).
    window.on_photos_visible_hint(move |f| {
        SCAN_HINT.store(f.clamp(0.0, 1.0).to_bits(), std::sync::atomic::Ordering::Relaxed);
    });
    // "Show N more" — grow the tile window by one page and rebuild the view.
    let w = window.as_weak();
    window.on_photos_show_more(move || {
        let Some(w0) = w.upgrade() else { return; };
        let cat = w0.get_photos_category().to_string();
        let q = w0.get_photos_query().to_string();
        photos_show_more(w.clone(), cat, q);
    });
    let w = window.as_weak();
    window.on_videos_show_more(move || {
        let Some(w0) = w.upgrade() else { return; };
        let cat = w0.get_video_category().to_string();
        videos_show_more(w.clone(), cat);
    });
    // Live FS watcher (np.p1.lib.watch): notify across every watched folder.
    // Renames update items rows in place; deletes flip missing_since. The
    // event is applied to every section pool — UPDATE is a no-op where the
    // path isn't indexed, so no per-section routing is needed.
    {
        let folders = load_watched_folders();
        // Always spawn — even with zero folders — so a download destination added
        // later this session can attach to the live watcher (np: dynamic-watch).
        {
            let mut cfg = tulipix_core::libraries::LibrariesConfig::default();
            for (i, path) in folders.iter().enumerate() {
                cfg.add(tulipix_core::libraries::Library {
                    id: format!("watch-{i}"), path: path.clone(),
                    section: tulipix_core::libraries::Section::Photos, // unused by the watcher
                    last_scan: None, item_count: 0, size_bytes: 0,
                    exclude_globs: Vec::new(), cadence_override: None, realtime_notify: true,
                });
            }
            let (tx, rx) = std::sync::mpsc::channel();
            match tulipix_core::watcher::spawn(&cfg, tx) {
                Ok(watcher) => {
                    // Hand the watcher to the module so it stays alive AND so
                    // folders added mid-session (new download dirs) can attach.
                    tulipix_core::watcher::install(watcher);
                    // Book roots are rows in books.db, not entries in
                    // LibrariesConfig, so `spawn` above never saw them. Roots
                    // added this session attach themselves in
                    // `books::scan::add_folder`; ones stored in a previous
                    // session need this. Covers the Genesis download folder,
                    // which registers through that same call.
                    tokio::runtime::Handle::current().spawn(async {
                        if let Ok(pool) = pool_for("books").await {
                            tulipix_books::scan::watch_folders(&pool).await;
                        }
                    });
                    let rt = tokio::runtime::Handle::current();
                    let fsweak = window.as_weak();
                    std::thread::Builder::new().name("tulipix-fsapply".into()).spawn(move || {
                        while let Ok(evt) = rx.recv() {
                            // Coalesce a burst (bulk delete / folder move) into a
                            // single apply + one live UI refresh.
                            let mut batch = vec![evt];
                            std::thread::sleep(std::time::Duration::from_millis(350));
                            while let Ok(e) = rx.try_recv() { batch.push(e); }
                            let needs_resync = batch.iter().any(|e| matches!(e,
                                tulipix_core::watcher::FsEvent::Deleted(_)
                                | tulipix_core::watcher::FsEvent::Renamed { .. }));
                            let fsweak = fsweak.clone();
                            rt.spawn(async move {
                                for section in ["photos", "videos", "music", "books", "cloud"] {
                                    if let Ok(pool) = pool_for(section).await {
                                        for e in &batch {
                                            let _ = tulipix_core::watcher::apply_event(&pool, e).await;
                                        }
                                    }
                                }
                                // A delete/rename removed or moved a file — refresh
                                // every section's grid so it disappears live.
                                if needs_resync {
                                    resync_after_fs_change(fsweak);
                                }
                            });
                        }
                    }).ok();
                    tracing::info!(folders = folders.len(), "fs watcher live (np.p1.lib.watch)");
                }
                Err(e) => tracing::warn!(error = %e, "fs watcher spawn failed"),
            }
        }
    }
    // First-open model-update prompt (np.p1.ai.update-prompt): a few seconds
    // after launch, nudge once if a manifest entry outruns an installed model.
    {
        let weak = window.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
            let msg = ai_update_summary();
            if msg.starts_with("Updates available") {
                let _ = weak.upgrade_in_event_loop(move |w| w.set_caps_nudge(msg.into()));
            }
        });
    }
    seed_shortcut_groups(&window);
    window.set_palette_rows(slint::ModelRc::new(slint::VecModel::from(build_palette_rows(""))));

}

/// The generic settings panels (AI · Endpoints · Security · Data · System),
/// capability gating, the chat overlay and the API-keys panel.
///
/// Carved out of `main()` verbatim: same statements, same order, no captured
/// state beyond `window`. Splitting main() is the peak-compile-RAM lever in
/// this crate — one 4,000-line function is one enormous MIR body.
fn wire_settings_panels(window: &MainWindow) {
    // ── Generic settings-panel handlers (AI · Endpoints · Security · Data ·
    // System). Toggles + text fields persist into Settings.flags / .advanced;
    // actions dispatch one-shot operations. All re-seed the panels after.
    let w = window.as_weak();
    window.on_setting_toggle(move |key, on| {
        let Some(w) = w.upgrade() else { return; };
        let key = key.to_string();
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.flags.insert(key.clone(), on);
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save flag"); }
        // Auto-lock takes effect live: enabling arms the idle threshold,
        // disabling pushes it a year out so idle stops doing anything.
        if key == "autolock" {
            let secs = if on { if s.idle_lock_secs > 0 { s.idle_lock_secs } else { 600 } }
                       else { IDLE_NEVER_SECS };
            tulipix_core::idle::set_threshold(secs);
        }
        // Home music-block side — apply live so the layout flips on toggle.
        if key == "home.music-left" { w.set_home_music_left(on); }
        tracing::info!(%key, on, "setting toggled");
        seed_settings_panels(&w);
    });
    let w = window.as_weak();
    window.on_setting_text(move |key, val| {
        let key = key.to_string();
        let val = val.to_string();
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        if key == "idle_lock_secs" {
            s.idle_lock_secs = val.trim().parse::<u64>().unwrap_or(0);
            // Re-arm the idle threshold live when auto-lock is enabled.
            if s.flag("autolock", false) {
                tulipix_core::idle::set_threshold(if s.idle_lock_secs > 0 { s.idle_lock_secs } else { 600 });
            }
        } else if val.trim().is_empty() {
            s.advanced.remove(&key);
        } else {
            s.advanced.insert(key.clone(), val);
        }
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save advanced"); }
        // Apply the universal tools dir live (no restart).
        if key == "tools.bin-dir" {
            tulipix_core::thumbs::set_tool_dir(s.advanced.get("tools.bin-dir").map(|v| v.as_str()));
        }
        // Design language repaints live — it is only paint, so there is nothing
        // to rebuild and no reason to make anyone restart to see the choice.
        if key == "ui.design-language" {
            if let Some(w) = w.upgrade() {
                w.set_design_lang(design_lang_index(&s.text("ui.design-language")));
            }
        }
        // Mini-player + tray styles. Both are read back off the window by the
        // widget/tray plumbing, so pushing the new value here is all it takes
        // for the picker to light up and the next open to use it.
        if key == "ui.mini-widget.style" {
            if let Some(w) = w.upgrade() {
                let style = MiniStyle::from_name(&s.text("ui.mini-widget.style"));
                w.set_mini_widget_style(style.name().into());
            }
        }
        if key == "ui.tray-menu.style" {
            if let Some(w) = w.upgrade() {
                let popup = s.text("ui.tray-menu.style") != "native";
                w.set_tray_menu_style(if popup { "popup" } else { "native" }.into());
            }
        }
        tracing::info!(%key, "setting text edited");
        // Segmented pickers (whisper model choice) need a re-seed so the
        // selected pill updates; free-text fields must NOT re-seed per
        // keystroke or the field would lose focus.
        if key.starts_with("ai.model.") || key == "ai.voice-lang" {
            if let Some(w) = w.upgrade() { seed_settings_panels(&w); }
        }
    });
    let w = window.as_weak();
    window.on_setting_action(move |key| {
        let Some(w) = w.upgrade() else { return; };
        let key = key.to_string();
        match key.as_str() {
            "backup" => { let r = run_backup(); tracing::info!(ok = r.is_ok(), "backup"); }
            "export" => { let r = run_export(); tracing::info!(ok = r.is_ok(), "export json"); }
            // Pick the universal external-tools directory, persist + apply live.
            "tools-dir-browse" => {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Choose the folder containing mpv / yt-dlp / ffmpeg")
                    .pick_folder()
                {
                    let path = dir.display().to_string();
                    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
                    s.advanced.insert("tools.bin-dir".into(), path.clone());
                    if let Err(e) = s.save() { tracing::warn!(error = %e, "save tools dir"); }
                    tulipix_core::thumbs::set_tool_dir(Some(&path));
                    seed_settings_panels(&w);
                }
            }
            // Lock-screen wallpapers — a folder, of which the first ten
            // pictures become the slideshow. A folder rather than ten separate
            // file pickers: one dialog, and swapping the set later is a matter
            // of moving files rather than re-running the picker ten times.
            "lock-wallpapers-browse" => {
                if let Some(dir) = rfd::FileDialog::new()
                    .set_title("Choose the folder holding your lock screen pictures")
                    .pick_folder()
                {
                    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
                    s.advanced.insert("lock.wallpapers".into(), dir.display().to_string());
                    if let Err(e) = s.save() { tracing::warn!(error = %e, "save lock wallpapers"); }
                    seed_settings_panels(&w);
                }
            }
            "lock-wallpapers-clear" => {
                let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
                s.advanced.insert("lock.wallpapers".into(), String::new());
                if let Err(e) = s.save() { tracing::warn!(error = %e, "clear lock wallpapers"); }
                w.set_ambient_photo(Default::default());
                w.set_ambient_photo_next(Default::default());
                seed_settings_panels(&w);
            }
            // Reset the tools directory back to the app default (bundled + PATH),
            // behind a confirmation dialog.
            "tools-dir-reset" => {
                let ok = rfd::MessageDialog::new()
                    .set_title("Reset tools directory")
                    .set_description("Clear the custom tools folder and fall back to the app's bundled binaries and system PATH?")
                    .set_buttons(rfd::MessageButtons::YesNo)
                    .show();
                if ok == rfd::MessageDialogResult::Yes {
                    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
                    s.advanced.remove("tools.bin-dir");
                    if let Err(e) = s.save() { tracing::warn!(error = %e, "reset tools dir"); }
                    tulipix_core::thumbs::set_tool_dir(None);
                    seed_settings_panels(&w);
                }
            }
            // Migration import wizard (np.p1.migration): pick the source file,
            // auto-detect its kind, dry-run a plan, surface counts; Picasa
            // stars apply to the photo library (favorite flag) right away.
            "migration" => {
                let weak = w.as_weak();
                let rt = tokio::runtime::Handle::current();
                std::thread::spawn(move || {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("Pick a library to import — Plex .db / .picasa.ini / iTunes .xml / Lightroom .lrcat / foobar2000 .fpl")
                        .pick_file() else { return; };
                    let Some(kind) = tulipix_core::migration::SourceKind::detect(&path) else {
                        let _ = weak.upgrade_in_event_loop(|w| w.set_caps_nudge(
                            "Couldn't recognise that file — expected Plex library.db, .picasa.ini, iTunes Library.xml, .lrcat or .fpl.".into()));
                        return;
                    };
                    let plan = match tulipix_core::migration::dry_run(kind, &path) {
                        Ok(p) => p,
                        Err(e) => {
                            let msg = format!("Import scan failed: {e}");
                            let _ = weak.upgrade_in_event_loop(move |w| w.set_caps_nudge(msg.into()));
                            return;
                        }
                    };
                    let applied = if matches!(kind, tulipix_core::migration::SourceKind::Picasa) {
                        apply_picasa_stars(&path, &rt)
                    } else { 0 };
                    // iTunes Library.xml: actually merge play counts + star
                    // ratings into matching music tracks (by absolute path) —
                    // not just a dry-run count (np.p4.music.import).
                    let merged: u64 = if matches!(kind, tulipix_core::migration::SourceKind::ITunesXml) {
                        std::fs::read_to_string(&path).ok().map(|xml| {
                            let tracks = tulipix_music::import::parse_itunes(&xml);
                            rt.block_on(async {
                                let Ok(pool) = pool_for("music").await else { return 0 };
                                tulipix_music::import::merge(&pool, &tracks).await.unwrap_or(0)
                            })
                        }).unwrap_or(0)
                    } else { 0 };
                    let msg = format!(
                        "{kind:?} import — {} photos · {} tracks · {} playlists · {} ratings{}{}{}",
                        plan.photos, plan.music_tracks, plan.playlists, plan.ratings,
                        if applied > 0 { format!(" · {applied} stars applied") } else { String::new() },
                        if merged > 0 { format!(" · {merged} tracks updated (plays + stars)") } else { String::new() },
                        if plan.warnings.is_empty() { String::new() } else { format!(" · {} warning(s) in logs", plan.warnings.len()) });
                    for warn in &plan.warnings { tracing::warn!(%warn, "migration"); }
                    let _ = weak.upgrade_in_event_loop(move |w| w.set_caps_nudge(msg.into()));
                });
            }
            "bug-report" => {
                if let Some(dir) = tulipix_core::paths::data_dir().map(|d| d.join("logs")) {
                    let _ = tulipix_platform::fm::reveal_in_file_manager(&dir);
                }
            }
            "open-logs" => {
                if let Some(dir) = tulipix_core::paths::data_dir().map(|d| d.join("logs")) {
                    let _ = tulipix_platform::fm::reveal_in_file_manager(&dir);
                }
            }
            // Real update check (np.p1.ai.update-prompt): manifest vs installed
            // versions on disk; older-versioned installs surface as updates.
            "ai-update-check" => {
                // Sweep-progress pill inside the button while the check runs.
                AI_CHECK_BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
                seed_settings_panels(&w);
                let weak = w.as_weak();
                tokio::runtime::Handle::current().spawn(async move {
                    let msg = ai_update_summary();
                    AI_CHECK_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_caps_nudge(msg.into());
                        seed_settings_panels(&w);
                    });
                });
            }
            "clear-thumb-cache" => { let _ = tulipix_core::thumbs::clear_cache(); }
            // Model download/verify flow (np.p1.onboarding.ai-models).
            k if k.starts_with("ai-dl-") => {
                let name = k.trim_start_matches("ai-dl-").to_string();
                let Some(entry) = ai_manifest().find(&name).cloned() else { return; };
                // Ignore re-clicks while this model is already downloading.
                if ai_dl_progress().lock().map(|g| g.contains_key(&name)).unwrap_or(false) { return; }
                if let Ok(mut g) = ai_dl_progress().lock() { g.insert(name.clone(), 0.0); }
                seed_settings_panels(&w);
                let weak = w.as_weak();
                tokio::runtime::Handle::current().spawn(async move {
                    // Repaint the row's progress pill on every ≥1% step.
                    let mut last = -1.0f32;
                    let prog_name = name.clone();
                    let prog_weak = weak.clone();
                    let res = tulipix_photos::ai::models::download_with_progress(&entry, move |f| {
                        if f - last >= 0.01 || f >= 1.0 {
                            last = f;
                            if let Ok(mut g) = ai_dl_progress().lock() { g.insert(prog_name.clone(), f); }
                            let _ = prog_weak.upgrade_in_event_loop(|w| seed_settings_panels(&w));
                        }
                    }).await;
                    if let Ok(mut g) = ai_dl_progress().lock() { g.remove(&name); }
                    let msg = match res {
                        Ok(_) => format!("{name} installed and ready"),
                        Err(e) => format!("{name} download failed: {e}"),
                    };
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_caps_nudge(msg.into());
                        seed_settings_panels(&w); // refresh install states
                    });
                });
            }
            _ => tracing::info!(%key, "settings action (no-op)"),
        }
        seed_settings_panels(&w);
    });

    // ── Capability gating (np.p1.caps.ui / caps.events) ────────────────────
    window.set_account_sync_allowed(true); // local-model: full access, every capability unlocked
    // Surface every capability denial as an upgrade nudge toast.
    let w = window.as_weak();
    tulipix_core::caps::on_denied(move |cap, tier| {
        let w = w.clone();
        let msg = format!("{cap:?} needs a higher tier (current: {tier:?})");
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w.upgrade() { w.set_caps_nudge(msg.into()); }
        });
    });

    // ── Chat assistant overlay (np.p1.llm.chat) ────────────────────────────
    // The overlay is wired; the engine behind it is not. It used to point at
    // `--features lazy-ai-ep`, which linked tulipix-ai without connecting it to
    // anything, so the flag could never have made this work. Say what is true.
    window.set_chat_turns(slint::ModelRc::new(slint::VecModel::from(vec![ChatTurn {
        role: "assistant".into(),
        content: "Hi! Once a model is configured in Settings → AI Models I can search \
                  and act on your library.".into(),
    }])));
    let w = window.as_weak();
    window.on_chat_submit(move |q| {
        let Some(w) = w.upgrade() else { return; };
        let q = q.to_string();
        if q.trim().is_empty() { return; }
        let model = w.get_chat_turns();
        let turns = model.as_any().downcast_ref::<slint::VecModel<ChatTurn>>();
        let ai_on = tulipix_core::settings::Settings::load().map(|s| s.flag("ai.chat", false)).unwrap_or(false);
        let reply = if ai_on {
            "The chat engine isn't bundled in this build yet — your message was logged.".to_string()
        } else {
            "AI chat is disabled. Enable it in Settings → AI Models.".to_string()
        };
        if let Some(tv) = turns {
            tv.push(ChatTurn { role: "user".into(), content: q.clone().into() });
            tv.push(ChatTurn { role: "assistant".into(), content: reply.into() });
        }
        tracing::info!(%q, "chat submitted");
    });

    // ── API Keys panel handlers ────────────────────────────────────────────
    // Toggle App-default ↔ My-key. Switching to "My key" with no stored key
    // opens the OS-native entry path is out of scope here; we flip the source
    // and surface whether a custom key exists.
    let w = window.as_weak();
    window.on_api_row_toggle(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        let service = row.service.to_string();
        // Switching to "my key" without one stored → open the editor instead.
        if row.use_app_default && !row.user_key_set {
            row.editing = true;
            model.set_row_data(i as usize, row);
            return;
        }
        row.use_app_default = !row.use_app_default;
        // Re-read quota for the new source.
        let source = if row.use_app_default {
            tulipix_core::api_keys::KeySource::AppDefault
        } else {
            tulipix_core::api_keys::KeySource::UserKey
        };
        let q = tulipix_core::api_keys::quota_state(&service, source);
        row.quota_used = q.used as i32;
        row.quota_limit = q.limit as i32;
        model.set_row_data(i as usize, row);
        tracing::info!(%service, "api key source toggled");
    });
    let w = window.as_weak();
    window.on_api_row_test(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        let service = row.service.to_string();
        // "Test" = does a key resolve for this service? App-default is always
        // present; a custom key must exist in the keychain.
        let ok = if row.use_app_default {
            true
        } else {
            tulipix_core::api_keys::fetch(&service).ok().flatten().is_some()
        };
        row.status = if ok { "ok" } else { "error" }.into();
        model.set_row_data(i as usize, row);
        tracing::info!(%service, ok, "api key tested");
    });
    // Edit = open the row's inline key editor.
    let w = window.as_weak();
    window.on_api_row_edit(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        row.editing = true;
        model.set_row_data(i as usize, row);
    });
    let w = window.as_weak();
    window.on_api_row_edit_cancel(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        row.editing = false;
        model.set_row_data(i as usize, row);
    });
    // Save the typed key into the OS keychain and switch the row to it.
    let w = window.as_weak();
    window.on_api_row_key_save(move |i, key| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        let service = row.service.to_string();
        let key = key.trim().to_string();
        if key.is_empty() { return; }
        match tulipix_core::api_keys::store(&service, &key) {
            Ok(()) => {
                // yt-dlp's cookie flags are cached for the session (the keyring
                // read is a D-Bus hop and sits on every listing now), so a new
                // value has to be published or it takes until the next launch.
                tulipix_core::ytdlp::forget_cookies();
                row.user_key_set = true;
                row.use_app_default = false;
                row.editing = false;
                let q = tulipix_core::api_keys::quota_state(&service, tulipix_core::api_keys::KeySource::UserKey);
                row.quota_used = q.used as i32;
                row.quota_limit = q.limit as i32;
                model.set_row_data(i as usize, row);
                tracing::info!(%service, "custom api key stored");
            }
            Err(e) => {
                w.set_caps_nudge(format!("Couldn't store the key: {e}").into());
                tracing::warn!(%service, error = %e, "api key store failed");
            }
        }
    });
    // Remove the stored key — back to the built-in app key.
    let w = window.as_weak();
    window.on_api_row_key_remove(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        let service = row.service.to_string();
        let _ = tulipix_core::api_keys::delete(&service);
        tulipix_core::ytdlp::forget_cookies();
        row.user_key_set = false;
        row.use_app_default = true;
        row.editing = false;
        let q = tulipix_core::api_keys::quota_state(&service, tulipix_core::api_keys::KeySource::AppDefault);
        row.quota_used = q.used as i32;
        row.quota_limit = q.limit as i32;
        model.set_row_data(i as usize, row);
        tracing::info!(%service, "custom api key removed");
    });

}

/// Library panel actions, the scan schedule, the command palette, voice
/// search, the error boundary and the properties window.
///
/// Carved out of `main()` verbatim: same statements, same order, no captured
/// state beyond `window`. Splitting main() is the peak-compile-RAM lever in
/// this crate — one 4,000-line function is one enormous MIR body.
fn wire_library_panel(window: &MainWindow) {
    // ── Scan schedule handlers ─────────────────────────────────────────────
    let w = window.as_weak();
    window.on_set_default_cadence(move |c| {
        let Some(w) = w.upgrade() else { return; };
        if let Some(cad) = cadence_from_str(&c) {
            let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
            s.libraries.default_cadence = cad;
            if let Err(e) = s.save() { tracing::warn!(error = %e, "save default cadence"); }
            w.set_default_cadence(c);
            tracing::info!(cadence = %cadence_str(cad), "default scan cadence set");
        }
    });
    let w = window.as_weak();
    window.on_set_cadence_override(move |id, c| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_scan_rows();
        for i in 0..model.row_count() {
            if let Some(mut row) = model.row_data(i) {
                if row.id == id { row.cadence_override = c.clone(); model.set_row_data(i, row); break; }
            }
        }
        tracing::info!(%id, cadence = %c, "per-library cadence override set");
    });

    // ── Library panel actions ──────────────────────────────────────────────
    let w = window.as_weak();
    window.on_lib_rescan_all(move || {
        let Some(w) = w.upgrade() else { return; };
        set_lib_busy(&w.as_weak(), "Rescanning all libraries…", -1.0);
        set_scan_silent(false); // surface the detailed per-section scan overlay
        for path in load_watched_folders() {
            if path.exists() { add_folder_path(&w, path); }
        }
        populate_folder_roots(&w);
        // The section scans run async; the scan overlay shows the live count,
        // so clear the maintenance bar once the kicks are dispatched.
        clear_lib_busy(&w.as_weak());
        tulipix_common::log_activity(
            "emerald",
            "Library rescan started",
            &format!("{} watched folders", load_watched_folders().len()),
        );
        tracing::info!("rescan-all requested");
    });
    let w = window.as_weak();
    window.on_lib_clear_thumb_cache(move || {
        let Some(w) = w.upgrade() else { return; };
        set_lib_busy(&w.as_weak(), "Clearing thumbnail cache…", -1.0);
        let weak = w.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            if let Some(dir) = tulipix_core::paths::thumbs_dir() {
                let _ = tokio::task::spawn_blocking(move || {
                    let _ = std::fs::remove_dir_all(&dir);
                    let _ = std::fs::create_dir_all(&dir);
                }).await;
                tracing::info!("thumbnail cache cleared");
            }
            clear_lib_busy(&weak);
        });
    });
    let w = window.as_weak();
    window.on_lib_rebuild_fts(move || {
        let Some(w) = w.upgrade() else { return; };
        set_lib_busy(&w.as_weak(), "Rebuilding search index…", 0.0);
        let weak = w.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            // Re-index the photo full-text-search table item-by-item so the
            // FTS5 index matches the current library + tags + people. Driven
            // through the maintenance progress bar.
            if let Ok(pool) = pool_for("photos").await {
                let ids: Vec<i64> = sqlx::query_scalar(
                    "SELECT id FROM items WHERE section = 'photos' AND missing_since IS NULL")
                    .fetch_all(&pool).await.unwrap_or_default();
                let total = ids.len().max(1);
                for (n, id) in ids.iter().enumerate() {
                    let _ = tulipix_photos::search::index_item(&pool, *id).await;
                    if n % 16 == 0 {
                        set_lib_busy(&weak, "Rebuilding search index…", (n as f32) / (total as f32));
                    }
                }
                tracing::info!(count = ids.len(), "FTS rebuild complete");
            }
            clear_lib_busy(&weak);
        });
    });
    // Reset App — wipe every section DB row, watched folders, folder tags, the
    // thumbnail cache, and all in-memory accumulators so the app starts fresh.
    // Destructive, behind an explicit danger button in Settings → Libraries.
    let w = window.as_weak();
    window.on_lib_reset_app(move || {
        let Some(w) = w.upgrade() else { return; };
        // Confirmation happens in-app (Settings → Libraries reset-confirm modal)
        // before this fires — native rfd dialogs don't render on some Linux WMs,
        // which is why the button previously appeared to do nothing.
        // Empty the in-memory accumulators so the grids blank out immediately.
        if let Ok(mut g) = photo_full().lock() { g.clear(); }
        if let Ok(mut g) = video_full().lock() { g.clear(); }
        if let Ok(mut g) = music_full().lock() { g.clear(); }
        if let Ok(mut g) = music_paths().lock() { g.clear(); }
        let weak = w.as_weak();
        set_lib_busy(&weak, "Resetting Tulipix — erasing all app data…", 0.08);
        tokio::runtime::Handle::current().spawn(async move {
            // Delete every app tree with a staged progress pill. Linux unlinks the
            // open sqlite files cleanly; the relaunched `--factory-reset` process
            // re-wipes anything a locked handle leaves behind (Windows-safe).
            let dirs = [
                ("index & databases", tulipix_core::paths::data_dir()),
                ("settings & folders", tulipix_core::paths::config_dir()),
                ("thumbnails & cache", tulipix_core::paths::cache_dir()),
            ];
            let n = dirs.len();
            for (i, (label, d)) in dirs.into_iter().enumerate() {
                let msg = format!("Nuking {label}…");
                let frac = 0.15 + 0.7 * (i as f32 / n as f32);
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_lib_busy_task(msg.into());
                    w.set_lib_busy_frac(frac);
                });
                if let Some(dir) = d {
                    let _ = std::fs::remove_dir_all(&dir);
                    let _ = std::fs::create_dir_all(&dir);
                }
                tokio::time::sleep(std::time::Duration::from_millis(350)).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_lib_busy_task("Restarting fresh…".into());
                w.set_lib_busy_frac(1.0);
            });
            tokio::time::sleep(std::time::Duration::from_millis(450)).await;
            tracing::info!("app data reset — relaunching fresh");
            // Relaunch the executable; the new process wipes again BEFORE opening
            // any DB (pre-DB), then finds no settings.json → shows onboarding.
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).arg("--factory-reset").spawn();
            }
            std::process::exit(0);
        });
    });
    let w = window.as_weak();
    window.on_lib_row_remove(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_library_rows();
        let removed = model.row_data(i as usize).map(|r| (r.path.to_string(), r.section.to_string()));
        let rows: Vec<LibraryRow> = (0..model.row_count())
            .filter(|&j| j != i as usize)
            .filter_map(|j| model.row_data(j))
            .collect();
        w.set_library_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
        // Drop from the watched-folder file if no other row references the path.
        if let Some((p, section)) = removed {
            forget_watched_folder(&w, &p);
            // Books register scan roots in their own DB too — unregister so
            // the next books sweep doesn't re-add the folder's books.
            if section == "books" {
                tokio::runtime::Handle::current().spawn(async move {
                    if let Ok(pool) = pool_for("books").await {
                        let _ = tulipix_books::scan::remove_folder(&pool, &p).await;
                    }
                });
            }
        }
        rebuild_scan_rows(&w);
        tracing::info!(index = i, "library row removed");
    });
    // Rescan a single watched location — re-walk just that row's folder.
    let w = window.as_weak();
    window.on_lib_row_rescan(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let Some(row) = w.get_library_rows().row_data(i as usize) else { return; };
        let path = PathBuf::from(row.path.to_string());
        if !path.exists() { return; }
        set_lib_busy(&w.as_weak(), &format!("Rescanning {}…", row.path), -1.0);
        set_scan_silent(false);
        add_folder_path(&w, path);
        populate_folder_roots(&w);
        clear_lib_busy(&w.as_weak());
        tracing::info!(index = i, "library row rescan");
    });
    // Re-thumbnail a single location — drop its cached thumbs, then rescan so
    // they regenerate.
    let w = window.as_weak();
    window.on_lib_row_rethumb(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let Some(row) = w.get_library_rows().row_data(i as usize) else { return; };
        let path = PathBuf::from(row.path.to_string());
        if !path.exists() { return; }
        set_lib_busy(&w.as_weak(), &format!("Re-thumbnailing {}…", row.path), -1.0);
        // Clearing the whole thumb cache is the safe, simple option: the rescan
        // below regenerates exactly the thumbs this folder needs.
        if let Some(dir) = tulipix_core::paths::thumbs_dir() {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::create_dir_all(&dir);
        }
        set_scan_silent(false);
        add_folder_path(&w, path);
        clear_lib_busy(&w.as_weak());
        tracing::info!(index = i, "library row rethumb");
    });
    window.on_lib_row_edit_exclude(move |i| tracing::info!(index = i, "library row edit-exclude requested"));
    // Music row: persist the chosen section (5-section dropdown + Save).
    let w = window.as_weak();
    window.on_lib_row_save_section(move |i, label| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_library_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        if row.section != "music" { return; }
        let key = music_section_key(&label);
        let folder = row.path.to_string();
        set_folder_section(&folder, key);
        row.music_section = music_section_label(key).into();
        model.set_row_data(i as usize, row);
        // Keep is_audiobook in sync with the folder's section assignment.
        let aud = key == "audiobooks";
        let folder2 = folder.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("music").await {
                let _ = tulipix_music::audiobooks::set_folder_flag(&pool, &folder2, aud).await;
            }
        });
        populate_music_views(w.as_weak());
        populate_folder_roots(&w);
        tracing::info!(index = i, section = key, "library music-row section saved");
    });

    // ── Command palette ────────────────────────────────────────────────────
    let w = window.as_weak();
    window.on_palette_query_changed(move |q| {
        let Some(w) = w.upgrade() else { return; };
        w.set_palette_rows(slint::ModelRc::new(slint::VecModel::from(build_palette_rows(&q))));
    });
    let w = window.as_weak();
    window.on_palette_activated(move |id| {
        let Some(w) = w.upgrade() else { return; };
        let id = id.to_string();
        match id.as_str() {
            "lock" => { tulipix_core::account::lock();
                let mut u = w.get_user(); u.mode = Mode::Locked; w.set_user(u);
                w.set_ambient_caption("Locked — click to resume".into());
                w.set_ambient_active(true); }
            "rescan" => { for path in load_watched_folders() { if path.exists() { add_folder_path(&w, path); } } }
            "shortcuts" => { w.set_shortcuts_open(true); }
            s if s.starts_with("go:") => {
                let sec = s.trim_start_matches("go:").to_string();
                if sec.starts_with("settings:") {
                    w.set_active_section("settings".into());
                    w.set_active_settings_tab(sec.trim_start_matches("settings:").into());
                } else {
                    w.set_active_section(sec.into());
                }
            }
            _ => {}
        }
        w.set_palette_query("".into());
    });

    // ── Voice search (np.voice) — mic button in every search bar ───────────
    // start(target): record ~5 s from the default mic with the bundled ffmpeg,
    // transcribe with the whisper model picked for the "voice" task, then
    // drop the text into `target`'s search box and fire its search callback.
    {
        let w = window.as_weak();
        window.global::<VoiceSearch>().on_start(move |target| {
            let Some(win) = w.upgrade() else { return; };
            if !tulipix_core::settings::Settings::load().map(|s| s.flag("ai.voice", true)).unwrap_or(true) {
                win.set_caps_nudge("Voice search is turned off — enable it in Settings → AI Features".into());
                return;
            }
            let g = win.global::<VoiceSearch>();
            g.set_target(target.clone());
            g.set_state("listening".into());
            let session = VOICE_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            let weak = w.clone();
            let target = target.to_string();
            tokio::runtime::Handle::current().spawn(async move {
                let listened = voice_record().await;
                if VOICE_GEN.load(std::sync::atomic::Ordering::SeqCst) != session { return; }
                let wk = weak.clone();
                let _ = wk.upgrade_in_event_loop(|w| {
                    w.global::<VoiceSearch>().set_state("busy".into());
                });
                let res = match listened {
                    Ok(wav) => voice_transcribe(&wav).await,
                    Err(e) => Err(e),
                };
                if VOICE_GEN.load(std::sync::atomic::Ordering::SeqCst) != session { return; }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    match res {
                        Ok(text) if !text.trim().is_empty() => voice_route(&w, &target, text.trim()),
                        Ok(_) => w.set_caps_nudge("Didn't catch that — try again closer to the microphone".into()),
                        Err(e) => w.set_caps_nudge(format!("Voice search failed: {e}").into()),
                    }
                    let g = w.global::<VoiceSearch>();
                    g.set_state("idle".into());
                    g.set_target("".into());
                });
            });
        });
        let w = window.as_weak();
        window.global::<VoiceSearch>().on_stop(move || {
            VOICE_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); // discard in-flight
            if let Some(w) = w.upgrade() {
                let g = w.global::<VoiceSearch>();
                g.set_state("idle".into());
                g.set_target("".into());
            }
        });
    }

    // ── Error boundary actions ─────────────────────────────────────────────
    window.on_error_open_logs(move || {
        if let Some(dir) = tulipix_core::paths::data_dir().map(|d| d.join("logs")) {
            let _ = tulipix_platform::fm::reveal_in_file_manager(&dir);
        }
    });
    let w = window.as_weak();
    window.on_error_reload(move || { if let Some(w) = w.upgrade() { w.set_error_open(false); } });
    window.on_error_quit(move || { let _ = slint::quit_event_loop(); });
    // Close button on the hover title bar in logo-fullscreen mode.
    window.on_app_quit(move || { let _ = slint::quit_event_loop(); });

    // ── Properties window — open containing folder ─────────────────────────
    let w = window.as_weak();
    window.on_props_open_folder(move || {
        let Some(w) = w.upgrade() else { return; };
        let p = PathBuf::from(w.get_props().path.to_string());
        let _ = tulipix_platform::fm::reveal_in_file_manager(&p);
    });
}

/// Register the bundled Sora variable font with the Slint runtime so the UI
/// always renders with the canonical brand typeface — no fallback to system
/// "Sans". The font covers weights 100..900; Slint picks the right axis.
fn register_bundled_fonts() {
    static SORA_VAR: &[u8] = include_bytes!("../../../resources/fonts/Sora[wght].ttf");
    // Slint 1.17 renamed the module with the fontique bump (0.8 → 0.10); the
    // API is unchanged. It is versioned in the path on purpose, so this moves
    // again on the next fontique major.
    let blob = slint::fontique_010::fontique::Blob::new(std::sync::Arc::new(SORA_VAR.to_vec()));
    let mut collection = slint::fontique_010::shared_collection();
    let registered = collection.register_fonts(blob, None);
    tracing::info!(count = registered.len(), "registered bundled fonts");
}

// Photos section (helpers, statics, editor, viewer) extracted to tulipix_sec_photos. See wire-up in main + `use tulipix_sec_photos::*`.

// Videos section library (statics, parse, TMDB scrape, show cards) extracted to tulipix_sec_videos. See `use tulipix_sec_videos::*`.

// fmt_duration / fmt_clock / fmt_date moved to tulipix_common.

// Source path picked for the profile cover/avatar cropper — stashed between
// the file-picker callback and crop-confirm (np.p1.profile.cover).
static CROP_SOURCE: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
fn crop_source() -> &'static std::sync::Mutex<Option<PathBuf>> {
    CROP_SOURCE.get_or_init(|| std::sync::Mutex::new(None))
}

// watched_folders_path / load_watched_folders moved to tulipix_common.

/// True inside the seasonal India-branding windows: Aug 1–31 and Jan 15–31,
/// every calendar year. Civil date from unix days (no chrono dependency).
fn festival_season() -> bool {
    let z = now_secs() / 86400 + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    matches!((m, d), (8, _) | (1, 15..=31))
}

/// Append `path` to the watched-folder list (deduped) and persist.
fn persist_watched_folder(path: &std::path::Path) {
    let Some(file) = watched_folders_path() else { return; };
    let mut list = load_watched_folders();
    if list.iter().any(|p| p == path) { return; }
    list.push(path.to_path_buf());
    let as_str: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
    if let Some(parent) = file.parent() { let _ = std::fs::create_dir_all(parent); }
    match serde_json::to_string_pretty(&as_str) {
        Ok(body) => { let _ = std::fs::write(&file, body); }
        Err(e) => tracing::warn!(error = %e, "serialize watched folders"),
    }
    // Both the in-app picker and the status page's Add Folder land here, so one
    // call covers both — the Activity Timeline sees the add either way.
    tulipix_common::log_activity(
        "emerald", "Folder added to library", &path.display().to_string());
}

// ── Music folder → section tag (np.p5.atmusic.folder-sections) ──────────────
// Each scanned music folder can be assigned to one of the 5 top music sections
// so the user controls where its tracks belong. Persisted next to the watched
// folders so the assignment survives restarts.
// MUSIC_SECTIONS + folder-section helpers moved to tulipix_common.

/// Open a library video in an external mpv window: resume from the stored
/// position (np.p3.watch-progress) and record the access for the Continue rail
/// (np.p3.last-accessed).
fn play_video_at(weak: slint::Weak<MainWindow>, idx: i32) {
    let Some(path) = video_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else {
        tracing::warn!(idx, "play-video: no path for index");
        return;
    };
    let item_id = video_ids().lock().ok().and_then(|g| g.get(idx as usize).copied());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Resume position + access touch (best-effort; never block playback).
        let resume = if let Some(id) = item_id {
            if let Ok(pool) = pool_for("videos").await {
                let _ = tulipix_videos::last_accessed::touch(&pool, id).await;
                let r = tulipix_videos::watch_progress::resume(&pool, id).await.ok().flatten();
                // First watch: seed 1s so the Home CONTINUE strip picks the video
                // up immediately (real position overwrites this on mpv exit).
                if r.is_none() {
                    let _ = tulipix_videos::watch_progress::update(&pool, id, 1.0, None).await;
                }
                r
            } else { None }
        } else { None };
        // Play in an external mpv window. There was an in-app player here once,
        // libmpv rendering into a Slint texture; it froze the whole UI on weak
        // iGPUs (Intel HD 5500 — the render happens on Slint's render thread and
        // blocks it), so every call site was routed here and the player was
        // deleted in v0.8.0. mpv's own window + OSC gives full controls,
        // including embedded audio and subtitle tracks, and never blocks Slint.
        // Resume + progress writeback flow via the IPC socket.
        spawn_mpv_windowed(path.clone(), resume, item_id);
        let _ = weak.upgrade_in_event_loop(move |w| {
            // A fresh access — refresh the Continue tab if it's showing, and the
            // Home CONTINUE strip so the video appears there instantly.
            let cat = w.get_video_category().to_string();
            if cat == "continue" || cat == "library" { kick_video_refresh(w.as_weak(), cat); }
            kick_home_continue(&w);
        });
    });
}

/// Anime4K-style GLSL shader chain (np.p3.player.upscale): all `.glsl` files in
/// `<config>/shaders`, sorted, joined for mpv's `--glsl-shaders` list option.
/// Returns `(dir, count)` for status display; the chain itself via `.0`.
// anime4k_shader_args moved to tulipix_common.

// spawn_mpv_windowed moved to tulipix_common (playback core).








/// After external FS deletes/renames are applied to the DB, prune the in-memory
/// library mirrors to files that still exist and rebuild every section's grid,
/// so a file removed in the OS disappears from the app live (np.p1.lib.watch).
/// The section DB queries already exclude `missing_since IS NOT NULL`; this
/// keeps the in-memory tile mirrors (music/photos/videos) in sync with that.
fn resync_after_fs_change(weak: slint::Weak<MainWindow>) {
    // Prune the mirrors (these are stat() calls — done off the UI thread).
    if let Ok(mut g) = music_full().lock() { g.retain(|(_, p, _)| p.exists()); }
    if let Ok(mut g) = photo_full().lock() { g.retain(|(_, p, _)| p.exists()); }
    if let Ok(mut g) = video_full().lock() { g.retain(|(_, p, _)| p.exists()); }
    // Books keep their own `missing` flag on the `books` table, which
    // `watcher::apply_event` does not touch — it only knows the shared `items`
    // shape. Reconcile here, and only redraw the grid when a row actually
    // changed, so an unrelated delete elsewhere in the library does not reset
    // the Books page under the user.
    {
        let weak = weak.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("books").await else { return };
            match tulipix_books::scan::reconcile_missing(&pool).await {
                Ok(0) => {}
                Ok(n) => {
                    tracing::info!(count = n, "books: files vanished, flagged missing");
                    tulipix_sec_books::books_refresh(weak, 0);
                }
                Err(e) => tracing::warn!(error = %e, "books: reconcile_missing"),
            }
        });
    }
    let _ = weak.upgrade_in_event_loop(|w| {
        // Music — tiles + dashboard/browse.
        rebuild_music_tiles(&w);
        populate_music_views(w.as_weak());
        populate_folder_roots(&w);
        // Photos — counts + active category grid.
        w.set_photos_total(photo_full().lock().map(|g| g.len() as i32).unwrap_or(0));
        w.set_photos_folder_count(photo_folder_count());
        let pcat = w.get_photos_category().to_string();
        let pq = w.get_photos_query().to_string();
        kick_category_refresh(w.as_weak(), pcat, pq);
        // Videos — active category grid.
        let vcat = w.get_video_category().to_string();
        kick_video_refresh(w.as_weak(), vcat);
    });
}




















// ── Cloud data layer (np.p4.cloud.remotes / .browse) ─────────────────────────
fn theme_choice_str(c: ThemeChoice) -> &'static str {
    match c {
        ThemeChoice::Light => "light",
        ThemeChoice::ExtraDark => "extra-dark",
        ThemeChoice::Oled => "oled",
        ThemeChoice::System => "system",
    }
}

fn cadence_str(c: ScanCadence) -> &'static str {
    match c {
        ScanCadence::Manual => "manual",
        ScanCadence::Hourly => "hourly",
        ScanCadence::Daily  => "daily",
        ScanCadence::Weekly => "weekly",
    }
}

fn cadence_from_str(s: &str) -> Option<ScanCadence> {
    match s {
        "manual" => Some(ScanCadence::Manual),
        "hourly" => Some(ScanCadence::Hourly),
        "daily"  => Some(ScanCadence::Daily),
        "weekly" => Some(ScanCadence::Weekly),
        _ => None,
    }
}

/// Seed the API-Keys panel: one row per known service, with the live
/// app-default quota and whether a custom key is stored in the keychain.
/// Services where a personal API key actually exists / helps. The rest of
/// api_keys::SERVICES are keyless public APIs (MusicBrainz, Cover Art,
/// LRCLIB) or not keys at all (OAuth flows, cookies) — hidden from the panel.
const KEYED_SERVICES: &[&str] = &["tmdb", "tvdb", "opensubtitles", "lastfm", "libretranslate"];

fn seed_api_rows(w: &MainWindow) {
    use tulipix_core::api_keys::{self, KeySource};
    let rows: Vec<ApiKeyRow> = api_keys::SERVICES.iter()
        .filter(|svc| KEYED_SERVICES.contains(*svc))
        .map(|svc| {
        let user_set = api_keys::fetch(svc).ok().flatten().is_some();
        let source = if user_set { KeySource::UserKey } else { KeySource::AppDefault };
        let q = api_keys::quota_state(svc, source);
        ApiKeyRow {
            service: (*svc).into(),
            label: pretty_service(svc).into(),
            use_app_default: !user_set,
            user_key_set: user_set,
            quota_used: q.used as i32,
            quota_limit: q.limit as i32,
            status: "unknown".into(),
            editing: false,
        }
    }).collect();
    w.set_api_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

fn pretty_service(svc: &str) -> String {
    match svc {
        "tmdb" => "The Movie DB",
        "tvdb" => "TheTVDB",
        "opensubtitles" => "OpenSubtitles",
        "lrclib" => "LRCLIB (lyrics)",
        "musicbrainz" => "MusicBrainz",
        "coverart" => "Cover Art Archive",
        "libretranslate" => "LibreTranslate",
        "lastfm" => "Last.fm",
        "google_oauth" => "Google OAuth",
        "github_oauth" => "GitHub OAuth",
        "ytdlp_cookies" => "yt-dlp cookies",
        other => other,
    }.to_string()
}

/// Rebuild the Scan-Schedule per-library rows from the live library model so
/// the Schedule tab lists exactly what the Libraries tab shows.
fn rebuild_scan_rows(w: &MainWindow) {
    let lib = w.get_library_rows();
    let rows: Vec<ScanLibraryRow> = (0..lib.row_count()).filter_map(|i| {
        lib.row_data(i).map(|r| ScanLibraryRow {
            id: r.id.clone(),
            path: r.path.clone(),
            section: r.section.clone(),
            cadence_override: slint::SharedString::new(),
        })
    }).collect();
    w.set_scan_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Drop a path from the watched-folder file unless another library row still
/// references it.
fn forget_watched_folder(w: &MainWindow, path: &str) {
    let lib = w.get_library_rows();
    let still_used = (0..lib.row_count()).any(|i| lib.row_data(i).is_some_and(|r| r.path == path));
    if still_used { return; }
    let Some(file) = watched_folders_path() else { return; };
    let kept: Vec<String> = load_watched_folders().into_iter()
        .map(|p| p.display().to_string())
        .filter(|p| p != path)
        .collect();
    if let Ok(body) = serde_json::to_string_pretty(&kept) { let _ = std::fs::write(&file, body); }
    tulipix_common::log_activity("orange", "Folder removed from library", path);
}

/// Static keyboard-shortcut groups for the help overlay (? / F1).
fn seed_shortcut_groups(w: &MainWindow) {
    let mk = |keys: &str, label: &str| ShortcutItem { keys: keys.into(), label: label.into() };
    let group = |title: &str, items: Vec<ShortcutItem>| ShortcutGroup {
        title: title.into(),
        items: slint::ModelRc::new(slint::VecModel::from(items)),
    };
    let groups = vec![
        group("Global", vec![
            mk("Ctrl / ⌘ + K", "Open command palette"),
            mk("? / F1", "Show keyboard shortcuts"),
            mk("Ctrl / ⌘ + L", "Lock the app"),
        ]),
        group("Photos", vec![
            mk("← / →", "Previous / next photo"),
            mk("Space", "Toggle slideshow"),
            mk("Esc", "Close viewer"),
        ]),
        group("Music", vec![
            mk("Click tile", "Play track"),
            mk("Next / Prev", "Change track in now-playing bar"),
        ]),
    ];
    w.set_shortcut_groups(slint::ModelRc::new(slint::VecModel::from(groups)));
}

/// Build (and rank) command-palette rows for `query`. Commands are static
/// actions + section/settings deep-links; ranking is a simple subsequence
/// fuzzy score so the best match floats to the top.
fn build_palette_rows(query: &str) -> Vec<PaletteRow> {
    let catalog: &[(&str, &str, &str, &str)] = &[
        ("go:photos",   "Photos",   "Jump to section",  "Section"),
        ("go:videos",   "Videos",   "Jump to section",  "Section"),
        ("go:music",    "Music",    "Jump to section",  "Section"),
        ("go:books",    "Books",    "Jump to section",  "Section"),
        ("go:cloud",    "Cloud",    "Jump to section",  "Section"),
        ("go:tools",    "Tools",    "Jump to section",  "Section"),
        ("go:settings", "Settings", "Open settings",    "Section"),
        ("go:settings:profile",    "Profile",    "Settings → Profile",    "Settings"),
        ("go:settings:appearance", "Appearance", "Settings → Appearance", "Settings"),
        ("go:settings:libraries",  "Libraries",  "Settings → Libraries",  "Settings"),
        ("go:settings:playback",   "Playback",   "Settings → Playback",   "Settings"),
        ("go:settings:services",   "Services & Keys", "Settings → Services & Keys", "Settings"),
        ("go:settings:ai",         "AI Features", "Settings → AI Features", "Settings"),
        ("go:settings:security",   "Security",   "Settings → Security",   "Settings"),
        ("go:settings:data",       "Backup & Data", "Settings → Backup & Data", "Settings"),
        ("go:settings:advanced",   "Advanced",   "Settings → Advanced",   "Settings"),
        ("rescan",    "Rescan libraries", "Re-scan every watched folder", "Command"),
        ("lock",      "Lock now",         "Lock the app + show screensaver", "Command"),
        ("shortcuts", "Keyboard shortcuts", "Show the shortcut help", "Command"),
    ];
    let q = query.trim().to_lowercase();
    let mut scored: Vec<(f32, PaletteRow)> = catalog.iter().filter_map(|(id, title, sub, kind)| {
        let score = if q.is_empty() { 1.0 } else { fuzzy_score(&q, &title.to_lowercase())? };
        Some((score, PaletteRow {
            id: (*id).into(), title: (*title).into(),
            subtitle: (*sub).into(), kind: (*kind).into(), score,
        }))
    }).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    scored.into_iter().map(|(_, r)| r).collect()
}

/// Subsequence fuzzy match: every char of `q` must appear in order in `hay`.
/// Score rewards contiguous runs + early matches. `None` = no match.
// fuzzy_score moved to tulipix_common.

/// Gather filesystem + content properties for the Properties window.
fn build_props(path: &std::path::Path) -> AssetProperties {
    use slint::{ModelRc, VecModel};
    let meta = std::fs::metadata(path).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = fmt_time_gmt(meta.as_ref().and_then(|m| m.modified().ok()));
    let ctime = fmt_time_gmt(meta.as_ref().and_then(|m| m.created().ok()));
    let mime = tulipix_platform::fm::guess_mime(path);
    // Hash only modest files so opening Properties never stalls the UI.
    let sha = if size > 0 && size <= 64 * 1024 * 1024 {
        std::fs::read(path).ok().map(|bytes| {
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(&bytes);
            format!("{:x}", h.finalize())
        }).unwrap_or_else(|| "—".into())
    } else {
        "(skipped — file > 64 MB)".into()
    };
    let is_image = mime.starts_with("image/");
    // EXIF readout for images (same 24 attrs as the viewer); plain facts else.
    let rows: Vec<MetaRow> = if is_image {
        exif_rows(path).into_iter()
            .map(|(k, v)| MetaRow { k: k.into(), v: v.into() })
            .collect()
    } else {
        vec![
            MetaRow { k: "File".into(), v: path.file_name().and_then(|s| s.to_str()).unwrap_or("—").into() },
            MetaRow { k: "Size".into(), v: human_size(size).into() },
            MetaRow { k: "MIME".into(), v: mime.clone().into() },
        ]
    };
    let histogram = if is_image { histogram_image(path) } else { slint::Image::default() };
    AssetProperties {
        path: path.display().to_string().into(),
        size_bytes: size as i32,
        size_human: human_size(size).into(),
        mtime: mtime.into(),
        ctime: ctime.into(),
        mime: mime.into(),
        sha256: sha.into(),
        library_state: "indexed".into(),
        edits_applied: 0,
        metadata: ModelRc::new(VecModel::from(rows)),
        histogram,
    }
}

/// Format a filesystem timestamp as a GMT/UTC datetime string.
fn fmt_time_gmt(t: Option<std::time::SystemTime>) -> String {
    use chrono::{DateTime, Utc};
    t.map(|t| DateTime::<Utc>::from(t).format("%Y-%m-%d %H:%M:%S GMT").to_string())
        .unwrap_or_else(|| "—".into())
}

// human_size moved to tulipix_common (shared with Cloud).

/// Minimal URL query-component percent-encoder (spaces → +).
fn urlencoding(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "+".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

// MEDIA_CONTROLS moved to tulipix_common.

// media_set_playing moved to tulipix_common.

/// Register an OS media-control surface (MPRIS / SMTC / MediaPlayer) so the
/// hardware/keyboard media keys drive playback (np.p4.music.player-keys).
fn setup_media_controls(window: &MainWindow) {
    use souvlaki::{MediaControlEvent, MediaControls, MediaPlayback, PlatformConfig, SeekDirection};

    // Windows SMTC requires the native window handle; souvlaki panics on
    // `hwnd: None`. The HWND only exists once the window is realized, so this is
    // called after show (see the single-shot timer at startup). Other platforms
    // (MPRIS/MediaPlayer) take `None`.
    #[cfg(target_os = "windows")]
    let hwnd = {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match window.window().window_handle().window_handle().map(|h| h.as_raw()) {
            Ok(RawWindowHandle::Win32(h)) => Some(h.hwnd.get() as *mut std::ffi::c_void),
            other => {
                tracing::warn!(?other, "media controls: no Win32 HWND yet, skipping");
                return;
            }
        }
    };
    #[cfg(not(target_os = "windows"))]
    let hwnd = None;

    let config = PlatformConfig { dbus_name: "tulipix", display_name: "Tulipix", hwnd };
    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => { tracing::warn!(error = ?e, "media controls unavailable"); return; }
    };
    let weak = window.as_weak();
    let attached = controls.attach(move |event: MediaControlEvent| {
        let weak = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = weak.upgrade() else { return; };
            match event {
                MediaControlEvent::Toggle => w.invoke_music_toggle_pause(),
                MediaControlEvent::Play  => if !w.get_music_playing() { w.invoke_music_toggle_pause(); },
                MediaControlEvent::Pause => if  w.get_music_playing() { w.invoke_music_toggle_pause(); },
                MediaControlEvent::Stop  => if  w.get_music_playing() { w.invoke_music_toggle_pause(); },
                MediaControlEvent::Next     => w.invoke_music_next(),
                MediaControlEvent::Previous => w.invoke_music_prev(),
                // Scrubbing from the applet. `SetPosition` is absolute;
                // `SeekBy` is a relative jump; a bare `Seek` carries no amount,
                // so it gets the same 10 s the app's own arrow keys use.
                MediaControlEvent::SetPosition(p) => w.invoke_music_seek(p.0.as_secs_f32()),
                MediaControlEvent::SeekBy(dir, d) => {
                    let step = d.as_secs_f32()
                        * if dir == SeekDirection::Backward { -1.0 } else { 1.0 };
                    w.invoke_music_seek((w.get_music_pos() + step).max(0.0));
                }
                MediaControlEvent::Seek(dir) => {
                    let step = if dir == SeekDirection::Backward { -10.0 } else { 10.0 };
                    w.invoke_music_seek((w.get_music_pos() + step).max(0.0));
                }
                // MPRIS requires the value to be echoed back, or the remote's
                // slider springs to where it thinks we still are. The app's
                // scale is 0..130 (mpv allows the boost), the wire's is 0..1.
                MediaControlEvent::SetVolume(v) => {
                    w.invoke_music_set_volume((v as f32 * 100.0).clamp(0.0, 130.0));
                    tulipix_common::media_set_volume(v);
                }
                MediaControlEvent::Raise => crate::miniwin::restore(&w),
                MediaControlEvent::Quit  => w.invoke_app_quit(),
                _ => {}
            }
        });
    });
    if let Err(e) = attached { tracing::warn!(error = ?e, "media controls attach failed"); return; }
    // Advertise as Playing so desktop environments route the media keys to us.
    let _ = controls.set_playback(MediaPlayback::Playing { progress: None });
    MEDIA_CONTROLS.with(|c| *c.borrow_mut() = Some(controls));
}

/// Apply Picasa `star=yes` marks to the photo library (np.p1.migration):
/// each starred section header is a bare file name; match photos by file name
/// (same dir as the .ini first, then any path suffix) and set `starred`.
/// Returns how many stars landed.
fn apply_picasa_stars(ini: &std::path::Path, rt: &tokio::runtime::Handle) -> u32 {
    let Ok(text) = std::fs::read_to_string(ini) else { return 0; };
    let dir = ini.parent().map(|d| d.to_path_buf()).unwrap_or_default();
    // Collect file names whose section contains star=yes.
    let mut starred: Vec<String> = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = Some(inner.to_string());
            continue;
        }
        if t.starts_with("star=yes") {
            if let Some(name) = &current { starred.push(name.clone()); }
        }
    }
    if starred.is_empty() { return 0; }
    rt.block_on(async move {
        let Ok(pool) = pool_for("photos").await else { return 0u32; };
        let mut n = 0u32;
        for name in starred {
            let exact = dir.join(&name).to_string_lossy().into_owned();
            let res = sqlx::query(
                "UPDATE photo_meta SET starred = 1 WHERE item_id IN
                   (SELECT id FROM items WHERE path = ?1 OR path LIKE '%/' || ?2)")
                .bind(&exact).bind(&name).execute(&pool).await;
            if let Ok(r) = res { if r.rows_affected() > 0 { n += 1; } }
        }
        n
    })
}

/// Open a file in the OS default application (Play in default player).
fn open_in_default_app(path: &std::path::Path) {
    #[cfg(target_os = "linux")]
    let prog = "xdg-open";
    #[cfg(target_os = "macos")]
    let prog = "open";
    #[cfg(target_os = "windows")]
    let prog = "explorer";
    match std::process::Command::new(prog).arg(path).spawn() {
        Ok(_) => {}
        Err(e) => tracing::error!(error = %e, ?path, "open in default app failed"),
    }
}

// ── Data-driven Settings panels (AI · Endpoints · Security · Data · System) ──

/// Update summary for the AI models (np.p1.ai.update-prompt): per manifest
/// entry — up-to-date, an older install awaiting update, or not installed.
fn ai_update_summary() -> String {
    let mut current = 0; let mut missing = 0; let mut stale: Vec<String> = Vec::new();
    let root = tulipix_photos::ai::models::models_root();
    for m in &ai_manifest().models {
        if tulipix_photos::ai::models::is_installed(m) { current += 1; continue; }
        // Any older "<name>-<version>" install dir → counts as update pending.
        let older = root.as_deref().and_then(|r| std::fs::read_dir(r).ok()).into_iter().flatten().flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&format!("{}-", m.name)));
        if older { stale.push(format!("{} → v{}", m.name, m.version)); } else { missing += 1; }
    }
    if stale.is_empty() {
        format!("Models: {current} up-to-date · {missing} not installed — nothing to update.")
    } else {
        format!("Updates available: {} · {current} current · {missing} not installed.", stale.join(", "))
    }
}

/// Compiled-in AI model manifest (resources/ai-models.toml) — parsed once.
/// Source of truth for the Settings → AI Models rows and the update checker.
fn ai_manifest() -> &'static tulipix_core::ai_models::Manifest {
    static M: std::sync::OnceLock<tulipix_core::ai_models::Manifest> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        tulipix_core::ai_models::Manifest::from_toml(include_str!("../../../resources/ai-models.toml"))
            .unwrap_or_else(|e| { tracing::error!(error = %e, "ai-models.toml parse"); Default::default() })
    })
}

fn si(key: &str, kind: &str, label: &str, desc: &str, value: &str, on: bool, state: &str) -> SettingItem {
    SettingItem {
        key: key.into(), kind: kind.into(), label: label.into(),
        desc: desc.into(), value: value.into(), on, state: state.into(),
        btn: "".into(), frac: 0.0,
    }
}
fn hdr(label: &str) -> SettingItem { si("", "header", label, "", "", false, "") }
/// Status + trailing action button in a single row.
fn statact(key: &str, label: &str, desc: &str, value: &str, state: &str, btn: &str) -> SettingItem {
    let mut r = si(key, "status-action", label, desc, value, false, state);
    r.btn = btn.into();
    r
}
/// Whisper-model segmented picker row (tiny · base · small · turbo).
fn model_choice(s: &tulipix_core::settings::Settings, key: &str, label: &str, desc: &str) -> SettingItem {
    si(key, "model-choice", label, desc, &s.text(key), false, "")
}
fn tog(s: &tulipix_core::settings::Settings, key: &str, def: bool, label: &str, desc: &str) -> SettingItem {
    si(key, "toggle", label, desc, "", s.flag(key, def), "")
}
fn txt(s: &tulipix_core::settings::Settings, key: &str, label: &str, desc: &str) -> SettingItem {
    si(key, "text", label, desc, &s.text(key), false, "")
}
fn stat(label: &str, value: &str, state: &str) -> SettingItem { si("", "status", label, "", value, false, state) }
fn act(key: &str, label: &str, desc: &str, btn: &str) -> SettingItem { si(key, "action", label, desc, btn, false, "") }

/// Profile display name, trimmed and capped at 12 **characters** (not bytes —
/// an emoji name must not be cut mid-codepoint). The cap exists for the Welcome
/// home layout: its greeting is sized off a width factor, and past 12 the
/// headline wraps to a second line. Applied on save and again on load, so a
/// name persisted before the cap existed is clamped too.
fn clamp_profile_name(name: &str) -> String {
    name.trim().chars().take(12).collect()
}

/// Time-of-day greeting for the Home header; recomputed on each Home landing
/// so a long-running app stays fresh across day boundaries.
fn set_home_greeting_now(w: &MainWindow) {
    use chrono::Timelike;
    let name = w.get_user().display_name.to_string();
    let who = if name.trim().is_empty() { "there".to_string() } else { name };
    let g = match chrono::Local::now().hour() {
        5..=11  => format!("Good Morning, {who}"),
        12..=16 => format!("Good Afternoon, {who}"),
        _       => format!("Good Evening, {who}"),
    };
    w.set_home_greeting(g.into());
    w.set_home_date_line(chrono::Local::now().format("%A, %B %-d").to_string().into());
    // The quote shelf: shuffled ONCE per run, not per landing, so walking back
    // to Home does not restart the rotation. The Tools tile's count comes off
    // the same catalog the Tools section builds its ops list from.
    if w.get_home_quotes().row_count() == 0 {
        let rows: Vec<HomeQuote> = tulipix_common::quotes::shuffled().into_iter()
            .map(|(text, author)| HomeQuote { text: text.into(), author: author.into() })
            .collect();
        w.set_home_quotes(slint::ModelRc::new(slint::VecModel::from(rows)));
        w.set_tools_op_total(tulipix_sec_tools::op_total() as i32);
    }
}

/// Seed the Home command-center stats (np.p6.home). Cheap COUNTs per section
/// DB; called at startup and on every landing on the Home section. Every
/// count is best-effort — a failed query leaves 0/"", never errors.
// ── Status dashboard ────────────────────────────────────────────────────────
// The sidebar lamp, and two renderings of the same snapshot: the loopback page
// the Status button opens in the browser (`tulipix-status`, which also carries
// the loopback + token reasoning), and the native Status section
// (`tulipix-sec-status` + `page_status.slint`). One `collect()` feeds both —
// they are kept side by side deliberately, so the two approaches can be
// compared on the same data rather than argued about.

/// The one status server for this process. Bound on the first Status click and
/// never rebound: a dashboard nobody has opened has no business holding a port.
static STATUS_SRV: tokio::sync::OnceCell<tulipix_status::Running> =
    tokio::sync::OnceCell::const_new();

/// Whether the native Status section is the one on screen. Written by the
/// refresh tick, which is the only place that can read the active section, and
/// read by that same tick to decide how often to run — the loop cannot ask the
/// UI thread a question and use the answer in the same pass, so it uses the
/// previous one. Being one tick late to speed up costs nothing.
static STATUS_PAGE_OPEN: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Count and measure the installed AI model files. Walks the models directory
/// rather than trusting the manifest: what matters on a status page is what is
/// actually on disk, which is exactly where the manifest can be wrong.
fn status_models() -> (i64, i64) {
    let Some(root) = tulipix_photos::ai::models::models_root() else { return (0, 0) };
    let (mut count, mut total) = (0i64, 0i64);
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
                continue;
            }
            let name = e.file_name().to_string_lossy().to_lowercase();
            if name.ends_with(".onnx") || name.ends_with(".bin") || name.ends_with(".gguf") {
                count += 1;
                total += e.metadata().map(|m| m.len() as i64).unwrap_or(0);
            }
        }
    }
    (count, total)
}

/// Everything the status pass cannot read from a database: in-process services
/// and build facts. Rebuilt each tick — all of it is cheap, and a cached copy
/// would be the one thing on the page that is stale.
fn status_live() -> tulipix_status::Live {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let x = tulipix_sec_transfer::status_snapshot(now);
    let (models, model_bytes) = status_models();
    tulipix_status::Live {
        transfer_running: x.running,
        transfer_port: x.port,
        transfer_ip: x.address,
        transfer_devices: x.devices,
        transfer_inflight: x.inflight,
        transfer_sent: x.sent_bytes,
        transfer_recv: x.recv_bytes,
        ai_models: models,
        ai_models_bytes: model_bytes,
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        renderer: if cfg!(feature = "renderer-skia") { "skia".into() } else { "femtovg".into() },
        uptime_s: tulipix_status::uptime_secs(),
        scan: scan_rows_for_status(),
    }
}

/// The live scan counters, in the shape the status page draws a progress bar
/// from. Read straight off the same `SCAN_STATE` the in-app scan overlay reads,
/// so the browser and the window can never show different progress for the same
/// rescan.
fn scan_rows_for_status() -> Vec<tulipix_status::collect::ScanRow> {
    use std::sync::atomic::Ordering::Relaxed;
    let Ok(g) = scan_state().lock() else { return Vec::new() };
    g.iter()
        .map(|(name, c)| tulipix_status::collect::ScanRow {
            section: (*name).to_string(),
            total: c.total.load(Relaxed) as i64,
            added: c.added.load(Relaxed) as i64,
            failed: c.failed.load(Relaxed) as i64,
            active: c.active.load(Relaxed),
        })
        .collect()
}

/// One immediate snapshot into the native Status page, for the moment someone
/// lands on it. The tick owns the steady state; this owns the first frame.
fn kick_status_snapshot(window: &MainWindow) {
    let weak = window.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let snap = tulipix_status::collect::collect(&status_live()).await;
        STATUS_PAGE_OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_status_level(snap.level.as_str().into());
            w.set_status_note(snap.note.as_str().into());
            tulipix_sec_status::apply(&w, &snap.json);
        });
    });
}

fn wire_status(window: &MainWindow) {
    // Start the session clock now rather than on first read, so "Session" means
    // how long the app has been up and not how long the page has been open.
    tulipix_status::uptime_secs();

    // Opening Settings › System › Status. The tick below only fills the page
    // while it is on screen, so without this the tab would sit empty for up to
    // the slow period after being opened.
    let weak = window.as_weak();
    window.on_status_refresh(move || {
        if let Some(w) = weak.upgrade() {
            kick_status_snapshot(&w);
        }
    });

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<tulipix_status::Action>();

    // Actions arriving from the page. Every one of them ends in a Slint call,
    // so each hops to the UI thread instead of running on the tokio worker that
    // received the request.
    let weak = window.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        while let Some(action) = rx.recv().await {
            let posted = weak.upgrade_in_event_loop(move |w| match action {
                tulipix_status::Action::OpenSection(s) => {
                    w.set_active_section(s.clone().into());
                    w.invoke_section_changed(s.into());
                }
                tulipix_status::Action::Rescan => w.invoke_lib_rescan_all(),
                // Both of these run the app's own handlers rather than a
                // parallel implementation: the picker has to persist and scan
                // through one path, and the reset has to be the same wipe the
                // Settings button performs — a second one would drift.
                tulipix_status::Action::AddFolder => {
                    // The picker is modal and belongs to the window, so bring
                    // the window up first; a dialog behind the browser is a
                    // click that appeared to do nothing.
                    let _ = w.window().show();
                    w.invoke_lib_add();
                }
                tulipix_status::Action::ResetApp => w.invoke_lib_reset_app(),
            });
            // The window is gone: the app is closing and so is this loop.
            if posted.is_err() {
                break;
            }
        }
    });

    // The refresh tick. It runs whether or not the page is open, because the
    // sidebar lamp needs it either way — but slowly when nobody is watching.
    // Re-querying ten databases every two seconds for a page that is not on
    // screen is the kind of thing that shows up in someone's battery graph.
    let weak = window.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        loop {
            let snap = tulipix_status::collect::collect(&status_live()).await;
            if let Some(srv) = STATUS_SRV.get() {
                srv.publish(&snap);
            }
            let (level, note) = (snap.level.clone(), snap.note.clone());
            let json = snap.json.clone();
            if weak
                .upgrade_in_event_loop(move |w| {
                    w.set_status_level(level.into());
                    w.set_status_note(note.into());
                    // The native Status page draws the same snapshot. Filled
                    // only while its Settings tab is on screen: building ten
                    // nested models for a page nobody is looking at is the work
                    // the slow tick below exists to avoid, and this is the one
                    // place that can tell whether anyone is looking.
                    let on = w.get_active_section().as_str() == "settings"
                        && w.get_active_settings_tab().as_str() == "status";
                    if on {
                        tulipix_sec_status::apply(&w, &json);
                    }
                    STATUS_PAGE_OPEN.store(on, std::sync::atomic::Ordering::Relaxed);
                })
                .is_err()
            {
                break;
            }
            // Either dashboard being open earns the fast tick: they are two
            // renderings of one snapshot, so whichever is in front decides.
            let watched = STATUS_SRV.get().is_some_and(|s| s.watched())
                || STATUS_PAGE_OPEN.load(std::sync::atomic::Ordering::Relaxed);
            let period = if watched { 2 } else { 30 };
            tokio::time::sleep(std::time::Duration::from_secs(period)).await;
        }
    });

    window.on_status_clicked(move || {
        let tx = tx.clone();
        tokio::runtime::Handle::current().spawn(async move {
            match STATUS_SRV.get_or_try_init(|| tulipix_status::start(tx)).await {
                // A page that is still polling is a page that is still open.
                // Handing the URL to the browser again would stack up identical
                // tabs; asking the open one to come forward does not.
                Ok(srv) if srv.watched() => {
                    tracing::info!("status: dashboard already open — asking it to come forward");
                    srv.request_focus();
                }
                Ok(srv) => tulipix_status::open_in_browser(&srv.url),
                Err(e) => tracing::warn!(error = %e, "status: could not start the server"),
            }
        });
    });
}

fn kick_home_stats(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let mut s = HomeStats::default();
        let count = |pool: sqlx::SqlitePool, q: &'static str| async move {
            sqlx::query_scalar::<_, i64>(q).fetch_one(&pool).await.unwrap_or(0) as i32
        };
        // Running library totals for the header line ("N items · X B"): every
        // section's `items` table carries a `size` column.
        let sum_items = |pool: sqlx::SqlitePool| async move {
            sqlx::query_as::<_, (i64, i64)>(
                "SELECT COUNT(*), COALESCE(SUM(size), 0) FROM items")
                .fetch_one(&pool).await.unwrap_or((0, 0))
        };
        // Every section lives in its own SQLite file, so the per-section
        // blocks are independent — join! runs them concurrently and the
        // header stats land in ~max(section) latency instead of the sum.
        let photos_f = async {
            let Ok(pool) = pool_for("photos").await else { return (0, 0, (0i64, 0i64)); };
            (count(pool.clone(), "SELECT COUNT(*) FROM items").await,
             count(pool.clone(), "SELECT COUNT(*) FROM albums").await,
             sum_items(pool).await)
        };
        let videos_f = async {
            let Ok(pool) = pool_for("videos").await else { return (0, 0, (0i64, 0i64)); };
            (count(pool.clone(), "SELECT COUNT(*) FROM items").await,
             count(pool.clone(), "SELECT COUNT(*) FROM shows").await,
             sum_items(pool).await)
        };
        let music_f = async {
            let Ok(pool) = pool_for("music").await else { return (0, 0, (0i64, 0i64), None); };
            let nb = sum_items(pool.clone()).await;
            let songs = count(pool.clone(),
                "SELECT COUNT(*) FROM track_meta WHERE is_audiobook = 0").await;
            let audiobooks = count(pool.clone(),
                "SELECT COUNT(DISTINCT folder) FROM track_meta WHERE is_audiobook = 1").await;
            // Continue card: newest in-progress audiobook (folder = the book).
            let cont = sqlx::query_as::<_, (String, f64)>(
                "SELECT tm.folder, ap.position_s FROM audiobook_progress ap \
                 JOIN track_meta tm ON tm.item_id = ap.item_id \
                 WHERE ap.finished = 0 AND ap.position_s > 0 AND tm.folder IS NOT NULL \
                 ORDER BY ap.updated DESC LIMIT 1")
                .fetch_optional(&pool).await.ok().flatten()
                .map(|(folder, pos)| {
                    let book = std::path::Path::new(&folder).file_name()
                        .map(|f| f.to_string_lossy().into_owned()).unwrap_or(folder.clone());
                    (format!("🎧 {book}"),
                     format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60))
                });
            (songs, audiobooks, nb, cont)
        };
        let podcasts_f = async {
            let Ok(pool) = pool_for("podcasts").await else { return (0, None); };
            let n = count(pool.clone(), "SELECT COUNT(*) FROM podcasts").await;
            // Continue card: newest partially-played episode.
            let cont = sqlx::query_as::<_, (String, f64, Option<f64>)>(
                "SELECT COALESCE(title, ''), position_s, duration_s FROM podcast_episodes \
                 WHERE position_s > 0 AND played = 0 \
                 ORDER BY COALESCE(downloaded_at, published) DESC LIMIT 1")
                .fetch_optional(&pool).await.ok().flatten()
                .filter(|(title, _, _)| !title.is_empty())
                .map(|(title, pos, dur)| {
                    (format!("🎙 {title}"),
                     match dur {
                         Some(d) if d > 0.0 =>
                             format!("{}:{:02} · {}%", (pos as i64) / 60, (pos as i64) % 60,
                                     ((pos / d) * 100.0).round() as i64),
                         _ => format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60),
                     })
                });
            (n, cont)
        };
        let radio_f = async {
            let Ok(pool) = pool_for("radio").await else { return 0; };
            count(pool, "SELECT COUNT(*) FROM radio_stations").await
        };
        let cloud_f = async {
            let Ok(pool) = pool_for("cloud").await else { return 0; };
            count(pool, "SELECT COUNT(*) FROM remotes").await
        };
        let tools_f = async {
            let Ok(pool) = pool_for("tools").await else { return (0, 0); };
            (count(pool.clone(), "SELECT COUNT(*) FROM jobs WHERE state = 'running'").await,
             count(pool, "SELECT COUNT(*) FROM jobs WHERE state = 'queued'").await)
        };
        // Books — total library count + how many are currently in progress
        // (a reading_progress row with real forward progress, book still here).
        let books_f = async {
            let Ok(pool) = pool_for("books").await else { return (0, 0); };
            (count(pool.clone(), "SELECT COUNT(*) FROM books WHERE missing = 0").await,
             count(pool,
                "SELECT COUNT(*) FROM progress p JOIN books b ON b.id = p.book_id \
                 WHERE b.finished = 0 AND b.missing = 0 \
                       AND (p.page > 0 OR p.char_offset > 0 OR p.percent > 0)").await)
        };
        let (photos, videos, music, podcasts, radio, cloud, tools, books) =
            tokio::join!(photos_f, videos_f, music_f, podcasts_f, radio_f, cloud_f, tools_f, books_f);
        let mut total_items: i64 = 0;
        let mut total_bytes: i64 = 0;
        let mut add = |(n, b): (i64, i64)| { total_items += n; total_bytes += b; };
        s.photos = photos.0; s.photos_albums = photos.1; add(photos.2);
        s.videos = videos.0; s.videos_shows = videos.1; add(videos.2);
        s.songs = music.0; s.audiobooks = music.1; add(music.2);
        if let Some((c, sub)) = music.3 { s.continue3 = c.into(); s.continue3_sub = sub.into(); }
        s.podcasts = podcasts.0;
        if let Some((c, sub)) = podcasts.1 { s.continue2 = c.into(); s.continue2_sub = sub.into(); }
        s.radio = radio;
        s.books = books.0;
        s.books_reading = books.1;
        s.cloud_remotes = cloud;
        s.tools_jobs = tools.0;
        if tools.1 > 0 { s.tools_note = format!("{} queued", tools.1).into(); }
        // Header totals line — "12,304 items · 41 GB" (thousands-separated).
        let items_str = {
            let d = total_items.to_string();
            let bytes = d.as_bytes();
            let mut out = String::new();
            for (i, c) in bytes.iter().enumerate() {
                if i > 0 && (bytes.len() - i) % 3 == 0 { out.push(','); }
                out.push(*c as char);
            }
            out
        };
        let secondary = format!("{items_str} items · {}", human_size(total_bytes as u64));
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_home_stats(s);
            let mut u = w.get_user();
            u.secondary = secondary.into();
            w.set_user(u);
        });
    });
}

// Recent photo/video absolute paths mirroring the Home coverflow order, so a
// click on the centre tile can open that exact item (viewer / player).
static RECENT_HOME_PHOTOS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
static RECENT_HOME_VIDEOS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
// Recent book ids mirroring the Home Books row order — books open by library id
// (not path), so a click on a cover routes straight to that book in the reader.
static RECENT_HOME_BOOKS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn recent_home_photos() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    RECENT_HOME_PHOTOS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
fn recent_home_videos() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    RECENT_HOME_VIDEOS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
fn recent_home_books() -> &'static std::sync::Mutex<Vec<i64>> {
    RECENT_HOME_BOOKS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Generate thumbnails for the Home rows concurrently (4 at a time — enough to
/// hide latency, few enough to not stampede ffmpeg on a cold cache), keeping
/// the input order. Falls back to the source path when a thumb can't be made.
async fn home_thumbs_parallel(paths: Vec<String>, kind: tulipix_core::thumbs::ThumbKind) -> Vec<PathBuf> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let handles: Vec<_> = paths.into_iter().map(|p| {
        let sem = sem.clone();
        tokio::spawn(async move {
            let _permit = sem.acquire().await.ok();
            let src = PathBuf::from(&p);
            thumb_for(src.clone(), kind).await.unwrap_or(src)
        })
    }).collect();
    let mut thumbs = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(t) = h.await { thumbs.push(t); }
    }
    thumbs
}

/// Decoded pixels for one Home rail image. `slint::Image` is not `Send`, so it
/// cannot cross a thread — but `SharedPixelBuffer` is, which is what makes this
/// possible at all.
type HomePixels = slint::SharedPixelBuffer<slint::Rgba8Pixel>;

/// Decode `paths` into pixel buffers off the UI thread, in order.
///
/// The rails used to call `Image::load_from_path` inside the `upgrade_in_event_loop`
/// closure, which put every decode on the event loop: ten photos, ten videos,
/// ten covers and four baked hardcovers is ~34 image decodes in front of the
/// first frame of Home. That is the hitch when the app opens — the fan-out
/// across sections was already parallel, the decoding at the end of it was not.
///
/// Missing or undecodable files are skipped, so the caller gets a dense list.
async fn decode_home_pixels(paths: Vec<PathBuf>) -> Vec<HomePixels> {
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
    let handles: Vec<_> = paths
        .into_iter()
        .map(|p| {
            let sem = sem.clone();
            // Permit taken in the async half: `spawn_blocking` cannot await one,
            // and without it thirty-odd decodes would each claim a blocking
            // thread at once — the opposite of not overloading the machine.
            tokio::spawn(async move {
                let _permit = sem.acquire().await.ok();
                tokio::task::spawn_blocking(move || {
                    let img = image::open(&p).ok()?.into_rgba8();
                    let (w, h) = img.dimensions();
                    Some(HomePixels::clone_from_slice(img.as_raw(), w, h))
                })
                .await
                .ok()
                .flatten()
            })
        })
        .collect();
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        if let Ok(Some(px)) = h.await {
            out.push(px);
        }
    }
    out
}

/// Wrap decoded pixels back into images. Cheap — no decode, just a handle.
fn home_images(px: Vec<HomePixels>) -> Vec<slint::Image> {
    px.into_iter().map(slint::Image::from_rgba8).collect()
}

/// Load the 10 most-recent photo thumbnails for the Home Photos slideshow.
/// DB read, thumb render and image decode all happen off the UI thread; the
/// event loop only wraps the finished pixels.
fn kick_home_photos(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        // Same ordering as the Photos timeline (taken_at → mtime desc, trashed/
        // archived/missing excluded) so "recent" here matches what the user sees.
        let paths: Vec<String> = timeline_order("").await
            .into_iter().take(10).map(|(p, _)| p).collect();
        if let Ok(mut g) = recent_home_photos().lock() {
            *g = paths.iter().map(PathBuf::from).collect();
        }
        // Render thumbs off-thread, collect the paths (slint::Image isn't Send, so
        // the actual Image decode happens on the UI thread inside the closure).
        // Concurrent (bounded) instead of one-by-one: a cold cache means real
        // decodes per item, and serially that held the whole row back.
        let thumbs = home_thumbs_parallel(paths, tulipix_core::thumbs::ThumbKind::Photo).await;
        let px = decode_home_pixels(thumbs).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_home_recent_photos(slint::ModelRc::new(slint::VecModel::from(home_images(px))));
        });
    });
}

/// Load the 10 most-recent video thumbnails for the Home Videos slideshow.
fn kick_home_videos(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return; };
        let paths = sqlx::query_scalar::<_, String>(
            "SELECT abs_path FROM items WHERE missing_since IS NULL ORDER BY added DESC LIMIT 10")
            .fetch_all(&pool).await.unwrap_or_default();
        if let Ok(mut g) = recent_home_videos().lock() {
            *g = paths.iter().map(PathBuf::from).collect();
        }
        // Same bounded-concurrency thumb render as the Photos row — video
        // thumbs cost an ffmpeg frame-grab each on a cold cache.
        let thumbs = home_thumbs_parallel(paths, tulipix_core::thumbs::ThumbKind::Video).await;
        let px = decode_home_pixels(thumbs).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_home_recent_videos(slint::ModelRc::new(slint::VecModel::from(home_images(px))));
        });
    });
}

/// Load the 10 most-recently-added book covers for the Home Books row. Book
/// covers are already extracted image files (no thumb render needed), so this
/// just loads them straight off disk. The book ids are stashed in row order so
/// a cover click opens that exact book by library id in the reader.
fn kick_home_books(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("books").await else { return; };
        // Newest first, only books with a real cover on disk — an empty cover
        // slot would break the id ↔ row-index alignment the click relies on.
        let rows = sqlx::query_as::<_, (i64, String, String, String)>(
            "SELECT id, cover_path, title, author FROM books \
             WHERE missing = 0 AND cover_path != '' ORDER BY added_at DESC LIMIT 10")
            .fetch_all(&pool).await.unwrap_or_default();
        if let Ok(mut g) = recent_home_books().lock() {
            *g = rows.iter().map(|(id, _, _, _)| *id).collect();
        }
        // Covers for the row, decoded off-thread like the other two rails.
        let cover_px =
            decode_home_pixels(rows.iter().map(|(_, p, _, _)| PathBuf::from(p)).collect()).await;
        // The Welcome shelf's four: flat art plus its baked hardcover. Slot-wise
        // (Option per image, not a filtered list) because a book whose bake has
        // not landed yet still occupies its place on the shelf.
        let shelf_src: Vec<(PathBuf, PathBuf)> = rows
            .iter()
            .take(4)
            .map(|(_, p, _, _)| {
                let flat = PathBuf::from(p);
                let baked = tulipix_books::covers::baked_path(
                    &flat, tulipix_books::covers::BOOK_SUFFIX);
                (flat, baked)
            })
            .collect();
        let shelf_px: Vec<(Option<HomePixels>, Option<HomePixels>)> =
            tokio::task::spawn_blocking(move || {
                let load = |p: &std::path::Path| -> Option<HomePixels> {
                    let img = image::open(p).ok()?.into_rgba8();
                    let (w, h) = img.dimensions();
                    Some(HomePixels::clone_from_slice(img.as_raw(), w, h))
                };
                shelf_src.iter().map(|(flat, baked)| (load(baked), load(flat))).collect()
            })
            .await
            .unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_home_book_covers(slint::ModelRc::new(slint::VecModel::from(home_images(cover_px))));
            // The Welcome layout's Books shelf wants the BAKED `_book5`
            // hardcover, the same rendition the Books grid draws, plus the
            // per-title hue its outlines use. Load-only: the bake is produced at
            // scan time and by the prebake sweep, never here, so a book whose
            // bake has not landed yet simply falls back to its flat art.
            let wrap = |px: Option<HomePixels>| {
                px.map(slint::Image::from_rgba8).unwrap_or_default()
            };
            let books: Vec<HomeBook> = rows.iter().take(4).zip(shelf_px)
                .map(|((id, _, title, author), (baked, flat))| {
                    let [r, g, b] = tulipix_books::cover_hue_rgb(title);
                    HomeBook {
                        book: wrap(baked),
                        cover: wrap(flat),
                        title: title.clone().into(),
                        author: author.clone().into(),
                        hue: slint::Color::from_rgb_u8(r, g, b),
                        id: *id as i32,
                    }
                }).collect();
            w.set_home_recent_books(slint::ModelRc::new(slint::VecModel::from(books)));
        });
    });
}


/// Load cloud remotes (name + backend + storage usage) for the Home Cloud row.
fn kick_home_cloud(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("cloud").await else { return; };
        let rows = sqlx::query_as::<_, (String, String)>(
            "SELECT name, backend FROM remotes ORDER BY name COLLATE NOCASE LIMIT 3")
            .fetch_all(&pool).await.unwrap_or_default();
        // Push the tiles immediately (usage blank), then again once the
        // rclone-about usage map resolves — first paint stays instant even
        // when a remote needs the full 15s timeout.
        let push = |weak: &slint::Weak<MainWindow>,
                    rows: Vec<(String, String)>,
                    usage: std::collections::HashMap<String, String>| {
            let _ = weak.upgrade_in_event_loop(move |w| {
                let remotes: Vec<HomeRemote> = rows.into_iter()
                    .map(|(name, backend)| HomeRemote {
                        usage: usage.get(&name).cloned().unwrap_or_default().into(),
                        name: name.into(), backend: backend.into(),
                    })
                    .collect();
                w.set_home_cloud_remotes(slint::ModelRc::new(slint::VecModel::from(remotes)));
            });
        };
        // Last-known usage seeds the instant push so the quota line never
        // flickers blank on the periodic re-kick.
        static LAST_USAGE: OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> = OnceLock::new();
        let last = LAST_USAGE.get_or_init(Default::default);
        push(&weak, rows.clone(), last.lock().map(|g| g.clone()).unwrap_or_default());
        let names: Vec<String> = rows.iter().map(|(n, _)| n.clone()).collect();
        if names.is_empty() { return; }
        let usage = tulipix_sec_cloud::home_usage_map(&names).await;
        if let Ok(mut g) = last.lock() { *g = usage.clone(); }
        push(&weak, rows, usage);
    });
}

// ── Home CONTINUE strip v2 (np.p6.home.continue) ─────────────────────────────
// Several in-progress items across books / podcasts / audiobooks with covers +
// progress bars, filterable from the UI chips. Rows are gathered off-thread as
// a Send-safe snapshot (pixel buffers, not Images); the event-loop push only
// wraps pixels — O(1) per row.
#[derive(Clone)]
struct HomeContRow {
    kind: &'static str,   // "book" | "podcast" | "audiobook" | "video"
    title: String,
    author: String,
    sub: String,
    frac: f32,            // 0‥1; < 0 = unknown (bar hidden)
    cover: Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    id: i64,
    path: String,
    ts: i64,              // recency for the "All" interleave
}
/// Episode id currently loaded in the podcast player (0 = none). The 5s
/// playback ticker persists its position — nothing else ever wrote
/// podcast_episodes.position_s, so resume/Continue never saw live listening.
static PODCAST_NOW_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
static HOME_CONT_ROWS: OnceLock<std::sync::Mutex<Vec<HomeContRow>>> = OnceLock::new();
static HOME_CONT_FILTER: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
fn home_cont_rows() -> &'static std::sync::Mutex<Vec<HomeContRow>> {
    HOME_CONT_ROWS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
fn home_cont_filter() -> &'static std::sync::Mutex<String> {
    HOME_CONT_FILTER.get_or_init(|| std::sync::Mutex::new("all".into()))
}
// User-dismissed CONTINUE items ("kind:id" / "audiobook:folder"), persisted to
// continue_dismissed.json so a removed card never resurfaces on refresh.
static HOME_CONT_DISMISSED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
fn home_cont_dismissed() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    HOME_CONT_DISMISSED.get_or_init(|| {
        let set = tulipix_core::paths::config_dir()
            .and_then(|d| std::fs::read_to_string(d.join("continue_dismissed.json")).ok())
            .and_then(|s| serde_json::from_str::<Vec<String>>(&s).ok())
            .map(|v| v.into_iter().collect())
            .unwrap_or_default();
        std::sync::Mutex::new(set)
    })
}
fn home_cont_dismiss_key(kind: &str, id: i64, path: &str) -> String {
    if kind == "audiobook" { format!("audiobook:{path}") } else { format!("{kind}:{id}") }
}
fn save_home_cont_dismissed() {
    let Some(dir) = tulipix_core::paths::config_dir() else { return; };
    let Ok(g) = home_cont_dismissed().lock() else { return; };
    let v: Vec<&String> = g.iter().collect();
    if let Ok(s) = serde_json::to_string(&v) {
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("continue_dismissed.json"), s);
    }
}

// ── Home layout (Settings › PERSONAL › Home Layout) ─────────────────────────
// Four compositions over one data set. Spec: docs/home-layouts/README.md.
// Ids are the on-disk names (`classic` / `cinema` / `stream` / `welcome`) and do
// not change; the picker shows them as Poweruser / Cinema / Timeline / Focused.
// Stored in Settings.advanced, so there is no schema change: `home.layout` holds
// the choice and `home.hidden.<layout>` a comma-separated list of switched-off
// card ids, per layout, so switching away and back restores what you had.

/// Every card id any layout can own, in one place. `apply_home_cards` walks this
/// to set the window booleans, so adding a card is one line here plus one there.
const HOME_CARD_IDS: [&str; 15] = [
    "hero", "continue", "player", "quick", "photos", "videos", "music", "books",
    "cloud", "tools", "transfer", "finances", "library", "ticker", "hub",
];

fn home_layout_valid(v: &str) -> &'static str {
    match v {
        "cinema" => "cinema",
        "stream" => "stream",
        "classic" => "classic",
        // Unknown or unset lands on Focused (`welcome`), the default Home.
        // Unknown covers the three layouts that were deleted, so a settings
        // file naming one of those opens on the default rather than a blank
        // page. Ids are the on-disk names and stay as they are — the display
        // names (Poweruser / Cinema / Timeline / Focused) live in the picker.
        _ => "welcome",
    }
}

/// The cards a layout offers, in popup order. A layout never lists a card it has
/// no place for, and never lists one whose switch would not do anything yet —
/// Classic's rail cards (player, quick) still owe their geometry work, so they
/// are absent until `docs/home-layouts/05-classic.md` is done.
fn home_layout_cards(layout: &str) -> &'static [(&'static str, &'static str, &'static str)] {
    match layout {
        // My Hub left Cinema on 2026-08-10: eight launchers on the shelf said
        // what the sidebar already says, and they cost Continue its height.
        "cinema" => &[
            ("hero", "Hero", "The full-bleed backdrop and Resume for what you were watching"),
            ("continue", "Continue rail", "In-progress films, books and episodes — and the hero's slideshow"),
            ("quick", "Quick actions", "Stream, YouTube, Music D/L, Genesis, Radio and Live TV in the header"),
            ("player", "Player panel", "What is playing, on the right — and a shuffled library when nothing is"),
            ("hub", "My Hub card", "One section card at a time under the player, on a 10-second turn"),
        ],
        "stream" => &[
            ("photos", "Photo events", "Imports and album changes in the feed"),
            ("videos", "Video events", "Additions and what you left half-watched"),
            ("music", "Music events", "What played, and what finished"),
            ("books", "Book events", "Reading and listening progress"),
            ("cloud", "Cloud events", "Sync runs and their results"),
            ("tools", "Tool events", "Finished and failed jobs"),
            ("transfer", "Transfer events", "Files sent and received, plus the TRANSFERS block"),
            ("finances", "Money events", "Dues and payments, plus the STANDING block"),
            ("quick", "Quick actions", "Stream, YouTube, Music D/L, Genesis, Radio and Live TV in the header"),
            ("player", "Player block", "Now playing at the top of the rail"),
            ("library", "Library block", "The counter table in the rail"),
            ],
        "welcome" => &[
            ("hero", "Hero greeting", "The big hello and the collage from your own library"),
            ("quick", "Launch bar", "Stream, Random Radio, Music D/L, Genesis, Live TV, Tools"),
            ("continue", "Continue card", "Three in-progress rows with their progress bars"),
            ("library", "Recently Added", "The six newest thumbnails"),
            ("photos", "Photos tile", "One tile in My Hub"),
            ("videos", "Videos tile", "One tile in My Hub"),
            ("music", "Music tile", "One tile in My Hub"),
            ("books", "Books tile", "One tile in My Hub"),
            ("cloud", "Clouds tile", "One tile in My Hub"),
            ("tools", "Tools tile", "One tile in My Hub"),
            ("transfer", "Transfer tile", "One tile in My Hub"),
            ("finances", "Finances tile", "One tile in My Hub"),
            ("player", "Player bar", "The transport pinned along the bottom of the page"),
        ],
        // classic
        _ => &[
            ("photos", "Photos", "Slideshow tile, top row"),
            ("videos", "Videos", "Slideshow tile, top row"),
            ("books", "Books", "Cover spines and the book you are reading"),
            ("cloud", "Cloud", "Your remotes and their sizes"),
            ("tools", "Tools", "The tools you reach for most"),
            ("continue", "Continue", "The four-slot resume strip"),
        ],
    }
}

/// Layouts that put the Finances numbers on Home, and therefore need one
/// Finances pass when Home opens — the section itself may never have run.
fn home_layout_wants_money(layout: &str) -> bool {
    // Timeline and Focused put the money somewhere: a rail block, a hub tile
    // subtitle. Cinema is back on the list — its hub carousel carries the same
    // Finances tile. Classic leaves it to the section.
    matches!(layout, "stream" | "welcome" | "cinema")
}

fn home_hidden_key(layout: &str) -> String { format!("home.hidden.{layout}") }

/// Card ids switched off in this layout. Unknown ids are ignored on read, so a
/// card renamed later cannot poison the file.
fn home_hidden(layout: &str) -> std::collections::HashSet<String> {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.text(&home_hidden_key(layout))
        .split(',')
        .map(|p| p.trim())
        .filter(|p| HOME_CARD_IDS.contains(p))
        .map(|p| p.to_string())
        .collect()
}

fn home_hidden_save(layout: &str, hidden: &std::collections::HashSet<String>) {
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    let key = home_hidden_key(layout);
    if hidden.is_empty() {
        s.advanced.remove(&key);
    } else {
        // Written in HOME_CARD_IDS order so the file is stable across writes.
        let csv = HOME_CARD_IDS.iter().filter(|id| hidden.contains(**id))
            .copied().collect::<Vec<_>>().join(",");
        s.advanced.insert(key, csv);
    }
    if let Err(e) = s.save() { tracing::warn!(error = %e, "save home cards"); }
}

/// Push the active layout's card state into the window: the fifteen booleans the
/// layouts read, and the row model the customize popup lists.
fn apply_home_cards(w: &MainWindow) {
    let layout = w.get_home_layout().to_string();
    let hidden = home_hidden(&layout);
    let on = |id: &str| !hidden.contains(id);
    w.set_hc_hero(on("hero"));
    w.set_hc_continue(on("continue"));
    w.set_hc_player(on("player"));
    w.set_hc_quick(on("quick"));
    w.set_hc_photos(on("photos"));
    w.set_hc_videos(on("videos"));
    w.set_hc_music(on("music"));
    w.set_hc_books(on("books"));
    w.set_hc_cloud(on("cloud"));
    w.set_hc_tools(on("tools"));
    w.set_hc_transfer(on("transfer"));
    w.set_hc_finances(on("finances"));
    w.set_hc_library(on("library"));
    w.set_hc_ticker(on("ticker"));
    w.set_hc_hub(on("hub"));

    let cards = home_layout_cards(&layout);
    let live = cards.iter().filter(|(id, _, _)| on(*id)).count();
    let rows: Vec<HomeCardRow> = cards.iter().map(|(id, label, hint)| HomeCardRow {
        id: (*id).into(),
        label: (*label).into(),
        hint: (*hint).into(),
        on: on(*id),
        // The last card standing cannot be switched off: Home would be blank.
        locked: on(id) && live <= 1,
        })
        .collect();
    w.set_home_cards(slint::ModelRc::new(slint::VecModel::from(rows)));

    // Stream's section cards filter the feed, so the rows it already gathered
    // have to be re-pushed against the new card state. Cheap: no queries, just
    // the cached rows through the filter again.
    push_home_events(&w.as_weak());
}

fn wire_home_layout(window: &MainWindow) {
    let w = window.as_weak();
    window.on_use_home_layout(move |l| {
        let Some(w) = w.upgrade() else { return; };
        let layout = home_layout_valid(l.as_str());
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.advanced.insert("home.layout".into(), layout.to_string());
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save home layout"); }
        w.set_home_layout(layout.into());
        apply_home_cards(&w);
        // The money panel (Cinema), row (Editorial), pane (Columns) and rail block
        // (Stream) all read the Finances props, which only the section fills. Ask
        // for one pass now so choosing a layout is not followed by em dashes.
        #[cfg(feature = "finances")]
        {
            if home_layout_wants_money(layout) { tulipix_sec_finances::refresh(&w); }
        }
        push_home_cinema_extras(&w);
        // Stream's feed costs eight queries, so it is gathered when the layout
        // that shows it is chosen rather than on every Home landing.
        if layout == "stream" { kick_home_events(&w); }
        tracing::info!(%layout, "home layout chosen");
    });

    let w = window.as_weak();
    window.on_home_card_set(move |card, on| {
        let Some(w) = w.upgrade() else { return; };
        let card = card.to_string();
        if !HOME_CARD_IDS.contains(&card.as_str()) { return; }
        let layout = w.get_home_layout().to_string();
        let mut hidden = home_hidden(&layout);
        if on {
            hidden.remove(&card);
        } else {
            // Enforced here as well as in the popup: a layout with nothing left
            // is a blank Home, and the UI is not the only caller.
            let cards = home_layout_cards(&layout);
            let live = cards.iter().filter(|(id, _, _)| !hidden.contains(*id)).count();
            if live <= 1 { return; }
            hidden.insert(card);
        }
        home_hidden_save(&layout, &hidden);
        apply_home_cards(&w);
    });

    let w = window.as_weak();
    window.on_home_cards_reset(move || {
        let Some(w) = w.upgrade() else { return; };
        let layout = w.get_home_layout().to_string();
        home_hidden_save(&layout, &std::collections::HashSet::new());
        apply_home_cards(&w);
    });

    // Cinema's hero buttons. Resume routes by kind through the same handlers the
    // Continue strip uses, so there is one resume path per medium, not two.
    let w = window.as_weak();
    window.on_home_hero_resume(move || {
        let Some(w) = w.upgrade() else { return; };
        let kind = w.get_home_hero_kind().to_string();
        let id = w.get_home_hero_id();
        let path = w.get_home_hero_path().to_string();
        match kind.as_str() {
            "video" => w.invoke_home_continue_video(id, path.into()),
            "podcast" => w.invoke_music_podcast_play(id),
            "book" => w.invoke_books_open_details(id),
            _ => {
                w.set_music_view("audiobooks".into());
                w.set_active_section("music".into());
                w.invoke_section_changed("music".into());
            }
        }
    });
    let w = window.as_weak();
    window.on_home_hero_details(move || {
        let Some(w) = w.upgrade() else { return; };
        // "Details" means the section that owns the item, with it selected.
        let kind = w.get_home_hero_kind().to_string();
        let id = w.get_home_hero_id();
        if kind == "book" {
            w.invoke_books_open_details(id);
            return;
        }
        let section = if kind == "video" { "videos" } else { "music" };
        w.set_active_section(section.into());
        w.invoke_section_changed(section.into());
    });
}

/// The money and transfer facts Home shows — Cinema's two glass panels and
/// Editorial's two rows — shaped for Home so neither page has to import
/// `page_finances.slint`. Reads what the Finances section has already loaded;
/// empty until it has run once.
/// `2026-08-15` → `15-08-26`. Anything that is not an ISO date comes back
/// unchanged, so a humanised string the section may hand over still prints.
fn home_due_date(iso: &str) -> String {
    let p: Vec<&str> = iso.split('-').collect();
    match p.as_slice() {
        [y, m, d] if y.len() == 4 && m.len() == 2 && d.len() == 2 => {
            format!("{d}-{m}-{}", &y[2..])
        }
        _ => iso.to_string(),
    }
}

fn push_home_cinema_extras(w: &MainWindow) {
    // Twelve months as 0‥1 heights, oldest first — the bar row wants a ratio,
    // and the section already computed the percentage.
    let months: Vec<f32> = w.get_fin_months().iter()
        .map(|m| (m.expense_pct as f32 / 100.0).clamp(0.0, 1.0))
        .collect();
    w.set_home_fin_months(slint::ModelRc::new(slint::VecModel::from(months)));
    // …and what each bar stands for, so Timeline's chart can restate STANDING
    // when a bar is picked without asking the backend for another month.
    let labels: Vec<slint::SharedString> =
        w.get_fin_months().iter().map(|m| m.long.clone()).collect();
    w.set_home_fin_month_labels(slint::ModelRc::new(slint::VecModel::from(labels)));
    let spends: Vec<slint::SharedString> =
        w.get_fin_months().iter().map(|m| m.expense.clone()).collect();
    w.set_home_fin_month_spends(slint::ModelRc::new(slint::VecModel::from(spends)));

    // Up to three obligations, overdue first — the panel is 274 px wide and a
    // fourth row would push the shelf.
    let mut dues: Vec<HomeDue> = w.get_fin_needs_you().iter()
        .map(|o| HomeDue {
            name: o.name.clone(),
            amount: if o.actual.is_empty() { o.estimate.clone() } else { o.actual.clone() },
            // The section stores ISO (`2026-08-15`); Home prints DD-MM-YY, which
            // is what the rest of this app's dates look like.
            when: home_due_date(o.due.as_str()).into(),
            late: o.status.as_str() == "overdue",
        })
        .collect();
    dues.sort_by_key(|d| !d.late);
    dues.truncate(3);
    w.set_home_fin_dues(slint::ModelRc::new(slint::VecModel::from(dues)));

    // Transfer stays a door: the server binds only while that section is open,
    // so Home can honestly show where files land and nothing more. The path comes
    // from the prop `tulipix_sec_transfer::wire` already resolved at boot — the
    // raw setting is empty until someone picks a folder, the resolved one is not.
    w.set_home_transfer_inbox(w.get_transfer_inbox());
    w.set_home_transfer_note("".into());
}

// ── Stream layout: the activity feed ────────────────────────────────────────
// Spec: docs/home-layouts/04-stream.md §Data. There is no events table in
// tulipix, and this does not add one: eight small `ORDER BY … LIMIT` queries plus
// the app's activity log, merged and sorted in Rust when Home opens. Correct by
// construction — it reads the same rows the sections read — and there are no new
// write paths to keep honest.

/// One merged activity event, before it becomes a `HomeEvent` for the page.
#[derive(Clone)]
struct HomeEvRow {
    at: i64,
    section: &'static str,
    kind: &'static str,
    title: String,
    sub: String,
    /// Pill label. Only set where there is a concrete thing to resume — the row
    /// itself already opens the section, and two controls doing one job is noise.
    action: &'static str,
    alarm: bool,
    id: i64,
    path: String,
}

/// How many events the feed keeps after the merge. Rows are newest-first by the
/// time this bites, so the cap drops the OLDEST — the tail of the list — and the
/// hundredth-newest event is the last one Timeline will ever draw.
const HOME_EVENTS_MAX: usize = 50;
/// Same section + same kind inside this window collapses into one row, so a big
/// import cannot flood the feed with four hundred "added" lines.
const HOME_EVENT_WINDOW: i64 = 15 * 60;

fn home_ev_cache() -> &'static std::sync::Mutex<Vec<HomeEvRow>> {
    static C: std::sync::OnceLock<std::sync::Mutex<Vec<HomeEvRow>>> = std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

fn file_name_of(path: &str) -> String {
    std::path::Path::new(path).file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string())
}

/// Newly scanned files in a section that keeps the common `items` table.
///
/// The title is written as `"one|many"`: `collapse_events` picks the half it
/// needs once it knows how many rows folded together, so neither the query nor
/// the page has to guess the plural.
async fn ev_items(section: &'static str, one: &'static str, many: &'static str) -> Vec<HomeEvRow> {
    let Ok(pool) = pool_for(section).await else { return Vec::new(); };
    sqlx::query_as::<_, (String, i64)>(
        "SELECT abs_path, added FROM items WHERE missing_since IS NULL \
         ORDER BY added DESC LIMIT 30")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(path, at)| HomeEvRow {
            at, section, kind: "added",
            title: format!("{one}|{many}"),
            sub: file_name_of(&path),
            action: "", alarm: false, id: -1, path,
        })
        .collect()
}

/// Books: what was added, and what you were reading. Progress is the one resume
/// source with a timestamp of its own, which is why videos only report additions.
async fn ev_books() -> Vec<HomeEvRow> {
    let Ok(pool) = pool_for("books").await else { return Vec::new(); };
    let mut out: Vec<HomeEvRow> = sqlx::query_as::<_, (String, String, i64)>(
        "SELECT title, author, added_at FROM books WHERE missing = 0 \
         ORDER BY added_at DESC LIMIT 12")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(title, author, at)| HomeEvRow {
            at, section: "books", kind: "added",
            title: "Book added|books added".into(),
            sub: if author.is_empty() { title } else { format!("{title} · {author}") },
            action: "", alarm: false, id: -1, path: String::new(),
        })
        .collect();
    out.extend(
        sqlx::query_as::<_, (i64, String, f64, i64)>(
            "SELECT b.id, b.title, p.percent, p.updated_at \
             FROM progress p JOIN books b ON b.id = p.book_id \
             WHERE b.finished = 0 AND b.missing = 0 ORDER BY p.updated_at DESC LIMIT 8")
            .fetch_all(&pool).await.unwrap_or_default()
            .into_iter()
            .map(|(id, title, pct, at)| HomeEvRow {
                at, section: "books", kind: "resumed",
                title: format!("Reading {title}"),
                sub: format!("{}% through", (pct.clamp(0.0, 100.0)).round() as i64),
                action: "Resume", alarm: false, id, path: String::new(),
            }));
    out
}

/// What played. `play_history` is written by the player itself, so this is the
/// one source that needs no interpretation.
async fn ev_music() -> Vec<HomeEvRow> {
    let Ok(pool) = pool_for("music").await else { return Vec::new(); };
    sqlx::query_as::<_, (Option<String>, String, i64)>(
        "SELECT tm.title, i.abs_path, h.played_at FROM play_history h \
         JOIN items i ON i.id = h.item_id \
         LEFT JOIN track_meta tm ON tm.item_id = h.item_id \
         ORDER BY h.played_at DESC LIMIT 30")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(title, path, at)| HomeEvRow {
            at, section: "music", kind: "played",
            title: "Track played|tracks played".into(),
            sub: title.filter(|t| !t.is_empty()).unwrap_or_else(|| file_name_of(&path)),
            action: "", alarm: false, id: -1, path: String::new(),
        })
        .collect()
}

/// Podcast episodes that arrived. Episodes have a `published` date but no local
/// timestamp, so this reports the feed's clock, which is the honest one.
async fn ev_podcasts() -> Vec<HomeEvRow> {
    let Ok(pool) = pool_for("podcasts").await else { return Vec::new(); };
    sqlx::query_as::<_, (i64, Option<String>, String, Option<i64>)>(
        "SELECT e.id, e.title, p.title, e.published FROM podcast_episodes e \
         JOIN podcasts p ON p.id = e.podcast_id \
         WHERE e.published IS NOT NULL ORDER BY e.published DESC LIMIT 8")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .filter_map(|(id, ep, show, published)| {
            let at = published?;
            Some(HomeEvRow {
                at, section: "music", kind: "episode",
                title: format!("New episode — {}", ep.unwrap_or_else(|| show.clone())),
                sub: show, action: "Play", alarm: false, id, path: String::new(),
            })
        })
        .collect()
}

/// Finished and failed Tools jobs, from the executor's own table.
async fn ev_tools() -> Vec<HomeEvRow> {
    let Ok(pool) = pool_for("tools").await else { return Vec::new(); };
    sqlx::query_as::<_, (String, String, i64, Option<String>)>(
        "SELECT kind, state, updated, message FROM jobs \
         WHERE state IN ('done', 'error') ORDER BY updated DESC LIMIT 10")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(kind, state, at, msg)| HomeEvRow {
            at, section: "tools", kind: "job",
            title: if state == "error" { format!("Job failed — {kind}") }
                else { format!("Job finished — {kind}") },
            sub: msg.filter(|m| !m.is_empty()).unwrap_or_else(|| "no message".into()),
            action: "", alarm: state == "error", id: -1, path: String::new(),
        })
        .collect()
}

/// The transfer ledger. Both directions: a phone that pushed a file here is
/// exactly what this feed exists to show, and it happens with the app untouched.
async fn ev_transfers() -> Vec<HomeEvRow> {
    let Ok(pool) = pool_for("transfers").await else { return Vec::new(); };
    sqlx::query_as::<_, (String, String, i64, String, String, i64)>(
        "SELECT direction, name, bytes, peer, status, at FROM transfers \
         ORDER BY at DESC LIMIT 10")
        .fetch_all(&pool).await.unwrap_or_default()
        .into_iter()
        .map(|(dir, name, bytes, peer, status, at)| {
            let ok = status == "ok";
            HomeEvRow {
                at, section: "transfer",
                kind: if dir == "in" { "received" } else { "sent" },
                title: match (dir.as_str(), ok) {
                    (_, false) => format!("Transfer failed — {name}"),
                    ("in", _) => format!("Received {name}"),
                    _ => format!("Sent {name}"),
                },
                sub: format!("{} · {peer}", human_size(bytes.max(0) as u64)),
                action: "", alarm: !ok, id: -1, path: String::new(),
            }
        })
        .collect()
}

/// Collapse runs of the same section + kind inside `HOME_EVENT_WINDOW`, and turn
/// the `"one|many"` titles the `added` / `played` sources carry into real ones.
fn collapse_events(rows: Vec<HomeEvRow>) -> Vec<HomeEvRow> {
    let mut out: Vec<HomeEvRow> = Vec::new();
    let mut counts: Vec<usize> = Vec::new();
    for r in rows {
        let fold = out.last().is_some_and(|p: &HomeEvRow| {
            p.section == r.section && p.kind == r.kind && p.at - r.at <= HOME_EVENT_WINDOW
        });
        if fold {
            if let Some(n) = counts.last_mut() { *n += 1; }
            continue;
        }
        out.push(r);
        counts.push(1);
    }
    for (r, n) in out.iter_mut().zip(counts) {
        let parts = r.title.split_once('|').map(|(a, b)| (a.to_string(), b.to_string()));
        if let Some((one, many)) = parts {
            r.title = if n > 1 { format!("{n} {many}") } else { one };
            if n > 1 { r.sub = format!("{} +{} more", r.sub, n - 1); }
        } else if n > 1 {
            r.sub = format!("{} +{} more", r.sub, n - 1);
        }
    }
    out
}

/// Gather every source, merge, collapse, cap, cache — then push through the
/// active filter. Runs on Home entry for the Stream layout only.
fn kick_home_events(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        // Independent databases, so they run concurrently: the feed lands in
        // ~max(source) rather than the sum.
        let (photos, videos, books, music, pods, tools, transfers) = tokio::join!(
            ev_items("photos", "Photo added", "photos added"),
            ev_items("videos", "Video added", "videos added"),
            ev_books(),
            ev_music(),
            ev_podcasts(),
            ev_tools(),
            ev_transfers(),
        );
        let mut all: Vec<HomeEvRow> = Vec::new();
        all.extend(photos);
        all.extend(videos);
        all.extend(books);
        all.extend(music);
        all.extend(pods);
        all.extend(tools);
        all.extend(transfers);

        #[cfg(feature = "finances")]
        {
            for (at, title, sub, alarm) in tulipix_sec_finances::recent_events(10).await {
                all.push(HomeEvRow {
                    at, section: "finances", kind: "paid",
                    title, sub, action: "", alarm, id: -1, path: String::new(),
                });
            }
        }

        // The events with no table of their own: a folder watched, a folder
        // dropped, a rescan started. Section "library", so no section card hides
        // them — they are about the library itself.
        for (at, _accent, title, sub) in tulipix_common::load_activity() {
            all.push(HomeEvRow {
                at, section: "library", kind: "note",
                title, sub, action: "", alarm: false, id: -1, path: String::new(),
            });
        }

        all.retain(|r| r.at > 0);
        all.sort_by(|a, b| b.at.cmp(&a.at));
        let mut rows = collapse_events(all);
        rows.truncate(HOME_EVENTS_MAX);
        if let Ok(mut g) = home_ev_cache().lock() { *g = rows; }
        push_home_events(&weak);
    });
}

/// Which filter chip a section belongs to. "library" notes only show under
/// Everything: they are not media, money or a device.
fn home_ev_in_filter(section: &str, filter: &str) -> bool {
    match filter {
        "media" => matches!(section, "photos" | "videos" | "music" | "books"),
        "money" => section == "finances",
        "devices" => matches!(section, "transfer" | "cloud" | "tools"),
        _ => true,
    }
}

/// Push the cached events through the section cards and the active filter chip,
/// deciding the clock string and the group caption on the way.
fn push_home_events(weak: &slint::Weak<MainWindow>) {
    let rows = home_ev_cache().lock().map(|g| g.clone()).unwrap_or_default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        let filter = w.get_home_feed_filter().to_string();
        let on = |section: &str| match section {
            "photos" => w.get_hc_photos(),
            "videos" => w.get_hc_videos(),
            "music" => w.get_hc_music(),
            "books" => w.get_hc_books(),
            "cloud" => w.get_hc_cloud(),
            "tools" => w.get_hc_tools(),
            "transfer" => w.get_hc_transfer(),
            "finances" => w.get_hc_finances(),
            // Library notes are not a card; they always belong.
            _ => true,
        };
        // Thumbnails come from the feeds Home already decoded — the newest photos
        // *are* the newest additions, so the first three answer for that row. No
        // per-event decode, which the spec's risk 2 warns about.
        let pick = |m: &slint::ModelRc<slint::Image>, i: usize| -> slint::Image {
            use slint::Model;
            m.row_data(i).unwrap_or_default()
        };
        let photos_m = w.get_home_recent_photos();
        let videos_m = w.get_home_recent_videos();
        let books_m = w.get_home_book_covers();
        let mut photo_done = false;
        let mut video_done = false;
        let mut book_done = false;

        let now = tulipix_common::now_secs();
        let today = chrono::Local::now().date_naive();
        let mut last_group = String::new();
        let mut out: Vec<HomeEvent> = Vec::new();
        for r in rows.iter().filter(|r| on(r.section) && home_ev_in_filter(r.section, &filter)) {
            let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(r.at, 0)
                .map(|t| t.with_timezone(&chrono::Local));
            let group = if now - r.at < 1800 {
                "NOW"
            } else {
                match dt.map(|d| (today - d.date_naive()).num_days()) {
                    Some(0) => "TODAY",
                    Some(1) => "YESTERDAY",
                    _ => "EARLIER",
                }
            };
            let head = group != last_group;
            last_group = group.to_string();
            // One thumbnail cluster per medium — the newest row of that section.
            let (t1, t2, t3) = match (r.section, r.kind) {
                ("photos", "added") if !photo_done => {
                    photo_done = true;
                    (pick(&photos_m, 0), pick(&photos_m, 1), pick(&photos_m, 2))
                }
                ("videos", "added") if !video_done => {
                    video_done = true;
                    (pick(&videos_m, 0), pick(&videos_m, 1), pick(&videos_m, 2))
                }
                ("books", _) if !book_done => {
                    book_done = true;
                    (pick(&books_m, 0), slint::Image::default(), slint::Image::default())
                }
                _ => (slint::Image::default(), slint::Image::default(), slint::Image::default()),
            };
            out.push(HomeEvent {
                at: dt.map(|d| d.format("%H:%M").to_string()).unwrap_or_default().into(),
                // The clock alone never said which day a row belonged to once the
                // feed reached past yesterday — the gutter prints both now.
                day: dt.map(|d| d.format("%d-%m-%y").to_string()).unwrap_or_default().into(),
                group: group.into(),
                head,
                section: r.section.into(),
                kind: r.kind.into(),
                title: r.title.as_str().into(),
                sub: r.sub.as_str().into(),
                action: r.action.into(),
                alarm: r.alarm,
                id: r.id as i32,
                path: r.path.as_str().into(),
                t1, t2, t3,
            });
        }
        // An empty today is not an empty page: the feed simply reaches further
        // back, and says so.
        let stale = out.first()
            .is_some_and(|e| e.group.as_str() != "NOW" && e.group.as_str() != "TODAY");
        w.set_home_feed_note(if stale { "Nothing today — showing what came before.".into() }
            else { slint::SharedString::new() });
        w.set_home_events(slint::ModelRc::new(slint::VecModel::from(out)));
    });
}

fn wire_home_stream(window: &MainWindow) {
    // The Finances refresh is async: Home pushes its money blocks on entry, and
    // the numbers land afterwards. This is that landing — without it, Timeline's
    // STANDING and Cinema's Finances tile stayed empty until the Finances page
    // had been opened once.
    let w = window.as_weak();
    window.on_home_money_ready(move || {
        if let Some(w) = w.upgrade() { push_home_cinema_extras(&w); }
    });
    let w = window.as_weak();
    window.on_home_feed_filter_set(move |f| {
        let Some(w) = w.upgrade() else { return; };
        w.set_home_feed_filter(f);
        push_home_events(&w.as_weak());
    });
    let w = window.as_weak();
    window.on_home_event_action(move |e| {
        let Some(w) = w.upgrade() else { return; };
        match (e.section.as_str(), e.kind.as_str()) {
            ("books", "resumed") => w.invoke_books_open_details(e.id),
            ("music", "episode") => w.invoke_music_podcast_play(e.id),
            // Everything else has no id worth acting on; the row click already
            // opens the section, so this is the same door.
            (s, _) => {
                w.set_active_section(s.into());
                w.invoke_section_changed(s.into());
            }
        }
    });
}

/// Cap a CONTINUE card title at 50 characters, then "...". Counted in chars,
/// not bytes, so a Devanagari or accented title is not cut mid-codepoint.
const CONTINUE_TITLE_CHARS: usize = 50;

fn clip_title(title: &str) -> String {
    if title.chars().count() <= CONTINUE_TITLE_CHARS {
        return title.to_string();
    }
    let head: String = title.chars().take(CONTINUE_TITLE_CHARS).collect();
    format!("{}...", head.trim_end())
}

/// Cinema's hero: the newest in-progress item, spelled out for the page. Nothing
/// queries for this — it is the first row the CONTINUE strip already gathered, so
/// the hero and the rail can never disagree about what you were last doing.
fn set_home_hero(w: &MainWindow, row: Option<&HomeContRow>) {
    let Some(r) = row else {
        // Empty library, or nothing started yet. The page has its own copy for
        // this case; clearing the title is what selects it.
        w.set_home_hero_kind("".into());
        w.set_home_hero_title("".into());
        w.set_home_hero_kicker("".into());
        w.set_home_hero_meta("".into());
        w.set_home_hero_frac(-1.0);
        w.set_home_hero_art(slint::Image::default());
        w.set_home_hero_id(-1);
        w.set_home_hero_path("".into());
        return;
    };
    // The kicker says which medium it is, in the words that medium uses; the meta
    // line carries the detail the 62 px title has no room for.
    let kicker = match r.kind {
        "video" => "VIDEO",
        "podcast" => "PODCAST",
        "audiobook" => "AUDIOBOOK",
        _ => "BOOK",
    };
    let meta = if r.sub.is_empty() { r.author.clone() } else { r.sub.clone() };
    w.set_home_hero_kind(r.kind.into());
    w.set_home_hero_title(r.title.as_str().into());
    w.set_home_hero_kicker(if r.author.is_empty() { kicker.into() }
        else { format!("{kicker} · {}", r.author).into() });
    w.set_home_hero_meta(meta.into());
    w.set_home_hero_frac(r.frac);
    w.set_home_hero_art(r.cover.clone().map(slint::Image::from_rgba8).unwrap_or_default());
    w.set_home_hero_id(r.id as i32);
    w.set_home_hero_path(r.path.as_str().into());
}

/// Push the cached CONTINUE rows through the active kind filter (≤4 cards —
/// the strip has four fixed pill slots). "All" leads with the newest of each
/// kind (video/book/podcast/audiobook) for variety, then BACKFILLS the empty
/// slots with the next-newest rows whatever their kind — a library with only
/// books in progress used to show one card and three empty wells. Kind tabs
/// show up to 4 of that kind.
fn push_home_continue(weak: &slint::Weak<MainWindow>) {
    let filter = home_cont_filter().lock().map(|g| g.clone()).unwrap_or_else(|_| "all".into());
    let rows = home_cont_rows().lock().map(|g| g.clone()).unwrap_or_default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        // Cinema's hero is the newest in-progress item overall, taken before the
        // filter runs: the chips are a Continue-strip control, and the backdrop
        // has no business changing because someone clicked "Podcasts".
        set_home_hero(&w, rows.first());
        // Rows are already newest-first, so "first of each kind" = newest.
        let picked: Vec<_> = if filter == "all" {
            // Strictly ONE per kind, and nothing else. Free slots used to be
            // backfilled with the next-newest rows, which meant "All" could show
            // four books — the same four the Books chip shows — and stopped
            // being the overview it is there to be.
            let mut seen_kinds = std::collections::HashSet::new();
            let mut out: Vec<_> = Vec::with_capacity(4);
            for r in rows.iter() {
                if out.len() == 4 { break; }
                if seen_kinds.insert(r.kind) { out.push(r.clone()); }
            }
            out
        } else {
            rows.iter().filter(|r| r.kind == filter).take(4).cloned().collect()
        };
        let items: Vec<HomeContinue> = picked.into_iter()
            .map(|r| HomeContinue {
                kind: r.kind.into(),
                title: clip_title(&r.title).into(),
                author: r.author.into(),
                sub: r.sub.into(),
                frac: r.frac,
                cover: r.cover.map(slint::Image::from_rgba8).unwrap_or_default(),
                id: r.id as i32,
                path: r.path.into(),
            })
            .collect();
        w.set_home_continue_rows(slint::ModelRc::new(slint::VecModel::from(items)));
    });
}

/// Gather in-progress books / podcast episodes / audiobooks (≤4 each, newest
/// first) with covers, cache them, then push through the active filter.
fn kick_home_continue(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        // The four sources live in four separate DBs — gather them
        // concurrently (join!) so the strip fills in ~max latency, not sum.
        // Books — reading_progress rows, cover from book_meta.
        let books_f = async {
            let mut rows: Vec<HomeContRow> = Vec::new();
            let Ok(pool) = pool_for("books").await else { return rows; };
            let items = sqlx::query_as::<_, (i64, String, String, String, i64, i64, f64, String, i64)>(
                "SELECT b.id, b.title, b.author, b.path, p.page, p.total_pages, p.percent, \
                        b.cover_path, COALESCE(p.updated_at, 0) \
                 FROM progress p \
                 JOIN books b ON b.id = p.book_id \
                 WHERE b.finished = 0 AND b.missing = 0 \
                       AND (p.page > 0 OR p.char_offset > 0 OR p.percent > 0) \
                 ORDER BY p.updated_at DESC LIMIT 4")
                .fetch_all(&pool).await.unwrap_or_default();
            for (id, title, author, path, page, total, percent, cover, ts) in items {
                let name = if title.is_empty() {
                    std::path::Path::new(&path).file_stem()
                        .map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone())
                } else { title };
                let frac = if total > 0 {
                    (page as f32 / total as f32).clamp(0.0, 1.0)
                } else if percent > 0.0 {
                    (percent as f32 / 100.0).clamp(0.0, 1.0)
                } else {
                    -1.0
                };
                let sub = if total > 0 {
                    format!("Page {page}/{total}")
                } else if percent > 0.0 {
                    format!("{}%", percent.round() as i64)
                } else {
                    format!("page {page}")
                };
                let cover = tulipix_sec_music::decode_art_px(
                    Some(cover).filter(|c| !c.is_empty()).map(PathBuf::from)).await;
                rows.push(HomeContRow { kind: "book", title: name, author, sub, frac, cover, id, path, ts });
            }
            rows
        };
        // Podcasts — in-progress episodes (position saved by the 5s playback
        // ticker; `played` is set on OPEN so it can't gate this list, and ≥95%
        // through counts as finished). Episode art falls back to show art.
        let podcasts_f = async {
            let mut rows: Vec<HomeContRow> = Vec::new();
            let Ok(pool) = pool_for("podcasts").await else { return rows; };
            let items = sqlx::query_as::<_, (i64, String, String, f64, Option<f64>, String, String, i64)>(
                "SELECT e.id, COALESCE(e.title, ''), COALESCE(p.title, ''), e.position_s, e.duration_s, \
                        COALESCE(e.image_url, ''), COALESCE(NULLIF(p.custom_image, ''), p.image_url, ''), \
                        COALESCE(e.published, 0) \
                 FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id \
                 WHERE e.position_s > 5 \
                   AND (e.duration_s IS NULL OR e.duration_s <= 0 OR e.position_s < e.duration_s * 0.95) \
                 ORDER BY COALESCE(e.published, 0) DESC LIMIT 4")
                .fetch_all(&pool).await.unwrap_or_default();
            let client = tulipix_core::net::http().clone();
            for (id, title, show, pos, dur, ep_img, show_img, ts) in items {
                if title.is_empty() { continue; }
                let (frac, sub) = match dur {
                    Some(d) if d > 1.0 => (
                        ((pos / d) as f32).clamp(0.0, 1.0),
                        format!("{}m left / {}", ((((d - pos) / 60.0).ceil()) as i64).max(1),
                            tulipix_sec_music::fmt_hm(d)),
                    ),
                    _ => (-1.0, format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60)),
                };
                let src = if !ep_img.is_empty() { ep_img } else { show_img };
                let cover_path = tulipix_sec_music::resolve_artwork(
                    &client, &format!("home_ep_{id}"), &src).await;
                let cover = tulipix_sec_music::decode_art_px(cover_path).await;
                rows.push(HomeContRow { kind: "podcast", title, author: show, sub, frac, cover, id, path: String::new(), ts });
            }
            rows
        };
        // Audiobooks — newest chapter resume per book folder (dedup on folder).
        let audiobooks_f = async {
            let mut rows: Vec<HomeContRow> = Vec::new();
            let Ok(pool) = pool_for("music").await else { return rows; };
            let _ = sqlx::query("CREATE TABLE IF NOT EXISTS audiobook_covers (folder TEXT PRIMARY KEY, path TEXT NOT NULL)")
                .execute(&pool).await;
            let custom: std::collections::HashMap<String, String> =
                sqlx::query_as("SELECT folder, path FROM audiobook_covers")
                    .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
            // Resolved book metadata (5-method lookup chain) — the card title
            // was the raw FOLDER name and the author line was missing.
            let meta: std::collections::HashMap<String, (String, String)> =
                sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
                    "SELECT folder, title, author FROM audiobook_meta")
                    .fetch_all(&pool).await.unwrap_or_default().into_iter()
                    .map(|(f, t, a)| (f, (t.unwrap_or_default(), a.unwrap_or_default())))
                    .collect();
            let items = sqlx::query_as::<_, (String, i64, f64, i64, String)>(
                "SELECT tm.folder, ap.item_id, ap.position_s, COALESCE(ap.updated, 0), COALESCE(i.abs_path, '') \
                 FROM audiobook_progress ap \
                 JOIN track_meta tm ON tm.item_id = ap.item_id \
                 LEFT JOIN items i ON i.id = ap.item_id \
                 WHERE ap.finished = 0 AND ap.position_s > 0 AND tm.folder IS NOT NULL \
                 ORDER BY ap.updated DESC LIMIT 12")
                .fetch_all(&pool).await.unwrap_or_default();
            let mut seen = std::collections::HashSet::new();
            for (folder, item_id, pos, ts, chapter) in items {
                if !seen.insert(folder.clone()) { continue; }
                if seen.len() > 4 { break; }
                let (mt, ma) = meta.get(&folder).cloned().unwrap_or_default();
                let title = if mt.is_empty() { tulipix_sec_music::book_title(&folder) } else { mt };
                let author = if ma.is_empty() { "Audiobook".to_string() } else { ma };
                // Chapter-level resume → show book progress as chapter i/n
                // (raw seconds counters are meaningless across chapters).
                let chapters = tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default();
                let idx = chapters.iter().position(|c| *c == item_id);
                let (frac, sub) = match idx {
                    Some(i) if !chapters.is_empty() => (
                        ((i + 1) as f32 / chapters.len() as f32).clamp(0.0, 1.0),
                        format!("Chapter {}/{}", i + 1, chapters.len()),
                    ),
                    _ => (-1.0, format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60)),
                };
                let cover = tulipix_sec_music::audiobook_cover_px(
                    &folder,
                    custom.get(&folder).map(PathBuf::from),
                    (!chapter.is_empty()).then(|| PathBuf::from(&chapter)),
                ).await;
                rows.push(HomeContRow {
                    kind: "audiobook", title, author, sub, frac, cover,
                    id: item_id, path: folder, ts,
                });
            }
            rows
        };
        // Videos — in-progress watches (same filter as the Videos section's
        // Continue tab), newest watch first. Cover = TMDB poster else thumb.
        let videos_f = async {
            let mut rows: Vec<HomeContRow> = Vec::new();
            let Ok(pool) = pool_for("videos").await else { return rows; };
            let items = sqlx::query_as::<_, (i64, String, f64, Option<f64>, Option<String>, i64)>(
                "SELECT vm.item_id, i.abs_path, COALESCE(wp.position_s, 0.0), \
                        COALESCE(wp.duration_s, vm.duration_s), mv.poster_local, \
                        COALESCE(vm.last_accessed, 0) \
                 FROM video_meta vm \
                 JOIN items i ON i.id = vm.item_id \
                 LEFT JOIN watch_progress wp ON wp.item_id = vm.item_id \
                 LEFT JOIN movies mv ON mv.item_id = vm.item_id \
                 WHERE vm.deleted_at IS NULL AND vm.archived = 0 \
                   AND vm.last_accessed IS NOT NULL \
                   AND COALESCE(wp.finished, 0) = 0 AND COALESCE(wp.position_s, 0) > 0 \
                 ORDER BY vm.last_accessed DESC LIMIT 4")
                .fetch_all(&pool).await.unwrap_or_default();
            for (id, path, pos, dur, poster, ts) in items {
                let title = std::path::Path::new(&path).file_stem()
                    .map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                let (frac, sub) = match dur {
                    Some(d) if d > 1.0 => (
                        ((pos / d) as f32).clamp(0.0, 1.0),
                        format!("{}m left / {}", ((((d - pos) / 60.0).ceil()) as i64).max(1),
                            tulipix_sec_music::fmt_hm(d)),
                    ),
                    _ => (-1.0, format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60)),
                };
                let cover_path = match poster.filter(|p| !p.is_empty()) {
                    Some(p) => Some(PathBuf::from(p)),
                    None => thumb_for(PathBuf::from(&path), tulipix_core::thumbs::ThumbKind::Video).await.ok(),
                };
                let cover = tulipix_sec_music::decode_art_px(cover_path).await;
                rows.push(HomeContRow { kind: "video", title, author: "Video".into(), sub, frac, cover, id, path, ts });
            }
            rows
        };
        let (b, p, a, v) = tokio::join!(books_f, podcasts_f, audiobooks_f, videos_f);
        let mut rows: Vec<HomeContRow> = Vec::new();
        rows.extend(b); rows.extend(p); rows.extend(a); rows.extend(v);
        // Drop user-dismissed items, then "All" interleaves by recency.
        let dismissed = home_cont_dismissed().lock().map(|g| g.clone()).unwrap_or_default();
        rows.retain(|r| !dismissed.contains(&home_cont_dismiss_key(r.kind, r.id, &r.path)));
        rows.sort_by(|a, b| b.ts.cmp(&a.ts));
        if let Ok(mut g) = home_cont_rows().lock() { *g = rows; }
        push_home_continue(&weak);
    });
}

/// Seed every data-driven Settings panel from the persisted settings + live
/// runtime diagnostics. Cheap; re-run after any toggle/action.
fn seed_settings_panels(w: &MainWindow) {
    use slint::{ModelRc, VecModel};
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();

    // AI Features — np.p1.llm.*, np.p1.ai.update-*, np.p1.onboarding.ai-models.
    // Manifest-driven: ONE row per model — status dot + Download/Verify button
    // together (they used to be two separate rows).
    let mut ai = vec![hdr("ON-DEVICE MODELS")];
    for m in &ai_manifest().models {
        let installed = tulipix_photos::ai::models::is_installed(m);
        let mb = m.size_bytes / 1_000_000;
        let (nice, purpose) = model_display(&m.name, &m.cap);
        let dl = ai_dl_progress().lock().ok().and_then(|g| g.get(m.name.as_str()).copied());
        let mut row = statact(&format!("ai-dl-{}", m.name),
            &format!("{nice} · {mb} MB"), purpose,
            if dl.is_some() { "Downloading" } else if installed { "Installed" } else { "Not downloaded" },
            if dl.is_some() { "busy" } else if installed { "ok" } else { "muted" },
            if installed { "Verify" } else { "Download" });
        if let Some(f) = dl { row.frac = f; }
        ai.push(row);
    }
    {
        let mut chk = act("ai-update-check", "Check for model updates", "Compare installed models against the latest versions", "Check");
        if AI_CHECK_BUSY.load(std::sync::atomic::Ordering::Relaxed) { chk.state = "busy".into(); }
        ai.push(chk);
    }
    ai.extend([
        hdr("VOICE RECOGNITION MODEL"),
        si("ai.voice-lang", "lang-choice", "Spoken language",
            "What the mic listens for — pinning a language beats auto-detect on short clips",
            &{ let v = s.text("ai.voice-lang"); if v.is_empty() { "en".into() } else { v } }, false, ""),
        model_choice(&s, "ai.model.voice", "Voice search",
            "Used by the mic button in search fields — Tiny answers fastest"),
        model_choice(&s, "ai.model.transcribe", "Transcribe & subtitles",
            "Used by the Tools transcriber and video subtitles — bigger models catch more words"),
        stat("Model sizes", "Tiny bundled · Base 60 MB · Small 190 MB · Turbo 574 MB — download above", "muted"),
    ]);
    ai.extend([
        tog(&s, "ai.captions", false, "Describe photos automatically", "Writes captions and alt-text for new photos as they are added"),
        tog(&s, "ai.voice", true, "Voice search", "The mic button in search bars — speak instead of typing"),
        tog(&s, "ai.chat", false, "Chat assistant (Ctrl+J)", "Ask questions about your library in plain language"),
        hdr("BOOK READ-ALOUD"),
        tog(&s, "books.tts.neural", true, "Use neural voice (Kokoro)",
            "Natural AI voice for the reader's Read Aloud. Off = robotic espeak voice (no model, needs espeak-ng). Neural needs the kokoro-82m model above + a books-tts build."),
        hdr("CLOUD"),
        tog(&s, "ai.cloud-offload", false, "Allow cloud AI help", "Send selected questions to a cloud AI service. Off = everything stays on-device"),
    ]);
    w.set_ai_rows(ModelRc::new(VecModel::from(ai)));
    w.set_ai_req_rows(ModelRc::new(VecModel::from(ai_requirement_rows())));

    // Servers & sources — np.p1.api.*. Plain-language copy: optional keys
    // first, provider toggles second, self-host overrides last.
    let ep = vec![
        hdr("OPTIONAL SERVICE KEYS"),
        txt(&s, "api.spotify-id", "Spotify client ID", "Better music search and recommendations. Free key from developer.spotify.com"),
        txt(&s, "api.spotify-secret", "Spotify client secret", "Goes together with the client ID above"),
        txt(&s, "api.youtube-data", "YouTube API key", "Richer YouTube search results and video details"),
        txt(&s, "api.piped-instance", "YouTube backend server (Piped)", "The server used to browse YouTube. Leave blank for the default"),
        hdr("EXTRA METADATA SOURCES"),
        tog(&s, "api.discogs", false, "Discogs", "Extra music metadata when MusicBrainz has no match"),
        tog(&s, "api.anidb", false, "AniDB", "Anime titles, episodes, and ratings"),
        tog(&s, "api.anilist", false, "AniList", "A second anime source when AniDB misses"),
        tog(&s, "api.subscene", false, "Subscene / Addic7ed", "Backup subtitle sources when OpenSubtitles is busy"),
        tog(&s, "api.trakt", false, "Trakt.tv", "Track what you watch with a Trakt account"),
        tog(&s, "api.listenbrainz", false, "ListenBrainz", "Scrobble played music — the open Last.fm alternative"),
        hdr("SELF-HOSTED SERVERS (ADVANCED)"),
        txt(&s, "api.update-channel", "Update server", "Only needed for mirrors or offline networks"),
        txt(&s, "api.sentry", "Crash-report server", "Send crash reports to your own Sentry server"),
        txt(&s, "api.nominatim", "Place-name server", "Turns photo GPS coordinates into place names"),
        txt(&s, "api.radio-browser", "Radio station server", "Mirror for the internet-radio directory"),
        txt(&s, "api.autoeq", "Headphone EQ database", "Mirror for AutoEq headphone profiles"),
        txt(&s, "api.tmdb-image-base", "Poster artwork server", "Mirror for movie and show artwork"),
    ];
    w.set_endpoint_rows(ModelRc::new(VecModel::from(ep)));

    // Security — np.p1.idle-autolock, sec.passkey, db-encrypt, sec.sandbox.
    let idle_secs_val = if s.idle_lock_secs > 0 { s.idle_lock_secs.to_string() } else { String::new() };
    let sec = vec![
        hdr("LOCK"),
        tog(&s, "autolock", false, "Auto-lock when idle", "Lock the app and show the screensaver after a period of no activity"),
        si("idle_lock_secs", "text", "Idle timeout (seconds)", "How long before auto-lock kicks in — blank means 600 (ten minutes). Idle is only counted while the window is on screen; minimised does not lock", &idle_secs_val, false, ""),
        txt(&s, "lock.wallpapers", "Lock screen wallpapers", "Folder of pictures for the lock screen slideshow — the first ten are used, one every twelve seconds. Blank = the gradient"),
        act("lock-wallpapers-browse", "Choose wallpaper folder", "Pick the folder holding the pictures you want behind the lock screen", "Browse…"),
        act("lock-wallpapers-clear", "Clear wallpapers", "Back to the plain gradient", "Clear"),
        hdr("UNLOCK"),
        tog(&s, "passkey", false, "Unlock with a passkey", "Use a security key or fingerprint instead of a password"),
        hdr("ENCRYPTION"),
        tog(&s, "db-encrypt", false, "Encrypt the library database", "Protects your library index if the disk is stolen — applies on next launch"),
        stat("OS sandbox", "Not applicable on Linux", "muted"),
    ];
    w.set_security_rows(ModelRc::new(VecModel::from(sec)));

    // Data & tools — backup/restore, export, migration, bug-report, multi-user.
    let data = vec![
        hdr("BACKUP"),
        act("backup", "Back up settings", "Saves your settings and folder list so you can restore them later", "Back up"),
        act("export", "Export library", "Writes your whole library (lists, edits, tags) to portable JSON files", "Export"),
        hdr("IMPORT"),
        act("migration", "Import from another app", "Bring over a library from Plex or Picasa", "Import"),
        hdr("PROBLEMS"),
        act("bug-report", "Report a problem", "Opens the log folder with system info ready to attach", "Open"),
        tog(&s, "multi-user", false, "Separate library per computer user", "Each OS account gets its own Tulipix library"),
    ];
    w.set_data_rows(ModelRc::new(VecModel::from(data)));

    // Playback — its own Settings section (was buried inside System).
    let pb = vec![
        hdr("SUBTITLES"),
        txt(&s, "playback.sub-size", "Subtitle size", "In pixels — 28 if left blank"),
        txt(&s, "playback.sub-color", "Subtitle colour", "A colour code like #ffffff — applies on the next play"),
        stat("Subtitles next to the video", "Loaded automatically (.srt / .vtt / .ass)", "ok"),
        hdr("VIDEO"),
        tog(&s, "playback.interpolation", false, "Smoother motion", "Frame interpolation — can be heavy on laptop graphics"),
        tog(&s, "playback.upscale", false, "Upscale shaders (Anime4K)", "Sharper upscaling — drop .glsl shader files in the folder below"),
        stat("Upscale shader folder", &{
                 let dir = crate::dirs_default().map(|d| d.join("shaders").display().to_string())
                     .unwrap_or_else(|| "<config>/shaders".into());
                 match anime4k_shader_args() {
                     Some((_, n)) => format!("{n} shader(s) in {dir}"),
                     None => format!("Empty — put Anime4K .glsl files in {dir}"),
                 }
             },
             if anime4k_shader_args().is_some() { "ok" } else { "muted" }),
        stat("Keep display awake", "While a video plays", "ok"),
        hdr("AUDIO & MUSIC"),
        tog(&s, "playback.audio-exclusive", false, "Exclusive audio output", "Bit-perfect output straight to the audio device — silences other apps"),
        txt(&s, "music.eq-preset", "Music equalizer preset", "flat · rock · pop · jazz · bass · treble — applies on the next track"),
    ];
    w.set_playback_rows(ModelRc::new(VecModel::from(pb)));

    // Advanced — live diagnostics + bundled tools + platform status.
    let cache_mb = tulipix_core::thumbs::cache_size().map(|b| b / (1024 * 1024)).unwrap_or(0);
    // ggml-tiny status — resolve it the same way the app actually loads the model
    // (user tools-dir, then the bundled per-OS copy walked from current_exe). The
    // old `bundled_present` check hard-coded a linux-x86_64 dev path, so it always
    // read "Missing" on installed Windows/macOS builds even with the model present.
    let ggml_tiny = "ggml-tiny-1.0.bin";
    let ggml_in_tools = tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("tools.bin-dir")).filter(|d| !d.trim().is_empty())
        .map(|d| std::path::Path::new(d.trim()).join(ggml_tiny).exists())
        .unwrap_or(false);
    let ggml_bundled = tulipix_core::thumbs::bundled_file(ggml_tiny).is_some();
    let (ggml_label, ggml_state) = if ggml_in_tools { ("Tools directory", "ok") }
        else if ggml_bundled { ("Bundled (75 MB)", "ok") } else { ("Missing", "warn") };
    let mut sys = vec![
        hdr("PERFORMANCE (LIVE)"),
        stat("Startup to ready", &format!("{} ms", startup_ms()), if startup_ms() < 500 { "ok" } else { "warn" }),
        stat("Allocator", allocator_name(), "ok"),
        stat("Resident memory", &format!("{} MB", rss_mb()), if rss_mb() < 300 { "ok" } else { "warn" }),
        stat("Thumbnail cache", &format!("{} MB", cache_mb), "muted"),
        tog(&s, "power-aware", true, "Battery / network aware", "Pause background scanning on battery or metered connections"),
        hdr("BUNDLED TOOLS"),
        // Universal override — look for mpv / yt-dlp / ffmpeg / ffprobe / etc. in
        // this folder first (before the bundled copy and PATH). One setting for
        // every external tool; handy on Windows where they aren't on PATH.
        txt(&s, "tools.bin-dir", "Tools directory", "Folder holding mpv, yt-dlp, ffmpeg… — checked before bundled + PATH (blank = off)"),
        act("tools-dir-browse", "Choose tools directory", "Pick the folder containing your external tool binaries", "Browse…"),
        act("tools-dir-reset", "Reset tools directory", "Clear the custom folder and fall back to the app's bundled binaries + system PATH", "Reset"),
        tool_row("ffmpeg"), tool_row("ffprobe"), tool_row("rclone"),
        tool_row("yt-dlp"),
        tog(&s, tulipix_core::ytdlp::AUTO_UPDATE_FLAG, true, "Keep yt-dlp up to date",
            "Checks weekly. Sites change constantly and a yt-dlp a few weeks old starts failing downloads with 403"),
        txt(&s, "ytdlp.player-clients", "yt-dlp player clients",
            "Advanced, blank = yt-dlp's own defaults. Only set this if a yt-dlp issue tells you to, e.g. default,tv,android"),
        tool_row("whisper-cli"), tool_row("mpv"),
        stat("ggml-tiny model", ggml_label, ggml_state),
        tool_row("exiftool"),
        hdr("PLATFORM INTEGRATIONS"),
        tog(&s, "notifications", true, "Actionable notifications", "Snooze / mark-played / open-version actions"),
        tog(&s, "mcp-server", false, "MCP server", "Expose the library to external AI agents"),
        tog(&s, "crash-upload", false, "Opt-in crash uploader", "Send minidumps to the configured Sentry DSN"),
        tog(&s, "follow-system-accent", false, "Follow system accent", "Material You / OS accent colour"),
        tog(&s, "follow-os-font-scale", false, "Honour OS font scale", "Dynamic Type / accessibility text size"),
        // Live platform posture (np.p1.window-chrome / .share-sheet /
        // .appintents / .widgets / .live-activities / .sec.sandbox / .tray) —
        // real probes, not hardcoded strings.
        stat("Window chrome", window_chrome_label(), "ok"),
        stat("Share sheet", if cfg!(target_os = "linux") { "Not available on Linux (xdg-portal share is file-manager only)" } else { "Native" }, "muted"),
        stat("AppIntents / Shortcuts", if cfg!(target_os = "macos") { "Available" } else { "Not available on this OS" }, "muted"),
        stat("Desktop widgets", if cfg!(target_os = "linux") { "Not available on Linux" } else { "Available" }, "muted"),
        stat("Live Activities", if cfg!(target_os = "macos") { "Available" } else { "Not available on this OS" }, "muted"),
        stat("OS sandbox", if cfg!(target_os = "linux") { "Flatpak portals when packaged; unsandboxed dev build" } else { "Platform sandbox" }, "muted"),
        stat("System tray",
             if TRAY_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) { "Active" } else { "Init failed / feature off" },
             if TRAY_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) { "ok" } else { "warn" }),
        // Live frame telemetry (np.p1.perf.jank) — slow-frame counter from the
        // render-loop watchdog; dev builds log call-trees, release counts only.
        stat("Frames > 16 ms (session)", &SLOW_FRAMES.load(std::sync::atomic::Ordering::Relaxed).to_string(),
             if SLOW_FRAMES.load(std::sync::atomic::Ordering::Relaxed) < 60 { "ok" } else { "warn" }),
        hdr("BUILD & LOCALE"),
        // Per-crate lazy-load state (np.p1.perf.lazy-crate): compiled-in or
        // excluded from this binary, per feature flag.
        // The lazy-whisper / lazy-ai-ep / lazy-plugins rows are gone with their
        // features: they claimed the crates "load on first use", but nothing in
        // the workspace ever called them. Speech-to-text is the bundled
        // whisper-cli binary, reported under BUNDLED TOOLS above.
        stat("ONNX editor ops (ai-onnx)", if cfg!(feature = "ai-onnx") { "Compiled" } else { "Not in this binary" },
             if cfg!(feature = "ai-onnx") { "ok" } else { "muted" }),
        stat("Adaptive layout", "Desktop breakpoints", "ok"),
        stat("Player embedding", "Out-of-process mpv — isolation by design (embedded GL parked: froze Intel iGPUs)", "ok"),
        // Format-support audit (np.p4.music.formats) — the claim table.
        stat("Audio formats", &{
                 let all: Vec<&str> = tulipix_music::formats::FORMATS.iter().map(|f| f.ext).collect();
                 let gaps = tulipix_music::formats::gapless_gaps();
                 format!("{} — gapless unverified: {}", all.join(" · "), gaps.join(", "))
             }, "ok"),
        // The PLUGINS section is gone too. tulipix-plugins defines an ABI and no
        // host ever loads it, its wasmtime and mlua runtimes are behind features
        // nothing enables, and the row invited a rebuild with a flag that only
        // added unreachable code. Reinstate it when a plugin can actually run.
        act("open-logs", "Open log folder", "tracing JSON logs with daily rotation", "Open"),
    ];
    sys.shrink_to_fit();
    w.set_system_rows(ModelRc::new(VecModel::from(sys)));
}

/// Voice-session generation counter — bumping it discards in-flight results
/// (stop button, or a new session superseding an old one).
static VOICE_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// "Check for model updates" in flight — renders the button as a progress pill.
static AI_CHECK_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Live model-download progress (model name → 0..1) — drives the determinate
/// progress pill in the AI Features rows.
fn ai_dl_progress() -> &'static std::sync::Mutex<std::collections::HashMap<String, f32>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f32>>> = std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Resolve a CLI tool for voice capture: user tools dir / bundled first, PATH second.
fn voice_tool(name: &str) -> Option<std::path::PathBuf> {
    let p = tulipix_core::thumbs::tool_bin(name);
    if p.is_absolute() && p.exists() { return Some(p); }
    if on_path(name) { return Some(std::path::PathBuf::from(name)); }
    None
}

/// Record ~5 s of 16 kHz mono audio from the default microphone. Returns the
/// temp wav path.
///
/// Linux: the bundled static ffmpeg has NO pulse/alsa input devices compiled
/// in, so try the native recorders every desktop ships instead — arecord
/// (self-terminating, clean header), then parecord / pw-record (stopped with
/// SIGINT via coreutils `timeout` so they finalise the wav), then a system
/// ffmpeg if one exists.
async fn voice_record() -> anyhow::Result<std::path::PathBuf> {
    let wav = std::env::temp_dir().join(format!("tulipix-voice-{}.wav", std::process::id()));
    let _ = tokio::fs::remove_file(&wav).await;

    #[cfg(target_os = "linux")]
    {
        let w = wav.to_string_lossy().into_owned();
        let attempts: &[&[&str]] = &[
            &["arecord", "-q", "-d", "5", "-f", "S16_LE", "-r", "16000", "-c", "1", &w],
            &["timeout", "-s", "INT", "5", "parecord", "--rate=16000", "--channels=1",
              "--format=s16le", "--file-format=wav", &w],
            &["timeout", "-s", "INT", "5", "pw-record", "--rate", "16000", "--channels", "1", &w],
            &["ffmpeg", "-hide_banner", "-f", "pulse", "-i", "default",
              "-t", "5", "-ac", "1", "-ar", "16000", "-y", &w],
        ];
        for cmd in attempts {
            if !on_path(cmd[0]) { continue; }
            let _ = tokio::fs::remove_file(&wav).await;
            let st = tokio::process::Command::new(cmd[0]).args(&cmd[1..])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status().await;
            // `timeout -s INT` exits 124 when the window elapses — that IS the
            // success path for the run-forever recorders.
            let ran = matches!(st, Ok(s) if s.success() || s.code() == Some(124));
            let got = tokio::fs::metadata(&wav).await.map(|m| m.len() > 44).unwrap_or(false);
            if ran && got { return Ok(wav); }
        }
        anyhow::bail!("microphone capture failed — tried arecord, parecord, pw-record, ffmpeg. \
                       Is a microphone connected?");
    }

    #[cfg(not(target_os = "linux"))]
    {
        // Windows (gyan.dev) and macOS (martin-riedl) ffmpeg builds ship with
        // dshow / avfoundation input devices — capture directly.
        let ffmpeg = voice_tool("ffmpeg")
            .ok_or_else(|| anyhow::anyhow!("ffmpeg not found (bundled copy missing?)"))?;
        let mut c = tokio::process::Command::new(&ffmpeg);
        #[cfg(target_os = "macos")]
        c.args(["-f", "avfoundation", "-i", ":0"]);
        #[cfg(target_os = "windows")]
        {
            let dev = windows_default_mic(&ffmpeg).await
                .ok_or_else(|| anyhow::anyhow!("no microphone found"))?;
            c.args(["-f", "dshow", "-i", &dev]);
        }
        c.args(["-t", "5", "-ac", "1", "-ar", "16000", "-y"]).arg(&wav);
        c.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let st = c.status().await?;
        if !st.success() || !wav.exists() {
            anyhow::bail!("microphone capture failed — is a mic connected and allowed?");
        }
        Ok(wav)
    }
}

/// First dshow audio-capture device name (Windows) via ffmpeg enumeration.
#[cfg(target_os = "windows")]
async fn windows_default_mic(ffmpeg: &std::path::Path) -> Option<String> {
    let out = tokio::process::Command::new(ffmpeg)
        .args(["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"])
        .output().await.ok()?;
    let s = String::from_utf8_lossy(&out.stderr);
    for line in s.lines() {
        if line.contains("(audio)") {
            let a = line.find('"')?;
            let rest = &line[a + 1..];
            let b = rest.find('"')?;
            return Some(format!("audio={}", &rest[..b]));
        }
    }
    None
}

/// Transcribe a wav with whisper-cli using the "voice" task's chosen model.
async fn voice_transcribe(wav: &std::path::Path) -> anyhow::Result<String> {
    let model = tulipix_core::ai_models::whisper_model_for("voice")
        .ok_or_else(|| anyhow::anyhow!("no whisper model — check Settings → AI Features"))?;
    let whisper = voice_tool("whisper-cli")
        .ok_or_else(|| anyhow::anyhow!("whisper-cli not found (bundled copy missing?)"))?;
    // Language: constrained to what the user actually speaks (Settings → AI
    // Features). Whisper's auto-detect on short clips loves to guess wrong
    // exotic languages; pinning the language fixes most mis-hearings.
    let lang = tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("ai.voice-lang")).filter(|l| !l.is_empty())
        .unwrap_or_else(|| "en".into());
    let out = tokio::process::Command::new(&whisper)
        .arg("-m").arg(&model)
        .arg("-f").arg(wav)
        .args(["-nt", "-l", &lang])
        .output().await?;
    let _ = tokio::fs::remove_file(wav).await;
    if !out.status.success() {
        anyhow::bail!("transcription failed ({})", out.status);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" "))
}

/// Put transcribed text into `target`'s search box and fire its search.
fn voice_route(w: &MainWindow, target: &str, text: &str) {
    let t: slint::SharedString = text.into();
    match target {
        "photos"   => { w.set_photos_query(t.clone()); w.invoke_photo_search(t); }
        "videos"   => { w.set_video_query(t.clone());  w.invoke_video_search(t); }
        // "books" — section torn out pending redesign (newbook22/newreader);
        // voice search routing will be re-wired with the new Books UI.
        "cloud"    => { w.set_cloud_query(t.clone());  w.invoke_cloud_search(t); }
        "tools"    => { w.set_tools_query(t.clone());  w.invoke_tools_search(t); }
        "music"    => { w.set_music_query(t.clone());  w.invoke_music_search(t); }
        "podcasts" => { w.set_music_query(t.clone());  w.invoke_music_podcast_search(t); }
        "radio"    => { w.set_music_query(t.clone());  w.invoke_music_radio_search(t); }
        "youtube"  => { w.set_music_query(t.clone());  w.invoke_music_yt_search(t); }
        _ => tracing::warn!(%target, "voice route: unknown target"),
    }
}

/// The lock screen's wallpapers: the first ten pictures in the folder named by
/// `lock.wallpapers`, in name order. Empty when the setting is blank or the
/// folder holds nothing we can decode — the overlay falls back to its gradient.
fn lock_wallpapers() -> Vec<std::path::PathBuf> {
    let Some(dir) = tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("lock.wallpapers"))
        .filter(|d| !d.trim().is_empty())
    else { return Vec::new(); };
    let Ok(rd) = std::fs::read_dir(dir.trim()) else { return Vec::new(); };
    let mut out: Vec<std::path::PathBuf> = rd
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension().and_then(|e| e.to_str())
                .map(|e| matches!(e.to_ascii_lowercase().as_str(),
                                  "jpg" | "jpeg" | "png" | "webp" | "bmp" | "tiff" | "gif"))
                .unwrap_or(false)
        })
        .collect();
    out.sort();
    out.truncate(10);
    out
}

/// Drive the lock screen's wallpaper slideshow.
///
/// One repeating timer, alive for the session, that does nothing at all unless
/// the overlay is up. Each turn it loads the next picture on a worker thread —
/// decode plus a Gaussian, neither of which belongs on the event loop — and
/// drops the result into whichever of the overlay's two image slots is
/// currently hidden, then flips `ambient-photo-fade`. The .slint side animates
/// that flip, so the pictures ping-pong between the slots and there is never a
/// snap back through zero.
///
/// The blur is applied here because Slint has no blur at draw time; softening
/// the picture is what keeps the clock and the now-playing card readable over
/// a busy photograph.
fn wire_lock_slideshow(window: &MainWindow) {
    use std::cell::Cell;
    thread_local! {
        static TIMER: std::cell::RefCell<Option<slint::Timer>> =
            const { std::cell::RefCell::new(None) };
        /// Which wallpaper comes next, and whether a load is already in flight
        /// (a slow disk must not stack up four decodes).
        static NEXT: Cell<usize> = const { Cell::new(0) };
        static BUSY: Cell<bool> = const { Cell::new(false) };
        /// Seconds the overlay has been up. Ticking once a second rather than
        /// once every twelve is what makes the FIRST picture land immediately
        /// on lock instead of after a twelve-second stare at the gradient.
        static TICK: Cell<u32> = const { Cell::new(0) };
    }
    let weak = window.as_weak();
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(1), move || {
        let Some(w) = weak.upgrade() else { return };
        if !w.get_ambient_active() {
            TICK.with(|c| c.set(0));   // re-arm: the next lock opens on a picture
            return;
        }
        let t = TICK.with(|c| { let t = c.get(); c.set(t + 1); t });
        if t % 12 != 0 || BUSY.with(|c| c.get()) { return; }
        let shots = lock_wallpapers();
        if shots.is_empty() { return; }
        let i = NEXT.with(|c| { let i = c.get() % shots.len(); c.set(i + 1); i });
        let path = shots[i].clone();
        BUSY.with(|c| c.set(true));
        let wk = w.as_weak();
        std::thread::spawn(move || {
            // 1600px is plenty for a full-screen backdrop and keeps the blur
            // (which is O(pixels)) off the far side of a second.
            let px = image::open(&path).ok().map(|img| {
                let img = img.thumbnail(1600, 1600).to_rgba8();
                let blurred = image::imageops::blur(&img, 6.0);
                let (bw, bh) = (blurred.width(), blurred.height());
                slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                    blurred.as_raw(), bw, bh)
            });
            let _ = wk.upgrade_in_event_loop(move |w| {
                BUSY.with(|c| c.set(false));
                let Some(px) = px else { return };
                let img = slint::Image::from_rgba8(px);
                // Fill the slot that is faded OUT, then cross to it.
                if w.get_ambient_photo_fade() < 0.5 {
                    w.set_ambient_photo_next(img);
                    w.set_ambient_photo_fade(1.0);
                } else {
                    w.set_ambient_photo(img);
                    w.set_ambient_photo_fade(0.0);
                }
            });
        });
    });
    TIMER.with(|c| *c.borrow_mut() = Some(timer));
}

/// Installed RAM in MB, or 0 where we cannot tell. Used only to colour the
/// requirement rows — a machine that cannot be measured gets a neutral row
/// rather than a guess.
fn total_ram_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo").ok().and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| kb / 1024)
        }).unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    { 0 }
}

/// The "Requirements" sheet on AI Features: one row per on-device model saying
/// what it wants from the machine.
///
/// Built off the SAME manifest as the download rows, so the two lists can never
/// disagree about which models exist. The RAM figure is a recommendation, not a
/// measurement: weights have to be resident and the runtime needs working
/// buffers on top, which in practice lands near three times the file — floored
/// at 512 MB so a tiny model does not read as free.
fn ai_requirement_rows() -> Vec<SettingItem> {
    let ram = total_ram_mb();
    let mut rows = vec![hdr("THIS COMPUTER")];
    rows.push(stat(
        "Installed memory",
        &if ram > 0 { format!("{:.1} GB", ram as f64 / 1024.0) } else { "Unknown".to_string() },
        if ram == 0 { "muted" } else if ram >= 8192 { "ok" } else { "warn" },
    ));
    rows.push(stat(
        "Graphics acceleration",
        if cfg!(feature = "ai-onnx") { "ONNX runtime compiled in — GPU used when available" }
        else { "CPU only in this build" },
        if cfg!(feature = "ai-onnx") { "ok" } else { "muted" },
    ));

    rows.push(hdr("ON-DEVICE MODELS"));
    for m in &ai_manifest().models {
        let mb = m.size_bytes / 1_000_000;
        let need = (mb * 3).max(512);
        let (nice, purpose) = model_display(&m.name, &m.cap);
        // What the row asks for, in the order it matters: memory, then disk,
        // then whether a GPU is required or merely welcome.
        let gpu = if m.min_vram_mb > 0 {
            format!(" · {} MB VRAM", m.min_vram_mb)
        } else {
            String::new()
        };
        let state = if ram == 0 { "muted" }
            else if ram >= need + 2048 { "ok" }
            else if ram >= need { "warn" }
            else { "error" };
        rows.push(si(
            "", "status", nice,
            if purpose.is_empty() { "Downloads on first use" } else { purpose },
            &format!("~{} MB RAM · {} MB disk{}", need, mb, gpu),
            false, state,
        ));
    }

    rows.push(hdr("VOICE RECOGNITION (WHISPER)"));
    // Not in the manifest — these ship or download through the whisper-cli
    // path, so their sizes are stated rather than read.
    for (name, mb, note) in [
        ("Tiny", 75u64, "Bundled. Fine for search phrases"),
        ("Base", 142, "The all-rounder"),
        ("Small", 466, "Catches names and accents"),
        ("Turbo", 1500, "Dictation-grade; slowest to load"),
    ] {
        let need = (mb * 2).max(512);
        let state = if ram == 0 { "muted" }
            else if ram >= need + 2048 { "ok" }
            else if ram >= need { "warn" }
            else { "error" };
        rows.push(si("", "status", &format!("Whisper {name}"), note,
                     &format!("~{need} MB RAM · {mb} MB disk"), false, state));
    }

    rows.push(hdr("NOTES"));
    rows.push(stat("Only what you use is loaded",
                   "Models load on demand and unload after", "muted"));
    rows.push(stat("Transcription is CPU-heavy",
                   "Expect roughly real-time on four cores", "muted"));
    rows
}

/// Human name + purpose line for a manifest model, keyed off its capability
/// gate so the Settings row says what the model DOES, not its filename.
fn model_display<'a>(name: &'a str, cap: &str) -> (&'a str, &'static str) {
    match cap {
        "photos.ai.faces" => ("Face detection", "Finds faces in photos so people can be grouped"),
        "photos.ai.heal" => ("Magic eraser", "Removes unwanted objects in the photo editor"),
        "photos.ai.sky" => ("Smart select", "Selects sky / objects for one-tap edits"),
        "voice.balanced" => ("Whisper Base (balanced)", "Good accuracy at near-instant speed — best all-rounder"),
        "voice.accurate" => ("Whisper Small (accurate)", "Catches names and accents — great for subtitles"),
        "voice.best" => ("Whisper Turbo (best)", "Top accuracy for dictation-grade transcription"),
        _ => (name, ""),
    }
}

/// Tray init result (np.p1.tray) — System row shows the real outcome.
static TRAY_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Slow-frame counter (np.p1.perf.jank) — frames over the 16 ms budget.
static SLOW_FRAMES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Actual window-chrome mode (np.p1.window-chrome), mirroring
/// `tulipix_platform::init_window_chrome`'s probe.
fn window_chrome_label() -> &'static str {
    #[cfg(target_os = "linux")]
    { if std::env::var_os("WAYLAND_DISPLAY").is_some() { "Wayland CSD" } else { "X11 SSD" } }
    #[cfg(target_os = "windows")]
    { "DWM Mica" }
    #[cfg(target_os = "macos")]
    { "NSVisualEffect vibrancy" }
}

fn allocator_name() -> &'static str {
    if cfg!(feature = "alloc-mimalloc") { "mimalloc" }
    else if cfg!(feature = "alloc-jemalloc") { "jemalloc" }
    else { "system malloc" }
}

/// Time to ready, frozen at the first turn of the event loop. Falls back to the
/// live elapsed time only while startup is still in progress.
fn startup_ms() -> u64 {
    READY_MS.get().copied().unwrap_or_else(|| {
        APP_START.get().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0)
    })
}

/// Resident set size in MB from /proc/self/statm (Linux). 0 elsewhere.
fn rss_mb() -> u64 {
    std::fs::read_to_string("/proc/self/statm").ok()
        .and_then(|s| s.split_whitespace().nth(1).and_then(|p| p.parse::<u64>().ok()))
        .map(|pages| pages * 4096 / (1024 * 1024))
        .unwrap_or(0)
}

// bundled_bin_dir / bundled_present / on_path moved to the tulipix-common crate.
/// Status row for a CLI tool: user tools-dir > bundled > system > missing.
/// Where one external tool is coming from, in the order the app resolves:
/// the override folder, then the app's own updated copy, then the bundled copy,
/// then PATH.
///
/// yt-dlp additionally prints its version, because that is the number anyone
/// looking at this row is there to check: a download failing with 403 is nearly
/// always a binary some weeks old, and "Bundled" never said that.
fn tool_row(name: &str) -> SettingItem {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let file = format!("{name}{ext}");
    let version = if name == "yt-dlp" { tulipix_core::ytdlp::installed_version() } else { None };
    let shown = |source: &str| -> String {
        match &version {
            Some(v) => format!("{source} · {v}"),
            None => source.to_string(),
        }
    };
    let in_tools_dir = tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("tools.bin-dir")).filter(|d| !d.trim().is_empty())
        .map(|d| std::path::Path::new(d.trim()).join(&file).exists())
        .unwrap_or(false);
    let updated = tulipix_core::ytdlp::managed_dir().is_some_and(|d| d.join(&file).exists());
    if in_tools_dir { stat(name, &shown("Tools directory"), "ok") }
    else if updated { stat(name, &shown("Updated"), "ok") }
    else if bundled_present(name) { stat(name, &shown("Bundled"), "ok") }
    else if on_path(name) || on_path(&file) { stat(name, &shown("System"), "warn") }
    else { stat(name, "Missing", "error") }
}

/// Minimal backup: copy settings.json + watched_folders.json into a
/// timestamped folder under the data dir. (Full DB/edit zip is future work.)
fn run_backup() -> std::io::Result<()> {
    let Some(data) = tulipix_core::paths::data_dir() else { return Ok(()); };
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    let dest = data.join("backups").join(ts.to_string());
    std::fs::create_dir_all(&dest)?;
    if let Some(cfg) = tulipix_core::paths::config_dir() {
        for f in ["settings.json", "watched_folders.json"] {
            let src = cfg.join(f);
            if src.exists() { let _ = std::fs::copy(&src, dest.join(f)); }
        }
    }
    let _ = tulipix_platform::fm::reveal_in_file_manager(&dest);
    Ok(())
}

/// Export an (empty-shell) portable JSON envelope to data_dir/exports.
fn run_export() -> anyhow::Result<()> {
    let Some(data) = tulipix_core::paths::data_dir() else { return Ok(()); };
    let dir = data.join("exports");
    std::fs::create_dir_all(&dir)?;
    let env = tulipix_core::export::empty();
    let bytes = tulipix_core::export::to_json(&env)?;
    let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0);
    let file = dir.join(format!("tulipix-export-{ts}.json"));
    std::fs::write(&file, bytes)?;
    let _ = tulipix_platform::fm::reveal_in_file_manager(&dir);
    Ok(())
}

// YouTube section extracted to tulipix_sec_music.

// pool_for + migrate_split_from_music moved to the tulipix-common crate.

fn section_for_ext(ext: &str) -> Option<&'static str> {
    use tulipix_core::thumbs::{kind_for, ThumbKind};
    // PDFs thumb as Doc but read in the Books section (np.p5.books.pdf).
    if ext.eq_ignore_ascii_case("pdf") {
        return Some("books");
    }
    match kind_for(ext) {
        ThumbKind::Photo => Some("photos"),
        ThumbKind::Video => Some("videos"),
        ThumbKind::Audio => Some("music"),
        ThumbKind::Book  => Some("books"),
        _ => None,
    }
}

fn classify_folder(root: &std::path::Path) -> std::collections::HashMap<&'static str, i64> {
    let mut counts: std::collections::HashMap<&'static str, i64> = std::collections::HashMap::new();
    for entry in walkdir::WalkDir::new(root).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) else { continue; };
        if let Some(section) = section_for_ext(ext) {
            *counts.entry(section).or_insert(0) += 1;
        }
    }
    tracing::info!(?counts, "folder classified");
    counts
}

#[allow(dead_code)] // section→enum helper kept alongside lib_section_enum's siblings
fn lib_section_enum(section: &str) -> LibrarySection {
    match section {
        "videos" => LibrarySection::Videos,
        "music"  => LibrarySection::Music,
        "books"  => LibrarySection::Books,
        _ => LibrarySection::Photos,
    }
}

fn thumb_kind_for_section(section: &str) -> tulipix_core::thumbs::ThumbKind {
    use tulipix_core::thumbs::ThumbKind;
    match section {
        "videos" => ThumbKind::Video,
        "music"  => ThumbKind::Audio,
        "books"  => ThumbKind::Book,
        _ => ThumbKind::Photo,
    }
}

/// Shared per-section progress counters — written by the scan task on every
/// file, read by a single ~80 ms flush task that posts ONE event-loop
/// callback per tick. Hammering `upgrade_in_event_loop` per file froze the
/// app on big folders.
#[derive(Default)]
struct SectionCounters {
    total: std::sync::atomic::AtomicI32,
    added: std::sync::atomic::AtomicI32,
    failed: std::sync::atomic::AtomicI32,
    active: std::sync::atomic::AtomicBool,
    last_error: std::sync::Mutex<String>,
}

static SCAN_STATE: OnceLock<std::sync::Mutex<std::collections::HashMap<&'static str, std::sync::Arc<SectionCounters>>>> =
    OnceLock::new();

fn scan_state() -> &'static std::sync::Mutex<std::collections::HashMap<&'static str, std::sync::Arc<SectionCounters>>> {
    SCAN_STATE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn counters_for(section: &'static str) -> std::sync::Arc<SectionCounters> {
    let mut g = scan_state().lock().unwrap();
    g.entry(section)
        .or_insert_with(|| std::sync::Arc::new(SectionCounters::default()))
        .clone()
}

fn reset_section_counters(section: &'static str, total: i32) {
    let c = counters_for(section);
    use std::sync::atomic::Ordering::Relaxed;
    c.total.store(total, Relaxed);
    c.added.store(0, Relaxed);
    c.failed.store(0, Relaxed);
    c.active.store(true, Relaxed);
    if let Ok(mut g) = c.last_error.lock() { g.clear(); }
}

// When true, scans run without surfacing the progress card (startup restore,
// post-save library refresh). The grid still populates; only the popup is muted.
static SCAN_SILENT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
fn scan_silent() -> bool { SCAN_SILENT.load(std::sync::atomic::Ordering::Relaxed) }
fn set_scan_silent(v: bool) { SCAN_SILENT.store(v, std::sync::atomic::Ordering::Relaxed); }

/// Feed the Downloader's Rescan pill from the music section's own counters.
///
/// Deliberately separate from the scan popup below: the Rescan button starts a
/// SILENT scan (no popup — it is a music-only refresh, not a library event), and
/// `flush_progress` returns early on those. Without this the button had no way
/// to say it was working.
fn push_music_rescan(weak: &slint::Weak<MainWindow>) {
    use std::sync::atomic::Ordering::Relaxed;
    let (total, done, active) = match scan_state().lock() {
        Ok(g) => match g.get("music") {
            Some(c) => (c.total.load(Relaxed),
                        c.added.load(Relaxed) + c.failed.load(Relaxed),
                        c.active.load(Relaxed)),
            None => (0, 0, false),
        },
        Err(_) => (0, 0, false),
    };
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_dl_rescan_busy(active);
        w.set_music_dl_rescan_frac(if total > 0 { done as f32 / total as f32 } else { 0.0 });
    });
}

fn flush_progress(weak: &slint::Weak<MainWindow>) {
    push_music_rescan(weak);
    if scan_silent() { return; }
    let snapshot: Vec<(&'static str, i32, i32, i32, bool, String)> = {
        let g = scan_state().lock().unwrap();
        use std::sync::atomic::Ordering::Relaxed;
        g.iter()
            .map(|(name, c)| {
                let err = c.last_error.lock().map(|s| s.clone()).unwrap_or_default();
                (
                    *name,
                    c.total.load(Relaxed),
                    c.added.load(Relaxed),
                    c.failed.load(Relaxed),
                    c.active.load(Relaxed),
                    err,
                )
            })
            .collect()
    };
    let _ = weak.upgrade_in_event_loop(move |w| {
        let model = w.get_scan_progress();
        let mut rows: Vec<ScanProgress> = (0..model.row_count())
            .map(|i| model.row_data(i).unwrap())
            .collect();
        for (section, total, added, failed, active, err) in snapshot {
            if let Some(r) = rows.iter_mut().find(|r| r.section.as_str() == section) {
                r.total = total;
                r.added = added;
                r.failed = failed;
                r.active = active;
                if !err.is_empty() { r.last_error = err.into(); }
            }
        }
        let any_active = rows.iter().any(|r| r.active);
        // Auto-dismiss: once every section finished, keep the popup up for
        // 4 s (so the final counts are readable) then hide it. The popup shows
        // while scan-active OR scan-progress has rows, so the timer must clear
        // BOTH; it must also arm on all_done alone — scan_active was just set
        // false above, so gating on it meant the timer never fired.
        let all_done = !rows.is_empty()
            && rows.iter().all(|r| r.total > 0 && r.added + r.failed >= r.total);
        w.set_scan_progress(slint::ModelRc::new(slint::VecModel::from(rows)));
        w.set_scan_active(any_active);
        use std::sync::atomic::Ordering::Relaxed;
        if all_done && !SCAN_HIDE_SCHEDULED.swap(true, Relaxed) {
            let weak = w.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_secs(4), move || {
                if let Some(w) = weak.upgrade() {
                    w.set_scan_active(false);
                    w.set_scan_progress(slint::ModelRc::new(
                        slint::VecModel::from(Vec::<ScanProgress>::new())));
                }
            });
        }
        if !all_done { SCAN_HIDE_SCHEDULED.store(false, Relaxed); }
    });
}

/// One pending auto-hide per finished scan session (reset when a new scan
/// starts producing unfinished rows again).
static SCAN_HIDE_SCHEDULED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// True while any section's library scan is running.
fn any_scan_active() -> bool {
    scan_state()
        .lock()
        .map(|g| g.values().any(|c| c.active.load(std::sync::atomic::Ordering::Relaxed)))
        .unwrap_or(false)
}

/// The idle-only background indexer (np.p2.ai.background).
///
/// `tulipix_photos::ai::background` has always owned the *policy* — when to run,
/// when to pause, how big a batch — and said in its own header that the worker
/// loop "lives in tulipix-app's main runtime". It did not; nothing called
/// `decide` or `pending`, so no photo was ever indexed outside a scan. This is
/// that loop.
///
/// Shape mirrors the library scan: claim a bounded batch, do the per-file work
/// in parallel off the reactor, write the results, repeat. The differences are
/// that this one yields to the user — it only runs while idle and off battery,
/// and rechecks between every batch — and that its queue is defined by
/// `photo_ai_state`, so it drains and then costs one query per tick.
fn spawn_background_indexer() {
    use tulipix_photos::ai::background::{self, BackgroundPolicy, RunState, Stage, SystemSnapshot};

    static SPAWNED: OnceLock<()> = OnceLock::new();
    if SPAWNED.set(()).is_err() { return; }

    // Stages with an implementation behind them. Faces and Tags are here now
    // that SCRFD/ArcFace and YOLOX are wired; each still no-ops when its blob
    // is not installed, checked inside the stage rather than here so that
    // downloading a model takes effect without a restart.
    //
    // CLIP is absent: `ai/clip.rs` still ships only its Null embedder, and
    // running it would mark every photo considered while embedding nothing —
    // draining the queue to a wrong answer and leaving the real model with no
    // work to find.
    const LIVE_STAGES: [Stage; 4] = [Stage::Exif, Stage::Fts, Stage::Faces, Stage::Tags];

    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let policy = BackgroundPolicy::default();
        // Consecutive passes that found nothing. A drained library is the
        // steady state — most of the time this loop exists to do nothing — so
        // it backs off to a couple of minutes rather than opening the pool and
        // running two counting queries every five seconds forever.
        let mut quiet_passes: u32 = 0;
        loop {
            tokio::time::sleep(policy.tick * (1u32 << quiet_passes.min(5))).await;

            // A library scan is the one thing guaranteed to be competing for the
            // same disk and cores, and it is the user waiting on a result. Yield
            // to it outright — this check does not depend on idle detection.
            if any_scan_active() {
                continue;
            }

            let snapshot = SystemSnapshot {
                // CAVEAT: this is weaker than it looks. `idle::mark_active` is
                // documented as being called "on every pointer/key event" and is
                // in fact called from exactly two places — the ambient overlay's
                // dismiss handler and its own minimised-window re-arm. Nothing
                // reports ordinary input, so after `spawn_tracker`'s first tick
                // this counts seconds since app start, not seconds since the
                // user last did something.
                //
                // The existing consumer (ambient screensaver / auto-lock) does
                // not notice because auto-lock is off by default, which parks
                // the threshold a year out. Wiring real input reporting is a fix
                // to the idle subsystem, not to the indexer, so it is left
                // alone here — and `any_scan_active` above plus the small batch,
                // the backoff and the battery gate are what actually keep this
                // loop out of the user's way in the meantime.
                idle_secs: tulipix_core::idle::idle_secs(),
                on_battery: matches!(
                    tulipix_core::power_aware::power_source(),
                    tulipix_core::power_aware::PowerSource::Battery
                ),
            };
            if background::decide(&policy, snapshot) != RunState::Active {
                // Not a quiet pass — there may be plenty of work, the user is
                // simply using the machine. Keep the short tick so indexing
                // resumes promptly once they stop.
                continue;
            }

            let Ok(pool) = pool_for("photos").await else {
                quiet_passes = quiet_passes.saturating_add(1);
                continue;
            };
            // One batch per pass, not a drain loop: the idle/battery check above
            // is the yield point, and someone who touches the keyboard mid-drain
            // should get the machine back at the next batch boundary rather than
            // when the library runs out.
            let mut worked = false;
            for stage in LIVE_STAGES {
                let batch = match background::next_batch(&pool, stage, policy.batch_size as i64).await {
                    Ok(b) if !b.is_empty() => b,
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::warn!(stage = stage.key(), error = %e, "indexer queue");
                        continue;
                    }
                };
                let n = batch.len();
                if let Err(e) = run_index_stage(&pool, stage, batch).await {
                    tracing::warn!(stage = stage.key(), error = %e, "indexer stage");
                } else {
                    tracing::debug!(stage = stage.key(), count = n, "indexed");
                }
                worked = true;
                break;
            }
            quiet_passes = if worked { 0 } else { quiet_passes.saturating_add(1) };
        }
    });
}

/// Run one stage over one claimed batch, marking every item considered.
///
/// Every item is marked whether or not the stage produced anything, which is
/// what lets the queue drain — see the note on `photo_ai_state`.
async fn run_index_stage(
    pool: &sqlx::SqlitePool,
    stage: tulipix_photos::ai::background::Stage,
    batch: Vec<(i64, String)>,
) -> Result<()> {
    use tulipix_photos::ai::background::{self, Stage};

    match stage {
        Stage::Exif => {
            // Re-read EXIF for photos that never got a `photo_meta` row — a
            // library scanned by an older build, or a file whose read failed.
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(scan_concurrency()));
            let handles: Vec<_> = batch
                .into_iter()
                .map(|(id, path)| {
                    let sem = sem.clone();
                    tokio::spawn(async move {
                        let _permit = sem.acquire().await.ok();
                        let facts = tokio::task::spawn_blocking(move || {
                            tulipix_photos::exif::read(std::path::Path::new(&path)).unwrap_or_default()
                        })
                        .await
                        .ok();
                        (id, facts)
                    })
                })
                .collect();
            for h in handles {
                let Ok((id, Some(facts))) = h.await else { continue };
                if let Err(e) = tulipix_photos::exif::write_facts(pool, id, &facts).await {
                    tracing::warn!(item = id, error = %e, "indexer exif write");
                    continue;
                }
                background::mark_done(pool, id, Stage::Exif, "").await?;
            }
        }
        Stage::Fts => {
            // The full-text index was only ever built by the "Rebuild search
            // index" button in Settings → Maintenance, so a freshly scanned
            // library had an empty `photo_fts`. Keeping it current here is what
            // makes search work without the user knowing that button exists.
            for (id, _) in batch {
                if let Err(e) = tulipix_photos::search::index_item(pool, id).await {
                    tracing::warn!(item = id, error = %e, "indexer fts");
                    continue;
                }
                background::mark_done(pool, id, Stage::Fts, "").await?;
            }
        }
        Stage::Tags => {
            // No detector installed is not the same as "detected nothing":
            // marking these done would drain the queue to a wrong answer and
            // leave the real model with no work when it arrives.
            let Some(tagger) = make_tagger() else { return Ok(()) };
            for (id, path) in batch {
                let p = std::path::PathBuf::from(&path);
                match tulipix_photos::ai::tags::ingest_predictions(pool, id, &p, tagger.as_ref(), 0.35).await {
                    Ok(n) => tracing::debug!(item = id, tags = n, "tagged"),
                    // A file that cannot be decoded will never tag; mark it
                    // considered so it stops coming back round.
                    Err(e) => tracing::debug!(item = id, error = %e, "tag skipped"),
                }
                background::mark_done(pool, id, Stage::Tags, tagger.source()).await?;
            }
        }
        Stage::Faces => {
            let Some((detector, embedder)) = make_face_models() else { return Ok(()) };
            let mut found = 0usize;
            for (id, path) in batch {
                let p = std::path::PathBuf::from(&path);
                match tulipix_photos::ai::faces::extract_crops(pool, id, &p, detector.as_ref()).await {
                    Ok(ids) => found += ids.len(),
                    Err(e) => tracing::debug!(item = id, error = %e, "face detect skipped"),
                }
                background::mark_done(pool, id, Stage::Faces, embedder.model_name()).await?;
            }
            if found > 0 {
                // Embed the crops just written, then re-cluster. Clustering is
                // over the whole library by nature — a new face can merge two
                // existing piles — so it runs once per batch, not per photo.
                tulipix_photos::ai::faces::embed_pending(pool, embedder.as_ref(), found as i64 * 2).await?;
                let n = tulipix_photos::ai::face_clusters::recluster(
                    pool,
                    tulipix_photos::ai::face_clusters::DEFAULT_THRESHOLD,
                )
                .await?;
                tracing::debug!(faces = found, people = n, "reclustered");
            }
        }
        // CLIP still needs its embedder — see `LIVE_STAGES`. Kept exhaustive so
        // adding one is a compile error here rather than a silent no-op.
        Stage::Clip => {}
    }
    Ok(())
}

/// Spawn the singleton flush ticker — coalesces atomic counters into one UI
/// post per ~80 ms regardless of file throughput.
fn spawn_progress_flusher(weak: slint::Weak<MainWindow>) {
    static SPAWNED: OnceLock<()> = OnceLock::new();
    if SPAWNED.set(()).is_err() { return; }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(80)).await;
            flush_progress(&weak);
        }
    });
}

/// Walk a folder, classify every file by extension, return the list of files
/// that belong to `section`. Uses the same classifier as classify_folder so
/// the per-section walk matches the up-front total exactly.
fn list_section_files(root: &std::path::Path, section: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() { continue; }
        let Some(ext) = entry.path().extension().and_then(|s| s.to_str()) else { continue; };
        if section_for_ext(ext) == Some(section) {
            out.push(entry.path().to_path_buf());
        }
    }
    out
}

// now_secs moved to tulipix_common.

/// Render a thumb for `src`. Runs on a blocking worker so the tokio reactor
/// stays responsive while the decode is crunching.
///
/// The photo/ffmpeg choice lives in `tulipix_core::thumbs::render_photo` now:
/// JPEG/PNG/WebP/GIF/BMP/TIFF decode in-process and only fall back to ffmpeg if
/// that fails, so the common formats work without `just fetch` and without a
/// subprocess. This used to be a second renderer here, writing JPEGs into
/// `~/.config/Tulipix/cache/thumbs` while the real cache lives in
/// `~/.cache/Tulipix/thumbs` — a split the LRU eviction, the cache-size readout
/// and "Clear cache" all knew nothing about, and which meant every photo still
/// paid for a doomed ffmpeg spawn before reaching it.
///
/// `Ok(None)` is `ThumbKind::OsFallback`: no thumbnail is produced and the
/// caller draws the source path via the OS icon theme.
async fn thumb_for(src: PathBuf, kind: tulipix_core::thumbs::ThumbKind) -> Result<PathBuf> {
    tokio::task::spawn_blocking(move || {
        match tulipix_core::thumbs::render_or_cache(
            &src,
            tulipix_core::thumbs::ThumbSpec { kind, width: 320, height: 320 },
        ) {
            Ok(Some(t)) => Ok(t.path),
            Ok(None) => Ok(src.clone()),
            Err(e) => Err(e),
        }
    })
    .await
    .map_err(|e| anyhow::anyhow!("join: {e}"))?
}

/// Turn an internal error into a single-line message for the progress card.
/// Strips the chain to its leaf so we don't paint "open sqlite pool foo:
/// database is locked: sqlite error code 5" — the user wants "Database
/// locked: file.jpg" instead.
fn friendly_err(stage: &str, path: &std::path::Path, e: &anyhow::Error) -> String {
    let leaf = e.chain().last().map(|c| c.to_string()).unwrap_or_else(|| e.to_string());
    let lower = leaf.to_ascii_lowercase();
    let short = if lower.contains("permission denied") || lower.contains("eacces") {
        "Permission denied"
    } else if lower.contains("no such file") || lower.contains("not found") || lower.contains("enoent") {
        "Not found"
    } else if lower.contains("locked") || lower.contains("busy") || lower.contains("sqlite_busy") {
        "DB locked, retrying"
    } else if lower.contains("decode") || lower.contains("unsupported") || lower.contains("invalid") {
        "Unsupported format"
    } else if lower.contains("ffmpeg") || lower.contains("ffprobe") {
        "ffmpeg missing — run `just fetch`"
    } else {
        // Fall back to first 60 chars of the leaf error.
        let trimmed: String = leaf.chars().take(60).collect();
        return format!("{stage} ({}): {trimmed}", path.file_name().and_then(|s| s.to_str()).unwrap_or(""));
    };
    format!("{stage} ({}): {short}", path.file_name().and_then(|s| s.to_str()).unwrap_or(""))
}

#[cfg(unix)]
fn inode_of(meta: &std::fs::Metadata) -> i64 {
    use std::os::unix::fs::MetadataExt;
    meta.ino() as i64
}
#[cfg(not(unix))]
fn inode_of(_meta: &std::fs::Metadata) -> i64 { 0 }

/// A file's identity for the `items` row, read once in `scan_file_work` and
/// carried to the DB stage so no `stat` happens under the write lock.
#[derive(Debug, Clone, Copy)]
struct FileMeta {
    inode: i64,
    size: i64,
    mtime: i64,
}

/// Everything one scanned file needs, gathered without touching the database.
///
/// Stat, EXIF and thumbnail are pure per-file work over the filesystem with no
/// shared state, which is exactly what makes them parallelisable; the DB half
/// that follows is the part that must serialise.
struct ScanFile {
    /// Index in the original walk order, so grid placement stays stable no
    /// matter which order the batch happens to finish in.
    slot: usize,
    path: PathBuf,
    /// `None` when the file could not be stat'd — it vanished mid-scan.
    meta: Option<FileMeta>,
    /// Photos only.
    facts: Option<tulipix_photos::exif::ExifFacts>,
    /// The rendered thumb, or a message ready for the progress card.
    thumb: std::result::Result<PathBuf, String>,
}

/// The filesystem half of scanning one file. Blocking throughout — call it from
/// `spawn_blocking`, never on the reactor.
fn scan_file_work(
    slot: usize,
    path: PathBuf,
    section: &str,
    kind: tulipix_core::thumbs::ThumbKind,
) -> ScanFile {
    let meta = std::fs::metadata(&path).ok().map(|m| FileMeta {
        inode: inode_of(&m),
        size: m.len() as i64,
        mtime: m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
    });
    let facts = (section == "photos")
        .then(|| tulipix_photos::exif::read(&path).unwrap_or_default());
    // `render_or_cache` directly rather than `thumb_for`: we are already on a
    // blocking worker, so there is nothing to gain from another hop.
    let thumb = match tulipix_core::thumbs::render_or_cache(
        &path,
        tulipix_core::thumbs::ThumbSpec { kind, width: 320, height: 320 },
    ) {
        Ok(Some(t)) => Ok(t.path),
        // OsFallback — no thumb is produced and the tile draws the source.
        Ok(None) => Ok(path.clone()),
        Err(e) => Err(friendly_err("Thumb", &path, &e)),
    };
    ScanFile { slot, path, meta, facts, thumb }
}

/// Files whose filesystem half may be in flight at once.
///
/// The work is decode- and stat-bound, so past the core count there is nothing
/// to win, and the ceiling keeps a many-core box from having that many ffmpeg
/// subprocesses live at once for the formats that still need one.
fn scan_concurrency() -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(2, 8)
}

/// Files per transaction. Large enough that the write lock is taken rarely,
/// small enough that a batch's worth of work is still bounded and progress
/// still moves visibly.
const SCAN_BATCH: usize = 64;

/// How often a scan in progress pushes what it has found into the live grid.
///
/// Throttled by time rather than by file count because a rebuild costs the same
/// whether the scan is crawling over RAW files or racing through small JPEGs:
/// the window is capped at `PHOTO_PAGE` tiles, so the expensive half is bounded
/// and what varies is how often it is paid. Per batch would mean ~78 rebuilds
/// for a 5,000-file import, most of them superseded before they finished.
const SCAN_STREAM_EVERY: std::time::Duration = std::time::Duration::from_millis(800);

/// Append freshly scanned rows to the section's in-memory library, deduped by
/// absolute path so a re-scan, or a second watched folder, cannot double them.
///
/// Ordering note: rows arrive in scan order (viewport-proximity first), not in
/// `walkdir` order as they did when the whole list was appended at the end. That
/// is not a regression to defend against — `walkdir` order is filesystem order,
/// arbitrary to begin with, and every view either takes its order from the DB
/// (timeline, folder, the category tabs) or sorts for itself (Library).
fn append_section_rows(section: &str, rows: Vec<(String, PathBuf, PathBuf)>) {
    if rows.is_empty() { return; }
    let acc = match section {
        "photos" => photo_full(),
        "videos" => video_full(),
        "music" => music_full(),
        _ => return,
    };
    let Ok(mut g) = acc.lock() else { return };
    let mut have: std::collections::HashSet<String> =
        g.iter().map(|(_, o, _)| o.to_string_lossy().into_owned()).collect();
    for row in rows {
        if have.insert(row.1.to_string_lossy().into_owned()) {
            g.push(row);
        }
    }
}

/// Rebuild the section's live view from what has been accumulated so far, so a
/// scan fills the grid as it runs instead of staying blank until it ends.
///
/// This is the same work the end-of-scan block does; running it mid-scan is
/// safe because a grid rebuild is a pure function of the accumulator, and
/// because `kick_category_refresh` stamps each rebuild and drops superseded
/// ones — so a tick that lands while a later one is already in flight throws
/// its own result away rather than fighting over the grid.
fn stream_section_refresh(section: &'static str, weak: &slint::Weak<MainWindow>) {
    let _ = weak.upgrade_in_event_loop(move |w| match section {
        "photos" => {
            // Guard released on this line: `photo_folder_count` takes the same
            // lock, and `std::sync::Mutex` is not reentrant.
            let total = photo_full().lock().map(|g| g.len() as i32).unwrap_or(0);
            w.set_photos_total(total);
            w.set_photos_folder_count(photo_folder_count());
            let cat = w.get_photos_category().to_string();
            let q = w.get_photos_query().to_string();
            kick_category_refresh(w.as_weak(), cat, q);
        }
        "videos" => {
            let cat = w.get_video_category().to_string();
            kick_video_refresh(w.as_weak(), cat);
        }
        "music" => {
            rebuild_music_tiles(&w);
            populate_music_views(w.as_weak());
        }
        _ => {}
    });
}

/// Insert (or update) a single file row in the section DB. Returns the item id.
/// Caller is responsible for any per-section side effects.
///
/// Takes a connection rather than the pool so the scan loop can hand it the
/// batch transaction and commit item rows together with their sections'
/// side-effect rows. SQLite takes exactly one writer at a time, so every extra
/// autocommit statement is another acquisition of the same global write lock —
/// with several library scans running at once the loser busy-waits on
/// `busy_timeout` (5s), which is what the multi-second "slow statement"
/// warnings were.
///
/// `meta` is passed in rather than stat'd here: the filesystem read belongs in
/// `scan_file_work`, where it happens in parallel and outside the write lock.
async fn upsert_one(
    conn: &mut sqlx::SqliteConnection,
    section: &str,
    path: &std::path::Path,
    meta: FileMeta,
) -> Result<i64> {
    let abs = path.to_string_lossy().into_owned();
    let FileMeta { inode, size, mtime } = meta;
    let now = now_secs();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM items WHERE abs_path = ?",
    )
    .bind(&abs)
    .fetch_optional(&mut *conn)
    .await
    .context("query existing row")?;
    if let Some(id) = existing {
        sqlx::query(
            "UPDATE items SET inode = ?, size = ?, mtime = ?, missing_since = NULL, updated = ? WHERE id = ?",
        )
        .bind(inode).bind(size).bind(mtime).bind(now).bind(id)
        .execute(&mut *conn).await.context("update row")?;
        Ok(id)
    } else {
        let res = sqlx::query(
            "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&abs).bind(inode).bind(size).bind(mtime)
        .bind(section).bind(now).bind(now)
        .execute(&mut *conn).await.context("insert row")?;
        Ok(res.last_insert_rowid())
    }
}

/// One file's rows inside an open batch transaction: the `items` row plus its
/// section's side-effect row. Purely SQL — the stat, EXIF and thumbnail it
/// depends on were all done in `scan_file_work`.
async fn scan_write_row(
    conn: &mut sqlx::SqliteConnection,
    section: &str,
    f: &ScanFile,
    meta: FileMeta,
) -> Result<i64> {
    let id = upsert_one(&mut *conn, section, &f.path, meta).await?;
    if let Some(facts) = f.facts.as_ref() {
        tulipix_photos::exif::write_facts(&mut *conn, id, facts).await.context("photo_meta")?;
        // The scan has just read this file's EXIF, so record the attempt in the
        // same transaction. Without it the background indexer would queue every
        // freshly scanned photo for a second read that can only reach the same
        // answer. The FTS stage is deliberately NOT marked — the scan does not
        // do it, so those stay queued and the indexer picks them up.
        tulipix_photos::ai::background::mark_done(
            &mut *conn,
            id,
            tulipix_photos::ai::background::Stage::Exif,
            "",
        )
        .await
        .context("ai_state exif")?;
    } else if section == "videos" {
        sqlx::query("INSERT OR IGNORE INTO video_meta (item_id) VALUES (?)")
            .bind(id).execute(&mut *conn).await.context("video_meta")?;
    }
    Ok(id)
}

/// A whole batch of scanned files committed in ONE transaction: each `items`
/// row plus its section's side-effect row (`photo_meta` from the already-read
/// EXIF, or the empty `video_meta` seed). Returns one result per input, in
/// order, so the caller can attribute a failure to its file.
///
/// The write lock is global to the database — several library scans share one
/// pool per section — so the number of times it is taken is what decides
/// whether the losers busy-wait on `busy_timeout` and log multi-second "slow
/// statement" warnings. One acquisition per batch rather than per file is the
/// point: at `SCAN_BATCH` = 64 that is 64× fewer.
///
/// Nothing here touches the filesystem. Every stat, EXIF read and thumbnail
/// happened in `scan_file_work` before this was called, so the write lock is
/// never held across I/O — one slow photo cannot stall every other scan.
async fn scan_write_batch(
    pool: &sqlx::SqlitePool,
    section: &str,
    batch: &[ScanFile],
) -> Vec<Result<i64>> {
    let _w = scan_write_lock(section).lock().await;
    let mut tx = match pool.begin().await.context("begin scan tx") {
        Ok(tx) => tx,
        // The batch never opened, so nothing in it landed.
        Err(e) => {
            let msg = format!("{e:#}");
            return batch.iter().map(|_| Err(anyhow::anyhow!("{msg}"))).collect();
        }
    };
    let mut out: Vec<Result<i64>> = Vec::with_capacity(batch.len());
    for f in batch {
        // A file that vanished between the walk and the stat has no row to
        // write; it fails on its own without costing the rest of the batch.
        let Some(meta) = f.meta else {
            out.push(Err(anyhow::anyhow!("stat {}: not found", f.path.display())));
            continue;
        };
        out.push(scan_write_row(&mut tx, section, f, meta).await);
    }
    // A failed commit invalidates every id handed out above — none of those
    // rows exist. Report the whole batch as failed rather than let the caller
    // act on ids (TV classification, poster scrape) that point at nothing.
    if let Err(e) = tx.commit().await.context("commit scan tx") {
        let msg = format!("{e:#}");
        return batch.iter().map(|_| Err(anyhow::anyhow!("{msg}"))).collect();
    }
    out
}

/// One write mutex per section, held only across one batch's statements.
/// Several library scans share a section's pool, and SQLite admits exactly one
/// writer; without this they discover that by busy-waiting on `busy_timeout`
/// (5s) and logging multi-second "slow statement" warnings. The lock turns that
/// spin into a fair queue. It is NEVER held across file I/O.
fn scan_write_lock(section: &str) -> &'static tokio::sync::Mutex<()> {
    static LOCKS: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, &'static tokio::sync::Mutex<()>>>,
    > = std::sync::OnceLock::new();
    let map = LOCKS.get_or_init(Default::default);
    let mut g = map.lock().unwrap_or_else(|e| e.into_inner());
    *g.entry(section.to_string())
        .or_insert_with(|| Box::leak(Box::new(tokio::sync::Mutex::new(()))))
}

/// Scroll-position hint for the scan thumb queue (np.p1.thumbs.lazy):
/// f32 fraction 0..1 of the grid the viewport currently shows, as bits.
static SCAN_HINT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn kick_section_scan(
    window: &MainWindow,
    root: PathBuf,
    lib_id: String,
    section: &'static str,
    _expected_total: i64,
) {
    let weak = window.as_weak();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let counters = counters_for(section);
        use std::sync::atomic::Ordering::Relaxed;

        // 1) Open the section DB. If this fails, the whole section is dead —
        // surface the reason so the user knows why.
        let pool = match pool_for(section).await {
            Ok(p) => p,
            Err(e) => {
                let msg = format!("DB open: {e:#}");
                tracing::error!(section, "{msg}");
                let t = counters.total.load(Relaxed);
                counters.failed.store(t, Relaxed);
                counters.active.store(false, Relaxed);
                if let Ok(mut g) = counters.last_error.lock() { *g = msg; }
                flush_progress(&weak);
                return;
            }
        };

        // Books bypass the generic items scan entirely: books.db has no
        // `items` table, and the section has its own scanner (metadata chain,
        // baked 3D covers, FTS). Register the folder there and drive the
        // shared progress counters so the popup + Watched panels stay live.
        if section == "books" {
            // ponytail: scan_all_progress sweeps every registered book folder,
            // so a global lock keeps startup-restore of several watched
            // folders from running duplicate concurrent sweeps.
            static BOOKS_SCAN: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
            let guard = BOOKS_SCAN.lock().await;
            let _ = tulipix_books::scan::add_folder(&pool, &root.display().to_string()).await;
            let c2 = counters.clone();
            let weak_p = weak.clone();
            let weak_g = weak.clone();
            let _ = tulipix_books::scan::scan_all_progress(&pool, move |total, done, _title| {
                c2.total.store(total as i32, Relaxed);
                c2.added.store(done as i32, Relaxed);
                let step = (total / 100).max(1);
                if done % step == 0 || done == total { flush_progress(&weak_p); }
                // Live grid: swap freshly-ingested books in every ~10 files
                // instead of only when the whole sweep finishes.
                if done % 10 == 0 {
                    tulipix_sec_books::books_refresh(weak_g.clone(), 0);
                }
            })
            .await;
            // Release the scan lock BEFORE the slow content indexing below —
            // holding it there made a second "Add books" wait behind minutes
            // of FTS parsing with its progress popup stuck at zero.
            drop(guard);
            counters.active.store(false, Relaxed);
            flush_progress(&weak);
            let n = list_section_files(&root, section).len() as i32;
            let lib_id_ui = lib_id.clone();
            let _ = weak.upgrade_in_event_loop(move |w| {
                let model = w.get_library_rows();
                let mut rows: Vec<LibraryRow> = (0..model.row_count())
                    .map(|i| model.row_data(i).unwrap()).collect();
                for r in rows.iter_mut() {
                    if r.id == lib_id_ui.as_str() {
                        r.r#last_scan = "just now".into();
                        r.item_count = n;
                    }
                }
                w.set_library_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
            });
            tulipix_sec_books::books_refresh(weak.clone(), 0);
            // The scan only wrote rows. Cover art (extract + two bakes per
            // book) drains here instead, in batches, so the sweep itself
            // finishes at parse speed and the grid fills in behind it.
            tulipix_sec_books::books_build_art(weak.clone());
            tulipix_sec_books::books_backfill_metadata(weak.clone());
            // Content indexing for library-wide search — slow on big
            // libraries but resumable, so a partial run picks up next time.
            // Own lock so two adds can't double-index the same books.
            static BOOKS_INDEX: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
            let _ig = BOOKS_INDEX.lock().await;
            let _ = tulipix_books::scan::index_contents(&pool).await;
            return;
        }

        // 2) Walk + filter files belonging to this section. No populator —
        // we drive the loop ourselves so per-file failures land in progress.
        let files = list_section_files(&root, section);
        let actual = files.len() as i32;
        counters.total.store(actual, Relaxed);

        let kind = thumb_kind_for_section(section);
        // Viewport-intersection lazy thumbs (np.p1.thumbs.lazy): the scroll
        // position publishes a 0..1 hint (SCAN_HINT); each iteration processes
        // the pending file nearest the viewport, so on-screen tiles get their
        // thumbs first. Grid order stays stable — slots are indexed.
        let total_files = files.len();
        let mut pending: std::collections::BTreeMap<usize, PathBuf> =
            files.iter().cloned().enumerate().collect();
        let mut slots: Vec<Option<(PathBuf, String, PathBuf)>> = vec![None; total_files];
        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(scan_concurrency()));
        // Rows found since the last push to the grid, and when that push was.
        // `None` means "due now", so the first completed batch paints rather
        // than waiting out a full interval on an empty page.
        let mut streamed: Vec<(String, PathBuf, PathBuf)> = Vec::new();
        let mut last_stream: Option<std::time::Instant> = None;
        while !pending.is_empty() {
            // 2a) Claim the batch nearest the viewport. The hint is sampled once
            // per batch rather than once per file — re-reading it between two
            // files of the same batch would only add noise.
            let frac = f32::from_bits(SCAN_HINT.load(Relaxed)).clamp(0.0, 1.0);
            let want = ((frac as f64) * total_files.saturating_sub(1) as f64) as usize;
            let mut claimed: Vec<(usize, PathBuf)> = Vec::with_capacity(SCAN_BATCH);
            while claimed.len() < SCAN_BATCH && !pending.is_empty() {
                let key = pending.range(want..).next().map(|(k, _)| *k)
                    .or_else(|| pending.range(..want).next_back().map(|(k, _)| *k))
                    .unwrap_or(0);
                let p = pending.remove(&key).unwrap();
                claimed.push((key, p));
            }

            // 2b) The filesystem half — stat, EXIF and thumbnail — in parallel.
            // These touch only their own file, so there is nothing to serialise
            // between them; the permit bounds how many run at once, and it is
            // taken in the async half because `spawn_blocking` cannot await one.
            let handles: Vec<_> = claimed
                .into_iter()
                .map(|(slot, path)| {
                    let sem = sem.clone();
                    tokio::spawn(async move {
                        let _permit = sem.acquire().await.ok();
                        tokio::task::spawn_blocking(move || {
                            scan_file_work(slot, path, section, kind)
                        })
                        .await
                        .ok()
                    })
                })
                .collect();
            let mut batch: Vec<ScanFile> = Vec::with_capacity(handles.len());
            for h in handles {
                if let Ok(Some(f)) = h.await { batch.push(f); }
            }

            // 2c) The DB half — every row for the batch in one transaction, so
            // the global write lock is taken once instead of `SCAN_BATCH` times.
            let ids = scan_write_batch(&pool, section, &batch).await;

            // 2d) Per-file bookkeeping, in batch order.
            for (f, id) in batch.into_iter().zip(ids) {
                let id = match id {
                    Ok(id) => id,
                    Err(e) => {
                        let msg = friendly_err("DB insert", &f.path, &e);
                        tracing::warn!(section, path = %f.path.display(), "{msg}");
                        counters.failed.fetch_add(1, Relaxed);
                        if let Ok(mut g) = counters.last_error.lock() { *g = msg; }
                        continue;
                    }
                };

                if section == "videos" {
                    // TV detection (np.p3.episodes): an SxxEyy filename ⇒ episode
                    // of the show named by its parent folder. Populates
                    // shows+episodes so the Videos "TV" tab can group it. Left
                    // outside the transaction above: it does its own multi-table
                    // work.
                    classify_tv_episode(&pool, id, &f.path).await;
                    // TMDB/TVDB poster scrape runs in background so it doesn't
                    // block the scan loop (np.p3.tmdb).
                    let pool2 = pool.clone();
                    let path2 = f.path.clone();
                    tokio::spawn(async move { scrape_video_tmdb(pool2, id, path2).await; });
                }

                let label = f.path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let (thumb, ok) = match f.thumb {
                    Ok(thumb) => (thumb, true),
                    Err(msg) => {
                        // No thumb: the tile still appears, drawing the source.
                        if let Ok(mut g) = counters.last_error.lock() { *g = msg; }
                        (f.path.clone(), false)
                    }
                };
                if ok {
                    counters.added.fetch_add(1, Relaxed);
                } else {
                    counters.failed.fetch_add(1, Relaxed);
                }
                slots[f.slot] = Some((thumb.clone(), label.clone(), f.path.clone()));
                // The accumulator's tuple order differs from the slot's.
                streamed.push((label, f.path, thumb));
            }
            // No progress flush here on purpose: `spawn_progress_flusher`
            // already coalesces those atomics into one UI post per ~80 ms
            // regardless of throughput, and posting per batch on top of it would
            // only add event-loop work the ticker exists to avoid.
            //
            // The grid is a different matter — nothing else pushes it — so what
            // has been found so far goes in on a timer.
            let due = last_stream.map(|t| t.elapsed() >= SCAN_STREAM_EVERY).unwrap_or(true);
            if due && !streamed.is_empty() {
                append_section_rows(section, std::mem::take(&mut streamed));
                stream_section_refresh(section, &weak);
                last_stream = Some(std::time::Instant::now());
            }
        }
        // Whatever the last interval did not carry. The end-of-scan block below
        // re-appends everything from `slots` anyway and dedupes, so this is
        // about the rows being present before that runs, not about losing them.
        append_section_rows(section, std::mem::take(&mut streamed));
        // Re-assemble in original walk order — grid placement stays stable.
        let tiles: Vec<(PathBuf, String, PathBuf)> = slots.into_iter().flatten().collect();
        counters.active.store(false, Relaxed);
        flush_progress(&weak);

        let n = tiles.len() as i32;
        // Slots hold (thumb, label, orig); the accumulator wants (label, orig,
        // thumb). Appended off the UI thread — these are plain global mutexes,
        // and the dedup means the rows already streamed in cost nothing here.
        let full: Vec<(String, PathBuf, PathBuf)> = tiles
            .into_iter()
            .map(|(thumb, label, orig)| (label, orig, thumb))
            .collect();
        append_section_rows(section, full);
        // One last rebuild so the grid reflects the finished library.
        //
        // This used to build a `Vec<PhotoTile>` here first, calling
        // `Image::load_from_path` once per file — every thumbnail in the scan,
        // decoded on the event loop — and then every single arm below dropped it
        // on the floor with `let _ = (paths, out, n)`. On a five-thousand-photo
        // import that was five thousand JPEG decodes on the UI thread whose only
        // effect was the hitch they caused. The grid is rebuilt from the
        // accumulator by the refresh below, as the old comment said it was.
        stream_section_refresh(section, &weak);

        let lib_id_for_ui = lib_id.clone();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if section == "music" {
                // Once per scan, not on the periodic refresh: the scanned-roots
                // list so a freshly added folder shows up with its count, and the
                // tag extraction pass for tracks lacking them (np.p4.music.tags).
                //
                // The audiobook flag goes first and is awaited: a folder added to
                // the Audiobooks section is tagged the instant it is picked, but
                // `is_audiobook` is an UPDATE over rows that only exist now. Set
                // it before anything reads them and the chapters never appear in
                // My Music at all, rather than sitting there until the next
                // rebuild happened to re-assert the tag.
                apply_music_folder_sections(w.as_weak());
            }
            let model = w.get_library_rows();
            let mut rows: Vec<LibraryRow> = (0..model.row_count())
                .map(|i| model.row_data(i).unwrap()).collect();
            for r in rows.iter_mut() {
                if r.id == lib_id_for_ui.as_str() {
                    r.r#last_scan = "just now".into();
                    r.item_count = n;
                }
            }
            w.set_library_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
            // Milestone confetti (np.p1.ds.empty-state): celebrate crossing
            // 1k photos / 1k tracks / 500 videos — once per section per session.
            let milestone = match (section, n) {
                ("photos", n) if n >= 1000 => Some(format!("{n} photos — library milestone! 🎉")),
                ("music", n) if n >= 1000 => Some(format!("{n} tracks — library milestone! 🎉")),
                ("videos", n) if n >= 500 => Some(format!("{n} videos — library milestone! 🎉")),
                _ => None,
            };
            if let Some(m) = milestone {
                static FIRED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
                let fired = FIRED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()));
                if fired.lock().map(|mut g| g.insert(section.to_string())).unwrap_or(false) {
                    w.set_milestone(m.into());
                }
            }
            tracing::info!(section, count = n, "grid populated");
        });
    });
}

// dirs_default / dirs_default_documents moved to the tulipix-common crate.

fn wire_youtube(window: &MainWindow) {
    // ── YouTube section wiring (np.p4.music.youtube) ────────────────────────
    let w = window.as_weak();
    window.on_music_yt_set_tab(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_tab(t.clone());
        // Models are warmed once (warm_youtube) and refreshed by their own actions
        // (download/sub/cache/remove), so plain sub-tab switches are instant — no
        // re-decoding thumbnails on every click. First open warms if not already.
        warm_youtube(&w0);
        // Reload watched positions so the progress bars on the cards are right
        // for whatever this tab is about to draw.
        tokio::runtime::Handle::current().spawn(refresh_yt_progress());
    });
    let w = window.as_weak();
    window.on_music_yt_back(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_channel_open(false);
        w0.set_music_yt_playlist_open(false);
    });
    let w = window.as_weak();
    window.on_music_yt_unsub(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let id = id.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await { let _ = tulipix_music::youtube::store::unsubscribe(&pool, &id).await; }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_subs(&w));
        });
    });
    // Manual refresh — re-fetch subscriber/video counts for every channel. Counts
    // never auto-refresh on opening the page; this button is the only trigger.
    let w = window.as_weak();
    window.on_music_yt_refresh_subs(move || {
        if let Some(w0) = w.upgrade() { yt_fetch_sub_meta(w0.as_weak(), true); }
    });
    let w = window.as_weak();
    window.on_music_yt_subs_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_subs_sort(s);
        w0.set_music_yt_subs_page(0);
        populate_yt_subs(&w0);
    });
    // Pin a channel to the Home right rail (newest first, max 9, persistent).
    let w = window.as_weak();
    window.on_music_yt_add_home_channel(move |id| {
        let Some(w0) = w.upgrade() else { return; };
        yt_add_home_channel(&id);
        yt_reco_clear();              // Home rail changed → rebuild Recommended
        populate_yt_subs(&w0);
        populate_yt_recommended(&w0);
    });
    // Unpin a channel from the Home rail (fired after the confirm dialog).
    let w = window.as_weak();
    window.on_music_yt_remove_home_channel(move |id| {
        let Some(w0) = w.upgrade() else { return; };
        yt_remove_home_channel(&id);
        yt_reco_clear();
        populate_yt_subs(&w0);
        populate_yt_recommended(&w0);
    });
    // Toggle the Subscriptions page filter (subscribed ↔ unsubscribed); persist.
    let w = window.as_weak();
    window.on_music_yt_subs_toggle_filter(move || {
        let Some(w0) = w.upgrade() else { return; };
        let next = if yt_subs_filter() == "unsub" { "sub" } else { "unsub" };
        yt_set_subs_filter(next);
        w0.set_music_yt_subs_page(0);
        populate_yt_subs(&w0);
    });
    let w = window.as_weak();
    window.on_music_yt_subs_toggle_dir(move || {
        let Some(w0) = w.upgrade() else { return; };
        let dir = if w0.get_music_yt_subs_dir() == "asc" { "desc" } else { "asc" };
        w0.set_music_yt_subs_dir(dir.into());
        w0.set_music_yt_subs_page(0);
        populate_yt_subs(&w0);
    });
    // Subscribe/unsubscribe confirmation → dispatch the real action.
    let w = window.as_weak();
    window.on_music_yt_sub_confirm_yes(move || {
        let Some(w0) = w.upgrade() else { return; };
        if w0.get_music_yt_sub_confirm_channelpage() {
            w0.invoke_music_yt_channel_toggle_sub();
            return;
        }
        let id = w0.get_music_yt_sub_confirm_id().to_string();
        if w0.get_music_yt_sub_confirm_add() {
            // Re-subscribe a previously unsubscribed channel.
            let title = w0.get_music_yt_sub_confirm_title().to_string();
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                if let Ok(pool) = pool_for("youtube").await {
                    let _ = tulipix_music::youtube::store::import_subs(&pool,
                        &[tulipix_music::youtube::subscriptions::ImportedSub { channel_id: id, title }]).await;
                }
                let _ = weak.upgrade_in_event_loop(|w| populate_yt_subs(&w));
            });
        } else {
            w0.invoke_music_yt_unsub(id.into());
        }
    });
    let w = window.as_weak();
    window.on_music_yt_subs_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_yt_subs_page() + d).clamp(0, (w0.get_music_yt_subs_pages() - 1).max(0));
        w0.set_music_yt_subs_page(next);
        populate_yt_subs(&w0);
    });
    let w = window.as_weak();
    window.on_music_yt_clear_subs(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await { let _ = tulipix_music::youtube::store::clear_subs(&pool).await; }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_subs(&w));
        });
    });
    // Import Takeout subscriptions (CSV/JSON) via the native file picker.
    let w = window.as_weak();
    window.on_music_yt_import(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Takeout subscriptions", &["csv", "json"])
            .set_title("Import YouTube subscriptions")
            .pick_file() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            match tulipix_music::youtube::subscriptions::parse(&text) {
                Ok(subs) if !subs.is_empty() => {
                    if let Ok(pool) = pool_for("youtube").await {
                        let _ = tulipix_music::youtube::store::import_subs(&pool, &subs).await;
                    }
                    let _ = weak.upgrade_in_event_loop(|w| {
                        w.set_music_yt_add_open(false);
                        w.set_music_yt_status(slint::SharedString::new());
                        w.set_music_yt_tab("subscriptions".into());
                        populate_yt_subs(&w);
                        yt_fetch_sub_meta(w.as_weak(), false);
                    });
                }
                _ => {
                    let _ = weak.upgrade_in_event_loop(|w|
                        w.set_music_yt_status("Couldn't read that file — pick a Takeout subscriptions.csv or .json.".into()));
                }
            }
        });
    });
    // Add a single channel by URL (/@handle, /channel/UC…, /c/…, or a video URL).
    let w = window.as_weak();
    window.on_music_yt_add_channel_url(move |url| {
        let Some(w0) = w.upgrade() else { return; };
        let url = url.trim().to_string();
        if url.is_empty() { return; }
        w0.set_music_yt_status("Resolving channel…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            match ytdlp_resolve_channel(&url).await {
                Some((cid, title)) => {
                    if let Ok(pool) = pool_for("youtube").await {
                        let _ = tulipix_music::youtube::store::import_subs(&pool,
                            &[tulipix_music::youtube::subscriptions::ImportedSub { channel_id: cid, title }]).await;
                    }
                    let _ = weak.upgrade_in_event_loop(|w| {
                        w.set_music_yt_add_open(false);
                        w.set_music_yt_add_url(slint::SharedString::new());
                        w.set_music_yt_status(slint::SharedString::new());
                        w.set_music_yt_tab("subscriptions".into());
                        populate_yt_subs(&w);
                        yt_fetch_sub_meta(w.as_weak(), false);
                    });
                }
                None => {
                    let _ = weak.upgrade_in_event_loop(|w|
                        w.set_music_yt_status("Couldn't resolve that channel URL — paste a channel or video link.".into()));
                }
            }
        });
    });
    // Home search — Piped (+ yt-dlp fallback), Shorts filtered. Shows YT_PAGE,
    // "Load more" appends.
    let w = window.as_weak();
    window.on_music_yt_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        let q = q.trim().to_string();
        if q.is_empty() {
            // Cleared → drop search results so Home shows "Recommended" again.
            { let mut st = yt_search_state().lock().unwrap(); st.all.clear(); st.shown = 0; st.nextpage = None; }
            yt_render_search(&w0);
            return;
        }
        w0.set_music_yt_busy(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let dir = yt_thumb_dir();
            let videos = yt_fetch_search(&q, 20).await;
            let rows = yt_vid_data_all(&client, &dir, &videos).await;
            yt_remember(&rows);
            if let Ok(pool) = pool_for("youtube").await {
                let _ = tulipix_music::youtube::store::push_recent_search(&pool, &q).await;
            }
            let _ = q;
            {
                let mut st = yt_search_state().lock().unwrap();
                st.all = rows;
                st.nextpage = None; st.shown = YT_PAGE;
            }
            let _ = weak.upgrade_in_event_loop(|w| { yt_render_search(&w); populate_yt_recent(&w); });
        });
    });
    let w = window.as_weak();
    window.on_music_yt_search_more(move || {
        let Some(w0) = w.upgrade() else { return; };
        { let mut st = yt_search_state().lock().unwrap(); st.shown = (st.shown + YT_PAGE).min(st.all.len()); }
        yt_render_search(&w0);
    });
    let w = window.as_weak();
    window.on_music_yt_recent_click(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_query(q.clone());
        w0.invoke_music_yt_search(q);
    });
    let w = window.as_weak();
    window.on_music_yt_clear_recent(move || {
        let Some(w0) = w.upgrade() else { return; };
        // Clear the UI list immediately, then wipe the DB rows in the background.
        w0.set_music_yt_recent(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::SharedString>::new())));
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await {
                let _ = tulipix_music::youtube::store::clear_recent_searches(&pool).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_recent(&w));
        });
    });
    // Context-aware top-search filter for the list pages (subs/playlists/cached/downloads).
    let w = window.as_weak();
    window.on_music_yt_filter(move |kind, q| {
        let Some(w0) = w.upgrade() else { return; };
        let q = q.to_string();
        match kind.as_str() {
            "subs"      => { if let Ok(mut g) = yt_subs_q().lock()   { *g = q; } populate_yt_subs(&w0); }
            "playlists" => { if let Ok(mut g) = yt_pls_q().lock()    { *g = q; } populate_yt_playlists(&w0); }
            "cached"    => { if let Ok(mut g) = yt_cached_q().lock() { *g = q; } populate_yt_cached(&w0); }
            "downloads" => { if let Ok(mut g) = yt_dls_q().lock()    { *g = q; } w0.set_music_yt_downloads_page(0); populate_yt_downloads(&w0); }
            _ => {}
        }
    });
    // Open a channel page (first page).
    let w = window.as_weak();
    window.on_music_yt_open_channel(move |cid| {
        let Some(w0) = w.upgrade() else { return; };
        let cid = cid.to_string();
        { let mut st = yt_ch_search_state().lock().unwrap(); st.all.clear(); st.shown = 0; }
        w0.set_music_yt_busy(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let pool = pool_for("youtube").await.ok();
            // Subscribed? + total-subscribers line (cheap; from yt_subs meta) + the
            // channel's own name from the sub record (so we never show "Channel").
            let (subscribed, sub_line, fallback_name, cached) = match &pool {
                Some(p) => {
                    let subs = tulipix_music::youtube::store::list_subs(p).await.unwrap_or_default();
                    let me = subs.iter().find(|s| s.channel_id == cid);
                    let sub = me.map(|s| s.subscribed).unwrap_or(false);
                    let line = me.and_then(|s| s.sub_count).filter(|n| *n > 0)
                        .map(|n| format!("{} subscribers", yt_fmt_count(n))).unwrap_or_default();
                    let nm = me.map(|s| s.title.clone()).filter(|t| !t.is_empty()).unwrap_or_default();
                    let cache = tulipix_music::youtube::store::get_channel_cache(p, &cid).await.unwrap_or_default();
                    (sub, line, nm, cache)
                }
                None => (false, String::new(), String::new(), vec![]),
            };
            // Cache-first: show stored videos instantly; only fetch when empty.
            let rows: Vec<YtVidData> = if !cached.is_empty() {
                cached.iter().map(yt_channelvid_to_data).collect()
            } else {
                let client = tulipix_core::net::http().clone();
                let dir = yt_thumb_dir();
                let videos = ytdlp_channel_latest(&cid, 10).await;
                let r = yt_vid_data_all(&client, &dir, &videos).await;
                if let Some(p) = &pool {
                    let cv: Vec<_> = r.iter().map(yt_data_to_channelvid).collect();
                    let _ = tulipix_music::youtube::store::set_channel_cache(p, &cid, &cv).await;
                }
                r
            };
            yt_remember(&rows);
            let name = rows.first().map(|r| r.channel.clone()).filter(|c| !c.is_empty())
                .or_else(|| (!fallback_name.is_empty()).then(|| fallback_name.clone()))
                .unwrap_or_else(|| "Channel".to_string());
            // Avatar from the sub record if we have it; else leave default.
            let avatar = match &pool {
                Some(p) => tulipix_music::youtube::store::list_subs(p).await.unwrap_or_default()
                    .into_iter().find(|s| s.channel_id == cid).and_then(|s| s.avatar_path).unwrap_or_default(),
                None => String::new(),
            };
            let decoded = yt_videos(&rows).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_yt_channel_id(cid.into());
                w.set_music_yt_channel_title(name.into());
                w.set_music_yt_channel_avatar(yt_img(&avatar));
                w.set_music_yt_channel_sub(sub_line.into());
                w.set_music_yt_channel_videos(yt_model(decoded));
                w.set_music_yt_channel_subscribed(subscribed);
                w.set_music_yt_channel_query(slint::SharedString::new());
                w.set_music_yt_channel_results(yt_video_model(&[]));
                w.set_music_yt_channel_results_more(false);
                w.set_music_yt_channel_mode("latest".into());
                w.set_music_yt_channel_open(true);
                w.set_music_yt_busy(false);
            });
        });
    });
    // Channel Refresh — re-fetch latest 10 + update the cache (latest mode).
    let w = window.as_weak();
    window.on_music_yt_channel_refresh(move || {
        let Some(w0) = w.upgrade() else { return; };
        let cid = w0.get_music_yt_channel_id().to_string();
        if cid.is_empty() { return; }
        w0.set_music_yt_channel_mode("latest".into());
        w0.set_music_yt_busy(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_latest(&cid, 10).await;
            let rows = yt_vid_data_all(&client, &dir, &videos).await;
            yt_remember(&rows);
            if let Ok(p) = pool_for("youtube").await {
                let cv: Vec<_> = rows.iter().map(yt_data_to_channelvid).collect();
                let _ = tulipix_music::youtube::store::set_channel_cache(&p, &cid, &cv).await;
            }
            let decoded = yt_videos(&rows).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_yt_channel_videos(yt_model(decoded));
                w.set_music_yt_busy(false);
            });
        });
    });
    // Channel Popular — top-10 most-watched. Show stored list instantly (if any),
    // then refresh in the background and update live; tapping again re-refreshes.
    let w = window.as_weak();
    window.on_music_yt_channel_popular(move || {
        let Some(w0) = w.upgrade() else { return; };
        let cid = w0.get_music_yt_channel_id().to_string();
        if cid.is_empty() { return; }
        w0.set_music_yt_channel_mode("popular".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let pool = pool_for("youtube").await.ok();
            // 1. Instant: show whatever is cached so far (no spinner if we have it).
            let cached: Vec<YtVidData> = match &pool {
                Some(p) => tulipix_music::youtube::store::get_channel_popular(p, &cid).await
                    .unwrap_or_default().iter().map(yt_channelvid_to_data).collect(),
                None => vec![],
            };
            let had_cache = !cached.is_empty();
            if had_cache {
                yt_remember(&cached);
                let decoded = yt_videos(&cached).await;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_yt_channel_videos(yt_model(decoded)));
            } else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_busy(true));
            }
            // 2. Background fetch fresh top-10, store, then update if changed.
            let client = tulipix_core::net::http().clone();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_popular(&cid, 10).await;
            let fresh = yt_vid_data_all(&client, &dir, &videos).await;
            if !fresh.is_empty() {
                yt_remember(&fresh);
                if let Some(p) = &pool {
                    let cv: Vec<_> = fresh.iter().map(yt_data_to_channelvid).collect();
                    let _ = tulipix_music::youtube::store::set_channel_popular(p, &cid, &cv).await;
                }
                let changed = fresh.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
                    != cached.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
                if changed || !had_cache {
                    let decoded = yt_videos(&fresh).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        if w.get_music_yt_channel_mode() == "popular" {
                            w.set_music_yt_channel_videos(yt_model(decoded));
                        }
                        w.set_music_yt_busy(false);
                    });
                    return;
                }
            }
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_busy(false));
        });
    });
    // Channel-scoped search — results live in their own model (latest list is
    // kept intact). Empty query clears results so the latest videos reappear.
    let w = window.as_weak();
    window.on_music_yt_channel_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        let cid = w0.get_music_yt_channel_id().to_string();
        let q = q.trim().to_string();
        if cid.is_empty() { return; }
        if q.is_empty() {
            { let mut st = yt_ch_search_state().lock().unwrap(); st.all.clear(); st.shown = 0; }
            w0.set_music_yt_channel_results(yt_video_model(&[]));
            w0.set_music_yt_channel_results_more(false);
            w0.set_music_yt_busy(false);
            return;
        }
        w0.set_music_yt_busy(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_search(&cid, &q, 20).await;
            let rows = yt_vid_data_all(&client, &dir, &videos).await;
            yt_remember(&rows);
            { let mut st = yt_ch_search_state().lock().unwrap(); st.shown = rows.len().min(10); st.all = rows; }
            let _ = weak.upgrade_in_event_loop(|w| yt_render_channel_search(&w));
        });
    });
    // Channel search "Load more" — reveal the next 10 (capped at 20 total).
    let w = window.as_weak();
    window.on_music_yt_channel_load_more(move || {
        let Some(w0) = w.upgrade() else { return; };
        { let mut st = yt_ch_search_state().lock().unwrap(); st.shown = (st.shown + 10).min(st.all.len()).min(20); }
        yt_render_channel_search(&w0);
    });
    let w = window.as_weak();
    window.on_music_yt_channel_toggle_sub(move || {
        let Some(w0) = w.upgrade() else { return; };
        let cid = w0.get_music_yt_channel_id().to_string();
        let title = w0.get_music_yt_channel_title().to_string();
        let want = !w0.get_music_yt_channel_subscribed();
        w0.set_music_yt_channel_subscribed(want);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("youtube").await else { return; };
            if want {
                let _ = tulipix_music::youtube::store::import_subs(&pool,
                    &[tulipix_music::youtube::subscriptions::ImportedSub { channel_id: cid, title }]).await;
            } else {
                let _ = tulipix_music::youtube::store::unsubscribe(&pool, &cid).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_subs(&w));
        });
    });
    // Play — audio plays in the in-app player; video opens the resolution picker.
    let w = window.as_weak();
    window.on_music_yt_play(move |id, want_video| {
        let Some(w0) = w.upgrade() else { return; };
        let id = id.to_string();
        if want_video {
            // Saved default → skip the picker and play straight at that quality.
            let def = yt_default_res();
            if def != -999 {
                let start = {
                    let same = yt_cur_audio().lock().map(|g| *g == id).unwrap_or(false);
                    if same { w0.get_music_pos() as f64 } else { 0.0 }
                };
                yt_watch_video(w.clone(), id, def, start);
            } else {
                w0.set_music_yt_res_id(id.into());
                w0.set_music_yt_res_open(true);
            }
            return;
        }
        // A single tap cancels any active play-queue, then streams instantly.
        if let Ok(mut g) = yt_queue().lock() { g.0.clear(); g.1 = 0; }
        w0.set_music_yt_playing_pl_id(-1);  // single video → no playlist is "playing"
        // Instant feedback: paint the now-playing card from the row we already have
        // so a card tap / ⋯ "Play (audio)" responds the moment it's clicked instead
        // of waiting on the stream-URL resolve — same snappiness as the Play button.
        if let Some(m) = yt_lookup(&id) {
            w0.set_music_player_mode("music".into());
            w0.set_music_yt_now_video(true);
            // This paints before the stream resolves, so it also has to take
            // down the last track's lyrics — otherwise they keep scrolling over
            // a YouTube video for as long as the resolve takes.
            clear_music_lyrics(&w0);
            w0.set_music_np_title(m.title.clone().into());
            w0.set_music_np_sub(if m.channel.is_empty() { "YouTube".into() } else { m.channel.clone().into() });
            w0.set_music_np_art(yt_img(&m.thumb));
            w0.set_music_np_accent(if m.thumb.is_empty() { slint::Color::from_rgb_u8(0xef, 0x44, 0x44) }
                else { dominant_color(std::path::Path::new(&m.thumb)).unwrap_or(slint::Color::from_rgb_u8(0xef, 0x44, 0x44)) });
            w0.set_music_pos(0.0); w0.set_music_dur(0.0);
            w0.set_music_pos_label("0:00".into()); w0.set_music_dur_label("0:00".into());
            w0.set_music_playing(true);
        }
        yt_play_audio(w.clone(), id);
    });
    // Play a DOWNLOADED item from its local file: video downloads open windowed
    // mpv (the downloaded video, not re-streamed audio); audio downloads play
    // in-app from the local file. No caching — they're already permanent.
    let w = window.as_weak();
    window.on_music_yt_play_download(move |path| {
        let Some(_w0) = w.upgrade() else { return; };
        let path = path.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("youtube").await else { return; };
            // The Downloads grid identifies a row by its unique media-file path so a
            // video downloaded at several resolutions plays the exact one tapped.
            let dls = tulipix_music::youtube::store::list_downloads(&pool, 100000).await.unwrap_or_default();
            match dls.into_iter().find(|d| d.media_path == path) {
                Some(d) if !d.media_path.is_empty() && std::path::Path::new(&d.media_path).exists() => {
                    if d.quality == "Audio" {
                        let (idd, p, t, c, th) = (d.video_id.clone(), d.media_path.clone(), d.title.clone(), d.channel.clone(), d.thumb_path.clone());
                        if let Ok(mut g) = yt_queue().lock() { g.0.clear(); g.1 = 0; }
                        let _ = weak.upgrade_in_event_loop(move |w| yt_play_inapp(&w, idd, p, t, c, th, 0.0));
                    } else {
                        yt_play_local_video(weak.clone(), d.media_path.clone());
                    }
                }
                // Missing local file → fall back to streaming the audio by video id.
                Some(d) => { if let Ok(mut g) = yt_queue().lock() { g.0.clear(); g.1 = 0; } yt_play_audio(weak.clone(), d.video_id.clone()); }
                None => {}
            }
        });
    });
    // Watch the currently-playing YouTube audio as video (player transport button).
    let w = window.as_weak();
    window.on_music_yt_watch_current(move || {
        let Some(w0) = w.upgrade() else { return; };
        let id = yt_cur_audio().lock().map(|g| g.clone()).unwrap_or_default();
        if id.is_empty() { return; }
        // The picker lives in the YouTube view; switch to it so the popup renders
        // even when triggered from the global mini/zen player on another tab.
        w0.set_music_view("youtube".into());
        // Offer the resolution picker (same as a normal video tap); yt-play-
        // resolution hands off the current audio position for a seamless switch.
        w0.set_music_yt_res_id(id.into());
        w0.set_music_yt_res_open(true);
    });
    // Watch video — stops in-app audio, plays video fullscreen at the audio's
    // position, then resumes audio for the same video where the video stopped.
    let w = window.as_weak();
    window.on_music_yt_play_resolution(move |id, height| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_res_open(false);
        // If this same video's audio is playing, hand off its position to the video.
        let start = {
            let same = yt_cur_audio().lock().map(|g| *g == id.to_string()).unwrap_or(false);
            if same { w0.get_music_pos() as f64 } else { 0.0 }
        };
        yt_watch_video(w.clone(), id.to_string(), height as i64, start);
    });
    // Save / clear the default watch resolution (np.p4.music.youtube).
    window.set_music_yt_default_res(yt_default_res() as i32);
    let w = window.as_weak();
    window.on_music_yt_set_default_res(move |h| {
        let Some(w0) = w.upgrade() else { return; };
        yt_store_default_res(Some(h as i64));
        w0.set_music_yt_default_res(h);
        w0.set_music_yt_res_open(false);
    });
    let w = window.as_weak();
    window.on_music_yt_reset_video_prefs(move || {
        let Some(w0) = w.upgrade() else { return; };
        yt_store_default_res(None);
        w0.set_music_yt_default_res(-999);
    });
    // Download — explicit, permanent (bestaudio → opus).
    // Download button → opens the quality picker.
    let w = window.as_weak();
    window.on_music_yt_download(move |id| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_dl_id(id);
        w0.set_music_yt_dl_open(true);
    });
    // Actual download at a chosen quality: height>0 = VP9 video + best audio,
    // height==0 = best video, height<0 = audio only (opus).
    let w = window.as_weak();
    window.on_music_yt_download_resolution(move |id, height| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_dl_open(false);
        let id = id.to_string();
        let meta = yt_lookup(&id).unwrap_or_default();
        yt_dl_enqueue(w.clone(), YtDlJobData {
            id, title: meta.title, channel: meta.channel, thumb: meta.thumb,
            height: height as i64, frac: 0.0, status: "Queued".to_string(),
        });
    });
    let w = window.as_weak();
    window.on_music_yt_remove_cached(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let id = id.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await {
                if let Some(p) = tulipix_music::youtube::store::remove_cached(&pool, &id).await.ok().flatten() { let _ = std::fs::remove_file(&p); }
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_cached(&w));
        });
    });
    let w = window.as_weak();
    window.on_music_yt_remove_download(move |path| {
        let Some(_w0) = w.upgrade() else { return; };
        let path = path.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await {
                // Path-targeted: removes just this resolution, not the video's others.
                if let Some(p) = tulipix_music::youtube::store::remove_download_by_path(&pool, &path).await.ok().flatten() { let _ = std::fs::remove_file(&p); }
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_downloads(&w));
        });
    });
    let w = window.as_weak();
    window.on_music_yt_clear_cached(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await {
                for p in tulipix_music::youtube::store::clear_cached(&pool).await.unwrap_or_default() { let _ = std::fs::remove_file(&p); }
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_cached(&w));
        });
    });
    let w = window.as_weak();
    window.on_music_yt_clear_downloads(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await {
                for p in tulipix_music::youtube::store::clear_downloads(&pool).await.unwrap_or_default() { let _ = std::fs::remove_file(&p); }
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_downloads(&w));
        });
    });
    // Downloads list paging + sort (10/page, mirrors podcast downloads).
    let w = window.as_weak();
    window.on_music_yt_downloads_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_yt_downloads_page() + d).clamp(0, (w0.get_music_yt_downloads_pages() - 1).max(0));
        w0.set_music_yt_downloads_page(next);
        populate_yt_downloads(&w0);
    });
    let w = window.as_weak();
    window.on_music_yt_downloads_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_downloads_sort(s);
        w0.set_music_yt_downloads_page(0);
        populate_yt_downloads(&w0);
    });
    // Playlists.
    let w = window.as_weak();
    window.on_music_yt_create_playlist(move |name| {
        let Some(_w0) = w.upgrade() else { return; };
        let name = name.trim().to_string();
        if name.is_empty() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await { let _ = tulipix_music::youtube::store::create_playlist(&pool, &name).await; }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_playlists(&w));
        });
    });
    let w = window.as_weak();
    window.on_music_yt_delete_playlist(move |pid| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await { let _ = tulipix_music::youtube::store::delete_playlist(&pool, pid as i64).await; }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_playlists(&w));
        });
    });
    let w = window.as_weak();
    window.on_music_yt_open_playlist(move |pid| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_busy(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("youtube").await else { return; };
            let (name, source_url, video_count) = tulipix_music::youtube::store::get_playlist(&pool, pid as i64)
                .await.ok().flatten().unwrap_or_default();
            let total = match video_count.filter(|n| *n > 0) {
                Some(n) => n,
                None => tulipix_music::youtube::store::playlist_item_count(&pool, pid as i64).await.unwrap_or(0),
            };
            { let mut g = yt_pl_open().lock().unwrap(); g.id = pid as i64; g.source_url = source_url; g.total = total; }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_yt_playlist_title(name.into());
                w.set_music_yt_playlist_query(slint::SharedString::new());
                w.set_music_yt_playlist_sort("default".into());
                w.set_music_yt_playlist_open(true);
                w.set_music_yt_busy(false);
                yt_playlist_load(w.as_weak(), 0, "default".to_string(), String::new());
            });
        });
    });
    let w = window.as_weak();
    window.on_music_yt_playlist_page_go(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_yt_playlist_page() - 1 + d).max(0);
        w0.set_music_yt_busy(true);
        yt_playlist_load(w.clone(), next as i64, w0.get_music_yt_playlist_sort().to_string(), w0.get_music_yt_playlist_query().to_string());
    });
    let w = window.as_weak();
    window.on_music_yt_playlist_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        yt_playlist_load(w.clone(), 0, w0.get_music_yt_playlist_sort().to_string(), q.to_string());
    });
    let w = window.as_weak();
    window.on_music_yt_playlist_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_playlist_sort(s.clone());
        yt_playlist_load(w.clone(), 0, s.to_string(), w0.get_music_yt_playlist_query().to_string());
    });
    // Play all (open detail page) — queue every video + play, with full metadata.
    let w = window.as_weak();
    window.on_music_yt_playlist_play_all(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_pl_playall_busy(true);
        let id = yt_pl_open().lock().ok().map(|g| g.id).unwrap_or(-1);
        if id < 0 { w0.set_music_yt_pl_playall_busy(false); return; }
        yt_play_all_playlist(w.clone(), id);
    });
    // Play all from the playlist card (Playlists grid) — same, addressed by id.
    let w = window.as_weak();
    window.on_music_yt_playlist_play_all_id(move |pid| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_pl_playall_busy(true);
        yt_play_all_playlist(w.clone(), pid as i64);
    });
    // Import a YouTube playlist by URL (metadata + count only; videos lazy).
    let w = window.as_weak();
    window.on_music_yt_import_url(move |url| {
        let Some(w0) = w.upgrade() else { return; };
        let url = url.trim().to_string();
        if url.is_empty() { return; }
        w0.set_music_yt_pl_import_busy(true);
        w0.set_music_yt_pl_import_status("Reading playlist…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let meta = ytdlp_playlist_meta(&url).await;
            match meta {
                Some((title, count)) => {
                    if let Ok(pool) = pool_for("youtube").await {
                        let _ = tulipix_music::youtube::store::create_remote_playlist(&pool, &title, &url, count).await;
                    }
                    let _ = weak.upgrade_in_event_loop(|w| {
                        w.set_music_yt_pl_import_busy(false);
                        w.set_music_yt_pl_import_status(slint::SharedString::new());
                        w.set_music_yt_pl_import_url(slint::SharedString::new());
                        w.set_music_yt_pl_import_open(false);
                        populate_yt_playlists(&w);
                    });
                }
                None => {
                    let _ = weak.upgrade_in_event_loop(|w| {
                        w.set_music_yt_pl_import_busy(false);
                        w.set_music_yt_pl_import_status("Couldn't read that playlist URL.".into());
                    });
                }
            }
        });
    });
    // Import a Takeout playlist CSV (video ids → local playlist, count only).
    let w = window.as_weak();
    window.on_music_yt_import_playlist_file(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(path) = rfd::FileDialog::new()
            .add_filter("Playlist export (JSON/CSV/TXT)", &["json", "csv", "txt"])
            .set_title("Import a YouTube playlist (JSON, CSV or TXT)")
            .pick_file() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let text = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            let ids = tulipix_music::youtube::subscriptions::parse_playlist_ids_any(&text);
            if ids.is_empty() { return; }
            let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported playlist").to_string();
            if let Ok(pool) = pool_for("youtube").await {
                if let Ok(pid) = tulipix_music::youtube::store::create_playlist(&pool, &name).await {
                    let _ = tulipix_music::youtube::store::add_playlist_ids(&pool, pid, &ids).await;
                }
            }
            let _ = weak.upgrade_in_event_loop(|w| { w.set_music_yt_pl_import_open(false); populate_yt_playlists(&w); });
        });
    });
    let w = window.as_weak();
    window.on_music_yt_add_to_playlist(move |id, pl| {
        let Some(_w0) = w.upgrade() else { return; };
        let id = id.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("youtube").await else { return; };
            let pid = if pl < 0 {
                tulipix_music::youtube::store::create_playlist(&pool, "New Playlist").await.unwrap_or(0)
            } else { pl as i64 };
            if pid > 0 {
                let m = yt_lookup(&id).unwrap_or_default();
                let _ = tulipix_music::youtube::store::add_to_playlist(&pool, pid, &id, &m.title, &m.channel, &m.thumb, m.dur_s).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_playlists(&w));
        });
    });
    // Podcast sub-tab switch (Home / Subscribed / Downloads).
    let w = window.as_weak();
    window.on_music_podcast_set_tab(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        // Pure UI swap. All three lists (Home/Subscribed/Downloads) are populated
        // once on entering Podcasts and again on Refresh/Fetch/Add/Reset — so a
        // tab change must NOT re-query the DB (that was the switch lag/stutter).
        // Trends is the one exception: it fetches the baked feed list (cached for
        // the session) the first time it's opened.
        let is_trends = t == "trends";
        let is_downloads = t == "downloads";
        w0.set_music_podcast_tab(t);
        // Trends: build only the first time (or after a reset emptied it). The
        // grid model + sort flags persist, so re-entering is an instant UI swap
        // instead of re-decoding 21 cover images on every tab change.
        if is_trends && w0.get_music_podcast_trends().row_count() == 0 {
            populate_podcast_trends(&w0);
        }
        // Downloads: episodes finish downloading while the user is on other
        // tabs, so this list re-queries on entry (cheap now that art decodes
        // off-thread + caches — the old "no re-query" rule predates that).
        if is_downloads {
            populate_podcast_downloads(&w0);
        }
    });
    // Filter the Subscribed grid by category (reset to page 1).
    let w = window.as_weak();
    window.on_music_podcast_set_cat(move |c| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_cat(c);
        w0.set_music_podcast_sub_page(0);
        populate_podcasts(&w0);
    });
    // Subscribed / Home grid pagination (re-render from DB).
    let w = window.as_weak();
    window.on_music_podcast_sub_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_podcast_sub_page() + d).clamp(0, (w0.get_music_podcast_sub_pages() - 1).max(0));
        w0.set_music_podcast_sub_page(next);
        populate_podcasts(&w0);
    });
    let w = window.as_weak();
    window.on_music_podcast_home_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_podcast_home_page() + d).clamp(0, (w0.get_music_podcast_home_pages() - 1).max(0));
        w0.set_music_podcast_home_page(next);
        populate_podcasts(&w0);
    });
    // Home "Your shows" — sort (name / category / latest episode).
    let w = window.as_weak();
    window.on_music_podcast_home_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_home_sort(s);
        w0.set_music_podcast_home_page(0);
        populate_podcasts(&w0);
    });
    // Toggle a show's presence on Home "Your shows" (pin/unpin).
    let w = window.as_weak();
    window.on_music_podcast_toggle_home(move |pid| {
        if w.upgrade().is_none() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let _ = sqlx::query("UPDATE podcasts SET home_pinned = 1 - COALESCE(home_pinned,0) WHERE id = ?")
                .bind(pid as i64).execute(&pool).await;
            // Reflect new pin state on the open detail page, if any.
            let pinned: i64 = sqlx::query_scalar("SELECT COALESCE(home_pinned,0) FROM podcasts WHERE id = ?")
                .bind(pid as i64).fetch_one(&pool).await.unwrap_or(0);
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_podcast_d_pinned(pinned != 0);
                populate_podcasts(&w);
            });
        });
    });
    // Downloads tab — sort + pagination.
    let w = window.as_weak();
    window.on_music_podcast_dl_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_dl_sort(s);
        w0.set_music_podcast_dl_page(0);
        populate_podcast_downloads(&w0);
    });
    let w = window.as_weak();
    window.on_music_podcast_dl_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_podcast_dl_page() + d).clamp(0, (w0.get_music_podcast_dl_pages() - 1).max(0));
        w0.set_music_podcast_dl_page(next);
        populate_podcast_downloads(&w0);
    });
    // Re-fetch a single feed by id (Subscribed/Home context menu → Fetch).
    let w = window.as_weak();
    window.on_music_podcast_fetch(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let url: Option<String> = sqlx::query_scalar("SELECT feed_url FROM podcasts WHERE id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            if let Some(url) = url {
                let client = tulipix_core::net::http().clone();
                if let Ok(resp) = client.get(&url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
                    if let Ok(xml) = resp.text().await {
                        let feed = tulipix_music::podcasts::parse_feed(&xml);
                        let _ = tulipix_music::podcasts::subscribe(&pool, &url, &feed).await;
                    }
                }
            }
            let _ = weak.upgrade_in_event_loop(|w| refresh_podcast_views(&w));
        });
    });
    // Unsubscribe a feed by id — removes it (and episodes, via cascade) everywhere.
    let w = window.as_weak();
    window.on_music_podcast_unsubscribe(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            // Drop any cached offline files for this feed first.
            let paths: Vec<(Option<String>,)> = sqlx::query_as(
                "SELECT downloaded_path FROM podcast_episodes WHERE podcast_id = ? AND downloaded_path IS NOT NULL")
                .bind(id as i64).fetch_all(&pool).await.unwrap_or_default();
            for (p,) in paths { if let Some(p) = p { let _ = std::fs::remove_file(&p); } }
            let _ = sqlx::query("DELETE FROM podcast_episodes WHERE podcast_id = ?").bind(id as i64).execute(&pool).await;
            let _ = sqlx::query("DELETE FROM podcasts WHERE id = ?").bind(id as i64).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                // If the open detail belongs to this feed, close it.
                let cur = cur_podcast_id().lock().map(|g| *g).unwrap_or(-1);
                if cur == id as i64 {
                    w.set_music_podcast_detail_open(false);
                    if let Ok(mut g) = cur_podcast_id().lock() { *g = -1; }
                }
                populate_podcasts(&w);
                populate_podcast_latest(&w);
                populate_podcast_downloads(&w);
            });
        });
    });
    // Remove an episode's offline copy (Downloads context menu).
    let w = window.as_weak();
    window.on_music_podcast_remove_download(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let path: Option<String> = sqlx::query_scalar("SELECT downloaded_path FROM podcast_episodes WHERE id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            if let Some(p) = path { let _ = std::fs::remove_file(&p); }
            let _ = sqlx::query("UPDATE podcast_episodes SET downloaded_path = NULL WHERE id = ?").bind(id as i64).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| refresh_podcast_views(&w));
        });
    });
    // Clear ALL offline downloads (user request 2026-06-12): delete every
    // cached file + null the paths in one pass.
    let w = window.as_weak();
    window.on_music_podcast_clear_downloads(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let paths: Vec<String> = sqlx::query_scalar(
                "SELECT downloaded_path FROM podcast_episodes WHERE downloaded_path IS NOT NULL")
                .fetch_all(&pool).await.unwrap_or_default();
            let n = paths.len();
            for p in paths { let _ = std::fs::remove_file(&p); }
            let _ = sqlx::query("UPDATE podcast_episodes SET downloaded_path = NULL WHERE downloaded_path IS NOT NULL")
                .execute(&pool).await;
            tracing::info!(removed = n, "podcast downloads cleared");
            let _ = weak.upgrade_in_event_loop(|w| refresh_podcast_views(&w));
        });
    });
    // Detail page — sort (newest/oldest) + pagination.
    let w = window.as_weak();
    window.on_music_podcast_d_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_d_sort(s);
        w0.set_music_podcast_d_page(0);
        let pid = cur_podcast_id().lock().map(|g| *g).unwrap_or(-1);
        if pid >= 0 { load_podcast_detail(&w0, pid); }
    });
    let w = window.as_weak();
    window.on_music_podcast_d_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let pages = w0.get_music_podcast_d_pages();
        let next = (w0.get_music_podcast_d_page() + d).clamp(0, (pages - 1).max(0));
        w0.set_music_podcast_d_page(next);
        let pid = cur_podcast_id().lock().map(|g| *g).unwrap_or(-1);
        if pid >= 0 { load_podcast_detail(&w0, pid); }
    });
    // Back from a podcast detail page to the grid.
    let w = window.as_weak();
    window.on_music_podcast_back(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_detail_open(false);
        if let Ok(mut g) = cur_podcast_id().lock() { *g = -1; }
        refresh_podcast_views(&w0);
    });
    // Audiobook play — CHAPTER-level resume only (user decree 2026-07-12): a
    // chapter always starts from 0:00, never a mid-chapter `--start` seek. The
    // progress row only records WHICH chapter is current; in-chapter moments
    // are what bookmarks are for. (The old in-file position leaked into the
    // global music.resume and cut the start of plain songs.)
    let w = window.as_weak();
    window.on_music_audiobook_play(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let id = music_ids().lock().ok()
            .and_then(|g| g.get(pos as usize).copied()).filter(|i| *i >= 0);
        let speed_now = w0.get_music_book_speed() as f64;
        let weak = w0.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            if let Some(id) = id {
                let Ok(pool) = pool_for("music").await else { return; };
                let _ = tulipix_music::audiobooks::mark_audiobook(&pool, id).await;
                // Register this chapter as CURRENT immediately — the red row,
                // card bar + "In progress" tab key off audiobook_progress.
                let _ = tulipix_music::audiobooks::save_progress(&pool, id, 1.0, speed_now).await;
                // Chapter queue for the BookMini + live detail refresh. Derive
                // the book folder from the clicked chapter itself — the
                // detail's cur_book_folder may be unset or on another book.
                let folder = music_paths().lock().ok()
                    .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.display().to_string())))
                    .unwrap_or_default();
                if !folder.is_empty() {
                    let ids = tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default();
                    let (listened, current) = tulipix_music::audiobooks::chapter_states(&pool, &ids).await.unwrap_or_default();
                    let detail_folder = cur_book_folder().lock().map(|g| g.clone()).unwrap_or_default();
                    let wk = weak.clone();
                    let _ = wk.upgrade_in_event_loop(move |w0| {
                        let (rows, _, _, _) = chapter_rows_for(&ids, &listened, current);
                        w0.set_music_book_chapter_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
                        if w0.get_music_audiobook_detail_open() && detail_folder == folder {
                            fill_book_detail(&w0, &folder, &ids, &listened, current);
                        }
                    });
                }
            }
            let _ = weak.upgrade_in_event_loop(move |w0| {
                play_music_at(&w0, pos);
                // Force book mode even when the songs store hasn't warmed yet.
                w0.set_music_player_mode("book".into());
                if !w0.get_music_ab_d_title().is_empty() {
                    w0.set_music_np_sub(w0.get_music_ab_d_title());
                }
                w0.set_music_np_album("".into());
                // Audiobooks have no bottom bar — the mini player IS the player.
                w0.invoke_music_center_mini();
                let speed = w0.get_music_book_speed();
                music_ipc(&["set_property", "audio-pitch-correction", "yes"]);
                music_ipc(&["set_property", "speed", &speed.to_string()]);
            });
        });
    });
    let w = window.as_weak();
    window.on_music_set_book_speed(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let s = tulipix_music::audiobooks::clamp_speed(s as f64);
        w0.set_music_book_speed(s as f32);
        music_ipc(&["set_property", "speed", &s.to_string()]);
        if let Some(id) = current_music_id(&w0) {
            let pos = w0.get_music_pos() as f64;
            // Speed is a property of the BOOK, not of the chapter you happen to
            // be on — a narrator you want at 1.4× stays at 1.4× at chapter 12.
            let ids = ab_open_ids().lock().map(|g| g.clone()).unwrap_or_default();
            let book = if ids.contains(&id) { ids } else { vec![id] };
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("music").await else { return; };
                let _ = tulipix_music::audiobooks::save_progress(&pool, id, pos, s).await;
                let _ = tulipix_music::audiobooks::set_book_speed(&pool, &book, s).await;
            });
        }
    });
    // Chapter step (±1) — mpv chapter property on the live audiobook.
    window.on_music_book_chapter(move |delta| {
        music_ipc(&["add", "chapter", &delta.to_string()]);
    });
    // Skip-silence toggle (np.p5.music.audiobook-chapters) — mpv silenceremove
    // audio filter, trims gaps/dead air during long-form playback.
    let w = window.as_weak();
    window.on_music_book_skip_silence(move || {
        let Some(w0) = w.upgrade() else { return; };
        let on = !w0.get_music_book_skip_silence_on();
        w0.set_music_book_skip_silence_on(on);
        if on {
            // start_periods/start_threshold trim leading silence per segment.
            music_ipc(&["set_property", "af", "lavfi=[silenceremove=start_periods=1:start_threshold=-50dB:stop_periods=-1:stop_threshold=-50dB]"]);
        } else {
            music_ipc(&["set_property", "af", ""]);
        }
    });
    // Bookmark the current position, from the mini players. Unnamed, like every
    // other bookmark — the detail page is where they get named. It lands in the
    // same list the detail panel shows, so the two agree.
    let w = window.as_weak();
    window.on_music_book_bookmark(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        let pos = w0.get_music_pos() as f64;
        let ids = ab_open_ids().lock().map(|g| g.clone()).unwrap_or_default();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::audiobooks::add_bookmark(&pool, id, pos, "").await;
            // Only refresh the panel if the playing book is the open one.
            if ids.contains(&id) {
                let _ = weak.upgrade_in_event_loop(move |w| load_book_detail_bookmarks(&w, ids));
            }
        });
    });
    // ── Book-level bookmarks (detail page) ──────────────────────────────────
    // These act on the OPEN book, which may not be the playing one — the
    // player-panel chips already cover "the chapter I am hearing".
    let w = window.as_weak();
    window.on_music_ab_bm_add(move || {
        let Some(w0) = w.upgrade() else { return; };
        // Bookmark where playback actually is, if this book is the one playing;
        // otherwise the start of the chapter Resume would pick, so the button
        // still does something sensible on a book you are only browsing.
        let ids = ab_open_ids().lock().map(|g| g.clone()).unwrap_or_default();
        let playing = current_music_id(&w0).filter(|id| ids.contains(id));
        let (id, pos) = match playing {
            Some(id) => (id, w0.get_music_pos() as f64),
            None => {
                let idx = w0.get_music_ab_d_resume_index();
                let Some(id) = music_id_at(idx) else { return; };
                (id, 0.0)
            }
        };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::audiobooks::add_bookmark(&pool, id, pos, "").await;
            let _ = weak.upgrade_in_event_loop(move |w| load_book_detail_bookmarks(&w, ids));
        });
    });
    let w = window.as_weak();
    window.on_music_ab_bm_jump(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let Some((id, pos)) = ab_open_marks().lock().ok().and_then(|g| g.get(i as usize).copied()) else { return; };
        // A bookmark can live in a chapter other than the playing one, so the
        // jump is "play that chapter, then seek" rather than a bare seek.
        match current_music_id(&w0) {
            Some(cur) if cur == id => music_ipc(&["seek", &pos.to_string(), "absolute"]),
            _ => {
                let Some(idx) = music_pos_of(id) else { return; };
                w0.invoke_music_audiobook_play(idx);
                // The new mpv needs to exist before it can be seeked.
                let weak = w.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
                    let _ = weak.upgrade_in_event_loop(move |_| {
                        music_ipc(&["seek", &pos.to_string(), "absolute"]);
                    });
                });
            }
        }
    });
    let w = window.as_weak();
    window.on_music_ab_bm_remove(move |i| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some((id, pos)) = ab_open_marks().lock().ok().and_then(|g| g.get(i as usize).copied()) else { return; };
        let ids = ab_open_ids().lock().map(|g| g.clone()).unwrap_or_default();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::audiobooks::remove_bookmark(&pool, id, pos).await;
            let _ = weak.upgrade_in_event_loop(move |w| load_book_detail_bookmarks(&w, ids));
        });
    });
    let w = window.as_weak();
    window.on_music_ab_bm_rename(move |i, label| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some((id, pos)) = ab_open_marks().lock().ok().and_then(|g| g.get(i as usize).copied()) else { return; };
        let ids = ab_open_ids().lock().map(|g| g.clone()).unwrap_or_default();
        let label = label.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::audiobooks::rename_bookmark(&pool, id, pos, &label).await;
            let _ = weak.upgrade_in_event_loop(move |w| load_book_detail_bookmarks(&w, ids));
        });
    });
    // Mark the whole book finished, or reset it for a re-listen.
    let w = window.as_weak();
    window.on_music_ab_set_finished(move |on| {
        let Some(_w0) = w.upgrade() else { return; };
        let ids = ab_open_ids().lock().map(|g| g.clone()).unwrap_or_default();
        if ids.is_empty() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::audiobooks::set_finished(&pool, &ids, on).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_ab_d_finished(on);
                // The card grid's "Finished" filter reads the same flag.
                populate_audiobooks(&w);
            });
        });
    });
    // Open a book (by folder) → build the detail hero + chapter list.
    let w = window.as_weak();
    window.on_music_audiobook_open(move |folder| {
        let Some(_w0) = w.upgrade() else { return; };
        let folder = folder.to_string();
        if let Ok(mut g) = cur_book_folder().lock() { *g = folder.clone(); }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let ids = tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default();
            let (listened, current) = tulipix_music::audiobooks::chapter_states(&pool, &ids).await.unwrap_or_default();
            let _ = weak.upgrade_in_event_loop(move |w| fill_book_detail(&w, &folder, &ids, &listened, current));
        });
    });
    let w = window.as_weak();
    window.on_music_audiobook_back(move || {
        if let Some(w0) = w.upgrade() { w0.set_music_audiobook_detail_open(false); }
    });
    // Audiobooks sub-tab (All / In progress / Finished / Folders) — closes any
    // open book detail and re-filters the card grid.
    let w = window.as_weak();
    window.on_music_ab_set_tab(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        if let Ok(mut g) = ab_tab().lock() { *g = t.to_string(); }
        w0.set_music_ab_tab(t);
        w0.set_music_audiobook_detail_open(false);
        populate_audiobooks(&w0);
    });
    // Custom cover (np.p5.music.audiobook-chapters — own art): file picker →
    // persisted in audiobook_covers; the detail hero + cards refresh at once.
    let w = window.as_weak();
    window.on_music_audiobook_set_cover(move || {
        let folder = cur_book_folder().lock().map(|g| g.clone()).unwrap_or_default();
        audiobook_pick_cover(w.clone(), folder);
    });
    let w = window.as_weak();
    window.on_music_audiobook_set_cover_for(move |folder| {
        audiobook_pick_cover(w.clone(), folder.to_string());
    });
    // Reveal an audiobook folder in the system file manager (Folders tab menu).
    window.on_music_ab_folder_find(move |folder| {
        open_in_default_app(std::path::Path::new(folder.as_str()));
    });
    // Re-fetch every subscribed podcast feed (np.p5.music.podcast-feeds auto-refresh).
    let w = window.as_weak();
    window.on_music_podcast_refresh(move || {
        let Some(w0) = w.upgrade() else { return; };
        if w0.get_music_podcast_refresh_busy() { return; }
        w0.set_music_podcast_refresh_busy(true);
        w0.set_music_podcast_refresh_frac(0.0);
        w0.set_music_podcast_refresh_status("↻ Starting…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_podcast_refresh_busy(false));
                return;
            };
            let feeds: Vec<(i64, String)> = sqlx::query_as("SELECT id, feed_url FROM podcasts")
                .fetch_all(&pool).await.unwrap_or_default();
            let n = feeds.len().max(1);
            let client = tulipix_core::net::http().clone();
            let mut total_added: i64 = 0;
            for (i, (pid, url)) in feeds.iter().enumerate() {
                let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?")
                    .bind(pid).fetch_one(&pool).await.unwrap_or(0);
                if let Ok(resp) = client.get(url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
                    if let Ok(xml) = resp.text().await {
                        let feed = tulipix_music::podcasts::parse_feed(&xml);
                        let prog = weak.clone();
                        let base = i;
                        let _ = tulipix_music::podcasts::subscribe_with_progress(&pool, url, &feed, move |done, total| {
                            let frac = ((base as f32) + (done as f32 / total.max(1) as f32)) / n as f32;
                            let p = prog.clone();
                            let _ = p.upgrade_in_event_loop(move |w| {
                                w.set_music_podcast_refresh_frac(frac);
                                w.set_music_podcast_refresh_status(format!("↻ {}/{}", base + 1, n).into());
                            });
                        }).await;
                    }
                }
                let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?")
                    .bind(pid).fetch_one(&pool).await.unwrap_or(0);
                total_added += (after - before).max(0);
                let (frac, ii, added) = ((i + 1) as f32 / n as f32, i + 1, total_added);
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_music_podcast_refresh_frac(frac);
                    w.set_music_podcast_refresh_status(format!("↻ {ii}/{n} · +{added}").into());
                });
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_podcast_refresh_busy(false);
                w.set_music_podcast_refresh_frac(0.0);
                w.set_music_podcast_refresh_status("".into());
                populate_podcasts(&w);
                populate_podcast_latest(&w);
                populate_podcast_downloads(&w);
            });
        });
    });
    // Fetch artist art + bio from MusicBrainz / Cover Art Archive (np.p5.music.art-bio).
    let w = window.as_weak();
    window.on_music_fetch_art_bio(move || {
        let Some(w0) = w.upgrade() else { return; };
        let artist = w0.get_music_np_sub().to_string();
        if artist.is_empty() || artist == "Playing from your library" {
            w0.set_music_artist_bio("Current track has no artist tag — edit it first.".into());
            return;
        }
        let title = w0.get_music_np_title().to_string();
        let id = current_music_id(&w0);
        w0.set_music_artist_bio("Fetching…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let blurb = match tulipix_music::musicbrainz::lookup_artist(&client, &artist).await {
                Ok(s) => s.artists.iter().max_by_key(|a| a.score)
                    .map(tulipix_music::musicbrainz::artist_blurb)
                    .unwrap_or_else(|| format!("No MusicBrainz match for “{artist}”.")),
                Err(_) => "Lookup failed — check your network / endpoints in Settings.".to_string(),
            };
            // Cover Art Archive fetch for the missing album art (np.p5.music.art-bio).
            let mut cover_note = String::new();
            if let Some(id) = id {
                if let Ok(pool) = pool_for("music").await {
                    let album: Option<(i64, Option<String>)> = sqlx::query_as(
                        "SELECT al.id, al.cover_path FROM track_meta tm
                         JOIN albums al ON al.id = tm.album_id WHERE tm.item_id = ?")
                        .bind(id).fetch_optional(&pool).await.ok().flatten();
                    if let Some((album_id, existing)) = album {
                        let has_cover = existing.as_deref().map(|p| std::path::Path::new(p).exists()).unwrap_or(false);
                        if !has_cover && !title.is_empty() {
                            if let Ok(rec) = tulipix_music::musicbrainz::lookup_recording(&client, &artist, &title).await {
                                if let Some(url) = tulipix_music::musicbrainz::best_cover_url(&rec) {
                                    if let Ok(resp) = client.get(&url)
                                        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                                        .send().await {
                                        if resp.status().is_success() {
                                            if let Ok(bytes) = resp.bytes().await {
                                                let dir = tulipix_core::paths::cache_dir()
                                                    .unwrap_or_else(std::env::temp_dir).join("music_covers");
                                                let _ = std::fs::create_dir_all(&dir);
                                                let dest = dir.join(format!("album-{album_id}.jpg"));
                                                if std::fs::write(&dest, &bytes).is_ok() {
                                                    let _ = sqlx::query("UPDATE albums SET cover_path = ? WHERE id = ?")
                                                        .bind(dest.to_string_lossy().as_ref()).bind(album_id).execute(&pool).await;
                                                    cover_note = "\n\n✓ Album cover fetched.".into();
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let blurb = format!("{blurb}{cover_note}");
            let refresh = !cover_note.is_empty();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_artist_bio(blurb.into());
                if refresh { populate_music_views(w.as_weak()); }
            });
        });
    });
    // Open the full-screen player — refresh synced lyrics + queue first.
    let w = window.as_weak();
    window.on_music_open_fullscreen(move || {
        let Some(w) = w.upgrade() else { return; };
        load_music_lyrics(&w);
        build_music_queue(&w);
        w.set_music_fullscreen(true);
    });
    // Toggle the app-wide mini-player — refresh the queue when opening. A
    // pinned (bubbled) mini always RESTORES instead of toggling away, so the
    // header pip in radio/podcasts/audiobooks reliably brings the player back.
    let w = window.as_weak();
    window.on_music_open_mini(move || {
        let Some(w) = w.upgrade() else { return; };
        if w.get_music_mini_bubble() {
            w.set_music_mini_bubble(false);
            w.set_music_mini_open(true);
            build_music_queue(&w); load_music_lyrics(&w);
            return;
        }
        let open = !w.get_music_mini_open();
        w.set_music_mini_open(open);
        if open { build_music_queue(&w); load_music_lyrics(&w); }
    });
}

fn wire_music_metadata(window: &MainWindow) {
    // ── Metadata manager (np.p5.atmusic.metadata-manager) ──
    let w = window.as_weak();
    window.on_music_open_meta_manager(move || {
        let Some(w0) = w.upgrade() else { return; };
        build_meta_rows(&w0);
        w0.set_music_meta_mgr_page(0);
        w0.set_music_meta_mgr_open(true);
        publish_meta_page(&w0);
    });
    let w = window.as_weak();
    window.on_music_meta_mgr_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let pages = w0.get_music_meta_mgr_pages().max(1);
        let next = (w0.get_music_meta_mgr_page() + d).clamp(0, pages - 1);
        w0.set_music_meta_mgr_page(next);
        publish_meta_page(&w0);
    });
    // Stat-card filter (all | tagged | missing) — reset to page 0, republish.
    let w = window.as_weak();
    window.on_music_meta_mgr_set_filter(move |f| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_meta_mgr_filter(f);
        w0.set_music_meta_mgr_page(0);
        publish_meta_page(&w0);
    });
    // Edit a song's tags straight from the metadata manager (reuses the tag editor).
    let w = window.as_weak();
    window.on_music_meta_mgr_edit(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        w0.invoke_music_song_edit_media(pos);
    });
    // Fetch a single song's metadata now.
    let w = window.as_weak();
    window.on_music_meta_fetch_one(move |pos| {
        let Some(_w0) = w.upgrade() else { return; };
        let item_id = match music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) { Some(id) => id, None => return };
        let stem = music_paths().lock().ok()
            .and_then(|g| g.get(pos as usize).cloned())
            .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string()))
            .unwrap_or_default();
        if stem.is_empty() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let client = tulipix_core::net::http().clone();
            let res = fetch_and_store_meta(&pool, &client, item_id, &stem).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                match res {
                    Ok(Some(f)) => update_meta_row(&w, pos, "Matched", Some(&f)),
                    Ok(None)    => update_meta_row(&w, pos, "No match", None),
                    Err(_)      => update_meta_row(&w, pos, "Error", None),
                }
                populate_music_views(w.as_weak());
            });
        });
    });
    // Fetch every song's metadata (rate-limited to respect MusicBrainz's 1 req/s).
    let w = window.as_weak();
    window.on_music_meta_fetch_all(move || {
        let Some(w0) = w.upgrade() else { return; };
        if w0.get_music_meta_fetch_status().ends_with('%') { return; } // already running
        w0.set_music_meta_fetch_status("0%".into());
        w0.set_music_meta_fetch_progress(0.0);
        let jobs: Vec<(i32, i64, String)> = {
            let songs = music_songs().lock().map(|g| g.clone()).unwrap_or_default();
            let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
            let ids = music_ids().lock().map(|g| g.clone()).unwrap_or_default();
            songs.iter().filter_map(|s| {
                let item_id = *ids.get(s.pos as usize)?;
                let stem = paths.get(s.pos as usize)
                    .and_then(|p| p.file_stem()).and_then(|x| x.to_str()).unwrap_or("").to_string();
                if stem.is_empty() { None } else { Some((s.pos, item_id, stem)) }
            }).collect()
        };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let client = tulipix_core::net::http().clone();
            let total = jobs.len().max(1);
            for (i, (pos, item_id, stem)) in jobs.into_iter().enumerate() {
                let res = fetch_and_store_meta(&pool, &client, item_id, &stem).await;
                let frac = (i + 1) as f32 / total as f32;
                let _ = weak.upgrade_in_event_loop(move |w| {
                    match res {
                        Ok(Some(f)) => update_meta_row(&w, pos, "Matched", Some(&f)),
                        Ok(None)    => update_meta_row(&w, pos, "No match", None),
                        Err(_)      => update_meta_row(&w, pos, "Error", None),
                    }
                    w.set_music_meta_fetch_progress(frac);
                    w.set_music_meta_fetch_status(format!("{}%", (frac * 100.0) as i32).into());
                });
                tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_music_meta_fetch_status("".into());
                w.set_music_meta_fetch_progress(0.0);
                populate_music_views(w.as_weak());
            });
        });
    });
    // Batch lyrics sync — fetch LRCLIB lyrics for every library track missing them.
    let w = window.as_weak();
    window.on_music_sync_all_lyrics(move || {
        let Some(w0) = w.upgrade() else { return; };
        if w0.get_music_lyrics_sync_status().ends_with('%') { return; } // already running
        w0.set_music_lyrics_sync_status("0%".into());
        w0.set_music_lyrics_sync_progress(0.0);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let rows: Vec<(i64, Option<String>, Option<String>, Option<String>, f64)> = sqlx::query_as(
                "SELECT tm.item_id, tm.title, ar.name, al.title, COALESCE(tm.duration_s, 0)
                 FROM track_meta tm
                 LEFT JOIN artists ar ON ar.id = tm.artist_id
                 LEFT JOIN albums  al ON al.id = tm.album_id
                 LEFT JOIN lyrics  ly ON ly.item_id = tm.item_id
                 WHERE (ly.content IS NULL OR ly.content = '') AND tm.title IS NOT NULL AND tm.title <> ''")
                .fetch_all(&pool).await.unwrap_or_default();
            let total = rows.len();
            if total == 0 { let _ = weak.upgrade_in_event_loop(|w| w.set_music_lyrics_sync_status("All synced".into())); return; }
            // Per-request timeout so one hung connection can't stall the batch.
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(12))
                .build().unwrap_or_default();
            // Bounded parallelism: up to 8 LRCLIB lookups in flight at once (≈one
            // batch of work where the old loop did one-at-a-time + 150ms sleeps).
            let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
            let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let found = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let mut set = tokio::task::JoinSet::new();
            for (id, title, artist, album, dur) in rows {
                let title = title.unwrap_or_default();
                let artist = artist.unwrap_or_default();
                let album = album.unwrap_or_default();
                let (pool, client) = (pool.clone(), client.clone());
                let (sem, done, found, weak) = (sem.clone(), done.clone(), found.clone(), weak.clone());
                set.spawn(async move {
                    let _permit = sem.acquire().await;
                    if !title.is_empty() {
                        let url = tulipix_music::lyrics::get_url(&artist, &title, &album, dur);
                        // Up to 2 attempts: retry once on a transport error or 429.
                        for attempt in 0..2u32 {
                            match client.get(&url)
                                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                                .send().await {
                                Ok(resp) => {
                                    if resp.status().as_u16() == 429 {
                                        tokio::time::sleep(std::time::Duration::from_millis(400 * (attempt as u64 + 1))).await;
                                        continue;
                                    }
                                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                                        let synced = json["syncedLyrics"].as_str().unwrap_or("");
                                        let plain = json["plainLyrics"].as_str().unwrap_or("");
                                        if !synced.is_empty() {
                                            let _ = tulipix_music::lyrics::store(&pool, id, synced, true, "lrclib").await;
                                            found.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        } else if !plain.is_empty() {
                                            let _ = tulipix_music::lyrics::store(&pool, id, plain, false, "lrclib").await;
                                            found.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        }
                                    }
                                    break;
                                }
                                Err(_) => { tokio::time::sleep(std::time::Duration::from_millis(300)).await; }
                            }
                        }
                    }
                    let d = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                    if d % 4 == 0 || d == total {
                        let pct = d * 100 / total;
                        let frac = d as f32 / total as f32;
                        let _ = weak.upgrade_in_event_loop(move |w| {
                            w.set_music_lyrics_sync_status(format!("{pct}%").into());
                            w.set_music_lyrics_sync_progress(frac);
                        });
                    }
                });
            }
            while set.join_next().await.is_some() {}
            let found = found.load(std::sync::atomic::Ordering::Relaxed);
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_lyrics_sync_status(format!("{found} found").into());
                w.set_music_lyrics_sync_progress(1.0);
                // Re-derive synced flags from the DB so every grid pill updates live.
                populate_music_views(w.as_weak());
                if w.get_music_lyrics_mgr_open() { rebuild_lyrics_manager(&w); }
            });
        });
    });
    // Songs list sort (toggles direction when the same column is re-picked).
    let w = window.as_weak();
    window.on_music_set_song_sort(move |s| {
        let Some(w) = w.upgrade() else { return; };
        let s = s.to_string();
        let dir = if w.get_music_song_sort() == s.as_str() {
            if w.get_music_song_dir() == "asc" { "desc" } else { "asc" }
        } else if s == "title" || s == "artist" { "asc" } else { "desc" };
        w.set_music_song_sort(s.clone().into());
        w.set_music_song_dir(dir.into());
        sort_music_songs(&s, dir);
        w.set_music_song_page(0);
        rebuild_music_songs_page(&w);
    });
    // Songs list pagination (delta −1 / +1, clamped).
    let w = window.as_weak();
    window.on_music_set_song_page(move |delta| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_song_pages().max(1);
        let next = (w.get_music_song_page() + delta).clamp(0, pages - 1);
        w.set_music_song_page(next);
        rebuild_music_songs_page(&w);
    });
    // Music search — filter the Songs list (title/artist substring).
    let w = window.as_weak();
    window.on_music_search(move |q| {
        let Some(w) = w.upgrade() else { return; };
        if let Ok(mut g) = music_query_filter().lock() { *g = q.to_string(); }
        // Typing a query jumps off Home (nothing to search there) into Songs.
        if !q.trim().is_empty() && w.get_music_lib_tab() == "home" { w.set_music_lib_tab("songs".into()); }
        // Search acts on whatever the current My Music sub-tab shows.
        w.set_music_song_page(0);
        rebuild_music_songs_page(&w);
        rebuild_grouped_search(&w, q.as_str()); // np.p5.atmusic.lib-grouped-search
        // Search acts on whatever sub-tab is showing — filter that tab's view.
        let tab = w.get_music_lib_tab().to_string();
        match tab.as_str() {
            "albums" | "artists" | "genres" | "folders" | "playlists" => {
                w.set_music_browse_page(0);
                rebuild_browse_tab(&w, &tab);
            }
            "favorites" => { w.set_music_fav_page(0); rebuild_fav_page(&w); }
            "history" => { rebuild_history_page(&w); }
            _ => {}
        }
    });
    // Songs list/grid view toggle (thumb styling).
    let w = window.as_weak();
    window.on_music_set_songs_view(move |v| {
        if let Some(w) = w.upgrade() { w.set_music_songs_view(v); }
    });
    // Songs grid density slider (np.p4.music.grid-density) — clamp 100..300,
    // live re-flow via the bound property, persisted across relaunch.
    let w = window.as_weak();
    window.on_music_set_grid_density(move |px| {
        let Some(w) = w.upgrade() else { return; };
        let px = tulipix_music::grid_density::clamp_target(px as f64);
        w.set_music_grid_density(px as f32);
        save_music_pref("music.grid_density", &format!("{px:.0}"));
    });
    // Home "Recently played" pager.
    let w = window.as_weak();
    window.on_music_set_recent_page(move |delta| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_recent_pages().max(1);
        w.set_music_recent_page((w.get_music_recent_page() + delta).clamp(0, pages - 1));
        rebuild_recent_page(&w);
    });
    // Browse sort (albums/artists/genres/folders/playlists) — name|count, toggles dir.
    let w = window.as_weak();
    window.on_music_set_browse_sort(move |s| {
        let Some(w) = w.upgrade() else { return; };
        let s = s.to_string();
        let dir = if w.get_music_browse_sort() == s.as_str() {
            if w.get_music_browse_dir() == "asc" { "desc" } else { "asc" }
        } else if s == "count" { "desc" } else { "asc" };
        w.set_music_browse_sort(s.into());
        w.set_music_browse_dir(dir.into());
        w.set_music_browse_page(0);
        rebuild_browse_tab(&w, &w.get_music_lib_tab());
    });
    // Albums/Artists pagination (delta −1 / +1, clamped to page count).
    let w = window.as_weak();
    window.on_music_set_browse_page(move |delta| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_browse_pages().max(1);
        let next = (w.get_music_browse_page() + delta).clamp(0, pages - 1);
        w.set_music_browse_page(next);
        rebuild_browse_tab(&w, &w.get_music_lib_tab());
    });
    // Favorites pagination (20/page).
    let w = window.as_weak();
    window.on_music_set_fav_page(move |delta| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_fav_pages().max(1);
        let next = (w.get_music_fav_page() + delta).clamp(0, pages - 1);
        w.set_music_fav_page(next);
        rebuild_fav_page(&w);
    });
    // Shuffle toggle (affects Next).
    let w = window.as_weak();
    window.on_music_toggle_shuffle(move || {
        if let Some(w) = w.upgrade() {
            w.set_music_shuffle(!w.get_music_shuffle());
            shuffle_reset(); // fresh order + history on every toggle
        }
    });
    // AT parity — live thumbnail-size slider (np.p5.atmusic.thumb-slider).
    let w = window.as_weak();
    window.on_music_set_thumb_size(move |v| {
        let Some(w) = w.upgrade() else { return; };
        let px = v.clamp(100.0, 300.0);
        w.set_music_thumb_size(px);
        save_music_pref("music.thumb_size", &format!("{px:.0}"));
    });
    // AT parity — Play All / Shuffle All the library (np.p5.atmusic.lib-playall).
    let w = window.as_weak();
    window.on_music_play_all(move || {
        if let Some(w) = w.upgrade() {
            w.set_music_shuffle(false);
            if w.get_music_np_total() > 0 || music_paths().lock().map(|g| !g.is_empty()).unwrap_or(false) {
                clear_context_queue(&w);   // the library IS the list now
                play_music_at(&w, 0);
            }
        }
    });
    let w = window.as_weak();
    window.on_music_shuffle_all(move || {
        let Some(w) = w.upgrade() else { return; };
        let total = music_paths().lock().map(|g| g.len()).unwrap_or(0);
        if total == 0 { return; }
        w.set_music_shuffle(true);
        let start = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos()).unwrap_or(0) as usize) % total;
        clear_context_queue(&w);           // the library IS the list now
        play_music_at(&w, start as i32);
    });
    // AT parity — jump to a 1-based page in the Songs list (np.p5.atmusic.lib-jump-page).
    let w = window.as_weak();
    window.on_music_jump_page(move |p| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_song_pages().max(1);
        let next = (p - 1).clamp(0, pages - 1);
        w.set_music_song_page(next);
        w.set_music_jump_text("".into());
        rebuild_music_songs_page(&w);
    });
    // History pagination — 30 plays / page, up to 5 pages (np.p5.atmusic.history-page).
    let w = window.as_weak();
    window.on_music_set_history_page(move |d| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_history_pages().max(1);
        let next = (w.get_music_history_page() + d).clamp(1, pages);
        w.set_music_history_page(next);
        rebuild_history_page(&w);
    });
    // AT parity — clear the playback history (np.p5.atmusic.history-page).
    let w = window.as_weak();
    window.on_music_clear_history(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_history_page(1);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("DELETE FROM play_history").execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| { populate_history(&w); populate_music_views(w.as_weak()); });
        });
    });
    // AT parity — Album / Artist detail (np.p5.atmusic.album-detail / .artist-detail).
    // Tiles carry their first track's playback position; resolve it to the
    // album_id / artist_id via the live track_meta, then open the overlay.
    let w = window.as_weak();
    window.on_music_album_open(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT album_id FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = weak.upgrade_in_event_loop(move |w| open_album_detail(&w, aid));
            }
        });
        let _ = w0;
    });
    let w = window.as_weak();
    window.on_music_artist_open(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT artist_id FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = weak.upgrade_in_event_loop(move |w| open_artist_detail(&w, aid));
            }
        });
        let _ = w0;
    });
    // Favorite an album / artist (heart on its browse thumb) — np.p5.music.fav-collections.
    let w = window.as_weak();
    window.on_music_album_fav(move |pos| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT album_id FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = sqlx::query("UPDATE albums SET loved = CASE COALESCE(loved,0) WHEN 1 THEN 0 ELSE 1 END WHERE id = ?")
                    .bind(aid).execute(&pool).await;
                let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
            }
        });
    });
    let w = window.as_weak();
    window.on_music_artist_fav(move |pos| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE artists ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT artist_id FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = sqlx::query("UPDATE artists SET loved = CASE COALESCE(loved,0) WHEN 1 THEN 0 ELSE 1 END WHERE id = ?")
                    .bind(aid).execute(&pool).await;
                let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
            }
        });
    });
    // Rate an album / artist (stars on its browse thumb).
    let w = window.as_weak();
    window.on_music_album_rate(move |pos, n| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT album_id FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = sqlx::query("UPDATE albums SET rating = ? WHERE id = ?").bind(n as i64).bind(aid).execute(&pool).await;
                let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
            }
        });
    });
    let w = window.as_weak();
    window.on_music_artist_rate(move |pos, n| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE artists ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT artist_id FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = sqlx::query("UPDATE artists SET rating = ? WHERE id = ?").bind(n as i64).bind(aid).execute(&pool).await;
                let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
            }
        });
    });
    // Favourite / rate the open album or artist (info-box buttons).
    let w = window.as_weak();
    window.on_music_detail_fav(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let (kind, id, _) = music_detail().lock().map(|g| g.clone()).unwrap_or_default();
        if id < 0 || (kind != "album" && kind != "artist") { return; }
        let table = if kind == "album" { "albums" } else { "artists" };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN loved INTEGER NOT NULL DEFAULT 0")).execute(&pool).await;
            let _ = sqlx::query(&format!("UPDATE {table} SET loved = CASE COALESCE(loved,0) WHEN 1 THEN 0 ELSE 1 END WHERE id = ?"))
                .bind(id).execute(&pool).await;
            let loved: i64 = sqlx::query_scalar(&format!("SELECT COALESCE(loved,0) FROM {table} WHERE id = ?"))
                .bind(id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
            let _ = weak.upgrade_in_event_loop(move |w| { w.set_music_detail_loved(loved != 0); populate_music_views(w.as_weak()); });
        });
    });
    let w = window.as_weak();
    window.on_music_detail_rate(move |n| {
        let Some(w0) = w.upgrade() else { return; };
        let (kind, id, _) = music_detail().lock().map(|g| g.clone()).unwrap_or_default();
        if id < 0 || (kind != "album" && kind != "artist") { return; }
        let table = if kind == "album" { "albums" } else { "artists" };
        w0.set_music_detail_stars(n);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query(&format!("ALTER TABLE {table} ADD COLUMN rating INTEGER NOT NULL DEFAULT 0")).execute(&pool).await;
            let _ = sqlx::query(&format!("UPDATE {table} SET rating = ? WHERE id = ?")).bind(n as i64).bind(id).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
        });
    });
    // Genre detail — open the genre's page (list) before any playback.
    let w = window.as_weak();
    window.on_music_genre_open(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if let Some(genre) = sqlx::query_scalar::<_, Option<String>>("SELECT genre FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten() {
                if !genre.is_empty() { let _ = weak.upgrade_in_event_loop(move |w| open_genre_detail(&w, genre)); }
            }
        });
        let _ = w0;
    });
    // Right-click a genre tile on the grid → pick a cover image for that genre.
    let w = window.as_weak();
    window.on_music_genre_set_art(move |pos| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(item_id) = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied()) else { return; };
        let Some(file) = rfd::FileDialog::new().set_title("Choose genre cover")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let genre: Option<String> = sqlx::query_scalar("SELECT genre FROM track_meta WHERE item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten();
            let Some(genre) = genre.filter(|g| !g.is_empty()) else { return; };
            let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("music_covers");
            let _ = std::fs::create_dir_all(&dir);
            let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
            let safe: String = genre.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
            let dest = dir.join(format!("genre-{safe}.{ext}"));
            if std::fs::copy(&file, &dest).is_err() { return; }
            let dest_s = dest.to_string_lossy().to_string();
            let _ = weak.upgrade_in_event_loop(move |w| {
                save_music_pref(&format!("music.genre.cover.{genre}"), &dest_s);
                populate_music_views(w.as_weak());
            });
        });
    });
    // AT parity — remove a scanned root (np.p5.atmusic.lib-folder-mgmt): forget it,
    // re-scan the rest, refresh the roots list + library views.
    let w = window.as_weak();
    window.on_music_folder_remove_root(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let roots = load_watched_folders();
        let Some(path) = roots.get(i as usize).cloned() else { return; };
        forget_watched_folder(&w0, &path.to_string_lossy());
        // Drop the removed root's tracks from the accumulator so they vanish
        // without waiting for a rescan of the remaining roots.
        prune_music_full_under(&path);
        rebuild_music_tiles(&w0);
        for p in load_watched_folders() { if p.exists() { add_folder_path(&w0, p); } }
        populate_folder_roots(&w0);
        populate_music_views(w0.as_weak());
    });
    // Folder → its own songs page (detail overlay listing that folder's tracks).
    let w = window.as_weak();
    window.on_music_folder_open(move |pos| {
        if let Some(w0) = w.upgrade() { open_folder_detail(&w0, pos); }
    });
    // Right-click a folder → remove its tracks from the music library.
    let w = window.as_weak();
    window.on_music_folder_remove(move |pos| {
        let Some(_w0) = w.upgrade() else { return; };
        let Some(dir) = music_paths().lock().ok()
            .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.to_path_buf()))) else { return; };
        let ids = folder_track_ids(&dir);
        if ids.is_empty() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            for id in &ids {
                let _ = sqlx::query("DELETE FROM items WHERE id = ?").bind(id).execute(&pool).await;
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                prune_music_full_under(&dir);
                rebuild_music_tiles(&w);
                populate_music_views(w.as_weak());
            });
        });
    });
    // Right-click a folder → reveal it in the OS file manager.
    let w = window.as_weak();
    window.on_music_folder_find_system(move |pos| {
        let _ = w;
        let Some(dir) = music_paths().lock().ok()
            .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.to_path_buf()))) else { return; };
        open_in_default_app(&dir);
    });
    // Reassign a folder to the next of the 5 music sections (np.p5.atmusic.folder-sections).
    let w = window.as_weak();
    window.on_music_folder_cycle_section(move |pos| {
        // Upgraded only to prove the window is still alive; the repaint happens
        // back on the loop once the flag has actually been written.
        let Some(_w0) = w.upgrade() else { return; };
        let Some(folder) = music_paths().lock().ok()
            .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.display().to_string()))) else { return; };
        let next = cycle_folder_section(&folder);
        tracing::info!(folder = %folder, section = %next, "music folder section reassigned");
        // The tag alone moves nothing: `is_audiobook` is what every My Music
        // query filters on. Without this, cycling INTO Audiobooks left the
        // chapters in the songs list, and cycling back OUT of it never returned
        // them -- the folder was tagged My Music and still invisible there.
        let aud = next == "audiobooks";
        let folder2 = folder.clone();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("music").await {
                let _ = tulipix_music::audiobooks::set_folder_flag(&pool, &folder2, aud).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                populate_music_views(w.as_weak());
                populate_audiobooks(&w);
            });
        });
    });
    // Player redesign — sleep timer set to a chosen interval (np.p4.music.sleep-timer).
    let w = window.as_weak();
    window.on_music_set_sleep(move |min| { arm_sleep_timer(&w, min); });
    // Zen player — true OS fullscreen (no title bar) on enter, restore on exit.
    let w = window.as_weak();
    window.on_music_enter_zen(move || { if let Some(w) = w.upgrade() { w.window().set_fullscreen(true); } });
    let w = window.as_weak();
    window.on_music_exit_zen(move || { if let Some(w) = w.upgrade() { w.window().set_fullscreen(false); } });
    // Click the now-playing title/artist anywhere in the player → open that page.
    let w = window.as_weak();
    window.on_music_open_now_album(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        if w0.get_music_fullscreen() { w0.set_music_fullscreen(false); w0.window().set_fullscreen(false); }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT album_id FROM track_meta WHERE item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = weak.upgrade_in_event_loop(move |w| open_album_detail(&w, aid));
            }
        });
    });
    let w = window.as_weak();
    window.on_music_open_now_artist(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        if w0.get_music_fullscreen() { w0.set_music_fullscreen(false); w0.window().set_fullscreen(false); }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT artist_id FROM track_meta WHERE item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = weak.upgrade_in_event_loop(move |w| open_artist_detail(&w, aid));
            }
        });
    });
    // AT parity — manual LRCLIB search/picker (np.p5.atmusic.lyrics-search-modal).
    let w = window.as_weak();
    window.on_music_lyrics_search(move || {
        let Some(w0) = w.upgrade() else { return; };
        // Default the query to the current track's "Artist Title" when empty.
        let mut q = w0.get_music_lyrics_search_query().to_string();
        if q.trim().is_empty() {
            let title = w0.get_music_np_title().to_string();
            let artist = w0.get_music_np_sub().to_string();
            let artist = if artist == "Playing from your library" { String::new() } else { artist };
            q = format!("{artist} {title}").trim().to_string();
            w0.set_music_lyrics_search_query(q.clone().into());
        }
        if q.trim().is_empty() { return; }
        w0.set_music_lyrics_search_status("Searching LRCLIB…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let url = format!("{}/search?q={}", tulipix_music::lyrics::LRCLIB_BASE,
                q.trim().replace(' ', "+"));
            let client = tulipix_core::net::http().clone();
            let results: Vec<serde_json::Value> = match client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
                Ok(r) => r.json().await.unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            let mut labels: Vec<slint::SharedString> = Vec::new();
            let mut store: Vec<(String, bool)> = Vec::new();
            for r in results.iter().take(25) {
                let tn = r["trackName"].as_str().unwrap_or("");
                let an = r["artistName"].as_str().unwrap_or("");
                let al = r["albumName"].as_str().unwrap_or("");
                let synced = r["syncedLyrics"].as_str().unwrap_or("");
                let plain = r["plainLyrics"].as_str().unwrap_or("");
                let (content, is_sync) = if !synced.is_empty() { (synced, true) } else { (plain, false) };
                if content.is_empty() { continue; }
                let tag = if is_sync { "🎵 synced" } else { "📄 plain" };
                labels.push(format!("{tag}  ·  {an} – {tn}{}", if al.is_empty() { String::new() } else { format!("  ({al})") }).into());
                store.push((content.to_string(), is_sync));
            }
            let n = labels.len();
            if let Ok(mut g) = lyrics_search_store().lock() { *g = store; }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_lyrics_search_results(slint::ModelRc::new(slint::VecModel::from(labels)));
                w.set_music_lyrics_search_status(if n == 0 { "No matches — try a different query.".into() } else { format!("{n} matches").into() });
            });
        });
    });
    let w = window.as_weak();
    window.on_music_lyrics_search_pick(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        let Some((content, synced)) = lyrics_search_store().lock().ok().and_then(|g| g.get(i as usize).cloned()) else { return; };
        w0.set_music_lyrics_search_open(false);
        w0.set_music_player_panel("lyrics".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("music").await {
                let _ = tulipix_music::lyrics::store(&pool, id, &content, synced, "lrclib-manual").await;
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                load_music_lyrics(&w);
                // Real-time: flag the song as synced in the list + repaint.
                if let Ok(mut g) = music_songs().lock() {
                    if let Some(s) = g.iter_mut().find(|s| s.item_id == id) { s.synced = synced; }
                }
                rebuild_music_songs_page(&w);
            });
        });
    });
    // AT parity — dockable side panel (np.p5.atmusic.sidebar-panels): set the
    // panel + populate its content (queue / lyrics) on open.
    let w = window.as_weak();
    window.on_music_side_panel_show(move |which| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_side_panel(which.clone());
        match which.as_str() {
            "queue" => build_music_queue(&w0),
            "lyrics" => load_music_lyrics(&w0),
            _ => {}
        }
    });
    let w = window.as_weak();
    window.on_music_detail_close(move || { if let Some(w) = w.upgrade() { w.set_music_detail_open(false); } });
    // Album page → open the album's artist (click the artist name in the info box).
    let w = window.as_weak();
    window.on_music_detail_open_artist(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let ids = music_detail().lock().map(|g| g.2.clone()).unwrap_or_default();
        let Some(first) = ids.first().copied() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if let Some(aid) = sqlx::query_scalar::<_, Option<i64>>("SELECT artist_id FROM track_meta WHERE item_id = ?")
                .bind(first).fetch_optional(&pool).await.ok().flatten().flatten() {
                let _ = weak.upgrade_in_event_loop(move |w| open_artist_detail(&w, aid));
            }
        });
    });
    let w = window.as_weak();
    window.on_music_set_detail_page(move |delta| {
        let Some(w) = w.upgrade() else { return; };
        let pages = w.get_music_detail_pages().max(1);
        let next = (w.get_music_detail_page() + delta).clamp(0, pages - 1);
        w.set_music_detail_page(next);
        publish_detail_page(&w);
    });
    // Sort the open detail's tracks by an attribute (title/artist/duration).
    let w = window.as_weak();
    window.on_music_set_detail_sort(move |s| {
        let Some(w) = w.upgrade() else { return; };
        let s = s.to_string();
        let dir = if w.get_music_detail_sort() == s.as_str() {
            if w.get_music_detail_sort_dir() == "asc" { "desc" } else { "asc" }
        } else { "asc" };
        w.set_music_detail_sort(s.clone().into());
        w.set_music_detail_sort_dir(dir.into());
        let dur_secs = |d: &str| -> i64 {
            let mut it = d.split(':').filter_map(|x| x.trim().parse::<i64>().ok());
            match (it.next(), it.next()) { (Some(m), Some(sec)) => m * 60 + sec, (Some(sec), None) => sec, _ => 0 }
        };
        DETAIL_ROWS.with(|r| {
            let mut v = r.borrow_mut();
            match s.as_str() {
                "title"    => v.sort_by_key(|a| a.title.to_lowercase()),
                "artist"   => v.sort_by_key(|a| a.artist.to_lowercase()),
                "duration" => v.sort_by_key(|a| dur_secs(a.duration.as_str())),
                "album"    => {
                    let alb_of: std::collections::HashMap<i32, String> = music_songs().lock()
                        .map(|g| g.iter().map(|s| (s.pos, s.album.to_lowercase())).collect()).unwrap_or_default();
                    v.sort_by(|a, b| alb_of.get(&a.index).cmp(&alb_of.get(&b.index)));
                }
                _ => {}
            }
            if dir == "desc" { v.reverse(); }
        });
        w.set_music_detail_page(0);
        publish_detail_page(&w);
    });
    let w = window.as_weak();
    window.on_music_detail_play_all(move || { if let Some(w) = w.upgrade() { detail_play(&w, false); } });
    let w = window.as_weak();
    window.on_music_detail_shuffle(move || { if let Some(w) = w.upgrade() { detail_play(&w, true); } });
    let w = window.as_weak();
    window.on_music_detail_play_track(move |pos| {
        let Some(w) = w.upgrade() else { return; };
        // A track inside an album / artist / genre / folder detail plays that
        // detail and nothing else: the queue becomes its remaining tracks, in
        // the order the list shows them.
        let ids = music_detail().lock().map(|g| g.2.clone()).unwrap_or_default();
        match music_id_at(pos) {
            Some(id) if ids.contains(&id) => {
                set_ctx_pending();
                play_music_at(&w, pos);
                set_context_queue(&w, ids, Some(id), "detail");
            }
            _ => play_music_at(&w, pos),
        }
    });
    // Fetch the album cover from Cover Art Archive for the open album detail.
    let w = window.as_weak();
    window.on_music_detail_fetch_meta(move || {
        let Some(w0) = w.upgrade() else { return; };
        let (kind, album_id, _) = music_detail().lock().map(|g| g.clone()).unwrap_or_default();
        if kind != "album" || album_id < 0 { return; }
        w0.set_music_detail_status("Fetching cover…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let info: Option<(String, Option<String>)> = sqlx::query_as(
                "SELECT al.title, ar.name FROM albums al LEFT JOIN artists ar ON ar.id = al.artist_id WHERE al.id = ?")
                .bind(album_id).fetch_optional(&pool).await.ok().flatten();
            let Some((title, artist)) = info else { return; };
            let client = tulipix_core::net::http().clone();
            let mut ok = false;
            if let Ok(rec) = tulipix_music::musicbrainz::lookup_recording(&client, &artist.clone().unwrap_or_default(), &title).await {
                if let Some(url) = tulipix_music::musicbrainz::best_cover_url(&rec) {
                    if let Ok(resp) = client.get(&url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
                        if resp.status().is_success() {
                            if let Ok(bytes) = resp.bytes().await {
                                let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("music_covers");
                                let _ = std::fs::create_dir_all(&dir);
                                let dest = dir.join(format!("album-{album_id}.jpg"));
                                if std::fs::write(&dest, &bytes).is_ok() {
                                    let _ = sqlx::query("UPDATE albums SET cover_path = ? WHERE id = ?")
                                        .bind(dest.to_string_lossy().as_ref()).bind(album_id).execute(&pool).await;
                                    ok = true;
                                }
                            }
                        }
                    }
                }
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_detail_status(if ok { "✓ Cover updated.".into() } else { "No cover found.".into() });
                if ok { open_album_detail(&w, album_id); populate_music_views(w.as_weak()); }
            });
        });
    });
    // Replace album cover / artist image from a local file.
    let w = window.as_weak();
    window.on_music_detail_replace_art(move || {
        let Some(w0) = w.upgrade() else { return; };
        let (kind, id, _) = music_detail().lock().map(|g| g.clone()).unwrap_or_default();
        // Genre detail has no DB row (id = -1); its cover is a per-genre pref keyed
        // on the genre name (np.p4.music.genre-art).
        let genre_name = w0.get_music_detail_title().to_string();
        if kind != "genre" && id < 0 { return; }
        let Some(file) = rfd::FileDialog::new().set_title("Choose image")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() else { return; };
        w0.set_music_detail_status("Updating art…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("music_covers");
            let _ = std::fs::create_dir_all(&dir);
            let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
            let dest = if kind == "genre" {
                let safe: String = genre_name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                dir.join(format!("genre-{safe}.{ext}"))
            } else { dir.join(format!("{kind}-{id}.{ext}")) };
            let ok = std::fs::copy(&file, &dest).is_ok();
            if ok {
                if kind == "album" {
                    let _ = sqlx::query("UPDATE albums SET cover_path = ? WHERE id = ?")
                        .bind(dest.to_string_lossy().as_ref()).bind(id).execute(&pool).await;
                } else if kind == "artist" {
                    let _ = sqlx::query("UPDATE artists SET image_path = ? WHERE id = ?")
                        .bind(dest.to_string_lossy().as_ref()).bind(id).execute(&pool).await;
                }
            }
            let dest_s = dest.to_string_lossy().to_string();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_detail_status(if ok { "✓ Art updated.".into() } else { "Couldn't read that image.".into() });
                if ok {
                    if kind == "album" { open_album_detail(&w, id); }
                    else if kind == "artist" { open_artist_detail(&w, id); }
                    else if kind == "genre" {
                        save_music_pref(&format!("music.genre.cover.{genre_name}"), &dest_s);
                        open_genre_detail(&w, genre_name.clone());
                        populate_music_views(w.as_weak());
                    }
                }
            });
        });
    });
    // Sleep timer (np.p4.music.sleep-timer) — cycle Off→10→15→30→60→track-end→Off.
    let w = window.as_weak();
    window.on_music_cycle_sleep(move || {
        let Some(w0) = w.upgrade() else { return; };
        let next = match w0.get_music_sleep_min() { 0 => 10, 10 => 15, 15 => 30, 30 => 60, 60 => -1, _ => 0 };
        arm_sleep_timer(&w, next);
    });
}

fn wire_music_p5a(window: &MainWindow) {
    // ── Phase 5 music — synced lyrics / audio output / visualizer / cast / tags ──
    // Synced-lyrics offset nudge (±0.25 s) — re-derives the active line.
    let w = window.as_weak();
    window.on_music_lyrics_offset(move |delta| {
        let Some(w) = w.upgrade() else { return; };
        w.set_music_lyrics_offset_ms(w.get_music_lyrics_offset_ms() + delta);
        update_lyrics_active(&w);
    });
    // Output device picker (np.p5.music.output) — route the live track + persist.
    let w = window.as_weak();
    window.on_music_set_device(move |d| {
        let Some(w) = w.upgrade() else { return; };
        w.set_music_device(d.clone());
        save_music_pref("music.device", d.as_str());
        music_ipc(&["set_property", "audio-device", d.as_str()]);
    });
    let w = window.as_weak();
    window.on_music_toggle_exclusive(move || {
        let Some(w) = w.upgrade() else { return; };
        let on = !w.get_music_exclusive();
        w.set_music_exclusive(on);
        save_music_pref("music.exclusive", if on { "1" } else { "0" });
        music_ipc(&["set_property", "audio-exclusive", if on { "yes" } else { "no" }]);
    });
    // Gapless (np.p5.music.gapless) — live + persisted.
    let w = window.as_weak();
    window.on_music_toggle_gapless(move || {
        let Some(w) = w.upgrade() else { return; };
        let on = !w.get_music_gapless();
        w.set_music_gapless(on);
        save_music_pref("music.gapless", if on { "1" } else { "0" });
        music_ipc(&["set_property", "gapless-audio", if on { "yes" } else { "no" }]);
    });
    // Home dashboard gradient outline (np.p4.music.home-connect) — persisted.
    let w = window.as_weak();
    window.on_music_toggle_home_connect(move || {
        let Some(w) = w.upgrade() else { return; };
        let on = !w.get_music_home_connect();
        w.set_music_home_connect(on);
        save_music_pref("music.home_connect", if on { "1" } else { "0" });
    });
    // YouTube Home gradient outline (default on) — persisted.
    let w = window.as_weak();
    window.on_music_toggle_yt_home_connect(move || {
        let Some(w) = w.upgrade() else { return; };
        let on = !w.get_music_yt_home_connect();
        w.set_music_yt_home_connect(on);
        save_music_pref("music.yt_home_connect", if on { "1" } else { "0" });
    });
    // YouTube listing backend — auto (Piped, falling back to yt-dlp) | piped | ytdlp.
    let w = window.as_weak();
    window.on_music_set_yt_fetcher(move |v| {
        let Some(w) = w.upgrade() else { return; };
        let v = v.to_string();
        let v = if v == "piped" || v == "ytdlp" { v } else { "auto".to_string() };
        w.set_music_yt_fetcher(v.clone().into());
        save_music_pref("music.yt.fetcher", &v);
    });
    // Bold gradient outline on the now-playing mini players (default on) — persisted.
    let w = window.as_weak();
    window.on_music_toggle_mini_outline(move || {
        let Some(w) = w.upgrade() else { return; };
        let on = !w.get_music_mini_outline();
        w.set_music_mini_outline(on);
        save_music_pref("music.mini_outline", if on { "1" } else { "0" });
    });
    let w = window.as_weak();
    window.on_music_set_crossfade(move |v| {
        let Some(w) = w.upgrade() else { return; };
        let v = (v as f64).clamp(0.0, 12.0);
        w.set_music_crossfade(v as f32);
        save_music_pref("music.crossfade", &format!("{v:.0}"));
    });
    // Headphone EQ (np.p4.music.headphone-eq) — AutoEq preset auto-matched to
    // the output device name; correction rides the af chain of every spawn.
    let w = window.as_weak();
    window.on_music_hp_match(move |query| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_hp_status("Matching…".into());
        let weak = w.clone();
        let device = w0.get_music_device().to_string();
        let query = query.to_string();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let index = hp_index(&client).await;
            if index.is_empty() {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_hp_status(
                    "Could not load the AutoEq preset index (network / api.autoeq mirror).".into()));
                return;
            }
            let keys: Vec<String> = index.iter().map(|(n, _)| n.clone()).collect();
            // Explicit search text wins; otherwise match the output device name(s).
            let hit: Option<String> = if !query.trim().is_empty() {
                tulipix_music::headphone_eq::match_device(&query, &keys).cloned()
            } else {
                let mut names = vec![device.clone()];
                names.extend(enumerate_audio_devices().into_iter().map(|d| d.name));
                names.iter().filter(|n| n.as_str() != "auto" && !n.is_empty())
                    .find_map(|n| tulipix_music::headphone_eq::match_device(n, &keys)).cloned()
            };
            match hit.and_then(|k| index.iter().find(|(n, _)| *n == k).cloned()) {
                Some(p) => hp_apply(weak, Some(p)),
                None => { let _ = weak.upgrade_in_event_loop(|w| w.set_music_hp_status(
                    "No AutoEq match — type your headphone model and press Enter.".into())); }
            }
        });
    });
    let w = window.as_weak();
    window.on_music_hp_off(move || { hp_apply(w.clone(), None); });

    // Preamp (extra dB on top of ReplayGain) — live + persisted.
    let w = window.as_weak();
    window.on_music_set_preamp(move |v| {
        let Some(w) = w.upgrade() else { return; };
        let v = (v as f64).clamp(-12.0, 12.0);
        w.set_music_preamp_db(v as f32);
        save_music_pref("music.preamp", &format!("{v:.0}"));
        music_ipc(&["set_property", "replaygain-preamp", &format!("{v:.0}")]);
    });
    // ReplayGain (np.p5.music.replaygain) — off | track | album, live + persisted.
    let w = window.as_weak();
    window.on_music_set_replaygain(move |r| {
        let Some(w) = w.upgrade() else { return; };
        w.set_music_replaygain(r.clone());
        save_music_pref("music.replaygain", r.as_str());
        let mpv = match r.as_str() { "track" => "track", "album" => "album", _ => "no" };
        music_ipc(&["set_property", "replaygain", mpv]);
    });
    // Visualizer style — persist the last-chosen style so it sticks across launches.
    window.on_music_set_vis_style(move |s| {
        save_music_pref("music.vis_style", &s.to_string());
    });
    // Zen visualizer on/off — with it off the lyrics take the whole middle.
    window.on_music_set_zen_viz(move |on| {
        save_music_pref("music.zen_viz", if on { "1" } else { "0" });
    });
    // ReplayGain loudness scan (np.p5.music.replaygain) — compute gains for
    // untagged files: ffmpeg ebur128 per track → RG2 gain → track_meta, then
    // album gains. Progress-fill button mirrors the lyrics-sync pattern.
    let w = window.as_weak();
    window.on_music_rg_scan(move || {
        let Some(w0) = w.upgrade() else { return; };
        if w0.get_music_rg_scan_status().starts_with("Scanning") { return; } // already running
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let todo = tulipix_music::replaygain::untagged(&pool).await.unwrap_or_default();
            if todo.is_empty() {
                let _ = weak.upgrade_in_event_loop(|w| { w.set_music_rg_scan_status("All tagged".into()); });
                return;
            }
            let total = todo.len();
            let ffmpeg = tulipix_core::thumbs::tool_bin("ffmpeg");
            for (done, (id, path)) in todo.into_iter().enumerate() {
                let out = tokio::process::Command::new(&ffmpeg)
                    .args(["-hide_banner", "-nostats", "-i"]).arg(&path)
                    .args(["-map", "a:0", "-af", "ebur128", "-f", "null", "-"])
                    .no_window()
                    .output().await;
                if let Ok(out) = out {
                    let err = String::from_utf8_lossy(&out.stderr);
                    if let Some(lufs) = tulipix_music::replaygain::parse_ebur128_integrated(&err) {
                        let gain = tulipix_music::replaygain::gain_from_lufs(lufs);
                        let _ = tulipix_music::replaygain::store_track_gain(&pool, id, gain).await;
                    }
                }
                let frac = (done + 1) as f32 / total as f32;
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_music_rg_scan_progress(frac);
                    w.set_music_rg_scan_status(format!("Scanning {}/{total}", done + 1).into());
                });
            }
            let _ = tulipix_music::replaygain::recompute_album_gains(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| { w.set_music_rg_scan_status("Done".into()); });
        });
    });
    // Scrobble opt-in (np.p5.music.scrobble) — toggle + persist.
    let w = window.as_weak();
    window.on_music_toggle_scrobble(move || {
        let Some(w) = w.upgrade() else { return; };
        let on = !w.get_music_scrobble_on();
        w.set_music_scrobble_on(on);
        save_music_pref("music.scrobble", if on { "1" } else { "0" });
    });
    // Last.fm signed session-key auth (np.p5.music.scrobble). Two-stage flow:
    // 1st click — auth.getToken + open the authorize page in the browser;
    // 2nd click — auth.getSession trades the approved token for a session key,
    // stored in the OS keychain as service "lastfm.session". The API key +
    // shared secret come from Settings → API Keys as "API_KEY:SHARED_SECRET".
    let w = window.as_weak();
    window.on_music_lastfm_connect(move || {
        let Some(w0) = w.upgrade() else { return; };
        let creds = tulipix_core::api_keys::fetch("lastfm").ok().flatten()
            .and_then(|v| tulipix_music::scrobble::parse_key_secret(&v));
        let Some((api_key, secret)) = creds else {
            w0.set_music_lastfm_status("Add your Last.fm credentials in Settings → API Keys as API_KEY:SHARED_SECRET first.".into());
            return;
        };
        let stage = w0.get_music_lastfm_stage().to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            if stage != "pending" {
                // Stage 1: request token + browser authorize.
                let params = tulipix_music::scrobble::signed_params(
                    &[("method", "auth.getToken"), ("api_key", &api_key)], &secret);
                let resp = client.get(tulipix_music::scrobble::API_ROOT).query(&params).send().await;
                let token = match resp {
                    Ok(r) => r.json::<serde_json::Value>().await.ok()
                        .and_then(|v| v["token"].as_str().map(str::to_string)),
                    Err(_) => None,
                };
                let Some(token) = token else {
                    let _ = weak.upgrade_in_event_loop(|w| w.set_music_lastfm_status("Token request failed — check credentials / network.".into()));
                    return;
                };
                let url = tulipix_music::scrobble::authorize_url(&api_key, &token);
                let _ = std::process::Command::new(if cfg!(target_os = "macos") { "open" } else { "xdg-open" }).arg(&url).spawn();
                if let Ok(mut g) = lastfm_pending_token().lock() { *g = Some(token); }
                let _ = weak.upgrade_in_event_loop(|w| {
                    w.set_music_lastfm_stage("pending".into());
                    w.set_music_lastfm_status("Approve Tulipix in the browser tab, then click \"Finish Last.fm connect\".".into());
                });
            } else {
                // Stage 2: trade the approved token for a session key.
                let Some(token) = lastfm_pending_token().lock().ok().and_then(|g| g.clone()) else { return; };
                let params = tulipix_music::scrobble::signed_params(
                    &[("method", "auth.getSession"), ("api_key", &api_key), ("token", &token)], &secret);
                let resp = client.get(tulipix_music::scrobble::API_ROOT).query(&params).send().await;
                let session = match resp {
                    Ok(r) => r.json::<serde_json::Value>().await.ok(),
                    Err(_) => None,
                };
                let (key, user) = match &session {
                    Some(v) => (v["session"]["key"].as_str().map(str::to_string),
                                v["session"]["name"].as_str().unwrap_or("?").to_string()),
                    None => (None, String::new()),
                };
                let Some(sk) = key else {
                    let _ = weak.upgrade_in_event_loop(|w| w.set_music_lastfm_status("Session exchange failed — did you approve in the browser?".into()));
                    return;
                };
                let _ = tulipix_core::api_keys::store("lastfm.session", &sk);
                if let Ok(mut g) = lastfm_pending_token().lock() { *g = None; }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_music_lastfm_stage("connected".into());
                    w.set_music_lastfm_status(format!("Connected as {user} — scrobbles now submit to Last.fm.").into());
                });
            }
        });
    });
    // Cast discovery (np.p5.music.cast) — SSDP scan for DLNA renderers.
    let w = window.as_weak();
    window.on_music_scan_cast(move || {
        let weak = w.clone();
        std::thread::spawn(move || {
            let found = discover_cast_devices();
            let names: Vec<slint::SharedString> = found.iter().map(|d| d.name.clone().into()).collect();
            if let Ok(mut g) = cast_targets().lock() { *g = found; }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_cast_devices(slint::ModelRc::new(slint::VecModel::from(names)));
            });
        });
    });
    // Picking a renderer hands the current track off to it (np.p5.music.cast):
    // serve the file over HTTP + SOAP SetAVTransportURI/Play at the renderer.
    let w = window.as_weak();
    window.on_music_set_cast(move |c| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_cast_target(c.clone());
        if !c.is_empty() { cast_current_track(&w0, c.as_str()); }
    });
    // Cast session transport (np.b2.music.cast-v2) — SOAP Pause/Play/Stop at
    // the renderer picked above.
    let w = window.as_weak();
    window.on_music_cast_pause(move || {
        let Some(w0) = w.upgrade() else { return; };
        cast_pause_toggle(&w0);
    });
    let w = window.as_weak();
    window.on_music_cast_stop(move || {
        let Some(w0) = w.upgrade() else { return; };
        cast_stop(&w0);
    });
    // Instant mix / auto-DJ (np.p5.music.instant-mix) — build an "Up next" from
    // tracks sharing the current track's artist or genre (tag similarity).
    let w = window.as_weak();
    window.on_music_instant_mix(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let similar: Vec<i64> = sqlx::query_scalar(
                "SELECT tm.item_id FROM track_meta tm
                 WHERE tm.item_id != ?1
                   AND ( tm.artist_id = (SELECT artist_id FROM track_meta WHERE item_id = ?1)
                      OR (tm.genre IS NOT NULL AND tm.genre = (SELECT genre FROM track_meta WHERE item_id = ?1)) )
                 ORDER BY RANDOM() LIMIT 40")
                .bind(id).fetch_all(&pool).await.unwrap_or_default();
            let _ = tulipix_music::queue::clear(&pool).await;
            for sid in &similar { let _ = tulipix_music::queue::enqueue(&pool, *sid, "instant-mix").await; }
            let _ = weak.upgrade_in_event_loop(move |w| {
                set_instant_mix_queue(&w, &similar);
                w.set_music_player_panel("queue".into());
            });
        });
    });
    // Tag editor (np.p5.music.tag-editor) — open with current values.
    let w = window.as_weak();
    window.on_music_tag_edit(move || {
        let Some(w) = w.upgrade() else { return; };
        if current_music_id(&w).is_none() { return; }
        if let Ok(mut g) = tag_edit_target().lock() { *g = None; } // target = now-playing
        w.set_music_tag_title(w.get_music_np_title());
        let sub = w.get_music_np_sub();
        w.set_music_tag_artist(if sub == "Playing from your library" { "".into() } else { sub });
        w.set_music_tag_album("".into());
        w.set_music_tag_album_artist("".into());
        w.set_music_tag_track("".into());
        w.set_music_tag_disc("".into());
        w.set_music_tag_date("".into());
        w.set_music_tag_genre("".into());
        w.set_music_tag_credits("".into());
        w.set_music_tag_locked(false);
        w.set_music_tag_fetch_status("".into());
        w.set_music_tag_art(w.get_music_np_art());
        w.set_music_tag_open(true);
        // Prefill stored fields + refresh the dropdown for the now-playing track.
        if let Some(id) = current_music_id(&w) { prefill_tag_editor(&w.as_weak(), id); }
    });
    // Tag editor — pick a new cover image (writes the song's album cover).
    let w = window.as_weak();
    window.on_music_tag_change_art(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = editing_target(&w0) else { return; };
        let Some(file) = rfd::FileDialog::new().set_title("Choose cover art")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let album_id: Option<i64> = sqlx::query_scalar("SELECT album_id FROM track_meta WHERE item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten().flatten();
            let Some(album_id) = album_id else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_tag_fetch_status("No album to attach art to.".into()));
                return;
            };
            let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("music_covers");
            let _ = std::fs::create_dir_all(&dir);
            let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
            let dest = dir.join(format!("album-{album_id}.{ext}"));
            if std::fs::copy(&file, &dest).is_err() { return; }
            let _ = sqlx::query("UPDATE albums SET cover_path = ? WHERE id = ?")
                .bind(dest.to_string_lossy().as_ref()).bind(album_id).execute(&pool).await;
            // Load the Image on the UI thread (slint::Image is not Send).
            let dest_s = dest.to_string_lossy().to_string();
            let _ = weak.upgrade_in_event_loop(move |w| {
                let img = slint::Image::load_from_path(std::path::Path::new(&dest_s)).unwrap_or_default();
                w.set_music_tag_art(img);
                populate_music_views(w.as_weak());
                refresh_open_detail(&w);
            });
        });
    });
    // Tag editor — fetch tags from MusicBrainz into the editor fields (review then Save).
    let w = window.as_weak();
    window.on_music_tag_fetch(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = editing_target(&w0) else { return; };
        // Build a query from the editor's current title/artist (fall back to filename).
        let title_q = w0.get_music_tag_title().to_string();
        let artist_q = w0.get_music_tag_artist().to_string();
        w0.set_music_tag_fetch_status("Fetching…".into());
        let pos = music_ids().lock().ok().and_then(|g| g.iter().position(|x| *x == id)).map(|p| p as i32).unwrap_or(-1);
        let stem = if pos >= 0 { music_paths().lock().ok().and_then(|g| g.get(pos as usize).cloned())
            .and_then(|p| p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string())).unwrap_or_default() } else { String::new() };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = tulipix_core::net::http().clone();
            let (aq, tq, _) = if !title_q.trim().is_empty() { (artist_q.clone(), title_q.clone(), None) }
                else { parse_music_filename(&stem) };
            let search = tulipix_music::musicbrainz::lookup_recording(&client, &aq, &tq).await;
            let meta = search.ok().and_then(|s| tulipix_music::musicbrainz::best_metadata(&s));
            let _ = weak.upgrade_in_event_loop(move |w| {
                match meta {
                    Some(m) => {
                        if !m.title.is_empty()  { w.set_music_tag_title(m.title.into()); }
                        if !m.artist.is_empty() { w.set_music_tag_artist(m.artist.into()); }
                        if !m.album.is_empty()  { w.set_music_tag_album(m.album.into()); }
                        if let Some(g) = m.genre { w.set_music_tag_genre(g.into()); }
                        if let Some(d) = m.release_date { w.set_music_tag_date(d.into()); }
                        if !m.credits.is_empty() { w.set_music_tag_credits(m.credits.into()); }
                        w.set_music_tag_fetch_status("✓ Fetched — review & Save".into());
                    }
                    None => w.set_music_tag_fetch_status("No match found.".into()),
                }
            });
        });
    });
    // Tag editor — toggle the user-lock (persist immediately so Fetch-all honours it).
    let w = window.as_weak();
    window.on_music_tag_toggle_lock(move || {
        let Some(w0) = w.upgrade() else { return; };
        let on = !w0.get_music_tag_locked();
        w0.set_music_tag_locked(on);
        let Some(id) = editing_target(&w0) else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(&pool).await;
            let _ = sqlx::query("UPDATE track_meta SET user_locked = ? WHERE item_id = ?")
                .bind(if on { 1 } else { 0 }).bind(id).execute(&pool).await;
            let _ = weak;
        });
    });
    let w = window.as_weak();
    window.on_music_tag_save(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_tag_open(false);
        // Prefer the context-menu target (Edit media info on an arbitrary song),
        // else fall back to the now-playing track.
        let target = tag_edit_target().lock().ok().and_then(|g| *g);
        let Some(id) = target.or_else(|| current_music_id(&w0)) else { return; };
        if let Ok(mut g) = tag_edit_target().lock() { *g = None; }
        let title = w0.get_music_tag_title().to_string();
        let artist = w0.get_music_tag_artist().to_string();
        let album = w0.get_music_tag_album().to_string();
        let album_artist = w0.get_music_tag_album_artist().to_string();
        let release_date = w0.get_music_tag_date().to_string();
        let genre = w0.get_music_tag_genre().to_string();
        let credits = w0.get_music_tag_credits().to_string();
        let track_no: Option<i64> = w0.get_music_tag_track().trim().parse().ok().filter(|n| *n > 0);
        let disc_no: Option<i64> = w0.get_music_tag_disc().trim().parse().ok().filter(|n| *n > 0);
        // Reflect immediately in the now-playing bar only when editing it.
        if target.is_none() || target == current_music_id(&w0) {
            if !title.is_empty() { w0.set_music_np_title(title.clone().into()); }
            w0.set_music_np_sub(if artist.is_empty() { "Playing from your library".into() } else { artist.clone().into() });
        }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
            let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
            let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(&pool).await;
            // Upsert artist + album so the change shows on the Songs/Album/Artist
            // pages (album name lives in albums.title via album_id).
            let artist_id: Option<i64> = if artist.is_empty() { None } else {
                tulipix_music::scan::get_or_create_artist(&pool, &artist).await.ok()
            };
            let year = release_date.get(0..4).and_then(|y| y.parse::<i64>().ok()).filter(|y| *y > 0);
            let album_id: Option<i64> = if album.is_empty() { None } else {
                tulipix_music::scan::get_or_create_album(&pool, &album, artist_id, year).await.ok()
            };
            let _ = sqlx::query(
                "UPDATE track_meta SET title = ?, artist_id = COALESCE(?, artist_id),
                    album_id = COALESCE(?, album_id), album_artist = ?,
                    release_date = ?, genre = ?, credits = ?, year = COALESCE(?, year),
                    track_no = COALESCE(?, track_no), disc_no = COALESCE(?, disc_no),
                    user_locked = 1
                 WHERE item_id = ?")
                .bind(if title.is_empty() { None } else { Some(title.clone()) })
                .bind(artist_id).bind(album_id)
                .bind(if album_artist.trim().is_empty() { None } else { Some(album_artist.trim().to_string()) })
                .bind(if release_date.trim().is_empty() { None } else { Some(release_date.trim().to_string()) })
                .bind(if genre.trim().is_empty() { None } else { Some(genre.trim().to_string()) })
                .bind(if credits.trim().is_empty() { None } else { Some(credits.trim().to_string()) })
                .bind(year)
                .bind(track_no).bind(disc_no)
                .bind(id).execute(&pool).await;
            // Write the tags back into the file itself via ffmpeg (np.p5.music.tag-editor),
            // so the metadata survives a re-scan / shows in other players.
            let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            if let Some(path) = path {
                let (title, artist, album) = (title.clone(), artist.clone(), album.clone());
                let (album_artist, genre, date) = (album_artist.clone(), genre.clone(), release_date.clone());
                let _ = tokio::task::spawn_blocking(move || write_audio_tags(&path, &FileTags {
                    title: &title, artist: &artist, album: &album,
                    album_artist: &album_artist, genre: &genre, date: &date,
                    track_no, disc_no,
                })).await;
            }
            // Refresh every surface: library lists + browse tiles, and the open
            // album/artist/genre detail overlay if one is showing.
            let _ = weak.upgrade_in_event_loop(|w| { populate_music_views(w.as_weak()); refresh_open_detail(&w); });
        });
    });
}

fn wire_music_p6(window: &MainWindow) {
    // ── Phase 6 callbacks ────────────────────────────────────────────────────

    // Feature 1: Memories refresh
    let w = window.as_weak();
    window.on_memories_refresh(move || {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let cat = "memories".to_string();
            let query = weak.upgrade().map(|w| w.get_photos_query().to_string()).unwrap_or_default();
            kick_category_refresh(weak, cat, query);
        });
    });

    // Feature 2: Places refresh
    let w = window.as_weak();
    window.on_places_refresh(move || {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let cat = "places".to_string();
            let query = weak.upgrade().map(|w| w.get_photos_query().to_string()).unwrap_or_default();
            kick_category_refresh(weak, cat, query);
        });
    });

    // Feature 3: Photo label (via context action — handled above in on_photo_context_action)
    // Also direct callback for future use
    let w = window.as_weak();
    window.on_photo_label(move |idx, label| {
        let Some(w0) = w.upgrade() else { return; };
        let action = format!("label:{}", label.as_str());
        w0.invoke_photo_context_action(idx, action.into());
    });

    // Feature 5: Stack toggle
    let w = window.as_weak();
    window.on_stack_toggle(move |idx| {
        let Some(w0) = w.upgrade() else { return; };
        let paths = photo_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned());
        let Some(path) = paths else { return; };
        let path_s = path.to_string_lossy().into_owned();
        let weak = w0.as_weak();
        let cat = w0.get_photos_category().to_string();
        let query = w0.get_photos_query().to_string();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let item_id = item_id_for(&pool, std::path::Path::new(&path_s)).await;
            if let Some(id) = item_id {
                let stack_id: Option<i64> = sqlx::query_scalar(
                    "SELECT sm.stack_id FROM photo_stack_members sm \
                     WHERE sm.item_id = ? LIMIT 1"
                ).bind(id).fetch_optional(&pool).await.ok().flatten();
                if let Some(sid) = stack_id {
                    let current: Option<i64> = sqlx::query_scalar(
                        "SELECT expanded FROM photo_stacks WHERE id = ?"
                    ).bind(sid).fetch_optional(&pool).await.ok().flatten();
                    let new_expanded = current.unwrap_or(0) == 0;
                    let _ = tulipix_photos::stacks::set_expanded(&pool, sid, new_expanded).await;
                    reload_phase6_caches(&pool).await;
                }
            }
            kick_category_refresh(weak, cat, query);
        });
    });

    // Feature 6: Dedupe scan
    let w = window.as_weak();
    window.on_dedupe_scan(move || {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let _ = weak.upgrade_in_event_loop(|w| w.set_photo_dedupe_scanning(true));
            if let Ok(pool) = pool_for("photos").await {
                let _ = tulipix_photos::dedup::build_clusters(&pool).await;
                let group_data = load_dedupe_groups(&pool).await;
                let _ = weak.upgrade_in_event_loop(move |w| {
                    use slint::{ModelRc, VecModel};
                    let groups = dedupe_groups_from_paths(group_data);
                    w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
                    w.set_photo_dedupe_scanning(false);
                });
            } else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_photo_dedupe_scanning(false));
            }
        });
    });

    // Feature 6: Dedupe keep (delete the unwanted side, remove cluster)
    let w = window.as_weak();
    window.on_dedupe_keep(move |cluster_id, keep_left| {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let members: Vec<(i64,)> = sqlx::query_as(
                "SELECT item_id FROM dedup_members WHERE cluster_id = ? LIMIT 2"
            ).bind(cluster_id as i64).fetch_all(&pool).await.unwrap_or_default();
            if members.len() < 2 { return; }
            let delete_id = if keep_left { members[1].0 } else { members[0].0 };
            let _ = tulipix_photos::trash::soft_delete(&pool, &[delete_id]).await;
            let _ = sqlx::query("DELETE FROM dedup_clusters WHERE id = ?")
                .bind(cluster_id as i64).execute(&pool).await;
            let group_data = load_dedupe_groups(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                use slint::{ModelRc, VecModel};
                let groups = dedupe_groups_from_paths(group_data);
                // In place, not a model swap: this runs from a per-row button, and
                // replacing the model deletes the tile that button lives on
                // (slint#6426 — the books move-to-trash crash).
                if let Some(groups) = replace_rows(&w.get_photo_dedupe_groups(), groups) {
                    w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
                }
            });
        });
    });

    // Feature 6: Dedupe both — keep both, just remove the cluster
    let w = window.as_weak();
    window.on_dedupe_both(move |cluster_id| {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let _ = sqlx::query("DELETE FROM dedup_clusters WHERE id = ?")
                .bind(cluster_id as i64).execute(&pool).await;
            let group_data = load_dedupe_groups(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                use slint::{ModelRc, VecModel};
                let groups = dedupe_groups_from_paths(group_data);
                // In place, not a model swap: this runs from a per-row button, and
                // replacing the model deletes the tile that button lives on
                // (slint#6426 — the books move-to-trash crash).
                if let Some(groups) = replace_rows(&w.get_photo_dedupe_groups(), groups) {
                    w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
                }
            });
        });
    });

    // Feature 6: Dedupe trash — soft-delete both photos, then drop the cluster
    let w = window.as_weak();
    window.on_dedupe_trash(move |cluster_id| {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let members: Vec<(i64,)> = sqlx::query_as(
                "SELECT item_id FROM dedup_members WHERE cluster_id = ? LIMIT 2"
            ).bind(cluster_id as i64).fetch_all(&pool).await.unwrap_or_default();
            let ids: Vec<i64> = members.into_iter().map(|m| m.0).collect();
            if !ids.is_empty() {
                let _ = tulipix_photos::trash::soft_delete(&pool, &ids).await;
            }
            let _ = sqlx::query("DELETE FROM dedup_clusters WHERE id = ?")
                .bind(cluster_id as i64).execute(&pool).await;
            let group_data = load_dedupe_groups(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                use slint::{ModelRc, VecModel};
                let groups = dedupe_groups_from_paths(group_data);
                // In place, not a model swap: this runs from a per-row button, and
                // replacing the model deletes the tile that button lives on
                // (slint#6426 — the books move-to-trash crash).
                if let Some(groups) = replace_rows(&w.get_photo_dedupe_groups(), groups) {
                    w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
                }
            });
        });
    });

    // Dedupe: click a pair photo → open it in the viewer (edit=false) or the
    // editor (edit=true). Members aren't in the active grid list, so resolve the
    // abs path straight from the DB and open it standalone.
    let w = window.as_weak();
    window.on_dedupe_open(move |cluster_id, left, edit| {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let members: Vec<(i64,)> = sqlx::query_as(
                "SELECT item_id FROM dedup_members WHERE cluster_id = ? ORDER BY item_id LIMIT 2"
            ).bind(cluster_id as i64).fetch_all(&pool).await.unwrap_or_default();
            let idx = if left { 0 } else { 1 };
            let Some(m) = members.get(idx) else { return; };
            let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                .bind(m.0).fetch_optional(&pool).await.ok().flatten();
            let Some(path) = path else { return; };
            let pathbuf = std::path::PathBuf::from(path);
            let _ = weak.upgrade_in_event_loop(move |w| {
                if edit {
                    open_editor(w.as_weak(), 0, &pathbuf);
                } else {
                    show_photo_path(&w, &pathbuf);
                    w.set_viewer_open(true);
                }
            });
        });
    });

    // Feature 7: Slideshow export
    let w = window.as_weak();
    window.on_slideshow_export(move |output_path| {
        let output_path = output_path.to_string();
        let weak = w.clone();
        let sel_indices = selection_snapshot();
        let paths: Vec<PathBuf> = {
            let pp = photo_paths().lock().unwrap();
            sel_indices.iter()
                .filter_map(|&i| pp.get(i as usize).cloned())
                .collect()
        };
        if paths.is_empty() {
            let _ = weak.upgrade_in_event_loop(|w| w.set_photo_slideshow_status("No photos selected.".into()));
            return;
        }
        tokio::runtime::Handle::current().spawn(async move {
            let _ = weak.upgrade_in_event_loop(|w| w.set_photo_slideshow_status("Rendering…".into()));
            let spec = tulipix_photos::slideshow::SlideshowSpec {
                photos: paths,
                width: 1920, height: 1080,
                fps: 30,
                slide_secs: 3.0, xfade_secs: 0.5,
                music: None,
                out_path: PathBuf::from(&output_path),
            };
            match tokio::task::spawn_blocking(move || tulipix_photos::slideshow::render(&spec)).await {
                Ok(Ok(out)) => {
                    let msg = format!("Slideshow saved: {}", out.display());
                    let _ = weak.upgrade_in_event_loop(move |w| w.set_photo_slideshow_status(msg.into()));
                }
                Ok(Err(e)) => {
                    let msg = format!("Error: {e}");
                    let _ = weak.upgrade_in_event_loop(move |w| w.set_photo_slideshow_status(msg.into()));
                }
                Err(e) => {
                    let msg = format!("Task error: {e}");
                    let _ = weak.upgrade_in_event_loop(move |w| w.set_photo_slideshow_status(msg.into()));
                }
            }
        });
    });

    // Feature 8: Smart rule test
    let w = window.as_weak();
    window.on_smart_rule_test(move |json_rule| {
        let json_str = json_rule.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let count = match serde_json::from_str::<serde_json::Value>(&json_str) {
                Ok(v) => match tulipix_photos::smart_albums::compile(&v) {
                    Ok(compiled) => tulipix_photos::smart_albums::matches(&pool, &compiled).await
                        .map(|ids| ids.len() as i32).unwrap_or(-1),
                    Err(e) => { tracing::warn!(error=%e, "smart rule compile"); -1 }
                },
                Err(e) => { tracing::warn!(error=%e, "smart rule json parse"); -1 }
            };
            let _ = weak.upgrade_in_event_loop(move |w| w.set_photo_smart_rule_match_count(count));
        });
    });

    // Feature 8: Smart rule save
    let w = window.as_weak();
    window.on_smart_rule_save(move |album_id, json_rule| {
        let json_str = json_rule.to_string();
        let album_id = album_id as i64;
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            // If album_id == 0, create a new smart album
            let target_id = if album_id == 0 {
                let now = now_secs();
                // Find a unique name
                let base = "Smart Album";
                let existing: Vec<String> = sqlx::query_scalar("SELECT name FROM albums WHERE name LIKE 'Smart Album%'")
                    .fetch_all(&pool).await.unwrap_or_default();
                let name = if !existing.contains(&base.to_string()) {
                    base.to_string()
                } else {
                    let mut n = 2u32;
                    loop {
                        let candidate = format!("{base} {n}");
                        if !existing.contains(&candidate) { break candidate; }
                        n += 1;
                    }
                };
                match sqlx::query_scalar::<_, i64>(
                    "INSERT INTO albums (name, smart_rule, created, updated) VALUES (?, ?, ?, ?) RETURNING id"
                ).bind(&name).bind(&json_str).bind(now).bind(now).fetch_one(&pool).await {
                    Ok(id) => id,
                    Err(e) => { tracing::warn!(error=%e, "smart album create"); return; }
                }
            } else { album_id };
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json_str) {
                let _ = tulipix_photos::smart_albums::save_rule(&pool, target_id, &v).await;
            }
            // Refresh albums list
            let cards = load_albums().await;
            let _ = weak.upgrade_in_event_loop(move |w| populate_albums(&w, cards));
        });
    });

    let w = window.as_weak();
    window.on_dismiss_scan_progress(move || {
        // Mark every counter as finished so the flush ticker doesn't
        // resurrect a dismissed row, then clear UI in one atomic post.
        if let Ok(g) = scan_state().lock() {
            for c in g.values() {
                c.active.store(false, std::sync::atomic::Ordering::Relaxed);
            }
        }
        if let Some(w) = w.upgrade() {
            w.set_scan_progress(slint::ModelRc::new(slint::VecModel::from(Vec::<ScanProgress>::new())));
            w.set_scan_active(false);
        }
    });

    // User-card menu actions — local-only app: the only modes are Local and
    // Locked (lockscreen). No account, no sign-in, no upgrade nudges.
    let w = window.as_weak();
    window.on_user_action(move |action| {
        tracing::info!(?action, "user action");
        if action.as_str() == "settings-account" {
            // User-card "Settings → Profile" shortcut.
            if let Some(w) = w.upgrade() {
                w.set_active_section("settings".into());
                w.set_active_settings_tab("profile".into());
            }
            return;
        }
        let next = match action.as_str() {
            "lock" => { tulipix_core::account::lock(); tulipix_core::account::AppMode::Locked }
            "switch-to-local" => {
                tulipix_core::account::set(tulipix_core::account::AppMode::Local);
                tulipix_core::account::AppMode::Local
            }
            // Lock toggle: locked → unlock, otherwise lock.
            "switch-mode" => {
                if tulipix_core::account::current() == tulipix_core::account::AppMode::Locked {
                    tulipix_core::account::unlock()
                } else {
                    tulipix_core::account::lock();
                    tulipix_core::account::AppMode::Locked
                }
            }
            _ => return,
        };
        if let Some(w) = w.upgrade() {
            let mut user = w.get_user();
            user.mode = match next {
                tulipix_core::account::AppMode::Locked => Mode::Locked,
                _ => Mode::Local,
            };
            w.set_user(user);
            w.set_account_sync_allowed(true); // local app: full access, every capability unlocked
            // Locking shows the screensaver as a lock screen.
            if action.as_str() == "lock" {
                w.set_ambient_clock(clock_now().into());
                w.set_ambient_caption("Locked — click to resume".into());
                w.set_ambient_active(true);
            }
        }
    });

    // Profile card: live app version + browser links.
    window.set_app_version(env!("CARGO_PKG_VERSION").into());
    window.on_open_url(move |url| {
        let url = url.to_string();
        // Only ever hand the OS a web URL. This callback is generic, and some of
        // what reaches it is remote data (stream metadata, feeds, book
        // metadata); without the scheme check a value like `C:\payload.exe` or
        // `file://…` would be launched as readily as a link.
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            tracing::warn!(%url, "refusing to open non-web url");
            return;
        }
        // explorer.exe, not `cmd /C start`: cmd re-parses its command line after
        // argv splitting, so `&`, `|` and `^` inside a URL became new commands.
        #[cfg(target_os = "windows")]
        let r = std::process::Command::new("explorer.exe").arg(&url).spawn();
        #[cfg(target_os = "macos")]
        let r = std::process::Command::new("open").arg(&url).spawn();
        #[cfg(all(unix, not(target_os = "macos")))]
        let r = std::process::Command::new("xdg-open").arg(&url).spawn();
        if let Err(e) = r { tracing::warn!(error = %e, %url, "open url"); }
    });

    // Profile editor (Settings → Profile) — persist name + avatar emoji into
    // the generic settings KV and reflect them in the sidebar user card.
    let w = window.as_weak();
    window.on_profile_save(move |name, emoji, logo| {
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        let name = clamp_profile_name(&name);
        if name.is_empty() { s.advanced.remove("profile.name"); }
        else { s.advanced.insert("profile.name".into(), name.clone()); }
        if emoji.is_empty() { s.advanced.remove("profile.emoji"); }
        else { s.advanced.insert("profile.emoji".into(), emoji.to_string()); }
        // Sidebar logo choice (np.p1.profile.logo): 0 default · 1 color · 2 dark ·
        // 3 white · 4 India (the seasonal mark, picked on purpose so it holds all year).
        if logo == 0 { s.advanced.remove("profile.logo"); }
        else { s.advanced.insert("profile.logo".into(), logo.to_string()); }
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save settings (profile)"); }
        if let Some(w) = w.upgrade() {
            let mut u = w.get_user();
            u.display_name = name.into();
            u.avatar_emoji = emoji;
            w.set_user(u);
            w.set_app_logo_choice(logo);
        }
    });

    // Cover/avatar upload — pick an image, open the crop dialog
    // (np.p1.profile.cover). One helper serves both modes; the mode string
    // rides the crop-dialog-mode window property and returns via crop-confirm.
    fn open_crop_picker(w: &MainWindow, mode: &str) {
        let title = if mode == "avatar" { "Choose avatar photo" } else { "Choose cover photo" };
        let Some(file) = rfd::FileDialog::new().set_title(title)
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() else { return; };
        let Ok((nat_w, nat_h)) = image::image_dimensions(&file) else {
            w.set_caps_nudge("Couldn't read that image file.".into());
            return;
        };
        let Ok(img) = slint::Image::load_from_path(&file) else {
            w.set_caps_nudge("Couldn't decode that image file.".into());
            return;
        };
        *crop_source().lock().unwrap() = Some(file);
        w.set_crop_dialog_mode(mode.into());
        w.set_crop_dialog_nat_w(nat_w as i32);
        w.set_crop_dialog_nat_h(nat_h as i32);
        w.set_crop_dialog_image(img);
        w.set_crop_dialog_open(true);
    }
    let w = window.as_weak();
    window.on_pick_cover_image(move || {
        if let Some(w) = w.upgrade() { open_crop_picker(&w, "cover"); }
    });
    let w = window.as_weak();
    window.on_pick_avatar_image(move || {
        if let Some(w) = w.upgrade() { open_crop_picker(&w, "avatar"); }
    });
    // Crop confirmed — crop+save the PNG from the full-res source, rebind the
    // profile-card image. Files live at <data_dir>/profile/{cover,avatar}.png.
    let w = window.as_weak();
    window.on_crop_confirm(move |mode, zoom, pan_x, pan_y, mask_w, mask_h| {
        let Some(w) = w.upgrade() else { return; };
        w.set_crop_dialog_open(false);
        let Some(src) = crop_source().lock().unwrap().take() else { return; };
        let Some(dir) = tulipix_core::paths::data_dir().map(|d| d.join("profile")) else { return; };
        let (dest, out_w, out_h) = if mode.as_str() == "avatar" {
            (dir.join("avatar.png"), 480u32, 480u32)
        } else {
            (dir.join("cover.png"), 1200u32, 300u32)
        };
        match profile_image::save_cropped(&src, &dest, mask_w, mask_h, out_w, out_h, zoom, pan_x, pan_y) {
            Ok(()) => {
                let img = slint::Image::load_from_path(&dest).unwrap_or_default();
                let mut u = w.get_user();
                if mode.as_str() == "avatar" { u.avatar_image = img; } else { u.cover_image = img; }
                w.set_user(u);
            }
            Err(e) => {
                tracing::warn!(error = %e, "crop/save profile image");
                w.set_caps_nudge("Couldn't save the cropped image.".into());
            }
        }
    });
}

fn wire_music_playlist(window: &MainWindow) {
    // ── Phase 5 music — playlist builder / M3U / queue reorder ──────────────
    // Open a playlist's detail (tile `index` carries the playlist DB id).
    let w = window.as_weak();
    window.on_music_playlist_open(move |pid| {
        if let Some(w) = w.upgrade() { build_playlist_detail(&w, pid as i64); }
    });
    // Home → Recently added → "See all". The smart playlist is created on first
    // use with the same rule the Playlists tab's one-click chip uses, so the two
    // entry points cannot end up with two playlists of the same name.
    let w = window.as_weak();
    window.on_music_open_recent_playlist(move || {
        let Some(w0) = w.upgrade() else { return; };
        let weak = w.clone();
        w0.set_music_lib_tab("playlists".into());
        tokio::runtime::Handle::current().spawn(async move {
            use tulipix_music::playlists::{SmartRule, Combine};
            let Ok(pool) = pool_for("music").await else { return; };
            const NAME: &str = "Recently Added";
            let rule = SmartRule { combine: Combine::All, conditions: vec![], limit: Some(100) };
            let pid = match tulipix_music::playlists::find_by_name(&pool, NAME).await.ok().flatten() {
                Some(id) => id,
                None => match tulipix_music::playlists::create(&pool, NAME, Some(&rule)).await {
                    Ok(id) => id,
                    Err(_) => return,
                },
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                populate_music_views(w.as_weak());
                build_playlist_detail(&w, pid);
            });
        });
    });
    // Set a custom cover image for the open playlist (np.p5.music.playlists-builder).
    let w = window.as_weak();
    window.on_music_playlist_set_art(move || {
        let Some(w0) = w.upgrade() else { return; };
        let id = current_playlist().lock().map(|g| g.0).unwrap_or(-1);
        if id < 0 { return; }
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Choose playlist art")
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
            .pick_file()
        {
            let p = path.to_string_lossy().into_owned();
            save_music_pref(&format!("music.playlist.cover.{id}"), &p);
            w0.set_music_playlist_cover(slint::Image::load_from_path(&path).unwrap_or_default());
            populate_music_views(w0.as_weak()); // reflect the art on the playlists grid too
        }
    });
    let w = window.as_weak();
    window.on_music_playlist_back(move || {
        if let Some(w) = w.upgrade() {
            w.set_music_playlist_name("".into());
            w.set_music_playlist_tracks(slint::ModelRc::new(slint::VecModel::<MusicSongRow>::default()));
        }
    });
    // Create a manual playlist from the modal's name.
    let w = window.as_weak();
    window.on_music_playlist_create(move || {
        let Some(w0) = w.upgrade() else { return; };
        let name = w0.get_music_playlist_new_name().to_string();
        w0.set_music_playlist_new_open(false);
        w0.set_music_playlist_new_name("".into());
        if name.trim().is_empty() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::playlists::create(&pool, name.trim(), None).await;
            let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
        });
    });
    // One-click smart playlists.
    let w = window.as_weak();
    window.on_music_playlist_create_smart(move |kind| {
        let Some(w0) = w.upgrade() else { return; };
        let weak = w.clone();
        let kind = kind.to_string();
        tokio::runtime::Handle::current().spawn(async move {
            use tulipix_music::playlists::{SmartRule, Combine, Condition, Field, Op};
            let Ok(pool) = pool_for("music").await else { return; };
            let (name, rule) = match kind.as_str() {
                "loved" => ("Loved", SmartRule { combine: Combine::All,
                    conditions: vec![Condition { field: Field::Loved, op: Op::Eq, value: "1".into() }], limit: None }),
                _ => ("Recently Added", SmartRule { combine: Combine::All, conditions: vec![], limit: Some(100) }),
            };
            // One-time add: re-tapping the chip must not stack duplicates.
            if tulipix_music::playlists::find_by_name(&pool, name).await.ok().flatten().is_none() {
                let _ = tulipix_music::playlists::create(&pool, name, Some(&rule)).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
        });
        let _ = w0;
    });
    // Delete a playlist (confirmed in the UI) — by id, or the open one when -1.
    let w = window.as_weak();
    window.on_music_playlist_delete(move |pid| {
        let Some(w0) = w.upgrade() else { return; };
        let id = if pid < 0 { current_playlist().lock().map(|g| g.0).unwrap_or(-1) } else { pid as i64 };
        if id < 0 { return; }
        // Close the detail page if it is the one being deleted.
        let open_id = current_playlist().lock().map(|g| g.0).unwrap_or(-1);
        if open_id == id {
            w0.set_music_playlist_name("".into());
            w0.set_music_playlist_tracks(slint::ModelRc::new(slint::VecModel::<MusicSongRow>::default()));
        }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::playlists::delete(&pool, id).await;
            let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
        });
    });
    let w = window.as_weak();
    window.on_music_playlist_play_track(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        // Queue the rest of the playlist in its displayed order (np.p5.music.playlist-order).
        let (_pid, ids) = current_playlist().lock().map(|g| g.clone()).unwrap_or((-1, Vec::new()));
        set_ctx_pending();
        play_music_at(&w0, pos);
        set_context_queue(&w0, ids, music_id_at(pos), "playlist");
    });
    // Remove the i-th track from the open playlist.
    let w = window.as_weak();
    window.on_music_playlist_remove_track(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let (pid, ids) = current_playlist().lock().map(|g| g.clone()).unwrap_or((-1, Vec::new()));
        if pid < 0 || i < 0 || (i as usize) >= ids.len() { return; }
        let item_id = ids[i as usize];
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ? AND item_id = ?")
                .bind(pid).bind(item_id).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| { build_playlist_detail(&w, pid); populate_music_views(w.as_weak()); });
        });
        let _ = w0;
    });
    // Reorder a track within the open playlist (np.p5.music.playlists-builder).
    let w = window.as_weak();
    window.on_music_playlist_move(move |from, to| {
        let Some(_w0) = w.upgrade() else { return; };
        if to < 0 { return; }
        let pid = current_playlist().lock().map(|g| g.0).unwrap_or(-1);
        if pid < 0 || from < 0 { return; }
        let (from, to) = (from as usize, to as usize);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            if tulipix_music::playlists::reorder(&pool, pid, from, to).await.is_ok() {
                let _ = weak.upgrade_in_event_loop(move |w| build_playlist_detail(&w, pid));
            }
        });
    });
    // Playlist sort — Custom (saved/manual order = play order) | title | artist.
    let w = window.as_weak();
    window.on_music_playlist_sort_set(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let s = s.to_string();
        let dir = if w0.get_music_playlist_sort() == s.as_str() {
            if w0.get_music_playlist_sort_dir() == "asc" { "desc" } else { "asc" }
        } else { "asc" };
        w0.set_music_playlist_sort(s.clone().into());
        w0.set_music_playlist_sort_dir(dir.into());
        let pid = current_playlist().lock().map(|g| g.0).unwrap_or(-1);
        if pid >= 0 { build_playlist_detail(&w0, pid); }
    });
    // Import an M3U/M3U8/PLS file → new playlist of matched library tracks.
    let w = window.as_weak();
    window.on_music_playlist_import(move || {
        let Some(file) = rfd::FileDialog::new().set_title("Import playlist")
            .add_filter("Playlists", &["m3u", "m3u8", "pls"]).pick_file() else { return; };
        let base = file.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        let name = file.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported").to_string();
        let Ok(content) = std::fs::read_to_string(&file) else { return; };
        // .pls vs .m3u/.m3u8 by extension (np.p5.music.import-playlists).
        let is_pls = file.extension().and_then(|e| e.to_str())
            .map(|e| e.eq_ignore_ascii_case("pls")).unwrap_or(false);
        let paths = if is_pls {
            tulipix_music::playlists::parse_pls(&content, &base)
        } else {
            tulipix_music::playlists::parse_m3u(&content, &base)
        };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let pid = tulipix_music::playlists::create(&pool, &name, None).await.unwrap_or(-1);
            if pid < 0 { return; }
            for p in &paths {
                let id: Option<i64> = sqlx::query_scalar("SELECT id FROM items WHERE path = ?")
                    .bind(p.to_string_lossy().as_ref()).fetch_optional(&pool).await.ok().flatten();
                if let Some(id) = id { let _ = tulipix_music::playlists::append(&pool, pid, id).await; }
            }
            let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
        });
    });
    // Export the current Up-Next queue (or the library) to an M3U file.
    let _w = window.as_weak();
    window.on_music_playlist_export(move || {
        let Some(file) = rfd::FileDialog::new().set_title("Export queue")
            .set_file_name("tulipix-queue.m3u8").add_filter("Playlists", &["m3u8", "m3u"]).save_file() else { return; };
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let ids = tulipix_music::queue::list(&pool).await.unwrap_or_default();
            let paths: Vec<PathBuf> = if ids.is_empty() {
                music_paths().lock().map(|g| g.clone()).unwrap_or_default()
            } else {
                let mut v = Vec::new();
                for id in ids {
                    if let Ok(Some(p)) = sqlx::query_scalar::<_, String>("SELECT path FROM items WHERE id = ?")
                        .bind(id).fetch_optional(&pool).await { v.push(PathBuf::from(p)); }
                }
                v
            };
            let _ = std::fs::write(&file, tulipix_music::playlists::write_m3u(&paths));
            tracing::info!(file = %file.display(), tracks = paths.len(), "exported M3U");
        });
    });
    // Repurpose add-to-playlist as a picker; choose a target playlist.
    let w = window.as_weak();
    window.on_music_add_pick(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_add_pick_open(false);
        let ids = pick_playlist_ids().lock().map(|g| g.clone()).unwrap_or_default();
        let Some(&pid) = ids.get(i as usize) else { return; };
        let Some(track) = current_music_id(&w0) else { return; };
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("music").await {
                let _ = tulipix_music::playlists::append(&pool, pid, track).await;
            }
        });
    });
    // Queue drag-reorder (up/down) — persists via play_queue.move_item.
    let w = window.as_weak();
    window.on_music_queue_move(move |from, to| {
        let Some(w0) = w.upgrade() else { return; };
        if to < 0 { return; }
        let weak = w.clone();
        let (from, to) = (from as usize, to as usize);
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::queue::move_item(&pool, from, to).await;
            let _ = weak.upgrade_in_event_loop(|w| build_music_queue(&w));
        });
        let _ = w0;
    });

    // Audiobook progress writeback (user report 2026-06-12: card bars stuck at
    // 0%, books never reached In-progress): every 15 s of playback, persist the
    // live position for the playing chapter. The card bars + In-progress /
    // Finished tabs all read audiobook_progress.
    {
        let weak = window.as_weak();
        let saver: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
        saver.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(15), move || {
            let Some(w) = weak.upgrade() else { return; };
            if !w.get_music_playing() { return; }
            let Some(id) = current_music_id(&w) else { return; };
            // Book mode is set explicitly on every audiobook play — unlike the
            // songs-store `is_audiobook` flag, which is a stale snapshot on a
            // book's FIRST-ever listen (mark_audiobook only hits the DB at
            // click time), so progress was never saved and resume landed at 0.
            if w.get_music_player_mode().as_str() != "book" { return; }
            let pos = w.get_music_pos() as f64;
            let speed = w.get_music_book_speed() as f64;
            let refresh_cards = w.get_music_view() == "audiobooks";
            let wk = weak.clone();
            tokio::runtime::Handle::current().spawn(async move {
                if let Ok(pool) = pool_for("music").await {
                    let _ = tulipix_music::audiobooks::save_progress(&pool, id, pos.max(1.0), speed).await;
                }
                // Live-refresh the card bars while the Audiobooks view is open.
                if refresh_cards {
                    let _ = wk.upgrade_in_event_loop(|w| populate_audiobooks(&w));
                }
            });
        });
    }
    // UI-thread jank watchdog (np.p1.perf.jank): a 100 ms repeating timer
    // measures its own drift — the timer only fires late when the event loop
    // was blocked, so drift > 16 ms ≈ at least one dropped frame.
    //
    // Debug builds only. It is a development instrument — it writes to the log
    // and nothing else reads it — and in a release build it was ten wakeups a
    // second, forever, on battery, to measure a number nobody would see.
    #[cfg(debug_assertions)]
    {
        let last = std::cell::Cell::new(std::time::Instant::now());
        let jank_timer: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
        jank_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(100), move || {
            let now = std::time::Instant::now();
            let drift = now.duration_since(last.get()).as_millis() as i64 - 100;
            last.set(now);
            if drift > 16 {
                let n = SLOW_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                if n % 100 == 0 { tracing::warn!(slow_frames = n, last_block_ms = drift, "UI jank watchdog"); }
            }
        });
    }
    // Audio visualizer animation (np.p5.music.visualizer) — a synthetic spectrum
    // while a track plays; ~11 fps keeps it lively without burning CPU.
    {
        let weak = window.as_weak();
        let start = std::time::Instant::now();
        let vis_timer: &'static slint::Timer = Box::leak(Box::new(slint::Timer::default()));
        vis_timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(90), move || {
            let Some(w) = weak.upgrade() else { return; };
            // Nothing on screen consumes the bars unless Music or Home is the
            // open section AND the main window is actually up. Minimised to the
            // desktop widget, the old gate (playing?) was still true, so this
            // rebuilt a spectrum eleven times a second and pushed it into a
            // hidden window — the widget does not draw bars at all.
            let drawn = matches!(w.get_active_section().as_str(), "music" | "home")
                && w.window().is_visible();
            if !drawn || !w.get_music_playing() {
                if w.get_music_vis_bars().row_count() > 0 {
                    w.set_music_vis_bars(slint::ModelRc::new(slint::VecModel::<f32>::default()));
                }
                return;
            }
            // Shape from the synthetic spectrum, energy from the REAL audio loudness
            // (ebur128) so every style pulses to the actual beat, not just a clock.
            let amp = MUSIC_LOUDNESS.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
            let env = 0.18 + 0.82 * amp;
            let bars: Vec<f32> = tulipix_music::visualizer::synthetic_bars(
                tulipix_music::visualizer::DEFAULT_BARS, start.elapsed().as_secs_f64())
                .into_iter().map(|b| (b * env).clamp(0.0, 1.0)).collect();
            w.set_music_vis_bars(slint::ModelRc::new(slint::VecModel::from(bars)));
        });
    }
    // Initial music audio-config + output-device list from persisted settings.
    {
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        window.set_music_gapless(s.advanced.get("music.gapless").map(|v| v != "0").unwrap_or(true));
        // Restore the podcast queue saved last session.
        podcast_queue_load(window);
        window.set_music_home_connect(s.advanced.get("music.home_connect").map(|v| v == "1").unwrap_or(false));
        // YouTube Home gradient outline defaults ON when unset.
        window.set_music_yt_home_connect(s.advanced.get("music.yt_home_connect").map(|v| v == "1").unwrap_or(true));
        window.set_music_yt_fetcher(s.advanced.get("music.yt.fetcher")
            .filter(|v| v.as_str() == "piped" || v.as_str() == "ytdlp")
            .cloned().unwrap_or_else(|| "auto".into()).into());
        // Mini-player bold outline defaults ON when unset.
        window.set_music_mini_outline(s.advanced.get("music.mini_outline").map(|v| v == "1").unwrap_or(true));
        window.set_music_crossfade(s.advanced.get("music.crossfade").and_then(|v| v.parse().ok()).unwrap_or(0.0));
        window.set_music_preamp_db(s.advanced.get("music.preamp").and_then(|v| v.parse().ok()).unwrap_or(0.0));
        // Saved headphone-EQ correction — restore the af chunk + status line.
        if let Some(af) = s.advanced.get("music.hp_af").filter(|a| !a.is_empty()) {
            if let Ok(mut g) = headphone_af().lock() { *g = af.clone(); }
            let name = s.advanced.get("music.hp_preset").cloned().unwrap_or_default();
            window.set_music_hp_status(format!("Headphone EQ: {name}.").into());
        }
        window.set_music_replaygain(s.advanced.get("music.replaygain").cloned().unwrap_or_else(|| "off".into()).into());
        window.set_music_grid_density(s.advanced.get("music.grid_density")
            .and_then(|v| v.parse::<f64>().ok())
            .map(tulipix_music::grid_density::clamp_target).unwrap_or(200.0) as f32);
        window.set_music_device(s.advanced.get("music.device").cloned().unwrap_or_else(|| "auto".into()).into());
        window.set_music_exclusive(s.advanced.get("music.exclusive").map(|v| v == "1").unwrap_or(false));
        window.set_music_scrobble_on(s.advanced.get("music.scrobble").map(|v| v == "1").unwrap_or(false));
        // Last.fm session key already in the keychain → show connected state.
        if tulipix_core::api_keys::fetch("lastfm.session").ok().flatten().is_some() {
            window.set_music_lastfm_stage("connected".into());
            window.set_music_lastfm_status("Last.fm connected.".into());
        }
        let tsz = s.advanced.get("music.thumb_size").and_then(|v| v.parse::<f32>().ok()).unwrap_or(168.0).clamp(100.0, 300.0);
        window.set_music_thumb_size(tsz);
        // Last-used visualizer style (0..4), default Line (4).
        window.set_music_vis_style(s.advanced.get("music.vis_style").and_then(|v| v.parse::<i32>().ok()).unwrap_or(4).clamp(0, 4));
        window.set_music_zen_viz(s.advanced.get("music.zen_viz").map(|v| v != "0").unwrap_or(true));
        populate_eq_customs(&window);
        let weak = window.as_weak();
        std::thread::spawn(move || {
            let ids: Vec<slint::SharedString> = enumerate_audio_devices().iter().map(|d| d.id.clone().into()).collect();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_devices(slint::ModelRc::new(slint::VecModel::from(ids)));
            });
        });
    }

    // Build the right-click menu per tile. Order:
    //   Open · Open in Default Viewer · Archive · Star · Reveal · Trash · Properties
    // "Default Viewer" is the cross-platform label for the OS default app — no
    // per-app enumeration (avoids platform-specific names like gwenview).
    let w = window.as_weak();
    window.on_open_context(move |idx, mx, my| {
        let Some(w) = w.upgrade() else { return; };
        let mk = |id: &str, label: &str, danger: bool| ContextItem {
            id: id.into(), label: label.into(), shortcut: "".into(), danger,
        };
        let category = w.get_photos_category().to_string();
        let items: Vec<ContextItem> = if category == "trash" {
            vec![
                mk("open", "Open", false),
                mk("open-default", "Open in Default Viewer", false),
                mk("restore", "Restore", false),
                mk("reveal", "Reveal In File System", false),
                mk("props", "Properties", false),
                mk("delete-forever", "Delete permanently", true),
            ]
        } else {
            vec![
                mk("open", "Open", false),
                mk("edit", "Edit", false),
                mk("open-default", "Open in Default Viewer", false),
                mk("archive", if category == "archive" { "Unarchive" } else { "Archive" }, false),
                mk("star", "Star", false),
                mk("reveal", "Reveal In File System", false),
                mk("props", "Properties", false),
                mk("trash", "Move to Trash", true),
            ]
        };
        w.set_ctx_items(slint::ModelRc::new(slint::VecModel::from(items)));
        w.set_ctx_index(idx);
        w.set_ctx_x(mx);
        w.set_ctx_y(my);
        w.set_ctx_open(true);
    });

    // Photo tile right-click actions — Open / Reveal / Properties / Open-with,
    // plus the photos-section flag actions (Star / Archive / Trash / Restore).
    let w = window.as_weak();
    window.on_photo_context_action(move |idx, action| {
        let Some(w0) = w.upgrade() else { return; };
        let Some(path) = photo_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else { return; };
        let action = action.to_string();
        // — Synchronous, non-DB actions —
        if action == "open" {
            // Open inside the app's photo viewer (not the OS default app).
            show_photo_at(&w0, idx);
            w0.set_viewer_open(true);
            return;
        }
        if action == "edit" || action == "caption" {
            // Open the non-destructive editor on this photo (np.p2.edit.*).
            // "caption" goes to the editor where ImageDescription (caption) is editable.
            open_editor(w.clone(), idx, &path);
            return;
        }
        if action == "open-default" {
            if let Err(e) = tulipix_platform::fm::open_default(&path) {
                tracing::error!(error = %e, "open in default viewer failed");
            }
            return;
        }
        if action == "props" {
            w0.set_props(build_props(&path));
            w0.set_props_open(true);
            return;
        }
        if action == "reveal" {
            if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&path) {
                tracing::error!(error = %e, "reveal failed");
            }
            return;
        }
        // — DB flag actions — run off-thread, then refresh the current tab. —
        let category = w0.get_photos_category().to_string();
        let query = w0.get_photos_query().to_string();
        let weak = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("photos").await else { return; };
            let Some(id) = item_id_for(&pool, &path).await else {
                tracing::warn!(path = %path.display(), "context action: item id not found");
                return;
            };
            let res = match action.as_str() {
                "star"    => tulipix_photos::star::toggle(&pool, id).await.map(|_| ()),
                // Toggle on the archive flag relative to the tab being viewed.
                "archive" => tulipix_photos::archive::set(&pool, &[id], category != "archive").await.map(|_| ()),
                "trash"   => tulipix_photos::trash::soft_delete(&pool, &[id]).await.map(|_| ()),
                "restore" => tulipix_photos::trash::restore(&pool, &[id]).await.map(|_| ()),
                "delete-forever" => {
                    let _ = tulipix_platform::fm::move_to_trash(&path); // file → OS trash
                    tulipix_photos::trash::purge_row(&pool, id).await
                }
                label_action if label_action.starts_with("label:") => {
                    let lbl = &label_action[6..]; // "red", "orange", etc. or ""
                    let r = if lbl.is_empty() {
                        sqlx::query("UPDATE photo_meta SET color_label = NULL WHERE item_id = ?")
                            .bind(id).execute(&pool).await.map(|_| ())
                    } else {
                        sqlx::query("UPDATE photo_meta SET color_label = ? WHERE item_id = ?")
                            .bind(lbl).bind(id).execute(&pool).await.map(|_| ())
                    };
                    // Update local cache immediately.
                    let path_s = path.to_string_lossy().into_owned();
                    if let Ok(mut g) = color_labels().lock() {
                        if lbl.is_empty() { g.remove(&path_s); } else { g.insert(path_s, lbl.to_string()); }
                    }
                    r.map_err(anyhow::Error::from)
                }
                _ => Ok(()),
            };
            match res {
                Ok(())  => tracing::info!(%action, item = id, "photo flag action"),
                Err(e)  => tracing::error!(error = %e, %action, "photo flag action failed"),
            }
            // Flash the matching tab as confirmation (star 0.8 s / archive 2 s / trash 1 s).
            let (blink, dur_ms) = match action.as_str() {
                "star" => ("starred", 800u64),
                "archive" => ("archive", 2000u64),
                "trash" => ("trash", 1000u64),
                _ => ("", 0u64),
            };
            let bweak = weak.clone();
            kick_category_refresh(weak, category, query);
            if !blink.is_empty() {
                let b = blink.to_string();
                let wk = bweak.clone();
                let _ = bweak.upgrade_in_event_loop(move |w| {
                    w.set_photos_blink(b.into());
                    let wk2 = wk.clone();
                    BLINK_TIMER.with(|t| {
                        t.borrow().start(
                            slint::TimerMode::SingleShot,
                            std::time::Duration::from_millis(dur_ms),
                            move || { if let Some(w) = wk2.upgrade() { w.set_photos_blink("".into()); } },
                        );
                    });
                });
            }
        });
    });
}

fn wire_music_podcasts(window: &MainWindow) {
    // ── Phase 5 music — podcasts / audiobooks / artist bio ──────────────────
    // OPML import: pick a file, subscribe to every feed in it (skipping ones
    // already subscribed); progress lands in the Add-podcast status line.
    let w = window.as_weak();
    window.on_music_podcast_opml_import(move || {
        let weak = w.clone();
        let rt = tokio::runtime::Handle::current();
        std::thread::spawn(move || {
            let Some(path) = rfd::FileDialog::new().set_title("Import podcast subscriptions (OPML)")
                .add_filter("OPML", &["opml", "xml"]).pick_file() else { return; };
            let Ok(xml) = std::fs::read_to_string(&path) else { return; };
            let feeds = tulipix_music::podcasts::parse_opml(&xml);
            if feeds.is_empty() {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_podcast_add_status("No feeds found in that OPML file.".into()));
                return;
            }
            rt.spawn(async move {
                let existing: std::collections::HashSet<String> = match pool_for("podcasts").await {
                    Ok(pool) => sqlx::query_scalar::<_, String>("SELECT feed_url FROM podcasts")
                        .fetch_all(&pool).await.unwrap_or_default().into_iter().collect(),
                    Err(_) => Default::default(),
                };
                let fresh: Vec<String> = feeds.into_iter()
                    .filter(|(_, u)| !existing.contains(u)).map(|(_, u)| u).collect();
                let n = fresh.len();
                let _ = weak.clone().upgrade_in_event_loop(move |w| {
                    w.set_music_podcast_add_status(
                        if n == 0 { "All feeds in that OPML are already subscribed.".to_string() }
                        else { format!("Importing {n} feed(s)…") }.into());
                });
                for url in fresh {
                    subscribe_feed_with_progress(weak.clone(), url);
                }
            });
        });
    });
    // OPML export: dump every subscription (title + feed URL) to a picked file.
    let w = window.as_weak();
    window.on_music_podcast_opml_export(move || {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let subs: Vec<(String, String)> = sqlx::query_as(
                "SELECT COALESCE(title, ''), feed_url FROM podcasts ORDER BY title")
                .fetch_all(&pool).await.unwrap_or_default();
            let n = subs.len();
            std::thread::spawn(move || {
                let Some(file) = rfd::FileDialog::new().set_title("Export podcast subscriptions (OPML)")
                    .set_file_name("tulipix-podcasts.opml")
                    .add_filter("OPML", &["opml"]).save_file() else { return; };
                let ok = std::fs::write(&file, tulipix_music::podcasts::to_opml(&subs)).is_ok();
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_podcast_add_status(
                    if ok { format!("Exported {n} subscription(s).") } else { "Export failed — could not write the file.".to_string() }.into()));
            });
        });
    });
    // Subscribe to a podcast RSS feed (np.p5.music.podcast-feeds) — fetch + parse.
    let w = window.as_weak();
    window.on_music_podcast_subscribe(move || {
        let Some(w0) = w.upgrade() else { return; };
        let url = w0.get_music_podcast_url().to_string();
        if url.trim().is_empty() { return; }
        // Keep the dialog open and show the busy/progress state; the URL stays put
        // (so a failed add can be edited + retried) until it actually succeeds.
        w0.set_music_podcast_add_busy(true);
        w0.set_music_podcast_add_frac(0.0);
        w0.set_music_podcast_add_status("Fetching feed…".into());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let fail = |weak: slint::Weak<MainWindow>, msg: &'static str| {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_music_podcast_add_busy(false);
                    w.set_music_podcast_add_frac(0.0);
                    w.set_music_podcast_add_status(msg.into());
                });
            };
            let Ok(pool) = pool_for("podcasts").await else { fail(weak.clone(), "Database error"); return; };
            let client = tulipix_core::net::http().clone();
            let Ok(resp) = client.get(url.trim()).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await
                else { fail(weak.clone(), "Network error — check the URL"); return; };
            let Ok(xml) = resp.text().await else { fail(weak.clone(), "Could not read the feed"); return; };
            let feed = tulipix_music::podcasts::parse_feed(&xml);
            if feed.title.is_none() && feed.episodes.is_empty() { fail(weak.clone(), "Not a valid podcast feed"); return; }
            // Drive the dialog progress bar per-episode (throttled to whole %).
            let prog = weak.clone();
            let mut last_pct = -1i32;
            let res = tulipix_music::podcasts::subscribe_with_progress(&pool, url.trim(), &feed, move |done, total| {
                let pct = ((done as f64 / total.max(1) as f64) * 100.0) as i32;
                if pct == last_pct { return; }
                last_pct = pct;
                let frac = done as f32 / total.max(1) as f32;
                let p = prog.clone();
                let _ = p.upgrade_in_event_loop(move |w| {
                    w.set_music_podcast_add_frac(frac);
                    w.set_music_podcast_add_status(format!("{done} / {total} episodes").into());
                });
            }).await;
            let ok = res.is_ok();
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_podcast_add_busy(false);
                w.set_music_podcast_add_frac(0.0);
                if ok {
                    w.set_music_podcast_add_status("".into());
                    w.set_music_podcast_add_open(false);
                    w.set_music_podcast_url("".into());
                    populate_podcasts(&w);
                    populate_podcast_latest(&w);
                } else {
                    w.set_music_podcast_add_status("Subscribe failed".into());
                }
            });
        });
    });
    // Clear ALL subscriptions + their cached files, with live removal progress.
    let w = window.as_weak();
    window.on_music_podcast_reset_all(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_reset_busy(true);
        w0.set_music_podcast_reset_frac(0.0);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_podcast_reset_busy(false));
                return;
            };
            let paths: Vec<(Option<String>,)> = sqlx::query_as(
                "SELECT downloaded_path FROM podcast_episodes WHERE downloaded_path IS NOT NULL")
                .fetch_all(&pool).await.unwrap_or_default();
            let total = paths.len().max(1);
            for (i, (p,)) in paths.iter().enumerate() {
                if let Some(p) = p { let _ = std::fs::remove_file(p); }
                let frac = (i + 1) as f32 / total as f32;
                let wk = weak.clone();
                let _ = wk.upgrade_in_event_loop(move |w| w.set_music_podcast_reset_frac(frac));
            }
            let _ = sqlx::query("DELETE FROM podcast_episodes").execute(&pool).await;
            let _ = sqlx::query("DELETE FROM podcasts").execute(&pool).await;
            let _ = sqlx::query("VACUUM").execute(&pool).await;
            if let Ok(mut g) = cur_podcast_id().lock() { *g = -1; }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_music_podcast_reset_busy(false);
                w.set_music_podcast_reset_frac(0.0);
                w.set_music_podcast_detail_open(false);
                refresh_podcast_views(&w);
            });
        });
    });
    // Set a custom artwork for the open podcast (detail page → "Change art").
    let w = window.as_weak();
    window.on_music_podcast_set_thumb(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let pid = cur_podcast_id().lock().map(|g| *g).unwrap_or(-1);
        if pid < 0 { return; }
        let Some(file) = rfd::FileDialog::new().set_title("Choose podcast artwork")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("podcast_art");
            let _ = std::fs::create_dir_all(&dir);
            let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
            let dest = dir.join(format!("pod-custom-{pid}.{ext}"));
            if std::fs::copy(&file, &dest).is_err() { return; }
            let dest_s = dest.to_string_lossy().to_string();
            // Store the local path in custom_image (not image_url) so a later
            // feed refresh — which overwrites image_url — can never wipe it.
            // resolve_artwork loads the local file directly.
            let _ = sqlx::query("UPDATE podcasts SET custom_image = ? WHERE id = ?")
                .bind(&dest_s).bind(pid).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                load_podcast_detail(&w, pid);
                populate_podcasts(&w);
                populate_podcast_latest(&w);
            });
        });
    });
    // Search across subscribed podcasts (title/author/category/feed + episode
    // titles) — filters the Subscribed grid. Empty query restores the full list.
    let w = window.as_weak();
    window.on_music_podcast_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        let q = q.to_string();
        // Route the query to whichever podcast tab (or the single-podcast page) is
        // active, so each surface filters independently.
        let which = if w0.get_music_podcast_detail_open() { "detail" }
            else { match w0.get_music_podcast_tab().as_str() {
                "home" => "home", "trends" => "trends", "downloads" => "downloads", _ => "subs" } };
        if let Ok(mut f) = pod_filters().lock() {
            match which { "home" => f.home = q.clone(), "trends" => f.trends = q.clone(),
                "downloads" => f.downloads = q.clone(), "detail" => f.detail = q.clone(), _ => f.subs = q.clone() };
        }
        match which {
            "home" => populate_podcast_latest(&w0),
            "trends" => render_trends(&w0),
            "downloads" => { w0.set_music_podcast_dl_page(0); populate_podcast_downloads(&w0); }
            "detail" => { w0.set_music_podcast_d_page(0);
                if let Ok(g) = cur_podcast_id().lock() { load_podcast_detail(&w0, *g); } }
            _ => { w0.set_music_podcast_cat("All".into()); w0.set_music_podcast_sub_page(0); populate_podcasts(&w0); }
        }
    });
    // Trends — subscribe to a baked feed by its index in
    // resources/podcast-feeds.txt.
    let w = window.as_weak();
    window.on_music_podcast_trend_subscribe(move |idx| {
        let Some(w0) = w.upgrade() else { return; };
        let feeds = trend_feed_urls();
        let Some(url) = feeds.get(idx as usize).cloned() else { return; };
        w0.set_music_podcast_subscribing(true);
        w0.set_music_podcast_subscribe_frac(0.0);
        w0.set_music_podcast_subscribe_status("".into());
        subscribe_feed_with_progress(w.clone(), url);
    });
    // Trends — sort + pagination (no network; renders from the session cache).
    let w = window.as_weak();
    window.on_music_podcast_trends_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_trends_sort(s);
        w0.set_music_podcast_trends_page(0);
        render_trends(&w0);
    });
    let w = window.as_weak();
    window.on_music_podcast_trends_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_podcast_trends_page() + d).clamp(0, (w0.get_music_podcast_trends_pages() - 1).max(0));
        w0.set_music_podcast_trends_page(next);
        render_trends(&w0);
    });
    // Trends — open the read-only info card for one feed (count + latest only).
    let w = window.as_weak();
    window.on_music_podcast_trend_info(move |idx| {
        let Some(w0) = w.upgrade() else { return; };
        let feeds = trend_feed_urls();
        let Some(url) = feeds.get(idx as usize).cloned() else { return; };
        if let Ok(mut g) = cur_trend_idx().lock() { *g = idx; }
        if let Ok(mut g) = cur_info_feed().lock() { *g = url.clone(); }
        // Pre-fill from the cache; the network fetch fills count/latest/desc.
        let meta = trend_cache().lock().ok().and_then(|g| g.get(idx as usize).cloned());
        w0.set_music_podcast_info_open(true);
        w0.set_music_podcast_info_loading(true);
        w0.set_music_podcast_info_episodes(0);
        w0.set_music_podcast_info_latest("".into());
        w0.set_music_podcast_info_desc("".into());
        if let Some(m) = &meta {
            w0.set_music_podcast_info_title(m.title.clone().into());
            w0.set_music_podcast_info_author(m.author.clone().into());
            w0.set_music_podcast_info_category(m.category.clone().into());
            w0.set_music_podcast_info_image(m.art.as_ref().and_then(|p| slint::Image::load_from_path(p).ok()).unwrap_or_default());
        }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let subscribed = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM podcasts WHERE feed_url = ?")
                .bind(&url).fetch_one(&pool).await.unwrap_or(0) > 0;
            let client = tulipix_core::net::http().clone();
            let (mut count, mut latest, mut desc) = (0i32, String::new(), String::new());
            if let Ok(resp) = client.get(&url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
                if let Ok(xml) = resp.text().await {
                    let feed = tulipix_music::podcasts::parse_feed(&xml);
                    count = feed.episodes.len() as i32;
                    latest = feed.episodes.iter().filter_map(|e| e.published).max().map(fmt_date).unwrap_or_default();
                    desc = feed.description;
                }
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_podcast_info_loading(false);
                w.set_music_podcast_info_episodes(count);
                w.set_music_podcast_info_latest(latest.into());
                w.set_music_podcast_info_desc(desc.into());
                w.set_music_podcast_info_subscribed(subscribed);
            });
        });
    });
    // Info card for a SUBSCRIBED podcast (by DB id) — straight from the DB, no
    // network. Shares the same popup as Trends.
    let w = window.as_weak();
    window.on_music_podcast_info_open_id(move |pid| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_podcast_info_open(true);
        w0.set_music_podcast_info_loading(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
                "SELECT COALESCE(title, feed_url), COALESCE(author,''), COALESCE(category,''), COALESCE(description,''),
                        COALESCE(NULLIF(custom_image,''), image_url, ''), feed_url
                 FROM podcasts WHERE id = ?").bind(pid as i64).fetch_optional(&pool).await.ok().flatten();
            let Some((title, author, category, desc, img, feed_url)) = row else { return; };
            let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?")
                .bind(pid as i64).fetch_one(&pool).await.unwrap_or(0);
            let latest_pub: Option<i64> = sqlx::query_scalar("SELECT MAX(published) FROM podcast_episodes WHERE podcast_id = ?")
                .bind(pid as i64).fetch_one(&pool).await.ok().flatten();
            let latest = latest_pub.map(fmt_date).unwrap_or_default();
            // Track this feed for Save / thumb; map to a trend index if it's baked.
            let tidx = trend_feed_urls().iter().position(|u| u == &feed_url).map(|p| p as i32).unwrap_or(-1);
            if let Ok(mut g) = cur_trend_idx().lock() { *g = tidx; }
            if let Ok(mut g) = cur_info_feed().lock() { *g = feed_url.clone(); }
            let client = tulipix_core::net::http().clone();
            let art = resolve_artwork(&client, &format!("pod-{pid}"), &img).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_podcast_info_loading(false);
                w.set_music_podcast_info_title(title.into());
                w.set_music_podcast_info_author(author.into());
                w.set_music_podcast_info_category(category.into());
                w.set_music_podcast_info_desc(desc.into());
                w.set_music_podcast_info_episodes(count as i32);
                w.set_music_podcast_info_latest(latest.into());
                w.set_music_podcast_info_subscribed(true);
                w.set_music_podcast_info_image(art.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
            });
        });
    });
    // Info card — save the edited category (cache override + DB).
    let w = window.as_weak();
    window.on_music_podcast_info_save_category(move || {
        let Some(w0) = w.upgrade() else { return; };
        let url = cur_info_feed().lock().map(|g| g.clone()).unwrap_or_default();
        if url.is_empty() { return; }
        let idx = cur_trend_idx().lock().map(|g| *g).unwrap_or(-1);
        let cat = w0.get_music_podcast_info_category().to_string();
        if idx >= 0 { if let Ok(mut g) = trend_cache().lock() { if let Some(m) = g.get_mut(idx as usize) { m.category = cat.clone(); } } }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let _ = sqlx::query("UPDATE podcasts SET category = ? WHERE feed_url = ?").bind(&cat).bind(&url).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(|w| { render_trends(&w); populate_podcasts(&w); });
        });
    });
    // Info card — replace the thumbnail (cache + DB custom_image).
    let w = window.as_weak();
    window.on_music_podcast_info_set_thumb(move || {
        let Some(_w0) = w.upgrade() else { return; };
        let url = cur_info_feed().lock().map(|g| g.clone()).unwrap_or_default();
        if url.is_empty() { return; }
        let idx = cur_trend_idx().lock().map(|g| *g).unwrap_or(-1);
        let Some(file) = rfd::FileDialog::new().set_title("Choose podcast artwork")
            .add_filter("Images", &["jpg", "jpeg", "png", "webp"]).pick_file() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("podcast_art");
            let _ = std::fs::create_dir_all(&dir);
            let ext = file.extension().and_then(|e| e.to_str()).unwrap_or("jpg");
            // Name by DB id when subscribed (so the grid's pod-{id} lookup finds it),
            // else by trend index.
            let pid: Option<i64> = sqlx::query_scalar("SELECT id FROM podcasts WHERE feed_url = ?").bind(&url).fetch_optional(&pool).await.ok().flatten();
            let stem = match pid { Some(id) => format!("pod-custom-{id}"), None => format!("trend-custom-{idx}") };
            let dest = dir.join(format!("{stem}.{ext}"));
            if std::fs::copy(&file, &dest).is_err() { return; }
            let dest_s = dest.to_string_lossy().to_string();
            let _ = sqlx::query("UPDATE podcasts SET custom_image = ? WHERE feed_url = ?").bind(&dest_s).bind(&url).execute(&pool).await;
            if idx >= 0 { if let Ok(mut g) = trend_cache().lock() { if let Some(m) = g.get_mut(idx as usize) { m.art = Some(dest.clone()); } } }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_podcast_info_image(slint::Image::load_from_path(&dest).unwrap_or_default());
                render_trends(&w);
                populate_podcasts(&w);
            });
        });
    });
    // Info card — Subscribe to the open feed.
    let w = window.as_weak();
    window.on_music_podcast_info_subscribe(move || {
        let Some(w0) = w.upgrade() else { return; };
        let url = cur_info_feed().lock().map(|g| g.clone()).unwrap_or_default();
        if url.is_empty() { return; }
        w0.set_music_podcast_subscribing(true);
        w0.set_music_podcast_subscribe_frac(0.0);
        w0.set_music_podcast_subscribe_status("".into());
        subscribe_feed_with_progress(w.clone(), url);
    });
    // Open the owning podcast's page from a downloaded episode's context menu.
    let w = window.as_weak();
    window.on_music_podcast_goto_show(move |ep_id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let pid: Option<i64> = sqlx::query_scalar("SELECT podcast_id FROM podcast_episodes WHERE id = ?")
                .bind(ep_id as i64).fetch_optional(&pool).await.ok().flatten();
            if let Some(pid) = pid {
                let _ = weak.upgrade_in_event_loop(move |w| open_podcast(&w, pid as i32));
            }
        });
    });
    let w = window.as_weak();
    window.on_music_podcast_open(move |i| {
        if let Some(w) = w.upgrade() { open_podcast(&w, i); }
    });
    // Play an episode by DB id (works from any list — detail / Home / Downloads /
    // mini-player). Prefers the offline copy; switches the bottom player to
    // podcast mode; marks the episode played.
    let w = window.as_weak();
    window.on_music_podcast_play(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let row: Option<(String, String, Option<String>, String, i64, String, String, f64)> = sqlx::query_as(
                "SELECT e.title, e.audio_url, e.downloaded_path, COALESCE(e.image_url,''),
                        p.id, COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), COALESCE(p.title,''),
                        COALESCE(e.position_s, 0)
                 FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            let Some((title, url, dl, eimg, pid, pimg, show, resume_pos)) = row else { return; };
            if url.is_empty() && dl.is_none() { return; }
            PODCAST_NOW_ID.store(id as i64, std::sync::atomic::Ordering::Relaxed);
            let client = tulipix_core::net::http().clone();
            let art = match cache_artwork(&client, &format!("ep-{id}"), &eimg).await {
                Some(p) => Some(p),
                None => cache_artwork(&client, &format!("pod-{pid}"), &pimg).await,
            };
            let _ = sqlx::query("UPDATE podcast_episodes SET played = 1 WHERE id = ?").bind(id as i64).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                let src = dl.clone().filter(|p| std::path::Path::new(p).exists()).unwrap_or(url);
                play_music_url(&w, &src, &title);
                // Marks the row in every episode list — Home, Downloads and the
                // show page all render the same component.
                w.set_music_podcast_np_id(id);
                w.set_music_player_mode("podcast".into());
                w.set_music_yt_now_video(false);
                w.set_music_np_title(title.into());
                w.set_music_np_sub(if show.is_empty() { "Podcast".into() } else { show.into() });
                w.set_music_np_accent(slint::Color::from_rgb_u8(0x8b, 0x5c, 0xf6));
                if let Some(p) = art { if let Ok(im) = slint::Image::load_from_path(&p) { w.set_music_np_art(im); } }
                // Open the mini-player centered in the page (podcasts have no bottom bar).
                w.invoke_music_center_mini();
                // Carry the chosen playback speed across episodes.
                let sp = w.get_music_podcast_speed();
                music_ipc(&["set_property", "audio-pitch-correction", "yes"]);
                if (sp - 1.0).abs() > 0.01 { music_ipc(&["set_property", "speed", &sp.to_string()]); }
                // Resume where the last listen stopped (same pattern as audiobooks).
                if resume_pos > 5.0 { music_ipc(&["seek", &resume_pos.to_string(), "absolute"]); }
                refresh_podcast_views(&w);
                // Queue reflects WHERE playback started: a single-podcast page keeps
                // its own episodes; the Downloads tab queues the downloads list;
                // anything else (Home) queues the Latest-episodes feed.
                if !w.get_music_podcast_detail_open() {
                    if w.get_music_podcast_tab().as_str() == "downloads" {
                        w.set_music_podcast_d_episodes(w.get_music_podcast_downloads());
                    } else {
                        w.set_music_podcast_d_episodes(w.get_music_podcast_latest());
                    }
                }
            });
        });
    });
    // ── Cross-show episode queue ────────────────────────────────────────────
    let w = window.as_weak();
    window.on_music_podcast_queue_add(move |id| {
        let Some(w0) = w.upgrade() else { return; };
        if let Ok(mut g) = podcast_queue().lock() {
            // Tapping the button on something already queued removes it, so
            // the one control both adds and undoes.
            match g.iter().position(|e| *e == id) {
                Some(i) => { g.remove(i); }
                None => g.push(id),
            }
        }
        podcast_queue_sync(&w0);
    });
    let w = window.as_weak();
    window.on_music_podcast_queue_remove(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        if let Ok(mut g) = podcast_queue().lock() {
            if (i as usize) < g.len() { g.remove(i as usize); }
        }
        podcast_queue_sync(&w0);
    });
    let w = window.as_weak();
    window.on_music_podcast_queue_clear(move || {
        let Some(w0) = w.upgrade() else { return; };
        if let Ok(mut g) = podcast_queue().lock() { g.clear(); }
        podcast_queue_sync(&w0);
    });
    // Skip ±N seconds in the playing episode.
    window.on_music_podcast_skip(move |secs| {
        music_ipc(&["seek", &secs.to_string(), "relative"]);
    });
    // Set podcast playback speed (pitch-preserving) on the live stream.
    let w = window.as_weak();
    window.on_music_podcast_set_speed(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let s = (s as f64).clamp(0.5, 3.0);
        w0.set_music_podcast_speed(s as f32);
        music_ipc(&["set_property", "audio-pitch-correction", "yes"]);
        music_ipc(&["set_property", "speed", &s.to_string()]);
    });
    // Save (download) an episode by id to the app cache — streamed with a live
    // inline progress bar (podcast-dl-id / podcast-dl-frac drive the UI).
    // Episodes download ONE at a time: extra clicks queue up behind the active
    // one and the header bar badge shows what's still waiting.
    let w = window.as_weak();
    window.on_music_podcast_download(move |id| {
        let Some(w0) = w.upgrade() else { return; };
        {
            let Ok(mut q) = podcast_dl_queue().lock() else { return; };
            // Already active or already queued — nothing to add.
            if w0.get_music_podcast_dl_id() == id || q.contains(&id) { return; }
            q.push_back(id);
            let active = if w0.get_music_podcast_dl_id() >= 0 { 1 } else { 0 };
            w0.set_music_podcast_dl_queue(q.len() as i32 + active);
        }
        // Re-render lists so the queued row's ⤓ flips to "Queued" right away.
        refresh_podcast_views(&w0);
        // One drain worker at a time; an existing one picks the new entry up.
        if PODCAST_DL_ACTIVE.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            loop {
                let (next, remaining) = match podcast_dl_queue().lock() {
                    Ok(mut g) => { let n = g.pop_front(); (n, g.len()) }
                    Err(_) => (None, 0),
                };
                let Some(id) = next else { break; };
                let wk = weak.clone();
                let _ = wk.upgrade_in_event_loop(move |w| w.set_music_podcast_dl_queue(remaining as i32 + 1));
                podcast_download_one(weak.clone(), id).await;
            }
            PODCAST_DL_ACTIVE.store(false, std::sync::atomic::Ordering::SeqCst);
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_podcast_dl_queue(0));
        });
    });
    // Open the show-notes / transcript panel for an episode by id.
    let w = window.as_weak();
    window.on_music_podcast_transcript(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("podcasts").await else { return; };
            let row: Option<(String, String)> = sqlx::query_as(
                "SELECT COALESCE(title,''), COALESCE(description,'') FROM podcast_episodes WHERE id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Some((title, desc)) = row {
                    w.set_music_podcast_transcript_title(title.into());
                    w.set_music_podcast_transcript_text(desc.into());
                    w.set_music_podcast_transcript_open(true);
                }
            });
        });
    });
}

/// Classify a watched folder, seed one library row + progress row per
/// section that has matching files, and kick the scans that fill the tile
/// models. Shared by the folder picker and the startup restore path.
/// Apply the folder → music-section map to `is_audiobook`, then refresh the
/// roots list and kick the tag ingest. Ordered, because the folder counts and
/// every My Music view read the flag this sets.
fn apply_music_folder_sections(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("music").await {
            let sections = load_folder_sections();
            if let Err(e) =
                tulipix_music::audiobooks::apply_folder_sections(&pool, &sections).await
            {
                tracing::warn!(error = %e, "audiobook section flags");
            }
        }
        let _ = weak.upgrade_in_event_loop(|w| {
            populate_folder_roots(&w);
            ingest_music_tags(w.as_weak());
        });
    });
}

fn add_folder_path(window: &MainWindow, path: PathBuf) {
    let counts = classify_folder(&path);
    add_folder_counted(window, path, counts);
}

/// The same, with the classification walk already done.
///
/// `classify_folder` walks the WHOLE tree and stats every file. On a warm page
/// cache that is milliseconds; on the first launch after a boot it is the
/// single slowest thing the app does, and it used to run on the main thread
/// before the window was shown — which is exactly why the first launch dragged
/// and every launch after it felt instant. The startup restore now walks off
/// the main thread and calls this when it has the counts (see
/// `add_folder_path_deferred`); an explicit "Add folder" still goes through
/// `add_folder_path`, where the user has just picked the folder and is
/// expecting it to be looked at.
fn add_folder_counted(
    window: &MainWindow,
    path: PathBuf,
    counts: std::collections::HashMap<&'static str, i64>,
) {
    let total_all: i64 = counts.values().sum();
    tracing::info!(total_all, "classified totals");

    // Seed library rows + initial progress rows for every section that
    // matched at least one file. Progress rows are kept around (frozen)
    // after the scan finishes so the user can see the final count.
    let model = window.get_library_rows();
    let mut rows: Vec<LibraryRow> = (0..model.row_count())
        .map(|i| model.row_data(i).unwrap())
        .collect();
    let mut progress_rows: Vec<ScanProgress> = Vec::new();
    let mut scheduled: Vec<(String, &'static str, i64)> = Vec::new();
    let path_str = path.display().to_string();
    for section in ["photos", "videos", "music", "books"] {
        let total = counts.get(section).copied().unwrap_or(0);
        if total == 0 { continue; }
        // Dedup: a rescan / startup-restore re-adds the same watched folder.
        // If this (path, section) already has a library row, reuse it and
        // mark it scanning instead of appending a duplicate copy.
        let lib_id = if let Some(i) = rows.iter().position(
            |r| r.path.as_str() == path_str.as_str() && r.section.as_str() == section)
        {
            rows[i].r#last_scan = "scanning".into();
            rows[i].item_count = 0;
            rows[i].id.to_string()
        } else {
            let id = format!("lib-{}-{}", section, rows.len() + 1);
            let music_section = if section == "music" {
                music_section_label(load_folder_sections().get(&path_str).map(|s| s.as_str()).unwrap_or("mymusic")).into()
            } else { slint::SharedString::new() };
            rows.push(LibraryRow {
                id:          id.clone().into(),
                path:        path_str.clone().into(),
                section:     section.into(),
                r#last_scan: "scanning".into(),
                item_count:  0,
                size_pretty: "0 B".into(),
                cadence:     "default".into(),
                music_section,
            });
            id
        };
        progress_rows.push(ScanProgress {
            section: section.into(),
            label: section_label(section).into(),
            total: total as i32,
            added: 0,
            failed: 0,
            active: true,
            last_error: slint::SharedString::new(),
        });
        reset_section_counters(section, total as i32);
        scheduled.push((lib_id, section, total));
    }
    window.set_library_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    // Silent scans (startup restore / post-save refresh) skip the popup; the
    // grid still fills via each section scan.
    if !scan_silent() {
        window.set_scan_progress(slint::ModelRc::new(slint::VecModel::from(progress_rows)));
        window.set_scan_active(total_all > 0);
    }
    rebuild_scan_rows(window);

    for (lib_id, section, total) in scheduled {
        kick_section_scan(window, path.clone(), lib_id, section, total);
    }
}

/// Startup restore: classify off the main thread, then seed the rows and kick
/// the scans back on it. Nothing here is visible before the first frame — the
/// grids fill as each section's scan reports in, exactly as they do for a
/// folder added by hand — so the walk has no business holding up the window.
fn add_folder_path_deferred(window: &MainWindow, path: PathBuf) {
    let weak = window.as_weak();
    std::thread::spawn(move || {
        let counts = classify_folder(&path);
        let _ = weak.upgrade_in_event_loop(move |w| add_folder_counted(&w, path, counts));
    });
}

fn section_label(section: &str) -> &'static str {
    match section {
        "photos" => "Photos",
        "videos" => "Videos",
        "music"  => "Music",
        "books"  => "Books",
        _ => "Other",
    }
}
