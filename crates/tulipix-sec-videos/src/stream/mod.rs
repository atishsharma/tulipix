//! Stream tab wiring — remote catalogue search, detail view, and playback.
//!
//! The typed backend lives in `tulipix_videos::stream`; everything here is the
//! UI half: one shared client, the poster cache, and the async handlers that
//! `main.rs` hangs off the `window.on_video_stream_*` callbacks.
//!
//! This module holds what every handler needs — the shared state, the client,
//! the action epoch, and the small formatters. Each surface of the tab gets its
//! own file, and their public handlers are re-exported here so `main.rs` sees
//! one flat `tulipix_sec_videos::stream_*` namespace.
//!
//! Playback hands the resolved URL to the same windowed mpv the local library
//! uses (`spawn_mpv_windowed_tracked`), with progress written back to
//! `stream_progress` — a remote title has no row in the videos DB to hang the
//! local `watch_progress` off.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use tulipix_common::*;
use tulipix_ui::*;
use tulipix_videos::stream::{self, Caption, Details, StreamClient, StreamError, StreamFile};

mod detail;
mod download;
mod hosts;
mod play;
mod saved;
mod search;

pub use detail::*;
pub use download::*;
pub use hosts::*;
pub use play::*;
pub use saved::*;
pub use search::*;

// ---- shared state ----

/// Everything the Stream tab remembers between callbacks, behind one lock.
///
/// This used to be eleven separate statics that had to agree with each other;
/// they did not, and the mismatches were real bugs (a subject id from one field
/// used with a season from another). One struct makes the invalid combinations
/// unrepresentable and the update sites obvious.
#[derive(Debug)]
pub(crate) struct StreamState {
    /// Subject id the open title was fetched with.
    ///
    /// Resource lookups must use *this*, not `Details::id`. The detail payload's
    /// `id` field is frequently absent, and an empty `subjectId=` in the resource
    /// query is a 400 — which is why switching episodes once failed while the
    /// first load (which still had the id in hand) worked.
    pub open_id: String,
    /// Details of the open title, so a season/episode/dub switch can re-query
    /// without another detail fetch.
    pub details: Option<Details>,
    /// Selected `(season, episode)`; `(0, 0)` means "a movie, no episode axis".
    pub selection: (i64, i64),
    /// `(season number, subject id)` for a show the catalogue splits across one
    /// subject per season. Empty when the subject carries its own seasons.
    pub season_subjects: Vec<(i64, String)>,
    /// Every subject id belonging to a split show maps to that show's full
    /// season list, so opening any season still knows about its siblings.
    pub season_map: std::collections::HashMap<String, Vec<(i64, String)>>,
    /// Files backing the current episode, parallel to the rows in the UI.
    pub files: Vec<StreamFile>,
    /// Subtitle tracks for the current episode, one per language.
    pub subs: Vec<Caption>,
    /// Index into `subs`, or `None` for no subtitles.
    pub sub_choice: Option<usize>,
    /// Language *name* of the chosen subtitle. Kept separately from the index
    /// so a pick of "English" survives an episode whose tracks are ordered
    /// differently.
    pub sub_lang: Option<String>,
    /// Resolution rung the user picked; `""` means every rung (one request each).
    pub resolution: String,
    /// In-memory detail cache for the hover preview, so skimming the grid does
    /// not hammer the servers.
    pub preview_cache: std::collections::HashMap<String, Details>,
    /// Index into `files` treated as the "current" stream — the one the info-box
    /// Play/Copy/Download act on. Re-armed by [`stream::quality::pick`] each time
    /// an episode's streams load, so changing episode never needs a row click.
    pub current_stream: usize,
    /// Whether the open title is saved. Drives the bookmark button's state.
    pub bookmarked: bool,
    /// Saved-page sort: key ("date"|"name"|"type") and ascending flag.
    pub bm_sort: String,
    pub bm_asc: bool,
}

