//! Settings: the source order and its health, every preference on the page, and
//! the Servers modal.
//!
//! The age gate lives here too, because it is the one setting that changes what
//! the *other* surfaces are allowed to show. It reuses `tulipix_core::parental`
//! rather than inventing a second rating model — the app already knows what a
//! certification means.

use slint::ComponentHandle;
use tulipix_common::pool_for;
use tulipix_ui::*;
use tulipix_videos::splus::{self, Title};

use super::model;

pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_splus_settings_load(move || {
        if let Some(w) = w.upgrade() {
            push_prefs(&w);
            reload_health(w.as_weak(), false);
        }
    });

    let w = window.as_weak();
    window.on_splus_set_source_enabled(move |id, on| {
        let Some(w) = w.upgrade() else { return };
        splus::prefs::set_source_enabled(&id, on);
        reload_health(w.as_weak(), false);
    });

    let w = window.as_weak();
    window.on_splus_move_source(move |id, delta| {
        let Some(w) = w.upgrade() else { return };
        let mut order: Vec<String> = current_order();
        let Some(at) = order.iter().position(|s| *s == id.as_str()) else { return };
        let to = (at as i32 + delta).clamp(0, order.len() as i32 - 1) as usize;
        let moved = order.remove(at);
        order.insert(to, moved);
        splus::prefs::set_source_order(&order);
        reload_health(w.as_weak(), false);
    });

    let w = window.as_weak();
    window.on_splus_source_test(move || {
        let Some(w) = w.upgrade() else { return };
        reload_health(w.as_weak(), true);
    });

    let w = window.as_weak();
    window.on_splus_source_reset(move || {
        let Some(w) = w.upgrade() else { return };
        splus::prefs::set_source_order(&[]);
        slint::spawn_local(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = splus::health::reset(&pool).await;
            }
            reload_health(w.as_weak(), false);
        })
        .ok();
    });

    let w = window.as_weak();
    window.on_splus_set_flag(move |key, on| {
        let Some(w) = w.upgrade() else { return };
        match key.as_str() {
            "skip_op_ed" => splus::prefs::set_skip_op_ed(on),
            "hide_finished" => splus::prefs::set_hide_finished(on),
            "subs_wyzie" => splus::prefs::set_subs_wyzie(on),
            "subs_subdl" => splus::prefs::set_subs_subdl(on),
            "subs_with_file" => splus::prefs::set_subs_with_file(on),
            "into_library" => splus::prefs::set_into_library(on),
            "notify_download" => splus::prefs::set_notify_download_done(on),
            "notify_episode" => splus::prefs::set_notify_new_episode(on),
            other => tracing::warn!(key = other, "splus: unknown flag"),
        }
        push_prefs(&w);
    });

    let w = window.as_weak();
    window.on_splus_set_text(move |key, value| {
        let Some(w) = w.upgrade() else { return };
        match key.as_str() {
            "audio_pref" => splus::prefs::set_audio_pref(&value),
            "subs_lang" => splus::prefs::set_subs_lang(&value),
            other => tracing::warn!(key = other, "splus: unknown setting"),
        }
        push_prefs(&w);
    });

    let w = window.as_weak();
    window.on_splus_set_num(move |key, value| {
        let Some(w) = w.upgrade() else { return };
        let v = i64::from(value);
        match key.as_str() {
            "autoplay_seconds" => splus::prefs::set_autoplay_seconds(v),
            "watched_threshold" => splus::prefs::set_watched_threshold(v),
            "quality" => splus::prefs::set_preferred_height(v as i32),
            "slots" => splus::prefs::set_download_slots(v),
            other => tracing::warn!(key = other, "splus: unknown setting"),
        }
        push_prefs(&w);
    });

    // `on_splus_pick_download_dir` is wired in the app crate, where the file
    // dialog already lives — see main.rs. It calls back into `set_dir` below.

    // ── Servers modal ──
    let w = window.as_weak();
    window.on_splus_servers_opened(move || {
        let Some(w) = w.upgrade() else { return };
        let text = splus::prefs::extra_hosts().join("\n");
        w.set_splus_hosts_text(
            if text.is_empty() { default_host_block() } else { text }.into(),
        );
        w.set_splus_hosts_saved(false);
        reload_health(w.as_weak(), false);
    });

    let w = window.as_weak();
    window.on_splus_hosts_save(move |text| {
        let Some(w) = w.upgrade() else { return };
        splus::prefs::set_extra_hosts(&text);
        w.set_splus_hosts_saved(true);
        reload_health(w.as_weak(), false);
    });

    let w = window.as_weak();
    window.on_splus_hosts_reset(move || {
        let Some(w) = w.upgrade() else { return };
        splus::prefs::set_extra_hosts("");
        w.set_splus_hosts_text(default_host_block().into());
        w.set_splus_hosts_saved(false);
    });

    let w = window.as_weak();
    window.on_splus_hosts_check(move || {
        let Some(w) = w.upgrade() else { return };
        reload_health(w.as_weak(), true);
    });
}

