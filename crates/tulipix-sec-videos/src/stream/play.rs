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
        let (mut args, named) = subtitle_args(&tracks, chosen).await;
        args.extend(subtitle_style_args());
        let resume = resume_point().await;
        spawn_mpv_windowed_tracked(
            PathBuf::from(file.url),
            resume,
            None,
            args,
            progress_sink(weak.clone()),
        );
        // The next episode is very likely the next thing wanted: resolve it into
        // the cache now so switching to it is instant.
        prefetch_next();

        let mut msg = match (named, chosen.and_then(|i| tracks.get(i))) {
            (0, _) => format!("Playing {label} in mpv · no subtitles"),
            (n, Some(c)) => format!("Playing {label} in mpv · {} subtitles · {}", n, c.lang),
            (n, None) => format!("Playing {label} in mpv · {n} subtitles · off"),
        };
        if let Some(at) = resume {
            msg.push_str(&format!(" · resumed at {}", fmt_clock(at)));
        }
        set_status(&weak, msg, false);
    });
}

/// Where the open episode was left, or `None` to start from the top.
async fn resume_point() -> Option<f64> {
    let (subject_id, (season, episode)) = with_state(|st| (st.open_id.clone(), st.selection));
    if subject_id.is_empty() {
        return None;
    }
    let pool = pool_for("videos").await.ok()?;
    stream::progress::get(&pool, &subject_id, season, episode).await?.resume_at()
}

/// mpv flags for the subtitle look the user set (size, timing offset).
fn subtitle_style_args() -> Vec<String> {
    let mut out = Vec::new();
    let scale = stream::subs::scale();
    if (scale - 1.0).abs() > f32::EPSILON {
        out.push(format!("--sub-scale={scale:.2}"));
    }
    let delay = stream::subs::delay();
    if delay.abs() > f32::EPSILON {
        out.push(format!("--sub-delay={delay:.1}"));
    }
    out
}

/// Resolve the next episode's streams in the background, so choosing it paints
/// from cache. Silent: no status line, no repaint, and any failure is dropped.
fn prefetch_next() {
    let Some((subject_id, season, episode)) = next_episode() else { return };
    let res = current_resolution();
    tokio::runtime::Handle::current().spawn(async move {
        let key = stream::cache::Key::new(&subject_id, season, episode, &res);
        let Ok(pool) = pool_for("videos").await else { return };
        if stream::cache::load(&pool, &key).await.is_some() {
            return; // already have it
        }
        let Ok(c) = client().await else { return };
        if let Ok(files) = c
            .resources(&subject_id, season.max(0) as usize, episode.max(0) as usize, &res)
            .await
        {
            if !files.is_empty() {
                let _ = stream::cache::store(&pool, &key, &files).await;
            }
        }
    });
}

/// `(subject, season, episode)` of the episode after the open one, when the
/// title is a series and the season has one.
fn next_episode() -> Option<(String, i64, i64)> {
    let (subject_id, (season, episode), details) =
        with_state(|st| (st.open_id.clone(), st.selection, st.details.clone()));
    let d = details?;
    if subject_id.is_empty() || !d.is_series || episode <= 0 {
        return None;
    }
    // The episode count for the season being played; a split show carries one
    // season per subject, so its own first entry is the right one.
    let max_ep = d
        .seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| d.seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0);
    (episode < max_ep).then_some((subject_id, season, episode + 1))
}

/// Snapshot what identifies the episode now playing, and hand back a hook that
/// records where it got to.
///
/// The hook fires on mpv's watcher thread, which is a plain `std::thread` with
/// no tokio context — hence the runtime handle captured here, where there is
/// one. Calling `Handle::current()` inside the hook would panic, and with
/// `panic = "abort"` that closes the app.
fn progress_sink(weak: slint::Weak<MainWindow>) -> Option<PlaybackEnd> {
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
        let finished = stream::progress::is_finished(position_s, duration_s);
        let weak = weak.clone();
        rt.spawn(async move {
            if let Ok(pool) = pool_for("videos").await {
                let _ = stream::progress::record(&pool, &entry).await;
            }
            // Repaint the landing row now rather than on the next tab visit —
            // going back after watching something and finding the old position
            // still on the card reads as the progress not having been kept.
            stream_feed_load(weak.clone());
            // Watched to the end: line up the next episode the way a streaming
            // app would. Only after the write, so its progress bar is correct.
            if finished {
                autoplay_next(weak).await;
            }
        });
    }))
}

