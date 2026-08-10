//! Live TV wiring: the iptv-org playlist picker, the channel grid, and handing
//! a channel to mpv.
//!
//! A channel is a live HLS URL and nothing else — there is no detail page, no
//! resume point and no episode. That is why this file is a fraction of the size
//! of the Stream tab's: load a list, filter it, play one.
//!
//! The grid is paged here rather than in Slint. A country list is thousands of
//! channels; the filter, the sort, the page slice and the logo fetch all have to
//! agree on which thirty rows are on screen, and one owner is the only way they
//! can.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use tulipix_common::{PlaybackEnd, spawn_mpv_windowed_tracked};
use tulipix_ui::*;
use tulipix_videos::livetv::{self, Channel, Kind};

use crate::stream::{cache_covers, load_image};

/// Channels per page — a 7×6 grid. Logos are fetched a page at a time, so this
/// is also the size of one network burst.
const PAGE_SIZE: usize = 42;

/// How much of a channel name the header pill shows before it trails off.
const PILL_CHARS: usize = 20;

/// Everything currently loaded, plus the picker's working selection.
///
/// The selection is kept here rather than read back from settings on every
/// click, so ticking a box does not write a file per keystroke.
#[derive(Default)]
struct State {
    channels: Vec<Channel>,
    /// URLs of the playlists ticked in the picker, saved on Load.
    picked: Vec<String>,
    /// The channel handed to mpv, and its index into `channels`.
    now: Option<(usize, Channel)>,
}

fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Bumped by every load. A slow playlist that lands after a newer load started
/// is dropped rather than painting over it.
static GEN: AtomicU64 = AtomicU64::new(0);

