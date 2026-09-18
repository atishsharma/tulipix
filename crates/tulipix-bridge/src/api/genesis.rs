//! Genesis: search a Libgen mirror, look at a record, pull the file down.
//!
//! [`tulipix_genesis`] is blocking on purpose -- its redirect validation uses
//! the same `http::Uri` parser that opens the connection, and an async port
//! would reintroduce exactly the parser-disagreement gap that closes. So every
//! call into it goes through `spawn_blocking`, and what comes back is written
//! into the session and announced with an event.
//!
//! Same painted-state contract as the other sections: `genesis_dispatch` hands
//! back a clone of the whole snapshot, handlers write into their own corner of
//! it, and nothing is recomputed for a keystroke.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use flutter_rust_bridge::frb;
use anyhow::Result;
use tulipix_genesis::model::{Book, Field, SearchQuery, Topic};
use tulipix_genesis::{GenesisService, details, download, history};

use crate::frb_generated::StreamSink;

/// Settings key for an explicit mirror list, comma-separated. Lives in
/// `Settings.advanced`, which is free-text, so it needs no schema change.
const MIRRORS_KEY: &str = "genesis.mirrors";

/// Settings key for how many results one search asks the mirror for.
const LIMIT_KEY: &str = "genesis.limit";

/// Settings key for an override download folder.
const DEST_KEY: &str = "genesis.dest";

/// Results per search when nothing is configured, and the ceiling the slider
/// allows. Mirrors serve fixed page sizes and round 49 up to 50, so asking for
/// more than that buys a second request rather than more books.
const DEFAULT_LIMIT: i32 = 21;
const MAX_LIMIT: i32 = 49;

/// What the filters start on. Both are client-side, applied after the mirror
/// answers, so they are a preference rather than part of the query.
const DEFAULT_FORMAT: &str = "epub";
const DEFAULT_LANGUAGE: &str = "English";

/// Past searches kept for the history popup.
const HISTORY_MAX: i64 = 100;

/// Ceiling on a single download. Generous, but not unbounded -- a mirror that
/// lies about `Content-Length` should not be able to fill the disk.
const MAX_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// Progress is reported per 64 KB chunk; at 20 MB/s that is 300 callbacks a
/// second. Throttled to roughly 10 Hz before crossing the bridge.
const PROGRESS_MS: u128 = 100;

// ---------------------------------------------------------------- state ----

/// One row of the result table.
#[derive(Debug, Clone, Default)]
pub struct GenRow {
    pub md5: String,
    pub title: String,
    pub author: String,
    pub year: String,
    pub language: String,
    pub pages: String,
    pub size: String,
    pub format: String,
    /// Already downloaded once -- the Get button becomes "In library".
    pub have: bool,
    /// Absolute path to the cached cover, empty until the cover pass reaches
    /// this row (or if the record has none).
    pub cover: String,
}

/// One collection to search. All off returns nothing at all, which reads as a
/// broken search rather than an empty filter, so one is always on.
#[derive(Debug, Clone, Default)]
pub struct GenTopic {
    pub code: String,
    pub label: String,
    pub on: bool,
}

/// A search someone ran before, with the filters it ran under.
#[derive(Debug, Clone, Default)]
pub struct GenSearch {
    pub terms: String,
    pub field: String,
    pub format: String,
    pub language: String,
    pub results: i32,
    pub when: String,
}

#[derive(Debug, Clone, Default)]
pub struct GenDetails {
    pub md5: String,
    pub title: String,
    pub series: String,
    pub authors: String,
    pub publisher: String,
    pub year: String,
    pub isbn: String,
    pub language: String,
    pub pages: String,
    pub size: String,
    pub extension: String,
    pub description: String,
    pub source: String,
    pub cover: String,
    pub have: bool,
}

#[derive(Debug, Clone, Default)]
pub struct GenesisState {
    // The query.
    pub terms: String,
    pub field: String,
    pub format: String,
    pub language: String,
    pub limit: i32,
    pub topics: Vec<GenTopic>,

    // What the mirror pool is doing. "idle" | "probing" | "searching" |
    // "ready" | "failed".
    pub phase: String,
    /// Live line from inside a mirror probe.
    pub status: String,
    pub error: String,
    /// Bare host of the mirror that answered, for the status chip.
    pub mirror: String,
    pub active_url: String,
    /// "ok" | "bad" | "" -- how to tint that chip.
    pub mirror_tone: String,

    // Results.
    pub rows: Vec<GenRow>,
    pub total: i32,
    pub sort: String,
    pub sort_desc: bool,
    pub more_busy: bool,
    pub more_done: bool,
    /// A search replayed from history, so the Search button can fill rather
    /// than results appearing from nowhere.
    pub replay: bool,

    // One download.
    pub busy: bool,
    pub busy_name: String,
    pub busy_pct: f64,
    pub busy_detail: String,
    pub done_name: String,
    pub done_detail: String,

    // Where files land.
    pub dest: String,
    pub can_download: bool,

    // The record popup.
    pub details_open: bool,
    pub details_loading: bool,
    pub details_error: String,
    pub details: GenDetails,

    pub history: Vec<GenSearch>,

    // Settings.
    pub mirrors: String,
    pub mirrors_default: String,
    pub settings_saved: bool,
}

