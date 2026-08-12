//! The desktop mini-player widget, the tray popup, and the window controls for
//! the caption row we draw ourselves (`ui/title_row.slint`).
//!
//! Both extra windows are their own top-level Slint windows, so none of
//! `MainWindow`'s state reaches them: Slint globals are per-component-tree.
//! Rather than teach the ~20 places that set now-playing state to also write to
//! two more windows, one timer copies the handful of properties across while a
//! window is actually visible — the same 500 ms cadence the tray already ran on,
//! and the seek clock only moves once a second anyway.

use crate::{MainWindow, MiniWidget, ThemeChoice, TrayPopup};
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use tulipix_music::mini_player::{MiniStyle, PILL_CLUSTER, PILL_CLUSTER_NO_PIN, PILL_LYRICS};
use tulipix_platform::TrayNowPlaying;

thread_local! {
    /// Built on first open, then kept: a Slint window costs its component tree
    /// to build, and the widget is opened and dismissed constantly.
    static MINI: RefCell<Option<MiniWidget>> = const { RefCell::new(None) };
    static POPUP: RefCell<Option<TrayPopup>> = const { RefCell::new(None) };
    /// The size the widget is supposed to be right now, so the settle watchdog
    /// below enforces the CURRENT intent — a corner drag rewrites this, and the
    /// watchdog then holds the dragged size instead of yanking it back.
    static WANT_SIZE: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
    static SETTLE: RefCell<Option<slint::Timer>> = const { RefCell::new(None) };
    /// Art already encoded for the tray menu, keyed by the track it came from,
    /// so a 500 ms tick does not re-encode a PNG it encoded a moment ago.
    static ART_CACHE: RefCell<(String, Vec<u8>)> = const { RefCell::new((String::new(), Vec::new())) };
    /// The same for the OS media applet: the artwork URL last worked out, and
    /// the track+size it belongs to.
    static MEDIA_ART: RefCell<(String, Option<String>)> = const { RefCell::new((String::new(), None)) };
    /// Which of the two artwork file names was written last — see `media_cover`.
    static MEDIA_ART_SLOT: Cell<u8> = const { Cell::new(0) };
    /// The last metadata dict published, as the key that identifies it.
    static MEDIA_META: RefCell<String> = const { RefCell::new(String::new()) };
    /// Last volume published to MPRIS, so an unchanged one is not signalled
    /// twice a second.
    static MEDIA_VOL: Cell<f32> = const { Cell::new(-1.0) };
    /// Has the warm-up run? Separate from `MINI` being `Some`, because a failed
    /// build must not be retried on every tick.
    static WARMED: Cell<bool> = const { Cell::new(false) };
    /// Ticks counted since something was loaded in the player, so the warm-up
    /// lands seconds AFTER playback starts rather than on the same frame.
    /// Stops counting once it has fired.
    static WARM_WAIT: Cell<u8> = const { Cell::new(0) };
}

/// 500 ms ticks between "a track is loaded" and building the widget — 5 seconds.
/// Long enough that the in-app player has finished laying itself out and its
/// artwork has landed. Fires once per session and never again, so nothing about
/// it is tied to any later track change.
const WARM_DELAY_TICKS: u8 = 10;

/// Whether the app draws its own caption row.
///
/// The Mini Player button has to sit with the window buttons, and nothing can be
/// added to a row the OS draws — so the window goes frameless and we draw the
/// row. `TULIPIX_NATIVE_FRAME=1` gives the native decorations back, which is the
/// escape hatch if a compositor makes a mess of a frameless toplevel.
///
/// macOS is excluded: its buttons are drawn by the system at the left of the
/// frame, and putting our button beside them means keeping the native titlebar
/// and hanging an accessory off it (transparent titlebar + fullsize content
/// view), not going frameless. Until that lands, macOS keeps native chrome and
/// reaches the widget through the shortcut and the tray.
pub fn csd_enabled() -> bool {
    if std::env::var_os("TULIPIX_NATIVE_FRAME").is_some() {
        return false;
    }
    !cfg!(target_os = "macos")
}

