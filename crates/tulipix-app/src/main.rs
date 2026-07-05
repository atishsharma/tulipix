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
    anime4k_shader_args, bundled_present, cycle_folder_section, dirs_default, dirs_default_documents,
    fmt_clock, fmt_date, folder_sections_path, fuzzy_score, histogram_image,
    human_size, kill_all_mpv, load_folder_sections, load_watched_folders,
    music_ipc, music_proc, music_section_key, music_section_label,
    now_secs, on_path, pool_for, set_folder_section, spawn_mpv_windowed,
    video_ipc, watched_folders_path, MUSIC_GEN,
};
// MPRIS/SMTC handle storage now lives in common; setup_media_controls in main
// writes to it.
pub(crate) use tulipix_common::MEDIA_CONTROLS;
// Photos section (grid/library/editor/viewer helpers) lives in
// tulipix-sec-photos; glob-import so the photo `window.on_*` callbacks that
// stay in main keep calling `show_photo_at` / `open_editor` / … unqualified.
use tulipix_sec_photos::*;
// Music section (My Music/Podcasts/Radio/Audiobooks + YouTube tabs) lives in
// tulipix-sec-music; glob-import so the music `window.on_*` callbacks that stay
// in main keep calling its helpers unqualified.
use tulipix_sec_music::*;
// Videos library (grid/show models, season parsing, TMDB, discover) lives in
// tulipix-sec-videos; the embedded player stays in main and calls into it.
use tulipix_sec_videos::*;

/// Idle threshold meaning "never" — pushed a year out so the idle listener never
/// fires while auto-lock is disabled (the default).
const IDLE_NEVER_SECS: u64 = 60 * 60 * 24 * 365;

#[cfg(feature = "dev-reload")]
mod dev_reload;
#[cfg(feature = "hot")]
mod hot;
#[cfg(feature = "embedded-mpv")]
mod mpv;
// No-op stand-in when libmpv isn't linked (e.g. Windows --no-default-features).
#[cfg(not(feature = "embedded-mpv"))]
#[path = "mpv_stub.rs"]
mod mpv;
mod books;
mod profile_image;

