//! The video transport: one mpv, in its own top-level window.
//!
//! Separate from `mpv.rs` on purpose. That one is the *audio* singleton —
//! headless, `--no-video`, driven continuously by a player bar. This one is
//! fire-and-forget: a file or a URL goes to mpv, mpv draws its own window with
//! its own OSC, and the only thing that comes back is where playback got to.
//!
//! This is `tulipix_common::spawn_mpv_windowed_tracked` minus the parts that
//! only exist because the caller was Slint. The policy is unchanged — the same
//! flags, the same Settings keys, the same `--start` on resume, the same
//! writeback into `watch_progress` — because a video that resumes in one build
//! and not the other is not a port.
//!
//! There was an embedded player once, libmpv rendering into a Slint texture. It
//! froze the whole UI on weak integrated GPUs and was deleted in v0.8.0. The
//! Flutter port does not reintroduce it: `media_kit` would be the same bet with
//! a different name.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tulipix_core::proc::NoWindow;

/// Called once, after mpv exits, with `(position_s, duration_s)`.
///
/// Both are zero when mpv never reported them — a stream that failed to open.
pub type PlaybackEnd = Arc<dyn Fn(f64, f64) + Send + Sync + 'static>;

/// PID of the window that is up, or 0. An atomic rather than the `Child`,
/// because the thread that owns the `Child` is blocked in `wait()`.
static PID: AtomicU32 = AtomicU32::new(0);

/// Bumped by every launch. A watcher captures it and drops its own reports once
/// a newer launch owns the window — switching channels kills the running mpv,
/// so the old process's exit hook fires *after* the new one started.
static GENERATION: AtomicU32 = AtomicU32::new(0);

fn socket() -> &'static Mutex<PathBuf> {
    static S: OnceLock<Mutex<PathBuf>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(endpoint()))
}

/// Its own socket name, not the Slint build's `tulipix-mpv`: both builds run
/// side by side during the port, and two apps sharing one IPC endpoint means
/// each one's commands land in whichever mpv answered first.
fn endpoint() -> PathBuf {
    let pid = std::process::id();
    #[cfg(windows)]
    {
        PathBuf::from(format!(r"\\.\pipe\tulipix-flutter-video-{pid}"))
    }
    #[cfg(not(windows))]
    {
        std::env::temp_dir().join(format!("tulipix-flutter-video-{pid}.sock"))
    }
}

#[cfg(not(windows))]
type Conn = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Conn = std::fs::File;

#[cfg(not(windows))]
fn connect_once(path: &Path) -> std::io::Result<Conn> {
    std::os::unix::net::UnixStream::connect(path)
}

#[cfg(windows)]
fn connect_once(path: &Path) -> std::io::Result<Conn> {
    std::fs::OpenOptions::new().read(true).write(true).open(path)
}

/// Retry for ~3 s. The socket does not exist the instant mpv is spawned, and a
/// window that has to negotiate a live stream can take longer than a file.
fn connect(path: &Path) -> std::io::Result<Conn> {
    let mut last = std::io::Error::new(std::io::ErrorKind::NotConnected, "mpv ipc: not attempted");
    for _ in 0..60 {
        match connect_once(path) {
            Ok(s) => return Ok(s),
            Err(e) => {
                last = e;
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
        }
    }
    Err(last)
}

/// Kill the window, if one is up.
pub fn stop() {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    let pid = PID.swap(0, Ordering::SeqCst);
    if pid == 0 {
        return;
    }
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"])
        .no_window()
        .status();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("kill").arg("-9").arg(pid.to_string()).status();
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

#[cfg(target_os = "linux")]
fn die_with_parent(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: `prctl` is async-signal-safe and touches only this process's own
    // death signal. Nothing here allocates or locks, which is the whole rule
    // for a `pre_exec` closure.
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0);
            Ok(())
        });
    }
}

#[cfg(not(target_os = "linux"))]
fn die_with_parent(_cmd: &mut std::process::Command) {
    // No PR_SET_PDEATHSIG on macOS or Windows. A Win32 Job Object with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE would harden crash-time cleanup.
}

/// Settings → Playback, as CLI options at spawn.
///
/// The Slint build pushed these into libmpv at runtime; with playback
/// out-of-process they go in at launch, which is also why they only take effect
/// on the next play. Everything is opt-in except the subtitle style.
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