/// Hand a move-drag to the compositor.
///
/// Wayland has no window-positioning API at all — a frameless window cannot
/// move itself, it can only ask the compositor to take over the drag. Same call
/// works on X11 and Windows, so there is one path rather than three.
fn begin_drag(win: &slint::Window) {
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    {
        use slint::winit_030::WinitWindowAccessor;
        win.with_winit_window(|w| {
            if let Err(e) = w.drag_window() {
                tracing::debug!(error = %e, "window drag refused");
            }
        });
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let _ = win;
}

/// Style currently chosen in Settings › Profile & Style.
fn stored_style(window: &MainWindow) -> MiniStyle {
    MiniStyle::from_name(&window.get_mini_widget_style())
}

/// Copy now-playing state from the main window onto a widget/popup. Both take
/// the same property names, which is why this is one function with two callers.
macro_rules! push_np {
    ($src:expr, $dst:expr) => {{
        let w = $src;
        let d = $dst;
        d.set_art(w.get_music_np_art());
        d.set_accent(w.get_music_np_accent());
        d.set_np_title(w.get_music_np_title());
        d.set_np_sub(w.get_music_np_sub());
        d.set_playing(w.get_music_playing());
        d.set_pos(w.get_music_pos());
        d.set_dur(w.get_music_dur());
        d.set_pos_label(w.get_music_pos_label());
        d.set_dur_label(w.get_music_dur_label());
        d.set_shuffle(w.get_music_shuffle());
        d.set_repeat(w.get_music_repeat());
        d.set_dark(w.get_dark());
        d.set_oled(w.get_oled());
    }};
}

/// Reference px the pill is currently extended by — its button cluster when the
/// chevron has it out, nothing otherwise. Reference px, not on-screen px: the
/// geometry helpers scale it themselves.
fn pill_extra(mini: &MiniWidget, style: MiniStyle) -> f64 {
    if style != MiniStyle::Pill || !mini.get_pill_open() {
        return 0.0;
    }
    if mini.get_no_stacking_control() { PILL_CLUSTER_NO_PIN } else { PILL_CLUSTER }
}

/// The same for the height: the pill's lyrics row, when it is out.
///
/// Gated on `has-lyrics` as well as on the toggle, matching the widget's own
/// `pill-lyrics-on` — a track without lyrics draws no row, so it must not be
/// given the height for one either.
fn pill_extra_h(mini: &MiniWidget, style: MiniStyle) -> f64 {
    let on = style == MiniStyle::Pill && mini.get_pill_lyrics() && mini.get_has_lyrics();
    if on { PILL_LYRICS } else { 0.0 }
}

/// Put the pill back to plain: no button cluster, no lyrics row.
///
/// Called whenever the widget leaves the pill or is opened fresh. The extras are
/// window WIDTH and HEIGHT rather than anything the layout can absorb, so a pill
/// left extended would come back at a size its base geometry does not describe.
fn collapse_pill(mini: &MiniWidget) {
    mini.set_pill_open(false);
    mini.set_pill_lyrics(false);
}

/// The three lyric lines the square style shows behind its artwork.
///
/// Flattened to plain strings rather than handing the widget the row model: it
/// is a separate window with its own component tree, and prev/current/next is
/// the whole of what it draws — the same three the in-app player's lyrics face
/// shows.
fn push_lyrics(window: &MainWindow, mini: &MiniWidget) {
    use slint::Model;
    let rows = window.get_music_lyrics_rows();
    let n = rows.row_count();
    let active = window.get_music_lyrics_active();
    let line = |i: i32| -> slint::SharedString {
        if i < 0 || i as usize >= n {
            return Default::default();
        }
        rows.row_data(i as usize).map(|r| r.text).unwrap_or_default()
    };
    // Only when the rows belong to what is playing. A podcast, a station, a
    // YouTube track or an audiobook chapter carries no lyrics, and the square
    // style went on flipping to the last song's over all of them.
    mini.set_has_lyrics(n > 0 && window.get_music_lyrics_live());
    mini.set_lyric_prev(line(active - 1));
    mini.set_lyric_now(line(active));
    mini.set_lyric_next(line(active + 1));
}

/// Resize the widget to `w`×`h` and keep it there while the window settles.
///
/// One `set_size` is not enough and never was. A window Slint has not put on
/// screen yet has no surface to size — the backend parks the request in the
/// winit attributes for the event loop to build from — and on Wayland the
/// compositor's configure arrives after that and wins. Hiding the widget throws
/// the surface away, so *every* re-open is a first open again: the widget came
/// back at the layout's preferred width each time it was expanded into the app
/// and minimised back out, not only on the very first show.
///
/// So: request it, then check it for the next ~800 ms and re-request whenever
/// what is on screen disagrees. The checks cost nothing once the size holds,
/// and `WANT_SIZE` means a corner drag during that window is respected rather
/// than fought.
fn apply_size(mini: &MiniWidget, w: f64, h: f64) {
    WANT_SIZE.with(|c| c.set((w, h)));
    mini.window().set_size(slint::LogicalSize::new(w as f32, h as f32));

    let mw = mini.as_weak();
    let ticks = Cell::new(0u32);
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(70), move || {
        let stop = || SETTLE.with(|c| { if let Some(t) = c.borrow().as_ref() { t.stop(); } });
        ticks.set(ticks.get() + 1);
        let Some(m) = mw.upgrade() else { return stop() };
        if ticks.get() > 12 || !m.window().is_visible() {
            return stop();
        }
        let (tw, th) = WANT_SIZE.with(|c| c.get());
        let cur = m.window().size().to_logical(m.window().scale_factor());
        if (cur.width as f64 - tw).abs() > 1.0 || (cur.height as f64 - th).abs() > 1.0 {
            m.window().set_size(slint::LogicalSize::new(tw as f32, th as f32));
        }
    });
    SETTLE.with(|c| *c.borrow_mut() = Some(timer));
}

