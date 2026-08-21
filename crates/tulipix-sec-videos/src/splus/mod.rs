//! Stream Plus wiring — the window half of `tulipix_videos::splus`.
//!
//! Shape copied from `stream/` on purpose: one shared state, one action epoch,
//! and a file per surface. What is *not* copied is any of its behaviour — the
//! two tabs share the poster cache, the DB file and mpv, and nothing else.
//!
//! The poster cache in particular is deliberately the Stream tab's
//! (`crate::stream::cache_covers`): a second cache would be the clearest sign
//! that this is two apps rather than one.

use slint::{ModelRc, SharedString, VecModel};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tulipix_common::pool_for;
use tulipix_ui::*;
use tulipix_videos::splus::{self, Episode, EpisodeRef, Title, source::Playable};

pub mod detail;
pub mod downloads;
pub mod home;
pub mod library;
pub mod play;
pub mod settings;

/// How many history rows the Library page holds.
pub const HISTORY_MAX: i64 = 200;
/// Results per search page. AniList caps a page at 50.
pub const PAGE_SIZE: i64 = 24;

/// Everything the tab is currently looking at.
///
/// One lock rather than a field each: every handler needs two or three of these
/// together and a torn read between them is how a detail view ends up resolving
/// the previous title's episode.
#[derive(Default)]
pub struct State {
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

pub fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Bumped by every action that starts a network round trip.
///
/// A slow search that lands after the user has moved on is dropped rather than
/// painting over what they are looking at now — the same rule the Stream tab and
/// Live TV both keep.
pub static EPOCH: AtomicU64 = AtomicU64::new(0);

pub fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn is_current(epoch: u64) -> bool {
    EPOCH.load(Ordering::SeqCst) == epoch
}

/// Find a title by the key the UI hands back.
///
/// The UI never holds a `Title`; it holds a key, and every list the backend
/// pushed is searched for it. That keeps the model one-way — Slint never owns
/// anything Rust has to read back.
pub fn find(key: &str) -> Option<Title> {
    let st = state();
    for list in [&st.results, &st.featured, &st.trending, &st.seasonal] {
        if let Some(t) = list.iter().find(|t| title_key(t) == key) {
            return Some(t.clone());
        }
    }
    st.open.as_ref().filter(|t| title_key(t) == key).cloned()
}

pub fn title_key(t: &Title) -> String {
    match (t.anilist_id, t.tmdb_id) {
        (Some(a), _) => format!("al:{a}"),
        (None, Some(x)) => format!("tmdb:{x}"),
        _ => format!("q:{}", t.title),
    }
}

/// The episode the detail view is pointing at right now.
pub fn current_ref() -> Option<EpisodeRef> {
    let st = state();
    let title = st.open.clone()?;
    Some(EpisodeRef {
        title,
        season: st.season.max(1),
        episode: st.episode.max(1),
        audio: if st.audio.is_empty() { "sub".into() } else { st.audio.clone() },
    })
}

// ── model helpers ─────────────────────────────────────────────────────────

pub fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::new(VecModel::from(v))
}

pub fn strings(v: Vec<String>) -> ModelRc<SharedString> {
    model(v.into_iter().map(SharedString::from).collect::<Vec<_>>())
}

/// Turn titles into cards, fetching their posters first.
///
/// One await for the whole batch, so a shelf of twenty costs one burst rather
/// than twenty sequential round trips.
pub async fn cards(titles: &[Title], saved_keys: &[String]) -> Vec<SPlusCard> {
    let covers = crate::stream::cache_covers(
        titles.iter().map(|t| t.cover_url.clone()).collect(),
    )
    .await;
    titles
        .iter()
        .zip(covers)
        .map(|(t, cover)| {
            let key = title_key(t);
            SPlusCard {
                saved: saved_keys.iter().any(|k| *k == key),
                key: key.into(),
                title: t.display_title().into(),
                note: note_line(t).into(),
                poster: crate::stream::load_image(cover),
                badge: t.format.clone().into(),
                progress: 0.0,
                blocked: false,
            }
        })
        .collect()
}

/// "2023 · 28 ep · Sub + Dub" — blank parts drop out rather than leaving a
/// dangling separator.
pub fn note_line(t: &Title) -> String {
    let mut parts: Vec<String> = Vec::new();
    if let Some(y) = t.year {
        parts.push(y.to_string());
    }
    match t.episodes {
        Some(n) if n > 1 => parts.push(format!("{n} ep")),
        _ => {}
    }
    if let Some(s) = t.score {
        parts.push(format!("★ {:.1}", s as f64 / 10.0));
    }
    if let (Some(ep), Some(at)) = (t.next_episode, t.next_airing_at) {
        let _ = at;
        parts.push(format!("ep {ep} next"));
    }
    parts.join(" · ")
}

/// The full meta line on a detail hero.
pub fn meta_line(t: &Title) -> String {
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

/// Everything on entry: schema, queue reconciliation, and the first shelves.
pub fn wire(window: &MainWindow) {
    home::wire(window);
    detail::wire(window);
    play::wire(window);
    downloads::wire(window);
    library::wire(window);
    settings::wire(window);
}

/// Apply the schema and clean up anything a previous session left mid-flight.
/// Called once, from the app's startup path.
pub async fn init() {
    let Ok(pool) = pool_for("videos").await else {
        tracing::warn!("splus: videos DB unavailable — the tab will be empty");
        return;
    };
    if let Err(e) = splus::schema::apply_schema(&pool).await {
        tracing::warn!(error = %e, "splus: schema could not be applied");
        return;
    }
    splus::downloads::reconcile(&pool).await;
    splus::downloads::pump(&pool).await;
}