/// Open `src` — a path or a URL — in an mpv window.
///
/// `resume` seeks in at launch. `item_id` writes progress back into the local
/// library's `watch_progress`; `on_end` is the escape hatch for media that has
/// no `items` row, which is every remote title. Both may be supplied; neither
/// has to be.
///
/// Returns immediately: the spawn, the watcher and the wait all happen on a
/// plain thread, because a `Child::wait` that ran on a bridge worker would hold
/// it for the length of the film.
///
/// ponytail: progress is reported once, at exit, matching what the local
/// library has always done. A crash or a kill loses the session; periodic ticks
/// would be a timer around the same shared cell.
pub fn play(
    src: String,
    resume: Option<f64>,
    item_id: Option<i64>,
    extra_args: Vec<String>,
    on_end: Option<PlaybackEnd>,
) {
    // One video at a time, and never over the music: a new window replaces
    // whatever was on, which is what the Slint build's `kill_music_proc` +
    // `stop_video` pair does at this point.
    crate::mpv::stop();
    stop();
    let generation = GENERATION.load(Ordering::SeqCst);
    let rt = tokio::runtime::Handle::current();

    std::thread::spawn(move || {
        let sock = endpoint();
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&sock);
        if let Ok(mut g) = socket().lock() {
            *g = sock.clone();
        }

        let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
        cmd.no_window();
        cmd.arg(&src)
            .arg("--force-window=yes")
            .arg("--window-maximized=yes")
            .arg("--keep-open=no")
            .arg(format!("--input-ipc-server={}", sock.display()));
        if let Some(r) = resume.filter(|r| *r > 1.0) {
            cmd.arg(format!("--start={r}"));
        }
        for a in settings_args() {
            cmd.arg(a);
        }
        for a in &extra_args {
            cmd.arg(a);
        }
        die_with_parent(&mut cmd);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                tracing::error!(error = %e, "mpv window launch failed");
                return;
            }
        };
        PID.store(child.id(), Ordering::SeqCst);

        // Reader thread: observe time-pos + duration into a shared cell. mpv
        // serves several IPC clients, so this one coexists with the watcher's.
        let pos = Arc::new(Mutex::new((0f64, 0f64)));
        let seen = pos.clone();
        let sockp = sock.clone();
        let reader = std::thread::spawn(move || {
            let Ok(stream) = connect(&sockp) else { return };
            let Ok(mut tx) = stream.try_clone() else { return };
            if tx
                .write_all(
                    b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n\
                      {\"command\":[\"observe_property\",2,\"duration\"]}\n",
                )
                .is_err()
            {
                return;
            }
            for line in BufReader::new(stream).lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
                if v["event"] != "property-change" {
                    continue;
                }
                let Some(d) = v["data"].as_f64() else { continue };
                let Ok(mut g) = seen.lock() else { continue };
                match v["name"].as_str() {
                    Some("time-pos") => g.0 = d,
                    Some("duration") => g.1 = d,
                    _ => {}
                }
            }
        });

        let _ = child.wait();
        let _ = PID.compare_exchange(child.id(), 0, Ordering::SeqCst, Ordering::SeqCst);
        #[cfg(not(windows))]
        let _ = std::fs::remove_file(&sock);
        let _ = reader.join();

        // Replaced, not ended: a newer launch owns the window, and reporting
        // this one's position would write it over the new one's.
        if GENERATION.load(Ordering::SeqCst) != generation {
            return;
        }

        let (p, d) = pos.lock().map(|g| *g).unwrap_or((0.0, 0.0));
        if let Some(sink) = on_end {
            sink(p, d);
        }
        if let Some(id) = item_id {
            rt.spawn(async move {
                let Ok(pool) = crate::db::videos_pool().await else { return };
                if d > 0.0 && p >= d * 0.98 {
                    let _ = tulipix_videos::watch_progress::mark_finished(pool, id, true).await;
                } else if p > 1.0 {
                    let _ = tulipix_videos::watch_progress::update(pool, id, p, Some(d)).await;
                }
            });
        }
    });
}

/// Follow the mpv that was just spawned, for the two things a live stream needs
/// after the process is already up.
///
/// * The status line says "Opening …" from the moment the button is pressed. A
///   live stream can take seconds to negotiate, so it is only promoted to
///   "Playing …" once mpv reports a running clock.
/// * Switching audio track on a live stream leaves mpv playing buffered video
///   with nothing on it: the new track starts downloading from the live edge
///   and the picture has to catch up. Seeking to the end of the seekable window
///   on every `aid` change closes that gap instead of waiting it out.
///
/// `on_playing` runs at most once, on the watcher thread.
pub fn watch_live(on_playing: impl Fn() + Send + 'static) {
    let generation = GENERATION.load(Ordering::SeqCst);
    std::thread::spawn(move || {
        // The socket is created by the mpv being spawned right now, after it
        // unlinks the previous one. Connecting into that gap gets a refusal
        // from the dead socket.
        std::thread::sleep(std::time::Duration::from_millis(400));
        let Ok(sock) = socket().lock().map(|g| g.clone()) else { return };
        let Ok(stream) = connect(&sock) else { return };
        let Ok(mut tx) = stream.try_clone() else { return };
        if tx
            .write_all(
                b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n\
                  {\"command\":[\"observe_property\",2,\"aid\"]}\n",
            )
            .is_err()
        {
            return;
        }
        let mut running = false;
        let mut seen_aid = false;
        for line in BufReader::new(stream).lines().map_while(Result::ok) {
            if GENERATION.load(Ordering::SeqCst) != generation {
                return; // another channel owns the window now
            }
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            if v["event"] != "property-change" {
                continue;
            }
            match v["name"].as_str() {
                Some("time-pos") if !running => {
                    if v["data"].as_f64().unwrap_or(0.0) > 0.0 {
                        running = true;
                        on_playing();
                    }
                }
                Some("aid") => {
                    // The first report is the track mpv opened with, which is
                    // already at the live edge.
                    if seen_aid {
                        let _ = tx.write_all(b"{\"command\":[\"seek\",100,\"absolute-percent\"]}\n");
                    }
                    seen_aid = true;
                }
                _ => {}
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_socket_is_this_processs_own_not_the_slint_builds() {
        let name = endpoint().to_string_lossy().into_owned();
        assert!(name.contains("tulipix-flutter-video"), "{name}");
        assert!(name.contains(&std::process::id().to_string()), "{name}");
    }

    #[test]
    fn seek_args_cover_both_halves_of_a_broken_range_response() {
        let args = network_seek_args();
        // Forward seeking on a host that answers 200 instead of 206.
        assert!(args.iter().any(|a| a == "--force-seekable=yes"));
        // Backward seeking, which needs the cache to have kept what is behind.
        assert!(args.iter().any(|a| a.starts_with("--demuxer-max-back-bytes=")));
    }
}
