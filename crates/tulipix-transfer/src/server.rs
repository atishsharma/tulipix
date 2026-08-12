//! The axum router, the bind, and the handlers behind it.
//!
//! Nothing here may panic: the release profile is `panic = "abort"`, so a panic
//! in a request handler takes down the whole app rather than the request. Every
//! fallible step is a `let … else` or a `match`, and every mutex is taken
//! through [`lock`], which recovers a poisoned guard instead of unwrapping it.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use axum::{
    body::Body,
    extract::{ConnectInfo, Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Router,
};
use sqlx::SqlitePool;

use crate::auth::{kind_for, label_for, Auth, PinResult, Token, MAX_DEVICES, TOKEN_TTL};
use crate::share::Tray;
use crate::{inbox, ledger, range};

/// The session cookie. HttpOnly, so the page never handles the token itself.
const COOKIE: &str = "tx";
/// Read size for downloads. Big enough that a 4 GB file is not a syscall storm,
/// small enough that a cancelled download stops promptly.
const CHUNK: usize = 64 * 1024;

pub use tulipix_core::util::unix_secs as now_secs;

/// A poisoned mutex means some other handler panicked; the data behind it is
/// still the data. Recovering beats taking the process down with it.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// How long a finished upload stays on screen before it clears itself.
///
/// Only successes expire. A failure is the one thing worth reading late, so it
/// stays until it is dismissed by hand — the Recent Transfers row below is the
/// permanent record either way.
pub const DONE_LINGER: std::time::Duration = std::time::Duration::from_secs(12);

/// One upload in flight, for the Receive pane and `/api/status`.
#[derive(Clone, Debug)]
pub struct Upload {
    pub id: u64,
    pub name: String,
    pub total: u64,
    pub done: u64,
    /// "active" | "done" | "failed"
    pub state: &'static str,
    pub peer: String,
    /// When it stopped moving. `None` while still active.
    pub finished: Option<std::time::Instant>,
}

impl Upload {
    /// Whether the Receive pane and the phone should still be drawing this.
    ///
    /// Time-based rather than a sweep on a timer: there is no tick inside the
    /// server, and filtering at read time means the desktop and the phone
    /// agree without either of them mutating shared state to do it.
    pub fn visible(&self) -> bool {
        match (self.state, self.finished) {
            ("done", Some(at)) => at.elapsed() < DONE_LINGER,
            _ => true,
        }
    }
}

/// One download on the wire, keyed by the ledger row it wrote on its first
/// chunk. Enough for the Recent Transfers row to show how far along it is; the
/// rate is derived by the UI from two samples, the same way the totals are.
#[derive(Clone, Debug)]
pub struct Outgoing {
    /// `transfers.id`. The row on screen and the bytes moving are matched by it.
    pub row: i64,
    pub name: String,
    pub done: u64,
    pub total: u64,
}

pub struct AppState {
    pub auth: Mutex<Auth>,
    pub tray: Mutex<Tray>,
    /// Behind a mutex so changing it in Settings takes effect without a restart.
    pub inbox: Mutex<PathBuf>,
    pub pool: Option<SqlitePool>,
    pub uploads: Mutex<Vec<Upload>>,
    /// Downloads in flight. Emptied as each finishes — unlike `uploads`, which
    /// lingers, because the Recent Transfers row *is* the record of a send.
    pub sends: Mutex<Vec<Outgoing>>,
    next_upload: AtomicU64,
    /// Bumped on every ledger write. The UI refetches Recent Transfers when it
    /// changes rather than polling the database on a timer.
    pub rev: AtomicU64,
    /// Bytes actually put on the wire this run, counted where they are read and
    /// written rather than where a transfer is announced. A cumulative pair, so
    /// a reader that samples twice gets a rate without this side owning a clock
    /// — and a resumed or abandoned download contributes only what it moved.
    pub sent_bytes: AtomicU64,
    pub recv_bytes: AtomicU64,
    /// `(PEM, DER)` of the root, for the phones that opt into installing it.
    /// `None` when the server fell back to plain HTTP, in which case there is
    /// nothing to trust and the download routes 404.
    pub ca: Mutex<Option<(String, Vec<u8>)>>,
    /// SHA-256 of the leaf this run is serving, colon-separated hex. Empty on
    /// plain HTTP. Shown on the desktop and on the gateway so the certificate a
    /// phone accepts on first use is one that can be checked.
    pub fingerprint: Mutex<String>,
}

pub type Shared = Arc<AppState>;

impl AppState {
    fn bump(&self) {
        self.rev.fetch_add(1, Ordering::Relaxed);
    }

    /// The row id, when there is a ledger to write to. `None` with no database
    /// (tests, and a run where `transfers.db` would not open) — which is also
    /// why nothing downstream may treat a missing id as an error.
    async fn record(&self, row: ledger::Row) -> Option<i64> {
        let pool = self.pool.as_ref()?;
        let id = match ledger::record(pool, row, now_secs() as i64).await {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(error = %e, "transfer: ledger write failed");
                None
            }
        };
        self.bump();
        id
    }

    /// Settle a send: `ok` when the last chunk went out, `failed` when the
    /// stream ended early. Idempotent — the drop guard and the normal end of
    /// the stream both arrive for a download that completed.
    async fn settle(&self, row: i64, status: &'static str) {
        {
            let mut sends = lock(&self.sends);
            let Some(at) = sends.iter().position(|s| s.row == row) else { return };
            sends.remove(at);
        }
        if let Some(pool) = &self.pool {
            if let Err(e) = ledger::finish(pool, row, status).await {
                tracing::warn!(error = %e, "transfer: ledger update failed");
            }
        }
        self.bump();
    }
}

