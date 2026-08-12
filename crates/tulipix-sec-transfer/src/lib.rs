//! Transfer-section wiring: the bridge between [`tulipix_transfer`] — which
//! knows nothing about Slint — and every `window.on_transfer_*` callback.
//!
//! The server's lifetime is the section's: [`section_changed`] binds the port
//! on the way in and drops it on the way out, so nothing is listening on the
//! café wifi joined yesterday. tulipix-app calls [`wire`] once at startup.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tulipix_sec_photos::replace_rows;
use tulipix_transfer::{human_size, TransferService};
use tulipix_ui::*;

mod qr;

/// Settings key for the inbox folder. Lives in `Settings.advanced`, which is a
/// free-text map, so adding it needs no schema change.
const INBOX_KEY: &str = "transfer.inbox";

/// How often the page re-reads the service while the section is open. Fast
/// enough that an upload's progress bar moves smoothly, slow enough to be free.
const TICK_MS: u64 = 600;

/// The same, while sharing is on but the user is somewhere else in the app.
/// Nothing is on screen to repaint, but an upload can still land, and the
/// post-upload library rescan hangs off this tick.
const IDLE_TICK_MS: u64 = 3_000;

/// The service, parked in an `Option` so an async `start`/`stop` can own it
/// outright rather than holding a lock across an await. Callbacks that arrive
/// during those few milliseconds see `None` and no-op, which is the correct
/// answer for all of them.
fn service() -> &'static Mutex<Option<TransferService>> {
    static SERVICE: OnceLock<Mutex<Option<TransferService>>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(None))
}

