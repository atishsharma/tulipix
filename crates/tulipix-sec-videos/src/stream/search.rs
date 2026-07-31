//! Search, suggestions, hover preview, and the recent-terms list.

use super::*;

/// Search the remote catalogue and repaint the results grid. Posters are cached
/// concurrently so a slow image host cannot hold up the whole grid.
pub fn stream_search(weak: slint::Weak<MainWindow>, query: String) {
    let query = query.trim().to_string();
    // A search always returns to the results/landing view — close any open
    // detail so it doesn't stay layered over the results.
    clear_open_title();
    let _ = weak.upgrade_in_event_loop(|w| w.set_video_stream_detail_open(false));
    with_state(|st| {
        st.query = query.clone();
        st.page = 0;
        st.results.clear();
    });
    if query.is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_results(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamCard>::new())));
            w.set_video_stream_more(false);
            w.set_video_stream_status("".into());
            w.set_video_stream_busy(false);
        });
        return;
    }
    run_search(weak, query, 1);
}

/// Pull the next page of the search on screen and append it.
///
/// The catalogue pages at 20 a time and never says how many there are, so a
/// short page is the only end-of-results signal there is.
pub fn stream_search_more(weak: slint::Weak<MainWindow>) {
    let (query, page) = with_state(|st| (st.query.clone(), st.page));
    if query.is_empty() || page == 0 {
        return;
    }
    run_search(weak, query, page + 1);
}

/// Page 1 replaces the grid; later pages append to it.
fn run_search(weak: slint::Weak<MainWindow>, query: String, page: usize) {
    let epoch = next_epoch(); // also abandons any detail load still in flight
    set_status(&weak, if page > 1 { "Loading more…" } else { "Searching…" }, true);
    tokio::runtime::Handle::current().spawn(async move {
        let c = match catalogue().await {
            Ok(c) => c,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        // Remember the term only once a server has actually been reached, so a
        // dead host list does not fill the landing screen with junk.
        if page == 1 {
            push_recent(&weak, &query);
        }
        let hits = match c.search(&query, page).await {
            Ok(h) => h,
            Err(e) => return set_status(&weak, explain(&e), false),
        };
        // Search "dune" then "loki" quickly and the slower answer must not win.
        if !is_current(epoch) {
            return;
        }
        if hits.is_empty() && page == 1 {
            with_state(|st| st.results.clear());
            let _ = weak.upgrade_in_event_loop(|w| {
                w.set_video_stream_results(slint::ModelRc::new(slint::VecModel::from(Vec::<StreamCard>::new())));
                w.set_video_stream_more(false);
            });
            return set_status(&weak, "Nothing found.", false);
        }

        remember_season_maps(&hits);
        // A page that came back short is the last one. Folding split seasons and
        // duplicate dubs shrinks a full page well below `PAGE_LEN`, so the test
        // is deliberately generous — offering one dead "load more" is a smaller
        // sin than hiding half the catalogue.
        let more = hits.len() >= PAGE_LEN / 2;
        let covers = cache_covers(hits.iter().map(|h| h.cover.clone()).collect()).await;
        let host = c.active_host().await;
        if !is_current(epoch) {
            return; // poster caching is slow; re-check before painting
        }

        let fresh: Vec<CardData> = hits
            .iter()
            .zip(covers)
            .map(|(h, poster)| CardData {
                id: h.id.clone(),
                title: h.title.clone(),
                line: h.year.clone(),
                // Result cards carry no resume state.
                meta: String::new(),
                note: String::new(),
                poster,
                is_series: h.is_series,
                // A split show advertises its season count on the card.
                seasons: h.season_subjects.len() as i32,
                progress: 0.0,
            })
            .collect();

        let all = with_state(|st| {
            if page == 1 {
                st.results = fresh.clone();
            } else {
                // The catalogue repeats itself across pages more than it should.
                for card in &fresh {
                    if !st.results.iter().any(|c| c.id == card.id) {
                        st.results.push(card.clone());
                    }
                }
            }
            st.page = page;
            st.results.clone()
        });

        let count = all.len();
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_video_stream_results(slint::ModelRc::new(slint::VecModel::from(cards_of(&all))));
            w.set_video_stream_more(more);
            w.set_video_stream_status(format!("{count} results · {host}").into());
            w.set_video_stream_busy(false);
        });
    });
}

/// What the catalogue returns per page (`perPage` in the search payload).
const PAGE_LEN: usize = 20;