impl Default for StreamState {
    fn default() -> Self {
        Self {
            open_id: String::new(),
            details: None,
            selection: (0, 0),
            season_subjects: Vec::new(),
            season_map: std::collections::HashMap::new(),
            files: Vec::new(),
            subs: Vec::new(),
            sub_choice: None,
            sub_lang: Some("English".to_string()),
            // The quality filter is remembered across restarts, so the tab opens
            // on the rung the user last chose rather than back at "all".
            resolution: stream::quality::filter(),
            preview_cache: std::collections::HashMap::new(),
            current_stream: 0,
            bookmarked: false,
            bm_sort: "date".to_string(),
            bm_asc: false,
        }
    }
}

static STATE: OnceLock<Mutex<StreamState>> = OnceLock::new();
fn state() -> &'static Mutex<StreamState> {
    STATE.get_or_init(|| Mutex::new(StreamState::default()))
}

/// Read or mutate the shared state. Never hold the guard across an `.await` —
/// every caller here takes what it needs and gets out.
pub(crate) fn with_state<R>(f: impl FnOnce(&mut StreamState) -> R) -> R
where
    R: Default,
{
    match state().lock() {
        Ok(mut g) => f(&mut g),
        Err(_) => R::default(),
    }
}

/// The subject id resource lookups must use. `None` means nothing is open.
pub(crate) fn opened_subject() -> Option<String> {
    with_state(|s| (!s.open_id.is_empty()).then(|| s.open_id.clone()))
}

pub(crate) fn current_resolution() -> String {
    with_state(|s| s.resolution.clone())
}

/// Clear everything tied to the open title, keeping the search-level state.
pub(crate) fn clear_open_title() {
    with_state(|s| {
        s.open_id.clear();
        s.details = None;
        s.season_subjects.clear();
        s.files.clear();
        s.subs.clear();
        s.sub_choice = None;
    });
}

// ---- split-season shows ----

/// Record the season lists that came back with a search, keyed by every subject
/// in each list. Replaces the previous search's entries.
pub(crate) fn remember_season_maps(hits: &[stream::SearchHit]) {
    with_state(|st| {
        st.season_map.clear();
        for hit in hits.iter().filter(|h| h.season_subjects.len() > 1) {
            for (_, id) in &hit.season_subjects {
                st.season_map.insert(id.clone(), hit.season_subjects.clone());
            }
        }
    })
}

/// Season siblings of `subject_id`, or empty when it is not a split show.
pub(crate) fn siblings_of(subject_id: &str) -> Vec<(i64, String)> {
    with_state(|st| st.season_map.get(subject_id).cloned().unwrap_or_default())
}

// ---- shared client ----

/// Built once and reused; dropped whenever the host list changes.
static CLIENT: OnceLock<Mutex<Option<StreamClient>>> = OnceLock::new();
fn client_cell() -> &'static Mutex<Option<StreamClient>> {
    CLIENT.get_or_init(|| Mutex::new(None))
}

/// Monotonic counter bumped by every user action in the tab.
///
/// Detail and stream fetches are async and overlap: click Episode 5 then
/// Episode 7 and both requests are in flight, so whichever *finishes* last
/// used to win and paint its streams under the other episode's highlight.
/// Every fetch carries the epoch it started in and drops its result if a newer
/// action has happened since.
static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn next_epoch() -> u64 {
    EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1
}

pub(crate) fn is_current(epoch: u64) -> bool {
    EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch
}

/// Reuse the live client, or build one over the configured hosts and warm it up
/// so later requests carry a session token.
///
/// `tokio::sync::OnceCell` would be tidier, but the cell has to be resettable
/// when the host list changes — so this keeps the plain lock and accepts that
/// two simultaneous first-calls may both build one. The loser is dropped.
pub(crate) async fn client() -> Result<StreamClient, StreamError> {
    if let Some(c) = client_cell().lock().ok().and_then(|g| g.clone()) {
        return Ok(c);
    }
    // Install any persisted signing key before the first request signs anything.
    stream::sign_key::apply();
    let c = StreamClient::new(stream::hosts::load())?;
    c.init().await?;
    if let Ok(mut g) = client_cell().lock() {
        // Another task may have finished first; keep whichever is already in.
        if g.is_none() {
            *g = Some(c.clone());
        }
        return Ok(g.clone().unwrap_or(c));
    }
    Ok(c)
}

/// Force the next call to rebuild the client — used after a host edit so the new
/// list takes effect without a restart.
pub fn stream_invalidate_client() {
    if let Ok(mut g) = client_cell().lock() {
        *g = None;
    }
}

