//! Shared app infrastructure for the section-wiring crates.
//!
//! Owns the per-section SQLite pool cache (`pool_for`), the data-directory
//! helpers, and the bundled-binary probes — everything a section crate needs
//! that isn't section-specific, without depending on `tulipix-app`.

use anyhow::Result;
pub mod mpv_ipc;
pub mod player;
use tulipix_core::proc::NoWindow;
use std::path::PathBuf;
use std::sync::OnceLock;

static PHOTOS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static VIDEOS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static MUSIC_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
static BOOKS_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
static CLOUD_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
// Music sub-sections split out of music.db (no items FK — self-contained).
static PODCASTS_POOL: OnceLock<sqlx::SqlitePool> = OnceLock::new();
static RADIO_POOL:    OnceLock<sqlx::SqlitePool> = OnceLock::new();
static YOUTUBE_POOL:  OnceLock<sqlx::SqlitePool> = OnceLock::new();
// Tools job queue (tools.db) — shared by the GUI Tools section + CLI.
static TOOLS_POOL:    OnceLock<sqlx::SqlitePool> = OnceLock::new();

/// Open (or return the cached) SQLite pool for a section, applying its schema
/// on first open. Both the GUI and CLI go through here so every front-end sees
/// the same DB + migrations.
pub async fn pool_for(section: &str) -> Result<sqlx::SqlitePool> {
    let cache = match section {
        "photos" => &PHOTOS_POOL,
        "videos" => &VIDEOS_POOL,
        "music"  => &MUSIC_POOL,
        "books"  => &BOOKS_POOL,
        "cloud"  => &CLOUD_POOL,
        "podcasts" => &PODCASTS_POOL,
        "radio"    => &RADIO_POOL,
        "youtube"  => &YOUTUBE_POOL,
        "tools"    => &TOOLS_POOL,
        _ => anyhow::bail!("unknown section"),
    };
    if let Some(p) = cache.get() { return Ok(p.clone()); }
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
        "books"  => tulipix_books::schema::apply(&pool).await?,
        "cloud"  => tulipix_cloud::schema::apply(&pool).await?,
        "tools"  => tulipix_tools::schema::apply(&pool).await?,
        _ => {}
    }
    let _ = cache.set(pool.clone());
    Ok(pool)
}

