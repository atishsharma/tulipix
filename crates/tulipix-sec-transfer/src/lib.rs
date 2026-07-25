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

async fn start_service() {
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
    w.set_transfer_attempts(attempts_line(&snap).into());

    let files: Vec<TransferFile> = snap
        .files
        .iter()
        .map(|f| TransferFile {
            id: f.id as i32,
            name: f.name.clone().into(),
            size: human_size(f.bytes).into(),
            kind: kind_of(&f.name).into(),
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
        .map(|d| TransferDevice {
            token: d.token.clone().into(),
            label: d.label.clone().into(),
            seen: ago(d.last_seen).into(),
        })
        .collect();
    set_rows(&w.get_transfer_devices(), devices, |rows| w.set_transfer_devices(rows));

    let uploads: Vec<TransferUp> = snap
        .uploads
        .iter()
        .map(|u| TransferUp {
            name: u.name.clone().into(),
            size: human_size(u.total.max(u.done)).into(),
            pct: if u.total == 0 { 0.0 } else { (u.done as f32 / u.total as f32).min(1.0) },
            state: u.state.clone().into(),
        })
        .collect();
    set_rows(&w.get_transfer_uploads(), uploads, |rows| w.set_transfer_uploads(rows));

    // The QR carries a single-use key, so it is minted once per address rather
    // than on every tick — a code that changed twice a second would be
    // unscannable. A restart or a different interface changes the address, and
    // the URL it was drawn for is what notices.
    if snap.running {
        let mut drawn = qr_drawn_for();
        if *drawn != snap.url {
            if let Some(img) = with(|svc| svc.pairing_url()).and_then(|u| qr::render(&u)) {
                w.set_transfer_qr(img);
                drawn.clone_from(&snap.url);
            }
        }
    } else {
        w.set_transfer_qr(slint::Image::default());
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

/// The thing to check when the desktop says "Serving" and the phone says it
/// cannot connect. Every platform has one silent way to swallow the packets,
/// and in every case the bind succeeded, so nothing upstream can detect it —
/// the honest move is to name the fix with the real port in it.
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
            "Phone says it cannot connect? A default-deny firewall (ufw, firewalld) drops this \
             before it reaches the app, and the desktop cannot tell — the bind succeeded either \
             way. Open the port on that one link:\n\
             sudo ufw allow in on {iface} to any port {port} proto tcp"
        )
    }
    #[cfg(target_os = "windows")]
    {
        format!(
            "Phone says it cannot connect? Windows Firewall asks once, on the first bind, and \
             dismissing that dialog silently blocks every later connection. Allow Tulipix for \
             private networks, or open TCP {port}."
        )
    }
    #[cfg(target_os = "macos")]
    {
        format!(
            "Phone says it cannot connect? Check System Settings → Privacy & Security → Local \
             Network and allow Tulipix; also confirm the firewall is not blocking TCP {port}."
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
    let weak = w.as_weak();
    spawn(async move {
        let entries = tulipix_transfer::ledger::recent(&pool, page).await.unwrap_or_default();
        let total = tulipix_transfer::ledger::count(&pool).await.unwrap_or(0);
        type Row = (String, String, String, String, String, String, String, String, String, bool);
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
                .map(|(direction, name, path, size, peer, status, stamp, kind, kindlabel, missing)| {
                    TransferRow {
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
                    }
                })
                .collect();
            set_rows(&w.get_transfer_rows(), rows, |rows| w.set_transfer_rows(rows));
            w.set_transfer_pages(pages as i32);
        });
    });
}

// ── small formatters ────────────────────────────────────────────────────────

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
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

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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
