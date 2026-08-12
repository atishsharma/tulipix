//! Genesis-section wiring: the bridge between [`tulipix_genesis`] — which is
//! blocking and knows nothing about Slint — and every `window.on_genesis_*`
//! callback.
//!
//! The core stays blocking on purpose (see that crate's docs), so every call
//! into it goes through `spawn_blocking` and comes back to the UI via
//! `upgrade_in_event_loop`. Nothing here holds a lock across an await.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use slint::{ComponentHandle, Model, ModelRc, VecModel};
use tulipix_genesis::model::{Book, Field, SearchQuery, Topic};
use tulipix_genesis::{GenesisService, details, download, history};
use tulipix_sec_photos::replace_rows;
use tulipix_ui::*;

/// Settings key for an explicit mirror list, comma-separated. Lives in
/// `Settings.advanced`, which is free-text, so it needs no schema change.
const MIRRORS_KEY: &str = "genesis.mirrors";

/// Settings key for how many results one search asks the mirror for.
const LIMIT_KEY: &str = "genesis.limit";

/// Results per search when nothing is configured, and the ceiling the slider
/// allows. Mirrors serve fixed page sizes and round 49 up to 50, so asking for
/// more than that buys a second request rather than more books.
const DEFAULT_LIMIT: i32 = 21;
const MAX_LIMIT: i32 = 49;

/// What the filters start on. Both are client-side, applied after the mirror
/// answers, so they are a preference rather than part of the query — set once
/// on entry and left alone after that, because the alternative is resetting a
/// choice someone made two searches ago.
const DEFAULT_FORMAT: &str = "epub";
const DEFAULT_LANGUAGE: &str = "English";

/// Past searches kept for the history popup: ten a page, ten pages.
const HISTORY_MAX: i64 = 100;

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

/// How long the "Saved" confirmation on the settings dialog stays.
const SAVED_LINGER_MS: u64 = 2_000;

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
    /// Clears the settings dialog's "Saved ✓".
    static SAVED_TIMER: slint::Timer = slint::Timer::default();
    /// Puts the Search button back after a search replayed from history.
    static REPLAY_TIMER: slint::Timer = slint::Timer::default();
}

/// How long the replay pill stays at full after its results land — without it
/// the fill vanishes at the moment it completes, which reads as an interruption
/// rather than as a finish.
const REPLAY_LINGER_MS: u64 = 700;

/// Which column the result table is ordered by, and which way.
///
/// Kept here rather than on the window because sorting runs over the whole
/// result set, which the window then shows in one list. Empty key = the order
/// the mirror returned, which is its own relevance ranking and the only
/// ordering that is not arbitrary.
fn sort_state() -> MutexGuard<'static, (String, bool)> {
    static S: OnceLock<Mutex<(String, bool)>> = OnceLock::new();
    S.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

// ── entry points ────────────────────────────────────────────────────────────