/// One-time migration: move `tables` (each `(name, explicit_column_list)`) out
/// of the legacy shared `music.db` into the freshly-opened section `dest` pool.
/// Idempotent: no-ops once the source tables are gone.
async fn migrate_split_from_music(dest: &sqlx::SqlitePool, tables: &[(&str, &str)]) {
    let Some(music_path) = tulipix_core::paths::db_path("music") else { return; };
    if !music_path.exists() { return; }
    let Ok(mut conn) = dest.acquire().await else { return; };
    if sqlx::query(&format!("ATTACH DATABASE '{}' AS legacy", music_path.display()))
        .execute(&mut *conn).await.is_err() { return; }
    let mut moved_any = false;
    for (table, cols) in tables {
        let exists: Option<String> = sqlx::query_scalar(
            "SELECT name FROM legacy.sqlite_master WHERE type='table' AND name = ?")
            .bind(*table).fetch_optional(&mut *conn).await.ok().flatten();
        if exists.is_none() { continue; }
        moved_any = true;
        let _ = sqlx::query(&format!(
            "INSERT OR IGNORE INTO {table} ({cols}) SELECT {cols} FROM legacy.{table}"))
            .execute(&mut *conn).await;
    }
    if moved_any {
        for (table, _) in tables.iter().rev() {
            let _ = sqlx::query(&format!("DROP TABLE IF EXISTS legacy.{table}")).execute(&mut *conn).await;
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

// ---- Cross-section image helpers ------------------------------------------
// Shared by the Cloud preview, the Photos viewer/properties panels, and the
// editor curve grid — so they live here rather than in any one section crate.

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

/// Decode `path` and render its histogram; an empty 256×100 buffer on failure.
pub fn histogram_image(path: &std::path::Path) -> slint::Image {
    let Ok(img) = image::open(path) else {
        use slint::{Rgba8Pixel, SharedPixelBuffer};
        return slint::Image::from_rgba8(SharedPixelBuffer::<Rgba8Pixel>::new(256, 100));
    };
    slint::Image::from_rgba8(histogram_buf(&img))
}

/// Render a 256×100 additive RGB histogram buffer from an already-decoded
/// image. Shared by the viewer/properties panels and the editor curve grid.
pub fn histogram_buf(img: &image::DynamicImage) -> slint::SharedPixelBuffer<slint::Rgba8Pixel> {
    use slint::{Rgba8Pixel, SharedPixelBuffer};
    const W: usize = 256;
    const H: usize = 100;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(W as u32, H as u32);
    let px = buf.make_mut_slice();
    for p in px.iter_mut() { *p = Rgba8Pixel { r: 0, g: 0, b: 0, a: 0 }; }

    let small = img.thumbnail(256, 256).to_rgb8();
    let (mut rh, mut gh, mut bh) = ([0u32; 256], [0u32; 256], [0u32; 256]);
    for p in small.pixels() {
        rh[p[0] as usize] += 1; gh[p[1] as usize] += 1; bh[p[2] as usize] += 1;
    }
    let maxv = rh.iter().chain(&gh).chain(&bh).copied().max().unwrap_or(1).max(1);
    for x in 0..W {
        for (count, (cr, cg, cb)) in [
            (rh[x], (210u16, 40, 40)),
            (gh[x], (40, 200, 90)),
            (bh[x], (50, 120, 230)),
        ] {
            let bar = ((count as f64 / maxv as f64) * (H as f64 - 1.0)).round() as usize;
            for y in (H - bar)..H {
                let idx = y * W + x;
                let c = px[idx];
                px[idx] = Rgba8Pixel {
                    r: (c.r as u16 + cr).min(255) as u8,
                    g: (c.g as u16 + cg).min(255) as u8,
                    b: (c.b as u16 + cb).min(255) as u8,
                    a: 235,
                };
            }
        }
    }
    buf
}

// ---- Watched folders + time (shared by every media section) ----------------
/// JSON file holding the list of watched root folders, so libraries survive
/// restarts (the grids re-scan from these on launch).
pub fn watched_folders_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

pub fn load_watched_folders() -> Vec<PathBuf> {
    let Some(p) = watched_folders_path() else { return Vec::new(); };
    let Ok(body) = std::fs::read_to_string(&p) else { return Vec::new(); };
    serde_json::from_str::<Vec<String>>(&body)
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

/// Append `add` to `existing` unless already present (path-equality).
pub fn merge_watched(mut existing: Vec<PathBuf>, add: &std::path::Path) -> Vec<PathBuf> {
    if !existing.iter().any(|p| p == add) {
        existing.push(add.to_path_buf());
    }
    existing
}

pub fn save_watched_folders(folders: &[PathBuf]) {
    if let Some(p) = watched_folders_path() {
        let list: Vec<String> = folders.iter().map(|p| p.display().to_string()).collect();
        if let Ok(body) = serde_json::to_string_pretty(&list) {
            let _ = std::fs::write(p, body);
        }
    }
}

/// Add `dir` to the watched-folders set (idempotent) and persist. Returns true
/// if it was newly added.
pub fn add_watched_folder(dir: &std::path::Path) -> bool {
    let existing = load_watched_folders();
    let had = existing.iter().any(|p| p == dir);
    if !had {
        save_watched_folders(&merge_watched(existing, dir));
        // Attach the new folder to the running FS watcher so deletes/renames
        // there update the library live, without waiting for a restart.
        tulipix_core::watcher::watch_path(dir);
    }
    !had
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
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
/// Kill the headless music mpv if one is running.
pub fn kill_music_proc() {
    MUSIC_GEN.fetch_add(1, std::sync::atomic::Ordering::SeqCst); // suppress auto-advance
    if let Ok(mut g) = music_proc().lock() {
        if let Some(mut child) = g.take() { let _ = child.kill(); let _ = child.wait(); }
    }
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


/// Send a single JSON command to the live music mpv over its IPC socket.
pub fn music_ipc(args: &[&str]) {
    use std::io::Write;
    let Some(sock) = music_sock().lock().ok().and_then(|g| g.clone()) else { return; };
    let payload = format!("{{\"command\":[{}]}}\n",
        args.iter().map(|a| {
            // numbers/bools pass through; everything else is JSON-quoted.
            if a.parse::<f64>().is_ok() || **a == *"true" || **a == *"false" { a.to_string() }
            else { format!("\"{a}\"") }
        }).collect::<Vec<_>>().join(","));
    if let Ok(mut s) = mpv_ipc::connect(&sock) {
        let _ = s.write_all(payload.as_bytes());
    }
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

/// Launch a video in an external mpv window with resume + watch-progress
/// writeback over the JSON IPC socket. Runs entirely off the UI thread so the
/// app never blocks on playback (np.p3.player — windowed path).
pub fn spawn_mpv_windowed(path: PathBuf, resume: Option<f64>, item_id: Option<i64>) {
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
        // GLSL upscale chain (np.p3.player.upscale) — opt-in + shaders present.
        let s = tulipix_core::settings::Settings::load().unwrap_or_default();
        if s.flag("playback.upscale", false) {
            if let Some((chain, _)) = anime4k_shader_args() {
                cmd.arg(format!("--glsl-shaders={chain}"));
            }
        }
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
pub fn set_folder_section(folder: &str, key: &str) {
    let mut map = load_folder_sections();
    map.insert(folder.to_string(), key.to_string());
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
}

/// Mirror the real playback state to the OS media surface so the desktop sends
/// the correct Play vs Pause event for the next media-key press.
pub fn media_set_playing(playing: bool) {
    MEDIA_CONTROLS.with(|c| {
        if let Some(ctrl) = c.borrow_mut().as_mut() {
            let st = if playing {
                souvlaki::MediaPlayback::Playing { progress: None }
            } else {
                souvlaki::MediaPlayback::Paused { progress: None }
            };
            let _ = ctrl.set_playback(st);
        }
    });
}


#[cfg(test)]
mod watched_tests {
    use super::merge_watched;
    use std::path::{Path, PathBuf};

    #[test]
    fn merge_watched_dedups_and_appends() {
        let out = merge_watched(vec![PathBuf::from("/a")], Path::new("/b"));
        assert_eq!(out, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        let same = merge_watched(vec![PathBuf::from("/a")], Path::new("/a"));
        assert_eq!(same, vec![PathBuf::from("/a")]);
    }
}
