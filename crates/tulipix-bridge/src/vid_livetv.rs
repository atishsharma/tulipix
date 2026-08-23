//! Live TV: the iptv-org playlist picker, the channel grid, and handing a
//! channel to mpv.
//!
//! A channel is a live HLS URL and nothing else — no detail page, no resume
//! point, no episode. That is why this file is a fraction of the size of the
//! Stream tab's: load a list, filter it, play one.
//!
//! The grid is paged here rather than in Dart. A country list is thousands of
//! channels; the filter, the sort, the page slice and the logo fetch all have
//! to agree on which forty-two rows are on screen, and one owner is the only
//! way they can.
//!
//! Ported from `tulipix_sec_videos::livetv`, which cannot be linked here — it
//! depends on slint. The one real shape change is the logo cache: Slint kept
//! decoded `slint::Image`s on the UI thread, and this keeps file paths, because
//! Flutter decodes from a path itself.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tulipix_videos::livetv::{self, Channel, Kind};

use crate::api::videos::{
    self as v, LiveChannel, LiveGroup, LivePlaylist, LiveView, Session, VideosCmd, VideosEvent,
    with,
};
use crate::vid_stream::{cache_covers, path_str};

/// Channels per page — a 7×6 grid. Logos are fetched a page at a time, so this
/// is also the size of one network burst.
const PAGE_SIZE: usize = 42;

/// How much of a channel name the header pill shows before it trails off.
const PILL_CHARS: usize = 20;

/// How many logo paths to remember. An iptv-org playlist carries thousands of
/// channels and every one scrolled past used to stay resident for the session.
/// Evicting costs a map miss and a re-read from the on-disk cover cache (see
/// `cache_cover`, which checks the disk before the network), never a download.
const LOGO_CACHE_MAX: usize = 128;

// ---------------------------------------------------------------- state ----

/// Everything currently loaded, plus the picker's working selection.
///
/// The selection lives here rather than being read back from settings on every
/// click, so ticking a box does not write a file per keystroke.
#[derive(Default)]
pub(crate) struct LiveSession {
    pub channels: Vec<Channel>,
    /// URLs of the playlists ticked in the picker, saved on Save.
    pub picked: Vec<String>,
    /// The channel handed to mpv, and its index into `channels`.
    pub now: Option<(usize, Channel)>,
    /// Remote logo URL → the file it was cached to. `None` means the download
    /// was tried and failed, so a dead logo is asked for once per session
    /// rather than once per page turn.
    pub logos: HashMap<String, Option<PathBuf>>,
    /// Insertion order for `logos`, oldest first — the eviction queue.
    pub logo_order: VecDeque<String>,
    /// URLs with a fetch in flight, so paging back and forth cannot ask twice.
    pub pending: HashSet<String>,
}

/// Bumped by every load. A slow playlist that lands after a newer load started
/// is dropped rather than painting over it.
static GEN: AtomicU64 = AtomicU64::new(0);

/// Bumped by every play. Switching channels kills the running mpv, so the old
/// process's exit hook fires *after* the new one started — without this it
/// would clear a pill that belongs to the channel now on air.
static PLAY_GEN: AtomicU64 = AtomicU64::new(0);

/// What the tab opens with, before anything has been loaded.
pub(crate) fn init_view(view: &mut LiveView) {
    view.sort = "default".into();
    view.asc = true;
    view.config_tab = "country".into();
    view.pages = 1;
    view.now_index = -1;
}

// -------------------------------------------------------------- dispatch ----