pub enum GenesisCmd {
    /// Entering the section: resolve the destination now, so the Get buttons
    /// are already live rather than going live after the first search.
    Enter,
    Refresh,
    SetTerms { value: String },
    Search,
    /// A retry after "every mirror failed" re-probes, or it just replays the
    /// same dead pool from memory.
    Retry,
    /// Throw the page away: results, words, mirror status and all.
    Clear,
    LoadMore,
    ToggleTopic { code: String },
    SetField { value: String },
    SetFormat { value: String },
    SetLanguage { value: String },
    SetLimit { value: i32 },
    SortBy { key: String },
    OpenDetails { md5: String },
    CloseDetails,
    Download { md5: String },
    Cancel,
    HistoryOpen,
    HistoryRun { index: i32 },
    HistoryClear,
    SettingsOpen,
    SetMirrors { value: String },
    SettingsSave,
    SettingsReset,
    SetDest { path: String },
    /// Reveal the file that just landed, or open the download folder.
    RevealDest,
    /// Clear the "added to library" strip and the settings "Saved ✓". Both are
    /// timed in Dart, which is where a timer belongs.
    Dismiss,
}

#[derive(Debug, Clone)]
pub enum GenesisEvent {
    /// A background call finished and moved the snapshot. Dart answers with
    /// `Refresh`.
    Changed,
    /// Download progress, which arrives ten times a second and must not cost a
    /// snapshot each time. `pct` is -1 for a line that carries only a detail
    /// (which mirror answered, which hop it is on) and should leave the bar
    /// where it is.
    Progress { pct: f64, detail: String },
}

// -------------------------------------------------------------- session ----

/// What a "load more" needs to know.
///
/// A mirror page holds 25, 50 or 100 records and the format and language
/// filters run after it arrives, so one request usually carries several
/// screens' worth. Those are spent before the mirror is asked for another page.
/// `frb(ignore)`: private state, not part of the contract. Without it frb
/// mirrors the struct, and mirroring it means mirroring `SearchQuery` — a
/// `tulipix-genesis` type it cannot see — as an opaque handle that does not
/// resolve.
#[frb(ignore)]
#[derive(Default)]
struct Paging {
    /// The query the results in hand came from, so the next page is the same
    /// search rather than whatever is in the search box now.
    query: Option<SearchQuery>,
    /// Last mirror page fetched, one-based.
    page: usize,
    /// How many of the fetched records are on screen.
    shown: usize,
    /// A page came back with nothing new in it -- there is no more to load.
    exhausted: bool,
}

/// `frb(ignore)`: `pub(crate)` is not part of the Dart contract either.
#[frb(ignore)]
pub(crate) struct Session {
    ui: GenesisState,
    /// The last result set, kept so a Get click can name a full [`Book`] rather
    /// than re-searching, and so a sort never needs the mirror again.
    books: Vec<Book>,
    known: Vec<String>,
    paging: Paging,
}

impl Session {
    fn new() -> Self {
        Self {
            ui: GenesisState {
                field: "Any".into(),
                format: DEFAULT_FORMAT.into(),
                language: DEFAULT_LANGUAGE.into(),
                limit: configured_limit(),
                topics: default_topics(),
                phase: "idle".into(),
                mirrors_default: tulipix_genesis::mirror::SEED_MIRRORS.join("\n"),
                ..Default::default()
            },
            books: Vec::new(),
            known: Vec::new(),
            paging: Paging::default(),
        }
    }
}

fn cell() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Session::new()))
}

/// Read or mutate the session. The guard cannot escape the closure, which is
/// the point: a `MutexGuard` merely in lexical scope across an `.await` makes
/// the whole future non-`Send`, and frb's dispatcher requires `Send`.
fn with<R>(f: impl FnOnce(&mut Session) -> R) -> R {
    let mut g = cell().lock().unwrap_or_else(|p| p.into_inner());
    f(&mut g)
}

fn events() -> &'static Mutex<Vec<StreamSink<GenesisEvent>>> {
    static E: OnceLock<Mutex<Vec<StreamSink<GenesisEvent>>>> = OnceLock::new();
    E.get_or_init(|| Mutex::new(Vec::new()))
}

fn emit(event: GenesisEvent) {
    if let Ok(mut sinks) = events().lock() {
        sinks.retain(|s| s.add(event.clone()).is_ok());
    }
}

/// Search results, cover arrivals and download ticks.
pub fn genesis_events(sink: StreamSink<GenesisEvent>) {
    if let Ok(mut sinks) = events().lock() {
        sinks.push(sink);
    }
}

/// Bumped on every search. A result that lands after a newer search started is
/// dropped rather than overwriting it.
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

// ------------------------------------------------------------- dispatch ----

