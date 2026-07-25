//! Local file transfer over Wi-Fi or a hotspot: the desktop serves a small web
//! page, a phone browser downloads what is shared and uploads into the inbox.
//!
//! No Slint types cross this boundary, so everything here is testable without a
//! renderer — the whole crate runs in CI's light `test` job. The UI layer sees
//! exactly one type, [`TransferService`], and one read model, [`Snapshot`].

pub mod auth;
pub mod inbox;
pub mod ledger;
pub mod names;
pub mod net;
pub mod range;
pub mod server;
pub mod share;

use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};

use server::{Running, Shared};

/// The port to ask for first. Arbitrary, in the IANA dynamic range, and — the
/// only thing that matters — the same every launch, so `ufw allow 8420/tcp` or
/// a Windows firewall rule stays valid. Falls back to an OS-assigned port when
/// something else already holds it.
pub const PORT: u16 = 8420;

/// One tray row, as the UI wants it.
pub struct FileRow {
    pub id: u64,
    pub name: String,
    pub bytes: u64,
}

/// One paired phone.
pub struct DeviceRow {
    pub token: String,
    pub label: String,
    pub last_seen: u64,
}

/// One upload in flight (or just finished).
pub struct UploadRow {
    /// Server-side upload id, so a failed row can name itself to `dismiss_upload`.
    pub id: u64,
    pub name: String,
    pub total: u64,
    pub done: u64,
    pub state: String,
}

/// Everything the Transfer page draws, read in one go so the UI never holds a
/// lock of its own.
#[derive(Default)]
pub struct Snapshot {
    pub running: bool,
    pub port: u16,
    pub pin: String,
    /// `http://{ip}:{port}/` — what a person types into a phone.
    pub url: String,
    /// The interfaces a phone could reach, best first: `(name, ip)`.
    pub interfaces: Vec<(String, String)>,
    /// Which of them the URL is built from.
    pub iface: String,
    pub files: Vec<FileRow>,
    pub devices: Vec<DeviceRow>,
    /// `(address, wrong guesses)`, worst first. Empty when nobody has missed.
    pub attempts: Vec<(String, u32)>,
    pub uploads: Vec<UploadRow>,
    pub total_bytes: u64,
    /// Bumped on every ledger write; the UI refetches Recent Transfers when it
    /// changes rather than polling the database.
    pub rev: u64,
    pub inbox: PathBuf,
    pub inbox_ok: bool,
    /// Whatever went wrong, verbatim. Empty when nothing did.
    pub error: String,
}

/// Start, stop and read the transfer server. One per app; the UI layer holds it
/// behind a mutex and calls into it from Slint callbacks.
#[derive(Default)]
pub struct TransferService {
    running: Option<Running>,
    inbox: PathBuf,
    /// The interface the URL is built from. Empty means "the first usable one".
    iface: String,
    error: String,
    /// Both of these are read on every UI tick, and both cost a syscall to
    /// compute — a write probe and a `getifaddrs`. Cached at the points that can
    /// actually change them (start, and the two setters) rather than measured
    /// several times a second.
    inbox_ok: bool,
    ifaces: Vec<(String, Ipv4Addr)>,
}

impl TransferService {
    pub fn new(inbox: PathBuf) -> Self {
        let inbox_ok = inbox::writable(&inbox).is_ok();
        Self {
            running: None,
            inbox,
            iface: String::new(),
            error: String::new(),
            inbox_ok,
            ifaces: net::interfaces(),
        }
    }

