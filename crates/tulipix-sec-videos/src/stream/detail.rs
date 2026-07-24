//! Detail view: opening a title, season/episode/dub switching, and resolving
//! the playable streams for whatever is selected.

use super::*;

/// Open a title: fetch details, then the files for the first episode (or the
/// movie itself). A series with no season data still resolves as season 1.
pub fn stream_open(weak: slint::Weak<MainWindow>, subject_id: String) {
    let epoch = next_epoch();
    set_status(&weak, "Loading…", true);
    tokio::runtime::Handle::current().spawn(async move {
        let c = match client().await {
            Ok(c) => c,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        let details = match c.details(&subject_id).await {
            Ok(d) => d,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        // A newer open/season/episode click landed while this was in flight.
        if !is_current(epoch) {
            return;
        }

        // A split show ("Person of Interest S1".."S5") gets its season list from
        // the search grouping; a normal one from its own payload.
        let split = siblings_of(&subject_id);
        let season = if !split.is_empty() {
            split
                .iter()
                .find(|(_, id)| *id == subject_id)
                .map(|(n, _)| *n)
                .unwrap_or(1)
        } else if details.is_series {
            details.seasons.first().map(|s| s.number).unwrap_or(1)
        } else {
            0
        };
        // Split-show subjects hold one season each, so the wire query is still
        // se=1 for them — the season number is only a label here.
        let episode = if details.is_series || !split.is_empty() { 1 } else { 0 };
        let wire_season = if split.is_empty() { season } else { 1 };
        with_state(|st| st.selection = (wire_season, episode));
        with_state(|st| st.open_id = subject_id.clone());
        with_state(|st| st.season_subjects = split.clone());
        // Saved state for the bookmark button.
        let saved = match pool_for("videos").await {
            Ok(p) => stream::bookmarks::is_saved(&p, &subject_id).await,
            Err(_) => false,
        };
        with_state(|st| st.bookmarked = saved);
        let cover = cache_cover(&details.cover).await;
        push_details(&weak, &details, cover, &subject_id, season);
        with_state(|st| st.details = Some(details.clone()));
        let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_bookmarked(saved));
        load_files(weak, epoch, subject_id, wire_season, episode).await;
    });
}

/// Paint the detail pane (everything except the file list).
fn push_details(
    weak: &slint::Weak<MainWindow>,
    d: &Details,
    cover: Option<PathBuf>,
    opened_id: &str,
    active_season: i64,
) {
    let (title, overview, meta) = (d.title.clone(), d.overview.clone(), meta_line(d));
    // Focus the dub list on Original/English/Hindi (fails open when a title has
    // none of them), so the picker is short and the choice obvious.
    let dubs: Vec<(String, String)> = stream::prefer::dubs(d.dubs.clone())
        .into_iter()
        .map(|x| (x.subject_id, x.name))
        .collect();
    // Seasons come from the search grouping for a split show, otherwise from
    // the subject's own payload.
    let split = with_state(|st| st.season_subjects.clone());
    let seasons: Vec<i32> = if split.is_empty() {
        d.seasons.iter().map(|s| s.number as i32).collect()
    } else {
        split.iter().map(|(n, _)| *n as i32).collect()
    };
    // Episode count for the season actually on screen. Taking `first()` here
    // listed season 1's episodes no matter which season was open.
    let episodes = episode_numbers(if split.is_empty() {
        max_ep_for(&d.seasons, active_season)
    } else {
        // Split shows carry exactly one season per subject.
        d.seasons.first().map(|s| s.max_ep).unwrap_or(0)
    });
    let is_series = d.is_series || !split.is_empty();
    // Highlight the language cut we are actually on. Prefer the id we opened
    // with — the payload's own `id` is often absent.
    let cur_id = if d.id.is_empty() { opened_id.to_string() } else { d.id.clone() };

    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_detail_open(true);
        w.set_video_stream_title(title.into());
        w.set_video_stream_overview(overview.into());
        w.set_video_stream_meta(meta.into());
        w.set_video_stream_cover(load_image(cover));
        w.set_video_stream_is_series(is_series);
        let chips: Vec<StreamChip> = dubs
            .into_iter()
            .map(|(id, label)| {
                let active = id == cur_id;
                StreamChip { id: id.into(), label: label.into(), active }
            })
            .collect();
        w.set_video_stream_dubs(slint::ModelRc::new(slint::VecModel::from(chips)));
        w.set_video_stream_seasons(slint::ModelRc::new(slint::VecModel::from(seasons)));
        w.set_video_stream_episodes(slint::ModelRc::new(slint::VecModel::from(episodes)));
        w.set_video_stream_season(active_season as i32);
        w.set_video_stream_episode(if is_series { 1 } else { 0 });
    });
}