pub struct Config {
    pub inbox: PathBuf,
    pub bind: String,
    /// Serve over TLS with the app's own certificate. False only in tests, which
    /// exercise the routes rather than the transport and would otherwise all
    /// need a client that trusts a CA generated moments earlier.
    pub tls: bool,
    /// `None` in tests and when `transfers.db` could not be opened — the server
    /// still transfers files, it just does not remember having done so.
    pub pool: Option<SqlitePool>,
}

pub struct Running {
    pub port: u16,
    pub pin: String,
    pub state: Shared,
    /// False when a certificate could not be produced and the server fell back
    /// to plain HTTP — the URL scheme and the QR both key off this.
    pub secure: bool,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
    /// Kept alive for as long as the server is: dropping the daemon withdraws
    /// the `tulipix.local` record.
    _mdns: Option<crate::mdns::Advert>,
}

impl Running {
    pub async fn stop(self) {
        let _ = self.shutdown.send(());
        let _ = self.handle.await;
    }

    /// Close the port without waiting for in-flight requests to drain. For the
    /// app-exit path, where there is no longer a runtime to await on.
    pub fn abort(self) {
        let _ = self.shutdown.send(());
        self.handle.abort();
    }
}

pub async fn start(cfg: Config) -> anyhow::Result<Running> {
    inbox::sweep_stale(&cfg.inbox);

    let now = now_secs();
    let mut auth = Auth::new(now);
    // A phone that paired before the app restarted keeps its 48 hours.
    if let Some(pool) = &cfg.pool {
        let _ = ledger::forget_expired(pool, now as i64).await;
        // Whatever was mid-flight when this process last ended is not moving
        // now. Left alone it would sit at "sending" for good, and a row that
        // never looks failed is a row the retry button never offers to redo.
        let _ = ledger::settle_stale(pool).await;
        if let Ok(rows) = ledger::devices(pool, now as i64).await {
            for d in rows {
                auth.restore(Token {
                    value: d.token,
                    label: d.label,
                    kind: d.kind,
                    ip: d.ip,
                    name: d.name,
                    pin: d.pin,
                    issued: d.last_seen as u64,
                    expires: d.expires as u64,
                    last_seen: d.last_seen as u64,
                    // Nothing can be in flight before the server is listening.
                    active_until: 0,
                });
            }
        }
    }
    let pin = auth.pin().to_string();

    let state: Shared = Arc::new(AppState {
        auth: Mutex::new(auth),
        tray: Mutex::new(Tray::default()),
        inbox: Mutex::new(cfg.inbox.clone()),
        pool: cfg.pool,
        uploads: Mutex::new(Vec::new()),
        sends: Mutex::new(Vec::new()),
        next_upload: AtomicU64::new(0),
        rev: AtomicU64::new(0),
        sent_bytes: AtomicU64::new(0),
        recv_bytes: AtomicU64::new(0),
        ca: Mutex::new(None),
        fingerprint: Mutex::new(String::new()),
    });

    let app = Router::new()
        .route("/", get(page))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/logo.png", get(logo))
        // Installable-app assets. All static, all unauthenticated: they are the
        // shell, and a phone has to be able to fetch the manifest and the icons
        // before it has any way to prove who it is. Nothing here says anything
        // about what is being shared.
        .route("/manifest.webmanifest", get(manifest))
        .route("/sw.js", get(sw))
        .route("/icon-192.png", get(icon_192))
        .route("/icon-512.png", get(icon_512))
        .route("/icon-maskable.png", get(icon_maskable))
        // Where the OS share sheet posts. The service worker catches this
        // before it reaches the network and hands the files to the page, so a
        // request arriving here means the worker is not running — an
        // uninstalled browser, or one with workers switched off. Answering with
        // the page rather than a 404 at least lands somewhere useful.
        .route("/share", get(page).post(share_fallback))
        .route("/auth", post(auth_post))
        .route("/api/files", get(files))
        .route("/api/status", get(status))
        // What the gateway probes. Unauthenticated on purpose: the answer it is
        // after is "did the TLS handshake succeed", which is settled before any
        // request body exists. 204 so there is nothing to read, and no auth so a
        // phone that has not paired yet still gets a clean answer.
        .route("/api/ping", get(ping))
        .route("/dl/{id}", get(download))
        .route("/upload/{name}", put(upload))
        // The same three the plaintext side answers, so a phone that has already
        // trusted us can still reach the page that explains how to untrust.
        .route("/trust", get(trust))
        .route("/tulipix-ca.crt", get(ca_crt))
        .route("/tulipix-ca.pem", get(ca_pem))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    let port = listener.local_addr()?.port();

    // Every address the leaf has to be valid for. A phone reaching us by IP and
    // one reaching us by name must both get a certificate that matches what
    // they typed, or the padlock is a warning either way.
    let ips: Vec<std::net::IpAddr> = crate::net::interfaces()
        .into_iter()
        .map(|(_, ip)| std::net::IpAddr::V4(ip))
        .chain(std::iter::once(std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST)))
        .collect();

    // A certificate is worth having but not worth refusing to start over: with
    // no data directory to keep a CA in, plain HTTP still transfers files. It
    // only costs the in-page camera scanner, which needs a secure origin.
    let identity = match (cfg.tls, crate::tls::Identity::load(&ips)) {
        (false, _) => None,
        (true, Ok(id)) => Some(id),
        (true, Err(e)) => {
            tracing::warn!(error = %e, "transfer: no certificate, serving plain HTTP");
            None
        }
    };
    let secure = identity.is_some();
    if let Some(id) = &identity {
        *lock(&state.ca) = Some((id.ca_pem.clone(), id.ca_der.clone()));
        *lock(&state.fingerprint) = id.fingerprint.clone();
        tracing::info!(names = ?id.names, fingerprint = %id.fingerprint, "transfer: serving HTTPS");
    }

    let mdns = crate::mdns::advertise(&ips, port, secure);

    let (tx, rx) = tokio::sync::oneshot::channel();
    let svc = app.into_make_service_with_connect_info::<SocketAddr>();
    let handle = tokio::spawn(async move {
        let shutdown = async {
            let _ = rx.await;
        };
        let served = match identity {
            Some(id) => {
                // `.tap_io` with a no-op is not decoration. `ConnectInfo` needs
                // `Connected<IncomingStream<'_, L>>` for SocketAddr, and axum
                // only implements that for its own TcpListener — writing it here
                // for our listener is refused by the orphan rule, since
                // `IncomingStream<TlsListener>` is a foreign type that merely
                // mentions a local one. axum does ship a blanket impl for
                // `TapIo<L, F>`, so wrapping is the supported way to keep peer
                // addresses on a custom listener, and the tap itself is free.
                use axum::serve::ListenerExt as _;
                let tls = crate::tls::TlsListener::new(listener, &id).tap_io(|_| {});
                axum::serve(tls, svc).with_graceful_shutdown(shutdown).await
            }
            None => axum::serve(listener, svc).with_graceful_shutdown(shutdown).await,
        };
        if let Err(e) = served {
            tracing::warn!(error = %e, "transfer: server stopped");
        }
    });

    Ok(Running { port, pin, state, secure, shutdown: tx, handle, _mdns: mdns })
}

