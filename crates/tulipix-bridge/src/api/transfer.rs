//! The Transfer section.
//!
//! `tulipix_transfer::TransferService` owns the axum server, the pairing keys,
//! the tray of offered files and the ledger. None of that moves. What this
//! module adds is what a UI needs and a server does not: one snapshot of the
//! whole page, a command enum, and the two pieces of arithmetic that only the
//! thing holding a clock can do — transfer rates, which are the difference
//! between two ticks, and the "did something just land in the inbox" edge.
//!
//! Polled, not streamed. The Slint build runs a 600 ms timer over exactly this
//! shape, because a progress bar is a sampled value: there is no event to
//! subscribe to that says "a further 64 KB arrived". Dart runs the same timer
//! and calls `transfer_dispatch(Refresh)`.

use crate::api::photos;
use anyhow::Result;
use flutter_rust_bridge::frb;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use tulipix_transfer::{human_size, TransferService};

/// Settings key for the inbox folder. `Settings.advanced` is a free-text map,
/// so this needs no schema change — and it is the same key the Slint build
/// reads, so the two agree about where files land.
const INBOX_KEY: &str = "transfer.inbox";

/// How many phones may be paired at once. Mirrors `auth::MAX_DEVICES`, which is
/// what actually refuses the eleventh; this is the caption.
const DEVICE_MAX: i64 = 10;

// ----------------------------------------------------------------- types ----

/// One file the desktop is offering. `id` is the tray id, never a path — the
/// phone cannot name a file on disk at all.
#[derive(Debug, Clone)]
pub struct TransferFile {
    pub id: i64,
    pub name: String,
    pub size: String,
    pub kind: String,
    /// The device this file is for, already resolved to a display name. Empty
    /// means every paired device.
    pub to: String,
}

/// One phone that has authenticated, until its 48 hours are up.
#[derive(Debug, Clone)]
pub struct TransferDevice {
    pub token: String,
    /// What to call it: the name typed on the desktop, the device type when
    /// there is none, and the User-Agent reading for one paired before either
    /// was recorded.
    pub name: String,
    /// The device type, which chooses the icon.
    pub kind: String,
    /// "Android · Chrome" — the detail line inside the popup.
    pub label: String,
    pub ip: String,
    /// The PIN this device paired with, which is not necessarily today's.
    pub pin: String,
    /// How long the pairing has left, coarsely.
    pub remaining: String,
    pub seen: String,
    /// Bytes are moving to or from this device right now.
    pub busy: bool,
}

/// One upload in flight, or just finished. `state` is active | done | failed.
#[derive(Debug, Clone)]
pub struct TransferUpload {
    pub id: i64,
    pub name: String,
    pub size: String,
    pub pct: f64,
    pub state: String,
    /// "12.4 MB / 40 MB", empty unless it is actually moving.
    pub moved: String,
    /// "3.1 MB/s", same rule — a rate on a finished row is a number about the
    /// past pretending to be about now.
    pub rate: String,
}

/// One row of the ledger. `stamp`, not `when`: `when` is a Slint keyword on the
/// other side of this port and the two structs are read together.
#[derive(Debug, Clone, Default)]
pub struct TransferLedgerRow {
    pub id: i64,
    /// in | out.
    pub direction: String,
    pub name: String,
    pub path: String,
    pub size: String,
    pub peer: String,
    /// The ledger's own word: ok | failed | sending.
    pub status: String,
    pub stamp: String,
    pub kind: String,
    pub kindlabel: String,
    /// The file has since been moved or deleted. Greyed, not hidden: knowing
    /// something arrived and then went is useful.
    pub missing: bool,
    pub pct: f64,
    /// "43% · 3.1 MB/s" while it is still on the wire.
    pub progress: String,
}

/// One address a phone could reach. A hotspot and a Wi-Fi link commonly both
/// qualify, so this is a picker rather than a guess.
#[derive(Debug, Clone)]
pub struct TransferIface {
    pub name: String,
    pub ip: String,
}

/// The pairing code as a module matrix, not as pixels.
///
/// Slint rasterises it to an image because that is what a Slint `Image` takes.
/// Dart can paint squares, so what crosses the boundary is the matrix: it is a
/// twentieth of the bytes, it stays crisp at the enlarged size, and inverting
/// it is a colour swap on the Dart side rather than a second render — which
/// would otherwise mean spending a second single-use pairing key on a theme.
#[derive(Debug, Clone)]
pub struct QrCode {
    pub size: i64,
    /// Row-major, `size * size` entries. True is a dark module.
    pub modules: Vec<bool>,
}

