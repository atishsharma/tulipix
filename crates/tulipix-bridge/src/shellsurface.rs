//! The three places the OS shows a running media app, none of which the port
//! had: the desktop's media applet, the status-bar tray, and the taskbar.
//!
//! The Slint build registers all three. This is the same registration, moved to
//! the side of the bridge that now owns playback.
//!
//! Everything here is one dedicated thread. `souvlaki::MediaControls` is not
//! `Send` on every platform, and `tulipix_common`'s publish helpers keep the
//! handle in a `thread_local!` — so the handle is created on that thread, used
//! only from there, and reached from anywhere else through a channel. Remote
//! commands travel the other way as `MusicEvent::Remote`, which Dart answers
//! with the matching `MusicCmd`: the deck lives in Dart now, so a media key has
//! to land there rather than being executed here.

use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

use crate::api::music::{emit, MusicEvent};

/// What the media thread is asked to publish.
enum Msg {
    Track {
        title: String,
        artist: String,
        album: String,
        cover: Option<String>,
        dur: f32,
    },
    Progress {
        playing: bool,
        pos: f32,
    },
    Volume(f64),
}

fn tx() -> &'static OnceLock<Sender<Msg>> {
    static T: OnceLock<Sender<Msg>> = OnceLock::new();
    &T
}

fn send(msg: Msg) {
    if let Some(t) = tx().get() {
        let _ = t.send(msg);
    }
}

/// Register with the desktop: media applet, tray, media keys.
///
/// Idempotent, and safe to call before anything is playing — the applet wants
/// the player to exist before it will route a media key to it, which is why
/// this advertises a paused state rather than waiting for a first track.
pub fn start() {
    let (t, rx) = channel::<Msg>();
    if tx().set(t).is_err() {
        return;
    }
    std::thread::Builder::new()
        .name("tulipix-media".into())
        .spawn(move || {
            attach();
            for msg in rx {
                match msg {
                    Msg::Track {
                        title,
                        artist,
                        album,
                        cover,
                        dur,
                    } => tulipix_common::media_set_track(
                        &title,
                        &artist,
                        &album,
                        cover.as_deref(),
                        dur,
                    ),
                    Msg::Progress { playing, pos } => {
                        tulipix_common::media_set_progress(playing, pos)
                    }
                    Msg::Volume(v) => tulipix_common::media_set_volume(v),
                }
            }
        })
        .ok();
    start_tray();
}

/// Build the `MediaControls` and hand every remote command to Dart.
///
/// The handle goes into `tulipix_common::MEDIA_CONTROLS`, which is a
/// `thread_local!` — so it has to be stored on the thread that will publish
/// through it, and that is this one.
fn attach() {
    use souvlaki::{MediaControlEvent, MediaControls, MediaPlayback, PlatformConfig, SeekDirection};

    // Windows' SMTC needs the native window handle; MPRIS and the macOS info
    // centre take none, and the Flutter runner does not hand one across.
    let config = PlatformConfig {
        dbus_name: "tulipix",
        display_name: "Tulipix",
        hwnd: None,
    };
    let mut controls = match MediaControls::new(config) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!(error = ?e, "media controls unavailable");
            return;
        }
    };
    let attached = controls.attach(|event: MediaControlEvent| {
        // Names, not enum arms, because the far side of this is Dart. One
        // string and one number covers every command the protocol defines.
        let (action, value) = match event {
            MediaControlEvent::Toggle => ("toggle", 0.0),
            MediaControlEvent::Play => ("play", 0.0),
            MediaControlEvent::Pause => ("pause", 0.0),
            MediaControlEvent::Stop => ("stop", 0.0),
            MediaControlEvent::Next => ("next", 0.0),
            MediaControlEvent::Previous => ("prev", 0.0),
            MediaControlEvent::SetPosition(p) => ("seek", p.0.as_secs_f64()),
            // `SeekBy` carries an amount and a direction; a bare `Seek` carries
            // only a direction, so it gets the ten seconds the app's own arrow
            // keys use.
            MediaControlEvent::SeekBy(dir, d) => (
                "seekby",
                d.as_secs_f64() * if dir == SeekDirection::Backward { -1.0 } else { 1.0 },
            ),
            MediaControlEvent::Seek(dir) => (
                "seekby",
                if dir == SeekDirection::Backward { -10.0 } else { 10.0 },
            ),
            // The wire is 0..1 and the app's scale is 0..130 — mpv allows the
            // boost. MPRIS also requires the value to be echoed back, or a
            // slider dragged on a remote springs to where it thinks we are.
            MediaControlEvent::SetVolume(v) => {
                // Through the channel, not `tulipix_common` directly: this
                // closure runs on souvlaki's own D-Bus thread, and the handle
                // the echo needs is a `thread_local!` belonging to ours.
                set_volume(v);
                ("volume", v * 130.0)
            }
            MediaControlEvent::Raise => ("raise", 0.0),
            MediaControlEvent::Quit => ("quit", 0.0),
            _ => return,
        };
        emit(MusicEvent::Remote {
            action: action.into(),
            value,
        });
    });
    if let Err(e) = attached {
        tracing::warn!(error = ?e, "media controls attach failed");
        return;
    }
    // Advertise before anything plays, so the desktop routes media keys here.
    let _ = controls.set_playback(MediaPlayback::Paused { progress: None });
    tulipix_common::MEDIA_CONTROLS.with(|c| *c.borrow_mut() = Some(controls));
}