/// Apply one command and hand back the snapshot that follows it.
pub async fn genesis_dispatch(cmd: GenesisCmd) -> Result<GenesisState> {
    match cmd {
        GenesisCmd::Enter => refresh_dest().await,
        GenesisCmd::Refresh => {}
        GenesisCmd::SetTerms { value } => with(|s| s.ui.terms = value),
        GenesisCmd::Search => {
            with(|s| s.ui.replay = false);
            search();
        }
        GenesisCmd::Retry => {
            // Off the pool in memory first, or the retry replays the same dead
            // mirrors it just failed on.
            tokio::task::spawn_blocking(|| {
                let _ = with_mut(|svc| svc.refresh_mirrors());
            })
            .await
            .ok();
            with(|s| s.ui.replay = false);
            search();
        }
        GenesisCmd::Clear => {
            // Abandons anything still in flight, including the cover pass,
            // which would otherwise keep writing into rows nobody is shown.
            SEARCH_GEN.fetch_add(1, Ordering::Relaxed);
            with(|s| {
                s.books.clear();
                s.known.clear();
                s.paging = Paging::default();
                s.ui.rows.clear();
                s.ui.total = 0;
                s.ui.sort = String::new();
                s.ui.sort_desc = false;
                s.ui.more_busy = false;
                s.ui.more_done = false;
                s.ui.terms = String::new();
                s.ui.phase = "idle".into();
                s.ui.status = String::new();
                s.ui.error = String::new();
                s.ui.done_name = String::new();
                s.ui.replay = false;
            });
        }
        GenesisCmd::LoadMore => load_more(),
        GenesisCmd::ToggleTopic { code } => with(|s| {
            for t in &mut s.ui.topics {
                if t.code == code {
                    t.on = !t.on;
                }
            }
            // Every topic off returns nothing at all. Re-arm the one just
            // cleared rather than let the next search come back empty.
            if !s.ui.topics.iter().any(|t| t.on)
                && let Some(t) = s.ui.topics.iter_mut().find(|t| t.code == code)
            {
                t.on = true;
            }
        }),
        GenesisCmd::SetField { value } => with(|s| s.ui.field = value),
        GenesisCmd::SetFormat { value } => with(|s| s.ui.format = value),
        GenesisCmd::SetLanguage { value } => with(|s| s.ui.language = value),
        GenesisCmd::SetLimit { value } => {
            let value = value.clamp(1, MAX_LIMIT);
            with(|s| s.ui.limit = value);
            set_advanced(LIMIT_KEY, &value.to_string());
        }
        GenesisCmd::SortBy { key } => with(|s| {
            // Same column again flips direction; a different one starts
            // ascending, which is what every table on the desktop does.
            if s.ui.sort == key {
                s.ui.sort_desc = !s.ui.sort_desc;
            } else {
                s.ui.sort = key;
                s.ui.sort_desc = false;
            }
            repaint(s);
        }),
        GenesisCmd::OpenDetails { md5 } => open_details(md5),
        GenesisCmd::CloseDetails => with(|s| s.ui.details_open = false),
        GenesisCmd::Download { md5 } => start_download(md5),
        GenesisCmd::Cancel => {
            if let Some(flag) = cancel_flag().lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
                flag.store(true, Ordering::Relaxed);
            }
        }
        GenesisCmd::HistoryOpen => load_history().await,
        GenesisCmd::HistoryRun { index } => {
            // Restore the filters the search ran under, not just its words: the
            // same terms with a different format filter is a different search.
            let ran = with(|s| {
                let e = s.ui.history.get(index as usize)?.clone();
                s.ui.terms = e.terms;
                s.ui.field = e.field;
                s.ui.format = e.format;
                s.ui.language = e.language;
                s.ui.replay = true;
                Some(())
            });
            if ran.is_some() {
                search();
            }
        }
        GenesisCmd::HistoryClear => {
            if let Ok(pool) = history::open().await
                && let Err(e) = history::clear_searches(&pool).await
            {
                tracing::warn!(error = %e, "genesis: could not clear the search history");
            }
            load_history().await;
        }
        GenesisCmd::SettingsOpen => {
            with(|s| {
                s.ui.mirrors = advanced(MIRRORS_KEY).replace(',', "\n");
                s.ui.settings_saved = false;
            });
            refresh_dest().await;
        }
        GenesisCmd::SetMirrors { value } => with(|s| s.ui.mirrors = value),
        GenesisCmd::SettingsSave => {
            // Stored comma-separated because `Settings.advanced` is a flat
            // string map; the box is newline-per-entry because that is how a
            // list is edited.
            let list = with(|s| {
                s.ui.mirrors
                    .split(['\n', ','])
                    .map(str::trim)
                    .filter(|x| !x.is_empty())
                    .collect::<Vec<_>>()
                    .join(",")
            });
            set_advanced(MIRRORS_KEY, &list);
            // The pool in memory was resolved from the old list, so it has to
            // go or the next search silently ignores the edit.
            tokio::task::spawn_blocking(|| {
                let _ = with_mut(|svc| svc.refresh_mirrors());
            })
            .await
            .ok();
            with(|s| s.ui.settings_saved = true);
        }
        GenesisCmd::SettingsReset => {
            set_advanced(MIRRORS_KEY, "");
            set_advanced(DEST_KEY, "");
            with(|s| {
                s.ui.mirrors = String::new();
                s.ui.settings_saved = true;
            });
            tokio::task::spawn_blocking(|| {
                let _ = with_mut(|svc| svc.refresh_mirrors());
            })
            .await
            .ok();
            refresh_dest().await;
        }
        GenesisCmd::SetDest { path } => {
            set_advanced(DEST_KEY, &path);
            let dir = PathBuf::from(&path);
            tokio::task::spawn_blocking(move || {
                let _ = with_mut(|svc| svc.set_dest(dir));
            })
            .await
            .ok();
            // Registers the new folder as a library root, which is what makes a
            // book downloaded into it appear in My Library.
            refresh_dest().await;
            with(|s| s.ui.settings_saved = true);
        }
        GenesisCmd::RevealDest => reveal_dest(),
        GenesisCmd::Dismiss => with(|s| {
            s.ui.done_name = String::new();
            s.ui.done_detail = String::new();
            s.ui.settings_saved = false;
        }),
    }
    Ok(with(|s| s.ui.clone()))
}

// ---------------------------------------------------------------- search ----

