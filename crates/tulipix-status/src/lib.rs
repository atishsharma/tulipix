//! The Status section: a dashboard the app serves to your browser.
//!
//! Slint has no HTML renderer, and the one thing this page wants to be is a
//! wide, dense, freely-styled document — so it is not a Slint page at all. The
//! sidebar button starts a loopback HTTP server and opens the default browser
//! at it. Everything the page shows comes from [`collect`], which reads the
//! same databases the GUI does.
//!
//! # Why this is not open to the network
//!
//! The listener binds `127.0.0.1` and nothing else. Not `0.0.0.0`, not a LAN
//! address: the socket is unreachable from another machine by construction,
//! so there is no firewall rule to write and no port to forward. (Contrast
//! `tulipix-transfer`, which is *meant* to be reached by a phone and therefore
//! binds a private interface and speaks TLS.)
//!
//! Loopback alone is not enough, because the page can drive the app — it can
//! switch sections and start a rescan. Any web page open in the same browser
//! can POST to `127.0.0.1` on a guessed port; it cannot read the response
//! (that is what the same-origin policy is for), but a blind POST is still a
//! write. So every route carries a 256-bit token minted at startup and handed
//! over only in the URL the app itself opens. A request without it is refused
//! before it reaches any handler.
//!
//! The port is OS-assigned (`:0`). A fixed port is worth having when a rule
//! must be written against it, which is exactly what does not happen here, and
//! a fixed port is one more thing for an unrelated page to guess.

pub mod collect;

pub use collect::{Live, Snapshot};

use axum::{
    extract::{Query, State as AxState},
    http::{header, StatusCode},
    response::{Html, IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicI64, Ordering},
    Arc, RwLock,
};

/// What the page can ask the app to do. Sent over a channel rather than called
/// directly: the handlers run on a tokio worker and every one of these ends in
/// a Slint call, which must happen on the UI thread.
#[derive(Debug, Clone)]
pub enum Action {
    /// Switch the app to a section (the popup's "Open … in Tulipix").
    OpenSection(String),
    /// Re-scan every library root.
    Rescan,
    /// Open the app's own "add watched folder" picker.
    ///
    /// A file input in the browser would hand back a *copy* of the bytes with
    /// no path attached, which is the opposite of what a watched folder is. The
    /// page asks the app to run the same picker its Settings screen runs, so
    /// the folder is registered, persisted and scanned by exactly one code path.
    AddFolder,
    /// Erase every database, setting and cache, then relaunch clean. The same
    /// handler Settings → Libraries → Reset App fires.
    ResetApp,
}

pub type Actions = tokio::sync::mpsc::UnboundedSender<Action>;

struct Inner {
    token: String,
    /// The rendered snapshot, swapped wholesale by the app's refresh tick.
    /// `RwLock` because reads (one per polling tab) vastly outnumber writes
    /// (one per tick), and a reader never blocks another reader.
    snap: RwLock<Arc<Value>>,
    /// Unix seconds of the most recent `/api/status` fetch. The app reads this
    /// to decide how hard to refresh: there is no reason to re-query ten
    /// databases every two seconds for a page nobody has open.
    last_seen: AtomicI64,
    /// Set when Status is clicked while a page is already polling. The next
    /// `/api/status` response carries it once and clears it, and the live page
    /// pulls itself to the front. See [`Running::request_focus`].
    focus: std::sync::atomic::AtomicBool,
    actions: Actions,
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// A listening status server.
pub struct Running {
    pub port: u16,
    /// The full URL including the token — what gets handed to the browser.
    pub url: String,
    inner: Arc<Inner>,
    shutdown: Option<tokio::sync::oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

impl Running {
    /// Publish a freshly collected snapshot. Cheap: swaps one `Arc`.
    pub fn publish(&self, snap: &Snapshot) {
        if let Ok(mut w) = self.inner.snap.write() {
            *w = Arc::new(snap.json.clone());
        }
    }