/// Build the widget window once and wire its callbacks back to the main
/// window's own music callbacks — the widget owns no playback logic.
fn build_mini(window: &MainWindow) -> Option<MiniWidget> {
    let mini = match MiniWidget::new() {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!(error = %e, "mini widget: window creation failed");
            return None;
        }
    };
    // Wayland has no "keep above" — xdg-shell carries no stacking request, and
    // winit's `set_window_level` is an empty function on that backend, so the
    // `always-on-top` property the pin toggles is silently dropped. Rather than
    // leave a button that visibly latches and does nothing, the widget hides it
    // there; on X11 (`_NET_WM_STATE_ABOVE`), Windows and macOS it works.
    mini.set_no_stacking_control(cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some());

    let w = window.as_weak();
    mini.on_playpause(move || { if let Some(w) = w.upgrade() { w.invoke_music_toggle_pause(); } });
    let w = window.as_weak();
    mini.on_next(move || { if let Some(w) = w.upgrade() { w.invoke_music_next(); } });
    let w = window.as_weak();
    mini.on_prev(move || { if let Some(w) = w.upgrade() { w.invoke_music_prev(); } });
    let w = window.as_weak();
    mini.on_seek(move |s| { if let Some(w) = w.upgrade() { w.invoke_music_seek(s); } });
    let w = window.as_weak();
    mini.on_toggle_shuffle(move || { if let Some(w) = w.upgrade() { w.invoke_music_toggle_shuffle(); } });
    let w = window.as_weak();
    mini.on_cycle_repeat(move || { if let Some(w) = w.upgrade() { w.invoke_music_cycle_repeat(); } });
    let w = window.as_weak();
    mini.on_set_volume(move |v| { if let Some(w) = w.upgrade() { w.invoke_music_set_volume(v); } });
    let w = window.as_weak();
    mini.on_toggle_mute(move || { if let Some(w) = w.upgrade() { w.invoke_music_toggle_mute(); } });
    // Zen is a full-screen view of the main window, so it has to bring the app
    // back with it; `restore` does the showing, this only picks the destination.
    let w = window.as_weak();
    mini.on_open_zen(move || {
        if let Some(w) = w.upgrade() {
            w.set_music_fullscreen(true);
            restore(&w);
        }
    });

    // Pin is the widget's own state — nothing else in the app cares whether it
    // floats — so it is toggled here rather than round-tripped through settings.
    let mw = mini.as_weak();
    mini.on_toggle_pin(move || {
        if let Some(m) = mw.upgrade() {
            m.set_pinned(!m.get_pinned());
        }
    });
    // Light ⇄ dark for the whole app, not just the widget: the widget's own
    // `dark` is pushed from the main window every tick, so flipping it here
    // alone would be undone within 500 ms.
    let w = window.as_weak();
    mini.on_toggle_theme(move || {
        let Some(w) = w.upgrade() else { return };
        let next = if w.get_dark() { ThemeChoice::Light } else { ThemeChoice::ExtraDark };
        w.set_theme_choice(next);
        w.invoke_theme_changed(next);
    });

    // Next layout, in the order the picker lists them. Persisted, so the choice
    // survives a close and the Settings picker agrees with the widget.
    let w = window.as_weak();
    let mw = mini.as_weak();
    mini.on_cycle_style(move || {
        let (Some(w), Some(m)) = (w.upgrade(), mw.upgrade()) else { return };
        let next = match stored_style(&w) {
            MiniStyle::Bar => MiniStyle::Square,
            MiniStyle::Square => MiniStyle::Pill,
            MiniStyle::Pill => MiniStyle::Bar,
        };
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.advanced.insert("ui.mini-widget.style".into(), next.name().into());
        if let Err(e) = s.save() {
            tracing::warn!(error = %e, "save settings (mini widget style)");
        }
        w.set_mini_widget_style(next.name().into());
        m.set_style(next.name().into());
        // Leaving the pill retires whatever it had out, so coming back to it
        // lands on the collapsed pill the base size describes rather than on
        // the shape it was in when it was cycled away from.
        collapse_pill(&m);
        let (nw, nh) = next.window_size();
        apply_size(&m, nw, nh);
    });

    // The chevron widens the WINDOW rather than opening the cluster inward: at
    // pill width, four buttons taken out of the title leave about five
    // characters of it. Height is untouched, and the widget scales itself off
    // its height, so nothing in it changes size as the window grows.
    //
    // Scale comes off the HEIGHT here — the axis this toggle does not touch —
    // so the two extras never have to be unwound from the number they are
    // being re-added to.
    let w = window.as_weak();
    let mw = mini.as_weak();
    mini.on_pill_extend(move || {
        let (Some(w), Some(m)) = (w.upgrade(), mw.upgrade()) else { return };
        let style = stored_style(&w);
        if style != MiniStyle::Pill { return }
        let (bw, bh) = style.base_size();
        let extra_h = pill_extra_h(&m, style);
        let sf = m.window().scale_factor();
        let scale = m.window().size().to_logical(sf).height as f64 / (bh + extra_h);
        apply_size(&m, (bw + pill_extra(&m, style)) * scale, (bh + extra_h) * scale);
    });

    // The mic on the pill's artwork, the other way round: the row is height, so
    // the scale is read off the WIDTH, which this toggle leaves alone.
    let w = window.as_weak();
    let mw = mini.as_weak();
    mini.on_pill_lyrics_toggled(move || {
        let (Some(w), Some(m)) = (w.upgrade(), mw.upgrade()) else { return };
        let style = stored_style(&w);
        if style != MiniStyle::Pill { return }
        let (bw, bh) = style.base_size();
        let extra_w = pill_extra(&m, style);
        let sf = m.window().scale_factor();
        let scale = m.window().size().to_logical(sf).width as f64 / (bw + extra_w);
        apply_size(&m, (bw + extra_w) * scale, (bh + pill_extra_h(&m, style)) * scale);
    });

    let mw = mini.as_weak();
    mini.on_begin_drag(move || {
        if let Some(m) = mw.upgrade() { begin_drag(m.window()); }
    });
    // Corner grip. We drive the resize ourselves rather than handing it to the
    // compositor, because only we can hold the style's aspect ratio.
    //
    // The delta is applied to the window's CURRENT size, not to its size when
    // the drag began. The grip is anchored to the bottom-right corner, so it
    // moves as the window grows and the cursor's offset within it shrinks by
    // exactly what was applied — the two cancel, and the reported delta is
    // always the residual against the size on screen right now. Using a
    // press-time baseline instead double-counts and the widget oscillates.
    let mw = mini.as_weak();
    let w = window.as_weak();
    mini.on_resize_drag(move |dx, dy| {
        let (Some(m), Some(main)) = (mw.upgrade(), w.upgrade()) else { return };
        let sf = m.window().scale_factor();
        let cur = m.window().size().to_logical(sf).width as f64;
        let style = stored_style(&main);
        let (nw, nh) = style.resize_locked_pill(
            cur, dx as f64, dy as f64, pill_extra(&m, style), pill_extra_h(&m, style));
        WANT_SIZE.with(|c| c.set((nw, nh)));
        m.window().set_size(slint::LogicalSize::new(nw as f32, nh as f32));
    });
    // Expand — widget away, app back.
    let w = window.as_weak();
    mini.on_expand(move || {
        if let Some(w) = w.upgrade() { restore(&w); }
    });
    // Close is to the tray, not a quit: the tray menu is the other half of this
    // feature and stays reachable.
    mini.on_close_widget(close_mini);
    Some(mini)
}