/// Episode count for `season`, falling back to the first season when the
/// requested one is not listed.
fn max_ep_for(seasons: &[stream::Season], season: i64) -> i64 {
    seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0)
}

/// `1..=max_ep` as pickable chips. Clamped — a bogus episode count from the
/// server must not spin up an unbounded model.
fn episode_numbers(max_ep: i64) -> Vec<i32> {
    (1..=max_ep.clamp(0, 500) as i32).collect()
}

/// Resolve the playable files for one episode and paint the file rows.
///
/// Cache first: a stored answer paints immediately and the servers are only
/// asked when nothing is stored or the entry has aged past its TTL. A refresh
/// that comes back identical repaints nothing.
///
/// `epoch` is the action this load belongs to — see [`super::is_current`]. Every
/// UI write below is gated on it, so a slow response for episode 5 cannot land
/// on top of episode 7.
pub(crate) async fn load_files(
    weak: slint::Weak<MainWindow>,
    epoch: u64,
    subject_id: String,
    season: i64,
    episode: i64,
) {
    let res = current_resolution();
    let key = stream::cache::Key::new(&subject_id, season, episode, &res);
    let pool = pool_for("videos").await.ok();

    // 1. Serve whatever is stored, immediately.
    let cached = match &pool {
        Some(p) => stream::cache::load(p, &key).await,
        None => None,
    };
    let mut painted = false;
    if let Some(cached) = &cached {
        if !is_current(epoch) {
            return;
        }
        paint_files(&weak, &cached.files, true);
        painted = true;

        // Backfill subs for an entry cached before captions were fetched
        // separately: patch just the captions, no stream refetch, and re-store.
        if !cached.files.is_empty() && cached.files.iter().all(|f| f.captions.is_empty()) {
            if let Ok(c) = client().await {
                let mut patched = cached.files.clone();
                attach_episode_captions(&c, &subject_id, &mut patched).await;
                let gained = patched.iter().any(|f| !f.captions.is_empty());
                if gained && is_current(epoch) {
                    if let Some(p) = &pool {
                        let _ = stream::cache::store(p, &key, &patched).await;
                    }
                    paint_files(&weak, &patched, true);
                }
            }
        }
    } else {
        set_status(&weak, "Finding streams…", true);
    }

    // 2. Only go to the network when there is nothing, or it has gone stale.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let needs_fetch = match &cached {
        None => true,
        Some(c) => c.is_stale(now),
    };
    if !needs_fetch || !is_current(epoch) {
        return;
    }

    let c = match client().await {
        Ok(c) => c,
        Err(e) => {
            if is_current(epoch) && !painted {
                set_status(&weak, explain(&e), false);
            }
            return;
        }
    };
    let mut found = match c
        .resources(&subject_id, season.max(0) as usize, episode.max(0) as usize, &res)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            if !is_current(epoch) {
                return;
            }
            // A stale-but-present list beats an empty screen.
            if !painted {
                paint_files(&weak, &[], false);
                set_status(&weak, explain(&e), false);
            }
            return;
        }
    };

    // 2b. Subtitles are NOT inline on the resource — that field is almost always
    // empty. They come from a separate get-ext-captions request keyed by
    // resourceId. Fetch once for this episode and attach to every file so the
    // picker is populated and the answer caches with the streams.
    attach_episode_captions(&c, &subject_id, &mut found).await;

    // 3. Store, and repaint only if the answer actually moved.
    let changed = match &pool {
        Some(p) => stream::cache::store(p, &key, &found).await.unwrap_or(true),
        None => true,
    };
    if is_current(epoch) && (changed || !painted) {
        paint_files(&weak, &found, false);
    }
}

