//! Stream Plus: the anime lane and the second provider stack.
//!
//! Shape copied from `vid_stream.rs` on purpose — one shared session, one
//! action epoch, the same poster cache. What is *not* shared is any behaviour:
//! the two tabs have separate catalogues, separate tables and separate
//! settings, and only the DB file, the cover directory and mpv in common.
//!
//! Ported from `tulipix_sec_videos::splus`, which cannot be linked here (slint).
//! The one structural change is that every handler awaits inline rather than
//! spawning onto the UI loop: a bridge call is already on a worker thread, and
//! whatever it writes into the session is in the snapshot it returns.

use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tulipix_videos::splus::{self, Episode, EpisodeRef, Title, source::Playable};

use crate::api::videos::{
    self as v, Session, SplusCard, SplusChip, SplusDownload, SplusEpisode, SplusFile, SplusHealth,
    SplusHistory, SplusView, VideosCmd, VideosEvent, with,
};
use crate::vid_stream::{cache_covers, path_str};

/// How many history rows the Library page holds.
const HISTORY_MAX: i64 = 200;
/// Results per search page. AniList caps a page at 50.
const PAGE_SIZE: i64 = 24;
/// Shelf sizes on the home screen.
const SHELF: i64 = 18;
/// The hero rota — enough to feel like one, few enough that the covers are a
/// single burst.
const FEATURED: usize = 6;
/// Rows in the Continue shelf.
const CONTINUE: i64 = 12;
/// Recent search terms under the box.
const RECENT: i64 = 8;

// ---------------------------------------------------------------- state ----

/// Everything the tab is currently looking at.
///
/// One struct rather than a static each: every handler needs two or three of
/// these together, and a torn read between them is how a detail view ends up
/// resolving the previous title's episode.
#[derive(Default)]
pub(crate) struct SplusSession {
    /// Search results, in the order the grid draws them.
    pub results: Vec<Title>,
    pub query: String,
    pub page: i64,
    pub more: bool,
    /// Home shelves, keyed the same way results are.
    pub featured: Vec<Title>,
    pub trending: Vec<Title>,
    pub seasonal: Vec<Title>,
    /// The open title, and what is selected inside it.
    pub open: Option<Title>,
    pub episodes: Vec<Episode>,
    pub season: i64,
    pub episode: i64,
    pub audio: String,
    /// Files resolved for the current episode, and which one is picked.
    pub files: Vec<Playable>,
    pub file: usize,
    /// Subtitle tracks for the current episode, and which one is picked.
    pub subs: Vec<splus::subs::Track>,
    pub sub: usize,
    /// Skip spans for the current episode.
    pub spans: Vec<splus::aniskip::Span>,
}

/// Bumped by every action that starts a network round trip. A slow search that
/// lands after the user has moved on is dropped rather than painting over what
/// they are looking at now — the rule the Stream tab and Live TV both keep.
static EPOCH: AtomicU64 = AtomicU64::new(0);

fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

fn is_current(epoch: u64) -> bool {
    EPOCH.load(Ordering::SeqCst) == epoch
}

/// Bumped by every play. Switching episodes kills the running mpv, so the old
/// process's exit hook fires *after* the new one started — without this it
/// would write the previous episode's position over the new one's.
static PLAY_GEN: AtomicU64 = AtomicU64::new(0);

/// What the tab opens with. Every value here is a stored preference, so the
/// settings page is correct before it has ever been visited.
pub(crate) fn init_view(view: &mut SplusView) {
    view.view = "home".into();
    view.lane = "anime".into();
    view.sort = "added".into();
    view.lib_tab = "saved".into();
    view.d_file = -1;
    push_prefs(view);
}

// -------------------------------------------------------------- dispatch ----