// ---- small UI helpers ----

pub(crate) fn set_status(weak: &slint::Weak<MainWindow>, msg: impl Into<String>, busy: bool) {
    let msg = msg.into();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_status(msg.into());
        w.set_video_stream_busy(busy);
    });
}

/// Plain-language failure text — users see these, not the Debug form.
pub(crate) fn explain(e: &StreamError) -> String {
    match e {
        StreamError::NoHosts => "No servers configured — open Hosts and add one.".into(),
        StreamError::HostsExhausted => "No server answered. Check your connection or edit Hosts.".into(),
        StreamError::MissingToken => "Servers answered but refused a session. Try different Hosts.".into(),
        StreamError::ApiStatus(404) => "Nothing found for that.".into(),
        StreamError::ApiStatus(s) => format!("Servers returned {s}. Try again or edit Hosts."),
        StreamError::Reqwest(_) => "Network error.".into(),
        StreamError::Json(_) => "Unreadable response from the server.".into(),
    }
}

fn cover_dir() -> Option<PathBuf> {
    tulipix_core::paths::cache_dir().map(|d| d.join("videos").join("stream"))
}

/// Download a remote poster into the cache, keyed by a hash of its URL, and
/// return the local path. Already-cached files are reused untouched.
pub(crate) async fn cache_cover(url: &str) -> Option<PathBuf> {
    if url.is_empty() {
        return None;
    }
    let dir = cover_dir()?;
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(url.as_bytes());
    let path = dir.join(format!("{:x}.jpg", h.finalize()));
    if path.exists() {
        return Some(path);
    }
    tokio::fs::create_dir_all(&dir).await.ok()?;
    let bytes = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// Cache a batch of poster URLs concurrently, preserving input order.
pub(crate) async fn cache_covers(urls: Vec<String>) -> Vec<Option<PathBuf>> {
    let tasks: Vec<_> = urls
        .into_iter()
        .map(|u| tokio::spawn(async move { cache_cover(&u).await }))
        .collect();
    let mut out = Vec::with_capacity(tasks.len());
    for t in tasks {
        out.push(t.await.ok().flatten());
    }
    out
}

pub(crate) fn load_image(path: Option<PathBuf>) -> slint::Image {
    path.as_deref()
        .and_then(|p| slint::Image::load_from_path(p).ok())
        .unwrap_or_default()
}

/// "2024 · Drama · USA · TV-MA · 50 min · ★ 8.7" — blank fields drop out.
pub(crate) fn meta_line(d: &Details) -> String {
    let mut parts: Vec<String> = Vec::new();
    for f in [&d.year, &d.genre, &d.country, &d.content_rating, &d.duration] {
        if !f.is_empty() {
            parts.push(f.clone());
        }
    }
    if d.rating > 0.0 {
        parts.push(format!("★ {:.1}", d.rating));
    }
    parts.join("  ·  ")
}

/// "2.1 GB · h265" for the row subtitle; either half may be missing.
pub(crate) fn file_sub(f: &StreamFile) -> String {
    [f.size.as_str(), f.codec.as_str()]
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join("  ·  ")
}

/// First `max` characters of `s`, adding an ellipsis when it was cut. Counts by
/// `char`, not byte, so it never splits a multibyte glyph.
pub(crate) fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max).collect();
    out.push('…');
    out
}

/// One subtitle entry per language across every file of an episode, keeping the
/// first URL seen for each. Languages are compared case-insensitively.
pub(crate) fn caption_union(files: &[StreamFile]) -> Vec<Caption> {
    let mut out: Vec<Caption> = Vec::new();
    for f in files {
        for c in &f.captions {
            if c.url.is_empty() {
                continue;
            }
            let dup = out.iter().any(|o| {
                (!c.lang.is_empty() && o.lang.eq_ignore_ascii_case(&c.lang)) || o.url == c.url
            });
            if !dup {
                out.push(c.clone());
            }
        }
    }
    out
}

// ---- preferences ----

/// Push the persisted quality filter into the UI. Called when the tab opens, so
/// the pill shows the rung the state was already initialised with.
pub fn stream_prefs_load(weak: slint::Weak<MainWindow>) {
    let res = current_resolution();
    let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_resolution(res.into()));
}

