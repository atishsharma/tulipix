//! Playback: handing a resolved stream to mpv, subtitle attachment, and the
//! progress written back when playback ends.

use super::*;

/// Hand the chosen file to the windowed mpv, with the episode's subtitle tracks
/// attached. Where playback got to is written to `stream_progress` on exit — a
/// remote title has no `items` row for the local `watch_progress` to key off.
///
/// Tracks are downloaded to files named after their language rather than handed
/// to mpv as bare URLs, because mpv labels a track with its filename: a raw URL
/// shows up in the track menu as an unreadable string. The language picked in
/// the detail pane is preselected; the rest stay available in mpv's own menu.
pub fn stream_play(weak: slint::Weak<MainWindow>, index: i32) {
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    // This launch is the user's answer to "what quality?" — every later episode
    // arms the same rung.
    stream::quality::remember(file.resolution);
    let label = if file.resolution > 0 { format!("{}p", file.resolution) } else { "stream".into() };
    set_status(&weak, format!("Starting {label}…"), true);

    tokio::runtime::Handle::current().spawn(async move {
        let (tracks, chosen) = with_state(|st| (st.subs.clone(), st.sub_choice));
        let (args, named) = subtitle_args(&tracks, chosen).await;
        spawn_mpv_windowed_tracked(PathBuf::from(file.url), None, None, args, progress_sink());

        let msg = match (named, chosen.and_then(|i| tracks.get(i))) {
            (0, _) => format!("Playing {label} in mpv · no subtitles"),
            (n, Some(c)) => format!("Playing {label} in mpv · {} subtitles · {}", n, c.lang),
            (n, None) => format!("Playing {label} in mpv · {n} subtitles · off"),
        };
        set_status(&weak, msg, false);
    });
}

/// Snapshot what identifies the episode now playing, and hand back a hook that
/// records where it got to.
///
/// The hook fires on mpv's watcher thread, which is a plain `std::thread` with
/// no tokio context — hence the runtime handle captured here, where there is
/// one. Calling `Handle::current()` inside the hook would panic, and with
/// `panic = "abort"` that closes the app.
fn progress_sink() -> Option<PlaybackEnd> {
    let (subject_id, (season, episode)) = with_state(|st| (st.open_id.clone(), st.selection));
    if subject_id.is_empty() {
        return None;
    }
    let (title, cover_url, is_series) = with_state(|st| {
        st.details
            .as_ref()
            .map(|d| (d.title.clone(), d.cover.clone(), d.is_series))
            .unwrap_or_default()
    });
    let rt = tokio::runtime::Handle::current();

    Some(std::sync::Arc::new(move |position_s: f64, duration_s: f64| {
        // mpv never reported a position: the stream failed to open, and a
        // zero-progress history row would be noise.
        if position_s <= 1.0 {
            return;
        }
        let entry = stream::progress::Entry {
            subject_id: subject_id.clone(),
            season,
            episode,
            title: title.clone(),
            cover_url: cover_url.clone(),
            is_series,
            position_s,
            duration_s,
            ..Default::default()
        };
        rt.spawn(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = stream::progress::record(&pool, &entry).await;
            }
        });
    }))
}

/// Cache one subtitle track under a language-derived filename. Returns the local
/// path, or `None` if it could not be fetched.
async fn cache_subtitle(c: &Caption, idx: usize) -> Option<PathBuf> {
    let dir = tulipix_core::paths::cache_dir()?.join("videos").join("stream-subs");
    tokio::fs::create_dir_all(&dir).await.ok()?;

    // mpv shows the file stem as the track name, so the stem *is* the label.
    let lang: String = c
        .lang
        .chars()
        .map(|ch| if ch.is_alphanumeric() || ch == '-' || ch == ' ' { ch } else { '_' })
        .collect();
    let stem = if lang.trim().is_empty() { format!("Track {}", idx + 1) } else { lang.trim().to_string() };
    let ext = if c.ext.is_empty() { "srt".to_string() } else { c.ext.clone() };
    let path = dir.join(format!("{stem}.{ext}"));

    let bytes = reqwest::Client::new()
        .get(&c.url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?;
    if bytes.is_empty() {
        return None;
    }
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// mpv flags for the episode's subtitles: the chosen language first (so it is
/// track 1 and can be preselected with `--sid=1`), then the rest.
///
/// Returns `(args, tracks_attached)`. Any track that fails to download is simply
/// left out — subtitles must never be the reason a video does not start.
async fn subtitle_args(tracks: &[Caption], chosen: Option<usize>) -> (Vec<String>, usize) {
    if tracks.is_empty() {
        return (Vec::new(), 0);
    }
    // Chosen track first, remaining in their original order.
    let mut order: Vec<usize> = chosen.into_iter().filter(|i| *i < tracks.len()).collect();
    order.extend((0..tracks.len()).filter(|i| Some(*i) != chosen));

    let mut args = Vec::new();
    let mut attached = 0usize;
    for i in order {
        if let Some(path) = cache_subtitle(&tracks[i], i).await {
            args.push(format!("--sub-file={}", path.display()));
            attached += 1;
        }
    }
    if attached > 0 {
        // Track 1 is whatever we put first: the picked language, or nothing
        // picked means subtitles stay off until the user turns them on in mpv.
        args.push(if chosen.is_some() { "--sid=1".to_string() } else { "--sid=no".to_string() });
    }
    (args, attached)
}

// ---- copy link ----

/// Put the chosen stream's URL on the system clipboard (the TUI's `[C]`).
pub fn stream_copy_link(weak: slint::Weak<MainWindow>, index: i32) {
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    match arboard::Clipboard::new().and_then(|mut c| c.set_text(file.url)) {
        Ok(()) => set_status(&weak, "Stream link copied.", false),
        Err(e) => set_status(&weak, format!("Could not copy: {e}"), false),
    }
}

// ---- info-box actions ----

/// Info-box Play — plays the currently selected stream.
pub fn stream_play_current(weak: slint::Weak<MainWindow>) {
    let i = current_index();
    if i >= 0 {
        stream_play(weak, i);
    }
}

pub fn stream_copy_current(weak: slint::Weak<MainWindow>) {
    let i = current_index();
    if i >= 0 {
        stream_copy_link(weak, i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subtitle_args_are_empty_without_tracks() {
        let (args, n) = subtitle_args(&[], Some(0)).await;
        assert!(args.is_empty());
        assert_eq!(n, 0);
    }
}
