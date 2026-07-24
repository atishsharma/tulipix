//! Bookmarks — the Saved page and the detail pane's save button.

use super::*;

/// Toggle the open title's saved state and reflect it on the button.
pub fn stream_toggle_bookmark(weak: slint::Weak<MainWindow>) {
    let Some(d) = with_state(|st| st.details.clone()) else { return };
    let Some(subject) = opened_subject() else { return };
    let split = with_state(|st| st.season_subjects.clone());
    // For a split show, save the whole show under its first-season subject so it
    // reopens on season 1, not whichever season happened to be on screen.
    let (subject_id, is_series) = if let Some((_, first)) = split.first() {
        (first.clone(), true)
    } else {
        (subject, d.is_series)
    };
    let bm = stream::bookmarks::Bookmark {
        subject_id,
        title: d.title.clone(),
        year: d.year.clone(),
        cover_url: d.cover.clone(),
        is_series,
        meta: meta_line(&d),
        overview: d.overview.clone(),
        // What the show lists today is the baseline; the badge is for what
        // turns up after this.
        seen_max_ep: d.seasons.last().map(|s| s.max_ep).unwrap_or(0),
        latest_max_ep: 0,
    };
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let saved = stream::bookmarks::toggle(&pool, &bm).await.unwrap_or(false);
        with_state(|st| st.bookmarked = saved);
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_video_stream_bookmarked(saved);
            w.set_video_stream_status(
                if saved { "Saved to bookmarks." } else { "Removed from bookmarks." }.into(),
            );
        });
    });
}

/// Populate the Saved page.
pub fn stream_bookmarks_load(weak: slint::Weak<MainWindow>) {
    let (key, asc) = with_state(|st| (st.bm_sort.clone(), st.bm_asc));
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let mut list = stream::bookmarks::list(&pool).await;
        sort_bookmarks(&mut list, &key, asc);
        let covers = cache_covers(list.iter().map(|b| b.cover_url.clone()).collect()).await;
        let cards: Vec<(stream::bookmarks::Bookmark, Option<PathBuf>)> =
            list.into_iter().zip(covers).collect();
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<StreamCard> = cards
                .into_iter()
                .map(|(b, poster)| {
                    // `seasons` doubles as the new-episode badge count here, and
                    // has to be read before the rest of `b` moves into the card.
                    let badge = if b.has_new() { b.new_count() as i32 } else { 0 };
                    StreamCard {
                        id: b.subject_id.into(),
                        title: b.title.into(),
                        year: b.year.into(),
                        poster: load_image(poster),
                        is_series: b.is_series,
                        seasons: badge,
                        meta: b.meta.into(),
                        overview: truncate_chars(&b.overview, 100).into(),
                        progress: 0.0,
                    }
                })
                .collect();
            w.set_video_stream_bookmarks(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

/// Check saved series for episodes added since they were last looked at.
///
/// Runs when the Saved page opens, at most once a day per show, and one at a
/// time — this is background curiosity, not something worth a burst of traffic.
pub fn stream_bookmarks_refresh(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let due = stream::bookmarks::due_for_check(&pool, 24 * 60 * 60).await;
        if due.is_empty() {
            return;
        }
        let Ok(c) = client().await else { return };
        let mut news = false;
        for bm in due {
            let Ok(d) = c.details(&bm.subject_id).await else { continue };
            let latest = d.seasons.last().map(|s| s.max_ep).unwrap_or(0);
            if latest <= 0 {
                continue;
            }
            news |= stream::bookmarks::note_latest(&pool, &bm.subject_id, latest)
                .await
                .unwrap_or(false);
        }
        // Only repaint when something actually changed.
        if news {
            stream_bookmarks_load(weak);
        }
    });
}

// ---- watch history ----