/// Build the widget's window ahead of time, without showing it.
///
/// `MiniWidget` is a second top-level Slint component and its tree is not small.
/// Building it on the click that opens it puts that whole cost on the event loop
/// at the one moment the user is watching. Doing it once, five seconds into the
/// first track, means the click that follows only has to push properties and
/// show a window that already exists.
///
/// Two things keep it out of the way. It runs from the 500 ms tick rather than
/// the play path, so it never lands on the frame that starts a song — that is
/// the busiest frame the event loop has, and sharing it is what made the
/// in-app player stall. And `mini-widget-ready` is only raised afterwards, so
/// the caption row's Mini Player button appears when the window behind it is
/// real rather than sitting there as a button that would stall on its press.
fn warm_mini(window: &MainWindow) {
    if WARMED.with(|c| c.get()) {
        return;
    }
    WARMED.with(|c| c.set(true));
    MINI.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = build_mini(window);
        }
        tracing::info!(built = slot.is_some(), "mini widget: background warm-up");
    });
    // Raised even when the build failed: `open_mini` retries on the click, and
    // a button that never appears is worse than one that has to do the work
    // itself on a machine where the second window cannot be created at all.
    window.set_mini_widget_ready(true);
}

/// Show the widget and hide the main window — the app "becomes" the widget.
pub fn open_mini(window: &MainWindow) {
    let style = stored_style(window);
    MINI.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = build_mini(window);
        }
        let Some(mini) = slot.as_ref() else { return };
        mini.set_style(style.name().into());
        push_np!(window, mini);
        push_lyrics(window, mini);
        mini.set_volume(window.get_music_volume());
        mini.set_muted(window.get_music_muted());
        // Opening always restores the class's base size. The widget scales its
        // contents off its own width, so a size dragged for one style would be
        // the wrong shape for the next. Requested before `show` — that is what
        // marks the size as explicit and stops `set_visible` from using the
        // layout's preferred size — and held there by the watchdog after.
        // A pill that was left extended reopens collapsed — the width it was
        // last shown at is not the width its base size describes.
        collapse_pill(mini);
        let (w, h) = style.window_size();
        apply_size(mini, w, h);
        if let Err(e) = mini.show() {
            tracing::warn!(error = %e, "mini widget: show failed");
            return;
        }
        apply_size(mini, w, h);
        window.window().hide().ok();
    });
    // The app is now only a music widget — put down everything the widget does
    // not draw. `restore` builds it back. See `release_for_widget`.
    crate::release_for_widget(window);
}

