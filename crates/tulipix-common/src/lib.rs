//! Shared app infrastructure for the section-wiring crates.
//!
//! Owns the per-section SQLite pool cache (`pool_for`), the data-directory
//! helpers, and the bundled-binary probes — everything a section crate needs
//! that isn't section-specific, without depending on `tulipix-app`.

use anyhow::Result;
pub mod mpv_ipc;
pub mod player;
pub mod quotes;
use tulipix_core::proc::NoWindow;
use std::path::PathBuf;

/// Per-section pool slot. An *async* once-cell, not `OnceLock`: initialisation
/// awaits (open + schema + migrations), and several tasks routinely race here
/// at startup. With check-then-set, every racing caller opened its own pool and
/// re-applied the whole schema — one library was seen running the same
/// migration three times at once, fighting for the write lock.
type PoolCell = tokio::sync::OnceCell<sqlx::SqlitePool>;

static PHOTOS_POOL: PoolCell = PoolCell::const_new();
static VIDEOS_POOL: PoolCell = PoolCell::const_new();
static MUSIC_POOL:  PoolCell = PoolCell::const_new();
static CLOUD_POOL:  PoolCell = PoolCell::const_new();
// Music sub-sections split out of music.db (no items FK — self-contained).
static PODCASTS_POOL: PoolCell = PoolCell::const_new();
static RADIO_POOL:    PoolCell = PoolCell::const_new();
static YOUTUBE_POOL:  PoolCell = PoolCell::const_new();
// Tools job queue (tools.db) — shared by the GUI Tools section + CLI.
static TOOLS_POOL:    PoolCell = PoolCell::const_new();
// Books library (books.db) — standalone, no items FK.
static BOOKS_POOL:    PoolCell = PoolCell::const_new();
// Transfer ledger (transfers.db). Read-only from here: the schema belongs to
// `tulipix-transfer`, which creates it when sharing first starts. Before that
// every query against it errors and the callers fall back to zero — which is
// the true answer for a machine that has never shared a file.
static TRANSFERS_POOL: PoolCell = PoolCell::const_new();

/// Open (or return the cached) SQLite pool for a section, applying its schema
/// on first open. Both the GUI and CLI go through here so every front-end sees
/// the same DB + migrations.
pub async fn pool_for(section: &str) -> Result<sqlx::SqlitePool> {
    let cache = match section {
        "photos" => &PHOTOS_POOL,
        "videos" => &VIDEOS_POOL,
        "music"  => &MUSIC_POOL,
        "cloud"  => &CLOUD_POOL,
        "podcasts" => &PODCASTS_POOL,
        "radio"    => &RADIO_POOL,
        "youtube"  => &YOUTUBE_POOL,
        "tools"    => &TOOLS_POOL,
        "books"    => &BOOKS_POOL,
        "transfers" => &TRANSFERS_POOL,
        _ => anyhow::bail!("unknown section"),
    };
    // Exactly one initialiser runs, however many tasks arrive at once; the
    // rest await its result instead of duplicating the work.
    cache.get_or_try_init(|| build_pool(section)).await.cloned()
}

/// Open a section's DB and apply its schema + migrations. Called at most once
/// per section — `pool_for` owns the caching.
async fn build_pool(section: &str) -> Result<sqlx::SqlitePool> {
    let handle = tulipix_core::db::DbHandle::open(section)?;
    let pool = handle.init_pool().await?;
    match section {
        "photos" => {
            tulipix_photos::schema::apply(&pool).await?;
            // Phase 6 migrations (safe — ALTER TABLE IF NOT EXISTS equivalent).
            let _ = sqlx::query("ALTER TABLE photo_meta ADD COLUMN color_label TEXT").execute(&pool).await;
            // Apply stacks schema (idempotent CREATE TABLE IF NOT EXISTS).
            let _ = tulipix_photos::stacks::apply_schema(&pool).await;
            // Dedup tables (created by build_clusters; ensure they exist).
            let _ = sqlx::query(
                "CREATE TABLE IF NOT EXISTS dedup_clusters (
                    id      INTEGER PRIMARY KEY AUTOINCREMENT,
                    kind    TEXT NOT NULL,
                    key     TEXT NOT NULL,
                    created INTEGER NOT NULL,
                    UNIQUE(kind, key)
                )"
            ).execute(&pool).await;
            let _ = sqlx::query(
                "CREATE TABLE IF NOT EXISTS dedup_members (
                    cluster_id INTEGER NOT NULL REFERENCES dedup_clusters(id) ON DELETE CASCADE,
                    item_id    INTEGER NOT NULL REFERENCES items(id) ON DELETE CASCADE,
                    PRIMARY KEY (cluster_id, item_id)
                )"
            ).execute(&pool).await;
        }
        "videos" => tulipix_videos::schema::apply(&pool).await?,
        "music"  => tulipix_music::schema::apply(&pool).await?,
        "podcasts" => {
            tulipix_music::podcasts::apply_schema(&pool).await?;
            migrate_split_from_music(&pool, &[
                ("podcasts",
                 "id, feed_url, title, author, image_url, category, description, last_checked"),
                ("podcast_episodes",
                 "id, podcast_id, guid, title, audio_url, published, duration_s, \
                  description, image_url, downloaded_path, position_s, played"),
            ]).await;
        }
        "radio" => {
            tulipix_music::radio::apply_schema(&pool).await?;
            migrate_split_from_music(&pool, &[
                ("radio_stations",
                 "id, station_uuid, name, url, favicon, country, tags, favourite"),
            ]).await;
        }
        "youtube" => {
            tulipix_music::youtube::store::apply_schema(&pool).await?;
        }
        "cloud"  => tulipix_cloud::schema::apply(&pool).await?,
        "tools"  => tulipix_tools::schema::apply(&pool).await?,
        "books"  => tulipix_books::schema::apply(&pool).await?,
        _ => {}
    }
    Ok(pool)
}

