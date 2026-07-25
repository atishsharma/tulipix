//! Genesis-section wiring: the bridge between [`tulipix_genesis`] — which is
//! blocking and knows nothing about Slint — and every `window.on_genesis_*`
//! callback.
//!
//! The core stays blocking on purpose (see that crate's docs), so every call
//! into it goes through `spawn_blocking` and comes back to the UI via
//! `upgrade_in_event_loop`. Nothing here holds a lock across an await.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tulipix_genesis::model::{Book, Field, SearchQuery, Topic};
use tulipix_genesis::{GenesisService, download, history};
use tulipix_sec_photos::replace_rows;
use tulipix_ui::*;

/// Settings key for an explicit mirror list, comma-separated. Lives in
/// `Settings.advanced`, which is free-text, so it needs no schema change.
const MIRRORS_KEY: &str = "genesis.mirrors";

/// Settings key for an override download folder. Empty means "the first Books
/// library root", which is what makes the post-download rescan find the file.
const DEST_KEY: &str = "genesis.dest";

/// Ceiling on a single download. Generous, but not unbounded — a mirror that
/// lies about `Content-Length` should not be able to fill the disk.
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// How long the "Added to library" strip stays before it clears itself.
const DONE_LINGER_MS: u64 = 6_000;

/// Progress is reported per 64 KB chunk; at 20 MB/s that is 300 callbacks a
/// second. Throttled to roughly 10 Hz before crossing into the event loop.
const PROGRESS_MS: u128 = 100;

/// The service, parked in an `Option` so a blocking task can take it, own it
/// outright for the duration of a call, and put it back — rather than holding
/// the mutex across the seconds a search or download takes.
fn service() -> &'static Mutex<Option<GenesisService>> {
    static SERVICE: OnceLock<Mutex<Option<GenesisService>>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(None))
}

fn guard() -> MutexGuard<'static, Option<GenesisService>> {
    service().lock().unwrap_or_else(|e| e.into_inner())
}

/// Bumped on every search. A result that lands after a newer search started is
/// dropped rather than overwriting it — the same idea `tulipix-sec-books` uses
/// for its refresh generations.
static SEARCH_GEN: AtomicU64 = AtomicU64::new(0);

/// The file the last download produced, for "Show file". Empty until one lands.
fn last_download() -> MutexGuard<'static, String> {
    static AT: OnceLock<Mutex<String>> = OnceLock::new();
    AT.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Set by Cancel, read by the download loop between chunks.
fn cancel_flag() -> &'static Mutex<Option<Arc<AtomicBool>>> {
    static FLAG: OnceLock<Mutex<Option<Arc<AtomicBool>>>> = OnceLock::new();
    FLAG.get_or_init(|| Mutex::new(None))
}

thread_local! {
    /// Clears the "Added to library" strip. Bound to the thread that started
    /// it, which is always the UI thread here.
    static DONE_TIMER: slint::Timer = slint::Timer::default();
}

// ── entry points ────────────────────────────────────────────────────────────