/// Everything the page draws, in one read of the service.
#[derive(Debug, Clone)]
pub struct TransferState {
    pub running: bool,
    /// The header line: the bind error verbatim when there is one, otherwise
    /// what is being served where.
    pub status: String,
    /// The one firewall fix worth trying first, for this machine, with its real
    /// port and link in it. Empty unless sharing.
    pub hint: String,
    /// Wrong PIN guesses, one line per address. Empty when nobody has missed.
    pub attempts: String,
    pub url: String,
    /// Where a phone installs the certificate, over plain HTTP. Empty when the
    /// server is not on TLS, which is what hides the strip.
    pub trust_url: String,
    pub fingerprint: String,
    /// The mDNS form of the same address, offered alongside it rather than
    /// instead of it: plenty of phones cannot resolve a `.local` name.
    pub host_url: String,
    pub pin: String,
    pub port: i64,
    /// Bumped whenever a new code is minted. Dart refetches `transfer_qr` when
    /// it changes rather than carrying a kilobyte of matrix on every tick.
    /// Zero when there is no code, which is what blanks the plate.
    pub qr_rev: i64,
    pub files: Vec<TransferFile>,
    /// "3 files · 41.2 MB", empty when the tray is.
    pub total: String,
    pub devices: Vec<TransferDevice>,
    pub device_max: i64,
    pub uploads: Vec<TransferUpload>,
    pub rows: Vec<TransferLedgerRow>,
    pub ifaces: Vec<TransferIface>,
    pub iface: String,
    /// Which paired device the tray is pointed at, empty for everyone.
    pub share_target: String,
    pub share_target_name: String,
    pub inbox: String,
    pub inbox_ok: bool,
    pub page: i64,
    pub pages: i64,
    /// The ledger's sort column as the table's visual index:
    /// Name 0 | Size 1 | Type 2 | Status 3 | From/To 4 | Time 5.
    pub sort: i64,
    pub sort_desc: bool,
}

#[derive(Debug, Clone)]
pub enum TransferCmd {
    /// Re-read the service and rebuild the page. What the timer sends.
    Refresh,
    Start,
    Stop,
    /// Opens the OS file chooser and adds whatever comes back.
    AddFiles,
    AddFolder,
    Remove { id: i64 },
    Clear,
    /// Empty aims the tray at every paired device.
    SetShareTarget { token: String },
    /// Open the phone page in this machine's own browser, already paired.
    OpenUrl,
    Forget { token: String },
    RenameDevice { token: String, name: String },
    SetIface { ip: String },
    PickInbox,
    OpenInbox,
    /// Reveal a ledger row's file in the file manager, by ledger id.
    OpenRow { row_id: i64 },
    /// Re-offer the file behind a failed send, and drop the row it failed on.
    RetryRow { row_id: i64 },
    ClearHistory,
    SetPage { page: i64 },
    SetSort { col: i64, desc: bool },
    DismissUpload { id: i64 },
}

// --------------------------------------------------------------- session ----

/// The service, parked in an `Option` so an async start/stop can own it
/// outright rather than holding a lock across an await. Commands that arrive
/// during those few milliseconds see `None` and no-op, which is the correct
/// answer for all of them.
///
/// `frb(ignore)` is not needed on a function, but every private struct below it
/// carries one: frb parses this whole module and will otherwise emit codecs for
/// types Dart never sees.
fn service() -> &'static Mutex<Option<TransferService>> {
    static S: std::sync::OnceLock<Mutex<Option<TransferService>>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(None))
}

fn guard() -> std::sync::MutexGuard<'static, Option<TransferService>> {
    service().lock().unwrap_or_else(|e| e.into_inner())
}

/// Run `f` against the service if it is parked.
fn with<R>(f: impl FnOnce(&TransferService) -> R) -> Option<R> {
    guard().as_ref().map(f)
}

/// What the Status dashboard reads about this service.
///
/// Returns the "not sharing" default when the service is parked mid-start/stop,
/// which is what it is at that moment.
pub(crate) fn status_snapshot(now: u64) -> tulipix_transfer::StatusSnapshot {
    with(|s| s.status_snapshot(now)).unwrap_or_default()
}

/// What the page remembers between commands. Not the service's business: the
/// ledger's sort order and page are a view, and a second window on the same
/// service would want its own.
#[frb(ignore)]
#[derive(Default)]
struct TransferSession {
    page: i64,
    sort: i64,
    sort_desc: bool,
    /// The last page of ledger rows built, so `OpenRow` and `RetryRow` can take
    /// an id rather than a screen position — a row index means something
    /// different the instant a tick reorders the table under the pointer.
    rows: Vec<TransferLedgerRow>,
    pages: i64,
    /// The ledger revision the rows were built from. A tick that changed
    /// nothing must not hit the database.
    seen_rev: u64,
    /// Completed uploads the last tick knew about. A rise means something
    /// landed in the inbox, which is when the library is asked to look again.
    seen_done: usize,
    qr: Option<QrCode>,
    qr_rev: i64,
    /// The URL the code on screen was drawn for.
    qr_for: String,
}

