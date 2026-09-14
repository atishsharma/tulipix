//! The video transport: what to play, and the settings it plays under.
//!
//! Nothing here spawns a process any more. mpv used to draw its own top-level
//! window, and 200 lines of this file were the cost of that choice — a JSON IPC
//! socket to read the clock back through, `PR_SET_PDEATHSIG` so a crashed app
//! did not leave an orphan film playing, and a `kill -9` to close it. Flutter
//! plays the video itself now, through `media_kit`, which is libmpv in-process:
//! the picture is a texture inside the window and the overlays are widgets over
//! it, rather than a foreign window that cannot be composited with.
//!
//! What did not move is the policy, and that is the point of the split. The
//! Settings keys, the subtitle style, the resume seek, the network-seek flags
//! and the writeback into `watch_progress` are all still decided here — the
//! same values the Slint build used, in the same order. Dart receives them as
//! `k=v` mpv properties and sets them on its player; it never decides one.
//!
//! There was an embedded player once, libmpv rendering into a Slint texture. It
//! froze the whole UI on weak integrated GPUs and was deleted in v0.8.0. This is
//! not that: the freeze was Slint's software compositor doing the upload on the
//! UI thread, and media_kit hands the frame to the Flutter engine as a texture
//! the GPU already owns.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use crate::api::videos::{emit, VideosEvent};

/// Called once, when playback ends, with `(position_s, duration_s)`.
///
/// Both are zero when Dart never reported them — a stream that failed to open.
pub type PlaybackEnd = Arc<dyn Fn(f64, f64) + Send + Sync + 'static>;

/// Bumped by every launch and every stop. Dart echoes it back as the token on
/// its reports, and a report whose token is not the current one is dropped —
/// switching channels replaces the player, so the old source's final position
/// can arrive after the new one has started.
static GENERATION: AtomicU32 = AtomicU32::new(0);

/// The hooks belonging to whatever is on screen. One at a time, because one
/// video is on screen at a time.
struct Pending {
    generation: u32,
    /// Local-library row to write the position back into, if it has one.
    item_id: Option<i64>,
    on_end: Option<PlaybackEnd>,
    /// Live TV only — see [`watch_live`].
    on_playing: Option<Box<dyn Fn() + Send + 'static>>,
}

fn pending() -> &'static Mutex<Option<Pending>> {
    static P: OnceLock<Mutex<Option<Pending>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// Close the player, if one is up.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = pending().lock() {
        *g = None;
    }
    emit(VideosEvent::VideoStop);
}

/// GLSL upscale chain: every `.glsl` in `<config>/shaders`, sorted, joined for
/// mpv's list option. `None` when the folder is empty or absent.
fn shader_chain() -> Option<String> {
    let dir = tulipix_core::paths::config_dir()?.join("shaders");
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("glsl"))
        .filter_map(|p| p.to_str().map(str::to_string))
        .collect();
    if files.is_empty() {
        return None;
    }
    files.sort();
    Some(files.join(":"))
}