/// Bring the app back and dismiss the desktop widget.
///
/// Every door into the main window goes through here — the widget's restore
/// button, its double-click, the zen button, the tray's Open item, the popup —
/// so "the app is open" and "a widget is floating" can never both be true. The
/// tick in `sync` catches the paths we do not own, like a taskbar click.
pub fn restore(window: &MainWindow) {
    close_mini();
    close_popup();
    let _ = window.show();
    window.window().set_minimized(false);
    // Refill whatever `release_for_widget` put down on the way out.
    crate::restore_from_widget(window);
}

pub fn close_mini() {
    MINI.with(|cell| {
        if let Some(m) = cell.borrow().as_ref() {
            let _ = m.hide();
        }
    });
}

fn mini_visible() -> bool {
    MINI.with(|cell| cell.borrow().as_ref().is_some_and(|m| m.window().is_visible()))
}

fn build_popup(window: &MainWindow) -> Option<TrayPopup> {
    let popup = match TrayPopup::new() {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "tray popup: window creation failed");
            return None;
        }
    };
    let w = window.as_weak();
    popup.on_playpause(move || { if let Some(w) = w.upgrade() { w.invoke_music_toggle_pause(); } });
    let w = window.as_weak();
    popup.on_next(move || { if let Some(w) = w.upgrade() { w.invoke_music_next(); } });
    let w = window.as_weak();
    popup.on_prev(move || { if let Some(w) = w.upgrade() { w.invoke_music_prev(); } });
    let w = window.as_weak();
    popup.on_seek(move |s| { if let Some(w) = w.upgrade() { w.invoke_music_seek(s); } });
    let w = window.as_weak();
    popup.on_set_volume(move |v| { if let Some(w) = w.upgrade() { w.invoke_music_set_volume(v); } });
    let w = window.as_weak();
    popup.on_toggle_mute(move || { if let Some(w) = w.upgrade() { w.invoke_music_toggle_mute(); } });
    let w = window.as_weak();
    popup.on_toggle_shuffle(move || { if let Some(w) = w.upgrade() { w.invoke_music_toggle_shuffle(); } });
    let w = window.as_weak();
    popup.on_cycle_repeat(move || { if let Some(w) = w.upgrade() { w.invoke_music_cycle_repeat(); } });
    let w = window.as_weak();
    popup.on_open_app(move || {
        if let Some(w) = w.upgrade() { restore(&w); }
    });
    let w = window.as_weak();
    popup.on_open_mini(move || {
        close_popup();
        if let Some(w) = w.upgrade() { open_mini(&w); }
    });
    popup.on_quit_app(|| { let _ = slint::quit_event_loop(); });
    popup.on_dismiss(close_popup);
    Some(popup)
}