fn detect_dark() -> bool {
    // dark-light v2 reads the XDG portal color-scheme via zbus on Linux, which
    // needs a Tokio reactor. Run the probe inside a one-shot single-threaded
    // runtime so it works regardless of caller context.
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
    let dark = match choice {
        ThemeChoice::Light => false,
        ThemeChoice::ExtraDark => true,
        ThemeChoice::System => detect_dark(),
    };
    window.set_dark(dark);
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

static APP_START: OnceLock<std::time::Instant> = OnceLock::new();

fn main() -> Result<()> {
    let _ = APP_START.set(std::time::Instant::now());
    // The embedded player hands libmpv frames to Slint as a BorrowedOpenGLTexture,
    // which requires the OpenGL skia surface (so the rendering notifier yields a
    // NativeOpenGL context). Force it unless the user overrode the backend.
    if std::env::var_os("SLINT_BACKEND").is_none() {
        // Safety: set at the very top of main, before any threads spawn.
        // skia-opengl is required for the embedded-mpv texture path; a lean
        // femtovg build (no embedded player) uses the femtovg backend instead.
        #[cfg(feature = "renderer-skia")]
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-skia-opengl"); }
        #[cfg(all(not(feature = "renderer-skia"), feature = "renderer-femtovg"))]
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-femtovg"); }
    }
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .init();
    tulipix_core::crash::install_panic_hook();
    let _ = tulipix_core::logging::LOG.write_event("info", "tulipix", "startup");

    // zbus (transitive via rfd/xdg-portal + dark-light + ashpd) requires a
    // Tokio reactor in scope for the calling thread. Enter a multi-thread
    // runtime so any sync zbus call has a reactor regardless of caller.
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    let _rt_guard = rt.enter();

    if std::env::args().any(|a| a == "--trace-caps") {
        tulipix_core::caps::enable_trace();
    }
    tracing::info!(version = env!("CARGO_PKG_VERSION"), "tulipix starting");
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

    let window = MainWindow::new()?;
    register_bundled_fonts();
    tulipix_platform::install_menubar(&tulipix_platform::default_menubar());
    // Tray icon — the compiled-in tulip mark (64px, RGBA-decoded here so the
    // platform crate needs no image dependency).
    let tray_icon_rgba = image::load_from_memory(include_bytes!("../../../resources/icons/tulipix-64.png"))
        .ok().map(|img| { let rgba = img.to_rgba8(); let (w, h) = rgba.dimensions(); (rgba.into_raw(), w, h) });
    let tray_ok = tulipix_platform::init_tray(tray_icon_rgba);
    TRAY_ACTIVE.store(tray_ok, std::sync::atomic::Ordering::Relaxed);

    // ── Embedded player (np.p3.player.*) ── libmpv renders into a GL texture
    // presented by Slint. The rendering notifier is the only place the GL
    // context is current, so frames are produced there.
    mpv::set_window(window.as_weak());
    {
        let wk = window.as_weak();
        if let Err(e) = window.window().set_rendering_notifier(move |state, api| {
            match state {
                slint::RenderingState::BeforeRendering => {
                    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                        if let Some(w) = wk.upgrade() {
                            let sz = w.window().size();
                            if let Some(img) = mpv::render_frame(get_proc_address, sz.width as i32, sz.height as i32) {
                                w.set_player_frame(img);
                            }
                        }
                    }
                }
                slint::RenderingState::RenderingTeardown => mpv::teardown(),
                _ => {}
            }
        }) {
            tracing::error!(error = %e, "rendering notifier unavailable — embedded video disabled");
        }
    }
    // mpv property changes → UI.
    {
        let wk = window.as_weak();
        mpv::on_update(move |u| {
            let wk = wk.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = wk.upgrade() else { return; };
                use mpv::Update::*;
                match u {
                    TimePos(t)  => { w.set_player_pos(t as f32); w.set_player_pos_label(fmt_clock(t).into()); }
                    Duration(d) => { w.set_player_duration(d as f32); w.set_player_dur_label(fmt_clock(d).into()); }
                    Pause(p)    => w.set_player_paused(p),
                    Volume(v)   => w.set_player_volume(v as f32),
                    Speed(s)    => w.set_player_speed(s as f32),
                    Mute(m)     => w.set_player_muted(m),
                    Idle(i)     => w.set_player_buffering(i && !w.get_player_paused()),
                    Chapter(_)  => {}
                    FileLoaded  => {
                        // Apply the pending resume seek now the file is loaded.
                        if let Some(r) = player_resume().lock().ok().and_then(|mut g| g.take()) {
                            if r > 1.0 { mpv::command(&["seek", &r.to_string(), "absolute"]); }
                        }
                        // Subtitle styling / HDR / audio / interpolation / sleep.
                        apply_playback_prefs();
                        // Auto-load the best sibling subtitle if one exists next to
                        // the video (np.p3.sub.local); mpv lists it alongside any
                        // embedded tracks and selects it.
                        if let Some(p) = player_path().lock().ok().and_then(|g| g.clone()) {
                            if let Some(c) = tulipix_videos::sub_local::autopick(&p, &["en", "eng"]) {
                                let sp = c.path.to_string_lossy().into_owned();
                                mpv::command(&["sub-add", &sp, "select"]);
                                tracing::info!(sub = %sp, "auto-loaded sibling subtitle");
                            }
                        }
                    }
                    EndFile => {
                        if w.get_player_open() {
                            // Natural end → mark finished, then close.
                            if let Some(id) = *player_item().lock().unwrap_or_else(|p| p.into_inner()) {
                                let handle = tokio::runtime::Handle::current();
                                handle.spawn(async move {
                                    if let Ok(pool) = pool_for("videos").await {
                                        let _ = tulipix_videos::watch_progress::mark_finished(&pool, id, true).await;
                                    }
                                });
                            }
                            // Distinguish a real EOF from a `stop`: only auto-close
                            // when we're near the end.
                            let (pos, dur) = (w.get_player_pos() as f64, w.get_player_duration() as f64);
                            if dur > 0.0 && pos >= dur * 0.98 { player_close(&w); }
                        }
                    }
                }
            });
        });
    }
    // Track / chapter / hwdec lists → UI (rebuilt on each file load).
    {
        let wk = window.as_weak();
        mpv::on_tracks(move |tracks, chapters, hwdec| {
            let wk = wk.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = wk.upgrade() else { return; };
                let mut audio: Vec<PlayerTrack> = Vec::new();
                let mut subs: Vec<PlayerTrack> = vec![PlayerTrack { id: -1, label: "Off".into(), selected: false }];
                for t in &tracks {
                    let row = PlayerTrack { id: t.id as i32, label: t.label.clone().into(), selected: t.selected };
                    match t.kind.as_str() {
                        "audio" => audio.push(row),
                        "sub"   => { if t.selected { subs[0].selected = false; } subs.push(row); }
                        _ => {}
                    }
                }
                let chaps: Vec<PlayerChapter> = chapters.iter().map(|c| PlayerChapter {
                    index: c.index as i32, title: c.title.clone().into(), time: fmt_clock(c.time).into(),
                }).collect();
                w.set_player_audio_tracks(slint::ModelRc::new(slint::VecModel::from(audio)));
                w.set_player_sub_tracks(slint::ModelRc::new(slint::VecModel::from(subs)));
                w.set_player_chapters(slint::ModelRc::new(slint::VecModel::from(chaps)));
                w.set_player_hwdec(hwdec.into());
            });
        });
    }
    // Player control callbacks.
    register_player_controls(&window);
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

    // Initial theme — start light (overridden below by persisted setting).
    apply_theme_choice(&window, ThemeChoice::Light);

    // Singleton 80 ms flush ticker — drains atomic counters into the UI in
    // one coalesced post per tick so per-file scan throughput doesn't spam
    // the main thread.
    spawn_progress_flusher(window.as_weak());

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

    // Music-section theme toggle (sun/moon/star) — persist the light/dark/OLED
    // choice so it survives restarts (np.p5.music.theme-memory).
    let w = window.as_weak();
    window.on_music_theme_changed(move || {
        let Some(w) = w.upgrade() else { return; };
        let mode = if w.get_music_light() { "light" }
                   else if w.get_music_oled() { "oled" }
                   else { "dark" };
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.advanced.insert("music.theme".into(), mode.to_string());
        if let Err(e) = s.save() { tracing::warn!(error = %e, "save settings (music theme)"); }
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

    /// Classify a watched folder, seed one library row + progress row per
    /// section that has matching files, and kick the scans that fill the tile
    /// models. Shared by the folder picker and the startup restore path.
    fn add_folder_path(window: &MainWindow, path: PathBuf) {
        let counts = classify_folder(&path);
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

    fn section_label(section: &str) -> &'static str {
        match section {
            "photos" => "Photos",
            "videos" => "Videos",
            "music"  => "Music",
            "books"  => "Books",
            _ => "Other",
        }
    }

    // Add a freshly-picked folder straight into one music sub-section
    // (mymusic|podcasts|audiobooks|radio|youtube). The Add button is
    // section-scoped — whatever music tab is open is where the folder lands; no
    // universal "which section?" popup. Audiobooks get metadata-gated flagging
    // (np.p5.music.audiobook-detect): only files that look like audiobooks are
    // kept (whole-folder fallback when none carry the metadata).
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

    // ── Books: library view tabs + reader (np.p4.books.*) ──
    let w = window.as_weak();
    window.on_book_set_view(move |v| {
        if let Some(w) = w.upgrade() { w.set_book_view(v.clone()); refresh_books(&w); }
    });
    let w = window.as_weak();
    window.on_book_search(move |_q| {
        // `book-query` is bound in the .slint, so the window already holds the text.
        if let Some(w) = w.upgrade() { refresh_books(&w); }
    });
    let w = window.as_weak();
    window.on_book_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let mode = s.to_string();
        let cur_mode = w0.get_book_sort().to_string();
        let cur_dir = w0.get_book_sort_dir().to_string();
        // Re-click flips direction; switching mode resets (title/author = A→Z asc,
        // recent = newest first / desc).
        let dir = if mode == cur_mode {
            if cur_dir == "asc" { "desc" } else { "asc" }
        } else if mode == "recent" { "desc" } else { "asc" };
        w0.set_book_sort(mode.into());
        w0.set_book_sort_dir(dir.into());
        refresh_books(&w0);
    });
    let w = window.as_weak();
    window.on_book_open(move |idx| { open_book(w.clone(), idx); });
    let w = window.as_weak();
    window.on_book_reader_close(move || {
        if let Some(w) = w.upgrade() {
            save_reader_progress();
            w.set_book_reader_open(false);
        }
        reader_clear();
    });
    let w = window.as_weak();
    window.on_book_next(move || { if let Some(w) = w.upgrade() { reader_step(&w, 1); } });
    let w = window.as_weak();
    window.on_book_prev(move || { if let Some(w) = w.upgrade() { reader_step(&w, -1); } });
    let w = window.as_weak();
    window.on_book_scrub(move |frac| { if let Some(w) = w.upgrade() { reader_scrub(&w, frac); } });
    let w = window.as_weak();
    window.on_book_toggle_spread(move || { if let Some(w) = w.upgrade() { reader_toggle_spread(&w); } });
    let w = window.as_weak();
    window.on_book_toggle_rtl(move || { if let Some(w) = w.upgrade() { reader_toggle_rtl(&w); } });
    let w = window.as_weak();
    window.on_book_toggle_invert(move || { if let Some(w) = w.upgrade() { reader_toggle_invert(&w); } });
    let w = window.as_weak();
    window.on_book_toggle_guided(move || {
        // np.p5.books.comic-guided — panel-by-panel reading on/off.
        if let Some(w) = w.upgrade() {
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    s.guided = !s.guided;
                    s.panel = 0;
                    s.panels_page = usize::MAX; // force re-detect on render
                }
            });
            reader_render(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_set_font(move |px| { if let Some(w) = w.upgrade() { reader_set_typo(&w, Some(px), None, None, None); } });
    let w = window.as_weak();
    window.on_book_set_line(move |lh| { if let Some(w) = w.upgrade() { reader_set_typo(&w, None, Some(lh), None, None); } });
    let w = window.as_weak();
    window.on_book_set_family(move |fam| { if let Some(w) = w.upgrade() { reader_set_typo(&w, None, None, Some(fam), None); } });
    let w = window.as_weak();
    window.on_book_set_margin(move |m| { if let Some(w) = w.upgrade() { reader_set_typo(&w, None, None, None, Some(m)); } });
    let w = window.as_weak();
    window.on_book_reflow(move |width, height| { if let Some(w) = w.upgrade() { reader_reflow(&w, width, height); } });
    let w = window.as_weak();
    window.on_book_toggle_text_dark(move || {
        if let Some(w) = w.upgrade() { w.set_book_text_dark(!w.get_book_text_dark()); }
    });

    // ── Books Phase 5: TOC / bookmarks / themes / series (np.p5.books.*) ──
    let w = window.as_weak();
    window.on_book_toggle_toc(move || {
        if let Some(w) = w.upgrade() { w.set_book_toc_open(!w.get_book_toc_open()); }
    });
    let w = window.as_weak();
    window.on_book_jump_chapter(move |page| {
        if let Some(w) = w.upgrade() {
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    if fmt_is_text(&s.format) {
                        // `page` is a chapter index from the TOC → its first page.
                        let c = (page as usize).min(s.chapter_starts.len().saturating_sub(1));
                        s.page = s.chapter_starts.get(c).copied().unwrap_or(0);
                        s.chapter = c;
                    } else {
                        s.comic.page = (page as usize).min(s.comic.total.saturating_sub(1));
                        s.panel = 0; // guided view restarts on the jumped-to page
                    }
                }
            });
            reader_render(&w);
            save_reader_progress();
            w.set_book_toc_open(false);
        }
    });
    let w = window.as_weak();
    window.on_book_toggle_bookmarks(move || {
        if let Some(w) = w.upgrade() { w.set_book_bookmarks_open(!w.get_book_bookmarks_open()); }
    });
    let w = window.as_weak();
    window.on_book_add_bookmark(move || {
        let Some(w0) = w.upgrade() else { return; };
        let color = w0.get_book_bm_color().to_string();
        let snap = READER.with(|r| {
            r.borrow().as_ref().map(|s| {
                let page = if fmt_is_comic(&s.format) { s.comic.page as i64 } else { s.page as i64 };
                (s.item_id, page)
            })
        });
        let Some((item_id, page)) = snap else { return; };
        if item_id < 0 { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            if let Ok(pool) = pool_for("books").await {
                let _ = tulipix_books::progress::add_bookmark(
                    &pool, item_id, &page.to_string(), page, None, Some(&color)).await;
                // Reload bookmarks into UI
                if let Ok(bms) = tulipix_books::progress::bookmarks(&pool, item_id).await {
                    let _ = wk.upgrade_in_event_loop(move |w| {
                        let rows: Vec<BookmarkRow> = bms.into_iter().map(|(id, pg, note, color)| BookmarkRow {
                            id: id as i32,
                            page: pg as i32,
                            note: note.unwrap_or_default().into(),
                            color: color.unwrap_or_default().into(),
                        }).collect();
                        w.set_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(rows)));
                    });
                }
            }
        });
    });
    let w = window.as_weak();
    window.on_book_jump_bookmark(move |page| {
        if let Some(w) = w.upgrade() {
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    if fmt_is_comic(&s.format) {
                        s.comic.page = (page as usize).min(s.comic.total.saturating_sub(1));
                        s.panel = 0;
                    } else {
                        s.page = (page as usize).min(s.pages.len().saturating_sub(1));
                        s.chapter = chapter_of_page(s, s.page);
                    }
                }
            });
            reader_render(&w);
            save_reader_progress();
            w.set_book_bookmarks_open(false);
        }
    });
    let w = window.as_weak();
    window.on_book_toggle_read_mode(move || {
        if let Some(w) = w.upgrade() {
            let new_mode = if w.get_book_read_mode().as_str() == "paginated" { "scroll" } else { "paginated" };
            w.set_book_read_mode(new_mode.into());
            // Scroll mode shows the whole chapter; paginated shows one page.
            reader_render(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_set_theme(move |t| {
        if let Some(w) = w.upgrade() { w.set_book_theme(t); }
    });
    let w = window.as_weak();
    window.on_book_tts_start(move || {
        // np.p4/p5.books.tts — read the current page aloud sentence by
        // sentence, surfacing the spoken sentence in the reader overlay.
        // Offline Piper voice first, platform TTS fallback per sentence.
        let Some(_w) = w.upgrade() else { return; };
        let text = READER.with(|r| r.borrow().as_ref()
            .and_then(|s| s.pages.get(s.page).cloned()).unwrap_or_default());
        if text.trim().is_empty() { return; }
        // A bumped generation stops any previous run; the stop button bumps too.
        let generation = TTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
        let wk = w.clone();
        std::thread::spawn(move || {
            let sentences = tulipix_books::tts::chunk_sentences(&text, 400);
            for sentence in sentences {
                if TTS_GENERATION.load(std::sync::atomic::Ordering::SeqCst) != generation { break; }
                let shown = sentence.clone();
                let _ = wk.upgrade_in_event_loop(move |w| {
                    w.set_book_tts_sentence(shown.into());
                });
                tts_speak_sentence(&sentence);
            }
            // Clear the overlay only if no newer run took over.
            if TTS_GENERATION.load(std::sync::atomic::Ordering::SeqCst) == generation {
                let _ = wk.upgrade_in_event_loop(move |w| {
                    w.set_book_tts_sentence("".into());
                });
            }
        });
    });
    let w = window.as_weak();
    window.on_book_tts_stop(move || {
        // Invalidate the running generation; the speaking thread notices
        // before its next sentence.
        TTS_GENERATION.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if let Some(w) = w.upgrade() { w.set_book_tts_sentence("".into()); }
    });
    let w = window.as_weak();
    window.on_book_define(move |word| {
        // np.p5.books.dictionary — definition + translation + Wikipedia lookup.
        let Some(w0) = w.upgrade() else { return; };
        let word = word.to_string().trim().to_string();
        if word.is_empty() { return; }
        w0.set_book_define_result(format!("Looking up “{word}”…").into());
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let result = define_word(&word).await;
            let _ = wk.upgrade_in_event_loop(move |w| {
                w.set_book_define_result(result.into());
            });
        });
    });
    let w = window.as_weak();
    window.on_book_find_text(move |query| {
        if let Some(w) = w.upgrade() {
            let q = query.to_string().to_lowercase();
            if q.trim().is_empty() { return; }
            READER.with(|r| {
                let mut g = r.borrow_mut();
                if let Some(s) = g.as_mut() {
                    if fmt_is_text(&s.format) {
                        // Same query again = find-NEXT: scan after the current
                        // page and wrap; a new query starts from the top.
                        let total = s.pages.len();
                        let start = if s.last_query == q { s.page + 1 } else { 0 };
                        let hit = (0..total)
                            .map(|off| (start + off) % total.max(1))
                            .find(|&i| s.pages[i].to_lowercase().contains(&q));
                        if let Some(idx) = hit {
                            s.page = idx;
                            s.chapter = chapter_of_page(s, s.page);
                        }
                        s.last_query = q.clone();
                    }
                }
            });
            reader_render(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_series_open(move |series_id| {
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let books = match pool_for("books").await {
                Ok(pool) => tulipix_books::library::series_books(&pool, series_id as i64).await.unwrap_or_default(),
                Err(_) => Vec::new(),
            };
            // Open the first book in the series
            if let Some(&first_id) = books.first() {
                let idx = book_ids().lock().ok().and_then(|g| g.iter().position(|&id| id == first_id)).unwrap_or(0);
                open_book(wk, idx as i32);
            }
        });
    });
    let w = window.as_weak();
    window.on_book_collection_open(move |col_id| {
        if let Some(w) = w.upgrade() {
            w.set_book_view(format!("col:{col_id}").into());
            refresh_books(&w);
        }
    });
    let w = window.as_weak();
    window.on_book_add_to_collection(move |idx, name| {
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let name = name.to_string().trim().to_string();
        if item_id < 0 || name.is_empty() { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let _ = tulipix_books::library::add_to_collection(&pool, &name, item_id).await;
            let _ = wk.upgrade_in_event_loop(move |w| refresh_books(&w));
        });
    });
    let w = window.as_weak();
    window.on_book_fetch_meta(move |idx| {
        // np.p4.books.metadata — Google Books title lookup; fills only empty
        // book_meta fields (metadata::apply COALESCEs), then refreshes the grid.
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let stem = book_paths().lock().ok()
            .and_then(|g| g.get(idx as usize).cloned())
            .and_then(|p| p.file_stem().and_then(|s| s.to_str().map(|s| s.to_string())))
            .unwrap_or_default();
        if item_id < 0 { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            // Prefer the stored title; fall back to the file stem.
            let title: String = sqlx::query_scalar::<_, Option<String>>(
                "SELECT title FROM book_meta WHERE item_id = ?1")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten().flatten()
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(stem);
            if title.trim().is_empty() { return; }
            let url = tulipix_books::metadata::google_books_url(&title);
            let Ok(resp) = reqwest::get(&url).await else { return; };
            let Ok(body) = resp.text().await else { return; };
            let Some(meta) = tulipix_books::metadata::parse_google_books(&body) else { return; };
            let _ = tulipix_books::metadata::apply(&pool, item_id, &meta).await;
            let _ = wk.upgrade_in_event_loop(move |w| refresh_books(&w));
        });
    });
    let w = window.as_weak();
    window.on_book_export_notes(move |idx| {
        // np.p5.books.export-notes — Markdown + CSV into ~/Documents/Tulipix
        // Notes/, with a visible confirmation in the books stats bar.
        let Some(_w0) = w.upgrade() else { return; };
        let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
        let title = book_paths().lock().ok()
            .and_then(|g| g.get(idx as usize).cloned())
            .and_then(|p| p.file_stem().and_then(|s| s.to_str().map(|s| s.to_string())))
            .unwrap_or_default();
        if item_id < 0 { return; }
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let status = async {
                let pool = pool_for("books").await.ok()?;
                let bms = tulipix_books::progress::bookmarks(&pool, item_id).await.ok()?;
                if bms.is_empty() {
                    return Some("No bookmarks to export for this book".to_string());
                }
                let dir = dirs_default_documents().join("Tulipix Notes");
                std::fs::create_dir_all(&dir).ok()?;
                let safe: String = title.chars()
                    .map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' { c } else { '_' })
                    .collect();
                let mut md = format!("# Bookmarks: {title}\n\n");
                let mut csv = String::from("page,color,note\n");
                for (_id, page, note, color) in &bms {
                    let n = note.as_deref().unwrap_or("");
                    let c = color.as_deref().unwrap_or("");
                    md.push_str(&format!("- **Page {}**{}{}\n", page + 1,
                        if c.is_empty() { String::new() } else { format!(" `{c}`") },
                        if n.is_empty() { String::new() } else { format!(" — {n}") }));
                    csv.push_str(&format!("{},{},\"{}\"\n", page + 1, c, n.replace('"', "\"\"")));
                }
                std::fs::write(dir.join(format!("{safe}.md")), &md).ok()?;
                std::fs::write(dir.join(format!("{safe}.csv")), &csv).ok()?;
                Some(format!("{} notes exported → {}", bms.len(), dir.display()))
            }.await.unwrap_or_else(|| "Export failed — see logs".to_string());
            let _ = wk.upgrade_in_event_loop(move |w| {
                w.set_book_export_status(status.into());
                // Auto-clear after 6s without holding the window alive.
                let weak = w.as_weak();
                slint::Timer::single_shot(std::time::Duration::from_secs(6), move || {
                    if let Some(w) = weak.upgrade() { w.set_book_export_status("".into()); }
                });
            });
        });
    });
    let w = window.as_weak();
    window.on_book_goal_adjust(move |delta| {
        // ± yearly reading goal (np.p5.books.stats); persisted in book_prefs.
        let wk = w.clone();
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let Ok(pool) = pool_for("books").await else { return; };
            let cur = tulipix_books::progress::get_pref(&pool, "year_goal").await
                .ok().flatten().and_then(|v| v.parse::<i64>().ok()).unwrap_or(12);
            let next = (cur + delta as i64).clamp(1, 999);
            let _ = tulipix_books::progress::set_pref(&pool, "year_goal", &next.to_string()).await;
            let _ = wk.upgrade_in_event_loop(move |w| { refresh_books(&w); });
        });
    });

    // ── Cloud: rclone remotes CRUD + remote-tree browse (np.p4.cloud.*) ──
    tulipix_sec_cloud::wire(&window);

    // Tools section wiring lives in the tulipix-sec-tools crate. Under the `hot`
    // feature it is routed through the tulipix-hot dylib so the callbacks can be
    // re-wired into the running app on every dylib rebuild (no restart).
    #[cfg(not(feature = "hot"))]
    tulipix_sec_tools::wire(&window);
    #[cfg(feature = "hot")]
    crate::hot::wire_and_watch(&window);

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
        if k.as_str() == "discover" {
            kick_discover_refresh(w.clone());
        } else {
            kick_video_refresh(w.clone(), w0.get_video_category().to_string());
        }
    });

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
        if let Some(w) = w.upgrade() { play_music_at(&w, idx); }
    });
    let w = window.as_weak();
    window.on_music_next(move || {
        let Some(w) = w.upgrade() else { return; };
        // YouTube queue active → skip within it (honours shuffle); else library.
        if YT_QUEUE_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
            yt_queue_jump(w.as_weak(), true, w.get_music_shuffle());
            return;
        }
        let total = w.get_music_np_total();
        if total <= 0 { return; }
        // Shuffle → next from the shuffled bag (no repeats per cycle); else
        // sequential wrap.
        let next = if w.get_music_shuffle() && total > 1 {
            shuffle_next(total, w.get_music_np_index())
        } else {
            (w.get_music_np_index() + 1).rem_euclid(total)
        };
        play_music_at(&w, next);
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
        // Under shuffle, walk the real played order back instead of index−1.
        if w.get_music_shuffle() {
            if let Some(prev) = shuffle_prev_index(total) {
                play_music_at(&w, prev);
                return;
            }
        }
        play_music_at(&w, (w.get_music_np_index() - 1).rem_euclid(total));
    });
    let w = window.as_weak();
    window.on_music_stop(move || {
        MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); // suppress auto-advance
        if let Ok(mut g) = music_proc().lock() {
            if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
        }
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
    window.on_section_changed(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        if s.as_str() == "home" {
            // Fresh greeting (time of day) + counts on every Home landing.
            set_home_greeting_now(&w0);
            kick_home_stats(&w0);
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
                "folders" => populate_folder_roots(&w),
                "albums" | "artists" | "genres" => { w.set_music_browse_page(0); rebuild_browse_tab(&w, t.as_str()); }
                _ => {}
            }
        }
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
        w0.set_music_tag_art(tile_thumb_at(&w0, pos));
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
        let Some(w0) = w.upgrade() else { return; };
        let Some(folder) = music_paths().lock().ok()
            .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.display().to_string()))) else { return; };
        let next = cycle_folder_section(&folder);
        tracing::info!(folder = %folder, section = %next, "music folder section reassigned");
        populate_music_views(w0.as_weak());
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
            let client = reqwest::Client::new();
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
    window.on_music_detail_play_track(move |pos| { if let Some(w) = w.upgrade() { play_music_at(&w, pos); } });
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
    // Trends — subscribe to a baked/hardcoded feed by its index in podc.md.
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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
            let row: Option<(String, String, Option<String>, String, i64, String, String)> = sqlx::query_as(
                "SELECT e.title, e.audio_url, e.downloaded_path, COALESCE(e.image_url,''),
                        p.id, COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), COALESCE(p.title,'')
                 FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id WHERE e.id = ?")
                .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
            let Some((title, url, dl, eimg, pid, pimg, show)) = row else { return; };
            if url.is_empty() && dl.is_none() { return; }
            let client = reqwest::Client::new();
            let art = match cache_artwork(&client, &format!("ep-{id}"), &eimg).await {
                Some(p) => Some(p),
                None => cache_artwork(&client, &format!("pod-{pid}"), &pimg).await,
            };
            let _ = sqlx::query("UPDATE podcast_episodes SET played = 1 WHERE id = ?").bind(id as i64).execute(&pool).await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                let src = dl.clone().filter(|p| std::path::Path::new(p).exists()).unwrap_or(url);
                play_music_url(&w, &src, &title);
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
    // ── YouTube section wiring (np.p4.music.youtube) ────────────────────────
    let w = window.as_weak();
    window.on_music_yt_set_tab(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_yt_tab(t.clone());
        // Models are warmed once (warm_youtube) and refreshed by their own actions
        // (download/sub/cache/remove), so plain sub-tab switches are instant — no
        // re-decoding thumbnails on every click. First open warms if not already.
        warm_youtube(&w0);
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
            let client = reqwest::Client::new();
            let dir = yt_thumb_dir();
            let videos = ytdlp_search(&q, 20).await;
            let mut rows = Vec::with_capacity(videos.len());
            for v in &videos { rows.push(yt_vid_data(&client, &dir, v).await); }
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
                let client = reqwest::Client::new();
                let dir = yt_thumb_dir();
                let videos = ytdlp_channel_latest(&cid, 10).await;
                let mut r = Vec::with_capacity(videos.len());
                for v in &videos { r.push(yt_vid_data(&client, &dir, v).await); }
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
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_yt_channel_id(cid.into());
                w.set_music_yt_channel_title(name.into());
                w.set_music_yt_channel_avatar(yt_img(&avatar));
                w.set_music_yt_channel_sub(sub_line.into());
                w.set_music_yt_channel_videos(yt_video_model(&rows));
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
            let client = reqwest::Client::new();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_latest(&cid, 10).await;
            let mut rows = Vec::with_capacity(videos.len());
            for v in &videos { rows.push(yt_vid_data(&client, &dir, v).await); }
            yt_remember(&rows);
            if let Ok(p) = pool_for("youtube").await {
                let cv: Vec<_> = rows.iter().map(yt_data_to_channelvid).collect();
                let _ = tulipix_music::youtube::store::set_channel_cache(&p, &cid, &cv).await;
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_yt_channel_videos(yt_video_model(&rows));
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
                let rows = cached.clone();
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_yt_channel_videos(yt_video_model(&rows)));
            } else {
                let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_busy(true));
            }
            // 2. Background fetch fresh top-10, store, then update if changed.
            let client = reqwest::Client::new();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_popular(&cid, 10).await;
            let mut fresh = Vec::with_capacity(videos.len());
            for v in &videos { fresh.push(yt_vid_data(&client, &dir, v).await); }
            if !fresh.is_empty() {
                yt_remember(&fresh);
                if let Some(p) = &pool {
                    let cv: Vec<_> = fresh.iter().map(yt_data_to_channelvid).collect();
                    let _ = tulipix_music::youtube::store::set_channel_popular(p, &cid, &cv).await;
                }
                let changed = fresh.iter().map(|r| r.id.clone()).collect::<Vec<_>>()
                    != cached.iter().map(|r| r.id.clone()).collect::<Vec<_>>();
                if changed || !had_cache {
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        if w.get_music_yt_channel_mode() == "popular" {
                            w.set_music_yt_channel_videos(yt_video_model(&fresh));
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
            let client = reqwest::Client::new();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_search(&cid, &q, 20).await;
            let mut rows = Vec::with_capacity(videos.len());
            for v in &videos { rows.push(yt_vid_data(&client, &dir, v).await); }
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
                let client = reqwest::Client::new();
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
    // Audiobook play with pitch-preserving speed + resume (np.p5.music.audiobook-chapters).
    let w = window.as_weak();
    window.on_music_audiobook_play(move |pos| {
        let Some(w0) = w.upgrade() else { return; };
        play_music_at(&w0, pos);
        // Force book mode even when the cover cache hasn't seen this folder yet
        // (play_music_at only detects books through that cache).
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
        if let Some(id) = current_music_id(&w0) { load_book_bookmarks(&w0, id); }
        if let Some(id) = current_music_id(&w0) {
            let speed = speed as f64;
            tokio::runtime::Handle::current().spawn(async move {
                if let Ok(pool) = pool_for("music").await {
                    let _ = tulipix_music::audiobooks::mark_audiobook(&pool, id).await;
                    let (pos_s, _) = tulipix_music::audiobooks::resume(&pool, id).await.unwrap_or((0.0, 1.0));
                    if pos_s > 1.0 { music_ipc(&["seek", &pos_s.to_string(), "absolute"]); }
                    // Register the book as in-progress IMMEDIATELY — the card
                    // bar + "In progress" tab key off audiobook_progress rows.
                    let _ = tulipix_music::audiobooks::save_progress(&pool, id, pos_s.max(1.0), speed).await;
                }
            });
        }
    });
    let w = window.as_weak();
    window.on_music_set_book_speed(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        let s = tulipix_music::audiobooks::clamp_speed(s as f64);
        w0.set_music_book_speed(s as f32);
        music_ipc(&["set_property", "speed", &s.to_string()]);
        if let Some(id) = current_music_id(&w0) {
            let pos = w0.get_music_pos() as f64;
            tokio::runtime::Handle::current().spawn(async move {
                if let Ok(pool) = pool_for("music").await { let _ = tulipix_music::audiobooks::save_progress(&pool, id, pos, s).await; }
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
    // Add a bookmark at the current position (np.p5.music.audiobook-chapters).
    let w = window.as_weak();
    window.on_music_book_bookmark(move || {
        let Some(w0) = w.upgrade() else { return; };
        let Some(id) = current_music_id(&w0) else { return; };
        let pos = w0.get_music_pos() as f64;
        let label = fmt_clock(pos);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::audiobooks::add_bookmark(&pool, id, pos, &label).await;
            let _ = weak.upgrade_in_event_loop(move |w| load_book_bookmarks(&w, id));
        });
    });
    // Jump to the i-th bookmark of the playing audiobook.
    let w = window.as_weak();
    window.on_music_book_bookmark_jump(move |i| {
        let Some(_w0) = w.upgrade() else { return; };
        let pos = book_bookmarks().lock().ok().and_then(|g| g.get(i as usize).copied());
        if let Some(pos) = pos { music_ipc(&["seek", &pos.to_string(), "absolute"]); }
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
            let _ = weak.upgrade_in_event_loop(move |w| fill_book_detail(&w, &folder, &ids));
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
            let client = reqwest::Client::new();
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
            let client = reqwest::Client::new();
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

    // ── Phase 5 music — playlist builder / M3U / queue reorder ──────────────
    // Open a playlist's detail (tile `index` carries the playlist DB id).
    let w = window.as_weak();
    window.on_music_playlist_open(move |pid| {
        if let Some(w) = w.upgrade() { build_playlist_detail(&w, pid as i64); }
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
        play_music_at(&w0, pos);
        // Queue the rest of the playlist in its displayed order (np.p5.music.playlist-order).
        let (_pid, ids) = current_playlist().lock().map(|g| g.clone()).unwrap_or((-1, Vec::new()));
        let clicked_id = music_ids().lock().ok().and_then(|g| g.get(pos as usize).copied());
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::queue::clear(&pool).await;
            if let Some(start) = clicked_id.and_then(|cid| ids.iter().position(|x| *x == cid)) {
                for off in 1..ids.len() {
                    let id = ids[(start + off) % ids.len()];
                    let _ = tulipix_music::queue::enqueue(&pool, id, "playlist").await;
                }
            }
            let _ = weak.upgrade_in_event_loop(|w| build_music_queue(&w));
        });
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
            let idx = w.get_music_np_index();
            let is_book = music_songs().lock().ok()
                .map(|g| g.iter().any(|s| s.pos == idx && s.is_audiobook)).unwrap_or(false);
            if !is_book { return; }
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
            if !w.get_music_playing() {
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
        window.set_music_home_connect(s.advanced.get("music.home_connect").map(|v| v == "1").unwrap_or(false));
        // YouTube Home gradient outline defaults ON when unset.
        window.set_music_yt_home_connect(s.advanced.get("music.yt_home_connect").map(|v| v == "1").unwrap_or(true));
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
                w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
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
                w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
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
                w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
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
        #[cfg(target_os = "windows")]
        let r = std::process::Command::new("cmd").args(["/C", "start", "", &url]).spawn();
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
        let name = name.trim().to_string();
        if name.is_empty() { s.advanced.remove("profile.name"); }
        else { s.advanced.insert("profile.name".into(), name.clone()); }
        if emoji.is_empty() { s.advanced.remove("profile.emoji"); }
        else { s.advanced.insert("profile.emoji".into(), emoji.to_string()); }
        // Sidebar logo choice (np.p1.profile.logo): 0 default · 1 color · 2 dark · 3 white.
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

    // ── Settings panels: load persisted settings, seed the UI models ───────
    {
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        // Restore the saved profile identity into the sidebar user card.
        {
            let mut u = window.get_user();
            u.display_name = s.text("profile.name").into();
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
        set_home_greeting_now(&window);
        kick_home_stats(&window);
        // Restore the last-used app theme and keep it until the user changes it.
        let choice = match s.theme.as_str() {
            "extra-dark" => ThemeChoice::ExtraDark,
            "system"     => ThemeChoice::System,
            _             => ThemeChoice::Light,
        };
        window.set_theme_choice(choice);
        apply_theme_choice(&window, choice);
        // Restore the last-used Music-section theme (light / dark / OLED).
        match s.advanced.get("music.theme").map(|v| v.as_str()) {
            Some("dark") => { window.set_music_light(false); window.set_music_oled(false); }
            Some("oled") => { window.set_music_light(false); window.set_music_oled(true); }
            _            => { window.set_music_light(true);  window.set_music_oled(false); }
        }
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
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Ok(mut g) = category_paths().lock() { *g = Some(CatFilter { set, order: paths }); }
                w.set_photos_filter_title(title.into());
                w.set_photos_category("facephotos".into());
                selection_clear();
                apply_photo_filter(&w, "");
            });
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
        let handle = tokio::runtime::Handle::current();
        handle.spawn(async move {
            let paths = tag_photo_paths(&label).await;
            let set: std::collections::HashSet<String> = paths.iter().cloned().collect();
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Ok(mut g) = category_paths().lock() { *g = Some(CatFilter { set, order: paths }); }
                w.set_photos_filter_title(label.into());
                w.set_photos_category("tagphotos".into());
                selection_clear();
                apply_photo_filter(&w, "");
            });
        });
    });

    // ── Albums tab (np.p2.albums) ─────────────────────────────────────────
    // Open an album → its photos in the flat grid (category "albumphotos").
    let w = window.as_weak();
    window.on_photo_album_open(move |id| {
        let weak = w.clone();
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
            let _ = weak.upgrade_in_event_loop(move |w| {
                if let Ok(mut g) = category_paths().lock() { *g = Some(CatFilter { set, order: paths }); }
                w.set_photos_filter_title(title.into());
                w.set_photos_category("albumphotos".into());
                selection_clear();
                apply_photo_filter(&w, "");
            });
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
    // Live FS watcher (np.p1.lib.watch): notify across every watched folder.
    // Renames update items rows in place; deletes flip missing_since. The
    // event is applied to every section pool — UPDATE is a no-op where the
    // path isn't indexed, so no per-section routing is needed.
    {
        let folders = load_watched_folders();
        if !folders.is_empty() {
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
                    // Keep the watcher alive for the app's lifetime.
                    Box::leak(Box::new(watcher));
                    let rt = tokio::runtime::Handle::current();
                    std::thread::Builder::new().name("tulipix-fsapply".into()).spawn(move || {
                        while let Ok(evt) = rx.recv() {
                            let evt2 = evt.clone();
                            rt.spawn(async move {
                                for section in ["photos", "videos", "music", "books", "cloud"] {
                                    if let Ok(pool) = pool_for(section).await {
                                        let _ = tulipix_core::watcher::apply_event(&pool, &evt2).await;
                                    }
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
    // AI execution crate is off by default; the overlay is fully wired and
    // explains how to enable it rather than failing silently.
    window.set_chat_turns(slint::ModelRc::new(slint::VecModel::from(vec![ChatTurn {
        role: "assistant".into(),
        content: "Hi! I can search and act on your library once the AI engine is enabled \
                  (Settings → AI Models, or build with --features lazy-ai-ep).".into(),
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
        row.user_key_set = false;
        row.use_app_default = true;
        row.editing = false;
        let q = tulipix_core::api_keys::quota_state(&service, tulipix_core::api_keys::KeySource::AppDefault);
        row.quota_used = q.used as i32;
        row.quota_limit = q.limit as i32;
        model.set_row_data(i as usize, row);
        tracing::info!(%service, "custom api key removed");
    });

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
        // Destructive — confirm before wiping every library DB + cache.
        let yes = rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Warning)
            .set_title("Reset Tulipix?")
            .set_description("This erases the entire library index, watched folders, tags, and thumbnails. Your actual media files are NOT touched. This cannot be undone.")
            .set_buttons(rfd::MessageButtons::OkCancelCustom("Reset everything".into(), "Cancel".into()))
            .show();
        if !matches!(yes, rfd::MessageDialogResult::Custom(ref s) if s == "Reset everything") { return; }
        set_lib_busy(&w.as_weak(), "Clearing all data — starting fresh…", -1.0);
        // Forget persisted lists.
        if let Some(p) = watched_folders_path() { let _ = std::fs::remove_file(p); }
        if let Some(p) = folder_sections_path() { let _ = std::fs::remove_file(p); }
        // Drop the thumbnail cache.
        if let Some(dir) = tulipix_core::paths::thumbs_dir() {
            let _ = std::fs::remove_dir_all(&dir);
            let _ = std::fs::create_dir_all(&dir);
        }
        // Clear in-memory accumulators so the grids empty immediately.
        if let Ok(mut g) = photo_full().lock() { g.clear(); }
        if let Ok(mut g) = video_full().lock() { g.clear(); }
        if let Ok(mut g) = music_full().lock() { g.clear(); }
        if let Ok(mut g) = music_paths().lock() { g.clear(); }
        let weak = w.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            // Truncate the core items table per section. Derived views read via
            // joins on items, so emptying it empties every section's library.
            for section in ["photos", "videos", "music", "books"] {
                if let Ok(pool) = pool_for(section).await {
                    let _ = sqlx::query("DELETE FROM items").execute(&pool).await;
                }
            }
            tracing::info!("app data reset — fresh start");
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_library_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<LibraryRow>::new())));
                rebuild_scan_rows(&w);
                w.set_onboarding_lib_added(false);
                // Rebuild every section view from the now-empty DBs.
                rebuild_music_tiles(&w);
                populate_music_views(w.as_weak());
                populate_folder_roots(&w);
                w.set_photos_total(0);
                kick_category_refresh(w.as_weak(), w.get_photos_category().to_string(), w.get_photos_query().to_string());
                kick_video_refresh(w.as_weak(), w.get_video_category().to_string());
                refresh_books(&w);
                clear_lib_busy(&w.as_weak());
            });
        });
    });
    let w = window.as_weak();
    window.on_lib_row_remove(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_library_rows();
        let removed_path = model.row_data(i as usize).map(|r| r.path.to_string());
        let rows: Vec<LibraryRow> = (0..model.row_count())
            .filter(|&j| j != i as usize)
            .filter_map(|j| model.row_data(j))
            .collect();
        w.set_library_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
        // Drop from the watched-folder file if no other row references the path.
        if let Some(p) = removed_path { forget_watched_folder(&w, &p); }
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

    // ── Properties window — open containing folder ─────────────────────────
    let w = window.as_weak();
    window.on_props_open_folder(move || {
        let Some(w) = w.upgrade() else { return; };
        let p = PathBuf::from(w.get_props().path.to_string());
        let _ = tulipix_platform::fm::reveal_in_file_manager(&p);
    });

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
                add_folder_path(&window, path);
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

    // Launch filling the screen (maximised, decorations kept) rather than a
    // small floating window.
    window.window().set_maximized(true);
    window.run()?;

    // Stop all playback so nothing keeps playing after the window closes.
    kill_all_mpv();
    // Tear down any rclone mounts spun up for the cloud section.
    tulipix_sec_cloud::cloud_unmount_all();

    // Persist gate-hit counter on shutdown
    if let Some(cache) = dirs_default().map(|d| d.join("cache")) {
        let _ = tulipix_core::caps::persist_hit_counts(&cache);
    }
    Ok(())
}

/// Register the bundled Sora variable font with the Slint runtime so the UI
/// always renders with the canonical brand typeface — no fallback to system
/// "Sans". The font covers weights 100..900; Slint picks the right axis.
fn register_bundled_fonts() {
    static SORA_VAR: &[u8] = include_bytes!("../../../resources/fonts/Sora[wght].ttf");
    let blob = slint::fontique_08::fontique::Blob::new(std::sync::Arc::new(SORA_VAR.to_vec()));
    let mut collection = slint::fontique_08::shared_collection();
    let registered = collection.register_fonts(blob, None);
    tracing::info!(count = registered.len(), "registered bundled fonts");
}

// Photos section (helpers, statics, editor, viewer) extracted to tulipix_sec_photos. See wire-up in main + `use tulipix_sec_photos::*`.

// Videos section library (statics, parse, TMDB scrape, show cards) extracted to tulipix_sec_videos. See `use tulipix_sec_videos::*`.

// fmt_duration / fmt_clock / fmt_date moved to tulipix_common.

// Currently-playing item (for watch_progress writeback) + pending resume seek.
static PLAYER_ITEM: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
fn player_item() -> &'static std::sync::Mutex<Option<i64>> {
    PLAYER_ITEM.get_or_init(|| std::sync::Mutex::new(None))
}
static PLAYER_RESUME: std::sync::OnceLock<std::sync::Mutex<Option<f64>>> = std::sync::OnceLock::new();
fn player_resume() -> &'static std::sync::Mutex<Option<f64>> {
    PLAYER_RESUME.get_or_init(|| std::sync::Mutex::new(None))
}
static PLAYER_PATH: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
fn player_path() -> &'static std::sync::Mutex<Option<PathBuf>> {
    PLAYER_PATH.get_or_init(|| std::sync::Mutex::new(None))
}
// Source path picked for the profile cover/avatar cropper — stashed between
// the file-picker callback and crop-confirm (np.p1.profile.cover).
static CROP_SOURCE: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
fn crop_source() -> &'static std::sync::Mutex<Option<PathBuf>> {
    CROP_SOURCE.get_or_init(|| std::sync::Mutex::new(None))
}
static PLAYER_FS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// PiP state (np.p3.player.pip) — floating always-on-top mini mpv window.
static PLAYER_PIP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Apply per-playback mpv preferences once a file is loaded: subtitle styling
/// (np.p3.sub.styling), HDR routing (np.p3.player.hdr), exclusive audio output
/// (np.p3.player.audio), optional motion interpolation (np.p3.player.upscale),
/// and keep-display-awake while playing (np.p3.player.sleep). Tunables live in
/// Settings → System ("PLAYBACK").
fn apply_playback_prefs() {
    use tulipix_player::{audio, hdr};
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();

    // Subtitle styling → mpv sub-* properties.
    let mut style = tulipix_videos::sub_styling::SubtitleStyle::default();
    if let Some(v) = s.advanced.get("playback.sub-size").and_then(|x| x.trim().parse::<f32>().ok()) {
        style.font_size_px = v;
    }
    if let Some(c) = s.advanced.get("playback.sub-color") {
        if !c.trim().is_empty() { style.color = c.trim().to_string(); }
    }
    style.clamp();
    for (k, v) in style.to_mpv_options() { mpv::set_string(&k, &v); }

    // HDR routing — detect the source transfer from mpv; tone-map to an SDR
    // panel by default (Linux has no reliable display-HDR probe).
    let src = match mpv::get_prop("video-params/gamma").as_deref() {
        Some("pq") => hdr::SourceHdr::Hdr10,
        Some("hlg") => hdr::SourceHdr::Hlg,
        _ => hdr::SourceHdr::Sdr,
    };
    let plan = hdr::plan(src, &hdr::DisplayCaps::default());
    for (k, v) in plan.mpv_opts { mpv::set_string(&k, &v); }

    // Exclusive audio output (opt-in) → ao backend opts.
    let mode = if s.flag("playback.audio-exclusive", false) {
        audio::AudioMode::Exclusive
    } else {
        audio::AudioMode::Shared
    };
    let backend = audio::pick_backend(mode);
    for (k, v) in audio::mpv_opts(backend) { mpv::set_string(&k, &v); }

    // Motion interpolation (opt-in — heavy on integrated GPUs).
    if s.flag("playback.interpolation", false) {
        mpv::set_string("video-sync", "display-resample");
        mpv::set_flag("interpolation", true);
    } else {
        mpv::set_flag("interpolation", false);
    }

    // Keep the display awake while a video is open (np.p3.player.sleep).
    mpv::set_flag("stop-screensaver", true);
}

/// Persist the current playhead to watch_progress + refresh the Continue tab.
fn player_persist_progress(weak: slint::Weak<MainWindow>, pos: f64, dur: f64) {
    let id = *player_item().lock().unwrap_or_else(|p| p.into_inner());
    let Some(id) = id else { return; };
    if pos <= 0.0 { return; }
    let dur_opt = if dur > 0.0 { Some(dur) } else { None };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = tulipix_videos::watch_progress::update(&pool, id, pos, dur_opt).await;
        }
        let _ = weak.upgrade_in_event_loop(|w| {
            let cat = w.get_video_category().to_string();
            kick_video_refresh(w.as_weak(), cat);
        });
    });
}