/// mpv flags that make a remote video-on-demand stream properly seekable.
///
/// Without these, seeking only works forward and only as far as the cache
/// already reaches: a host that answers a Range request with `200` instead of
/// `206` makes mpv mark the stream unseekable, and the default back-buffer is
/// small enough that stepping backwards forces a refetch. `--hr-seek=yes` is
/// what makes a chapter jump land on the chapter mark.
///
/// Deliberately not applied to Live TV: a live stream has nothing behind the
/// playhead worth keeping, and a 128 MiB back-buffer on a channel left running
/// is real memory for no benefit.
///
/// Still spelled as CLI flags rather than bare properties: every caller builds
/// one flat `Vec<String>` of these, `--sub-file=`s and style options together,
/// and [`props`] is the single place that turns the lot into properties.
pub fn network_seek_args() -> Vec<String> {
    [
        "--force-seekable=yes",
        "--cache=yes",
        "--demuxer-max-bytes=128MiB",
        "--demuxer-max-back-bytes=128MiB",
        "--hr-seek=yes",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Settings → Playback, as mpv options.
///
/// The Slint build pushed these into libmpv at runtime and the process build
/// passed them at spawn; both are the same list. They apply per-open now, which
/// is the one behaviour that improved for free — a Settings change takes effect
/// on the next video rather than the next app run.
fn settings_args() -> Vec<String> {
    let mut out = Vec::new();
    let s = tulipix_core::settings::Settings::load().unwrap_or_default();
    if s.flag("playback.upscale", false) {
        if let Some(chain) = shader_chain() {
            out.push(format!("--glsl-shaders={chain}"));
        }
    }
    let mut style = tulipix_videos::sub_styling::SubtitleStyle::default();
    if let Some(v) =
        s.advanced.get("playback.sub-size").and_then(|x| x.trim().parse::<f32>().ok())
    {
        style.font_size_px = v;
    }
    if let Some(c) =
        s.advanced.get("playback.sub-color").map(|c| c.trim()).filter(|c| !c.is_empty())
    {
        style.color = c.to_string();
    }
    style.clamp();
    for (k, v) in style.to_mpv_options() {
        out.push(format!("--{k}={v}"));
    }
    if s.flag("playback.interpolation", false) {
        out.push("--interpolation=yes".into());
        out.push("--video-sync=display-resample".into());
    }
    if s.flag("playback.audio-exclusive", false) {
        out.push("--audio-exclusive=yes".into());
    }
    out
}

/// `--k=v` → `k=v`, dropping anything that is not an option assignment.
///
/// The one arg that is not a property is `--sub-file`, which repeats: mpv takes
/// each as an append to a list. It survives as repeated `sub-file=` entries and
/// Dart loads them in order — see the note on `VideosEvent::VideoPlay`.
fn props(args: Vec<String>) -> Vec<String> {
    args.into_iter()
        .filter_map(|a| {
            let a = a.strip_prefix("--")?;
            a.contains('=').then(|| a.to_string())
        })
        .collect()
}

/// Open `src` — a path or a URL — in the in-app player.
///
/// `resume` seeks in on open. `item_id` writes progress back into the local
/// library's `watch_progress`; `on_end` is the escape hatch for media that has
/// no `items` row, which is every remote title. Both may be supplied; neither
/// has to be.
///
/// Returns immediately. Nothing here waits for the film: Dart reports back
/// through [`started`] and [`ended`].
pub fn play(
    src: String,
    resume: Option<f64>,
    item_id: Option<i64>,
    extra_args: Vec<String>,
    on_end: Option<PlaybackEnd>,
) {
    // One video at a time, and never over the music: opening a video replaces
    // whatever was on, which is what the Slint build's `kill_music_proc` +
    // `stop_video` pair does at this point.
    crate::mpv::stop();
    stop();
    let generation = GENERATION.load(Ordering::SeqCst);
    if let Ok(mut g) = pending().lock() {
        *g = Some(Pending { generation, item_id, on_end, on_playing: None });
    }
    let mut options = settings_args();
    options.extend(extra_args);
    emit(VideosEvent::VideoPlay {
        token: generation as i64,
        src,
        start_at: resume.filter(|r| *r > 1.0).unwrap_or(0.0),
        props: props(options),
    });
}

/// Follow the video that was just opened, for the thing a live stream needs:
/// the status line says "Opening …" from the moment the button is pressed, and
/// a live stream can take seconds to negotiate, so it is only promoted to
/// "Playing …" once the player reports a running clock.
///
/// The other half of the old live watcher — seeking to the live edge whenever
/// the audio track changes, because a new track starts downloading from the
/// edge and leaves the picture behind — is Dart's now: it owns the track
/// switch, so it is the only side that knows one happened.
///
/// `on_playing` runs at most once. Call it after [`play`].
pub fn watch_live(on_playing: impl Fn() + Send + 'static) {
    if let Ok(mut g) = pending().lock() {
        if let Some(p) = g.as_mut() {
            p.on_playing = Some(Box::new(on_playing));
        }
    }
}

/// Dart saw a running clock. Live TV's promotion out of "Opening …".
pub fn started(token: i64) {
    let hook = pending().lock().ok().and_then(|mut g| {
        let p = g.as_mut()?;
        (p.generation as i64 == token).then(|| p.on_playing.take())?
    });
    if let Some(f) = hook {
        f();
    }
}

/// Dart stopped playing, at `pos` of `dur`. Where the old build ran this on the
/// far side of `Child::wait`, this is the close of the player page.
///
/// A stale token means a newer source owns the player and this report belongs
/// to the one it replaced — writing its position would write it over the new
/// one's.
pub async fn ended(token: i64, pos: f64, dur: f64) {
    // Taken in a block of its own: the guard must be gone before the first
    // `await`, or this future stops being `Send` and the dispatcher will not
    // hold it.
    let taken = {
        let Ok(mut g) = pending().lock() else { return };
        let matches = g.as_ref().is_some_and(|p| p.generation as i64 == token);
        if matches { g.take() } else { None }
    };
    let Some(p) = taken else { return };
    if let Some(sink) = p.on_end {
        sink(pos, dur);
    }
    let Some(id) = p.item_id else { return };
    let Ok(pool) = crate::db::videos_pool().await else { return };
    if dur > 0.0 && pos >= dur * 0.98 {
        let _ = tulipix_videos::watch_progress::mark_finished(pool, id, true).await;
        // Watched to the end: Trakt hears about it, if it is switched on and
        // the account is linked. Detached, so closing the player never waits.
        crate::api::videos::trakt_push_watched(id);
    } else if pos > 1.0 {
        let _ = tulipix_videos::watch_progress::update(pool, id, pos, Some(dur)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seek_args_cover_both_halves_of_a_broken_range_response() {
        let args = network_seek_args();
        // Forward seeking on a host that answers 200 instead of 206.
        assert!(args.iter().any(|a| a == "--force-seekable=yes"));
        // Backward seeking, which needs the cache to have kept what is behind.
        assert!(args.iter().any(|a| a.starts_with("--demuxer-max-back-bytes=")));
    }

    #[test]
    fn cli_flags_become_properties_and_non_assignments_are_dropped() {
        let out = props(vec![
            "--force-seekable=yes".into(),
            "--sub-file=/tmp/a.srt".into(),
            "--sub-file=/tmp/b.srt".into(),
            // No `=`: nothing to set, and mpv's bare flags are all defaults here.
            "--fullscreen".into(),
            "not-an-option".into(),
        ]);
        assert_eq!(
            out,
            vec!["force-seekable=yes", "sub-file=/tmp/a.srt", "sub-file=/tmp/b.srt"]
        );
    }

    #[test]
    fn a_stale_token_is_ignored() {
        // Two launches: the first one's report must not fire the second's hook.
        let first = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = fired.clone();
        *pending().lock().unwrap() = Some(Pending {
            generation: first + 1, // as if a newer launch replaced it
            item_id: None,
            on_end: None,
            on_playing: Some(Box::new(move || f.store(true, Ordering::SeqCst))),
        });
        started(first as i64);
        assert!(!fired.load(Ordering::SeqCst));
        started(first as i64 + 1);
        assert!(fired.load(Ordering::SeqCst));
    }
}