pub fn close_popup() {
    POPUP.with(|cell| {
        if let Some(p) = cell.borrow().as_ref() {
            let _ = p.hide();
        }
    });
}

/// Tray left-click with the popup style: show it, or dismiss it if it is
/// already up — a second click on a tray icon closes its menu.
pub fn toggle_popup(window: &MainWindow) {
    POPUP.with(|cell| {
        let mut slot = cell.borrow_mut();
        if slot.is_none() {
            *slot = build_popup(window);
        }
        let Some(popup) = slot.as_ref() else { return };
        if popup.window().is_visible() {
            let _ = popup.hide();
            return;
        }
        push_np!(window, popup);
        popup.set_volume(window.get_music_volume());
        popup.set_muted(window.get_music_muted());
        // Deliberately not positioned: Wayland gives a toplevel no say in where
        // it lands, and there is no portable way to ask where the tray icon is.
        // X11/Windows put it wherever the WM would put a new window.
        if let Err(e) = popup.show() {
            tracing::warn!(error = %e, "tray popup: show failed");
        }
    });
}

/// Album art as a small PNG for the tray menu row, cached per track.
///
/// A miss starts the encode on a worker and returns nothing; the icon appears on
/// the following tick. It used to encode inline, which put a resize of a
/// full-size cover on the event loop at every track change.
fn tray_art_png(window: &MainWindow) -> Vec<u8> {
    let key = format!("{}|{}", window.get_music_np_title(), window.get_music_np_sub());
    // Borrow, decide, drop — so nothing here depends on where a temporary
    // happens to be released.
    let hit = ART_CACHE.with(|c| {
        let g = c.borrow();
        (g.0 == key).then(|| g.1.clone())
    });
    if let Some(png) = hit {
        return png;
    }
    // Claim the key first: the miss must not re-spawn the same encode on every
    // tick while the worker is running.
    ART_CACHE.with(|c| *c.borrow_mut() = (key.clone(), Vec::new()));
    spawn_tray_encode(&window.get_music_np_art(), key);
    Vec::new()
}

/// Write the artwork out for the OS media applet, off the event loop.
///
/// MPRIS and SMTC both take a URL and neither takes pixels, so a cover that is
/// not already a file on disk has to be encoded. That is a resize plus a PNG
/// encode of a ~1000px image, and doing it inline on a track change was long
/// enough to be felt as the app stalling every time the song changed.
///
/// `SharedPixelBuffer` is `Send` (`slint::Image` is not), so the pixels are
/// lifted here and everything expensive happens on the worker. When it lands,
/// the path is filed against the key it was started for and the published
/// metadata is invalidated, so the next tick re-sends the dict with the cover in
/// it. If the track has moved on by then the key no longer matches and the
/// result is dropped.
///
/// Two alternating file names rather than one: clients cache artwork by URL, and
/// a single path rewritten in place leaves the previous track's cover on screen.
fn spawn_cover_encode(art: &slint::Image, key: String) {
    let sz = art.size();
    if sz.width == 0 || sz.height == 0 {
        return;
    }
    let (Some(px), Some(dir)) = (art.to_rgba8(), tulipix_core::paths::cache_dir()) else {
        return;
    };
    let slot = MEDIA_ART_SLOT.with(|c| {
        c.set(c.get() ^ 1);
        c.get()
    });
    std::thread::spawn(move || {
        let Some(rgba) =
            image::RgbaImage::from_raw(px.width(), px.height(), px.as_bytes().to_vec())
        else {
            return;
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return;
        }
        let path = dir.join(format!("nowplaying-{slot}.png"));
        // Lockscreen size. The source is often a 1000px cover and none of that
        // reaches the applet.
        if image::DynamicImage::ImageRgba8(rgba).thumbnail(512, 512).save(&path).is_err() {
            return;
        }
        let url = path.to_string_lossy().into_owned();
        let _ = slint::invoke_from_event_loop(move || {
            let landed = MEDIA_ART.with(|c| {
                let mut g = c.borrow_mut();
                let mine = g.0 == key;
                if mine {
                    g.1 = Some(url);
                }
                mine
            });
            if landed {
                MEDIA_META.with(|c| c.borrow_mut().clear());
            }
        });
    });
}