/// Close the embedded player, saving progress.
fn player_close(w: &MainWindow) {
    let (pos, dur) = (w.get_player_pos() as f64, w.get_player_duration() as f64);
    mpv::stop();
    w.set_player_open(false);
    if PLAYER_FS.swap(false, std::sync::atomic::Ordering::AcqRel) { w.window().set_fullscreen(false); }
    player_persist_progress(w.as_weak(), pos, dur);
    *player_item().lock().unwrap_or_else(|p| p.into_inner()) = None;
}

// watched_folders_path / load_watched_folders moved to tulipix_common.

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
}

// ── Music folder → section tag (np.p5.atmusic.folder-sections) ──────────────
// Each scanned music folder can be assigned to one of the 5 top music sections
// so the user controls where its tracks belong. Persisted next to the watched
// folders so the assignment survives restarts.
// MUSIC_SECTIONS + folder-section helpers moved to tulipix_common.

/// Open a library video in the embedded player (np.p3.player.*): resume from
/// the stored position (np.p3.watch-progress) and record the access for the
/// Continue rail (np.p3.last-accessed). libmpv renders the frames in-window.
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
                tulipix_videos::watch_progress::resume(&pool, id).await.ok().flatten()
            } else { None }
        } else { None };
        // Play in an external mpv window — the embedded libmpv/skia-opengl render
        // path froze the whole UI on weak iGPUs (Intel HD 5500: it blocks the
        // render thread). mpv's own window + OSC gives full controls and never
        // blocks Slint. Resume + progress writeback still flow via the IPC socket.
        spawn_mpv_windowed(path.clone(), resume, item_id);
        let _ = weak.upgrade_in_event_loop(move |w| {
            // A fresh access — refresh the Continue tab if it's showing.
            let cat = w.get_video_category().to_string();
            if cat == "continue" || cat == "library" { kick_video_refresh(w.as_weak(), cat); }
        });
    });
}