pub fn wire(window: &MainWindow) {
    set_topics(window, &default_topics());
    window.set_genesis_limit(configured_limit());
    window.set_genesis_format(DEFAULT_FORMAT.into());
    window.set_genesis_language(DEFAULT_LANGUAGE.into());

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
        w.set_genesis_replay(false);
        start_search(&w);
    });

    let w = window.as_weak();
    window.on_genesis_retry(move || {
        let Some(w) = w.upgrade() else { return };
        // A retry after "every mirror failed" must re-probe, or it just replays
        // the same dead pool from memory.
        let _ = with_mut(|svc| svc.refresh_mirrors());
        w.set_genesis_replay(false);
        start_search(&w);
    });

    // Throw the page away: results, words, mirror status and all. Confirmed in
    // the UI first — this is the one control on the page that destroys work.
    let w = window.as_weak();
    window.on_genesis_clear(move || {
        let Some(w) = w.upgrade() else { return };
        // Abandons anything still in flight, including the cover pass, which
        // would otherwise keep writing into rows that are no longer shown.
        SEARCH_GEN.fetch_add(1, Ordering::Relaxed);
        stash(Vec::new(), Vec::new());
        *paging() = Paging::default();
        *sort_state() = (String::new(), false);
        w.set_genesis_more_busy(false);
        w.set_genesis_more_done(false);
        w.set_genesis_terms("".into());
        w.set_genesis_phase("idle".into());
        w.set_genesis_status("".into());
        w.set_genesis_error("".into());
        w.set_genesis_done_name("".into());
        w.set_genesis_replay(false);
        push_rows(&w);
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

    // The three filter dropdowns. The value comes back from the menu, so these
    // only have to store it — no cycling, no wrapping.
    let w = window.as_weak();
    window.on_genesis_set_field(move |value| {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_field(value);
    });

    let w = window.as_weak();
    window.on_genesis_set_format(move |value| {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_format(value);
    });

    let w = window.as_weak();
    window.on_genesis_set_language(move |value| {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_language(value);
    });

    let w = window.as_weak();
    window.on_genesis_load_more(move || {
        let Some(w) = w.upgrade() else { return };
        load_more(&w);
    });

    // ── history ─────────────────────────────────────────────────────────────

    let w = window.as_weak();
    window.on_genesis_history_opened(move || {
        let Some(w) = w.upgrade() else { return };
        load_history(&w);
    });

    let w = window.as_weak();
    window.on_genesis_history_run(move |entry| {
        let Some(w) = w.upgrade() else { return };
        // Restore the filters the search ran under, not just its words: the
        // same terms with a different format filter is a different search, and
        // running it under whatever happens to be selected now would not be
        // the search being clicked on.
        w.set_genesis_terms(entry.terms);
        w.set_genesis_field(entry.field);
        w.set_genesis_format(entry.format);
        w.set_genesis_language(entry.language);
        w.set_genesis_history_open(false);
        // Turns the Search button into a filling pill, so the results that
        // appear are visibly the old search running again rather than something
        // the page decided to show on its own.
        w.set_genesis_replay(true);
        start_search(&w);
    });

    let w = window.as_weak();
    window.on_genesis_history_clear(move || {
        let Some(w) = w.upgrade() else { return };
        let weak = w.as_weak();
        spawn(async move {
            if let Ok(pool) = history::open().await
                && let Err(e) = history::clear_searches(&pool).await
            {
                tracing::warn!(error = %e, "genesis: could not clear the search history");
            }
            let _ = weak.upgrade_in_event_loop(|w| load_history(&w));
        });
    });

    // ── record details ──────────────────────────────────────────────────────

    let w = window.as_weak();
    window.on_genesis_open_details(move |md5| {
        let Some(w) = w.upgrade() else { return };
        start_details(&w, md5.to_string());
    });

    let w = window.as_weak();
    window.on_genesis_close_details(move || {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_details_open(false);
    });

    // ── results per search ──────────────────────────────────────────────────

    let w = window.as_weak();
    window.on_genesis_set_limit(move |value| {
        let Some(w) = w.upgrade() else { return };
        let value = value.clamp(1, MAX_LIMIT);
        w.set_genesis_limit(value);
        set_advanced(LIMIT_KEY, &value.to_string());
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

    // ── result table ────────────────────────────────────────────────────────

    let w = window.as_weak();
    window.on_genesis_sort_clicked(move |key| {
        let Some(w) = w.upgrade() else { return };
        {
            let mut s = sort_state();
            // Same column again flips direction; a different one starts
            // ascending, which is what every table on the desktop does.
            if s.0 == key.as_str() {
                s.1 = !s.1;
            } else {
                *s = (key.to_string(), false);
            }
        }
        push_rows(&w);
    });

    // ── settings ────────────────────────────────────────────────────────────

    let w = window.as_weak();
    window.on_genesis_settings_opened(move || {
        let Some(w) = w.upgrade() else { return };
        w.set_genesis_mirrors(advanced(MIRRORS_KEY).replace(',', "\n").into());
        w.set_genesis_mirrors_default(tulipix_genesis::mirror::SEED_MIRRORS.join("\n").into());
        w.set_genesis_settings_saved(false);
        refresh_dest(&w);
    });

    let w = window.as_weak();
    window.on_genesis_settings_save(move || {
        let Some(w) = w.upgrade() else { return };
        // Stored comma-separated because `Settings.advanced` is a flat
        // string map; the box is newline-per-entry because that is how a list
        // is edited.
        let list = w
            .get_genesis_mirrors()
            .split(['\n', ','])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(",");
        set_advanced(MIRRORS_KEY, &list);
        // The pool in memory was resolved from the old list, so it has to go or
        // the next search silently ignores the edit.
        let _ = with_mut(|svc| svc.refresh_mirrors());
        flash_saved(&w);
    });

    let w = window.as_weak();
    window.on_genesis_settings_reset(move || {
        let Some(w) = w.upgrade() else { return };
        set_advanced(MIRRORS_KEY, "");
        set_advanced(DEST_KEY, "");
        w.set_genesis_mirrors("".into());
        let _ = with_mut(|svc| svc.refresh_mirrors());
        refresh_dest(&w);
        flash_saved(&w);
    });

    let w = window.as_weak();
    window.on_genesis_pick_dest(move || {
        let start = resolve_dest();
        let weak = w.clone();
        spawn(async move {
            let Some(dir) = rfd::AsyncFileDialog::new()
                .set_title("Where should Genesis downloads land?")
                .set_directory(&start)
                .pick_folder()
                .await
            else {
                return;
            };
            let path = dir.path().display().to_string();
            set_advanced(DEST_KEY, &path);
            let _ = with_mut(|svc| svc.set_dest(PathBuf::from(&path)));
            let _ = weak.upgrade_in_event_loop(move |w| {
                // Registers the new folder as a library root, which is what
                // makes a book downloaded into it appear in My Library.
                refresh_dest(&w);
                flash_saved(&w);
            });
        });
    });
}

/// Let the replay pill finish filling, then put the Search button back.
///
/// A no-op unless a search was actually replayed from history, so the ordinary
/// path never arms a timer.
fn end_replay(w: &MainWindow) {
    if !w.get_genesis_replay() {
        return;
    }
    let weak = w.as_weak();
    REPLAY_TIMER.with(|t| {
        t.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(REPLAY_LINGER_MS),
            move || {
                if let Some(w) = weak.upgrade() {
                    w.set_genesis_replay(false);
                }
            },
        )
    });
}

/// Show "Saved ✓" on the settings dialog, then put the button back.
fn flash_saved(w: &MainWindow) {
    w.set_genesis_settings_saved(true);
    let weak = w.as_weak();
    SAVED_TIMER.with(|t| {
        t.start(
            slint::TimerMode::SingleShot,
            std::time::Duration::from_millis(SAVED_LINGER_MS),
            move || {
                if let Some(w) = weak.upgrade() {
                    w.set_genesis_settings_saved(false);
                }
            },
        )
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

    // A fresh search starts paging over: page one, nothing revealed yet, and
    // the query kept so "load more" asks for the next page of *this* search
    // rather than of whatever is in the box by then.
    *paging() = Paging { query: Some(query.clone()), page: 1, shown: 0, exhausted: false };
    w.set_genesis_more_busy(false);
    w.set_genesis_more_done(false);

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
            end_replay(&w);
            match result {
                Some(Ok(served)) => {
                    w.set_genesis_mirror(served.mirror.clone().into());
                    w.set_genesis_active_url(served.url.clone().into());
                    w.set_genesis_mirror_tone("ok".into());
                    w.set_genesis_status("".into());
                    w.set_genesis_phase("ready".into());
                    remember_search(&w, &terms, served.value.len() as i64);
                    fill_rows(&w, served.value, seq);
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

/// Stash the result set, mark what is already downloaded, and paint the table.
///
/// The history lookup is one query for the whole result set rather than one per
/// row, which would be fifty round trips to paint one table.
fn fill_rows(w: &MainWindow, books: Vec<Book>, seq: u64) {
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
            stash(books, known);
            // A new search shows the mirror's own order. Any other choice means
            // the first thing shown is the new results sorted by a column
            // someone clicked during the previous search and long forgot.
            *sort_state() = (String::new(), false);
            reveal(&w, configured_limit().max(1) as usize, seq);
        });
    });
}

/// Put another `step` of the fetched records on screen.
///
/// Covers are fetched for exactly the rows this revealed. A mirror page can
/// hold a hundred records and only a screenful is ever shown at once, so
/// fetching art for all of them would be a hundred record-page requests for
/// books nobody has looked at.
fn reveal(w: &MainWindow, step: usize, seq: u64) {
    let total = results().0.len();
    let from = paging().shown;
    let to = (from + step).min(total);
    paging().shown = to;

    push_rows(w);
    if to > from {
        let fresh: Vec<Book> = results().0[from..to].to_vec();
        // Rows first, covers second: a cover lands in its row by MD5, and one
        // arriving before the row exists would find nothing to fill.
        fetch_covers(w, fresh, seq);
    }
    update_more(w);
}

/// Whether "Load more" still has anywhere to go: records fetched but not shown,
/// or a mirror page that has not been asked for yet.
fn update_more(w: &MainWindow) {
    let total = results().0.len();
    let p = paging();
    w.set_genesis_more_done(p.shown >= total && p.exhausted);
}

/// Show more results, from what is already in hand where possible.
fn load_more(w: &MainWindow) {
    if w.get_genesis_more_busy() {
        return;
    }
    let step = configured_limit().max(1) as usize;
    let seq = SEARCH_GEN.load(Ordering::Relaxed);

    // Spend the over-fetch first. No network, so this is instant.
    let (shown, total) = (paging().shown, results().0.len());
    if shown < total {
        reveal(w, step, seq);
        return;
    }

    let (query, page, exhausted) = {
        let p = paging();
        (p.query.clone(), p.page, p.exhausted)
    };
    if exhausted {
        w.set_genesis_more_done(true);
        return;
    }
    let Some(mut query) = query else { return };
    query.page = page + 1;

    w.set_genesis_more_busy(true);
    w.set_genesis_status("".into());
    let weak = w.as_weak();
    spawn_blocking(move || {
        let result = with_mut(|svc| svc.search(&query, &|_| {}));
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_genesis_more_busy(false);
            // A newer search started while this page was in flight; its results
            // are on screen and these belong to a query nobody is looking at.
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            match result {
                Some(Ok(served)) => {
                    let fresh = append(served.value);
                    {
                        let mut p = paging();
                        p.page += 1;
                        // Mirrors repeat the last page rather than 404 once you
                        // walk off the end, so "nothing new" is the only honest
                        // end-of-results signal there is.
                        p.exhausted = fresh.is_empty();
                    }
                    mark_known(&w, fresh.iter().map(|b| b.md5.clone()).collect());
                    reveal(&w, step, seq);
                }
                Some(Err(e)) => {
                    // The results already on screen are still good, so this
                    // says what happened and leaves the button up to try again
                    // — the page is not torn down over one extra page failing.
                    w.set_genesis_status(e.to_string().into());
                }
                None => w.set_genesis_more_done(true),
            }
        });
    });
}

/// What a "load more" needs to know.
///
/// A mirror page holds 25, 50 or 100 records and the format and language
/// filters run after it arrives, so one request usually carries several
/// screens' worth. Those are spent before the mirror is asked for another page.
#[derive(Default)]
struct Paging {
    /// The query the results in hand came from, so the next page is the same
    /// search rather than whatever is in the search box now.
    query: Option<SearchQuery>,
    /// Last mirror page fetched, one-based.
    page: usize,
    /// How many of the fetched records are on screen.
    shown: usize,
    /// A page came back with nothing new in it — there is no more to load.
    exhausted: bool,
}

fn paging() -> MutexGuard<'static, Paging> {
    static P: OnceLock<Mutex<Paging>> = OnceLock::new();
    P.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// The last result set, kept so a Get click can name a full [`Book`] rather
/// than re-searching to find the row the user pressed — and so a sort never
/// needs the mirror again.
fn results() -> MutexGuard<'static, (Vec<Book>, Vec<String>)> {
    static ROWS: OnceLock<Mutex<(Vec<Book>, Vec<String>)>> = OnceLock::new();
    ROWS.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

fn stash(books: Vec<Book>, known: Vec<String>) {
    *results() = (books, known);
}

/// Add records that are not already in hand, and hand back the new ones.
///
/// Mirrors overlap their pages, and a repeat would otherwise show up as a
/// duplicate card with the same MD5 — and be counted as progress.
fn append(books: Vec<Book>) -> Vec<Book> {
    let mut g = results();
    let fresh: Vec<Book> =
        books.into_iter().filter(|b| !g.0.iter().any(|h| h.md5 == b.md5)).collect();
    g.0.extend(fresh.iter().cloned());
    fresh
}

/// Fill in the "already downloaded" mark for freshly appended records.
///
/// One query for the whole batch, the same as a first search does — the flag is
/// what turns a card's Download pill into "In library".
fn mark_known(w: &MainWindow, md5s: Vec<String>) {
    if md5s.is_empty() {
        return;
    }
    let weak = w.as_weak();
    spawn(async move {
        let Ok(pool) = history::open().await else { return };
        let Ok(known) = history::known(&pool, &md5s).await else { return };
        if known.is_empty() {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            results().1.extend(known);
            push_rows(&w);
        });
    });
}