/// One-time migration: move `tables` (each `(name, explicit_column_list)`) out
/// of the legacy shared `music.db` into the freshly-opened section `dest` pool.
/// Idempotent: no-ops once the source tables are gone.
async fn migrate_split_from_music(dest: &sqlx::SqlitePool, tables: &[(&str, &str)]) {
    let Some(music_path) = tulipix_core::paths::db_path("music") else { return; };
    if !music_path.exists() { return; }
    let Ok(mut conn) = dest.acquire().await else { return; };
    if sqlx::query(sqlx::AssertSqlSafe(format!("ATTACH DATABASE '{}' AS legacy", music_path.display())))
        .execute(&mut *conn).await.is_err() { return; }
    let mut moved_any = false;
    for (table, cols) in tables {
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM legacy.sqlite_master WHERE type='table' AND name = ?")
            .bind(*table).fetch_optional(&mut *conn).await.ok().flatten();
        if exists.is_none() { continue; }
        moved_any = true;
        let _ = sqlx::query(sqlx::AssertSqlSafe(format!(
            "INSERT OR IGNORE INTO {table} ({cols}) SELECT {cols} FROM legacy.{table}")))
            .execute(&mut *conn).await;
    }
    if moved_any {
        for (table, _) in tables.iter().rev() {
            let _ = sqlx::query(sqlx::AssertSqlSafe(format!("DROP TABLE IF EXISTS legacy.{table}"))).execute(&mut *conn).await;
        }
        tracing::info!("migrated {} table(s) out of music.db into a split section DB", tables.len());
    }
    let _ = sqlx::query("DETACH DATABASE legacy").execute(&mut *conn).await;
}

/// App data dir (`<config>/Tulipix`), per OS.
pub fn dirs_default() -> Option<std::path::PathBuf> {
    let base = if cfg!(target_os = "linux") {
        std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("APPDATA").map(std::path::PathBuf::from)
    };
    base.map(|b| b.join("Tulipix"))
}

/// The user's Documents folder (best effort), home as the fallback.
pub fn dirs_default_documents() -> std::path::PathBuf {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let docs = home.join("Documents");
    if docs.is_dir() { docs } else { home }
}

/// The user's Music folder (best effort): `$XDG_MUSIC_DIR` on Linux, else
/// `~/Music`; home as the final fallback. Default download destination.
pub fn dirs_default_music() -> std::path::PathBuf {
    if cfg!(target_os = "linux") {
        if let Some(dir) = std::env::var_os("XDG_MUSIC_DIR").map(std::path::PathBuf::from) {
            if dir.is_dir() {
                return dir;
            }
        }
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    home.join("Music")
}

/// Directory holding the per-OS bundled binaries (dev layout).
pub fn bundled_bin_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../resources/bin/linux-x86_64"))
}

/// Is `file` present in the bundled-binary dir?
pub fn bundled_present(file: &str) -> bool { bundled_bin_dir().join(file).exists() }

/// Is `name` resolvable on PATH?
pub fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").map(|paths| {
        std::env::split_paths(&paths).any(|d| d.join(name).exists())
    }).unwrap_or(false)
}

// ---- Sizes ------------------------------------------------------------------

/// Human-readable byte count: scaled unit + thousands-separated raw bytes.
pub fn human_size(bytes: u64) -> String {
    let b = bytes as f64;
    let (val, unit) = if b >= 1_073_741_824.0 { (b / 1_073_741_824.0, "GB") }
        else if b >= 1_048_576.0 { (b / 1_048_576.0, "MB") }
        else if b >= 1024.0 { (b / 1024.0, "KB") }
        else { (b, "bytes") };
    // Thousands-separated raw byte count.
    let mut raw = String::new();
    let digits = bytes.to_string();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 { raw.push(','); }
        raw.push(c);
    }
    if unit == "bytes" { format!("{raw} bytes") } else { format!("{val:.2} {unit} ({raw} bytes)") }
}

// ---- Watched folders + time (shared by every media section) ----------------
// The list itself is `tulipix_core::watched`'s: one owner, atomic writes, a
// backup. These are the names the rest of the app already calls.

pub fn load_watched_folders() -> Vec<PathBuf> {
    tulipix_core::watched::load()
}