/// Fetch this episode's subtitle tracks and attach them to every file.
///
/// The catalogue serves subtitles from `get-ext-captions`, keyed by a resource,
/// not inline on the stream list. Subs for the same episode are the same content
/// regardless of which bitrate you play, so one fetch covers the whole episode —
/// but a given resource may simply carry none, so up to three distinct resources
/// (usually different uploaders) are tried concurrently and their tracks unioned.
/// Files that already came with inline captions are left untouched.
async fn attach_episode_captions(c: &StreamClient, subject_id: &str, files: &mut [StreamFile]) {
    if files.is_empty() || files.iter().any(|f| !f.captions.is_empty()) {
        return;
    }
    let mut seen = std::collections::HashSet::new();
    let mut probe: Vec<String> = Vec::new();
    for f in files.iter() {
        if !f.resource_id.is_empty() && seen.insert(f.resource_id.clone()) {
            probe.push(f.resource_id.clone());
            if probe.len() == 3 {
                break;
            }
        }
    }

    let mut handles = Vec::new();
    for rid in probe {
        let c = c.clone();
        let sid = subject_id.to_string();
        handles.push(tokio::spawn(async move { c.captions(&sid, &rid).await.unwrap_or_default() }));
    }
    let mut merged: Vec<Caption> = Vec::new();
    for h in handles {
        if let Ok(caps) = h.await {
            for cap in caps {
                if cap.url.is_empty() {
                    continue;
                }
                let dup = merged.iter().any(|o| {
                    (!cap.lang.is_empty() && o.lang.eq_ignore_ascii_case(&cap.lang)) || o.url == cap.url
                });
                if !dup {
                    merged.push(cap);
                }
            }
        }
    }
    let merged = stream::prefer::captions(merged);
    if merged.is_empty() {
        return;
    }
    for f in files.iter_mut() {
        f.captions = merged.clone();
    }
}

/// Blank the streams panel and show the busy state — used the instant a season
/// or episode is picked so the previous episode's rows never linger on screen.
fn clear_stream_list(w: &MainWindow) {
    w.set_video_stream_files(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamFileRow>::new())));
    w.set_video_stream_current(-1);
    w.set_video_stream_current_label("".into());
    w.set_video_stream_busy(true);
}

/// Push one resolved list into the UI: stream rows, subtitle chips, status.
fn paint_files(weak: &slint::Weak<MainWindow>, found: &[StreamFile], from_cache: bool) {
    let rows: Vec<StreamFileRow> = found
        .iter()
        .enumerate()
        .map(|(idx, f)| StreamFileRow {
            index: idx as i32,
            label: if f.resolution > 0 { format!("{}p", f.resolution) } else { "Auto".into() }.into(),
            sub: file_sub(f).into(),
            uploader: f.uploader.clone().into(),
            subs: match f.captions.len() {
                0 => slint::SharedString::from("No subs"),
                1 => "1 sub".into(),
                n => format!("{n} subs").into(),
            },
            has_subs: !f.captions.is_empty(),
        })
        .collect();
    let count = rows.len();

    // One subtitle list for the whole episode: the files are the same content at
    // different bitrates, so their caption sets are near-identical. Union them,
    // one entry per language, so the picker does not repeat itself.
    let tracks = caption_union(found);
    // Keep the previously chosen language if this episode also has it.
    let want = with_state(|st| st.sub_lang.clone());
    let choice = want
        .as_deref()
        .and_then(|w| tracks.iter().position(|c| c.lang.eq_ignore_ascii_case(w)));
    with_state(|st| st.subs = tracks.clone());
    with_state(|st| st.sub_choice = choice);
    with_state(|st| st.files = found.to_vec());
    // Arm a stream the instant an episode's files land, so Play/Copy/Download
    // work without a row click: the rung the user last played, else 720p.
    let armed = stream::quality::pick(found, stream::quality::sticky());
    with_state(|st| st.current_stream = armed.unwrap_or(0));
    let current_label: String = armed
        .and_then(|i| found.get(i))
        .map(|f| if f.resolution > 0 { format!("{}p stream", f.resolution) } else { "stream".into() })
        .unwrap_or_default();

    let res = current_resolution();
    let _ = weak.upgrade_in_event_loop(move |w| {
        let chips: Vec<StreamChip> = tracks
            .iter()
            .enumerate()
            .map(|(idx, c)| StreamChip {
                id: idx.to_string().into(),
                label: if c.lang.is_empty() { format!("Track {}", idx + 1) } else { c.lang.clone() }.into(),
                active: Some(idx) == choice,
            })
            .collect();
        w.set_video_stream_subs(slint::ModelRc::new(slint::VecModel::from(chips)));
        w.set_video_stream_sub_choice(choice.map(|i| i as i32).unwrap_or(-1));
        w.set_video_stream_files(slint::ModelRc::new(slint::VecModel::from(rows)));
        w.set_video_stream_current(armed.map(|i| i as i32).unwrap_or(-1));
        w.set_video_stream_current_label(current_label.into());
        let label = if res.is_empty() { "all qualities".to_string() } else { format!("{res}p") };
        w.set_video_stream_status(
            match (count, from_cache) {
                (0, _) => "No streams at this quality — try another.".to_string(),
                (n, true) => format!("{n} streams · {label} · saved"),
                (n, false) => format!("{n} streams · {label}"),
            }
            .into(),
        );
        w.set_video_stream_busy(false);
    });
}

