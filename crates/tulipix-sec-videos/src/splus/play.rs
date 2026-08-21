//! Playback: handing a resolved stream to the same windowed mpv the rest of the
//! app uses, and writing progress back when it exits.
//!
//! There is no second player here. `spawn_mpv_windowed_tracked` is what the
//! local library, the Stream tab and Live TV all call; Stream Plus differs only
//! in the extra arguments it passes (referer headers, a subtitle URL, an
//! AniSkip `--start`) and in which table the progress lands in.

use slint::ComponentHandle;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use tulipix_common::{PlaybackEnd, pool_for, spawn_mpv_windowed_tracked};
use tulipix_ui::*;
use tulipix_videos::splus::{self, EpisodeRef, source::Playable};

use super::state;

/// Bumped by every play. Switching episodes kills the running mpv, so the old
/// process's exit hook fires *after* the new one started — without this it would
/// write the previous episode's position over the new one's.
static PLAY_GEN: AtomicU64 = AtomicU64::new(0);

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_splus_play(move || {
        let Some(w) = w.upgrade() else { return };
        let Some((ep, file)) = super::detail::selected() else {
            w.set_splus_status("Nothing resolved to play yet.".into());
            return;
        };
        play(w.as_weak(), ep, file);
    });

    let w = window.as_weak();
    window.on_splus_cast(move || {
        let Some(w) = w.upgrade() else { return };
        // A resolved stream is already a public URL, so casting is the Stream
        // tab's problem exactly as it stands — no local media server needed.
        let Some((_, file)) = super::detail::selected() else { return };
        w.set_splus_status(format!("Casting is set up in Stream Setting · {}", file.source).into());
    });

    let w = window.as_weak();
    window.on_splus_add_to_library(move || {
        let Some(w) = w.upgrade() else { return };
        w.set_splus_status(
            "Downloads land in the library when “Download into the library” is on.".into(),
        );
    });
}

pub fn play(weak: slint::Weak<MainWindow>, ep: EpisodeRef, file: Playable) {
    let epoch = PLAY_GEN.fetch_add(1, Ordering::SeqCst) + 1;
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Ok(pool) = pool_for("videos").await else { return };

        let resume = splus::library::resume_at(&pool, &ep).await;
        let mut args: Vec<String> = Vec::new();

        // Some hosts only serve with the referer they handed the URL out under.
        for (k, v) in &file.headers {
            args.push(format!("--http-header-fields={k}: {v}"));
        }
        args.push(format!("--user-agent={}", tulipix_core::net::BROWSER_UA));

        // The picked subtitle rides along as a URL — mpv fetches it itself, so
        // nothing has to be written to disk to watch with subtitles.
        {
            let st = state();
            if let Some(t) = st.subs.get(st.sub) {
                args.push(format!("--sub-file={}", t.url));
            }
        }

        // Skip the opening, but never yank someone backwards into it.
        let spans = state().spans.clone();
        args.extend(splus::aniskip::mpv_args(&spans, resume));
        args.push(format!("--force-media-title={} {}", ep.title.display_title(), ep.label()));

        w.set_splus_status(
            format!("Opening {} {} in mpv…", ep.title.display_title(), ep.label()).into(),
        );

        let ep2 = ep.clone();
        let source = file.source.to_string();
        let quality = file.label.clone();
        let weak2 = weak.clone();
        let on_end: PlaybackEnd = Arc::new(move |pos, dur| {
            // Only the current episode's exit is allowed to write progress.
            if PLAY_GEN.load(Ordering::SeqCst) != epoch {
                return;
            }
            let ep3 = ep2.clone();
            let source = source.clone();
            let quality = quality.clone();
            let weak3 = weak2.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("videos").await else { return };
                if let Err(e) =
                    splus::library::note_progress(&pool, &ep3, &source, &quality, pos, dur).await
                {
                    tracing::warn!(error = %e, "splus: progress not recorded");
                }
                // Refresh whatever the user is looking at now.
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = weak3.upgrade() {
                        super::library::reload(w.as_weak());
                    }
                });
            });
        });

        // mpv takes a URL exactly where it takes a path.
        spawn_mpv_windowed_tracked(
            PathBuf::from(&file.url),
            resume,
            None,
            args,
            Some(on_end),
        );
    })
    .ok();
}

/// Play whatever a history row points at, resolving it again first — the URL a
/// provider handed out last week has almost certainly expired.
pub fn play_from_history(weak: slint::Weak<MainWindow>, key: String) {
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Ok(pool) = pool_for("videos").await else { return };
        let Some(row) = splus::library::history(&pool, super::HISTORY_MAX)
            .await
            .into_iter()
            .find(|h| h.key == key)
        else {
            return;
        };
        // Reopening through the detail view is the honest path: it re-resolves,
        // re-fetches subtitles and re-reads skip times, all of which a stale
        // history row cannot carry.
        let title = match row.title_key.strip_prefix("al:").and_then(|s| s.parse::<i64>().ok()) {
            Some(id) => splus::anilist::by_id(id).await.ok().flatten(),
            None => None,
        };
        let Some(title) = title else {
            w.set_splus_status("That title could not be reopened.".into());
            return;
        };
        {
            let mut st = state();
            st.open = Some(title.clone());
            st.season = row.season;
            st.episode = row.episode.max(1);
            st.audio = if row.audio.is_empty() { "sub".into() } else { row.audio.clone() };
        }
        w.set_splus_view("detail".into());
        w.set_splus_d_title(title.display_title().into());
        w.set_splus_d_meta(super::meta_line(&title).into());
        w.set_splus_d_overview(title.overview.clone().into());
        super::detail::load_detail(w.as_weak());
    })
    .ok();
}