impl TransferSession {
    fn fresh() -> Self {
        Self {
            // Newest first: the same default `ledger::Sort` has, and the same
            // one the Slint page opens on.
            sort: 5,
            sort_desc: true,
            seen_rev: u64::MAX,
            ..Self::default()
        }
    }
}

fn session() -> &'static Mutex<TransferSession> {
    static S: std::sync::OnceLock<Mutex<TransferSession>> = std::sync::OnceLock::new();
    S.get_or_init(|| Mutex::new(TransferSession::fresh()))
}

fn sess() -> std::sync::MutexGuard<'static, TransferSession> {
    session().lock().unwrap_or_else(|e| e.into_inner())
}

/// Serialises start against start, and start against stop.
///
/// A start takes the service out and owns it across an await, so two arriving
/// together each built one: the second bound the fallback port, then parked
/// itself over the first — which dropped the first server and freed the real
/// port again. The page was left showing a port that Stop/Start then "fixed".
/// With this held, the second caller finds the service already running and
/// `TransferService::start` returns early.
fn lifecycle() -> &'static tokio::sync::Mutex<()> {
    static L: std::sync::OnceLock<tokio::sync::Mutex<()>> = std::sync::OnceLock::new();
    L.get_or_init(tokio::sync::Mutex::default)
}

// ---------------------------------------------------------------- exports ---

/// Apply one command and return the page that results from it.
///
/// Every arm ends in a snapshot, including the ones that only opened a file
/// chooser: the alternative is a UI that has to guess when to ask again.
pub async fn transfer_dispatch(cmd: TransferCmd) -> Result<TransferState> {
    match cmd {
        TransferCmd::Refresh => {}
        TransferCmd::Start => start_service().await,
        TransferCmd::Stop => stop_service().await,
        TransferCmd::AddFiles => {
            if let Some(files) = rfd::AsyncFileDialog::new().set_title("Share files").pick_files().await
            {
                let _ = with(|svc| {
                    for f in &files {
                        svc.add(f.path());
                    }
                });
            }
        }
        TransferCmd::AddFolder => {
            if let Some(dir) =
                rfd::AsyncFileDialog::new().set_title("Share a folder").pick_folder().await
            {
                let _ = with(|svc| svc.add(dir.path()));
            }
        }
        TransferCmd::Remove { id } => {
            let _ = with(|svc| svc.remove(id.max(0) as u64));
        }
        TransferCmd::Clear => {
            let _ = with(|svc| svc.clear());
        }
        TransferCmd::SetShareTarget { token } => {
            // Empty means everyone; the tray normalises it, so the UI can send
            // the chip's own value without a special case for the first chip.
            let _ = with(|svc| svc.set_share_target(Some(token)));
        }
        TransferCmd::OpenUrl => {
            // Through the same single-use pairing key the QR carries, not a
            // desktop-only bypass: one way in, one thing to reason about. The
            // desktop then shows up as a paired device, which is what it is.
            let url = with(|svc| svc.pairing_url()).unwrap_or_default();
            if !url.is_empty() {
                open_url(&url);
            }
        }
        TransferCmd::Forget { token } => {
            // The service is taken out for the duration of the await and put
            // straight back: `forget_device` takes `&self` and never re-enters
            // here, and a MutexGuard held across an await is not `Send`.
            let taken = { guard().take() };
            if let Some(svc) = taken {
                svc.forget_device(&token).await;
                *guard() = Some(svc);
            }
        }
        TransferCmd::RenameDevice { token, name } => {
            let taken = { guard().take() };
            if let Some(svc) = taken {
                svc.rename_device(&token, &name).await;
                *guard() = Some(svc);
            }
        }
        TransferCmd::SetIface { ip } => {
            // The listener is bound to one address, so picking a different one
            // is a rebind, not a relabel — otherwise the URL would name an
            // interface the socket is not on.
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
            if restart {
                stop_service().await;
                start_service().await;
            }
        }
        TransferCmd::PickInbox => {
            if let Some(dir) = rfd::AsyncFileDialog::new()
                .set_title("Choose the inbox folder")
                .pick_folder()
                .await
            {
                set_inbox(dir.path());
            }
        }
        TransferCmd::OpenInbox => {
            let path = inbox_path();
            let _ = std::fs::create_dir_all(&path);
            if let Err(e) = tulipix_platform::fm::open_default(&path) {
                tracing::warn!(error = %e, "transfer: could not open the inbox");
            }
        }
        TransferCmd::OpenRow { row_id } => {
            let path = { sess().rows.iter().find(|r| r.id == row_id).map(|r| r.path.clone()) };
            if let Some(path) = path.filter(|p| !p.is_empty()) {
                let path = PathBuf::from(path);
                if path.exists() {
                    if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&path) {
                        tracing::warn!(error = %e, "transfer: could not reveal the file");
                    }
                }
            }
        }
        TransferCmd::RetryRow { row_id } => retry(row_id).await?,
        TransferCmd::ClearHistory => {
            if let Some(pool) = with(|svc| svc.pool()).flatten() {
                if let Err(e) = tulipix_transfer::ledger::clear_transfers(&pool).await {
                    tracing::warn!(error = %e, "transfer: could not clear the ledger");
                }
            }
            {
                let mut s = sess();
                s.page = 0;
                s.seen_rev = u64::MAX;
            }
        }
        TransferCmd::SetPage { page } => {
            let mut s = sess();
            s.page = page.max(0);
            s.seen_rev = u64::MAX;
        }
        TransferCmd::SetSort { col, desc } => {
            let mut s = sess();
            s.sort = col;
            s.sort_desc = desc;
            // Back to page one: staying on page four of a differently-ordered
            // table shows rows that have nothing to do with what was clicked.
            s.page = 0;
            s.seen_rev = u64::MAX;
        }
        TransferCmd::DismissUpload { id } => {
            let _ = with(|svc| svc.dismiss_upload(id.max(0) as u64));
        }
    }
    snapshot().await
}