/// Add `dir` to the watched-folders set (idempotent) and persist. Returns true
/// if it was newly added.
pub fn add_watched_folder(dir: &std::path::Path) -> bool {
    let added = tulipix_core::watched::add(dir);
    if added {
        // Attach the new folder to the running FS watcher so deletes/renames
        // there update the library live, without waiting for a restart.
        tulipix_core::watcher::watch_path(dir);
        log_activity("emerald", "Folder added to library", &dir.display().to_string());
    }
    added
}

pub use tulipix_core::util::unix_secs_i64 as now_secs;

// ---- Activity log ---------------------------------------------------------
/// The things a person does to the app that no database writes down: watching
/// a folder, unwatching one, starting a rescan, resetting.
///
/// The status page's Activity Timeline was fed by the Tools job table alone,
/// so adding a folder — the most consequential thing you can do to a library —
/// left no mark on the page that reports on that library. Sections that keep
/// their own tables (transfers, finances, playback) are still read from those;
/// this file is only for the events that had nowhere else to live.
///
/// A JSON array, rewritten whole and capped. It is appended a handful of times
/// a day at the very most, so rewriting beats keeping a second on-disk format.
pub const ACTIVITY_MAX: usize = 40;

pub fn activity_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("activity.json"))
}

/// `(unix seconds, accent key, title, detail)`, newest first.
pub fn load_activity() -> Vec<(i64, String, String, String)> {
    let Some(p) = activity_path() else { return Vec::new() };
    let Ok(body) = std::fs::read_to_string(&p) else { return Vec::new() };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) else { return Vec::new() };
    let s = |o: &serde_json::Value, k: &str| {
        o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
    };
    v.as_array()
        .map(|rows| {
            rows.iter()
                .map(|o| {
                    (
                        o.get("at").and_then(|x| x.as_i64()).unwrap_or(0),
                        s(o, "accent"),
                        s(o, "title"),
                        s(o, "desc"),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Record one event. `accent` is the section key the status page tints it with
/// ("system", "photos", "tools", …). Best-effort: a status page that misses an
/// entry is not worth failing a folder add over.
pub fn log_activity(accent: &str, title: &str, desc: &str) {
    let Some(p) = activity_path() else { return };
    let mut rows = load_activity();
    rows.insert(0, (now_secs(), accent.into(), title.into(), desc.into()));
    rows.truncate(ACTIVITY_MAX);
    let out: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(at, accent, title, desc)| {
            serde_json::json!({ "at": at, "accent": accent, "title": title, "desc": desc })
        })
        .collect();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(body) = serde_json::to_string_pretty(&out) {
        let _ = std::fs::write(&p, body);
    }
}

// ---- Out-of-process mpv playback core (shared by music/video/cloud) -------
pub static MUSIC_PROC: std::sync::OnceLock<std::sync::Mutex<Option<std::process::Child>>> =
    std::sync::OnceLock::new();
pub fn music_proc() -> &'static std::sync::Mutex<Option<std::process::Child>> {
    MUSIC_PROC.get_or_init(|| std::sync::Mutex::new(None))
}
/// PID of the windowed video mpv (0 = none). Tracked so music↔video share one
/// "universal" stream and so playback dies with the app.
pub static VIDEO_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
/// Make a spawned child die with us — on Linux the kernel sends SIGKILL when
/// the parent (tulipix) exits for ANY reason (close, crash, kill), so mpv never
/// orphans. `PR_SET_PDEATHSIG` is Linux-only; on other OSes children are reaped
/// by `kill_all_mpv()` on window close + `child.kill()` on track change.
#[cfg(target_os = "linux")]
pub fn mpv_die_with_parent(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL as libc::c_ulong, 0, 0, 0);
            Ok(())
        });
    }
}
#[cfg(not(target_os = "linux"))]
pub fn mpv_die_with_parent(_cmd: &mut std::process::Command) {
    // macOS/Windows: no PR_SET_PDEATHSIG. A Win32 Job Object with
    // JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE would harden crash-time cleanup.
}
/// Kill the windowed video mpv if one is running.
pub fn stop_video() {
    let pid = VIDEO_PID.swap(0, std::sync::atomic::Ordering::SeqCst);
    if pid == 0 { return; }
    #[cfg(windows)]
    let _ = std::process::Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F", "/T"]).no_window().status();
    #[cfg(not(windows))]
    let _ = std::process::Command::new("kill").arg("-9").arg(pid.to_string()).status();
}
/// Stop the headless music mpv without an instant SIGKILL: ask it to `quit`
/// over IPC and give it a short grace window to close its audio stream
/// cleanly, hard-killing only if it doesn't exit. SIGKILL-ing a PipeWire
/// client mid link-activation can wedge WirePlumber session-wide — every
/// later mpv then starts with no sound until the session restarts (user
/// report 2026-07-12, audiobooks: rapid chapter respawns). Also clears the
/// socket singleton + file so later one-shot IPC calls no-op fast instead of
/// probing a dead endpoint. Does NOT touch `MUSIC_GEN` — callers that need to
/// suppress auto-advance bump it themselves.
pub fn stop_music_child() {
    let sock = music_sock().lock().ok().and_then(|mut g| g.take());
    let child = music_proc().lock().ok().and_then(|mut g| g.take());
    let Some(mut child) = child else {
        if let Some(s) = &sock { mpv_ipc::cleanup(s); }
        return;
    };
    // Quit down the shared connection when there is one, so the goodbye does
    // not itself cost a broken-pipe line. Then drop it: this process is on its
    // way out and the handle is about to be dead.
    if !music_ipc_raw("{\"command\":[\"quit\"]}\n") {
        if let Some(s) = &sock {
            let _ = ipc_oneshot(s, "{\"command\":[\"quit\"]}\n");
        }
    }
    set_music_cmd(None);
    // Reaping is what used to cost the CALLER up to 400 ms of sleep, and nearly
    // every caller is the Slint event loop — that is the stall between one song
    // ending and the next one starting. mpv has already been told to quit above
    // and closes its audio stream within a few milliseconds, so the grace
    // window, the kill fallback and the socket unlink all move off-thread. Both
    // singletons were taken above, so a play starting in the same breath
    // installs its own handles and this thread can no longer see them.
    std::thread::spawn(move || {
        let mut exited = false;
        for _ in 0..8 {
            match child.try_wait() {
                Ok(Some(_)) => { exited = true; break; }
                Ok(None) => std::thread::sleep(std::time::Duration::from_millis(50)),
                Err(_) => break,
            }
        }
        if !exited { let _ = child.kill(); let _ = child.wait(); }
        // The endpoint path is derived from prefix + pid, so a respawn that
        // happened while this thread waited is listening on the SAME file.
        // Unlinking it then would delete the live socket out from under the
        // track that is playing; only clean up when nothing has claimed it.
        if let Some(s) = &sock {
            let reused = music_sock().lock().ok()
                .map(|g| g.as_deref() == Some(s.as_path())).unwrap_or(false);
            if !reused { mpv_ipc::cleanup(s); }
        }
    });
}
/// Kill the headless music mpv if one is running.
pub fn kill_music_proc() {
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); // suppress auto-advance
    stop_music_child();
}
/// Stop every mpv we spawned — called when the app window closes so nothing keeps
/// playing in the background (np: universal-player teardown).
pub fn kill_all_mpv() { kill_music_proc(); stop_video(); }

