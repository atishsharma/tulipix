//! The audio transport: what to play, and the state of what is playing.
//!
//! Nothing here spawns a process any more. There used to be a headless mpv on
//! the far side of a JSON IPC socket, and most of this file was the cost of
//! that: a spawn, a connect-with-retry, a reader thread parsing property-change
//! lines, a shared write half so a volume drag did not open a socket per frame,
//! and a kill-and-reap so a session of skipping tracks did not leave a screen of
//! zombies. Flutter plays the audio itself now, through `media_kit`, which is
//! the same libmpv in-process.
//!
//! What did not move is the policy, and that is the whole point of the split.
//! Which flags, what ReplayGain does, how a device id becomes an `audio-device`,
//! what EOF means for the queue, which tab owns the deck — all still decided
//! here and in `tulipix_music`, the frozen domain crate. Dart receives `k=v` mpv
//! properties and a source; it never decides one.
//!
//! One deck, not five. My Music, Podcasts, Audiobooks, Radio and YouTube audio
//! all land on the same singleton, so starting a podcast stops the album that
//! was playing — which is what the five tabs sharing one player bar means.
//! `Slot` records which of them owns it, so the bar knows whether Next means
//! "next track" or "next episode".

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::api::music::{emit, MusicEvent};

/// Which tab owns the deck. The player bar reads this to decide what its
/// transport buttons mean; `Idle` is "nothing has ever played".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    Idle,
    Music,
    Podcast,
    Book,
    Radio,
    Youtube,
}

impl Slot {
    pub fn as_str(self) -> &'static str {
        match self {
            Slot::Idle => "idle",
            Slot::Music => "music",
            Slot::Podcast => "podcast",
            Slot::Book => "book",
            Slot::Radio => "radio",
            Slot::Youtube => "youtube",
        }
    }
}

/// Everything Dart has told us about the deck. Read synchronously by every
/// snapshot, so it is a plain mutex over copyable fields.
#[derive(Debug, Clone)]
pub struct Obs {
    pub pos: f64,
    pub dur: f64,
    pub paused: bool,
    pub volume: f64,
    pub muted: bool,
    /// mpv's `media-title`. For a radio stream this is the ICY title, which is
    /// the only place the actual song is readable — the station name is ours.
    pub media_title: String,
    /// True between `play()` and Dart reporting the end.
    pub loaded: bool,
}

impl Default for Obs {
    fn default() -> Self {
        Self {
            pos: 0.0,
            dur: 0.0,
            paused: false,
            volume: 80.0,
            muted: false,
            media_title: String::new(),
            loaded: false,
        }
    }
}

/// Bumped on every `play()` and every `stop()`. Dart echoes it back as the
/// token on its reports: a report for a track the user already replaced must
/// not fire this one's EOF, or the next track would be skipped the instant it
/// started.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn obs() -> &'static Mutex<Obs> {
    static O: OnceLock<Mutex<Obs>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(Obs::default()))
}

fn slot() -> &'static Mutex<Slot> {
    static S: OnceLock<Mutex<Slot>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(Slot::Idle))
}

/// What is on the deck, as the UI names it. Kept here rather than in the
/// section state because a track can outlive the view that started it: leaving
/// My Music for Radio must not blank the bar.
#[derive(Debug, Clone, Default)]
pub struct NowPlaying {
    pub item_id: i64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub art: String,
    /// Video id / station uuid / episode id, for the tab that owns it.
    pub key: String,
}

fn now() -> &'static Mutex<NowPlaying> {
    static N: OnceLock<Mutex<NowPlaying>> = OnceLock::new();
    N.get_or_init(|| Mutex::new(NowPlaying::default()))
}

/// The callbacks belonging to whatever is on the deck. One at a time, because
/// one thing is on the deck at a time.
struct Hooks {
    generation: u64,
    on_prop: Box<dyn Fn(&Obs) + Send + 'static>,
    on_eof: Option<Box<dyn FnOnce() + Send + 'static>>,
}

fn hooks() -> &'static Mutex<Option<Hooks>> {
    static H: OnceLock<Mutex<Option<Hooks>>> = OnceLock::new();
    H.get_or_init(|| Mutex::new(None))
}

/// Read the current observations. Never blocks.
pub fn observe() -> Obs {
    obs().lock().map(|g| g.clone()).unwrap_or_default()
}

pub fn current_slot() -> Slot {
    slot().lock().map(|g| *g).unwrap_or(Slot::Idle)
}

pub fn now_playing() -> NowPlaying {
    now().lock().map(|g| g.clone()).unwrap_or_default()
}

pub fn set_now_playing(np: NowPlaying) {
    if let Ok(mut g) = now().lock() {
        *g = np;
    }
}

/// `--k=v` → `k=v`, dropping anything that is not an option assignment.
///
/// Both transports build their options as mpv CLI flags — that is the form the
/// domain crates return and the form the Slint build passed at spawn — and this
/// is the one place that turns them into the properties Dart sets.
pub(crate) fn to_props(args: impl IntoIterator<Item = String>) -> Vec<String> {
    args.into_iter()
        .filter_map(|a| {
            let a = a.strip_prefix("--")?;
            a.contains('=').then(|| a.to_string())
        })
        .collect()
}

/// Stop whatever is playing. Bumps the generation first, so a report already in
/// flight from the outgoing source is dropped rather than treated as this one
/// ending.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = hooks().lock() {
        *g = None;
    }
    if let Ok(mut g) = obs().lock() {
        *g = Obs::default();
    }
    if let Ok(mut g) = slot().lock() {
        *g = Slot::Idle;
    }
    emit(MusicEvent::AudioStop);
}

