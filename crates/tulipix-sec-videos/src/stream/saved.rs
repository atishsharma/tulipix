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

/// How many saved shows one refresh pass will check. `due_for_check` can return
/// forty; forty sequential detail fetches on tab open is a lot of traffic for a
/// badge, and whatever is left is still due on the next pass.
const REFRESH_BATCH: usize = 8;

/// Check saved series for episodes added since they were last looked at.
///
/// Runs when the tab opens, at most once a day per show, a few at a time and
/// one after another — this is background curiosity, not something worth a
/// burst of traffic.
pub fn stream_bookmarks_refresh(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let due = stream::bookmarks::due_for_check(&pool, 24 * 60 * 60).await;
        if due.is_empty() {
            return;
        }
        let Ok(c) = client().await else { return };
        let mut news = false;
        for bm in due.into_iter().take(REFRESH_BATCH) {
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

/// Rows per page of the History list.
const HISTORY_PAGE: usize = 25;
/// How far back the list goes at all.
const HISTORY_MAX: i64 = 500;

/// Fill one page of the History list, newest first.
pub fn stream_history_load(weak: slint::Weak<MainWindow>, page: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let all = stream::progress::history(&pool, HISTORY_MAX).await;
        let pages = all.len().div_ceil(HISTORY_PAGE).max(1);
        // A page that no longer exists (the last entry on it was removed) falls
        // back to the last one that does.
        let page = (page.max(0) as usize).min(pages - 1);
        let entries: Vec<stream::progress::Entry> =
            all.into_iter().skip(page * HISTORY_PAGE).take(HISTORY_PAGE).collect();

        let covers = cache_covers(entries.iter().map(|e| e.cover_url.clone()).collect()).await;
        let rows: Vec<(stream::progress::Entry, Option<PathBuf>)> =
            entries.into_iter().zip(covers).collect();

        let _ = weak.upgrade_in_event_loop(move |w| {
            let list: Vec<StreamHistoryRow> = rows
                .into_iter()
                .map(|(e, poster)| StreamHistoryRow {
                    id: e.subject_id.clone().into(),
                    title: e.title.clone().into(),
                    kind: history_kind(&e).into(),
                    state: history_state(&e).into(),
                    when: relative_time(e.updated).into(),
                    poster: load_image(poster),
                    progress: if e.finished { 1.0 } else { e.fraction() as f32 },
                    finished: e.finished,
                    season: e.season as i32,
                    episode: e.episode as i32,
                })
                .collect();
            w.set_video_stream_history(slint::ModelRc::new(slint::VecModel::from(list)));
            w.set_video_stream_history_page(page as i32);
            w.set_video_stream_history_pages(pages as i32);
        });
    });
}

/// "Series · S02E04" / "Movie".
fn history_kind(e: &stream::progress::Entry) -> String {
    if e.is_series && (e.season > 0 || e.episode > 0) {
        format!("Series  ·  S{:02}E{:02}", e.season.max(1), e.episode.max(1))
    } else if e.is_series {
        "Series".to_string()
    } else {
        "Movie".to_string()
    }
}

/// "Watched" / "34%  ·  48 min left".
fn history_state(e: &stream::progress::Entry) -> String {
    if e.finished {
        return "Watched".to_string();
    }
    let pct = format!("{:.0}%", e.fraction() * 100.0);
    match e.remaining_s() {
        Some(s) if s >= 60.0 => format!("{pct}  ·  {:.0} min left", s / 60.0),
        Some(_) => format!("{pct}  ·  nearly done"),
        None => pct,
    }
}

/// "just now" / "3 hours ago" / "2 days ago".
fn relative_time(then: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let secs = (now - then).max(0);
    let plural = |n: i64, unit: &str| {
        format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
    };
    match secs {
        s if s < 90 => "just now".to_string(),
        s if s < 3600 => plural(s / 60, "minute"),
        s if s < 86_400 => plural(s / 3600, "hour"),
        s if s < 2_592_000 => plural(s / 86_400, "day"),
        s => plural(s / 2_592_000, "month"),
    }
}

/// Open a title straight from History and start it playing where it left off.
///
/// The episode has to travel with the request: opening a title otherwise lands
/// on episode 1, so Play on an "S02E04" row used to start the season opener.
pub fn stream_history_play(
    weak: slint::Weak<MainWindow>,
    subject_id: String,
    season: i32,
    episode: i32,
) {
    set_pending_play(PendingPlay {
        subject_id: subject_id.clone(),
        season: season.max(0) as i64,
        episode: episode.max(0) as i64,
    });
    stream_open(weak, subject_id);
}

/// Forget everything on the History page.
pub fn stream_history_clear(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::progress::clear(&pool).await;
        }
        stream_history_load(weak.clone(), 0);
        stream_feed_load(weak);
    });
}

/// Forget one title's history, staying on the page you were reading.
pub fn stream_history_remove(weak: slint::Weak<MainWindow>, subject_id: String, page: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::progress::remove(&pool, &subject_id).await;
        }
        stream_history_load(weak, page);
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

    fn entry(finished: bool, pos: f64, dur: f64, series: bool) -> stream::progress::Entry {
        stream::progress::Entry {
            subject_id: "s".into(),
            season: 2,
            episode: 4,
            title: "Severance".into(),
            is_series: series,
            position_s: pos,
            duration_s: dur,
            finished,
            ..Default::default()
        }
    }

    #[test]
    fn history_lines_read_as_where_you_got_to() {
        assert_eq!(history_kind(&entry(false, 0.0, 0.0, true)), "Series  ·  S02E04");
        assert_eq!(history_kind(&entry(false, 0.0, 0.0, false)), "Movie");
        assert_eq!(history_state(&entry(true, 990.0, 1000.0, true)), "Watched");
        assert_eq!(history_state(&entry(false, 300.0, 1200.0, true)), "25%  ·  15 min left");
        // An unknown length still gives a percentage of nothing rather than a lie.
        assert_eq!(history_state(&entry(false, 300.0, 0.0, true)), "0%");
    }

    #[test]
    fn relative_time_rounds_to_the_unit_that_reads_best() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        assert_eq!(relative_time(now), "just now");
        assert_eq!(relative_time(now - 600), "10 minutes ago");
        assert_eq!(relative_time(now - 3600), "1 hour ago");
        assert_eq!(relative_time(now - 2 * 86_400), "2 days ago");
        // A clock that went backwards must not print a negative age.
        assert_eq!(relative_time(now + 500), "just now");
    }

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