/// Order what has been revealed so far and push it.
///
/// The cut is taken in the mirror's own order — its relevance ranking — and the
/// sort is applied to what that yields. The other way round, changing the sort
/// would change *which* books are on screen, so ordering by title would quietly
/// swap the results for a different set of them.
fn push_rows(w: &MainWindow) {
    let (key, desc) = sort_state().clone();
    let shown = paging().shown;
    let (books, known) = {
        let g = results();
        (g.0.clone(), g.1.clone())
    };

    let mut ordered: Vec<&Book> = books.iter().take(shown).collect();
    if !key.is_empty() {
        ordered.sort_by(|a, b| {
            let ord = match key.as_str() {
                "author" => text(a.authors_or_unknown()).cmp(&text(b.authors_or_unknown())),
                // Year and pages are strings on the record because mirrors put
                // anything in those columns; a numeric compare on "1998" and
                // "200?" has to fall back to text rather than to zero.
                "year" => number(a.year.as_deref()).cmp(&number(b.year.as_deref())),
                "language" => text(a.language.as_deref().unwrap_or("")).cmp(&text(b.language.as_deref().unwrap_or(""))),
                "pages" => number(a.pages.as_deref()).cmp(&number(b.pages.as_deref())),
                "size" => a.size_bytes.unwrap_or(0).cmp(&b.size_bytes.unwrap_or(0)),
                "format" => a.ext().cmp(b.ext()),
                _ => text(&a.title).cmp(&text(&b.title)),
            };
            // Titles break every tie, so an ordering is stable across repeated
            // clicks instead of reshuffling equal rows.
            let ord = ord.then_with(|| text(&a.title).cmp(&text(&b.title)));
            if desc { ord.reverse() } else { ord }
        });
    }

    let total = ordered.len();
    let rows: Vec<GenesisRow> = ordered
        .into_iter()
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
            // Whatever a past search already fetched shows immediately; the
            // rest arrive as the cover pass works through them.
            cover: cover_image(&b.md5),
        })
        .collect();

    set_rows(&w.get_genesis_rows(), rows, |rows| w.set_genesis_rows(rows));
    w.set_genesis_total(total as i32);
    w.set_genesis_sort(key.into());
    w.set_genesis_sort_desc(desc);
}

