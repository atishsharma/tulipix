//! The audio transport: one headless mpv, driven over its JSON IPC socket.
//!
//! This is `tulipix_common::{player, mpv_ipc}` minus the parts that only exist
//! because the caller was Slint. It is not a second implementation of a policy
//! — the policy (which flags, what ReplayGain does, how a device id becomes an
//! `--audio-device`) still comes from `tulipix_music::{player, output_device}`,
//! the frozen domain crate. What is rewritten here is the ~120 lines of spawn,
//! socket and reader thread, because `tulipix-common` depends on slint and
//! nothing the bridge links may.
//!
//! One process, not five. My Music, Podcasts, Audiobooks, Radio and YouTube
//! audio all land on the same singleton, so starting a podcast stops the album
//! that was playing — which is what the five tabs sharing one player bar
//! means. `Slot` records which of them owns the current process, so the bar
//! knows whether Next means "next track" or "next episode".

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use tulipix_core::proc::NoWindow;

/// Which tab owns the running process. The player bar reads this to decide
/// what its transport buttons mean; `Idle` is "nothing has ever played".
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

/// Everything the reader thread has learned from mpv. Read synchronously by
/// every snapshot, so it is a plain mutex over copyable fields rather than a
/// channel — a snapshot must never wait on a network stream's next property.
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
    /// True between `play()` and the reader thread noticing the socket close.
    pub loaded: bool,
    /// Momentary loudness from the `ebur128` meter, normalised to 0..1. The
    /// visualizer's energy: shape is synthetic, but the pulse is the real
    /// audio. Deliberately NOT part of the event stream — it arrives many
    /// times a second, so the UI polls it instead (`music_loudness`).
    pub loud: f64,
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
            loud: 0.0,
        }
    }
}

/// Bumped on every `play()` and every `stop()`. A reader thread captures it at
/// spawn and checks it on the way out: a dying reader must not report EOF for
/// a track the user already replaced, or the next track would be skipped the
/// instant it started.
static GENERATION: AtomicU64 = AtomicU64::new(0);

fn obs() -> &'static Mutex<Obs> {
    static O: OnceLock<Mutex<Obs>> = OnceLock::new();
    O.get_or_init(|| Mutex::new(Obs::default()))
}

fn child() -> &'static Mutex<Option<Child>> {
    static C: OnceLock<Mutex<Option<Child>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// The write half of the reader's connection, handed over once the reader has
/// it. Controls go down this rather than opening a socket per click: a
/// connect-write-drop per volume drag is one mpv "Write error (Broken pipe)"
/// per event in its log, and a socket setup for every frame of the slider.
fn ipc() -> &'static Mutex<Option<Conn>> {
    static W: OnceLock<Mutex<Option<Conn>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(None))
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

#[cfg(not(windows))]
type Conn = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Conn = std::fs::File;

fn endpoint() -> PathBuf {
    let pid = std::process::id();
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\tulipix-flutter-music-{pid}"))
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir().join(format!("tulipix-flutter-music-{pid}.sock"))
    }
}

#[cfg(not(windows))]
fn connect_once(path: &Path) -> std::io::Result<Conn> {
    std::os::unix::net::UnixStream::connect(path)
}

#[cfg(windows)]
fn connect_once(path: &Path) -> std::io::Result<Conn> {
    std::fs::OpenOptions::new().read(true).write(true).open(path)
}