/// Active music IPC socket path (mpv `--input-ipc-server`) for live control.
pub static MUSIC_SOCK: std::sync::OnceLock<std::sync::Mutex<Option<PathBuf>>> = std::sync::OnceLock::new();
pub fn music_sock() -> &'static std::sync::Mutex<Option<PathBuf>> {
    MUSIC_SOCK.get_or_init(|| std::sync::Mutex::new(None))
}
/// Playback generation — bumped on each play/stop so a finished track's reader
/// only auto-advances if it's still the current one.
pub static MUSIC_GEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);


/// The live music-mpv IPC connection, shared by every command sender.
///
/// This is the same socket the player's reader thread is listening on, cloned
/// at the write end. It exists because the obvious alternative — connect,
/// write, drop — is what mpv complains about: it answers EVERY command with a
/// reply and broadcasts its events to every attached client, so a client that
/// writes and closes is a socket mpv then writes into. That is one
/// `[ipc_N] Write error (Broken pipe)` per command, with N climbing forever,
/// and a track change sends a burst of them (volume, mute, replaygain, speed,
/// pause, loadfile…) which is why the burst arrived a dozen at a time.
///
/// One long-lived connection, drained by the reader thread, means mpv has
/// somewhere to put those replies and nothing to complain about.
pub static MUSIC_CMD: std::sync::OnceLock<std::sync::Mutex<Option<mpv_ipc::IpcConn>>> =
    std::sync::OnceLock::new();
pub fn music_cmd() -> &'static std::sync::Mutex<Option<mpv_ipc::IpcConn>> {
    MUSIC_CMD.get_or_init(|| std::sync::Mutex::new(None))
}

/// Hand the shared command connection over (or drop it when the reader dies).
pub fn set_music_cmd(conn: Option<mpv_ipc::IpcConn>) {
    if let Ok(mut g) = music_cmd().lock() {
        *g = conn;
    }
}

/// Write a raw, newline-terminated JSON-IPC payload to the live music mpv.
///
/// Returns whether it went out on the SHARED connection. `false` means the
/// caller cannot expect a reply to be seen by the reader thread — either
/// nothing is connected yet (mpv was spawned a moment ago and the reader is
/// still retrying) or the write failed, in which case the dead connection is
/// dropped so the next call re-establishes or falls back.
pub fn music_ipc_raw(payload: &str) -> bool {
    use std::io::Write;
    let Ok(mut g) = music_cmd().lock() else { return false; };
    let Some(conn) = g.as_mut() else { return false; };
    if conn.write_all(payload.as_bytes()).is_ok() {
        return true;
    }
    // mpv is gone, or the pipe half-closed under us. Drop it: a stale handle
    // would fail every later command silently.
    *g = None;
    false
}