/// The pairing code currently on screen, or `None` when not sharing.
///
/// Fetched on its own rather than carried in `TransferState`, because a 33×33
/// matrix on every 600 ms tick is a kilobyte a second to say nothing new.
#[frb(sync)]
pub fn transfer_qr() -> Option<QrCode> {
    sess().qr.clone()
}

/// Stop serving. Called when the app is closing, so a closed window does not
/// leave a listener behind on the café wifi joined yesterday.
///
/// Synchronous and abrupt, unlike `Stop`: by this point there may be no runtime
/// left to await a graceful shutdown on.
#[frb(sync)]
pub fn transfer_shutdown() {
    if let Some(svc) = guard().as_mut() {
        svc.abort();
    }
}

// ------------------------------------------------------------- internals ----

async fn start_service() {
    let _serialised = lifecycle().lock().await;
    let inbox = inbox_path();
    let mut svc = { guard().take() }.unwrap_or_else(|| TransferService::new(inbox.clone()));
    svc.set_inbox(inbox);
    let started = svc.start().await;
    if !started {
        tracing::info!("transfer: not serving — {}", svc.snapshot().error);
    }
    *guard() = Some(svc);
    // A fresh run has a fresh ledger revision to compare against, and the
    // rate table from the last run describes files that are no longer moving.
    let mut s = sess();
    s.seen_rev = u64::MAX;
    s.seen_done = 0;
}