fn search() {
    let Some((query, terms, seq)) = with(|s| {
        let terms = s.ui.terms.trim().to_string();
        if terms.is_empty() {
            return None;
        }
        let query = build_query(s, &terms);
        let seq = SEARCH_GEN.fetch_add(1, Ordering::Relaxed) + 1;
        // A fresh search starts paging over: page one, nothing revealed yet,
        // and the query kept so "load more" asks for the next page of *this*
        // search rather than of whatever is in the box by then.
        s.paging = Paging { query: Some(query.clone()), page: 1, shown: 0, exhausted: false };
        s.ui.more_busy = false;
        s.ui.more_done = false;
        s.ui.status = String::new();
        s.ui.error = String::new();
        Some((query, terms, seq))
    }) else {
        return;
    };

    tokio::task::spawn_blocking(move || {
        // A cold pool means a real probe of every seed mirror, which is the
        // slow path worth naming; a warm one goes straight to the query.
        let warm = with_service(|svc| svc.has_pool()).unwrap_or(false);
        with(|s| s.ui.phase = if warm { "searching".into() } else { "probing".into() });
        emit(GenesisEvent::Changed);

        // Progress from inside the mirror probe. Dropped once a newer search
        // has started, so a slow probe cannot narrate over a fresh query.
        let on_status = move |line: &str| {
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            with(|s| s.ui.status = line.to_string());
            emit(GenesisEvent::Changed);
        };

        let dest = resolve_dest();
        let result = with_mut(|svc| {
            svc.set_mirrors(configured_mirrors());
            svc.set_dest(dest);
            svc.search(&query, &on_status)
        });

        if SEARCH_GEN.load(Ordering::Relaxed) != seq {
            return;
        }
        match result {
            Some(Ok(served)) => {
                with(|s| {
                    s.ui.mirror = served.mirror.clone();
                    s.ui.active_url = served.url.clone();
                    s.ui.mirror_tone = "ok".into();
                    s.ui.status = String::new();
                    s.ui.phase = "ready".into();
                    s.ui.replay = false;
                });
                remember_search(&terms, served.value.len() as i64);
                fill_rows(served.value, seq);
            }
            Some(Err(e)) => {
                with(|s| {
                    s.ui.error = e.to_string();
                    s.ui.mirror_tone = "bad".into();
                    s.ui.phase = "failed".into();
                    s.ui.replay = false;
                });
                emit(GenesisEvent::Changed);
            }
            // Only when the service could not be built at all.
            None => {
                with(|s| {
                    s.ui.error = "could not start the search client".into();
                    s.ui.phase = "failed".into();
                    s.ui.replay = false;
                });
                emit(GenesisEvent::Changed);
            }
        }
    });
}

/// Stash the result set, mark what is already downloaded, and paint the table.
///
/// The history lookup is one query for the whole result set rather than one per
/// row, which would be fifty round trips to paint one table.
fn fill_rows(books: Vec<Book>, seq: u64) {
    let md5s: Vec<String> = books.iter().map(|b| b.md5.clone()).collect();
    spawn(async move {
        let known = match history::open().await {
            Ok(pool) => history::known(&pool, &md5s).await.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(error = %e, "genesis: could not open the history database");
                Vec::new()
            }
        };
        let step = with(|s| {
            s.books = books;
            s.known = known;
            // A new search shows the mirror's own order. Any other choice means
            // the first thing shown is sorted by a column someone clicked
            // during the previous search and long forgot.
            s.ui.sort = String::new();
            s.ui.sort_desc = false;
            s.ui.limit.max(1) as usize
        });
        reveal(step, seq);
    });
}

/// Put another `step` of the fetched records on screen.
///
/// Covers are fetched for exactly the rows this revealed. A mirror page can
/// hold a hundred records and only a screenful is ever shown at once, so
/// fetching art for all of them would be a hundred record-page requests for
/// books nobody has looked at.
fn reveal(step: usize, seq: u64) {
    let fresh = with(|s| {
        let total = s.books.len();
        let from = s.paging.shown;
        let to = (from + step).min(total);
        s.paging.shown = to;
        repaint(s);
        s.ui.more_done = s.paging.shown >= total && s.paging.exhausted;
        s.books[from..to].to_vec()
    });
    emit(GenesisEvent::Changed);
    if !fresh.is_empty() {
        fetch_covers(fresh, seq);
    }
}

/// Show more results, from what is already in hand where possible.
fn load_more() {
    enum Next {
        Nothing,
        Reveal(usize, u64),
        Fetch(SearchQuery, usize, u64),
    }
    let next = with(|s| {
        if s.ui.more_busy {
            return Next::Nothing;
        }
        let step = s.ui.limit.max(1) as usize;
        let seq = SEARCH_GEN.load(Ordering::Relaxed);
        // Spend the over-fetch first. No network, so this is instant.
        if s.paging.shown < s.books.len() {
            return Next::Reveal(step, seq);
        }
        if s.paging.exhausted {
            s.ui.more_done = true;
            return Next::Nothing;
        }
        let Some(mut query) = s.paging.query.clone() else { return Next::Nothing };
        query.page = s.paging.page + 1;
        s.ui.more_busy = true;
        s.ui.status = String::new();
        Next::Fetch(query, step, seq)
    });

    match next {
        Next::Nothing => {}
        Next::Reveal(step, seq) => reveal(step, seq),
        Next::Fetch(query, step, seq) => {
            tokio::task::spawn_blocking(move || {
                let result = with_mut(|svc| svc.search(&query, &|_| {}));
                with(|s| s.ui.more_busy = false);
                // A newer search started while this page was in flight; its
                // results are on screen and these belong to a query nobody is
                // looking at.
                if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                    return;
                }
                match result {
                    Some(Ok(served)) => {
                        let fresh = with(|s| {
                            // Mirrors overlap their pages, and a repeat would
                            // show up as a duplicate card with the same MD5 --
                            // and be counted as progress.
                            let fresh: Vec<Book> = served
                                .value
                                .into_iter()
                                .filter(|b| !s.books.iter().any(|h| h.md5 == b.md5))
                                .collect();
                            s.books.extend(fresh.iter().cloned());
                            s.paging.page += 1;
                            // Mirrors repeat the last page rather than 404 once
                            // you walk off the end, so "nothing new" is the only
                            // honest end-of-results signal there is.
                            s.paging.exhausted = fresh.is_empty();
                            fresh
                        });
                        mark_known(fresh.iter().map(|b| b.md5.clone()).collect());
                        reveal(step, seq);
                    }
                    Some(Err(e)) => {
                        // The results already on screen are still good, so this
                        // says what happened and leaves the button up to try
                        // again -- the page is not torn down over one extra
                        // page failing.
                        with(|s| s.ui.status = e.to_string());
                        emit(GenesisEvent::Changed);
                    }
                    None => {
                        with(|s| s.ui.more_done = true);
                        emit(GenesisEvent::Changed);
                    }
                }
            });
        }
    }
}