/// Send `payload` on a throwaway connection without leaving mpv a broken pipe.
///
/// The shared connection above removed the *burst* of broken-pipe lines, but
/// not the last one: whenever a command is sent before the reader thread has
/// attached (the ~100 ms after a respawn) it lands here, and connect-write-drop
/// is precisely what mpv complains about. Two things make it write into a
/// socket we already closed — the reply to the command, and the event stream it
/// broadcasts to **every** attached client. So:
///
/// * `disable_event all` first, which silences the broadcast for this client
///   only, and
/// * read the replies before hanging up, so nothing is left in flight.
///
/// Returns `None` when the endpoint could not be reached or written, otherwise
/// the reply lines — `disable_event`'s ack first, then one per payload command.
/// Bounded by a 250 ms read timeout, because UI-thread callers land here.
pub fn ipc_oneshot(sock: &std::path::Path, payload: &str) -> Option<Vec<String>> {
    use std::io::Write;
    let mut c = mpv_ipc::connect(sock).ok()?;
    let want = payload.lines().filter(|l| !l.trim().is_empty()).count();
    let msg = format!("{{\"command\":[\"disable_event\",\"all\"]}}\n{payload}");
    c.write_all(msg.as_bytes()).ok()?;
    // Windows named pipes carry no per-handle read timeout, and a blocking read
    // on the UI thread is worse than the log line this exists to remove.
    #[cfg(windows)]
    return Some(Vec::new());
    #[cfg(not(windows))]
    {
        use std::io::BufRead;
        c.set_read_timeout(Some(std::time::Duration::from_millis(250))).ok()?;
        let mut out = Vec::new();
        for line in std::io::BufReader::new(c).lines().map_while(Result::ok) {
            out.push(line);
            if out.len() > want {
                break; // the ack for each command, plus disable_event's
            }
        }
        Some(out)
    }
}

/// Build the JSON for one command.
fn ipc_payload(args: &[&str]) -> String {
    format!("{{\"command\":[{}]}}\n",
        args.iter().map(|a| {
            // numbers/bools pass through; everything else is JSON-quoted.
            if a.parse::<f64>().is_ok() || **a == *"true" || **a == *"false" { a.to_string() }
            else { format!("\"{a}\"") }
        }).collect::<Vec<_>>().join(","))
}

/// Send a single JSON command to the live music mpv over its IPC socket.
pub fn music_ipc(args: &[&str]) {
    let payload = ipc_payload(args);
    if music_ipc_raw(&payload) {
        return;
    }
    // No shared connection: mpv was spawned moments ago and its reader has not
    // attached yet, or this is a control sent after the process died. A
    // one-shot connection still gets the command through, and `ipc_oneshot`
    // makes it cost nothing in mpv's log either.
    let Some(sock) = music_sock().lock().ok().and_then(|g| g.clone()) else { return; };
    let _ = ipc_oneshot(&sock, &payload);
}

/// Same JSON-IPC push for the windowed VIDEO mpv ("tulipix-mpv" socket) —
/// drives PiP float / shader toggles on the external player window.
pub fn video_ipc(args: &[&str]) {
    use std::io::Write;
    let sock = mpv_ipc::endpoint("tulipix-mpv");
    let payload = format!("{{\"command\":[{}]}}\n",
        args.iter().map(|a| {
            if a.parse::<f64>().is_ok() || **a == *"true" || **a == *"false" { a.to_string() }
            else { format!("\"{a}\"") }
        }).collect::<Vec<_>>().join(","));
    if let Ok(mut s) = mpv_ipc::connect(&sock) {
        let _ = s.write_all(payload.as_bytes());
    }
}

pub fn anime4k_shader_args() -> Option<(String, usize)> {
    let dir = dirs_default()?.join("shaders");
    let mut files: Vec<String> = std::fs::read_dir(&dir).ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("glsl"))
        .filter_map(|p| p.to_str().map(str::to_string))
        .collect();
    if files.is_empty() { return None; }
    files.sort();
    Some((files.join(":"), files.len()))
}

/// mpv flags that make a remote **video-on-demand** stream properly seekable.
///
/// The symptom without these is that seeking only works forward, and only as
/// far as the cache already reaches. Two things cause that, and both are here:
///
/// * A host that answers a Range request with a plain `200` instead of `206`
///   makes mpv mark the stream unseekable, at which point every seek is served
///   out of the demuxer cache or refused. `--force-seekable=yes` makes mpv issue
///   the seek regardless; when the host genuinely cannot serve it the seek fails
///   and playback continues, which beats never trying.
/// * Seeking *backwards* needs the cache to have kept what is behind the
///   playhead, and mpv's default back-buffer is small enough that stepping back
///   much at all forces a refetch — the exact request the hosts above fumble.
///
/// `--hr-seek=yes` is what makes a chapter jump land on the chapter mark rather
/// than the nearest keyframe before it. Chapter navigation itself is mpv's own
/// (its built-in bindings, PgUp/PgDn) and needs nothing from us — it just could
/// not work while seeking was broken.
///
/// Deliberately **not** applied to Live TV: a live stream has nothing behind the
/// playhead worth keeping, and a back-buffer this size on a channel left running
/// is real memory for no benefit.
///
/// ponytail: the cache sizes are a fixed guess, not a measurement. At a typical
/// 1080p bitrate 128MiB is roughly three or four minutes of back-seek. Raise
/// them if that proves short; they are peak RSS for the mpv process, not ours.
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

