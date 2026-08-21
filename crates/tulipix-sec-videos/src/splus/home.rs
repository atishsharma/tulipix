//! Home and Search: the shelves, the query, suggestions and the results grid.

use slint::ComponentHandle;
use tulipix_common::pool_for;
use tulipix_ui::*;
use tulipix_videos::splus::{self, Title};

use super::{PAGE_SIZE, cards, is_current, model, next_epoch, state, strings};

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_splus_home_load(move || {
        let Some(w) = w.upgrade() else { return };
        load_home(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_search(move |q| {
        let Some(w) = w.upgrade() else { return };
        let q = q.to_string();
        {
            let mut st = state();
            st.query = q.clone();
            st.page = 1;
        }
        w.set_splus_view("search".into());
        run_search(w.as_weak(), q, false);
    });

    let w = window.as_weak();
    window.on_splus_load_more(move || {
        let Some(w) = w.upgrade() else { return };
        let (q, page) = {
            let mut st = state();
            st.page += 1;
            (st.query.clone(), st.page)
        };
        let _ = page;
        run_search(w.as_weak(), q, true);
    });

    // Suggestions come from the recent list only. Asking AniList on every
    // keystroke would burn the 30-per-minute budget in one word typed.
    let w = window.as_weak();
    window.on_splus_suggest(move |q| {
        let Some(w) = w.upgrade() else { return };
        let q = q.to_string().to_lowercase();
        slint::spawn_local(async move {
            let Ok(pool) = pool_for("videos").await else { return };
            let recent = splus::library::recent(&pool, 40).await;
            let hits: Vec<String> = recent
                .into_iter()
                .filter(|r| q.len() >= 2 && r.to_lowercase().contains(&q))
                .take(5)
                .collect();
            w.set_splus_suggestions(strings(hits));
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_recent_clear(move || {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::library::clear_recent(&pool).await;
            }
            w.set_splus_recent(strings(Vec::new()));
        })
        .ok();
    });
}

/// Featured, trending and this season, plus whatever is part-watched.
///
/// One epoch for the whole set: re-entering the tab while the first load is
/// still in flight must not paint two overlapping sets of shelves.
pub fn load_home(weak: slint::Weak<MainWindow>) {
    let epoch = next_epoch();
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        w.set_splus_busy(true);
        w.set_splus_status("Loading…".into());

        let pool = pool_for("videos").await.ok();
        let saved_keys: Vec<String> = match &pool {
            Some(p) => splus::library::saved(p, splus::library::Sort::Added)
                .await
                .into_iter()
                .map(|s| s.key)
                .collect(),
            None => Vec::new(),
        };

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        // Civil month without pulling in a date crate: the year length is only
        // needed to pick a season name, so a 30.44-day mean is close enough and
        // wrong for at most a day at a boundary.
        let days = now / 86_400;
        let year = 1970 + (days as f64 / 365.2425) as i64;
        let month = (((days as f64 % 365.2425) / 30.44) as u32 % 12) + 1;
        let season = splus::anilist::season_of(month);

        let (trending, seasonal) = tokio::join!(
            splus::anilist::trending(18),
            splus::anilist::seasonal(season, year, 18),
        );
        if !is_current(epoch) {
            return;
        }

        let trending = trending.unwrap_or_else(|e| {
            tracing::debug!(error = %e, "splus: trending unavailable");
            Vec::new()
        });
        let seasonal = seasonal.unwrap_or_default();
        // The hero is the top of trending — six is enough to feel like a rota
        // and few enough that the covers are one burst.
        let featured: Vec<Title> = trending.iter().take(6).cloned().collect();

        let feat_cards = cards(&featured, &saved_keys).await;
        let trend_cards = cards(&trending, &saved_keys).await;
        let seas_cards = cards(&seasonal, &saved_keys).await;

        let cont = match &pool {
            Some(p) => super::library::history_rows(p, true, 12).await,
            None => Vec::new(),
        };
        let recent = match &pool {
            Some(p) => splus::library::recent(p, 8).await,
            None => Vec::new(),
        };

        if !is_current(epoch) {
            return;
        }
        {
            let mut st = state();
            st.featured = featured;
            st.trending = trending;
            st.seasonal = seasonal;
        }
        w.set_splus_featured(model(feat_cards));
        w.set_splus_featured_idx(0);
        w.set_splus_trending(model(trend_cards));
        w.set_splus_seasonal(model(seas_cards));
        w.set_splus_season_label(format!("{} {year}", pretty(season)).into());
        w.set_splus_continue(model(cont));
        w.set_splus_recent(strings(recent));
        w.set_splus_busy(false);
        w.set_splus_status("".into());
    })
    .ok();
}

fn pretty(season: &str) -> &'static str {
    match season {
        "WINTER" => "Winter",
        "SPRING" => "Spring",
        "SUMMER" => "Summer",
        _ => "Fall",
    }
}

/// Run a search. `append` keeps what is on screen and adds a page to it.
pub fn run_search(weak: slint::Weak<MainWindow>, query: String, append: bool) {
    if query.trim().is_empty() {
        if let Some(w) = weak.upgrade() {
            w.set_splus_results(model(Vec::new()));
            w.set_splus_results_total(0);
            w.set_splus_results_more(false);
        }
        return;
    }
    let epoch = next_epoch();
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        w.set_splus_busy(true);
        w.set_splus_status(format!("Searching for “{query}”…").into());

        let page = state().page.max(1);
        let found = splus::anilist::search(&query, page, PAGE_SIZE).await;
        if !is_current(epoch) {
            return;
        }
        let (mut titles, more) = match found {
            Ok(v) => v,
            Err(e) => {
                w.set_splus_busy(false);
                w.set_splus_status(format!("Search failed — {e}").into());
                return;
            }
        };

        let pool = pool_for("videos").await.ok();
        if let Some(p) = &pool {
            splus::library::note_term(p, &query).await;
        }
        let saved_keys: Vec<String> = match &pool {
            Some(p) => splus::library::saved(p, splus::library::Sort::Added)
                .await
                .into_iter()
                .map(|s| s.key)
                .collect(),
            None => Vec::new(),
        };

        // The age gate applies before anything is drawn, not after: a blocked
        // title must not appear and then vanish.
        titles.retain(|t| super::settings::allowed(t));

        let all: Vec<Title> = if append {
            let mut prev = state().results.clone();
            prev.extend(titles.clone());
            prev
        } else {
            titles.clone()
        };
        let drawn = cards(&all, &saved_keys).await;
        if !is_current(epoch) {
            return;
        }
        {
            let mut st = state();
            st.results = all.clone();
            st.more = more;
        }
        w.set_splus_results(model(drawn));
        w.set_splus_results_total(all.len() as i32);
        w.set_splus_results_more(more);
        w.set_splus_recent(strings(match &pool {
            Some(p) => splus::library::recent(p, 8).await,
            None => Vec::new(),
        }));
        w.set_splus_busy(false);
        w.set_splus_status(if all.is_empty() { "Nothing matched that." } else { "" }.into());
    })
    .ok();
}