pub fn wire(window: &MainWindow) {
    set_topics(window, &default_topics());

    let w = window.as_weak();
    window.on_genesis_enter(move || {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_open(true);
        // Resolve the destination now, so the Get buttons are already right
        // rather than going live a moment after the first search.
        refresh_dest(&w);
    });

    let w = window.as_weak();
    window.on_genesis_back(move || {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_open(false);
    });

    let w = window.as_weak();
    window.on_genesis_search(move || {
        let Some(w) = w.upgrade() else { return };
        start_search(&w);
    });

    let w = window.as_weak();
    window.on_genesis_retry(move || {
        let Some(w) = w.upgrade() else { return };
        // A retry after "every mirror failed" must re-probe, or it just replays
        // the same dead pool from memory.
        let _ = with_mut(|svc| svc.refresh_mirrors());
        start_search(&w);
    });

    let w = window.as_weak();
    window.on_genesis_download(move |md5| {
        let Some(w) = w.upgrade() else { return };
        start_download(&w, md5.to_string());
    });

    window.on_genesis_cancel(move || {
        if let Some(flag) = cancel_flag().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    });

    let w = window.as_weak();
    window.on_genesis_toggle_topic(move |code| {
        let Some(w) = w.upgrade() else { return };
        let mut topics = read_topics(&w);
        // `SharedString` on the left: it implements `PartialEq<String>`, and
        // `String` has no matching impl the other way round.
        for t in &mut topics {
            if code == t.code {
                t.on = !t.on;
            }
        }
        // Every topic off returns nothing at all, which reads as a broken
        // search rather than an empty filter. Re-arm the one just cleared.
        if !topics.iter().any(|t| t.on)
            && let Some(t) = topics.iter_mut().find(|t| code == t.code)
        {
            t.on = true;
        }
        set_topics(&w, &topics);
    });

    let w = window.as_weak();
    window.on_genesis_cycle_field(move || {
        let Some(w) = w.upgrade() else { return };
        let next = cycle(&w.get_genesis_field(), FIELDS);
        w.set_genesis_field(next.into());
    });

    let w = window.as_weak();
    window.on_genesis_cycle_format(move || {
        let Some(w) = w.upgrade() else { return };
        let next = cycle(&w.get_genesis_format(), FORMATS);
        w.set_genesis_format(next.into());
    });

    let w = window.as_weak();
    window.on_genesis_cycle_language(move || {
        let Some(w) = w.upgrade() else { return };
        let next = cycle(&w.get_genesis_language(), LANGUAGES);
        w.set_genesis_language(next.into());
    });

    let w = window.as_weak();
    window.on_genesis_open_dest(move || {
        let Some(w) = w.upgrade() else { return };
        // Reveal the file that just landed when there is one — selecting it in
        // a folder of many is more use than opening the folder. Falls back to
        // the folder itself, which is what the idle state points at.
        let last = last_download().clone();
        let file = PathBuf::from(&last);
        if !last.is_empty() && file.exists() {
            if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&file) {
                tracing::warn!(error = %e, "genesis: could not reveal the download");
            }
            return;
        }
        let dir = PathBuf::from(w.get_genesis_dest().to_string());
        if dir.as_os_str().is_empty() {
            return;
        }
        if let Err(e) = tulipix_platform::fm::open_default(&dir) {
            tracing::warn!(error = %e, "genesis: could not open the download folder");
        }
    });
}

// ── search ──────────────────────────────────────────────────────────────────

fn start_search(w: &MainWindow) {
    let terms = w.get_genesis_terms().trim().to_string();
    if terms.is_empty() {
        return;
    }

    let query = build_query(w, &terms);
    let seq = SEARCH_GEN.fetch_add(1, Ordering::Relaxed) + 1;

    // A cold pool means a real probe of every seed mirror, which is the slow
    // path worth naming; a warm one goes straight to the query.
    let warm = with(|svc| svc.has_pool()).unwrap_or(false);
    w.set_genesis_phase(if warm { "searching".into() } else { "probing".into() });
    w.set_genesis_status("".into());
    w.set_genesis_error("".into());

    let weak = w.as_weak();
    let status_weak = w.as_weak();
    spawn_blocking(move || {
        // Progress from inside the mirror probe. Dropped if a newer search has
        // already started, so a slow probe cannot narrate over a fresh query.
        let on_status = move |line: &str| {
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            let line = line.to_string();
            let _ = status_weak.upgrade_in_event_loop(move |w| {
                w.set_genesis_status(line.into());
            });
        };

        let dest = resolve_dest();
        let result = with_mut(|svc| {
            svc.set_mirrors(configured_mirrors());
            svc.set_dest(dest);
            svc.search(&query, &on_status)
        });

        let _ = weak.upgrade_in_event_loop(move |w| {
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            match result {
                Some(Ok(served)) => {
                    w.set_genesis_mirror(served.mirror.clone().into());
                    w.set_genesis_mirror_tone("ok".into());
                    w.set_genesis_status("".into());
                    w.set_genesis_phase("ready".into());
                    fill_rows(&w, served.value);
                }
                Some(Err(e)) => {
                    w.set_genesis_error(e.to_string().into());
                    w.set_genesis_mirror_tone("bad".into());
                    w.set_genesis_phase("failed".into());
                }
                // Only when the service could not be built at all.
                None => {
                    w.set_genesis_error("could not start the search client".into());
                    w.set_genesis_phase("failed".into());
                }
            }
        });
    });
}

/// Push the rows, marking the ones already downloaded.
///
/// The history lookup is one query for the whole page rather than one per row,
/// which would be twenty-five round trips to paint one table.
fn fill_rows(w: &MainWindow, books: Vec<Book>) {
    let md5s: Vec<String> = books.iter().map(|b| b.md5.clone()).collect();
    let weak = w.as_weak();
    spawn(async move {
        let known = match history::open().await {
            Ok(pool) => history::known(&pool, &md5s).await.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "genesis: could not open the history database");
                Vec::new()
            }
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<GenesisRow> = books
                .iter()
                .map(|b| GenesisRow {
                    md5: b.md5.clone().into(),
                    title: b.title.clone().into(),
                    author: b.authors_or_unknown().to_string().into(),
                    year: b.year.clone().unwrap_or_default().into(),
                    language: b.language.clone().unwrap_or_default().into(),
                    pages: b.pages.clone().unwrap_or_default().into(),
                    size: b.size_human().into(),
                    format: b.ext().to_uppercase().into(),
                    have: known.contains(&b.md5),
                })
                .collect();
            set_rows(&w.get_genesis_rows(), rows, |rows| w.set_genesis_rows(rows));
            stash(books);
        });
    });
}