/// Anime4K-style GLSL shader chain (np.p3.player.upscale): all `.glsl` files in
/// `<config>/shaders`, sorted, joined for mpv's `--glsl-shaders` list option.
/// Returns `(dir, count)` for status display; the chain itself via `.0`.
// anime4k_shader_args moved to tulipix_common.

// spawn_mpv_windowed moved to tulipix_common (playback core).

/// Hook up the embedded player's control callbacks (transport, tracks, speed,
/// fullscreen, and keyboard gestures via tulipix-player::gestures).
fn register_player_controls(window: &MainWindow) {
    use std::sync::atomic::Ordering;
    window.on_player_toggle_pause(|| mpv::command(&["cycle", "pause"]));
    window.on_player_seek(|s| mpv::command(&["seek", &(s as f64).to_string(), "absolute"]));
    window.on_player_seek_rel(|d| mpv::command(&["seek", &(d as f64).to_string(), "relative"]));
    window.on_player_set_volume(|v| mpv::set_double("volume", v as f64));
    window.on_player_toggle_mute(|| mpv::command(&["cycle", "mute"]));
    window.on_player_set_speed(|s| mpv::set_double("speed", s as f64));
    window.on_player_set_audio(|id| mpv::set_string("aid", &id.to_string()));
    window.on_player_set_sub(|id| {
        if id < 0 { mpv::set_string("sid", "no"); } else { mpv::set_string("sid", &id.to_string()); }
    });
    // Manual subtitle file picker (np.p3.sub.manual) — adds + selects it.
    window.on_player_load_sub(|| {
        if let Some(path) = rfd::FileDialog::new()
            .set_title("Load subtitle")
            .add_filter("Subtitles", &["srt", "vtt", "ass", "ssa", "sub", "ttml"])
            .pick_file()
        {
            let p = path.to_string_lossy().into_owned();
            mpv::command(&["sub-add", &p, "select"]);
            tracing::info!(sub = %p, "manual subtitle loaded");
        }
    });
    window.on_player_set_chapter(|i| mpv::set_string("chapter", &i.to_string()));
    let w = window.as_weak();
    window.on_player_close(move || { if let Some(w) = w.upgrade() { player_close(&w); } });
    let w = window.as_weak();
    window.on_player_toggle_fullscreen(move || {
        if let Some(w) = w.upgrade() {
            let on = !PLAYER_FS.load(Ordering::Acquire);
            PLAYER_FS.store(on, Ordering::Release);
            w.window().set_fullscreen(on);
        }
    });
    let w = window.as_weak();
    window.on_player_key(move |k, shift| {
        let Some(w) = w.upgrade() else { return; };
        use tulipix_player::gestures::{map_key, KeyAction::*};
        let Some(a) = map_key(&k.to_lowercase(), shift) else { return; };
        match a {
            Play | Pause => mpv::command(&["cycle", "pause"]),
            Stop => player_close(&w),
            SeekShortBack => mpv::command(&["seek", "-5", "relative"]),
            SeekShortFwd  => mpv::command(&["seek", "5", "relative"]),
            SeekLongBack  => mpv::command(&["seek", "-60", "relative"]),
            SeekLongFwd   => mpv::command(&["seek", "60", "relative"]),
            VolUp   => mpv::set_double("volume", (w.get_player_volume() as f64 + 5.0).min(130.0)),
            VolDown => mpv::set_double("volume", (w.get_player_volume() as f64 - 5.0).max(0.0)),
            MuteToggle => mpv::command(&["cycle", "mute"]),
            SpeedUp    => mpv::set_double("speed", (w.get_player_speed() as f64 + 0.25).min(4.0)),
            SpeedDown  => mpv::set_double("speed", (w.get_player_speed() as f64 - 0.25).max(0.25)),
            SpeedReset => mpv::set_double("speed", 1.0),
            Fullscreen => {
                let on = !PLAYER_FS.load(Ordering::Acquire);
                PLAYER_FS.store(on, Ordering::Release);
                w.window().set_fullscreen(on);
            }
            // PiP (np.p3.player.pip): float the external mpv window — always-
            // on-top mini player at 1/3 scale, toggling back to normal. Talks
            // to the windowed instance over its IPC socket.
            Pip => {
                let on = !PLAYER_PIP.load(Ordering::Acquire);
                PLAYER_PIP.store(on, Ordering::Release);
                video_ipc(&["set_property", "ontop", if on { "true" } else { "false" }]);
                video_ipc(&["set_property", "border", if on { "false" } else { "true" }]);
                video_ipc(&["set_property", "window-scale", if on { "0.33" } else { "1.0" }]);
                video_ipc(&["set_property", "window-maximized", if on { "false" } else { "true" }]);
            }
            ToggleSubs => mpv::command(&["cycle", "sub"]),
        }
    });
}

// Video library rows/discover/refresh extracted to tulipix_sec_videos.
// book_paths[i] / book_ids[i] map a library-grid tile index → its file + item.
static BOOK_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
fn book_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    BOOK_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
static BOOK_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn book_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    BOOK_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

struct BookRow {
    abs_path: String,
    item_id: i64,
    title: String,
    author: String,
    cover_path: Option<String>,
    is_comic: bool,
    #[allow(dead_code)] // reading direction; carried for future right-to-left comic support
    rtl: bool,
    format: String,
    page: i64,
    total: Option<i64>,
    finished: bool,
}