/// The tray menu's copy of the same job, at menu-icon size.
fn spawn_tray_encode(art: &slint::Image, key: String) {
    let Some(px) = art.to_rgba8() else { return };
    std::thread::spawn(move || {
        let Some(rgba) =
            image::RgbaImage::from_raw(px.width(), px.height(), px.as_bytes().to_vec())
        else {
            return;
        };
        // Menu rows draw the icon at ~22px; anything larger is bytes over D-Bus
        // for pixels nobody sees.
        let small = image::DynamicImage::ImageRgba8(rgba).thumbnail(32, 32);
        let mut out = std::io::Cursor::new(Vec::new());
        if small.write_to(&mut out, image::ImageFormat::Png).is_err() {
            return;
        }
        let png = out.into_inner();
        let _ = slint::invoke_from_event_loop(move || {
            ART_CACHE.with(|c| {
                let mut g = c.borrow_mut();
                if g.0 == key {
                    g.1 = png;
                }
            });
        });
    });
}

/// Publish now-playing to the OS media surface — the desktop's media applet, the
/// lockscreen, and anything mirroring the session (a phone over KDE Connect).
///
/// The app registered as a player at startup and so has always had the media
/// keys, but nothing ever called `set_metadata`: the applet knew a player called
/// Tulipix existed and nothing else about it, which is why it drew a generic
/// note where every other player shows a cover, a title and a scrubber.
///
/// Driven from the same 500 ms tick as the tray rather than from the ~8 places
/// that set now-playing state, so podcasts, radio, YouTube and audiobooks are
/// covered by the same code as library tracks.
fn media_sync(window: &MainWindow) {
    let title = window.get_music_np_title();
    if title.is_empty() {
        return;
    }
    let sub = window.get_music_np_sub();
    let art = window.get_music_np_art();
    let sz = art.size();
    // Size is in the key because artwork lands AFTER the title on most paths —
    // without it the first dict would go out coverless and never be revised.
    let art_key = format!("{title}|{sub}|{}x{}", sz.width, sz.height);
    let hit = MEDIA_ART.with(|c| {
        let g = c.borrow();
        (g.0 == art_key).then(|| g.1.clone())
    });
    let cover = match hit {
        Some(p) => p,
        None => {
            // Most covers are already files and the `Image` remembers the path
            // it came from — that costs nothing. The rest are handed to a
            // worker, and the key is claimed here so the next tick sees a miss
            // for the same track as "already started" rather than starting it
            // again twice a second.
            let direct = art.path().map(|p| p.to_string_lossy().into_owned());
            MEDIA_ART.with(|c| *c.borrow_mut() = (art_key.clone(), direct.clone()));
            if direct.is_none() {
                spawn_cover_encode(&art, art_key.clone());
            }
            direct
        }
    };
    // Duration joins the key for the same reason: mpv reports the length a beat
    // after the track starts, and a dict published without it gives every remote
    // a scrubber with no end.
    let dur = window.get_music_dur();
    let meta_key = format!("{art_key}|{dur:.0}");
    let fresh = MEDIA_META.with(|c| {
        let mut g = c.borrow_mut();
        (*g != meta_key).then(|| *g = meta_key.clone()).is_some()
    });
    if fresh {
        tulipix_common::media_set_track(
            &title,
            &sub,
            &window.get_music_np_album(),
            cover.as_deref(),
            dur,
        );
    }
    tulipix_common::media_set_progress(window.get_music_playing(), window.get_music_pos());
    // 0..130 in the app (mpv allows the boost); 0..1 on the wire.
    let vol = window.get_music_volume();
    if (MEDIA_VOL.with(|c| c.get()) - vol).abs() > 0.5 {
        MEDIA_VOL.with(|c| c.set(vol));
        tulipix_common::media_set_volume(vol as f64 / 100.0);
    }
}