async fn stop_service() {
    let _serialised = lifecycle().lock().await;
    let taken = { guard().take() };
    if let Some(mut svc) = taken {
        svc.stop().await;
        *guard() = Some(svc);
    }
    let mut s = sess();
    s.seen_rev = u64::MAX;
    s.seen_done = 0;
    s.qr = None;
    s.qr_rev = 0;
    s.qr_for.clear();
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

fn set_inbox(dir: &Path) {
    let mut s = tulipix_core::settings::Settings::load().unwrap_or_default();
    s.advanced.insert(INBOX_KEY.to_string(), dir.display().to_string());
    if let Err(e) = s.save() {
        tracing::warn!(error = %e, "transfer: could not persist the inbox path");
    }
    if let Some(svc) = guard().as_mut() {
        svc.set_inbox(dir.to_path_buf());
    }
}

/// Re-offer the file behind a failed send.
///
/// The desktop cannot push, so "send it again" means putting the file back in
/// the tray for the phone to pull — and dropping the row it failed on, so the
/// table does not end up with a stale failure sitting next to the attempt that
/// replaced it.
async fn retry(row_id: i64) -> Result<()> {
    let row = { sess().rows.iter().find(|r| r.id == row_id).cloned() };
    let Some(row) = row else { return Ok(()) };
    if row.direction != "out" || row.path.is_empty() {
        return Ok(());
    }
    let path = PathBuf::from(&row.path);
    if !path.exists() {
        // The file moved since it failed. Nothing to re-offer, and the row is
        // already greyed to say so.
        return Ok(());
    }
    // Sharing may have been stopped since — a retry is what happens after the
    // connection comes back — and a tray that is not serving swallows the file
    // silently, so nothing is dropped from the ledger until it has actually
    // been re-offered.
    let offered = with(|svc| {
        if !svc.is_running() {
            return false;
        }
        svc.add(&path);
        true
    })
    .unwrap_or(false);
    if !offered {
        return Ok(());
    }
    if let Some(pool) = with(|svc| svc.pool()).flatten() {
        if let Err(e) = tulipix_transfer::ledger::forget(&pool, row_id).await {
            tracing::warn!(error = %e, "transfer: could not drop the failed row");
        }
    }
    sess().seen_rev = u64::MAX;
    Ok(())
}

/// Hand a URL to the OS default browser. Best-effort: a machine with no browser
/// registered is a machine where the address beside the button is still there
/// to be copied.
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

// ---------------------------------------------------------------- reading ---

/// Read the service once and build the whole page.
///
/// The ledger is only refetched when its revision moved, and the QR is only
/// redrawn when the address changed or the key behind it went stale — both
/// checks live here because this is the only thing that runs on every tick.
async fn snapshot() -> Result<TransferState> {
    // `None` only while an async start/stop owns the service outright, a window
    // of milliseconds. Hand back the last page rather than flickering "Off" and
    // straight back on.
    let Some(snap) = with(|svc| svc.snapshot()) else {
        return Ok(last_state().unwrap_or_else(off_state));
    };

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
    let rate_of = |key: &str| -> String {
        match rates.get(key).copied().unwrap_or(0) {
            0 => String::new(),
            b => format!("{}/s", human_size(b)),
        }
    };

    // A file carries a device *token*; the row has to show a name. Resolved
    // against the same device list the dropdown is drawn from, so a rename
    // shows on the tray row and on the closed dropdown as well as in its list.
    let device_name = |token: &str| -> String {
        snap.devices
            .iter()
            .find(|d| d.token == token)
            .map(display_name)
            // A token with no device behind it is one that has just been
            // forgotten; the tray widens those out on the next snapshot, so
            // this is a one-tick gap rather than a state to name.
            .unwrap_or_default()
    };

    let files: Vec<TransferFile> = snap
        .files
        .iter()
        .map(|f| TransferFile {
            id: f.id as i64,
            name: f.name.clone(),
            size: human_size(f.bytes),
            kind: kind_of(&f.name),
            to: if f.to.is_empty() { String::new() } else { device_name(&f.to) },
        })
        .collect();

    let devices: Vec<TransferDevice> = snap
        .devices
        .iter()
        .map(|d| TransferDevice {
            token: d.token.clone(),
            name: display_name(d),
            kind: d.kind.clone(),
            label: d.label.clone(),
            ip: d.ip.clone(),
            pin: d.pin.clone(),
            remaining: left(d.remaining),
            seen: when(d.last_seen as i64),
            busy: d.busy,
        })
        .collect();

    let uploads: Vec<TransferUpload> = snap
        .uploads
        .iter()
        .map(|u| TransferUpload {
            id: u.id as i64,
            name: u.name.clone(),
            size: human_size(u.total.max(u.done)),
            pct: if u.total == 0 { 0.0 } else { (u.done as f64 / u.total as f64).min(1.0) },
            // "12.4 MB / 40 MB" only while it is moving; a finished row already
            // says "Done" and its size, and two ways of saying the same size is
            // one too many.
            moved: if u.state == "active" && u.total > 0 {
                format!("{} / {}", human_size(u.done), human_size(u.total))
            } else {
                String::new()
            },
            rate: if u.state == "active" { rate_of(&format!("up:{}", u.id)) } else { String::new() },
            state: u.state.clone(),
        })
        .collect();

    // The QR carries a single-use key with a 60-second life, so the address is
    // not the only thing that can invalidate it: the first phone to scan spends
    // it, and the clock kills it either way. Minting once per address meant the
    // code on screen was dead within a minute and could never pair a second
    // device. `pairing_live` goes false at half the TTL, which puts a fresh code
    // up every 30s at worst — slow enough to scan, and never stale.
    if snap.running {
        let addr_changed = {
            let s = sess();
            s.qr.is_none() || s.qr_for != snap.url
        };
        if addr_changed || !with(|svc| svc.pairing_live()).unwrap_or(false) {
            if let Some(code) = with(|svc| svc.pairing_url()).and_then(|u| render_qr(&u)) {
                let mut s = sess();
                s.qr = Some(code);
                s.qr_rev += 1;
                s.qr_for.clone_from(&snap.url);
            }
        }
    }

    // Recent Transfers, only when the ledger actually moved. Ordering is the
    // database's job — the ten rows on screen are a window onto the whole
    // table, so sorting them in place would only reshuffle the window.
    let (page, sort, sort_desc, reload) = {
        let s = sess();
        (s.page, s.sort, s.sort_desc, s.seen_rev != snap.rev)
    };
    if reload {
        if let Some(pool) = with(|svc| svc.pool()).flatten() {
            let (rows, pages) = load_history(&pool, page, sort, sort_desc).await;
            let mut s = sess();
            s.rows = rows;
            s.pages = pages;
            s.seen_rev = snap.rev;
        }
    }

    // Something finished landing in the inbox. The rescan re-walks the watched
    // folders, so a section picks the file up only if the inbox is inside one
    // of its libraries — which is the rule we want.
    //
    // Photos is the only section this build has. When the rest land, the
    // fan-out belongs to the shell (phase 4), which is what owns "a file
    // arrived" as an app-wide event rather than a Transfer one.
    let done = snap.uploads.iter().filter(|u| u.state == "done").count();
    let landed = {
        let mut s = sess();
        let rose = done > s.seen_done;
        s.seen_done = done;
        rose
    };
    if landed {
        // Spawned: a library walk takes seconds and this runs on a 600 ms tick.
        tokio::spawn(async {
            if let Ok(pool) = crate::db::photos_pool().await {
                photos::scan_watched(pool).await;
            }
        });
    }

    let mut rows = { sess().rows.clone() };
    patch_send_progress(&mut rows, &snap, &rates);

    let (page, pages, sort, sort_desc, qr_rev) = {
        let s = sess();
        (s.page, s.pages.max(1), s.sort, s.sort_desc, if snap.running { s.qr_rev } else { 0 })
    };

    let state = TransferState {
        running: snap.running,
        status: status_line(&snap),
        hint: firewall_hint(&snap),
        attempts: attempts_line(&snap),
        url: snap.url.clone(),
        // Empty unless there is a certificate at all, which is what makes the
        // hint conditional without a second flag.
        trust_url: if snap.secure { snap.trust_url.clone() } else { String::new() },
        fingerprint: snap.fingerprint.clone(),
        host_url: snap.host_url.clone(),
        pin: snap.pin.clone(),
        // The Help panel interpolates the real port into every platform's
        // advice — a rule written for the wrong port is worse than no rule.
        // Before the first bind there is no live port, and the panel is
        // readable then too, so it falls back to the fixed one the next start
        // will ask for.
        port: if snap.port == 0 { tulipix_transfer::PORT as i64 } else { snap.port as i64 },
        qr_rev,
        total: if snap.files.is_empty() {
            String::new()
        } else {
            format!("{} · {}", plural(snap.files.len(), "file"), human_size(snap.total_bytes))
        },
        files,
        share_target_name: device_name(&snap.share_target),
        share_target: snap.share_target.clone(),
        devices,
        device_max: DEVICE_MAX,
        uploads,
        rows,
        ifaces: snap
            .interfaces
            .iter()
            .map(|(name, ip)| TransferIface { name: name.clone(), ip: ip.clone() })
            .collect(),
        iface: snap.iface.clone(),
        inbox: snap.inbox.display().to_string(),
        inbox_ok: snap.inbox_ok,
        page,
        pages,
        sort,
        sort_desc,
    };
    *last().lock().unwrap_or_else(|e| e.into_inner()) = Some(state.clone());
    Ok(state)
}

fn last() -> &'static Mutex<Option<TransferState>> {
    static L: std::sync::OnceLock<Mutex<Option<TransferState>>> = std::sync::OnceLock::new();
    L.get_or_init(|| Mutex::new(None))
}