/// The last result set, kept so a Get click can name a full [`Book`] rather
/// than re-searching to find the row the user pressed.
fn results() -> MutexGuard<'static, Vec<Book>> {
    static ROWS: OnceLock<Mutex<Vec<Book>>> = OnceLock::new();
    ROWS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn stash(books: Vec<Book>) {
    *results() = books;
}

// ── download ────────────────────────────────────────────────────────────────

fn start_download(w: &MainWindow, md5: String) {
    let Some(book) = results().iter().find(|b| b.md5 == md5).cloned() else {
        return;
    };
    let dest = resolve_dest();
    if dest.as_os_str().is_empty() {
        return;
    }

    let flag = Arc::new(AtomicBool::new(false));
    *cancel_flag().lock().unwrap_or_else(|e| e.into_inner()) = Some(flag.clone());

    w.set_genesis_busy(true);
    w.set_genesis_busy_name(book.title.clone().into());
    w.set_genesis_busy_pct(0.0);
    w.set_genesis_busy_detail("starting…".into());
    w.set_genesis_done_name("".into());

    let opts = download::Options {
        dest_dir: dest,
        filename: None,
        max_bytes: MAX_BYTES,
        // MD5 is both the download key and the integrity check, so verifying is
        // free. Never exposed as a toggle.
        verify: true,
        force: false,
        resume: true,
        cancel: Some(flag),
    };

    let weak = w.as_weak();
    let progress_weak = w.as_weak();
    let status_weak = w.as_weak();
    spawn_blocking(move || {
        let mut last = std::time::Instant::now();
        let mut report = move |done: u64, total: Option<u64>| {
            if last.elapsed().as_millis() < PROGRESS_MS {
                return;
            }
            last = std::time::Instant::now();
            let pct = match total {
                Some(t) if t > 0 => (done as f32 / t as f32).min(1.0),
                _ => 0.0,
            };
            let detail = match total {
                Some(t) if t > 0 => format!("{} / {}", human(done), human(t)),
                _ => human(done),
            };
            let _ = progress_weak.upgrade_in_event_loop(move |w| {
                w.set_genesis_busy_pct(pct);
                w.set_genesis_busy_detail(detail.into());
            });
        };

        let on_status = move |line: &str| {
            let line = line.to_string();
            let _ = status_weak.upgrade_in_event_loop(move |w| {
                w.set_genesis_busy_detail(line.into());
            });
        };

        let result = with_mut(|svc| svc.download(&book, &opts, &mut report, &on_status));

        match result {
            Some(Ok(served)) => {
                let path = served.value.path.display().to_string();
                let bytes = served.value.bytes;
                let verified = served.value.verified;
                finish_download(weak, book, path, served.mirror, bytes, verified);
            }
            Some(Err(e)) => {
                let msg = e.to_string();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_genesis_busy(false);
                    // A cancel is a choice, not a failure — it does not deserve
                    // the error banner.
                    if !msg.contains("cancelled") {
                        w.set_genesis_error(msg.into());
                        w.set_genesis_phase("failed".into());
                    }
                });
            }
            None => {
                let _ = weak.upgrade_in_event_loop(|w| w.set_genesis_busy(false));
            }
        }
    });
}

