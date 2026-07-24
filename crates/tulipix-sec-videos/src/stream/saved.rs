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
                .map(|(b, poster)| StreamCard {
                    id: b.subject_id.into(),
                    title: b.title.into(),
                    year: b.year.into(),
                    poster: load_image(poster),
                    is_series: b.is_series,
                    seasons: 0,
                    meta: b.meta.into(),
                    overview: truncate_chars(&b.overview, 100).into(),
                })
                .collect();
            w.set_video_stream_bookmarks(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
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