/// The status-bar item and its menu.
///
/// The icon is decoded here rather than in `tulipix-platform`, which takes raw
/// RGBA precisely so it needs no image dependency of its own.
fn start_tray() {
    let icon = image::load_from_memory(include_bytes!(
        "../../../resources/icons/tulipix-64.png"
    ))
    .ok()
    .map(|img| {
        let rgba = img.thumbnail(64, 64).to_rgba8();
        let (w, h) = rgba.dimensions();
        (rgba.into_raw(), w, h)
    });
    if !tulipix_platform::init_tray(icon) {
        tracing::warn!("tray: no StatusNotifier host");
        return;
    }
    // ksni pushes menu clicks onto a channel and the platform crate exposes
    // them by polling, which is what the Slint build's UI timer does. There is
    // no such timer here, so this is a thread that does nothing but drain.
    std::thread::Builder::new()
        .name("tulipix-tray-events".into())
        .spawn(|| loop {
            tulipix_platform::drain_tray_events(|id| {
                let action = match id {
                    "tray.open" | "tray.now" => "raise",
                    "tray.playpause" => "toggle",
                    "tray.next" => "next",
                    "tray.prev" => "prev",
                    "tray.shuffle" => "shuffle",
                    "tray.repeat.off" | "tray.repeat.all" | "tray.repeat.one" => "repeat",
                    "tray.mini" => "mini",
                    "tray.popup" => "raise",
                    "tray.quit" => "quit",
                    _ => return,
                };
                let value = match id {
                    "tray.repeat.all" => 1.0,
                    "tray.repeat.one" => 2.0,
                    _ => 0.0,
                };
                emit(MusicEvent::Remote {
                    action: action.into(),
                    value,
                });
            });
            std::thread::sleep(std::time::Duration::from_millis(200));
        })
        .ok();
}

/// Publish a new track to every surface. Called when the track changes, not on
/// a tick: the artwork has already been resolved by then and that is the part
/// worth not repeating.
pub fn set_track(title: &str, artist: &str, album: &str, cover: &str, dur: f32) {
    send(Msg::Track {
        title: title.into(),
        artist: artist.into(),
        album: album.into(),
        cover: (!cover.is_empty()).then(|| cover.to_string()),
        dur,
    });
}

/// Where playback got to, and whether it is running. Cheap enough for a tick:
/// souvlaki compares against what it last published.
pub fn set_progress(playing: bool, pos: f32) {
    send(Msg::Progress { playing, pos });
}

pub fn set_volume(vol_0_1: f64) {
    send(Msg::Volume(vol_0_1));
}

/// What the tray menu says about playback.
pub fn set_tray(np: tulipix_platform::TrayNowPlaying) {
    tulipix_platform::update_tray(np);
}