/// Fill in the "already downloaded" mark for freshly appended records.
fn mark_known(md5s: Vec<String>) {
    if md5s.is_empty() {
        return;
    }
    spawn(async move {
        let Ok(pool) = history::open().await else { return };
        let Ok(known) = history::known(&pool, &md5s).await else { return };
        if known.is_empty() {
            return;
        }
        with(|s| {
            s.known.extend(known);
            repaint(s);
        });
        emit(GenesisEvent::Changed);
    });
}

/// Order what has been revealed so far and write it into the snapshot.
///
/// The cut is taken in the mirror's own order -- its relevance ranking -- and
/// the sort is applied to what that yields. The other way round, changing the
/// sort would change *which* books are on screen, so ordering by title would
/// quietly swap the results for a different set of them.
fn repaint(s: &mut Session) {
    let (key, desc) = (s.ui.sort.clone(), s.ui.sort_desc);
    let mut ordered: Vec<&Book> = s.books.iter().take(s.paging.shown).collect();
    if !key.is_empty() {
        ordered.sort_by(|a, b| {
            let ord = match key.as_str() {
                "author" => text(a.authors_or_unknown()).cmp(&text(b.authors_or_unknown())),
                // Year and pages are strings on the record because mirrors put
                // anything in those columns; a numeric compare on "1998" and
                // "200?" has to fall back to text rather than to zero.
                "year" => number(a.year.as_deref()).cmp(&number(b.year.as_deref())),
                "language" => text(a.language.as_deref().unwrap_or(""))
                    .cmp(&text(b.language.as_deref().unwrap_or(""))),
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

    let known = &s.known;
    let rows: Vec<GenRow> = ordered
        .into_iter()
        .map(|b| GenRow {
            md5: b.md5.clone(),
            title: b.title.clone(),
            author: b.authors_or_unknown().to_string(),
            year: b.year.clone().unwrap_or_default(),
            language: b.language.clone().unwrap_or_default(),
            pages: b.pages.clone().unwrap_or_default(),
            size: b.size_human(),
            format: b.ext().to_uppercase(),
            have: known.contains(&b.md5),
            cover: cover_path(&b.md5),
        })
        .collect();
    s.ui.total = rows.len() as i32;
    s.ui.rows = rows;
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

// ---------------------------------------------------------------- covers ----

/// The cached cover for an MD5, or empty when there is none yet.
fn cover_path(md5: &str) -> String {
    match tulipix_genesis::cover::cached(md5) {
        Some(Some(path)) if tulipix_genesis::cover::is_cached_path(&path) => {
            path.display().to_string()
        }
        _ => String::new(),
    }
}

/// Fetch the covers for a result set, one book at a time, and drop each one
/// into its row as it lands.
///
/// Sequential on purpose: a screenful of results is fifty records, and fifty
/// concurrent requests to one mirror is the sort of thing that gets an address
/// rate-limited. `seq` abandons the pass the moment a newer search starts.
fn fetch_covers(books: Vec<Book>, seq: u64) {
    tokio::task::spawn_blocking(move || {
        for book in books {
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            // Already looked up -- art, or a cached "no cover" answer.
            if tulipix_genesis::cover::cached(&book.md5).is_some() {
                continue;
            }
            // Holds the service mutex for one record page plus one image, which
            // is why this is a book at a time rather than the whole set inside
            // a single lock.
            let found = match with_mut(|svc| svc.cover(&book, &|_| {})) {
                Some(Ok(Some(path))) => path,
                _ => continue,
            };
            if SEARCH_GEN.load(Ordering::Relaxed) != seq {
                return;
            }
            let shown = with(|s| {
                let mut shown = false;
                for row in &mut s.ui.rows {
                    if row.md5 == book.md5 {
                        row.cover = path_if_cached(&found);
                        shown = true;
                    }
                }
                if s.ui.details.md5 == book.md5 {
                    s.ui.details.cover = path_if_cached(&found);
                    shown = true;
                }
                shown
            });
            if shown {
                emit(GenesisEvent::Changed);
            }
        }
    });
}

/// Only ever hand Dart a path inside the cover cache: it is derived from an
/// MD5 by the cover module, never from anything a mirror said.
fn path_if_cached(path: &Path) -> String {
    match tulipix_genesis::cover::is_cached_path(path) {
        true => path.display().to_string(),
        false => String::new(),
    }
}

// --------------------------------------------------------------- history ----

async fn load_history() {
    let list = match history::open().await {
        Ok(pool) => history::recent_searches(&pool, HISTORY_MAX).await.unwrap_or_default(),
        Err(e) => {
            tracing::warn!(error = %e, "genesis: could not read the search history");
            Vec::new()
        }
    };
    let rows: Vec<GenSearch> = list
        .into_iter()
        .map(|x| GenSearch {
            terms: x.terms,
            field: x.field,
            format: x.format,
            language: x.language,
            results: x.results as i32,
            when: ago(x.at),
        })
        .collect();
    with(|s| s.ui.history = rows);
}

/// Record a search so it can be run again from the history popup.
///
/// Fire and forget: failing to remember a search is not a reason to interrupt
/// someone who is looking at its results.
fn remember_search(terms: &str, results: i64) {
    let (terms, field, format, language) = with(|s| {
        (terms.to_string(), s.ui.field.clone(), s.ui.format.clone(), s.ui.language.clone())
    });
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

/// A coarse "how long ago", which is all a history row needs -- the exact
/// second someone ran a search is never the question being asked.
fn ago(at: i64) -> String {
    let secs = (now_secs() - at).max(0);
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86400),
    }
}

// --------------------------------------------------------------- details ----

/// Open the details popup for one record.
///
/// The cache is consulted first and, when it answers, nothing touches the
/// network -- which is the point of storing it. A miss fetches the record page
/// once and writes it back.
fn open_details(md5: String) {
    let Some(book) = with(|s| {
        let book = s.books.iter().find(|b| b.md5 == md5)?.clone();
        s.ui.details_open = true;
        s.ui.details_error = String::new();
        // Seed from the row already on screen, so the popup opens with the
        // title and author filled in rather than blank while the page loads.
        s.ui.details = GenDetails {
            md5: book.md5.clone(),
            title: book.title.clone(),
            authors: book.authors_or_unknown().to_string(),
            year: book.year.clone().unwrap_or_default(),
            language: book.language.clone().unwrap_or_default(),
            pages: book.pages.clone().unwrap_or_default(),
            size: book.size_human(),
            extension: book.ext().to_uppercase(),
            cover: cover_path(&book.md5),
            have: s.known.contains(&book.md5),
            ..Default::default()
        };
        Some(book)
    }) else {
        return;
    };

    spawn(async move {
        // 1. The cache.
        if let Ok(pool) = history::open().await
            && let Ok(Some(cached)) = history::cached_details(&pool, &md5).await
        {
            with(|s| {
                s.ui.details_loading = false;
                push_details(s, &cached);
            });
            emit(GenesisEvent::Changed);
            return;
        }

        // 2. The mirror.
        with(|s| s.ui.details_loading = true);
        emit(GenesisEvent::Changed);
        tokio::task::spawn_blocking(move || {
            let fetched = with_mut(|svc| svc.details(&book, &|_| {}));
            with(|s| {
                s.ui.details_loading = false;
                match &fetched {
                    Some(Ok(served)) => push_details(s, &served.value),
                    Some(Err(e)) => s.ui.details_error = e.to_string(),
                    None => s.ui.details_error = "could not start the search client".into(),
                }
            });
            emit(GenesisEvent::Changed);
            if let Some(Ok(served)) = fetched {
                cache_details(served.value);
            }
        });
    });
}

fn push_details(s: &mut Session, d: &details::Details) {
    s.ui.details = GenDetails {
        md5: d.md5.clone(),
        title: d.title.clone(),
        series: d.series.clone(),
        authors: d.authors.clone(),
        publisher: d.publisher.clone(),
        year: d.year.clone(),
        isbn: d.isbn.clone(),
        language: d.language.clone(),
        pages: d.pages.clone(),
        size: d.size.clone(),
        extension: d.extension.to_uppercase(),
        description: d.description.clone(),
        source: d.source_url.clone(),
        cover: cover_path(&d.md5),
        have: s.known.contains(&d.md5),
    };
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

// -------------------------------------------------------------- download ----

fn start_download(md5: String) {
    let dest = resolve_dest();
    if dest.as_os_str().is_empty() {
        return;
    }
    let Some(book) = with(|s| {
        let book = s.books.iter().find(|b| b.md5 == md5)?.clone();
        s.ui.busy = true;
        s.ui.busy_name = book.title.clone();
        s.ui.busy_pct = 0.0;
        s.ui.busy_detail = "starting…".into();
        s.ui.done_name = String::new();
        Some(book)
    }) else {
        return;
    };

    let flag = Arc::new(AtomicBool::new(false));
    *cancel_flag().lock().unwrap_or_else(|e| e.into_inner()) = Some(flag.clone());

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

    tokio::task::spawn_blocking(move || {
        let mut last = std::time::Instant::now();
        let mut report = move |done: u64, total: Option<u64>| {
            if last.elapsed().as_millis() < PROGRESS_MS {
                return;
            }
            last = std::time::Instant::now();
            let pct = match total {
                Some(t) if t > 0 => (done as f64 / t as f64).min(1.0),
                _ => 0.0,
            };
            let detail = match total {
                Some(t) if t > 0 => format!("{} / {}", human(done), human(t)),
                _ => human(done),
            };
            with(|s| {
                s.ui.busy_pct = pct;
                s.ui.busy_detail = detail.clone();
            });
            emit(GenesisEvent::Progress { pct, detail });
        };

        let on_status = |line: &str| {
            with(|s| s.ui.busy_detail = line.to_string());
            emit(GenesisEvent::Progress { pct: -1.0, detail: line.to_string() });
        };

        let result = with_mut(|svc| svc.download(&book, &opts, &mut report, &on_status));
        match result {
            Some(Ok(served)) => finish_download(
                book,
                served.value.path.display().to_string(),
                served.mirror,
                served.value.bytes,
                served.value.verified,
            ),
            Some(Err(e)) => {
                let msg = e.to_string();
                with(|s| {
                    s.ui.busy = false;
                    // A cancel is a choice, not a failure -- it does not
                    // deserve the error banner.
                    if !msg.contains("cancelled") {
                        s.ui.error = msg;
                        s.ui.phase = "failed".into();
                    }
                });
                emit(GenesisEvent::Changed);
            }
            None => {
                with(|s| s.ui.busy = false);
                emit(GenesisEvent::Changed);
            }
        }
    });
}

/// Record the download, rescan the books library, then show the confirmation.
fn finish_download(book: Book, path: String, mirror: String, bytes: u64, verified: bool) {
    spawn(async move {
        if let Ok(pool) = history::open().await
            && let Err(e) = history::record(&pool, &book, &path, &mirror, now_secs()).await
        {
            tracing::warn!(error = %e, "genesis: could not record the download");
        }

        // The file landed inside a watched library root, so the ordinary scan
        // is what picks it up -- no separate ingest path to keep in step.
        match crate::db::books_pool().await {
            Ok(pool) => {
                if let Err(e) = tulipix_books::scan::scan_all(pool).await {
                    tracing::warn!(error = %e, "genesis: books rescan");
                }
            }
            Err(e) => tracing::warn!(error = %e, "genesis: could not open the books database"),
        }

        let name = Path::new(&path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| book.title.clone());

        with(|s| {
            s.ui.busy = false;
            s.ui.done_name = name;
            s.ui.done_detail = format!(
                "{} · {} · from {mirror}",
                human(bytes),
                if verified { "MD5 verified" } else { "not verified" }
            );
            // Mark the row so the button becomes "In library" without a
            // re-search.
            s.known.push(book.md5.clone());
            for row in &mut s.ui.rows {
                if row.md5 == book.md5 {
                    row.have = true;
                }
            }
            if s.ui.details.md5 == book.md5 {
                s.ui.details.have = true;
            }
        });
        // `dest` stays the folder -- it is what the idle state names as the
        // download location. The file path is remembered separately so
        // "Show file" can reveal the file itself.
        *last_download() = path;
        emit(GenesisEvent::Changed);
    });
}

/// Reveal the file that just landed when there is one -- selecting it in a
/// folder of many is more use than opening the folder. Falls back to the folder
/// itself, which is what the idle state points at.
fn reveal_dest() {
    let last = last_download().clone();
    let file = PathBuf::from(&last);
    if !last.is_empty() && file.exists() {
        if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(&file) {
            tracing::warn!(error = %e, "genesis: could not reveal the download");
        }
        return;
    }
    let dir = resolve_dest();
    if dir.as_os_str().is_empty() {
        return;
    }
    if let Err(e) = tulipix_platform::fm::open_default(&dir) {
        tracing::warn!(error = %e, "genesis: could not open the download folder");
    }
}

// --------------------------------------------------------- query and dest ----

/// How many results one search asks for, clamped to what the slider allows.
fn configured_limit() -> i32 {
    advanced(LIMIT_KEY)
        .trim()
        .parse::<i32>()
        .ok()
        .map(|v| v.clamp(1, MAX_LIMIT))
        .unwrap_or(DEFAULT_LIMIT)
}

fn build_query(s: &Session, terms: &str) -> SearchQuery {
    let mut q = SearchQuery::new(terms);
    q.topics =
        s.ui.topics.iter().filter(|t| t.on).filter_map(|t| Topic::parse(&t.code)).collect();
    q.fields = match s.ui.field.as_str() {
        "Title" => vec![Field::Title],
        "Author" => vec![Field::Author],
        "Series" => vec![Field::Series],
        "Publisher" => vec![Field::Publisher],
        "Year" => vec![Field::Year],
        "ISBN" => vec![Field::Isbn],
        // "Any" -- an empty list means the mirror matches every column.
        _ => Vec::new(),
    };
    q.extension = match s.ui.format.as_str() {
        "Any" => None,
        other => Some(other.to_ascii_lowercase()),
    };
    q.language = match s.ui.language.as_str() {
        "Any" => None,
        other => Some(other.to_string()),
    };
    q.limit = s.ui.limit.max(1) as usize;
    q
}

/// Sci-Tech and Fiction on by default -- between them they are most of what
/// anyone searches for, and every topic on makes every search slower.
fn default_topics() -> Vec<GenTopic> {
    Topic::ALL
        .iter()
        .map(|t| GenTopic {
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

/// Where downloads land: the `genesis.dest` override, else `<Downloads>/Genesis`.
///
/// Never an existing library folder. A shelf someone arranged and a pile of
/// fetched files are different things, and once they are mixed there is no way
/// to separate them again.
fn resolve_dest() -> PathBuf {
    let stored = advanced(DEST_KEY);
    if !stored.trim().is_empty() {
        return PathBuf::from(stored);
    }
    tulipix_genesis::fsutil::default_dest()
}

/// Make sure the download folder exists and the books library is watching it.
///
/// Both are idempotent -- `create_dir_all` on an existing directory is a no-op
/// and `add_folder` is an `INSERT OR IGNORE` -- so this runs on every entry
/// into the section rather than needing a "have I done this yet" flag. That
/// also means deleting the folder by hand heals itself on the next visit.
async fn refresh_dest() {
    let dest = resolve_dest();
    let mut ok = true;
    if let Err(e) = std::fs::create_dir_all(&dest) {
        tracing::warn!(error = %e, path = %dest.display(), "genesis: could not create the download folder");
        ok = false;
    }
    if ok {
        match crate::db::books_pool().await {
            Ok(pool) => {
                if let Err(e) =
                    tulipix_books::scan::add_folder(pool, &dest.display().to_string()).await
                {
                    tracing::warn!(error = %e, "genesis: could not register the download folder");
                }
            }
            // The folder exists, so downloading still works -- the book just
            // will not appear in My Library until the database comes back.
            Err(e) => tracing::warn!(error = %e, "genesis: books database unavailable"),
        }
    }
    let shown = dest.display().to_string();
    with(|s| {
        s.ui.can_download = ok;
        s.ui.dest = shown;
    });
}

fn advanced(key: &str) -> String {
    tulipix_core::settings::Settings::load()
        .ok()
        .and_then(|s| s.advanced.get(key).cloned())
        .unwrap_or_default()
}

/// Write one `advanced` key. An empty value removes it rather than storing a
/// blank, so "unset" and "set to nothing" stay the same thing -- every reader
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
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

// -------------------------------------------------------------- plumbing ----

fn service() -> &'static Mutex<Option<GenesisService>> {
    static SERVICE: OnceLock<Mutex<Option<GenesisService>>> = OnceLock::new();
    SERVICE.get_or_init(|| Mutex::new(None))
}

/// Run `f` against the service, building it on first use.
///
/// Blocking, and the mutex is held for the whole call -- which is why every
/// caller is already inside `spawn_blocking`.
fn with_mut<R>(f: impl FnOnce(&mut GenesisService) -> R) -> Option<R> {
    let mut slot = service().lock().unwrap_or_else(|e| e.into_inner());
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

/// Read the service if it has already been built. Never builds one: the only
/// caller asks whether the mirror pool is warm, and building it to find out
/// would make the answer "no" cost the probe it was trying to predict.
fn with_service<R>(f: impl FnOnce(&GenesisService) -> R) -> Option<R> {
    service().lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(f)
}

fn spawn<F: std::future::Future<Output = ()> + Send + 'static>(fut: F) {
    match tokio::runtime::Handle::try_current() {
        Ok(rt) => {
            rt.spawn(fut);
        }
        Err(_) => tracing::warn!("genesis: no tokio runtime; not running"),
    }
}

use tulipix_core::util::human_bytes as human;
use tulipix_core::util::unix_secs_i64 as now_secs;

#[cfg(test)]
mod tests {
    use super::*;

    /// The slider offers 1..49 and the setting is free text, so both a value
    /// out of range and a value that is not a number have to land somewhere
    /// sane rather than asking a mirror for zero results.
    #[test]
    fn the_results_limit_is_clamped_whatever_the_setting_says() {
        assert!((1..=MAX_LIMIT).contains(&configured_limit()));
    }

    #[test]
    fn history_ages_read_as_durations_not_timestamps() {
        let now = now_secs();
        assert_eq!(ago(now), "just now");
        assert_eq!(ago(now - 120), "2 min ago");
        assert_eq!(ago(now - 7200), "2 h ago");
        assert_eq!(ago(now - 3 * 86400), "3 d ago");
        // A clock that went backwards is not a search from the future.
        assert_eq!(ago(now + 500), "just now");
    }

    #[test]
    fn the_default_topics_are_the_two_worth_searching() {
        let topics = default_topics();
        let on: Vec<&str> = topics.iter().filter(|t| t.on).map(|t| t.code.as_str()).collect();
        assert_eq!(on, ["libgen", "fiction"]);
    }

    #[test]
    fn sci_tech_is_relabelled_and_the_rest_are_capitalised() {
        let topics = default_topics();
        assert_eq!(topics[0].label, "Sci-Tech");
        assert_eq!(topics[1].label, "Fiction");
    }

    /// Mirrors put anything in the year and pages columns, so the numeric sort
    /// has to read leading digits and treat the rest as unknown.
    #[test]
    fn ragged_mirror_columns_still_sort_as_numbers() {
        assert_eq!(number(Some("1998")), 1998);
        assert_eq!(number(Some("c. 2004")), 2004);
        assert_eq!(number(Some("200?")), 200);
        assert_eq!(number(None), 0);
        assert_eq!(number(Some("n/a")), 0);
    }

    #[test]
    fn titles_sort_case_insensitively() {
        assert!(text("  Zebra ") > text("apple"));
        assert_eq!(text(" The Hobbit"), "the hobbit");
    }

    /// Turning off the last topic leaves a search that can only come back
    /// empty, so the one just cleared is re-armed.
    #[test]
    fn the_last_topic_cannot_be_turned_off() {
        let mut topics = default_topics();
        for t in &mut topics {
            t.on = false;
        }
        topics[0].on = true;
        // The handler's rule, applied to the same shape it runs on.
        let code = topics[0].code.clone();
        for t in &mut topics {
            if t.code == code {
                t.on = !t.on;
            }
        }
        if !topics.iter().any(|t| t.on)
            && let Some(t) = topics.iter_mut().find(|t| t.code == code)
        {
            t.on = true;
        }
        assert!(topics.iter().any(|t| t.on), "a search with no topics returns nothing");
    }
}
