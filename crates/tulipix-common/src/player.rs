//! Player core — Phase 1: the shared mpv transport.
//!
//! Every music section (My Music · Podcasts · Audiobooks · Radio · YouTube
//! audio) used to hand-roll the same block: spawn a headless control-socket
//! mpv, stash the child + socket in the shared singletons, then run a reader
//! thread that turns mpv `property-change` events into now-playing updates and
//! auto-advances on EOF. [`spawn_audio`] is that block, factored once.
//!
//! It is deliberately a thin **backend transport**: section-specific concerns
//! (which mpv flags, what each property means in the UI, what "advance" does)
//! stay in the caller via `pre_args`, the `on_prop` closure, and `on_eof`. That
//! keeps behavior identical while removing the duplication, and gives Phase 2 a
//! single seam to slot an embedded-libmpv backend behind.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use tulipix_core::proc::NoWindow;

use crate::{mpv_die_with_parent, mpv_ipc, music_proc, music_sock, stop_video, MUSIC_GEN};

/// One headless-audio mpv launch. `pre_args` carries the section-built flags
/// (af/EQ, replaygain, output device, volume, mute …); the universal headless
/// flags + IPC server are added here. `observe` is the `(id, property)` set the
/// reader subscribes to. `generation` is the caller's `MUSIC_GEN` snapshot so a
/// finished track only auto-advances if it is still the current one.
pub struct AudioLaunch<'a> {
    pub prefix: &'a str,
    pub mpv_bin: PathBuf,
    pub src: &'a Path,
    pub pre_args: Vec<String>,
    pub observe: &'a [(u64, &'a str)],
    pub generation: u64,
}

/// Spawn the headless audio mpv, store it in the shared `music_proc`/`music_sock`
/// singletons (so `music_ipc` / `kill_music_proc` keep working), and run the
/// reader thread.
///
/// * `on_prop(name, data)` runs on the **reader thread** for every mpv
///   `property-change` — the caller decides whether to handle it in-thread
///   (e.g. an atomic) or hop to the Slint event loop.
/// * `on_eof()` runs once the socket closes **iff** `MUSIC_GEN` still equals
///   `generation` (natural end, not a user-triggered replacement).
///
/// Mirrors the previous per-section block exactly, including `stop_video()`
/// (audio takes over the universal stream) and `mpv_die_with_parent`.
pub fn spawn_audio<P, E>(launch: AudioLaunch, on_prop: P, on_eof: E) -> std::io::Result<()>
where
    P: Fn(&str, &serde_json::Value) + Send + 'static,
    E: FnOnce() + Send + 'static,
{
    let sock = mpv_ipc::endpoint(launch.prefix);
    mpv_ipc::cleanup(&sock);

    let mut cmd = std::process::Command::new(&launch.mpv_bin);
    cmd.no_window();
    cmd.arg("--no-video").arg("--force-window=no").arg("--idle=no")
        .arg(format!("--input-ipc-server={}", sock.display()));
    for a in &launch.pre_args {
        cmd.arg(a);
    }
    // Audio takes over the universal stream from any windowed video.
    stop_video();
    mpv_die_with_parent(&mut cmd);

    let child = cmd.arg(launch.src).spawn()?;
    if let Ok(mut g) = music_proc().lock() {
        *g = Some(child);
    }
    if let Ok(mut g) = music_sock().lock() {
        *g = Some(sock.clone());
    }

    let observe: Vec<(u64, String)> =
        launch.observe.iter().map(|(id, n)| (*id, n.to_string())).collect();
    let gen_id = launch.generation;
    std::thread::spawn(move || {
        if let Ok(mut stream) = mpv_ipc::connect(&sock) {
            let mut sub = String::new();
            for (id, name) in &observe {
                sub.push_str(&format!("{{\"command\":[\"observe_property\",{id},\"{name}\"]}}\n"));
            }
            let _ = stream.write_all(sub.as_bytes());
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] != "property-change" {
                    continue;
                }
                let name = v["name"].as_str().unwrap_or("");
                on_prop(name, &v["data"]);
            }
        }
        // Socket closed = track ended (or was replaced). Auto-advance only if
        // this is still the active generation (natural EOF, not user action).
        if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) == gen_id {
            on_eof();
        }
    });
    Ok(())
}