// ── responses ───────────────────────────────────────────────────────────────

fn asset(body: &'static str, mime: &'static str) -> Response {
    ([(header::CONTENT_TYPE, mime)], body).into_response()
}

/// The one page that is not the app: a valid pairing key arriving when ten
/// devices are already paired. Its own page rather than the PIN form, because the
/// PIN is not the problem and there is nothing to type.
fn full_page() -> Response {
    let body = format!(
        "<!doctype html><meta name=viewport content=\"width=device-width,initial-scale=1\">\
         <title>Tulipix — device limit</title>\
         <style>body{{font:16px/1.5 system-ui,sans-serif;margin:0;min-height:100vh;display:grid;\
         place-items:center;background:#14110f;color:#f5efe9}}div{{max-width:22rem;padding:1.5rem;\
         text-align:center}}b{{color:#f0a35e}}</style>\
         <div><p><b>{MAX_DEVICES} devices are already paired.</b></p>\
         <p>Open Transfer on the desktop, tap a device in the Connection card and forget it, \
         then scan the code again.</p></div>"
    );
    (
        StatusCode::CONFLICT,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        body,
    )
        .into_response()
}

fn json(body: String) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

/// `Path=/` and `SameSite=Strict` so the cookie is only ever sent back to this
/// server by this page. HttpOnly keeps it out of reach of anything injected.
fn cookie_for(token: &Token) -> String {
    format!(
        "{COOKIE}={}; Path=/; HttpOnly; SameSite=Strict; Max-Age={TOKEN_TTL}",
        token.value
    )
}

fn token_in(headers: &HeaderMap) -> Option<String> {
    let raw = headers.get(header::COOKIE)?.to_str().ok()?;
    raw.split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.to_string())
}

/// The token value behind the request, if it is one we issued and still good.
///
/// The transfer handlers need the value itself, not just a yes: it is how the
/// Connection card knows which of the ten circles to ring while bytes move.
fn caller(st: &AppState, headers: &HeaderMap) -> Option<String> {
    let value = token_in(headers)?;
    lock(&st.auth).check(&value, now_secs()).map(|_| value)
}

fn user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(label_for)
        .unwrap_or_else(|| "Device · Browser".to_string())
}

/// Just the device type, for the caption and the icon on the round button.
fn device_kind(headers: &HeaderMap) -> &'static str {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(kind_for)
        .unwrap_or("Device")
}

fn peer_ip(peer: SocketAddr) -> String {
    peer.ip().to_string()
}

// ── handlers ────────────────────────────────────────────────────────────────