fn last_state() -> Option<TransferState> {
    last().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// What the page looks like before anything has ever been started.
fn off_state() -> TransferState {
    TransferState {
        running: false,
        status: "Not sharing. Start to open a page on this network.".into(),
        hint: String::new(),
        attempts: String::new(),
        url: String::new(),
        trust_url: String::new(),
        fingerprint: String::new(),
        host_url: String::new(),
        pin: String::new(),
        port: tulipix_transfer::PORT as i64,
        qr_rev: 0,
        files: Vec::new(),
        total: String::new(),
        devices: Vec::new(),
        device_max: DEVICE_MAX,
        uploads: Vec::new(),
        rows: Vec::new(),
        ifaces: Vec::new(),
        iface: String::new(),
        share_target: String::new(),
        share_target_name: String::new(),
        inbox: inbox_path().display().to_string(),
        inbox_ok: true,
        page: 0,
        pages: 1,
        sort: 5,
        sort_desc: true,
    }
}

/// One page of the ledger, plus how many pages there are.
async fn load_history(
    pool: &sqlx::SqlitePool,
    page: i64,
    sort: i64,
    desc: bool,
) -> (Vec<TransferLedgerRow>, i64) {
    let sort = tulipix_transfer::ledger::Sort::from_index(sort as i32);
    let entries = tulipix_transfer::ledger::recent(pool, page.max(0) as usize, sort, desc)
        .await
        .unwrap_or_default();
    let total = tulipix_transfer::ledger::count(pool).await.unwrap_or(0);
    let rows = entries
        .into_iter()
        .map(|e| {
            let path = e.abs_path.unwrap_or_default();
            TransferLedgerRow {
                id: e.id,
                direction: e.direction,
                // A row whose file has been moved or deleted is greyed, not
                // hidden: knowing something arrived and then went is useful.
                missing: !path.is_empty() && !Path::new(&path).exists(),
                kind: kind_of(&e.name),
                kindlabel: kind_label(&e.name),
                name: e.name,
                path,
                size: human_size(e.bytes.max(0) as u64),
                peer: e.peer,
                status: e.status,
                stamp: when(e.at),
                // Filled in by `patch_send_progress`, which is what knows the
                // bytes actually on the wire.
                pct: 0.0,
                progress: String::new(),
            }
        })
        .collect();
    let pages = ((total as usize).div_ceil(tulipix_transfer::ledger::PAGE_SIZE)).max(1) as i64;
    (rows, pages)
}

/// Fill in the progress of every ledger row that is still sending, and blank it
/// on every row that is not.
///
/// Patched onto the cached rows rather than refetched: the table reloads only
/// when the ledger's revision changes, which a download in progress does not
/// touch, so without this a 4 GB download claims to be done the moment its row
/// is written on the first chunk.
fn patch_send_progress(
    rows: &mut [TransferLedgerRow],
    snap: &tulipix_transfer::Snapshot,
    rates: &HashMap<String, u64>,
) {
    for row in rows.iter_mut() {
        if row.status != "sending" {
            row.pct = 0.0;
            row.progress.clear();
            continue;
        }
        match snap.sends.iter().find(|s| s.row == row.id) {
            Some(s) if s.total > 0 => {
                let pct = (s.done as f64 / s.total as f64).min(1.0);
                let rate = rates.get(&format!("tx:{}", s.row)).copied().unwrap_or(0);
                let label = format!("{}%", (pct * 100.0).round() as i64);
                row.progress =
                    if rate > 0 { format!("{label} · {}/s", human_size(rate)) } else { label };
                row.pct = pct;
            }
            // In the ledger as sending, but not on the wire: the settling
            // update is one tick behind, or this is a row from an earlier run.
            _ => {
                row.pct = 0.0;
                row.progress.clear();
            }
        }
    }
}

/// One file's byte counter, as it was the last time it was looked at.
#[frb(ignore)]
struct Sample {
    done: u64,
    at: std::time::Instant,
    rate: f64,
}

fn rate_table() -> &'static Mutex<HashMap<String, Sample>> {
    static R: std::sync::OnceLock<Mutex<HashMap<String, Sample>>> = std::sync::OnceLock::new();
    R.get_or_init(Mutex::default)
}