// ── search history ──────────────────────────────────────────────────────────

/// Record a search so it can be run again from the history popup.
///
/// Fire and forget: failing to remember a search is not a reason to interrupt
/// someone who is looking at its results.
fn remember_search(w: &MainWindow, terms: &str, results: i64) {
    let (terms, field, format, language) = (
        terms.to_string(),
        w.get_genesis_field().to_string(),
        w.get_genesis_format().to_string(),
        w.get_genesis_language().to_string(),
    );
    let at = now_secs();
    spawn(async move {
        let Ok(pool) = history::open().await else { return };
        if let Err(e) =
            history::record_search(&pool, &terms, &field, &format, &language, results, at).await
        {
            tracing::warn!(error = %e, "genesis: could not record the search");
        }
    });
}

/// Load the recent searches into the popup's model.
fn load_history(w: &MainWindow) {
    let weak = w.as_weak();
    spawn(async move {
        let list = match history::open().await {
            Ok(pool) => history::recent_searches(&pool, HISTORY_MAX).await.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "genesis: could not read the search history");
                Vec::new()
            }
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<GenesisSearch> = list
                .into_iter()
                .map(|s| GenesisSearch {
                    terms: s.terms.into(),
                    field: s.field.into(),
                    format: s.format.into(),
                    language: s.language.into(),
                    results: s.results as i32,
                    when: ago(s.at).into(),
                })
                .collect();
            set_rows(&w.get_genesis_history(), rows, |rows| w.set_genesis_history(rows));
        });
    });
}