// ── Persistent transport (np.b2.music.persistent) ──────────────────────────
// One long-lived `--idle=yes` mpv for library tracks: the next track is a
// `loadfile` over the live IPC socket instead of kill+respawn, removing the
// process-spawn gap between tracks (and letting mpv's own `--gapless-audio`
// keep the device open). The process is only respawned when the session-level
// flags (af/EQ, output device, replaygain…) change; per-track knobs (volume,
// mute) travel over IPC as properties. Streaming sections (radio / podcasts /
// YouTube audio) keep the spawn-per-source transport above — their sources
// are network URLs where a dying pipeline should tear the process down.

type PropHandler = Box<dyn Fn(&str, &serde_json::Value) + Send>;
type EofHandler = Box<dyn Fn() + Send>;

/// Session args the live persistent process was spawned with — a mismatch on
/// the next play forces a respawn so changed flags actually apply. `None`
/// while no persistent process is alive.
static PERSIST_ARGS: std::sync::OnceLock<std::sync::Mutex<Option<Vec<String>>>> =
    std::sync::OnceLock::new();
fn persist_args() -> &'static std::sync::Mutex<Option<Vec<String>>> {
    PERSIST_ARGS.get_or_init(|| std::sync::Mutex::new(None))
}

/// Current track's callbacks + generation. Swapped on every `loadfile`; the
/// reader thread calls them under the lock (its only consumer), which spares
/// the handlers a `Sync` bound.
#[allow(clippy::type_complexity)]
static PERSIST_HANDLERS: std::sync::OnceLock<
    std::sync::Mutex<Option<(PropHandler, EofHandler, u64)>>,
> = std::sync::OnceLock::new();
fn persist_handlers() -> &'static std::sync::Mutex<Option<(PropHandler, EofHandler, u64)>> {
    PERSIST_HANDLERS.get_or_init(|| std::sync::Mutex::new(None))
}

/// A library-track play request against the persistent backend.
pub struct PersistentLaunch<'a> {
    pub prefix: &'a str,
    pub mpv_bin: PathBuf,
    pub src: &'a Path,
    /// Process-level flags (af/EQ, output device, replaygain, gapless…).
    /// Changing any of these respawns the process.
    pub session_args: Vec<String>,
    /// Per-track `(property, value)` pairs — applied over IPC before each
    /// `loadfile`, or as `--property=value` on a fresh spawn.
    pub load_props: Vec<(String, String)>,
    /// Cold-start resume offset. Only honoured on spawn (`--start`); when a
    /// process is already live the request forces a respawn instead of a
    /// mid-stream seek race.
    pub start_s: Option<f64>,
    pub observe: &'a [(u64, &'a str)],
    pub generation: u64,
}

/// `loadfile` over the live socket — built with serde_json rather than
/// `music_ipc`, whose naive quoting would corrupt `\` in Windows paths.
fn ipc_loadfile(src: &Path) {
    let Some(sock) = music_sock().lock().ok().and_then(|g| g.clone()) else { return; };
    let cmd = serde_json::json!({ "command": ["loadfile", src.display().to_string(), "replace"] });
    if let Ok(mut s) = mpv_ipc::connect(&sock) {
        let _ = s.write_all(format!("{cmd}\n").as_bytes());
    }
}