/// The page. A `?k=` carries the QR's single-use pairing key: presenting it is
/// equivalent to entering the PIN, so the cookie is set before the page loads
/// and the phone never sees the PIN form.
async fn page(
    State(st): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(q): Query<std::collections::HashMap<String, String>>,
    headers: HeaderMap,
) -> Response {
    let body = asset(include_str!("web/app.html"), "text/html; charset=utf-8");
    let Some(key) = q.get("k") else { return body };

    // A device that is already paired, scanning the code again, is the same
    // device. Spending the key here issued it a second token, filed a second
    // entry in the Connection card under the same phone, and burned a pairing
    // key another phone was queued up to use. The cookie it is already carrying
    // answers the question the key was going to — so it goes straight in, and
    // the key stays on the desktop for whoever actually needs it.
    if caller(&st, &headers).is_some() {
        return body;
    }

    let now = now_secs();
    let label = user_agent(&headers);
    let kind = device_kind(&headers);
    let ip = peer_ip(peer);
    // Three outcomes, not two: a bad key falls through to the PIN form, but a
    // good key with no room left has to say so — otherwise scanning the QR looks
    // like it worked and every later call answers 401.
    let issued = {
        let mut auth = lock(&st.auth);
        match auth.try_pairing_key(key, now) {
            Some(token) => match auth.remember(token, &label, kind, &ip) {
                Some(token) => Ok(token),
                None => Err(true),
            },
            None => Err(false),
        }
    };
    let token = match issued {
        Ok(t) => t,
        Err(true) => return full_page(),
        Err(false) => return body,
    };

    if let Some(pool) = &st.pool {
        let _ = ledger::remember_device(pool, &token, token.issued as i64).await;
    }

    let mut res = body;
    if let Ok(value) = header::HeaderValue::from_str(&cookie_for(&token)) {
        res.headers_mut().insert(header::SET_COOKIE, value);
    }
    res
}

async fn css() -> Response {
    asset(include_str!("web/app.css"), "text/css; charset=utf-8")
}

/// The web-app manifest. `application/manifest+json` rather than plain JSON:
/// Chrome accepts either, but the spec names this one and Safari is fussier.
async fn manifest() -> Response {
    asset(
        include_str!("web/manifest.webmanifest"),
        "application/manifest+json; charset=utf-8",
    )
}

/// The service worker.
///
/// `Service-Worker-Allowed: /` is what lets a worker served from anywhere claim
/// the whole origin; it is already at the root here, so the header is belt and
/// braces. `no-cache` is not: a worker cached for a day is a worker that keeps
/// serving last week's shell after a Tulipix upgrade, and the browser has no
/// other way to find out it changed.
async fn sw() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
            (
                header::HeaderName::from_static("service-worker-allowed"),
                "/",
            ),
        ],
        include_str!("web/sw.js"),
    )
        .into_response()
}

fn png(body: &'static [u8]) -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        body,
    )
        .into_response()
}

/// Home-screen icons. Two square marks and one maskable: Android crops an icon
/// to whatever shape the launcher uses, and a square mark cropped to a circle
/// loses its corners, so the maskable one carries its own padding.
async fn icon_192() -> Response {
    png(include_bytes!("web/icon-192.png").as_slice())
}

async fn icon_512() -> Response {
    png(include_bytes!("web/icon-512.png").as_slice())
}

async fn icon_maskable() -> Response {
    png(include_bytes!("web/icon-maskable.png").as_slice())
}

/// A share that got past the service worker.
///
/// This should not happen: the worker registers on the first page load and the
/// app cannot be installed — and so cannot be a share target — without one. If
/// it does, the files are in a multipart body that no cookie was attached to,
/// because the session cookie is SameSite=Strict and the operating system, not
/// the page, started this request. There is nothing safe to do with them, so
/// say so plainly rather than dropping them silently.
async fn share_fallback() -> Response {
    (
        StatusCode::SEE_OTHER,
        [(header::LOCATION, "/?share=empty")],
    )
        .into_response()
}

async fn js() -> Response {
    asset(include_str!("web/app.js"), "text/javascript; charset=utf-8")
}

/// The app mark, at 96px. Serves as both the favicon and the header logo, so a
/// tab left open on the phone is identifiable among a row of blank favicons.
/// Checked in already downscaled — the 819 KB source in `resources/appicons` is
/// an absurd thing to hand a phone for a 26px image.
/// Reached only over TLS, which is the whole of its meaning: a caller that gets
/// an answer has a browser that trusts this certificate.
async fn ping() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

/// The gateway, identical on both schemes — the plaintext side serves the same
/// page from `tls::Plain`, which is where a first-time phone actually meets it.
async fn trust(State(st): State<Shared>) -> Response {
    let page = crate::tls::gateway(&lock(&st.fingerprint));
    ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], page).into_response()
}

fn ca_download(st: &Shared, pem: bool) -> Response {
    let Some((ca_pem, ca_der)) = lock(&st.ca).clone() else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let (mime, name, body) = if pem {
        ("application/x-pem-file", "tulipix-ca.pem", ca_pem.into_bytes())
    } else {
        ("application/x-x509-ca-cert", "tulipix-ca.crt", ca_der)
    };
    let disposition = format!("attachment; filename=\"{name}\"");
    (
        [
            (header::CONTENT_TYPE, mime.to_string()),
            (header::CONTENT_DISPOSITION, disposition),
        ],
        body,
    )
        .into_response()
}

async fn ca_crt(State(st): State<Shared>) -> Response {
    ca_download(&st, false)
}

async fn ca_pem(State(st): State<Shared>) -> Response {
    ca_download(&st, true)
}

async fn logo() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_bytes!("web/logo.png").as_slice(),
    )
        .into_response()
}