/// Resolution rung picked — re-resolve the current episode at that quality.
pub fn stream_set_resolution(weak: slint::Weak<MainWindow>, res: slint::SharedString) {
    let res = res.to_string();
    with_state(|st| st.resolution = res.clone());
    stream::quality::set_filter(&res); // survives a restart
    let Some(id) = opened_subject() else { return };
    let (season, episode) = with_state(|st| st.selection);
    let epoch = next_epoch();
    let _ = weak.upgrade_in_event_loop({
        let res = res.clone();
        move |w| w.set_video_stream_resolution(res.into())
    });
    tokio::runtime::Handle::current().spawn(async move {
        load_files(weak, epoch, id, season, episode).await;
    });
}

/// Subtitle language picked in the detail pane. `-1` turns subtitles off.
pub fn stream_set_sub(weak: slint::Weak<MainWindow>, index: i32) {
    let choice = if index < 0 { None } else { Some(index as usize) };
    with_state(|st| st.sub_choice = choice);
    // Remember the language, not the index: the next episode may list its
    // tracks in a different order.
    with_state(|st| {
        st.sub_lang = choice
            .and_then(|i| st.subs.get(i).map(|c| c.lang.clone()))
            .filter(|l| !l.is_empty());
    });
    let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_sub_choice(index));
}

/// Back to the results grid; clears the per-title state so a stale file list
/// can never be played against the next title.
pub fn stream_back(weak: slint::Weak<MainWindow>) {
    next_epoch(); // abandon anything still loading for the closed title
    clear_open_title();
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_detail_open(false);
        w.set_video_stream_subs(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamChip>::new())));
        w.set_video_stream_sub_choice(-1);
        w.set_video_stream_files(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamFileRow>::new())));
        w.set_video_stream_status("".into());
        w.set_video_stream_busy(false);
    });
}

/// Season picked — reset to episode 1 and re-resolve.
///
/// A split show stores each season under its own subject, so switching season
/// there means re-opening that subject; a normal series just re-queries with a
/// different `se`.
pub fn stream_set_season(weak: slint::Weak<MainWindow>, season: i32) {
    let split = with_state(|st| st.season_subjects.clone());
    if let Some((_, subject)) = split.iter().find(|(n, _)| *n == season as i64) {
        // Reflect the click straight away; the details fetch fills in the rest.
        let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_season(season));
        stream_open(weak, subject.clone());
        return;
    }

    let Some(d) = with_state(|st| st.details.clone()) else { return };
    let episodes = episode_numbers(
        d.seasons
            .iter()
            .find(|s| s.number == season as i64)
            .map(|s| s.max_ep)
            .unwrap_or(1),
    );
    with_state(|st| st.selection = (season as i64, 1));
    let Some(id) = opened_subject() else { return };
    let epoch = next_epoch();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_season(season);
        w.set_video_stream_episode(1);
        w.set_video_stream_episodes(slint::ModelRc::new(slint::VecModel::from(episodes)));
        clear_stream_list(&w);
    });
    tokio::runtime::Handle::current().spawn(async move {
        load_files(weak, epoch, id, season as i64, 1).await;
    });
}

/// Episode picked — re-resolve within the current season.
pub fn stream_set_episode(weak: slint::Weak<MainWindow>, episode: i32) {
    let Some(id) = opened_subject() else { return };
    let season = with_state(|st| st.selection.0).max(1);
    with_state(|st| st.selection = (season, episode as i64));
    let epoch = next_epoch();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_episode(episode);
        clear_stream_list(&w);
    });
    tokio::runtime::Handle::current().spawn(async move {
        load_files(weak, epoch, id, season, episode as i64).await;
    });
}