pub(crate) async fn dispatch(cmd: &VideosCmd) -> Result<()> {
    match cmd {
        VideosCmd::SplusSetView { view } => with(|s| s.ui.splus.view = view.clone()),
        VideosCmd::SplusHomeLoad => load_home().await,
        VideosCmd::SplusSearch { query } => {
            with(|s| {
                s.sp.query = query.clone();
                s.sp.page = 1;
                s.ui.splus.query = query.clone();
                s.ui.splus.view = "search".into();
            });
            run_search(query.clone(), false).await;
        }
        VideosCmd::SplusSetLane { lane } => {
            let query = with(|s| {
                s.ui.splus.lane = lane.clone();
                s.sp.page = 1;
                s.sp.query.clone()
            });
            run_search(query, false).await;
        }
        VideosCmd::SplusSuggest { query } => suggest(query).await,
        VideosCmd::SplusLoadMore => {
            let query = with(|s| {
                s.sp.page += 1;
                s.sp.query.clone()
            });
            run_search(query, true).await;
        }
        VideosCmd::SplusRecentClear => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::library::clear_recent(pool).await;
            }
            with(|s| s.ui.splus.recent.clear());
        }

        // ---- detail ----
        VideosCmd::SplusOpenCard { key } => open_card(key).await,
        VideosCmd::SplusBack => with(|s| s.ui.splus.view = "search".into()),
        VideosCmd::SplusSetAudio { id } => {
            with(|s| s.sp.audio = id.clone());
            load_detail().await;
        }
        VideosCmd::SplusSetSeason { id } => {
            with(|s| s.sp.season = id.parse::<i64>().unwrap_or(1).max(1));
            load_detail().await;
        }
        VideosCmd::SplusSetEpisode { episode } => {
            with(|s| {
                s.sp.episode = (*episode).max(1);
                s.ui.splus.d_episode = s.sp.episode;
            });
            resolve().await;
        }
        VideosCmd::SplusSetFile { index } => {
            with(|s| {
                let i = (*index).max(0) as usize;
                if i < s.sp.files.len() {
                    s.sp.file = i;
                    s.ui.splus.d_file = i as i64;
                }
            });
        }
        VideosCmd::SplusSetSub { id } => {
            with(|s| {
                let i = id.parse::<usize>().unwrap_or(0);
                if i < s.sp.subs.len() {
                    s.sp.sub = i;
                    push_subs(s);
                }
            });
        }
        VideosCmd::SplusToggleSave => {
            let Some(t) = with(|s| s.sp.open.clone()) else { return Ok(()) };
            let pool = v::pool().await?;
            match splus::library::toggle_saved(pool, &t).await {
                Ok(now_saved) => with(|s| s.ui.splus.d_saved = now_saved),
                Err(e) => tracing::warn!(error = %e, "splus: could not save the title"),
            }
        }
        VideosCmd::SplusRetrySource => resolve().await,

        // ---- playback ----
        VideosCmd::SplusPlay => play().await,
        VideosCmd::SplusCast => {
            // A resolved stream is already a public URL, so casting is the
            // Stream tab's problem exactly as it stands — no local media server
            // is involved.
            let source = with(|s| s.sp.files.get(s.sp.file).map(|f| f.source).unwrap_or(""));
            set_status(&format!("Casting is set up in Stream Setting · {source}"));
        }
        VideosCmd::SplusAddToLibrary => set_status(
            "Downloads land in the library when “Download into the library” is on.",
        ),

        // ---- library ----
        VideosCmd::SplusLibraryLoad => library_reload().await,
        VideosCmd::SplusSetLibTab { tab } => with(|s| s.ui.splus.lib_tab = tab.clone()),
        VideosCmd::SplusSetSort { key } => {
            with(|s| s.ui.splus.sort = key.clone());
            library_reload().await;
        }
        VideosCmd::SplusSavedRemove { key } => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::library::remove_saved(pool, key).await;
            }
            library_reload().await;
        }
        VideosCmd::SplusHistoryPlay { key } => play_from_history(key.clone()).await,
        VideosCmd::SplusHistoryRemove { key } => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::library::remove_history(pool, key).await;
            }
            library_reload().await;
        }
        VideosCmd::SplusHistoryClear => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::library::clear_history(pool).await;
            }
            library_reload().await;
        }

        // ---- downloads ----
        VideosCmd::SplusDownload => download_one().await,
        VideosCmd::SplusDownloadSeason => download_season().await,
        VideosCmd::SplusDownloadsLoad => downloads_reload().await,
        VideosCmd::SplusDlCancel { id } => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::downloads::cancel(pool, *id).await;
            }
            downloads_reload().await;
        }
        VideosCmd::SplusDlRetry { id } => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::downloads::retry(pool, *id).await;
                splus::downloads::pump(pool).await;
            }
            downloads_reload().await;
        }
        VideosCmd::SplusDlDelete { id } => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::downloads::delete_file(pool, *id).await;
            }
            downloads_reload().await;
        }
        VideosCmd::SplusDlCancelAll => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::downloads::cancel_all(pool).await;
            }
            downloads_reload().await;
        }
        VideosCmd::SplusDlClear => {
            if let Ok(pool) = v::pool().await {
                let _ = splus::downloads::clear_finished(pool).await;
            }
            downloads_reload().await;
        }
        VideosCmd::SplusDlPlay { id } => {
            let pool = v::pool().await?;
            let Some(row) = splus::downloads::list(pool).await.into_iter().find(|r| r.id == *id)
            else {
                return Ok(());
            };
            if row.dest.is_empty() {
                return Ok(());
            }
            // A finished download is an ordinary local file — it plays through
            // exactly the path the library uses.
            crate::vmpv::play(row.dest.clone(), None, None, Vec::new(), None);
            set_status(&format!("Playing {}", row.title));
        }
        VideosCmd::SplusDlReveal { id } => {
            let pool = v::pool().await?;
            if let Some(row) = splus::downloads::list(pool).await.into_iter().find(|r| r.id == *id)
            {
                let file = std::path::Path::new(&row.dest);
                if let Err(e) = tulipix_platform::fm::reveal_in_file_manager(file) {
                    tracing::warn!(error = %e, "splus: could not reveal the file");
                }
            }
        }

        // ---- settings ----
        VideosCmd::SplusSettingsLoad => {
            with(|s| push_prefs(&mut s.ui.splus));
            reload_health(false).await;
        }
        VideosCmd::SplusSetSourceEnabled { id, on } => {
            splus::prefs::set_source_enabled(id, *on);
            reload_health(false).await;
        }
        VideosCmd::SplusMoveSource { id, delta } => {
            let mut order = current_order();
            if let Some(at) = order.iter().position(|s| s == id) {
                let to = (at as i64 + delta).clamp(0, order.len() as i64 - 1) as usize;
                let moved = order.remove(at);
                order.insert(to, moved);
                splus::prefs::set_source_order(&order);
            }
            reload_health(false).await;
        }
        VideosCmd::SplusSourceTest | VideosCmd::SplusHostsCheck => reload_health(true).await,
        VideosCmd::SplusSourceReset => {
            splus::prefs::set_source_order(&[]);
            if let Ok(pool) = v::pool().await {
                let _ = splus::health::reset(pool).await;
            }
            reload_health(false).await;
        }
        VideosCmd::SplusSetFlag { key, on } => {
            match key.as_str() {
                "skip_op_ed" => splus::prefs::set_skip_op_ed(*on),
                "hide_finished" => splus::prefs::set_hide_finished(*on),
                "subs_wyzie" => splus::prefs::set_subs_wyzie(*on),
                "subs_subdl" => splus::prefs::set_subs_subdl(*on),
                "subs_with_file" => splus::prefs::set_subs_with_file(*on),
                "into_library" => splus::prefs::set_into_library(*on),
                "notify_download" => splus::prefs::set_notify_download_done(*on),
                "notify_episode" => splus::prefs::set_notify_new_episode(*on),
                "allow_adult" => splus::prefs::set_allow_adult(*on),
                other => tracing::warn!(key = other, "splus: unknown flag"),
            }
            with(|s| push_prefs(&mut s.ui.splus));
        }
        VideosCmd::SplusSetText { key, value } => {
            match key.as_str() {
                "audio_pref" => splus::prefs::set_audio_pref(value),
                "subs_lang" => splus::prefs::set_subs_lang(value),
                "age_max" => splus::prefs::set_age_max(value),
                "age_system" => splus::prefs::set_age_system(value),
                other => tracing::warn!(key = other, "splus: unknown setting"),
            }
            with(|s| push_prefs(&mut s.ui.splus));
        }
        VideosCmd::SplusSetNum { key, value } => {
            match key.as_str() {
                "autoplay_seconds" => splus::prefs::set_autoplay_seconds(*value),
                "watched_threshold" => splus::prefs::set_watched_threshold(*value),
                "quality" => splus::prefs::set_preferred_height(*value as i32),
                "slots" => splus::prefs::set_download_slots(*value),
                other => tracing::warn!(key = other, "splus: unknown setting"),
            }
            with(|s| push_prefs(&mut s.ui.splus));
        }
        VideosCmd::SplusSetDownloadDir { path } => {
            splus::prefs::set_download_dir(path);
            with(|s| push_prefs(&mut s.ui.splus));
        }
        VideosCmd::SplusServersOpened => {
            let text = splus::prefs::extra_hosts().join("\n");
            with(|s| {
                s.ui.splus.hosts_text =
                    if text.is_empty() { default_host_block() } else { text.clone() };
                s.ui.splus.hosts_saved = false;
            });
            reload_health(false).await;
        }
        VideosCmd::SplusHostsSave { text } => {
            splus::prefs::set_extra_hosts(text);
            with(|s| s.ui.splus.hosts_saved = true);
            reload_health(false).await;
        }
        VideosCmd::SplusHostsReset => {
            splus::prefs::set_extra_hosts("");
            with(|s| {
                s.ui.splus.hosts_text = default_host_block();
                s.ui.splus.hosts_saved = false;
            });
        }
        // Nothing here — the caller has already tried the other two pages.
        _ => {}
    }
    Ok(())
}

