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
    // Stop whatever is on the audio singleton FIRST. Every caller is supposed
    // to do this, and a second call is a cheap no-op — but forgetting it does
    // not merely double up the sound: `music_proc()` is overwritten below, and
    // dropping a `std::process::Child` does not kill the process. The old mpv
    // then plays on with no handle left to stop it, under the new track's
    // artwork and seek bar, for the rest of the session. One guard here beats
    // one at each of the four call sites.
    crate::stop_music_child();
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
            // Same once-a-second throttle the persistent reader applies; see it
            // for why. Podcasts, radio and the legacy per-track path all land
            // here, and they push into the same UI properties.
            let mut last_pos = i64::MIN;
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] != "property-change" {
                    continue;
                }
                let name = v["name"].as_str().unwrap_or("");
                if name == "time-pos" {
                    let secs = v["data"].as_f64().unwrap_or(-1.0).floor() as i64;
                    if secs == last_pos {
                        continue;
                    }
                    last_pos = secs;
                }
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

/// Which persistent session is current. Bumped on every respawn, captured by
/// that spawn's reader thread, and checked by it on the way out: a dying
/// reader must only clear the session state if it is still the one that owns
/// it. Without this, killing the old process and spawning a new one in the
/// same breath lets the old reader wake up *after* the new handlers are
/// installed and wipe them — a track that plays with a dead now-playing bar
/// and never auto-advances.
static PERSIST_SESSION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    /// Resume offset. On a fresh spawn it becomes `--start`; a request for one
    /// against a live process forces a respawn rather than a mid-stream seek
    /// race. Either way `ipc_loadfile` re-states it (as `"none"` when there
    /// isn't one) on every load — see the leak it exists to stop.
    pub start_s: Option<f64>,
    pub observe: &'a [(u64, &'a str)],
    pub generation: u64,
}

/// `loadfile` over the live socket — built with serde_json rather than
/// `music_ipc`, whose naive quoting would corrupt `\` in Windows paths.
/// Load `src` into the live mpv, pinning the start position for THIS file.
///
/// `--start` is a session-level option, not a per-file one: a process spawned
/// with `--start=35` to resume a podcast keeps `start=35` for every later
/// `loadfile` on that same process, so the next track begins 35 seconds in —
/// and the one after that, until something forces a respawn. Setting the
/// property explicitly on every load is what stops one track's resume point
/// leaking into the rest of the session. `"none"` is mpv's own default.
///
/// Both commands go down the SHARED connection the reader thread is draining,
/// so mpv has somewhere to put its replies. A connect-write-drop client is one
/// `[ipc_N] Write error (Broken pipe)` in mpv's log per command.
///
/// Returns whether mpv **acknowledged** the load. Every step here can fail
/// quietly — no socket recorded, a stale endpoint nothing listens on, a write
/// into a half-closed pipe — and a silent failure is the worst outcome on this
/// path: the UI has already swapped to the new track, so the previous one plays
/// on underneath a seek bar counting a song it is not playing. The caller
/// respawns on `false`, which is loud and correct.
fn ipc_loadfile(src: &Path, start_s: Option<f64>) -> bool {
    let start = match start_s {
        Some(v) if v > 1.0 => format!("{v:.0}"),
        _ => "none".to_string(),
    };
    // Distinct ids on purpose: mpv answers the `set_property` first, and if it
    // carried the load's id the wait below would return on THAT reply — an ack
    // for a load mpv had not looked at yet, which is exactly the silent failure
    // this function exists to catch.
    let set = serde_json::json!({
        "command": ["set_property", "start", start], "request_id": 1 });
    let cmd = serde_json::json!({
        "command": ["loadfile", src.display().to_string(), "replace"],
        "request_id": LOAD_REQ_ID });
    let payload = format!("{set}\n{cmd}\n");

    // Arm the answer slot BEFORE writing, or a fast reply lands in a slot that
    // is about to be cleared and the load reads as unacknowledged.
    LOAD_ACK.store(ACK_WAITING, std::sync::atomic::Ordering::SeqCst);
    if crate::music_ipc_raw(&payload) {
        return wait_load_ack();
    }
    LOAD_ACK.store(ACK_NONE, std::sync::atomic::Ordering::SeqCst);

    // No shared connection yet — mpv is alive but its reader has not attached.
    // Fall back to a one-shot client that reads its own reply.
    let Some(sock) = music_sock().lock().ok().and_then(|g| g.clone()) else { return false; };
    let Ok(mut s) = mpv_ipc::connect(&sock) else { return false; };
    if s.write_all(payload.as_bytes()).is_err() {
        return false;
    }
    load_ack(s)
}

