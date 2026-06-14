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

#[cfg(all(feature = "alloc-mimalloc", any(target_os = "linux", target_os = "windows")))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(all(feature = "alloc-jemalloc", any(target_os = "linux", target_os = "windows"), not(feature = "alloc-mimalloc")))]
#[global_allocator]
static GLOBAL: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

slint::include_modules!();

#[cfg(feature = "dev-reload")]
mod dev_reload;
#[cfg(feature = "embedded-mpv")]
mod mpv;
// No-op stand-in when libmpv isn't linked (e.g. Windows --no-default-features).
#[cfg(not(feature = "embedded-mpv"))]
#[path = "mpv_stub.rs"]
mod mpv;
mod mpv_ipc;
mod books;
mod cloud;

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
        unsafe { std::env::set_var("SLINT_BACKEND", "winit-skia-opengl"); }
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
    let idle_secs = std::env::var("TULIPIX_IDLE_SECS").ok()
        .and_then(|s| s.parse::<u64>().ok())
        .or_else(|| (boot_settings.flag("autolock", false) && boot_settings.idle_lock_secs > 0)
            .then_some(boot_settings.idle_lock_secs))
        .unwrap_or(600);
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
    let w = window.as_weak();
    window.on_cloud_refresh(move || { cloud_refresh_remotes(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_connect_open_dialog(move || {
        if let Some(w) = w.upgrade() {
            w.set_cloud_form_name("".into());
            w.set_cloud_form_backend("drive".into());
            w.set_cloud_form_opts("".into());
            w.set_cloud_status("".into());
            w.set_cloud_connect_open(true);
        }
    });
    let w = window.as_weak();
    window.on_cloud_connect_cancel(move || { if let Some(w) = w.upgrade() { w.set_cloud_connect_open(false); } });
    let w = window.as_weak();
    window.on_cloud_connect_submit(move || {
        let Some(w0) = w.upgrade() else { return; };
        cloud_connect(w.clone(), w0.get_cloud_form_name().to_string(),
            w0.get_cloud_form_backend().to_string(), w0.get_cloud_form_opts().to_string());
    });
    let w = window.as_weak();
    window.on_cloud_remote_open(move |name| { cloud_open_remote(w.clone(), name.to_string()); });
    let w = window.as_weak();
    window.on_cloud_remote_delete(move |name| { cloud_delete(w.clone(), name.to_string()); });
    let w = window.as_weak();
    window.on_cloud_entry_activate(move |idx| {
        let Some(w0) = w.upgrade() else { return; };
        let entries = w0.get_cloud_entries();
        if let Some(e) = entries.row_data(idx as usize) {
            if e.is_dir { cloud_into(w.clone(), e.name.to_string()); }
            else {
                let (remote, rel, name) = cloud_entry_target(&e.name);
                cloud_open_file(w.clone(), remote, rel, name);
            }
        }
    });
    let w = window.as_weak();
    window.on_cloud_ctx_action(move |idx, action| {
        let Some(w0) = w.upgrade() else { return; };
        let entries = w0.get_cloud_entries();
        let Some(e) = entries.row_data(idx as usize) else { return; };
        let (remote, rel, name) = cloud_entry_target(&e.name);
        match action.as_str() {
            "open" => {
                if e.is_dir { cloud_into(w.clone(), name); }
                else { cloud_open_file(w.clone(), remote, rel, name); }
            }
            "openwith" => cloud_open_mounted(w.clone(), remote, rel, false),
            "reveal"   => cloud_open_mounted(w.clone(), remote, rel, true),
            "download" => cloud_download(w.clone(), remote, rel, name),
            "copypath" => w0.set_cloud_status(tulipix_cloud::context::rclone_url(&remote, &rel).into()),
            "delete"   => cloud_delete_entry(w.clone(), remote, rel, e.is_dir),
            _ => {}
        }
    });
    let w = window.as_weak();
    window.on_cloud_browse_up(move || { cloud_up(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_search(move |_q| {
        // `cloud-query` is bound in the .slint; filter the cached listing in place.
        if let Some(w0) = w.upgrade() { cloud_set_filtered(&w0); }
    });

    // ── Tools: category tabs + op search (static catalog, no backend yet) ──
    let w = window.as_weak();
    window.on_tools_set_category(move |c| {
        if let Some(w0) = w.upgrade() { w0.set_tools_category(c.clone()); tools_refresh(&w0); }
    });
    let w = window.as_weak();
    window.on_tools_search(move |_q| {
        if let Some(w0) = w.upgrade() { tools_refresh(&w0); }
    });
    window.on_tools_open(move |t| {
        // Op execution is wired in a later pass.
        let _ = t;
    });
    tools_refresh(&window);

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
        let total = w.get_music_np_total();
        if total <= 0 { return; }
        // Shuffle → random next; else sequential wrap.
        let next = if w.get_music_shuffle() && total > 1 {
            let mut n = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos()).unwrap_or(0) as i32).rem_euclid(total);
            if n == w.get_music_np_index() { n = (n + 1).rem_euclid(total); }
            n
        } else {
            (w.get_music_np_index() + 1).rem_euclid(total)
        };
        play_music_at(&w, next);
    });
    let w = window.as_weak();
    window.on_music_prev(move || {
        let Some(w) = w.upgrade() else { return; };
        let total = w.get_music_np_total();
        if total <= 0 { return; }
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
    window.on_music_play_queue(move |i| { if let Some(w) = w.upgrade() { play_music_at(&w, i); } });
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
                    populate_podcasts(&w);
                    populate_podcast_latest(&w);
                    populate_podcast_downloads(&w);
                }
                "audiobooks" => populate_audiobooks(&w),
                // Counts feed the Home tiles; an empty cache (first ever open)
                // auto-triggers one full Refresh so the section self-populates.
                "radio" => radio_load_counts(w.as_weak(), true),
                "youtube" => {
                    w.set_music_yt_channel_open(false);
                    w.set_music_yt_playlist_open(false);
                    populate_yt_subs(&w);
                    populate_yt_cached(&w);
                    populate_yt_downloads(&w);
                    populate_yt_recent(&w);
                    populate_yt_recommended(&w);
                    populate_yt_playlists(&w);
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
        if s.as_str() == "music" {
            populate_podcasts(&w0);
            populate_podcast_latest(&w0);
            populate_podcast_downloads(&w0);
            populate_podcast_trends(&w0);
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
        w0.set_music_song_props_open(true);
        // Fill genre / release date / credits from the DB asynchronously.
        if item_id >= 0 {
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("music").await else { return; };
                let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
                let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
                let row: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                    "SELECT genre, release_date, credits FROM track_meta WHERE item_id = ?")
                    .bind(item_id).fetch_optional(&pool).await.ok().flatten();
                if let Some((g, rd, cr)) = row {
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_music_props_genre(g.unwrap_or_default().into());
                        w.set_music_props_release(rd.unwrap_or_default().into());
                        w.set_music_props_credits(cr.unwrap_or_default().into());
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
    // Context menu — Delete: remove the file from disk AND the library, then refresh.
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
        if w0.get_music_meta_fetch_status().starts_with('⟳') { return; } // already running
        w0.set_music_meta_fetch_status("⟳ 0%".into());
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
                    w.set_music_meta_fetch_status(format!("⟳ {}%", (frac * 100.0) as i32).into());
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
        if w0.get_music_lyrics_sync_status().starts_with('⟳') { return; } // already running
        w0.set_music_lyrics_sync_status("⟳ 0%".into());
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
            if total == 0 { let _ = weak.upgrade_in_event_loop(|w| w.set_music_lyrics_sync_status("✓ all synced".into())); return; }
            let client = reqwest::Client::new();
            let (mut done, mut found) = (0usize, 0usize);
            for (id, title, artist, album, dur) in rows {
                let title = title.unwrap_or_default();
                let artist = artist.unwrap_or_default();
                let album = album.unwrap_or_default();
                if !title.is_empty() {
                    let url = tulipix_music::lyrics::get_url(&artist, &title, &album, dur);
                    if let Ok(resp) = client.get(&url)
                        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                        .send().await {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            let synced = json["syncedLyrics"].as_str().unwrap_or("");
                            let plain = json["plainLyrics"].as_str().unwrap_or("");
                            if !synced.is_empty() { let _ = tulipix_music::lyrics::store(&pool, id, synced, true, "lrclib").await; found += 1; }
                            else if !plain.is_empty() { let _ = tulipix_music::lyrics::store(&pool, id, plain, false, "lrclib").await; found += 1; }
                        }
                    }
                }
                done += 1;
                if done % 4 == 0 || done == total {
                    let pct = done * 100 / total;
                    let frac = done as f32 / total as f32;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        w.set_music_lyrics_sync_status(format!("⟳ {pct}%").into());
                        w.set_music_lyrics_sync_progress(frac);
                    });
                }
                tokio::time::sleep(std::time::Duration::from_millis(150)).await; // be gentle on LRCLIB
            }
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_lyrics_sync_status(format!("✓ {found} found").into());
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
        if let Some(w) = w.upgrade() { w.set_music_shuffle(!w.get_music_shuffle()); }
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
    window.on_music_set_sleep(move |min| {
        use std::sync::atomic::Ordering;
        let Some(w0) = w.upgrade() else { return; };
        let min = min.max(0);
        w0.set_music_sleep_min(min);
        let tok = SLEEP_GEN.fetch_add(1, Ordering::SeqCst) + 1; // cancels any prior timer
        if min > 0 {
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(min as u64 * 60)).await;
                if SLEEP_GEN.load(Ordering::SeqCst) != tok { return; } // superseded
                let _ = weak.upgrade_in_event_loop(|w| {
                    if let Ok(mut g) = music_proc().lock() {
                        if let Some(mut c) = g.take() { let _ = c.kill(); let _ = c.wait(); }
                    }
                    w.set_music_playing(false);
                    w.set_music_sleep_min(0);
                });
            });
        }
    });
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
    // Sleep timer (np.p4.music.sleep-timer) — cycle Off→15→30→60→Off; a fresh
    // cycle bumps SLEEP_GEN so the previous timer no-ops when it fires.
    let w = window.as_weak();
    window.on_music_cycle_sleep(move || {
        use std::sync::atomic::Ordering;
        let Some(w0) = w.upgrade() else { return; };
        let next = match w0.get_music_sleep_min() { 0 => 10, 10 => 15, 15 => 30, 30 => 60, _ => 0 };
        w0.set_music_sleep_min(next);
        let tok = SLEEP_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        if next > 0 {
            let weak = w.clone();
            let handle = tokio::runtime::Handle::current();
            handle.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_secs(next as u64 * 60)).await;
                if SLEEP_GEN.load(Ordering::SeqCst) != tok { return; } // superseded
                let _ = weak.upgrade_in_event_loop(|w| {
                    if let Ok(mut g) = music_proc().lock() {
                        if let Some(mut c) = g.take() { let _ = c.kill(); let _ = c.wait(); }
                    }
                    w.set_music_playing(false);
                    w.set_music_sleep_min(0);
                    tracing::info!("music sleep timer fired");
                });
            });
        }
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
    let w = window.as_weak();
    window.on_music_set_crossfade(move |v| {
        let Some(w) = w.upgrade() else { return; };
        let v = (v as f64).clamp(0.0, 12.0);
        w.set_music_crossfade(v as f32);
        save_music_pref("music.crossfade", &format!("{v:.0}"));
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
                    user_locked = 1
                 WHERE item_id = ?")
                .bind(if title.is_empty() { None } else { Some(title.clone()) })
                .bind(artist_id).bind(album_id)
                .bind(if album_artist.trim().is_empty() { None } else { Some(album_artist.trim().to_string()) })
                .bind(if release_date.trim().is_empty() { None } else { Some(release_date.trim().to_string()) })
                .bind(if genre.trim().is_empty() { None } else { Some(genre.trim().to_string()) })
                .bind(if credits.trim().is_empty() { None } else { Some(credits.trim().to_string()) })
                .bind(year)
                .bind(id).execute(&pool).await;
            // Write the tags back into the file itself via ffmpeg (np.p5.music.tag-editor),
            // so the metadata survives a re-scan / shows in other players.
            let path: Option<String> = sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            if let Some(path) = path {
                let (title, artist, album) = (title.clone(), artist.clone(), album.clone());
                let _ = tokio::task::spawn_blocking(move || write_audio_tags(&path, &title, &artist, &album)).await;
            }
            // Refresh every surface: library lists + browse tiles, and the open
            // album/artist/genre detail overlay if one is showing.
            let _ = weak.upgrade_in_event_loop(|w| { populate_music_views(w.as_weak()); refresh_open_detail(&w); });
        });
    });

    // ── Phase 5 music — podcasts / audiobooks / artist bio ──────────────────
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
        match t.as_str() {
            "subscriptions" => populate_yt_subs(&w0),
            "cached"        => populate_yt_cached(&w0),
            "downloads"     => populate_yt_downloads(&w0),
            "playlists"     => populate_yt_playlists(&w0),
            "home" => { populate_yt_subs(&w0); populate_yt_cached(&w0); populate_yt_downloads(&w0); populate_yt_recent(&w0); populate_yt_recommended(&w0); }
            _ => {}
        }
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
                        yt_fetch_sub_meta(w.as_weak());
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
                        yt_fetch_sub_meta(w.as_weak());
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
        if q.is_empty() { return; }
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
                let videos = ytdlp_channel_latest(&cid, 5).await;
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
                w.set_music_yt_channel_open(true);
                w.set_music_yt_busy(false);
            });
        });
    });
    // Channel Refresh — re-fetch latest 5 + update the cache.
    let w = window.as_weak();
    window.on_music_yt_channel_refresh(move || {
        let Some(w0) = w.upgrade() else { return; };
        let cid = w0.get_music_yt_channel_id().to_string();
        if cid.is_empty() { return; }
        w0.set_music_yt_busy(true);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let client = reqwest::Client::new();
            let dir = yt_thumb_dir();
            let videos = ytdlp_channel_latest(&cid, 5).await;
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
        yt_play_audio(w.clone(), id);
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
    window.on_music_yt_remove_download(move |id| {
        let Some(_w0) = w.upgrade() else { return; };
        let id = id.to_string();
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("youtube").await {
                if let Some(p) = tulipix_music::youtube::store::remove_download(&pool, &id).await.ok().flatten() { let _ = std::fs::remove_file(&p); }
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
                let dls = tulipix_music::youtube::store::list_downloads(&pool, 100000).await.unwrap_or_default();
                for d in dls { if let Some(p) = tulipix_music::youtube::store::remove_download(&pool, &d.video_id).await.ok().flatten() { let _ = std::fs::remove_file(&p); } }
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
    // Play all — queue every video in the open playlist (audio) and auto-advance.
    let w = window.as_weak();
    window.on_music_yt_playlist_play_all(move || {
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let (id, source_url, total) = { let g = yt_pl_open().lock().unwrap(); (g.id, g.source_url.clone(), g.total) };
            let Ok(pool) = pool_for("youtube").await else { return; };
            let ids: Vec<String> = if let Some(url) = &source_url {
                // Remote: flat id list (cheap), fall back to whatever's cached.
                let vids = ytdlp_playlist_window(url, 1, total.max(1)).await;
                if !vids.is_empty() { vids.iter().map(|v| v.id.clone()).collect() }
                else { tulipix_music::youtube::store::get_playlist_cache(&pool, id).await.unwrap_or_default()
                    .iter().map(|c| c.video_id.clone()).collect() }
            } else {
                tulipix_music::youtube::store::playlist_items_page(&pool, id, 0, 100000).await.unwrap_or_default()
                    .iter().map(|it| it.video_id.clone()).collect()
            };
            if ids.is_empty() { return; }
            if let Ok(mut g) = yt_queue().lock() { *g = (ids.clone(), 0); }
            yt_play_audio(weak, ids[0].clone());
        });
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
        window.set_music_crossfade(s.advanced.get("music.crossfade").and_then(|v| v.parse().ok()).unwrap_or(0.0));
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

    // ── Settings panels: load persisted settings, seed the UI models ───────
    {
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        // Always launch in light theme regardless of the persisted choice
        // (user preference). The in-session theme picker still works; we just
        // never *start* dark.
        let choice = ThemeChoice::Light;
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
        let _ = &w; // panels not re-seeded on every keystroke
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
                    let msg = format!(
                        "{kind:?} import — {} photos · {} tracks · {} playlists · {} ratings{}{}",
                        plan.photos, plan.music_tracks, plan.playlists, plan.ratings,
                        if applied > 0 { format!(" · {applied} stars applied") } else { String::new() },
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
                let weak = w.as_weak();
                tokio::runtime::Handle::current().spawn(async move {
                    let msg = ai_update_summary();
                    let _ = weak.upgrade_in_event_loop(move |w| w.set_caps_nudge(msg.into()));
                });
            }
            "clear-thumb-cache" => { let _ = tulipix_core::thumbs::clear_cache(); }
            // Model download/verify flow (np.p1.onboarding.ai-models).
            k if k.starts_with("ai-dl-") => {
                let name = k.trim_start_matches("ai-dl-").to_string();
                let Some(entry) = ai_manifest().find(&name).cloned() else { return; };
                let weak = w.as_weak();
                let mb = entry.size_bytes / 1_000_000;
                w.set_caps_nudge(format!("Downloading {name} (~{mb} MB)…").into());
                tokio::runtime::Handle::current().spawn(async move {
                    let msg = match tulipix_photos::ai::models::download(&entry).await {
                        Ok(p) => format!("{name} ready · {}", p.display()),
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
    // Edit = clear any stored custom key (full key entry needs a secure text
    // dialog, tracked separately); clearing falls the service back to app-default.
    let w = window.as_weak();
    window.on_api_row_edit(move |i| {
        let Some(w) = w.upgrade() else { return; };
        let model = w.get_api_rows();
        let Some(mut row) = model.row_data(i as usize) else { return; };
        let service = row.service.to_string();
        let _ = tulipix_core::api_keys::delete(&service);
        row.user_key_set = false;
        row.use_app_default = true;
        model.set_row_data(i as usize, row);
        tracing::info!(%service, "custom api key cleared");
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
    cloud_unmount_all();

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

static PHOTOS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static VIDEOS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static MUSIC_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
static BOOKS_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
static CLOUD_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
// Music sub-sections split out of music.db (no items FK — self-contained).
static PODCASTS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static RADIO_POOL:    OnceLock<sqlx::SqlitePool> = OnceLock::new();
static YOUTUBE_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
// Map photo tile index → absolute path so the click handler can pop the
// viewer with the original (not the 320px thumb).
static PHOTO_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
fn photo_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    PHOTO_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Full (unfiltered) photo list for the grid: (label, original path, thumb path).
// Lets search rebuild the visible grid without re-scanning.
static PHOTO_FULL: std::sync::OnceLock<std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>>> = std::sync::OnceLock::new();
fn photo_full() -> &'static std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>> {
    PHOTO_FULL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Filter for the active photos category. `set` is membership; `order` (when
// non-empty) is the backend's display order so freshly-flagged items land at
// the top (e.g. star/trash sort newest-first). Recent leaves `order` empty so
// the grid keeps the natural scan order. `None` = no filter (show everything).
#[derive(Default, Clone)]
struct CatFilter {
    set: std::collections::HashSet<String>,
    order: Vec<String>,
}
static CATEGORY_PATHS: std::sync::OnceLock<std::sync::Mutex<Option<CatFilter>>> =
    std::sync::OnceLock::new();
fn category_paths() -> &'static std::sync::Mutex<Option<CatFilter>> {
    CATEGORY_PATHS.get_or_init(|| std::sync::Mutex::new(None))
}

/// Compute the `abs_path` filter for `category` by querying the photos DB.
/// Returns `None` only on a pool error for recent (so the grid never blanks);
/// otherwise `Some(CatFilter)`. Non-recent categories carry `order` (backend
/// recency) so newly-flagged photos appear at the top of the grid.
async fn category_path_set(category: &str) -> Option<CatFilter> {
    let pool = match pool_for("photos").await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(category, error = %e, "category: photos pool open failed");
            // Recent falls back to "show everything" so a DB hiccup never blanks
            // the main grid; the flag tabs fall back to empty.
            return if category == "recent" { None } else { Some(CatFilter::default()) };
        }
    };
    let paths: Vec<String> = match category {
        // Recent = the default library view: everything except trashed/archived.
        "recent" => sqlx::query_scalar(
            "SELECT items.abs_path FROM items \
             LEFT JOIN photo_meta ON photo_meta.item_id = items.id \
             WHERE items.missing_since IS NULL \
               AND photo_meta.deleted_at IS NULL \
               AND (photo_meta.archived IS NULL OR photo_meta.archived = 0)",
        )
        .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "recent query"); Vec::new() }),
        // Photos pinned into any manual album.
        "albums" => sqlx::query_scalar(
            "SELECT DISTINCT items.abs_path FROM album_items \
             JOIN items ON items.id = album_items.item_id",
        )
        .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "albums query"); Vec::new() }),
        // Photos taken on this month/day in any past year.
        "memories" => tulipix_photos::memories::on_this_day(&pool, now_secs())
            .await
            .map(|hits| hits.into_iter().map(|h| h.abs_path).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "memories query"); Vec::new() }),
        // Geotagged photos (EXIF GPS present).
        "places" => sqlx::query_scalar(
            "SELECT items.abs_path FROM items \
             JOIN photo_meta ON photo_meta.item_id = items.id \
             WHERE photo_meta.gps_lat IS NOT NULL AND photo_meta.gps_lon IS NOT NULL",
        )
        .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "places query"); Vec::new() }),
        // Starred / Archive / Trash — backed by the photos flag modules.
        "starred" => tulipix_photos::star::list(&pool, 100_000).await
            .map(|v| v.into_iter().map(|(_, p)| p).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "starred list"); Vec::new() }),
        "archive" => tulipix_photos::archive::list(&pool, 0, 100_000).await
            .map(|v| v.into_iter().map(|(_, p)| p).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "archive list"); Vec::new() }),
        "trash" => tulipix_photos::trash::list(&pool).await
            .map(|v| v.into_iter().map(|e| e.abs_path).collect())
            .unwrap_or_else(|e| { tracing::warn!(error=%e, "trash list"); Vec::new() }),
        _ => Vec::new(),
    };
    // Recent keeps natural scan order (empty `order`); every other category
    // displays in the backend's order so flagged items surface at the top.
    let set = paths.iter().cloned().collect();
    let order = if category == "recent" { Vec::new() } else { paths };
    Some(CatFilter { set, order })
}

/// Resolve a photo's `items.id` from its absolute path.
async fn item_id_for(pool: &sqlx::SqlitePool, path: &std::path::Path) -> Option<i64> {
    let abs = path.to_string_lossy().into_owned();
    sqlx::query_scalar::<_, i64>("SELECT id FROM items WHERE abs_path = ?")
        .bind(abs).fetch_optional(pool).await.ok().flatten()
}

/// Phase 6 helpers — load auxiliary badge caches from the photos DB.
async fn reload_phase6_caches(pool: &sqlx::SqlitePool) {
    // Color labels
    let labels: Vec<(String, String)> = sqlx::query_as(
        "SELECT items.abs_path, COALESCE(pm.color_label, '') FROM items \
         JOIN photo_meta pm ON pm.item_id = items.id \
         WHERE pm.color_label IS NOT NULL AND pm.color_label != ''"
    ).fetch_all(pool).await.unwrap_or_default();
    if let Ok(mut g) = color_labels().lock() {
        g.clear();
        for (path, label) in labels { g.insert(path, label); }
    }
    // path → item_id
    let id_map: Vec<(String, i64)> = sqlx::query_as(
        "SELECT abs_path, id FROM items WHERE section = 'photos' AND missing_since IS NULL"
    ).fetch_all(pool).await.unwrap_or_default();
    if let Ok(mut g) = photo_item_ids().lock() {
        g.clear();
        for (path, id) in id_map { g.insert(path, id); }
    }
    // Live photo IDs
    let live = tulipix_photos::live_photos::detect_in_library(pool).await.unwrap_or_default();
    if let Ok(mut g) = live_ids().lock() {
        g.clear();
        for lp in live { g.insert(lp.still_item_id); }
    }
    // Stack info
    match tulipix_photos::stacks::list(pool).await {
        Ok(stacks) => {
            let hidden = tulipix_photos::stacks::hidden_ids(pool).await.unwrap_or_default();
            // Build cover_path → size and hidden_paths sets
            let id_map_snap = photo_item_ids().lock().map(|g| g.clone()).unwrap_or_default();
            let id_to_path: std::collections::HashMap<i64, String> =
                id_map_snap.iter().map(|(p, id)| (*id, p.clone())).collect();
            let mut cover_map: std::collections::HashMap<String, i32> = Default::default();
            let mut hidden_paths: std::collections::HashSet<String> = Default::default();
            for stack in &stacks {
                if let Some(cover_path) = id_to_path.get(&stack.cover_id) {
                    cover_map.insert(cover_path.clone(), stack.member_ids.len() as i32);
                }
            }
            for hid in hidden {
                if let Some(p) = id_to_path.get(&hid) { hidden_paths.insert(p.clone()); }
            }
            if let Ok(mut g) = stack_info().lock() { *g = (cover_map, hidden_paths); }
        }
        Err(e) => tracing::warn!(error=%e, "stacks::list failed"),
    }
}

/// Phase 6 — memory rail loading.
async fn load_memory_rails(pool: &sqlx::SqlitePool) -> Vec<(i32, String, Vec<String>)> {
    let hits = tulipix_photos::memories::on_this_day(pool, now_secs()).await.unwrap_or_default();
    let mut map: std::collections::BTreeMap<i32, Vec<String>> = std::collections::BTreeMap::new();
    for h in hits { map.entry(h.years_ago).or_default().push(h.abs_path); }
    map.into_iter().map(|(y, paths)| {
        let label = if y == 1 { "1 year ago".to_string() } else { format!("{y} years ago") };
        (y, label, paths)
    }).collect()
}

/// Phase 6 — map cluster loading. Returns (lat, lon, count, thumb_path, label) tuples (no slint::Image).
async fn load_map_clusters(pool: &sqlx::SqlitePool) -> Vec<(f32, f32, i32, PathBuf, String)> {
    let clusters = tulipix_photos::map::cluster_pins(pool, 1.0).await.unwrap_or_default();
    let mut out = Vec::with_capacity(clusters.len());
    for c in clusters {
        let cover_path: Option<String> = sqlx::query_scalar(
            "SELECT abs_path FROM items WHERE id = ?"
        ).bind(c.cover_item_id).fetch_optional(pool).await.ok().flatten();
        let thumb: PathBuf = cover_path.as_deref()
            .and_then(|p| {
                let full = photo_full().lock().ok()?;
                full.iter().find(|(_, orig, _)| orig.to_string_lossy() == p).map(|(_, _, th)| th.clone())
            })
            .unwrap_or_default();
        let label = format!("{:.1}°N {:.1}°E", c.lat, c.lon);
        out.push((c.lat as f32, c.lon as f32, c.count as i32, thumb, label));
    }
    out
}

/// Phase 6 — dedupe group loading. Returns tuples (no slint::Image) for thread safety.
/// Returns: (cluster_id, kind, left_thumb_path, right_thumb_path, left_label, right_label)
async fn load_dedupe_groups(pool: &sqlx::SqlitePool) -> Vec<(i32, String, PathBuf, PathBuf, String, String)> {
    let rows: Vec<(i64, String, i64)> = sqlx::query_as(
        "SELECT dm.cluster_id, dc.kind, dm.item_id \
         FROM dedup_members dm JOIN dedup_clusters dc ON dc.id = dm.cluster_id \
         WHERE (SELECT COUNT(*) FROM dedup_members WHERE cluster_id = dm.cluster_id) >= 2 \
         ORDER BY dm.cluster_id, dm.item_id LIMIT 2000"
    ).fetch_all(pool).await.unwrap_or_default();
    let mut cluster_map: std::collections::BTreeMap<i64, (String, Vec<i64>)> = Default::default();
    for (cid, kind, item_id) in rows {
        let e = cluster_map.entry(cid).or_insert_with(|| (kind, Vec::new()));
        if e.1.len() < 2 { e.1.push(item_id); }
    }
    let by_orig: std::collections::HashMap<i64, PathBuf> = {
        let id_map = photo_item_ids().lock().ok().map(|g| g.clone()).unwrap_or_default();
        let full = photo_full().lock().unwrap();
        let path_to_thumb: std::collections::HashMap<String, PathBuf> =
            full.iter().map(|(_, orig, th)| (orig.to_string_lossy().into_owned(), th.clone())).collect();
        drop(full);
        id_map.into_iter().filter_map(|(path, id)| {
            path_to_thumb.get(&path).map(|th| (id, th.clone()))
        }).collect()
    };
    let abs_paths: std::collections::HashMap<i64, String> =
        photo_item_ids().lock().ok().map(|g| g.iter().map(|(p, &id)| (id, p.clone())).collect()).unwrap_or_default();
    let fname = |p: &str| std::path::Path::new(p).file_name()
        .and_then(|s| s.to_str()).unwrap_or(p).to_string();
    let mut out = Vec::new();
    for (cluster_id, (kind, members)) in cluster_map {
        if members.len() < 2 { continue; }
        let left_id = members[0]; let right_id = members[1];
        let left_path = abs_paths.get(&left_id).cloned().unwrap_or_default();
        let right_path = abs_paths.get(&right_id).cloned().unwrap_or_default();
        let left_thumb = by_orig.get(&left_id).cloned().unwrap_or_default();
        let right_thumb = by_orig.get(&right_id).cloned().unwrap_or_default();
        out.push((cluster_id as i32, kind, left_thumb, right_thumb, fname(&left_path), fname(&right_path)));
    }
    out
}

/// Build `Vec<DedupeGroup>` from path tuples on the UI thread (where slint::Image is safe).
fn dedupe_groups_from_paths(data: Vec<(i32, String, PathBuf, PathBuf, String, String)>) -> Vec<DedupeGroup> {
    data.into_iter().map(|(cluster_id, kind, lp, rp, ll, rl)| DedupeGroup {
        cluster_id,
        kind: kind.into(),
        left_thumb: slint::Image::load_from_path(&lp).unwrap_or_default(),
        right_thumb: slint::Image::load_from_path(&rp).unwrap_or_default(),
        left_label: ll.into(),
        right_label: rl.into(),
    }).collect()
}

/// Build `Vec<MapCluster>` from path tuples on the UI thread.
fn map_clusters_from_paths(data: Vec<(f32, f32, i32, PathBuf, String)>) -> Vec<MapCluster> {
    data.into_iter().map(|(lat, lon, count, th, label)| MapCluster {
        lat, lon, count,
        cover: slint::Image::load_from_path(&th).unwrap_or_default(),
        label: label.into(),
    }).collect()
}

/// Recompute the allowed-path set for `category` off-thread, then rebuild the
/// photo grid (honouring `query`) on the UI thread. Shared by the category-tab
/// switch, right-click flag actions, and the post-scan refresh.
fn kick_category_refresh(weak: slint::Weak<MainWindow>, category: String, query: String) {
    // A rebuild re-orders photo_paths, so any selection indices are now stale.
    selection_clear();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Refresh starred membership so every grid colours its star correctly.
        // Also reload Phase 6 caches (labels, live IDs, stacks) on each grid refresh.
        if let Ok(pool) = pool_for("photos").await {
            if let Ok(list) = tulipix_photos::star::list(&pool, 1_000_000).await {
                if let Ok(mut g) = starred_paths().lock() {
                    *g = list.into_iter().map(|(_, p)| p).collect();
                }
            }
            reload_phase6_caches(&pool).await;
        }
        match category.as_str() {
            // Timeline = date-grouped grid, newest→oldest (the DB supplies the
            // order + month label; thumbs come from photo_full on the UI thread).
            "timeline" => {
                let order = timeline_order(&query).await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_timeline(&w, order, &query));
            }
            // A single library folder opened as a grouped grid, sorted per the
            // active sort mode.
            "folder" => {
                let folder = selected_folder().lock().map(|g| g.clone()).unwrap_or_default();
                let sort = sort_mode().lock().map(|g| g.clone()).unwrap_or_else(|_| "date".into());
                let dir = sort_dir().lock().map(|g| g.clone()).unwrap_or_else(|_| "desc".into());
                let order = folder_order(&folder, &sort, &dir).await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_timeline(&w, order, &query));
            }
            // Library = folder list, derived from the loaded photo grid.
            "library" => {
                let _ = weak.upgrade_in_event_loop(move |w| populate_library(&w, &query));
            }
            // People tab = face-cluster avatar cards (np.p2.ai.face-clusters).
            "people" => {
                let cards = load_people_cards().await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_people(&w, cards));
            }
            // Things tab = object-tag list (np.p2.ai.tags).
            "things" => {
                let things = load_things().await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_things(&w, things));
            }
            // Albums tab = album cover cards (np.p2.albums).
            "albums" => {
                let cards = load_albums().await;
                let _ = weak.upgrade_in_event_loop(move |w| populate_albums(&w, cards));
            }
            // Person / tag / album drill-in — the open handler already stored the
            // path set in category_paths; just rebuild the flat grid from it.
            "facephotos" | "tagphotos" | "albumphotos" => {
                let _ = weak.upgrade_in_event_loop(move |w| apply_photo_filter(&w, &query));
            }
            // Phase 6 — Memories: year-rail view.
            "memories" => {
                if let Ok(pool) = pool_for("photos").await {
                    let rails_raw = load_memory_rails(&pool).await;
                    let weak2 = weak.clone();
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        use slint::{ModelRc, VecModel};
                        let rails: Vec<MemoryRail> = rails_raw.into_iter().map(|(year, ago, paths)| {
                            let starred = starred_snapshot();
                            let tiles: Vec<PhotoTile> = {
                                let full = photo_full().lock().unwrap();
                                let by_path: std::collections::HashMap<String, (String, std::path::PathBuf)> =
                                    full.iter().map(|e| (e.1.to_string_lossy().into_owned(), (e.0.clone(), e.2.clone()))).collect();
                                drop(full);
                                paths.iter().enumerate().filter_map(|(i, p)| {
                                    by_path.get(p).map(|(label, thumb)| {
                                        let color_label = color_labels().lock().ok()
                                            .and_then(|g| g.get(p).cloned())
                                            .unwrap_or_default();
                                        let item_id = photo_item_ids().lock().ok()
                                            .and_then(|g| g.get(p).copied());
                                        let is_live = item_id.map(|id| live_ids().lock().ok()
                                            .map(|g| g.contains(&id)).unwrap_or(false)).unwrap_or(false);
                                        let (stack_count, _hidden) = {
                                            
                                            stack_info().lock().ok()
                                                .map(|g| (g.0.get(p).copied().unwrap_or(0), g.1.contains(p)))
                                                .unwrap_or_default()
                                        };
                                        PhotoTile {
                                            thumb: slint::Image::load_from_path(thumb).unwrap_or_default(),
                                            label: label.clone().into(), col: i as i32, row: 0, index: i as i32,
                                            starred: starred.contains(p), selected: false,
                                            color_label: color_label.into(), is_live, stack_count, count: 0,
                                        }
                                    })
                                }).collect()
                            };
                            MemoryRail {
                                year,
                                ago: ago.into(),
                                tiles: ModelRc::new(VecModel::from(tiles)),
                            }
                        }).collect();
                        w.set_photo_memory_rails(ModelRc::new(VecModel::from(rails)));
                        let _ = weak2;
                    });
                }
            }
            // Phase 6 — Places: cluster grid.
            "places" => {
                if let Ok(pool) = pool_for("photos").await {
                    let cluster_data = load_map_clusters(&pool).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        use slint::{ModelRc, VecModel};
                        let clusters = map_clusters_from_paths(cluster_data);
                        w.set_photo_map_clusters(ModelRc::new(VecModel::from(clusters)));
                    });
                }
            }
            // Phase 6 — Dedupe: load pairs if any exist.
            "dedupe" => {
                if let Ok(pool) = pool_for("photos").await {
                    let group_data = load_dedupe_groups(&pool).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        use slint::{ModelRc, VecModel};
                        let groups = dedupe_groups_from_paths(group_data);
                        w.set_photo_dedupe_groups(ModelRc::new(VecModel::from(groups)));
                    });
                }
            }
            // Starred / Archive / Trash = flat filtered grid, sorted per the
            // active sort mode/direction (shared with the folder view).
            _ => {
                let mut set = category_path_set(&category).await;
                if let Some(cf) = set.as_mut() {
                    let sort = sort_mode().lock().map(|g| g.clone()).unwrap_or_else(|_| "date".into());
                    let dir = sort_dir().lock().map(|g| g.clone()).unwrap_or_else(|_| "desc".into());
                    sort_paths(&mut cf.order, &sort, &dir).await;
                }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    if let Ok(mut g) = category_paths().lock() { *g = set; }
                    apply_photo_filter(&w, &query);
                });
            }
        }
    });
}

// Library folder currently opened in the "folder" view + the active sort.
static SELECTED_FOLDER: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn selected_folder() -> &'static std::sync::Mutex<String> {
    SELECTED_FOLDER.get_or_init(|| std::sync::Mutex::new(String::new()))
}
static SORT_MODE: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn sort_mode() -> &'static std::sync::Mutex<String> {
    SORT_MODE.get_or_init(|| std::sync::Mutex::new("date".into()))
}
static SORT_DIR: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn sort_dir() -> &'static std::sync::Mutex<String> {
    SORT_DIR.get_or_init(|| std::sync::Mutex::new("desc".into()))
}
// Library folder-list sort: mode ("name" | "count") + direction.
static LIB_SORT: std::sync::OnceLock<std::sync::Mutex<(String, String)>> = std::sync::OnceLock::new();
fn lib_sort() -> &'static std::sync::Mutex<(String, String)> {
    LIB_SORT.get_or_init(|| std::sync::Mutex::new(("name".into(), "asc".into())))
}
// Starred abs_paths — refreshed before each grid rebuild so tiles can show a
// filled (yellow) vs hollow (white) star.
static STARRED_PATHS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::OnceLock::new();
fn starred_paths() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    STARRED_PATHS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}
fn starred_snapshot() -> std::collections::HashSet<String> {
    starred_paths().lock().map(|g| g.clone()).unwrap_or_default()
}

// Phase 6 — Color labels cache: abs_path → label string ("red", "orange", …, "").
static COLOR_LABELS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::OnceLock::new();
fn color_labels() -> &'static std::sync::Mutex<std::collections::HashMap<String, String>> {
    COLOR_LABELS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

// Phase 6 — Live photo item IDs.
static LIVE_IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<i64>>> =
    std::sync::OnceLock::new();
fn live_ids() -> &'static std::sync::Mutex<std::collections::HashSet<i64>> {
    LIVE_IDS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

// Phase 6 — Stack info: (cover_path → stack_size, hidden_paths).
static STACK_INFO: std::sync::OnceLock<std::sync::Mutex<(std::collections::HashMap<String, i32>, std::collections::HashSet<String>)>> =
    std::sync::OnceLock::new();
fn stack_info() -> &'static std::sync::Mutex<(std::collections::HashMap<String, i32>, std::collections::HashSet<String>)> {
    STACK_INFO.get_or_init(|| std::sync::Mutex::new((Default::default(), Default::default())))
}

// Phase 6 — path → item_id map for badge lookups.
static PHOTO_ITEM_IDS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, i64>>> =
    std::sync::OnceLock::new();
fn photo_item_ids() -> &'static std::sync::Mutex<std::collections::HashMap<String, i64>> {
    PHOTO_ITEM_IDS.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

// ── Multi-select (np.p2.multiselect) ───────────────────────────────────────
// Selected photo indices into the active `photo_paths` order, plus the range
// anchor (last toggled tile). Indices are cleared on every grid rebuild since a
// rebuild re-orders `photo_paths`.
#[derive(Default)]
struct SelState { sel: std::collections::HashSet<i32>, anchor: i32 }
static SELECTION: std::sync::OnceLock<std::sync::Mutex<SelState>> = std::sync::OnceLock::new();
fn selection() -> &'static std::sync::Mutex<SelState> {
    SELECTION.get_or_init(|| std::sync::Mutex::new(SelState::default()))
}
fn selection_clear() {
    if let Ok(mut g) = selection().lock() { g.sel.clear(); g.anchor = 0; }
}
fn selection_snapshot() -> std::collections::HashSet<i32> {
    selection().lock().map(|g| g.sel.clone()).unwrap_or_default()
}

// Clipboard for Copy/Cut → Paste. `(source paths, is_cut)`. Survives grid
// rebuilds so you can copy in one folder and paste in another.
static CLIPBOARD: std::sync::OnceLock<std::sync::Mutex<(Vec<PathBuf>, bool)>> = std::sync::OnceLock::new();
fn clipboard() -> &'static std::sync::Mutex<(Vec<PathBuf>, bool)> {
    CLIPBOARD.get_or_init(|| std::sync::Mutex::new((Vec::new(), false)))
}
fn clipboard_has() -> bool {
    clipboard().lock().map(|g| !g.0.is_empty()).unwrap_or(false)
}

thread_local! {
    // Single-shot timer that clears the tab "blink" confirmation. UI-thread only.
    static BLINK_TIMER: std::cell::RefCell<slint::Timer> = std::cell::RefCell::new(slint::Timer::default());
    // VecModels backing the currently-shown photo grids, so selection toggles
    // can repaint the `selected` flag in place (no image reload). UI-thread only.
    static GRID_MODELS: std::cell::RefCell<Vec<std::rc::Rc<slint::VecModel<PhotoTile>>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Reset the registered grid models (called at the start of every grid build).
fn grid_models_reset() { GRID_MODELS.with(|m| m.borrow_mut().clear()); }
/// Register a freshly-built grid VecModel for in-place selection repaint.
fn grid_models_push(m: std::rc::Rc<slint::VecModel<PhotoTile>>) {
    GRID_MODELS.with(|g| g.borrow_mut().push(m));
}

/// Repaint the `selected` flag on every visible tile from the selection set.
fn repaint_selection() {
    let sel = selection_snapshot();
    GRID_MODELS.with(|models| {
        for model in models.borrow().iter() {
            for r in 0..model.row_count() {
                if let Some(mut t) = model.row_data(r) {
                    let want = sel.contains(&t.index);
                    if t.selected != want { t.selected = want; model.set_row_data(r, t); }
                }
            }
        }
    });
}

/// Push selection count / mode / paste-availability to the UI, then repaint.
fn refresh_selection_meta(w: &MainWindow) {
    let count = selection().lock().map(|g| g.sel.len()).unwrap_or(0) as i32;
    let can_paste = clipboard_has() && w.get_photos_category() == "folder";
    w.set_photos_select_count(count);
    w.set_photos_selecting(count > 0);
    w.set_photos_can_paste(can_paste);
    repaint_selection();
}

/// Reorder `order` (abs_paths) by `sort`/`dir`. Default (date desc) is a no-op
/// since the lists already arrive newest-first.
async fn sort_paths(order: &mut [String], sort: &str, dir: &str) {
    if sort == "date" && dir == "desc" { return; }
    let map: std::collections::HashMap<String, (i64, i64)> =
        photos_meta().await.into_iter().map(|(p, dt, s)| (p, (dt, s))).collect();
    let fname = |p: &str| std::path::Path::new(p).file_name()
        .and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    order.sort_by(|a, b| match sort {
        "name" => fname(a).cmp(&fname(b)),
        "size" => map.get(a).map(|x| x.1).unwrap_or(0).cmp(&map.get(b).map(|x| x.1).unwrap_or(0)),
        _ => map.get(a).map(|x| x.0).unwrap_or(0).cmp(&map.get(b).map(|x| x.0).unwrap_or(0)),
    });
    if dir == "desc" { order.reverse(); }
}

/// (abs_path, date, size) for every live photo (trashed/archived excluded).
/// Date is taken_at, falling back to file mtime.
async fn photos_meta() -> Vec<(String, i64, i64)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    let rows: Vec<(String, Option<i64>, i64, i64)> = sqlx::query_as(
        "SELECT items.abs_path, photo_meta.taken_at, items.mtime, items.size \
         FROM items LEFT JOIN photo_meta ON photo_meta.item_id = items.id \
         WHERE items.section = 'photos' AND items.missing_since IS NULL \
           AND photo_meta.deleted_at IS NULL \
           AND (photo_meta.archived IS NULL OR photo_meta.archived = 0)",
    )
    .fetch_all(&pool).await.unwrap_or_else(|e| { tracing::warn!(error=%e, "photos_meta query"); Vec::new() });
    rows.into_iter().map(|(p, taken, mtime, size)| (p, taken.unwrap_or(mtime), size)).collect()
}

/// Timeline order — newest first, grouped by month.
async fn timeline_order(_query: &str) -> Vec<(String, String)> {
    let mut m = photos_meta().await;
    m.sort_by(|a, b| b.1.cmp(&a.1));
    m.into_iter().map(|(p, dt, _)| (p, month_label(dt))).collect()
}

/// Folder-view order — photos directly inside `folder`, ordered + grouped per
/// `sort` (date → month groups; name/size → a single group) and `dir`.
async fn folder_order(folder: &str, sort: &str, dir: &str) -> Vec<(String, String)> {
    let mut m: Vec<(String, i64, i64)> = photos_meta().await.into_iter()
        .filter(|(p, _, _)| {
            std::path::Path::new(p).parent()
                .map(|d| d.to_string_lossy() == folder).unwrap_or(false)
        })
        .collect();
    let fname = |p: &str| std::path::Path::new(p).file_name()
        .and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let asc = dir == "asc";
    let arrow = if asc { "▲" } else { "▼" };
    match sort {
        "name" => {
            m.sort_by_key(|a| fname(&a.0));
            if !asc { m.reverse(); }
            let label = format!("By name {arrow}");
            m.into_iter().map(|(p, _, _)| (p, label.clone())).collect()
        }
        "size" => {
            m.sort_by_key(|a| a.2);
            if !asc { m.reverse(); }
            let label = format!("By size {arrow}");
            m.into_iter().map(|(p, _, _)| (p, label.clone())).collect()
        }
        _ => {
            m.sort_by_key(|a| a.1);
            if !asc { m.reverse(); }
            m.into_iter().map(|(p, dt, _)| (p, month_label(dt))).collect()
        }
    }
}

/// "May 2026" for a unix timestamp (GMT).
fn month_label(unix: i64) -> String {
    use chrono::{DateTime, Utc};
    DateTime::<Utc>::from_timestamp(unix, 0)
        .map(|d| d.format("%B %Y").to_string())
        .unwrap_or_else(|| "Undated".into())
}

/// Build the timeline date groups from `order` + the cached thumbs, set them
/// on the window, and keep photo_paths in flattened group order for the viewer.
fn populate_timeline(w: &MainWindow, order: Vec<(String, String)>, query: &str) {
    use slint::{ModelRc, VecModel};
    let q = query.trim().to_lowercase();
    let cols = 6i32;
    let full = photo_full().lock().unwrap();
    let by_path: std::collections::HashMap<String, &(String, PathBuf, PathBuf)> =
        full.iter().map(|e| (e.1.to_string_lossy().into_owned(), e)).collect();
    let starred = starred_snapshot();

    grid_models_reset();
    let mut groups: Vec<PhotoGroup> = Vec::new();
    let mut cur_label = String::new();
    let mut cur: Vec<PhotoTile> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    let flush = |label: &str, tiles: &mut Vec<PhotoTile>, groups: &mut Vec<PhotoGroup>| {
        if !tiles.is_empty() {
            // Hold the VecModel so selection can repaint the group's tiles.
            let model = std::rc::Rc::new(VecModel::from(std::mem::take(tiles)));
            grid_models_push(model.clone());
            groups.push(PhotoGroup { label: label.into(), tiles: ModelRc::from(model) });
        }
    };
    for (path, label) in &order {
        let Some((fname, orig, thumb)) = by_path.get(path).map(|e| (&e.0, &e.1, &e.2)) else { continue; };
        if !q.is_empty() && !fname.to_lowercase().contains(&q) { continue; }
        if *label != cur_label && !cur.is_empty() {
            flush(&cur_label, &mut cur, &mut groups);
        }
        cur_label = label.clone();
        let orig_str = orig.to_string_lossy().into_owned();
        let color_label = color_labels().lock().ok()
            .and_then(|g| g.get(&orig_str).cloned()).unwrap_or_default();
        let item_id_opt = photo_item_ids().lock().ok().and_then(|g| g.get(&orig_str).copied());
        let is_live = item_id_opt.map(|id| live_ids().lock().ok()
            .map(|g| g.contains(&id)).unwrap_or(false)).unwrap_or(false);
        let (stack_count, is_hidden) = stack_info().lock().ok()
            .map(|g| (g.0.get(&orig_str).copied().unwrap_or(0), g.1.contains(&orig_str)))
            .unwrap_or_default();
        if is_hidden { continue; }
        let gi = paths.len() as i32;
        cur.push(PhotoTile {
            thumb: slint::Image::load_from_path(thumb).unwrap_or_default(),
            label: fname.clone().into(),
            col: gi % cols, row: gi / cols, index: gi,
            starred: starred.contains(path),
            selected: false,
            color_label: color_label.into(),
            is_live,
            stack_count,
            count: 0,
        });
        paths.push(orig.clone());
    }
    flush(&cur_label, &mut cur, &mut groups);
    drop(full);
    *photo_paths().lock().unwrap() = paths;
    w.set_photo_groups(ModelRc::new(VecModel::from(groups)));
    refresh_selection_meta(w);
}

/// Build the Library folder list (one row per parent dir of the photo grid),
/// optionally filtered by `query` against the folder name.
fn populate_library(w: &MainWindow, query: &str) {
    use slint::{ModelRc, VecModel};
    let q = query.trim().to_lowercase();
    let full = photo_full().lock().unwrap();
    // dir → (count, first thumb path)
    let mut order: Vec<String> = Vec::new();
    let mut map: std::collections::HashMap<String, (i32, PathBuf)> = std::collections::HashMap::new();
    for (_, orig, thumb) in full.iter() {
        let Some(dir) = orig.parent() else { continue; };
        let key = dir.to_string_lossy().into_owned();
        let e = map.entry(key.clone()).or_insert_with(|| { order.push(key.clone()); (0, thumb.clone()) });
        e.0 += 1;
    }
    drop(full);
    let mut rows: Vec<FolderRow> = order.into_iter().filter_map(|path| {
        let (count, thumb) = map.remove(&path).unwrap();
        let name = std::path::Path::new(&path).file_name()
            .and_then(|s| s.to_str()).unwrap_or(&path).to_string();
        if !q.is_empty() && !name.to_lowercase().contains(&q) { return None; }
        Some(FolderRow {
            name: name.into(),
            path: path.into(),
            count,
            cover: slint::Image::load_from_path(&thumb).unwrap_or_default(),
        })
    }).collect();
    // Sort by name or photo count, per the library sort selector.
    let (mode, dir) = lib_sort().lock().map(|g| g.clone()).unwrap_or_else(|_| ("name".into(), "asc".into()));
    if mode == "count" {
        rows.sort_by_key(|a| a.count);
    } else {
        rows.sort_by_key(|a| a.name.to_lowercase());
    }
    if dir == "desc" { rows.reverse(); }
    w.set_photo_folders(ModelRc::new(VecModel::from(rows)));
}

/// Rebuild the photo grid from the retained full list, filtered by `query`
/// (case-insensitive filename match). Keeps `photo_paths` in sync for the viewer.
fn apply_photo_filter(w: &MainWindow, query: &str) {
    use slint::{ModelRc, VecModel};
    let q = query.trim().to_lowercase();
    let cols = 6i32;
    // Active category filter: None = recent fallback (all photos).
    let cat = category_paths().lock().ok().and_then(|g| g.clone());
    let full = photo_full().lock().unwrap();

    // Choose iteration order: the category's `order` (backend recency) when it
    // has one, else the natural scan order from photo_full.
    let ordered: Vec<&(String, PathBuf, PathBuf)> = match &cat {
        Some(c) if !c.order.is_empty() => {
            let by_path: std::collections::HashMap<String, &(String, PathBuf, PathBuf)> =
                full.iter().map(|e| (e.1.to_string_lossy().into_owned(), e)).collect();
            c.order.iter().filter_map(|p| by_path.get(p.as_str()).copied()).collect()
        }
        _ => full.iter().collect(),
    };
    let starred = starred_snapshot();

    let mut tiles: Vec<PhotoTile> = Vec::new();
    let mut paths: Vec<PathBuf> = Vec::new();
    for (label, orig, thumb) in ordered {
        if !q.is_empty() && !label.to_lowercase().contains(&q) { continue; }
        let orig_str = orig.to_string_lossy().into_owned();
        if let Some(c) = &cat {
            if !c.set.contains(&orig_str) { continue; }
        }
        let color_label = color_labels().lock().ok()
            .and_then(|g| g.get(&orig_str).cloned()).unwrap_or_default();
        let item_id_opt = photo_item_ids().lock().ok().and_then(|g| g.get(&orig_str).copied());
        let is_live = item_id_opt.map(|id| live_ids().lock().ok()
            .map(|g| g.contains(&id)).unwrap_or(false)).unwrap_or(false);
        let (stack_count, is_hidden) = stack_info().lock().ok()
            .map(|g| (g.0.get(&orig_str).copied().unwrap_or(0), g.1.contains(&orig_str)))
            .unwrap_or_default();
        if is_hidden { continue; }
        let i = tiles.len() as i32;
        tiles.push(PhotoTile {
            thumb: slint::Image::load_from_path(thumb).unwrap_or_default(),
            label: label.clone().into(),
            col: i % cols, row: i / cols, index: i,
            starred: starred.contains(&orig_str),
            selected: false,
            color_label: color_label.into(),
            is_live,
            stack_count,
            count: 0,
        });
        paths.push(orig.clone());
    }
    drop(full);
    *photo_paths().lock().unwrap() = paths;
    grid_models_reset();
    let model = std::rc::Rc::new(VecModel::from(tiles));
    grid_models_push(model.clone());
    w.set_photo_tiles(ModelRc::from(model));
    refresh_selection_meta(w);
}

// ── People / Things tabs (np.p2.ai.face-clusters / .name / .tags) ──────────

/// Load face-cluster cards: (person_id, name, face_count, cover_thumb_abs_path).
/// Cover = the person's `cover_face` crop, falling back to any face crop.
async fn load_people_cards() -> Vec<(i64, Option<String>, i64, Option<String>)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    let people = match tulipix_photos::ai::people::list(&pool).await {
        Ok(p) => p,
        Err(e) => { tracing::warn!(error=%e, "people::list"); return Vec::new(); }
    };
    let dir = tulipix_photos::ai::faces::face_thumbs_dir();
    let mut out = Vec::with_capacity(people.len());
    for p in people {
        // Resolve a cover crop path (relative to face_thumbs_dir).
        let rel: Option<String> = if let Some(fid) = p.cover_face_id {
            sqlx::query_scalar::<_, String>("SELECT crop_path FROM faces WHERE id = ? AND crop_path <> ''")
                .bind(fid).fetch_optional(&pool).await.ok().flatten()
        } else { None };
        let rel = match rel {
            Some(r) => Some(r),
            None => sqlx::query_scalar::<_, String>(
                "SELECT crop_path FROM faces WHERE person_id = ? AND crop_path <> '' LIMIT 1")
                .bind(p.id).fetch_optional(&pool).await.ok().flatten(),
        };
        let cover = rel.and_then(|r| dir.as_ref().map(|d| d.join(r).to_string_lossy().into_owned()));
        out.push((p.id, p.name, p.face_count, cover));
    }
    out
}

/// Build the People tab model on the UI thread (Image loads must run here).
fn populate_people(w: &MainWindow, cards: Vec<(i64, Option<String>, i64, Option<String>)>) {
    use slint::{ModelRc, VecModel};
    let rows: Vec<PersonCard> = cards.into_iter().map(|(id, name, count, cover)| {
        let named = name.as_deref().map(|s| !s.is_empty()).unwrap_or(false);
        PersonCard {
            id: id as i32,
            name: name.unwrap_or_default().into(),
            count: count as i32,
            cover: cover.and_then(|p| slint::Image::load_from_path(std::path::Path::new(&p)).ok())
                .unwrap_or_default(),
            named,
        }
    }).collect();
    w.set_photo_people(ModelRc::new(VecModel::from(rows)));
}

/// Build the Things tab model on the UI thread.
fn populate_things(w: &MainWindow, things: Vec<(String, i64)>) {
    use slint::{ModelRc, VecModel};
    let rows: Vec<ThingChip> = things.into_iter()
        .map(|(label, count)| ThingChip { label: label.into(), count: count as i32 })
        .collect();
    w.set_photo_things(ModelRc::new(VecModel::from(rows)));
}

/// Albums + cover path (cover_id's photo, else first member). (np.p2.albums)
async fn load_albums() -> Vec<(i64, String, i64, Option<String>)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    let albums = tulipix_photos::albums::list(&pool).await.unwrap_or_else(|e| {
        tracing::warn!(error=%e, "albums::list"); Vec::new()
    });
    let mut out = Vec::with_capacity(albums.len());
    for a in albums {
        let cover: Option<String> = match a.cover_id {
            Some(cid) => sqlx::query_scalar("SELECT abs_path FROM items WHERE id = ?")
                .bind(cid).fetch_optional(&pool).await.ok().flatten(),
            None => sqlx::query_scalar(
                "SELECT i.abs_path FROM album_items ai JOIN items i ON i.id = ai.item_id \
                 WHERE ai.album_id = ? LIMIT 1")
                .bind(a.id).fetch_optional(&pool).await.ok().flatten(),
        };
        out.push((a.id, a.name, a.item_count, cover));
    }
    out
}

fn album_cards(cards: Vec<(i64, String, i64, Option<String>)>) -> Vec<AlbumCard> {
    cards.into_iter().map(|(id, name, count, cover)| AlbumCard {
        id: id as i32,
        name: name.into(),
        count: count as i32,
        cover: cover.and_then(|p| slint::Image::load_from_path(std::path::Path::new(&p)).ok()).unwrap_or_default(),
    }).collect()
}

/// Build the Albums tab model on the UI thread.
fn populate_albums(w: &MainWindow, cards: Vec<(i64, String, i64, Option<String>)>) {
    w.set_photo_albums(slint::ModelRc::new(slint::VecModel::from(album_cards(cards))));
}

/// Item ids for the current photo selection (grid index → path → items.id).
async fn selected_item_ids(pool: &sqlx::SqlitePool, paths: Vec<PathBuf>) -> Vec<i64> {
    let mut ids = Vec::with_capacity(paths.len());
    for p in paths { if let Some(id) = item_id_for(pool, &p).await { ids.push(id); } }
    ids
}

/// Snapshot the current selection's absolute paths (grid order).
fn selected_photo_paths() -> Vec<PathBuf> {
    let sel = selection().lock().map(|g| g.sel.clone()).unwrap_or_default();
    let pp = photo_paths().lock();
    match pp { Ok(g) => sel.iter().filter_map(|&i| g.get(i as usize).cloned()).collect(), Err(_) => Vec::new() }
}

async fn load_things() -> Vec<(String, i64)> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    tulipix_photos::ai::tags::things(&pool).await
        .unwrap_or_else(|e| { tracing::warn!(error=%e, "tags::things"); Vec::new() })
}

/// Live abs_paths for every photo carrying tag `label` (newest first).
async fn tag_photo_paths(label: &str) -> Vec<String> {
    let Ok(pool) = pool_for("photos").await else { return Vec::new(); };
    sqlx::query_scalar::<_, String>(
        "SELECT i.abs_path FROM items i
         JOIN item_tags it ON it.item_id = i.id
         JOIN tags t ON t.id = it.tag_id
         JOIN photo_meta pm ON pm.item_id = i.id
         WHERE t.name = ? AND i.missing_since IS NULL
           AND pm.deleted_at IS NULL AND pm.archived = 0
         ORDER BY i.added DESC",
    ).bind(label).fetch_all(&pool).await
    .unwrap_or_else(|e| { tracing::warn!(error=%e, "tag_photo_paths"); Vec::new() })
}

// ── Photo editor state (np.p2.edit.*) ──────────────────────────────────────
static EDITOR_ITEM: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
fn editor_item() -> &'static std::sync::Mutex<Option<i64>> {
    EDITOR_ITEM.get_or_init(|| std::sync::Mutex::new(None))
}
static EDITOR_SRC: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
fn editor_src() -> &'static std::sync::Mutex<Option<PathBuf>> {
    EDITOR_SRC.get_or_init(|| std::sync::Mutex::new(None))
}
// Full-resolution working copy of the original. Ops apply here so geometric
// ops (crop/resize) use real pixel coordinates and export matches the preview;
// the preview frame is downscaled only for display.
static EDITOR_ORIG: std::sync::OnceLock<std::sync::Mutex<Option<image::DynamicImage>>> = std::sync::OnceLock::new();
fn editor_orig() -> &'static std::sync::Mutex<Option<image::DynamicImage>> {
    EDITOR_ORIG.get_or_init(|| std::sync::Mutex::new(None))
}
// Dimensions of the *current* edited result (after the active stack). Crop maps
// UI fractions against these.
static EDITOR_CUR_DIMS: std::sync::OnceLock<std::sync::Mutex<(u32, u32)>> = std::sync::OnceLock::new();
fn editor_cur_dims() -> &'static std::sync::Mutex<(u32, u32)> {
    EDITOR_CUR_DIMS.get_or_init(|| std::sync::Mutex::new((0, 0)))
}

/// Build a display-sized RGBA buffer (≤1600px long edge) from a result image,
/// returning the buffer plus the result's true full-res dimensions.
fn to_display_buffer(out: &image::DynamicImage) -> (slint::SharedPixelBuffer<slint::Rgba8Pixel>, u32, u32) {
    let (fw, fh) = (out.width(), out.height());
    let disp = if fw.max(fh) > 1600 { out.thumbnail(1600, 1600) } else { out.clone() };
    let rgba = disp.to_rgba8();
    let (dw, dh) = (rgba.width(), rgba.height());
    let mut buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(dw, dh);
    buf.make_mut_bytes().copy_from_slice(rgba.as_raw());
    (buf, fw, fh)
}
static EDITOR_STACK: std::sync::OnceLock<std::sync::Mutex<tulipix_photos::editor::ops::EditStack>> = std::sync::OnceLock::new();
fn editor_stack() -> &'static std::sync::Mutex<tulipix_photos::editor::ops::EditStack> {
    EDITOR_STACK.get_or_init(|| std::sync::Mutex::new(Default::default()))
}
// True while the trailing Adjust op is the one the sliders are live-editing
// (so slider moves replace it instead of stacking a new op each tick).
static EDITOR_ADJUST_LIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn parse_preset(s: &str) -> Option<tulipix_photos::editor::filters::Preset> {
    use tulipix_photos::editor::filters::Preset;
    Some(match s {
        "bw" => Preset::BW, "sepia" => Preset::Sepia, "vintage" => Preset::Vintage,
        "drama" => Preset::Drama, "hdr" => Preset::Hdr, "polaroid" => Preset::Polaroid,
        "faded" => Preset::Faded, _ => return None,
    })
}

fn editor_reset_sliders(w: &MainWindow) {
    w.set_editor_exposure(0.0);
    w.set_editor_contrast(0.0);
    w.set_editor_saturation(0.0);
    w.set_editor_temperature(0.0);
    w.set_editor_highlights(0.0);
    w.set_editor_shadows(0.0);
}

/// Open the editor on the photo at `idx` (path already resolved): show the
/// overlay immediately, then load the saved edit stack + decode a preview.
fn open_editor(weak: slint::Weak<MainWindow>, _idx: i32, path: &std::path::Path) {
    let path = path.to_path_buf();
    if let Some(w) = weak.upgrade() {
        editor_reset_sliders(&w);
        w.set_editor_active_filter("".into());
        w.set_editor_status("".into());
        w.set_editor_filename(
            path.file_name().and_then(|s| s.to_str()).unwrap_or("").into());
        w.set_editor_busy(true);
        w.set_editor_open(true);
    }
    let weak2 = weak.clone();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let item_id = match pool_for("photos").await {
            Ok(pool) => item_id_for(&pool, &path).await,
            Err(_) => None,
        };
        let stack = match (item_id, pool_for("photos").await) {
            (Some(id), Ok(pool)) => tulipix_photos::editor::ops::load(&pool, id).await.unwrap_or_default(),
            _ => Default::default(),
        };
        let path2 = path.clone();
        // Decode at full resolution + build the "original" display frame.
        let decoded = tokio::task::spawn_blocking(move || {
            image::open(&path2).ok().map(|im| {
                let (buf, fw, fh) = to_display_buffer(&im);
                (im, buf, fw, fh)
            })
        }).await.ok().flatten();
        if let Ok(mut g) = editor_item().lock() { *g = item_id; }
        if let Ok(mut g) = editor_src().lock() { *g = Some(path); }
        if let Ok(mut g) = editor_stack().lock() { *g = stack; }
        if let Some((img, _, fw, fh)) = &decoded {
            if let Ok(mut g) = editor_orig().lock() { *g = Some(img.clone()); }
            if let Ok(mut g) = editor_cur_dims().lock() { *g = (*fw, *fh); }
        }
        EDITOR_ADJUST_LIVE.store(false, std::sync::atomic::Ordering::Relaxed);
        // Read EXIF for the Info viewer + Meta editor panels (off the UI thread).
        let exif_path = editor_src().lock().ok().and_then(|g| g.clone());
        let (dump, fields) = match exif_path {
            Some(p) => tokio::task::spawn_blocking(move || (format_exif(&p), exif_edit_fields(&p)))
                .await.unwrap_or_default(),
            None => (String::new(), Default::default()),
        };
        let orig_frame = decoded.map(|(_, buf, fw, fh)| (buf, fw, fh));
        let _ = weak2.upgrade_in_event_loop(move |w| {
            if let Some((buf, fw, fh)) = orig_frame {
                w.set_editor_original(slint::Image::from_rgba8(buf));
                w.set_editor_nat_w(fw as i32);
                w.set_editor_nat_h(fh as i32);
            }
            let (a, c, d, m, dt) = fields;
            w.set_editor_exif(dump.into());
            w.set_editor_exif_artist(a.into());
            w.set_editor_exif_copyright(c.into());
            w.set_editor_exif_description(d.into());
            w.set_editor_exif_comment(m.into());
            w.set_editor_exif_date(dt.into());
        });
        editor_render(weak2);
    });
}

/// Re-render the editor preview off-thread by applying the current stack to the
/// cached preview-scaled original, then push the frame + undo/redo flags.
fn editor_render(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = tokio::task::spawn_blocking(|| -> Option<(slint::SharedPixelBuffer<slint::Rgba8Pixel>, bool, bool, i32, u32, u32, slint::SharedPixelBuffer<slint::Rgba8Pixel>)> {
            let stack = editor_stack().lock().ok()?.clone();
            let orig = editor_orig().lock().ok()?.clone()?;
            let out = tulipix_photos::editor::ops::apply(orig.clone(), &stack).unwrap_or(orig);
            let (buf, fw, fh) = to_display_buffer(&out);
            let hist = histogram_buf(&out);
            if let Ok(mut g) = editor_cur_dims().lock() { *g = (fw, fh); }
            let can_undo = stack.undo_idx > 0;
            let can_redo = stack.undo_idx < stack.ops.len();
            Some((buf, can_undo, can_redo, stack.undo_idx as i32, fw, fh, hist))
        }).await.ok().flatten();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Some((buf, cu, cr, n, fw, fh, hist)) = res {
                w.set_editor_preview(slint::Image::from_rgba8(buf));
                w.set_editor_histogram(slint::Image::from_rgba8(hist));
                w.set_editor_can_undo(cu);
                w.set_editor_can_redo(cr);
                w.set_editor_op_count(n);
                // Crop/aspect math in the UI maps against the current result dims.
                w.set_editor_nat_w(fw as i32);
                w.set_editor_nat_h(fh as i32);
            }
            w.set_editor_busy(false);
        });
    });
}

/// Map an extension to an export format (for save-to-original).
fn format_for_ext(ext: &str) -> tulipix_photos::editor::export::ExportFormat {
    use tulipix_photos::editor::export::ExportFormat;
    match ext.to_ascii_lowercase().as_str() {
        "png" => ExportFormat::Png,
        "webp" => ExportFormat::Webp,
        "tif" | "tiff" => ExportFormat::Tiff,
        "heic" | "heif" => ExportFormat::Heic,
        _ => ExportFormat::Jpeg,
    }
}

/// First free `<stem>-NN.<ext>` (zero-padded) in `dir`, starting at 01.
fn incremented_stem(dir: &std::path::Path, base: &str, ext: &str) -> String {
    for n in 1..10_000 {
        let stem = format!("{base}-{n:02}");
        if !dir.join(format!("{stem}.{ext}")).exists() { return stem; }
    }
    format!("{base}-edit")
}

/// Resolve the super-resolution model path: `TULIPIX_SR_MODEL` env override,
/// else `<data>/models/swin2sr-x4.onnx`. None ⇒ no real model installed.
#[allow(dead_code)] // wired for the super-resolution feature (not yet on a UI path)
fn sr_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TULIPIX_SR_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() { return Some(p); }
    }
    let p = tulipix_photos::ai::models::models_root()?.join("swin2sr-x4.onnx");
    p.exists().then_some(p)
}

/// Build an upscaler: real ORT model when the `ai-onnx` feature is on and the
/// model is installed, else the Lanczos fallback. Returns `(impl, is_ai)`.
fn make_upscaler() -> (Box<dyn tulipix_photos::editor::upscale::Upscaler>, bool) {
    #[cfg(feature = "ai-onnx")]
    {
        if let Some(p) = sr_model_path() {
            match tulipix_photos::ai::onnx::OrtUpscaler::load(&p) {
                Ok(u) => return (Box::new(u), true),
                Err(e) => tracing::error!(error = %e, "OrtUpscaler load failed; using Lanczos"),
            }
        }
    }
    (Box::new(tulipix_photos::editor::upscale::NullUpscaler), false)
}

/// AI super-resolution (np.p2.edit.upscale). Applies the current edit stack on
/// the full-res original, runs the model (or Lanczos fallback), writes the
/// result next to the source, then rebases the editor onto that file so
/// further edits + export operate on the upscaled pixels.
fn editor_upscale(weak: slint::Weak<MainWindow>) {
    use image::GenericImageView;
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, image::DynamicImage, u32, u32, bool)> {
            let orig = editor_orig().lock().ok().and_then(|g| g.clone())
                .ok_or_else(|| anyhow::anyhow!("no image open"))?;
            let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
            let base = tulipix_photos::editor::ops::apply(orig.clone(), &stack).unwrap_or(orig);
            let (up, is_ai) = make_upscaler();
            let out = tulipix_photos::editor::upscale::apply(base, 4, up.as_ref())?;
            let (w, h) = out.dimensions();
            // Write next to the source as a PNG (lossless preserves the SR detail).
            let src = src.ok_or_else(|| anyhow::anyhow!("no source path"))?;
            let dir = src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base_stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
            let stem = incremented_stem(&dir, &format!("{base_stem}-upscaled"), "png");
            let dest = dir.join(format!("{stem}.png"));
            out.save(&dest)?;
            Ok((dest, out, w, h, is_ai))
        }).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            match result {
                Ok(Ok((dest, img, ow, oh, is_ai))) => {
                    // Rebase the editor onto the upscaled file.
                    if let Ok(mut g) = editor_src().lock() { *g = Some(dest.clone()); }
                    if let Ok(mut g) = editor_orig().lock() { *g = Some(img); }
                    if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
                    let tag = if is_ai { "AI upscaled" } else { "Lanczos upscaled (install model for AI)" };
                    w.set_editor_status(format!("{tag} → {ow}×{oh}").into());
                    editor_render(w.as_weak());
                    w.invoke_refresh_library();
                }
                Ok(Err(e)) => { tracing::error!(error = %e, "upscale"); w.set_editor_status(format!("Upscale failed: {e}").into()); w.set_editor_busy(false); }
                Err(e) => { tracing::error!(error = %e, "upscale join"); w.set_editor_status("Upscale failed.".into()); w.set_editor_busy(false); }
            }
        });
    });
}

/// DeOldify colourise model path: `TULIPIX_COLORIZE_MODEL` env override, else
/// `<data>/models/deoldify.onnx`.
#[allow(dead_code)] // wired for the colourise feature (not yet on a UI path)
fn colorize_model_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("TULIPIX_COLORIZE_MODEL") {
        let p = PathBuf::from(p);
        if p.exists() { return Some(p); }
    }
    let p = tulipix_photos::ai::models::models_root()?.join("deoldify.onnx");
    p.exists().then_some(p)
}

#[cfg(feature = "ai-onnx")]
fn make_coloriser() -> Option<Box<dyn tulipix_photos::editor::colorize::Coloriser>> {
    let p = colorize_model_path()?;
    match tulipix_photos::ai::onnx::OrtColoriser::load(&p) {
        Ok(c) => Some(Box::new(c)),
        Err(e) => { tracing::error!(error = %e, "OrtColoriser load failed"); None }
    }
}
#[cfg(not(feature = "ai-onnx"))]
fn make_coloriser() -> Option<Box<dyn tulipix_photos::editor::colorize::Coloriser>> { None }

/// AI colourisation (np.p2.edit.colorize). Applies the stack, runs DeOldify,
/// writes `<stem>-colorized-NN.png` next to the source, rebases the editor.
fn editor_colorize(weak: slint::Weak<MainWindow>) {
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<(PathBuf, image::DynamicImage)> {
            let coloriser = make_coloriser().ok_or_else(|| anyhow::anyhow!(
                "DeOldify model not installed — put deoldify.onnx in <data>/models"))?;
            let orig = editor_orig().lock().ok().and_then(|g| g.clone())
                .ok_or_else(|| anyhow::anyhow!("no image open"))?;
            let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
            let base = tulipix_photos::editor::ops::apply(orig.clone(), &stack).unwrap_or(orig);
            let out = tulipix_photos::editor::colorize::apply(base, coloriser.as_ref())?;
            let src = src.ok_or_else(|| anyhow::anyhow!("no source path"))?;
            let dir = src.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base_stem = src.file_stem().and_then(|s| s.to_str()).unwrap_or("photo");
            let stem = incremented_stem(&dir, &format!("{base_stem}-colorized"), "png");
            let dest = dir.join(format!("{stem}.png"));
            out.save(&dest)?;
            Ok((dest, out))
        }).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            match result {
                Ok(Ok((dest, img))) => {
                    if let Ok(mut g) = editor_src().lock() { *g = Some(dest); }
                    if let Ok(mut g) = editor_orig().lock() { *g = Some(img); }
                    if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
                    w.set_editor_status("Colorized".into());
                    editor_render(w.as_weak());
                    w.invoke_refresh_library();
                }
                Ok(Err(e)) => { tracing::error!(error = %e, "colorize"); w.set_editor_status(format!("Colorize failed: {e}").into()); w.set_editor_busy(false); }
                Err(e) => { tracing::error!(error = %e, "colorize join"); w.set_editor_status("Colorize failed.".into()); w.set_editor_busy(false); }
            }
        });
    });
}

/// "Save a copy" — decode the full-res original, apply the stack, and write a
/// new file *into the original's own folder* named `<stem>-NN.<ext>`.
fn editor_export(weak: slint::Weak<MainWindow>, format: String, quality: u8) {
    use tulipix_photos::editor::export::{ExportFormat, ExifPolicy, ExportSpec};
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
    let Some(src) = src else { return; };
    let fmt = match format.as_str() {
        "PNG" => ExportFormat::Png, "WEBP" => ExportFormat::Webp,
        "TIFF" => ExportFormat::Tiff, "HEIC" => ExportFormat::Heic,
        _ => ExportFormat::Jpeg,
    };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let src2 = src.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<PathBuf> {
            let img = image::open(&src2)?;
            let edited = tulipix_photos::editor::ops::apply(img, &stack)?;
            let out_dir = src2.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base = src2.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
            let stem = incremented_stem(&out_dir, &base, fmt.extension());
            let spec = ExportSpec {
                format: fmt, quality: quality.clamp(1, 100), exif: ExifPolicy::Preserve,
                out_dir, stem,
            };
            tulipix_photos::editor::export::write(&edited, Some(&src2), &spec)
        }).await;
        let ok = matches!(result, Ok(Ok(_)));
        let msg = match result {
            Ok(Ok(p)) => format!("Saved copy → {}", p.file_name().and_then(|s| s.to_str()).unwrap_or("")),
            Ok(Err(e)) => { tracing::error!(error=%e, "save copy"); "Save failed — see logs.".to_string() }
            Err(e) => { tracing::error!(error=%e, "save copy join"); "Save failed.".to_string() }
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_editor_status(msg.into());
            if ok { w.invoke_refresh_library(); } // background, no scan popup
        });
    });
}

/// "Save to original" — decode the full-res original, apply the stack, and
/// overwrite the source file in place (destructive). Clears the edit stack
/// since the pixels are now baked into the file.
fn editor_save_original(weak: slint::Weak<MainWindow>) {
    use tulipix_photos::editor::export::{ExifPolicy, ExportSpec};
    let src = editor_src().lock().ok().and_then(|g| g.clone());
    let stack = editor_stack().lock().map(|g| g.clone()).unwrap_or_default();
    let Some(src) = src else { return; };
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let src2 = src.clone();
        let result = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let img = image::open(&src2)?;
            let edited = tulipix_photos::editor::ops::apply(img, &stack)?;
            let ext = src2.extension().and_then(|s| s.to_str()).unwrap_or("jpg");
            let out_dir = src2.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let base = src2.file_stem().and_then(|s| s.to_str()).unwrap_or("photo").to_string();
            // export::write builds `<stem>.<fmt-ext>` in out_dir; matching the
            // format to the source extension makes it overwrite the original.
            let spec = ExportSpec {
                format: format_for_ext(ext), quality: 95, exif: ExifPolicy::Preserve,
                out_dir, stem: base,
            };
            tulipix_photos::editor::export::write(&edited, Some(&src2), &spec)?;
            Ok(())
        }).await;
        let item = editor_item().lock().map(|g| *g).unwrap_or(None);
        match result {
            Ok(Ok(())) => {
                // Pixels baked in → reset the stack + clear the DB edit row.
                if let Ok(mut g) = editor_stack().lock() { g.ops.clear(); g.undo_idx = 0; }
                if let Some(id) = item {
                    if let Ok(pool) = pool_for("photos").await {
                        let _ = tulipix_photos::editor::ops::save(&pool, id, &Default::default()).await;
                    }
                }
                let _ = weak.upgrade_in_event_loop(move |w| {
                    editor_reset_sliders(&w);
                    w.set_editor_active_filter("".into());
                    w.set_editor_op_count(0);
                    w.set_editor_can_undo(false);
                    w.set_editor_can_redo(false);
                    w.set_editor_status("Saved to original.".into());
                    w.invoke_refresh_library(); // background refresh (no scan popup)
                });
            }
            Ok(Err(e)) => {
                tracing::error!(error=%e, "save original");
                let _ = weak.upgrade_in_event_loop(move |w| w.set_editor_status("Save failed — see logs.".into()));
            }
            Err(e) => {
                tracing::error!(error=%e, "save original join");
                let _ = weak.upgrade_in_event_loop(move |w| w.set_editor_status("Save failed.".into()));
            }
        }
    });
}

/// Distinct parent directories across the full photo list (= "N folders").
fn photo_folder_count() -> i32 {
    let full = photo_full().lock().unwrap();
    let mut parents: Vec<&std::path::Path> = full.iter().filter_map(|(_, o, _)| o.parent()).collect();
    parents.sort();
    parents.dedup();
    parents.len() as i32
}

/// Copy (or move, when `is_cut`) `srcs` into `target`, resolving name
/// collisions. Returns the number that landed. Cross-device moves fall back to
/// copy-then-delete. (np.p2.multiselect paste)
fn paste_into(srcs: &[PathBuf], target: &std::path::Path, is_cut: bool) -> usize {
    let mut done = 0usize;
    for src in srcs {
        // Skip a no-op paste into the file's own directory on copy.
        let Some(name) = src.file_name() else { continue; };
        let dest = unique_dest(&target.join(name));
        if src == &dest { continue; }
        let ok = if is_cut {
            std::fs::rename(src, &dest).is_ok()
                || (std::fs::copy(src, &dest).is_ok() && std::fs::remove_file(src).is_ok())
        } else {
            std::fs::copy(src, &dest).is_ok()
        };
        if ok { done += 1; } else { tracing::warn!(src = %src.display(), "paste failed"); }
    }
    done
}

/// First free path of the form `stem.ext`, `stem (1).ext`, `stem (2).ext`, …
fn unique_dest(dest: &std::path::Path) -> PathBuf {
    if !dest.exists() { return dest.to_path_buf(); }
    let parent = dest.parent().unwrap_or_else(|| std::path::Path::new("."));
    let stem = dest.file_stem().and_then(|s| s.to_str()).unwrap_or("file");
    let ext = dest.extension().and_then(|s| s.to_str());
    for n in 1..10_000 {
        let name = match ext {
            Some(e) => format!("{stem} ({n}).{e}"),
            None => format!("{stem} ({n})"),
        };
        let cand = parent.join(name);
        if !cand.exists() { return cand; }
    }
    dest.to_path_buf()
}

/// The watched-folder root that contains `path` (so a paste rescans the right
/// library), falling back to the path's own directory.
fn library_root_for(_w: &MainWindow, path: &std::path::Path) -> Option<PathBuf> {
    for root in load_watched_folders() {
        if path.starts_with(&root) { return Some(root); }
    }
    Some(path.to_path_buf())
}

// Map video tile index → absolute path so a click can hand the file to mpv.
// Rebuilt by kick_video_refresh to match whatever tab is on screen.
static VIDEO_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
fn video_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    VIDEO_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Map video tile index → DB item id (parallel to video_paths), for flag actions.
static VIDEO_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn video_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    VIDEO_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Accumulated scan output for the videos section: (label, abs_path, thumb_path).
// kick_video_refresh joins this against the DB so a tab switch re-orders without
// re-rendering any thumbnails.
type VideoEntry = (String, PathBuf, PathBuf);
static VIDEO_FULL: std::sync::OnceLock<std::sync::Mutex<Vec<VideoEntry>>> = std::sync::OnceLock::new();
fn video_full() -> &'static std::sync::Mutex<Vec<VideoEntry>> {
    VIDEO_FULL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

// Currently drilled-into TV show (None = showing show cards / not in a show).
static VIDEO_SHOW: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
fn video_show() -> &'static std::sync::Mutex<Option<i64>> {
    VIDEO_SHOW.get_or_init(|| std::sync::Mutex::new(None))
}

/// Parse a `SxxEyy` / `SxxExx` season+episode out of a filename. Tolerant of
/// `s01e01`, `S1E1`, `S01.E01`. Returns (season, episode).
fn parse_season_episode(name: &str) -> Option<(i64, i64)> {
    let b = name.as_bytes();
    let lower = name.to_ascii_lowercase();
    let lb = lower.as_bytes();
    let mut i = 0;
    while i < lb.len() {
        if lb[i] == b's' {
            // read up to 2 digits for season
            let mut j = i + 1;
            let s0 = j;
            while j < lb.len() && lb[j].is_ascii_digit() && j - s0 < 2 { j += 1; }
            if j == s0 { i += 1; continue; }
            let season: i64 = lower[s0..j].parse().ok()?;
            // optional separator then 'e'
            let mut k = j;
            while k < lb.len() && (lb[k] == b'.' || lb[k] == b' ' || lb[k] == b'_' || lb[k] == b'-') { k += 1; }
            if k < lb.len() && lb[k] == b'e' {
                let e0 = k + 1;
                let mut e = e0;
                while e < lb.len() && lb[e].is_ascii_digit() && e - e0 < 3 { e += 1; }
                if e > e0 {
                    let episode: i64 = lower[e0..e].parse().ok()?;
                    let _ = b; // keep original for potential future use
                    return Some((season, episode));
                }
            }
        }
        i += 1;
    }
    None
}

/// Fetch the stored TMDB API key from keychain (None → TMDB scraping disabled).
fn tmdb_api_key() -> Option<String> {
    tulipix_core::api_keys::fetch("tmdb").ok().flatten()
}

/// Local poster cache directory: ~/.cache/tulipix/videos/posters/
fn poster_cache_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("videos").join("posters"))
}

/// Background TMDB scrape for a freshly-scanned video file.
/// Detects movie vs TV episode, fetches metadata + poster, stores locally.
/// Fire-and-forget: caller spawns this as a separate task.
async fn scrape_video_tmdb(pool: sqlx::SqlitePool, item_id: i64, path: PathBuf) {
    let Some(api_key) = tmdb_api_key() else { return; };
    let Some(cache_dir) = poster_cache_dir() else { return; };
    let client = tulipix_videos::tmdb::TmdbClient::new(api_key);
    // Determine if this item has been classified as a TV episode.
    let is_episode: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM episodes WHERE item_id = ?")
        .bind(item_id).fetch_one(&pool).await.unwrap_or(0) > 0;
    if is_episode {
        scrape_show_for_item(&pool, item_id, &client, &cache_dir).await;
    } else {
        scrape_movie_for_item(&pool, item_id, &path, &client, &cache_dir).await;
    }
}

/// Parse filename → TMDB movie search → upsert movies + cache poster.
async fn scrape_movie_for_item(
    pool: &sqlx::SqlitePool,
    item_id: i64,
    path: &std::path::Path,
    client: &tulipix_videos::tmdb::TmdbClient,
    cache_dir: &std::path::Path,
) {
    use tulipix_videos::tmdb::MetadataProvider as _;
    // Skip if already scraped.
    let cached: Option<String> = sqlx::query_scalar("SELECT poster_local FROM movies WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await.ok().flatten().flatten();
    if cached.is_some() { return; }
    let name = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let parsed = tulipix_videos::agents::parse_filename(name);
    let Ok(Some(meta)) = client.search_movie(&parsed.title, parsed.year).await else { return; };
    let _ = tulipix_videos::tmdb::upsert_movie(pool, item_id, &meta).await;
    if let Some(pp) = &meta.poster_path {
        if let Ok(local) = client.cache_poster(pp, cache_dir).await {
            let _ = sqlx::query("UPDATE movies SET poster_local = ? WHERE item_id = ?")
                .bind(local.to_string_lossy().as_ref()).bind(item_id).execute(pool).await;
        }
    }
}

/// Look up the parent show → TMDB TV search → update shows row + cache poster.
async fn scrape_show_for_item(
    pool: &sqlx::SqlitePool,
    item_id: i64,
    client: &tulipix_videos::tmdb::TmdbClient,
    cache_dir: &std::path::Path,
) {
    use tulipix_videos::tmdb::MetadataProvider as _;
    let Ok(Some(show_id)) = sqlx::query_scalar::<_, i64>(
        "SELECT show_id FROM episodes WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await else { return; };
    // Skip if already scraped.
    let cached: Option<String> = sqlx::query_scalar("SELECT poster_local FROM shows WHERE id = ?")
        .bind(show_id).fetch_optional(pool).await.ok().flatten().flatten();
    if cached.is_some() { return; }
    let title: Option<String> = sqlx::query_scalar("SELECT title FROM shows WHERE id = ?")
        .bind(show_id).fetch_optional(pool).await.ok().flatten();
    let Some(title) = title else { return; };
    let Ok(Some(meta)) = client.search_show(&title).await else { return; };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    // Update the existing show row in-place (avoids duplicate rows).
    let _ = sqlx::query(
        "UPDATE shows SET tmdb_id=?, tvdb_id=?, year=?, overview=?, \
         poster_path=?, backdrop_path=?, updated=? WHERE id=?")
        .bind(meta.tmdb_id).bind(meta.tvdb_id).bind(meta.year).bind(&meta.overview)
        .bind(&meta.poster_path).bind(&meta.backdrop_path).bind(now).bind(show_id)
        .execute(pool).await;
    if let Some(pp) = &meta.poster_path {
        if let Ok(local) = client.cache_poster(pp, cache_dir).await {
            let _ = sqlx::query("UPDATE shows SET poster_local = ? WHERE id = ?")
                .bind(local.to_string_lossy().as_ref()).bind(show_id).execute(pool).await;
        }
    }
}

/// Get-or-create a local `shows` row by title (no TMDB/TVDB id), returning id.
async fn get_or_create_show(pool: &sqlx::SqlitePool, title: &str) -> Option<i64> {
    if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>("SELECT id FROM shows WHERE title = ?")
        .bind(title).fetch_optional(pool).await { return Some(id); }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64).unwrap_or(0);
    sqlx::query("INSERT INTO shows (title, updated) VALUES (?, ?)")
        .bind(title).bind(now).execute(pool).await.ok()?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM shows WHERE title = ?")
        .bind(title).fetch_optional(pool).await.ok().flatten()
}

/// If `path`'s filename carries an SxxEyy tag, link it as an episode of the
/// show named by its parent directory.
async fn classify_tv_episode(pool: &sqlx::SqlitePool, item_id: i64, path: &std::path::Path) {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else { return; };
    let Some((season, episode)) = parse_season_episode(name) else { return; };
    let show_title = path.parent()
        .and_then(|p| p.file_name()).and_then(|s| s.to_str())
        .unwrap_or("Unknown Show").to_string();
    let Some(show_id) = get_or_create_show(pool, &show_title).await else { return; };
    let _ = tulipix_videos::episodes::upsert(
        pool, item_id, show_id, season, episode, None, None, None, None, None).await;
}

/// One TV show card for the Videos "TV" tab.
struct ShowRow { id: i64, title: String, count: i64, cover_path: String, poster_local: Option<String> }

/// Load show cards (title + episode count + poster; falls back to first episode's abs_path).
async fn video_show_cards(pool: &sqlx::SqlitePool) -> Vec<ShowRow> {
    let rows: Vec<(i64, String, i64, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT s.id, s.title, COUNT(e.item_id) AS n, \
                (SELECT i.abs_path FROM episodes e2 JOIN items i ON i.id = e2.item_id \
                 WHERE e2.show_id = s.id ORDER BY e2.season, e2.episode LIMIT 1) AS cover, \
                s.poster_local \
         FROM shows s JOIN episodes e ON e.show_id = s.id \
         GROUP BY s.id ORDER BY s.title",
    ).fetch_all(pool).await.unwrap_or_default();
    rows.into_iter().map(|(id, title, count, cover, poster_local)| ShowRow {
        id, title, count, cover_path: cover.unwrap_or_default(), poster_local,
    }).collect()
}

// Top-level video filter (tv|movies|local) + search query — read by video_rows_for.
static VIDEO_KIND: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn video_kind() -> &'static std::sync::Mutex<String> {
    VIDEO_KIND.get_or_init(|| std::sync::Mutex::new("local".into()))
}
static VIDEO_QUERY: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn video_query() -> &'static std::sync::Mutex<String> {
    VIDEO_QUERY.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// Format a duration in seconds as `H:MM:SS` (or `M:SS` under an hour).
fn fmt_duration(secs: f64) -> String {
    if !(secs.is_finite()) || secs < 1.0 { return String::new(); }
    let total = secs as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Clock label for the player scrubber — always renders (`0:00` at zero).
fn fmt_clock(secs: f64) -> String {
    let secs = if secs.is_finite() && secs > 0.0 { secs } else { 0.0 };
    let total = secs as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Unix-epoch seconds → "12 Jun 2022" (podcast episode dates).
fn fmt_date(epoch: i64) -> String {
    use chrono::{TimeZone, Utc, Datelike};
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match Utc.timestamp_opt(epoch, 0).single() {
        Some(dt) => format!("{} {} {}", dt.day(), MON[(dt.month0() as usize).min(11)], dt.year()),
        None => String::new(),
    }
}

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

/// JSON file holding the list of watched root folders, so libraries survive
/// restarts (the grids re-scan from these on launch).
fn watched_folders_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

fn load_watched_folders() -> Vec<PathBuf> {
    let Some(p) = watched_folders_path() else { return Vec::new(); };
    let Ok(body) = std::fs::read_to_string(&p) else { return Vec::new(); };
    serde_json::from_str::<Vec<String>>(&body)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
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
}

// ── Music folder → section tag (np.p5.atmusic.folder-sections) ──────────────
// Each scanned music folder can be assigned to one of the 5 top music sections
// so the user controls where its tracks belong. Persisted next to the watched
// folders so the assignment survives restarts.
const MUSIC_SECTIONS: [&str; 5] = ["mymusic", "podcasts", "audiobooks", "radio", "youtube"];

fn folder_sections_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("music_folder_sections.json"))
}
fn load_folder_sections() -> std::collections::HashMap<String, String> {
    folder_sections_path()
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default()
}
fn save_folder_sections(map: &std::collections::HashMap<String, String>) {
    let Some(file) = folder_sections_path() else { return; };
    if let Some(parent) = file.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(body) = serde_json::to_string_pretty(map) { let _ = std::fs::write(&file, body); }
}
/// Human label for a section key.
fn music_section_label(key: &str) -> &'static str {
    match key {
        "podcasts"   => "Podcasts",
        "audiobooks" => "Audiobooks",
        "radio"      => "Radio",
        "youtube"    => "YouTube",
        _            => "My Music",
    }
}
/// Section key for a human label (inverse of `music_section_label`).
fn music_section_key(label: &str) -> &'static str {
    match label {
        "Podcasts"   => "podcasts",
        "Audiobooks" => "audiobooks",
        "Radio"      => "radio",
        "YouTube"    => "youtube",
        _            => "mymusic",
    }
}
/// Persist a folder → section assignment (settings dropdown / chip both use this).
fn set_folder_section(folder: &str, key: &str) {
    let mut map = load_folder_sections();
    map.insert(folder.to_string(), key.to_string());
    save_folder_sections(&map);
}
/// Advance a folder's section tag to the next of the 5 and persist it.
fn cycle_folder_section(folder: &str) -> String {
    let mut map = load_folder_sections();
    let cur = map.get(folder).map(|s| s.as_str()).unwrap_or("mymusic");
    let idx = MUSIC_SECTIONS.iter().position(|s| *s == cur).unwrap_or(0);
    let next = MUSIC_SECTIONS[(idx + 1) % MUSIC_SECTIONS.len()].to_string();
    map.insert(folder.to_string(), next.clone());
    save_folder_sections(&map);
    next
}

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
fn anime4k_shader_args() -> Option<(String, usize)> {
    let dir = crate::dirs_default()?.join("shaders");
    let mut files: Vec<String> = std::fs::read_dir(&dir).ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("glsl"))
        .filter_map(|p| p.to_str().map(str::to_string))
        .collect();
    if files.is_empty() { return None; }
    files.sort();
    Some((files.join(":"), files.len()))
}

/// Launch a video in an external mpv window with resume + watch-progress
/// writeback over the JSON IPC socket. Runs entirely off the UI thread so the
/// app never blocks on playback (np.p3.player — windowed path).
fn spawn_mpv_windowed(path: PathBuf, resume: Option<f64>, item_id: Option<i64>) {
    use std::io::{BufRead, BufReader, Write};
    let rt = tokio::runtime::Handle::current();
    // Universal single stream: a new video stops music + any prior video.
    kill_music_proc();
    stop_video();
    std::thread::spawn(move || {
        let sock = mpv_ipc::endpoint("tulipix-mpv");
        mpv_ipc::cleanup(&sock);
        let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
        cmd.arg(&path)
            .arg("--force-window=yes")
            .arg("--window-maximized=yes")   // open full-size (windowed, not borderless)
            .arg("--keep-open=no")
            .arg(format!("--input-ipc-server={}", sock.display()));
        if let Some(r) = resume { if r > 1.0 { cmd.arg(format!("--start={r}")); } }
        // GLSL upscale chain (np.p3.player.upscale) — opt-in + shaders present.
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        if s.flag("playback.upscale", false) {
            if let Some((chain, _)) = anime4k_shader_args() {
                cmd.arg(format!("--glsl-shaders={chain}"));
            }
        }
        mpv_die_with_parent(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => { tracing::error!(error = %e, "mpv window launch failed"); return; }
        };
        let vpid = child.id();
        VIDEO_PID.store(vpid, std::sync::atomic::Ordering::SeqCst);
        // Reader thread: observe time-pos + duration into a shared cell.
        let pos = std::sync::Arc::new(std::sync::Mutex::new((0f64, 0f64)));
        let pos2 = pos.clone();
        let sockp = sock.clone();
        let reader = std::thread::spawn(move || {
            let Ok(mut stream) = mpv_ipc::connect(&sockp) else { return; };
            let _ = stream.write_all(
                b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n{\"command\":[\"observe_property\",2,\"duration\"]}\n");
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] == "property-change" {
                    if let Some(d) = v["data"].as_f64() {
                        if let Ok(mut g) = pos2.lock() {
                            match v["name"].as_str() {
                                Some("time-pos") => g.0 = d,
                                Some("duration") => g.1 = d,
                                _ => {}
                            }
                        }
                    }
                }
            }
        });
        let _ = child.wait();
        let _ = VIDEO_PID.compare_exchange(vpid, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst);
        let _ = std::fs::remove_file(&sock);
        let _ = reader.join();
        let (p, d) = pos.lock().map(|g| *g).unwrap_or((0.0, 0.0));
        if let Some(id) = item_id {
            rt.spawn(async move {
                if let Ok(pool) = pool_for("videos").await {
                    if d > 0.0 && p >= d * 0.98 {
                        let _ = tulipix_videos::watch_progress::mark_finished(&pool, id, true).await;
                    } else if p > 1.0 {
                        let _ = tulipix_videos::watch_progress::update(&pool, id, p, Some(d)).await;
                    }
                }
            });
        }
    });
}

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

/// A single video library row joined across video_meta + watch_progress.
struct VideoRow {
    abs_path: String,
    item_id: i64,
    starred: bool,
    watched: bool,
    progress: f32,
    duration: String,
    season: i32,
    episode: i32,
    poster_local: Option<String>,  // TMDB-cached poster abs path (np.p3.tmdb)
}

/// Query the videos DB for the rows that belong to `category`, already ordered
/// the way the tab wants to show them.
async fn video_rows_for(pool: &sqlx::SqlitePool, category: &str) -> Vec<VideoRow> {
    let (filter, mut order): (&str, String) = match category {
        "continue" => (
            "vm.deleted_at IS NULL AND vm.archived = 0 AND vm.last_accessed IS NOT NULL \
             AND COALESCE(wp.finished,0) = 0 AND COALESCE(wp.position_s,0) > 0",
            "vm.last_accessed DESC".into(),
        ),
        "starred" => ("vm.deleted_at IS NULL AND vm.starred = 1", "i.added DESC, i.id DESC".into()),
        "archive" => ("vm.deleted_at IS NULL AND vm.archived = 1", "i.added DESC, i.id DESC".into()),
        "trash"   => ("vm.deleted_at IS NOT NULL", "vm.deleted_at DESC".into()),
        _         => ("vm.deleted_at IS NULL AND vm.archived = 0", "i.added DESC, i.id DESC".into()),
    };
    // Top-level kind (tv|movies|local). Movies restricts to scraped movies;
    // TV restricts to episodes (and, when drilled into a show, to that show,
    // ordered by season/episode); local shows everything in the section.
    let kind = video_kind().lock().map(|g| g.clone()).unwrap_or_else(|_| "local".into());
    // `episodes` is always joined so season/episode are available for the
    // grouped browser; the tv kind_clause restricts to actual episodes.
    let kind_clause: String = match kind.as_str() {
        "movies" => " AND vm.item_id IN (SELECT item_id FROM movies)".into(),
        "tv" => {
            order = "ep.season, ep.episode".into();
            match *video_show().lock().unwrap_or_else(|p| p.into_inner()) {
                Some(id) => format!(" AND ep.show_id = {id}"),
                None     => " AND ep.item_id IS NOT NULL".into(),
            }
        }
        _ => String::new(),
    };
    let sql = format!(
        "SELECT i.abs_path, vm.item_id, vm.starred, \
                COALESCE(wp.finished,0) AS finished, \
                COALESCE(wp.position_s, 0.0) AS pos, \
                COALESCE(wp.duration_s, vm.duration_s) AS dur, \
                COALESCE(ep.season, 0) AS season, \
                COALESCE(ep.episode, 0) AS episode, \
                mv.poster_local AS poster_local \
         FROM video_meta vm \
         JOIN items i ON i.id = vm.item_id \
         LEFT JOIN watch_progress wp ON wp.item_id = vm.item_id \
         LEFT JOIN episodes ep ON ep.item_id = vm.item_id \
         LEFT JOIN movies mv ON mv.item_id = vm.item_id \
         WHERE {filter}{kind_clause} ORDER BY {order}",
    );
    let rows: Vec<(String, i64, i64, i64, f64, Option<f64>, i64, i64, Option<String>)> =
        sqlx::query_as(&sql).fetch_all(pool).await.unwrap_or_default();
    let q = video_query().lock().map(|g| g.to_lowercase()).unwrap_or_default();
    rows.into_iter().filter(|(abs_path, ..)| {
        // Filename search filter (case-insensitive substring).
        q.is_empty() || std::path::Path::new(abs_path).file_name()
            .and_then(|s| s.to_str()).map(|n| n.to_lowercase().contains(&q)).unwrap_or(false)
    }).map(|(abs_path, item_id, starred, finished, pos, dur, season, episode, poster_local)| {
        let progress = match dur {
            Some(d) if d > 0.0 => (pos / d).clamp(0.0, 1.0) as f32,
            _ => 0.0,
        };
        VideoRow {
            abs_path, item_id,
            starred: starred != 0,
            watched: finished != 0,
            progress,
            duration: dur.map(fmt_duration).unwrap_or_default(),
            season: season as i32,
            episode: episode as i32,
            poster_local,
        }
    }).collect()
}

/// Format a TMDB `release_date` ("YYYY-MM-DD") → human-readable "Jun 14, 2025".
fn fmt_release_date(s: &str) -> String {
    let b = s.as_bytes();
    if b.len() < 10 { return s.to_string(); }
    let month = match &s[5..7] {
        "01" => "Jan", "02" => "Feb", "03" => "Mar", "04" => "Apr",
        "05" => "May", "06" => "Jun", "07" => "Jul", "08" => "Aug",
        "09" => "Sep", "10" => "Oct", "11" => "Nov", "12" => "Dec",
        _ => return s.to_string(),
    };
    let day: i32 = s[8..10].parse().unwrap_or(0);
    let year = &s[0..4];
    format!("{month} {day}, {year}")
}

/// Load the four TMDB discover rails from the DB cache, download any missing
/// posters, then push all four models to the UI thread. Kicks a background
/// TMDB refresh so the next open of the Discover tab is always fresh.
fn kick_discover_refresh(weak: slint::Weak<MainWindow>) {
    use tulipix_videos::discover::{DiscoverKind, TmdbDiscover, load_feed, refresh};
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return; };
        let api_key = tmdb_api_key();
        let cache_dir = poster_cache_dir();
        // Helper: load one kind, optionally trigger a background TMDB refresh
        // when the key is available.
        async fn load_kind(
            pool: &sqlx::SqlitePool,
            kind: DiscoverKind,
            api_key: Option<&str>,
            _cache_dir: Option<&std::path::Path>,
        ) -> Vec<tulipix_videos::discover::DiscoverItem> {
            if let Some(key) = api_key {
                let client = TmdbDiscover::new(key);
                // Refresh in background — don't await; serve from cache below.
                let _ = refresh(pool, &client, kind).await;
            }
            load_feed(pool, kind).await.unwrap_or_default()
        }
        // Run all four kinds concurrently.
        let (trending_movies, trending_shows, upcoming, on_air) = tokio::join!(
            load_kind(&pool, DiscoverKind::TrendingMoviesDay, api_key.as_deref(), cache_dir.as_deref()),
            load_kind(&pool, DiscoverKind::TrendingShowsDay,  api_key.as_deref(), cache_dir.as_deref()),
            load_kind(&pool, DiscoverKind::UpcomingMovies,    api_key.as_deref(), cache_dir.as_deref()),
            load_kind(&pool, DiscoverKind::OnTheAirShows,     api_key.as_deref(), cache_dir.as_deref()),
        );

        // For each item, cache the poster locally and return a DiscoverCard.
        fn build_cards(
            items: Vec<tulipix_videos::discover::DiscoverItem>,
            cache_dir: Option<&std::path::Path>,
        ) -> Vec<(tulipix_videos::discover::DiscoverItem, Option<PathBuf>)> {
            items.into_iter().map(|it| {
                let local = it.poster_path.as_deref().and_then(|pp| {
                    cache_dir.and_then(|dir| {
                        // Check if already cached (sha256 path = same formula as cache_image).
                        use sha2::{Digest, Sha256};
                        let url = format!("https://image.tmdb.org/t/p/w500{}", pp);
                        let mut h = Sha256::new();
                        h.update(url.as_bytes());
                        let name = format!("{:x}.jpg", h.finalize());
                        let p = dir.join(name);
                        if p.exists() { Some(p) } else { None }
                    })
                });
                (it, local)
            }).collect()
        }
        let cd = cache_dir.as_deref();
        let trending_movies_cards = build_cards(trending_movies, cd);
        let trending_shows_cards  = build_cards(trending_shows, cd);
        let upcoming_cards        = build_cards(upcoming, cd);
        let on_air_cards          = build_cards(on_air, cd);

        let _ = weak.upgrade_in_event_loop(move |w| {
            fn to_slint(pairs: Vec<(tulipix_videos::discover::DiscoverItem, Option<PathBuf>)>) -> Vec<DiscoverCard> {
                pairs.into_iter().map(|(it, local)| {
                    let poster = local
                        .as_deref()
                        .and_then(|p| slint::Image::load_from_path(p).ok())
                        .unwrap_or_default();
                    let release = it.release_date.as_deref()
                        .map(fmt_release_date).unwrap_or_default();
                    DiscoverCard {
                        tmdb_id: it.tmdb_id as i32,
                        title: it.title.into(),
                        year: it.year.unwrap_or(0) as i32,
                        poster,
                        vote: it.vote_average.unwrap_or(0.0) as f32,
                        release_date: release.into(),
                        is_movie: it.is_movie,
                    }
                }).collect()
            }
            w.set_video_discover_movies(slint::ModelRc::new(slint::VecModel::from(to_slint(trending_movies_cards))));
            w.set_video_discover_shows(slint::ModelRc::new(slint::VecModel::from(to_slint(trending_shows_cards))));
            w.set_video_discover_upcoming(slint::ModelRc::new(slint::VecModel::from(to_slint(upcoming_cards))));
            w.set_video_discover_on_air(slint::ModelRc::new(slint::VecModel::from(to_slint(on_air_cards))));
        });

        // Now download any missing posters in the background and refresh models.
        if let (Some(key), Some(dir)) = (api_key.as_deref(), cache_dir.as_deref()) {
            let client = tulipix_videos::tmdb::TmdbClient::new(key);
            // Collect all items that need poster download.
            // Re-load all four kinds from DB to get their poster_paths.
            let all_items = {
                let mut v = Vec::new();
                for kind in [DiscoverKind::TrendingMoviesDay, DiscoverKind::TrendingShowsDay,
                              DiscoverKind::UpcomingMovies, DiscoverKind::OnTheAirShows] {
                    if let Ok(items) = load_feed(&pool, kind).await {
                        v.extend(items);
                    }
                }
                v
            };
            let dir_owned = dir.to_path_buf();
            tokio::spawn(async move {
                for it in all_items {
                    if let Some(pp) = &it.poster_path {
                        let _ = client.cache_poster(pp, &dir_owned).await;
                    }
                }
            });
        }
    });
}

/// Rebuild the videos grid for `category` off-thread, then set the tiles +
/// index→path/id maps on the UI thread. Thumbnails come from the accumulated
/// scan output (video_full) so a tab switch never re-renders a frame.
fn kick_video_refresh(weak: slint::Weak<MainWindow>, category: String) {
    let handle = tokio::runtime::Handle::current();
    // TV tab, not drilled into a show, on the Library sub-tab → show cards.
    let kind = video_kind().lock().map(|g| g.clone()).unwrap_or_else(|_| "local".into());
    let drilled = video_show().lock().map(|g| g.is_some()).unwrap_or(false);
    let tv_cards = kind == "tv" && !drilled && category == "library";
    if tv_cards {
        handle.spawn(async move {
            let (cards, next_up) = match pool_for("videos").await {
                Ok(pool) => (
                    video_show_cards(&pool).await,
                    tulipix_videos::episodes::next_up(&pool, 12).await.unwrap_or_default(),
                ),
                Err(_) => (Vec::new(), Vec::new()),
            };
            let _ = weak.upgrade_in_event_loop(move |w| {
                let by_path: std::collections::HashMap<String, PathBuf> = video_full()
                    .lock().map(|g| g.iter()
                        .map(|(_, orig, thumb)| (orig.to_string_lossy().into_owned(), thumb.clone()))
                        .collect())
                    .unwrap_or_default();
                let shows: Vec<ShowCard> = cards.into_iter().map(|s| {
                    // Prefer TMDB cached poster (np.p3.tmdb); fall back to
                    // ffmpeg scan thumb, then the raw episode abs path.
                    let thumb = s.poster_local.as_deref()
                        .map(PathBuf::from)
                        .filter(|p| p.exists())
                        .or_else(|| by_path.get(&s.cover_path).cloned())
                        .unwrap_or_else(|| PathBuf::from(&s.cover_path));
                    ShowCard {
                        id: s.id as i32,
                        title: s.title.into(),
                        cover: slint::Image::load_from_path(&thumb).unwrap_or_default(),
                        count: s.count as i32,
                    }
                }).collect();
                w.set_video_shows(slint::ModelRc::new(slint::VecModel::from(shows)));
                // "Next Up" rail (np.p3.episodes): one continue-watching tile
                // per show. tile.index maps into video_paths via video_ids so
                // the normal click→play path works unchanged.
                let id_pos: std::collections::HashMap<i64, i32> = video_ids().lock()
                    .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect())
                    .unwrap_or_default();
                let path_by_pos: Vec<String> = video_paths().lock()
                    .map(|g| g.iter().map(|p| p.to_string_lossy().into_owned()).collect())
                    .unwrap_or_default();
                let nu_tiles: Vec<VideoTile> = next_up.iter().filter_map(|n| {
                    let pos = *id_pos.get(&n.episode.item_id)?;
                    let abs = path_by_pos.get(pos as usize).cloned().unwrap_or_default();
                    let thumb = n.episode.still_path.as_deref()
                        .map(PathBuf::from).filter(|p| p.exists())
                        .or_else(|| by_path.get(&abs).cloned());
                    let dur_s = n.episode.runtime_min.unwrap_or(0) as f64 * 60.0;
                    let progress = match (n.resume_position_s, dur_s > 0.0) {
                        (Some(p), true) => (p / dur_s).clamp(0.0, 1.0) as f32,
                        _ => 0.0,
                    };
                    Some(VideoTile {
                        thumb: thumb.and_then(|t| slint::Image::load_from_path(&t).ok()).unwrap_or_default(),
                        label: format!("{} · S{:02}E{:02}{}", n.show_title, n.episode.season, n.episode.episode,
                            n.episode.title.as_deref().map(|t| format!(" — {t}")).unwrap_or_default()).into(),
                        index: pos,
                        starred: false, watched: false,
                        progress,
                        duration: n.episode.runtime_min.map(|m| format!("{m} min")).unwrap_or_default().into(),
                        season: n.episode.season as i32,
                        episode: n.episode.episode as i32,
                    })
                }).collect();
                w.set_video_next_up(slint::ModelRc::new(slint::VecModel::from(nu_tiles)));
                w.set_video_show_open(false);
                w.set_video_tiles(slint::ModelRc::new(slint::VecModel::from(Vec::<VideoTile>::new())));
                w.set_video_seasons(slint::ModelRc::new(slint::VecModel::from(Vec::<VideoSeason>::new())));
            });
        });
        return;
    }
    handle.spawn(async move {
        let rows = match pool_for("videos").await {
            Ok(pool) => video_rows_for(&pool, &category).await,
            Err(_) => Vec::new(),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_video_show_open(video_show().lock().map(|g| g.is_some()).unwrap_or(false));
            // abs_path → (label, thumb_path) from the scan accumulator.
            let by_path: std::collections::HashMap<String, (String, PathBuf)> = video_full()
                .lock().map(|g| g.iter()
                    .map(|(label, orig, thumb)| (orig.to_string_lossy().into_owned(), (label.clone(), thumb.clone())))
                    .collect())
                .unwrap_or_default();
            let mut tiles: Vec<VideoTile> = Vec::with_capacity(rows.len());
            let mut paths: Vec<PathBuf> = Vec::with_capacity(rows.len());
            let mut ids: Vec<i64> = Vec::with_capacity(rows.len());
            for r in &rows {
                let (label, ffmpeg_thumb) = by_path.get(&r.abs_path).cloned().unwrap_or_else(|| {
                    let l = std::path::Path::new(&r.abs_path).file_name()
                        .and_then(|s| s.to_str()).unwrap_or("").to_string();
                    (l, PathBuf::from(&r.abs_path))
                });
                // Prefer TMDB poster (np.p3.tmdb) over ffmpeg frame thumbnail.
                let thumb = r.poster_local.as_deref()
                    .map(PathBuf::from)
                    .filter(|p| p.exists())
                    .unwrap_or(ffmpeg_thumb);
                let img = slint::Image::load_from_path(&thumb).unwrap_or_default();
                tiles.push(VideoTile {
                    thumb: img,
                    label: label.into(),
                    index: tiles.len() as i32,
                    starred: r.starred,
                    watched: r.watched,
                    progress: r.progress,
                    duration: r.duration.clone().into(),
                    season: r.season,
                    episode: r.episode,
                });
                paths.push(PathBuf::from(&r.abs_path));
                ids.push(r.item_id);
            }
            if let Ok(mut g) = video_paths().lock() { *g = paths; }
            if let Ok(mut g) = video_ids().lock() { *g = ids; }
            // Drilled into a TV show → group episodes by season for the browser
            // (np.p3.episodes). Rows are already ordered season,episode so a
            // single pass yields contiguous season groups; tiles keep their
            // global `index` so click→play still maps into video_paths.
            let drilled_tv = video_kind().lock().map(|g| *g == "tv").unwrap_or(false)
                && video_show().lock().map(|g| g.is_some()).unwrap_or(false);
            let seasons: Vec<VideoSeason> = if drilled_tv {
                let mut groups: Vec<(i32, Vec<VideoTile>)> = Vec::new();
                for t in &tiles {
                    if groups.last().map(|g| g.0) != Some(t.season) {
                        groups.push((t.season, Vec::new()));
                    }
                    groups.last_mut().unwrap().1.push(t.clone());
                }
                groups.into_iter().map(|(num, ts)| VideoSeason {
                    number: num,
                    label: if num > 0 { format!("Season {num}") } else { "Specials".to_string() }.into(),
                    tiles: slint::ModelRc::new(slint::VecModel::from(ts)),
                }).collect()
            } else {
                Vec::new()
            };
            w.set_video_seasons(slint::ModelRc::new(slint::VecModel::from(seasons)));
            w.set_video_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
        });
    });
}

// ── Books data layer (np.p4.books.library / progress) ───────────────────────
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
        std::process::Command::new("powershell").args(["-NoProfile","-Command",&ps]).status()
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
                .status().map(|s| s.success()).unwrap_or(false)
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
static CLOUD_REMOTE: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn cloud_remote() -> &'static std::sync::Mutex<String> {
    CLOUD_REMOTE.get_or_init(|| std::sync::Mutex::new(String::new()))
}
static CLOUD_PATH: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn cloud_path() -> &'static std::sync::Mutex<String> {
    CLOUD_PATH.get_or_init(|| std::sync::Mutex::new(String::new()))
}
// Full (unfiltered) listing of the current folder: (name, is_dir, size, modified).
static CLOUD_ALL: std::sync::OnceLock<std::sync::Mutex<Vec<(String, bool, String, String)>>> = std::sync::OnceLock::new();
fn cloud_all() -> &'static std::sync::Mutex<Vec<(String, bool, String, String)>> {
    CLOUD_ALL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Rebuild the visible `cloud-entries` model from the cached full listing,
/// applying the header search filter (case-insensitive name match). The `index`
/// is re-enumerated to match the filtered model so entry-activate stays correct.
fn cloud_set_filtered(w: &MainWindow) {
    let q = w.get_cloud_query().to_string().trim().to_lowercase();
    let rows: Vec<CloudEntry> = cloud_all()
        .lock()
        .map(|g| {
            g.iter()
                .filter(|(name, _, _, _)| q.is_empty() || name.to_lowercase().contains(&q))
                .enumerate()
                .map(|(i, (name, is_dir, size, modified))| CloudEntry {
                    name: name.clone().into(),
                    is_dir: *is_dir,
                    size: size.clone().into(),
                    modified: modified.clone().into(),
                    index: i as i32,
                })
                .collect()
        })
        .unwrap_or_default();
    w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(rows)));
}

// Static Tools catalog: (key, title, icon, ops). Mirrors the cards the page used
// to show; the detail body + search read from here.
const TOOLS_CATALOG: &[(&str, &str, &str, &[&str])] = &[
    ("fileops", "File ops",  "📁", &["Rename", "Merge", "Split", "Hash", "Folder diff"]),
    ("video",   "Video",     "🎬", &["Compress", "Trim", "Convert", "Thumbnails", "Download"]),
    ("audio",   "Audio",     "🎵", &["Compress", "Normalise (R128)", "Extract tracks"]),
    ("photo",   "Photo",     "🖼", &["Compress", "Resize", "Watermark", "PDF tools"]),
    ("subs",    "Subtitles", "💬", &["Transcribe (whisper)", "Burn-in subtitles"]),
    ("queue",   "Queue",     "⚙", &["Parallel workers", "Pause / resume", "Retry"]),
];

/// Rebuild the Tools body from the window's category + query. Empty query shows
/// the selected category's ops; a query matches op names across every category.
fn tools_refresh(w: &MainWindow) {
    let cat = w.get_tools_category().to_string();
    let q = w.get_tools_query().to_string().trim().to_lowercase();
    let mut rows: Vec<ToolOpRow> = Vec::new();
    if q.is_empty() {
        if let Some((_, title, icon, ops)) = TOOLS_CATALOG.iter().find(|(k, ..)| *k == cat) {
            w.set_tools_cat_title((*title).into());
            w.set_tools_cat_icon((*icon).into());
            for op in *ops {
                rows.push(ToolOpRow { cat: (*title).into(), label: (*op).into() });
            }
        }
    } else {
        for (_, title, _, ops) in TOOLS_CATALOG {
            for op in *ops {
                if op.to_lowercase().contains(&q) {
                    rows.push(ToolOpRow { cat: (*title).into(), label: (*op).into() });
                }
            }
        }
    }
    w.set_tools_op_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// `rclone config dump` → mirror into cloud.db → set the remotes model.
fn cloud_refresh_remotes(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let ok = tokio::task::spawn_blocking(cloud::available).await.unwrap_or(false);
        let dump = if ok {
            tokio::task::spawn_blocking(|| cloud::run(&tulipix_cloud::remotes::dump_args()))
                .await.ok().and_then(|r| r.ok())
        } else { None };
        let parsed = dump.as_deref().map(tulipix_cloud::remotes::parse_dump).unwrap_or_default();
        if let Ok(pool) = pool_for("cloud").await {
            for (name, backend) in &parsed {
                let _ = tulipix_cloud::remotes::upsert(&pool, name, backend, false).await;
            }
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_rclone_ok(ok);
            let rows: Vec<CloudRemote> = parsed.into_iter().map(|(name, backend)| CloudRemote {
                name: name.into(), backend: backend.into(),
            }).collect();
            w.set_cloud_remotes(slint::ModelRc::new(slint::VecModel::from(rows)));
            if !ok {
                w.set_cloud_status("rclone not found — install rclone or bundle it in resources/bin".into());
            }
        });
    });
}

/// Create a remote non-interactively from the connect dialog fields.
fn cloud_connect(weak: slint::Weak<MainWindow>, name: String, backend: String, opts: String) {
    if name.trim().is_empty() || backend.trim().is_empty() { return; }
    // Parse "key=value" lines / commas.
    let pairs: Vec<(String, String)> = opts
        .split(['\n', ','])
        .filter_map(|kv| {
            let kv = kv.trim();
            if kv.is_empty() { return None; }
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let name_c = name.clone();
        let res = tokio::task::spawn_blocking(move || {
            let refs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            cloud::run(&tulipix_cloud::remotes::create_args(&name_c, &backend, &refs))
        }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let _ = weak.upgrade_in_event_loop(move |w| {
            match res {
                Ok(_) => {
                    w.set_cloud_connect_open(false);
                    w.set_cloud_form_name("".into());
                    w.set_cloud_form_opts("".into());
                    w.set_cloud_status(format!("Connected '{name}'").into());
                    cloud_refresh_remotes(w.as_weak());
                }
                Err(e) => w.set_cloud_status(format!("Connect failed: {e}").into()),
            }
        });
    });
}

fn cloud_delete(weak: slint::Weak<MainWindow>, name: String) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let name_c = name.clone();
        let _ = tokio::task::spawn_blocking(move || cloud::run(&tulipix_cloud::remotes::delete_args(&name_c))).await;
        if let Ok(pool) = pool_for("cloud").await {
            let _ = tulipix_cloud::remotes::remove(&pool, &name).await;
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            if cloud_remote().lock().map(|g| *g == name).unwrap_or(false) {
                if let Ok(mut g) = cloud_remote().lock() { g.clear(); }
                w.set_cloud_active_remote("".into());
                w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<CloudEntry>::new())));
            }
            w.set_cloud_status(format!("Removed '{name}'").into());
            cloud_refresh_remotes(w.as_weak());
        });
    });
}

/// `rclone lsjson <remote>:<path>` → file table for the active remote+path.
fn cloud_browse(weak: slint::Weak<MainWindow>) {
    let remote = cloud_remote().lock().map(|g| g.clone()).unwrap_or_default();
    if remote.is_empty() { return; }
    let path = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let (remote_c, path_c) = (remote.clone(), path.clone());
        let out = tokio::task::spawn_blocking(move || {
            cloud::run(&tulipix_cloud::browse::lsjson_args(&remote_c, &path_c))
        }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let _ = weak.upgrade_in_event_loop(move |w| {
            match out {
                Ok(json) => {
                    let mut entries = tulipix_cloud::browse::parse_lsjson(&json).unwrap_or_default();
                    tulipix_cloud::browse::sort_entries(&mut entries, tulipix_cloud::browse::SortKey::Name);
                    // Cache the full listing so the header search can filter it
                    // in place without re-hitting rclone.
                    let all: Vec<(String, bool, String, String)> = entries.into_iter().map(|e| (
                        e.name,
                        e.is_dir,
                        if e.is_dir { "—".to_string() } else { human_size(e.size.max(0) as u64) },
                        e.mod_time.chars().take(10).collect::<String>(),
                    )).collect();
                    if let Ok(mut g) = cloud_all().lock() { *g = all; }
                    cloud_set_filtered(&w);
                    w.set_cloud_status("".into());
                }
                Err(e) => {
                    if let Ok(mut g) = cloud_all().lock() { g.clear(); }
                    w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<CloudEntry>::new())));
                    w.set_cloud_status(format!("Browse failed: {e}").into());
                }
            }
        });
    });
}

fn cloud_open_remote(weak: slint::Weak<MainWindow>, name: String) {
    if let Ok(mut g) = cloud_remote().lock() { *g = name.clone(); }
    if let Ok(mut g) = cloud_path().lock() { g.clear(); }
    if let Some(w) = weak.upgrade() {
        w.set_cloud_active_remote(name.into());
        w.set_cloud_path("".into());
    }
    cloud_browse(weak);
}

fn cloud_into(weak: slint::Weak<MainWindow>, dir: String) {
    if let Ok(mut g) = cloud_path().lock() {
        if g.is_empty() { *g = dir; } else { *g = format!("{}/{}", g, dir); }
    }
    let p = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    if let Some(w) = weak.upgrade() { w.set_cloud_path(p.into()); }
    cloud_browse(weak);
}

fn cloud_up(weak: slint::Weak<MainWindow>) {
    if let Ok(mut g) = cloud_path().lock() {
        match g.rfind('/') {
            Some(i) => { g.truncate(i); }
            None => g.clear(),
        }
    }
    let p = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    if let Some(w) = weak.upgrade() { w.set_cloud_path(p.into()); }
    cloud_browse(weak);
}

// ── Cloud file open / context actions (mount-on-demand) ─────────────────────

/// (remote, remote-relative path, file name) for the entry `name` in the
/// currently-browsed directory.
fn cloud_entry_target(name: &str) -> (String, String, String) {
    let remote = cloud_remote().lock().map(|g| g.clone()).unwrap_or_default();
    let path = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    let rel = if path.is_empty() { name.to_string() } else { format!("{path}/{name}") };
    (remote, rel, name.to_string())
}

fn cloud_is_video(ext: &str) -> bool {
    matches!(ext, "mp4"|"mkv"|"mov"|"avi"|"webm"|"m4v"|"wmv"|"flv"|"mpg"|"mpeg"|"ts"|"m2ts"|"3gp"|"ogv")
}
fn cloud_is_audio(ext: &str) -> bool {
    matches!(ext, "mp3"|"flac"|"wav"|"aac"|"ogg"|"oga"|"m4a"|"opus"|"wma"|"aiff"|"alac")
}
fn cloud_is_image(ext: &str) -> bool {
    matches!(ext, "jpg"|"jpeg"|"png"|"webp"|"gif"|"bmp"|"tif"|"tiff"|"heic"|"heif"|"avif"|"jxl")
}

// Live `rclone mount` processes keyed by remote name (kept alive + reused).
static CLOUD_MOUNTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (PathBuf, std::process::Child)>>> = std::sync::OnceLock::new();
fn cloud_mounts() -> &'static std::sync::Mutex<std::collections::HashMap<String, (PathBuf, std::process::Child)>> {
    CLOUD_MOUNTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Is `mp` an active mountpoint? Linux scans mountinfo; elsewhere a non-empty
/// readable dir is treated as ready (best-effort).
fn cloud_is_mounted(mp: &std::path::Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let want = mp.to_string_lossy();
        std::fs::read_to_string("/proc/self/mountinfo")
            .map(|s| s.lines().any(|l| l.split(' ').nth(4) == Some(want.as_ref())))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    { std::fs::read_dir(mp).map(|mut d| d.next().is_some()).unwrap_or(false) }
}

/// Ensure `remote` is FUSE/WinFsp-mounted; return the local mountpoint.
/// Idempotent — reuses a live mount. Blocking: wrap in spawn_blocking.
fn cloud_mount_ensure(remote: &str) -> anyhow::Result<PathBuf> {
    if let Ok(mut g) = cloud_mounts().lock() {
        if let Some((mp, child)) = g.get_mut(remote) {
            let alive = child.try_wait().ok().flatten().is_none();
            if alive && cloud_is_mounted(mp) { return Ok(mp.clone()); }
            let _ = child.kill();
            // Linux unmounts via fusermount; other platforms drop the child and
            // let WinFsp/macFUSE reap the stale mount on its own.
            #[cfg(target_os = "linux")]
            { let mp = mp.clone(); let _ = std::process::Command::new("fusermount").args(["-u"]).arg(&mp).status(); }
            g.remove(remote);
        }
    }
    let base = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data dir"))?
        .join("mounts").join(remote);
    std::fs::create_dir_all(&base)?;
    let rclone = tulipix_core::thumbs::tool_bin("rclone");
    let args = tulipix_cloud::mount::mount_args(
        remote, &base.to_string_lossy(),
        tulipix_cloud::mount::FsKind::for_os(std::env::consts::OS));
    let child = std::process::Command::new(&rclone)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn rclone mount: {e}"))?;
    let mut ok = false;
    for _ in 0..150 {
        if cloud_is_mounted(&base) { ok = true; break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !ok {
        anyhow::bail!("mount did not come up — is FUSE/WinFsp installed?");
    }
    if let Ok(mut g) = cloud_mounts().lock() { g.insert(remote.to_string(), (base.clone(), child)); }
    Ok(base)
}

/// Best-effort unmount of every live cloud mount (called on app shutdown).
fn cloud_unmount_all() {
    if let Ok(mut g) = cloud_mounts().lock() {
        for (_remote, (mp, mut child)) in g.drain() {
            let _ = child.kill();
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("fusermount").args(["-uz"]).arg(&mp).status(); }
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("umount").arg(&mp).status(); }
            let _ = &mp;
        }
    }
}

/// Open a cloud file the natural way: video/audio → mpv, image → viewer,
/// anything else → the OS default app. Mounts the remote first.
fn cloud_open_file(weak: slint::Weak<MainWindow>, remote: String, rel: String, name: String) {
    if let Some(w) = weak.upgrade() { w.set_cloud_status(format!("Opening {name}… (mounting {remote})").into()); }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let remote2 = remote.clone();
        let mp = match tokio::task::spawn_blocking(move || cloud_mount_ensure(&remote2)).await {
            Ok(Ok(mp)) => mp,
            Ok(Err(e)) => {
                tracing::error!(error=%e, "cloud mount");
                let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Mount failed: {e}").into()));
                return;
            }
            Err(_) => return,
        };
        let local = PathBuf::from(tulipix_cloud::context::mount_path(&mp.to_string_lossy(), &rel));
        let ext = local.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if cloud_is_video(&ext) || cloud_is_audio(&ext) {
            spawn_mpv_windowed(local, None, None);
            let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Playing {name}").into()));
        } else if cloud_is_image(&ext) {
            let _ = weak.upgrade_in_event_loop(move |w| { cloud_show_image(&w, &local); w.set_cloud_status("".into()); });
        } else {
            let local2 = local.clone();
            let _ = tokio::task::spawn_blocking(move || tulipix_platform::fm::open_default(&local2)).await;
            let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Opened {name}").into()));
        }
    });
}

/// Mount then either reveal the file in the OS file manager or open it with the
/// default app (context-menu "Reveal" / "Open with default app").
fn cloud_open_mounted(weak: slint::Weak<MainWindow>, remote: String, rel: String, reveal: bool) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let remote2 = remote.clone();
        let mp = match tokio::task::spawn_blocking(move || cloud_mount_ensure(&remote2)).await {
            Ok(Ok(mp)) => mp,
            Ok(Err(e)) => { let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Mount failed: {e}").into())); return; }
            Err(_) => return,
        };
        let local = PathBuf::from(tulipix_cloud::context::mount_path(&mp.to_string_lossy(), &rel));
        let _ = tokio::task::spawn_blocking(move || {
            if reveal { tulipix_platform::fm::reveal_in_file_manager(&local) }
            else { tulipix_platform::fm::open_default(&local) }
        }).await;
    });
}

/// Show a single cloud image in the full-screen viewer (total = 1).
fn cloud_show_image(w: &MainWindow, path: &std::path::Path) {
    let img = slint::Image::load_from_path(path).unwrap_or_default();
    let sz = img.size();
    w.set_viewer_image(img);
    w.set_viewer_nat_w(sz.width as i32);
    w.set_viewer_nat_h(sz.height as i32);
    w.set_viewer_label(path.file_name().and_then(|s| s.to_str()).unwrap_or("").into());
    w.set_viewer_index(0);
    w.set_viewer_total(1);
    w.set_viewer_zoom(1.0);
    w.set_viewer_exif(format_exif(path).into());
    w.set_viewer_histogram(histogram_image(path));
    w.set_viewer_open(true);
}

/// `rclone copyto remote:rel ~/Downloads/name` — a one-off download.
fn cloud_download(weak: slint::Weak<MainWindow>, remote: String, rel: String, name: String) {
    let handle = tokio::runtime::Handle::current();
    if let Some(w) = weak.upgrade() { w.set_cloud_status(format!("Downloading {name}…").into()); }
    handle.spawn(async move {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let dest = PathBuf::from(home).join("Downloads").join(&name);
        let _ = std::fs::create_dir_all(dest.parent().unwrap_or(std::path::Path::new(".")));
        let src = format!("{remote}:{rel}");
        let dest_s = dest.to_string_lossy().into_owned();
        let res = tokio::task::spawn_blocking(move || {
            cloud::run(&["copyto".into(), src, dest_s])
        }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let msg = match res {
            Ok(_) => format!("Downloaded {name} → ~/Downloads"),
            Err(e) => { tracing::error!(error=%e, "cloud download"); format!("Download failed: {e}") }
        };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
    });
}

/// Delete a cloud entry (`deletefile` for files, `purge` for dirs), then
/// re-browse the current directory.
fn cloud_delete_entry(weak: slint::Weak<MainWindow>, remote: String, rel: String, is_dir: bool) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let target = format!("{remote}:{rel}");
        let verb = if is_dir { "purge" } else { "deletefile" };
        let res = tokio::task::spawn_blocking(move || cloud::run(&[verb.into(), target]))
            .await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let msg = match res {
            Ok(_) => "Deleted.".to_string(),
            Err(e) => { tracing::error!(error=%e, "cloud delete"); format!("Delete failed: {e}") }
        };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
        cloud_browse(weak);
    });
}

// Map music tile index → absolute path for playback.
static MUSIC_PATHS: std::sync::OnceLock<std::sync::Mutex<Vec<PathBuf>>> = std::sync::OnceLock::new();
fn music_paths() -> &'static std::sync::Mutex<Vec<PathBuf>> {
    MUSIC_PATHS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Accumulated music tracks across ALL watched folders: (label, abs, thumb).
// Like photo_full/video_full, this is the source of truth the tiles + paths are
// rebuilt from, so adding a second folder ACCUMULATES instead of replacing the
// first. Deduped by abs path.
static MUSIC_FULL: std::sync::OnceLock<std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>>> = std::sync::OnceLock::new();
fn music_full() -> &'static std::sync::Mutex<Vec<(String, PathBuf, PathBuf)>> {
    MUSIC_FULL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// Rebuild the music tiles + playback-path list from the accumulated
/// `music_full` set. Each tile's `index` is its playback position.
///
/// `music_tiles` stays position-aligned and COMPLETE (audiobook chapters
/// included) because rails/albums/audiobook cards resolve thumbs by playback
/// position. The My Music *Tracks grid* binds the separate `music_tracks_grid`
/// model, which drops every track under a folder assigned to a non-"My Music"
/// section — audiobooks live in their own tab, not in the song grid.
fn rebuild_music_tiles(w: &MainWindow) {
    let full = music_full().lock().map(|g| g.clone()).unwrap_or_default();
    let sections = load_folder_sections();
    let excluded: Vec<PathBuf> = sections.iter()
        .filter(|(_, key)| key.as_str() != "mymusic")
        .map(|(folder, _)| PathBuf::from(folder))
        .collect();
    let mut paths: Vec<PathBuf> = Vec::with_capacity(full.len());
    let mut tiles: Vec<PhotoTile> = Vec::with_capacity(full.len());
    let mut grid: Vec<PhotoTile> = Vec::with_capacity(full.len());
    for (i, (label, orig, thumb_path)) in full.iter().enumerate() {
        let tile = PhotoTile {
            thumb: slint::Image::load_from_path(thumb_path).unwrap_or_default(),
            label: label.clone().into(),
            index: i as i32,
            ..Default::default()
        };
        if !excluded.iter().any(|e| orig.starts_with(e)) {
            grid.push(tile.clone());
        }
        tiles.push(tile);
        paths.push(orig.clone());
    }
    if let Ok(mut g) = music_paths().lock() { *g = paths; }
    w.set_music_tracks_grid(slint::ModelRc::new(slint::VecModel::from(grid)));
    w.set_music_tiles(slint::ModelRc::new(slint::VecModel::from(tiles)));
}
/// Drop every accumulated music track whose abs path is under `dir` (used when
/// a folder is removed from the library so its tiles disappear without a rescan).
fn prune_music_full_under(dir: &std::path::Path) {
    if let Ok(mut g) = music_full().lock() {
        g.retain(|(_, orig, _)| !orig.starts_with(dir));
    }
}
// Parallel to music_paths: the music.db item_id at each playback position, so
// item_id-keyed features (dashboard / browse / rating) map back to a position.
static MUSIC_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn music_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    MUSIC_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// item_id of the track at the current now-playing position (None if unknown).
fn current_music_id(w: &MainWindow) -> Option<i64> {
    let idx = w.get_music_np_index();
    music_ids().lock().ok().and_then(|g| g.get(idx as usize).copied()).filter(|id| *id >= 0)
}

/// One track's metadata for the detailed, sortable Songs list.
#[derive(Clone)]
struct SongMeta { pos: i32, item_id: i64, title: String, artist: String, album: String, duration_s: f64, added: i64, plays: i64, loved: bool, stars: i32, synced: bool, release_date: String, is_audiobook: bool }
static MUSIC_SONGS: std::sync::OnceLock<std::sync::Mutex<Vec<SongMeta>>> = std::sync::OnceLock::new();
fn music_songs() -> &'static std::sync::Mutex<Vec<SongMeta>> {
    MUSIC_SONGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
const SONG_PAGE: usize = 35;
const RECENT_PAGE: usize = 6;
static MUSIC_QUERY: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn music_query_filter() -> &'static std::sync::Mutex<String> {
    MUSIC_QUERY.get_or_init(|| std::sync::Mutex::new(String::new()))
}
// Recently-played pool (up to 18) for the Home 6-per-page × 3-page pager.
static MUSIC_RECENT: std::sync::OnceLock<std::sync::Mutex<Vec<SongMeta>>> = std::sync::OnceLock::new();
fn music_recent() -> &'static std::sync::Mutex<Vec<SongMeta>> {
    MUSIC_RECENT.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Browse sources (tile + sort-count) per tab. PhotoTile holds a slint Image
// (not Send/Sync) so this lives in a UI-thread thread_local, not a static.
thread_local! {
    static MUSIC_BROWSE: std::cell::RefCell<std::collections::HashMap<&'static str, Vec<(PhotoTile, i64)>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
    // Full track list of the open album/artist/genre detail (display paginated 25/page).
    static DETAIL_ROWS: std::cell::RefCell<Vec<MusicSongRow>> = const { std::cell::RefCell::new(Vec::new()) };
    // Artist detail — the artist's albums column (2/row, 8/page, follows the track page).
    static DETAIL_ARTIST_ALBUMS: std::cell::RefCell<Vec<PhotoTile>> = const { std::cell::RefCell::new(Vec::new()) };
    // Metadata manager — every song's row (display paginated 30/page).
    static META_ROWS: std::cell::RefCell<Vec<MetaMgrRow>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Split a filename stem on " - " into (artist, title, album?) per the user's
/// "Artist - Title - Album" naming convention (np.p5.atmusic.metadata-manager).
fn parse_music_filename(stem: &str) -> (String, String, Option<String>) {
    let parts: Vec<&str> = stem.split(" - ").map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    match parts.len() {
        0 => (String::new(), stem.trim().to_string(), None),
        1 => (String::new(), parts[0].to_string(), None),
        2 => (parts[0].to_string(), parts[1].to_string(), None),
        _ => (parts[0].to_string(), parts[1].to_string(), Some(parts[2..].join(" - "))),
    }
}

/// Look the filename up on MusicBrainz and write the best match's title /
/// artist / album / year / genre back to `track_meta` (creating artist/album
/// rows as needed). Returns the fetched metadata for UI feedback.
async fn fetch_and_store_meta(
    pool: &sqlx::SqlitePool, client: &reqwest::Client, item_id: i64, stem: &str,
) -> anyhow::Result<Option<tulipix_music::musicbrainz::FetchedMeta>> {
    // Make sure the extra columns exist (idempotent).
    let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(pool).await;
    let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(pool).await;
    // Respect the user-lock — manually edited songs are never auto-overwritten.
    let locked: i64 = sqlx::query_scalar("SELECT COALESCE(user_locked, 0) FROM track_meta WHERE item_id = ?")
        .bind(item_id).fetch_optional(pool).await.ok().flatten().unwrap_or(0);
    if locked != 0 { return Ok(None); }
    let (artist_q, title_q, album_q) = parse_music_filename(stem);
    if title_q.is_empty() { return Ok(None); }
    let search = tulipix_music::musicbrainz::lookup_recording(client, &artist_q, &title_q).await?;
    let Some(meta) = tulipix_music::musicbrainz::best_metadata(&search) else { return Ok(None); };
    // Prefer MB values, fall back to the parsed filename where MB is blank.
    let f_title  = if meta.title.is_empty()  { title_q.clone() }  else { meta.title.clone() };
    let f_artist = if meta.artist.is_empty() { artist_q.clone() } else { meta.artist.clone() };
    let f_album  = if meta.album.is_empty()  { album_q.unwrap_or_default() } else { meta.album.clone() };
    let artist_id = if f_artist.is_empty() { None }
        else { tulipix_music::scan::get_or_create_artist(pool, &f_artist).await.ok() };
    let album_id = if f_album.is_empty() { None }
        else { tulipix_music::scan::get_or_create_album(pool, &f_album, artist_id, meta.year).await.ok() };
    let credits = if meta.credits.is_empty() { None } else { Some(meta.credits.clone()) };
    sqlx::query(
        "UPDATE track_meta SET title = ?, artist_id = COALESCE(?, artist_id),
            album_id = COALESCE(?, album_id), genre = COALESCE(?, genre), year = COALESCE(?, year),
            release_date = COALESCE(?, release_date), credits = COALESCE(?, credits)
         WHERE item_id = ?")
        .bind(&f_title).bind(artist_id).bind(album_id)
        .bind(&meta.genre).bind(meta.year)
        .bind(&meta.release_date).bind(&credits).bind(item_id)
        .execute(pool).await?;
    Ok(Some(meta))
}

/// Build the metadata-manager row list from the live song + path tables.
fn build_meta_rows(w: &MainWindow) {
    let songs = music_songs().lock().map(|g| g.clone()).unwrap_or_default();
    let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
    // My Music only — audiobook chapters have their own section and would
    // flood the manager with untagged rows.
    let rows: Vec<MetaMgrRow> = songs.iter().filter(|s| !s.is_audiobook).map(|s| {
        let file = paths.get(s.pos as usize)
            .and_then(|p| p.file_stem()).and_then(|x| x.to_str()).unwrap_or("").to_string();
        // A song counts as "Tagged" when it already carries both artist + album.
        let tagged = !s.artist.trim().is_empty() && !s.album.trim().is_empty();
        MetaMgrRow {
            file: file.into(), title: s.title.clone().into(), artist: s.artist.clone().into(),
            album: s.album.clone().into(), status: if tagged { "Tagged".into() } else { "Pending".into() }, index: s.pos,
        }
    }).collect();
    w.set_music_meta_mgr_total(rows.len() as i32);
    META_ROWS.with(|r| *r.borrow_mut() = rows);
}

/// True when a metadata row counts as already-tagged (matched online or carries tags).
fn meta_row_tagged(status: &str) -> bool { status == "Tagged" || status == "Matched" }

/// Publish the current metadata-manager page (30 rows) + counts + page count,
/// filtered by the active stat-card filter (all | tagged | missing).
fn publish_meta_page(w: &MainWindow) {
    const PER: usize = 30;
    META_ROWS.with(|r| {
        let all = r.borrow();
        let total = all.len();
        let tagged = all.iter().filter(|x| meta_row_tagged(&x.status)).count();
        w.set_music_meta_mgr_total(total as i32);
        w.set_music_meta_mgr_tagged(tagged as i32);
        w.set_music_meta_mgr_missing((total - tagged) as i32);
        let filter = w.get_music_meta_mgr_filter().to_string();
        let filtered: Vec<MetaMgrRow> = all.iter().filter(|x| match filter.as_str() {
            "tagged" => meta_row_tagged(&x.status),
            "missing" => !meta_row_tagged(&x.status),
            _ => true,
        }).cloned().collect();
        let pages = filtered.len().div_ceil(PER).max(1);
        let page = (w.get_music_meta_mgr_page().max(0) as usize).min(pages - 1);
        w.set_music_meta_mgr_pages(pages as i32);
        w.set_music_meta_mgr_page(page as i32);
        let slice: Vec<MetaMgrRow> = filtered.iter().skip(page * PER).take(PER).cloned().collect();
        w.set_music_meta_mgr_rows(slint::ModelRc::new(slint::VecModel::from(slice)));
    });
}

/// Update one metadata row's status (and tags, when a match was found), then
/// republish the visible page.
fn update_meta_row(w: &MainWindow, pos: i32, status: &str, fetched: Option<&tulipix_music::musicbrainz::FetchedMeta>) {
    META_ROWS.with(|r| {
        let mut all = r.borrow_mut();
        if let Some(row) = all.iter_mut().find(|x| x.index == pos) {
            row.status = status.into();
            if let Some(f) = fetched {
                if !f.title.is_empty()  { row.title  = f.title.clone().into(); }
                if !f.artist.is_empty() { row.artist = f.artist.clone().into(); }
                if !f.album.is_empty()  { row.album  = f.album.clone().into(); }
            }
        }
    });
    publish_meta_page(w);
}
/// Store the full artist-albums list and publish the first 8-per-page slice.
fn set_detail_artist_albums(w: &MainWindow, albums: Vec<PhotoTile>) {
    DETAIL_ARTIST_ALBUMS.with(|r| *r.borrow_mut() = albums);
    publish_detail_artist_albums(w);
}
/// Publish the current 8-album page (keyed to the track page index).
fn publish_detail_artist_albums(w: &MainWindow) {
    const PER: usize = 8;
    DETAIL_ARTIST_ALBUMS.with(|r| {
        let all = r.borrow();
        let page = w.get_music_detail_page().max(0) as usize;
        let slice: Vec<PhotoTile> = all.iter().skip(page * PER).take(PER).cloned().collect();
        w.set_music_detail_artist_albums(slint::ModelRc::new(slint::VecModel::from(slice)));
    });
}
/// Store the detail's full rows and publish the first 25-track page.
fn set_detail_rows(w: &MainWindow, rows: Vec<MusicSongRow>) {
    DETAIL_ROWS.with(|r| *r.borrow_mut() = rows);
    w.set_music_detail_page(0);
    publish_detail_page(w);
}
/// Publish the current detail page (25 tracks) + page count.
fn publish_detail_page(w: &MainWindow) {
    const PER: usize = 25;
    DETAIL_ROWS.with(|r| {
        let rows = r.borrow();
        let pages = rows.len().div_ceil(PER).max(1);
        let page = (w.get_music_detail_page().max(0) as usize).min(pages - 1);
        let slice: Vec<MusicSongRow> = rows.iter().skip(page * PER).take(PER).cloned().collect();
        w.set_music_detail_pages(pages as i32);
        w.set_music_detail_page(page as i32);
        w.set_music_detail_tracks(slint::ModelRc::new(slint::VecModel::from(slice)));
    });
    // Keep the artist-albums column in step with the track page.
    publish_detail_artist_albums(w);
}
fn browse_tab_key(tab: &str) -> Option<&'static str> {
    ["albums", "artists", "genres", "folders", "playlists"].into_iter().find(|k| *k == tab)
}

/// Filter the cached Artists/Albums browse tiles by the search query for the
/// unified grouped-search header (np.p5.atmusic.lib-grouped-search).
fn rebuild_grouped_search(w: &MainWindow, q: &str) {
    let q = q.trim().to_lowercase();
    let pick = |tab: &'static str, limit: usize| -> Vec<PhotoTile> {
        if q.is_empty() { return Vec::new(); }
        MUSIC_BROWSE.with(|m| m.borrow().get(tab).cloned().unwrap_or_default())
            .into_iter().filter(|(t, _)| t.label.to_lowercase().contains(&q))
            .map(|(t, _)| t).take(limit).collect()
    };
    // Artists: 9 per line × 2 lines = 18. Albums: 7 per line × 2 lines = 14.
    w.set_music_search_artists(slint::ModelRc::new(slint::VecModel::from(pick("artists", 18))));
    w.set_music_search_albums(slint::ModelRc::new(slint::VecModel::from(pick("albums", 14))));
}
fn set_browse_src(tab: &'static str, src: Vec<(PhotoTile, i64)>) {
    MUSIC_BROWSE.with(|m| { m.borrow_mut().insert(tab, src); });
}

/// Rebuild the Home "Recently played" page (6 rows) from the recent pool.
fn rebuild_recent_page(w: &MainWindow) {
    let g = match music_recent().lock() { Ok(g) => g, Err(_) => return };
    let pages = g.len().div_ceil(RECENT_PAGE).clamp(1, 3);
    let page = (w.get_music_recent_page() as usize).min(pages - 1);
    let tiles = w.get_music_tiles();
    let rows: Vec<MusicSongRow> = g.iter().skip(page * RECENT_PAGE).take(RECENT_PAGE).map(|s| MusicSongRow {
        thumb: if s.pos >= 0 && (s.pos as usize) < tiles.row_count() {
            tiles.row_data(s.pos as usize).map(|t| t.thumb).unwrap_or_default() } else { slint::Image::default() },
        title: s.title.clone().into(), artist: s.artist.clone().into(),
        duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
        index: s.pos,
    }).collect();
    w.set_music_recent_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_recent_pages(pages as i32);
    w.set_music_recent_page(page as i32);
}

/// Re-sort + publish the active browse tab's tiles by name/count (np.p4 sort).
fn rebuild_browse_tab(w: &MainWindow, tab: &str) {
    let Some(tab) = browse_tab_key(tab) else { return; };
    let mut v: Vec<(PhotoTile, i64)> = MUSIC_BROWSE.with(|m| m.borrow().get(tab).cloned().unwrap_or_default());
    // Context search — filter the current browse tab by the search box query.
    {
        let q = music_query_filter().lock().map(|s| s.trim().to_lowercase()).unwrap_or_default();
        if !q.is_empty() { v.retain(|(t, _)| t.label.to_lowercase().contains(&q)); }
    }
    let sort = w.get_music_browse_sort().to_string();
    let asc = w.get_music_browse_dir() == "asc";
    v.sort_by(|a, b| {
        let o = match sort.as_str() {
            "count"  => a.1.cmp(&b.1),
            "rating" => a.0.stack_count.cmp(&b.0.stack_count),
            _        => a.0.label.to_lowercase().cmp(&b.0.label.to_lowercase()),
        };
        if asc { o } else { o.reverse() }
    });
    // Albums show their track count next to the name (easier visual sorting).
    let tiles: Vec<PhotoTile> = v.into_iter().map(|(mut t, c)| {
        if tab == "albums" || tab == "genres" { t.label = format!("{}  ·  {} tracks", t.label, c).into(); }
        t
    }).collect();
    // Albums/Artists are paginated 21/page (Songs-grid style); the rest show all.
    const BROWSE_PER: usize = 21;
    let page_slice = |w: &MainWindow, tiles: Vec<PhotoTile>| -> Vec<PhotoTile> {
        let pages = tiles.len().div_ceil(BROWSE_PER).max(1);
        let page = (w.get_music_browse_page().max(0) as usize).min(pages - 1);
        w.set_music_browse_pages(pages as i32);
        w.set_music_browse_page(page as i32);
        tiles.into_iter().skip(page * BROWSE_PER).take(BROWSE_PER).collect()
    };
    match tab {
        "albums"    => w.set_music_albums(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        "artists"   => w.set_music_artists(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        "genres"    => w.set_music_genres(slint::ModelRc::new(slint::VecModel::from(page_slice(w, tiles)))),
        "folders"   => w.set_music_folders(slint::ModelRc::new(slint::VecModel::from(tiles))),
        "playlists" => w.set_music_playlists(slint::ModelRc::new(slint::VecModel::from(tiles))),
        _ => {}
    }
}

/// Build the "Up next" queue panel. Prefers the persisted `play_queue` (so a
/// reordered/explicit queue survives relaunch — np.p5.music.queue-persist) and
/// falls back to the tracks after the current position (np.p4.music.queue).
fn build_music_queue(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids = tulipix_music::queue::list(&pool).await.unwrap_or_default();
        if ids.is_empty() { let _ = weak.upgrade_in_event_loop(build_sequential_queue); return; }
        let _ = weak.upgrade_in_event_loop(move |w| set_instant_mix_queue(&w, &ids));
    });
}

/// The classic sequential "Up next" (tracks after the current position).
/// Audiobook chapter playing → the queue is the BOOK's remaining chapters
/// only, never the My Music library that happens to follow it positionally.
fn build_sequential_queue(w: MainWindow) {
    let total = w.get_music_np_total();
    if total <= 0 { return; }
    let cur = w.get_music_np_index();
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
        .unwrap_or_default();
    let tiles = w.get_music_tiles();
    let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
    let cur_dir = paths.get(cur as usize).and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let is_book = cur_dir.as_ref()
        .map(|d| ab_cover_cache().lock().ok().map(|g| g.contains_key(&d.display().to_string())).unwrap_or(false)
            || load_folder_sections().get(&d.display().to_string()).map(|k| k == "audiobooks").unwrap_or(false))
        .unwrap_or(false);
    let next_positions: Vec<i32> = if is_book {
        // Remaining chapters of this book, in order, no wrap.
        ((cur + 1)..total)
            .filter(|&pos| paths.get(pos as usize).and_then(|p| p.parent()) == cur_dir.as_deref())
            .take(30)
            .collect()
    } else {
        (1..=30.min(total - 1)).map(|off| (cur + off).rem_euclid(total)).collect()
    };
    // Audiobook chapters have no per-track art — use the book cover as each
    // queue row's thumb so rows read like a normal queue (▶ overlay on art).
    let book_thumb: Option<slint::Image> = if is_book {
        cur_dir.as_ref().and_then(|d| ab_cover_cache().lock().ok()
            .and_then(|g| g.get(&d.display().to_string()).cloned()))
            .map(|px| art_image(&Some(px)))
    } else { None };
    let rows: Vec<MusicSongRow> = next_positions.into_iter().map(|pos| {
        let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        MusicSongRow {
            // Chapters always show the book cover (incl. user-picked custom
            // art) — their scan thumbs are generic waveform placeholders.
            thumb: match &book_thumb {
                Some(cover) => cover.clone(),
                None => if (pos as usize) < tiles.row_count() { tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default() } else { slint::Image::default() },
            },
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
        }
    }).collect();
    w.set_music_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Build an "Up next" queue panel from explicit item_ids (used by Instant Mix).
/// item_ids are mapped back to their playback positions via `music_ids`.
fn set_instant_mix_queue(w: &MainWindow, ids: &[i64]) {
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect())
        .unwrap_or_default();
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
        .unwrap_or_default();
    let tiles = w.get_music_tiles();
    let rows: Vec<MusicSongRow> = ids.iter().filter_map(|id| pos_of.get(id).copied()).map(|pos| {
        let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        MusicSongRow {
            thumb: if (pos as usize) < tiles.row_count() { tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default() } else { slint::Image::default() },
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
        }
    }).collect();
    w.set_music_queue_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

// Parsed synced-lyric lines for the current track: (ms, text), driving the
// active-line highlight during playback (np.p5.music.lyrics-synced).
static MUSIC_LYRICS_LINES: std::sync::OnceLock<std::sync::Mutex<Vec<(i64, String)>>> = std::sync::OnceLock::new();
fn music_lyrics_lines() -> &'static std::sync::Mutex<Vec<(i64, String)>> {
    MUSIC_LYRICS_LINES.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// Last cast-discovery results, indexed by the names shown in the UI.
static CAST_TARGETS: std::sync::OnceLock<std::sync::Mutex<Vec<tulipix_music::cast::CastDevice>>> = std::sync::OnceLock::new();
fn cast_targets() -> &'static std::sync::Mutex<Vec<tulipix_music::cast::CastDevice>> {
    CAST_TARGETS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
// The currently-open playlist detail: (playlist_id, ordered item_ids) — lets the
// remove button map a row index back to an item (np.p5.music.playlists-builder).
static CURRENT_PLAYLIST: std::sync::OnceLock<std::sync::Mutex<(i64, Vec<i64>)>> = std::sync::OnceLock::new();
fn current_playlist() -> &'static std::sync::Mutex<(i64, Vec<i64>)> {
    CURRENT_PLAYLIST.get_or_init(|| std::sync::Mutex::new((-1, Vec::new())))
}
// Playlist ids backing the add-to-playlist picker (parallel to the shown names).
static PICK_PLAYLIST_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn pick_playlist_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    PICK_PLAYLIST_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Map a list of item_ids (in order) to MusicSongRows, resolving each to its
/// playback position so the existing `play-music`/`tile-clicked` path plays it.
/// Ids not in the current library are skipped. Shared by Favorites + History.
fn song_rows_for_ids(w: &MainWindow, ids: &[i64]) -> Vec<MusicSongRow> {
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect()).unwrap_or_default();
    let tiles = w.get_music_tiles();
    ids.iter().filter_map(|id| pos_of.get(id).copied()).map(|pos| {
        let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        MusicSongRow {
            thumb: if pos >= 0 && (pos as usize) < tiles.row_count() { tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default() } else { slint::Image::default() },
            title: if title.is_empty() { "Track".into() } else { title.into() },
            artist: artist.into(),
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
        }
    }).collect()
}

// LRCLIB manual-search results for the current pick (content, is_synced).
static LYRICS_SEARCH: std::sync::OnceLock<std::sync::Mutex<Vec<(String, bool)>>> = std::sync::OnceLock::new();
fn lyrics_search_store() -> &'static std::sync::Mutex<Vec<(String, bool)>> {
    LYRICS_SEARCH.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Scanned-roots list with per-root music track counts (np.p5.atmusic.lib-folder-mgmt).
fn populate_folder_roots(w: &MainWindow) {
    let roots = load_watched_folders();
    // Folders assigned to other music sections (Audiobooks etc.) belong to
    // their own section's UI — never to the My Music Folders tab.
    let sections = load_folder_sections();
    let excluded: Vec<PathBuf> = sections.iter()
        .filter(|(_, key)| key.as_str() != "mymusic")
        .map(|(folder, _)| PathBuf::from(folder))
        .collect();
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let mut labels: Vec<slint::SharedString> = Vec::new();
        for r in &roots {
            if excluded.iter().any(|e| r.starts_with(e)) { continue; }
            let prefix = format!("{}%", r.to_string_lossy());
            let n: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM items i JOIN track_meta tm ON tm.item_id = i.id \
                 WHERE i.section = 'music' AND i.missing_since IS NULL \
                   AND COALESCE(tm.is_audiobook, 0) = 0 AND i.abs_path LIKE ?")
                .bind(&prefix).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
            let name = r.file_name().and_then(|s| s.to_str()).unwrap_or(".").to_string();
            labels.push(format!("{name}   ·   {n} tracks   —   {}", r.display()).into());
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_folder_roots(slint::ModelRc::new(slint::VecModel::from(labels)));
        });
    });
}

/// Favorites page (np.p5.atmusic.favorites-page) — all loved tracks.
/// All loved track ids (favorites), paginated 20/page in the UI.
static FAV_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn fav_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    FAV_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// Filter item_ids by the current search query (title/artist substring),
/// resolving each via the cached `music_songs` metadata. Empty query = all.
fn filter_ids_by_query(ids: &[i64]) -> Vec<i64> {
    let q = music_query_filter().lock().map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if q.is_empty() { return ids.to_vec(); }
    let by_id: std::collections::HashMap<i64, (String, String)> = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.item_id, (s.title.to_lowercase(), s.artist.to_lowercase()))).collect())
        .unwrap_or_default();
    ids.iter().copied().filter(|id| by_id.get(id)
        .map(|(t, a)| t.contains(&q) || a.contains(&q)).unwrap_or(false)).collect()
}
/// Publish one 20-track page of favorites + the page count.
fn rebuild_fav_page(w: &MainWindow) {
    const PER: usize = 20;
    let all = fav_ids().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = filter_ids_by_query(&all);
    let pages = ids.len().div_ceil(PER).max(1);
    let page = (w.get_music_fav_page().max(0) as usize).min(pages - 1);
    let slice: Vec<i64> = ids.iter().skip(page * PER).take(PER).copied().collect();
    let rows = song_rows_for_ids(w, &slice);
    w.set_music_fav_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_fav_pages(pages as i32);
    w.set_music_fav_page(page as i32);
}
fn populate_favorites(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE loved = 1 ORDER BY title COLLATE NOCASE")
            .fetch_all(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Ok(mut g) = fav_ids().lock() { *g = ids; }
            w.set_music_fav_page(0);
            rebuild_fav_page(&w);
        });
    });
}

/// Playback History page (np.p5.atmusic.history-page) — most-recent plays first.
/// Cached ids so the search box can filter the list live.
static HISTORY_IDS: std::sync::OnceLock<std::sync::Mutex<Vec<i64>>> = std::sync::OnceLock::new();
fn history_ids() -> &'static std::sync::Mutex<Vec<i64>> {
    HISTORY_IDS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
/// Re-publish history rows, applying the current search query filter and paging
/// 30 plays per page (up to 5 pages = 150 most-recent plays).
fn rebuild_history_page(w: &MainWindow) {
    const PER: usize = 30;
    const MAX_PAGES: usize = 5;
    let all = history_ids().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = filter_ids_by_query(&all);
    let pages = ids.len().div_ceil(PER).clamp(1, MAX_PAGES);
    w.set_music_history_pages(pages as i32);
    let page = (w.get_music_history_page().max(1) as usize).min(pages);
    w.set_music_history_page(page as i32);
    let start = (page - 1) * PER;
    let slice: Vec<i64> = ids.iter().skip(start).take(PER).copied().collect();
    let rows = song_rows_for_ids(w, &slice);
    w.set_music_history_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}
fn populate_history(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM play_history ORDER BY played_at DESC LIMIT 150")
            .fetch_all(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Ok(mut g) = history_ids().lock() { *g = ids; }
            rebuild_history_page(&w);
        });
    });
}

// Current Album/Artist detail context: (kind, db id, ordered track item_ids).
static MUSIC_DETAIL: std::sync::OnceLock<std::sync::Mutex<(String, i64, Vec<i64>)>> = std::sync::OnceLock::new();
fn music_detail() -> &'static std::sync::Mutex<(String, i64, Vec<i64>)> {
    MUSIC_DETAIL.get_or_init(|| std::sync::Mutex::new((String::new(), -1, Vec::new())))
}

/// Open the Album detail overlay (np.p5.atmusic.album-detail): header (cover,
/// title, album-artist · year) + full tracklist ordered by disc/track number.
fn open_album_detail(w: &MainWindow, album_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let hdr: Option<(String, Option<i64>, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT al.title, al.year, al.cover_path, ar.name
             FROM albums al LEFT JOIN artists ar ON ar.id = al.artist_id WHERE al.id = ?")
            .bind(album_id).fetch_optional(&pool).await.ok().flatten();
        let Some((title, year, cover, artist)) = hdr else { return; };
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let al_loved: i64 = sqlx::query_scalar("SELECT COALESCE(loved,0) FROM albums WHERE id = ?").bind(album_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let al_rating: i64 = sqlx::query_scalar("SELECT COALESCE(rating,0) FROM albums WHERE id = ?").bind(album_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE album_id = ? ORDER BY COALESCE(disc_no,0), COALESCE(track_no,0), title")
            .bind(album_id).fetch_all(&pool).await.unwrap_or_default();
        let sub = {
            let mut s = artist.unwrap_or_default();
            if let Some(y) = year { if y > 0 { if !s.is_empty() { s.push_str("  ·  "); } s.push_str(&y.to_string()); } }
            s
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows = song_rows_for_ids(&w, &ids);
            let art = rows.first().map(|r| r.thumb.clone())
                .or_else(|| cover.as_ref().filter(|p| std::path::Path::new(p).exists())
                    .map(|p| slint::Image::load_from_path(std::path::Path::new(p)).unwrap_or_default()))
                .unwrap_or_default();
            if let Ok(mut g) = music_detail().lock() { *g = ("album".into(), album_id, ids.clone()); }
            w.set_music_detail_kind("album".into());
            w.set_music_detail_title(title.into());
            w.set_music_detail_subtitle(sub.into());
            w.set_music_detail_art(art);
            w.set_music_detail_bio("".into());
            w.set_music_detail_status("".into());
            w.set_music_detail_loved(al_loved != 0);
            w.set_music_detail_stars(al_rating as i32);
            set_detail_rows(&w, rows);
            w.set_music_detail_open(true);
        });
    });
}

/// Open the Artist detail overlay (np.p5.atmusic.artist-detail): header (image,
/// name, track/album counts), all tracks, and a MusicBrainz bio fetched async.
fn open_artist_detail(w: &MainWindow, artist_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let hdr: Option<(String, Option<String>, Option<String>)> = sqlx::query_as(
            "SELECT name, image_path, bio FROM artists WHERE id = ?")
            .bind(artist_id).fetch_optional(&pool).await.ok().flatten();
        let Some((name, image, bio)) = hdr else { return; };
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE artist_id = ? ORDER BY COALESCE(album_id,0), COALESCE(track_no,0), title")
            .bind(artist_id).fetch_all(&pool).await.unwrap_or_default();
        let album_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(DISTINCT album_id) FROM track_meta WHERE artist_id = ? AND album_id IS NOT NULL")
            .bind(artist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        // This artist's own favourite / rating state (for the info-box buttons).
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let ar_loved: i64 = sqlx::query_scalar("SELECT COALESCE(loved,0) FROM artists WHERE id = ?").bind(artist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let ar_rating: i64 = sqlx::query_scalar("SELECT COALESCE(rating,0) FROM artists WHERE id = ?").bind(artist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        // Albums by this artist (cover + first track) for the 70/30 right column.
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let alb: Vec<(i64, String, Option<String>, i64, i64, i64)> = sqlx::query_as(
            "SELECT al.id, al.title, al.cover_path, MIN(tm.item_id), COALESCE(al.loved,0), COALESCE(al.rating,0) \
             FROM track_meta tm JOIN albums al ON al.id = tm.album_id \
             WHERE tm.artist_id = ? GROUP BY al.id ORDER BY al.title COLLATE NOCASE")
            .bind(artist_id).fetch_all(&pool).await.unwrap_or_default();
        let sub = format!("{} track{}  ·  {} album{}",
            ids.len(), if ids.len() == 1 { "" } else { "s" },
            album_count, if album_count == 1 { "" } else { "s" });
        let cached_bio = bio.clone().unwrap_or_default();
        let _ = weak.upgrade_in_event_loop({
            let name = name.clone();
            move |w| {
                let rows = song_rows_for_ids(&w, &ids);
                let art = rows.first().map(|r| r.thumb.clone())
                    .or_else(|| image.as_ref().filter(|p| std::path::Path::new(p).exists())
                        .map(|p| slint::Image::load_from_path(std::path::Path::new(p)).unwrap_or_default()))
                    .unwrap_or_default();
                if let Ok(mut g) = music_detail().lock() { *g = ("artist".into(), artist_id, ids.clone()); }
                w.set_music_detail_kind("artist".into());
                w.set_music_detail_title(name.into());
                w.set_music_detail_subtitle(sub.into());
                w.set_music_detail_art(art);
                w.set_music_detail_bio(cached_bio.into());
                w.set_music_detail_loved(ar_loved != 0);
                w.set_music_detail_stars(ar_rating as i32);
                w.set_music_detail_status("".into());
                set_detail_rows(&w, rows);
                // Build the artist-albums column tiles (cover → first-track thumb).
                let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                    .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
                let tiles = w.get_music_tiles();
                let album_tiles: Vec<PhotoTile> = alb.iter().map(|(_aid, title, cover, first, loved, rating)| {
                    let pos = pos_of.get(first).copied().unwrap_or(-1);
                    let thumb = cover.as_ref().filter(|p| std::path::Path::new(p).exists())
                        .map(|p| slint::Image::load_from_path(std::path::Path::new(p)).unwrap_or_default())
                        .or_else(|| if pos >= 0 && (pos as usize) < tiles.row_count() { tiles.row_data(pos as usize).map(|t| t.thumb) } else { None })
                        .unwrap_or_default();
                    PhotoTile { thumb, label: title.clone().into(), index: pos,
                        starred: *loved != 0, stack_count: *rating as i32, ..Default::default() }
                }).collect();
                set_detail_artist_albums(&w, album_tiles);
                w.set_music_detail_open(true);
            }
        });
        // Background MusicBrainz bio fetch when we don't have one cached.
        if bio.as_deref().unwrap_or("").is_empty() {
            let client = reqwest::Client::new();
            if let Ok(s) = tulipix_music::musicbrainz::lookup_artist(&client, &name).await {
                if let Some(blurb) = s.artists.iter().max_by_key(|a| a.score)
                    .map(tulipix_music::musicbrainz::artist_blurb) {
                    let _ = sqlx::query("UPDATE artists SET bio = ? WHERE id = ?")
                        .bind(&blurb).bind(artist_id).execute(&pool).await;
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        if w.get_music_detail_open() && w.get_music_detail_kind() == "artist" {
                            w.set_music_detail_bio(blurb.into());
                        }
                    });
                }
            }
        }
    });
}

/// Open a Genre detail overlay (np.p4.music.browse) — all tracks in the genre,
/// shown before playback like album/artist pages.
fn open_genre_detail(w: &MainWindow, genre: String) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE genre = ? ORDER BY COALESCE(artist_id,0), COALESCE(album_id,0), title")
            .bind(&genre).fetch_all(&pool).await.unwrap_or_default();
        let sub = format!("{} track{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows = song_rows_for_ids(&w, &ids);
            // Genre cover override wins over the first track's album art.
            let art = load_music_pref(&format!("music.genre.cover.{genre}"))
                .map(|c| slint::Image::load_from_path(std::path::Path::new(&c)).unwrap_or_default())
                .filter(|im| im.size().width > 0)
                .unwrap_or_else(|| rows.first().map(|r| r.thumb.clone()).unwrap_or_default());
            if let Ok(mut g) = music_detail().lock() { *g = ("genre".into(), -1, ids.clone()); }
            w.set_music_detail_kind("genre".into());
            w.set_music_detail_title(genre.into());
            w.set_music_detail_subtitle(sub.into());
            w.set_music_detail_art(art);
            w.set_music_detail_bio("".into());
            w.set_music_detail_status("".into());
            set_detail_rows(&w, rows);
            w.set_music_detail_open(true);
        });
    });
}

/// Re-open whatever album/artist/genre detail overlay is currently showing, so a
/// metadata edit reflects in the overlay without navigating away.
fn refresh_open_detail(w: &MainWindow) {
    if !w.get_music_detail_open() { return; }
    let (kind, id, _) = music_detail().lock().map(|g| g.clone()).unwrap_or_default();
    match kind.as_str() {
        "album" if id >= 0 => open_album_detail(w, id),
        "artist" if id >= 0 => open_artist_detail(w, id),
        "genre" => open_genre_detail(w, w.get_music_detail_title().to_string()),
        _ => {}
    }
}

/// Item ids whose file lives directly in `dir` (one folder level), by playback
/// position — using the in-memory paths/ids so no path SQL is needed.
fn folder_track_ids(dir: &std::path::Path) -> Vec<i64> {
    let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
    let ids = music_ids().lock().map(|g| g.clone()).unwrap_or_default();
    paths.iter().zip(ids.iter())
        .filter(|(p, _)| p.parent() == Some(dir))
        .map(|(_, id)| *id).collect()
}
/// Open a folder's own songs page (detail overlay) — its directly-contained tracks.
fn open_folder_detail(w: &MainWindow, pos: i32) {
    let Some(dir) = music_paths().lock().ok()
        .and_then(|g| g.get(pos as usize).and_then(|p| p.parent().map(|d| d.to_path_buf()))) else { return; };
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let ids = folder_track_ids(&dir);
    let title = dir.file_name().and_then(|s| s.to_str()).unwrap_or("Folder").to_string();
    let sub = format!("{} track{}", ids.len(), if ids.len() == 1 { "" } else { "s" });
    let rows = song_rows_for_ids(w, &ids);
    let art = rows.first().map(|r| r.thumb.clone()).unwrap_or_default();
    if let Ok(mut g) = music_detail().lock() { *g = ("folder".into(), -1, ids.clone()); }
    w.set_music_detail_kind("folder".into());
    w.set_music_detail_title(title.into());
    w.set_music_detail_subtitle(sub.into());
    w.set_music_detail_art(art);
    w.set_music_detail_bio("".into());
    w.set_music_detail_status("".into());
    set_detail_rows(w, rows);
    w.set_music_detail_open(true);
    let _ = pos_of;
}

/// Queue all of the open detail's tracks and start playback (Play All / Shuffle).
fn detail_play(w: &MainWindow, shuffle: bool) {
    let ids = music_detail().lock().map(|g| g.2.clone()).unwrap_or_default();
    if ids.is_empty() { return; }
    let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let mut positions: Vec<i32> = ids.iter().filter_map(|id| pos_of.get(id).copied()).collect();
    if positions.is_empty() { return; }
    w.set_music_shuffle(shuffle);
    let first = if shuffle {
        let n = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos()).unwrap_or(0) as usize) % positions.len();
        positions.swap(0, n);
        positions[0]
    } else { positions[0] };
    // Build a persisted play_queue from the rest so advance follows the album.
    let rest_ids: Vec<i64> = positions.iter().skip(1).filter_map(|p| {
        music_ids().lock().ok().and_then(|g| g.get(*p as usize).copied())
    }).collect();
    play_music_at(w, first);
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let _ = tulipix_music::queue::clear(&pool).await;
        for id in &rest_ids { let _ = tulipix_music::queue::enqueue(&pool, *id, "detail").await; }
        let _ = weak.upgrade_in_event_loop(|w| build_music_queue(&w));
    });
}

/// Extract a vivid dominant colour from an album-art file for the now-playing
/// gradient wash (np.p5.atmusic.art-gradient — a small color-thief-style
/// quantiser over a downscaled copy, biased to saturated buckets).
fn dominant_color(path: &std::path::Path) -> Option<slint::Color> {
    let img = image::open(path).ok()?.thumbnail(48, 48).to_rgb8();
    // 4×4×4 colour histogram, weighted by saturation × a mid-luma preference.
    let mut buckets = [(0.0f64, 0u64, 0u64, 0u64, 0u64); 64];
    for p in img.pixels() {
        let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let max = r.max(g).max(b); let min = r.min(g).min(b);
        let sat = if max > 0.0 { (max - min) / max } else { 0.0 };
        let luma = (0.299 * r + 0.587 * g + 0.114 * b) / 255.0;
        let weight = sat * (1.0 - (luma - 0.55).abs()); // favour vivid, mid-bright
        let idx = ((p[0] >> 6) as usize) * 16 + ((p[1] >> 6) as usize) * 4 + (p[2] >> 6) as usize;
        let e = &mut buckets[idx];
        e.0 += weight; e.1 += 1; e.2 += p[0] as u64; e.3 += p[1] as u64; e.4 += p[2] as u64;
    }
    let best = buckets.iter().filter(|e| e.1 > 0).max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))?;
    if best.1 == 0 { return None; }
    let (r, g, b) = ((best.2 / best.1) as u8, (best.3 / best.1) as u8, (best.4 / best.1) as u8);
    Some(slint::Color::from_rgb_u8(r, g, b))
}

/// Load a playlist's tracks into the detail view: rows (mapped to playback
/// positions for play) + the ordered item ids (for remove).
/// Custom cover image path for a playlist (stored in Settings, not the DB).
fn playlist_cover_path(id: i64) -> Option<String> {
    let s = tulipix_core::settings::Settings::load().ok()?;
    s.advanced.get(&format!("music.playlist.cover.{id}")).cloned()
        .filter(|p| std::path::Path::new(p).exists())
}
fn build_playlist_detail(w: &MainWindow, playlist_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let name: String = sqlx::query_scalar("SELECT name FROM playlists WHERE id = ?")
            .bind(playlist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or_default();
        let smart: i64 = sqlx::query_scalar("SELECT COALESCE(is_smart,0) FROM playlists WHERE id = ?")
            .bind(playlist_id).fetch_optional(&pool).await.ok().flatten().unwrap_or(0);
        let item_ids: Vec<i64> = if smart != 0 {
            let rule_json: Option<String> = sqlx::query_scalar("SELECT rule_json FROM playlists WHERE id = ?")
                .bind(playlist_id).fetch_optional(&pool).await.ok().flatten();
            match rule_json.and_then(|j| serde_json::from_str::<tulipix_music::playlists::SmartRule>(&j).ok()) {
                Some(rule) => tulipix_music::playlists::evaluate(&pool, &rule).await.unwrap_or_default(),
                None => Vec::new(),
            }
        } else {
            tulipix_music::playlists::items(&pool, playlist_id).await.unwrap_or_default()
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let pos_of: std::collections::HashMap<i64, i32> = music_ids().lock()
                .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect())
                .unwrap_or_default();
            let by_pos: std::collections::HashMap<i32, (String, String, f64)> = music_songs().lock()
                .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect())
                .unwrap_or_default();
            let tiles = w.get_music_tiles();
            let mut rows: Vec<MusicSongRow> = item_ids.iter().map(|id| {
                let pos = pos_of.get(id).copied().unwrap_or(-1);
                let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
                MusicSongRow {
                    thumb: if pos >= 0 && (pos as usize) < tiles.row_count() { tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default() } else { slint::Image::default() },
                    title: if title.is_empty() { "Track".into() } else { title.into() },
                    artist: artist.into(),
                    duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
                    index: pos,
                }
            }).collect();
            // Apply the playlist sort — Custom keeps the saved/manual order.
            let psort = w.get_music_playlist_sort().to_string();
            match psort.as_str() {
                "title"  => rows.sort_by_key(|a| a.title.to_lowercase()),
                "artist" => rows.sort_by_key(|a| a.artist.to_lowercase()),
                _ => {}
            }
            if psort != "custom" && w.get_music_playlist_sort_dir() == "desc" { rows.reverse(); }
            // Cover: custom art (Settings) → else the first track's thumb.
            let cover = playlist_cover_path(playlist_id)
                .map(|p| slint::Image::load_from_path(std::path::Path::new(&p)).unwrap_or_default())
                .or_else(|| rows.first().map(|r| r.thumb.clone()))
                .unwrap_or_default();
            // Persist the playlist in its *displayed* order so playback follows it.
            let id_of_pos: std::collections::HashMap<i32, i64> = pos_of.iter().map(|(id, p)| (*p, *id)).collect();
            let ordered_ids: Vec<i64> = rows.iter().filter_map(|r| id_of_pos.get(&r.index).copied()).collect();
            if let Ok(mut g) = current_playlist().lock() { *g = (playlist_id, ordered_ids); }
            w.set_music_playlist_name(if name.is_empty() { "Playlist".into() } else { name.into() });
            w.set_music_playlist_cover(cover);
            w.set_music_playlist_tracks(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

// id of the podcast whose detail page is currently open.
static CUR_PODCAST_ID: std::sync::OnceLock<std::sync::Mutex<i64>> = std::sync::OnceLock::new();
fn cur_podcast_id() -> &'static std::sync::Mutex<i64> {
    CUR_PODCAST_ID.get_or_init(|| std::sync::Mutex::new(-1))
}
// Bookmark positions (seconds) for the playing audiobook, parallel to the chips.
static BOOK_BOOKMARKS: std::sync::OnceLock<std::sync::Mutex<Vec<f64>>> = std::sync::OnceLock::new();
fn book_bookmarks() -> &'static std::sync::Mutex<Vec<f64>> {
    BOOK_BOOKMARKS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Load the playing audiobook's bookmarks into the UI chips (np.p5.music.audiobook-chapters).
fn load_book_bookmarks(w: &MainWindow, item_id: i64) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let marks = tulipix_music::audiobooks::bookmarks(&pool, item_id).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Ok(mut g) = book_bookmarks().lock() { *g = marks.iter().map(|(p, _)| *p).collect(); }
            let labels: Vec<slint::SharedString> = marks.iter()
                .map(|(p, l)| if l.is_empty() { fmt_clock(*p).into() } else { l.clone().into() }).collect();
            w.set_music_book_bookmarks(slint::ModelRc::new(slint::VecModel::from(labels)));
        });
    });
}

/// Cache an http(s) artwork URL to a local file (keyed by `key`), returning the
/// path. Re-uses an already-downloaded file. Used for podcast/episode thumbs.
async fn cache_artwork(client: &reqwest::Client, key: &str, url: &str) -> Option<std::path::PathBuf> {
    if url.is_empty() || !url.starts_with("http") { return None; }
    let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("podcast_art");
    let _ = std::fs::create_dir_all(&dir);
    let ext = url.rsplit('.').next().filter(|e| e.len() <= 4 && !e.contains('/')).unwrap_or("jpg");
    let dest = dir.join(format!("{key}.{ext}"));
    if dest.exists() { return Some(dest); }
    let bytes = client.get(url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
        .send().await.ok()?.bytes().await.ok()?;
    std::fs::write(&dest, &bytes).ok()?;
    Some(dest)
}

/// Resolve a podcast/show artwork source that may be either an http(s) URL OR a
/// local file path (a user-supplied custom thumbnail). Local existing files are
/// used as-is; URLs go through [`cache_artwork`].
async fn resolve_artwork(client: &reqwest::Client, key: &str, src: &str) -> Option<std::path::PathBuf> {
    if src.is_empty() { return None; }
    let p = std::path::Path::new(src);
    if p.is_file() { return Some(p.to_path_buf()); }
    cache_artwork(client, key, src).await
}

// ── Off-UI-thread artwork decode ─────────────────────────────────────────────
// `slint::Image::load_from_path` decodes on the event loop; podcast/show art
// is routinely 1400×1400 JPEG, so a grid of covers froze input for seconds on
// every section switch. Workers decode into SharedPixelBuffers (memoised by
// path) and the event-loop closure only wraps them — wrapping is O(1).
type ArtPx = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
fn art_px_cache() -> &'static std::sync::Mutex<std::collections::HashMap<PathBuf, ArtPx>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<PathBuf, ArtPx>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Decode `path` (downscaled to ≤512px — card size) off the UI thread.
async fn decode_art_px(path: Option<PathBuf>) -> Option<ArtPx> {
    let path = path?;
    if let Some(hit) = art_px_cache().lock().ok().and_then(|g| g.get(&path).cloned()) {
        return Some(hit);
    }
    let key = path.clone();
    let px = tokio::task::spawn_blocking(move || {
        let img = image::open(&path).ok()?;
        let img = if img.width() > 512 || img.height() > 512 { img.thumbnail(512, 512) } else { img };
        let rgba = img.to_rgba8();
        let (w, h) = rgba.dimensions();
        Some(ArtPx::clone_from_slice(rgba.as_raw(), w, h))
    }).await.ok().flatten()?;
    if let Ok(mut g) = art_px_cache().lock() { g.insert(key, px.clone()); }
    Some(px)
}

/// Wrap a pre-decoded buffer for display. Cheap; safe on the UI thread.
fn art_image(px: &Option<ArtPx>) -> slint::Image {
    px.as_ref().map(|b| slint::Image::from_rgba8(b.clone())).unwrap_or_default()
}

/// Fill the Podcasts grid with subscribed feeds, filtered by the active
/// category, plus the category-chip list (np.p5.music.podcast-feeds).
// Send-safe subscription summary (no slint::Image).
#[derive(Clone)]
struct PodAllData {
    id: i64,
    title: String,
    author: String,
    category: String,
    art: Option<ArtPx>,
    unplayed: i64,
    episodes: i64,
    latest: i64,        // MAX(published) across this show's episodes (Home sort)
    pinned: bool,       // home_pinned — shown on Home "Your shows"
}
const PODCAST_SUB_PAGE: usize = 21;   // Subscribed grid: 3 rows × 7
const PODCAST_HOME_PAGE: usize = 14;  // Home "Your shows": 2 rows × 7
const PODCAST_HOME_MAX_PAGES: usize = 2;  // Home caps at 2 pages; rest live on Subscribed

// ── Trends (hardcoded podcast directory) ──────────────────────────────────
// Feed URLs are baked into the binary from podc.md at the repo root, so the
// Trends page stays populated even after a full podcast-library reset. Edit
// podc.md (one feed URL per line; '#'/blank lines ignored) and rebuild to add
// more. Per-feed metadata (title/author/art/category) is fetched live and
// cached for the session.
const TREND_FEEDS_RAW: &str = include_str!("../../../podc.md");

fn trend_feed_urls() -> Vec<String> {
    TREND_FEEDS_RAW.lines()
        .map(|l| l.trim())
        .filter(|l| l.starts_with("http"))
        .map(|s| s.to_string())
        .collect()
}

#[derive(Clone)]
struct TrendMeta {
    feed_url: String,
    title: String,
    author: String,
    category: String,
    art: Option<std::path::PathBuf>,
}

fn trend_cache() -> &'static std::sync::Mutex<Vec<TrendMeta>> {
    static C: OnceLock<std::sync::Mutex<Vec<TrendMeta>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

const PODCAST_TREND_PAGE: usize = 21;   // Trends grid: 3 rows × 7

fn cur_trend_idx() -> &'static std::sync::Mutex<i32> {
    static C: OnceLock<std::sync::Mutex<i32>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(-1))
}

// Feed URL of the podcast whose info card is open (Trends or Subscribed) — drives
// Save-category / Update-thumb / Subscribe regardless of how it was opened.
fn cur_info_feed() -> &'static std::sync::Mutex<String> {
    static C: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(String::new()))
}

// Per-tab podcast search filters — each tab (and the single-podcast page) keeps
// its own query so search works independently across Home/Trends/Subscribed/
// Downloads and the detail page.
#[derive(Default, Clone)]
struct PodFilters { home: String, trends: String, subs: String, downloads: String, detail: String }
fn pod_filters() -> &'static std::sync::Mutex<PodFilters> {
    static C: OnceLock<std::sync::Mutex<PodFilters>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(PodFilters::default()))
}
fn pod_filter(which: &str) -> String {
    let g = pod_filters().lock().map(|f| f.clone()).unwrap_or_default();
    match which { "home" => g.home, "trends" => g.trends, "subs" => g.subs, "downloads" => g.downloads, "detail" => g.detail, _ => String::new() }
}

/// Render the Trends grid from the session cache: sort (name/category), paginate
/// (21/page), and flag which feeds are already subscribed. No network.
fn render_trends(w: &MainWindow) {
    let weak = w.as_weak();
    let sort = w.get_music_podcast_trends_sort().to_string();
    let page = w.get_music_podcast_trends_page().max(0) as usize;
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let subs: Vec<(String,)> = sqlx::query_as("SELECT feed_url FROM podcasts").fetch_all(&pool).await.unwrap_or_default();
        let subset: std::collections::HashSet<String> = subs.into_iter().map(|(u,)| u).collect();
        let metas = trend_cache().lock().map(|g| g.clone()).unwrap_or_default();
        // Keep the original (feed-order) index for subscribe/info-by-index.
        let mut indexed: Vec<(usize, TrendMeta)> = metas.into_iter().enumerate().collect();
        // Filter (Trends search) — title/author/category.
        let filter = pod_filter("trends");
        let needle = filter.trim().to_lowercase();
        if !needle.is_empty() {
            indexed.retain(|(_, m)| m.title.to_lowercase().contains(&needle)
                || m.author.to_lowercase().contains(&needle)
                || m.category.to_lowercase().contains(&needle));
        }
        match sort.as_str() {
            "category" => indexed.sort_by(|a, b| a.1.category.to_lowercase().cmp(&b.1.category.to_lowercase())
                .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
            "subscribed" => indexed.sort_by(|a, b| subset.contains(&b.1.feed_url).cmp(&subset.contains(&a.1.feed_url))
                .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
            "unsubscribed" => indexed.sort_by(|a, b| subset.contains(&a.1.feed_url).cmp(&subset.contains(&b.1.feed_url))
                .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
            _ => indexed.sort_by_key(|a| a.1.title.to_lowercase()),
        }
        let total = indexed.len();
        let pages = total.div_ceil(PODCAST_TREND_PAGE).max(1);
        let page = page.min(pages - 1);
        let slice: Vec<(usize, TrendMeta)> = indexed.into_iter().skip(page * PODCAST_TREND_PAGE).take(PODCAST_TREND_PAGE).collect();
        // Decode the visible page's art off-thread before touching the UI.
        let mut slice_px: Vec<(usize, TrendMeta, Option<ArtPx>)> = Vec::with_capacity(slice.len());
        for (i, m) in slice {
            let px = decode_art_px(m.art.clone()).await;
            slice_px.push((i, m, px));
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<PodcastTrendCard> = slice_px.iter().map(|(i, m, px)| PodcastTrendCard {
                title: m.title.clone().into(),
                author: m.author.clone().into(),
                category: m.category.clone().into(),
                image: art_image(px),
                feed_url: m.feed_url.clone().into(),
                subscribed: subset.contains(&m.feed_url),
                index: *i as i32,
            }).collect();
            w.set_music_podcast_trends(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_music_podcast_trends_pages(pages as i32);
            w.set_music_podcast_trends_page(page as i32);
        });
    });
}

/// Subscribe to `url`, driving the subscribe progress bar (Trends button /
/// info-card Subscribe) as episodes are stored, then refresh the grids.
fn subscribe_feed_with_progress(weak: slint::Weak<MainWindow>, url: String) {
    tokio::runtime::Handle::current().spawn(async move {
        let finish = |weak: slint::Weak<MainWindow>| { let _ = weak.upgrade_in_event_loop(|w| { w.set_music_podcast_subscribing(false); }); };
        let Ok(pool) = pool_for("podcasts").await else { finish(weak); return; };
        let client = reqwest::Client::new();
        let resp = match client.get(&url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
            Ok(r) => r, Err(_) => { finish(weak); return; } };
        let xml = match resp.text().await { Ok(x) => x, Err(_) => { finish(weak); return; } };
        let feed = tulipix_music::podcasts::parse_feed(&xml);
        if feed.title.is_none() && feed.episodes.is_empty() { finish(weak); return; }
        let cbw = weak.clone();
        let _ = tulipix_music::podcasts::subscribe_with_progress(&pool, &url, &feed, move |done, total| {
            // Throttle UI updates (~40 steps max) so we don't flood the event loop.
            let step = (total / 40).max(1);
            if total > 0 && (done % step == 0 || done >= total) {
                let frac = done as f32 / total as f32;
                let status = format!("Adding… {done}/{total}");
                let _ = cbw.upgrade_in_event_loop(move |w| {
                    w.set_music_podcast_subscribe_frac(frac);
                    w.set_music_podcast_subscribe_status(status.into());
                });
            }
        }).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_subscribe_frac(1.0);
            w.set_music_podcast_subscribing(false);
            w.set_music_podcast_info_subscribed(true);
            render_trends(&w);             // flip the card to ✓ Subscribed
            populate_podcasts(&w);
            populate_podcast_latest(&w);
        });
    });
}

/// Populate the Trends grid. Metadata is persisted in the `podcast_trends` table:
/// loaded from there on every launch (no network), and only feeds NOT yet in the
/// table are fetched + saved. Session-cached after the first build.
fn populate_podcast_trends(w: &MainWindow) {
    let weak = w.as_weak();
    let feeds = trend_feed_urls();
    let cached_now = trend_cache().lock().map(|g| g.len()).unwrap_or(0);
    if cached_now == feeds.len() { render_trends(w); return; }
    w.set_music_podcast_trends_loading(true);
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        // 1. Load whatever's already cached in the DB.
        let mut stored: std::collections::HashMap<String, TrendMeta> = std::collections::HashMap::new();
        if let Ok(pool) = pool_for("podcasts").await {
            let rows: Vec<(String, String, String, String, Option<String>)> = sqlx::query_as(
                "SELECT feed_url, COALESCE(title,''), COALESCE(author,''), COALESCE(category,''), art_path FROM podcast_trends")
                .fetch_all(&pool).await.unwrap_or_default();
            for (feed_url, title, author, category, art_path) in rows {
                let art = art_path.filter(|p| !p.is_empty())
                    .map(std::path::PathBuf::from)
                    .filter(|p| p.exists());
                stored.insert(feed_url.clone(), TrendMeta { feed_url, title, author, category, art });
            }
        }
        // 2. Fetch only the feeds missing from the DB (concurrently), then persist.
        let missing: Vec<(usize, String)> = feeds.iter().enumerate()
            .filter(|(_, u)| !stored.contains_key(*u)).map(|(i, u)| (i, u.clone())).collect();
        let handles: Vec<_> = missing.into_iter().map(|(i, url)| {
            let client = client.clone();
            tokio::spawn(async move {
                let (mut title, mut author, mut category, mut art) = (String::new(), String::new(), String::new(), None);
                if let Ok(resp) = client.get(&url).header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT).send().await {
                    if let Ok(xml) = resp.text().await {
                        let feed = tulipix_music::podcasts::parse_feed(&xml);
                        title = feed.title.unwrap_or_default();
                        author = feed.author;
                        category = feed.category;
                        art = resolve_artwork(&client, &format!("trend-{i}"), &feed.image_url).await;
                    }
                }
                if title.is_empty() { title = url.clone(); }
                if category.is_empty() { category = "Other".to_string(); }
                TrendMeta { feed_url: url, title, author, category, art }
            })
        }).collect();
        let mut fetched: Vec<TrendMeta> = Vec::new();
        for h in handles { if let Ok(m) = h.await { fetched.push(m); } }
        // Persist the newly fetched rows.
        if !fetched.is_empty() {
            if let Ok(pool) = pool_for("podcasts").await {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
                for m in &fetched {
                    let art_s = m.art.as_ref().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
                    let _ = sqlx::query(
                        "INSERT INTO podcast_trends (feed_url, title, author, category, art_path, fetched_at)
                         VALUES (?,?,?,?,?,?)
                         ON CONFLICT(feed_url) DO UPDATE SET title=excluded.title, author=excluded.author,
                            category=excluded.category, art_path=excluded.art_path, fetched_at=excluded.fetched_at")
                        .bind(&m.feed_url).bind(&m.title).bind(&m.author).bind(&m.category).bind(&art_s).bind(now)
                        .execute(&pool).await;
                }
            }
            for m in fetched { stored.insert(m.feed_url.clone(), m); }
        }
        // 3. Build the cache in feed (podc.md) order.
        let metas: Vec<TrendMeta> = feeds.iter().filter_map(|u| stored.get(u).cloned()).collect();
        if let Ok(mut g) = trend_cache().lock() { *g = metas; }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_trends_loading(false);
            render_trends(&w);
        });
    });
}

fn populate_podcasts(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let filter = pod_filter("subs");
        let like = format!("%{}%", filter.trim());
        let base = "SELECT p.id, COALESCE(p.title, p.feed_url), COALESCE(p.author,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), COALESCE(p.category,''),
                    (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id AND COALESCE(e.played,0) = 0),
                    (SELECT COUNT(*) FROM podcast_episodes e WHERE e.podcast_id = p.id),
                    (SELECT COALESCE(MAX(e.published),0) FROM podcast_episodes e WHERE e.podcast_id = p.id),
                    COALESCE(p.home_pinned,0)
             FROM podcasts p";
        let rows: Vec<(i64, String, String, String, String, i64, i64, i64, i64)> = if filter.trim().is_empty() {
            sqlx::query_as(&format!("{base} ORDER BY p.title COLLATE NOCASE")).fetch_all(&pool).await.unwrap_or_default()
        } else {
            sqlx::query_as(&format!("{base} WHERE p.title LIKE ?1 OR p.author LIKE ?1 OR p.category LIKE ?1 OR p.feed_url LIKE ?1
                    OR EXISTS (SELECT 1 FROM podcast_episodes e WHERE e.podcast_id = p.id AND e.title LIKE ?1)
                 ORDER BY p.title COLLATE NOCASE"))
                .bind(&like).fetch_all(&pool).await.unwrap_or_default()
        };
        let client = reqwest::Client::new();
        let mut all: Vec<PodAllData> = Vec::with_capacity(rows.len());
        for (id, title, author, img, category, unplayed, episodes, latest, pinned) in rows {
            let art_path = resolve_artwork(&client, &format!("pod-{id}"), &img).await;
            let art = decode_art_px(art_path).await;
            all.push(PodAllData { id, title, author, category, art, unplayed, episodes, latest, pinned: pinned != 0 });
        }
        let _ = weak.upgrade_in_event_loop(move |w| render_podcast_cards(&w, &all));
    });
}

/// Build the Subscribed (filtered + 21/page) and Home (14/page) card models
/// from the full subscription list on the UI thread.
fn render_podcast_cards(w: &MainWindow, all: &[PodAllData]) {
    let to_card = |i: usize, d: &PodAllData| PodcastCard {
        id: d.id as i32,
        title: d.title.clone().into(),
        author: d.author.clone().into(),
        category: d.category.clone().into(),
        image: art_image(&d.art),
        unplayed: d.unplayed as i32,
        episodes: d.episodes as i32,
        index: i as i32,
        home_pinned: d.pinned,
    };
    w.set_music_podcast_total(all.len() as i32);
    // Categories.
    let mut cats: Vec<String> = vec!["All".into()];
    for d in all { if !d.category.is_empty() && !cats.contains(&d.category) { cats.push(d.category.clone()); } }
    let cat_models: Vec<slint::SharedString> = cats.iter().map(|c| c.clone().into()).collect();
    w.set_music_podcast_categories(slint::ModelRc::new(slint::VecModel::from(cat_models)));
    // Subscribed — category filter + 21/page.
    let active = w.get_music_podcast_cat().to_string();
    let filtered: Vec<(usize, &PodAllData)> = all.iter().enumerate()
        .filter(|(_, d)| active == "All" || d.category == active).collect();
    let sub_pages = filtered.len().div_ceil(PODCAST_SUB_PAGE).max(1);
    let sub_page = (w.get_music_podcast_sub_page().max(0) as usize).min(sub_pages - 1);
    let sub_cards: Vec<PodcastCard> = filtered.iter().skip(sub_page * PODCAST_SUB_PAGE).take(PODCAST_SUB_PAGE)
        .map(|(i, d)| to_card(*i, d)).collect();
    w.set_music_podcast_sub_pages(sub_pages as i32);
    w.set_music_podcast_sub_page(sub_page as i32);
    w.set_music_podcast_cards(slint::ModelRc::new(slint::VecModel::from(sub_cards)));
    // Home "Your shows" — only shows the user pinned (home_pinned), capped at 2
    // pages; the rest live on the Subscribed tab. 14/page, sortable.
    let home_sort = w.get_music_podcast_home_sort().to_string();
    let mut home_order: Vec<(usize, &PodAllData)> = all.iter().enumerate().filter(|(_, d)| d.pinned).collect();
    match home_sort.as_str() {
        "category" => home_order.sort_by(|a, b| a.1.category.to_lowercase().cmp(&b.1.category.to_lowercase())
            .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
        "latest" => home_order.sort_by(|a, b| b.1.latest.cmp(&a.1.latest)
            .then(a.1.title.to_lowercase().cmp(&b.1.title.to_lowercase()))),
        _ => home_order.sort_by_key(|a| a.1.title.to_lowercase()),
    }
    let home_total = home_order.len().min(PODCAST_HOME_PAGE * PODCAST_HOME_MAX_PAGES);  // cap to 2 pages
    let home_pages = home_total.div_ceil(PODCAST_HOME_PAGE).max(1);
    let home_page = (w.get_music_podcast_home_page().max(0) as usize).min(home_pages - 1);
    let home_cards: Vec<PodcastCard> = home_order.iter().take(home_total)
        .skip(home_page * PODCAST_HOME_PAGE).take(PODCAST_HOME_PAGE)
        .map(|(i, d)| to_card(*i, d)).collect();
    w.set_music_podcast_home_pages(home_pages as i32);
    w.set_music_podcast_home_page(home_page as i32);
    w.set_music_podcast_home_cards(slint::ModelRc::new(slint::VecModel::from(home_cards)));
}

/// Open a feed's detail page by podcast id — header + episode list.
fn open_podcast(w: &MainWindow, pid_i32: i32) {
    let pid = pid_i32 as i64;
    if pid < 0 { return; }
    if let Ok(mut g) = cur_podcast_id().lock() { *g = pid; }
    // Clear the previous feed's header + episodes synchronously so opening a
    // different podcast never flashes the old one while the new page loads.
    w.set_music_podcast_d_title("Loading…".into());
    w.set_music_podcast_d_author("".into());
    w.set_music_podcast_d_category("".into());
    w.set_music_podcast_d_desc("".into());
    w.set_music_podcast_d_desc_short("".into());
    w.set_music_podcast_d_image(slint::Image::default());
    w.set_music_podcast_d_episodes(slint::ModelRc::new(slint::VecModel::from(Vec::<PodcastEpisodeRow>::new())));
    w.set_music_podcast_detail_open(true);
    w.set_music_podcast_d_page(0);
    w.set_music_podcast_d_id(pid_i32);
    load_podcast_detail(w, pid);
}

const PODCAST_PAGE: i64 = 15;

/// (Re)load the header + a 20-episode page for podcast `pid` into the detail
/// view, ordered by the chosen sort (newest/oldest first).
fn load_podcast_detail(w: &MainWindow, pid: i64) {
    let sort = w.get_music_podcast_d_sort().to_string();
    let page = w.get_music_podcast_d_page().max(0) as i64;
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let head: Option<(String, String, String, String, String, i64)> = sqlx::query_as(
            "SELECT COALESCE(title, feed_url), COALESCE(author,''), COALESCE(category,''), COALESCE(description,''), COALESCE(NULLIF(custom_image,''), image_url, ''), COALESCE(home_pinned,0)
             FROM podcasts WHERE id = ?").bind(pid).fetch_optional(&pool).await.ok().flatten();
        let filter = pod_filter("detail");
        let like = format!("%{}%", filter.trim());
        let has_f = !filter.trim().is_empty();
        let extra = if has_f { " AND title LIKE ?".to_string() } else { String::new() };
        let total: i64 = {
            let cq = format!("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?{extra}");
            let mut qb = sqlx::query_scalar(&cq).bind(pid);
            if has_f { qb = qb.bind(like.clone()); }
            qb.fetch_one(&pool).await.unwrap_or(0)
        };
        let pages = ((total + PODCAST_PAGE - 1) / PODCAST_PAGE).max(1);
        let page = page.min(pages - 1);
        let order = if sort == "old" { "ASC" } else { "DESC" };
        let q = format!(
            "SELECT id, COALESCE(title,''), audio_url, published, duration_s, COALESCE(image_url,''), downloaded_path, COALESCE(played,0)
             FROM podcast_episodes WHERE podcast_id = ?{extra}
             ORDER BY COALESCE(published,0) {order}, id {order} LIMIT ? OFFSET ?");
        let mut qb = sqlx::query_as(&q).bind(pid);
        if has_f { qb = qb.bind(like); }
        let eps: Vec<(i64, String, String, Option<i64>, Option<f64>, String, Option<String>, i64)> =
            qb.bind(PODCAST_PAGE).bind(page * PODCAST_PAGE)
                .fetch_all(&pool).await.unwrap_or_default();
        let client = reqwest::Client::new();
        // Only the show artwork is fetched here (usually already cached from the
        // grid). Per-episode thumbs are NOT fetched on the detail page — fetching
        // 20 images serially was the main cause of the slow open; every row falls
        // back to the show art, which is what most podcast apps show anyway.
        let head_art = if let Some((_, _, _, _, img, _)) = &head { resolve_artwork(&client, &format!("pod-{pid}"), img).await } else { None };
        let head_px = decode_art_px(head_art).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Some((title, author, category, desc, _, pinned)) = &head {
                w.set_music_podcast_d_title(title.clone().into());
                w.set_music_podcast_d_author(author.clone().into());
                w.set_music_podcast_d_category(category.clone().into());
                w.set_music_podcast_d_desc(desc.clone().into());
                // Info-card copy caps at 200 chars — the ⓘ Info popup keeps the
                // full text (char-boundary safe for multi-byte scripts).
                let short = if desc.chars().count() > 200 {
                    let mut s: String = desc.chars().take(200).collect();
                    s.push('…');
                    s
                } else { desc.clone() };
                w.set_music_podcast_d_desc_short(short.into());
                w.set_music_podcast_d_pinned(*pinned != 0);
            }
            w.set_music_podcast_d_image(art_image(&head_px));
            w.set_music_podcast_d_total(total as i32);
            w.set_music_podcast_d_pages(pages as i32);
            w.set_music_podcast_d_page(page as i32);
            let queued = podcast_dl_queue().lock().map(|g| g.clone()).unwrap_or_default();
            let rows: Vec<PodcastEpisodeRow> = eps.iter().enumerate().map(|(i, (id, title, _url, pub_, dur, _img, dl, played))| PodcastEpisodeRow {
                id: *id as i32,
                title: title.clone().into(),
                show: slint::SharedString::new(),
                date: pub_.map(fmt_date).unwrap_or_default().into(),
                duration: dur.map(|d| fmt_clock(d).into()).unwrap_or_default(),
                // Show artwork for every row (per-episode thumbs skipped for speed).
                image: art_image(&head_px),
                played: *played != 0,
                downloaded: dl.is_some(),
                queued: queued.contains(&(*id as i32)),
                dlinfo: slint::SharedString::new(),
                index: i as i32,
            }).collect();
            w.set_music_podcast_d_episodes(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

// Send-safe intermediate (no slint::Image) for cross-thread episode rows.
#[derive(Clone)]
struct EpRowData {
    id: i32,
    title: String,
    show: String,
    date: String,
    duration: String,
    art: Option<ArtPx>,
    played: bool,
    downloaded: bool,
    dlinfo: String,    // "12 Jun · 14:32" — when the offline copy was stored
}

/// Build the Home (Latest) feed — newest 14, one episode per show. Episode art
/// falls back to the show's feed artwork.
fn populate_podcast_latest(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let filter = pod_filter("home");
        let like = format!("%{}%", filter.trim());
        let extra = if filter.trim().is_empty() { "" } else { " AND (e.title LIKE ?1 OR p.title LIKE ?1)" };
        let q = format!(
            "SELECT e.id, COALESCE(e.title,''), e.audio_url, e.published, e.duration_s,
                    COALESCE(e.image_url,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), p.id, e.downloaded_path, COALESCE(e.played,0), COALESCE(p.title,''), e.downloaded_at
             FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id
             WHERE e.id = (SELECT e2.id FROM podcast_episodes e2 WHERE e2.podcast_id = e.podcast_id
                           ORDER BY COALESCE(e2.published,0) DESC, e2.id DESC LIMIT 1){extra}
             ORDER BY COALESCE(e.published,0) DESC, e.id DESC LIMIT 14");
        let mut qb = sqlx::query_as(&q);
        if !filter.trim().is_empty() { qb = qb.bind(like); }
        let eps: Vec<EpQueryRow> = qb.fetch_all(&pool).await.unwrap_or_default();
        let data = build_episode_data(eps).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_latest(slint::ModelRc::new(slint::VecModel::from(rows_from_data(&data))));
        });
    });
}

const PODCAST_DL_PAGE: i64 = 20;

/// Build the Downloads tab — cached episodes, sorted + paginated (20/page).
/// Pending episode downloads (FIFO) — clicks beyond the active one wait here.
fn podcast_dl_queue() -> &'static std::sync::Mutex<std::collections::VecDeque<i32>> {
    static C: OnceLock<std::sync::Mutex<std::collections::VecDeque<i32>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// True while a drain worker is alive (one worker, sequential downloads).
static PODCAST_DL_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Download ONE episode to the offline cache, driving the header progress bar
/// (dl-id / dl-frac / dl-title). Returns when the file is stored or failed.
async fn podcast_download_one(weak: slint::Weak<MainWindow>, id: i32) {
    let Ok(pool) = pool_for("podcasts").await else { return; };
    let row: Option<(String, Option<String>, String)> = sqlx::query_as(
        "SELECT audio_url, downloaded_path, COALESCE(title,'') FROM podcast_episodes WHERE id = ?")
        .bind(id as i64).fetch_optional(&pool).await.ok().flatten();
    let Some((url, dl, title)) = row else { return; };
    if url.is_empty() || dl.is_some() { return; }
    let dir = tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("podcast_offline");
    let _ = std::fs::create_dir_all(&dir);
    let ext = url.split('?').next().unwrap_or(&url).rsplit('.').next()
        .filter(|e| e.len() <= 4 && !e.contains('/')).unwrap_or("mp3").to_string();
    let dest = dir.join(format!("ep-{id}.{ext}"));
    // Mark this row as the in-flight download.
    let wk = weak.clone();
    let _ = wk.upgrade_in_event_loop(move |w| {
        w.set_music_podcast_dl_id(id);
        w.set_music_podcast_dl_frac(0.0);
        w.set_music_podcast_dl_title(title.into());
    });
    let client = reqwest::Client::new();
    let ok = {
        use std::io::Write;
        let mut got: u64 = 0;
        let mut last_pct: i32 = -1;
        let result: Option<()> = async {
            let mut resp = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                .send().await.ok()?;
            let total = resp.content_length();
            let mut file = std::fs::File::create(&dest).ok()?;
            while let Some(chunk) = resp.chunk().await.ok()? {
                file.write_all(&chunk).ok()?;
                got += chunk.len() as u64;
                if let Some(t) = total {
                    if t > 0 {
                        let pct = ((got as f64 / t as f64) * 100.0) as i32;
                        if pct != last_pct {
                            last_pct = pct;
                            let frac = (got as f64 / t as f64) as f32;
                            let wk = weak.clone();
                            let _ = wk.upgrade_in_event_loop(move |w| w.set_music_podcast_dl_frac(frac));
                        }
                    }
                }
            }
            Some(())
        }.await;
        result.is_some()
    };
    if ok {
        let _ = tulipix_music::podcasts::mark_downloaded(&pool, id as i64, dest.to_string_lossy().as_ref()).await;
    } else {
        let _ = std::fs::remove_file(&dest);
    }
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_podcast_dl_id(-1);
        w.set_music_podcast_dl_frac(0.0);
        w.set_music_podcast_dl_title("".into());
        refresh_podcast_views(&w);
    });
}

fn populate_podcast_downloads(w: &MainWindow) {
    let weak = w.as_weak();
    let sort = w.get_music_podcast_dl_sort().to_string();
    let page = w.get_music_podcast_dl_page().max(0) as i64;
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("podcasts").await else { return; };
        let filter = pod_filter("downloads");
        let like = format!("%{}%", filter.trim());
        let has_f = !filter.trim().is_empty();
        let extra = if has_f { " AND (e.title LIKE ? OR p.title LIKE ?)".to_string() } else { String::new() };
        let total: i64 = {
            let cq = format!("SELECT COUNT(*) FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id WHERE e.downloaded_path IS NOT NULL{extra}");
            let mut qb = sqlx::query_scalar(&cq);
            if has_f { qb = qb.bind(like.clone()).bind(like.clone()); }
            qb.fetch_one(&pool).await.unwrap_or(0)
        };
        let pages = ((total + PODCAST_DL_PAGE - 1) / PODCAST_DL_PAGE).max(1);
        let page = page.min(pages - 1);
        // "dl" (default) = most recently downloaded first ("dl-asc" flips it);
        // new/old = by publish date.
        let order_by = match sort.as_str() {
            "old" => "COALESCE(e.published,0) ASC, e.id ASC",
            "new" => "COALESCE(e.published,0) DESC, e.id DESC",
            "dl-asc" => "COALESCE(e.downloaded_at, e.published, 0) ASC, e.id ASC",
            _ => "COALESCE(e.downloaded_at, e.published, 0) DESC, e.id DESC",
        };
        let q = format!(
            "SELECT e.id, COALESCE(e.title,''), e.audio_url, e.published, e.duration_s,
                    COALESCE(e.image_url,''), COALESCE(NULLIF(p.custom_image,''), p.image_url, ''), p.id, e.downloaded_path, COALESCE(e.played,0), COALESCE(p.title,''), e.downloaded_at
             FROM podcast_episodes e JOIN podcasts p ON p.id = e.podcast_id
             WHERE e.downloaded_path IS NOT NULL{extra}
             ORDER BY {order_by} LIMIT ? OFFSET ?");
        let mut qb = sqlx::query_as(&q);
        if has_f { qb = qb.bind(like.clone()).bind(like); }
        let eps: Vec<EpQueryRow> =
            qb.bind(PODCAST_DL_PAGE).bind(page * PODCAST_DL_PAGE).fetch_all(&pool).await.unwrap_or_default();
        let data = build_episode_data(eps).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_podcast_dl_pages(pages as i32);
            w.set_music_podcast_dl_page(page as i32);
            w.set_music_podcast_downloads(slint::ModelRc::new(slint::VecModel::from(rows_from_data(&data))));
        });
    });
}

// Cross-show episode query row: id, title, audio_url, published, duration_s,
// episode_image, show_image, podcast_id, downloaded_path, played, show_title,
// downloaded_at (offline-copy timestamp).
type EpQueryRow = (i64, String, String, Option<i64>, Option<f64>, String, String, i64, Option<String>, i64, String, Option<i64>);

/// Off-thread: cache artwork + flatten a cross-show episode query into Send data.
async fn build_episode_data(eps: Vec<EpQueryRow>) -> Vec<EpRowData> {
    let client = reqwest::Client::new();
    let mut out = Vec::with_capacity(eps.len());
    for (id, title, _url, pub_, dur, ep_img, show_img, pid, dl, played, show, dl_at) in eps {
        // Prefer the episode's own image; else the show artwork — keyed `pod-{id}`
        // so it reuses the file the grid already cached (instant, never blank).
        let art = if !ep_img.is_empty() { cache_artwork(&client, &format!("ep-{id}"), &ep_img).await } else { None };
        let art = match art {
            Some(p) => Some(p),
            None if !show_img.is_empty() => resolve_artwork(&client, &format!("pod-{pid}"), &show_img).await,
            None => None,
        };
        let art = decode_art_px(art).await;
        out.push(EpRowData {
            id: id as i32,
            title,
            show,
            date: pub_.map(fmt_date).unwrap_or_default(),
            duration: dur.map(fmt_clock).unwrap_or_default(),
            art,
            played: played != 0,
            downloaded: dl.is_some(),
            dlinfo: dl_at.map(fmt_dl_stamp).unwrap_or_default(),
        });
    }
    out
}

/// Short local download stamp — "12 Jun · 14:32".
fn fmt_dl_stamp(epoch: i64) -> String {
    use chrono::{Local, TimeZone, Datelike, Timelike};
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match Local.timestamp_opt(epoch, 0).single() {
        Some(dt) => format!("{} {} · {:02}:{:02}", dt.day(), MON[(dt.month0() as usize).min(11)], dt.hour(), dt.minute()),
        None => String::new(),
    }
}

/// On the UI thread: turn Send data into UI rows (loads slint::Image).
fn rows_from_data(data: &[EpRowData]) -> Vec<PodcastEpisodeRow> {
    let queued = podcast_dl_queue().lock().map(|g| g.clone()).unwrap_or_default();
    data.iter().enumerate().map(|(i, d)| PodcastEpisodeRow {
        id: d.id,
        title: d.title.clone().into(),
        show: d.show.clone().into(),
        date: d.date.clone().into(),
        duration: d.duration.clone().into(),
        image: art_image(&d.art),
        played: d.played,
        downloaded: d.downloaded,
        queued: queued.contains(&d.id),
        dlinfo: d.dlinfo.clone().into(),
        index: i as i32,
    }).collect()
}

/// Repopulate whichever podcast surface is currently visible.
fn refresh_podcast_views(w: &MainWindow) {
    match w.get_music_podcast_tab().as_str() {
        "home" => { populate_podcasts(w); populate_podcast_latest(w); }
        "downloads" => populate_podcast_downloads(w),
        _ => populate_podcasts(w),
    }
    if w.get_music_podcast_detail_open() {
        let pid = cur_podcast_id().lock().map(|g| *g).unwrap_or(-1);
        if pid >= 0 { load_podcast_detail(w, pid); }
    }
}

/// Fill the audiobooks view with `is_audiobook` library tracks (np.p5.music.audiobook-chapters).
/// Folder basename → book title.
fn book_title(folder: &str) -> String {
    std::path::Path::new(folder).file_name()
        .map(|s| s.to_string_lossy().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| folder.to_string())
}

/// Seconds → "8h 12m" / "47m" pretty duration for book cards.
fn fmt_hm(secs: f64) -> String {
    let total = secs.max(0.0) as i64;
    let h = total / 3600;
    let m = (total % 3600) / 60;
    if h > 0 { format!("{}h {}m", h, m) } else { format!("{}m", m) }
}

/// In-memory maps used to turn library positions into title/artist/duration +
/// cover art (shared by the audiobook card + detail builders).
fn music_pos_maps() -> (std::collections::HashMap<i64, i32>, std::collections::HashMap<i32, (String, String, f64)>) {
    let pos_of = music_ids().lock()
        .map(|g| g.iter().enumerate().map(|(i, id)| (*id, i as i32)).collect()).unwrap_or_default();
    let by_pos = music_songs().lock()
        .map(|g| g.iter().map(|s| (s.pos, (s.title.clone(), s.artist.clone(), s.duration_s))).collect()).unwrap_or_default();
    (pos_of, by_pos)
}

/// Per-book decoded cover, keyed by folder — filled by `populate_audiobooks`,
/// read synchronously by the detail hero.
fn ab_cover_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, ArtPx>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<String, ArtPx>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Audiobook cover (np.p5.music.audiobook-chapters): a cover/folder image in
/// the book's directory wins; otherwise the album art embedded in the first
/// chapter's tags, extracted once via ffmpeg into the cache. Decoded
/// off-thread; None keeps the 📚 monogram.
async fn audiobook_cover_px(folder: &str, custom: Option<PathBuf>, first_chapter: Option<PathBuf>) -> Option<ArtPx> {
    // A user-chosen cover (audiobook_covers table) outranks everything and
    // bypasses the per-folder cache so a change shows immediately.
    if let Some(c) = custom.filter(|p| p.is_file()) {
        let px = decode_art_px(Some(c)).await?;
        if let Ok(mut g) = ab_cover_cache().lock() { g.insert(folder.to_string(), px.clone()); }
        return Some(px);
    }
    if let Some(hit) = ab_cover_cache().lock().ok().and_then(|g| g.get(folder).cloned()) {
        return Some(hit);
    }
    let dir = PathBuf::from(folder);
    let mut found: Option<PathBuf> = None;
    for name in ["cover.jpg", "cover.jpeg", "cover.png", "folder.jpg", "folder.png",
                 "Cover.jpg", "Cover.png", "Folder.jpg", "front.jpg"] {
        let p = dir.join(name);
        if p.is_file() { found = Some(p); break; }
    }
    // No loose art file → pull the embedded album art out of the first chapter.
    if found.is_none() {
        if let (Some(chapter), Some(base)) = (first_chapter, dirs_default()) {
            let out_dir = base.join("cache").join("abcover");
            let _ = std::fs::create_dir_all(&out_dir);
            use sha2::{Digest, Sha256};
            let mut h = Sha256::new();
            h.update(folder.as_bytes());
            let stem: String = h.finalize().iter().take(8).map(|b| format!("{b:02x}")).collect();
            let out = out_dir.join(format!("{stem}.png"));
            if !out.exists() {
                let ffmpeg = tulipix_core::thumbs::tool_bin("ffmpeg");
                let outc = out.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    std::process::Command::new(&ffmpeg)
                        .args(["-y", "-loglevel", "quiet", "-i"]).arg(&chapter)
                        .args(["-map", "0:v:0", "-frames:v", "1"]).arg(&outc)
                        .status()
                }).await;
            }
            if out.exists() { found = Some(out); }
        }
    }
    let px = decode_art_px(found).await?;
    if let Ok(mut g) = ab_cover_cache().lock() { g.insert(folder.to_string(), px.clone()); }
    Some(px)
}

/// Fill the audiobook detail hero + chapter list for one folder.
fn fill_book_detail(w: &MainWindow, folder: &str, ids: &[i64]) {
    let (pos_of, by_pos) = music_pos_maps();
    let tiles = w.get_music_tiles();
    let mut total = 0.0;
    let mut first_pos = -1;
    let mut rows: Vec<ChapterRow> = Vec::new();
    for id in ids {
        let Some(&pos) = pos_of.get(id) else { continue; };
        if first_pos < 0 { first_pos = pos; }
        let (title, _artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
        total += dur;
        let n = rows.len() + 1;
        rows.push(ChapterRow {
            title: if title.is_empty() { format!("Chapter {}", n).into() } else { title.into() },
            duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
            index: pos,
            played: false,
        });
    }
    // Real book cover (folder image / embedded art) decoded by the cards
    // populate; tile thumb only as the last resort.
    let cover = ab_cover_cache().lock().ok()
        .and_then(|g| g.get(folder).cloned())
        .map(slint::Image::from_rgba8)
        .or_else(|| {
            if first_pos >= 0 && (first_pos as usize) < tiles.row_count() {
                tiles.row_data(first_pos as usize).map(|t| t.thumb)
            } else { None }
        })
        .unwrap_or_default();
    w.set_music_ab_d_title(book_title(folder).into());
    w.set_music_ab_d_author("".into());
    w.set_music_ab_d_cover(cover);
    w.set_music_ab_d_total(fmt_hm(total).into());
    w.set_music_ab_d_chapters(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_ab_d_resume_index(first_pos);
    w.set_music_audiobook_detail_open(true);
}

fn populate_audiobooks(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // Reflect folder→section assignments: flag every audiobook-section
        // folder so its (now-scanned) tracks show up grouped below.
        for (folder, key) in load_folder_sections() {
            if key == "audiobooks" {
                // Metadata-gated flag (whole-folder fallback). Re-applied here so
                // it self-heals once the async tag ingest has filled genre/container.
                let _ = tulipix_music::audiobooks::flag_audiobook_folder(&pool, &folder).await;
            }
        }
        let ids: Vec<i64> = sqlx::query_scalar(
            "SELECT item_id FROM track_meta WHERE is_audiobook = 1")
            .fetch_all(&pool).await.unwrap_or_default();
        // User-chosen covers (np.p5.music.audiobook-chapters — custom art).
        let _ = sqlx::query("CREATE TABLE IF NOT EXISTS audiobook_covers (folder TEXT PRIMARY KEY, path TEXT NOT NULL)")
            .execute(&pool).await;
        let custom_covers: std::collections::HashMap<String, String> =
            sqlx::query_as("SELECT folder, path FROM audiobook_covers")
                .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        // Per-chapter resume positions → book status (in-progress / finished).
        let progress: std::collections::HashMap<i64, f64> = sqlx::query_as(
            "SELECT item_id, position_s FROM audiobook_progress")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        // One (folder, ordered chapter ids) entry per book card. Covers come
        // from custom art / a folder image / embedded album art, decoded off
        // the UI thread.
        let books = tulipix_music::audiobooks::book_folders(&pool).await.unwrap_or_default();
        let (pos_of, by_pos) = music_pos_maps();
        let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
        // (folder, chapter ids, total seconds, cover, resume 0..1, finished)
        let mut book_data: Vec<(String, Vec<i64>, f64, Option<ArtPx>, f32, bool)> =
            Vec::with_capacity(books.len());
        for (folder, _n) in &books {
            let cids = tulipix_music::audiobooks::book_chapters(&pool, folder).await.unwrap_or_default();
            let total: f64 = cids.iter()
                .filter_map(|id| pos_of.get(id))
                .filter_map(|pos| by_pos.get(pos).map(|(_, _, d)| *d))
                .sum();
            let first_path = cids.first()
                .and_then(|id| pos_of.get(id))
                .and_then(|&pos| paths.get(pos as usize).cloned());
            let custom = custom_covers.get(folder).map(PathBuf::from);
            let cover = audiobook_cover_px(folder, custom, first_path).await;
            // Furthest chapter with a saved position drives the resume bar;
            // "finished" = saved position ≥90% through the LAST chapter.
            let n = cids.len().max(1);
            let mut resume = 0.0f32;
            let mut finished = false;
            for (i, id) in cids.iter().enumerate() {
                let Some(&pos_s) = progress.get(id) else { continue; };
                let dur = pos_of.get(id).and_then(|p| by_pos.get(p)).map(|(_, _, d)| *d).unwrap_or(0.0);
                let frac_in = if dur > 1.0 { (pos_s / dur).clamp(0.0, 1.0) } else { 0.0 };
                resume = resume.max((i as f64 + frac_in) as f32 / n as f32);
                if i == n - 1 && frac_in >= 0.9 { finished = true; }
            }
            book_data.push((folder.clone(), cids, total, cover, resume, finished));
        }
        let tab = ab_tab().lock().map(|g| g.clone()).unwrap_or_default();
        // Folders view rows: "name · N chapters — /path".
        let folder_rows: Vec<String> = book_data.iter()
            .map(|(f, c, ..)| format!("{}   ·   {} chapters   —   {}", book_title(f), c.len(), f))
            .collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let tiles = w.get_music_tiles();
            let rows: Vec<MusicSongRow> = ids.iter().filter_map(|id| pos_of.get(id).copied()).map(|pos| {
                let (title, artist, dur) = by_pos.get(&pos).cloned().unwrap_or_default();
                MusicSongRow {
                    thumb: if pos >= 0 && (pos as usize) < tiles.row_count() { tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default() } else { slint::Image::default() },
                    title: if title.is_empty() { "Track".into() } else { title.into() },
                    artist: artist.into(),
                    duration: if dur > 0.0 { fmt_clock(dur).into() } else { "".into() },
                    index: pos,
                }
            }).collect();
            w.set_music_audiobooks(slint::ModelRc::new(slint::VecModel::from(rows)));
            let cards: Vec<BookCard> = book_data.iter()
                .filter(|(.., resume, finished)| match tab.as_str() {
                    "progress" => *resume > 0.0 && !finished,
                    "finished" => *finished,
                    _ => true,
                })
                .map(|(folder, cids, total, cover, resume, _)| BookCard {
                    id: folder.clone().into(),
                    title: book_title(folder).into(),
                    author: "".into(),
                    cover: art_image(cover),
                    chapters: cids.len() as i32,
                    total_time: fmt_hm(*total).into(),
                    resume_frac: *resume,
                }).collect();
            w.set_music_audiobook_cards(slint::ModelRc::new(slint::VecModel::from(cards)));
            let frows: Vec<slint::SharedString> = folder_rows.into_iter().map(Into::into).collect();
            w.set_music_ab_folders(slint::ModelRc::new(slint::VecModel::from(frows)));
        });
    });
}

/// Active Audiobooks sub-tab: all | progress | finished | folders.
fn ab_tab() -> &'static std::sync::Mutex<String> {
    static C: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new("all".to_string()))
}

/// Folder of the audiobook whose detail page is open (custom-cover target).
fn cur_book_folder() -> &'static std::sync::Mutex<String> {
    static C: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// File-picker → persist a custom cover for `folder` in audiobook_covers, then
/// refresh the cards, the open detail hero, and the now-playing art if a
/// chapter of this book is on the vinyl right now.
fn audiobook_pick_cover(weak: slint::Weak<MainWindow>, folder: String) {
    if folder.is_empty() { return; }
    tokio::runtime::Handle::current().spawn(async move {
        let Some(file) = rfd::AsyncFileDialog::new()
            .add_filter("Images", &["png", "jpg", "jpeg", "webp", "bmp"])
            .set_title("Choose audiobook cover")
            .pick_file().await else { return; };
        let path = file.path().to_path_buf();
        let Ok(pool) = pool_for("music").await else { return; };
        let _ = sqlx::query("CREATE TABLE IF NOT EXISTS audiobook_covers (folder TEXT PRIMARY KEY, path TEXT NOT NULL)")
            .execute(&pool).await;
        let _ = sqlx::query(
            "INSERT INTO audiobook_covers (folder, path) VALUES (?,?)
             ON CONFLICT(folder) DO UPDATE SET path = excluded.path")
            .bind(&folder).bind(path.to_string_lossy().as_ref()).execute(&pool).await;
        // Drop the stale decode and rebuild cards + the open detail hero.
        if let Ok(mut g) = ab_cover_cache().lock() { g.remove(&folder); }
        let ids = tulipix_music::audiobooks::book_chapters(&pool, &folder).await.unwrap_or_default();
        let px = audiobook_cover_px(&folder, Some(path), None).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            populate_audiobooks(&w);
            if w.get_music_audiobook_detail_open() {
                fill_book_detail(&w, &folder, &ids);
            }
            // Live vinyl art swap if this book is currently playing.
            let np = w.get_music_np_index();
            let np_folder = music_paths().lock().ok()
                .and_then(|g| g.get(np as usize).and_then(|p| p.parent().map(|d| d.display().to_string())));
            if np_folder.as_deref() == Some(folder.as_str()) {
                if let Some(px) = px { w.set_music_np_art(slint::Image::from_rgba8(px)); }
            }
        });
    });
}

/// Stream an arbitrary audio URL via a fresh headless mpv (podcast episodes).
/// Mirrors `play_music_at` minus the library-position bookkeeping.
fn play_music_url(w: &MainWindow, url: &str, title: &str) {
    if let Ok(mut g) = yt_cur_audio().lock() { g.clear(); }
    w.set_music_yt_now_video(false);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
    let sock = mpv_ipc::endpoint("tulipix-music");
    mpv_ipc::cleanup(&sock);
    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
    cmd.arg("--no-video").arg("--force-window=no").arg("--idle=no")
        .arg(format!("--input-ipc-server={}", sock.display()))
        .arg(format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32));
    // Carry the mute state across track changes (each track is a fresh mpv).
    if w.get_music_muted() { cmd.arg("--mute=yes"); }
    cmd.arg(format!("--af={}", music_full_af(&music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10])))));
    stop_video(); // music takes over the universal stream from any video
    mpv_die_with_parent(&mut cmd);
    match cmd.arg(url).spawn() {
        Ok(child) => { if let Ok(mut g) = music_proc().lock() { *g = Some(child); } }
        Err(e) => { tracing::error!(error = %e, "mpv stream launch failed"); return; }
    }
    if let Ok(mut g) = music_sock().lock() { *g = Some(sock.clone()); }
    let weak = w.as_weak();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        if let Ok(mut stream) = mpv_ipc::connect(&sock) {
            let _ = stream.write_all(concat!(
                "{\"command\":[\"observe_property\",1,\"time-pos\"]}\n",
                "{\"command\":[\"observe_property\",2,\"duration\"]}\n",
                "{\"command\":[\"observe_property\",3,\"pause\"]}\n").as_bytes());
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] != "property-change" { continue; }
                let name = v["name"].as_str().unwrap_or("").to_string();
                let wk = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = wk.upgrade() else { return; };
                    match name.as_str() {
                        "time-pos" => if let Some(d) = v["data"].as_f64() { w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into()); }
                        "duration" => if let Some(d) = v["data"].as_f64() { w.set_music_dur(d as f32); w.set_music_dur_label(fmt_clock(d).into()); }
                        "pause" => if let Some(p) = v["data"].as_bool() { w.set_music_playing(!p); }
                        _ => {}
                    }
                });
            }
        }
        if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) == my_gen {
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_playing(false));
        }
    });
    w.set_music_np_title(title.into());
    w.set_music_np_sub("Podcast".into());
    w.set_music_np_album("".into());      // no stale artist·album on the second line
    w.set_music_np_art(slint::Image::default());
    w.set_music_radio_np_uuid("".into()); // a non-radio stream ends any LIVE state
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("0:00".into());
}

// ── Internet radio (np.p4.music.radio) ──────────────────────────────────────
// Curated India-first presets + radio-browser search; favourites/recents live
// in radio.db; playback is a headless mpv whose ICY `media-title` becomes the
// live now-playing line.

/// The full station list behind the open radio page. Row `index` fields point
/// into THIS Vec, so sorting/paging the view never breaks play/fav targets.
fn radio_list() -> &'static std::sync::Mutex<Vec<tulipix_music::radio::Station>> {
    static C: OnceLock<std::sync::Mutex<Vec<tulipix_music::radio::Station>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Favourite uuids backing the heart flags of the current list.
fn radio_favs() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// Downloaded favicon paths by station uuid — survives re-renders (sort/page).
fn radio_icons() -> &'static std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<String, std::path::PathBuf>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// Monotonic fetch generation — a slow response from a previous genre/search
/// can never overwrite a newer list (nor can its favicon updates).
static RADIO_FETCH_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn radio_quality(codec: &str, bitrate: u32) -> String {
    let codec_ok = !codec.is_empty() && codec != "UNKNOWN";
    match (codec_ok, bitrate) {
        (false, 0) => String::new(),
        (false, b) => format!("{b}k"),
        (true, 0) => codec.to_uppercase(),
        (true, b) => format!("{} · {b}k", codec.to_uppercase()),
    }
}

/// Install a NEW station list (resets pagination) and render it.
fn radio_render(w: &MainWindow, stations: Vec<tulipix_music::radio::Station>, favs: &std::collections::HashSet<String>) {
    if let Ok(mut g) = radio_list().lock() { *g = stations; }
    if let Ok(mut g) = radio_favs().lock() { *g = favs.clone(); }
    w.set_music_radio_fav_page(0);
    radio_rerender(w);
}

/// Re-render the current list through the active sort (+ direction) and the
/// 20/page pagination on category pages and Favourites. Row `index` stays the
/// position in `radio_list`.
fn radio_rerender(w: &MainWindow) {
    let list = radio_list().lock().map(|g| g.clone()).unwrap_or_default();
    let favs = radio_favs().lock().map(|g| g.clone()).unwrap_or_default();
    let icons = radio_icons().lock().map(|g| g.clone()).unwrap_or_default();
    let mut order: Vec<usize> = (0..list.len()).collect();
    // ▲ asc / ▼ desc are literal: A→Z / Z→A for Name, low→high / high→low for
    // Bitrate; for Top, desc = most-voted first (the fetch order).
    let asc = w.get_music_radio_sort_dir().as_str() == "asc";
    match w.get_music_radio_sort().as_str() {
        "name" => { order.sort_by_key(|&i| list[i].name.trim().to_lowercase()); if !asc { order.reverse(); } }
        "bitrate" => { order.sort_by_key(|&i| list[i].bitrate); if !asc { order.reverse(); } }
        _ => { if asc { order.reverse(); } }
    }
    let paged = w.get_music_radio_cat_open()
        || w.get_music_radio_tab().as_str() == "favourites";
    let (pages, page) = if paged {
        let pages = order.len().div_ceil(20).max(1);
        (pages, (w.get_music_radio_fav_page().max(0) as usize).min(pages - 1))
    } else { (1, 0) };
    w.set_music_radio_fav_pages(pages as i32);
    w.set_music_radio_fav_page(page as i32);
    let slice: &[usize] = if paged { &order[page * 20..((page + 1) * 20).min(order.len())] } else { &order };
    let rows: Vec<RadioStation> = slice.iter().map(|&oi| {
        let s = &list[oi];
        let (icon, has_icon) = icons.get(&s.stationuuid)
            .and_then(|p| slint::Image::load_from_path(p).ok())
            .map_or((slint::Image::default(), false), |im| (im, true));
        RadioStation {
            uuid: s.stationuuid.clone().into(),
            name: s.name.trim().into(),
            initial: s.name.trim().chars().next()
                .map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "♪".into()).into(),
            tags: s.tags.split(',').map(str::trim).filter(|t| !t.is_empty())
                .take(3).collect::<Vec<_>>().join(" · ").into(),
            country: s.country.clone().into(),
            quality: radio_quality(&s.codec, s.bitrate).into(),
            icon, has_icon,
            fav: favs.contains(&s.stationuuid),
            index: oi as i32,
        }
    }).collect();
    w.set_music_radio_stations(slint::ModelRc::new(slint::VecModel::from(rows)));
}

/// Background favicon pass — fills tiles in as each icon lands. Reuses the
/// podcast artwork cache; updates are dropped once a newer fetch supersedes.
fn radio_fetch_icons(weak: slint::Weak<MainWindow>, stations: Vec<tulipix_music::radio::Station>, fgen: u64) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).build() else { return; };
        for s in stations.iter() {
            if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
            // Slint has no ICO decoder — skip those favicons.
            if s.favicon.is_empty() || s.favicon.to_lowercase().ends_with(".ico") { continue; }
            let key = format!("radio-{}", s.stationuuid.replace(|c: char| !c.is_ascii_alphanumeric(), "-"));
            let Some(path) = cache_artwork(&client, &key, &s.favicon).await else { continue; };
            // Stations lie about favicon formats (ICO bytes behind a .png URL) —
            // sniff the magic and drop undecodable files instead of letting the
            // image loader error on every render.
            let magic_ok = std::fs::read(&path).map(|b|
                b.starts_with(&[0x89, b'P', b'N', b'G']) || b.starts_with(&[0xFF, 0xD8])
                || b.starts_with(b"GIF8") || (b.len() > 11 && &b[8..12] == b"WEBP")
                || b.starts_with(b"<?xml") || b.starts_with(b"<svg")).unwrap_or(false);
            if !magic_ok { let _ = std::fs::remove_file(&path); continue; }
            if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
            let uuid = s.stationuuid.clone();
            let _ = weak.upgrade_in_event_loop(move |w| {
                // Remember the path (survives sort/page re-renders), then patch
                // the visible row by uuid — the view may be sorted/paged, so
                // model position ≠ fetch position.
                if let Ok(mut g) = radio_icons().lock() { g.insert(uuid.clone(), path.clone()); }
                let model = w.get_music_radio_stations();
                for r in 0..model.row_count() {
                    let Some(mut row) = model.row_data(r) else { continue; };
                    if row.uuid == uuid.as_str() {
                        if let Ok(img) = slint::Image::load_from_path(&path) {
                            row.icon = img; row.has_icon = true;
                            model.set_row_data(r, row);
                        }
                        break;
                    }
                }
            });
        }
    });
}

/// Fetch a radio-browser station list (browse preset or free-text search).
fn radio_fetch(weak: slint::Weak<MainWindow>, url: String) {
    let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _ = weak.upgrade_in_event_loop(|w| { w.set_music_radio_busy(true); w.set_music_radio_status("".into()); });
    tokio::runtime::Handle::current().spawn(async move {
        let favs: std::collections::HashSet<String> = match pool_for("radio").await {
            Ok(pool) => tulipix_music::radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect(),
            Err(_) => Default::default(),
        };
        let res = async {
            let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(15)).build()?;
            let list: Vec<tulipix_music::radio::Station> = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                .send().await?.error_for_status()?.json().await?;
            anyhow::Ok(list)
        }.await;
        if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
        match res {
            Ok(mut list) => {
                // radio-browser carries many duplicate registrations of the same
                // stream — keep the top-voted copy (the list arrives votes-desc).
                let mut seen = std::collections::HashSet::new();
                list.retain(|s| seen.insert(s.name.trim().to_lowercase()));
                let icons = list.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |w| {
                    w.set_music_radio_busy(false);
                    radio_render(&w, list, &favs);
                });
                radio_fetch_icons(weak, icons, fgen);
            }
            Err(e) => {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_music_radio_busy(false);
                    radio_render(&w, Vec::new(), &Default::default());
                    w.set_music_radio_status(format!("radio-browser unreachable — {e}").into());
                });
            }
        }
    });
}

/// Fill the grid from radio.db — `which` is "favourites" or "recent".
fn radio_show_saved(weak: slint::Weak<MainWindow>, which: &'static str) {
    let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let _ = weak.upgrade_in_event_loop(|w| { w.set_music_radio_busy(true); w.set_music_radio_status("".into()); });
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("radio").await else { return; };
        let list = match which {
            "recent" => tulipix_music::radio::recents(&pool, 50).await.unwrap_or_default(),
            _ => tulipix_music::radio::favourites(&pool).await.unwrap_or_default(),
        };
        let favs: std::collections::HashSet<String> =
            tulipix_music::radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect();
        if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
        let icons = list.clone();
        let _ = weak.clone().upgrade_in_event_loop(move |w| {
            w.set_music_radio_busy(false);
            radio_render(&w, list, &favs);
        });
        radio_fetch_icons(weak, icons, fgen);
    });
}

/// Re-populate the open page for whichever radio tab is active. Home shows the
/// category tiles (no list); categories load from the radio.db cache.
fn radio_reload_tab(w: &MainWindow) {
    match w.get_music_radio_tab().as_str() {
        "favourites" => radio_show_saved(w.as_weak(), "favourites"),
        "recent" => radio_show_saved(w.as_weak(), "recent"),
        _ => {
            if w.get_music_radio_cat_open() {
                radio_open_category(w, w.get_music_radio_genre().max(0) as usize);
            } else {
                radio_render(w, Vec::new(), &Default::default());
            }
        }
    }
}

/// Refresh the per-category station counts on the Home tiles. When the cache
/// is completely empty (first run) and `auto_refresh` is set, kicks off a full
/// Refresh so the section self-populates on first open.
fn radio_load_counts(weak: slint::Weak<MainWindow>, auto_refresh: bool) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("radio").await else { return; };
        let counts = tulipix_music::radio::cache_counts(&pool).await.unwrap_or_default();
        let total: i64 = counts.iter().map(|(_, n)| n).sum();
        let by_preset: std::collections::HashMap<String, i64> = counts.into_iter().collect();
        let row: Vec<i32> = tulipix_music::radio::PRESETS.iter()
            .map(|(_, q)| *by_preset.get(*q).unwrap_or(&0) as i32).collect();
        let _ = weak.clone().upgrade_in_event_loop(move |w| {
            w.set_music_radio_genre_counts(slint::ModelRc::new(slint::VecModel::from(row)));
            // Header pill on Home — every cached station across all categories.
            w.set_music_radio_total(total as i32);
            if total == 0 && auto_refresh && !w.get_music_radio_refresh_busy() {
                radio_refresh_all(&w);
            }
        });
    });
}

/// Open one curated category as a station list — cache-first; falls back to a
/// one-off network fetch (which seeds the cache) when the preset was never
/// refreshed.
fn radio_open_category(w: &MainWindow, i: usize) {
    let Some(&(label, q)) = tulipix_music::radio::PRESETS.get(i) else { return; };
    w.set_music_radio_genre(i as i32);
    w.set_music_radio_cat_open(true);
    w.set_music_radio_cat_title(label.into());
    let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    w.set_music_radio_busy(true);
    w.set_music_radio_status("".into());
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("radio").await else { return; };
        let mut list = tulipix_music::radio::load_cache(&pool, q).await.unwrap_or_default();
        if list.is_empty() {
            // Never refreshed — one network fetch seeds this preset's cache.
            if let Ok(fetched) = radio_fetch_preset(q).await {
                let _ = tulipix_music::radio::save_cache(&pool, q, &fetched).await;
                list = fetched;
            }
        }
        let favs: std::collections::HashSet<String> =
            tulipix_music::radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect();
        if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
        let icons = list.clone();
        let empty = list.is_empty();
        let _ = weak.clone().upgrade_in_event_loop(move |w| {
            w.set_music_radio_busy(false);
            if empty { w.set_music_radio_status("Nothing cached for this category — hit ↻ Refresh stations.".into()); }
            radio_render(&w, list, &favs);
        });
        radio_fetch_icons(weak, icons, fgen);
    });
}

/// One preset fetch: votes-ordered, hidebroken, name-deduped.
async fn radio_fetch_preset(q: &str) -> Result<Vec<tulipix_music::radio::Station>> {
    let client = reqwest::Client::builder().timeout(std::time::Duration::from_secs(20)).build()?;
    let mut list: Vec<tulipix_music::radio::Station> = client
        .get(tulipix_music::radio::browse_url(q, 60))
        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
        .send().await?.error_for_status()?.json().await?;
    let mut seen = std::collections::HashSet::new();
    list.retain(|s| seen.insert(s.name.trim().to_lowercase()));
    Ok(list)
}

/// "Refresh stations" — re-fetch EVERY curated category from radio-browser in
/// parallel, replacing the whole cache. The progress pill fills per preset.
fn radio_refresh_all(w: &MainWindow) {
    if w.get_music_radio_refresh_busy() { return; }
    w.set_music_radio_refresh_busy(true);
    w.set_music_radio_refresh_frac(0.0);
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let total = tulipix_music::radio::PRESETS.len();
        let done = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut handles = Vec::with_capacity(total);
        for &(_, q) in tulipix_music::radio::PRESETS {
            let done = done.clone();
            let weak = weak.clone();
            handles.push(tokio::spawn(async move {
                if let Ok(list) = radio_fetch_preset(q).await {
                    if let Ok(pool) = pool_for("radio").await {
                        let _ = tulipix_music::radio::save_cache(&pool, q, &list).await;
                    }
                }
                let n = done.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                let frac = n as f32 / total as f32;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_radio_refresh_frac(frac));
            }));
        }
        for h in handles { let _ = h.await; }
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_music_radio_refresh_busy(false);
            w.set_music_radio_refresh_frac(0.0);
            radio_load_counts(w.as_weak(), false);
            // An open category page re-reads its freshly replaced cache.
            if w.get_music_radio_cat_open() { radio_reload_tab(&w); }
        });
    });
}

/// Play a live radio stream — same headless-mpv path as [`play_music_url`] but
/// observing `media-title`: Shoutcast/Icecast ICY metadata carries
/// "Artist - Song" for most stations and becomes the now-playing title live.
fn play_radio(w: &MainWindow, st: &tulipix_music::radio::Station) {
    if let Ok(mut g) = yt_cur_audio().lock() { g.clear(); }
    w.set_music_yt_now_video(false);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
    let sock = mpv_ipc::endpoint("tulipix-music");
    mpv_ipc::cleanup(&sock);
    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
    cmd.arg("--no-video").arg("--force-window=no").arg("--idle=no")
        .arg(format!("--input-ipc-server={}", sock.display()))
        .arg(format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32));
    if w.get_music_muted() { cmd.arg("--mute=yes"); }
    cmd.arg(format!("--af={}", music_full_af(&music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10])))));
    // Live cushion: buffer ~10s before starting so transient network dips eat
    // the cache instead of stuttering (we run ~10s behind the live edge).
    cmd.arg("--cache=yes").arg("--cache-secs=30")
        .arg("--cache-pause-initial=yes").arg("--cache-pause-wait=10")
        .arg("--demuxer-readahead-secs=30");
    stop_video();
    mpv_die_with_parent(&mut cmd);
    match cmd.arg(&st.url).spawn() {
        Ok(child) => { if let Ok(mut g) = music_proc().lock() { *g = Some(child); } }
        Err(e) => { tracing::error!(error = %e, "mpv radio launch failed"); return; }
    }
    if let Ok(mut g) = music_sock().lock() { *g = Some(sock.clone()); }
    let station_name = st.name.trim().to_string();
    let stream_url = st.url.clone();
    let weak = w.as_weak();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        if let Ok(mut stream) = mpv_ipc::connect(&sock) {
            let _ = stream.write_all(concat!(
                "{\"command\":[\"observe_property\",1,\"time-pos\"]}\n",
                "{\"command\":[\"observe_property\",3,\"pause\"]}\n",
                "{\"command\":[\"observe_property\",5,\"media-title\"]}\n").as_bytes());
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] != "property-change" { continue; }
                let name = v["name"].as_str().unwrap_or("").to_string();
                let wk = weak.clone();
                let su = stream_url.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = wk.upgrade() else { return; };
                    match name.as_str() {
                        // Live stream: no duration — the position label shows time on air.
                        "time-pos" => if let Some(d) = v["data"].as_f64() { w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into()); }
                        "pause" => if let Some(p) = v["data"].as_bool() { w.set_music_playing(!p); }
                        "media-title" => if let Some(t) = v["data"].as_str() {
                            let t = t.trim();
                            // mpv reports the URL until the first ICY update — ignore those.
                            if !t.is_empty() && t != su && !t.starts_with("http") {
                                w.set_music_np_title(t.into());
                            }
                        }
                        _ => {}
                    }
                });
            }
        }
        if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) == my_gen {
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_music_playing(false);
                w.set_music_radio_np_uuid("".into());
            });
        }
    });
    w.set_music_player_mode("radio".into());
    w.set_music_radio_np_uuid(st.stationuuid.clone().into());
    w.set_music_radio_np_initial(station_name.chars().next()
        .map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "♪".into()).into());
    w.set_music_np_title(station_name.into());
    w.set_music_np_sub("📻 Internet Radio · LIVE".into());
    w.set_music_np_album("".into());      // no stale artist·album on the second line
    w.set_music_np_art(slint::Image::default());
    w.set_music_np_accent(slint::Color::from_rgb_u8(0x14, 0xb8, 0xa6));
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("LIVE".into());
    w.invoke_music_center_mini();
}

/// Register every radio callback + the curated genre chips.
fn wire_radio(window: &MainWindow) {
    use tulipix_music::radio;
    // Tile labels split into emoji + name — the Home cards show the icon big
    // above the name (the full label stays the category page title).
    let icons: Vec<slint::SharedString> = radio::PRESETS.iter()
        .map(|(l, _)| l.split_whitespace().next().unwrap_or("📻").into()).collect();
    let names: Vec<slint::SharedString> = radio::PRESETS.iter()
        .map(|(l, _)| l.split_once(' ').map_or(*l, |(_, n)| n).trim().into()).collect();
    window.set_music_radio_genres(slint::ModelRc::new(slint::VecModel::from(names)));
    window.set_music_radio_genre_icons(slint::ModelRc::new(slint::VecModel::from(icons)));

    let w = window.as_weak();
    window.on_music_radio_set_tab(move |t| {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_radio_cat_open(false); // tabs always leave any open list page
        w0.set_music_radio_tab(t);
        radio_reload_tab(&w0);
        if w0.get_music_radio_tab().as_str() == "home" { radio_load_counts(w.clone(), false); }
    });
    let w = window.as_weak();
    window.on_music_radio_set_genre(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        radio_open_category(&w0, i.max(0) as usize);
    });
    let w = window.as_weak();
    window.on_music_radio_back(move || {
        let Some(w0) = w.upgrade() else { return; };
        w0.set_music_radio_cat_open(false);
        w0.set_music_radio_tab("home".into());
        radio_render(&w0, Vec::new(), &Default::default());
        radio_load_counts(w.clone(), false);
    });
    let w = window.as_weak();
    window.on_music_radio_refresh(move || {
        let Some(w0) = w.upgrade() else { return; };
        radio_refresh_all(&w0);
    });
    let w = window.as_weak();
    window.on_music_radio_set_sort(move |s| {
        let Some(w0) = w.upgrade() else { return; };
        // Re-tapping the active sort flips its direction; switching sorts
        // resets to that sort's natural face (Name A→Z, Bitrate/Top high first).
        if w0.get_music_radio_sort() == s {
            let flipped = if w0.get_music_radio_sort_dir().as_str() == "asc" { "desc" } else { "asc" };
            w0.set_music_radio_sort_dir(flipped.into());
        } else {
            w0.set_music_radio_sort_dir(if s.as_str() == "name" { "asc" } else { "desc" }.into());
            w0.set_music_radio_sort(s);
        }
        w0.set_music_radio_fav_page(0);
        radio_rerender(&w0);
    });
    let w = window.as_weak();
    window.on_music_radio_fav_set_page(move |d| {
        let Some(w0) = w.upgrade() else { return; };
        let next = (w0.get_music_radio_fav_page() + d).clamp(0, (w0.get_music_radio_fav_pages() - 1).max(0));
        w0.set_music_radio_fav_page(next);
        radio_rerender(&w0);
    });
    let w = window.as_weak();
    window.on_music_radio_clear_recent(move || {
        if w.upgrade().is_none() { return; }
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("radio").await {
                let _ = tulipix_music::radio::clear_recents(&pool).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                if w.get_music_radio_tab().as_str() == "recent" { radio_reload_tab(&w); }
            });
        });
    });
    let w = window.as_weak();
    window.on_music_radio_search(move |q| {
        let Some(w0) = w.upgrade() else { return; };
        let q = q.trim().to_string();
        if q.is_empty() {
            w0.set_music_radio_cat_open(false);
            radio_reload_tab(&w0);
        } else if q.len() >= 2 {
            // Universal search — every cached category + saved stations in one
            // pass (offline); empty cache results fall back to radio-browser.
            w0.set_music_radio_tab("home".into());
            w0.set_music_radio_cat_open(true);
            w0.set_music_radio_cat_title(format!("🔍 “{q}” — all stations").into());
            let fgen = RADIO_FETCH_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
            w0.set_music_radio_busy(true);
            w0.set_music_radio_status("".into());
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("radio").await else { return; };
                let list = radio::search_cache(&pool, &q).await.unwrap_or_default();
                if RADIO_FETCH_GEN.load(std::sync::atomic::Ordering::SeqCst) != fgen { return; }
                if list.is_empty() {
                    // Nothing cached matches — go to the network.
                    let _ = weak.clone().upgrade_in_event_loop(move |w| {
                        radio_fetch(w.as_weak(), radio::search_url(&q, 60));
                    });
                    return;
                }
                let favs: std::collections::HashSet<String> =
                    radio::favourite_uuids(&pool).await.unwrap_or_default().into_iter().collect();
                let icons = list.clone();
                let _ = weak.clone().upgrade_in_event_loop(move |w| {
                    w.set_music_radio_busy(false);
                    radio_render(&w, list, &favs);
                });
                radio_fetch_icons(weak, icons, fgen);
            });
        }
    });
    let w = window.as_weak();
    window.on_music_radio_play(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let idx = i.max(0) as usize;
        let Some(st) = radio_list().lock().ok().and_then(|g| g.get(idx).cloned()) else { return; };
        play_radio(&w0, &st);
        // The grid favicon doubles as now-playing art.
        if let Some(row) = w0.get_music_radio_stations().row_data(idx) {
            if row.has_icon { w0.set_music_np_art(row.icon); }
        }
        tokio::runtime::Handle::current().spawn(async move {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64).unwrap_or(0);
            if let Ok(pool) = pool_for("radio").await {
                let _ = radio::touch_played(&pool, &st, now).await;
            }
            // Popularity click-ping (radio-browser etiquette) — fire and forget.
            if !st.stationuuid.starts_with("custom:") {
                if let Ok(client) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(8)).build() {
                    let _ = client.post(radio::click_url(&st.stationuuid))
                        .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                        .send().await;
                }
            }
        });
    });
    let w = window.as_weak();
    window.on_music_radio_fav(move |i| {
        let Some(w0) = w.upgrade() else { return; };
        let idx = i.max(0) as usize;
        let Some(st) = radio_list().lock().ok().and_then(|g| g.get(idx).cloned()) else { return; };
        // `i` indexes radio_list, NOT the (possibly sorted/paged) model — flip
        // the heart in the favs set and re-render so the right row updates.
        let now_fav = radio_favs().lock().map(|mut g| {
            if g.contains(&st.stationuuid) { g.remove(&st.stationuuid); false }
            else { g.insert(st.stationuuid.clone()); true }
        }).unwrap_or(false);
        radio_rerender(&w0);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("radio").await else { return; };
            let _ = if now_fav { radio::add_favourite(&pool, &st).await }
                    else { radio::remove_favourite(&pool, &st.stationuuid).await };
            // Unhearting while ON the favourites tab removes the card.
            let _ = weak.upgrade_in_event_loop(|w| {
                if w.get_music_radio_tab().as_str() == "favourites" { radio_reload_tab(&w); }
            });
        });
    });
    let w = window.as_weak();
    window.on_music_radio_add_save(move || {
        let Some(w0) = w.upgrade() else { return; };
        let name = w0.get_music_radio_add_name().trim().to_string();
        let url = w0.get_music_radio_add_url().trim().to_string();
        if url.is_empty() { return; }
        let st = radio::custom_station(if name.is_empty() { &url } else { &name }, &url);
        w0.set_music_radio_add_open(false);
        let weak = w.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("radio").await {
                let _ = radio::add_favourite(&pool, &st).await;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_music_radio_tab("favourites".into());
                radio_reload_tab(&w);
            });
        });
    });
}

/// Persist one `music.*` preference into Settings.advanced.
fn save_music_pref(key: &str, val: &str) {
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert(key.into(), val.into());
    let _ = s.save();
}

/// Read one `music.*` preference from Settings.advanced.
fn load_music_pref(key: &str) -> Option<String> {
    tulipix_core::settings::Settings::load().ok().and_then(|s| s.advanced.get(key).cloned())
}

/// The item id the tag editor targets — explicit context-menu target, else the
/// now-playing track.
fn editing_target(w: &MainWindow) -> Option<i64> {
    tag_edit_target().lock().ok().and_then(|g| *g).or_else(|| current_music_id(w))
}

/// The album-art thumbnail for a playback position (from the live tiles model).
fn tile_thumb_at(w: &MainWindow, pos: i32) -> slint::Image {
    use slint::Model;
    if pos < 0 { return slint::Image::default(); }
    let tiles = w.get_music_tiles();
    if (pos as usize) < tiles.row_count() {
        tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default()
    } else { slint::Image::default() }
}

/// Fill the tag-editor dropdown + stored fields (release date / genre / album
/// artist / credits / lock) for an item, async (np.p5.music.tag-editor).
fn prefill_tag_editor(weak: &slint::Weak<MainWindow>, id: i64) {
    let weak = weak.clone();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(&pool).await;
        let row: Option<(Option<String>, Option<String>, Option<String>, Option<String>, i64)> = sqlx::query_as(
            "SELECT release_date, genre, album_artist, credits, COALESCE(user_locked,0) FROM track_meta WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        let opts: Vec<String> = sqlx::query_scalar(
            "SELECT DISTINCT genre FROM track_meta WHERE genre IS NOT NULL AND genre != '' ORDER BY genre")
            .fetch_all(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            if let Some((d, g, aa, cr, lk)) = row {
                if let Some(d) = d { w.set_music_tag_date(d.into()); }
                if let Some(g) = g { w.set_music_tag_genre(g.into()); }
                if let Some(aa) = aa { w.set_music_tag_album_artist(aa.into()); }
                if let Some(cr) = cr { w.set_music_tag_credits(cr.into()); }
                w.set_music_tag_locked(lk != 0);
            }
            let opts: Vec<slint::SharedString> = opts.into_iter().map(|s| s.into()).collect();
            w.set_music_genre_options(slint::ModelRc::new(slint::VecModel::from(opts)));
        });
    });
}

/// Recompute the highlighted lyric line for the current playhead + offset.
fn update_lyrics_active(w: &MainWindow) {
    let t_ms = (w.get_music_pos() as f64 * 1000.0) as i64 - w.get_music_lyrics_offset_ms() as i64;
    let (active, prog) = music_lyrics_lines().lock().ok().map(|g| {
        let a = tulipix_music::lyrics::active_line(&g, t_ms);
        let p = a.map(|i| {
            let start = g[i].0;
            let end = g.get(i + 1).map(|l| l.0).unwrap_or(start + 4000);
            (((t_ms - start) as f64) / ((end - start).max(1) as f64)).clamp(0.0, 1.0) as f32
        }).unwrap_or(0.0);
        (a.map(|i| i as i32).unwrap_or(-1), p)
    }).unwrap_or((-1, 0.0));
    if w.get_music_lyrics_active() != active { w.set_music_lyrics_active(active); }
    w.set_music_lyrics_line_progress(prog); // karaoke wipe within the current line
}

/// mpv `--key=value` args for the persisted audio config (device / exclusive /
/// gapless / replaygain). Applied at launch in `play_music_at`.
fn music_audio_args(s: &tulipix_core::settings::Settings) -> Vec<String> {
    use tulipix_music::player::{AudioConfig, ReplayGainMode};
    let cfg = AudioConfig {
        gapless: s.advanced.get("music.gapless").map(|v| v != "0").unwrap_or(true),
        crossfade_s: s.advanced.get("music.crossfade").and_then(|v| v.parse().ok()).unwrap_or(0.0),
        replaygain: match s.advanced.get("music.replaygain").map(|v| v.as_str()) {
            Some("track") => ReplayGainMode::Track,
            Some("album") => ReplayGainMode::Album,
            _ => ReplayGainMode::Off,
        },
        preamp_db: 0.0,
    };
    let mut args = cfg.mpv_options();
    let device = s.advanced.get("music.device").cloned().unwrap_or_else(|| "auto".into());
    let exclusive = s.advanced.get("music.exclusive").map(|v| v == "1").unwrap_or(false);
    args.extend(tulipix_music::output_device::device_options(&device, exclusive));
    args
}

/// Enumerate audio output devices via `mpv --audio-device=help` (np.p5.music.output).
fn enumerate_audio_devices() -> Vec<tulipix_music::output_device::AudioDevice> {
    let out = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv")).arg("--audio-device=help").output();
    match out {
        Ok(o) => {
            let txt = String::from_utf8_lossy(&o.stdout);
            let mut d = tulipix_music::output_device::parse_device_list(&txt);
            if d.is_empty() { d.push(tulipix_music::output_device::default_device()); }
            d
        }
        Err(_) => vec![tulipix_music::output_device::default_device()],
    }
}

/// SSDP M-SEARCH for UPnP MediaRenderers; collects responses for ~2 s
/// (np.p5.music.cast). Runs on a worker thread (blocking socket).
/// One-track HTTP server for the cast handoff: serves exactly `path` on an
/// ephemeral port so the renderer can pull the bytes. Each cast replaces the
/// served file; the listener thread lives for the app's lifetime. Returns the
/// URL the renderer should fetch. No Range support — fine for play/stop v1.
static CAST_SERVE: std::sync::OnceLock<std::sync::Mutex<Option<(u16, std::path::PathBuf)>>> = std::sync::OnceLock::new();
fn cast_serve_url(path: &std::path::Path) -> Option<String> {
    let state = CAST_SERVE.get_or_init(|| std::sync::Mutex::new(None));
    let mut g = state.lock().ok()?;
    let port = match &*g {
        Some((port, _)) => *port,
        None => {
            let listener = std::net::TcpListener::bind("0.0.0.0:0").ok()?;
            let port = listener.local_addr().ok()?.port();
            std::thread::Builder::new().name("tulipix-cast-http".into()).spawn(move || {
                for stream in listener.incoming().flatten() {
                    let served = CAST_SERVE.get().and_then(|s| s.lock().ok().and_then(|g| g.clone()));
                    let Some((_, path)) = served else { continue };
                    let mut stream = stream;
                    std::thread::spawn(move || {
                        use std::io::{Read, Write};
                        let mut req = [0u8; 1024];
                        let _ = stream.read(&mut req);
                        let head_only = req.starts_with(b"HEAD");
                        let Ok(bytes) = std::fs::read(&path) else {
                            let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                            return;
                        };
                        let mime = match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
                            "mp3" => "audio/mpeg", "flac" => "audio/flac", "ogg" | "oga" => "audio/ogg",
                            "m4a" | "aac" => "audio/mp4", "wav" => "audio/wav", "opus" => "audio/opus",
                            _ => "application/octet-stream",
                        };
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: {mime}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            bytes.len());
                        let _ = stream.write_all(hdr.as_bytes());
                        if !head_only { let _ = stream.write_all(&bytes); }
                    });
                }
            }).ok()?;
            port
        }
    };
    *g = Some((port, path.to_path_buf()));
    // LAN-reachable local address: route-probe via UDP connect (no packets sent).
    let ip = std::net::UdpSocket::bind("0.0.0.0:0").ok()
        .and_then(|s| s.connect("8.8.8.8:80").ok().and_then(|_| s.local_addr().ok()))
        .map(|a| a.ip().to_string())?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("track");
    let enc: String = name.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' => (b as char).to_string(),
        _ => format!("%{b:02X}"),
    }).collect();
    Some(format!("http://{ip}:{port}/{enc}"))
}

/// The actual cast handoff (np.p5.music.cast): serve the current track over
/// HTTP, then SOAP SetAVTransportURI + Play at the renderer's AVTransport
/// control URL. Local mpv stops — the renderer owns playback.
fn cast_current_track(w: &MainWindow, device_name: &str) {
    let idx = w.get_music_np_index();
    let Some(path) = music_paths().lock().ok().and_then(|g| g.get(idx as usize).cloned()) else {
        w.set_music_cast_status("Play a track first, then pick a renderer.".into());
        return;
    };
    let Some(dev) = cast_targets().lock().ok()
        .and_then(|g| g.iter().find(|d| d.name == device_name).cloned()) else { return; };
    let Some(media_url) = cast_serve_url(std::path::Path::new(&path)) else {
        w.set_music_cast_status("Could not start the local stream server.".into());
        return;
    };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        let desc = match client.get(&dev.location).send().await {
            Ok(r) => r.text().await.unwrap_or_default(),
            Err(e) => { tracing::warn!(error = %e, "cast: description fetch failed"); String::new() }
        };
        let Some(ctl) = tulipix_music::cast::parse_control_url(&desc) else {
            let _ = weak.upgrade_in_event_loop(|w| w.set_music_cast_status("Renderer has no AVTransport service.".into()));
            return;
        };
        let ctl = tulipix_music::cast::resolve_url(&dev.location, &ctl);
        for (action, body) in [
            ("SetAVTransportURI", tulipix_music::cast::soap_set_uri(0, &media_url)),
            ("Play", tulipix_music::cast::soap_play(0)),
        ] {
            let ok = client.post(&ctl)
                .header("SOAPACTION", tulipix_music::cast::soap_action_header(action))
                .header(reqwest::header::CONTENT_TYPE, "text/xml; charset=\"utf-8\"")
                .body(body).send().await
                .map(|r| r.status().is_success()).unwrap_or(false);
            if !ok {
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_cast_status(format!("Renderer refused {action}.").into()));
                return;
            }
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            // Renderer owns playback now — stop the local pipeline.
            stop_music(&w);
            w.set_music_cast_status(format!("Casting to {}.", dev.name).into());
        });
    });
}

fn discover_cast_devices() -> Vec<tulipix_music::cast::CastDevice> {
    use std::net::UdpSocket;
    let mut out: Vec<tulipix_music::cast::CastDevice> = Vec::new();
    let Ok(sock) = UdpSocket::bind("0.0.0.0:0") else { return out; };
    let _ = sock.set_read_timeout(Some(std::time::Duration::from_millis(600)));
    let msg = tulipix_music::cast::ssdp_msearch();
    let _ = sock.send_to(msg.as_bytes(), "239.255.255.250:1900");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut buf = [0u8; 2048];
    while std::time::Instant::now() < deadline {
        match sock.recv_from(&mut buf) {
            Ok((n, _)) => {
                let raw = String::from_utf8_lossy(&buf[..n]);
                if let Some(dev) = tulipix_music::cast::parse_ssdp_response(&raw) {
                    if !out.iter().any(|d| d.location == dev.location) { out.push(dev); }
                }
            }
            Err(_) => continue,
        }
    }
    out
}

/// Stop playback immediately (kill mpv, suppress auto-advance). Used by the
/// cast handoff (renderer owns playback) and the lyrics-only "check" path.
fn stop_music(w: &MainWindow) {
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
    w.set_music_playing(false);
}

/// The track id currently shown in the read-only lyrics-view popup.
static MUSIC_VIEW_LYRICS_ID: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
fn music_view_lyrics_id() -> &'static std::sync::Mutex<Option<i64>> {
    MUSIC_VIEW_LYRICS_ID.get_or_init(|| std::sync::Mutex::new(None))
}
/// Target track for the tag editor when opened from a song's context menu
/// (Edit media info). `None` ⇒ the editor targets the now-playing track.
static TAG_EDIT_TARGET: std::sync::OnceLock<std::sync::Mutex<Option<i64>>> = std::sync::OnceLock::new();
fn tag_edit_target() -> &'static std::sync::Mutex<Option<i64>> {
    TAG_EDIT_TARGET.get_or_init(|| std::sync::Mutex::new(None))
}
/// Previewed-but-unsaved lyrics from a popup re-search: (synced, content).
static PENDING_VIEW_LYRICS: std::sync::OnceLock<std::sync::Mutex<Option<(bool, String)>>> = std::sync::OnceLock::new();
fn pending_view_lyrics() -> &'static std::sync::Mutex<Option<(bool, String)>> {
    PENDING_VIEW_LYRICS.get_or_init(|| std::sync::Mutex::new(None))
}
/// Last.fm request token awaiting browser approval (np.p5.music.scrobble).
static LASTFM_PENDING_TOKEN: std::sync::OnceLock<std::sync::Mutex<Option<String>>> = std::sync::OnceLock::new();
fn lastfm_pending_token() -> &'static std::sync::Mutex<Option<String>> {
    LASTFM_PENDING_TOKEN.get_or_init(|| std::sync::Mutex::new(None))
}

/// Populate the lyrics-view popup for a specific track without touching playback.
/// Loads stored lyrics (DB) or fetches from LRCLIB, then fills the viewer rows +
/// prefills the re-search fields with the track's name/artist/album.
fn view_lyrics_load(w: &MainWindow, id: i64) {
    w.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::<MusicLyricLine>::default()));
    w.set_music_lyrics_view_plain("Loading…".into());
    w.set_music_lyrics_view_title("".into());
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let sig: Option<(Option<String>, Option<String>, Option<String>, f64)> = sqlx::query_as(
            "SELECT tm.title, ar.name, al.title, COALESCE(tm.duration_s, 0)
             FROM track_meta tm
             LEFT JOIN artists ar ON ar.id = tm.artist_id
             LEFT JOIN albums  al ON al.id = tm.album_id
             WHERE tm.item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        let (title, artist, album, dur) = match sig {
            Some((t, ar, al, d)) => (t.unwrap_or_default(), ar.unwrap_or_default(), al.unwrap_or_default(), d),
            None => (String::new(), String::new(), String::new(), 0.0),
        };
        let mut row: Option<(i64, String)> = sqlx::query_as(
            "SELECT synced, content FROM lyrics WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        if row.as_ref().map(|(_, c)| c.is_empty()).unwrap_or(true) && !title.is_empty() {
            let url = tulipix_music::lyrics::get_url(&artist, &title, &album, dur);
            let client = reqwest::Client::new();
            if let Ok(resp) = client.get(&url)
                .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                .send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    let synced = json["syncedLyrics"].as_str().unwrap_or("");
                    let plain = json["plainLyrics"].as_str().unwrap_or("");
                    if !synced.is_empty() { let _ = tulipix_music::lyrics::store(&pool, id, synced, true, "lrclib").await; row = Some((1, synced.to_string())); }
                    else if !plain.is_empty() { let _ = tulipix_music::lyrics::store(&pool, id, plain, false, "lrclib").await; row = Some((0, plain.to_string())); }
                }
            }
        }
        let (synced, content) = row.unwrap_or((0, String::new()));
        let lines = if synced != 0 { tulipix_music::lyrics::parse_lrc(&content) } else { Vec::new() };
        let status = if synced != 0 { "Synced" } else if content.is_empty() { "Not Found" } else { "Normal" };
        let sub = if album.is_empty() { artist.clone() } else if artist.is_empty() { album.clone() } else { format!("{artist} · {album}") };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_lyrics_view_mode("lyrics".into());
            w.set_music_lyrics_view_results(slint::ModelRc::new(slint::VecModel::<LyricsResult>::default()));
            w.set_music_lyrics_view_title(title.clone().into());
            w.set_music_lyrics_view_sub(sub.into());
            w.set_music_lyrics_view_status(status.into());
            w.set_music_lyrics_view_q_name(title.into());
            w.set_music_lyrics_view_q_artist(artist.into());
            w.set_music_lyrics_view_q_album(album.into());
            let rows: Vec<MusicLyricLine> = lines.iter().map(|(ms, t)| MusicLyricLine {
                time: fmt_clock(*ms as f64 / 1000.0).into(), text: t.clone().into() }).collect();
            w.set_music_lyrics_view_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
            w.set_music_lyrics_view_plain(if synced == 0 {
                if content.is_empty() { "No lyrics found — try Search below.".to_string() } else { content }
            } else { String::new() }.into());
        });
    });
}

/// LRCLIB search candidates for the lyrics popup: (synced, content) per result.
static LYRICS_SEARCH_RESULTS: std::sync::OnceLock<std::sync::Mutex<Vec<(bool, String)>>> = std::sync::OnceLock::new();
fn lyrics_search_results() -> &'static std::sync::Mutex<Vec<(bool, String)>> {
    LYRICS_SEARCH_RESULTS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

thread_local! {
    /// Full library lyrics-manager list (one row per song), paged 20/screen.
    static LYRICS_MGR_ROWS: std::cell::RefCell<Vec<LyricsMgrRow>> = const { std::cell::RefCell::new(Vec::new()) };
}
const LYRICS_MGR_PAGE: usize = 20;

/// Publish the current lyrics-manager page (20 rows) + counters to the UI.
fn publish_lyrics_mgr_page(w: &MainWindow) {
    LYRICS_MGR_ROWS.with(|r| {
        let all = r.borrow();
        let total = all.len();
        let synced = all.iter().filter(|x| x.status == "Synced").count();
        let missing = all.iter().filter(|x| x.status == "Missing").count();
        // Filter the visible rows by the active stat-card filter (counts stay full).
        let filter = w.get_music_lyrics_mgr_filter().to_string();
        let filtered: Vec<LyricsMgrRow> = all.iter().filter(|x| match filter.as_str() {
            "synced" => x.status == "Synced",
            "normal" => x.status == "Normal",
            "missing" => x.status == "Missing",
            _ => true,
        }).cloned().collect();
        let pages = filtered.len().div_ceil(LYRICS_MGR_PAGE).max(1);
        let page = (w.get_music_lyrics_mgr_page() as usize).min(pages - 1);
        let slice: Vec<LyricsMgrRow> = filtered.iter().skip(page * LYRICS_MGR_PAGE).take(LYRICS_MGR_PAGE).cloned().collect();
        w.set_music_lyrics_mgr_rows(slint::ModelRc::new(slint::VecModel::from(slice)));
        w.set_music_lyrics_mgr_total(total as i32);
        w.set_music_lyrics_mgr_synced(synced as i32);
        w.set_music_lyrics_mgr_missing(missing as i32);
        w.set_music_lyrics_mgr_page(page as i32);
        w.set_music_lyrics_mgr_pages(pages as i32);
        w.set_music_lyrics_sync_progress(if total > 0 { (total - missing) as f32 / total as f32 } else { 0.0 });
    });
}

/// Rebuild the lyrics-manager list from the song library + the lyrics table.
/// My Music only — audiobook chapters never have lyrics and would drown the
/// "Missing" stats (np.p4.music.lyrics).
fn rebuild_lyrics_manager(w: &MainWindow) {
    let songs: Vec<(i32, i64, String, String)> = match music_songs().lock() {
        Ok(g) => g.iter().filter(|s| !s.is_audiobook)
            .map(|s| (s.pos, s.item_id, s.title.clone(), s.artist.clone())).collect(),
        Err(_) => return,
    };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // item_id → synced flag, for tracks that actually have lyric content.
        let rows: Vec<(i64, i64)> = sqlx::query_as(
            "SELECT item_id, synced FROM lyrics WHERE content IS NOT NULL AND content <> ''")
            .fetch_all(&pool).await.unwrap_or_default();
        let map: std::collections::HashMap<i64, i64> = rows.into_iter().collect();
        let list: Vec<LyricsMgrRow> = songs.iter().map(|(pos, id, title, artist)| {
            let status = match map.get(id) {
                Some(s) if *s != 0 => "Synced",
                Some(_) => "Normal",
                None => "Missing",
            };
            LyricsMgrRow { title: title.clone().into(), artist: artist.clone().into(),
                status: status.into(), index: *pos }
        }).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            LYRICS_MGR_ROWS.with(|r| *r.borrow_mut() = list);
            publish_lyrics_mgr_page(&w);
        });
    });
}

/// Load synced/plain lyrics for the current track from the lyrics table (empty
/// when none — LRCLIB fetch needs network). Parses synced LRC into rows for the
/// scrolling highlight (np.p4.music.lyrics / np.p5.music.lyrics-synced).
fn load_music_lyrics(w: &MainWindow) {
    w.set_music_lyrics_offset_ms(0);
    w.set_music_lyrics_active(-1);
    if let Ok(mut g) = music_lyrics_lines().lock() { g.clear(); }
    w.set_music_lyrics_rows(slint::ModelRc::new(slint::VecModel::<MusicLyricLine>::default()));
    let Some(id) = current_music_id(w) else { w.set_music_lyrics_text("".into()); return; };
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let mut row: Option<(i64, String)> = sqlx::query_as(
            "SELECT synced, content FROM lyrics WHERE item_id = ?")
            .bind(id).fetch_optional(&pool).await.ok().flatten();
        // DB miss → live LRCLIB fetch by track signature, then cache it
        // (np.p5.music.lyrics-synced — was DB-only).
        if row.as_ref().map(|(_, c)| c.is_empty()).unwrap_or(true) {
            let sig: Option<(Option<String>, Option<String>, f64)> = sqlx::query_as(
                "SELECT tm.title, ar.name, COALESCE(tm.duration_s, 0)
                 FROM track_meta tm LEFT JOIN artists ar ON ar.id = tm.artist_id
                 WHERE tm.item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            if let Some((Some(title), artist, dur)) = sig {
                let artist = artist.unwrap_or_default();
                let url = tulipix_music::lyrics::get_url(&artist, &title, "", dur);
                let client = reqwest::Client::new();
                if let Ok(resp) = client.get(&url)
                    .header(reqwest::header::USER_AGENT, tulipix_music::musicbrainz::USER_AGENT)
                    .send().await {
                    if let Ok(json) = resp.json::<serde_json::Value>().await {
                        let synced = json["syncedLyrics"].as_str().unwrap_or("");
                        let plain = json["plainLyrics"].as_str().unwrap_or("");
                        if !synced.is_empty() {
                            let _ = tulipix_music::lyrics::store(&pool, id, synced, true, "lrclib").await;
                            row = Some((1, synced.to_string()));
                        } else if !plain.is_empty() {
                            let _ = tulipix_music::lyrics::store(&pool, id, plain, false, "lrclib").await;
                            row = Some((0, plain.to_string()));
                        }
                    }
                }
            }
        }
        let (synced, content) = row.unwrap_or((0, String::new()));
        let lines = if synced != 0 { tulipix_music::lyrics::parse_lrc(&content) } else { Vec::new() };
        let _ = weak.upgrade_in_event_loop(move |w| {
            // Drop stale results: the track may have changed while we fetched, which
            // would otherwise show the previous song's lyrics (sync bug).
            if current_music_id(&w) != Some(id) { return; }
            w.set_music_lyrics_text(content.into());
            if !lines.is_empty() {
                let rows: Vec<MusicLyricLine> = lines.iter().map(|(ms, t)| MusicLyricLine {
                    time: fmt_clock(*ms as f64 / 1000.0).into(),
                    text: t.clone().into(),
                }).collect();
                w.set_music_lyrics_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
                if let Ok(mut g) = music_lyrics_lines().lock() { *g = lines; }
                update_lyrics_active(&w);
            }
        });
    });
}

/// Submit pending scrobbles (np.p5.music.scrobble) — ListenBrainz (token from
/// the keychain) and Last.fm (signed calls with the session key from the
/// connect flow). Drains oldest-first batches, marking rows submitted on 2xx.
fn submit_scrobbles() {
    submit_scrobbles_lastfm();
    let Some(token) = tulipix_core::api_keys::fetch("listenbrainz").ok().flatten() else { return; };
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let pending: Vec<(i64, i64, i64)> = sqlx::query_as(
            "SELECT id, item_id, played_at FROM scrobble_queue
             WHERE service = 'listenbrainz' AND submitted = 0 ORDER BY played_at ASC LIMIT 50")
            .fetch_all(&pool).await.unwrap_or_default();
        if pending.is_empty() { return; }
        let client = reqwest::Client::new();
        for (row_id, item_id, played_at) in pending {
            let meta: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT tm.title, ar.name, al.title
                 FROM track_meta tm
                 LEFT JOIN artists ar ON ar.id = tm.artist_id
                 LEFT JOIN albums  al ON al.id = tm.album_id
                 WHERE tm.item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten();
            let Some((Some(title), artist, album)) = meta else { continue; };
            let artist = artist.unwrap_or_default();
            if artist.is_empty() { continue; }
            let payload = tulipix_music::listenbrainz::single_listen(
                &artist, &title, album.as_deref(), played_at);
            let ok = client.post(tulipix_music::listenbrainz::SUBMIT_URL)
                .header(reqwest::header::AUTHORIZATION, tulipix_music::listenbrainz::auth_header(&token))
                .json(&payload).send().await
                .map(|r| r.status().is_success()).unwrap_or(false);
            if ok {
                let _ = sqlx::query("UPDATE scrobble_queue SET submitted = 1 WHERE id = ?")
                    .bind(row_id).execute(&pool).await;
            } else { break; } // network/auth issue — retry the rest next time
        }
    });
}

/// Drain the Last.fm half of the scrobble queue with signed `track.scrobble`
/// calls. Needs both the API creds (Settings → API Keys, "KEY:SECRET") and the
/// session key minted by the connect flow; otherwise rows stay queued.
fn submit_scrobbles_lastfm() {
    let creds = tulipix_core::api_keys::fetch("lastfm").ok().flatten()
        .and_then(|v| tulipix_music::scrobble::parse_key_secret(&v));
    let Some((api_key, secret)) = creds else { return; };
    let Some(sk) = tulipix_core::api_keys::fetch("lastfm.session").ok().flatten() else { return; };
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let pending = tulipix_music::scrobble::pending(&pool, 50).await.unwrap_or_default();
        if pending.is_empty() { return; }
        let client = reqwest::Client::new();
        for (row_id, item_id, played_at) in pending {
            let meta: Option<(Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
                "SELECT tm.title, ar.name, al.title
                 FROM track_meta tm
                 LEFT JOIN artists ar ON ar.id = tm.artist_id
                 LEFT JOIN albums  al ON al.id = tm.album_id
                 WHERE tm.item_id = ?")
                .bind(item_id).fetch_optional(&pool).await.ok().flatten();
            let Some((Some(title), artist, album)) = meta else { continue; };
            let artist = artist.unwrap_or_default();
            if artist.is_empty() { continue; }
            let ts = played_at.to_string();
            let mut params: Vec<(&str, &str)> = vec![
                ("method", "track.scrobble"), ("api_key", &api_key), ("sk", &sk),
                ("artist", &artist), ("track", &title), ("timestamp", &ts),
            ];
            let album = album.unwrap_or_default();
            if !album.is_empty() { params.push(("album", &album)); }
            let form = tulipix_music::scrobble::signed_params(&params, &secret);
            let ok = client.post(tulipix_music::scrobble::API_ROOT)
                .form(&form).send().await
                .map(|r| r.status().is_success()).unwrap_or(false);
            if ok {
                let _ = tulipix_music::scrobble::mark_submitted(&pool, &[row_id]).await;
            } else { break; } // auth/network issue — retry next drain
        }
    });
}

/// Sort the in-memory Songs list by the active mode/direction.
fn sort_music_songs(sort: &str, dir: &str) {
    let Ok(mut g) = music_songs().lock() else { return; };
    g.sort_by(|a, b| {
        let o = match sort {
            "title"  => a.title.to_lowercase().cmp(&b.title.to_lowercase()),
            "artist" => a.artist.to_lowercase().cmp(&b.artist.to_lowercase()),
            "plays"  => a.plays.cmp(&b.plays),
            "rating" => a.stars.cmp(&b.stars).then(a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            // Release date — songs without one sort last (treated as far-future).
            "release" => {
                let key = |s: &str| if s.trim().is_empty() { "9999".to_string() } else { s.to_string() };
                key(&a.release_date).cmp(&key(&b.release_date)).then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            },
            _        => a.added.cmp(&b.added),
        };
        if dir == "asc" { o } else { o.reverse() }
    });
}

/// Build the current Songs page (30 rows) into the UI model + page counters.
fn rebuild_music_songs_page(w: &MainWindow) {
    let g = match music_songs().lock() { Ok(g) => g, Err(_) => return };
    let q = music_query_filter().lock().map(|s| s.to_lowercase()).unwrap_or_default();
    // Filter by the search box (title/artist substring) before paginating.
    // Audiobook-flagged tracks are excluded — they live in the Audiobooks
    // section, not the My Music Songs list (np.p5.music.audiobook-detect).
    let view: Vec<&SongMeta> = g.iter().filter(|s| {
        !s.is_audiobook
            && (q.is_empty() || s.title.to_lowercase().contains(&q) || s.artist.to_lowercase().contains(&q))
    }).collect();
    let total = view.len();
    // Smaller pages while searching so songs stay above the artists/albums rows.
    let per = if q.is_empty() { SONG_PAGE } else { 14 };
    let pages = total.div_ceil(per).max(1);
    let page = (w.get_music_song_page() as usize).min(pages - 1);
    let tiles = w.get_music_tiles();
    let thumb_at = |pos: i32| -> slint::Image {
        if pos >= 0 && (pos as usize) < tiles.row_count() {
            tiles.row_data(pos as usize).map(|t| t.thumb).unwrap_or_default()
        } else { slint::Image::default() }
    };
    let page_rows: Vec<&SongMeta> = view.iter().skip(page * per).take(per).copied().collect();
    let rows: Vec<MusicSongRow> = page_rows.iter().map(|s| MusicSongRow {
        thumb: thumb_at(s.pos),
        title: s.title.clone().into(),
        artist: s.artist.clone().into(),
        duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
        index: s.pos,
    }).collect();
    // Rich list rows with the lyrics / fav / star columns.
    let rows_ex: Vec<SongRowEx> = page_rows.iter().map(|s| SongRowEx {
        thumb: thumb_at(s.pos),
        title: s.title.clone().into(),
        artist: s.artist.clone().into(),
        album: s.album.clone().into(),
        duration: if s.duration_s > 0.0 { fmt_clock(s.duration_s).into() } else { "".into() },
        index: s.pos,
        loved: s.loved,
        stars: s.stars,
        synced: s.synced,
    }).collect();
    w.set_music_songs(slint::ModelRc::new(slint::VecModel::from(rows)));
    w.set_music_songs_ex(slint::ModelRc::new(slint::VecModel::from(rows_ex)));
    w.set_music_song_total(total as i32);
    w.set_music_song_pages(pages as i32);
    w.set_music_song_page(page as i32);
}

/// Read audio tags from a file via the bundled (or PATH) ffprobe — title /
/// artist / album / genre / year / track / disc + duration & stream info — so
/// the music browse views have real metadata (np.p4.music.tags).
fn ffprobe_tags(path: &std::path::Path) -> tulipix_music::tags::TrackTags {
    use tulipix_music::tags::{self, TrackTags};
    let mut t = TrackTags::default();
    let ff = tulipix_core::thumbs::tool_bin("ffprobe");
    let out = std::process::Command::new(ff)
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path).output();
    let Ok(out) = out else { return fallback_title(t, path); };
    let json: serde_json::Value = match serde_json::from_slice(&out.stdout) { Ok(v) => v, Err(_) => return fallback_title(t, path) };
    let fmt = &json["format"];
    let tagv = &fmt["tags"];
    // Case-insensitive tag lookup (ffmpeg emits TITLE vs title per container).
    let get = |k: &str| -> Option<String> {
        tagv.as_object().and_then(|o| o.iter()
            .find(|(kk, _)| kk.to_lowercase() == k)
            .and_then(|(_, v)| v.as_str()).map(|s| s.to_string()))
    };
    t.title = get("title").and_then(|s| tags::clean(&s));
    t.artist = get("artist").and_then(|s| tags::clean(&s));
    t.album = get("album").and_then(|s| tags::clean(&s));
    t.album_artist = get("album_artist").and_then(|s| tags::clean(&s));
    t.genre = get("genre").and_then(|s| tags::clean(&s));
    t.year = get("date").or_else(|| get("year")).and_then(|s| tags::parse_year(&s));
    t.track_no = get("track").and_then(|s| tags::parse_track_no(&s));
    t.disc_no = get("disc").and_then(|s| tags::parse_track_no(&s));
    t.duration_s = fmt["duration"].as_str().and_then(|s| s.parse().ok());
    if let Some(streams) = json["streams"].as_array() {
        if let Some(a) = streams.iter().find(|s| s["codec_type"] == "audio") {
            t.codec = a["codec_name"].as_str().map(|s| s.to_string());
            t.sample_rate = a["sample_rate"].as_str().and_then(|s| s.parse().ok());
            t.channels = a["channels"].as_i64();
            t.bitrate = a["bit_rate"].as_str().and_then(|s| s.parse().ok());
        }
    }
    t.container = path.extension().and_then(|e| e.to_str()).and_then(tags::container_from_ext).map(|s| s.to_string());
    fallback_title(t, path)
}

/// Write title/artist/album tags back into the audio file via the bundled
/// ffmpeg (np.p5.music.tag-editor). Stream-copies to a sibling temp file, then
/// atomically renames over the original — so a decode failure leaves the source
/// untouched. Empty fields are left as-is on the file.
fn write_audio_tags(path: &str, title: &str, artist: &str, album: &str) {
    let src = std::path::Path::new(path);
    let Some(ext) = src.extension().and_then(|e| e.to_str()) else { return; };
    let tmp = src.with_extension(format!("tulipix-tmp.{ext}"));
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let mut cmd = std::process::Command::new(ff);
    cmd.arg("-v").arg("error").arg("-y").arg("-i").arg(src)
        .arg("-map_metadata").arg("0").arg("-c").arg("copy");
    if !title.is_empty()  { cmd.arg("-metadata").arg(format!("title={title}")); }
    if !artist.is_empty() { cmd.arg("-metadata").arg(format!("artist={artist}")); }
    if !album.is_empty()  { cmd.arg("-metadata").arg(format!("album={album}")); }
    cmd.arg(&tmp);
    match cmd.status() {
        Ok(st) if st.success() && tmp.exists() && std::fs::metadata(&tmp).map(|m| m.len() > 0).unwrap_or(false) => {
            if let Err(e) = std::fs::rename(&tmp, src) {
                tracing::warn!(error = %e, "tag write-back rename failed");
                let _ = std::fs::remove_file(&tmp);
            }
        }
        _ => { let _ = std::fs::remove_file(&tmp); }
    }
}

/// Fall back to the filename stem when the file carries no title tag.
fn fallback_title(mut t: tulipix_music::tags::TrackTags, path: &std::path::Path) -> tulipix_music::tags::TrackTags {
    if t.title.is_none() {
        t.title = path.file_stem().and_then(|s| s.to_str()).map(|s| s.to_string());
    }
    t
}

/// Extract tags for every music item missing metadata, upsert into track_meta /
/// artists / albums, then refresh the Music views (np.p4.music.tags + scan).
fn ingest_music_tags(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, abs_path FROM items WHERE section = 'music' AND missing_since IS NULL")
            .fetch_all(&pool).await.unwrap_or_default();
        let mut tagged = 0u32;
        for (id, path) in rows {
            // Idempotent: skip items already carrying a title.
            let done: Option<(i64,)> = sqlx::query_as(
                "SELECT item_id FROM track_meta WHERE item_id = ? AND title IS NOT NULL")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            if done.is_some() { continue; }
            let p = std::path::PathBuf::from(&path);
            if !p.exists() { continue; }
            let tags = tokio::task::spawn_blocking(move || ffprobe_tags(&p)).await.unwrap_or_default();
            if tulipix_music::scan::upsert_track(&pool, id, &path, &tags).await.is_ok() { tagged += 1; }
        }
        tracing::info!(tagged, "music tag ingest complete");
        let _ = weak.upgrade_in_event_loop(|w| populate_music_views(w.as_weak()));
    });
}

/// Build music_ids (item_id per playback position) from the music.db `items`
/// table, then populate the dashboard rails + browse groups (Albums/Artists/
/// Genres). Each browse/dashboard tile carries its playback position in `index`
/// so the existing `play-music` path plays it. (np.p4.music.dashboard / browse)
fn populate_music_views(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let Ok(pool) = pool_for("music").await else { return; };
        // abs_path → item_id for every present music item.
        let id_rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT id, abs_path FROM items WHERE section = 'music' AND missing_since IS NULL")
            .fetch_all(&pool).await.unwrap_or_default();
        let recent = tulipix_music::dashboard::recently_played(&pool, 20).await.unwrap_or_default();
        let most   = tulipix_music::dashboard::most_played(&pool, 20).await.unwrap_or_default();
        let loved  = tulipix_music::rating::loved(&pool, 20).await.unwrap_or_default();
        let fresh  = tulipix_music::dashboard::new_this_week(&pool, 20).await.unwrap_or_default();
        let albums = tulipix_music::browse::albums(&pool).await.unwrap_or_default();
        let artists = tulipix_music::browse::artists(&pool).await.unwrap_or_default();
        let genres = tulipix_music::browse::genres(&pool).await.unwrap_or_default();
        // Favorited albums/artists (np.p5.music.fav-collections). The `loved`
        // columns are added on demand; ignore the error when they already exist.
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN loved INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE albums ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE artists ADD COLUMN rating INTEGER NOT NULL DEFAULT 0").execute(&pool).await;
        let loved_albums: std::collections::HashSet<i64> = sqlx::query_scalar("SELECT id FROM albums WHERE loved = 1")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        let loved_artists: std::collections::HashSet<i64> = sqlx::query_scalar("SELECT id FROM artists WHERE loved = 1")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        let album_rating: std::collections::HashMap<i64, i64> = sqlx::query_as("SELECT id, rating FROM albums WHERE rating > 0")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        let artist_rating: std::collections::HashMap<i64, i64> = sqlx::query_as("SELECT id, rating FROM artists WHERE rating > 0")
            .fetch_all(&pool).await.unwrap_or_default().into_iter().collect();
        // Per-track grouping keys, to resolve a group's first playback position.
        let meta: Vec<(i64, Option<i64>, Option<i64>, Option<String>)> = sqlx::query_as(
            "SELECT item_id, album_id, artist_id, genre FROM track_meta WHERE is_audiobook = 0").fetch_all(&pool).await.unwrap_or_default();
        // Detailed Songs list rows (np.p4.music.browse list view).
        // On-demand columns (idempotent) so the SELECT below never fails on older DBs.
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN release_date TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN credits TEXT").execute(&pool).await;
        let _ = sqlx::query("ALTER TABLE track_meta ADD COLUMN user_locked INTEGER DEFAULT 0").execute(&pool).await;
        // music_songs stays complete (incl. audiobooks) so the Audiobooks section
        // can resolve each chapter's title/duration by position; the My Music
        // Songs *list* filters audiobooks out at render in rebuild_music_songs_page.
        let song_rows: Vec<(i64, Option<String>, Option<String>, Option<f64>, i64, i64, i64, i64, Option<i64>, Option<String>, Option<String>, i64)> = sqlx::query_as(
            "SELECT tm.item_id, tm.title, ar.name, tm.duration_s, it.added, tm.play_count, \
                    COALESCE(tm.loved,0), COALESCE(tm.rating,0), \
                    (SELECT ly.synced FROM lyrics ly WHERE ly.item_id = tm.item_id), al.title, tm.release_date, \
                    tm.is_audiobook \
             FROM track_meta tm JOIN items it ON it.id = tm.item_id AND it.missing_since IS NULL \
             LEFT JOIN artists ar ON ar.id = tm.artist_id \
             LEFT JOIN albums al ON al.id = tm.album_id").fetch_all(&pool).await.unwrap_or_default();
        // Folder hierarchy (np.p4.music.folders) — folder path per track.
        let folder_rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT item_id, folder FROM track_meta WHERE folder IS NOT NULL AND folder != '' AND is_audiobook = 0")
            .fetch_all(&pool).await.unwrap_or_default();
        // Playlists (np.p4.music.playlists) — name + first track. Smart playlists
        // (Loved / Recently Added) compute their tracks from a rule, so the LEFT
        // JOIN count is 0 — resolve those via the rule so counts/covers are real.
        let playlist_raw: Vec<(i64, String, i64, Option<i64>, i64, Option<String>)> = sqlx::query_as(
            "SELECT p.id, p.name, COUNT(pi.item_id), MIN(pi.item_id), COALESCE(p.is_smart,0), p.rule_json \
             FROM playlists p LEFT JOIN playlist_items pi ON pi.playlist_id = p.id GROUP BY p.id")
            .fetch_all(&pool).await.unwrap_or_default();
        let mut playlist_rows: Vec<(i64, String, i64, Option<i64>)> = Vec::with_capacity(playlist_raw.len());
        for (pid, name, n, first, is_smart, rule_json) in playlist_raw {
            if is_smart != 0 {
                let ids = match rule_json.as_ref().and_then(|j| serde_json::from_str::<tulipix_music::playlists::SmartRule>(j).ok()) {
                    Some(rule) => tulipix_music::playlists::evaluate(&pool, &rule).await.unwrap_or_default(),
                    None => Vec::new(),
                };
                playlist_rows.push((pid, name, ids.len() as i64, ids.first().copied()));
            } else {
                playlist_rows.push((pid, name, n, first));
            }
        }

        let _ = weak.upgrade_in_event_loop(move |w| {
            let id_of: std::collections::HashMap<String, i64> = id_rows.into_iter()
                .map(|(id, p)| (p, id)).collect();
            // music_ids aligned to music_paths order.
            let paths = music_paths().lock().map(|g| g.clone()).unwrap_or_default();
            let ids: Vec<i64> = paths.iter()
                .map(|p| id_of.get(&p.to_string_lossy().into_owned()).copied().unwrap_or(-1))
                .collect();
            // pos_of[item_id] = playback position.
            let mut pos_of: std::collections::HashMap<i64, i32> = std::collections::HashMap::new();
            for (i, id) in ids.iter().enumerate() { if *id >= 0 { pos_of.entry(*id).or_insert(i as i32); } }
            if let Ok(mut g) = music_ids().lock() { *g = ids.clone(); }

            // Snapshot the scanned song tiles by position to reuse thumbs/labels.
            let songs = w.get_music_tiles();
            let tile_at = |pos: i32| -> Option<PhotoTile> {
                if pos >= 0 && (pos as usize) < songs.row_count() { songs.row_data(pos as usize) } else { None }
            };
            let rail = |id_list: &[i64]| -> Vec<PhotoTile> {
                id_list.iter().filter_map(|id| pos_of.get(id).copied())
                    .filter_map(|pos| tile_at(pos).map(|mut t| { t.index = pos; t }))
                    .collect()
            };
            // First playback position for each grouping key.
            let mut first_album: std::collections::HashMap<i64, i32> = std::collections::HashMap::new();
            let mut first_artist: std::collections::HashMap<i64, i32> = std::collections::HashMap::new();
            let mut first_genre: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
            for (item_id, alb, art, genre) in &meta {
                let Some(&pos) = pos_of.get(item_id) else { continue; };
                if let Some(a) = alb { first_album.entry(*a).or_insert(pos); }
                if let Some(a) = art { first_artist.entry(*a).or_insert(pos); }
                if let Some(g) = genre { if !g.is_empty() { first_genre.entry(g.clone()).or_insert(pos); } }
            }

            w.set_music_recent(slint::ModelRc::new(slint::VecModel::from(rail(&recent))));
            w.set_music_most(slint::ModelRc::new(slint::VecModel::from(rail(&most))));
            w.set_music_loved(slint::ModelRc::new(slint::VecModel::from(rail(&loved))));
            w.set_music_fresh(slint::ModelRc::new(slint::VecModel::from(rail(&fresh))));

            // Albums — cover from cover_path (else the first track's thumb).
            let album_tiles_src: Vec<(PhotoTile, i64)> = albums.iter().map(|a| {
                let pos = first_album.get(&a.album_id).copied().unwrap_or(-1);
                let thumb = a.cover_path.as_ref()
                    .map(|c| slint::Image::load_from_path(std::path::Path::new(c)).unwrap_or_default())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                let label = match &a.artist { Some(ar) => format!("{} · {}", a.title, ar), None => a.title.clone() };
                (PhotoTile { thumb, label: label.into(), index: pos, starred: loved_albums.contains(&a.album_id),
                    stack_count: album_rating.get(&a.album_id).copied().unwrap_or(0) as i32,
                    count: a.track_count as i32, ..Default::default() }, a.track_count)
            }).collect();
            set_browse_src("albums", album_tiles_src.clone());
            rebuild_browse_tab(&w, "albums");
            // Favorited albums → the categorized Favorites page.
            let fav_album_tiles: Vec<PhotoTile> = album_tiles_src.iter().filter(|(t, _)| t.starred).map(|(t, _)| t.clone()).collect();
            w.set_music_fav_albums(slint::ModelRc::new(slint::VecModel::from(fav_album_tiles)));

            // Artists — cover from the artist's first track (album art); Genres stay label-only.
            let artist_src: Vec<(PhotoTile, i64)> = artists.iter().map(|(id, name, n)| {
                let pos = first_artist.get(id).copied().unwrap_or(-1);
                (PhotoTile {
                    thumb: tile_at(pos).map(|t| t.thumb).unwrap_or_default(),
                    label: name.clone().into(), index: pos, starred: loved_artists.contains(id),
                    stack_count: artist_rating.get(id).copied().unwrap_or(0) as i32,
                    count: *n as i32, ..Default::default()
                }, *n)
            }).collect();
            set_browse_src("artists", artist_src.clone());
            rebuild_browse_tab(&w, "artists");
            // Favorited artists → the categorized Favorites page.
            let fav_artist_tiles: Vec<PhotoTile> = artist_src.iter().filter(|(t, _)| t.starred).map(|(t, _)| t.clone()).collect();
            w.set_music_fav_artists(slint::ModelRc::new(slint::VecModel::from(fav_artist_tiles)));
            // Per-genre cover override (music.genre.cover.<genre>) wins over the
            // first-track album art (np.p4.music.browse / genre-art).
            let gcovers = tulipix_core::settings::Settings::load().map(|s| s.advanced).unwrap_or_default();
            let genre_src: Vec<(PhotoTile, i64)> = genres.iter().map(|(g, n)| {
                let pos = first_genre.get(g).copied().unwrap_or(-1);
                let thumb = gcovers.get(&format!("music.genre.cover.{g}"))
                    .map(|c| slint::Image::load_from_path(std::path::Path::new(c)).unwrap_or_default())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                (PhotoTile { thumb, label: g.clone().into(), index: pos, ..Default::default() }, *n)
            }).collect();
            set_browse_src("genres", genre_src);
            rebuild_browse_tab(&w, "genres");

            // Detailed Songs list — map each track to its playback position, with
            // a filename fallback for missing titles.
            let metas: Vec<SongMeta> = song_rows.into_iter().filter_map(|(id, title, artist, dur, added, plays, loved, rating, synced, album, release, is_audiobook)| {
                let pos = *pos_of.get(&id)?;
                let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| {
                    paths.get(pos as usize)
                        .and_then(|p| p.file_stem()).and_then(|s| s.to_str())
                        .unwrap_or("Unknown").to_string()
                });
                Some(SongMeta {
                    pos, item_id: id, title,
                    artist: artist.unwrap_or_default(),
                    album: album.unwrap_or_default(),
                    duration_s: dur.unwrap_or(0.0),
                    added, plays,
                    loved: loved != 0, stars: rating as i32, synced: synced.is_some(),
                    release_date: release.unwrap_or_default(),
                    is_audiobook: is_audiobook != 0,
                })
            }).collect();
            // Auto-fill the Songs-row pills with current coverage (lyrics synced %,
            // tags present %) so they reflect library state without opening either
            // manager (np.p5.atmusic.songs-pills).
            {
                let total = metas.len();
                if total > 0 {
                    let synced_n = metas.iter().filter(|m| m.synced).count();
                    let tagged_n = metas.iter()
                        .filter(|m| !m.artist.trim().is_empty() && !m.album.trim().is_empty()).count();
                    w.set_music_lyrics_sync_progress(synced_n as f32 / total as f32);
                    w.set_music_meta_fetch_progress(tagged_n as f32 / total as f32);
                }
            }
            // title/artist/duration by item_id, for the Home "Recently played" list.
            let info: std::collections::HashMap<i64, (String, String, f64)> = metas.iter()
                .map(|m| (ids[m.pos as usize], (m.title.clone(), m.artist.clone(), m.duration_s)))
                .collect();
            // Recently-played pool (≤18) → Home pager (6/page, 3 pages).
            let recent_pool: Vec<SongMeta> = recent.iter().take(18).filter_map(|id| {
                let pos = *pos_of.get(id)?;
                let (title, artist, dur) = info.get(id).cloned().unwrap_or_default();
                Some(SongMeta { pos, item_id: *id, title, artist, album: String::new(), duration_s: dur, added: 0, plays: 0, loved: false, stars: 0, synced: false, release_date: String::new(), is_audiobook: false })
            }).collect();
            if let Ok(mut g) = music_recent().lock() { *g = recent_pool; }
            w.set_music_recent_page(0);
            rebuild_recent_page(&w);

            // Top artists / albums (most tracks first, capped) for the Home grids.
            let mut top_artist_src = artists.clone();
            top_artist_src.sort_by(|a, b| b.2.cmp(&a.2));
            let top_artist_tiles: Vec<PhotoTile> = top_artist_src.iter().take(6).map(|(id, name, _)| {
                let pos = first_artist.get(id).copied().unwrap_or(-1);
                PhotoTile { thumb: tile_at(pos).map(|t| t.thumb).unwrap_or_default(),
                    label: name.clone().into(), index: pos, ..Default::default() }
            }).collect();
            w.set_music_top_artists(slint::ModelRc::new(slint::VecModel::from(top_artist_tiles)));
            let mut top_album_src: Vec<_> = album_tiles_src.clone();
            top_album_src.sort_by(|a, b| b.1.cmp(&a.1)); // by track_count
            let top_album_tiles: Vec<PhotoTile> = top_album_src.iter().take(7).map(|(t, _)| t.clone()).collect();
            w.set_music_top_albums(slint::ModelRc::new(slint::VecModel::from(top_album_tiles)));

            // Folders — first playback position per folder, basename label.
            let mut first_folder: std::collections::HashMap<String, i32> = std::collections::HashMap::new();
            for (item_id, folder) in &folder_rows {
                if let Some(&pos) = pos_of.get(item_id) { first_folder.entry(folder.clone()).or_insert(pos); }
            }
            let mut folder_counts: std::collections::HashMap<String, i64> = std::collections::HashMap::new();
            for (_, folder) in &folder_rows { *folder_counts.entry(folder.clone()).or_insert(0) += 1; }
            let mut folder_keys: Vec<&String> = first_folder.keys().collect();
            folder_keys.sort();
            // Section tag per folder (np.p5.atmusic.folder-sections) — carried in
            // `color_label` so the Folders grid can render the assignment chip.
            let sec_map = load_folder_sections();
            let folder_src: Vec<(PhotoTile, i64)> = folder_keys.iter().map(|folder| {
                let base = std::path::Path::new(folder.as_str()).file_name()
                    .and_then(|s| s.to_str()).unwrap_or(folder.as_str());
                let n = folder_counts.get(*folder).copied().unwrap_or(0);
                let sec = sec_map.get(*folder).cloned().unwrap_or_else(|| "mymusic".to_string());
                (PhotoTile {
                    label: format!("🗂 {base} · {n}").into(),
                    color_label: sec.into(),
                    index: first_folder.get(*folder).copied().unwrap_or(-1), ..Default::default()
                }, n)
            }).collect();
            set_browse_src("folders", folder_src);
            rebuild_browse_tab(&w, "folders");

            // Playlists — `index` carries the playlist DB id so playlist-open can
            // load its tracks (np.p5.music.playlists-builder).
            let playlist_src: Vec<(PhotoTile, i64)> = playlist_rows.iter().map(|(pid, name, n, first)| {
                // Cover: custom playlist art → else the first track's album thumb.
                let pos = first.and_then(|fid| pos_of.get(&fid).copied()).unwrap_or(-1);
                let thumb = playlist_cover_path(*pid)
                    .map(|p| slint::Image::load_from_path(std::path::Path::new(&p)).unwrap_or_default())
                    .or_else(|| tile_at(pos).map(|t| t.thumb))
                    .unwrap_or_default();
                (PhotoTile {
                    label: format!("{name} · {n}").into(), thumb,
                    index: *pid as i32, ..Default::default()
                }, *n)
            }).collect();
            set_browse_src("playlists", playlist_src);
            rebuild_browse_tab(&w, "playlists");

            if let Ok(mut g) = music_songs().lock() { *g = metas; }
            sort_music_songs(&w.get_music_song_sort(), &w.get_music_song_dir());
            w.set_music_song_page(0);
            rebuild_music_songs_page(&w);
        });
    });
}

// Single audio player instance — replacing it on each play avoids stacking
// overlapping mpv processes the way video (separate windows) can tolerate.
static MUSIC_PROC: std::sync::OnceLock<std::sync::Mutex<Option<std::process::Child>>> =
    std::sync::OnceLock::new();
fn music_proc() -> &'static std::sync::Mutex<Option<std::process::Child>> {
    MUSIC_PROC.get_or_init(|| std::sync::Mutex::new(None))
}
/// PID of the windowed video mpv (0 = none). Tracked so music↔video share one
/// "universal" stream and so playback dies with the app.
static VIDEO_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Make a spawned child die with us — on Linux the kernel sends SIGKILL when
/// the parent (tulipix) exits for ANY reason (close, crash, kill), so mpv never
/// orphans. `PR_SET_PDEATHSIG` is Linux-only; on other OSes children are reaped
/// by `kill_all_mpv()` on window close + `child.kill()` on track change.
#[cfg(target_os = "linux")]
fn mpv_die_with_parent(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0);
            Ok(())
        });
    }
}
#[cfg(not(target_os = "linux"))]
fn mpv_die_with_parent(_cmd: &mut std::process::Command) {
    // macOS/Windows: no PR_SET_PDEATHSIG. A Win32 Job Object with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE would harden crash-time cleanup.
}
/// Kill the windowed video mpv if one is running.
fn stop_video() {
    let pid = VIDEO_PID.swap(0, std::sync::atomic::Ordering::SeqCst);
    if pid == 0 { return; }
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"]).status();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("kill").arg("-9").arg(pid.to_string()).status();
}
/// Kill the headless music mpv if one is running.
fn kill_music_proc() {
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); // suppress auto-advance
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
}
/// Stop every mpv we spawned — called when the app window closes so nothing keeps
/// playing in the background (np: universal-player teardown).
fn kill_all_mpv() { kill_music_proc(); stop_video(); }
/// Generation counter for the music sleep timer; bumped on each cycle so a
/// pending timer task knows it was superseded.
static SLEEP_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Active music IPC socket path (mpv `--input-ipc-server`) for live control.
static MUSIC_SOCK: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
fn music_sock() -> &'static std::sync::Mutex<Option<PathBuf>> {
    MUSIC_SOCK.get_or_init(|| std::sync::Mutex::new(None))
}
/// Playback generation — bumped on each play/stop so a finished track's reader
/// only auto-advances if it's still the current one.
static MUSIC_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
/// Live 10-band equalizer gains (dB), shared by presets + per-band drags.
static MUSIC_EQ: std::sync::OnceLock<std::sync::Mutex<[f64; 10]>> = std::sync::OnceLock::new();
fn music_eq() -> &'static std::sync::Mutex<[f64; 10]> {
    MUSIC_EQ.get_or_init(|| std::sync::Mutex::new([0.0; 10]))
}
/// Build an mpv `af` value for the 10-band EQ. mpv's option parser treats
/// `|`/`=`/space specially, so the `anequalizer` params are length-quoted
/// (`%N%…`) — the format that survives both `--af=` and IPC `set_property af`.
/// Empty when flat (clears the filter).
fn music_eq_af(gains: &[f64; 10]) -> String {
    if gains.iter().all(|g| *g == 0.0) { return String::new(); }
    const F: [u32; 10] = [31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
    let entries = F.iter().zip(gains.iter()).map(|(f, g)| {
        let w = (*f as f64 * 0.7) as u32;
        format!("c0 f={f} w={w} g={g}|c1 f={f} w={w} g={g}")
    }).collect::<Vec<_>>().join("|");
    format!("anequalizer=params=%{}%{}", entries.len(), entries)
}

/// Live momentary loudness (LUFS) from the ebur128 visualizer filter, stored as
/// a normalized 0..1000 amplitude so the visualizer pulses to the actual audio.
static MUSIC_LOUDNESS: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);
/// LUFS → 0..1 envelope (≈ -45 dB silence .. -6 dB loud).
fn loudness_to_amp(lufs: f64) -> f32 { (((lufs + 45.0) / 39.0).clamp(0.0, 1.0)) as f32 }
/// Append the ebur128 metering filter (labelled `vis`) to the EQ `af` so we can
/// read real loudness. Empty EQ → just the meter.
fn music_full_af(eq_af: &str) -> String {
    const VIS: &str = "@vis:ebur128=metadata=1:video=0";
    if eq_af.is_empty() { VIS.to_string() } else { format!("{eq_af},{VIS}") }
}

/// Apply the current EQ gains to the live track + reflect them in the UI bands.
fn apply_music_eq(w: &MainWindow) {
    let gains = music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]);
    music_ipc(&["set_property", "af", &music_full_af(&music_eq_af(&gains))]);
    let bands: Vec<f32> = gains.iter().map(|g| *g as f32).collect();
    w.set_music_eq_bands(slint::ModelRc::new(slint::VecModel::from(bands)));
}

/// Saved custom EQ profiles, persisted as JSON in Settings.advanced.
fn load_eq_customs() -> Vec<(String, Vec<f64>)> {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.get("music.eq.custom")
        .and_then(|j| serde_json::from_str::<Vec<(String, Vec<f64>)>>(j).ok())
        .unwrap_or_default()
}
fn save_eq_customs(list: &[(String, Vec<f64>)]) {
    if let Ok(j) = serde_json::to_string(list) { save_music_pref("music.eq.custom", &j); }
}
/// Publish the custom-profile names into the EQ popup.
fn populate_eq_customs(w: &MainWindow) {
    let names: Vec<slint::SharedString> = load_eq_customs().into_iter().map(|(n, _)| n.into()).collect();
    w.set_music_eq_custom_names(slint::ModelRc::new(slint::VecModel::from(names)));
}

/// Send a single JSON command to the live music mpv over its IPC socket.
fn music_ipc(args: &[&str]) {
    use std::io::Write;
    let Some(sock) = music_sock().lock().ok().and_then(|g| g.clone()) else { return; };
    let payload = format!("{{\"command\":[{}]}}\n",
        args.iter().map(|a| {
            // numbers/bools pass through; everything else is JSON-quoted.
            if a.parse::<f64>().is_ok() || **a == *"true" || **a == *"false" { a.to_string() }
            else { format!("\"{a}\"") }
        }).collect::<Vec<_>>().join(","));
    if let Ok(mut s) = mpv_ipc::connect(&sock) {
        let _ = s.write_all(payload.as_bytes());
    }
}

/// Same JSON-IPC push for the windowed VIDEO mpv ("tulipix-mpv" socket) —
/// drives PiP float / shader toggles on the external player window.
fn video_ipc(args: &[&str]) {
    use std::io::Write;
    let sock = mpv_ipc::endpoint("tulipix-mpv");
    let payload = format!("{{\"command\":[{}]}}\n",
        args.iter().map(|a| {
            if a.parse::<f64>().is_ok() || **a == *"true" || **a == *"false" { a.to_string() }
            else { format!("\"{a}\"") }
        }).collect::<Vec<_>>().join(","));
    if let Ok(mut s) = mpv_ipc::connect(&sock) {
        let _ = s.write_all(payload.as_bytes());
    }
}

/// Advance to the next track honoring shuffle + repeat (off/all/one). Called on
/// natural end-of-file and by the Next button.
/// Advance to the next track. If a play_queue has upcoming tracks (Instant Mix
/// or a persisted/manual queue), follow it — pop the front and play it; only
/// fall back to sequential/shuffle/repeat order when the queue is empty
/// (np.p5.music.instant-mix follow + np.p5.music.queue-persist).
fn advance_music(w: &MainWindow) {
    // "Repeat one" always re-plays the current track, ignoring the queue.
    if w.get_music_repeat() == "one" { advance_sequential(w); return; }
    // Shuffle overrides the sequential auto-queue — pick a random next track.
    if w.get_music_shuffle() { advance_sequential(w); return; }
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("music").await else {
            let _ = weak.upgrade_in_event_loop(|w| advance_sequential(&w)); return; };
        match tulipix_music::queue::pop_first(&pool).await {
            Ok(Some(next_id)) => {
                let _ = weak.upgrade_in_event_loop(move |w| {
                    let pos = music_ids().lock().ok()
                        .and_then(|g| g.iter().position(|id| *id == next_id)).map(|p| p as i32);
                    match pos {
                        Some(p) => { play_music_at(&w, p); build_music_queue(&w); }
                        None => advance_sequential(&w),
                    }
                });
            }
            _ => { let _ = weak.upgrade_in_event_loop(|w| advance_sequential(&w)); }
        }
    });
}

/// Sequential / shuffle / repeat advance over the library list (the fallback
/// when no queue is active).
fn advance_sequential(w: &MainWindow) {
    let total = w.get_music_np_total();
    if total <= 0 { return; }
    let idx = w.get_music_np_index();
    let next = match w.get_music_repeat().as_str() {
        "one" => idx,
        _ if w.get_music_shuffle() && total > 1 => {
            let mut n = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos()).unwrap_or(0) as i32).rem_euclid(total);
            if n == idx { n = (n + 1).rem_euclid(total); }
            n
        }
        "all" => (idx + 1).rem_euclid(total),
        _ => { // off: stop at the end of the list
            if idx + 1 >= total { w.set_music_playing(false); return; }
            idx + 1
        }
    };
    play_music_at(w, next);
}

/// Play the music track at `idx`: stop the previous one, spawn a headless mpv
/// with a live IPC control socket, wire a reader thread that streams position /
/// duration / pause / volume into the now-playing bar, and auto-advances on EOF.
fn play_music_at(w: &MainWindow, idx: i32) {
    let (path, total) = {
        let Ok(g) = music_paths().lock() else { return; };
        let total = g.len() as i32;
        let Some(p) = g.get(idx as usize).cloned() else { return; };
        (p, total)
    };
    // Library track — not a YouTube video.
    if let Ok(mut g) = yt_cur_audio().lock() { g.clear(); }
    w.set_music_yt_now_video(false);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    // Stop the previous track.
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
    let sock = mpv_ipc::endpoint("tulipix-music");
    mpv_ipc::cleanup(&sock);
    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
    cmd.arg("--no-video").arg("--force-window=no").arg("--idle=no")
        .arg(format!("--input-ipc-server={}", sock.display()))
        .arg(format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32));
    // Carry the mute state across track changes (each track is a fresh mpv).
    if w.get_music_muted() { cmd.arg("--mute=yes"); }
    stop_video(); // music takes over the universal stream from any video
    mpv_die_with_parent(&mut cmd);
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    // Always attach the ebur128 meter (+ EQ if any) so the visualizer pulses to
    // the real audio loudness (np.p5.music.visualizer — beat sync).
    let eq_af = music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]));
    cmd.arg(format!("--af={}", music_full_af(&eq_af)));
    // Apply persisted audio config: device / exclusive / gapless / replaygain
    // (np.p5.music.output / .gapless / .replaygain).
    for a in music_audio_args(&s) { cmd.arg(a); }
    match cmd.arg(&path).spawn() {
        Ok(child) => { if let Ok(mut g) = music_proc().lock() { *g = Some(child); } }
        Err(e) => { tracing::error!(error = %e, "mpv audio launch failed"); return; }
    }
    if let Ok(mut g) = music_sock().lock() { *g = Some(sock.clone()); }

    // Untagged-file ReplayGain (np.p5.music.replaygain): mpv only honours RG
    // tags inside the file; feed the DB-computed gain through
    // `replaygain-fallback` so scanned-but-tagless tracks normalize too.
    {
        let mode = w.get_music_replaygain().to_string();
        let item_id = music_songs().lock().ok()
            .and_then(|g| g.iter().find(|s| s.pos == idx).map(|s| s.item_id));
        if let (Some(id), false) = (item_id, mode == "off") {
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("music").await else { return; };
                let Ok((track, album)) = tulipix_music::replaygain::gains_for(&pool, id).await else { return; };
                let gain = if mode == "album" { album.or(track) } else { track.or(album) };
                if let Some(g) = gain {
                    music_ipc(&["set_property", "replaygain-fallback", &format!("{g:.2}")]);
                }
            });
        }
    }

    // Reader thread: observe properties → UI; on socket close (mpv exited),
    // auto-advance if this track is still current.
    let weak = w.as_weak();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        if let Ok(mut stream) = mpv_ipc::connect(&sock) {
            let _ = stream.write_all(concat!(
                "{\"command\":[\"observe_property\",1,\"time-pos\"]}\n",
                "{\"command\":[\"observe_property\",2,\"duration\"]}\n",
                "{\"command\":[\"observe_property\",3,\"pause\"]}\n",
                "{\"command\":[\"observe_property\",4,\"volume\"]}\n",
                "{\"command\":[\"observe_property\",5,\"mute\"]}\n",
                "{\"command\":[\"observe_property\",6,\"af-metadata/vis/lavfi.r128.M\"]}\n").as_bytes());
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] != "property-change" { continue; }
                let name = v["name"].as_str().unwrap_or("").to_string();
                // Real loudness → atomic (handled in-thread, no event-loop hop per frame).
                if name == "af-metadata/vis/lavfi.r128.M" {
                    if let Some(l) = v["data"].as_str().and_then(|s| s.parse::<f64>().ok()) {
                        MUSIC_LOUDNESS.store((loudness_to_amp(l) * 1000.0) as i32, std::sync::atomic::Ordering::Relaxed);
                    }
                    continue;
                }
                let wk = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = wk.upgrade() else { return; };
                    match name.as_str() {
                        "time-pos" => if let Some(d) = v["data"].as_f64() {
                            w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into());
                            update_lyrics_active(&w); }
                        "duration" => if let Some(d) = v["data"].as_f64() {
                            w.set_music_dur(d as f32); w.set_music_dur_label(fmt_clock(d).into()); }
                        "pause"  => if let Some(p) = v["data"].as_bool() { w.set_music_playing(!p); media_set_playing(!p); }
                        "volume" => if let Some(d) = v["data"].as_f64() { w.set_music_volume(d as f32); }
                        "mute"   => if let Some(m) = v["data"].as_bool() { w.set_music_muted(m); }
                        _ => {}
                    }
                });
            }
        }
        // Socket closed = track ended (or was replaced). Auto-advance only if
        // this is still the active generation (natural EOF, not user action).
        if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) == my_gen {
            let _ = weak.upgrade_in_event_loop(|w| advance_music(&w));
        }
    });

    // Now-playing metadata — prefer real tags from the Songs store.
    let (title, artist) = music_songs().lock().ok()
        .and_then(|g| g.iter().find(|s| s.pos == idx).map(|s| (s.title.clone(), s.artist.clone())))
        .unwrap_or_else(|| (path.file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string(), String::new()));
    let thumb = tulipix_core::thumbs::render_or_cache(
        &path, tulipix_core::thumbs::ThumbSpec {
            kind: tulipix_core::thumbs::ThumbKind::Audio, width: 320, height: 320 })
        .ok().flatten().map(|t| t.path);
    // Audiobook chapter → the book's (possibly custom) cover is the vinyl art,
    // and the folder name drives book mode (second line = book title).
    let book_folder = path.parent().map(|d| d.display().to_string());
    let book_px = book_folder.as_ref()
        .and_then(|f| ab_cover_cache().lock().ok().and_then(|g| g.get(f).cloned()));
    let art = match &book_px {
        Some(px) => slint::Image::from_rgba8(px.clone()),
        None => thumb.as_ref()
            .map(|p| slint::Image::load_from_path(p).unwrap_or_default()).unwrap_or_default(),
    };
    // Dynamic accent from the cover (np.p5.atmusic.art-gradient).
    let accent = thumb.as_deref().and_then(dominant_color).unwrap_or(slint::Color::from_rgb_u8(0xec, 0x48, 0x99));
    w.set_music_np_accent(accent);
    w.set_music_np_title(title.into());
    // Audiobook chapters carry the book title on the second line (and survive
    // chapter auto-advance, which re-enters here); plain tracks show the artist.
    let is_book = book_px.is_some();
    w.set_music_np_sub(match (is_book, &book_folder) {
        (true, Some(f)) => book_title(f).into(),
        _ if artist.is_empty() => "Playing from your library".into(),
        _ => artist.into(),
    });
    w.set_music_np_art(art);
    w.set_music_np_index(idx);
    w.set_music_np_total(total);
    w.set_music_player_mode(if is_book { "book" } else { "music" }.into());
    w.set_music_radio_np_uuid("".into());    // a library track ends any radio LIVE state
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("0:00".into());
    w.set_music_np_loved(false);
    w.set_music_np_stars(0);
    w.set_music_np_album("".into());
    if let Some(id) = current_music_id(w) {
        let weak = w.as_weak();
        let scrobble = w.get_music_scrobble_on();
        tokio::runtime::Handle::current().spawn(async move {
            let Ok(pool) = pool_for("music").await else { return; };
            let _ = tulipix_music::queue::record_play(&pool, id, 0).await;
            // Refresh the Home rails so "Recently played" reflects this play
            // immediately (it used to stay stale until app restart).
            let wk2 = weak.clone();
            let _ = wk2.upgrade_in_event_loop(move |w| populate_music_views(w.as_weak()));
            // Scrobble now-playing (np.p5.music.scrobble) — opt-in; the submitter
            // drains the pending queue to Last.fm / ListenBrainz.
            if scrobble {
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64).unwrap_or(0);
                let _ = tulipix_music::scrobble::enqueue(&pool, id, now).await;
                // Mirror to ListenBrainz when enabled, then drain its queue.
                let lbz = tulipix_core::settings::Settings::load().ok()
                    .map(|s| s.flags.get("api.listenbrainz").copied().unwrap_or(false)).unwrap_or(false);
                if lbz {
                    let _ = tulipix_music::listenbrainz::enqueue(&pool, id, now).await;
                    submit_scrobbles();
                }
            }
            let row: Option<(i64, i64, Option<String>)> = sqlx::query_as(
                "SELECT COALESCE(tm.loved,0), COALESCE(tm.rating,0), al.title
                 FROM track_meta tm LEFT JOIN albums al ON al.id = tm.album_id
                 WHERE tm.item_id = ?")
                .bind(id).fetch_optional(&pool).await.ok().flatten();
            let (loved, stars, album) = row.unwrap_or((0, 0, None));
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_np_loved(loved != 0);
                w.set_music_np_stars(stars as i32);
                // Book mode keeps the second line as the bare book title.
                if w.get_music_player_mode().as_str() != "book" {
                    w.set_music_np_album(album.unwrap_or_default().into());
                }
            });
        });
    }
    // Always refresh lyrics for the new track so the now-playing lyric line
    // (above the seekbar) syncs automatically — independent of the side panel.
    load_music_lyrics(w);
}

/// Load the photo at `idx` from the current library list into the viewer,
/// updating the image, label, index and total. Shared by open/next/prev.
fn show_photo_at(w: &MainWindow, idx: i32) {
    let (path, total) = {
        let Ok(g) = photo_paths().lock() else { return; };
        let total = g.len() as i32;
        let Some(p) = g.get(idx as usize).cloned() else { return; };
        (p, total)
    };
    let img = slint::Image::load_from_path(&path).unwrap_or_default();
    let sz = img.size();
    w.set_viewer_image(img);
    w.set_viewer_nat_w(sz.width as i32);
    w.set_viewer_nat_h(sz.height as i32);
    w.set_viewer_label(path.file_name().and_then(|s| s.to_str()).unwrap_or("").into());
    w.set_viewer_index(idx);
    w.set_viewer_total(total);
    w.set_viewer_zoom(1.0); // reset zoom/pan on every photo change
    w.set_viewer_exif(format_exif(&path).into());
    w.set_viewer_histogram(histogram_image(&path));
}

/// Render a 256×100 RGB histogram for `path` into a Slint image. Channels are
/// drawn additively so overlapping bins brighten — the usual histogram look.
/// A decode failure yields a transparent image (the panel just shows empty).
fn histogram_image(path: &std::path::Path) -> slint::Image {
    let Ok(img) = image::open(path) else {
        use slint::{Rgba8Pixel, SharedPixelBuffer};
        return slint::Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::new(256, 100));
    };
    slint::Image::from_rgba8(histogram_buf(&img))
}

/// Render a 256×100 additive RGB histogram buffer from an already-decoded
/// image. Shared by the viewer/properties panels and the editor curve grid.
fn histogram_buf(img: &image::DynamicImage) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    use slint::{Rgba8Pixel, SharedPixelBuffer};
    const W: usize = 256;
    const H: usize = 100;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(W as u32, H as u32);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() { *p = Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 }; }

    let small = img.thumbnail(256, 256).to_rgb8();
    let (mut rh, mut gh, mut bh) = ([0u32; 256], [0u32; 256], [0u32; 256]);
    for p in small.pixels() {
        rh[p[0] as usize] += 1; gh[p[1] as usize] += 1; bh[p[2] as usize] += 1;
    }
    let maxv = rh.iter().chain(&gh).chain(&bh).copied().max().unwrap_or(1).max(1);
    for x in 0..W {
        for (count, (cr, cg, cb)) in [
            (rh[x], (210u16, 40, 40)),
            (gh[x], (40, 200, 90)),
            (bh[x], (50, 120, 230)),
        ] {
            let bar = ((count as f64 / maxv as f64) * (H as f64 - 1.0)).round() as usize;
            for y in (H - bar)..H {
                let idx = y * W + x;
                let c = px[idx];
                px[idx] = Rgba8Pixel {
                    r: (c.r as u16 + cr).min(255) as u8,
                    g: (c.g as u16 + cg).min(255) as u8,
                    b: (c.b as u16 + cb).min(255) as u8,
                    a: 235,
                };
            }
        }
    }
    buf
}

/// Build the viewer's Info panel — an exiftool-style readout of ~24 common
/// attributes. Missing tags render as an empty "—" so the layout is stable.
/// ~24 common photo attributes as (label, value) pairs. Missing tags render
/// as "—". Shared by the viewer info panel (joined string) and the Properties
/// window (structured rows).
fn exif_rows(path: &std::path::Path) -> Vec<(&'static str, String)> {
    use exif::{In, Reader, Tag};
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    let dims = image::image_dimensions(path).ok();
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_uppercase();
    let meta = std::fs::File::open(path).ok().and_then(|f| {
        let mut br = std::io::BufReader::new(f);
        Reader::new().read_from_container(&mut br).ok()
    });
    let g = |tag: Tag| -> String {
        meta.as_ref()
            .and_then(|e| e.get_field(tag, In::PRIMARY).map(|f| f.display_value().with_unit(e).to_string()))
            .unwrap_or_else(|| "—".into())
    };
    let dim_str = dims.map(|(w, h)| format!("{w} × {h}")).unwrap_or_else(|| "—".into());
    vec![
        ("File",        path.file_name().and_then(|s| s.to_str()).unwrap_or("—").to_string()),
        ("Format",      if ext.is_empty() { "—".into() } else { ext }),
        ("Size",        human_size(size)),
        ("Dimensions",  dim_str),
        ("Date taken",  g(Tag::DateTimeOriginal)),
        ("Date digit.", g(Tag::DateTimeDigitized)),
        ("Camera make", g(Tag::Make)),
        ("Camera model",g(Tag::Model)),
        ("Lens",        g(Tag::LensModel)),
        ("ISO",         g(Tag::PhotographicSensitivity)),
        ("Aperture",    g(Tag::FNumber)),
        ("Shutter",     g(Tag::ExposureTime)),
        ("Exp. program",g(Tag::ExposureProgram)),
        ("Exp. comp.",  g(Tag::ExposureBiasValue)),
        ("Metering",    g(Tag::MeteringMode)),
        ("Flash",       g(Tag::Flash)),
        ("Focal length",g(Tag::FocalLength)),
        ("Focal 35mm",  g(Tag::FocalLengthIn35mmFilm)),
        ("White bal.",  g(Tag::WhiteBalance)),
        ("Color space", g(Tag::ColorSpace)),
        ("Orientation", g(Tag::Orientation)),
        ("GPS",         {
            let lat = g(Tag::GPSLatitude); let lon = g(Tag::GPSLongitude);
            if lat == "—" && lon == "—" { "—".into() } else { format!("{lat}, {lon}") }
        }),
        ("Software",    g(Tag::Software)),
        ("Artist",      g(Tag::Artist)),
    ]
}

fn format_exif(path: &std::path::Path) -> String {
    exif_rows(path).iter().map(|(k, v)| format!("{k:<13}{v}")).collect::<Vec<_>>().join("\n")
}

/// Read the five editable metadata fields for the editor's Meta panel.
/// Returns (artist, copyright, description, comment, date). Missing → "".
fn exif_edit_fields(path: &std::path::Path) -> (String, String, String, String, String) {
    use exif::{In, Reader, Tag};
    let meta = std::fs::File::open(path).ok().and_then(|f| {
        let mut br = std::io::BufReader::new(f);
        Reader::new().read_from_container(&mut br).ok()
    });
    let g = |tag: Tag| -> String {
        meta.as_ref()
            .and_then(|e| e.get_field(tag, In::PRIMARY).map(|f| f.display_value().to_string()))
            .map(|s| s.trim().trim_matches('"').to_string())
            .unwrap_or_default()
    };
    (
        g(Tag::Artist),
        g(Tag::Copyright),
        g(Tag::ImageDescription),
        g(Tag::UserComment),
        g(Tag::DateTimeOriginal),
    )
}

/// Pick a random index in 0..total using a time-seeded xorshift (no rand dep).
fn rand_index(total: i32) -> i32 {
    if total <= 1 { return 0; }
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
    x ^= x << 13; x ^= x >> 7; x ^= x << 17;
    (x % total as u64) as i32
}

// ── Settings + overlay helpers ─────────────────────────────────────────────

/// Wall-clock HH:MM in the OS-local timezone (chrono::Local reads the system
/// TZ — e.g. Asia/Kolkata → IST +05:30), not UTC.
fn clock_now() -> String {
    chrono::Local::now().format("%H:%M").to_string()
}

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
fn seed_api_rows(w: &MainWindow) {
    use tulipix_core::api_keys::{self, KeySource};
    let rows: Vec<ApiKeyRow> = api_keys::SERVICES.iter().map(|svc| {
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
        ("go:settings:appearance", "Appearance", "Settings → Appearance", "Settings"),
        ("go:settings:libraries",  "Libraries",  "Settings → Libraries",  "Settings"),
        ("go:settings:schedule",   "Schedule",   "Settings → Schedule",   "Settings"),
        ("go:settings:api-keys",   "API Keys",   "Settings → API Keys",   "Settings"),
        ("go:settings:profile",    "Profile",    "Settings → Profile",    "Settings"),
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
fn fuzzy_score(q: &str, hay: &str) -> Option<f32> {
    let hay: Vec<char> = hay.chars().collect();
    let mut hi = 0usize;
    let mut score = 0.0f32;
    let mut run = 0.0f32;
    for qc in q.chars() {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi] == qc {
                run += 1.0;
                score += run + (1.0 / (hi as f32 + 1.0));
                hi += 1;
                found = true;
                break;
            }
            run = 0.0;
            hi += 1;
        }
        if !found { return None; }
    }
    Some(score)
}

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

/// Human-readable byte size: "2.89 MB (3,031,744 bytes)".
fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    let (val, unit) = if b >= 1_073_741_824.0 { (b / 1_073_741_824.0, "GB") }
        else if b >= 1_048_576.0 { (b / 1_048_576.0, "MB") }
        else if b >= 1024.0 { (b / 1024.0, "KB") }
        else { (b, "bytes") };
    // Thousands-separated raw byte count.
    let mut raw = String::new();
    let digits = bytes.to_string();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { raw.push(','); }
        raw.push(c);
    }
    if unit == "bytes" { format!("{raw} bytes") } else { format!("{val:.2} {unit} ({raw} bytes)") }
}

/// Minimal URL query-component percent-encoder (spaces → +).
fn urlencoding(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "+".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

thread_local! {
    /// OS media-control handle, held on the UI thread so playback-state updates
    /// (which must mirror real state or the DE sends the wrong key event) and the
    /// initial registration share one connection.
    static MEDIA_CONTROLS: std::cell::RefCell<Option<souvlaki::MediaControls>> = const { std::cell::RefCell::new(None) };
}

/// Mirror the real playback state to the OS media surface so the desktop sends
/// the correct Play vs Pause event for the next media-key press.
fn media_set_playing(playing: bool) {
    MEDIA_CONTROLS.with(|c| {
        if let Some(ctrl) = c.borrow_mut().as_mut() {
            let st = if playing {
                souvlaki::MediaPlayback::Playing { progress: None }
            } else {
                souvlaki::MediaPlayback::Paused { progress: None }
            };
            let _ = ctrl.set_playback(st);
        }
    });
}

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
    }
}
fn hdr(label: &str) -> SettingItem { si("", "header", label, "", "", false, "") }
fn tog(s: &tulipix_core::settings::Settings, key: &str, def: bool, label: &str, desc: &str) -> SettingItem {
    si(key, "toggle", label, desc, "", s.flag(key, def), "")
}
fn txt(s: &tulipix_core::settings::Settings, key: &str, label: &str, desc: &str) -> SettingItem {
    si(key, "text", label, desc, &s.text(key), false, "")
}
fn stat(label: &str, value: &str, state: &str) -> SettingItem { si("", "status", label, "", value, false, state) }
fn act(key: &str, label: &str, desc: &str, btn: &str) -> SettingItem { si(key, "action", label, desc, btn, false, "") }

/// Seed every data-driven Settings panel from the persisted settings + live
/// runtime diagnostics. Cheap; re-run after any toggle/action.
fn seed_settings_panels(w: &MainWindow) {
    use slint::{ModelRc, VecModel};
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();

    // AI Models — np.p1.llm.*, np.p1.ai.update-*, np.p1.onboarding.ai-models.
    // Manifest-driven: one status row + Download/Update action per model.
    let mut ai = vec![hdr("ON-DEVICE MODELS")];
    for m in &ai_manifest().models {
        let installed = tulipix_photos::ai::models::is_installed(m);
        let mb = m.size_bytes / 1_000_000;
        ai.push(stat(&format!("{} v{} ({})", m.name, m.version, m.quant),
            &if installed { format!("Installed · {mb} MB") } else { format!("Not downloaded · {mb} MB") },
            if installed { "ok" } else { "muted" }));
        ai.push(act(&format!("ai-dl-{}", m.name),
            &format!("{} {}", if installed { "Re-verify / update" } else { "Download" }, m.name),
            &format!("Fetch + SHA-256 pin from {}", m.url.split('/').take(3).collect::<Vec<_>>().join("/")),
            if installed { "Verify" } else { "Download" }));
    }
    ai.extend([
        stat("Local LLM (Phi-3-mini-Q4)", "Not downloaded", "muted"),
        stat("CLIP ViT-B/32 (captions)", "Not downloaded", "muted"),
        act("ai-update-check", "Check for model updates", "Compare installed versions against the latest manifest", "Check"),
    ]);
    ai.extend([
        tog(&s, "ai.captions", false, "Auto-captioning & alt-text", "CLIP + LLM fill captions on import"),
        tog(&s, "ai.voice", false, "Voice search / dictation", "Whisper streaming mic capture → query"),
        tog(&s, "ai.chat", false, "Chat assistant overlay (Cmd/Ctrl+J)", "Grounded, tool-calling library assistant"),
        hdr("CLOUD"),
        tog(&s, "ai.cloud-offload", false, "Allow cloud-LLM offload", "Send selected queries to Anthropic / OpenAI / Gemini"),
    ]);
    w.set_ai_rows(ModelRc::new(VecModel::from(ai)));

    // Endpoints & providers — np.p1.api.*.
    let ep = vec![
        hdr("CUSTOM ENDPOINTS"),
        txt(&s, "api.update-channel", "Update / appcast URL", "Air-gapped or mirror manifest"),
        txt(&s, "api.sentry", "Sentry DSN", "Use your own crash service"),
        txt(&s, "api.nominatim", "Nominatim URL", "Self-hosted reverse-geocoding"),
        txt(&s, "api.radio-browser", "radio-browser mirror", "DNS-SRV health-pick by default"),
        txt(&s, "api.autoeq", "AutoEq DB URL", "Fork / mirror; default = upstream"),
        txt(&s, "api.tmdb-image-base", "TMDB image base URL", "CDN mirror for poster art"),
        hdr("MUSIC DISCOVERY KEYS"),
        txt(&s, "api.spotify-id", "Spotify client ID", "Optional — richer search / recommendations (np.p5.atmusic.discovery-keys)"),
        txt(&s, "api.spotify-secret", "Spotify client secret", "Paired with the client ID"),
        txt(&s, "api.youtube-data", "YouTube Data API key", "Optional — richer in-app YouTube search / metadata"),
        txt(&s, "api.piped-instance", "Piped instance URL", "YouTube browsing backend — default https://pipedapi.kavin.rocks"),
        hdr("OPTIONAL PROVIDERS"),
        tog(&s, "api.discogs", false, "Discogs (music metadata)", "Fallback when MusicBrainz misses"),
        tog(&s, "api.anidb", false, "AniDB (anime)", "Titles / episodes / ratings"),
        tog(&s, "api.anilist", false, "AniList (anime)", "GraphQL alternate source"),
        tog(&s, "api.subscene", false, "Subscene / Addic7ed (subtitles)", "When OpenSubtitles is rate-limited"),
        tog(&s, "api.trakt", false, "Trakt.tv watch tracking", "Alternative to local-only history"),
        tog(&s, "api.listenbrainz", false, "ListenBrainz scrobble", "Open-data Last.fm alternative"),
    ];
    w.set_endpoint_rows(ModelRc::new(VecModel::from(ep)));

    // Security — np.p1.idle-autolock, sec.passkey, db-encrypt, sec.sandbox.
    let idle_secs_val = if s.idle_lock_secs > 0 { s.idle_lock_secs.to_string() } else { String::new() };
    let sec = vec![
        hdr("LOCK"),
        tog(&s, "autolock", false, "Auto-lock when idle", "Lock + show the screensaver after the timeout"),
        si("idle_lock_secs", "text", "Idle timeout (seconds)", "0 or blank = use the ambient default", &idle_secs_val, false, ""),
        hdr("AUTHENTICATION"),
        tog(&s, "passkey", false, "Passkey / FIDO2 unlock", "WebAuthn — YubiKey, Titan, platform authenticator"),
        hdr("DATA AT REST"),
        tog(&s, "db-encrypt", false, "Encrypt databases at rest", "SQLCipher-equivalent wrap (applies on next open)"),
        stat("OS sandbox", "Not applicable on Linux", "muted"),
    ];
    w.set_security_rows(ModelRc::new(VecModel::from(sec)));

    // Data & tools — backup/restore, export, migration, bug-report, multi-user.
    let data = vec![
        hdr("BACKUP"),
        act("backup", "Back up Tulipix data", "Copy settings + watched folders into the data dir", "Back up"),
        act("export", "Export library to JSON", "Portable per-section dump of rows + edits", "Export"),
        hdr("IMPORT"),
        act("migration", "Migration import wizard", "Import from Plex (libraries.db) or Picasa (.pmp)", "Import"),
        hdr("DIAGNOSTICS"),
        act("bug-report", "Bug report with logs", "Reveal the last logs + system info to attach", "Open"),
        tog(&s, "multi-user", false, "Multiple local users", "Separate Tulipix profiles per OS account"),
    ];
    w.set_data_rows(ModelRc::new(VecModel::from(data)));

    // System & performance — live diagnostics + bundled tools + platform.
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
        tog(&s, "power-aware", true, "Battery / network aware", "Pause indexer + transcoder on battery / metered"),
        hdr("PLAYBACK"),
        txt(&s, "playback.sub-size", "Subtitle size (px)", "Embedded-player subtitle font size · default 28"),
        txt(&s, "playback.sub-color", "Subtitle colour", "#RRGGBB · default white · applies on next play"),
        tog(&s, "playback.audio-exclusive", false, "Exclusive audio output", "Bit-perfect device-exclusive output (ALSA hw / WASAPI / CoreAudio)"),
        tog(&s, "playback.interpolation", false, "Motion interpolation", "Smooth-motion frame interpolation — heavy on integrated GPUs"),
        tog(&s, "playback.upscale", false, "GLSL upscale shaders (Anime4K)", "Applies .glsl shaders on the next play — drop shader files in the folder below"),
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
        stat("Sibling subtitle auto-load", "On (.srt/.vtt/.ass next to video)", "ok"),
        txt(&s, "music.eq-preset", "Music equalizer", "flat · rock · pop · jazz · bass · treble · applies on next track"),
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
        hdr("PLUGINS"),
        stat("Plugin engine (WASM / Lua)",
             if cfg!(feature = "lazy-plugins") { "ABI v0 — runtimes compiled, load on first use" } else { "ABI v0 ready — rebuild with --features lazy-plugins to load" },
             if cfg!(feature = "lazy-plugins") { "ok" } else { "muted" }),
        act("open-logs", "Open log folder", "tracing JSON logs with daily rotation", "Open"),
    ];
    sys.shrink_to_fit();
    w.set_system_rows(ModelRc::new(VecModel::from(sys)));
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

/// Directory holding the per-OS bundled binaries (dev layout).
fn bundled_bin_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/bin/linux-x86_64"))
}
fn bundled_present(file: &str) -> bool { bundled_bin_dir().join(file).exists() }
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").map(|paths| {
        std::env::split_paths(&paths).any(|d| d.join(name).exists())
    }).unwrap_or(false)
}
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

// ════════════════════════════════════════════════════════════════════════════
// YouTube section (np.p4.music.youtube) — helpers, in-memory state, populators.
// ════════════════════════════════════════════════════════════════════════════

const YT_CACHE_KEEP: i64 = 60;   // newest N auto-cached videos kept; rest evicted
const YT_PAGE: usize = 5;        // Home search results revealed per "Load more"

/// Send-safe video row gathered off the UI thread (thumb is a file path).
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
struct YtVidData {
    id: String,
    channel_id: String, // owning channel ("" = unknown; set for recommended)
    title: String,
    channel: String,
    meta: String,      // "312K views · 8 years ago"
    info: String,      // ≤100-char blurb
    duration: String,  // "4:12"
    dur_s: i64,
    thumb: String,     // cached PNG path
    #[serde(default)] fmt: String,      // download container badge ("" = none)
    #[serde(default)] quality: String,  // download quality badge ("" = none)
}

#[derive(Default)]
struct YtSearchState {
    all: Vec<YtVidData>,
    nextpage: Option<String>,
    shown: usize,
}

fn yt_search_state() -> &'static std::sync::Mutex<YtSearchState> {
    static S: OnceLock<std::sync::Mutex<YtSearchState>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(YtSearchState::default()))
}
fn yt_vids() -> &'static std::sync::Mutex<std::collections::HashMap<String, YtVidData>> {
    static S: OnceLock<std::sync::Mutex<std::collections::HashMap<String, YtVidData>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
fn yt_remember(rows: &[YtVidData]) {
    if let Ok(mut m) = yt_vids().lock() { for r in rows { m.insert(r.id.clone(), r.clone()); } }
}
fn yt_lookup(id: &str) -> Option<YtVidData> {
    yt_vids().lock().ok().and_then(|m| m.get(id).cloned())
}

#[allow(dead_code)] // kept: api.piped-instance setting still surfaced; reserved for an optional Piped backend
fn piped_instance() -> String {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.get("api.piped-instance").map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "https://pipedapi.kavin.rocks".to_string())
}
/// Saved default watch resolution height (-999 = unset). Persisted in settings.
fn yt_default_res() -> i64 {
    tulipix_core::settings::Settings::load().ok()
        .and_then(|s| s.advanced.get("yt.default-res").cloned())
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(-999)
}
fn yt_store_default_res(h: Option<i64>) {
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    match h {
        Some(v) => { s.advanced.insert("yt.default-res".into(), v.to_string()); }
        None => { s.advanced.remove("yt.default-res"); }
    }
    let _ = s.save();
}
/// User-pinned Home-rail channels (most-recent first, max 9). Persisted CSV.
const YT_HOME_MAX: usize = 9;
fn yt_home_channels() -> Vec<String> {
    tulipix_core::settings::Settings::load().ok()
        .and_then(|s| s.advanced.get("yt.home-channels").cloned())
        .map(|v| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect())
        .unwrap_or_default()
}
fn yt_add_home_channel(id: &str) {
    let mut list = yt_home_channels();
    list.retain(|x| x != id);          // de-dup
    list.insert(0, id.to_string());    // newest on top
    list.truncate(YT_HOME_MAX);        // keep at most 9 (drops the last)
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert("yt.home-channels".into(), list.join(","));
    let _ = s.save();
}
fn yt_thumb_dir() -> std::path::PathBuf {
    tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("youtube_thumbs")
}
fn yt_media_dir() -> std::path::PathBuf {
    tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("youtube_cache")
}
fn yt_dl_dir() -> std::path::PathBuf {
    tulipix_core::paths::data_dir().unwrap_or_else(std::env::temp_dir).join("youtube_downloads")
}

fn yt_fmt_count(n: i64) -> String {
    if n >= 1_000_000 { format!("{:.1}M", n as f64 / 1e6) }
    else if n >= 1_000 { format!("{:.0}K", n as f64 / 1e3) }
    else { n.to_string() }
}
fn yt_fmt_dur(secs: i64) -> String {
    if secs <= 0 { return String::new(); }
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}
fn yt_fmt_meta(views: i64, uploaded: &str) -> String {
    let mut parts = Vec::new();
    if views > 0 { parts.push(format!("{} views", yt_fmt_count(views))); }
    if !uploaded.is_empty() { parts.push(uploaded.to_string()); }
    parts.join(" · ")
}
fn yt_trunc100(s: &str) -> String {
    if s.chars().count() > 100 { s.chars().take(100).collect::<String>() + "…" } else { s.to_string() }
}

/// Convert a Piped video into a Send-safe row, fetching its thumbnail as PNG.
async fn yt_vid_data(client: &reqwest::Client, dir: &std::path::Path,
                     v: &tulipix_music::youtube::piped::Video) -> YtVidData {
    let thumb = if v.thumbnail.is_empty() { String::new() }
        else { tulipix_music::youtube::thumbs::fetch_png(client, dir, &v.thumbnail).await.unwrap_or_default() };
    YtVidData {
        id: v.id.clone(), channel_id: String::new(), title: v.title.clone(), channel: v.channel.clone(),
        meta: yt_fmt_meta(v.views, &v.uploaded), info: yt_trunc100(&v.blurb),
        duration: yt_fmt_dur(v.duration), dur_s: v.duration, thumb,
        ..Default::default()
    }
}

fn yt_img(path: &str) -> slint::Image {
    if path.is_empty() { Default::default() }
    else { slint::Image::load_from_path(std::path::Path::new(path)).unwrap_or_default() }
}

/// Build a YtVideo VecModel (UI thread — loads images from disk).
fn yt_video_model(rows: &[YtVidData]) -> slint::ModelRc<YtVideo> {
    let v: Vec<YtVideo> = rows.iter().enumerate().map(|(i, d)| YtVideo {
        id: d.id.clone().into(), channel_id: d.channel_id.clone().into(),
        title: d.title.clone().into(), channel: d.channel.clone().into(),
        meta: d.meta.clone().into(), info: d.info.clone().into(), duration: d.duration.clone().into(),
        thumb: yt_img(&d.thumb), index: i as i32,
        fmt: d.fmt.clone().into(), quality: d.quality.clone().into(),
    }).collect();
    slint::ModelRc::new(slint::VecModel::from(v))
}

fn yt_cached_to_data(c: &tulipix_music::youtube::store::CachedVideo) -> YtVidData {
    YtVidData {
        id: c.video_id.clone(), channel_id: String::new(), title: c.title.clone(), channel: c.channel.clone(),
        meta: String::new(), info: String::new(), duration: yt_fmt_dur(c.duration),
        dur_s: c.duration, thumb: c.thumb_path.clone(),
        fmt: c.fmt.clone(), quality: c.quality.clone(),
    }
}

fn yt_reco() -> &'static std::sync::Mutex<Vec<YtVidData>> {
    static S: OnceLock<std::sync::Mutex<Vec<YtVidData>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Home "Recommended" — latest video from up to 10 subscribed channels. Cached
/// in-memory for the session so it's built once.
/// Home recommendations refresh at most once a day. Order: in-memory (this
/// session) → on-disk cache if <24h old (no network) → otherwise fetch the
/// latest from subs and persist with a timestamp.
const YT_RECO_TTL: u64 = 24 * 60 * 60;

fn yt_now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs()).unwrap_or(0)
}
fn yt_reco_cache_path() -> std::path::PathBuf {
    tulipix_core::paths::cache_dir().unwrap_or_else(std::env::temp_dir).join("youtube_reco.json")
}
#[derive(serde::Serialize, serde::Deserialize, Default)]
struct YtRecoCache { fetched: u64, items: Vec<YtVidData> }

fn yt_reco_load() -> Option<YtRecoCache> {
    serde_json::from_str(&std::fs::read_to_string(yt_reco_cache_path()).ok()?).ok()
}
fn yt_reco_save(items: &[YtVidData]) {
    let c = YtRecoCache { fetched: yt_now_secs(), items: items.to_vec() };
    if let Ok(j) = serde_json::to_string(&c) {
        let p = yt_reco_cache_path();
        if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
        let _ = std::fs::write(p, j);
    }
}

fn populate_yt_recommended(w: &MainWindow) {
    // 1. In-memory (this session) — instant, no I/O.
    let cached = yt_reco().lock().map(|g| g.clone()).unwrap_or_default();
    if !cached.is_empty() { w.set_music_yt_recommended(yt_video_model(&cached)); return; }
    // 2. On-disk daily cache — reuse if younger than the TTL, no network.
    if let Some(c) = yt_reco_load() {
        if !c.items.is_empty() && yt_now_secs().saturating_sub(c.fetched) < YT_RECO_TTL {
            yt_remember(&c.items);
            if let Ok(mut g) = yt_reco().lock() { *g = c.items.clone(); }
            w.set_music_yt_recommended(yt_video_model(&c.items));
            return;
        }
    }
    // 3. Stale or absent — fetch the day's picks from subs and persist them.
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let subs: Vec<_> = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default()
            .into_iter().filter(|s| s.subscribed).collect();
        if subs.is_empty() { return; }
        let client = reqwest::Client::new();
        let dir = yt_thumb_dir();
        let mut out: Vec<YtVidData> = Vec::new();
        for s in subs.iter().take(10) {
            let vids = ytdlp_channel_latest(&s.channel_id, 1).await;
            if let Some(v) = vids.first() {
                let mut d = yt_vid_data(&client, &dir, v).await;
                d.channel_id = s.channel_id.clone();
                if d.channel.is_empty() { d.channel = s.title.clone(); }
                out.push(d);
            }
            if out.len() >= 10 { break; }
        }
        if out.is_empty() { return; }
        yt_remember(&out);
        yt_reco_save(&out);
        if let Ok(mut g) = yt_reco().lock() { *g = out.clone(); }
        let _ = weak.upgrade_in_event_loop(move |w| w.set_music_yt_recommended(yt_video_model(&out)));
    });
}

fn populate_yt_recent(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let recents = tulipix_music::youtube::store::list_recent_searches(&pool, 6).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let m: Vec<slint::SharedString> = recents.iter().map(|q| q.clone().into()).collect();
            w.set_music_yt_recent(slint::ModelRc::new(slint::VecModel::from(m)));
        });
    });
}

fn populate_yt_cached(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let rows: Vec<YtVidData> = tulipix_music::youtube::store::list_cached(&pool, 200).await
            .unwrap_or_default().iter().map(yt_cached_to_data).collect();
        yt_remember(&rows);
        let _ = weak.upgrade_in_event_loop(move |w| {
            let home: Vec<YtVidData> = rows.iter().take(3).cloned().collect();
            w.set_music_yt_cached(yt_video_model(&rows));
            w.set_music_yt_home_cached(yt_video_model(&home));
        });
    });
}

const YT_DL_LIST_PAGE: i64 = 10;

fn populate_yt_downloads(w: &MainWindow) {
    let weak = w.as_weak();
    let sort = w.get_music_yt_downloads_sort().to_string();
    let page = w.get_music_yt_downloads_page().max(0) as i64;
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        // DB returns newest-downloaded first; reorder client-side per the sort pill.
        let mut rows: Vec<YtVidData> = tulipix_music::youtube::store::list_downloads(&pool, 10000).await
            .unwrap_or_default().iter().map(yt_cached_to_data).collect();
        match sort.as_str() {
            "old" => rows.reverse(),
            "az" => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            _ => {} // "new" = DB order (newest first)
        }
        yt_remember(&rows);
        let home: Vec<YtVidData> = rows.iter().take(3).cloned().collect();
        let pages = ((rows.len() as i64 + YT_DL_LIST_PAGE - 1) / YT_DL_LIST_PAGE).max(1);
        let page = page.min(pages - 1);
        let start = (page * YT_DL_LIST_PAGE) as usize;
        let pagerows: Vec<YtVidData> = rows.into_iter().skip(start).take(YT_DL_LIST_PAGE as usize).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_yt_downloads_pages(pages as i32);
            w.set_music_yt_downloads_page(page as i32);
            w.set_music_yt_downloads(yt_video_model(&pagerows));
            w.set_music_yt_home_downloads(yt_video_model(&home));
        });
    });
}

const YT_PL_PER_PAGE: i64 = 10;

#[derive(Default)]
struct YtPlOpen { id: i64, source_url: Option<String>, total: i64 }
fn yt_pl_open() -> &'static std::sync::Mutex<YtPlOpen> {
    static S: OnceLock<std::sync::Mutex<YtPlOpen>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(YtPlOpen::default()))
}

fn yt_item_to_data(it: &tulipix_music::youtube::store::PlaylistItem) -> YtVidData {
    YtVidData {
        id: it.video_id.clone(), channel_id: String::new(), title: it.title.clone(), channel: it.channel.clone(),
        meta: String::new(), info: String::new(), duration: yt_fmt_dur(it.duration),
        dur_s: it.duration, thumb: it.thumb_path.clone(),
        ..Default::default()
    }
}

/// Load one page (10) of the open playlist into the detail view. Remote playlists
/// fetch windows via yt-dlp (cached); local playlists page the DB and lazily fill
/// missing metadata for just the shown items. Sort/search apply to the loaded page.
fn yt_playlist_load(weak: slint::Weak<MainWindow>, page: i64, sort: String, query: String) {
    tokio::runtime::Handle::current().spawn(async move {
        let (id, source_url, total) = { let g = yt_pl_open().lock().unwrap(); (g.id, g.source_url.clone(), g.total) };
        let Ok(pool) = pool_for("youtube").await else { return; };
        let page = page.max(0);
        let offset = page * YT_PL_PER_PAGE;
        let client = reqwest::Client::new();
        let dir = yt_thumb_dir();
        let mut rows: Vec<YtVidData> = if let Some(url) = &source_url {
            let cache = tulipix_music::youtube::store::get_playlist_cache(&pool, id).await.unwrap_or_default();
            let have: Vec<_> = cache.iter().skip(offset as usize).take(YT_PL_PER_PAGE as usize).cloned().collect();
            if !have.is_empty() {
                have.iter().map(yt_channelvid_to_data).collect()
            } else {
                let vids = ytdlp_playlist_window(url, offset + 1, offset + YT_PL_PER_PAGE).await;
                let mut cv = Vec::with_capacity(vids.len());
                for v in &vids { cv.push(yt_data_to_channelvid(&yt_vid_data(&client, &dir, v).await)); }
                let _ = tulipix_music::youtube::store::set_playlist_cache_window(&pool, id, offset, &cv).await;
                cv.iter().map(yt_channelvid_to_data).collect()
            }
        } else {
            let items = tulipix_music::youtube::store::playlist_items_page(&pool, id, offset, YT_PL_PER_PAGE).await.unwrap_or_default();
            let mut out = Vec::with_capacity(items.len());
            for it in &items {
                if it.title == it.video_id || it.thumb_path.is_empty() {
                    if let Some(v) = ytdlp_video_meta(&it.video_id).await {
                        let d = yt_vid_data(&client, &dir, &v).await;
                        let _ = tulipix_music::youtube::store::update_playlist_item_meta(&pool, id, &it.video_id, &d.title, &d.channel, &d.thumb, d.dur_s).await;
                        out.push(d);
                    } else { out.push(yt_item_to_data(it)); }
                } else { out.push(yt_item_to_data(it)); }
            }
            out
        };
        // Page-local sort + filter.
        let q = query.trim().to_lowercase();
        if !q.is_empty() { rows.retain(|r| r.title.to_lowercase().contains(&q)); }
        match sort.as_str() {
            "title" => rows.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
            "duration" => rows.sort_by(|a, b| b.dur_s.cmp(&a.dur_s)),
            _ => {}
        }
        yt_remember(&rows);
        let has_next = (offset + YT_PL_PER_PAGE) < total;
        let sub = format!("{total} videos");
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_yt_playlist_videos(yt_video_model(&rows));
            w.set_music_yt_playlist_page((page + 1) as i32);
            w.set_music_yt_playlist_has_next(has_next);
            w.set_music_yt_playlist_sub(sub.into());
        });
    });
}

fn populate_yt_playlists(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let pls = tulipix_music::youtube::store::list_playlists(&pool).await.unwrap_or_default();
        let rows: Vec<(i64, String, String, i64)> = pls.iter()
            .map(|p| (p.id, p.name.clone(), p.cover.clone().unwrap_or_default(), p.count)).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let m: Vec<YtPlaylist> = rows.iter().enumerate().map(|(i, (id, name, cover, count))| YtPlaylist {
                id: *id as i32, name: name.clone().into(), cover: yt_img(cover),
                sub: format!("{} video{}", count, if *count == 1 { "" } else { "s" }).into(),
                index: i as i32,
            }).collect();
            w.set_music_yt_playlists(slint::ModelRc::new(slint::VecModel::from(m)));
        });
    });
}

const YT_SUBS_PER_PAGE: usize = 15;

fn yt_sub_to_model(s: &tulipix_music::youtube::store::Sub, i: usize) -> YtSub {
    let videos = match s.video_count { Some(n) if n > 0 => format!("{n} videos"), _ => "—".to_string() };
    let subs = match s.sub_count { Some(n) if n > 0 => format!("{} subscribers", yt_fmt_count(n)), _ => String::new() };
    YtSub {
        channel_id: s.channel_id.clone().into(),
        title: s.title.clone().into(),
        avatar: yt_img(&s.avatar_path.clone().unwrap_or_default()),
        count: videos.into(),
        subs: subs.into(),
        subscribed: s.subscribed,
        index: i as i32,
    }
}

/// Sort + paginate the subscriptions and push the page slice (30) + Home rail (10).
fn yt_set_subs(w: &MainWindow, subs: &[tulipix_music::youtube::store::Sub]) {
    let sort = w.get_music_yt_subs_sort().to_string();
    let asc = w.get_music_yt_subs_dir() != "desc";
    let mut sorted: Vec<&tulipix_music::youtube::store::Sub> = subs.iter().collect();
    // Sort ascending by the chosen key (title as tiebreaker), then reverse for desc.
    match sort.as_str() {
        "videos" => sorted.sort_by(|a, b| a.video_count.unwrap_or(0).cmp(&b.video_count.unwrap_or(0))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
        "subscribers" => sorted.sort_by(|a, b| a.sub_count.unwrap_or(0).cmp(&b.sub_count.unwrap_or(0))
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
        // Subscribed first (asc), then unsubscribed; title tiebreaker.
        "status" => sorted.sort_by(|a, b| b.subscribed.cmp(&a.subscribed)
            .then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))),
        _ => sorted.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
    }
    if !asc { sorted.reverse(); }
    // Header count reflects only channels you're actually subscribed to.
    w.set_music_yt_sub_count(sorted.iter().filter(|s| s.subscribed).count() as i32);
    // Home rail = user-pinned channels (newest first, max 9); default to the
    // most-followed subscriptions when nothing's pinned yet.
    let pins = yt_home_channels();
    let home_subs: Vec<&tulipix_music::youtube::store::Sub> = if pins.is_empty() {
        sorted.iter().filter(|s| s.subscribed).take(YT_HOME_MAX).copied().collect()
    } else {
        pins.iter().filter_map(|pid| subs.iter().find(|s| &s.channel_id == pid)).take(YT_HOME_MAX).collect()
    };
    let home: Vec<YtSub> = home_subs.iter().enumerate().map(|(i, s)| yt_sub_to_model(s, i)).collect();
    w.set_music_yt_home_subs(slint::ModelRc::new(slint::VecModel::from(home)));
    let pages = sorted.len().div_ceil(YT_SUBS_PER_PAGE).max(1);
    let page = (w.get_music_yt_subs_page().max(0) as usize).min(pages - 1);
    w.set_music_yt_subs_pages(pages as i32);
    w.set_music_yt_subs_page(page as i32);
    let slice: Vec<YtSub> = sorted.iter().skip(page * YT_SUBS_PER_PAGE).take(YT_SUBS_PER_PAGE)
        .enumerate().map(|(i, s)| yt_sub_to_model(s, i)).collect();
    w.set_music_yt_subs(slint::ModelRc::new(slint::VecModel::from(slice)));
}

// Subscriptions page: list + display only. No network — counts come from DB
// (filled once at import time by `yt_fetch_sub_meta`).
fn populate_yt_subs(w: &MainWindow) {
    let weak = w.as_weak();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let subs = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
        let _ = weak.upgrade_in_event_loop(move |w| yt_set_subs(&w, &subs));
    });
}

// One-time metadata fetch (avatar + video count) for channels lacking it, with a
// live progress bar. Runs at import time only — keeps the Subscriptions page cheap.
fn yt_fetch_sub_meta(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("youtube").await else { return; };
        let subs = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
        let need: Vec<String> = subs.iter().filter(|s| s.fetched_at.is_none()).map(|s| s.channel_id.clone()).collect();
        if need.is_empty() { return; }
        let total = need.len();
        let client = reqwest::Client::new();
        let dir = yt_thumb_dir();
        let wk = weak.clone();
        let _ = wk.upgrade_in_event_loop(move |w| { w.set_music_yt_fetch_busy(true); w.set_music_yt_fetch_frac(0.0);
            w.set_music_yt_fetch_msg(format!("Fetching 0 / {total} channels…").into()); });
        for (i, cid) in need.into_iter().enumerate() {
            if let Some((avatar_url, followers, video_count)) = ytdlp_channel_meta(&cid).await {
                let avatar = if avatar_url.is_empty() { None }
                    else { tulipix_music::youtube::thumbs::fetch_png(&client, &dir, &avatar_url).await.ok() };
                let _ = tulipix_music::youtube::store::set_sub_meta(&pool, &cid, avatar.as_deref(), Some(video_count), Some(followers)).await;
            } else {
                let _ = tulipix_music::youtube::store::set_sub_meta(&pool, &cid, None, Some(0), Some(0)).await;
            }
            let done = i + 1;
            let frac = done as f32 / total as f32;
            if done % 3 == 0 || done == total {
                let subs_now = tulipix_music::youtube::store::list_subs(&pool).await.unwrap_or_default();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    yt_set_subs(&w, &subs_now);
                    w.set_music_yt_fetch_frac(frac);
                    w.set_music_yt_fetch_msg(format!("Fetching {done} / {total} channels…").into());
                });
            }
        }
        let _ = weak.upgrade_in_event_loop(|w| w.set_music_yt_fetch_busy(false));
    });
}

/// Render the current search stash (first `shown` items) into the Home center.
fn yt_render_search(w: &MainWindow) {
    let (rows, more): (Vec<YtVidData>, bool) = {
        let st = yt_search_state().lock().unwrap();
        let shown = st.shown.min(st.all.len());
        (st.all[..shown].to_vec(), shown < st.all.len() || st.nextpage.is_some())
    };
    w.set_music_yt_results(yt_video_model(&rows));
    w.set_music_yt_results_more(more);
    w.set_music_yt_busy(false);
}

// Channel-scoped search stash — separate from the cached "latest" list so the
// latest videos are never clobbered; results page 10 at a time up to 20.
#[derive(Default)]
struct YtChSearch { all: Vec<YtVidData>, shown: usize }
fn yt_ch_search_state() -> &'static std::sync::Mutex<YtChSearch> {
    static S: OnceLock<std::sync::Mutex<YtChSearch>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(YtChSearch::default()))
}
fn yt_render_channel_search(w: &MainWindow) {
    let (rows, more): (Vec<YtVidData>, bool) = {
        let st = yt_ch_search_state().lock().unwrap();
        let shown = st.shown.min(st.all.len());
        (st.all[..shown].to_vec(), shown < st.all.len())
    };
    w.set_music_yt_channel_results(yt_video_model(&rows));
    w.set_music_yt_channel_results_more(more);
    w.set_music_yt_busy(false);
}

async fn yt_dlp_fetch_audio(id: &str, dir: &std::path::Path) -> Option<String> {
    let _ = std::fs::create_dir_all(dir);
    let out = dir.join(format!("{id}.opus"));
    if out.exists() { return Some(out.to_string_lossy().into_owned()); }
    let url = format!("https://www.youtube.com/watch?v={id}");
    let tmpl = dir.join(format!("{id}.%(ext)s"));
    let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
    let _ = tokio::process::Command::new(bin)
        .arg("-f").arg("bestaudio").arg("-x").arg("--audio-format").arg("opus")
        .arg("--no-playlist").arg("-o").arg(&tmpl).arg(&url).status().await;
    if out.exists() { Some(out.to_string_lossy().into_owned()) } else { None }
}

/// Resolve a direct best-audio stream URL (no download) so playback can start
/// instantly — mpv streams the googlevideo URL while we cache the file in the
/// background (np.p4.music.youtube — instant audio, video-style streaming).
async fn yt_dlp_stream_url(id: &str) -> Option<String> {
    let url = format!("https://www.youtube.com/watch?v={id}");
    let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
    let out = tokio::process::Command::new(bin)
        .arg("-g").arg("-f").arg("bestaudio/best").arg("--no-playlist").arg(&url)
        .output().await.ok()?;
    if !out.status.success() { return None; }
    String::from_utf8_lossy(&out.stdout).lines().map(|l| l.trim().to_string())
        .find(|l| !l.is_empty())
}

fn yt_data_to_channelvid(d: &YtVidData) -> tulipix_music::youtube::store::ChannelVid {
    tulipix_music::youtube::store::ChannelVid {
        video_id: d.id.clone(), title: d.title.clone(), channel: d.channel.clone(),
        meta: d.meta.clone(), info: d.info.clone(), thumb_path: d.thumb.clone(), duration: d.dur_s,
    }
}
fn yt_channelvid_to_data(c: &tulipix_music::youtube::store::ChannelVid) -> YtVidData {
    YtVidData {
        id: c.video_id.clone(), channel_id: String::new(), title: c.title.clone(), channel: c.channel.clone(),
        meta: c.meta.clone(), info: c.info.clone(), duration: yt_fmt_dur(c.duration),
        dur_s: c.duration, thumb: c.thumb_path.clone(),
        ..Default::default()
    }
}

// ── Download queue (sequential, with progress) ──────────────────────────────
#[derive(Clone)]
struct YtDlJobData {
    id: String, title: String, channel: String, thumb: String,
    height: i64, frac: f32, status: String,
}
fn yt_dl_jobs() -> &'static std::sync::Mutex<Vec<YtDlJobData>> {
    static S: OnceLock<std::sync::Mutex<Vec<YtDlJobData>>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
static YT_DL_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn yt_dl_refresh(weak: &slint::Weak<MainWindow>) {
    let rows: Vec<YtDlJobData> = yt_dl_jobs().lock().map(|g| g.clone()).unwrap_or_default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        // The in-flight download (first row) drives the per-card Save progress bar.
        let (aid, afrac) = rows.first().map(|j| (j.id.clone(), j.frac)).unwrap_or_default();
        w.set_music_yt_dl_active_id(aid.into());
        w.set_music_yt_dl_active_frac(afrac);
        let m: Vec<YtDlJob> = rows.iter().map(|j| YtDlJob {
            title: j.title.clone().into(), thumb: yt_img(&j.thumb),
            status: j.status.clone().into(), frac: j.frac,
        }).collect();
        w.set_music_yt_dl_jobs(slint::ModelRc::new(slint::VecModel::from(m)));
    });
}

fn parse_ytdlp_pct(line: &str) -> Option<f32> {
    let l = line.trim();
    if !l.starts_with("[download]") { return None; }
    let p = l.find('%')?;
    let start = l[..p].rfind(' ')?;
    l[start..p].trim().parse::<f32>().ok().map(|v| (v / 100.0).clamp(0.0, 1.0))
}

fn yt_dl_set(id: &str, frac: f32, status: &str) {
    if let Ok(mut g) = yt_dl_jobs().lock() {
        if let Some(j) = g.iter_mut().find(|j| j.id == id) { j.frac = frac; j.status = status.to_string(); }
    }
}

fn yt_dl_enqueue(weak: slint::Weak<MainWindow>, job: YtDlJobData) {
    if let Ok(mut g) = yt_dl_jobs().lock() {
        if g.iter().any(|j| j.id == job.id) { return; }
        g.push(job);
    }
    yt_dl_refresh(&weak);
    if YT_DL_ACTIVE.swap(true, std::sync::atomic::Ordering::AcqRel) { return; }
    tokio::runtime::Handle::current().spawn(async move {
        loop {
            let job = { yt_dl_jobs().lock().ok().and_then(|g| g.first().cloned()) };
            let Some(job) = job else { break; };
            let dir = yt_dl_dir();
            let _ = std::fs::create_dir_all(&dir);
            yt_dl_set(&job.id, 0.0, "Starting…");
            yt_dl_refresh(&weak);
            let path = yt_dl_run(&job, &dir, &weak).await;
            if let Some(path) = path {
                if let Ok(pool) = pool_for("youtube").await {
                    let kind = if job.height < 0 { "audio" } else { "video" };
                    let (fmt, quality) = if job.height < 0 { ("OPUS", "Audio".to_string()) }
                        else if job.height == 0 { ("MKV", "Best".to_string()) }
                        else { ("MKV", format!("{}p", job.height)) };
                    let _ = tulipix_music::youtube::store::record_download(
                        &pool, &job.id, &job.title, &job.channel, &job.thumb, &path, 0, kind, fmt, &quality).await;
                }
            }
            if let Ok(mut g) = yt_dl_jobs().lock() { g.retain(|j| j.id != job.id); }
            yt_dl_refresh(&weak);
            let _ = weak.upgrade_in_event_loop(|w| populate_yt_downloads(&w));
        }
        YT_DL_ACTIVE.store(false, std::sync::atomic::Ordering::Release);
    });
}

async fn yt_dl_run(job: &YtDlJobData, dir: &std::path::Path, weak: &slint::Weak<MainWindow>) -> Option<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};
    let url = format!("https://www.youtube.com/watch?v={}", job.id);
    let tmpl = dir.join(format!("{}.%(ext)s", job.id));
    let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
    let mut cmd = tokio::process::Command::new(bin);
    let expected = if job.height < 0 {
        cmd.arg("-f").arg("bestaudio").arg("-x").arg("--audio-format").arg("opus");
        dir.join(format!("{}.opus", job.id))
    } else {
        let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
        let fmt = if job.height == 0 { "bestvideo+bestaudio/best".to_string() }
            else { format!("bestvideo[height<=?{0}][vcodec^=vp9]+bestaudio/bestvideo[height<=?{0}]+bestaudio/best[height<=?{0}]", job.height) };
        cmd.arg("-f").arg(fmt).arg("--merge-output-format").arg("mkv");
        // Only pin --ffmpeg-location to an absolute, existing binary; a bare name
        // ("ffmpeg") is NOT resolved against PATH by yt-dlp and silently breaks the
        // merge, leaving split .f###.m4a/.mp4 fragments and no .mkv → never recorded.
        if ff.is_absolute() && ff.exists() { cmd.arg("--ffmpeg-location").arg(&ff); }
        dir.join(format!("{}.mkv", job.id))
    };
    cmd.arg("--newline").arg("--no-playlist").arg("-o").arg(&tmpl).arg(&url)
        .stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null());
    let mut child = cmd.spawn().ok()?;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        let mut last = -1i32;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(frac) = parse_ytdlp_pct(&line) {
                let pct = (frac * 100.0) as i32;
                if pct != last { last = pct; yt_dl_set(&job.id, frac, &format!("Downloading {pct}%")); yt_dl_refresh(weak); }
            } else if line.contains("[Merger]") || line.contains("Merging") {
                yt_dl_set(&job.id, 0.99, "Merging…"); yt_dl_refresh(weak);
            }
        }
    }
    let _ = child.wait().await;
    if expected.exists() { return Some(expected.to_string_lossy().into_owned()); }
    // Fallback: the merged file should be `{id}.<ext>`. yt-dlp names split streams
    // `{id}.f###.<ext>`, so match by stem first; failing that, pick the largest
    // `{id}.*` file that isn't an in-progress fragment.
    if let Ok(rd) = std::fs::read_dir(dir) {
        let mut best: Option<(u64, std::path::PathBuf)> = None;
        for e in rd.flatten() {
            let p = e.path();
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if p.file_stem().and_then(|s| s.to_str()) == Some(job.id.as_str()) {
                return Some(p.to_string_lossy().into_owned());
            }
            if name.starts_with(&format!("{}.", job.id)) && !name.ends_with(".part") && !name.ends_with(".ytdl") {
                let sz = e.metadata().map(|m| m.len()).unwrap_or(0);
                if best.as_ref().map(|(b, _)| sz > *b).unwrap_or(true) { best = Some((sz, p)); }
            }
        }
        if let Some((_, p)) = best { return Some(p.to_string_lossy().into_owned()); }
    }
    None
}

// ── yt-dlp browsing backend (reliable, replaces flaky Piped at runtime) ──────
async fn ytdlp_json(args: Vec<String>) -> Option<serde_json::Value> {
    let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
    let out = tokio::process::Command::new(bin).args(&args).output().await.ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

fn yt_pick_thumb(v: &serde_json::Value) -> String {
    if let Some(t) = v.get("thumbnail").and_then(|x| x.as_str()) { if !t.is_empty() { return t.to_string(); } }
    if let Some(arr) = v.get("thumbnails").and_then(|x| x.as_array()) {
        for t in arr.iter().rev() { if let Some(u) = t.get("url").and_then(|x| x.as_str()) { return u.to_string(); } }
    }
    String::new()
}

/// Channel avatar: prefer a thumbnail whose id mentions "avatar", else first.
fn yt_pick_avatar(v: &serde_json::Value) -> String {
    if let Some(arr) = v.get("thumbnails").and_then(|x| x.as_array()) {
        for t in arr { if t.get("id").and_then(|x| x.as_str()).map(|s| s.contains("avatar")).unwrap_or(false) {
            if let Some(u) = t.get("url").and_then(|x| x.as_str()) { return u.to_string(); } } }
        if let Some(u) = arr.first().and_then(|t| t.get("url")).and_then(|x| x.as_str()) { return u.to_string(); }
    }
    yt_pick_thumb(v)
}

fn yt_entry_to_video(e: &serde_json::Value) -> tulipix_music::youtube::piped::Video {
    use tulipix_music::youtube::piped::Video;
    Video {
        id: e.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        title: e.get("title").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        channel: e.get("channel").or_else(|| e.get("uploader")).and_then(|x| x.as_str()).unwrap_or("").to_string(),
        duration: e.get("duration").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64,
        views: e.get("view_count").and_then(|x| x.as_i64()).unwrap_or(0),
        uploaded: String::new(),
        blurb: e.get("description").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        thumbnail: yt_pick_thumb(e),
        is_short: false,
    }
}

async fn ytdlp_search(query: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    let q = format!("ytsearch{n}:{query}");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(), q]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    tulipix_music::youtube::piped::without_shorts(vids)
}

/// Latest `n` videos of a channel via yt-dlp (cheap — flat, windowed to n).
async fn ytdlp_channel_latest(id: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    let url = format!("https://www.youtube.com/channel/{id}/videos");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-end".into(), n.to_string(), url]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    tulipix_music::youtube::piped::without_shorts(vids)
}

/// Search within a single channel.
async fn ytdlp_channel_search(id: &str, query: &str, n: usize) -> Vec<tulipix_music::youtube::piped::Video> {
    let q = query.trim().replace(' ', "%20");
    let url = format!("https://www.youtube.com/channel/{id}/search?query={q}");
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-end".into(), n.to_string(), url]).await else { return vec![]; };
    let vids = j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default();
    tulipix_music::youtube::piped::without_shorts(vids)
}

/// Remote playlist metadata: (title, video_count).
async fn ytdlp_playlist_meta(url: &str) -> Option<(String, i64)> {
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-items".into(), "0".into(), url.to_string()]).await?;
    let title = j.get("title").and_then(|x| x.as_str()).unwrap_or("Imported playlist").to_string();
    let count = j.get("playlist_count").and_then(|x| x.as_i64()).unwrap_or(0);
    Some((title, count))
}

/// One window [start..=end] (1-based) of a remote playlist's videos.
async fn ytdlp_playlist_window(url: &str, start: i64, end: i64) -> Vec<tulipix_music::youtube::piped::Video> {
    let Some(j) = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-start".into(), start.to_string(), "--playlist-end".into(), end.to_string(), url.to_string()]).await
        else { return vec![]; };
    j.get("entries").and_then(|e| e.as_array())
        .map(|arr| arr.iter().map(yt_entry_to_video).collect::<Vec<_>>()).unwrap_or_default()
}

/// Flat metadata for a single video id.
async fn ytdlp_video_meta(id: &str) -> Option<tulipix_music::youtube::piped::Video> {
    let url = format!("https://www.youtube.com/watch?v={id}");
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(), url]).await?;
    Some(yt_entry_to_video(&j))
}

/// Channel metadata: (avatar_url, follower_count, video_count).
async fn ytdlp_channel_meta(id: &str) -> Option<(String, i64, i64)> {
    let url = format!("https://www.youtube.com/channel/{id}");
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-items".into(), "0".into(), url]).await?;
    let followers = j.get("channel_follower_count").and_then(|x| x.as_i64()).unwrap_or(0);
    let video_count = j.get("playlist_count").and_then(|x| x.as_i64()).unwrap_or(0);
    Some((yt_pick_avatar(&j), followers, video_count))
}

/// Resolve any channel URL (/@handle, /channel/UC…, /c/…, /user/…, or a video
/// URL) into (channel_id, title) via yt-dlp.
async fn ytdlp_resolve_channel(url: &str) -> Option<(String, String)> {
    let j = ytdlp_json(vec!["--flat-playlist".into(), "-J".into(), "--no-warnings".into(),
        "--playlist-items".into(), "0".into(), url.to_string()]).await?;
    let cid = j.get("channel_id").and_then(|x| x.as_str())
        .or_else(|| j.get("uploader_id").and_then(|x| x.as_str()))
        .or_else(|| j.get("id").and_then(|x| x.as_str()))
        .unwrap_or("").to_string();
    if !cid.starts_with("UC") { return None; }
    let title = j.get("channel").and_then(|x| x.as_str())
        .or_else(|| j.get("uploader").and_then(|x| x.as_str()))
        .or_else(|| j.get("title").and_then(|x| x.as_str()))
        .unwrap_or("").to_string();
    Some((cid, title))
}

/// Play a downloaded file through the in-app mpv player (bottom/mini/zen),
/// setting now-playing metadata from the given strings.
fn yt_cur_audio() -> &'static std::sync::Mutex<String> {
    static S: OnceLock<std::sync::Mutex<String>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new(String::new()))
}

/// Active YouTube audio play-queue (video ids + current index). Empty = no queue;
/// a single tap clears it, Play-all fills it, EOF auto-advances to the next.
fn yt_queue() -> &'static std::sync::Mutex<(Vec<String>, usize)> {
    static S: OnceLock<std::sync::Mutex<(Vec<String>, usize)>> = OnceLock::new();
    S.get_or_init(|| std::sync::Mutex::new((Vec::new(), 0)))
}

/// Stream + play one YouTube video's audio (cached file preferred), caching the
/// opus in the background. Shared by single taps and queue advance.
fn yt_play_audio(weak: slint::Weak<MainWindow>, id: String) {
    tokio::runtime::Handle::current().spawn(async move {
        let dir = yt_media_dir();
        let meta = yt_lookup(&id).unwrap_or_default();
        let (title, channel, thumb) = (meta.title.clone(), meta.channel.clone(), meta.thumb.clone());
        let cached = dir.join(format!("{id}.opus"));
        if cached.exists() {
            let (p2, id2, t, c, th) = (cached.to_string_lossy().into_owned(), id.clone(), title.clone(), channel.clone(), thumb.clone());
            let _ = weak.upgrade_in_event_loop(move |w| yt_play_inapp(&w, id2, p2, t, c, th, 0.0));
            return;
        }
        if let Some(stream_url) = yt_dlp_stream_url(&id).await {
            let (u2, id2, t, c, th) = (stream_url, id.clone(), title.clone(), channel.clone(), thumb.clone());
            let _ = weak.upgrade_in_event_loop(move |w| yt_play_inapp(&w, id2, u2, t, c, th, 0.0));
        }
        let weak2 = weak.clone();
        tokio::runtime::Handle::current().spawn(async move {
            if let Some(path) = yt_dlp_fetch_audio(&id, &dir).await {
                if let Ok(pool) = pool_for("youtube").await {
                    let _ = tulipix_music::youtube::store::record_cached(
                        &pool, &id, &meta.title, &meta.channel, &meta.thumb, &path, meta.dur_s).await;
                    for (_v, p) in tulipix_music::youtube::store::evict_cached_over(&pool, YT_CACHE_KEEP).await.unwrap_or_default() {
                        let _ = std::fs::remove_file(&p);
                    }
                }
                let _ = weak2.upgrade_in_event_loop(|w| populate_yt_cached(&w));
            }
        });
    });
}

/// EOF reached on a queued track → play the next one (no-op if no queue/at end).
fn yt_queue_advance(weak: slint::Weak<MainWindow>) -> bool {
    let next = {
        let mut g = match yt_queue().lock() { Ok(g) => g, Err(_) => return false };
        if g.0.is_empty() || g.1 + 1 >= g.0.len() { return false; }
        g.1 += 1;
        g.0[g.1].clone()
    };
    yt_play_audio(weak, next);
    true
}

static YT_VID_POS: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

/// Watch a video fullscreen: stops the in-app audio, plays the muxed stream via
/// mpv (starting at `start` secs), tracks position, and on close resumes the
/// in-app audio for the same video at the position the video stopped.
fn yt_watch_video(weak: slint::Weak<MainWindow>, id: String, height: i64, start: f64) {
    // Stop in-app audio (kill the music mpv, invalidate its reader, clear UI).
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    if let Ok(mut g) = music_proc().lock() { if let Some(mut c) = g.take() { let _ = c.kill(); let _ = c.wait(); } }
    let _ = weak.upgrade_in_event_loop(|w| w.set_music_playing(false));
    tokio::runtime::Handle::current().spawn(async move {
        let fmt = if height <= 0 { "best".to_string() } else { format!("best[height<=?{height}]/best") };
        let url = format!("https://www.youtube.com/watch?v={id}");
        let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
        let stream = match tokio::process::Command::new(bin).arg("-g").arg("-f").arg(&fmt).arg("--no-playlist").arg(&url).output().await {
            Ok(o) => String::from_utf8_lossy(&o.stdout).lines().next().map(|l| l.to_string()).filter(|l| !l.is_empty()),
            Err(_) => None,
        };
        let Some(stream) = stream else { return; };
        let sock = mpv_ipc::endpoint("tulipix-yt-video");
        mpv_ipc::cleanup(&sock);
        YT_VID_POS.store(start as i64, std::sync::atomic::Ordering::Relaxed);
        let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
        // Maximized normal window (keeps the WM top bar), not borderless fullscreen.
        cmd.arg("--window-maximized=yes").arg("--force-window=immediate").arg("--title=Tulipix — YouTube")
            .arg(format!("--input-ipc-server={}", sock.display()));
        if start > 1.0 { cmd.arg(format!("--start={}", start as i64)); }
        cmd.arg(&stream);
        mpv_die_with_parent(&mut cmd);
        let Ok(_child) = cmd.spawn() else { return; };
        let weak2 = weak.clone();
        std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let mut conn = None;
            for _ in 0..50 {
                if let Ok(s) = mpv_ipc::connect(&sock) { conn = Some(s); break; }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            if let Some(mut stream) = conn {
                let _ = stream.write_all(b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n");
                let rd = BufReader::new(stream);
                for line in rd.lines().map_while(Result::ok) {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) {
                        if v["event"] == "property-change" && v["name"] == "time-pos" {
                            if let Some(d) = v["data"].as_f64() { YT_VID_POS.store(d as i64, std::sync::atomic::Ordering::Relaxed); }
                        }
                    }
                }
            }
            // mpv exited → resume in-app audio at the last video position.
            let pos = YT_VID_POS.load(std::sync::atomic::Ordering::Relaxed) as f64;
            let id2 = id.clone();
            let _ = weak2.upgrade_in_event_loop(move |w| {
                let weak3 = w.as_weak();
                tokio::runtime::Handle::current().spawn(async move {
                    let dir = yt_media_dir();
                    if let Some(path) = yt_dlp_fetch_audio(&id2, &dir).await {
                        let m = yt_lookup(&id2).unwrap_or_default();
                        let (t, c, th, idd) = (m.title, m.channel, m.thumb, id2.clone());
                        let _ = weak3.upgrade_in_event_loop(move |w| yt_play_inapp(&w, idd, path, t, c, th, pos));
                    }
                });
            });
        });
    });
}

fn yt_play_inapp(w: &MainWindow, id: String, path: String, title: String, sub: String, thumb: String, start: f64) {
    if let Ok(mut g) = yt_cur_audio().lock() { *g = id; }
    w.set_music_yt_now_video(true);
    let my_gen = MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
    let sock = mpv_ipc::endpoint("tulipix-music");
    mpv_ipc::cleanup(&sock);
    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
    cmd.arg("--no-video").arg("--force-window=no").arg("--idle=no")
        .arg(format!("--input-ipc-server={}", sock.display()))
        .arg(format!("--volume={}", w.get_music_volume().clamp(0.0, 130.0) as i32));
    if start > 1.0 { cmd.arg(format!("--start={}", start as i64)); }
    if w.get_music_muted() { cmd.arg("--mute=yes"); }
    stop_video();
    mpv_die_with_parent(&mut cmd);
    let eq_af = music_eq_af(&music_eq().lock().map(|g| *g).unwrap_or([0.0; 10]));
    cmd.arg(format!("--af={}", music_full_af(&eq_af)));
    match cmd.arg(&path).spawn() {
        Ok(child) => { if let Ok(mut g) = music_proc().lock() { *g = Some(child); } }
        Err(e) => { tracing::error!(error = %e, "yt mpv launch failed"); return; }
    }
    if let Ok(mut g) = music_sock().lock() { *g = Some(sock.clone()); }
    // Property reader → transport UI; on EOF just stop (no library auto-advance).
    let weak = w.as_weak();
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        if let Ok(mut stream) = mpv_ipc::connect(&sock) {
            let _ = stream.write_all(concat!(
                "{\"command\":[\"observe_property\",1,\"time-pos\"]}\n",
                "{\"command\":[\"observe_property\",2,\"duration\"]}\n",
                "{\"command\":[\"observe_property\",3,\"pause\"]}\n",
                "{\"command\":[\"observe_property\",4,\"volume\"]}\n",
                "{\"command\":[\"observe_property\",5,\"mute\"]}\n").as_bytes());
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] != "property-change" { continue; }
                let name = v["name"].as_str().unwrap_or("").to_string();
                let wk = weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = wk.upgrade() else { return; };
                    match name.as_str() {
                        "time-pos" => if let Some(d) = v["data"].as_f64() { w.set_music_pos(d as f32); w.set_music_pos_label(fmt_clock(d).into()); }
                        "duration" => if let Some(d) = v["data"].as_f64() { w.set_music_dur(d as f32); w.set_music_dur_label(fmt_clock(d).into()); }
                        "pause"  => if let Some(p) = v["data"].as_bool() { w.set_music_playing(!p); }
                        "volume" => if let Some(d) = v["data"].as_f64() { w.set_music_volume(d as f32); }
                        "mute"   => if let Some(m) = v["data"].as_bool() { w.set_music_muted(m); }
                        _ => {}
                    }
                });
            }
        }
        if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) == my_gen {
            // EOF on this track: advance the play-queue if one is active, else stop.
            let wk = weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if !yt_queue_advance(wk.clone()) {
                    if let Some(w) = wk.upgrade() { w.set_music_playing(false); }
                }
            });
        }
    });
    w.set_music_np_accent(thumb.is_empty().then(|| slint::Color::from_rgb_u8(0xef, 0x44, 0x44))
        .unwrap_or_else(|| dominant_color(std::path::Path::new(&thumb)).unwrap_or(slint::Color::from_rgb_u8(0xef, 0x44, 0x44))));
    w.set_music_np_title(title.into());
    w.set_music_np_sub(if sub.is_empty() { "YouTube".into() } else { sub.into() });
    w.set_music_np_art(yt_img(&thumb));
    w.set_music_player_mode("music".into());
    w.set_music_playing(true);
    w.set_music_pos(0.0); w.set_music_dur(0.0);
    w.set_music_pos_label("0:00".into()); w.set_music_dur_label("0:00".into());
}

async fn pool_for(section: &str) -> Result<sqlx::SqlitePool> {
    let cache = match section {
        "photos" => &PHOTOS_POOL,
        "videos" => &VIDEOS_POOL,
        "music"  => &MUSIC_POOL,
        "books"  => &BOOKS_POOL,
        "cloud"  => &CLOUD_POOL,
        "podcasts" => &PODCASTS_POOL,
        "radio"    => &RADIO_POOL,
        "youtube"  => &YOUTUBE_POOL,
        _ => anyhow::bail!("unknown section"),
    };
    if let Some(p) = cache.get() { return Ok(p.clone()); }
    let handle = tulipix_core::db::DbHandle::open(section)?;
    let pool = handle.init_pool().await?;
    match section {
        "photos" => {
            tulipix_photos::schema::apply(&pool).await?;
            // Phase 6 migrations (safe — ALTER TABLE IF NOT EXISTS equivalent).
            let _ = sqlx::query("ALTER TABLE photo_meta ADD COLUMN color_label TEXT").execute(&pool).await;
            // Apply stacks schema (idempotent CREATE TABLE IF NOT EXISTS).
            let _ = tulipix_photos::stacks::apply_schema(&pool).await;
            // Dedup tables (created by build_clusters; ensure they exist).
            let _ = sqlx::query(
                "CREATE TABLE IF NOT EXISTS dedup_clusters (
                    id      INTEGER PRIMARY KEY AUTOINCREMENT,
                    kind    TEXT NOT NULL,
                    key     TEXT NOT NULL,
                    created INTEGER NOT NULL,
                    UNIQUE(kind, key)
                )"
            ).execute(&pool).await;
            let _ = sqlx::query(
                "CREATE TABLE IF NOT EXISTS dedup_members (
                    cluster_id INTEGER NOT NULL REFERENCES dedup_clusters(id) ON DELETE CASCADE,
                    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                    PRIMARY KEY (cluster_id, item_id)
                )"
            ).execute(&pool).await;
        }
        "videos" => tulipix_videos::schema::apply(&pool).await?,
        "music"  => tulipix_music::schema::apply(&pool).await?,
        "podcasts" => {
            tulipix_music::podcasts::apply_schema(&pool).await?;
            // One-time lift of legacy rows out of the shared music.db.
            migrate_split_from_music(&pool, &[
                ("podcasts",
                 "id, feed_url, title, author, image_url, category, description, last_checked"),
                ("podcast_episodes",
                 "id, podcast_id, guid, title, audio_url, published, duration_s, \
                  description, image_url, downloaded_path, position_s, played"),
            ]).await;
        }
        "radio" => {
            tulipix_music::radio::apply_schema(&pool).await?;
            migrate_split_from_music(&pool, &[
                ("radio_stations",
                 "id, station_uuid, name, url, favicon, country, tags, favourite"),
            ]).await;
        }
        "youtube" => {
            tulipix_music::youtube::store::apply_schema(&pool).await?;
        }
        "books"  => tulipix_books::schema::apply(&pool).await?,
        "cloud"  => tulipix_cloud::schema::apply(&pool).await?,
        _ => {}
    }
    let _ = cache.set(pool.clone());
    Ok(pool)
}

/// One-time migration: move `tables` (each `(name, explicit_column_list)`) out
/// of the legacy shared `music.db` into the freshly-opened section `dest` pool.
///
/// Idempotent: no-ops once the source tables are gone (so it's safe to run on
/// every section open). Copies are parent→child (FK order); drops are the
/// reverse. Everything runs on a single acquired connection because `ATTACH` is
/// connection-scoped and the pool would otherwise hand later statements a
/// different connection that never saw the attach.
async fn migrate_split_from_music(dest: &sqlx::SqlitePool, tables: &[(&str, &str)]) {
    let Some(music_path) = tulipix_core::paths::db_path("music") else { return; };
    if !music_path.exists() { return; }
    let Ok(mut conn) = dest.acquire().await else { return; };
    // Attach the legacy DB read/write so we can also drop the moved tables.
    if sqlx::query(&format!("ATTACH DATABASE '{}' AS legacy", music_path.display()))
        .execute(&mut *conn).await.is_err() { return; }
    // Copy parent→child.
    let mut moved_any = false;
    for (table, cols) in tables {
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM legacy.sqlite_master WHERE type='table' AND name = ?")
            .bind(*table).fetch_optional(&mut *conn).await.ok().flatten();
        if exists.is_none() { continue; }
        moved_any = true;
        let _ = sqlx::query(&format!(
            "INSERT OR IGNORE INTO {table} ({cols}) SELECT {cols} FROM legacy.{table}"))
            .execute(&mut *conn).await;
    }
    // Drop child→parent so foreign keys in the legacy DB don't block the drop.
    if moved_any {
        for (table, _) in tables.iter().rev() {
            let _ = sqlx::query(&format!("DROP TABLE IF EXISTS legacy.{table}")).execute(&mut *conn).await;
        }
        tracing::info!("migrated {} table(s) out of music.db into a split section DB", tables.len());
    }
    let _ = sqlx::query("DETACH DATABASE legacy").execute(&mut *conn).await;
}

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
        w.set_scan_progress(slint::ModelRc::new(slint::VecModel::from(rows)));
        w.set_scan_active(any_active);
    });
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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

pub(crate) fn dirs_default() -> Option<std::path::PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    };
    base.map(|b| b.join("Tulipix"))
}

/// The user's Documents folder (best effort), home as the fallback — used by
/// note export so output lands somewhere visible, not in /tmp.
pub(crate) fn dirs_default_documents() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let docs = home.join("Documents");
    if docs.is_dir() { docs } else { home }
}