/// Bumped by every play. Switching channels kills the running mpv, so the old
/// process's exit hook fires *after* the new one started — without this it
/// would clear a pill that belongs to the channel now on air.
static PLAY_GEN: AtomicU64 = AtomicU64::new(0);

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_livetv_enter(move || {
        let Some(w) = w.upgrade() else { return };
        // Picked playlists come from settings on the first entry only; after
        // that the in-memory list is the truth.
        let (picked, loaded) = {
            let mut st = state();
            if st.picked.is_empty() {
                st.picked = livetv::selection();
            }
            (st.picked.len(), !st.channels.is_empty())
        };
        w.set_livetv_picked_count(picked as i32);
        if picked > 0 && !loaded {
            load(w.as_weak(), false);
        }
    });

    let w = window.as_weak();
    window.on_livetv_refresh(move || {
        let Some(w) = w.upgrade() else { return };
        // Refresh means "go and look again", so the day-old cache is dropped
        // first — otherwise the button does nothing for 24 hours.
        if let Err(e) = livetv::clear_cache() {
            tracing::warn!(error = %e, "livetv: could not clear the playlist cache");
        }
        load(w.as_weak(), true);
    });

    let w = window.as_weak();
    window.on_livetv_search(move |_q| {
        if let Some(w) = w.upgrade() {
            // A narrower list has fewer pages; staying on page 9 would show an
            // empty grid.
            w.set_livetv_page(0);
            push_channels(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_set_group(move |g| {
        if let Some(w) = w.upgrade() {
            w.set_livetv_group(g);
            w.set_livetv_page(0);
            push_channels(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_set_page(move |p| {
        if let Some(w) = w.upgrade() {
            w.set_livetv_page(p.max(0));
            push_channels(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_set_sort(move |key, asc| {
        if let Some(w) = w.upgrade() {
            w.set_livetv_sort(key);
            w.set_livetv_asc(asc);
            w.set_livetv_page(0);
            push_channels(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_play(move |index| {
        let Some(w) = w.upgrade() else { return };
        let Some(ch) = state().channels.get(index.max(0) as usize).cloned() else {
            return;
        };
        w.set_livetv_status(format!("Opening {} in mpv…", ch.name).into());
        state().now = Some((index.max(0) as usize, ch.clone()));
        show_now(&w, index.max(0), &ch);

        let epoch = PLAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
        let weak = w.as_weak();
        let on_end: PlaybackEnd = Arc::new(move |_pos, _dur| {
            // Only the *current* channel's exit clears the pill; a switch
            // retires the previous epoch first.
            if PLAY_GEN.load(Ordering::SeqCst) != epoch {
                return;
            }
            let _ = weak.upgrade_in_event_loop(|w| {
                state().now = None;
                clear_now(&w);
            });
        });
        // A live stream has no resume point and no library row to key one off.
        spawn_mpv_windowed_tracked(
            PathBuf::from(ch.url.clone()),
            None,
            None,
            Vec::new(),
            Some(on_end),
        );
        watch_mpv(w.as_weak(), ch.name.clone(), epoch);
    });

    let w = window.as_weak();
    window.on_livetv_info_opened(move || {
        let Some(w) = w.upgrade() else { return };
        let Some((_, ch)) = state().now.clone() else { return };
        w.set_livetv_now_cache(cache_line(&ch.logo).into());
    });

    // ── picker ──

    let w = window.as_weak();
    window.on_livetv_config_opened(move || {
        if let Some(w) = w.upgrade() {
            push_playlists(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_config_search(move |_q| {
        if let Some(w) = w.upgrade() {
            push_playlists(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_toggle_playlist(move |url| {
        let Some(w) = w.upgrade() else { return };
        {
            let mut st = state();
            // `SharedString` on the left: it implements `PartialEq<String>`,
            // and `String` has no matching impl the other way round.
            match st.picked.iter().position(|p| url == *p) {
                Some(at) => {
                    st.picked.remove(at);
                }
                None => st.picked.push(url.to_string()),
            }
        }
        w.set_livetv_picked_count(state().picked.len() as i32);
        push_playlists(&w);
    });

    let w = window.as_weak();
    window.on_livetv_config_clear(move || {
        let Some(w) = w.upgrade() else { return };
        state().picked.clear();
        w.set_livetv_picked_count(0);
        push_playlists(&w);
    });

    let w = window.as_weak();
    window.on_livetv_config_save(move || {
        let Some(w) = w.upgrade() else { return };
        let picked = state().picked.clone();
        match livetv::set_selection(&picked) {
            Ok(saved) => {
                state().picked = saved;
                w.set_livetv_picked_count(state().picked.len() as i32);
                load(w.as_weak(), true);
            }
            Err(msg) => w.set_livetv_status(msg.into()),
        }
    });
}

// ── loading ────────────────────────────────────────────────────────────────

/// Fetch every selected playlist and paint the grid.
fn load(weak: slint::Weak<MainWindow>, announce: bool) {
    let urls = state().picked.clone();
    if urls.is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_livetv_channels(ModelRc::new(VecModel::<LiveChannel>::default()));
            w.set_livetv_groups(ModelRc::new(VecModel::<LiveGroup>::default()));
            w.set_livetv_total(0);
            w.set_livetv_matched(0);
            w.set_livetv_pages(1);
            w.set_livetv_page(0);
            w.set_livetv_busy(false);
        });
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("livetv: no tokio runtime; not loading");
        return;
    };
    let epoch = GEN.fetch_add(1, Ordering::SeqCst) + 1;
    let n = urls.len();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_livetv_busy(true);
        if announce {
            w.set_livetv_status(
                format!("Fetching {n} playlist{}…", if n == 1 { "" } else { "s" }).into(),
            );
        }
    });

    handle.spawn(async move {
        let channels = livetv::load(tulipix_core::net::http(), &urls).await;
        if GEN.load(Ordering::SeqCst) != epoch {
            return; // a newer load started while these were downloading
        }
        let count = channels.len();
        let _ = weak.upgrade_in_event_loop(move |w| {
            state().channels = channels;
            w.set_livetv_total(count as i32);
            w.set_livetv_page(0);
            w.set_livetv_busy(false);
            w.set_livetv_status(
                if count == 0 {
                    "Those playlists came back empty.".to_string()
                } else {
                    format!("{count} channels")
                }
                .into(),
            );
            push_groups(&w);
            // Which also fetches the logos for the first page.
            push_channels(&w);
            resync_now(&w);
        });
    });
}

thread_local! {
    /// Logos already fetched, keyed by their remote URL: the decoded image and
    /// the file it was decoded from.
    ///
    /// `slint::Image` is not `Send`, so the decode has to happen on the UI
    /// thread — the paths cross the thread boundary, not the images. A URL that
    /// failed is stored with a blank image, so a dead logo is asked for once
    /// per session rather than once per page turn. The bytes themselves are
    /// already persistent: `cache_covers` writes them to the Videos section's
    /// on-disk cover cache, keyed by a hash of the URL, so a channel's logo
    /// survives a restart and is never downloaded twice.
    static LOGOS: RefCell<HashMap<String, (slint::Image, Option<PathBuf>)>> =
        RefCell::new(HashMap::new());
    /// URLs with a fetch in flight, so paging back and forth cannot ask twice.
    static PENDING: RefCell<HashSet<String>> = RefCell::new(HashSet::new());
}

/// Download the logos for the channels now on screen, then repaint.
fn fetch_logos(w: &MainWindow, urls: Vec<String>) {
    let want: Vec<String> = urls
        .into_iter()
        .filter(|u| !u.is_empty())
        .filter(|u| LOGOS.with(|m| !m.borrow().contains_key(u)))
        .filter(|u| PENDING.with(|p| p.borrow_mut().insert(u.clone())))
        .collect();
    if want.is_empty() {
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else { return };
    let weak = w.as_weak();
    let asked = want.clone();
    handle.spawn(async move {
        let paths = cache_covers(want).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            LOGOS.with(|m| {
                let mut m = m.borrow_mut();
                for (url, path) in asked.iter().zip(paths) {
                    m.insert(url.clone(), (load_image(path.clone()), path));
                }
            });
            PENDING.with(|p| {
                let mut p = p.borrow_mut();
                for url in &asked {
                    p.remove(url);
                }
            });
            push_channels(&w);
            // The pill's own logo may have been in this batch.
            if let Some((idx, ch)) = state().now.clone() {
                show_now(&w, idx as i32, &ch);
            }
        });
    });
}

/// The cached image for a logo URL, blank until it has been fetched.
fn logo_image(url: &str) -> slint::Image {
    LOGOS.with(|m| m.borrow().get(url).map(|(img, _)| img.clone()).unwrap_or_default())
}

/// "Logo cached · 4.2 KB" for the detail sheet.
fn cache_line(url: &str) -> String {
    if url.is_empty() {
        return "This channel publishes no logo.".into();
    }
    match LOGOS.with(|m| m.borrow().get(url).map(|(_, p)| p.clone())) {
        Some(Some(path)) => {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
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

// ── now playing ────────────────────────────────────────────────────────────

/// `name`, cut to [`PILL_CHARS`] with an ellipsis. Counted in characters, not
/// bytes — channel names are full of accents and non-Latin scripts.
fn pill_label(name: &str) -> String {
    if name.chars().count() <= PILL_CHARS {
        return name.to_string();
    }
    let head: String = name.chars().take(PILL_CHARS).collect();
    format!("{}…", head.trim_end())
}

fn show_now(w: &MainWindow, index: i32, ch: &Channel) {
    w.set_livetv_now_index(index);
    w.set_livetv_now_label(pill_label(&ch.name).into());
    w.set_livetv_now_res(if ch.res == 0 {
        SharedString::new()
    } else {
        format!("{}p", ch.res).into()
    });
    w.set_livetv_now_name(ch.name.clone().into());
    w.set_livetv_now_group(ch.group.clone().into());
    w.set_livetv_now_id(ch.id.clone().into());
    w.set_livetv_now_url(ch.url.clone().into());
    w.set_livetv_now_logo(logo_image(&ch.logo));
}

/// A reload replaces the whole channel list, so the playing channel's row may
/// have moved or gone. mpv is still playing either way — only the card
/// highlight is re-pointed, by stream URL rather than by the stale index.
fn resync_now(w: &MainWindow) {
    let found = {
        let st = state();
        let Some((_, now)) = st.now.as_ref() else { return };
        st.channels.iter().position(|c| c.url == now.url).map(|i| (i, now.clone()))
    };
    match found {
        Some((i, ch)) => {
            state().now = Some((i, ch.clone()));
            show_now(w, i as i32, &ch);
        }
        // Dropped from the selection mid-play. The pill stays — the stream is
        // still on — but no card claims it.
        None => w.set_livetv_now_index(-1),
    }
}

fn clear_now(w: &MainWindow) {
    w.set_livetv_now_index(-1);
    w.set_livetv_now_name(SharedString::new());
    w.set_livetv_now_label(SharedString::new());
    w.set_livetv_now_res(SharedString::new());
    w.set_livetv_now_group(SharedString::new());
    w.set_livetv_now_id(SharedString::new());
    w.set_livetv_now_url(SharedString::new());
    w.set_livetv_now_logo(slint::Image::default());
    w.set_livetv_now_cache(SharedString::new());
    w.set_livetv_info_open(false);
}

/// Follow the mpv we just spawned over its own JSON IPC socket.
///
/// Two jobs, both of which need to know what mpv is doing after the process is
/// already up:
///
/// * The status line says "Opening …" from the moment the button is pressed. A
///   live stream can take seconds to negotiate, so the line is only promoted to
///   "Playing …" once mpv reports a running clock.
/// * Switching audio track on a live stream leaves mpv playing buffered video
///   with nothing on it: the new track starts downloading from the live edge,
///   and the picture has to catch up to it before there is sound. Seeking to
///   the end of the seekable window on every `aid` change closes that gap
///   instead of waiting it out.
///
/// A second IPC client alongside the one `spawn_mpv_windowed_tracked` opens;
/// mpv serves several. If the connection fails the stream still plays — the
/// status line simply stays on "Opening".
fn watch_mpv(weak: slint::Weak<MainWindow>, name: String, epoch: u64) {
    std::thread::spawn(move || {
        use std::io::{BufRead, BufReader, Write};
        // The socket is created by the mpv being spawned right now, after it
        // unlinks the previous one. Connecting into that gap gets a refusal
        // from the dead socket, which `connect` (rightly) does not retry.
        std::thread::sleep(std::time::Duration::from_millis(400));
        let sock = tulipix_common::mpv_ipc::endpoint("tulipix-mpv");
        let Ok(conn) = tulipix_common::mpv_ipc::connect(&sock) else { return };
        let Ok(mut tx) = conn.try_clone() else { return };
        if tx
            .write_all(
                b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n\
                  {\"command\":[\"observe_property\",2,\"aid\"]}\n",
            )
            .is_err()
        {
            return;
        }
        let mut playing = false;
        let mut seen_aid = false;
        for line in BufReader::new(conn).lines().map_while(Result::ok) {
            if PLAY_GEN.load(Ordering::SeqCst) != epoch {
                return; // another channel owns the window now
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if v["event"] != "property-change" {
                continue;
            }
            match v["name"].as_str() {
                Some("time-pos") if !playing => {
                    if v["data"].as_f64().unwrap_or(0.0) > 0.0 {
                        playing = true;
                        let label = name.clone();
                        let _ = weak.upgrade_in_event_loop(move |w| {
                            if PLAY_GEN.load(Ordering::SeqCst) == epoch {
                                w.set_livetv_status(format!("Playing {label} in mpv").into());
                            }
                        });
                    }
                }
                Some("aid") => {
                    // The first report is the track mpv opened with, which is
                    // already at the live edge.
                    if seen_aid {
                        let _ =
                            tx.write_all(b"{\"command\":[\"seek\",100,\"absolute-percent\"]}\n");
                    }
                    seen_aid = true;
                }
                _ => {}
            }
        }
    });
}

// ── models ─────────────────────────────────────────────────────────────────

/// The sidebar: every distinct `group-title`, alphabetical, with how many
/// channels carry it — the count the row prints on its pill.
fn push_groups(w: &MainWindow) {
    let mut groups: Vec<(String, i32)> = Vec::new();
    for c in state().channels.iter() {
        if c.group.is_empty() { continue; }
        match groups.iter_mut().find(|(g, _)| *g == c.group) {
            Some((_, n)) => *n += 1,
            None => groups.push((c.group.clone(), 1)),
        }
    }
    groups.sort_by_key(|(g, _)| g.to_lowercase());
    let rows: Vec<LiveGroup> = groups.into_iter()
        .map(|(name, count)| LiveGroup { name: name.into(), count })
        .collect();
    w.set_livetv_groups(ModelRc::new(VecModel::from(rows)));
}

/// One page of the grid: narrowed by the search box and the sidebar, ordered by
/// the sort chips, sliced to [`PAGE_SIZE`].
///
/// `index` on each row is the position in the *unfiltered* list, so a click
/// still finds the right channel after a filter, a sort and a page turn.
fn push_channels(w: &MainWindow) {
    let q = w.get_livetv_query().trim().to_lowercase();
    let group = w.get_livetv_group().to_string();
    let sort = w.get_livetv_sort().to_string();
    let asc = w.get_livetv_asc();

    let (rows, wanted, matched, pages, page) = {
        let st = state();
        let mut ord: Vec<(usize, &Channel)> = st
            .channels
            .iter()
            .enumerate()
            .filter(|(_, c)| group.is_empty() || c.group == group)
            .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q))
            .collect();
        match sort.as_str() {
            "name" => ord.sort_by_key(|(_, c)| c.name.to_lowercase()),
            "group" => ord.sort_by_key(|(_, c)| (c.group.to_lowercase(), c.name.to_lowercase())),
            // Resolution comes off the display name, so plenty of channels do
            // not publish one; they sort as zero, which puts them together at
            // one end rather than scattered through the grid.
            "res" => ord.sort_by_key(|(_, c)| (c.res, c.name.to_lowercase())),
            // "default" is the order the playlists were merged in, which is
            // the mirror's own — sorting it would throw that away.
            _ => {}
        }
        if !asc {
            ord.reverse();
        }

        let matched = ord.len();
        let pages = matched.div_ceil(PAGE_SIZE).max(1);
        let page = (w.get_livetv_page().max(0) as usize).min(pages - 1);
        let start = page * PAGE_SIZE;
        let slice = &ord[start.min(matched)..(start + PAGE_SIZE).min(matched)];

        let rows: Vec<LiveChannel> = slice
            .iter()
            .map(|(i, c)| LiveChannel {
                index: *i as i32,
                id: c.id.clone().into(),
                name: c.name.clone().into(),
                group: c.group.clone().into(),
                logo: logo_image(&c.logo),
            })
            .collect();
        let wanted: Vec<String> = slice.iter().map(|(_, c)| c.logo.clone()).collect();
        (rows, wanted, matched, pages, page)
    };

    w.set_livetv_matched(matched as i32);
    w.set_livetv_pages(pages as i32);
    w.set_livetv_page(page as i32);
    w.set_livetv_channels(ModelRc::new(VecModel::from(rows)));
    fetch_logos(w, wanted);
}

/// The picker list for the open tab, narrowed by its own filter box.
fn push_playlists(w: &MainWindow) {
    let want = match w.get_livetv_config_tab().as_str() {
        "category" => Kind::Category,
        "language" => Kind::Language,
        _ => Kind::Country,
    };
    let q = w.get_livetv_config_query().trim().to_lowercase();
    let picked = state().picked.clone();

    let rows: Vec<LivePlaylist> = livetv::catalogue()
        .into_iter()
        .filter(|(kind, _, _)| *kind == want)
        .filter(|(_, name, _)| q.is_empty() || name.to_lowercase().contains(&q))
        .map(|(kind, name, url)| LivePlaylist {
            on: picked.iter().any(|p| *p == url),
            name: name.into(),
            kind: kind.label().to_lowercase().into(),
            url: url.into(),
        })
        .collect();
    w.set_livetv_playlists(ModelRc::new(VecModel::from(rows)));
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
        assert_eq!(pill_label(&"a".repeat(PILL_CHARS + 1)), format!("{}…", "a".repeat(PILL_CHARS)));
        // Multi-byte names must be cut on a character boundary — slicing by
        // byte would panic on the first accent.
        let cyrillic = "Первый канал Россия Москва";
        let cut = pill_label(cyrillic);
        assert!(cut.ends_with('…'));
        assert!(cut.chars().count() <= PILL_CHARS + 1, "{cut}");
    }

    #[test]
    fn a_channel_without_a_logo_is_not_reported_as_a_failed_download() {
        assert!(cache_line("").contains("no logo"));
    }
}