/// Opening the tab: clean up anything a previous session left mid-flight, then
/// draw the shelves if they are not already up.
pub(crate) async fn enter() {
    with(|s| push_prefs(&mut s.ui.splus));
    if let Ok(pool) = v::pool().await {
        splus::downloads::reconcile(pool).await;
        splus::downloads::pump(pool).await;
    }
    if with(|s| s.sp.trending.is_empty()) {
        load_home().await;
    }
    downloads_reload().await;
}

/// The URL of the picked file, for Dart's clipboard. Nothing to resolve — the
/// Stream Plus sources hand back direct links.
pub(crate) fn picked_link() -> String {
    with(|s| s.sp.files.get(s.sp.file).map(|f| f.url.clone()).unwrap_or_default())
}

// --------------------------------------------------------------- helpers ----

fn set_status(text: &str) {
    with(|s| {
        s.ui.splus.status = text.to_string();
        s.ui.splus.busy = false;
    });
}

fn busy(text: &str) {
    with(|s| {
        s.ui.splus.status = text.to_string();
        s.ui.splus.busy = true;
    });
}

/// Find a title by the key the UI hands back.
///
/// Dart never holds a `Title`; it holds a key, and every list the backend
/// painted is searched for it. That keeps the model one-way.
fn find(s: &SplusSession, key: &str) -> Option<Title> {
    for list in [&s.results, &s.featured, &s.trending, &s.seasonal] {
        if let Some(t) = list.iter().find(|t| title_key(t) == key) {
            return Some(t.clone());
        }
    }
    s.open.as_ref().filter(|t| title_key(t) == key).cloned()
}

fn title_key(t: &Title) -> String {
    match (t.anilist_id, t.tmdb_id) {
        (Some(a), _) => format!("al:{a}"),
        (None, Some(x)) => format!("tmdb:{x}"),
        _ => format!("q:{}", t.title),
    }
}

/// The episode the detail view is pointing at right now.
fn current_ref(s: &SplusSession) -> Option<EpisodeRef> {
    let title = s.open.clone()?;
    Some(EpisodeRef {
        title,
        season: s.season.max(1),
        episode: s.episode.max(1),
        audio: if s.audio.is_empty() { "sub".into() } else { s.audio.clone() },
    })
}

/// The `EpisodeRef` and file the play/download handlers act on.
fn selected() -> Option<(EpisodeRef, Playable)> {
    with(|s| {
        let ep = current_ref(&s.sp)?;
        let file = s.sp.files.get(s.sp.file).cloned()?;
        Some((ep, file))
    })
}

/// Turn titles into cards, fetching their posters first. One await for the
/// whole batch, so a shelf of twenty costs one burst rather than twenty
/// sequential round trips.
async fn cards(titles: &[Title], saved_keys: &[String]) -> Vec<SplusCard> {
    let covers = cache_covers(titles.iter().map(|t| t.cover_url.clone()).collect()).await;
    titles
        .iter()
        .zip(covers)
        .map(|(t, cover)| {
            let key = title_key(t);
            SplusCard {
                saved: saved_keys.iter().any(|k| *k == key),
                key,
                title: t.display_title().to_string(),
                note: note_line(t),
                poster: path_str(cover),
                badge: t.format.clone(),
                progress: 0.0,
                blocked: false,
            }
        })
        .collect()
}

/// "2023 · 28 ep · ★ 8.4" — blank parts drop out rather than leaving a dangling
/// separator.
fn note_line(t: &Title) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(y) = t.year {
        parts.push(y.to_string());
    }
    if let Some(n) = t.episodes.filter(|n| *n > 1) {
        parts.push(format!("{n} ep"));
    }
    if let Some(s) = t.score {
        parts.push(format!("★ {:.1}", s as f64 / 10.0));
    }
    if let Some(ep) = t.next_episode.filter(|_| t.next_airing_at.is_some()) {
        parts.push(format!("ep {ep} next"));
    }
    parts.join(" · ")
}

