//! Local file transfer over Wi-Fi or a hotspot: the desktop serves a small web
//! page, a phone browser downloads what is shared and uploads into the inbox.
//!
//! No Slint types cross this boundary, so everything here is testable without a
//! renderer — the whole crate runs in CI's light `test` job. The UI layer sees
//! exactly one type, [`TransferService`], and one read model, [`Snapshot`].

pub mod auth;
pub mod discover;
pub mod inbox;
pub mod ledger;
pub mod mdns;
pub mod names;
pub mod net;
pub mod peer;
pub mod range;
pub mod server;
pub mod share;
pub mod tls;

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
    /// The device token this file is offered to, empty for every paired device.
    pub to: String,
}

/// One paired phone, as the Connection card's round buttons want it.
pub struct DeviceRow {
    pub token: String,
    /// "Android · Chrome" — the detail line inside the popup.
    pub label: String,
    /// The device type: the caption when nothing has been typed, and the icon.
    pub kind: String,
    /// The name typed on the desktop, empty when there is none.
    pub name: String,
    pub ip: String,
    /// The PIN this device paired with, which is not necessarily today's.
    pub pin: String,
    pub last_seen: u64,
    /// Seconds until it has to pair again.
    pub remaining: u64,
    /// Bytes are moving to or from this device right now.
    pub busy: bool,
}

/// One other tulipix on the network, as the Connection card's grid wants it.
///
/// `paired` is the whole point: a found peer is drawn differently from a
/// trusted one, and clicking an unpaired row starts pairing rather than a
/// transfer.
pub struct PeerRow {
    pub host: String,
    pub ip: String,
    pub port: u16,
    pub paired: bool,
}

/// Mark each discovered peer against the addresses we hold tokens for.
fn peer_rows(found: &[discover::Found], paired_ips: &[String]) -> Vec<PeerRow> {
    found
        .iter()
        .map(|f| {
            let ip = f.ip.to_string();
            PeerRow {
                paired: paired_ips.iter().any(|p| p == &ip),
                host: f.host.clone(),
                port: f.port,
                ip,
            }
        })
        .collect()
}

/// What the Status dashboard reads about the transfer service — see
/// [`TransferService::status_snapshot`]. Separate from [`Snapshot`], which is
/// the much larger thing the Transfer *page* draws: the dashboard wants a
/// handful of numbers and no allocations per file.
#[derive(Default, Clone, Debug)]
pub struct StatusSnapshot {
    pub running: bool,
    pub port: u16,
    /// The interface the phone URL is built from. Empty when not sharing.
    pub address: String,
    pub devices: i64,
    pub inflight: i64,
    /// Cumulative bytes on the wire this run, out and in.
    pub sent_bytes: u64,
    pub recv_bytes: u64,
}

/// One download on the wire, matched to its Recent Transfers row by `row`.
///
/// A send has no pane of its own: the phone pulls the file, so the row in the
/// table is the only place it appears, and until this existed that row said
/// "Completed" from the first chunk.
pub struct SendRow {
    /// `transfers.id` of the ledger row this is filling in.
    pub row: i64,
    pub done: u64,
    pub total: u64,
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
    /// `https://{ip}:{port}/` — what a person types into a phone. Plain `http`
    /// only when no certificate could be made; [`Snapshot::secure`] says which.
    pub url: String,
    /// The nicer form of the same thing, over mDNS. Offered alongside the
    /// address rather than instead of it: plenty of phones cannot resolve a
    /// `.local` name, and the address always works.
    pub host_url: String,
    /// Where a phone lands the first time. Plain HTTP by design — the page that
    /// explains a certificate warning cannot sit behind that warning.
    pub trust_url: String,
    /// SHA-256 of the certificate this run serves, colon-separated hex. What a
    /// phone is accepting on first use, so it can be read off the desktop and
    /// compared instead of waved through. Empty on plain HTTP.
    pub fingerprint: String,
    /// Whether the server is actually on TLS.
    pub secure: bool,
    /// The interfaces a phone could reach, best first: `(name, ip)`.
    pub interfaces: Vec<(String, String)>,
    /// Which of them the URL is built from.
    pub iface: String,
    pub files: Vec<FileRow>,
    /// Which device the next file added will be for. Empty is everyone.
    pub share_target: String,
    pub devices: Vec<DeviceRow>,
    /// Other tulipix machines seen on the network, paired or not.
    pub peers: Vec<PeerRow>,
    /// `(address, wrong guesses)`, worst first. Empty when nobody has missed.
    pub attempts: Vec<(String, u32)>,
    pub uploads: Vec<UploadRow>,
    /// Downloads moving right now, for the table rows that are still filling.
    pub sends: Vec<SendRow>,
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