// ---- cache housekeeping ----

/// Trim the on-disk caches. Called when the tab opens: the stream table ages out
/// past a week, and the poster/subtitle dirs are capped by file count so they
/// cannot grow without bound.
pub fn stream_prune_caches() {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::cache::prune(&pool, 7 * 24 * 60 * 60).await;
        }
        if let Some(base) = tulipix_core::paths::cache_dir().map(|d| d.join("videos")) {
            cap_dir(&base.join("stream"), 400).await;
            cap_dir(&base.join("stream-subs"), 60).await;
        }
    });
}

/// Keep at most `max` files in `dir`, dropping the oldest first.
async fn cap_dir(dir: &std::path::Path, max: usize) {
    let Ok(mut rd) = tokio::fs::read_dir(dir).await else { return };
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let modified = e
            .metadata()
            .await
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        entries.push((modified, e.path()));
    }
    if entries.len() <= max {
        return;
    }
    entries.sort_by_key(|(t, _)| *t);
    for (_, path) in entries.iter().take(entries.len() - max) {
        let _ = tokio::fs::remove_file(path).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_line_skips_blank_fields() {
        let d = Details {
            year: "2024".into(),
            genre: "Drama".into(),
            rating: 8.74,
            ..Default::default()
        };
        assert_eq!(meta_line(&d), "2024  ·  Drama  ·  ★ 8.7");
        assert_eq!(meta_line(&Details::default()), "");
    }

    #[test]
    fn file_sub_handles_missing_halves() {
        let f = StreamFile { size: "2.1 GB".into(), codec: "h265".into(), ..Default::default() };
        assert_eq!(file_sub(&f), "2.1 GB  ·  h265");
        let f = StreamFile { codec: "h264".into(), ..Default::default() };
        assert_eq!(file_sub(&f), "h264");
        assert_eq!(file_sub(&StreamFile::default()), "");
    }

    #[test]
    fn errors_are_plain_language_not_debug() {
        for e in [
            StreamError::NoHosts,
            StreamError::HostsExhausted,
            StreamError::MissingToken,
            StreamError::ApiStatus(403),
        ] {
            let msg = explain(&e);
            assert!(!msg.is_empty());
            assert!(!msg.contains("StreamError"), "leaked Debug form: {msg}");
        }
        assert!(explain(&StreamError::ApiStatus(403)).contains("403"));
    }

    /// The fix for streams landing under the wrong episode: only the newest
    /// action may paint.
    #[test]
    fn generation_guard_invalidates_older_actions() {
        let first = next_epoch();
        assert!(is_current(first));
        let second = next_epoch();
        assert!(!is_current(first), "a superseded action must not paint");
        assert!(is_current(second));
    }

    #[test]
    fn truncate_chars_caps_and_marks_cut_by_char_not_byte() {
        assert_eq!(truncate_chars("short", 100), "short"); // untouched
        assert_eq!(truncate_chars("abcdef", 3), "abc…"); // cut + ellipsis
        assert_eq!(truncate_chars("abc", 3), "abc"); // exact length, no ellipsis
        // Multibyte: 4 codepoints capped to 2 must not split a glyph.
        assert_eq!(truncate_chars("héllo", 2), "hé…");
    }

    #[test]
    fn caption_union_keeps_one_entry_per_language() {
        let cap = |lang: &str, url: &str| Caption {
            url: url.into(), lang: lang.into(), ext: "srt".into(),
        };
        let files = vec![
            StreamFile {
                captions: vec![cap("English", "https://s/en1"), cap("Hindi", "https://s/hi")],
                ..Default::default()
            },
            StreamFile {
                // same languages on the 480p cut — must not double up the picker
                captions: vec![cap("english", "https://s/en2"), cap("", "")],
                ..Default::default()
            },
        ];
        let u = caption_union(&files);
        assert_eq!(u.len(), 2);
        assert_eq!(u[0].lang, "English");
        assert_eq!(u[0].url, "https://s/en1"); // first URL seen wins
        assert_eq!(u[1].lang, "Hindi");
        assert!(caption_union(&[]).is_empty());
    }
}