/// The full meta line on a detail hero.
fn meta_line(t: &Title) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(y) = t.year {
        parts.push(y.to_string());
    }
    if !t.format.is_empty() {
        parts.push(t.format.clone());
    }
    if let Some(n) = t.episodes {
        parts.push(format!("{n} episodes"));
    }
    if let Some(s) = t.score {
        parts.push(format!("★ {:.1}", s as f64 / 10.0));
    }
    if !t.genres.is_empty() {
        parts.push(t.genres.iter().take(3).cloned().collect::<Vec<_>>().join(", "));
    }
    if let Some(a) = t.anilist_id {
        parts.push(format!("AniList {a}"));
    }
    if let Some(m) = t.mal_id {
        parts.push(format!("MAL {m}"));
    }
    parts.join("  ·  ")
}

async fn saved_keys() -> Vec<String> {
    let Ok(pool) = v::pool().await else { return Vec::new() };
    splus::library::saved(pool, splus::library::Sort::Added)
        .await
        .into_iter()
        .map(|s| s.key)
        .collect()
}

// ------------------------------------------------------------ home/search ----

/// Featured, trending and this season, plus whatever is part-watched.
///
/// One epoch for the whole set: re-entering the tab while the first load is
/// still in flight must not paint two overlapping sets of shelves.
async fn load_home() {
    let epoch = next_epoch();
    busy("Loading…");

    let keys = saved_keys().await;
    let (season, year) = current_season();

    let (trending, seasonal) = tokio::join!(
        splus::anilist::trending(SHELF),
        splus::anilist::seasonal(season, year, SHELF),
    );
    if !is_current(epoch) {
        return;
    }
    let trending = trending.unwrap_or_else(|e| {
        tracing::debug!(error = %e, "splus: trending unavailable");
        Vec::new()
    });
    let seasonal = seasonal.unwrap_or_default();
    let featured: Vec<Title> = trending.iter().take(FEATURED).cloned().collect();

    let feat_cards = cards(&featured, &keys).await;
    let trend_cards = cards(&trending, &keys).await;
    let seas_cards = cards(&seasonal, &keys).await;
    let cont = history_rows(true, CONTINUE).await;
    let recent = recent_terms().await;

    if !is_current(epoch) {
        return;
    }
    with(|s| {
        s.sp.featured = featured;
        s.sp.trending = trending;
        s.sp.seasonal = seasonal;
        let sp = &mut s.ui.splus;
        sp.featured = feat_cards;
        sp.trending = trend_cards;
        sp.seasonal = seas_cards;
        sp.season_label = format!("{} {year}", pretty_season(season));
        sp.continue_rows = cont;
        sp.recent = recent;
        sp.busy = false;
        sp.status.clear();
    });
}

/// The civil month, without pulling in a date crate: the year length is only
/// needed to pick a season name, so a 365.2425-day mean is close enough and
/// wrong for at most a day at a boundary.
fn current_season() -> (&'static str, i64) {
    let days = v::now_secs() / 86_400;
    let year = 1970 + (days as f64 / 365.2425) as i64;
    let month = (((days as f64 % 365.2425) / 30.44) as u32 % 12) + 1;
    (splus::anilist::season_of(month), year)
}

fn pretty_season(season: &str) -> &'static str {
    match season {
        "WINTER" => "Winter",
        "SPRING" => "Spring",
        "SUMMER" => "Summer",
        _ => "Fall",
    }
}

async fn recent_terms() -> Vec<String> {
    let Ok(pool) = v::pool().await else { return Vec::new() };
    splus::library::recent(pool, RECENT).await
}

/// Suggestions come from the recent list only. Asking AniList on every
/// keystroke would burn the 30-per-minute budget in one word typed.
async fn suggest(query: &str) {
    let q = query.to_lowercase();
    let Ok(pool) = v::pool().await else { return };
    let recent = splus::library::recent(pool, 40).await;
    let hits: Vec<String> = recent
        .into_iter()
        .filter(|r| q.len() >= 2 && r.to_lowercase().contains(&q))
        .take(5)
        .collect();
    with(|s| s.ui.splus.suggestions = hits);
}

/// Run a search. `append` keeps what is on screen and adds a page to it.
async fn run_search(query: String, append: bool) {
    if query.trim().is_empty() {
        with(|s| {
            s.sp.results.clear();
            let sp = &mut s.ui.splus;
            sp.results.clear();
            sp.results_total = 0;
            sp.results_more = false;
        });
        return;
    }
    let epoch = next_epoch();
    busy(&format!("Searching for “{query}”…"));

    let page = with(|s| s.sp.page.max(1));
    let found = splus::anilist::search(&query, page, PAGE_SIZE).await;
    if !is_current(epoch) {
        return;
    }
    let (mut titles, more) = match found {
        Ok(v) => v,
        Err(e) => return set_status(&format!("Search failed — {e}")),
    };

    if let Ok(pool) = v::pool().await {
        splus::library::note_term(pool, &query).await;
    }
    let keys = saved_keys().await;

    // The age gate applies before anything is drawn, not after: a blocked title
    // must not appear and then vanish.
    titles.retain(allowed);

    let all: Vec<Title> = if append {
        let mut prev = with(|s| s.sp.results.clone());
        prev.extend(titles);
        prev
    } else {
        titles
    };
    let drawn = cards(&all, &keys).await;
    let recent = recent_terms().await;
    if !is_current(epoch) {
        return;
    }
    let empty = all.is_empty();
    with(|s| {
        s.sp.results = all;
        s.sp.more = more;
        let sp = &mut s.ui.splus;
        sp.results_total = drawn.len() as i64;
        sp.results = drawn;
        sp.results_more = more;
        sp.recent = recent;
        sp.busy = false;
        sp.status = if empty { "Nothing matched that.".into() } else { String::new() };
    });
}

// ---------------------------------------------------------------- detail ----