    /// Everything the Status dashboard reports about this service, read at one
    /// instant.
    ///
    /// One method rather than a getter per field: every caller wants the whole
    /// set together, and taking the auth lock once keeps the answer internally
    /// consistent — a device count from before a pairing and an in-flight count
    /// from after it would describe a state that never existed.
    ///
    /// The byte counters are cumulative for this run. Handing over a rate would
    /// mean owning a clock and a sample interval down here, when the only thing
    /// that knows how often it looks is the caller.
    pub fn status_snapshot(&self, now: u64) -> StatusSnapshot {
        let Some(run) = &self.running else {
            return StatusSnapshot::default();
        };
        let devices = run
            .state
            .auth
            .lock()
            .map(|a| a.device_count(now) as i64)
            .unwrap_or(0);
        // Both directions. A phone pulling a film is as much "in flight" as one
        // pushing a photo, and counting only uploads left the dashboard saying
        // nothing was running while a download filled the link.
        let uploading = run
            .state
            .uploads
            .lock()
            .map(|u| u.iter().filter(|x| x.state == "active").count() as i64)
            .unwrap_or(0);
        let sending = run.state.sends.lock().map(|s| s.len() as i64).unwrap_or(0);
        let inflight = uploading + sending;
        use std::sync::atomic::Ordering::Relaxed;
        StatusSnapshot {
            running: true,
            port: run.port,
            address: self.address(),
            devices,
            inflight,
            sent_bytes: run.state.sent_bytes.load(Relaxed),
            recv_bytes: run.state.recv_bytes.load(Relaxed),
        }
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
            tls: true,
        })
        .await;
        if started.is_err() {
            tracing::info!(port = PORT, "transfer: preferred port unavailable, asking the OS");
            started = server::start(server::Config {
                inbox: self.inbox.clone(),
                bind: format!("{addr}:0"),
                pool,
                tls: true,
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
        // The address, not `tulipix.local`: a QR that resolves on some phones
        // and not others is worse than four numbers that resolve on all of them,
        // and the certificate covers both names either way.
        format!("{}/?k={key}", self.base_url())
    }

    /// The address to hand a phone, with no trailing slash.
    ///
    /// Plain `http` even when the server is on TLS, and deliberately. A browser
    /// shows no interstitial for http, so this is the one entry point that works
    /// on a phone which has never seen our certificate: it lands on the gateway,
    /// which checks whether the phone can reach the TLS side and moves it across
    /// itself. Pointing a first-time phone at `https` directly is what put the
    /// browser's warning between it and the page explaining the warning.
    fn base_url(&self) -> String {
        let Some(run) = &self.running else { return String::new() };
        format!("http://{}:{}", self.address(), run.port)
    }

    /// Whether the key inside the QR currently on screen is still good. False
    /// means the code has to be re-minted and redrawn — see
    /// [`auth::Auth::pairing_live`] for why a code drawn once per address is not
    /// enough.
    pub fn pairing_live(&self) -> bool {
        let Some(run) = &self.running else { return false };
        lock(&run.state.auth).pairing_live(server::now_secs())
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

    /// Name a paired device. The name is clamped to fifteen characters by
    /// [`auth::clamp_name`] and persisted, so it survives a restart — it is the
    /// only thing on that list the user wrote.
    pub async fn rename_device(&self, token: &str, name: &str) {
        let Some(st) = self.state() else { return };
        let Some(stored) = lock(&st.auth).rename(token, name) else { return };
        if let Some(pool) = &st.pool {
            let _ = ledger::rename_device(pool, token, &stored).await;
        }
    }

    pub async fn forget_device(&self, token: &str) {
        let Some(st) = self.state() else { return };
        lock(&st.auth).forget(token);
        // Anything in the tray addressed only to this device is now addressed to
        // nobody. Widen it rather than leave it sitting there visible to no one,
        // which looks like a send that quietly failed.
        lock(&st.tray).release(token);
        if let Some(pool) = &st.pool {
            let _ = ledger::forget_device(pool, token).await;
        }
    }

    /// Point the share tray at one paired device, or at everyone with `None`.
    /// Files already in the tray keep whoever they were added for.
    pub fn set_share_target(&self, token: Option<String>) {
        let Some(st) = self.state() else { return };
        lock(&st.tray).set_target(token);
    }

    /// The pool behind `transfers.db`, for the Recent Transfers list. `None`
    /// until the server has started at least once.
    pub fn pool(&self) -> Option<sqlx::SqlitePool> {
        self.state().and_then(|st| st.pool.clone())
    }

    /// Other tulipix machines on this network. Empty is normal — see
    /// `discover.rs` on why mDNS is a convenience and never the path.
    pub fn peers(&self) -> Vec<PeerRow> {
        let Some(run) = &self.running else { return Vec::new() };
        let Some(browser) = run.browser.as_ref() else { return Vec::new() };
        peer_rows(&browser.peers(), &self.paired_ips())
    }

    /// Addresses we already hold a device token for. Empty today: pairing a
    /// discovered peer (rather than typing an address and a PIN) is a later
    /// task, and no token is issued with `kind == "tulipix"` yet — so every
    /// discovered peer currently draws as unpaired, which is correct for now
    /// rather than a gap.
    fn paired_ips(&self) -> Vec<String> {
        let Some(run) = &self.running else { return Vec::new() };
        lock(&run.state.auth)
            .devices(server::now_secs())
            .into_iter()
            .filter(|t| t.kind == "tulipix")
            .map(|t| t.ip)
            .collect()
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
        let (files, total_bytes, share_target) = {
            let tray = lock(&run.state.tray);
            (
                tray.items()
                    .iter()
                    .map(|i| FileRow {
                        id: i.id,
                        name: i.name.clone(),
                        bytes: i.bytes,
                        to: i.to.clone().unwrap_or_default(),
                    })
                    .collect(),
                tray.total_bytes(),
                tray.target().unwrap_or_default().to_string(),
            )
        };
        let (devices, attempts) = {
            let auth = lock(&run.state.auth);
            let devices: Vec<DeviceRow> = auth
                .devices(now)
                .into_iter()
                .map(|t| DeviceRow {
                    remaining: t.remaining(now),
                    busy: t.busy(now),
                    last_seen: t.last_seen,
                    token: t.value,
                    label: t.label,
                    kind: t.kind,
                    name: t.name,
                    ip: t.ip,
                    pin: t.pin,
                })
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

        let sends = lock(&run.state.sends)
            .iter()
            .map(|s| SendRow { row: s.row, done: s.done, total: s.total })
            .collect();

        Snapshot {
            running: true,
            port: run.port,
            pin: run.pin.clone(),
            url: self.base_url(),
            host_url: format!("http://{}:{}", tls::HOST, run.port),
            trust_url: format!("http://{}:{}/trust", self.address(), run.port),
            fingerprint: lock(&run.state.fingerprint).clone(),
            secure: run.secure,
            iface: self.address(),
            interfaces,
            files,
            share_target,
            devices,
            peers: self.peers(),
            attempts,
            uploads,
            sends,
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

    #[test]
    fn a_paired_peer_is_marked_and_an_unpaired_one_is_not() {
        use std::net::{IpAddr, Ipv4Addr};
        let found = vec![
            crate::discover::Found {
                host: "tulipix.local".into(),
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 31)),
                port: 8420,
            },
            crate::discover::Found {
                host: "tulipix.local".into(),
                ip: IpAddr::V4(Ipv4Addr::new(192, 168, 1, 99)),
                port: 8420,
            },
        ];
        let paired_ips = ["192.168.1.31".to_string()];
        let rows = peer_rows(&found, &paired_ips);

        assert_eq!(rows.len(), 2, "both are listed");
        assert!(rows[0].paired, "the one we have a token for is paired");
        assert!(!rows[1].paired, "the other is only found");
    }
}