/// Bytes per second for every file moving right now, keyed `up:<id>` for an
/// upload and `tx:<ledger row>` for a download.
///
/// Smoothed, because a 600 ms sample that happens to land between two 64 KB
/// chunks reads as a stall, and a rate that flickers to zero on a healthy
/// transfer is worse than no rate at all. Entries for files that have stopped
/// moving are dropped here rather than swept elsewhere — the caller passes the
/// whole live set on every tick, so anything absent is finished.
fn sample_rates(live: &[(String, u64)]) -> HashMap<String, u64> {
    let now = std::time::Instant::now();
    let mut table = rate_table().lock().unwrap_or_else(|e| e.into_inner());
    let mut out = HashMap::with_capacity(live.len());
    for (key, done) in live {
        let seen =
            table.entry(key.clone()).or_insert(Sample { done: *done, at: now, rate: 0.0 });
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

/// The pairing URL as a module matrix.
///
/// `fast_qr::Module` is a `u8` packing the module's type in the high bits and
/// its dark/light bit in bit 0; `value()` reads that bit. The one line here
/// that depends on that representation.
fn render_qr(url: &str) -> Option<QrCode> {
    if url.is_empty() {
        return None;
    }
    let qr = fast_qr::QRBuilder::new(url).build().ok()?;
    Some(QrCode {
        size: qr.size as i64,
        modules: qr.data[..qr.size * qr.size].iter().map(|m| m.value()).collect(),
    })
}

// ------------------------------------------------------------ formatters ----

/// What to call a device: the name typed on the desktop, the device type when
/// there is none, and — for one paired before either was recorded — whatever
/// its User-Agent said.
fn display_name(d: &tulipix_transfer::DeviceRow) -> String {
    if !d.name.is_empty() {
        d.name.clone()
    } else if !d.kind.is_empty() {
        d.kind.clone()
    } else {
        d.label.clone()
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

/// Wrong PIN guesses.
///
/// The lockout is invisible from the desktop otherwise: the guessing phone gets
/// a 429 and this end shows nothing, so a person grinding the PIN on your
/// network would go unnoticed until they gave up.
fn attempts_line(snap: &tulipix_transfer::Snapshot) -> String {
    if snap.attempts.is_empty() {
        return String::new();
    }
    let locked = tulipix_transfer::auth::MAX_ATTEMPTS;
    snap.attempts
        .iter()
        .map(|(ip, n)| {
            if *n >= locked {
                format!("{ip} — locked out after {n} wrong PINs")
            } else {
                format!("{ip} — {n} wrong of {locked}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The one fix worth trying first, for *this* machine, with its real port and
/// its real interface in it.
///
/// The Help panel carries static advice for every platform; this is the block
/// that cannot be static, because a firewall rule naming the wrong port or the
/// wrong link is worse than no rule at all. Only meaningful while sharing:
/// before the first bind there is no interface chosen and no port to name.
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

fn plural(n: usize, word: &str) -> String {
    if n == 1 { format!("1 {word}") } else { format!("{n} {word}s") }
}

/// How long a pairing has left. Coarse on purpose: the number that matters is
/// "today" or "tomorrow", and a ticking countdown would redraw the list every
/// second to say nothing new.
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
        plural(secs.div_ceil(60) as usize, "min")
    }
}

fn kind_of(name: &str) -> String {
    Path::new(name)
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
    let ext = Path::new(name)
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

/// "just now" / "12 min ago" / "3 h ago", then the date once it stops mattering.
fn when(at: i64) -> String {
    let delta = tulipix_core::util::unix_secs_i64() - at;
    match delta {
        d if d < 0 => fmt_date(at),
        d if d < 60 => "just now".into(),
        d if d < 3600 => format!("{} min ago", d / 60),
        d if d < 86_400 => format!("{} h ago", d / 3600),
        _ => fmt_date(at),
    }
}

/// "12 Jun 2022". `tulipix_common::fmt_date` is these four lines plus a
/// dependency on slint, which nothing the bridge links may have.
fn fmt_date(epoch: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(epoch, 0)
        .map(|d| d.format("%-d %b %Y").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_moments_read_relatively_and_old_ones_by_date() {
        let now = tulipix_core::util::unix_secs_i64();
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

    #[test]
    fn a_sending_row_takes_its_progress_from_the_wire_and_a_settled_one_loses_it() {
        // The ledger row is written on the first chunk, so without the patch a
        // 4 GB download reads as complete the moment it starts.
        let mut rows = vec![
            TransferLedgerRow {
                id: 7,
                direction: "out".into(),
                name: "film.mkv".into(),
                path: String::new(),
                size: "4 GB".into(),
                peer: "Pixel".into(),
                status: "sending".into(),
                stamp: "just now".into(),
                kind: "MKV".into(),
                kindlabel: "Video".into(),
                missing: false,
                pct: 0.0,
                progress: String::new(),
            },
            TransferLedgerRow {
                id: 8,
                status: "ok".into(),
                pct: 0.9,
                progress: "90%".into(),
                ..Default::default()
            },
        ];
        let mut snap = tulipix_transfer::Snapshot::default();
        snap.sends = vec![tulipix_transfer::SendRow { row: 7, done: 25, total: 100 }];
        let rates = HashMap::from([("tx:7".to_string(), 2_000_000u64)]);
        patch_send_progress(&mut rows, &snap, &rates);
        assert_eq!(rows[0].pct, 0.25);
        assert!(rows[0].progress.starts_with("25% · "));
        // A row that has settled must lose the bar it had, or it keeps a stale
        // one until the next ledger reload.
        assert_eq!(rows[1].pct, 0.0);
        assert!(rows[1].progress.is_empty());
    }

    #[test]
    fn a_rate_needs_two_samples_a_quarter_second_apart() {
        // The first sighting of a file has nothing to subtract from, so it must
        // report no rate rather than an enormous one.
        let key = "up:9001".to_string();
        let first = sample_rates(&[(key.clone(), 0)]);
        assert_eq!(first.get(&key).copied(), Some(0));
        // And a file that has stopped moving is dropped rather than lingering
        // in the table at its last speed.
        let gone = sample_rates(&[]);
        assert!(gone.is_empty());
    }
}