fn guard() -> std::sync::MutexGuard<'static, Option<TransferService>> {
    service().lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` against the service if it is parked and started.
fn with<R>(f: impl FnOnce(&TransferService) -> R) -> Option<R> {
    guard().as_ref().map(f)
}

thread_local! {
    /// Repaints the page while the section is open. A `slint::Timer` is bound to
    /// the thread that started it, which is always the UI thread here.
    static TICK: slint::Timer = slint::Timer::default();
}

/// The ledger revision the Recent Transfers list was last built from, so a tick
/// that changed nothing does not hit the database.
static SEEN_REV: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);

/// Completed uploads the last tick knew about. A rise means something landed in
/// the inbox, which is when the library is asked to look again.
static SEEN_DONE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The URL the QR currently on screen was drawn for. Empty when there is none.
fn qr_drawn_for() -> std::sync::MutexGuard<'static, String> {
    static AT: OnceLock<Mutex<String>> = OnceLock::new();
    AT.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

// ── entry points ────────────────────────────────────────────────────────────

pub fn wire(window: &MainWindow) {
    window.set_transfer_inbox(inbox_path().display().to_string().into());

    let w = window.as_weak();
    window.on_transfer_start(move || {
        let w = w.clone();
        spawn(async move {
            start_service().await;
            let _ = w.upgrade_in_event_loop(|w| refresh(&w));
        });
    });

    let w = window.as_weak();
    window.on_transfer_stop(move || {
        let w = w.clone();
        spawn(async move {
            stop_service().await;
            let _ = w.upgrade_in_event_loop(|w| refresh(&w));
        });
    });

    let w = window.as_weak();
    window.on_transfer_add_files(move || {
        let Some(w) = w.upgrade() else { return };
        let Some(paths) = rfd::FileDialog::new().set_title("Share files").pick_files() else {
            return;
        };
        let _ = with(|svc| {
            for p in &paths {
                svc.add(p);
            }
        });
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_transfer_add_folder(move || {
        let Some(w) = w.upgrade() else { return };
        let Some(dir) = rfd::FileDialog::new().set_title("Share a folder").pick_folder() else {
            return;
        };
        let _ = with(|svc| svc.add(&dir));
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_transfer_remove(move |id| {
        let Some(w) = w.upgrade() else { return };
        let _ = with(|svc| svc.remove(id.max(0) as u64));
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_transfer_clear(move || {
        let Some(w) = w.upgrade() else { return };
        let _ = with(|svc| svc.clear());
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_transfer_set_share_target(move |token| {
        let Some(w) = w.upgrade() else { return };
        // Empty means everyone; the tray normalises it, so the UI can send the
        // chip's own value without a special case for the first chip.
        let _ = with(|svc| svc.set_share_target(Some(token.to_string())));
        refresh(&w);
    });

    // Open the phone page in this machine's own browser, already paired.
    //
    // It goes through the same single-use pairing key the QR carries rather
    // than any desktop-only bypass: one way in, one thing to reason about. The
    // desktop then shows up in the Connection card as a paired device, which is
    // what it is — and scanning or clicking again re-uses that pairing instead
    // of stacking up a second one.
    let w = window.as_weak();
    window.on_transfer_open_url(move || {
        let Some(w) = w.upgrade() else { return };
        let url = with(|svc| svc.pairing_url()).unwrap_or_default();
        if url.is_empty() {
            return;
        }
        open_url(&url);
        // The device shows up once the browser actually loads the URL, which is
        // after this returns — the periodic tick is what catches it. This
        // refresh is only so the card is current the instant the click lands.
        refresh(&w);
    });

    let w = window.as_weak();
    window.on_transfer_forget(move |token| {
        let w = w.clone();
        let token = token.to_string();
        spawn(async move {
            // The service is only borrowed for the duration of the await inside
            // `forget_device`, which takes `&self` and never re-enters here.
            let taken = guard().take();
            if let Some(svc) = taken {
                svc.forget_device(&token).await;
                *guard() = Some(svc);
            }
            let _ = w.upgrade_in_event_loop(|w| refresh(&w));
        });
    });

    let w = window.as_weak();
    window.on_transfer_rename_device(move |token, name| {
        let w = w.clone();
        let token = token.to_string();
        let name = name.to_string();
        spawn(async move {
            // Same borrow dance as `forget`: the service is taken out for the
            // duration of the await and put straight back.
            let taken = guard().take();
            if let Some(svc) = taken {
                svc.rename_device(&token, &name).await;
                *guard() = Some(svc);
            }
            let _ = w.upgrade_in_event_loop(|w| refresh(&w));
        });
    });

    let w = window.as_weak();
    window.on_transfer_set_iface(move |ip| {
        let w = w.clone();
        let ip = ip.to_string();
        // The listener is bound to one address, so picking a different one is a
        // rebind, not a relabel — otherwise the URL would name an interface the
        // socket is not on.
        let restart = {
            let mut held = guard();
            match held.as_mut() {
                Some(svc) => {
                    svc.set_iface(ip);
                    svc.is_running()
                }
                None => false,
            }
        };
        spawn(async move {
            if restart {
                stop_service().await;
                start_service().await;
            }
            let _ = w.upgrade_in_event_loop(|w| refresh(&w));
        });
    });

    let w = window.as_weak();
    window.on_transfer_pick_inbox(move || {
        let Some(w) = w.upgrade() else { return };
        let Some(dir) = rfd::FileDialog::new().set_title("Choose the inbox folder").pick_folder()
        else {
            return;
        };
        let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
        s.advanced.insert(INBOX_KEY.to_string(), dir.display().to_string());
        if let Err(e) = s.save() {
            tracing::warn!(error = %e, "transfer: could not persist the inbox path");
        }
        if let Some(svc) = guard().as_mut() {
            svc.set_inbox(dir.clone());
        }
        w.set_transfer_inbox(dir.display().to_string().into());
        refresh(&w);
    });

    window.on_transfer_open_inbox(move || {
        let path = inbox_path();
        let _ = std::fs::create_dir_all(&path);
        if let Err(e) = tulipix_platform::fm::open_default(&path) {
            tracing::warn!(error = %e, "transfer: could not open the inbox");
        }
    });

    let w = window.as_weak();
    window.on_transfer_open_row(move |idx| {
        let Some(w) = w.upgrade() else { return };
        let Some(row) = w.get_transfer_rows().row_data(idx.max(0) as usize) else { return };
        if row.path.is_empty() {
            return;
        }
        let path = PathBuf::from(row.path.to_string());
        if !path.exists() {
            return;
        }
        if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&path) {
            tracing::warn!(error = %e, "transfer: could not reveal the file");
        }
    });

    // Retry a send that failed. The desktop cannot push, so "send it again"
    // means putting the file back in the tray for the phone to pull — and
    // dropping the row it failed on, so the table does not end up with a stale
    // failure sitting next to the attempt that replaced it.
    let w = window.as_weak();
    window.on_transfer_retry_row(move |idx| {
        let Some(w) = w.upgrade() else { return };
        let Some(row) = w.get_transfer_rows().row_data(idx.max(0) as usize) else { return };
        if row.direction != "out" || row.path.is_empty() {
            return;
        }
        let path = PathBuf::from(row.path.to_string());
        if !path.exists() {
            // The file moved since it failed. Nothing to re-offer, and the row
            // is already greyed to say so.
            return;
        }
        // Sharing may have been stopped since — a retry is what happens after
        // the connection comes back — and a tray that is not serving swallows
        // the file silently, so nothing is dropped from the ledger until it has
        // actually been re-offered.
        let offered = with(|svc| {
            if !svc.is_running() {
                return false;
            }
            svc.add(&path);
            true
        })
        .unwrap_or(false);
        if !offered {
            return;
        }
        let id = row.id as i64;
        let weak = w.as_weak();
        spawn(async move {
            if let Some(pool) = with(|svc| svc.pool()).flatten() {
                if let Err(e) = tulipix_transfer::ledger::forget(&pool, id).await {
                    tracing::warn!(error = %e, "transfer: could not drop the failed row");
                }
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                refresh(&w);
                load_history(&w, w.get_transfer_page().max(0) as usize);
            });
        });
    });

    let w = window.as_weak();
    window.on_transfer_clear_history(move || {
        let w = w.clone();
        spawn(async move {
            let pool = with(|svc| svc.pool()).flatten();
            if let Some(pool) = pool {
                if let Err(e) = tulipix_transfer::ledger::clear_transfers(&pool).await {
                    tracing::warn!(error = %e, "transfer: could not clear the ledger");
                }
            }
            let _ = w.upgrade_in_event_loop(|w| {
                w.set_transfer_page(0);
                load_history(&w, 0);
            });
        });
    });

    let w = window.as_weak();
    window.on_transfer_set_page(move |page| {
        let Some(w) = w.upgrade() else { return };
        let page = page.max(0);
        w.set_transfer_page(page);
        load_history(&w, page as usize);
    });

    let w = window.as_weak();
    window.on_transfer_set_sort(move |col, desc| {
        let Some(w) = w.upgrade() else { return };
        w.set_transfer_sort(col);
        w.set_transfer_sort_desc(desc);
        // Back to page one: staying on page four of a differently-ordered table
        // shows rows that have nothing to do with what was just clicked.
        w.set_transfer_page(0);
        load_history(&w, 0);
    });

    window.on_transfer_dismiss_upload(move |id| {
        let _ = with(|svc| svc.dismiss_upload(id.max(0) as u64));
    });
}

/// Called from tulipix-app's `on_section_changed`.
///
/// Entering the section binds the port. Leaving it does *not* drop it: sharing
/// lasts as long as the app does, so a phone can keep pulling a 4 GB file while
/// the desktop is used for something else. Stop is an explicit button, or app
/// exit — see [`shutdown`].
pub fn section_changed(window: &MainWindow, section: &str) {
    if section == "transfer" {
        enter(window);
    } else {
        leave(window);
    }
}

/// What the Status dashboard reads about this service.
///
/// Returns the "not sharing" default when the service is parked mid-start/stop,
/// which is what it is at that moment.
pub fn status_snapshot(now: u64) -> tulipix_transfer::StatusSnapshot {
    with(|s| s.status_snapshot(now)).unwrap_or_default()
}

/// Stop serving on the way out of the app. Called from the shutdown path so a
/// closed window does not leave a listener behind.
///
/// Synchronous, unlike [`leave`]: by this point the event loop has returned and
/// a task spawned onto the runtime may never get to run before it is torn down.
pub fn shutdown() {
    TICK.with(|t| t.stop());
    if let Some(svc) = guard().as_mut() {
        svc.abort();
    }
}

fn enter(window: &MainWindow) {
    let w = window.as_weak();
    spawn(async move {
        start_service().await;
        let _ = w.upgrade_in_event_loop(|w| {
            refresh(&w);
            w.set_transfer_page(0);
            load_history(&w, 0);
        });
    });
    tick(window, TICK_MS);
}

/// The server stays up; only the repaint slows down. An upload landing while
/// the user is in Photos still records itself and still triggers the rescan.
fn leave(window: &MainWindow) {
    if with(|svc| svc.is_running()).unwrap_or(false) {
        tick(window, IDLE_TICK_MS);
    } else {
        TICK.with(|t| t.stop());
    }
}

fn tick(window: &MainWindow, every_ms: u64) {
    let w = window.as_weak();
    TICK.with(|t| {
        // `start` on a running timer restarts it with the new interval.
        t.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(every_ms), move || {
            if let Some(w) = w.upgrade() {
                refresh(&w);
            }
        })
    });
}

// ── service lifecycle ───────────────────────────────────────────────────────

fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
        }
        // No reactor (a unit test, or a build that never entered the runtime):
        // the section simply does not start rather than panicking the app.
        Err(_) => tracing::warn!("transfer: no tokio runtime; not starting"),
    }
}

/// Serialises start against start, and start against stop.
///
/// A start takes the service out of `service()` and owns it across an await, so
/// two arriving together each built one: the second bound the fallback port
/// (the first already held 8420), then parked itself over the first — which
/// dropped the first server and freed 8420 again. The page was left showing a
/// random port that Stop/Start then "fixed", because by then 8420 really was
/// free. Home's Transfer tile does exactly that: `section-changed("transfer")`
/// and `transfer-start()` on the one click. With this held, the second caller
/// finds the service already running and `TransferService::start` returns
/// early.
fn lifecycle() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(tokio::sync::Mutex::default)
}

async fn start_service() {
    let _serialised = lifecycle().lock().await;
    let inbox = inbox_path();
    let mut svc = guard().take().unwrap_or_else(|| TransferService::new(inbox.clone()));
    svc.set_inbox(inbox);
    let started = svc.start().await;
    if !started {
        tracing::info!("transfer: not serving — {}", svc.snapshot().error);
    }
    *guard() = Some(svc);
}

async fn stop_service() {
    let _serialised = lifecycle().lock().await;
    let taken = guard().take();
    if let Some(mut svc) = taken {
        svc.stop().await;
        *guard() = Some(svc);
    }
    SEEN_REV.store(u64::MAX, std::sync::atomic::Ordering::Relaxed);
    SEEN_DONE.store(0, std::sync::atomic::Ordering::Relaxed);
}

fn inbox_path() -> PathBuf {
    let stored = tulipix_core::settings::Settings::load()
        .map(|s| s.text(INBOX_KEY))
        .unwrap_or_default();
    if stored.trim().is_empty() {
        tulipix_transfer::default_inbox()
    } else {
        PathBuf::from(stored)
    }
}

// ── painting ────────────────────────────────────────────────────────────────

/// Read the service once and push the whole page. Every list goes through
/// `replace_rows`: these lists are redrawn by buttons that live inside their own
/// rows, and swapping the model out from under one is slint#6426.
fn refresh(w: &MainWindow) {
    // `None` only while an async start/stop owns the service outright — a
    // window of milliseconds. Leave the page as it is rather than flickering
    // "Off" and back.
    let Some(snap) = with(|svc| svc.snapshot()) else { return };

    w.set_transfer_running(snap.running);
    w.set_transfer_url(snap.url.clone().into());
    w.set_transfer_pin(snap.pin.clone().into());
    // Empty unless there is a certificate at all, which is what makes the hint in
    // the Connection card conditional without a second flag.
    w.set_transfer_trust_url(if snap.secure { snap.trust_url.clone().into() } else { "".into() });
    w.set_transfer_fingerprint(snap.fingerprint.clone().into());
    w.set_transfer_host_url(snap.host_url.clone().into());
    w.set_transfer_inbox(snap.inbox.display().to_string().into());
    w.set_transfer_inbox_ok(snap.inbox_ok);
    w.set_transfer_iface(snap.iface.clone().into());
    w.set_transfer_total(if snap.files.is_empty() {
        SharedString::from("")
    } else {
        format!("{} · {}", plural(snap.files.len(), "file"), human_size(snap.total_bytes)).into()
    });
    w.set_transfer_status(status_line(&snap).into());
    w.set_transfer_hint(firewall_hint(&snap).into());
    // The Help panel interpolates the real port into every platform's advice —
    // a rule written for the wrong port is worse than no rule. Before the first
    // bind there is no live port, and the panel is readable then too, so it
    // falls back to the fixed one the next start will ask for.
    w.set_transfer_port(if snap.port == 0 {
        tulipix_transfer::PORT as i32
    } else {
        snap.port as i32
    });
    w.set_transfer_attempts(attempts_line(&snap).into());

    w.set_transfer_share_target(snap.share_target.clone().into());

    // A file carries a device *token*; the row has to show a name. Resolved here
    // against the same device list the dropdown is drawn from, so a rename shows
    // up on the tray row and on the closed dropdown as well as in its list.
    let device_name = |token: &str| -> String {
        snap.devices
            .iter()
            .find(|d| d.token == token)
            .map(|d| {
                if !d.name.is_empty() {
                    d.name.clone()
                } else if !d.kind.is_empty() {
                    d.kind.clone()
                } else {
                    d.label.clone()
                }
            })
            // A token with no device behind it is one that has just been
            // forgotten; the tray widens those out on the next snapshot, so this
            // is a one-tick gap rather than a state to name.
            .unwrap_or_default()
    };

    w.set_transfer_share_target_name(device_name(&snap.share_target).into());

    let files: Vec<TransferFile> = snap
        .files
        .iter()
        .map(|f| TransferFile {
            id: f.id as i32,
            name: f.name.clone().into(),
            size: human_size(f.bytes).into(),
            kind: kind_of(&f.name).into(),
            to: if f.to.is_empty() { String::new() } else { device_name(&f.to) }.into(),
        })
        .collect();
    set_rows(&w.get_transfer_files(), files, |rows| w.set_transfer_files(rows));

    // A hotspot and a Wi-Fi link commonly both qualify, so this is a picker in
    // the Send pane rather than a guess made here.
    let ifaces: Vec<TransferIface> = snap
        .interfaces
        .iter()
        .map(|(name, ip)| TransferIface { name: name.clone().into(), ip: ip.clone().into() })
        .collect();
    set_rows(&w.get_transfer_ifaces(), ifaces, |rows| w.set_transfer_ifaces(rows));

    let devices: Vec<TransferDevice> = snap
        .devices
        .iter()
        .enumerate()
        .map(|(i, d)| TransferDevice {
            token: d.token.clone().into(),
            // What to call it: the name typed on the desktop, the device type when
            // there is none, and — for a device paired before either was recorded
            // — whatever its User-Agent said.
            name: if !d.name.is_empty() {
                d.name.clone()
            } else if !d.kind.is_empty() {
                d.kind.clone()
            } else {
                d.label.clone()
            }
            .into(),
            kind: d.kind.clone().into(),
            label: d.label.clone().into(),
            ip: d.ip.clone().into(),
            pin: d.pin.clone().into(),
            remaining: left(d.remaining).into(),
            busy: d.busy,
            seen: ago(d.last_seen).into(),
            // The row a button lands in: five to a row in the Connection card.
            idx: i as i32,
        })
        .collect();
    set_rows(&w.get_transfer_devices(), devices, |rows| w.set_transfer_devices(rows));

    // One sampling for both directions: the service hands over cumulative byte
    // counts and no clock, on purpose, so the rate is the difference between
    // two ticks and this is the only place that knows how far apart they were.
    let moving: Vec<(String, u64)> = snap
        .uploads
        .iter()
        .filter(|u| u.state == "active")
        .map(|u| (format!("up:{}", u.id), u.done))
        .chain(snap.sends.iter().map(|s| (format!("tx:{}", s.row), s.done)))
        .collect();
    let rates = sample_rates(&moving);
    let rate_of = |key: &str| -> SharedString {
        match rates.get(key).copied().unwrap_or(0) {
            0 => SharedString::new(),
            b => format!("{}/s", human_size(b)).into(),
        }
    };

    let uploads: Vec<TransferUp> = snap
        .uploads
        .iter()
        .map(|u| TransferUp {
            id: u.id as i32,
            name: u.name.clone().into(),
            size: human_size(u.total.max(u.done)).into(),
            pct: if u.total == 0 { 0.0 } else { (u.done as f32 / u.total as f32).min(1.0) },
            // "12.4 MB / 40 MB" only while it is moving; a finished row already
            // says "Done" and its size, and two ways of saying the same size is
            // one too many.
            moved: if u.state == "active" && u.total > 0 {
                format!("{} / {}", human_size(u.done), human_size(u.total)).into()
            } else {
                SharedString::new()
            },
            rate: if u.state == "active" { rate_of(&format!("up:{}", u.id)) } else { "".into() },
            state: u.state.clone().into(),
        })
        .collect();
    set_rows(&w.get_transfer_uploads(), uploads, |rows| w.set_transfer_uploads(rows));

    // Downloads have no pane of their own — their row in Recent Transfers is
    // the only place they appear — so the progress is written onto that row
    // rather than into a list of its own. In place, not a model swap: these
    // rows carry live buttons, and swapping the model under one is slint#6426.
    patch_send_progress(w, &snap, &rates);

    // The QR carries a single-use key with a 60-second life, so the address is
    // not the only thing that can invalidate it: the first phone to scan spends
    // it, and the clock kills it either way. Minting once per address meant the
    // code on screen was dead within a minute and could never pair a second
    // device — forget a phone and try to scan again and you got the PIN form.
    //
    // So: redraw when the address changes *or* when the key behind it is no
    // longer live. `pairing_live` goes false at half the TTL, which puts a fresh
    // code up every 30s at worst — slow enough to scan, and never stale.
    if snap.running {
        let mut drawn = qr_drawn_for();
        if *drawn != snap.url || !with(|svc| svc.pairing_live()).unwrap_or(false) {
            // Both tones from the one key: the enlarged view can be flipped to
            // light-on-dark without minting a second code, which would spend a
            // pairing key for a colour change.
            if let Some(url) = with(|svc| svc.pairing_url()) {
                if let Some(img) = qr::render(&url, false) {
                    w.set_transfer_qr(img);
                    if let Some(inv) = qr::render(&url, true) {
                        w.set_transfer_qr_inv(inv);
                    }
                    drawn.clone_from(&snap.url);
                }
            }
        }
    } else {
        w.set_transfer_qr(slint::Image::default());
        w.set_transfer_qr_inv(slint::Image::default());
        qr_drawn_for().clear();
    }

    // Recent Transfers only when the ledger actually moved.
    let seen = SEEN_REV.load(std::sync::atomic::Ordering::Relaxed);
    if snap.running && seen != snap.rev {
        SEEN_REV.store(snap.rev, std::sync::atomic::Ordering::Relaxed);
        load_history(w, w.get_transfer_page().max(0) as usize);
    }

    // Something finished landing in the inbox. `refresh-library` re-walks the
    // watched folders silently, so a section picks the file up only if the
    // inbox is inside one of its libraries — which is the rule we want, and it
    // costs nothing to reuse the path the photo editor already uses on save.
    let done = snap.uploads.iter().filter(|u| u.state == "done").count();
    if done > SEEN_DONE.swap(done, std::sync::atomic::Ordering::Relaxed) {
        w.invoke_refresh_library();
    }
}

/// Wrong PIN guesses, for the Send pane.
///
/// The lockout is invisible from the desktop otherwise: the guessing phone gets
/// a 429 and this end shows nothing, so a person grinding the PIN on your
/// network would go unnoticed until they gave up.
fn attempts_line(snap: &tulipix_transfer::Snapshot) -> String {
    if snap.attempts.is_empty() {
        return String::new();
    }
    let locked = tulipix_transfer::auth::MAX_ATTEMPTS;
    let parts: Vec<String> = snap
        .attempts
        .iter()
        .map(|(ip, n)| {
            if *n >= locked {
                format!("{ip} — locked out after {n} wrong PINs")
            } else {
                format!("{ip} — {n} wrong of {locked}")
            }
        })
        .collect();
    parts.join("\n")
}

/// The one fix worth trying first, for *this* machine, with its real port and
/// its real interface in it.
///
/// The Help panel carries static advice for every platform; this is the block
/// that cannot be static, because a firewall rule naming the wrong port or the
/// wrong link is worse than no rule at all. It supplies the heading, so this
/// starts straight in on the answer.
///
/// Only meaningful while sharing: before the first bind there is no interface
/// chosen and no port to name.
fn firewall_hint(snap: &tulipix_transfer::Snapshot) -> String {
    if !snap.running {
        return String::new();
    }
    let port = snap.port;

    #[cfg(target_os = "linux")]
    {
        // The interface name, not the address: that is what a ufw rule takes,
        // and scoping the rule to one link means it does not follow the laptop
        // onto the next network it joins.
        let iface = snap
            .interfaces
            .iter()
            .find(|(_, ip)| *ip == snap.iface)
            .map(|(name, _)| name.as_str())
            .unwrap_or("wlan0");
        format!(
            "A default-deny firewall (ufw, firewalld) drops this before it reaches the app, and \
             the desktop cannot tell — the bind succeeded either way.\n\n\
             Run this, for the link you are actually sharing on:\n\n\
             sudo ufw allow in on {iface} to any port {port} proto tcp"
        )
    }
    #[cfg(target_os = "windows")]
    {
        format!(
            "Windows Firewall asks once, on the first bind, and dismissing that dialog silently \
             blocks every later connection — it never asks again.\n\n\
             Allow Tulipix for private networks, or open TCP {port}. See the Windows section \
             below for where that setting lives."
        )
    }
    #[cfg(target_os = "macos")]
    {
        format!(
            "macOS asks for Local Network permission once, and a denied prompt never returns.\n\n\
             Allow Tulipix at System Settings → Privacy & Security → Local Network, then confirm \
             the firewall is not blocking TCP {port}."
        )
    }
}

fn status_line(snap: &tulipix_transfer::Snapshot) -> String {
    if !snap.error.is_empty() {
        return snap.error.clone();
    }
    if !snap.running {
        return "Not sharing. Start to open a page on this network.".into();
    }
    if !snap.inbox_ok {
        return format!("Uploads are off: {} cannot be written to.", snap.inbox.display());
    }
    if cfg!(target_os = "windows") {
        // A dismissed firewall prompt looks exactly like a bug from the phone.
        return format!(
            "Serving on {}. If Windows asked about the firewall, allow it or the phone sees nothing.",
            snap.url
        );
    }
    format!("Serving on {}. Open until you stop sharing or close the app.", snap.url)
}

/// One file's byte counter, as it was the last time it was looked at.
struct Sample {
    done: u64,
    at: std::time::Instant,
    rate: f64,
}

fn rate_table() -> std::sync::MutexGuard<'static, std::collections::HashMap<String, Sample>> {
    static RATES: OnceLock<Mutex<std::collections::HashMap<String, Sample>>> = OnceLock::new();
    RATES.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Bytes per second for every file moving right now, keyed `up:<id>` for an
/// upload and `tx:<ledger row>` for a download.
///
/// Smoothed, because a 600 ms sample that happens to land between two 64 KB
/// chunks reads as a stall, and a rate that flickers to zero on a healthy
/// transfer is worse than no rate at all. Entries for files that have stopped
/// moving are dropped here rather than swept elsewhere — the caller passes the
/// whole live set on every tick, so anything absent is finished.
fn sample_rates(live: &[(String, u64)]) -> std::collections::HashMap<String, u64> {
    let now = std::time::Instant::now();
    let mut table = rate_table();
    let mut out = std::collections::HashMap::with_capacity(live.len());
    for (key, done) in live {
        let seen = table
            .entry(key.clone())
            .or_insert(Sample { done: *done, at: now, rate: 0.0 });
        let elapsed = now.duration_since(seen.at).as_secs_f64();
        if elapsed >= 0.25 {
            let moved = done.saturating_sub(seen.done) as f64;
            let instant = moved / elapsed;
            seen.rate = if seen.rate == 0.0 { instant } else { seen.rate * 0.6 + instant * 0.4 };
            seen.done = *done;
            seen.at = now;
        }
        out.insert(key.clone(), seen.rate as u64);
    }
    table.retain(|key, _| out.contains_key(key));
    out
}

/// Fill in the progress of every Recent Transfers row that is still sending,
/// and blank it on every row that is not.
///
/// `set_row_data` rather than a rebuilt model: the table is reloaded only when
/// the ledger's revision changes, which a download in progress does not touch,
/// and rebuilding it several times a second would tear down rows that hold live
/// buttons.
fn patch_send_progress(
    w: &MainWindow,
    snap: &tulipix_transfer::Snapshot,
    rates: &std::collections::HashMap<String, u64>,
) {
    let rows = w.get_transfer_rows();
    for i in 0..rows.row_count() {
        let Some(row) = rows.row_data(i) else { continue };
        if row.status != "sending" {
            // A row that has just settled keeps its stale bar until the reload
            // lands, which is a tick away.
            if row.pct != 0.0 || !row.progress.is_empty() {
                rows.set_row_data(i, TransferRow { pct: 0.0, progress: "".into(), ..row });
            }
            continue;
        }
        let live = snap.sends.iter().find(|s| s.row == row.id as i64);
        let (pct, progress) = match live {
            Some(s) if s.total > 0 => {
                let pct = (s.done as f32 / s.total as f32).min(1.0);
                let rate = rates.get(&format!("tx:{}", s.row)).copied().unwrap_or(0);
                let pctlabel = format!("{}%", (pct * 100.0).round() as i32);
                let label = if rate > 0 {
                    format!("{pctlabel} · {}/s", human_size(rate))
                } else {
                    pctlabel
                };
                (pct, SharedString::from(label))
            }
            // In the ledger as sending, but not on the wire: the settling
            // update is one tick behind, or this is a row from an earlier run.
            _ => (0.0, SharedString::new()),
        };
        if row.pct != pct || row.progress != progress {
            rows.set_row_data(i, TransferRow { pct, progress, ..row });
        }
    }
}

/// Install rows without swapping the model out from under a live repeater.
fn set_rows<T: Clone + 'static>(
    model: &ModelRc<T>,
    rows: Vec<T>,
    install: impl FnOnce(ModelRc<T>),
) {
    if let Some(rows) = replace_rows(model, rows) {
        install(ModelRc::new(VecModel::from(rows)));
    }
}

fn load_history(w: &MainWindow, page: usize) {
    let Some(pool) = with(|svc| svc.pool()).flatten() else {
        return;
    };
    // Ordering is the database's job — the ten rows on screen are a window onto
    // the whole table, so sorting them in place would only reshuffle the window.
    let sort = tulipix_transfer::ledger::Sort::from_index(w.get_transfer_sort());
    let desc = w.get_transfer_sort_desc();
    let weak = w.as_weak();
    spawn(async move {
        let entries =
            tulipix_transfer::ledger::recent(&pool, page, sort, desc).await.unwrap_or_default();
        let total = tulipix_transfer::ledger::count(&pool).await.unwrap_or(0);
        type Row =
            (i64, String, String, String, String, String, String, String, String, String, bool);
        let rows: Vec<Row> = entries
            .into_iter()
            .map(|e| {
                let path = e.abs_path.unwrap_or_default();
                // A row whose file has been moved or deleted is greyed, not
                // hidden: knowing something arrived and then went is useful.
                let missing = !path.is_empty() && !std::path::Path::new(&path).exists();
                let kind = kind_of(&e.name);
                let kindlabel = kind_label(&e.name);
                (
                    e.id,
                    e.direction,
                    e.name,
                    path,
                    human_size(e.bytes.max(0) as u64),
                    e.peer,
                    e.status,
                    when(e.at),
                    kind,
                    kindlabel,
                    missing,
                )
            })
            .collect();

        let pages = ((total as usize).div_ceil(tulipix_transfer::ledger::PAGE_SIZE)).max(1);
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<TransferRow> = rows
                .into_iter()
                .map(
                    |(
                        id,
                        direction,
                        name,
                        path,
                        size,
                        peer,
                        status,
                        stamp,
                        kind,
                        kindlabel,
                        missing,
                    )| {
                        TransferRow {
                            id: id as i32,
                            direction: direction.into(),
                            name: name.into(),
                            path: path.into(),
                            size: size.into(),
                            peer: peer.into(),
                            status: status.into(),
                            stamp: stamp.into(),
                            kind: kind.into(),
                            kindlabel: kindlabel.into(),
                            missing,
                            // Filled in on the next tick by `patch_send_progress`,
                            // which is what knows the bytes actually moving.
                            pct: 0.0,
                            progress: SharedString::new(),
                        }
                    },
                )
                .collect();
            set_rows(&w.get_transfer_rows(), rows, |rows| w.set_transfer_rows(rows));
            w.set_transfer_pages(pages as i32);
        });
    });
}

/// Hand a URL to the OS default browser. Best-effort: a machine with no browser
/// registered is a machine where the address beside the button is still there to
/// be copied.
fn open_url(url: &str) {
    let prog = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "windows") {
        "explorer"
    } else {
        "xdg-open"
    };
    if let Err(e) = std::process::Command::new(prog).arg(url).spawn() {
        tracing::warn!(error = %e, "transfer: could not open the browser");
    }
}

// ── small formatters ────────────────────────────────────────────────────────

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// How long a pairing has left, for the device popup. Coarse on purpose: the
/// number that matters is "today" or "tomorrow", and a ticking countdown would
/// redraw the list every second to say nothing new.
fn left(secs: u64) -> String {
    if secs == 0 {
        return "expired".into();
    }
    let hours = secs / 3600;
    if hours >= 1 {
        let minutes = (secs % 3600) / 60;
        if hours >= 6 || minutes == 0 {
            plural(hours as usize, "hour")
        } else {
            format!("{}, {}", plural(hours as usize, "hour"), plural(minutes as usize, "min"))
        }
    } else {
        plural(((secs + 59) / 60) as usize, "min")
    }
}

fn kind_of(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_uppercase())
        .filter(|e| e.len() <= 4)
        .unwrap_or_else(|| "FILE".into())
}

/// The second line under a filename in the transfers table — "PDF Document"
/// rather than a bare extension. Only the families worth naming; everything
/// else says what it is without pretending to know more.
fn kind_label(name: &str) -> String {
    let ext = std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let family = match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "avif" | "bmp" | "tiff" => "Image",
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "wmv" => "Video",
        "mp3" | "flac" | "wav" | "m4a" | "ogg" | "opus" | "aac" => "Audio",
        "pdf" => "PDF Document",
        "epub" | "mobi" | "azw3" | "cbz" | "cbr" => "Book",
        "zip" | "rar" | "7z" | "tar" | "gz" | "xz" | "zst" => "Archive",
        "doc" | "docx" | "odt" | "rtf" | "txt" | "md" => "Document",
        "xls" | "xlsx" | "ods" | "csv" => "Spreadsheet",
        "ppt" | "pptx" | "odp" => "Presentation",
        "apk" | "exe" | "msi" | "dmg" | "appimage" | "deb" | "rpm" => "Application",
        "" => return "File".into(),
        _ => return format!("{} file", ext.to_uppercase()),
    };
    family.to_string()
}

use tulipix_core::util::unix_secs_i64 as now_secs;

/// "just now" / "12 min ago" / "3 h ago", then the date once it stops mattering.
fn when(at: i64) -> String {
    let delta = now_secs() - at;
    match delta {
        d if d < 0 => tulipix_common::fmt_date(at),
        d if d < 60 => "just now".into(),
        d if d < 3600 => format!("{} min ago", d / 60),
        d if d < 86_400 => format!("{} h ago", d / 3600),
        _ => tulipix_common::fmt_date(at),
    }
}

fn ago(at: u64) -> String {
    when(at as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_moments_read_relatively_and_old_ones_by_date() {
        let now = now_secs();
        assert_eq!(when(now), "just now");
        assert_eq!(when(now - 600), "10 min ago");
        assert_eq!(when(now - 7200), "2 h ago");
        // Two days back falls through to the date.
        assert!(when(now - 2 * 86_400).contains(char::is_numeric));
    }

    #[test]
    fn file_kinds_are_short_badges_and_never_a_whole_name() {
        assert_eq!(kind_of("holiday.mp4"), "MP4");
        assert_eq!(kind_of("README"), "FILE");
        assert_eq!(kind_of("archive.tar.gz"), "GZ");
        // Anything longer than four characters is not a badge.
        assert_eq!(kind_of("notes.markdown"), "FILE");
    }

    #[test]
    fn counts_are_pluralised() {
        assert_eq!(plural(1, "file"), "1 file");
        assert_eq!(plural(3, "file"), "3 files");
        assert_eq!(plural(0, "file"), "0 files");
    }
}