    /// Re-read the interface list. Called when sharing starts; a network that
    /// changes underneath a running server is a stop-and-start, not a surprise.
    pub fn rescan(&mut self) {
        self.ifaces = net::interfaces();
        self.inbox_ok = inbox::writable(&self.inbox).is_ok();
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    pub fn inbox(&self) -> &Path {
        &self.inbox
    }

    pub fn set_inbox(&mut self, path: PathBuf) {
        self.inbox = path.clone();
        self.inbox_ok = inbox::writable(&self.inbox).is_ok();
        if let Some(run) = &self.running {
            *lock(&run.state.inbox) = path;
        }
    }

    /// Pick which interface the URL names. A hotspot and a Wi-Fi link commonly
    /// both qualify, and only the person holding the phone knows which one it
    /// is on.
    pub fn set_iface(&mut self, ip: String) {
        self.iface = ip;
    }

    /// Bind and start serving. Idempotent: starting an already-running service
    /// is a no-op rather than a second listener.
    pub async fn start(&mut self) -> bool {
        if self.running.is_some() {
            return true;
        }
        self.error.clear();
        self.rescan();

        if self.ifaces.is_empty() {
            self.error =
                "No Wi-Fi or hotspot connection. Join a network, or turn on a hotspot.".into();
            return false;
        }
        if let Err(e) = inbox::writable(&self.inbox) {
            // Uploads are refused, but downloads still work, so this is a
            // warning on the page rather than a refusal to start.
            tracing::warn!(error = %e, inbox = %self.inbox.display(), "transfer: inbox unwritable");
        }

        // A missing transfers.db is not fatal: the section still transfers
        // files, it just does not remember having done so.
        let pool = match ledger::open().await {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(error = %e, "transfer: transfers.db unavailable");
                None
            }
        };

        // Bound to one private address, not `0.0.0.0`. Listening on every
        // interface would open the port on a public one too when the machine
        // has one, no matter which address the QR shows — the socket does not
        // care what is on screen. Binding the chosen address makes local-only
        // structural: there is nothing for a routable interface to accept.
        let addr = self.address();

        // The preferred port first, an OS-assigned one only if it is taken. A
        // stable port is what makes a firewall rule worth writing: `ufw allow
        // 8420/tcp` holds across launches, whereas a rule for a port that is
        // different every time is not a rule at all. Costs one failed bind in
        // the rare case something else holds it.
        let mut started = server::start(server::Config {
            inbox: self.inbox.clone(),
            bind: format!("{addr}:{PORT}"),
            pool: pool.clone(),
        })
        .await;
        if started.is_err() {
            tracing::info!(port = PORT, "transfer: preferred port unavailable, asking the OS");
            started = server::start(server::Config {
                inbox: self.inbox.clone(),
                bind: format!("{addr}:0"),
                pool,
            })
            .await;
        }

        match started {
            Ok(run) => {
                let port = run.port;
                self.running = Some(run);
                self.check_reachable(port).await;
                true
            }
            Err(e) => {
                // Verbatim, and no silent retry beyond the port fallback above:
                // on Windows this is the firewall prompt having been dismissed,
                // and guessing hides that.
                self.error = format!("Could not start sharing: {e}");
                false
            }
        }
    }

    /// Connect to our own LAN address once, after binding.
    ///
    /// On macOS 14+ the Local Network permission prompt appears at first bind,
    /// and denying it leaves the server bound but unreachable — which from the
    /// phone is indistinguishable from a firewall problem or a typo in the URL.
    /// A connection to the address we are about to put on screen is the
    /// cheapest thing that tells those apart. Elsewhere this is a no-op: there
    /// is no equivalent silent-deny to detect.
    #[cfg(target_os = "macos")]
    async fn check_reachable(&mut self, port: u16) {
        let addr = format!("{}:{}", self.address(), port);
        let probe = tokio::time::timeout(
            std::time::Duration::from_secs(2),
            tokio::net::TcpStream::connect(&addr),
        )
        .await;
        if !matches!(probe, Ok(Ok(_))) {
            self.error = "macOS is blocking the local network. Allow Tulipix under System \
                          Settings → Privacy & Security → Local Network, then start again."
                .into();
        }
    }

    #[cfg(not(target_os = "macos"))]
    async fn check_reachable(&mut self, _port: u16) {}

    pub async fn stop(&mut self) {
        if let Some(run) = self.running.take() {
            run.stop().await;
        }
    }

    /// Close the port synchronously, for the app-exit path where awaiting is no
    /// longer possible.
    pub fn abort(&mut self) {
        if let Some(run) = self.running.take() {
            run.abort();
        }
    }

    fn state(&self) -> Option<&Shared> {
        self.running.as_ref().map(|r| &r.state)
    }

    pub fn add(&self, path: &Path) {
        let Some(st) = self.state() else { return };
        lock(&st.tray).add(path);
    }

    pub fn remove(&self, id: u64) {
        let Some(st) = self.state() else { return };
        lock(&st.tray).remove(id);
    }

    pub fn clear(&self) {
        let Some(st) = self.state() else { return };
        lock(&st.tray).clear();
    }

    /// Drop a finished upload from the Receive pane by hand.
    ///
    /// Successes clear themselves after [`server::DONE_LINGER`]; this is what
    /// clears a failure, which otherwise stays put on purpose. An upload still
    /// in flight is left alone — the row is the only sign it is happening.
    pub fn dismiss_upload(&self, id: u64) {
        let Some(st) = self.state() else { return };
        lock(&st.uploads).retain(|u| u.id != id || u.state == "active");
    }

    /// The URL behind the QR: `http://{ip}:{port}/?k={key}`, where the key is
    /// single-use and lives 60 seconds. Presenting it is equivalent to entering
    /// the PIN, so the PIN itself is never in the code — a photo of the screen
    /// taken later is worthless.
    pub fn pairing_url(&self) -> String {
        let Some(run) = &self.running else { return String::new() };
        let key = lock(&run.state.auth).new_pairing_key(server::now_secs());
        format!("http://{}:{}/?k={key}", self.address(), run.port)
    }