/// Request id used for both halves of a load. The reader thread watches for it.
const LOAD_REQ_ID: u64 = 2;
const ACK_NONE: u8 = 0;
const ACK_WAITING: u8 = 1;
const ACK_OK: u8 = 2;
const ACK_FAILED: u8 = 3;

/// mpv's answer to the last `loadfile`, filled in by the reader thread.
///
/// The reader owns the only read end of the shared connection, so the ack
/// cannot be read here directly — it is handed over through this instead.
pub(crate) static LOAD_ACK: std::sync::atomic::AtomicU8 =
    std::sync::atomic::AtomicU8::new(ACK_NONE);

/// Record a reply the reader thread recognised as ours.
pub(crate) fn note_load_ack(success: bool) {
    // Only while something is waiting: a reply that arrives after the wait gave
    // up must not be sitting in the slot when the next load arms it.
    let _ = LOAD_ACK.compare_exchange(
        ACK_WAITING,
        if success { ACK_OK } else { ACK_FAILED },
        std::sync::atomic::Ordering::SeqCst,
        std::sync::atomic::Ordering::SeqCst,
    );
}

/// Block until the reader reports the load, or 250ms passes.
///
/// The wait is what keeps this honest on the UI thread: a wedged mpv costs a
/// quarter second and a respawn, not a frozen window. Polling rather than a
/// condvar because the reader must never block on a UI-side lock.
fn wait_load_ack() -> bool {
    for _ in 0..50 {
        match LOAD_ACK.load(std::sync::atomic::Ordering::SeqCst) {
            ACK_OK => {
                LOAD_ACK.store(ACK_NONE, std::sync::atomic::Ordering::SeqCst);
                return true;
            }
            ACK_FAILED => {
                LOAD_ACK.store(ACK_NONE, std::sync::atomic::Ordering::SeqCst);
                return false;
            }
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    }
    LOAD_ACK.store(ACK_NONE, std::sync::atomic::Ordering::SeqCst);
    false
}

/// Read mpv's reply to `request_id: 2` (the `loadfile`) off the same
/// connection. mpv broadcasts its own events to every client, so the reply is
/// not necessarily the first line back — hence the scan for the request id.
///
/// The timeout is what keeps this safe on the UI thread: a wedged mpv costs a
/// quarter second and a respawn, not a frozen window.
#[cfg(not(windows))]
fn load_ack(s: mpv_ipc::IpcConn) -> bool {
    if s.set_read_timeout(Some(std::time::Duration::from_millis(250))).is_err() {
        return true; // cannot bound the read — do not risk blocking the UI
    }
    let rd = BufReader::new(s);
    for line in rd.lines().map_while(Result::ok).take(64) {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
        if v["request_id"] == LOAD_REQ_ID {
            return v["error"] == "success";
        }
    }
    false
}

/// Windows named pipes carry no per-handle read timeout, and a blocking read on
/// the UI thread is worse than the bug this check exists to catch. The writes
/// having succeeded is the acknowledgement here.
#[cfg(windows)]
fn load_ack(_s: mpv_ipc::IpcConn) -> bool {
    true
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

    // Boxed once and moved at most once: the fast path can now fall through to
    // the respawn, and both want to install the same pair of closures.
    let mut handlers: Option<(PropHandler, EofHandler)> =
        Some((Box::new(on_prop), Box::new(on_eof)));

    if compatible {
        // Swap the callbacks first so the volume/mute property echoes from the
        // IPC sets below already route to the new track's handler. `take()`
        // only inside the successful lock — taking first and then failing to
        // store would drop the handlers on the floor.
        if let Ok(mut h) = persist_handlers().lock() {
            if let Some((p, e)) = handlers.take() {
                *h = Some((p, e, launch.generation));
            }
        }
        for (k, v) in &launch.load_props {
            crate::music_ipc(&["set_property", k, v]);
        }
        crate::music_ipc(&["set_property", "pause", "no"]);
        if ipc_loadfile(launch.src, launch.start_s) {
            return Ok(());
        }
        // The live process did not take the load. Do NOT return — falling
        // through respawns, which stops the old track. Returning here is what
        // leaves the previous song playing under the new song's now-playing bar.
        tracing::warn!("persistent mpv did not acknowledge loadfile; respawning");
        // Reclaim what was just installed so the respawn can hand it to the new
        // reader. If this fails, the entry written above is still in place and
        // is the same pair — the respawn leaves it alone.
        handlers = persist_handlers().lock().ok()
            .and_then(|mut h| h.take())
            .map(|(p, e, _)| (p, e));
    }

    // (Re)spawn — mirrors spawn_audio, plus idle mode and the event-driven
    // reader below (EOF must come from `end-file`, the socket never closes
    // between tracks). Graceful stop: a SIGKILL mid PipeWire link-activation
    // can wedge WirePlumber for the whole session.
    crate::stop_music_child();
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
    let session = PERSIST_SESSION.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    if let Ok(mut g) = music_proc().lock() {
        *g = Some(child);
    }
    if let Ok(mut g) = music_sock().lock() {
        *g = Some(sock.clone());
    }
    if let Ok(mut g) = persist_args().lock() {
        *g = Some(launch.session_args.clone());
    }
    // After the session bump, so the dying reader of the process we just
    // stopped cannot come along and clear what we install here.
    if let Some((p, e)) = handlers {
        if let Ok(mut h) = persist_handlers().lock() {
            *h = Some((p, e, launch.generation));
        }
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
            // Every later command rides this same connection, which this
            // thread drains — see `crate::MUSIC_CMD`. Without the shared write
            // half each control opened its own client and closed it before mpv
            // could answer, which is the burst of broken-pipe lines a track
            // change used to print.
            crate::set_music_cmd(stream.try_clone().ok());
            // Last whole second forwarded to the UI — see the throttle below.
            let mut last_pos = i64::MIN;
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                // A reply, not an event: the only one anybody waits for is the
                // `loadfile` ack.
                if v["request_id"] == LOAD_REQ_ID {
                    note_load_ack(v["error"] == "success");
                    continue;
                }
                match v["event"].as_str().unwrap_or("") {
                    "property-change" => {
                        let name = v["name"].as_str().unwrap_or("");
                        // mpv reports `time-pos` many times a second, and every
                        // one of them crossed into the UI thread to set a float
                        // and re-format a clock string that only changes once a
                        // second. Nothing downstream draws finer than that — the
                        // seek bar moves a pixel a second — so a repeat of the
                        // same whole second is dropped here rather than waking
                        // the event loop to write the value it already holds.
                        if name == "time-pos" {
                            let secs = v["data"].as_f64().unwrap_or(-1.0).floor() as i64;
                            if secs == last_pos {
                                continue;
                            }
                            last_pos = secs;
                        }
                        if let Ok(h) = persist_handlers().lock() {
                            if let Some((p, _, _)) = h.as_ref() {
                                p(name, &v["data"]);
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
        // play respawns cleanly — but only if a newer session has not already
        // taken over, or this thread's death throes wipe the live track's
        // handlers.
        if PERSIST_SESSION.load(std::sync::atomic::Ordering::SeqCst) != session {
            return;
        }
        // Nothing is draining the socket any more, so the shared write half has
        // to go with it: writing into it would put mpv back where it started.
        crate::set_music_cmd(None);
        if let Ok(mut g) = persist_args().lock() {
            *g = None;
        }
        if let Ok(mut h) = persist_handlers().lock() {
            *h = None;
        }
    });
    Ok(())
}