/// A coarse "how long ago", which is all a history row needs — the exact second
/// someone ran a search is never the question being asked.
fn ago(at: i64) -> String {
    let secs = (now_secs() - at).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

// ── record details ──────────────────────────────────────────────────────────

/// Open the details popup for one record.
///
/// The cache is consulted first and, when it answers, nothing touches the
/// network — which is the point of storing it. A miss fetches the record page
/// once and writes it back.
fn start_details(w: &MainWindow, md5: String) {
    let Some(book) = results().0.iter().find(|b| b.md5 == md5).cloned() else {
        return;
    };

    w.set_genesis_details_open(true);
    w.set_genesis_details_error("".into());
    // Seed from the row already on screen, so the popup opens with the title
    // and author filled in rather than blank while the page is fetched.
    w.set_genesis_details(GenesisDetails {
        md5: book.md5.clone().into(),
        title: book.title.clone().into(),
        authors: book.authors_or_unknown().to_string().into(),
        year: book.year.clone().unwrap_or_default().into(),
        language: book.language.clone().unwrap_or_default().into(),
        pages: book.pages.clone().unwrap_or_default().into(),
        size: book.size_human().into(),
        extension: book.ext().to_uppercase().into(),
        cover: cover_image(&book.md5),
        ..Default::default()
    });

    let weak = w.as_weak();
    spawn(async move {
        // 1. The cache.
        if let Ok(pool) = history::open().await
            && let Ok(Some(cached)) = history::cached_details(&pool, &md5).await
        {
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_genesis_details_loading(false);
                push_details(&w, &cached);
            });
            return;
        }

        // 2. The mirror.
        let _ = weak.upgrade_in_event_loop(|w| w.set_genesis_details_loading(true));
        let fetch_weak = weak.clone();
        spawn_blocking(move || {
            let fetched = with_mut(|svc| svc.details(&book, &|_| {}));
            let _ = fetch_weak.upgrade_in_event_loop(move |w| {
                w.set_genesis_details_loading(false);
                match fetched {
                    Some(Ok(served)) => {
                        push_details(&w, &served.value);
                        cache_details(served.value);
                    }
                    Some(Err(e)) => w.set_genesis_details_error(e.to_string().into()),
                    None => w
                        .set_genesis_details_error("could not start the search client".into()),
                }
            });
        });
    });
}