// ---- search suggestions ----

/// Debounced autocomplete. Each keystroke bumps its own counter; only the last
/// one still standing after the pause actually asks the servers.
static SUGGEST_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn stream_suggest(weak: slint::Weak<MainWindow>, query: String) {
    use std::sync::atomic::Ordering;
    let query = query.trim().to_string();
    if query.len() < 2 {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_suggestions(slint::ModelRc::new(slint::VecModel::from(
                Vec::<slint::SharedString>::new(),
            )));
        });
        return;
    }
    let seq = SUGGEST_SEQ.fetch_add(1, Ordering::SeqCst) + 1;
    tokio::runtime::Handle::current().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        if SUGGEST_SEQ.load(Ordering::SeqCst) != seq {
            return; // superseded by a later keystroke
        }
        let Ok(c) = catalogue().await else { return };
        let Ok(names) = c.suggest(&query).await else { return };
        if SUGGEST_SEQ.load(Ordering::SeqCst) != seq {
            return;
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<slint::SharedString> = names.into_iter().map(Into::into).collect();
            w.set_video_stream_suggestions(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

pub fn stream_suggest_clear(weak: slint::Weak<MainWindow>) {
    SUGGEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_suggestions(slint::ModelRc::new(slint::VecModel::from(
            Vec::<slint::SharedString>::new(),
        )));
    });
}

// ---- hover preview ----

/// Details for the card under the cursor (the TUI's Info Preview pane).
///
/// Dwell-gated and memoised: skimming the grid must not fire a request per
/// card. An empty `subject_id` closes the preview.
pub fn stream_preview(weak: slint::Weak<MainWindow>, subject_id: String) {
    use std::sync::atomic::Ordering;
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::SeqCst) + 1;

    if subject_id.is_empty() {
        let _ = weak.upgrade_in_event_loop(|w| w.set_video_stream_preview_open(false));
        return;
    }
    if let Some(d) = with_state(|st| st.preview_cache.get(&subject_id).cloned()) {
        let cover_url = d.cover.clone();
        let weak2 = weak.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let cover = cache_cover(&cover_url).await;
            push_preview(&weak2, &d, cover);
        });
        return;
    }
    tokio::runtime::Handle::current().spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        if SEQ.load(Ordering::SeqCst) != seq {
            return; // pointer moved on
        }
        let Ok(c) = catalogue().await else { return };
        let Ok(d) = c.details(&subject_id).await else { return };
        with_state(|st| {
            // Bounded: the grid is at most a few dozen cards.
            if st.preview_cache.len() > 60 {
                st.preview_cache.clear();
            }
            st.preview_cache.insert(subject_id.clone(), d.clone());
        });
        let cover = cache_cover(&d.cover).await;
        if SEQ.load(Ordering::SeqCst) == seq {
            push_preview(&weak, &d, cover);
        }
    });
}

fn push_preview(weak: &slint::Weak<MainWindow>, d: &Details, cover: Option<PathBuf>) {
    let (title, meta, overview) = (d.title.clone(), meta_line(d), d.overview.clone());
    let seasons = d.seasons.len() as i32;
    let is_series = d.is_series;
    // Dub languages as a comma list — one more thing the card doesn't show.
    let langs: String = stream::prefer::dubs(d.dubs.clone())
        .iter()
        .map(|x| x.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_preview_title(title.into());
        w.set_video_stream_preview_meta(meta.into());
        w.set_video_stream_preview_overview(overview.into());
        w.set_video_stream_preview_seasons(seasons);
        w.set_video_stream_preview_is_series(is_series);
        w.set_video_stream_preview_langs(langs.into());
        w.set_video_stream_preview_cover(load_image(cover));
        w.set_video_stream_preview_open(true);
    });
}

// ---- recent searches ----

fn push_recent(weak: &slint::Weak<MainWindow>, term: &str) {
    publish_recent(weak, stream::recent::push(term));
}

fn publish_recent(weak: &slint::Weak<MainWindow>, list: Vec<String>) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        let rows: Vec<slint::SharedString> = list.into_iter().map(Into::into).collect();
        w.set_video_stream_recent(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
}

/// Fill the landing screen's recent list. Called when the Stream tab opens.
pub fn stream_recent_load(weak: slint::Weak<MainWindow>) {
    publish_recent(&weak, stream::recent::load());
}

pub fn stream_recent_clear(weak: slint::Weak<MainWindow>) {
    publish_recent(&weak, stream::recent::clear());
}