/// Retry for ~3 s: the socket is not ready the instant mpv is spawned. A
/// refusal is different from a miss — the file exists but nothing listens, so
/// it is a leftover from a dead mpv and will never come alive.
fn connect(path: &Path) -> std::io::Result<Conn> {
    let mut last =
        std::io::Error::new(std::io::ErrorKind::NotConnected, "mpv ipc: not attempted");
    for _ in 0..60 {
        match connect_once(path) {
            Ok(s) => return Ok(s),
            #[cfg(not(windows))]
            Err(e) if e.kind() == std::io::ErrorKind::ConnectionRefused => return Err(e),
            Err(e) => {
                last = e;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    Err(last)
}

/// The properties the reader subscribes to. `eof-reached` is not among them:
/// the socket closing is the end-of-file signal, and it is the one that also
/// fires when mpv dies for any other reason.
const OBSERVED: &[(u64, &str)] = &[
    (1, "time-pos"),
    (2, "duration"),
    (3, "pause"),
    (4, "volume"),
    (5, "mute"),
    (6, "media-title"),
    (7, "af-metadata/vis/lavfi.r128.M"),
];

/// Read the current observations. Never blocks on mpv.
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

/// Kill whatever is playing. Bumps the generation first, so the reader thread
/// that is about to notice the closed socket knows this was a replacement and
/// not a track ending.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut g) = ipc().lock() {
        *g = None;
    }
    if let Ok(mut g) = child().lock() {
        if let Some(mut c) = g.take() {
            let _ = c.kill();
            // Reap it. Without this the process stays a zombie for the life of
            // the app, and a session of skipping tracks leaves a screenful.
            let _ = c.wait();
        }
    }
    if let Ok(mut g) = obs().lock() {
        *g = Obs::default();
    }
    if let Ok(mut g) = slot().lock() {
        *g = Slot::Idle;
    }
}

/// One JSON command down the shared socket, e.g. `["set_property","pause",true]`.
/// Silently does nothing when nothing is playing, which is the right answer for
/// a volume slider dragged before the first track.
pub fn command(json: &str) {
    if let Ok(mut g) = ipc().lock() {
        if let Some(conn) = g.as_mut() {
            let line = format!("{json}\n");
            if conn.write_all(line.as_bytes()).is_err() {
                // The process went away between the snapshot and the click.
                *g = None;
            }
        }
    }
}

pub fn set_property(name: &str, value: &str) {
    command(&format!(r#"{{"command":["set_property","{name}",{value}]}}"#));
}

pub fn seek_absolute(secs: f64) {
    command(&format!(
        r#"{{"command":["seek",{secs},"absolute"]}}"#
    ));
    // Repaint from the requested position rather than waiting up to a second
    // for mpv's next `time-pos`. The scrubber is dragged, released, and looked
    // at immediately; a bar that snaps back before jumping forward reads as a
    // failed seek.
    if let Ok(mut g) = obs().lock() {
        g.pos = secs;
    }
}

/// Launch `src` (a path or a URL) under `args`, replacing whatever was playing.
///
/// `on_prop` runs on the reader thread for every property change that survives
/// the once-a-second `time-pos` throttle; `on_eof` runs exactly once, when the
/// socket closes, and only if this launch is still the current generation.
pub fn play<P, E>(
    src: &str,
    slot_kind: Slot,
    args: &[String],
    start_s: Option<f64>,
    on_prop: P,
    on_eof: E,
) -> std::io::Result<()>
where
    P: Fn(&Obs) + Send + 'static,
    E: FnOnce() + Send + 'static,
{
    stop();
    let generation = GENERATION.load(Ordering::SeqCst);

    let sock = endpoint();
    #[cfg(not(windows))]
    let _ = std::fs::remove_file(&sock);

    let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
    cmd.no_window();
    cmd.arg("--no-video")
        .arg("--force-window=no")
        .arg("--idle=no")
        .arg(format!("--input-ipc-server={}", sock.display()));
    for a in args {
        cmd.arg(a);
    }
    if let Some(t) = start_s.filter(|t| *t > 1.0) {
        cmd.arg(format!("--start={t}"));
    }
    let spawned = cmd.arg(src).spawn()?;

    if let Ok(mut g) = child().lock() {
        *g = Some(spawned);
    }
    if let Ok(mut g) = slot().lock() {
        *g = slot_kind;
    }
    if let Ok(mut g) = obs().lock() {
        *g = Obs { loaded: true, ..Obs::default() };
    }

    std::thread::spawn(move || {
        if let Ok(stream) = connect(&sock) {
            let mut stream = stream;
            let mut sub = String::new();
            for (id, name) in OBSERVED {
                sub.push_str(&format!(
                    "{{\"command\":[\"observe_property\",{id},\"{name}\"]}}\n"
                ));
            }
            let _ = stream.write_all(sub.as_bytes());
            if let Ok(mut g) = ipc().lock() {
                *g = stream.try_clone().ok();
            }

            // Once a second, not sixty times: `time-pos` arrives at mpv's
            // frame rate and every one of these becomes a Dart event and a
            // rebuild of the player bar.
            let mut last_second = i64::MIN;
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if v["event"] != "property-change" {
                    continue;
                }
                let name = v["name"].as_str().unwrap_or("");
                let data = &v["data"];
                // Loudness never becomes a Dart event: it updates the shared
                // observation and stops there, the way the Slint build keeps it
                // in an atomic rather than hopping the event loop per frame.
                if name == "af-metadata/vis/lavfi.r128.M" {
                    if let Some(l) = data.as_str().and_then(|t| t.parse::<f64>().ok()) {
                        if let Ok(mut g) = obs().lock() {
                            g.loud = ((l + 45.0) / 39.0).clamp(0.0, 1.0);
                        }
                    }
                    continue;
                }
                if name == "time-pos" {
                    let secs = data.as_f64().unwrap_or(-1.0).floor() as i64;
                    if secs == last_second {
                        continue;
                    }
                    last_second = secs;
                }
                let updated = {
                    let Ok(mut g) = obs().lock() else { continue };
                    match name {
                        "time-pos" => g.pos = data.as_f64().unwrap_or(g.pos),
                        "duration" => g.dur = data.as_f64().unwrap_or(0.0),
                        "pause" => g.paused = data.as_bool().unwrap_or(false),
                        "volume" => g.volume = data.as_f64().unwrap_or(g.volume),
                        "mute" => g.muted = data.as_bool().unwrap_or(false),
                        "media-title" => {
                            g.media_title = data.as_str().unwrap_or("").to_string()
                        }
                        _ => {}
                    }
                    g.clone()
                };
                on_prop(&updated);
            }
        }

        if GENERATION.load(Ordering::SeqCst) != generation {
            // Replaced, not ended. The newer launch owns the singletons now,
            // and clearing them here would blank a bar that is playing.
            return;
        }
        if let Ok(mut g) = ipc().lock() {
            *g = None;
        }
        if let Ok(mut g) = obs().lock() {
            g.loaded = false;
        }
        on_eof();
    });
    Ok(())
}