    /// Seconds since a browser last fetched the JSON. Large means nobody is
    /// looking.
    pub fn idle_secs(&self) -> i64 {
        now() - self.inner.last_seen.load(Ordering::Relaxed)
    }

    /// Has a page polled recently enough to be considered open.
    pub fn watched(&self) -> bool {
        self.idle_secs() < 12
    }

    /// Ask the page that is already open to bring itself forward, instead of
    /// handing the browser the URL a second time.
    ///
    /// Opening the URL again is what produces the pile of identical tabs. The
    /// alternative has one honest limit: a background process cannot raise a
    /// browser window, and `window.focus()` is only granted to a page the
    /// browser considers to have been interacted with. So the page also flashes
    /// itself, which is visible the moment it *is* looked at — better than a
    /// duplicate tab, and it never lies about having succeeded.
    pub fn request_focus(&self) {
        self.inner.focus.store(true, Ordering::Relaxed);
    }

    pub async fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        let _ = self.handle.await;
    }

    /// Close the port without draining. For the app-exit path, where there is
    /// no longer a runtime to await on.
    pub fn abort(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.handle.abort();
    }
}

/// 256 bits of OS randomness, hex. Not a password and never stored — it lives
/// for one run of one server and exists only so that a request proves it came
/// from the URL this app opened.
fn mint_token() -> String {
    let mut b = [0u8; 32];
    if getrandom::getrandom(&mut b).is_err() {
        // Refusing to serve beats serving unauthenticated: the page can drive
        // the app, so a predictable token is worse than no status page.
        return String::new();
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// Bind a loopback listener and start serving. The caller keeps the returned
/// [`Running`] for the life of the app — the app holds it in a `OnceCell`, so
/// clicking Status twice reuses the one listener instead of binding a second.
pub async fn start(actions: Actions) -> anyhow::Result<Running> {
    let token = mint_token();
    anyhow::ensure!(!token.is_empty(), "no system randomness for the status token");

    let inner = Arc::new(Inner {
        token,
        snap: RwLock::new(Arc::new(json!({ "level": "unknown", "headline": "Starting…" }))),
        last_seen: AtomicI64::new(0),
        focus: std::sync::atomic::AtomicBool::new(false),
        actions,
    });

    let app = Router::new()
        .route("/", get(page))
        .route("/logo.png", get(logo))
        .route("/title.woff2", get(title_font))
        .route("/api/status", get(status))
        .route("/api/open", post(open_section))
        .route("/api/rescan", post(rescan))
        .route("/api/add-folder", post(add_folder))
        .route("/api/reset", post(reset))
        .with_state(inner.clone());

    // Loopback only. See the module note: this is the whole network story.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let url = format!("http://127.0.0.1:{port}/?t={}", inner.token);

    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let served = axum::serve(listener, app).with_graceful_shutdown(async {
            let _ = rx.await;
        });
        if let Err(e) = served.await {
            tracing::warn!(error = %e, "status: server stopped");
        }
    });

    tracing::info!(port, "status: serving on loopback");
    Ok(Running { port, url, inner, shutdown: Some(tx), handle })
}

/// Token carried as `?t=…` on every request, including the page load, so the
/// page can read it back out of its own URL and reuse it for the API calls.
#[derive(serde::Deserialize)]
struct Auth {
    t: Option<String>,
}

/// JSON response without `axum::Json`. The workspace pins axum with
/// `default-features = false` and a deliberately short feature list (see the
/// note in the root manifest); `json` is not on it, and turning it on for this
/// crate would turn it on for `tulipix-transfer` too. Two lines here is the
/// cheaper trade.
fn json_body(v: &Value) -> Response {
    (
        [
            (header::CONTENT_TYPE, "application/json"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        serde_json::to_string(v).unwrap_or_else(|_| "{}".into()),
    )
        .into_response()
}

fn authed(inner: &Inner, q: &Auth) -> bool {
    // Constant-time-ish: compare full length regardless of where it diverges.
    // Overkill for a loopback token that changes every run, cheap to do right.
    match &q.t {
        Some(t) if t.len() == inner.token.len() => {
            let mut diff = 0u8;
            for (a, b) in t.as_bytes().iter().zip(inner.token.as_bytes()) {
                diff |= a ^ b;
            }
            diff == 0
        }
        _ => false,
    }
}

const DENIED: (StatusCode, &str) = (StatusCode::FORBIDDEN, "bad or missing token");

async fn page(AxState(inner): AxState<Arc<Inner>>, Query(q): Query<Auth>) -> Response {
    if !authed(&inner, &q) {
        return DENIED.into_response();
    }
    // `no-store`: the page is a live view of a local process. A cached copy is
    // a screenshot pretending to be current.
    (
        [(header::CACHE_CONTROL, "no-store")],
        Html(include_str!("../assets/status.html")),
    )
        .into_response()
}

/// The app's own icon, for the page mark and the favicon.
///
/// Unauthenticated on purpose, and the only route that is. It is a static image
/// compiled into the binary — there is no state behind it to read and nothing
/// behind it to drive — and a favicon that 403s is a browser-tab question mark
/// on every reload. Everything that reports or changes anything still needs the
/// token.
async fn logo() -> Response {
    (
        [
            (header::CONTENT_TYPE, "image/png"),
            // Unlike the page and the JSON, this one genuinely never changes
            // for the life of a build.
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_bytes!("../../../resources/appicons/sidebar-color.png").as_slice(),
    )
        .into_response()
}

/// Rubik Spray Paint, latin subset, for the one word in the page header.
///
/// Served rather than inlined as a data: URI. Base64 would add ~160 KB to a
/// 45 KB document on every load and to the binary that carries it; a route
/// costs one request the browser then caches for a day.
///
/// Bundled, not fetched: the page's whole promise is that it renders with the
/// network off, and a webfont from a CDN is the classic way that promise breaks
/// — plus it would tell a third party every time someone opened their own
/// dashboard. Licensed SIL OFL 1.1; see `assets/FONT-OFL.txt`, which ships
/// beside it.
///
/// Unauthenticated for the same reason as [`logo`]: a static asset with no
/// state behind it, and a 403 here would only mean an unstyled title.
async fn title_font() -> Response {
    (
        [
            (header::CONTENT_TYPE, "font/woff2"),
            (header::CACHE_CONTROL, "public, max-age=86400"),
        ],
        include_bytes!("../assets/rubik-spray-paint-latin.woff2").as_slice(),
    )
        .into_response()
}

async fn status(AxState(inner): AxState<Arc<Inner>>, Query(q): Query<Auth>) -> Response {
    if !authed(&inner, &q) {
        return DENIED.into_response();
    }
    inner.last_seen.store(now(), Ordering::Relaxed);
    let snap = inner.snap.read().ok().map(|s| s.as_ref().clone());
    match snap {
        Some(mut v) => {
            // Taken, not read: the request to come forward is consumed by the
            // first page to collect it, so two open tabs do not both jump and a
            // reload does not replay the last one.
            if inner.focus.swap(false, Ordering::Relaxed) {
                v["focus"] = json!(true);
            }
            json_body(&v)
        }
        None => (StatusCode::INTERNAL_SERVER_ERROR, "snapshot unavailable").into_response(),
    }
}

async fn open_section(
    AxState(inner): AxState<Arc<Inner>>,
    Query(q): Query<Auth>,
    body: String,
) -> Response {
    if !authed(&inner, &q) {
        return DENIED.into_response();
    }
    let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    let Some(section) = body.get("section").and_then(|v| v.as_str()) else {
        return (StatusCode::BAD_REQUEST, "no section").into_response();
    };
    // Allow-list, not passthrough: this value crosses into the UI thread and
    // sets the active section. An unknown string would leave the app on a
    // blank page, and the set of sections is closed and known.
    const SECTIONS: &[&str] = &[
        "home", "photos", "videos", "music", "books", "cloud", "tools", "transfer",
        "finances", "settings",
    ];
    if !SECTIONS.contains(&section) {
        return (StatusCode::BAD_REQUEST, "unknown section").into_response();
    }
    let _ = inner.actions.send(Action::OpenSection(section.to_string()));
    json_body(&json!({ "ok": true }))
}

async fn rescan(AxState(inner): AxState<Arc<Inner>>, Query(q): Query<Auth>) -> Response {
    if !authed(&inner, &q) {
        return DENIED.into_response();
    }
    let _ = inner.actions.send(Action::Rescan);
    json_body(&json!({ "ok": true }))
}

async fn add_folder(AxState(inner): AxState<Arc<Inner>>, Query(q): Query<Auth>) -> Response {
    if !authed(&inner, &q) {
        return DENIED.into_response();
    }
    let _ = inner.actions.send(Action::AddFolder);
    json_body(&json!({ "ok": true }))
}

/// Erase everything and relaunch. Guarded twice over: the token, and a phrase
/// the caller has to have typed.
///
/// The confirmation is checked here, not only in the page. Everything else a
/// route can do is recoverable — this one is not, and a check that lives only
/// in the JavaScript is a check that a `curl` with the token skips.
async fn reset(
    AxState(inner): AxState<Arc<Inner>>,
    Query(q): Query<Auth>,
    body: String,
) -> Response {
    if !authed(&inner, &q) {
        return DENIED.into_response();
    }
    let body: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    if body.get("confirm").and_then(|v| v.as_str()) != Some(RESET_PHRASE) {
        return (StatusCode::BAD_REQUEST, "reset not confirmed").into_response();
    }
    tracing::warn!("status: full app reset requested from the dashboard");
    let _ = inner.actions.send(Action::ResetApp);
    json_body(&json!({ "ok": true }))
}

/// What has to be typed to confirm a reset. Exported so the page and the server
/// cannot drift apart on it.
pub const RESET_PHRASE: &str = "RESET";

/// Hand a URL to the OS default browser.
pub fn open_in_browser(url: &str) {
    // Only ever a loopback http URL — this is called with a string this crate
    // built, but the check is cheap and keeps that true if a caller changes.
    if !url.starts_with("http://127.0.0.1:") {
        tracing::warn!(%url, "status: refusing to open non-loopback url");
        return;
    }
    #[cfg(target_os = "windows")]
    let r = std::process::Command::new("explorer.exe").arg(url).spawn();
    #[cfg(target_os = "macos")]
    let r = std::process::Command::new("open").arg(url).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let r = std::process::Command::new("xdg-open").arg(url).spawn();
    if let Err(e) = r {
        tracing::warn!(error = %e, %url, "status: could not open browser");
    }
}

/// Process start, for the session-uptime row. Set once, on first call.
pub fn uptime_secs() -> i64 {
    use std::sync::OnceLock;
    static START: OnceLock<std::time::Instant> = OnceLock::new();
    START.get_or_init(std::time::Instant::now).elapsed().as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_is_long_and_unique() {
        let a = mint_token();
        let b = mint_token();
        assert_eq!(a.len(), 64, "32 bytes as hex");
        assert_ne!(a, b, "a fresh token per server");
    }

    #[test]
    fn auth_rejects_everything_but_the_token() {
        let inner = Inner {
            token: "abcd".into(),
            snap: RwLock::new(Arc::new(json!({}))),
            last_seen: AtomicI64::new(0),
            focus: std::sync::atomic::AtomicBool::new(false),
            actions: tokio::sync::mpsc::unbounded_channel().0,
        };
        assert!(authed(&inner, &Auth { t: Some("abcd".into()) }));
        assert!(!authed(&inner, &Auth { t: Some("abce".into()) }));
        assert!(!authed(&inner, &Auth { t: Some("abc".into()) }), "short");
        assert!(!authed(&inner, &Auth { t: Some("abcde".into()) }), "long");
        assert!(!authed(&inner, &Auth { t: None }), "absent");
    }
}