async fn open_card(key: &str) {
    let Some(t) = with(|s| find(&s.sp, key)) else { return };
    with(|s| {
        s.sp.open = Some(t.clone());
        s.sp.season = 1;
        s.sp.episode = 1;
        s.sp.audio = splus::prefs::audio_pref().order()[0].to_string();
        s.sp.files.clear();
        s.sp.file = 0;
        s.sp.subs.clear();
        s.sp.spans.clear();
        let sp = &mut s.ui.splus;
        sp.view = "detail".into();
        sp.d_title = t.display_title().to_string();
        sp.d_meta = meta_line(&t);
        sp.d_overview = t.overview.clone();
        sp.d_badge = t.format.clone();
        sp.d_cover.clear();
        sp.d_episodes.clear();
        sp.d_files.clear();
        sp.d_subs.clear();
        sp.d_skip_note.clear();
        sp.d_resolved.clear();
        sp.d_file = -1;
    });
    // The hero art is the card's, so it is already on disk in the common case.
    let cover = crate::vid_stream::cache_cover(&t.cover_url).await;
    with(|s| s.ui.splus.d_cover = path_str(cover));
    load_detail().await;
}

/// Episode list for the current title, season and audio, then resolve the
/// episode the user is on.
async fn load_detail() {
    let epoch = next_epoch();
    let Some((t, audio)) = with(|s| {
        let t = s.sp.open.clone()?;
        Some((t, s.sp.audio.clone()))
    }) else {
        return;
    };
    with(|s| s.ui.splus.d_resolving = true);
    let Ok(pool) = v::pool().await else { return };

    let saved = splus::library::is_saved(pool, &t).await;
    with(|s| s.ui.splus.d_saved = saved);

    let (mut eps, src) = splus::source::episodes_any(pool, &t, &audio).await.unwrap_or_else(|e| {
        tracing::debug!(error = %e, "splus: no episode list");
        (Vec::new(), "")
    });
    if !is_current(epoch) {
        return;
    }

    // Per-episode progress comes from our own table, not the provider.
    let seen = splus::library::progress_map(pool, &t).await;
    for e in &mut eps {
        e.progress = seen.get(&e.number).copied().unwrap_or(0.0);
    }
    if splus::prefs::hide_finished() {
        eps.retain(|e| e.progress < 0.98);
    }

    // Resume where they were, not at episode one.
    let start = eps
        .iter()
        .find(|e| e.progress > 0.02 && e.progress < 0.98)
        .or_else(|| eps.iter().find(|e| e.progress <= 0.02))
        .map(|e| e.number)
        .unwrap_or(1);

    let rows = episode_rows(&eps).await;
    if !is_current(epoch) {
        return;
    }
    let chips = audio_chips(&t, &audio);
    with(|s| {
        s.sp.episodes = eps;
        s.sp.episode = start;
        let sp = &mut s.ui.splus;
        sp.d_episode = start;
        sp.d_episodes = rows;
        sp.d_resolved = if src.is_empty() { String::new() } else { format!("via {src}") };
        sp.d_audio = chips;
        // One season for now: AniList models a second cour as its own Media, so
        // a season switcher here would be lying about what the id points at.
        sp.d_seasons.clear();
    });
    resolve().await;
}

async fn episode_rows(eps: &[Episode]) -> Vec<SplusEpisode> {
    let thumbs = cache_covers(eps.iter().map(|e| e.thumb_url.clone()).collect()).await;
    eps.iter()
        .zip(thumbs)
        .map(|(e, th)| SplusEpisode {
            number: e.number,
            title: e.title.clone(),
            thumb: path_str(th),
            progress: e.progress as f64,
        })
        .collect()
}

/// Audio is only meaningful on the anime lane; a TMDB title has one track.
fn audio_chips(t: &Title, audio: &str) -> Vec<SplusChip> {
    if t.anilist_id.is_none() {
        return Vec::new();
    }
    ["sub", "dub"]
        .iter()
        .map(|a| SplusChip {
            id: (*a).into(),
            label: if *a == "sub" { "Sub".into() } else { "Dub".into() },
            active: *a == audio,
        })
        .collect()
}

/// Ask each source in turn for the current episode, then look for subtitles and
/// skip times alongside.
async fn resolve() {
    let epoch = next_epoch();
    let Some(ep) = with(|s| current_ref(&s.sp)) else { return };
    with(|s| {
        s.ui.splus.d_resolving = true;
        s.ui.splus.d_files.clear();
    });

    let Ok(pool) = v::pool().await else { return };
    let started = std::time::Instant::now();
    let files = splus::source::resolve_any(pool, &ep).await.unwrap_or_default();
    if !is_current(epoch) {
        return;
    }

    // Subtitles and skip times are optional garnish — neither failing is a
    // reason to leave the user without a Play button.
    let subs = splus::subs::search(&ep).await;
    let spans = match ep.title.mal_id {
        Some(mal) if splus::prefs::skip_op_ed() => {
            splus::aniskip::spans(mal, ep.episode, 0.0).await.unwrap_or_default()
        }
        _ => Vec::new(),
    };
    if !is_current(epoch) {
        return;
    }

    let pick = preferred(&files);
    let rows: Vec<SplusFile> = files
        .iter()
        .enumerate()
        .map(|(i, f)| SplusFile {
            index: i as i64,
            label: format!("{} · {}", f.label, f.source),
            sub: f.sub_label(),
            uploader: f.source.to_string(),
            subs: if subs.is_empty() {
                "No subs".to_string()
            } else {
                format!("{} subs", subs.len())
            },
            has_subs: !subs.is_empty(),
            failed: false,
            note: String::new(),
        })
        .collect();
    let skip_note = match spans.iter().find(|s| s.kind == "op") {
        Some(s) => format!("AniSkip · OP {} → {}", mmss(s.start), mmss(s.end)),
        None => String::new(),
    };
    let resolved = format!(
        "resolved in {:.1} s · {} files",
        started.elapsed().as_secs_f64(),
        files.len()
    );

    with(|s| {
        s.sp.files = files;
        s.sp.file = pick;
        s.sp.subs = subs;
        s.sp.sub = 0;
        s.sp.spans = spans;
        {
            let sp = &mut s.ui.splus;
            sp.d_files = rows;
            sp.d_file = pick as i64;
            sp.d_resolving = false;
            sp.d_resolved = resolved;
            sp.d_skip_note = skip_note;
        }
        push_subs(s);
    });
}