/// Write a fetched record page into the popup.
fn push_details(w: &MainWindow, d: &details::Details) {
    w.set_genesis_details(GenesisDetails {
        md5: d.md5.clone().into(),
        title: d.title.clone().into(),
        series: d.series.clone().into(),
        authors: d.authors.clone().into(),
        publisher: d.publisher.clone().into(),
        year: d.year.clone().into(),
        isbn: d.isbn.clone().into(),
        language: d.language.clone().into(),
        pages: d.pages.clone().into(),
        size: d.size.clone().into(),
        extension: d.extension.to_uppercase().into(),
        description: d.description.clone().into(),
        source: d.source_url.clone().into(),
        cover: cover_image(&d.md5),
        have: results().1.contains(&d.md5),
    });
}

fn cache_details(d: details::Details) {
    let at = now_secs();
    spawn(async move {
        let Ok(pool) = history::open().await else { return };
        if let Err(e) = history::store_details(&pool, &d, at).await {
            tracing::warn!(error = %e, "genesis: could not cache the record details");
        }
    });
}

// ── covers ──────────────────────────────────────────────────────────────────

/// The cached cover for an MD5, or an empty image when there is none yet.
///
/// Decoding happens on the UI thread because `slint::Image` is not `Send`; the
/// files are thumbnails, so this is cheap. The *fetching* is what runs off the
/// event loop.
fn cover_image(md5: &str) -> slint::Image {
    match tulipix_genesis::cover::cached(md5) {
        Some(Some(path)) => load_cover(&path),
        _ => slint::Image::default(),
    }
}