    /// The chosen interface's address, or the best one available.
    fn address(&self) -> String {
        if !self.iface.is_empty() && self.ifaces.iter().any(|(_, ip)| ip.to_string() == self.iface)
        {
            return self.iface.clone();
        }
        self.ifaces
            .first()
            .map(|(_, ip)| ip.to_string())
            .unwrap_or_else(|| Ipv4Addr::LOCALHOST.to_string())
    }

    pub async fn forget_device(&self, token: &str) {
        let Some(st) = self.state() else { return };
        lock(&st.auth).forget(token);
        if let Some(pool) = &st.pool {
            let _ = ledger::forget_device(pool, token).await;
        }
    }

    /// The pool behind `transfers.db`, for the Recent Transfers list. `None`
    /// until the server has started at least once.
    pub fn pool(&self) -> Option<sqlx::SqlitePool> {
        self.state().and_then(|st| st.pool.clone())
    }

    pub fn snapshot(&self) -> Snapshot {
        let interfaces: Vec<(String, String)> =
            self.ifaces.iter().map(|(n, ip)| (n.clone(), ip.to_string())).collect();
        let inbox_ok = self.inbox_ok;

        let Some(run) = &self.running else {
            return Snapshot {
                running: false,
                interfaces,
                inbox: self.inbox.clone(),
                inbox_ok,
                error: self.error.clone(),
                ..Snapshot::default()
            };
        };

        let now = server::now_secs();
        let (files, total_bytes) = {
            let tray = lock(&run.state.tray);
            (
                tray.items()
                    .iter()
                    .map(|i| FileRow { id: i.id, name: i.name.clone(), bytes: i.bytes })
                    .collect(),
                tray.total_bytes(),
            )
        };
        let (devices, attempts) = {
            let auth = lock(&run.state.auth);
            let devices: Vec<DeviceRow> = auth
                .devices(now)
                .into_iter()
                .map(|(token, label, last_seen)| DeviceRow { token, label, last_seen })
                .collect();
            (devices, auth.wrong_attempts())
        };
        // A finished upload clears itself after a few seconds; a failed one
        // stays until it is dismissed. Filtering here rather than sweeping on a
        // timer keeps the desktop and the phone reading the same list.
        let uploads = lock(&run.state.uploads)
            .iter()
            .filter(|u| u.visible())
            .map(|u| UploadRow {
                id: u.id,
                name: u.name.clone(),
                total: u.total,
                done: u.done,
                state: u.state.to_string(),
            })
            .collect();

        Snapshot {
            running: true,
            port: run.port,
            pin: run.pin.clone(),
            url: format!("http://{}:{}", self.address(), run.port),
            iface: self.address(),
            interfaces,
            files,
            devices,
            attempts,
            uploads,
            total_bytes,
            rev: run.state.rev.load(std::sync::atomic::Ordering::Relaxed),
            inbox: self.inbox.clone(),
            inbox_ok,
            error: self.error.clone(),
        }
    }
}

fn lock<T>(m: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Where uploads land when nothing has been chosen in Settings: a `Tulipix
/// Inbox` folder under the OS documents directory.
pub fn default_inbox() -> PathBuf {
    let docs = std::env::var_os("XDG_DOCUMENTS_DIR")
        .map(PathBuf::from)
        .or_else(|| dirs_documents())
        .unwrap_or_else(|| PathBuf::from("."));
    docs.join("Tulipix Inbox")
}

/// The OS documents directory, without adding a `dirs` dependency for one path.
fn dirs_documents() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var_os("USERPROFILE").map(|h| PathBuf::from(h).join("Documents"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Documents"))
    }
}

/// Bytes as a person reads them. Kept here rather than borrowed from
/// `tulipix-common`, which pulls Slint and would drag a renderer into this
/// crate's test job.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{} {}", value.round() as u64, UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_the_way_a_person_would_say_them() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1024), "1.0 KB");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(20 * 1024), "20 KB");
        assert_eq!(human_size(4 * 1024 * 1024 * 1024), "4.0 GB");
    }

    #[test]
    fn a_stopped_service_reports_stopped_rather_than_failing() {
        let svc = TransferService::new(std::env::temp_dir().join("tulipix-transfer-test"));
        let snap = svc.snapshot();
        assert!(!snap.running);
        assert!(snap.pin.is_empty());
        assert!(snap.url.is_empty());
        // Adding to the tray before starting is a no-op, not a panic.
        svc.add(Path::new("/nonexistent"));
        assert!(svc.snapshot().files.is_empty());
    }

    #[test]
    fn the_default_inbox_is_a_named_folder_not_the_documents_root() {
        assert!(default_inbox().ends_with("Tulipix Inbox"));
    }
}