/// The order the resolver would use right now, so the arrows move things
/// relative to what the user is looking at.
fn current_order() -> Vec<String> {
    let saved = splus::prefs::source_order();
    let all: Vec<String> = splus::source::all().iter().map(|s| s.id().to_string()).collect();
    let mut out: Vec<String> = saved.into_iter().filter(|s| all.contains(s)).collect();
    for id in all {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// Repaint the health list. `test` actually pings every source first, which
/// takes as long as the slowest timeout — hence the flag.
pub fn reload_health(weak: slint::Weak<MainWindow>, test: bool) {
    slint::spawn_local(async move {
        let Some(w) = weak.upgrade() else { return };
        let Ok(pool) = pool_for("videos").await else { return };

        if test {
            w.set_splus_hosts_checking(true);
            for src in splus::source::all() {
                match src.ping().await {
                    Ok(ms) => splus::health::note_ok(&pool, src.id(), ms).await,
                    Err(e) => {
                        splus::health::note_fail(&pool, src.id(), &splus::source::short_error(&e))
                            .await
                    }
                }
            }
            w.set_splus_hosts_checking(false);
        }

        let order = current_order();
        let mut rows = splus::health::list(&pool).await;
        rows.sort_by_key(|h| order.iter().position(|o| *o == h.source).unwrap_or(usize::MAX));
        let ui: Vec<SPlusHealth> = rows
            .iter()
            .map(|h| SPlusHealth {
                source: h.source.clone().into(),
                label: h.label.clone().into(),
                note: h.note.clone().into(),
                ok: h.ok,
                enabled: h.enabled,
                sunk: h.sunk,
            })
            .collect();
        w.set_splus_health(model(ui));
    })
    .ok();
}

fn push_prefs(w: &MainWindow) {
    // The chip compares against the stored value, so this has to be the stored
    // spelling, not a prettified one.
    w.set_splus_audio_pref(
        match splus::prefs::audio_pref() {
            splus::AudioPref::DubThenSub => "dub_then_sub",
            splus::AudioPref::SubThenDub => "sub_then_dub",
        }
        .into(),
    );
    w.set_splus_skip_op_ed(splus::prefs::skip_op_ed());
    w.set_splus_watched_pct((splus::prefs::watched_threshold() * 100.0).round() as i32);
    w.set_splus_autoplay_secs(splus::prefs::autoplay_seconds() as i32);
    w.set_splus_hide_finished(splus::prefs::hide_finished());
    w.set_splus_subs_wyzie(splus::prefs::subs_wyzie());
    w.set_splus_subs_subdl(splus::prefs::subs_subdl());
    w.set_splus_subs_lang(splus::prefs::subs_lang().into());
    w.set_splus_subs_with_file(splus::prefs::subs_with_file());
    w.set_splus_quality(splus::prefs::preferred_height());
    w.set_splus_into_library(splus::prefs::into_library());
    w.set_splus_slots(splus::prefs::download_slots() as i32);
    w.set_splus_notify_download(splus::prefs::notify_download_done());
    w.set_splus_notify_episode(splus::prefs::notify_new_episode());
    w.set_splus_download_dir(splus::prefs::download_dir().to_string_lossy().to_string().into());
}

fn default_host_block() -> String {
    let mut lines: Vec<String> = vec!["https://api.allanime.day/api".into()];
    for h in splus::vidsrc::hosts() {
        lines.push(h.origin.to_string());
    }
    lines.join("\n")
}

/// Called by the app crate once the folder dialog returns.
pub fn set_dir(w: &MainWindow, dir: &str) {
    splus::prefs::set_download_dir(dir);
    push_prefs(w);
}

/// The age gate.
///
/// The limit is compared on `parental::Rating`'s severity ladder — the app
/// already knows that TV-14 sits above PG — rather than on a second rating model
/// invented here. An unrated title is hidden when a limit is set, because "we do
/// not know" is not a reason to show it to a child; the one exception is the
/// anime lane, where AniList publishes no board certification at all and the
/// `isAdult` flag is the only signal there is.
pub fn allowed(t: &Title) -> bool {
    use tulipix_core::parental::Rating;

    if t.adult && !splus::prefs::allow_adult() {
        return false;
    }
    let limit = splus::prefs::age_max();
    let Some(cap) = Rating::parse(&limit).filter(|_| !limit.trim().is_empty()) else {
        return true;
    };
    match Rating::parse(&t.certification) {
        Some(r) if !t.certification.trim().is_empty() => r.severity() <= cap.severity(),
        _ => t.anilist_id.is_some(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rated(cert: &str) -> Title {
        Title { certification: cert.into(), tmdb_id: Some(1), ..Title::default() }
    }

    #[test]
    fn an_adult_title_is_gated_on_its_own_flag() {
        let t = Title { adult: true, anilist_id: Some(1), ..Title::default() };
        // Default is not to allow adult content, whatever the rating limit is.
        assert!(!allowed(&t));
    }

    #[test]
    fn a_rated_title_is_compared_on_severity() {
        // With no limit configured everything passes; this documents the
        // comparison itself rather than the stored setting.
        use tulipix_core::parental::Rating;
        assert!(Rating::parse("PG-13").unwrap().severity() <= Rating::parse("R").unwrap().severity());
        assert!(Rating::parse("NC-17").unwrap().severity() > Rating::parse("R").unwrap().severity());
        assert!(allowed(&rated("PG-13")));
    }
}