/// Record the download, rescan the library, then show the confirmation.
fn finish_download(
    weak: slint::Weak<MainWindow>,
    book: Book,
    path: String,
    mirror: String,
    bytes: u64,
    verified: bool,
) {
    spawn(async move {
        if let Ok(pool) = history::open().await
            && let Err(e) = history::record(&pool, &book, &path, &mirror, now_secs()).await
        {
            tracing::warn!(error = %e, "genesis: could not record the download");
        }

        // The file landed inside a watched library root, so the ordinary scan
        // is what picks it up — no separate ingest path to keep in step.
        let scanned = match tulipix_common::pool_for("books").await {
            Ok(books) => tulipix_books::scan::scan_all(&books).await.is_ok(),
            Err(e) => {
                tracing::warn!(error = %e, "genesis: could not open the books database");
                false
            }
        };

        let name = std::path::Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| book.title.clone());

        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_genesis_busy(false);
            w.set_genesis_done_name(name.into());
            w.set_genesis_done_detail(
                format!(
                    "{} · {} · from {mirror}",
                    human(bytes),
                    if verified { "MD5 verified" } else { "not verified" }
                )
                .into(),
            );
            // `genesis-dest` stays the folder — it is what the idle state names
            // as the download location. The file path is remembered separately
            // so "Show file" can reveal the file itself.
            *last_download() = path;

            // Mark the row so the button becomes "In library" without a
            // re-search.
            mark_have(&w, &book.md5);

            if scanned {
                tulipix_sec_books::books_refresh(w.as_weak(), 0);
            }

            let weak = w.as_weak();
            DONE_TIMER.with(|t| {
                t.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::from_millis(DONE_LINGER_MS),
                    move || {
                        if let Some(w) = weak.upgrade() {
                            w.set_genesis_done_name("".into());
                        }
                    },
                )
            });
        });
    });
}

fn mark_have(w: &MainWindow, md5: &str) {
    let rows: Vec<GenesisRow> = w
        .get_genesis_rows()
        .iter()
        .map(|r| {
            let mut r = r.clone();
            if r.md5 == md5 {
                r.have = true;
            }
            r
        })
        .collect();
    set_rows(&w.get_genesis_rows(), rows, |rows| w.set_genesis_rows(rows));
}

// ── query building ──────────────────────────────────────────────────────────

const FIELDS: &[&str] = &["Any", "Title", "Author", "Series", "Publisher", "Year", "ISBN"];
const FORMATS: &[&str] = &["Any", "epub", "pdf", "mobi", "azw3", "djvu", "cbz"];
const LANGUAGES: &[&str] = &["Any", "English", "German", "French", "Spanish", "Russian", "Italian"];

/// Next value in a fixed list, wrapping. Cheaper than a popup menu for a
/// handful of options, and it needs no positioning logic.
fn cycle(current: &str, options: &[&str]) -> String {
    let at = options.iter().position(|o| *o == current).unwrap_or(0);
    options[(at + 1) % options.len()].to_string()
}

fn build_query(w: &MainWindow, terms: &str) -> SearchQuery {
    let mut q = SearchQuery::new(terms);

    q.topics = read_topics(w)
        .iter()
        .filter(|t| t.on)
        .filter_map(|t| Topic::parse(&t.code))
        .collect();

    q.fields = match w.get_genesis_field().as_str() {
        "Title" => vec![Field::Title],
        "Author" => vec![Field::Author],
        "Series" => vec![Field::Series],
        "Publisher" => vec![Field::Publisher],
        "Year" => vec![Field::Year],
        "ISBN" => vec![Field::Isbn],
        // "Any" — an empty list means the mirror matches every column.
        _ => Vec::new(),
    };

    q.extension = match w.get_genesis_format().as_str() {
        "Any" => None,
        other => Some(other.to_ascii_lowercase()),
    };
    q.language = match w.get_genesis_language().as_str() {
        "Any" => None,
        other => Some(other.to_string()),
    };
    q.limit = 50;
    q
}

struct TopicState {
    code: String,
    label: String,
    on: bool,
}