fn push_subs(s: &mut Session) {
    let picked = s.sp.sub;
    let rows: Vec<SplusChip> = s
        .sp
        .subs
        .iter()
        .enumerate()
        .map(|(i, t)| SplusChip {
            id: i.to_string(),
            label: format!("{} · {}", t.label, t.source),
            active: i == picked,
        })
        .collect();
    s.ui.splus.d_subs = rows;
}

/// The closest file at or below the preferred height, or the first available
/// when everything is above it.
fn preferred(files: &[Playable]) -> usize {
    let want = splus::prefs::preferred_height();
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.height > 0 && f.height <= want)
        .max_by_key(|(_, f)| f.height)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as i64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

// -------------------------------------------------------------- playback ----

async fn play() {
    let Some((ep, file)) = selected() else {
        return set_status("Nothing resolved to play yet.");
    };
    let epoch = PLAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let Ok(pool) = v::pool().await else { return };
    let resume = splus::library::resume_at(pool, &ep).await;

    let mut args: Vec<String> = Vec::new();
    // Some hosts only serve with the referer they handed the URL out under.
    for (k, val) in &file.headers {
        args.push(format!("--http-header-fields={k}: {val}"));
    }
    args.push(format!("--user-agent={}", tulipix_core::net::BROWSER_UA));
    // The picked subtitle rides along as a URL — mpv fetches it itself, so
    // nothing has to be written to disk to watch with subtitles.
    let (sub_url, spans) =
        with(|s| (s.sp.subs.get(s.sp.sub).map(|t| t.url.clone()), s.sp.spans.clone()));
    if let Some(url) = sub_url {
        args.push(format!("--sub-file={url}"));
    }
    // Skip the opening, but never yank someone backwards into it.
    args.extend(splus::aniskip::mpv_args(&spans, resume));
    args.push(format!("--force-media-title={} {}", ep.title.display_title(), ep.label()));

    set_status(&format!("Opening {} {}…", ep.title.display_title(), ep.label()));

    let ep2 = ep.clone();
    let source = file.source.to_string();
    let quality = file.label.clone();
    let on_end: crate::vmpv::PlaybackEnd = std::sync::Arc::new(move |pos: f64, dur: f64| {
        // Only the current episode's exit is allowed to write progress.
        if PLAY_GEN.load(Ordering::SeqCst) != epoch {
            return;
        }
        let ep3 = ep2.clone();
        let source = source.clone();
        let quality = quality.clone();
        tokio::spawn(async move {
            let Ok(pool) = v::pool().await else { return };
            if let Err(e) =
                splus::library::note_progress(pool, &ep3, &source, &quality, pos, dur).await
            {
                tracing::warn!(error = %e, "splus: progress not recorded");
            }
            // Refresh whatever the user is looking at now.
            library_reload().await;
            v::emit(VideosEvent::Changed);
        });
    });

    // mpv takes a URL exactly where it takes a path.
    crate::vmpv::play(file.url.clone(), resume, None, args, Some(on_end));
}

/// Play whatever a history row points at, resolving it again first — the URL a
/// provider handed out last week has almost certainly expired.
async fn play_from_history(key: String) {
    let Ok(pool) = v::pool().await else { return };
    let Some(row) =
        splus::library::history(pool, HISTORY_MAX).await.into_iter().find(|h| h.key == key)
    else {
        return;
    };
    // Reopening through the detail view is the honest path: it re-resolves,
    // re-fetches subtitles and re-reads skip times, none of which a stale
    // history row can carry.
    let title = match row.title_key.strip_prefix("al:").and_then(|s| s.parse::<i64>().ok()) {
        Some(id) => splus::anilist::by_id(id).await.ok().flatten(),
        None => None,
    };
    let Some(title) = title else {
        return set_status("That title could not be reopened.");
    };
    with(|s| {
        s.sp.open = Some(title.clone());
        s.sp.season = row.season;
        s.sp.episode = row.episode.max(1);
        s.sp.audio = if row.audio.is_empty() { "sub".into() } else { row.audio.clone() };
        let sp = &mut s.ui.splus;
        sp.view = "detail".into();
        sp.d_title = title.display_title().to_string();
        sp.d_meta = meta_line(&title);
        sp.d_overview = title.overview.clone();
        sp.d_badge = title.format.clone();
    });
    load_detail().await;
}

// --------------------------------------------------------------- library ----

/// Repaint both lists. Cheap enough to call after any action rather than
/// surgically patching one row — the whole library is a few dozen rows.
async fn library_reload() {
    let Ok(pool) = v::pool().await else { return };
    let sort = splus::library::Sort::parse(&with(|s| s.ui.splus.sort.clone()));
    let saved = splus::library::saved(pool, sort).await;
    let covers = cache_covers(saved.iter().map(|s| s.cover_url.clone()).collect()).await;
    let cards: Vec<SplusCard> = saved
        .iter()
        .zip(covers)
        .map(|(s, cover)| SplusCard {
            key: s.key.clone(),
            title: s.display().to_string(),
            note: saved_note(s),
            poster: path_str(cover),
            badge: s.format.clone(),
            progress: s.progress as f64,
            saved: true,
            blocked: false,
        })
        .collect();
    let history = history_rows(false, HISTORY_MAX).await;
    // Home's Continue shelf reads the same table, so it goes stale otherwise.
    let cont = history_rows(true, CONTINUE).await;
    with(|s| {
        let sp = &mut s.ui.splus;
        sp.saved = cards;
        sp.history = history;
        sp.continue_rows = cont;
    });
}