/// Query book_meta + reading_progress for the present library, filtered by the
/// active view tab (all / reading / unread / comics).
async fn book_rows_for(pool: &sqlx::SqlitePool, view: &str, query: &str, sort: &str, dir: &str) -> Vec<BookRow> {
    // A drilled-in shelf is "col:<id>" — the id is parsed (never interpolated raw).
    let col_id = view.strip_prefix("col:").and_then(|v| v.parse::<i64>().ok());
    let col_filter = col_id.map(|id| {
        format!("AND bm.item_id IN (SELECT item_id FROM collection_items WHERE collection_id = {id})")
    });
    let filter = match view {
        "reading" => "AND COALESCE(rp.page,0) > 0 AND COALESCE(rp.finished,0) = 0",
        "unread"  => "AND (rp.item_id IS NULL OR (COALESCE(rp.page,0) = 0 AND COALESCE(rp.finished,0) = 0))",
        "comics"  => "AND bm.is_comic = 1",
        _          => col_filter.as_deref().unwrap_or(""),
    };
    let dir_sql = if dir.eq_ignore_ascii_case("desc") { "DESC" } else { "ASC" };
    let order = match sort {
        "author" => format!("bm.author COLLATE NOCASE {dir_sql}"),
        "recent" => format!("COALESCE(rp.updated,0) {dir_sql}, bm.title COLLATE NOCASE"),
        _         => format!("bm.title COLLATE NOCASE {dir_sql}"),
    };
    // Free-text filter over title + author (bound, so no injection).
    let q = query.trim();
    let search = if q.is_empty() { "" } else { "AND (bm.title LIKE ?1 OR bm.author LIKE ?1)" };
    let sql = format!(
        "SELECT i.abs_path, bm.item_id, COALESCE(bm.title,''), COALESCE(bm.author,''), bm.cover_path,
                bm.is_comic, bm.rtl, bm.format,
                COALESCE(rp.page,0), rp.total_pages, COALESCE(rp.finished,0)
         FROM book_meta bm JOIN items i ON i.id = bm.item_id
         LEFT JOIN reading_progress rp ON rp.item_id = bm.item_id
         WHERE i.missing_since IS NULL {filter} {search}
         ORDER BY {order}",
    );
    let mut qb = sqlx::query_as(&sql);
    if !q.is_empty() { qb = qb.bind(format!("%{q}%")); }
    let rows: Vec<(String, i64, String, String, Option<String>, i64, i64, String, i64, Option<i64>, i64)> =
        qb.fetch_all(pool).await.unwrap_or_default();
    rows.into_iter().map(|(abs_path, item_id, title, author, cover_path, is_comic, rtl, format, page, total, finished)| {
        BookRow { abs_path, item_id, title, author, cover_path, is_comic: is_comic != 0, rtl: rtl != 0, format, page, total, finished: finished != 0 }
    }).collect()
}

/// Re-query the Books grid from the window's current view/query/sort state.
fn refresh_books(w: &MainWindow) {
    kick_books_refresh(
        w.as_weak(),
        w.get_book_view().to_string(),
        w.get_book_query().to_string(),
        w.get_book_sort().to_string(),
        w.get_book_sort_dir().to_string(),
    );
}

fn kick_books_refresh(weak: slint::Weak<MainWindow>, view: String, query: String, sort: String, dir: String) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let (rows, series_data, author_data, col_data, read_stats) = match pool_for("books").await {
            Ok(pool) => {
                let r = book_rows_for(&pool, &view, &query, &sort, &dir).await;
                // Reading time / streak / yearly goal (np.p5.books.stats).
                let rs = tulipix_books::progress::reading_stats(&pool).await.unwrap_or_default();
                // Load series: id, name, count, cover_path
                let s: Vec<(i64, String, i64, Option<String>)> = sqlx::query_as(
                    "SELECT s.id, s.name, COUNT(bm.item_id), MIN(bm.cover_path)
                     FROM series s JOIN book_meta bm ON bm.series_id = s.id
                     JOIN items i ON i.id = bm.item_id
                     WHERE i.missing_since IS NULL
                     GROUP BY s.id ORDER BY s.name COLLATE NOCASE"
                ).fetch_all(&pool).await.unwrap_or_default();
                // Author pages: name + book count (np.p4.books.library).
                let a = tulipix_books::library::authors(&pool).await.unwrap_or_default();
                // Shelves (collections): id, name, count, a member cover.
                let c: Vec<(i64, String, i64, Option<String>)> = sqlx::query_as(
                    "SELECT c.id, c.name, COUNT(ci.item_id),
                            (SELECT bm.cover_path FROM collection_items ci2
                             JOIN book_meta bm ON bm.item_id = ci2.item_id
                             WHERE ci2.collection_id = c.id AND bm.cover_path IS NOT NULL
                             LIMIT 1)
                     FROM collections c LEFT JOIN collection_items ci ON ci.collection_id = c.id
                     GROUP BY c.id ORDER BY c.name COLLATE NOCASE"
                ).fetch_all(&pool).await.unwrap_or_default();
                (r, s, a, c, rs)
            },
            Err(_) => (Vec::new(), Vec::new(), Vec::new(), Vec::new(),
                       tulipix_books::progress::ReadingStats::default()),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let mut tiles: Vec<BookTile> = Vec::with_capacity(rows.len());
            let mut paths: Vec<PathBuf> = Vec::with_capacity(rows.len());
            let mut ids: Vec<i64> = Vec::with_capacity(rows.len());
            for r in &rows {
                // Cover: extracted EPUB/CBZ cover if we have one, else fall back
                // to the scan thumb path for the file (placeholder for PDF/CBR).
                let cover = r.cover_path.clone().unwrap_or_else(|| r.abs_path.clone());
                let img = slint::Image::load_from_path(std::path::Path::new(&cover)).unwrap_or_default();
                let progress = match r.total {
                    Some(t) if t > 0 => ((r.page + 1).min(t) as f32) / t as f32,
                    _ => if r.finished { 1.0 } else { 0.0 },
                };
                tiles.push(BookTile {
                    thumb: img,
                    title: r.title.clone().into(),
                    author: r.author.clone().into(),
                    index: tiles.len() as i32,
                    progress,
                    finished: r.finished,
                    comic: r.is_comic,
                    format: r.format.clone().into(),
                });
                paths.push(PathBuf::from(&r.abs_path));
                ids.push(r.item_id);
            }
            if let Ok(mut g) = book_paths().lock() { *g = paths; }
            if let Ok(mut g) = book_ids().lock() { *g = ids; }
            // Series tiles
            let series_tiles: Vec<SeriesRow> = series_data.into_iter().map(|(id, name, count, cover)| {
                let img = cover.as_deref()
                    .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                    .unwrap_or_default();
                SeriesRow { id: id as i32, name: name.into(), book_count: count as i32, cover: img }
            }).collect();
            w.set_book_series(slint::ModelRc::new(slint::VecModel::from(series_tiles)));
            // Author cards (np.p4.books.library — author pages)
            let author_rows: Vec<AuthorRow> = author_data.into_iter().map(|(name, count)| AuthorRow {
                name: name.into(), book_count: count as i32,
            }).collect();
            w.set_book_authors(slint::ModelRc::new(slint::VecModel::from(author_rows)));
            // Shelf cards (collections)
            let col_rows: Vec<CollectionRow> = col_data.into_iter().map(|(id, name, count, cover)| {
                let img = cover.as_deref()
                    .and_then(|p| slint::Image::load_from_path(std::path::Path::new(p)).ok())
                    .unwrap_or_default();
                CollectionRow { id: id as i32, name: name.into(), book_count: count as i32, cover: img }
            }).collect();
            w.set_book_collections(slint::ModelRc::new(slint::VecModel::from(col_rows)));
            // Reading stats string (np.p5.books.stats): counts + time read +
            // streak + yearly goal progress.
            let total = rows.len();
            let finished = rows.iter().filter(|r| r.finished).count();
            let reading = rows.iter().filter(|r| !r.finished && r.page > 0).count();
            let stats = if total > 0 {
                let mut s = format!("{total} books · {finished} finished · {reading} in progress");
                let (h, m) = (read_stats.total_seconds / 3600, (read_stats.total_seconds % 3600) / 60);
                if read_stats.total_seconds >= 60 {
                    if h > 0 { s.push_str(&format!(" · {h}h {m}m read")); }
                    else { s.push_str(&format!(" · {m}m read")); }
                }
                if read_stats.streak_days > 1 {
                    s.push_str(&format!(" · {}-day streak", read_stats.streak_days));
                }
                s.push_str(&format!(" · {}/{} this year",
                    read_stats.finished_this_year, read_stats.year_goal));
                s
            } else {
                String::new()
            };
            w.set_book_stats(stats.into());
            w.set_book_count(tiles.len() as i32);
            w.set_book_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
        });
    });
}

// ── Books reader (np.p4.books.reader / .navigation / .typography / .progress) ─
// The open book lives on the UI thread; comics page via ReaderState, EPUBs via
// a chapter list, both honouring saved reading_progress.
struct ReaderSession {
    item_id: i64,
    path: PathBuf,
    format: String,
    comic: tulipix_books::reader::ReaderState, // page/total/spread/rtl/invert (comics)
    chapters: Vec<String>,                     // EPUB plain-text per spine entry
    chapter: usize,
    typo: tulipix_books::typography::Typography,
    // EPUB pagination: `chapters` flattened into screen-pages for the given
    // geometry + typography. `chapter_starts[c]` is the flat page index where
    // chapter `c` begins (TOC jump + chapter label).
    pages: Vec<String>,
    chapter_starts: Vec<usize>,
    page: usize,    // current flat screen-page (text mode)
    page_w: f32,    // text-column px (for re-pagination)
    page_h: f32,
    turn: i32,      // bumped on each page turn → drives the fold animation
    // Comic guided view (np.p5.books.comic-guided): panel-by-panel stepping.
    guided: bool,
    panel: usize,                        // current panel on the current page
    panels: Vec<(u32, u32, u32, u32)>,   // detected rects for `panels_page`
    panels_page: usize,                  // page the cache belongs to
    // Find-in-book (np.p5.books.fulltext): repeating the query finds the NEXT
    // match after the current page (wraps).
    last_query: String,
    // Last rendered page faces — become the flip overlay's outgoing pages.
    last_left: String,
    last_right: String,
    // Reading-time tracking (np.p5.books.stats): start of the unflushed slice;
    // save_reader_progress flushes elapsed into reading_sessions and resets.
    read_since: std::time::Instant,
}
thread_local! {
    static READER: std::cell::RefCell<Option<ReaderSession>> = const { std::cell::RefCell::new(None) };
}
fn reader_clear() { READER.with(|r| *r.borrow_mut() = None); }

/// Reader-side format families (np.p5.books.formats): comics + raster
/// documents (scanned PDF via poppler, DjVu via djvulibre) page through
/// images, everything else reflows as paginated text.
fn fmt_is_comic(f: &str) -> bool { matches!(f, "cbz" | "cbr" | "pdf-raster" | "djvu") }
fn fmt_is_text(f: &str) -> bool { matches!(f, "epub" | "mobi" | "azw3" | "fb2" | "pdf") }

/// Read-aloud run counter: bumping it cancels the active sentence loop
/// (np.p5.books.tts). Monotonic; each start claims the new value.
static TTS_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Speak one sentence, blocking until audio finishes: Piper first, then the
/// platform voice. Sentence-sized calls keep the stop button responsive.
fn tts_speak_sentence(text: &str) {
    if tts_speak_piper(text) { return; }
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("spd-say").arg("--wait").arg(text).status()
        .or_else(|_| std::process::Command::new("espeak").arg(text).status());
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("say").arg(text).status();
    #[cfg(target_os = "windows")]
    let _ = {
        let ps = format!("Add-Type -AssemblyName System.Speech; \
            (New-Object System.Speech.Synthesis.SpeechSynthesizer).Speak(@'\n{}\n'@)",
            text.replace('\'', "''"));
        std::process::Command::new("powershell").args(["-NoProfile","-Command",&ps]).no_window().status()
    };
}

/// np.p4.books.tts — synthesize `text` with Piper (bundled or PATH binary +
/// the first .onnx voice in <data>/models/piper/) and play the wav. False when
/// any piece is missing so the caller can fall back to the platform voice.
fn tts_speak_piper(text: &str) -> bool {
    let attempt = || -> Option<()> {
        let piper = tulipix_core::thumbs::tool_bin("piper");
        let voices = dirs_default()?.join("models").join("piper");
        let voice = std::fs::read_dir(&voices).ok()?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .find(|p| p.extension().map(|e| e == "onnx").unwrap_or(false))?;
        let wav = std::env::temp_dir().join("tulipix-tts.wav");
        let mut child = std::process::Command::new(&piper)
            .arg("--model").arg(&voice)
            .arg("--output_file").arg(&wav)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .no_window()
            .spawn().ok()?;
        {
            use std::io::Write;
            child.stdin.take()?.write_all(text.as_bytes()).ok()?;
        }
        if !child.wait().ok()?.success() { return None; }
        // Play the wav with whatever audio CLI is around (bundled ffplay first).
        let ffplay = tulipix_core::thumbs::tool_bin("ffplay");
        let players: [(String, Vec<&str>); 4] = [
            (ffplay.display().to_string(), vec!["-nodisp", "-autoexit", "-loglevel", "quiet"]),
            (tulipix_core::thumbs::tool_bin("mpv").display().to_string(), vec!["--no-video", "--really-quiet"]),
            ("paplay".into(), vec![]),
            ("aplay".into(), vec!["-q"]),
        ];
        players.iter().any(|(bin, args)| {
            std::process::Command::new(bin).args(args).arg(&wav)
                .no_window().status().map(|s| s.success()).unwrap_or(false)
        }).then_some(())
    };
    attempt().is_some()
}

/// np.p5.books.dictionary — look a word up: dictionaryapi.dev definition,
/// MyMemory translation into the system locale, Wikipedia summary. Each source
/// is best-effort; whatever answered is concatenated.
async fn define_word(word: &str) -> String {
    let q: String = word.chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '\'' { c.to_string() }
             else { format!("%{:02X}", c as u32) })
        .collect();
    let mut out = String::new();
    // Dictionary definitions.
    if let Ok(r) = reqwest::get(format!("https://api.dictionaryapi.dev/api/v2/entries/en/{q}")).await {
        if let Ok(j) = r.json::<serde_json::Value>().await {
            if let Some(meanings) = j.get(0).and_then(|e| e.get("meanings")).and_then(|m| m.as_array()) {
                for m in meanings.iter().take(3) {
                    let pos = m.get("partOfSpeech").and_then(|p| p.as_str()).unwrap_or("");
                    if let Some(def) = m.pointer("/definitions/0/definition").and_then(|d| d.as_str()) {
                        out.push_str(&format!("• ({pos}) {def}\n"));
                    }
                }
            }
        }
    }
    // Translation into the system locale (skipped when the locale is English).
    let lang = std::env::var("LANG").unwrap_or_default()
        .get(0..2).unwrap_or("en").to_string();
    if lang != "en" && !lang.is_empty() {
        if let Ok(r) = reqwest::get(format!(
            "https://api.mymemory.translated.net/get?q={q}&langpair=en|{lang}")).await {
            if let Ok(j) = r.json::<serde_json::Value>().await {
                if let Some(t) = j.pointer("/responseData/translatedText").and_then(|t| t.as_str()) {
                    if !t.trim().is_empty() {
                        out.push_str(&format!("\n🌐 {lang}: {t}\n"));
                    }
                }
            }
        }
    }
    // Wikipedia summary.
    if let Ok(r) = reqwest::get(format!("https://en.wikipedia.org/api/rest_v1/page/summary/{q}")).await {
        if let Ok(j) = r.json::<serde_json::Value>().await {
            if let Some(extract) = j.get("extract").and_then(|e| e.as_str()) {
                if !extract.trim().is_empty() {
                    out.push_str(&format!("\n📖 Wikipedia: {extract}\n"));
                }
            }
        }
    }
    if out.trim().is_empty() {
        format!("No results for “{word}”. Check the spelling or try a simpler form.")
    } else {
        out
    }
}

