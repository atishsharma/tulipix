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