/// Launch a video in an external mpv window with resume + watch-progress
/// writeback over the JSON IPC socket. Runs entirely off the UI thread so the
/// app never blocks on playback (np.p3.player — windowed path).
pub fn spawn_mpv_windowed(path: PathBuf, resume: Option<f64>, item_id: Option<i64>) {
    spawn_mpv_windowed_with(path, resume, item_id, Vec::new())
}

/// As [`spawn_mpv_windowed`], plus caller-supplied mpv flags appended after the
/// built-in ones. Used by the Stream tab to attach remote subtitle tracks
/// (`--sub-file=…`), which have no local file to sit beside.
pub fn spawn_mpv_windowed_with(
    path: PathBuf,
    resume: Option<f64>,
    item_id: Option<i64>,
    extra_args: Vec<String>,
) {
    spawn_mpv_windowed_tracked(path, resume, item_id, extra_args, None)
}

/// Called once, after mpv exits, with `(position_s, duration_s)`.
///
/// Both are zero when mpv never reported them (a stream that failed to open).
pub type PlaybackEnd = std::sync::Arc<dyn Fn(f64, f64) + Send + Sync + 'static>;

/// As [`spawn_mpv_windowed_with`], plus a hook that receives where playback got
/// to. `item_id` writes progress into `watch_progress` for a local library item;
/// `on_end` is the escape hatch for media that has no `items` row — the Stream
/// tab's remote titles, which record against `stream_progress` instead.
///
/// ponytail: progress is reported once, at exit, matching what the local library
/// has always done. A crash or a kill loses the session. Periodic ticks are a
/// timer around the same shared cell if that ever matters.
pub fn spawn_mpv_windowed_tracked(
    path: PathBuf,
    resume: Option<f64>,
    item_id: Option<i64>,
    extra_args: Vec<String>,
    on_end: Option<PlaybackEnd>,
) {
    use std::io::{BufRead, BufReader, Write};
    let rt = tokio::runtime::Handle::current();
    // Universal single stream: a new video stops music + any prior video.
    kill_music_proc();
    stop_video();
    std::thread::spawn(move || {
        let sock = mpv_ipc::endpoint("tulipix-mpv");
        mpv_ipc::cleanup(&sock);
        let mut cmd = std::process::Command::new(tulipix_core::thumbs::tool_bin("mpv"));
        cmd.no_window();
        cmd.arg(&path)
            .arg("--force-window=yes")
            .arg("--window-maximized=yes")   // open full-size (windowed, not borderless)
            .arg("--keep-open=no")
            .arg(format!("--input-ipc-server={}", sock.display()));
        if let Some(r) = resume { if r > 1.0 { cmd.arg(format!("--start={r}")); } }
        // Settings → Playback. These used to be pushed into libmpv at runtime by
        // the in-app player; with playback out-of-process they go in as CLI
        // options at spawn, which is also why they only take effect on the next
        // play. Everything here is opt-in except the subtitle style.
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        // GLSL upscale chain — opt-in + shaders present.
        if s.flag("playback.upscale", false) {
            if let Some((chain, _)) = anime4k_shader_args() {
                cmd.arg(format!("--glsl-shaders={chain}"));
            }
        }
        // Subtitle styling. mpv's own renderer draws every track now, so the
        // style projects onto mpv options rather than the deleted Slint overlay.
        let mut style = tulipix_videos::sub_styling::SubtitleStyle::default();
        if let Some(v) = s.advanced.get("playback.sub-size").and_then(|x| x.trim().parse::<f32>().ok()) {
            style.font_size_px = v;
        }
        if let Some(c) = s.advanced.get("playback.sub-color").map(|c| c.trim()).filter(|c| !c.is_empty()) {
            style.color = c.to_string();
        }
        style.clamp();
        for (k, v) in style.to_mpv_options() {
            cmd.arg(format!("--{k}={v}"));
        }
        // Motion interpolation — heavy on integrated GPUs, hence opt-in.
        if s.flag("playback.interpolation", false) {
            cmd.arg("--interpolation=yes").arg("--video-sync=display-resample");
        }
        // Bit-perfect output straight to the device; silences other apps.
        if s.flag("playback.audio-exclusive", false) {
            cmd.arg("--audio-exclusive=yes");
        }
        cmd.args(&extra_args);
        mpv_die_with_parent(&mut cmd);
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => { tracing::error!(error = %e, "mpv window launch failed"); return; }
        };
        let vpid = child.id();
        VIDEO_PID.store(vpid, std::sync::atomic::Ordering::SeqCst);
        // Reader thread: observe time-pos + duration into a shared cell.
        let pos = std::sync::Arc::new(std::sync::Mutex::new((0f64, 0f64)));
        let pos2 = pos.clone();
        let sockp = sock.clone();
        let reader = std::thread::spawn(move || {
            let Ok(mut stream) = mpv_ipc::connect(&sockp) else { return; };
            let _ = stream.write_all(
                b"{\"command\":[\"observe_property\",1,\"time-pos\"]}\n{\"command\":[\"observe_property\",2,\"duration\"]}\n");
            let rd = BufReader::new(stream);
            for line in rd.lines().map_while(Result::ok) {
                let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue; };
                if v["event"] == "property-change" {
                    if let Some(d) = v["data"].as_f64() {
                        if let Ok(mut g) = pos2.lock() {
                            match v["name"].as_str() {
                                Some("time-pos") => g.0 = d,
                                Some("duration") => g.1 = d,
                                _ => {}
                            }
                        }
                    }
                }
            }
        });
        let _ = child.wait();
        let _ = VIDEO_PID.compare_exchange(vpid, 0, std::sync::atomic::Ordering::SeqCst, std::sync::atomic::Ordering::SeqCst);
        let _ = std::fs::remove_file(&sock);
        let _ = reader.join();
        let (p, d) = pos.lock().map(|g| *g).unwrap_or((0.0, 0.0));
        if let Some(sink) = on_end {
            sink(p, d);
        }
        if let Some(id) = item_id {
            rt.spawn(async move {
                if let Ok(pool) = pool_for("videos").await {
                    if d > 0.0 && p >= d * 0.98 {
                        let _ = tulipix_videos::watch_progress::mark_finished(&pool, id, true).await;
                    } else if p > 1.0 {
                        let _ = tulipix_videos::watch_progress::update(&pool, id, p, Some(d)).await;
                    }
                }
            });
        }
    });
}