fn family_to_int(f: tulipix_books::typography::FontFamily) -> i32 {
    use tulipix_books::typography::FontFamily::*;
    match f { Serif => 0, SansSerif => 1, OpenDyslexic => 2 }
}
fn int_to_family(i: i32) -> tulipix_books::typography::FontFamily {
    use tulipix_books::typography::FontFamily::*;
    match i { 1 => SansSerif, 2 => OpenDyslexic, _ => Serif }
}

/// Re-paginate the EPUB chapters into screen-pages for the session's current
/// geometry + typography. `preserve` keeps the reader near the same spot (by
/// fraction) across a reflow (font/margin/resize change).
fn reader_repaginate(s: &mut ReaderSession, preserve: bool) {
    use tulipix_books::paginate;
    let cap = paginate::chars_per_page(
        s.page_w, s.page_h, s.typo.font_px as f32, s.typo.line_height as f32, s.typo.family);
    let old_page = s.page;
    let old_total = s.pages.len();
    let mut pages: Vec<String> = Vec::new();
    let mut starts: Vec<usize> = Vec::with_capacity(s.chapters.len());
    for ch in &s.chapters {
        starts.push(pages.len());
        pages.extend(paginate::paginate(ch, cap));
    }
    if pages.is_empty() { pages.push(String::new()); starts = vec![0]; }
    s.chapter_starts = starts;
    s.pages = pages;
    s.page = if preserve {
        paginate::reflow_anchor(old_page, old_total, s.pages.len())
    } else {
        s.page.min(s.pages.len() - 1)
    };
    s.chapter = chapter_of_page(s, s.page);
}

/// Which chapter a flat screen-page belongs to.
fn chapter_of_page(s: &ReaderSession, page: usize) -> usize {
    match s.chapter_starts.binary_search(&page) {
        Ok(i) => i,
        Err(i) => i.saturating_sub(1),
    }
}

fn open_book(weak: slint::Weak<MainWindow>, idx: i32) {
    let Some(path) = book_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else { return; };
    let item_id = book_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).unwrap_or(-1);
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Saved progress (page index).
        let saved = match pool_for("books").await {
            Ok(pool) => tulipix_books::progress::get(&pool, item_id).await.ok().flatten(),
            Err(_) => None,
        };
        let saved_page = saved.map(|(p, _, _)| p as usize).unwrap_or(0);
        let pb = path.clone();
        // Heavy extraction off the UI thread.
        let prep = tokio::task::spawn_blocking(move || {
            let ingest = books::ingest_file(&pb);
            let fmt = ingest.as_ref().map(|i| i.format).unwrap_or("");
            let rtl = ingest.as_ref().map(|i| i.rtl).unwrap_or(false);
            match fmt {
                "cbz" | "cbr" | "djvu" => {
                    let total = books::comic_page_count(&pb);
                    // 0 pages = missing system tool (unrar/ddjvu) or bad file.
                    if total == 0 {
                        (format!("{fmt}-empty"), 0, Vec::new(), false)
                    } else {
                        (fmt.to_string(), total, Vec::<String>::new(), rtl)
                    }
                }
                "epub" | "mobi" | "azw3" | "fb2" | "pdf" => {
                    let chapters = match fmt {
                        "epub" => books::epub_chapters_text(&pb),
                        "mobi" | "azw3" => books::mobi_chapters_text(&pb),
                        "fb2" => books::fb2_chapters_text(&pb),
                        _ => books::pdf_chapters_text(&pb),
                    };
                    if chapters.is_empty() {
                        // Scanned PDF (no text layer): fall back to rendering
                        // pages via poppler and read it comic-style.
                        if fmt == "pdf" {
                            let total = books::comic_page_count(&pb);
                            if total > 0 {
                                return ("pdf-raster".to_string(), total, Vec::new(), false);
                            }
                        }
                        // Still nothing (DRM / missing tool) → honest notice.
                        (format!("{fmt}-empty"), 0, Vec::new(), false)
                    } else {
                        (fmt.to_string(), chapters.len(), chapters, false)
                    }
                }
                other => (other.to_string(), 0, Vec::new(), false),
            }
        }).await.unwrap_or(("".into(), 0, Vec::new(), false));
        let (format, total, chapters, rtl) = prep;
        let fmt2 = format.clone();
        let path2 = path.clone();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let comic = fmt_is_comic(&format);
            let mut state = tulipix_books::reader::ReaderState::new(total.max(1), comic, rtl);
            let start = saved_page.min(total.saturating_sub(1));
            let chapter = if comic { 0 } else { start };
            if comic { state.page = start; }
            let mut sess = ReaderSession {
                item_id, path: path.clone(), format: format.clone(),
                comic: state, chapters, chapter,
                typo: tulipix_books::typography::Typography::default(),
                pages: Vec::new(), chapter_starts: Vec::new(), page: 0,
                page_w: 700.0, page_h: 900.0, turn: 0,
                guided: false, panel: 0, panels: Vec::new(), panels_page: usize::MAX,
                last_query: String::new(),
                last_left: String::new(),
                last_right: String::new(),
                read_since: std::time::Instant::now(),
            };
            // Text formats: build the initial pagination (a real reflow follows
            // once the text stage reports its true size) and restore saved page.
            if fmt_is_text(&sess.format) {
                reader_repaginate(&mut sess, false);
                sess.page = saved_page.min(sess.pages.len().saturating_sub(1));
                sess.chapter = chapter_of_page(&sess, sess.page);
            }
            READER.with(|r| *r.borrow_mut() = Some(sess));
            w.set_book_reader_title(
                std::path::Path::new(&path).file_stem().and_then(|s| s.to_str()).unwrap_or("").into());
            w.set_book_toc_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<TocEntryRow>::new())));
            w.set_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(Vec::<BookmarkRow>::new())));
            w.set_book_comic_thumbs(slint::ModelRc::new(slint::VecModel::from(Vec::<slint::Image>::new())));
            reader_render(&w);
            w.set_book_reader_open(true);
        });
        // Comic timeline thumbs (np.p4.books.reader.navigation) — decoded off
        // the UI thread (disk-cached per page), pushed as one model when done.
        if fmt_is_comic(&fmt2) && total > 0 {
            let wk = weak.clone();
            tokio::task::spawn_blocking(move || {
                let thumbs: Vec<PathBuf> =
                    (0..total).filter_map(|i| books::comic_page_thumb(&path2, i)).collect();
                let _ = wk.upgrade_in_event_loop(move |w| {
                    let imgs: Vec<slint::Image> = thumbs.iter()
                        .filter_map(|p| slint::Image::load_from_path(p).ok()).collect();
                    w.set_book_comic_thumbs(slint::ModelRc::new(slint::VecModel::from(imgs)));
                });
            });
        }
        // Load TOC and bookmarks async after opening
        if item_id >= 0 {
            let wk = weak.clone();
            let handle2 = tokio::runtime::Handle::current();
            handle2.spawn(async move {
                let Ok(pool) = pool_for("books").await else { return; };
                let toc_data = tulipix_books::navigation::toc(&pool, item_id).await.unwrap_or_default();
                let bm_data = tulipix_books::progress::bookmarks(&pool, item_id).await.unwrap_or_default();
                let _ = wk.upgrade_in_event_loop(move |w| {
                    let toc_rows: Vec<TocEntryRow> = toc_data.into_iter().map(|e| TocEntryRow {
                        idx: e.idx as i32,
                        title: e.title.into(),
                        page: e.page.unwrap_or(0) as i32,
                    }).collect();
                    let bm_rows: Vec<BookmarkRow> = bm_data.into_iter().map(|(id, page, note, color)| BookmarkRow {
                        id: id as i32,
                        page: page as i32,
                        note: note.unwrap_or_default().into(),
                        color: color.unwrap_or_default().into(),
                    }).collect();
                    w.set_book_toc_entries(slint::ModelRc::new(slint::VecModel::from(toc_rows)));
                    w.set_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(bm_rows)));
                });
            });
        }
    });
}

/// Guided view (np.p5.books.comic-guided): make sure the panel rects for the
/// current page are detected before rendering. Mutates the session, so it runs
/// as a separate borrow ahead of `reader_render`'s shared borrow.
fn reader_ensure_panels() {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        if !(fmt_is_comic(&s.format) && s.guided) { return; }
        if s.panels_page == s.comic.page && !s.panels.is_empty() { return; }
        let rtl = s.comic.rtl;
        s.panels = books::comic_page_image(&s.path, s.comic.page, false)
            .map(|p| books::detect_panels(&p, rtl))
            .unwrap_or_default();
        s.panels_page = s.comic.page;
        s.panel = s.panel.min(s.panels.len().saturating_sub(1));
    });
}

fn reader_render(w: &MainWindow) {
    reader_ensure_panels();
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        match s.format.as_str() {
            f if fmt_is_comic(f) => {
                w.set_book_reader_kind("comic".into());
                let inv = s.comic.invert_images;
                if s.guided && !s.panels.is_empty() {
                    // One panel fills the stage; spread is ignored while guided.
                    let p = s.panel.min(s.panels.len() - 1);
                    let img = books::comic_panel_image(&s.path, s.comic.page, p, s.panels[p], inv);
                    w.set_book_page_left(img.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
                    w.set_book_page_right(slint::Image::default());
                    w.set_book_has_right(false);
                    w.set_book_page_label(format!(
                        "{} / {} · panel {} / {}",
                        s.comic.page + 1, s.comic.total, p + 1, s.panels.len()).into());
                } else {
                    let vis = s.comic.visible_pages();
                    let left = vis.first().and_then(|&i| books::comic_page_image(&s.path, i, inv));
                    let right = if matches!(s.comic.spread, tulipix_books::reader::SpreadMode::Double) {
                        vis.get(1).and_then(|&i| books::comic_page_image(&s.path, i, inv))
                    } else { None };
                    w.set_book_page_left(left.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
                    w.set_book_page_right(right.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default());
                    w.set_book_has_right(matches!(s.comic.spread, tulipix_books::reader::SpreadMode::Double) && vis.len() > 1);
                    w.set_book_page_label(format!("{} / {}", s.comic.page + 1, s.comic.total).into());
                }
                w.set_book_spread_double(matches!(s.comic.spread, tulipix_books::reader::SpreadMode::Double));
                w.set_book_rtl(s.comic.rtl);
                w.set_book_invert(s.comic.invert_images);
                w.set_book_guided(s.guided);
                w.set_book_progress(s.comic.fraction() as f32);
                w.set_book_page_index(s.comic.page as i32); // timeline highlight
                w.set_book_page_total(s.comic.total.max(1) as i32);
            }
            "epub" | "mobi" | "azw3" | "fb2" | "pdf" => {
                w.set_book_reader_kind("text".into());
                let total = s.pages.len().max(1);
                let page = s.page.min(total - 1);
                if w.get_book_read_mode() == "scroll" {
                    // Continuous mode (np.p4.books.continuous): whole chapter as
                    // one column; the nav bar steps/scrubs chapters.
                    let ctotal = s.chapters.len().max(1);
                    w.set_book_text(s.chapters.get(s.chapter).cloned().unwrap_or_default().into());
                    w.set_book_page_label(format!("Chapter {} / {}", s.chapter + 1, ctotal).into());
                    w.set_book_progress(s.chapter as f32 / (ctotal.saturating_sub(1).max(1) as f32));
                } else {
                    // Two-page spread: left/right faces; outgoing faces feed
                    // the flip overlay (np.p4.books.reader — real-book turn).
                    let two_up = w.get_book_two_up_active();
                    let left = s.pages.get(page).cloned().unwrap_or_default();
                    let right = if two_up {
                        s.pages.get(page + 1).cloned().unwrap_or_default()
                    } else { String::new() };
                    w.set_book_text_prev(std::mem::take(&mut s.last_left).into());
                    w.set_book_text_prev_right(std::mem::take(&mut s.last_right).into());
                    s.last_left = left.clone();
                    s.last_right = right.clone();
                    w.set_book_text(left.into());
                    w.set_book_text_right(right.into());
                    let label = if two_up && page + 1 < total {
                        format!("Ch {} · pages {}–{} / {}", s.chapter + 1, page + 1, page + 2, total)
                    } else {
                        format!("Ch {} · page {} / {}", s.chapter + 1, page + 1, total)
                    };
                    w.set_book_page_label(label.into());
                    w.set_book_progress(page as f32 / (total.saturating_sub(1).max(1) as f32));
                }
                w.set_book_page_index(page as i32);
                w.set_book_page_total(total as i32);
                w.set_book_font_px(s.typo.font_px as f32);
                w.set_book_line_height(s.typo.line_height as f32);
                w.set_book_line_height_val(s.typo.line_height as f32);
                w.set_book_margin_pct(s.typo.margin_pct as f32);
                w.set_book_font_family(family_to_int(s.typo.family));
                w.set_book_turn(s.turn);
            }
            _ => {
                w.set_book_reader_kind("unsupported".into());
                let label = match s.format.as_str() {
                    "cbr-empty" => "CBR needs unrar, bsdtar or 7z installed".to_string(),
                    "pdf-empty" => "Scanned PDF — install poppler-utils (pdftoppm) to view its pages".to_string(),
                    "djvu-empty" => "DjVu needs djvulibre (ddjvu) installed".to_string(),
                    f if f.ends_with("-empty") =>
                        format!("No readable text in this {} (DRM-protected?)",
                            f.trim_end_matches("-empty").to_uppercase()),
                    f => format!("{} files need a dedicated engine", f.to_uppercase()),
                };
                w.set_book_page_label(label.into());
            }
        }
    });
}

/// Persist the current reading position (page for comics / chapter for EPUB).
fn save_reader_progress() {
    let snap = READER.with(|r| {
        let mut g = r.borrow_mut();
        g.as_mut().map(|s| {
            let (page, total) = if s.comic.total > 0 && fmt_is_comic(&s.format) {
                (s.comic.page as i64, Some(s.comic.total as i64))
            } else {
                (s.page as i64, Some(s.pages.len().max(1) as i64))
            };
            // Flush the reading-time slice accumulated since the last save
            // (np.p5.books.stats) and restart the clock.
            let secs = s.read_since.elapsed().as_secs() as i64;
            s.read_since = std::time::Instant::now();
            (s.item_id, page, total, secs)
        })
    });
    let Some((item_id, page, total, secs)) = snap else { return; };
    if item_id < 0 { return; }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("books").await {
            let _ = tulipix_books::progress::save(&pool, item_id, &page.to_string(), page, total).await;
            // Cap a slice at 30 min so an overnight idle reader doesn't count.
            let _ = tulipix_books::progress::add_reading_time(&pool, item_id, secs.min(1800)).await;
        }
    });
}