/// Move to the next episode and play it, if the user is still on the same title
/// and there is one.
///
/// Anything else on screen — a different title open, or back at the results —
/// means the moment has passed and nothing should start playing.
async fn autoplay_next(weak: slint::Weak<MainWindow>) {
    if !stream::autoplay::enabled() {
        return;
    }
    let Some((subject_id, season, episode)) = next_episode() else { return };
    if opened_subject().as_deref() != Some(subject_id.as_str()) {
        return;
    }
    let epoch = next_epoch();
    with_state(|st| st.selection = (season, episode));
    let weak2 = weak.clone();
    let _ = weak.upgrade_in_event_loop(move |w| w.set_video_stream_episode(episode as i32));
    load_files(weak2.clone(), epoch, subject_id, season, episode).await;
    if !is_current(epoch) {
        return;
    }
    // load_files armed a stream; play whatever it settled on.
    let index = current_index();
    if index >= 0 {
        set_status(&weak2, format!("Playing episode {episode}…"), true);
        stream_play(weak2, index);
    }
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
    // Extension from the content, with the declared one as a hint. mpv decides
    // whether a `--sub-file` is a subtitle partly by its extension, so a track
    // the catalogue labels `"TEXT"` (or labels nothing at all) is silently
    // ignored when that label goes straight into the filename.
    let path = dir.join(format!("{stem}.{}", subtitle_ext(&bytes, &c.ext)));
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// Subtitle formats mpv loads from a `--sub-file`.
const SUB_EXTS: [&str; 5] = ["srt", "vtt", "ass", "ssa", "sub"];

/// Save a caption beside a downloaded video as `<video>.<ext>`, so the file
/// plays with the language that was picked when it was queued. Downloading it at
/// queue time costs a few KB and puts it in place before the video lands.
pub async fn save_sidecar_sub(dest: &std::path::Path, c: &Caption) -> Option<PathBuf> {
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
    if let Some(parent) = dest.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let path = dest.with_extension(subtitle_ext(&bytes, &c.ext));
    tokio::fs::write(&path, &bytes).await.ok()?;
    Some(path)
}

/// mpv flags for a downloaded file's sidecar subtitle, when one was saved with
/// it. Empty when there is none — `--sub-auto` would then be free to find
/// whatever else is in the folder, which is what a plain local file should do.
pub fn sidecar_sub_args(dest: &std::path::Path) -> Vec<String> {
    let Some(path) = SUB_EXTS.iter().map(|e| dest.with_extension(e)).find(|p| p.exists()) else {
        return Vec::new();
    };
    let mut args = vec![
        format!("--sub-file={}", path.display()),
        "--sub-auto=no".to_string(),
        "--sid=1".to_string(),
        "--sub-visibility=yes".to_string(),
    ];
    args.extend(subtitle_style_args());
    args
}

/// What this subtitle actually is. Content wins; the server's own label is used
/// only when it is one of the formats mpv accepts.
fn subtitle_ext(bytes: &[u8], declared: &str) -> String {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(64)]);
    let head = head.trim_start_matches('\u{feff}').trim_start();
    if head.starts_with("WEBVTT") {
        return "vtt".into();
    }
    if head.starts_with("[Script Info]") || head.starts_with("[V4+ Styles]") {
        return "ass".into();
    }
    let declared = declared.trim().trim_start_matches('.').to_ascii_lowercase();
    if SUB_EXTS.contains(&declared.as_str()) {
        return declared;
    }
    // SubRip is the catalogue's usual format and the safest default.
    "srt".into()
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
        if chosen.is_some() {
            // Ours are the only external subtitle files in play: without this mpv
            // also picks up anything sitting next to the media, which for a
            // remote URL is unpredictable and shifts the track numbering.
            args.push("--sub-auto=no".to_string());
            args.push("--sid=1".to_string());
            // A user mpv.conf carrying `sub-visibility=no` (or `no-sub`) leaves
            // the track selected but invisible, which looks exactly like
            // subtitles being broken. Command-line options win over the config.
            args.push("--sub-visibility=yes".to_string());
        } else {
            args.push("--sid=no".to_string());
        }
    }
    (args, attached)
}

// ---- trailer ----

/// Play the title's trailer in the same external mpv the streams use.
///
/// `ytdl://ytsearch1:…` hands the lookup to mpv's youtube-dl hook, so the first
/// result plays directly — no second video pipeline here, and no navigating the
/// user into another section to find it. Needs yt-dlp on PATH, which the
/// YouTube section already requires; without it mpv exits and the status says
/// nothing played.
pub fn stream_trailer(weak: slint::Weak<MainWindow>) {
    let Some(d) = with_state(|st| st.details.clone()) else { return };
    if d.title.is_empty() {
        return;
    }
    let query = [d.title.as_str(), d.year.as_str(), "trailer"]
        .iter()
        .filter(|p| !p.is_empty())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    set_status(&weak, format!("Looking for the {} trailer…", d.title), true);
    tokio::runtime::Handle::current().spawn(async move {
        spawn_mpv_windowed_tracked(
            PathBuf::from(format!("ytdl://ytsearch1:{query}")),
            None,
            None,
            vec!["--ytdl-format=best[height<=1080]".to_string()],
            None,
        );
        set_status(&weak, "Trailer opened in mpv.", false);
    });
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