// ---- Shared formatters / folder-section config / media surface --------------
/// Format a duration in seconds as `H:MM:SS` (or `M:SS` under an hour).
pub fn fmt_duration(secs: f64) -> String {
    if !(secs.is_finite()) || secs < 1.0 { return String::new(); }
    let total = secs as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Clock label for the player scrubber — always renders (`0:00` at zero).
pub fn fmt_clock(secs: f64) -> String {
    let secs = if secs.is_finite() && secs > 0.0 { secs } else { 0.0 };
    let total = secs as i64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Unix-epoch seconds → "12 Jun 2022" (podcast episode dates).
pub fn fmt_date(epoch: i64) -> String {
    use chrono::{TimeZone, Utc, Datelike};
    const MON: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    match Utc.timestamp_opt(epoch, 0).single() {
        Some(dt) => format!("{} {} {}", dt.day(), MON[(dt.month0() as usize).min(11)], dt.year()),
        None => String::new(),
    }
}

pub const MUSIC_SECTIONS: [&str; 5] = ["mymusic", "podcasts", "audiobooks", "radio", "youtube"];

pub fn folder_sections_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("music_folder_sections.json"))
}
pub fn load_folder_sections() -> std::collections::HashMap<String, String> {
    folder_sections_path()
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|b| serde_json::from_str(&b).ok())
        .unwrap_or_default()
}
pub fn save_folder_sections(map: &std::collections::HashMap<String, String>) {
    let Some(file) = folder_sections_path() else { return; };
    if let Some(parent) = file.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(body) = serde_json::to_string_pretty(map) { let _ = std::fs::write(&file, body); }
}
/// Human label for a section key.
pub fn music_section_label(key: &str) -> &'static str {
    match key {
        "podcasts"   => "Podcasts",
        "audiobooks" => "Audiobooks",
        "radio"      => "Radio",
        "youtube"    => "YouTube",
        _            => "My Music",
    }
}
/// Section key for a human label (inverse of `music_section_label`).
pub fn music_section_key(label: &str) -> &'static str {
    match label {
        "Podcasts"   => "podcasts",
        "Audiobooks" => "audiobooks",
        "Radio"      => "radio",
        "YouTube"    => "youtube",
        _            => "mymusic",
    }
}
/// Persist a folder → section assignment (settings dropdown / chip both use this).
/// Keys are stored WITHOUT trailing separators — folder pickers hand back
/// "/x/books/" while everything downstream (track_meta.folder, Path::parent)
/// uses "/x/books"; a slashed key made the audiobook flag pass match nothing.
pub fn set_folder_section(folder: &str, key: &str) {
    let f = folder.trim_end_matches(['/', '\\']);
    let f = if f.is_empty() { folder } else { f };
    let mut map = load_folder_sections();
    // Drop any older slashed twin of the same folder so one entry survives.
    map.retain(|k, _| k.trim_end_matches(['/', '\\']) != f);
    map.insert(f.to_string(), key.to_string());
    save_folder_sections(&map);
}
/// Advance a folder's section tag to the next of the 5 and persist it.
pub fn cycle_folder_section(folder: &str) -> String {
    let mut map = load_folder_sections();
    let cur = map.get(folder).map(|s| s.as_str()).unwrap_or("mymusic");
    let idx = MUSIC_SECTIONS.iter().position(|s| *s == cur).unwrap_or(0);
    let next = MUSIC_SECTIONS[(idx + 1) % MUSIC_SECTIONS.len()].to_string();
    map.insert(folder.to_string(), next.clone());
    save_folder_sections(&map);
    next
}

