//! The Downloads page: enqueueing, the row actions, and the poll that keeps
//! progress on screen while something is running.

use slint::ComponentHandle;
use tulipix_common::pool_for;
use tulipix_ui::*;
use tulipix_videos::splus;

use super::{model, state};

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_splus_download(move || {
        let Some(w) = w.upgrade() else { return };
        let Some((ep, file)) = super::detail::selected() else {
            w.set_splus_status("Nothing resolved to download yet.".into());
            return;
        };
        let sub = {
            let st = state();
            st.subs.get(st.sub).map(|t| t.url.clone()).unwrap_or_default()
        };
        slint::spawn_local(async move {
            let Ok(pool) = pool_for("videos").await else { return };
            match splus::downloads::enqueue(&pool, &ep, &file, &sub, "").await {
                Ok(_) => {
                    splus::downloads::pump(&pool).await;
                    w.set_splus_status(
                        format!("Queued {} {}", ep.title.display_title(), ep.label()).into(),
                    );
                    reload(w.as_weak());
                }
                Err(e) => w.set_splus_status(format!("Could not queue that — {e}").into()),
            }
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_download_season(move || {
        let Some(w) = w.upgrade() else { return };
        let Some(base) = super::current_ref() else { return };
        let episodes: Vec<i64> = state().episodes.iter().map(|e| e.number).collect();
        if episodes.is_empty() {
            w.set_splus_status("No episode list to download.".into());
            return;
        }
        // One batch id so the whole season can be cancelled as one thing.
        let batch = format!("{}:{}", base.key(), tulipix_core::util::unix_secs_i64());
        slint::spawn_local(async move {
            let Ok(pool) = pool_for("videos").await else { return };
            let mut queued = 0usize;
            for n in episodes {
                let ep = splus::EpisodeRef { episode: n, ..base.clone() };
                // Resolving every episode up front would be dozens of requests
                // before a single byte moves; resolve, queue, move on.
                let files = splus::source::resolve_any(&pool, &ep).await.unwrap_or_default();
                let Some(file) = files.get(super::detail::preferred(&files)) else {
                    continue;
                };
                if splus::downloads::enqueue(&pool, &ep, file, "", &batch).await.is_ok() {
                    queued += 1;
                    splus::downloads::pump(&pool).await;
                    w.set_splus_status(format!("Queued {queued} episodes…").into());
                    reload(w.as_weak());
                }
            }
            w.set_splus_status(format!("Queued {queued} episodes.").into());
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_downloads_load(move || {
        if let Some(w) = w.upgrade() {
            reload(w.as_weak());
        }
    });

    let w = window.as_weak();
    window.on_splus_dl_cancel(move |id| {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::downloads::cancel(&pool, i64::from(id)).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_dl_retry(move |id| {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::downloads::retry(&pool, i64::from(id)).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_dl_delete(move |id| {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::downloads::delete_file(&pool, i64::from(id)).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_dl_play(move |id| {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            let Ok(pool) = pool_for("videos").await else { return };
            let Some(row) = splus::downloads::list(&pool).await.into_iter().find(|r| r.id == id as i64)
            else {
                return;
            };
            if row.dest.is_empty() {
                return;
            }
            // A finished download is an ordinary local file — it plays through
            // exactly the path the library uses.
            tulipix_common::spawn_mpv_windowed_tracked(
                std::path::PathBuf::from(&row.dest),
                None,
                None,
                Vec::new(),
                None,
            );
            w.set_splus_status(format!("Playing {}", row.title).into());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_dl_reveal(move |id| {
        let Some(_w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            let Ok(pool) = pool_for("videos").await else { return };
            if let Some(row) =
                splus::downloads::list(&pool).await.into_iter().find(|r| r.id == id as i64)
            {
                let file = std::path::Path::new(&row.dest);
                if let Some(dir) = file.parent() {
                    // Same helper the Stream tab's Reveal uses — one behaviour
                    // per platform, defined once.
                    if let Err(e) = crate::stream::download::reveal_in_file_manager(file, dir) {
                        tracing::warn!(error = %e, "splus: could not reveal the file");
                    }
                }
            }
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_dl_cancel_all(move || {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::downloads::cancel_all(&pool).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_dl_clear(move || {
        let Some(w) = w.upgrade() else { return };
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::downloads::clear_finished(&pool).await;
            }
            reload(w.as_weak());
        })
        .ok();
    });
}

/// Repaint the list and the tab's running count.
pub fn reload(weak: slint::Weak<MainWindow>) {
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Ok(pool) = pool_for("videos").await else { return };
        let rows = splus::downloads::list(&pool).await;
        let active = rows.iter().filter(|r| r.active()).count();
        let running: i64 = rows.iter().filter(|r| r.state == "running").map(|r| r.done_bytes).sum();

        let ui: Vec<SPlusDownload> = rows
            .iter()
            .map(|r| SPlusDownload {
                id: r.id as i32,
                title: [r.title.as_str(), r.label.as_str(), r.quality.as_str()]
                    .into_iter()
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · ")
                    .into(),
                state: pretty_state(&r.state).into(),
                detail: detail_line(r).into(),
                progress: r.progress,
                active: r.active(),
                done: r.done(),
                failed: r.failed(),
            })
            .collect();
        w.set_splus_downloads(model(ui));
        w.set_splus_dl_active(active as i32);
        w.set_splus_dl_rate(
            if running > 0 {
                format!("{} done", human(running))
            } else {
                String::new()
            }
            .into(),
        );
    })
    .ok();
}

fn pretty_state(s: &str) -> &'static str {
    match s {
        "running" => "Downloading",
        "waiting" => "Waiting",
        "done" => "Saved",
        "missing" => "Missing",
        _ => "Failed",
    }
}

fn detail_line(r: &splus::downloads::DownloadRow) -> String {
    if !r.detail.is_empty() && (r.failed() || r.done()) {
        return r.detail.clone();
    }
    if r.total_bytes > 0 {
        return format!("{} / {} · {}", human(r.done_bytes), human(r.total_bytes), r.source);
    }
    if r.done_bytes > 0 {
        return format!("{} · {}", human(r.done_bytes), r.source);
    }
    r.source.clone()
}

fn human(n: i64) -> String {
    splus::source::human_bytes(n)
}
