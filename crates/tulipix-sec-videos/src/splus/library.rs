//! Library & History: the saved grid, the history list, and their actions.

use slint::ComponentHandle;
use sqlx::SqlitePool;
use tulipix_common::pool_for;
use tulipix_ui::*;
use tulipix_videos::splus;

use super::{HISTORY_MAX, model};

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_splus_library_load(move || {
        if let Some(w) = w.upgrade() {
            reload(w.as_weak());
        }
    });

    let w = window.as_weak();
    window.on_splus_set_sort(move |s| {
        let Some(w) = w.upgrade() else { return };
        w.set_splus_sort(s);
        reload(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_saved_remove(move |key| {
        let Some(w) = w.upgrade() else { return };
        let key = key.to_string();
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::library::remove_saved(&pool, &key).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_history_play(move |key| {
        if let Some(w) = w.upgrade() {
            super::play::play_from_history(w.as_weak(), key.to_string());
        }
    });

    let w = window.as_weak();
    window.on_splus_history_remove(move |key| {
        let Some(w) = w.upgrade() else { return };
        let key = key.to_string();
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::library::remove_history(&pool, &key).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_history_clear(move || {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::library::clear_history(&pool).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });
}

/// Repaint both lists. Cheap enough to call after any action rather than
/// surgically patching one row — the whole library is a few dozen rows.
pub fn reload(weak: slint::Weak<MainWindow>) {
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Ok(pool) = pool_for("videos").await else { return };

        let sort = splus::library::Sort::parse(&w.get_splus_sort());
        let saved = splus::library::saved(&pool, sort).await;
        let covers =
            crate::stream::cache_covers(saved.iter().map(|s| s.cover_url.clone()).collect()).await;
        let cards: Vec<SPlusCard> = saved
            .iter()
            .zip(covers)
            .map(|(s, cover)| SPlusCard {
                key: s.key.clone().into(),
                title: s.display().into(),
                note: saved_note(s).into(),
                poster: crate::stream::load_image(cover),
                badge: s.format.clone().into(),
                progress: s.progress,
                saved: true,
                blocked: false,
            })
            .collect();
        w.set_splus_saved(model(cards));

        let rows = history_rows(&pool, false, HISTORY_MAX).await;
        w.set_splus_history(model(rows));

        // Home's Continue row reads the same table, so it goes stale otherwise.
        let cont = history_rows(&pool, true, 12).await;
        w.set_splus_continue(model(cont));
    })
    .ok();
}

fn saved_note(s: &splus::library::Saved) -> String {
    if s.last_episode > 0 {
        let pct = (s.progress * 100.0).round() as i64;
        if s.format == "MOVIE" {
            return format!("{pct}%");
        }
        return format!("S{:02}E{:02} · {pct}%", s.last_season, s.last_episode);
    }
    match s.year {
        Some(y) if s.episodes > 0 => format!("{y} · {} ep", s.episodes),
        Some(y) => y.to_string(),
        None => "not started".into(),
    }
}

/// History rows for the UI. `only_unfinished` gives the Continue shelf.
pub async fn history_rows(
    pool: &SqlitePool,
    only_unfinished: bool,
    limit: i64,
) -> Vec<SPlusHistory> {
    let rows = if only_unfinished {
        splus::library::continue_watching(pool, limit).await
    } else {
        splus::library::history(pool, limit).await
    };
    let covers =
        crate::stream::cache_covers(rows.iter().map(|h| h.cover_url.clone()).collect()).await;
    rows.iter()
        .zip(covers)
        .map(|(h, cover)| SPlusHistory {
            key: h.key.clone().into(),
            title: format!("{} {}", h.title, h.kind.rsplit(" · ").next().unwrap_or(""))
                .trim()
                .to_string()
                .into(),
            kind: h.kind.clone().into(),
            state: h.state.clone().into(),
            when: h.when.clone().into(),
            detail: detail_line(h).into(),
            poster: crate::stream::load_image(cover),
            progress: h.progress,
            finished: h.finished,
        })
        .collect()
}

/// "AllManga · dub · 1080p" — whatever of it is known.
fn detail_line(h: &splus::library::HistoryRow) -> String {
    [h.source.as_str(), h.audio.as_str(), h.quality.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}