pub fn fuzzy_score(q: &str, hay: &str) -> Option<f32> {
    let hay: Vec<char> = hay.chars().collect();
    let mut hi = 0usize;
    let mut score = 0.0f32;
    let mut run = 0.0f32;
    for qc in q.chars() {
        let mut found = false;
        while hi < hay.len() {
            if hay[hi] == qc {
                run += 1.0;
                score += run + (1.0 / (hi as f32 + 1.0));
                hi += 1;
                found = true;
                break;
            }
            run = 0.0;
            hi += 1;
        }
        if !found { return None; }
    }
    Some(score)
}

thread_local! {
    /// OS media-control handle, held on the UI thread so playback-state updates
    /// (which must mirror real state or the DE sends the wrong key event) and the
    /// initial registration share one connection.
    pub static MEDIA_CONTROLS: std::cell::RefCell<Option<souvlaki::MediaControls>> = const { std::cell::RefCell::new(None) };
    /// Last position we told the OS about. `media_set_playing` is called from
    /// the toggle paths, which know the new state but not the clock, and a
    /// `PlaybackStatus` change that drops `Position` back to zero makes every
    /// remote's scrubber jump to the start on a pause.
    static MEDIA_POS: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

/// Publish the current track to the OS media surface — MPRIS on Linux, SMTC on
/// Windows, `MPNowPlayingInfoCenter` on macOS.
///
/// Without this the app registers as a player and gets the media keys, but the
/// desktop's own media applet has nothing to draw: no title, no artist, no
/// artwork, no length — which is why a phone mirroring the session showed a
/// generic note where every other player shows a cover.
///
/// Only call this when the track actually changed — the caller owns that check,
/// because on this side of it the artwork has already been resolved and that is
/// the part worth not repeating.
pub fn media_set_track(
    title: &str,
    artist: &str,
    album: &str,
    cover: Option<&str>,
    dur_secs: f32,
) {
    // Remote artwork (a podcast or YouTube thumbnail) is already a URL and must
    // stay one; a local file has to be handed over as `file://` or clients
    // ignore it.
    let cover_url = cover.map(|p| {
        if p.starts_with("http://") || p.starts_with("https://") || p.starts_with("file://") {
            p.to_string()
        } else {
            format!("file://{p}")
        }
    });
    MEDIA_CONTROLS.with(|c| {
        if let Some(ctrl) = c.borrow_mut().as_mut() {
            let _ = ctrl.set_metadata(souvlaki::MediaMetadata {
                title: (!title.is_empty()).then_some(title),
                artist: (!artist.is_empty()).then_some(artist),
                album: (!album.is_empty()).then_some(album),
                cover_url: cover_url.as_deref(),
                // A live stream has no length, and publishing a zero one gives
                // remotes a scrubber that looks broken rather than absent.
                duration: (dur_secs > 0.0)
                    .then(|| std::time::Duration::from_secs_f32(dur_secs)),
            });
        }
    });
}

/// Mirror the real playback state to the OS media surface so the desktop sends
/// the correct Play vs Pause event for the next media-key press.
pub fn media_set_playing(playing: bool) {
    media_set_progress(playing, MEDIA_POS.with(|c| c.get()));
}

/// The same, carrying where we are in the track — what fills in the elapsed
/// clock and the scrubber on the OS applet and on anything mirroring it.
pub fn media_set_progress(playing: bool, pos_secs: f32) {
    MEDIA_POS.with(|c| c.set(pos_secs.max(0.0)));
    let progress =
        Some(souvlaki::MediaPosition(std::time::Duration::from_secs_f32(pos_secs.max(0.0))));
    MEDIA_CONTROLS.with(|c| {
        if let Some(ctrl) = c.borrow_mut().as_mut() {
            let st = if playing {
                souvlaki::MediaPlayback::Playing { progress }
            } else {
                souvlaki::MediaPlayback::Paused { progress }
            };
            let _ = ctrl.set_playback(st);
        }
    });
}

/// Echo a volume change back to MPRIS.
///
/// The protocol requires it: a client that writes `Volume` gets no confirmation
/// until the player publishes the value it settled on, so a slider dragged on a
/// remote springs back without this. MPRIS-only — SMTC and the macOS info centre
/// carry no volume at all, and souvlaki does not define the call there.
pub fn media_set_volume(vol_0_1: f64) {
    #[cfg(all(unix, not(target_os = "macos")))]
    MEDIA_CONTROLS.with(|c| {
        if let Some(ctrl) = c.borrow_mut().as_mut() {
            let _ = ctrl.set_volume(vol_0_1.clamp(0.0, 1.0));
        }
    });
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    let _ = vol_0_1;
}