fn saved_note(s: &splus::library::Saved) -> String {
    if s.last_episode > 0 {
        let pct = (s.progress * 100.0).round() as i64;
        if s.format == "MOVIE" {
            return format!("{pct}%");
        }
        return format!("S{:02}E{:02} · {pct}%", s.last_season, s.last_episode);
    }
    match s.year {
        Some(y) if s.episodes > 0 => format!("{y} · {} ep", s.episodes),
        Some(y) => y.to_string(),
        None => "not started".into(),
    }
}

/// History rows for the UI. `only_unfinished` gives the Continue shelf.
async fn history_rows(only_unfinished: bool, limit: i64) -> Vec<SplusHistory> {
    let Ok(pool) = v::pool().await else { return Vec::new() };
    let rows = if only_unfinished {
        splus::library::continue_watching(pool, limit).await
    } else {
        splus::library::history(pool, limit).await
    };
    let covers = cache_covers(rows.iter().map(|h| h.cover_url.clone()).collect()).await;
    rows.iter()
        .zip(covers)
        .map(|(h, cover)| SplusHistory {
            key: h.key.clone(),
            title: format!("{} {}", h.title, h.kind.rsplit(" · ").next().unwrap_or(""))
                .trim()
                .to_string(),
            kind: h.kind.clone(),
            state: h.state.clone(),
            ago: h.when.clone(),
            detail: detail_line(h),
            poster: path_str(cover),
            progress: h.progress as f64,
            finished: h.finished,
        })
        .collect()
}