fn load_cover(path: &Path) -> slint::Image {
    // Only ever load out of the cover cache: the path is derived from an MD5
    // by the cover module, never from anything a mirror said.
    if !tulipix_genesis::cover::is_cached_path(path) {
        return slint::Image::default();
    }
    slint::Image::load_from_path(path).unwrap_or_default()
}

/// Fetch the covers for a result set, one book at a time, and drop each one
/// into its row as it lands.
///
/// Sequential on purpose: a screenful of results is fifty records, and fifty
/// concurrent requests to one mirror is the sort of thing that gets an address
/// rate-limited. `seq` abandons the pass the moment a newer search starts.
fn fetch_covers(w: &MainWindow, books: Vec<Book>, seq: u64) {
    let weak = w.as_weak();
    spawn_blocking(move || {
        for book in books {
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            if cover_known(&book.md5) {
                continue;
            }
            // Holds the service mutex for one record page plus one image, which
            // is why this is a book at a time rather than the whole set inside
            // a single lock.
            let found = match with_mut(|svc| svc.cover(&book, &|_| {})) {
                Some(Ok(Some(path))) => path,
                _ => continue,
            };
            let md5 = book.md5.clone();
            let _ = weak.upgrade_in_event_loop(move |w| {
                if SEARCH_GEN.load(Ordering::Relaxed) == seq {
                    set_cover(&w, &md5, &found);
                }
            });
        }
    });
}

/// True when this MD5 has already been looked up — art or a "no cover" answer.
fn cover_known(md5: &str) -> bool {
    tulipix_genesis::cover::cached(md5).is_some()
}

/// Put a freshly fetched cover into the row it belongs to.
///
/// `set_row_data` on the live model rather than rebuilding the whole list:
/// covers land one at a time, and re-pushing fifty rows per arrival would
/// restart the grid's layout fifty times.
fn set_cover(w: &MainWindow, md5: &str, path: &Path) {
    let model = w.get_genesis_rows();
    for i in 0..model.row_count() {
        if let Some(mut row) = model.row_data(i)
            && row.md5 == md5
        {
            row.cover = load_cover(path);
            model.set_row_data(i, row);
            return;
        }
    }
}

/// Case-folded sort key. Mirrors mix casing freely, so a byte compare would put
/// every lowercase title after every uppercase one.
fn text(s: &str) -> String {
    s.trim().to_lowercase()
}