/// Handle one command, or report that it belongs to another page.
pub(crate) async fn dispatch(cmd: &VideosCmd) -> Result<bool> {
    match cmd {
        VideosCmd::LiveRefresh => {
            // Refresh means "go and look again", so the day-old cache is
            // dropped first — otherwise the button does nothing for 24 hours.
            if let Err(e) = livetv::clear_cache() {
                tracing::warn!(error = %e, "livetv: could not clear the playlist cache");
            }
            load(true);
        }
        VideosCmd::LiveSearch { query } => {
            with(|s| {
                s.ui.live.query = query.clone();
                // A narrower list has fewer pages; staying on page 9 would show
                // an empty grid.
                s.ui.live.page = 0;
            });
            repaint();
        }
        VideosCmd::LiveSetGroup { group } => {
            with(|s| {
                s.ui.live.group = group.clone();
                s.ui.live.page = 0;
            });
            repaint();
        }
        VideosCmd::LiveSetPage { page } => {
            with(|s| s.ui.live.page = (*page).max(0));
            repaint();
        }
        VideosCmd::LiveSetSort { key, asc } => {
            with(|s| {
                s.ui.live.sort = key.clone();
                s.ui.live.asc = *asc;
                s.ui.live.page = 0;
            });
            repaint();
        }
        VideosCmd::LivePlay { index } => play(*index),
        VideosCmd::LiveInfoOpened => {
            with(|s| {
                let line = s
                    .live
                    .now
                    .as_ref()
                    .map(|(_, ch)| cache_line(&s.live, &ch.logo))
                    .unwrap_or_default();
                s.ui.live.now_cache = line;
            });
        }
        VideosCmd::LiveConfigLoad { tab, query } => {
            with(|s| {
                s.ui.live.config_tab = tab.clone();
                s.ui.live.config_query = query.clone();
                push_playlists(s);
            });
        }
        VideosCmd::LiveTogglePlaylist { url } => {
            with(|s| {
                match s.live.picked.iter().position(|p| p == url) {
                    Some(at) => {
                        s.live.picked.remove(at);
                    }
                    None => s.live.picked.push(url.clone()),
                }
                s.ui.live.picked_count = s.live.picked.len() as i64;
                push_playlists(s);
            });
        }
        VideosCmd::LiveConfigClear => {
            with(|s| {
                s.live.picked.clear();
                s.ui.live.picked_count = 0;
                push_playlists(s);
            });
        }
        VideosCmd::LiveConfigSave => {
            let picked = with(|s| s.live.picked.clone());
            match livetv::set_selection(&picked) {
                Ok(saved) => {
                    with(|s| {
                        s.live.picked = saved;
                        s.ui.live.picked_count = s.live.picked.len() as i64;
                    });
                    load(true);
                }
                Err(msg) => with(|s| s.ui.live.status = msg),
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Opening the tab. Picked playlists come from settings on the first entry
/// only; after that the in-memory list is the truth.
pub(crate) async fn enter() {
    let (picked, loaded) = with(|s| {
        if s.live.picked.is_empty() {
            s.live.picked = livetv::selection();
        }
        s.ui.live.picked_count = s.live.picked.len() as i64;
        (s.live.picked.len(), !s.live.channels.is_empty())
    });
    if picked > 0 && !loaded {
        load(false);
    }
}

// --------------------------------------------------------------- loading ----

/// Fetch every selected playlist and paint the grid.
///
/// Spawned rather than awaited: the caller's snapshot goes back to Dart with
/// `busy` already true, and the finished load announces itself with `Changed`.
/// Awaiting here would hold the whole section still for the length of a
/// multi-megabyte M3U download.
fn load(announce: bool) {
    let urls = with(|s| s.live.picked.clone());
    if urls.is_empty() {
        with(|s| {
            s.live.channels.clear();
            let live = &mut s.ui.live;
            live.channels.clear();
            live.groups.clear();
            live.total = 0;
            live.matched = 0;
            live.pages = 1;
            live.page = 0;
            live.busy = false;
        });
        return;
    }
    let epoch = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let n = urls.len();
    with(|s| {
        s.ui.live.busy = true;
        if announce {
            s.ui.live.status = format!("Fetching {n} playlist{}…", if n == 1 { "" } else { "s" });
        }
    });

    tokio::spawn(async move {
        let channels = livetv::load(tulipix_core::net::http(), &urls).await;
        if GEN.load(Ordering::SeqCst) != epoch {
            return; // a newer load started while these were downloading
        }
        let count = channels.len();
        with(|s| {
            s.live.channels = channels;
            {
                let live = &mut s.ui.live;
                live.total = count as i64;
                live.page = 0;
                live.busy = false;
                live.status = if count == 0 {
                    "Those playlists came back empty.".to_string()
                } else {
                    format!("{count} channels")
                };
            }
            push_groups(s);
            push_channels(s);
            resync_now(s);
        });
        // Which also fetches the logos for the first page.
        let wanted = with(claim_logos);
        fetch_logos(wanted).await;
        v::emit(VideosEvent::Changed);
    });
}

/// Repaint the grid and start the logo fetch for whatever landed on screen.
fn repaint() {
    let wanted = with(|s| {
        push_channels(s);
        claim_logos(s)
    });
    if wanted.is_empty() {
        return;
    }
    tokio::spawn(async move {
        fetch_logos(wanted).await;
        v::emit(VideosEvent::Changed);
    });
}

/// The logo URLs on the current page that are neither cached nor already being
/// fetched, marked as in flight so a second page turn cannot ask twice.
fn claim_logos(s: &mut Session) -> Vec<String> {
    let urls: Vec<String> = s.ui.live.channels.iter().map(|c| c.logo.clone()).collect();
    urls.into_iter()
        .filter(|u| !u.is_empty())
        .filter(|u| !s.live.logos.contains_key(u))
        .filter(|u| s.live.pending.insert(u.clone()))
        .collect()
}

/// Download the logos for the channels now on screen, then repaint their rows.
async fn fetch_logos(want: Vec<String>) {
    if want.is_empty() {
        return;
    }
    let paths = cache_covers(want.clone()).await;
    with(|s| {
        for (url, path) in want.iter().zip(paths) {
            logo_store(s, url.clone(), path);
            s.live.pending.remove(url);
        }
        push_channels(s);
        // The pill's own logo may have been in this batch.
        if let Some((idx, ch)) = s.live.now.clone() {
            show_now(s, idx as i64, &ch);
        }
    });
}

/// Record a fetched logo, evicting the oldest once over the cap.
fn logo_store(s: &mut Session, url: String, path: Option<PathBuf>) {
    if s.live.logos.insert(url.clone(), path).is_none() {
        s.live.logo_order.push_back(url);
    }
    while s.live.logo_order.len() > LOGO_CACHE_MAX {
        if let Some(old) = s.live.logo_order.pop_front() {
            s.live.logos.remove(&old);
        }
    }
}

/// The cached file for a logo URL, blank until it has been fetched.
fn logo_path(s: &LiveSession, url: &str) -> String {
    s.logos.get(url).cloned().flatten().map(|p| path_str(Some(p))).unwrap_or_default()
}

/// "Logo cached · 4.2 KB · /…" for the now-playing sheet.
fn cache_line(s: &LiveSession, url: &str) -> String {
    if url.is_empty() {
        return "This channel publishes no logo.".into();
    }
    match s.logos.get(url) {
        Some(Some(path)) => {
            let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            format!("Logo cached · {} · {}", human_bytes(size), path.display())
        }
        Some(None) => "The logo could not be downloaded.".into(),
        None => "Logo not fetched yet.".into(),
    }
}

fn human_bytes(n: u64) -> String {
    match n {
        0 => "0 B".into(),
        n if n < 1024 => format!("{n} B"),
        n if n < 1024 * 1024 => format!("{:.1} KB", n as f64 / 1024.0),
        n => format!("{:.1} MB", n as f64 / (1024.0 * 1024.0)),
    }
}

// ----------------------------------------------------------- now playing ----

fn play(index: i64) {
    let index = index.max(0);
    let Some(ch) = with(|s| s.live.channels.get(index as usize).cloned()) else { return };
    with(|s| {
        s.ui.live.status = format!("Opening {}…", ch.name);
        s.live.now = Some((index as usize, ch.clone()));
        show_now(s, index, &ch);
    });

    let epoch = PLAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let on_end: crate::vmpv::PlaybackEnd = std::sync::Arc::new(move |_pos: f64, _dur: f64| {
        // Only the *current* channel's exit clears the pill; a switch retires
        // the previous epoch first.
        if PLAY_GEN.load(Ordering::SeqCst) != epoch {
            return;
        }
        with(|s| {
            s.live.now = None;
            clear_now(s);
        });
        v::emit(VideosEvent::Changed);
    });
    // A live stream has no resume point and no library row to key one off.
    crate::vmpv::play(ch.url.clone(), None, None, Vec::new(), Some(on_end));

    // The status line says "Opening …" from the moment the button is pressed;
    // a live stream can take seconds to negotiate, so it is only promoted once
    // mpv reports a running clock.
    let name = ch.name.clone();
    crate::vmpv::watch_live(move || {
        if PLAY_GEN.load(Ordering::SeqCst) != epoch {
            return;
        }
        with(|s| s.ui.live.status = format!("Playing {name}"));
        v::emit(VideosEvent::Changed);
    });
}

/// `name`, cut to [`PILL_CHARS`] with an ellipsis. Counted in characters, not
/// bytes — channel names are full of accents and non-Latin scripts.
fn pill_label(name: &str) -> String {
    if name.chars().count() <= PILL_CHARS {
        return name.to_string();
    }
    let head: String = name.chars().take(PILL_CHARS).collect();
    format!("{}…", head.trim_end())
}

fn show_now(s: &mut Session, index: i64, ch: &Channel) {
    let logo = logo_path(&s.live, &ch.logo);
    let live = &mut s.ui.live;
    live.now_index = index;
    live.now_label = pill_label(&ch.name);
    live.now_res = if ch.res == 0 { String::new() } else { format!("{}p", ch.res) };
    live.now_name = ch.name.clone();
    live.now_group = ch.group.clone();
    live.now_id = ch.id.clone();
    live.now_url = ch.url.clone();
    live.now_logo = logo;
}

/// A reload replaces the whole channel list, so the playing channel's row may
/// have moved or gone. mpv is still playing either way — only the card
/// highlight is re-pointed, by stream URL rather than by the stale index.
fn resync_now(s: &mut Session) {
    let found = {
        let Some((_, now)) = s.live.now.as_ref() else { return };
        s.live.channels.iter().position(|c| c.url == now.url).map(|i| (i, now.clone()))
    };
    match found {
        Some((i, ch)) => {
            s.live.now = Some((i, ch.clone()));
            show_now(s, i as i64, &ch);
        }
        // Dropped from the selection mid-play. The pill stays — the stream is
        // still on — but no card claims it.
        None => s.ui.live.now_index = -1,
    }
}

fn clear_now(s: &mut Session) {
    let live = &mut s.ui.live;
    live.now_index = -1;
    live.now_name.clear();
    live.now_label.clear();
    live.now_res.clear();
    live.now_group.clear();
    live.now_id.clear();
    live.now_url.clear();
    live.now_logo.clear();
    live.now_cache.clear();
}

// ---------------------------------------------------------------- models ----

/// The sidebar: every distinct `group-title`, alphabetical, with how many
/// channels carry it — the count the row prints on its pill.
fn push_groups(s: &mut Session) {
    let mut groups: Vec<(String, i64)> = Vec::new();
    for c in s.live.channels.iter() {
        if c.group.is_empty() {
            continue;
        }
        match groups.iter_mut().find(|(g, _)| *g == c.group) {
            Some((_, n)) => *n += 1,
            None => groups.push((c.group.clone(), 1)),
        }
    }
    groups.sort_by_key(|(g, _)| g.to_lowercase());
    s.ui.live.groups =
        groups.into_iter().map(|(name, count)| LiveGroup { name, count }).collect();
}

/// One page of the grid: narrowed by the search box and the sidebar, ordered by
/// the sort chips, sliced to [`PAGE_SIZE`].
///
/// `index` on each row is the position in the *unfiltered* list, so a click
/// still finds the right channel after a filter, a sort and a page turn.
fn push_channels(s: &mut Session) {
    let q = s.ui.live.query.trim().to_lowercase();
    let group = s.ui.live.group.clone();
    let sort = s.ui.live.sort.clone();
    let asc = s.ui.live.asc;

    let mut ord: Vec<(usize, &Channel)> = s
        .live
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| group.is_empty() || c.group == group)
        .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q))
        .collect();
    match sort.as_str() {
        "name" => ord.sort_by_key(|(_, c)| c.name.to_lowercase()),
        "group" => ord.sort_by_key(|(_, c)| (c.group.to_lowercase(), c.name.to_lowercase())),
        // Resolution comes off the display name, so plenty of channels do not
        // publish one; they sort as zero, which puts them together at one end
        // rather than scattered through the grid.
        "res" => ord.sort_by_key(|(_, c)| (c.res, c.name.to_lowercase())),
        // "default" is the order the playlists were merged in, which is the
        // mirror's own — sorting it would throw that away.
        _ => {}
    }
    if !asc {
        ord.reverse();
    }

    let matched = ord.len();
    let pages = matched.div_ceil(PAGE_SIZE).max(1);
    let page = (s.ui.live.page.max(0) as usize).min(pages - 1);
    let start = page * PAGE_SIZE;
    let slice = &ord[start.min(matched)..(start + PAGE_SIZE).min(matched)];

    let rows: Vec<LiveChannel> = slice
        .iter()
        .map(|(i, c)| LiveChannel {
            index: *i as i64,
            id: c.id.clone(),
            name: c.name.clone(),
            group: c.group.clone(),
            logo: logo_path(&s.live, &c.logo),
        })
        .collect();

    let live = &mut s.ui.live;
    live.matched = matched as i64;
    live.pages = pages as i64;
    live.page = page as i64;
    live.channels = rows;
}

/// The picker list for the open tab, narrowed by its own filter box.
fn push_playlists(s: &mut Session) {
    let want = match s.ui.live.config_tab.as_str() {
        "category" => Kind::Category,
        "language" => Kind::Language,
        _ => Kind::Country,
    };
    let q = s.ui.live.config_query.trim().to_lowercase();
    let picked = s.live.picked.clone();

    s.ui.live.playlists = livetv::catalogue()
        .into_iter()
        .filter(|(kind, _, _)| *kind == want)
        .filter(|(_, name, _)| q.is_empty() || name.to_lowercase().contains(&q))
        .map(|(kind, name, url)| LivePlaylist {
            picked: picked.iter().any(|p| *p == url),
            name,
            kind: kind.label().to_lowercase(),
            url,
        })
        .collect();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_list_still_has_one_page() {
        // The page counter reads "1 / N"; N must never be zero, or an empty
        // search would leave the header claiming page 1 of 0.
        assert_eq!(0usize.div_ceil(PAGE_SIZE).max(1), 1);
        assert_eq!(1usize.div_ceil(PAGE_SIZE).max(1), 1);
        assert_eq!(PAGE_SIZE.div_ceil(PAGE_SIZE).max(1), 1);
        assert_eq!((PAGE_SIZE + 1).div_ceil(PAGE_SIZE).max(1), 2);
    }

    #[test]
    fn the_pill_label_is_cut_by_characters_not_bytes() {
        assert_eq!(pill_label("BBC News"), "BBC News");
        assert_eq!(pill_label(&"a".repeat(PILL_CHARS)), "a".repeat(PILL_CHARS));
        assert_eq!(
            pill_label(&"a".repeat(PILL_CHARS + 1)),
            format!("{}…", "a".repeat(PILL_CHARS))
        );
        // Multi-byte names must be cut on a character boundary — slicing by
        // byte would panic on the first accent.
        let cut = pill_label("Первый канал Россия Москва");
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= PILL_CHARS + 1, "{cut}");
    }

    #[test]
    fn a_channel_without_a_logo_is_not_reported_as_a_failed_download() {
        let s = LiveSession::default();
        assert!(cache_line(&s, "").contains("no logo"));
        // Never fetched is a different sentence from fetched-and-failed; the
        // sheet said "could not be downloaded" for both before this split.
        assert!(cache_line(&s, "http://x/logo.png").contains("not fetched"));
    }

    #[test]
    fn the_logo_cache_evicts_the_oldest_first() {
        let mut s = Session::probe();
        for i in 0..(LOGO_CACHE_MAX + 5) {
            logo_store(&mut s, format!("u{i}"), None);
        }
        assert_eq!(s.live.logos.len(), LOGO_CACHE_MAX);
        assert!(!s.live.logos.contains_key("u0"));
        assert!(s.live.logos.contains_key(&format!("u{}", LOGO_CACHE_MAX + 4)));
    }
}