/// Play `src` on the persistent audio mpv: reuse the live process via
/// `loadfile` when the session args match, respawn otherwise. `on_eof` is a
/// re-usable `Fn` (unlike [`spawn_audio`]'s `FnOnce`) because one process
/// outlives many tracks.
pub fn play_persistent<P, E>(launch: PersistentLaunch, on_prop: P, on_eof: E) -> std::io::Result<()>
where
    P: Fn(&str, &serde_json::Value) + Send + 'static,
    E: Fn() + Send + 'static,
{
    let alive = music_proc().lock().ok()
        .map(|mut g| match g.as_mut() {
            Some(c) => c.try_wait().map(|st| st.is_none()).unwrap_or(false),
            None => false,
        })
        .unwrap_or(false);
    let compatible = alive
        && launch.start_s.is_none()
        && persist_args().lock().ok()
            .map(|g| g.as_deref() == Some(&launch.session_args[..]))
            .unwrap_or(false);

    if compatible {
        // Swap the callbacks first so the volume/mute property echoes from the
        // IPC sets below already route to the new track's handler.
        if let Ok(mut h) = persist_handlers().lock() {
            *h = Some((Box::new(on_prop), Box::new(on_eof), launch.generation));
        }
        for (k, v) in &launch.load_props {
            crate::music_ipc(&["set_property", k, v]);
        }
        crate::music_ipc(&["set_property", "pause", "no"]);
        ipc_loadfile(launch.src);
        return Ok(());
    }

    // (Re)spawn — mirrors spawn_audio, plus idle mode and the event-driven
    // reader below (EOF must come from `end-file`, the socket never closes
    // between tracks).
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
    let sock = mpv_ipc::endpoint(launch.prefix);
    mpv_ipc::cleanup(&sock);

    let mut cmd = std::process::Command::new(&launch.mpv_bin);
    cmd.no_window();
    cmd.arg("--no-video").arg("--force-window=no").arg("--idle=yes").arg("--keep-open=no")
        .arg(format!("--input-ipc-server={}", sock.display()));
    for a in &launch.session_args {
        cmd.arg(a);
    }
    for (k, v) in &launch.load_props {
        cmd.arg(format!("--{k}={v}"));
    }
    if let Some(s) = launch.start_s {
        cmd.arg(format!("--start={s:.0}"));
    }
    stop_video();
    mpv_die_with_parent(&mut cmd);

    let child = cmd.arg(launch.src).spawn()?;
    if let Ok(mut g) = music_proc().lock() {
        *g = Some(child);
    }
    if let Ok(mut g) = music_sock().lock() {
        *g = Some(sock.clone());
    }
    if let Ok(mut g) = persist_args().lock() {
        *g = Some(launch.session_args.clone());
    }
    if let Ok(mut h) = persist_handlers().lock() {
        *h = Some((Box::new(on_prop), Box::new(on_eof), launch.generation));
    }

    let observe: Vec<(u64, String)> =
        launch.observe.iter().map(|(id, n)| (*id, n.to_string())).collect();
    std::thread::spawn(move || {
        if let Ok(mut stream) = mpv_ipc::connect(&sock) {
            let mut sub = String::new();
            for (id, name) in &observe {
                sub.push_str(&format!("{{\"command\":[\"observe_property\",{id},\"{name}\"]}}\n"));
            }
            let _ = stream.write_all(sub.as_bytes());
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                match v["event"].as_str().unwrap_or("") {
                    "property-change" => {
                        if let Ok(h) = persist_handlers().lock() {
                            if let Some((p, _, _)) = h.as_ref() {
                                p(v["name"].as_str().unwrap_or(""), &v["data"]);
                            }
                        }
                    }
                    // Natural end advances; `stop`/`redirect` reasons come from
                    // our own `loadfile replace` and user stops — skip those.
                    "end-file" if v["reason"] == "eof" => {
                        if let Ok(h) = persist_handlers().lock() {
                            if let Some((_, eof, gen_id)) = h.as_ref() {
                                if MUSIC_GEN.load(std::sync::atomic::Ordering::SeqCst) == *gen_id {
                                    eof();
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        // Socket gone = the process died (user stop, or another section took
        // over the audio singleton). Clear the session so the next library
        // play respawns cleanly.
        if let Ok(mut g) = persist_args().lock() {
            *g = None;
        }
        if let Ok(mut h) = persist_handlers().lock() {
            *h = None;
        }
    });
    Ok(())
}