/// "AllManga · dub · 1080p" — whatever of it is known.
fn detail_line(h: &splus::library::HistoryRow) -> String {
    [h.source.as_str(), h.audio.as_str(), h.quality.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

// ------------------------------------------------------------- downloads ----

async fn download_one() {
    let Some((ep, file)) = selected() else {
        return set_status("Nothing resolved to download yet.");
    };
    let sub = with(|s| s.sp.subs.get(s.sp.sub).map(|t| t.url.clone()).unwrap_or_default());
    let Ok(pool) = v::pool().await else { return };
    match splus::downloads::enqueue(pool, &ep, &file, &sub, "").await {
        Ok(_) => {
            splus::downloads::pump(pool).await;
            set_status(&format!("Queued {} {}", ep.title.display_title(), ep.label()));
            downloads_reload().await;
        }
        Err(e) => set_status(&format!("Could not queue that — {e}")),
    }
}

async fn download_season() {
    let Some(base) = with(|s| current_ref(&s.sp)) else { return };
    let episodes: Vec<i64> = with(|s| s.sp.episodes.iter().map(|e| e.number).collect());
    if episodes.is_empty() {
        return set_status("No episode list to download.");
    }
    // One batch id so the whole season can be cancelled as one thing.
    let batch = format!("{}:{}", base.key(), v::now_secs());
    let Ok(pool) = v::pool().await else { return };
    let mut queued = 0usize;
    for n in episodes {
        let ep = EpisodeRef { episode: n, ..base.clone() };
        // Resolving every episode up front would be dozens of requests before a
        // single byte moves; resolve, queue, move on.
        let files = splus::source::resolve_any(pool, &ep).await.unwrap_or_default();
        let Some(file) = files.get(preferred(&files)) else { continue };
        if splus::downloads::enqueue(pool, &ep, file, "", &batch).await.is_ok() {
            queued += 1;
            splus::downloads::pump(pool).await;
            set_status(&format!("Queued {queued} episodes…"));
            downloads_reload().await;
            v::emit(VideosEvent::Changed);
        }
    }
    set_status(&format!("Queued {queued} episodes."));
    downloads_reload().await;
}

/// Repaint the list and the tab's running count.
async fn downloads_reload() {
    let Ok(pool) = v::pool().await else { return };
    let rows = splus::downloads::list(pool).await;
    let active = rows.iter().filter(|r| r.active()).count();
    let running: i64 = rows.iter().filter(|r| r.state == "running").map(|r| r.done_bytes).sum();

    let ui: Vec<SplusDownload> = rows
        .iter()
        .map(|r| SplusDownload {
            id: r.id,
            title: [r.title.as_str(), r.label.as_str(), r.quality.as_str()]
                .into_iter()
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(" · "),
            state: pretty_state(&r.state).to_string(),
            detail: dl_detail(r),
            progress: r.progress as f64,
            active: r.active(),
            done: r.done(),
            failed: r.failed(),
        })
        .collect();
    with(|s| {
        let sp = &mut s.ui.splus;
        sp.downloads = ui;
        sp.dl_active = active as i64;
        sp.dl_rate =
            if running > 0 { format!("{} done", human(running)) } else { String::new() };
    });
}

fn pretty_state(s: &str) -> &'static str {
    match s {
        "running" => "Downloading",
        "waiting" => "Waiting",
        "done" => "Saved",
        "missing" => "Missing",
        _ => "Failed",
    }
}

fn dl_detail(r: &splus::downloads::DownloadRow) -> String {
    if !r.detail.is_empty() && (r.failed() || r.done()) {
        return r.detail.clone();
    }
    if r.total_bytes > 0 {
        return format!("{} / {} · {}", human(r.done_bytes), human(r.total_bytes), r.source);
    }
    if r.done_bytes > 0 {
        return format!("{} · {}", human(r.done_bytes), r.source);
    }
    r.source.clone()
}

fn human(n: i64) -> String {
    splus::source::human_bytes(n)
}

// -------------------------------------------------------------- settings ----

/// The order the resolver would use right now, so the arrows move things
/// relative to what the user is looking at.
fn current_order() -> Vec<String> {
    let saved = splus::prefs::source_order();
    let all: Vec<String> = splus::source::all().iter().map(|s| s.id().to_string()).collect();
    let mut out: Vec<String> = saved.into_iter().filter(|s| all.contains(s)).collect();
    for id in all {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Repaint the health list. `test` actually pings every source first, which
/// takes as long as the slowest timeout — hence the flag.
async fn reload_health(test: bool) {
    let Ok(pool) = v::pool().await else { return };
    if test {
        with(|s| s.ui.splus.hosts_checking = true);
        for src in splus::source::all() {
            match src.ping().await {
                Ok(ms) => splus::health::note_ok(pool, src.id(), ms).await,
                Err(e) => {
                    splus::health::note_fail(pool, src.id(), &splus::source::short_error(&e)).await
                }
            }
        }
        with(|s| s.ui.splus.hosts_checking = false);
    }

    let order = current_order();
    let mut rows = splus::health::list(pool).await;
    rows.sort_by_key(|h| order.iter().position(|o| *o == h.source).unwrap_or(usize::MAX));
    let ui: Vec<SplusHealth> = rows
        .iter()
        .map(|h| SplusHealth {
            source: h.source.clone(),
            label: h.label.clone(),
            note: h.note.clone(),
            ok: h.ok,
            enabled: h.enabled,
            sunk: h.sunk,
        })
        .collect();
    with(|s| s.ui.splus.health = ui);
}

fn push_prefs(sp: &mut SplusView) {
    // The chip compares against the stored value, so this has to be the stored
    // spelling, not a prettified one.
    sp.audio_pref = match splus::prefs::audio_pref() {
        splus::AudioPref::DubThenSub => "dub_then_sub",
        splus::AudioPref::SubThenDub => "sub_then_dub",
    }
    .into();
    sp.skip_op_ed = splus::prefs::skip_op_ed();
    sp.watched_pct = (splus::prefs::watched_threshold() * 100.0).round() as i64;
    sp.autoplay_secs = splus::prefs::autoplay_seconds();
    sp.hide_finished = splus::prefs::hide_finished();
    sp.subs_wyzie = splus::prefs::subs_wyzie();
    sp.subs_subdl = splus::prefs::subs_subdl();
    sp.subs_lang = splus::prefs::subs_lang();
    sp.subs_with_file = splus::prefs::subs_with_file();
    sp.quality = splus::prefs::preferred_height() as i64;
    sp.into_library = splus::prefs::into_library();
    sp.slots = splus::prefs::download_slots() as i64;
    sp.notify_download = splus::prefs::notify_download_done();
    sp.notify_episode = splus::prefs::notify_new_episode();
    sp.download_dir = splus::prefs::download_dir().to_string_lossy().into_owned();
}

fn default_host_block() -> String {
    let mut lines: Vec<String> = vec!["https://api.allanime.day/api".into()];
    for h in splus::vidsrc::hosts() {
        lines.push(h.origin.to_string());
    }
    lines.join("\n")
}

/// The age gate.
///
/// The limit is compared on `parental::Rating`'s severity ladder — the app
/// already knows that TV-14 sits above PG — rather than on a second rating
/// model invented here. An unrated title is hidden when a limit is set, because
/// "we do not know" is not a reason to show it to a child; the one exception is
/// the anime lane, where AniList publishes no board certification at all and the
/// `isAdult` flag is the only signal there is.
fn allowed(t: &Title) -> bool {
    use tulipix_core::parental::Rating;

    if t.adult && !splus::prefs::allow_adult() {
        return false;
    }
    let limit = splus::prefs::age_max();
    let Some(cap) = Rating::parse(&limit).filter(|_| !limit.trim().is_empty()) else {
        return true;
    };
    match Rating::parse(&t.certification) {
        Some(r) if !t.certification.trim().is_empty() => r.severity() <= cap.severity(),
        _ => t.anilist_id.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_prefers_anilist_then_tmdb_then_the_title() {
        let al = Title { anilist_id: Some(7), tmdb_id: Some(9), ..Title::default() };
        assert_eq!(title_key(&al), "al:7");
        let tm = Title { tmdb_id: Some(9), ..Title::default() };
        assert_eq!(title_key(&tm), "tmdb:9");
        let raw = Title { title: "Dune".into(), ..Title::default() };
        assert_eq!(title_key(&raw), "q:Dune");
    }

    #[test]
    fn a_note_line_never_leaves_a_dangling_separator() {
        assert_eq!(note_line(&Title::default()), "");
        let t = Title { year: Some(2023), episodes: Some(1), ..Title::default() };
        // One episode is not worth a count — a film would read "1 ep".
        assert_eq!(note_line(&t), "2023");
    }

    #[test]
    fn the_preferred_file_is_the_best_at_or_below_the_cap() {
        let want = splus::prefs::preferred_height();
        let mk = |h: i32| Playable {
            height: h,
            ..Playable::new("u", splus::source::PlayableKind::Mp4, "test")
        };
        let files = vec![mk(480), mk(want), mk(want + 1000)];
        assert_eq!(preferred(&files), 1);
        // Everything above the cap falls back to the first row rather than to
        // nothing — a Play button that cannot play is worse than a big file.
        assert_eq!(preferred(&[mk(want + 1000)]), 0);
        assert_eq!(preferred(&[]), 0);
    }

    #[test]
    fn an_adult_title_is_gated_on_its_own_flag() {
        let t = Title { adult: true, anilist_id: Some(1), ..Title::default() };
        assert!(!allowed(&t));
    }

    #[test]
    fn a_season_name_is_one_of_the_four_anilist_uses() {
        let (season, year) = current_season();
        assert!(matches!(season, "WINTER" | "SPRING" | "SUMMER" | "FALL"), "{season}");
        assert!(year >= 2020 && year < 2200, "{year}");
    }

    #[test]
    fn a_timestamp_is_printed_as_minutes_and_seconds() {
        assert_eq!(mmss(0.0), "00:00");
        assert_eq!(mmss(-5.0), "00:00");
        assert_eq!(mmss(90.4), "01:30");
        assert_eq!(mmss(3599.0), "59:59");
    }
}