fn reader_step(w: &MainWindow, dir: i32) {
    let scroll = w.get_book_read_mode() == "scroll";
    // Two-page spread turns two pages at once, like a real book.
    let step = if w.get_book_two_up_active() { 2usize } else { 1 };
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        if fmt_is_comic(&s.format) && s.guided {
            // Guided view: panel → panel, overflowing to the next/prev page
            // (panels for the new page are detected by reader_ensure_panels;
            // usize::MAX clamps to the LAST panel when stepping backwards).
            if dir > 0 {
                if s.panel + 1 < s.panels.len() { s.panel += 1; }
                else if s.comic.page + 1 < s.comic.total {
                    s.comic.page += 1; s.panel = 0; s.panels_page = usize::MAX;
                }
            } else if s.panel > 0 { s.panel -= 1; }
            else if s.comic.page > 0 {
                s.comic.page -= 1; s.panel = usize::MAX; s.panels_page = usize::MAX;
            }
        } else if fmt_is_comic(&s.format) {
            if dir > 0 { s.comic.next(); } else { s.comic.prev(); }
        } else if fmt_is_text(&s.format) && scroll {
            // Continuous mode steps whole chapters.
            let c = if dir > 0 { (s.chapter + 1).min(s.chapters.len().saturating_sub(1)) }
                    else { s.chapter.saturating_sub(1) };
            s.chapter = c;
            s.page = s.chapter_starts.get(c).copied().unwrap_or(0);
        } else if fmt_is_text(&s.format) {
            let total = s.pages.len();
            if dir > 0 {
                if s.page + step < total { s.page += step; s.turn += 1; }
                else if s.page + 1 < total { s.page = total - 1; s.turn += 1; }
            } else if s.page > 0 {
                s.page = s.page.saturating_sub(step);
                s.turn += 1;
            }
            s.chapter = chapter_of_page(s, s.page);
        }
    });
    w.set_book_turn_dir(if dir > 0 { 1 } else { -1 });
    reader_render(w);
    save_reader_progress();
}

fn reader_scrub(w: &MainWindow, frac: f32) {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        let Some(s) = g.as_mut() else { return; };
        if fmt_is_comic(&s.format) {
            s.comic.page = tulipix_books::navigation::scrub_to_page(frac as f64, s.comic.total);
            s.panel = 0;
        } else if fmt_is_text(&s.format) {
            let total = s.pages.len();
            s.page = tulipix_books::navigation::scrub_to_page(frac as f64, total);
            s.chapter = chapter_of_page(s, s.page);
        }
    });
    reader_render(w);
    save_reader_progress();
}

fn reader_toggle_spread(w: &MainWindow) {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        if let Some(s) = g.as_mut() {
            use tulipix_books::reader::SpreadMode::*;
            s.comic.spread = match s.comic.spread { Single => Double, Double => Single };
        }
    });
    reader_render(w);
}
fn reader_toggle_rtl(w: &MainWindow) {
    READER.with(|r| { if let Some(s) = r.borrow_mut().as_mut() { s.comic.rtl = !s.comic.rtl; } });
    reader_render(w);
}
fn reader_toggle_invert(w: &MainWindow) {
    READER.with(|r| { if let Some(s) = r.borrow_mut().as_mut() { s.comic.toggle_invert(); } });
    reader_render(w);
}
fn reader_set_typo(w: &MainWindow, font: Option<f32>, line: Option<f32>, family: Option<i32>, margin: Option<f32>) {
    READER.with(|r| {
        let mut g = r.borrow_mut();
        if let Some(s) = g.as_mut() {
            if let Some(f) = font { s.typo.font_px = f as f64; }
            if let Some(l) = line { s.typo.line_height = l as f64; }
            if let Some(fam) = family { s.typo.family = int_to_family(fam); }
            if let Some(m) = margin { s.typo.margin_pct = m as f64; }
            s.typo = s.typo.clamped();
            // Font/line/family changes alter how much text fits a page; reflow,
            // keeping the reader near the same spot. (Margin reflows via the
            // text-stage resize callback, but repaginate here too for keyboard.)
            if fmt_is_text(&s.format) { reader_repaginate(s, true); }
        }
    });
    reader_render(w);
}

/// The text stage reported a new size — re-paginate EPUB to fill it exactly.
fn reader_reflow(w: &MainWindow, width: f32, height: f32) {
    let changed = READER.with(|r| {
        let mut g = r.borrow_mut();
        match g.as_mut() {
            Some(s) if fmt_is_text(&s.format) && width > 8.0 && height > 8.0
                && ((s.page_w - width).abs() > 1.0 || (s.page_h - height).abs() > 1.0) => {
                s.page_w = width;
                s.page_h = height;
                reader_repaginate(s, true);
                true
            }
            _ => false,
        }
    });
    if changed { reader_render(w); }
}

// ── Cloud data layer (np.p4.cloud.remotes / .browse) ─────────────────────────
fn theme_choice_str(c: ThemeChoice) -> &'static str {
    match c {
        ThemeChoice::Light => "light",
        ThemeChoice::ExtraDark => "extra-dark",
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
    use souvlaki::{MediaControlEvent, MediaControls, MediaPlayback, PlatformConfig};

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

/// Time-of-day greeting for the Home header; recomputed on each Home landing
/// so a long-running app stays fresh across day boundaries.
fn set_home_greeting_now(w: &MainWindow) {
    use chrono::Timelike;
    let name = w.get_user().display_name.to_string();
    let who = if name.trim().is_empty() { "there".to_string() } else { name };
    let g = match chrono::Local::now().hour() {
        5..=11  => format!("Good morning, {who}"),
        12..=16 => format!("Good afternoon, {who}"),
        _       => format!("Good evening, {who}"),
    };
    w.set_home_greeting(g.into());
    w.set_home_date_line(chrono::Local::now().format("%A, %B %-d").to_string().into());
}

/// Seed the Home command-center stats (np.p6.home). Cheap COUNTs per section
/// DB; called at startup and on every landing on the Home section. Every
/// count is best-effort — a failed query leaves 0/"", never errors.
fn kick_home_stats(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let mut s = HomeStats::default();
        let count = |pool: sqlx::SqlitePool, q: &'static str| async move {
            sqlx::query_scalar::<_, i64>(q).fetch_one(&pool).await.unwrap_or(0) as i32
        };
        if let Ok(pool) = pool_for("photos").await {
            s.photos = count(pool.clone(), "SELECT COUNT(*) FROM items").await;
            s.photos_albums = count(pool, "SELECT COUNT(*) FROM albums").await;
        }
        if let Ok(pool) = pool_for("videos").await {
            s.videos = count(pool.clone(), "SELECT COUNT(*) FROM items").await;
            s.videos_shows = count(pool, "SELECT COUNT(*) FROM shows").await;
        }
        if let Ok(pool) = pool_for("music").await {
            s.songs = count(pool.clone(),
                "SELECT COUNT(*) FROM track_meta WHERE is_audiobook = 0").await;
            s.audiobooks = count(pool.clone(),
                "SELECT COUNT(DISTINCT folder) FROM track_meta WHERE is_audiobook = 1").await;
            // Continue card: newest in-progress audiobook (folder = the book).
            if let Ok(Some((folder, pos))) = sqlx::query_as::<_, (String, f64)>(
                "SELECT tm.folder, ap.position_s FROM audiobook_progress ap \
                 JOIN track_meta tm ON tm.item_id = ap.item_id \
                 WHERE ap.finished = 0 AND ap.position_s > 0 AND tm.folder IS NOT NULL \
                 ORDER BY ap.updated DESC LIMIT 1")
                .fetch_optional(&pool).await
            {
                let book = std::path::Path::new(&folder).file_name()
                    .map(|f| f.to_string_lossy().into_owned()).unwrap_or(folder.clone());
                s.continue3 = format!("🎧 {book}").into();
                s.continue3_sub = format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60).into();
            }
        }
        if let Ok(pool) = pool_for("podcasts").await {
            s.podcasts = count(pool.clone(), "SELECT COUNT(*) FROM podcasts").await;
            // Continue card: newest partially-played episode.
            if let Ok(Some((title, pos, dur))) = sqlx::query_as::<_, (String, f64, Option<f64>)>(
                "SELECT COALESCE(title, ''), position_s, duration_s FROM podcast_episodes \
                 WHERE position_s > 0 AND played = 0 \
                 ORDER BY COALESCE(downloaded_at, published) DESC LIMIT 1")
                .fetch_optional(&pool).await
            {
                if !title.is_empty() {
                    s.continue2 = format!("🎙 {title}").into();
                    s.continue2_sub = match dur {
                        Some(d) if d > 0.0 =>
                            format!("{}:{:02} · {}%", (pos as i64) / 60, (pos as i64) % 60,
                                    ((pos / d) * 100.0).round() as i64),
                        _ => format!("{}:{:02} in", (pos as i64) / 60, (pos as i64) % 60),
                    }.into();
                }
            }
        }
        if let Ok(pool) = pool_for("radio").await {
            s.radio = count(pool, "SELECT COUNT(*) FROM radio_stations").await;
        }
        if let Ok(pool) = pool_for("books").await {
            s.books = count(pool.clone(), "SELECT COUNT(*) FROM items").await;
            s.books_reading = count(pool.clone(),
                "SELECT COUNT(*) FROM reading_progress \
                 WHERE finished = 0 AND (page > 0 OR locator IS NOT NULL)").await;
            // Continue card: newest in-progress book.
            if let Ok(Some((title, path, page, total))) = sqlx::query_as::<_, (String, String, i64, Option<i64>)>(
                "SELECT COALESCE(NULLIF(bm.title, ''), ''), i.abs_path, rp.page, rp.total_pages \
                 FROM reading_progress rp \
                 JOIN items i ON i.id = rp.item_id \
                 LEFT JOIN book_meta bm ON bm.item_id = rp.item_id \
                 WHERE rp.finished = 0 AND (rp.page > 0 OR rp.locator IS NOT NULL) \
                 ORDER BY rp.updated DESC LIMIT 1")
                .fetch_optional(&pool).await
            {
                let name = if title.is_empty() {
                    std::path::Path::new(&path).file_stem()
                        .map(|f| f.to_string_lossy().into_owned()).unwrap_or(path.clone())
                } else { title };
                s.continue1 = format!("📖 {name}").into();
                s.continue1_sub = match total {
                    Some(t) if t > 0 => format!("page {page} · {}%", (page * 100) / t),
                    _ => format!("page {page}"),
                }.into();
            }
        }
        if let Ok(pool) = pool_for("cloud").await {
            s.cloud_remotes = count(pool, "SELECT COUNT(*) FROM remotes").await;
        }
        if let Ok(pool) = pool_for("tools").await {
            s.tools_jobs = count(pool.clone(),
                "SELECT COUNT(*) FROM jobs WHERE state = 'running'").await;
            let queued = count(pool,
                "SELECT COUNT(*) FROM jobs WHERE state = 'queued'").await;
            if queued > 0 { s.tools_note = format!("{queued} queued").into(); }
        }
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_home_stats(s); });
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
        hdr("CLOUD"),
        tog(&s, "ai.cloud-offload", false, "Allow cloud AI help", "Send selected questions to a cloud AI service. Off = everything stays on-device"),
    ]);
    w.set_ai_rows(ModelRc::new(VecModel::from(ai)));

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
        si("idle_lock_secs", "text", "Idle timeout (seconds)", "How long before auto-lock kicks in — blank uses the default", &idle_secs_val, false, ""),
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
    let mut sys = vec![
        hdr("PERFORMANCE (LIVE)"),
        stat("Startup to ready", &format!("{} ms", startup_ms()), if startup_ms() < 500 { "ok" } else { "warn" }),
        stat("Allocator", allocator_name(), "ok"),
        stat("Resident memory", &format!("{} MB", rss_mb()), if rss_mb() < 300 { "ok" } else { "warn" }),
        stat("Thumbnail cache", &format!("{} MB", cache_mb), "muted"),
        stat("GPU texture budget", "200 MB LRU", "ok"),
        stat("Off-main-thread IO/decode", "Enforced", "ok"),
        stat("Disk-IO throttle", "Auto (rotational detect)", "ok"),
        stat("Viewport prefetch", "Velocity-aware", "ok"),
        stat("120 Hz / VRR", "Frame budget 8.3 ms", "ok"),
        tog(&s, "power-aware", true, "Battery / network aware", "Pause background scanning on battery or metered connections"),
        hdr("BUNDLED TOOLS"),
        // Universal override — look for mpv / yt-dlp / ffmpeg / ffprobe / etc. in
        // this folder first (before the bundled copy and PATH). One setting for
        // every external tool; handy on Windows where they aren't on PATH.
        txt(&s, "tools.bin-dir", "Tools directory", "Folder holding mpv, yt-dlp, ffmpeg… — checked before bundled + PATH (blank = off)"),
        act("tools-dir-browse", "Choose tools directory", "Pick the folder containing your external tool binaries", "Browse…"),
        tool_row("ffmpeg"), tool_row("ffprobe"), tool_row("rclone"),
        tool_row("yt-dlp"), tool_row("whisper-cli"), tool_row("mpv"),
        stat("ggml-tiny model", if bundled_present("ggml-tiny-1.0.bin") { "Bundled (75 MB)" } else { "Missing" },
             if bundled_present("ggml-tiny-1.0.bin") { "ok" } else { "warn" }),
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
        stat("Installer size budget", "< 400 MB (Linux)", "ok"),
        stat("cargo-bloat CI gate", "Per-crate tracked", "ok"),
        // Per-crate lazy-load state (np.p1.perf.lazy-crate): compiled-in or
        // excluded from this binary, per feature flag.
        stat("whisper (lazy-whisper)", if cfg!(feature = "lazy-whisper") { "Compiled — loads on first use" } else { "Not in this binary" },
             if cfg!(feature = "lazy-whisper") { "ok" } else { "muted" }),
        stat("AI exec providers (lazy-ai-ep)", if cfg!(feature = "lazy-ai-ep") { "Compiled — loads on first use" } else { "Not in this binary" },
             if cfg!(feature = "lazy-ai-ep") { "ok" } else { "muted" }),
        stat("Plugin sandboxes (lazy-plugins)", if cfg!(feature = "lazy-plugins") { "Compiled — loads on first use" } else { "Not in this binary" },
             if cfg!(feature = "lazy-plugins") { "ok" } else { "muted" }),
        stat("ONNX editor ops (ai-onnx)", if cfg!(feature = "ai-onnx") { "Compiled" } else { "Not in this binary" },
             if cfg!(feature = "ai-onnx") { "ok" } else { "muted" }),
        stat("Localisation", "Fluent .ftl · en", "ok"),
        stat("RTL layout", "Auto-mirror per locale", "ok"),
        stat("Adaptive layout", "Desktop breakpoints", "ok"),
        stat("Player embedding", "Out-of-process mpv — isolation by design (embedded GL parked: froze Intel iGPUs)", "ok"),
        // Format-support audit (np.p4.music.formats) — the claim table.
        stat("Audio formats", &{
                 let all: Vec<&str> = tulipix_music::formats::FORMATS.iter().map(|f| f.ext).collect();
                 let gaps = tulipix_music::formats::gapless_gaps();
                 format!("{} — gapless unverified: {}", all.join(" · "), gaps.join(", "))
             }, "ok"),
        hdr("PLUGINS"),
        stat("Plugin engine (WASM / Lua)",
             if cfg!(feature = "lazy-plugins") { "ABI v0 — runtimes compiled, load on first use" } else { "ABI v0 ready — rebuild with --features lazy-plugins to load" },
             if cfg!(feature = "lazy-plugins") { "ok" } else { "muted" }),
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
        "books"    => { w.set_book_query(t.clone());   w.invoke_book_search(t); }
        "cloud"    => { w.set_cloud_query(t.clone());  w.invoke_cloud_search(t); }
        "tools"    => { w.set_tools_query(t.clone());  w.invoke_tools_search(t); }
        "music"    => { w.set_music_query(t.clone());  w.invoke_music_search(t); }
        "podcasts" => { w.set_music_query(t.clone());  w.invoke_music_podcast_search(t); }
        "radio"    => { w.set_music_query(t.clone());  w.invoke_music_radio_search(t); }
        "youtube"  => { w.set_music_query(t.clone());  w.invoke_music_yt_search(t); }
        _ => tracing::warn!(%target, "voice route: unknown target"),
    }
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