/// The body is the PIN and nothing else — six characters, so a request body of
/// any size is refused before it is compared.
async fn auth_post(
    State(st): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let given = body.trim();
    if given.len() > 16 {
        return StatusCode::BAD_REQUEST.into_response();
    }
    let now = now_secs();
    let ip = peer_ip(peer);
    let label = user_agent(&headers);
    let kind = device_kind(&headers);

    let outcome = {
        let mut auth = lock(&st.auth);
        match auth.try_pin(&ip, given, now) {
            PinResult::Ok(token) => match auth.remember(token, &label, kind, &ip) {
                Some(token) => Ok(token),
                // 409, not 401: the PIN was right. Ten devices are paired and
                // one of them has to be forgotten on the desktop.
                None => Err(StatusCode::CONFLICT),
            },
            PinResult::Wrong => Err(StatusCode::UNAUTHORIZED),
            // 429, not 401: the difference is what the page tells the user.
            PinResult::LockedOut => Err(StatusCode::TOO_MANY_REQUESTS),
        }
    };

    let token = match outcome {
        Ok(t) => t,
        Err(code) => return code.into_response(),
    };

    if let Some(pool) = &st.pool {
        let _ = ledger::remember_device(pool, &token, token.issued as i64).await;
    }

    let mut res = StatusCode::OK.into_response();
    if let Ok(value) = header::HeaderValue::from_str(&cookie_for(&token)) {
        res.headers_mut().insert(header::SET_COOKIE, value);
    }
    res
}

async fn files(State(st): State<Shared>, headers: HeaderMap) -> Response {
    // `caller`, not `authorised`: the list is now per device, so the identity
    // behind the cookie is the answer and not just the fact that there is one.
    let Some(token) = caller(&st, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let rows: Vec<serde_json::Value> = lock(&st.tray)
        .items_for(&token)
        .map(|i| {
            serde_json::json!({
                "id": i.id,
                "name": i.name,
                "bytes": i.bytes,
                "kind": kind_of(&i.name),
            })
        })
        .collect();
    json(serde_json::to_string(&rows).unwrap_or_else(|_| "[]".into()))
}

/// Polled by the page so a file added on the desktop shows up on the phone
/// without a reload, and so an upload's progress survives a screen lock.
async fn status(State(st): State<Shared>, headers: HeaderMap) -> Response {
    let Some(token) = caller(&st, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    // The count the page polls for "a file appeared" has to be the same count
    // `/api/files` would return, or a file addressed to another phone makes
    // this one refetch a list that did not change.
    let files = lock(&st.tray).items_for(&token).count();
    let uploads: Vec<serde_json::Value> = lock(&st.uploads)
        .iter()
        .filter(|u| u.visible())
        .map(|u| {
            serde_json::json!({
                "name": u.name, "total": u.total, "done": u.done, "state": u.state,
            })
        })
        .collect();
    json(
        serde_json::to_string(&serde_json::json!({ "files": files, "uploads": uploads }))
            .unwrap_or_else(|_| "{}".into()),
    )
}

/// Downloads name a tray id, never a path: the client cannot express a file on
/// disk at all, which removes traversal from this side by construction.
async fn download(
    State(st): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(id): Path<u64>,
    headers: HeaderMap,
) -> Response {
    let Some(token) = caller(&st, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let (name, path) = {
        let tray = lock(&st.tray);
        // 404, not 403: a file addressed to another device is not a file this
        // one is being refused, it is a file this one was never offered — and
        // the id is guessable, so the two answers must not be distinguishable.
        let Some(item) = tray.by_id(id).filter(|i| i.visible_to(&token)) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(path) = tray.readable(id) else {
            // In the tray but gone from disk — the tray can be minutes old.
            return StatusCode::GONE.into_response();
        };
        (item.name.clone(), path)
    };

    let Ok(meta) = tokio::fs::metadata(&path).await else {
        return StatusCode::GONE.into_response();
    };
    let len = meta.len();

    let asked = headers.get(header::RANGE).and_then(|v| v.to_str().ok()).unwrap_or("");
    // A malformed or unsatisfiable Range sends the whole file with 200 rather
    // than failing: the phone gets its file either way.
    let (start, end) = range::parse(asked, len).unwrap_or((0, len.saturating_sub(1)));
    let partial = !asked.is_empty() && (start, end) != (0, len.saturating_sub(1));
    let span = if len == 0 { 0 } else { end - start + 1 };

    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return StatusCode::GONE.into_response();
    };
    if start > 0 {
        use tokio::io::AsyncSeekExt as _;
        if file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
    }

    // Only the first request of a download is recorded. A resumed transfer sends
    // `Range: bytes=N-` with N > 0, and one row per resume would turn a flaky
    // Wi-Fi link into a page of identical entries.
    let row = if start == 0 {
        let from = path.to_string_lossy().into_owned();
        st.record(ledger::Row::sent(&name, &from, len as i64, &peer_ip(peer))).await
    } else {
        None
    };
    if let Some(row) = row {
        lock(&st.sends).push(Outgoing { row, name: name.clone(), done: 0, total: span });
    }

    // The stream stamps the device on every chunk, so the ring in the Connection
    // card follows the actual bytes rather than the request that started them —
    // a 200 MB video keeps the ring lit for as long as it is really sending.
    let body = Body::from_stream(file_stream(file, span, st.clone(), token, row));
    let disposition = format!("attachment; filename=\"{}\"", name.replace('"', "'"));
    let mut res = Response::new(body);
    {
        let h = res.headers_mut();
        insert(h, header::CONTENT_TYPE, "application/octet-stream");
        insert(h, header::CONTENT_LENGTH, &span.to_string());
        insert(h, header::ACCEPT_RANGES, "bytes");
        insert(h, header::CONTENT_DISPOSITION, &disposition);
        if partial {
            insert(h, header::CONTENT_RANGE, &format!("bytes {start}-{end}/{len}"));
        }
    }
    if partial {
        *res.status_mut() = StatusCode::PARTIAL_CONTENT;
    }
    res
}

fn insert(headers: &mut HeaderMap, name: header::HeaderName, value: &str) {
    if let Ok(v) = header::HeaderValue::from_str(value) {
        headers.insert(name, v);
    }
}

/// `span` bytes from wherever the handle is already seeked to, in `CHUNK` reads.
/// Settles the ledger row of a download however the stream ends.
///
/// A dropped body is the common ending, not the exceptional one: a phone that
/// walks out of range, a browser tab closed mid-file, a cancelled download.
/// Nothing calls back to say so, so `Drop` is the only place that can stop the
/// row saying "sending" forever.
struct SendGuard {
    st: Shared,
    row: Option<i64>,
}

impl SendGuard {
    async fn settle(&mut self, status: &'static str) {
        let Some(row) = self.row.take() else { return };
        self.st.settle(row, status).await;
    }
}

impl Drop for SendGuard {
    fn drop(&mut self) {
        let Some(row) = self.row.take() else { return };
        let st = self.st.clone();
        // Which ending this was is decided by the bytes counted, not by whether
        // the stream was polled one last time: with a known Content-Length the
        // server may stop asking as soon as it has them all, and a download that
        // completed must not be filed as a failure because of it.
        let status = {
            let sends = lock(&st.sends);
            match sends.iter().find(|s| s.row == row) {
                Some(s) if s.done >= s.total => "ok",
                _ => "failed",
            }
        };
        // Drop cannot await. Outside a runtime there is nothing to spawn onto
        // either — the row is then settled by `settle_stale` on the next start.
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            rt.spawn(async move { st.settle(row, status).await });
        }
    }
}

fn file_stream(
    file: tokio::fs::File,
    span: u64,
    st: Shared,
    token: String,
    row: Option<i64>,
) -> impl futures_util::stream::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    use tokio::io::AsyncReadExt as _;
    let guard = SendGuard { st: st.clone(), row };
    futures_util::stream::unfold((file, span, guard), move |(mut file, left, mut guard)| {
        let st = st.clone();
        let token = token.clone();
        async move {
            if left == 0 {
                guard.settle("ok").await;
                return None;
            }
            let want = CHUNK.min(left as usize);
            let mut buf = vec![0u8; want];
            match file.read(&mut buf).await {
                // A short file under a stale length: stop rather than pad with zeros.
                Ok(0) => {
                    guard.settle("failed").await;
                    None
                }
                Ok(n) => {
                    buf.truncate(n);
                    lock(&st.auth).mark_active(&token, now_secs());
                    st.sent_bytes.fetch_add(n as u64, Ordering::Relaxed);
                    if let Some(row) = guard.row {
                        if let Some(s) = lock(&st.sends).iter_mut().find(|s| s.row == row) {
                            s.done += n as u64;
                        }
                    }
                    Some((Ok(buf), (file, left - n as u64, guard)))
                }
                Err(e) => {
                    guard.settle("failed").await;
                    Some((Err(e), (file, 0, guard)))
                }
            }
        }
    })
}