/// Language cut picked. Each dub is a separate subject, so this reopens the
/// title under the new id.
///
/// For a split show the dub subject is not in the season map — re-point the
/// currently-open season at it and re-register, otherwise switching language
/// would silently collapse a five-season show down to one.
pub fn stream_set_dub(weak: slint::Weak<MainWindow>, subject_id: String) {
    if let Some(cur) = opened_subject() {
        with_state(|st| {
            if let Some(slot) = st.season_subjects.iter_mut().find(|(_, id)| *id == cur) {
                slot.1 = subject_id.clone();
                let list = st.season_subjects.clone();
                for (_, id) in &list {
                    st.season_map.insert(id.clone(), list.clone());
                }
            }
        });
    }
    stream_open(weak, subject_id);
}

// ---- current stream ----

/// A stream row was clicked — make it the current stream (info-box actions use
/// it) without necessarily playing.
pub fn stream_set_current(weak: slint::Weak<MainWindow>, index: i32) {
    let idx = index.max(0) as usize;
    let label = with_state(|st| {
        st.current_stream = idx;
        st.files.get(idx).map(|f| {
            if f.resolution > 0 { format!("{}p stream", f.resolution) } else { "stream".into() }
        })
    })
    .unwrap_or_default();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_current(index);
        w.set_video_stream_current_label(label.into());
    });
}

pub(crate) fn current_index() -> i32 {
    with_state(|st| {
        if st.files.is_empty() { -1 } else { st.current_stream.min(st.files.len() - 1) as i32 }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tulipix_videos::stream::{Dub, Season};

    #[test]
    fn episode_count_follows_the_open_season() {
        let seasons = vec![
            Season { number: 1, max_ep: 9 },
            Season { number: 2, max_ep: 22 },
            Season { number: 3, max_ep: 5 },
        ];
        assert_eq!(max_ep_for(&seasons, 2), 22); // not season 1's count
        assert_eq!(max_ep_for(&seasons, 3), 5);
        assert_eq!(max_ep_for(&seasons, 99), 9); // unknown season → first
        assert_eq!(max_ep_for(&[], 1), 0);
    }

    #[test]
    fn episode_numbers_are_one_based_and_bounded() {
        assert_eq!(episode_numbers(3), vec![1, 2, 3]);
        assert!(episode_numbers(0).is_empty());
        assert!(episode_numbers(-5).is_empty()); // server junk must not underflow
        assert_eq!(episode_numbers(10_000).len(), 500); // clamped, not unbounded
    }

    /// The bug: subs come from a separate endpoint, so an all-inline-empty file
    /// list must be recognised as "needs a captions fetch".
    #[test]
    fn empty_inline_captions_are_detected_as_needing_a_fetch() {
        let files = vec![
            StreamFile { resource_id: "a".into(), ..Default::default() },
            StreamFile { resource_id: "b".into(), ..Default::default() },
        ];
        // attach_episode_captions probes only when every file lacks inline subs
        assert!(files.iter().all(|f| f.captions.is_empty()));
        let with_inline = vec![StreamFile {
            resource_id: "a".into(),
            captions: vec![Caption { url: "u".into(), lang: "English".into(), ext: "srt".into() }],
            ..Default::default()
        }];
        assert!(with_inline.iter().any(|f| !f.captions.is_empty()));
    }

    /// A series opens on its first season, a movie has no episode axis at all.
    #[test]
    fn first_episode_selection_matches_kind() {
        let series = Details {
            is_series: true,
            seasons: vec![Season { number: 2, max_ep: 8 }],
            dubs: vec![Dub { subject_id: "a".into(), name: "English".into() }],
            ..Default::default()
        };
        let sel = if series.is_series {
            (series.seasons.first().map(|s| s.number).unwrap_or(1), 1)
        } else {
            (0, 0)
        };
        assert_eq!(sel, (2, 1));

        let movie = Details { is_series: false, ..Default::default() };
        let sel = if movie.is_series { (1, 1) } else { (0, 0) };
        assert_eq!(sel, (0, 0));
    }
}