fn startup_ms() -> u64 {
    APP_START.get().map(|t| t.elapsed().as_millis() as u64).unwrap_or(0)
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
fn tool_row(name: &str) -> SettingItem {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let in_tools_dir = tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("tools.bin-dir")).filter(|d| !d.trim().is_empty())
        .map(|d| std::path::Path::new(d.trim()).join(format!("{name}{ext}")).exists())
        .unwrap_or(false);
    if in_tools_dir { stat(name, "Tools directory", "ok") }
    else if bundled_present(name) { stat(name, "Bundled", "ok") }
    else if on_path(name) || on_path(&format!("{name}{ext}")) { stat(name, "System", "warn") }
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

fn flush_progress(weak: &slint::Weak<MainWindow>) {
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
        // 5 s (so the final counts are readable) then hide it.
        let all_done = !rows.is_empty()
            && rows.iter().all(|r| r.total > 0 && r.added + r.failed >= r.total);
        w.set_scan_progress(slint::ModelRc::new(slint::VecModel::from(rows)));
        w.set_scan_active(any_active);
        use std::sync::atomic::Ordering::Relaxed;
        if all_done && w.get_scan_active() && !SCAN_HIDE_SCHEDULED.swap(true, Relaxed) {
            let weak = w.as_weak();
            slint::Timer::single_shot(std::time::Duration::from_secs(5), move || {
                if let Some(w) = weak.upgrade() { w.set_scan_active(false); }
            });
        }
        if !all_done { SCAN_HIDE_SCHEDULED.store(false, Relaxed); }
    });
}

/// One pending auto-hide per finished scan session (reset when a new scan
/// starts producing unfinished rows again).
static SCAN_HIDE_SCHEDULED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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

/// Pure-Rust thumbnail renderer for the formats the `image` crate handles
/// natively (JPEG/PNG/WebP/GIF/BMP/TIFF). Writes a JPEG to the same cache
/// folder ThumbResult uses so the rest of the app can't tell the difference.
fn render_thumb_pure_rust(src: &std::path::Path) -> Result<PathBuf> {
    use sha2::{Digest, Sha256};
    let meta = std::fs::metadata(src).with_context(|| format!("stat {}", src.display()))?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let size = meta.len();
    let mut h = Sha256::new();
    h.update(src.display().to_string().as_bytes());
    h.update(format!(":{mtime}:{size}").as_bytes());
    let d = h.finalize();
    let mut key = String::with_capacity(24);
    for &b in d.iter().take(6) {
        key.push_str(&format!("{:02x}", b));
    }
    let cache_root = dirs_default()
        .map(|d| d.join("cache").join("thumbs"))
        .ok_or_else(|| anyhow::anyhow!("no cache dir"))?;
    std::fs::create_dir_all(&cache_root).ok();
    let out = cache_root.join(format!("{key}.jpg"));
    if out.exists() {
        return Ok(out);
    }
    let img = image::open(src).with_context(|| format!("decode {}", src.display()))?;
    let thumb = img.thumbnail(320, 320);
    let mut buf = std::io::BufWriter::new(std::fs::File::create(&out)?);
    thumb
        .to_rgb8()
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .with_context(|| format!("encode thumb {}", out.display()))?;
    Ok(out)
}

/// Render a thumb for `src` using ffmpeg if available, falling back to the
/// pure-Rust path for the photo case so the app works without `just fetch`.
/// Runs the whole thing on a blocking worker so the tokio reactor stays
/// responsive while ffmpeg / image-crate decode are crunching.
async fn thumb_for(src: PathBuf, kind: tulipix_core::thumbs::ThumbKind) -> Result<PathBuf> {
    tokio::task::spawn_blocking(move || {
        match tulipix_core::thumbs::render_or_cache(
            &src,
            tulipix_core::thumbs::ThumbSpec { kind, width: 320, height: 320 },
        ) {
            Ok(Some(t)) => Ok(t.path),
            Ok(None) => {
                if matches!(kind, tulipix_core::thumbs::ThumbKind::Photo) {
                    render_thumb_pure_rust(&src)
                } else {
                    Ok(src.clone())
                }
            }
            Err(e) => {
                if matches!(kind, tulipix_core::thumbs::ThumbKind::Photo) {
                    // ffmpeg missing / failed — try pure-rust JPEG/PNG path.
                    render_thumb_pure_rust(&src).context(e)
                } else {
                    Err(e)
                }
            }
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

/// Insert (or update) a single file row in the section DB. Returns the
/// item id. Caller is responsible for any per-section side effects.
async fn upsert_one(
    pool: &sqlx::SqlitePool,
    section: &str,
    path: &std::path::Path,
) -> Result<i64> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("stat {}", path.display()))?;
    let abs = path.to_string_lossy().into_owned();
    let inode = inode_of(&meta);
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let now = now_secs();
    let existing: Option<i64> = sqlx::query_scalar(
        "SELECT id FROM items WHERE abs_path = ?",
    )
    .bind(&abs)
    .fetch_optional(pool)
    .await
    .context("query existing row")?;
    if let Some(id) = existing {
        sqlx::query(
            "UPDATE items SET inode = ?, size = ?, mtime = ?, missing_since = NULL, updated = ? WHERE id = ?",
        )
        .bind(inode).bind(size).bind(mtime).bind(now).bind(id)
        .execute(pool).await.context("update row")?;
        Ok(id)
    } else {
        let res = sqlx::query(
            "INSERT INTO items (abs_path, inode, size, mtime, section, added, updated) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&abs).bind(inode).bind(size).bind(mtime)
        .bind(section).bind(now).bind(now)
        .execute(pool).await.context("insert row")?;
        Ok(res.last_insert_rowid())
    }
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
        while !pending.is_empty() {
            let frac = f32::from_bits(SCAN_HINT.load(Relaxed)).clamp(0.0, 1.0);
            let want = ((frac as f64) * total_files.saturating_sub(1) as f64) as usize;
            let key = pending.range(want..).next().map(|(k, _)| *k)
                .or_else(|| pending.range(..want).next_back().map(|(k, _)| *k))
                .unwrap_or(0);
            let slot_idx = key;
            let p = &pending.remove(&key).unwrap();
        {
            // 2a) DB insert. Bubble specific reason into the progress card.
            let id = match upsert_one(&pool, section, p).await {
                Ok(id) => id,
                Err(e) => {
                    let msg = friendly_err("DB insert", p, &e);
                    tracing::warn!(section, path = %p.display(), "{msg}");
                    counters.failed.fetch_add(1, Relaxed);
                    if let Ok(mut g) = counters.last_error.lock() { *g = msg; }
                    continue;
                }
            };

            // 2b) Per-section side effects (EXIF for photos, video_meta seed).
            // For books: extract real metadata + cover (np.p4.books.scan) — the
            // cover replaces the synthetic Book placeholder thumb below.
            let mut book_cover: Option<PathBuf> = None;
            let mut book_label: Option<String> = None;
            if section == "photos" {
                let _ = tulipix_photos::exif::ingest(&pool, id).await;
            } else if section == "videos" {
                let _ = sqlx::query("INSERT OR IGNORE INTO video_meta (item_id) VALUES (?)")
                    .bind(id).execute(&pool).await;
                // TV detection (np.p3.episodes): an SxxEyy filename ⇒ episode of
                // the show named by its parent folder. Populates shows+episodes
                // so the Videos "TV" tab can group it.
                classify_tv_episode(&pool, id, p).await;
                // TMDB/TVDB poster scrape runs in background so it doesn't
                // block the scan loop (np.p3.tmdb).
                {
                    let pool2 = pool.clone();
                    let path2 = p.to_owned();
                    tokio::spawn(async move { scrape_video_tmdb(pool2, id, path2).await; });
                }
            } else if section == "books" {
                let pb = p.clone();
                if let Ok(Some(ing)) = tokio::task::spawn_blocking(move || books::ingest_file(&pb)).await {
                    let cover = ing.cover.as_ref().map(|c| c.to_string_lossy().into_owned());
                    let _ = sqlx::query(
                        "INSERT INTO book_meta (item_id, format, title, author, language, is_comic, rtl, page_count, cover_path)
                         VALUES (?,?,?,?,?,?,?,?,?)
                         ON CONFLICT(item_id) DO UPDATE SET format=excluded.format, title=excluded.title,
                            author=excluded.author, language=excluded.language, is_comic=excluded.is_comic,
                            rtl=excluded.rtl, page_count=excluded.page_count, cover_path=excluded.cover_path",
                    )
                    .bind(id).bind(ing.format).bind(&ing.title).bind(&ing.author).bind(&ing.language)
                    .bind(ing.is_comic as i64).bind(ing.rtl as i64).bind(ing.page_count).bind(&cover)
                    .execute(&pool).await;
                    book_cover = ing.cover;
                    book_label = ing.title;
                }
            }

            // 2c) Thumbnail. Run on a blocking worker so ffmpeg / image-crate
            // decode doesn't choke the tokio reactor; pure-rust fallback for
            // photos when ffmpeg is missing.
            let label = book_label
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string());
            let (thumb, ok, err) = if let Some(cover) = book_cover {
                (cover, true, None) // real EPUB/CBZ cover
            } else {
                match thumb_for(p.clone(), kind).await {
                    Ok(t)  => (t, true, None),
                    Err(e) => (p.clone(), false, Some(friendly_err("Thumb", p, &e))),
                }
            };
            slots[slot_idx] = Some((thumb, label, p.clone()));
            if ok {
                counters.added.fetch_add(1, Relaxed);
            } else {
                counters.failed.fetch_add(1, Relaxed);
                if let Some(msg) = err {
                    if let Ok(mut g) = counters.last_error.lock() { *g = msg; }
                }
            }
        }
        }
        // Re-assemble in original walk order — grid placement stays stable.
        let tiles: Vec<(PathBuf, String, PathBuf)> = slots.into_iter().flatten().collect();
        counters.active.store(false, Relaxed);
        flush_progress(&weak);

        let cols = 6i32;
        let lib_id_for_ui = lib_id.clone();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let mut out: Vec<PhotoTile> = Vec::with_capacity(tiles.len());
            let mut paths: Vec<PathBuf> = Vec::with_capacity(tiles.len());
            let mut full: Vec<(String, PathBuf, PathBuf)> = Vec::with_capacity(tiles.len());
            for (i, (thumb_path, label, orig)) in tiles.into_iter().enumerate() {
                let img = slint::Image::load_from_path(&thumb_path).unwrap_or_default();
                full.push((label.clone(), orig.clone(), thumb_path.clone()));
                out.push(PhotoTile {
                    thumb: img,
                    label: label.into(),
                    col: (i as i32) % cols,
                    row: (i as i32) / cols,
                    index: i as i32,
                    starred: false,
                    selected: false,
                    color_label: "".into(),
                    is_live: false,
                    stack_count: 0,
                    count: 0,
                });
                paths.push(orig);
            }
            let n = out.len();
            match section {
                "photos" => {
                    let _ = (paths, out, n); // grid is rebuilt by the refresh below
                    // Append this folder's photos (dedup by abs path) so multiple
                    // watched folders accumulate instead of replacing each other.
                    if let Ok(mut g) = photo_full().lock() {
                        let have: std::collections::HashSet<String> =
                            g.iter().map(|(_, o, _)| o.to_string_lossy().into_owned()).collect();
                        for row in full {
                            if !have.contains(&row.1.to_string_lossy().into_owned()) { g.push(row); }
                        }
                        w.set_photos_total(g.len() as i32);
                    }
                    w.set_photos_folder_count(photo_folder_count());
                    // Rebuild the active tab from the accumulated library.
                    let cat = w.get_photos_category().to_string();
                    let q = w.get_photos_query().to_string();
                    kick_category_refresh(w.as_weak(), cat, q);
                }
                "videos" => {
                    // Accumulate this folder's videos (label, abs, thumb); the
                    // active tab is rebuilt from the DB by kick_video_refresh so
                    // star/archive/trash/progress all reflect live state.
                    let _ = (paths, out, n);
                    if let Ok(mut g) = video_full().lock() {
                        let have: std::collections::HashSet<String> =
                            g.iter().map(|(_, o, _)| o.to_string_lossy().into_owned()).collect();
                        for row in full {
                            if !have.contains(&row.1.to_string_lossy().into_owned()) { g.push(row); }
                        }
                    }
                    let cat = w.get_video_category().to_string();
                    kick_video_refresh(w.as_weak(), cat);
                }
                "music"  => {
                    let _ = (paths, out, n);
                    // Accumulate this folder's tracks (dedup by abs path) so adding
                    // a second folder ADDS to the library instead of replacing the
                    // first. The DB already holds every folder's items; this keeps
                    // the in-memory tiles/paths in sync with that.
                    if let Ok(mut g) = music_full().lock() {
                        let have: std::collections::HashSet<String> =
                            g.iter().map(|(_, o, _)| o.to_string_lossy().into_owned()).collect();
                        for row in full {
                            if !have.contains(&row.1.to_string_lossy().into_owned()) { g.push(row); }
                        }
                    }
                    rebuild_music_tiles(&w);
                    populate_music_views(w.as_weak());
                    // Refresh the scanned-roots list (Folders tab) so a freshly
                    // added music folder shows up immediately with its count.
                    populate_folder_roots(&w);
                    // Extract tags (title/artist/album/…) for items lacking them,
                    // then refresh the views (np.p4.music.tags).
                    ingest_music_tags(w.as_weak());
                }
                "books"  => {
                    // book_meta + covers were written during the scan side-effect;
                    // rebuild the active library view from the DB (progress, view
                    // filter, real titles/authors).
                    let _ = (paths, out, n);
                    refresh_books(&w);
                }
                _ => {}
            }
            let model = w.get_library_rows();
            let mut rows: Vec<LibraryRow> = (0..model.row_count())
                .map(|i| model.row_data(i).unwrap()).collect();
            for r in rows.iter_mut() {
                if r.id == lib_id_for_ui.as_str() {
                    r.r#last_scan = "just now".into();
                    r.item_count = n as i32;
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
