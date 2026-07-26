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

pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

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

pub struct AppState {
    pub auth: Mutex<Auth>,
    pub tray: Mutex<Tray>,
    /// Behind a mutex so changing it in Settings takes effect without a restart.
    pub inbox: Mutex<PathBuf>,
    pub pool: Option<SqlitePool>,
    pub uploads: Mutex<Vec<Upload>>,
    next_upload: AtomicU64,
    /// Bumped on every ledger write. The UI refetches Recent Transfers when it
    /// changes rather than polling the database on a timer.
    pub rev: AtomicU64,
}

pub type Shared = Arc<AppState>;

impl AppState {
    fn bump(&self) {
        self.rev.fetch_add(1, Ordering::Relaxed);
    }

    async fn record(&self, row: ledger::Row) {
        let Some(pool) = &self.pool else { return };
        if let Err(e) = ledger::record(pool, row, now_secs() as i64).await {
            tracing::warn!(error = %e, "transfer: ledger write failed");
        }
        self.bump();
    }
}

pub struct Config {
    pub inbox: PathBuf,
    pub bind: String,
    /// `None` in tests and when `transfers.db` could not be opened — the server
    /// still transfers files, it just does not remember having done so.
    pub pool: Option<SqlitePool>,
}

pub struct Running {
    pub port: u16,
    pub pin: String,
    pub state: Shared,
    shutdown: tokio::sync::oneshot::Sender<()>,
    handle: tokio::task::JoinHandle<()>,
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
        next_upload: AtomicU64::new(0),
        rev: AtomicU64::new(0),
    });

    let app = Router::new()
        .route("/", get(page))
        .route("/app.css", get(css))
        .route("/app.js", get(js))
        .route("/auth", post(auth_post))
        .route("/api/files", get(files))
        .route("/api/status", get(status))
        .route("/dl/{id}", get(download))
        .route("/upload/{name}", put(upload))
        .with_state(state.clone());

    let listener = tokio::net::TcpListener::bind(&cfg.bind).await?;
    let port = listener.local_addr()?.port();

    let (tx, rx) = tokio::sync::oneshot::channel();
    let handle = tokio::spawn(async move {
        let served =
            axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
                .with_graceful_shutdown(async {
                    let _ = rx.await;
                });
        if let Err(e) = served.await {
            tracing::warn!(error = %e, "transfer: server stopped");
        }
    });

    Ok(Running { port, pin, state, shutdown: tx, handle })
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

fn authorised(st: &AppState, headers: &HeaderMap) -> bool {
    caller(st, headers).is_some()
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

async fn js() -> Response {
    asset(include_str!("web/app.js"), "text/javascript; charset=utf-8")
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
    if !authorised(&st, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let rows: Vec<serde_json::Value> = lock(&st.tray)
        .items()
        .iter()
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
    if !authorised(&st, &headers) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let files = lock(&st.tray).items().len();
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
        let Some(item) = tray.by_id(id) else {
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
    if start == 0 {
        st.record(ledger::Row::sent(&name, len as i64, &peer_ip(peer))).await;
    }

    // The stream stamps the device on every chunk, so the ring in the Connection
    // card follows the actual bytes rather than the request that started them —
    // a 200 MB video keeps the ring lit for as long as it is really sending.
    let body = Body::from_stream(file_stream(file, span, st.clone(), token));
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
fn file_stream(
    file: tokio::fs::File,
    span: u64,
    st: Shared,
    token: String,
) -> impl futures_util::stream::Stream<Item = Result<Vec<u8>, std::io::Error>> {
    use tokio::io::AsyncReadExt as _;
    futures_util::stream::unfold((file, span), move |(mut file, left)| {
        let st = st.clone();
        let token = token.clone();
        async move {
            if left == 0 {
                return None;
            }
            let want = CHUNK.min(left as usize);
            let mut buf = vec![0u8; want];
            match file.read(&mut buf).await {
                // A short file under a stale length: stop rather than pad with zeros.
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    lock(&st.auth).mark_active(&token, now_secs());
                    Some((Ok(buf), (file, left - n as u64)))
                }
                Err(e) => Some((Err(e), (file, 0))),
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
            st.record(ledger::Row::received(&name, "", total as i64, &peer_ip).failed()).await;
            return StatusCode::BAD_REQUEST.into_response();
        };
        if sink.write(&chunk).await.is_err() {
            // Disk full, or the inbox went away mid-transfer.
            let written = sink.written;
            sink.abort().await;
            finish_upload(&st, id, "failed");
            st.record(ledger::Row::received(&name, "", written as i64, &peer_ip).failed()).await;
            return StatusCode::INSUFFICIENT_STORAGE.into_response();
        }
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
            st.record(ledger::Row::received(&shown, &landed, written as i64, &peer_ip)).await;
            StatusCode::OK.into_response()
        }
        Err(e) => {
            tracing::warn!(error = %e, "transfer: upload could not be finalised");
            finish_upload(&st, id, "failed");
            st.record(ledger::Row::received(&name, "", written as i64, &peer_ip).failed()).await;
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