/// Leading digits of a ragged mirror column. `None`/unparseable sorts first
/// ascending, which is where "unknown" belongs.
fn number(s: Option<&str>) -> u64 {
    s.unwrap_or_default()
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

// ── download ────────────────────────────────────────────────────────────────

fn start_download(w: &MainWindow, md5: String) {
    let Some(book) = results().0.iter().find(|b| b.md5 == md5).cloned() else {
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

/// How many results one search asks for, clamped to what the slider allows.
fn configured_limit() -> i32 {
    advanced(LIMIT_KEY)
        .trim()
        .parse::<i32>()
        .ok()
        .map(|v| v.clamp(1, MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
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
    q.limit = configured_limit().max(1) as usize;
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

/// Write one `advanced` key. An empty value removes it rather than storing a
/// blank, so "unset" and "set to nothing" stay the same thing — every reader
/// here treats empty as "use the default".
fn set_advanced(key: &str, value: &str) {
    let Ok(mut s) = tulipix_core::settings::Settings::load() else {
        tracing::warn!("genesis: could not load settings; {key} not saved");
        return;
    };
    if value.trim().is_empty() {
        s.advanced.remove(key);
    } else {
        s.advanced.insert(key.to_string(), value.trim().to_string());
    }
    if let Err(e) = s.save() {
        tracing::warn!(error = %e, "genesis: could not save settings");
    }
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

use tulipix_core::util::human_bytes as human;

use tulipix_core::util::unix_secs_i64 as now_secs;

#[cfg(test)]
mod tests {
    use super::*;

    /// The slider offers 1‥49 and the setting is free text, so both a value
    /// out of range and a value that is not a number have to land somewhere
    /// sane rather than asking a mirror for zero results.
    #[test]
    fn the_results_limit_is_clamped_whatever_the_setting_says() {
        assert_eq!(DEFAULT_LIMIT, 21);
        assert_eq!(MAX_LIMIT, 49);
        assert!(DEFAULT_LIMIT <= MAX_LIMIT);
        for raw in ["0", "-5", "999", "not a number", ""] {
            let parsed =
                raw.trim().parse::<i32>().ok().map(|v| v.clamp(1, MAX_LIMIT)).unwrap_or(DEFAULT_LIMIT);
            assert!((1..=MAX_LIMIT).contains(&parsed), "{raw} produced {parsed}");
        }
    }

    #[test]
    fn history_ages_read_as_durations_not_timestamps() {
        let now = now_secs();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 120), "2 min ago");
        assert_eq!(ago(now - 7200), "2 h ago");
        assert_eq!(ago(now - 172800), "2 d ago");
        // A clock that moved backwards must not produce a negative age.
        assert_eq!(ago(now + 60), "just now");
    }

    #[test]
    fn the_default_topics_are_the_two_worth_searching() {
        // `into_iter`, not `iter`: borrowing out of the temporary the call
        // returns leaves the refs dangling at the end of the statement.
        let on: Vec<String> =
            default_topics().into_iter().filter(|t| t.on).map(|t| t.code).collect();
        assert_eq!(on, ["libgen", "fiction"]);
    }

    #[test]
    fn ragged_mirror_columns_still_sort_as_numbers() {
        // Mirrors put anything in Year and Pages. Leading digits win, and
        // nothing parseable sorts as 0 — "unknown" first, ascending.
        assert_eq!(number(Some("1998")), 1998);
        assert_eq!(number(Some("c. 2004")), 2004, "leading prose is skipped");
        assert_eq!(number(Some("200?")), 200);
        assert_eq!(number(Some("")), 0);
        assert_eq!(number(None), 0);
    }

    #[test]
    fn titles_sort_case_insensitively() {
        // A byte compare would file every lowercase title after every
        // uppercase one, which reads as no sort at all.
        let mut v = vec![text("banana"), text("Apple"), text("cherry")];
        v.sort();
        assert_eq!(v, ["apple", "banana", "cherry"]);
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
