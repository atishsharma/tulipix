//! Live TV wiring: the iptv-org playlist picker, the channel grid, and handing
//! a channel to mpv.
//!
//! A channel is a live HLS URL and nothing else — there is no detail page, no
//! resume point and no episode. That is why this file is a fraction of the size
//! of the Stream tab's: load a list, filter it, play one.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tulipix_common::spawn_mpv_windowed;
use tulipix_ui::*;
use tulipix_videos::livetv::{self, Channel, Kind};

use crate::stream::{cache_covers, load_image};

/// Channel logos are small, but a country list is hundreds of them. Only the
/// first screenful or two are fetched up front; the rest keep the lettered
/// placeholder the card already draws.
const LOGO_BUDGET: usize = 60;

/// Everything currently loaded, plus the picker's working selection.
///
/// The selection is kept here rather than read back from settings on every
/// click, so ticking a box does not write a file per keystroke.
#[derive(Default)]
struct State {
    channels: Vec<Channel>,
    /// URLs of the playlists ticked in the picker, saved on Load.
    picked: Vec<String>,
}

fn state() -> MutexGuard<'static, State> {
    static S: OnceLock<Mutex<State>> = OnceLock::new();
    S.get_or_init(Mutex::default).lock().unwrap_or_else(|e| e.into_inner())
}

/// Bumped by every load. A slow playlist that lands after a newer load started
/// is dropped rather than painting over it.
static GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
            push_channels(&w);
        }
    });

    let w = window.as_weak();
    window.on_livetv_set_group(move |g| {
        if let Some(w) = w.upgrade() {
            w.set_livetv_group(g);
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
        // A live stream has no resume point and no library row to key one off.
        spawn_mpv_windowed(std::path::PathBuf::from(ch.url), None, None);
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
            w.set_livetv_groups(ModelRc::new(VecModel::<SharedString>::default()));
            w.set_livetv_total(0);
            w.set_livetv_busy(false);
        });
        return;
    }
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        tracing::warn!("livetv: no tokio runtime; not loading");
        return;
    };
    let epoch = GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
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
        if GEN.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            return; // a newer load started while these were downloading
        }
        // Logos are ordinary remote images, so the Stream tab's own poster
        // cache does the fetching and the on-disk reuse.
        let logo_urls: Vec<String> =
            channels.iter().take(LOGO_BUDGET).map(|c| c.logo.clone()).collect();
        let logos = cache_covers(logo_urls).await;
        if GEN.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            return;
        }
        let count = channels.len();
        let _ = weak.upgrade_in_event_loop(move |w| {
            state().channels = channels;
            LOGOS.with(|cell| *cell.borrow_mut() = logos);
            w.set_livetv_total(count as i32);
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
            push_channels(&w);
        });
    });
}

thread_local! {
    /// Cached logo paths, parallel to the first [`LOGO_BUDGET`] channels.
    /// `slint::Image` is not `Send`, so the decode has to happen on the UI
    /// thread — the paths cross the boundary, not the images.
    static LOGOS: std::cell::RefCell<Vec<Option<std::path::PathBuf>>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

// ── models ─────────────────────────────────────────────────────────────────

/// The group strip: every distinct `group-title`, alphabetical.
fn push_groups(w: &MainWindow) {
    let mut groups: Vec<String> = Vec::new();
    for c in state().channels.iter() {
        if !c.group.is_empty() && !groups.iter().any(|g| *g == c.group) {
            groups.push(c.group.clone());
        }
    }
    groups.sort_by_key(|g| g.to_lowercase());
    let rows: Vec<SharedString> = groups.into_iter().map(Into::into).collect();
    w.set_livetv_groups(ModelRc::new(VecModel::from(rows)));
}

/// The grid, narrowed by the search box and the group strip.
///
/// `index` on each row is the position in the *unfiltered* list, so a click
/// still finds the right channel after a filter.
fn push_channels(w: &MainWindow) {
    let q = w.get_livetv_query().trim().to_lowercase();
    let group = w.get_livetv_group().to_string();
    let logos = LOGOS.with(|cell| cell.borrow().clone());

    let rows: Vec<LiveChannel> = state()
        .channels
        .iter()
        .enumerate()
        .filter(|(_, c)| group.is_empty() || c.group == group)
        .filter(|(_, c)| q.is_empty() || c.name.to_lowercase().contains(&q))
        .map(|(i, c)| LiveChannel {
            index: i as i32,
            id: c.id.clone().into(),
            name: c.name.clone().into(),
            group: c.group.clone().into(),
            logo: load_image(logos.get(i).cloned().flatten()),
        })
        .collect();
    w.set_livetv_channels(ModelRc::new(VecModel::from(rows)));
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