/// The point of the whole design: the file itself is the request body, so there
/// is no multipart boundary to parse across chunks.
async fn upload(
    State(st): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(token) = caller(&st, &headers) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let dir = lock(&st.inbox).clone();
    let Ok(mut sink) = inbox::Sink::create(&dir, &name).await else {
        return StatusCode::INSUFFICIENT_STORAGE.into_response();
    };

    let total = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(0);
    let peer_ip = peer_ip(peer);
    let id = st.next_upload.fetch_add(1, Ordering::Relaxed);
    lock(&st.uploads).push(Upload {
        id,
        name: crate::names::safe_name(&name),
        total,
        done: 0,
        state: "active",
        peer: peer_ip.clone(),
        finished: None,
    });

    let mut stream = body.into_data_stream();
    use futures_util::StreamExt as _;
    while let Some(chunk) = stream.next().await {
        let Ok(chunk) = chunk else {
            // The client vanished — a phone that walked out of range.
            sink.abort().await;
            finish_upload(&st, id, "failed");
            let _ = st.record(ledger::Row::received(&name, "", total as i64, &peer_ip).failed()).await;
            return StatusCode::BAD_REQUEST.into_response();
        };
        if sink.write(&chunk).await.is_err() {
            // Disk full, or the inbox went away mid-transfer.
            let written = sink.written;
            sink.abort().await;
            finish_upload(&st, id, "failed");
            let _ = st.record(ledger::Row::received(&name, "", written as i64, &peer_ip).failed()).await;
            return StatusCode::INSUFFICIENT_STORAGE.into_response();
        }
        st.recv_bytes.fetch_add(chunk.len() as u64, Ordering::Relaxed);
        progress(&st, id, sink.written);
        lock(&st.auth).mark_active(&token, now_secs());
    }

    let written = sink.written;
    match sink.finish().await {
        Ok(path) => {
            finish_upload(&st, id, "done");
            let landed = path.display().to_string();
            let shown = path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone());
            let _ = st.record(ledger::Row::received(&shown, &landed, written as i64, &peer_ip)).await;
            StatusCode::OK.into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "transfer: upload could not be finalised");
            finish_upload(&st, id, "failed");
            let _ = st.record(ledger::Row::received(&name, "", written as i64, &peer_ip).failed()).await;
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

fn progress(st: &AppState, id: u64, done: u64) {
    if let Some(u) = lock(&st.uploads).iter_mut().find(|u| u.id == id) {
        u.done = done;
    }
}

fn finish_upload(st: &AppState, id: u64, state: &'static str) {
    let mut uploads = lock(&st.uploads);
    if let Some(u) = uploads.iter_mut().find(|u| u.id == id) {
        u.state = state;
        u.finished = Some(std::time::Instant::now());
        if state == "done" {
            u.total = u.done;
        }
    }
    // Keep the last few rows so the Receive pane and the phone can show what
    // just landed; drop the rest so a long session does not grow without bound.
    while uploads.len() > 20 {
        let Some(pos) = uploads.iter().position(|u| u.state != "active") else { break };
        uploads.remove(pos);
    }
}

/// Three or four letters for the file-kind badge on the phone.
fn kind_of(name: &str) -> String {
    std::path::Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_uppercase())
        .filter(|e| e.len() <= 4)
        .unwrap_or_else(|| "FILE".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn started() -> (Running, String, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let run = start(Config {
            inbox: dir.path().to_path_buf(),
            bind: "127.0.0.1:0".into(),
            pool: None,
            tls: false,
        })
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", run.port);
        (run, base, dir)
    }

    fn client() -> reqwest::Client {
        reqwest::Client::builder().cookie_store(true).build().unwrap()
    }

    fn upload_at(state: &'static str, finished: Option<std::time::Instant>) -> Upload {
        Upload {
            id: 1,
            name: "a.mp3".into(),
            total: 10,
            done: 10,
            state,
            peer: "10.0.0.2".into(),
            finished,
        }
    }

    #[test]
    fn a_finished_upload_clears_itself_but_a_failed_one_stays() {
        let now = std::time::Instant::now();
        let stale = now.checked_sub(DONE_LINGER * 2).expect("clock is past the epoch");

        assert!(upload_at("active", None).visible(), "an upload in flight is always shown");
        assert!(upload_at("done", Some(now)).visible(), "a success shows for a while");
        assert!(!upload_at("done", Some(stale)).visible(), "a success expires");
        // The one row worth reading late — it never expires on its own.
        assert!(upload_at("failed", Some(stale)).visible(), "a failure stays until dismissed");
    }

    #[tokio::test]
    async fn the_page_is_served_without_a_cookie() {
        let (run, base, _dir) = started().await;
        let res = reqwest::get(format!("{base}/")).await.unwrap();
        assert_eq!(res.status(), 200);
        run.stop().await;
    }

    #[tokio::test]
    async fn the_api_refuses_a_request_with_no_cookie() {
        let (run, base, _dir) = started().await;
        let res = reqwest::get(format!("{base}/api/files")).await.unwrap();
        assert_eq!(res.status(), 401);
        run.stop().await;
    }

    #[tokio::test]
    async fn the_right_pin_opens_the_api_and_the_wrong_one_does_not() {
        let (run, base, _dir) = started().await;
        let c = client();

        let bad = c.post(format!("{base}/auth")).body("000000").send().await.unwrap();
        assert_eq!(bad.status(), 401);

        let good = c.post(format!("{base}/auth")).body(run.pin.clone()).send().await.unwrap();
        assert_eq!(good.status(), 200);

        let files = c.get(format!("{base}/api/files")).send().await.unwrap();
        assert_eq!(files.status(), 200);
        run.stop().await;
    }

    /// Scanning the QR again from a phone that is already paired used to spend a
    /// pairing key, issue a second token and file the same phone twice in the
    /// Connection card. It is the same device: it goes straight in.
    #[tokio::test]
    async fn rescanning_the_code_from_a_paired_device_does_not_pair_it_twice() {
        let (run, base, _dir) = started().await;
        let c = client();
        c.post(format!("{base}/auth")).body(run.pin.clone()).send().await.unwrap();
        assert_eq!(lock(&run.state.auth).device_count(now_secs()), 1);

        let key = lock(&run.state.auth).new_pairing_key(now_secs());
        let res = c.get(format!("{base}/?k={key}")).send().await.unwrap();
        assert_eq!(res.status(), 200, "it still lands on the page");
        assert_eq!(
            lock(&run.state.auth).device_count(now_secs()),
            1,
            "and it is still one device, not two"
        );
        // The key it did not need is still there for a phone that does.
        assert!(
            lock(&run.state.auth).try_pairing_key(&key, now_secs()).is_some(),
            "the key was not spent"
        );
        run.stop().await;
    }

    /// A file added while the tray points at one device belongs to that device.
    /// Another paired phone must not see it in the list, and must not be able to
    /// reach it by guessing the id either.
    #[tokio::test]
    async fn a_targeted_file_is_invisible_to_every_other_device() {
        let (run, base, dir) = started().await;
        let p = dir.path().join("private.bin");
        std::fs::write(&p, b"secret").unwrap();

        // Pair one device, then address the tray to somebody else entirely.
        let c = client();
        c.post(format!("{base}/auth")).body(run.pin.clone()).send().await.unwrap();
        {
            let mut tray = lock(&run.state.tray);
            tray.set_target(Some("some-other-device".into()));
            tray.add(&p);
        }
        let id = lock(&run.state.tray).items()[0].id;

        let listed = c.get(format!("{base}/api/files")).send().await.unwrap();
        assert_eq!(listed.text().await.unwrap(), "[]", "not in this device's list");

        let reached = c.get(format!("{base}/dl/{id}")).send().await.unwrap();
        assert_eq!(reached.status(), 404, "and not reachable by id");

        // Point it at everyone and the same device can now see the same file.
        lock(&run.state.tray).release("some-other-device");
        let reached = c.get(format!("{base}/dl/{id}")).send().await.unwrap();
        assert_eq!(reached.status(), 200);
        run.stop().await;
    }

    #[tokio::test]
    async fn a_download_serves_the_tray_by_id_and_a_range_resumes_it() {
        let (run, base, dir) = started().await;
        let p = dir.path().join("song.bin");
        std::fs::write(&p, b"0123456789").unwrap();
        lock(&run.state.tray).add(&p);
        let id = lock(&run.state.tray).items()[0].id;

        let c = client();
        c.post(format!("{base}/auth")).body(run.pin.clone()).send().await.unwrap();

        let whole = c.get(format!("{base}/dl/{id}")).send().await.unwrap();
        assert_eq!(whole.status(), 200);
        assert_eq!(whole.bytes().await.unwrap().as_ref(), b"0123456789");

        let part = c
            .get(format!("{base}/dl/{id}"))
            .header("Range", "bytes=4-6")
            .send()
            .await
            .unwrap();
        assert_eq!(part.status(), 206);
        assert_eq!(part.bytes().await.unwrap().as_ref(), b"456");

        // An id that was never in the tray is a 404, not a path to try.
        let missing = c.get(format!("{base}/dl/9999")).send().await.unwrap();
        assert_eq!(missing.status(), 404);
        run.stop().await;
    }

    /// The ledger row of a download is written on the first chunk, so it has to
    /// be settled when the last one goes out — a row stuck at "sending" is one
    /// the table can never report as finished.
    #[tokio::test]
    async fn a_download_leaves_its_ledger_row_settled() {
        let dir = tempfile::tempdir().unwrap();
        let pool = sqlx::SqlitePool::connect("sqlite::memory:").await.unwrap();
        ledger::apply_schema(&pool).await.unwrap();
        let run = start(Config {
            inbox: dir.path().to_path_buf(),
            bind: "127.0.0.1:0".into(),
            pool: Some(pool.clone()),
            tls: false,
        })
        .await
        .unwrap();
        let base = format!("http://127.0.0.1:{}", run.port);

        let p = dir.path().join("song.bin");
        std::fs::write(&p, b"0123456789").unwrap();
        lock(&run.state.tray).add(&p);
        let id = lock(&run.state.tray).items()[0].id;

        let c = client();
        c.post(format!("{base}/auth")).body(run.pin.clone()).send().await.unwrap();
        let res = c.get(format!("{base}/dl/{id}")).send().await.unwrap();
        assert_eq!(res.bytes().await.unwrap().as_ref(), b"0123456789");

        // The settling can be one spawned task behind the last byte, so this
        // waits for it rather than assuming the ordering.
        let mut status = String::new();
        for _ in 0..50 {
            let rows = ledger::recent(&pool, 0, ledger::Sort::default(), true).await.unwrap();
            assert_eq!(rows.len(), 1, "one row per download, resumes included");
            status = rows[0].status.clone();
            if status != "sending" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(status, "ok");
        // And nothing is left claiming to be on the wire.
        assert!(lock(&run.state.sends).is_empty());
        run.stop().await;
    }

    #[tokio::test]
    async fn an_upload_lands_in_the_inbox_under_a_safe_name() {
        let (run, base, dir) = started().await;
        let c = client();
        c.post(format!("{base}/auth")).body(run.pin.clone()).send().await.unwrap();

        let res = c
            .put(format!("{base}/upload/{}", "..%2F..%2Fetc%2Fpasswd"))
            .body("hello")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
        assert_eq!(std::fs::read_to_string(dir.path().join("passwd")).unwrap(), "hello");
        run.stop().await;
    }

    #[tokio::test]
    async fn an_upload_without_a_cookie_writes_nothing() {
        let (run, base, dir) = started().await;
        let res = reqwest::Client::new()
            .put(format!("{base}/upload/x.bin"))
            .body("hello")
            .send()
            .await
            .unwrap();
        assert_eq!(res.status(), 401);
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        run.stop().await;
    }

    #[tokio::test]
    async fn stopping_the_server_closes_the_port() {
        let (run, base, _dir) = started().await;
        run.stop().await;
        assert!(reqwest::get(format!("{base}/")).await.is_err(), "port still answering");
    }
}