/// Sci-Tech and Fiction on by default — between them they are most of what
/// anyone searches for, and every topic on makes every search slower.
fn default_topics() -> Vec<TopicState> {
    Topic::ALL
        .iter()
        .map(|t| TopicState {
            code: t.name().to_string(),
            label: match t.name() {
                "libgen" => "Sci-Tech".to_string(),
                other => {
                    let mut c = other.chars();
                    match c.next() {
                        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                }
            },
            on: matches!(t, Topic::Libgen | Topic::Fiction),
        })
        .collect()
}

fn read_topics(w: &MainWindow) -> Vec<TopicState> {
    w.get_genesis_topics()
        .iter()
        .map(|t| TopicState {
            code: t.code.to_string(),
            label: t.label.to_string(),
            on: t.on,
        })
        .collect()
}

fn set_topics(w: &MainWindow, topics: &[TopicState]) {
    let rows: Vec<GenesisTopic> = topics
        .iter()
        .map(|t| GenesisTopic {
            code: t.code.clone().into(),
            label: t.label.clone().into(),
            on: t.on,
        })
        .collect();
    set_rows(&w.get_genesis_topics(), rows, |rows| w.set_genesis_topics(rows));
}

// ── destination ─────────────────────────────────────────────────────────────

/// Where downloads land: the `genesis.dest` override, else `<Downloads>/Genesis`.
///
/// Never an existing library folder. A shelf the user arranged and a pile of
/// fetched files are different things, and once they are mixed there is no way
/// to separate them again. The app creates its own folder and registers that as
/// a library root instead, so the books still appear in My Library without
/// anything being written into a curated folder.
fn resolve_dest() -> PathBuf {
    let stored = advanced(DEST_KEY);
    if !stored.trim().is_empty() {
        return PathBuf::from(stored);
    }
    tulipix_genesis::fsutil::default_dest()
}

/// Make sure the folder exists and the library is watching it.
///
/// Both are idempotent — `create_dir_all` on an existing directory is a no-op
/// and `add_folder` is an `INSERT OR IGNORE` — so this runs on every entry into
/// the section rather than needing a "have I done this yet" flag. That also
/// means deleting the folder by hand heals itself on the next visit.
async fn ensure_dest_registered(dest: &std::path::Path) -> bool {
    if let Err(e) = std::fs::create_dir_all(dest) {
        tracing::warn!(error = %e, path = %dest.display(), "genesis: could not create the download folder");
        return false;
    }
    match tulipix_common::pool_for("books").await {
        Ok(pool) => {
            if let Err(e) =
                tulipix_books::scan::add_folder(&pool, &dest.display().to_string()).await
            {
                tracing::warn!(error = %e, "genesis: could not register the download folder");
            }
            true
        }
        Err(e) => {
            // The folder exists, so downloading still works — the book just
            // will not appear in My Library until the database comes back.
            tracing::warn!(error = %e, "genesis: books database unavailable");
            true
        }
    }
}

fn refresh_dest(w: &MainWindow) {
    let dest = resolve_dest();
    let weak = w.as_weak();
    spawn(async move {
        let ok = ensure_dest_registered(&dest).await;
        let shown = dest.display().to_string();
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_genesis_can_download(ok);
            w.set_genesis_dest(shown.into());
        });
    });
}

fn advanced(key: &str) -> String {
    tulipix_core::settings::Settings::load()
        .ok()
        .and_then(|s| s.advanced.get(key).cloned())
        .unwrap_or_default()
}

fn configured_mirrors() -> Vec<String> {
    advanced(MIRRORS_KEY)
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

// ── plumbing ────────────────────────────────────────────────────────────────

/// Run `f` against the service, building it on first use.
///
/// Blocking, and the mutex is held for the whole call — which is why every
/// caller is already inside `spawn_blocking`.
fn with_mut<R>(f: impl FnOnce(&mut GenesisService) -> R) -> Option<R> {
    let mut slot = guard();
    if slot.is_none() {
        match GenesisService::new(configured_mirrors(), resolve_dest()) {
            Ok(svc) => *slot = Some(svc),
            Err(e) => {
                tracing::warn!(error = %e, "genesis: could not build the client");
                return None;
            }
        }
    }
    slot.as_mut().map(f)
}

fn with<R>(f: impl FnOnce(&GenesisService) -> R) -> Option<R> {
    guard().as_ref().map(f)
}

fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
        }
        Err(_) => tracing::warn!("genesis: no tokio runtime; not running"),
    }
}

fn spawn_blocking<F: FnOnce() + Send + 'static>(f: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn_blocking(f);
        }
        Err(_) => tracing::warn!("genesis: no tokio runtime; not running"),
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

fn human(bytes: u64) -> String {
    tulipix_genesis::model::human_bytes(bytes)
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycling_wraps_and_starts_over_from_an_unknown_value() {
        assert_eq!(cycle("Any", FIELDS), "Title");
        assert_eq!(cycle("ISBN", FIELDS), "Any", "the last option wraps");
        // An unrecognised value restarts rather than sticking.
        assert_eq!(cycle("nonsense", FIELDS), "Title");
    }

    #[test]
    fn the_default_topics_are_the_two_worth_searching() {
        let on: Vec<&str> = default_topics()
            .iter()
            .filter(|t| t.on)
            .map(|t| t.code.as_str())
            .collect();
        assert_eq!(on, ["libgen", "fiction"]);
    }

    #[test]
    fn sci_tech_is_relabelled_and_the_rest_are_capitalised() {
        let labels: Vec<String> = default_topics().into_iter().map(|t| t.label).collect();
        // "libgen" means nothing to a reader; the others just need a capital.
        assert!(labels.contains(&"Sci-Tech".to_string()));
        assert!(labels.contains(&"Fiction".to_string()));
        assert!(labels.contains(&"Magazines".to_string()));
    }
}
