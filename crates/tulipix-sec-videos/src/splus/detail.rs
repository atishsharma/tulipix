//! The detail view: opening a title, season/episode/audio switching, and
//! resolving the playable files for whatever is selected.

use slint::ComponentHandle;
use tulipix_common::pool_for;
use tulipix_ui::*;
use tulipix_videos::splus::{self, Episode, EpisodeRef};

use super::{current_ref, find, is_current, meta_line, model, next_epoch, state};

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_splus_open_card(move |key| {
        let Some(w) = w.upgrade() else { return };
        let Some(t) = find(&key) else { return };
        {
            let mut st = state();
            st.open = Some(t.clone());
            st.season = 1;
            st.episode = 1;
            st.audio = splus::prefs::audio_pref().order()[0].to_string();
            st.files.clear();
            st.file = 0;
            st.subs.clear();
            st.spans.clear();
        }
        w.set_splus_view("detail".into());
        w.set_splus_d_title(t.display_title().into());
        w.set_splus_d_meta(meta_line(&t).into());
        w.set_splus_d_overview(t.overview.clone().into());
        w.set_splus_d_badge(t.format.clone().into());
        w.set_splus_d_episodes(model(Vec::new()));
        w.set_splus_d_files(model(Vec::new()));
        load_detail(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_back(move || {
        if let Some(w) = w.upgrade() {
            w.set_splus_view("search".into());
        }
    });

    let w = window.as_weak();
    window.on_splus_set_audio(move |a| {
        let Some(w) = w.upgrade() else { return };
        state().audio = a.to_string();
        load_detail(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_set_season(move |s| {
        let Some(w) = w.upgrade() else { return };
        state().season = s.parse::<i64>().unwrap_or(1).max(1);
        load_detail(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_set_episode(move |e| {
        let Some(w) = w.upgrade() else { return };
        state().episode = i64::from(e).max(1);
        w.set_splus_d_episode(e.max(1));
        resolve(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_set_file(move |i| {
        let Some(w) = w.upgrade() else { return };
        let i = i.max(0) as usize;
        // One lock, one decision: a `state()` in the condition would still be
        // held inside the block and deadlock on the next call.
        let picked = {
            let mut st = state();
            (i < st.files.len()).then(|| {
                st.file = i;
                i
            })
        };
        if let Some(i) = picked {
            w.set_splus_d_file(i as i32);
        }
    });

    let w = window.as_weak();
    window.on_splus_set_sub(move |id| {
        let Some(w) = w.upgrade() else { return };
        let i = id.parse::<usize>().unwrap_or(0);
        let picked = {
            let mut st = state();
            (i < st.subs.len()).then(|| {
                st.sub = i;
            })
        };
        if picked.is_some() {
            push_subs(&w);
        }
    });

    let w = window.as_weak();
    window.on_splus_toggle_save(move || {
        let Some(w) = w.upgrade() else { return };
        let Some(t) = state().open.clone() else { return };
        slint::spawn_local(async move {
            let Ok(pool) = pool_for("videos").await else { return };
            match splus::library::toggle_saved(&pool, &t).await {
                Ok(now_saved) => w.set_splus_d_saved(now_saved),
                Err(e) => tracing::warn!(error = %e, "splus: could not save the title"),
            }
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_retry_source(move |_which| {
        let Some(w) = w.upgrade() else { return };
        resolve(w.as_weak());
    });

    let w = window.as_weak();
    window.on_splus_copy_link(move || {
        let Some(w) = w.upgrade() else { return };
        let st = state();
        let Some(f) = st.files.get(st.file) else { return };
        let url = f.url.clone();
        drop(st);
        match arboard::Clipboard::new().and_then(|mut c| c.set_text(url)) {
            Ok(()) => w.set_splus_status("Stream link copied.".into()),
            Err(e) => w.set_splus_status(format!("Could not copy: {e}").into()),
        }
    });
}

/// Episode list for the current title, season and audio, then resolve the
/// episode the user is on.
pub fn load_detail(weak: slint::Weak<MainWindow>) {
    let epoch = next_epoch();
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Some(t) = state().open.clone() else { return };
        let audio = state().audio.clone();

        w.set_splus_d_resolving(true);
        let Ok(pool) = pool_for("videos").await else { return };

        w.set_splus_d_saved(splus::library::is_saved(&pool, &t).await);

        let (mut eps, src) = splus::source::episodes_any(&pool, &t, &audio)
            .await
            .unwrap_or_else(|e| {
                tracing::debug!(error = %e, "splus: no episode list");
                (Vec::new(), "")
            });
        if !is_current(epoch) {
            return;
        }

        // Per-episode progress comes from our own table, not the provider.
        let seen = splus::library::progress_map(&pool, &t).await;
        for e in &mut eps {
            e.progress = seen.get(&e.number).copied().unwrap_or(0.0);
        }
        if splus::prefs::hide_finished() {
            eps.retain(|e| e.progress < 0.98);
        }

        // Resume where they were, not at episode one.
        let start = eps
            .iter()
            .find(|e| e.progress > 0.02 && e.progress < 0.98)
            .or_else(|| eps.iter().find(|e| e.progress <= 0.02))
            .map(|e| e.number)
            .unwrap_or(1);
        {
            let mut st = state();
            st.episodes = eps.clone();
            st.episode = start;
        }
        w.set_splus_d_episode(start as i32);
        push_episodes(&w, &eps).await;
        push_chips(&w, &t, &audio);
        w.set_splus_d_resolved(if src.is_empty() { String::new() } else { format!("via {src}") }.into());
        resolve(weak.clone());
    })
    .ok();
}

async fn push_episodes(w: &MainWindow, eps: &[Episode]) {
    let thumbs = crate::stream::cache_covers(
        eps.iter().map(|e| e.thumb_url.clone()).collect(),
    )
    .await;
    let rows: Vec<SPlusEpisode> = eps
        .iter()
        .zip(thumbs)
        .map(|(e, th)| SPlusEpisode {
            number: e.number as i32,
            title: e.title.clone().into(),
            thumb: crate::stream::load_image(th),
            progress: e.progress,
        })
        .collect();
    w.set_splus_d_episodes(model(rows));
}

fn push_chips(w: &MainWindow, t: &tulipix_videos::splus::Title, audio: &str) {
    // Audio is only meaningful on the anime lane; a TMDB title has one track.
    let audio_chips: Vec<SPlusChip> = if t.anilist_id.is_some() {
        ["sub", "dub"]
            .iter()
            .map(|a| SPlusChip {
                id: (*a).into(),
                label: if *a == "sub" { "Sub".into() } else { "Dub".into() },
                active: *a == audio,
            })
            .collect()
    } else {
        Vec::new()
    };
    w.set_splus_d_audio(model(audio_chips));
    // One season for now: AniList models a second cour as its own Media, so a
    // season switcher here would be lying about what the id points at.
    w.set_splus_d_seasons(model(Vec::new()));
}

/// Ask each source in turn for the current episode, then look for subtitles and
/// skip times alongside.
pub fn resolve(weak: slint::Weak<MainWindow>) {
    let epoch = next_epoch();
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Some(ep) = current_ref() else { return };
        w.set_splus_d_resolving(true);
        w.set_splus_d_files(model(Vec::new()));

        let Ok(pool) = pool_for("videos").await else { return };
        let started = std::time::Instant::now();
        let files = splus::source::resolve_any(&pool, &ep).await.unwrap_or_default();
        if !is_current(epoch) {
            return;
        }

        // Subtitles and skip times are optional garnish — neither failing is a
        // reason to leave the user without a Play button.
        let subs = splus::subs::search(&ep).await;
        let spans = match ep.title.mal_id {
            Some(mal) if splus::prefs::skip_op_ed() => {
                splus::aniskip::spans(mal, ep.episode, 0.0).await.unwrap_or_default()
            }
            _ => Vec::new(),
        };
        if !is_current(epoch) {
            return;
        }

        let pick = preferred(&files);
        {
            let mut st = state();
            st.files = files.clone();
            st.file = pick;
            st.subs = subs.clone();
            st.sub = 0;
            st.spans = spans.clone();
        }

        let rows: Vec<SPlusFile> = files
            .iter()
            .enumerate()
            .map(|(i, f)| SPlusFile {
                index: i as i32,
                label: format!("{} · {}", f.label, f.source).into(),
                sub: f.sub_label().into(),
                uploader: f.source.into(),
                subs: if subs.is_empty() {
                    "No subs".into()
                } else {
                    format!("{} subs", subs.len()).into()
                },
                has_subs: !subs.is_empty(),
                failed: false,
                note: "".into(),
            })
            .collect();
        w.set_splus_d_files(model(rows));
        w.set_splus_d_file(pick as i32);
        w.set_splus_d_resolving(false);
        w.set_splus_d_resolved(
            format!("resolved in {:.1} s · {} files", started.elapsed().as_secs_f64(), files.len())
                .into(),
        );
        w.set_splus_d_skip_note(
            match spans.iter().find(|s| s.kind == "op") {
                Some(s) => format!("AniSkip · OP {} → {}", mmss(s.start), mmss(s.end)),
                None => String::new(),
            }
            .into(),
        );
        push_subs(&w);
    })
    .ok();
}

fn push_subs(w: &MainWindow) {
    let st = state();
    let rows: Vec<SPlusChip> = st
        .subs
        .iter()
        .enumerate()
        .map(|(i, t)| SPlusChip {
            id: i.to_string().into(),
            label: format!("{} · {}", t.label, t.source).into(),
            active: i == st.sub,
        })
        .collect();
    drop(st);
    w.set_splus_d_subs(model(rows));
}

/// The closest file at or below the preferred height, or the best available
/// when everything is below it.
pub fn preferred(files: &[splus::source::Playable]) -> usize {
    let want = splus::prefs::preferred_height();
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| f.height > 0 && f.height <= want)
        .max_by_key(|(_, f)| f.height)
        .map(|(i, _)| i)
        .unwrap_or(0)
}

fn mmss(secs: f64) -> String {
    let s = secs.max(0.0).round() as i64;
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// The `EpisodeRef` the play/download handlers act on.
pub fn selected() -> Option<(EpisodeRef, splus::source::Playable)> {
    let ep = current_ref()?;
    let st = state();
    let file = st.files.get(st.file).cloned()?;
    Some((ep, file))
}