/// Set one mpv property on the deck. `value` is a JSON literal — `true`, `85`,
/// `"inf"` — which is what it was when this wrote `set_property` commands down
/// the IPC socket, and what Dart decodes on the other side.
///
/// Silently does nothing visible when nothing is playing, which is the right
/// answer for a volume slider dragged before the first track.
pub fn set_property(name: &str, value: &str) {
    emit(MusicEvent::AudioProp { name: name.to_string(), value: value.to_string() });
}

pub fn seek_absolute(secs: f64) {
    emit(MusicEvent::AudioSeek { secs });
    // Repaint from the requested position rather than waiting for the next
    // tick. The scrubber is dragged, released, and looked at immediately; a bar
    // that snaps back before jumping forward reads as a failed seek.
    if let Ok(mut g) = obs().lock() {
        g.pos = secs;
    }
}

/// Put `src` (a path or a URL) on the deck under `args`, replacing whatever was
/// playing.
///
/// `on_prop` runs on every report Dart sends — roughly once a second, plus
/// whenever pause/volume/mute/title changes. `on_eof` runs exactly once, when
/// Dart reports the end, and only if this launch is still the current one.
///
/// Returns nothing: there is no longer a failure that is knowable here. A
/// source that cannot be opened fails inside the player, and that arrives as
/// `music_audio_failed` — see [`failed`].
pub fn play<P, E>(
    src: &str,
    slot_kind: Slot,
    args: &[String],
    start_s: Option<f64>,
    on_prop: P,
    on_eof: E,
) where
    P: Fn(&Obs) + Send + 'static,
    E: FnOnce() + Send + 'static,
{
    stop();
    let generation = GENERATION.load(Ordering::SeqCst);

    if let Ok(mut g) = hooks().lock() {
        *g = Some(Hooks {
            generation,
            on_prop: Box::new(on_prop),
            on_eof: Some(Box::new(on_eof)),
        });
    }
    if let Ok(mut g) = slot().lock() {
        *g = slot_kind;
    }
    if let Ok(mut g) = obs().lock() {
        *g = Obs { loaded: true, ..Obs::default() };
    }

    // `vid=no`: the deck decodes audio only, which is what `--no-video` bought
    // at spawn. A YouTube URL resolved to a muxed stream would otherwise pull a
    // video track nobody is looking at.
    let mut props = vec!["vid=no".to_string()];
    props.extend(to_props(args.iter().cloned()));
    emit(MusicEvent::AudioPlay {
        token: generation as i64,
        src: src.to_string(),
        start_at: start_s.filter(|t| *t > 1.0).unwrap_or(0.0),
        props,
    });
}

/// Dart's periodic report. Updates the observation and runs `on_prop`, which is
/// what turns into the `Tick` the player bar draws.
pub fn report(
    token: i64,
    pos: f64,
    dur: f64,
    paused: bool,
    volume: f64,
    muted: bool,
    media_title: String,
) {
    let current = {
        let Ok(mut g) = obs().lock() else { return };
        g.pos = pos;
        g.dur = dur;
        g.paused = paused;
        g.volume = volume;
        g.muted = muted;
        g.media_title = media_title;
        g.clone()
    };
    if let Ok(g) = hooks().lock() {
        if let Some(h) = g.as_ref().filter(|h| h.generation as i64 == token) {
            (h.on_prop)(&current);
        }
    }
}

/// Dart reached the end of the source. Runs `on_eof` once — which is what
/// advances the queue — and only for the source that is still current.
pub fn ended(token: i64) {
    let hook = hooks().lock().ok().and_then(|mut g| {
        let h = g.as_mut()?;
        (h.generation as i64 == token).then(|| h.on_eof.take())?
    });
    if let Ok(mut g) = obs().lock() {
        g.loaded = false;
    }
    if let Some(f) = hook {
        f();
    }
}

/// Dart could not open the source. The old build learned this from `spawn()`
/// returning an error, which only ever caught a missing mpv binary; this
/// catches the cases that actually happen — a file that moved, a codec that is
/// not there, a stream that refused.
pub fn failed(token: i64, message: String) {
    let current = hooks()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|h| h.generation as i64 == token))
        .unwrap_or(false);
    if !current {
        return;
    }
    if let Ok(mut g) = obs().lock() {
        g.loaded = false;
    }
    emit(MusicEvent::Failed { message });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_flags_become_properties_and_non_assignments_are_dropped() {
        let out = to_props([
            "--af=lavfi=[...]".to_string(),
            "--volume=80".to_string(),
            // No `=`: nothing to set.
            "--no-video".to_string(),
            "not-an-option".to_string(),
        ]);
        assert_eq!(out, vec!["af=lavfi=[...]", "volume=80"]);
    }

    #[test]
    fn a_replaced_source_does_not_advance_the_queue() {
        let fired = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let f = fired.clone();
        play("/tmp/a.flac", Slot::Music, &[], None, |_| {}, move || {
            f.store(true, Ordering::SeqCst)
        });
        let stale = GENERATION.load(Ordering::SeqCst) as i64 - 1;
        ended(stale);
        assert!(!fired.load(Ordering::SeqCst), "an outgoing track ended the incoming one");
        ended(GENERATION.load(Ordering::SeqCst) as i64);
        assert!(fired.load(Ordering::SeqCst));
    }
}