/// Everything the tray menu says about playback, read off the main window.
pub fn tray_state(window: &MainWindow) -> TrayNowPlaying {
    let title = window.get_music_np_title().to_string();
    let has_track = !title.is_empty();
    TrayNowPlaying {
        artist: window.get_music_np_sub().to_string(),
        playing: window.get_music_playing(),
        shuffle: window.get_music_shuffle(),
        repeat: window.get_music_repeat().to_string(),
        art_png: if has_track { tray_art_png(window) } else { Vec::new() },
        popup: window.get_tray_menu_style() != "native",
        title,
        has_track,
    }
}

/// Push state into whichever of the two windows is currently on screen, plus
/// the tray menu. Called from the existing 500 ms tray tick.
pub fn sync(window: &MainWindow) {
    // Something is loaded in the player: build the widget in the background so
    // the button that opens it is instant rather than a stall. Once per session,
    // and ten ticks late on purpose — see `warm_mini`.
    if !WARMED.with(|c| c.get()) && !window.get_music_np_title().is_empty() {
        let n = WARM_WAIT.with(|c| {
            c.set(c.get() + 1);
            c.get()
        });
        if n >= WARM_DELAY_TICKS {
            warm_mini(window);
        }
    }
    // The app being on screen and a widget floating over the desktop are
    // mutually exclusive. `restore` covers the doors we own; this covers the
    // ones we do not — a taskbar click, a dock click, an activation by the WM.
    if mini_visible() && window.window().is_visible() {
        close_mini();
        return;
    }
    if mini_visible() {
        let style = stored_style(window);
        MINI.with(|cell| {
            if let Some(m) = cell.borrow().as_ref() {
                push_np!(window, m);
                push_lyrics(window, m);
                m.set_volume(window.get_music_volume());
                m.set_muted(window.get_music_muted());
                // Picking a different style in Settings resizes the widget that
                // is already on screen, rather than waiting for the next open.
                if m.get_style() != style.name() {
                    m.set_style(style.name().into());
                    collapse_pill(m);
                    let (w, h) = style.window_size();
                    apply_size(m, w, h);
                } else if style == MiniStyle::Pill
                    && m.get_pill_lyrics()
                    && !m.get_has_lyrics()
                {
                    // The row is gated on `has-lyrics` in the widget, so a track
                    // change to something without them takes it off screen. Give
                    // the window its height back to match, or the pill is left
                    // with a band of empty panel under it.
                    m.set_pill_lyrics(false);
                    let (bw, bh) = style.base_size();
                    let extra_w = pill_extra(m, style);
                    let sf = m.window().scale_factor();
                    let scale = m.window().size().to_logical(sf).width as f64 / (bw + extra_w);
                    apply_size(m, (bw + extra_w) * scale, bh * scale);
                }
            }
        });
    }
    POPUP.with(|cell| {
        if let Some(p) = cell.borrow().as_ref() {
            if p.window().is_visible() {
                push_np!(window, p);
                p.set_volume(window.get_music_volume());
                p.set_muted(window.get_music_muted());
            }
        }
    });
    tulipix_platform::update_tray(tray_state(window));
    media_sync(window);
}

/// Caption-row window controls, the Mini Player button, and the startup seed of
/// both style settings.
pub fn wire(window: &MainWindow) {
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    window.set_mini_widget_style(MiniStyle::from_name(&s.text("ui.mini-widget.style")).name().into());
    window.set_tray_menu_style(
        if s.text("ui.tray-menu.style") == "native" { "native" } else { "popup" }.into(),
    );

    let csd = csd_enabled();
    window.set_csd(csd);
    window.set_csd_mac(false);
    if csd {
        tracing::info!("window chrome: custom caption row (TULIPIX_NATIVE_FRAME=1 to disable)");
    }

    // No minimise / maximise / resize handlers here: Slint 1.17 exposes
    // `minimized` and `maximized` as two-way Window properties and grows resize
    // borders for frameless windows via `resize-border-width`, so the caption
    // row drives all three itself and stays correct when a tiling WM changes
    // the state behind our back.
    //
    // Close goes where the OS button went: quit. The tray, when it is running,
    // is reached by minimising or by the widget's own ✕.
    let w = window.as_weak();
    window.on_win_close(move || {
        if let Some(w) = w.upgrade() { w.invoke_app_quit(); }
    });
    let w = window.as_weak();
    window.on_win_drag(move || {
        if let Some(w) = w.upgrade() { begin_drag(w.window()); }
    });
    let w = window.as_weak();
    window.on_open_mini_widget(move || {
        if let Some(w) = w.upgrade() { open_mini(&w); }
    });
}