/// Fill the History page: everything played, newest first.
pub fn stream_history_load(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let entries = stream::progress::history(&pool, 200).await;
        let covers = cache_covers(entries.iter().map(|e| e.cover_url.clone()).collect()).await;
        let rows: Vec<(stream::progress::Entry, Option<PathBuf>)> =
            entries.into_iter().zip(covers).collect();

        let _ = weak.upgrade_in_event_loop(move |w| {
            let cards: Vec<StreamCard> = rows
                .into_iter()
                .map(|(e, poster)| StreamCard {
                    id: e.subject_id.clone().into(),
                    title: e.title.clone().into(),
                    year: history_line(&e).into(),
                    poster: load_image(poster),
                    is_series: e.is_series,
                    seasons: 0,
                    meta: Default::default(),
                    overview: Default::default(),
                    progress: if e.finished { 1.0 } else { e.fraction() as f32 },
                })
                .collect();
            w.set_video_stream_history(slint::ModelRc::new(slint::VecModel::from(cards)));
        });
    });
}

/// "S02E04 · watched" / "S02E04 · 34%" under a history card.
fn history_line(e: &stream::progress::Entry) -> String {
    let mut parts: Vec<String> = Vec::new();
    if e.is_series && (e.season > 0 || e.episode > 0) {
        parts.push(format!("S{:02}E{:02}", e.season.max(1), e.episode.max(1)));
    }
    parts.push(if e.finished {
        "watched".to_string()
    } else {
        format!("{:.0}%", e.fraction() * 100.0)
    });
    parts.join("  ·  ")
}

/// Forget everything on the History page.
pub fn stream_history_clear(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::progress::clear(&pool).await;
        }
        stream_history_load(weak.clone());
        stream_feed_load(weak);
    });
}

/// Forget one title's history.
pub fn stream_history_remove(weak: slint::Weak<MainWindow>, subject_id: String) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::progress::remove(&pool, &subject_id).await;
        }
        stream_history_load(weak);
    });
}

/// Sort the Saved list in place. `list()` already returns newest-first, so the
/// "date" descending case is a no-op.
fn sort_bookmarks(list: &mut [stream::bookmarks::Bookmark], key: &str, asc: bool) {
    match key {
        "name" => list.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase())),
        // Movies before series (then title), so the two kinds group together.
        "type" => list.sort_by(|a, b| {
            a.is_series.cmp(&b.is_series).then(a.title.to_lowercase().cmp(&b.title.to_lowercase()))
        }),
        _ => {} // "date": already newest-first from the query
    }
    if asc {
        list.reverse();
    }
}

/// Change the Saved-page sort and repaint.
pub fn stream_bookmarks_sort(weak: slint::Weak<MainWindow>, key: String, asc: bool) {
    with_state(|st| {
        st.bm_sort = key;
        st.bm_asc = asc;
    });
    stream_bookmarks_load(weak);
}

/// Remove one entry from the Saved page and refresh it.
pub fn stream_bookmark_remove(weak: slint::Weak<MainWindow>, subject_id: String) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::bookmarks::remove(&pool, &subject_id).await;
        }
        stream_bookmarks_load(weak);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookmark_sort_orders_by_key_and_direction() {
        use tulipix_videos::stream::bookmarks::Bookmark;
        let mk = |id: &str, title: &str, series: bool| Bookmark {
            subject_id: id.into(), title: title.into(), year: "2020".into(),
            cover_url: String::new(), is_series: series,
            meta: String::new(), overview: String::new(),
            seen_max_ep: 0, latest_max_ep: 0,
        };
        // list() is newest-first; simulate [b, a] as that order.
        let base = vec![mk("b", "Zebra", false), mk("a", "Apple", true)];

        let mut by_name = base.clone();
        sort_bookmarks(&mut by_name, "name", false);
        assert_eq!(by_name[0].title, "Apple");
        let mut by_name_desc = base.clone();
        sort_bookmarks(&mut by_name_desc, "name", true);
        assert_eq!(by_name_desc[0].title, "Zebra");

        let mut by_type = base.clone();
        sort_bookmarks(&mut by_type, "type", false); // movies before series
        assert!(!by_type[0].is_series);

        let mut by_date = base.clone();
        sort_bookmarks(&mut by_date, "date", false); // unchanged
        assert_eq!(by_date[0].subject_id, "b");
    }
}
