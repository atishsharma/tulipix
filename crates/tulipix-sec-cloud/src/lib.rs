//! Cloud section wiring extracted from tulipix-app: rclone remote CRUD, the
//! remote-tree browser, Sync & Tools, upload/download with progress bars,
//! mount/unmount — every `window.on_cloud_*` callback. tulipix-app calls
//! [`wire`] once at startup.

use slint::{ComponentHandle, Model};
use std::path::PathBuf;
use tulipix_ui::*;
use tulipix_common::{pool_for, spawn_mpv_windowed, histogram_image, load_watched_folders};
use tulipix_core::proc::NoWindow;
use tulipix_sec_photos::format_exif;

mod cloud;

/// Register every cloud callback on `window` and do the initial remote refresh.
pub fn wire(window: &MainWindow) {
    let w = window.as_weak();
    window.on_cloud_refresh(move || { cloud_refresh_remotes(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_connect_open_dialog(move || {
        if let Some(w) = w.upgrade() {
            w.set_cloud_form_name("".into());
            w.set_cloud_form_backend("drive".into());
            w.set_cloud_form_opts("".into());
            w.set_cloud_status("".into());
            w.set_cloud_connect_open(true);
        }
    });
    let w = window.as_weak();
    window.on_cloud_connect_cancel(move || { if let Some(w) = w.upgrade() { w.set_cloud_connect_open(false); } });
    let w = window.as_weak();
    window.on_cloud_connect_submit(move || {
        let Some(w0) = w.upgrade() else { return; };
        cloud_connect(w.clone(), w0.get_cloud_form_name().to_string(),
            w0.get_cloud_form_backend().to_string(), w0.get_cloud_form_opts().to_string());
    });
    let w = window.as_weak();
    window.on_cloud_remote_open(move |name| { cloud_open_remote(w.clone(), name.to_string()); });
    let w = window.as_weak();
    window.on_cloud_go_home(move || { cloud_go_home(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_remote_delete(move |name| { cloud_delete(w.clone(), name.to_string()); });
    let w = window.as_weak();
    window.on_cloud_entry_activate(move |idx| {
        let Some(w0) = w.upgrade() else { return; };
        let entries = w0.get_cloud_entries();
        if let Some(e) = entries.row_data(idx as usize) {
            if e.is_dir { cloud_into(w.clone(), e.name.to_string()); }
            else {
                let (remote, rel, name) = cloud_entry_target(&e.name);
                cloud_open_file(w.clone(), remote, rel, name);
            }
        }
    });
    let w = window.as_weak();
    window.on_cloud_ctx_action(move |idx, action| {
        let Some(w0) = w.upgrade() else { return; };
        let entries = w0.get_cloud_entries();
        let Some(e) = entries.row_data(idx as usize) else { return; };
        let (remote, rel, name) = cloud_entry_target(&e.name);
        match action.as_str() {
            "open" => {
                if e.is_dir { cloud_into(w.clone(), name); }
                else { cloud_open_file(w.clone(), remote, rel, name); }
            }
            "openwith" => cloud_open_mounted(w.clone(), remote, rel, false),
            "reveal"   => cloud_open_mounted(w.clone(), remote, rel, true),
            "download" => cloud_download(w.clone(), remote, rel, name),
            "copypath" => w0.set_cloud_status(tulipix_cloud::context::rclone_url(&remote, &rel).into()),
            "preview"  => cloud_preview(w.clone(), remote, rel, name),
            "import"   => cloud_import(w.clone(), remote, rel, name),
            "sharelink"=> cloud_share_link(w.clone(), remote, rel),
            "delete"   => cloud_delete_entry(w.clone(), remote, rel, e.is_dir),
            _ => {}
        }
    });
    let w = window.as_weak();
    window.on_cloud_browse_up(move || { cloud_up(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_search(move |_q| {
        // `cloud-query` is bound in the .slint; filter the cached listing in place.
        if let Some(w0) = w.upgrade() { cloud_set_filtered(&w0); }
    });
    let w = window.as_weak();
    window.on_cloud_search_deep(move || { cloud_deep_search(w.clone()); });

    // ── Cloud: Sync & Tools drawer (np.p5.cloud.*) ──
    let w = window.as_weak();
    window.on_cloud_tools_toggle(move || {
        if let Some(w0) = w.upgrade() {
            let open = !w0.get_cloud_tools_open();
            w0.set_cloud_tools_open(open);
            if open { cloud_refresh_jobs(w.clone()); cloud_refresh_usage(w.clone()); }
        }
    });
    let w = window.as_weak();
    window.on_cloud_run_sync(move || { cloud_run_sync(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_save_job(move || { cloud_save_job(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_job_toggle(move |id| { cloud_job_toggle(w.clone(), id); });
    let w = window.as_weak();
    window.on_cloud_job_delete(move |id| { cloud_job_delete(w.clone(), id); });
    let w = window.as_weak();
    window.on_cloud_job_run(move |id| { cloud_job_run(w.clone(), id); });
    let w = window.as_weak();
    window.on_cloud_run_copy(move || { cloud_run_copy(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_run_verify(move || { cloud_run_verify(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_run_dedupe(move || { cloud_run_dedupe(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_refresh_usage(move || { cloud_refresh_usage(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_reload_xfers(move || { cloud_reload_xfers(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_xfer_retry(move |id| { cloud_retry_xfer(w.clone(), id as i64); });
    let w = window.as_weak();
    window.on_cloud_xfer_clear(move |id| { cloud_clear_xfer(w.clone(), id as i64); });
    let w = window.as_weak();
    window.on_cloud_xfer_clear_all(move || { cloud_clear_all_xfers(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_crypt_open_dialog(move || {
        if let Some(w0) = w.upgrade() {
            let rem = w0.get_cloud_active_remote().to_string();
            w0.set_cloud_crypt_name(if rem.is_empty() { "vault".into() } else { format!("{rem}-crypt").into() });
            w0.set_cloud_crypt_wrapped(if rem.is_empty() { "".into() } else { format!("{rem}:").into() });
            w0.set_cloud_crypt_pass("".into());
            w0.set_cloud_crypt_pass2("".into());
            w0.set_cloud_crypt_open(true);
        }
    });
    let w = window.as_weak();
    window.on_cloud_crypt_submit(move || { cloud_crypt_submit(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_crypt_cancel(move || { if let Some(w0) = w.upgrade() { w0.set_cloud_crypt_open(false); } });
    let w = window.as_weak();
    window.on_cloud_mountopts_open_dialog(move |name| {
        if let Some(w0) = w.upgrade() {
            w0.set_cloud_mountopts_remote(name);
            w0.set_cloud_mountopts_open(true);
        }
    });
    let w = window.as_weak();
    window.on_cloud_mountopts_submit(move || { cloud_mountopts_submit(w.clone()); });
    let w = window.as_weak();
    window.on_cloud_mountopts_cancel(move || { if let Some(w0) = w.upgrade() { w0.set_cloud_mountopts_open(false); } });
    let w = window.as_weak();
    window.on_cloud_unmount_remote(move |name| { cloud_unmount_remote(w.clone(), name.to_string()); });
    let w = window.as_weak();
    window.on_cloud_upload_pick(move |kind| { cloud_upload_pick(w.clone(), kind.to_string()); });
    cloud_refresh_remotes(window.as_weak());
}

static CLOUD_REMOTE: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn cloud_remote() -> &'static std::sync::Mutex<String> {
    CLOUD_REMOTE.get_or_init(|| std::sync::Mutex::new(String::new()))
}
static CLOUD_PATH: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
fn cloud_path() -> &'static std::sync::Mutex<String> {
    CLOUD_PATH.get_or_init(|| std::sync::Mutex::new(String::new()))
}
// Full (unfiltered) listing of the current folder: (name, is_dir, size, modified, kind).
static CLOUD_ALL: std::sync::OnceLock<std::sync::Mutex<Vec<(String, bool, String, String, String)>>> = std::sync::OnceLock::new();
fn cloud_all() -> &'static std::sync::Mutex<Vec<(String, bool, String, String, String)>> {
    CLOUD_ALL.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}

/// Classify a cloud entry into a thumb-tile kind by extension.
fn cloud_entry_kind(name: &str, is_dir: bool) -> &'static str {
    if is_dir { return "folder"; }
    let ext = name.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if cloud_is_image(&ext) { "image" }
    else if cloud_is_video(&ext) { "video" }
    else if cloud_is_audio(&ext) { "audio" }
    else if matches!(ext.as_str(), "zip"|"rar"|"7z"|"tar"|"gz"|"xz"|"bz2"|"zst") { "archive" }
    else if matches!(ext.as_str(), "pdf"|"txt"|"md"|"doc"|"docx"|"odt"|"rtf"|"epub"|"json"|"csv"|"xls"|"xlsx") { "doc" }
    else { "file" }
}

/// Rebuild the visible `cloud-entries` model from the cached full listing,
/// applying the header search filter (case-insensitive name match). The `index`
/// is re-enumerated to match the filtered model so entry-activate stays correct.
fn cloud_set_filtered(w: &MainWindow) {
    let q = w.get_cloud_query().to_string().trim().to_lowercase();
    let rows: Vec<CloudEntry> = cloud_all()
        .lock()
        .map(|g| {
            g.iter()
                .filter(|(name, _, _, _, _)| q.is_empty() || name.to_lowercase().contains(&q))
                .enumerate()
                .map(|(i, (name, is_dir, size, modified, kind))| CloudEntry {
                    name: name.clone().into(),
                    is_dir: *is_dir,
                    size: size.clone().into(),
                    modified: modified.clone().into(),
                    index: i as i32,
                    kind: kind.clone().into(),
                })
                .collect()
        })
        .unwrap_or_default();
    w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(rows)));
}


/// `rclone config dump` → mirror into cloud.db → set the remotes model.
/// Is `name` currently mounted (live child + a real mountpoint)?
fn cloud_remote_mounted(name: &str) -> bool {
    cloud_mounts().lock().ok()
        .and_then(|g| g.get(name).map(|(mp, _)| cloud_is_mounted(mp)))
        .unwrap_or(false)
}

fn cloud_refresh_remotes(weak: slint::Weak<MainWindow>) {
    let _ = weak.upgrade_in_event_loop(|w| w.set_cloud_refreshing(true));
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let ok = tokio::task::spawn_blocking(cloud::available).await.unwrap_or(false);
        let dump = if ok {
            tokio::task::spawn_blocking(|| cloud::run(&tulipix_cloud::remotes::dump_args()))
                .await.ok().and_then(|r| r.ok())
        } else { None };
        let parsed = dump.as_deref().map(tulipix_cloud::remotes::parse_dump).unwrap_or_default();
        if let Ok(pool) = pool_for("cloud").await {
            for (name, backend) in &parsed {
                let _ = tulipix_cloud::remotes::upsert(&pool, name, backend, false).await;
            }
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_rclone_ok(ok);
            let rows: Vec<CloudRemote> = parsed.into_iter().map(|(name, backend)| CloudRemote {
                mounted: cloud_remote_mounted(&name),
                name: name.into(), backend: backend.into(),
            }).collect();
            w.set_cloud_remotes(slint::ModelRc::new(slint::VecModel::from(rows)));
            // keep the active remote's mount badge in sync
            let active = w.get_cloud_active_remote().to_string();
            if !active.is_empty() { w.set_cloud_active_mounted(cloud_remote_mounted(&active)); }
            if !ok {
                w.set_cloud_status("rclone not found — install rclone or bundle it in resources/bin".into());
            }
            w.set_cloud_refreshing(false);
        });
        if ok { cloud_startup_mounts().await; }
    });
}

/// Mount every remote flagged mount-on-startup, once per process
/// (np.p5.cloud.mount-cache).
async fn cloud_startup_mounts() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) { return; }
    let pool = match pool_for("cloud").await { Ok(p) => p, Err(_) => return };
    let want = tulipix_cloud::mount::startup_mounts(&pool).await.unwrap_or_default();
    for (id, _path) in want {
        if let Ok(Some(name)) = tulipix_cloud::remotes::name_of(&pool, id).await {
            let _ = tokio::task::spawn_blocking(move || cloud_mount_ensure(&name)).await;
        }
    }
}

/// Create a remote non-interactively from the connect dialog fields.
fn cloud_connect(weak: slint::Weak<MainWindow>, name: String, backend: String, opts: String) {
    if name.trim().is_empty() || backend.trim().is_empty() { return; }
    // Parse "key=value" lines / commas.
    let pairs: Vec<(String, String)> = opts
        .split(['\n', ','])
        .filter_map(|kv| {
            let kv = kv.trim();
            if kv.is_empty() { return None; }
            let (k, v) = kv.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let name_c = name.clone();
        let res = tokio::task::spawn_blocking(move || {
            let refs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            cloud::run(&tulipix_cloud::remotes::create_args(&name_c, &backend, &refs))
        }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let _ = weak.upgrade_in_event_loop(move |w| {
            match res {
                Ok(_) => {
                    w.set_cloud_connect_open(false);
                    w.set_cloud_form_name("".into());
                    w.set_cloud_form_opts("".into());
                    w.set_cloud_status(format!("Connected '{name}'").into());
                    cloud_refresh_remotes(w.as_weak());
                }
                Err(e) => w.set_cloud_status(format!("Connect failed: {e}").into()),
            }
        });
    });
}

fn cloud_delete(weak: slint::Weak<MainWindow>, name: String) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let name_c = name.clone();
        let _ = tokio::task::spawn_blocking(move || cloud::run(&tulipix_cloud::remotes::delete_args(&name_c))).await;
        if let Ok(pool) = pool_for("cloud").await {
            let _ = tulipix_cloud::remotes::remove(&pool, &name).await;
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            if cloud_remote().lock().map(|g| *g == name).unwrap_or(false) {
                if let Ok(mut g) = cloud_remote().lock() { g.clear(); }
                w.set_cloud_active_remote("".into());
                w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<CloudEntry>::new())));
            }
            w.set_cloud_status(format!("Removed '{name}'").into());
            cloud_refresh_remotes(w.as_weak());
        });
    });
}

/// `rclone lsjson <remote>:<path>` → file table for the active remote+path.
fn cloud_browse(weak: slint::Weak<MainWindow>) {
    let remote = cloud_remote().lock().map(|g| g.clone()).unwrap_or_default();
    if remote.is_empty() { return; }
    let path = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let (remote_c, path_c) = (remote.clone(), path.clone());
        let out = tokio::task::spawn_blocking(move || {
            cloud::run(&tulipix_cloud::browse::lsjson_args(&remote_c, &path_c))
        }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let _ = weak.upgrade_in_event_loop(move |w| {
            match out {
                Ok(json) => {
                    let mut entries = tulipix_cloud::browse::parse_lsjson(&json).unwrap_or_default();
                    tulipix_cloud::browse::sort_entries(&mut entries, tulipix_cloud::browse::SortKey::Name);
                    // Cache the full listing so the header search can filter it
                    // in place without re-hitting rclone.
                    let all: Vec<(String, bool, String, String, String)> = entries.into_iter().map(|e| {
                        let kind = cloud_entry_kind(&e.name, e.is_dir).to_string();
                        (
                            e.name,
                            e.is_dir,
                            if e.is_dir { "—".to_string() } else { cloud_human_size(e.size.max(0) as u64) },
                            e.mod_time.chars().take(10).collect::<String>(),
                            kind,
                        )
                    }).collect();
                    if let Ok(mut g) = cloud_all().lock() { *g = all; }
                    cloud_set_filtered(&w);
                    w.set_cloud_status("".into());
                }
                Err(e) => {
                    if let Ok(mut g) = cloud_all().lock() { g.clear(); }
                    w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<CloudEntry>::new())));
                    w.set_cloud_status(format!("Browse failed: {e}").into());
                }
            }
        });
    });
}

/// Compact size for the Cloud browser — units only (KB/MB/GB), no raw-byte
/// suffix (the shared `human_size` appends "(N bytes)" which wastes column
/// space here). Sub-KB files collapse to "< 1 KB".
fn cloud_human_size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1_073_741_824.0 { format!("{:.2} GB", b / 1_073_741_824.0) }
    else if b >= 1_048_576.0 { format!("{:.1} MB", b / 1_048_576.0) }
    else if b >= 1024.0 { format!("{:.1} KB", b / 1024.0) }
    else if bytes == 0 { "—".to_string() }
    else { "< 1 KB".to_string() }
}

/// Whole-remote search: `rclone lsjson --recursive` of the active remote+path,
/// keep objects whose path/name contains the query, and show them with their
/// full sub-path as the name (so open/descend still resolve correctly). Clearing
/// the query reloads the current folder. Triggered by the magnifier button, not
/// per-keystroke — the recursive list can be expensive on large remotes.
fn cloud_deep_search(weak: slint::Weak<MainWindow>) {
    let remote = cloud_remote().lock().map(|g| g.clone()).unwrap_or_default();
    if remote.is_empty() { return; }
    let Some(w0) = weak.upgrade() else { return; };
    let q = w0.get_cloud_query().to_string().trim().to_lowercase();
    if q.is_empty() { cloud_browse(weak); return; }
    let path = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    w0.set_cloud_refreshing(true);
    w0.set_cloud_status(format!("Searching all of {remote}…").into());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let (remote_c, path_c) = (remote.clone(), path.clone());
        let out = tokio::task::spawn_blocking(move || {
            cloud::run(&tulipix_cloud::browse::lsjson_recursive_args(&remote_c, &path_c))
        }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_refreshing(false);
            match out {
                Ok(json) => {
                    let entries = tulipix_cloud::browse::parse_lsjson(&json).unwrap_or_default();
                    let rows: Vec<CloudEntry> = entries.into_iter()
                        .filter(|e| e.path.to_lowercase().contains(&q) || e.name.to_lowercase().contains(&q))
                        .enumerate()
                        .map(|(i, e)| {
                            let kind = cloud_entry_kind(&e.name, e.is_dir).to_string();
                            CloudEntry {
                                name: e.path.clone().into(),
                                is_dir: e.is_dir,
                                size: if e.is_dir { "—".into() } else { cloud_human_size(e.size.max(0) as u64).into() },
                                modified: e.mod_time.chars().take(10).collect::<String>().into(),
                                index: i as i32,
                                kind: kind.into(),
                            }
                        }).collect();
                    let n = rows.len();
                    w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(rows)));
                    w.set_cloud_status(format!("{n} match{} for “{q}” across {remote}",
                        if n == 1 { "" } else { "es" }).into());
                }
                Err(e) => w.set_cloud_status(format!("Search failed: {e}").into()),
            }
        });
    });
}

/// Return to the remote list ("home") — clears the active remote + path.
fn cloud_go_home(weak: slint::Weak<MainWindow>) {
    if let Ok(mut g) = cloud_remote().lock() { g.clear(); }
    if let Ok(mut g) = cloud_path().lock() { g.clear(); }
    if let Ok(mut g) = cloud_all().lock() { g.clear(); }
    if let Some(w) = weak.upgrade() {
        w.set_cloud_active_remote("".into());
        w.set_cloud_active_mounted(false);
        w.set_cloud_path("".into());
        w.set_cloud_entries(slint::ModelRc::new(slint::VecModel::from(Vec::<CloudEntry>::new())));
        w.set_cloud_status("".into());
    }
}

fn cloud_open_remote(weak: slint::Weak<MainWindow>, name: String) {
    if let Ok(mut g) = cloud_remote().lock() { *g = name.clone(); }
    if let Ok(mut g) = cloud_path().lock() { g.clear(); }
    let mounted = cloud_remote_mounted(&name);
    if let Some(w) = weak.upgrade() {
        w.set_cloud_active_remote(name.into());
        w.set_cloud_active_mounted(mounted);
        w.set_cloud_path("".into());
    }
    cloud_browse(weak);
}

fn cloud_into(weak: slint::Weak<MainWindow>, dir: String) {
    if let Ok(mut g) = cloud_path().lock() {
        if g.is_empty() { *g = dir; } else { *g = format!("{}/{}", g, dir); }
    }
    let p = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    if let Some(w) = weak.upgrade() { w.set_cloud_path(p.into()); }
    cloud_browse(weak);
}

fn cloud_up(weak: slint::Weak<MainWindow>) {
    if let Ok(mut g) = cloud_path().lock() {
        match g.rfind('/') {
            Some(i) => { g.truncate(i); }
            None => g.clear(),
        }
    }
    let p = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    if let Some(w) = weak.upgrade() { w.set_cloud_path(p.into()); }
    cloud_browse(weak);
}

// ── Cloud file open / context actions (mount-on-demand) ─────────────────────

/// (remote, remote-relative path, file name) for the entry `name` in the
/// currently-browsed directory.
fn cloud_entry_target(name: &str) -> (String, String, String) {
    let remote = cloud_remote().lock().map(|g| g.clone()).unwrap_or_default();
    let path = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    let rel = if path.is_empty() { name.to_string() } else { format!("{path}/{name}") };
    (remote, rel, name.to_string())
}

fn cloud_is_video(ext: &str) -> bool {
    matches!(ext, "mp4"|"mkv"|"mov"|"avi"|"webm"|"m4v"|"wmv"|"flv"|"mpg"|"mpeg"|"ts"|"m2ts"|"3gp"|"ogv")
}
fn cloud_is_audio(ext: &str) -> bool {
    matches!(ext, "mp3"|"flac"|"wav"|"aac"|"ogg"|"oga"|"m4a"|"opus"|"wma"|"aiff"|"alac")
}
fn cloud_is_image(ext: &str) -> bool {
    matches!(ext, "jpg"|"jpeg"|"png"|"webp"|"gif"|"bmp"|"tif"|"tiff"|"heic"|"heif"|"avif"|"jxl")
}

// Live `rclone mount` processes keyed by remote name (kept alive + reused).
static CLOUD_MOUNTS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (PathBuf, std::process::Child)>>> = std::sync::OnceLock::new();
fn cloud_mounts() -> &'static std::sync::Mutex<std::collections::HashMap<String, (PathBuf, std::process::Child)>> {
    CLOUD_MOUNTS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Is `mp` an active mountpoint? Linux scans mountinfo; elsewhere a non-empty
/// readable dir is treated as ready (best-effort).
fn cloud_is_mounted(mp: &std::path::Path) -> bool {
    #[cfg(target_os = "linux")]
    {
        let want = mp.to_string_lossy();
        std::fs::read_to_string("/proc/self/mountinfo")
            .map(|s| s.lines().any(|l| l.split(' ').nth(4) == Some(want.as_ref())))
            .unwrap_or(false)
    }
    #[cfg(not(target_os = "linux"))]
    { std::fs::read_dir(mp).map(|mut d| d.next().is_some()).unwrap_or(false) }
}

/// Ensure `remote` is FUSE/WinFsp-mounted; return the local mountpoint.
/// Idempotent — reuses a live mount. Blocking: wrap in spawn_blocking.
fn cloud_mount_ensure(remote: &str) -> anyhow::Result<PathBuf> {
    if let Ok(mut g) = cloud_mounts().lock() {
        if let Some((mp, child)) = g.get_mut(remote) {
            let alive = child.try_wait().ok().flatten().is_none();
            if alive && cloud_is_mounted(mp) { return Ok(mp.clone()); }
            let _ = child.kill();
            // Linux unmounts via fusermount; other platforms drop the child and
            // let WinFsp/macFUSE reap the stale mount on its own.
            #[cfg(target_os = "linux")]
            { let mp = mp.clone(); let _ = std::process::Command::new("fusermount").args(["-u"]).arg(&mp).status(); }
            g.remove(remote);
        }
    }
    let base = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data dir"))?
        .join("mounts").join(remote);
    std::fs::create_dir_all(&base)?;
    let rclone = tulipix_core::thumbs::tool_bin("rclone");
    // Per-remote VFS cache preference (np.p5.cloud.mount-cache); defaults to full.
    let (cache_mode, max_age) = cloud_vfs()
        .lock().ok()
        .and_then(|g| g.get(remote).cloned())
        .unwrap_or_else(|| ("full".to_string(), String::new()));
    let cache = tulipix_cloud::mount::VfsCache::parse(&cache_mode)
        .unwrap_or(tulipix_cloud::mount::VfsCache::Full);
    let args = tulipix_cloud::mount::mount_args_cached(
        remote, &base.to_string_lossy(),
        tulipix_cloud::mount::FsKind::for_os(std::env::consts::OS),
        cache, &max_age);
    let child = std::process::Command::new(&rclone)
        .args(&args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .no_window()
        .spawn()
        .map_err(|e| anyhow::anyhow!("spawn rclone mount: {e}"))?;
    let mut ok = false;
    for _ in 0..150 {
        if cloud_is_mounted(&base) { ok = true; break; }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !ok {
        anyhow::bail!("mount did not come up — is FUSE/WinFsp installed?");
    }
    if let Ok(mut g) = cloud_mounts().lock() { g.insert(remote.to_string(), (base.clone(), child)); }
    Ok(base)
}

/// Best-effort unmount of every live cloud mount (called on app shutdown).
pub fn cloud_unmount_all() {
    if let Ok(mut g) = cloud_mounts().lock() {
        for (_remote, (mp, mut child)) in g.drain() {
            let _ = child.kill();
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("fusermount").args(["-uz"]).arg(&mp).status(); }
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("umount").arg(&mp).status(); }
            let _ = &mp;
        }
    }
}

/// Open a cloud file the natural way: video/audio → mpv, image → viewer,
/// anything else → the OS default app. Mounts the remote first.
fn cloud_open_file(weak: slint::Weak<MainWindow>, remote: String, rel: String, name: String) {
    if let Some(w) = weak.upgrade() { w.set_cloud_status(format!("Opening {name}… (mounting {remote})").into()); }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let remote2 = remote.clone();
        let mp = match tokio::task::spawn_blocking(move || cloud_mount_ensure(&remote2)).await {
            Ok(Ok(mp)) => mp,
            Ok(Err(e)) => {
                tracing::error!(error=%e, "cloud mount");
                let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Mount failed: {e}").into()));
                return;
            }
            Err(_) => return,
        };
        let local = PathBuf::from(tulipix_cloud::context::mount_path(&mp.to_string_lossy(), &rel));
        let ext = local.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
        if cloud_is_video(&ext) || cloud_is_audio(&ext) {
            spawn_mpv_windowed(local, None, None);
            let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Playing {name}").into()));
        } else if cloud_is_image(&ext) {
            let _ = weak.upgrade_in_event_loop(move |w| { cloud_show_image(&w, &local); w.set_cloud_status("".into()); });
        } else {
            let local2 = local.clone();
            let _ = tokio::task::spawn_blocking(move || tulipix_platform::fm::open_default(&local2)).await;
            let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Opened {name}").into()));
        }
    });
}

/// Mount then either reveal the file in the OS file manager or open it with the
/// default app (context-menu "Reveal" / "Open with default app").
fn cloud_open_mounted(weak: slint::Weak<MainWindow>, remote: String, rel: String, reveal: bool) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let remote2 = remote.clone();
        let mp = match tokio::task::spawn_blocking(move || cloud_mount_ensure(&remote2)).await {
            Ok(Ok(mp)) => mp,
            Ok(Err(e)) => { let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Mount failed: {e}").into())); return; }
            Err(_) => return,
        };
        let local = PathBuf::from(tulipix_cloud::context::mount_path(&mp.to_string_lossy(), &rel));
        let _ = tokio::task::spawn_blocking(move || {
            if reveal { tulipix_platform::fm::reveal_in_file_manager(&local) }
            else { tulipix_platform::fm::open_default(&local) }
        }).await;
    });
}

/// Show a single cloud image in the full-screen viewer (total = 1).
fn cloud_show_image(w: &MainWindow, path: &std::path::Path) {
    let img = slint::Image::load_from_path(path).unwrap_or_default();
    let sz = img.size();
    w.set_viewer_image(img);
    w.set_viewer_nat_w(sz.width as i32);
    w.set_viewer_nat_h(sz.height as i32);
    w.set_viewer_label(path.file_name().and_then(|s| s.to_str()).unwrap_or("").into());
    w.set_viewer_index(0);
    w.set_viewer_total(1);
    w.set_viewer_zoom(1.0);
    w.set_viewer_exif(format_exif(path).into());
    w.set_viewer_histogram(histogram_image(path));
    w.set_viewer_open(true);
}

/// `rclone copyto remote:rel ~/Downloads/name` — a one-off download.
/// Scan an rclone `--stats-one-line` log line for its "NN%" progress token.
fn cloud_parse_percent(line: &str) -> Option<u8> {
    let b = line.as_bytes();
    let pos = line.find('%')?;
    let mut start = pos;
    while start > 0 && b[start - 1].is_ascii_digit() { start -= 1; }
    if start < pos { line[start..pos].parse::<u32>().ok().map(|v| v.min(100) as u8) } else { None }
}

/// Run an rclone transfer streaming `--stats` progress into `cloud-op-frac`.
/// Does not toggle `op-active` (the caller frames the whole operation so a
/// multi-file upload shows one continuous bar).
async fn cloud_run_streamed(weak: slint::Weak<MainWindow>, mut args: Vec<String>, label: String) -> anyhow::Result<()> {
    let _ = weak.upgrade_in_event_loop({
        let l = label.clone();
        move |w| { w.set_cloud_op_label(l.into()); w.set_cloud_op_frac(0.0); }
    });
    args.push("--stats".into()); args.push("0.5s".into());
    args.push("--stats-one-line".into()); args.push("-v".into());
    let weak2 = weak.clone();
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        use std::io::BufRead;
        let rclone = tulipix_core::thumbs::tool_bin("rclone");
        let mut child = std::process::Command::new(&rclone)
            .args(&args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .no_window()
            .spawn()?;
        let mut last_err = String::new();
        if let Some(err) = child.stderr.take() {
            for line in std::io::BufReader::new(err).lines().map_while(Result::ok) {
                if let Some(p) = cloud_parse_percent(&line) {
                    let f = (p as f32 / 100.0).clamp(0.0, 1.0);
                    let _ = weak2.upgrade_in_event_loop(move |w| w.set_cloud_op_frac(f));
                }
                let lt = line.trim();
                if lt.contains("ERROR") || lt.contains("Failed") || lt.contains("error") {
                    last_err = lt.to_string();
                }
            }
        }
        let st = child.wait()?;
        if !st.success() {
            if last_err.is_empty() { anyhow::bail!("rclone exited with {st}"); }
            anyhow::bail!("{last_err}");
        }
        Ok(())
    }).await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)))
}

/// `rclone copyto remote:rel ~/Downloads/name` with a live progress bar.
fn cloud_download(weak: slint::Weak<MainWindow>, remote: String, rel: String, name: String) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let dest = PathBuf::from(home).join("Downloads").join(&name);
        let _ = std::fs::create_dir_all(dest.parent().unwrap_or(std::path::Path::new(".")));
        let src = format!("{remote}:{rel}");
        let dest_s = dest.to_string_lossy().into_owned();
        let _ = weak.upgrade_in_event_loop(|w| w.set_cloud_op_active(true));
        let res = cloud_run_streamed(weak.clone(), vec!["copyto".into(), src, dest_s], format!("Downloading {name}")).await;
        let ok = res.is_ok();
        cloud_log("download", &name, ok).await;
        let msg = match res { Ok(_) => format!("Downloaded {name} → ~/Downloads"), Err(e) => format!("Download failed: {e}") };
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_cloud_op_active(false); w.set_cloud_status(msg.into()); });
    });
}

/// Unmount one remote from the system (np.p5.cloud.mount-cache).
fn cloud_unmount_one(remote: &str) {
    if let Ok(mut g) = cloud_mounts().lock() {
        if let Some((mp, mut child)) = g.remove(remote) {
            let _ = child.kill();
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("fusermount").args(["-uz"]).arg(&mp).status(); }
            #[cfg(target_os = "macos")]
            { let _ = std::process::Command::new("umount").arg(&mp).status(); }
            let _ = &mp;
        }
    }
}

fn cloud_unmount_remote(weak: slint::Weak<MainWindow>, name: String) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let name2 = name.clone();
        let _ = tokio::task::spawn_blocking(move || cloud_unmount_one(&name2)).await;
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_active_mounted(false);
            w.set_cloud_status(format!("Unmounted {name}").into());
            cloud_refresh_remotes(w.as_weak());
        });
    });
}

/// Upload picked files / a folder to the active remote at the current path,
/// with a live progress bar (np.p5.cloud upload). `kind` = "files" | "folder".
fn cloud_upload_pick(weak: slint::Weak<MainWindow>, kind: String) {
    let remote = cloud_remote().lock().map(|g| g.clone()).unwrap_or_default();
    if remote.is_empty() { return; }
    let path = cloud_path().lock().map(|g| g.clone()).unwrap_or_default();
    let multi = weak.upgrade().map(|w| w.get_cloud_upload_multi()).unwrap_or(true);
    let transfers = if multi { "8" } else { "1" }.to_string();
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let kind2 = kind.clone();
        let picks: Vec<PathBuf> = tokio::task::spawn_blocking(move || {
            if kind2 == "folder" {
                rfd::FileDialog::new().set_title("Upload a folder").pick_folder().map(|p| vec![p]).unwrap_or_default()
            } else {
                rfd::FileDialog::new().set_title("Upload files").pick_files().unwrap_or_default()
            }
        }).await.unwrap_or_default();
        if picks.is_empty() { return; }
        let destbase = if path.is_empty() { format!("{remote}:") } else { format!("{remote}:{path}") };
        let _ = weak.upgrade_in_event_loop(|w| w.set_cloud_op_active(true));
        let total = picks.len();
        let mut all_ok = true;
        let mut last_err = String::new();
        for (i, p) in picks.into_iter().enumerate() {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("item").to_string();
            let src = p.to_string_lossy().into_owned();
            let is_dir = p.is_dir();
            // `rclone copy` into a destination DIRECTORY for both cases (more
            // forgiving than `copyto`): a file lands inside the current remote
            // path; a folder goes into a same-named subfolder under it.
            let dest = if is_dir {
                if destbase.ends_with(':') { format!("{destbase}{name}") } else { format!("{destbase}/{name}") }
            } else {
                destbase.clone()
            };
            // `--drive-chunk-size 64M`: big single files (e.g. a zip) upload to
            // Google Drive in 8 MiB chunks by default — thousands of round-trips.
            // 64 MiB chunks cut latency drastically. Flag is parsed for every
            // backend but only the drive backend allocates/uses the buffer, so
            // it's harmless (and a no-op) on onedrive/other remotes.
            let args = vec![
                "copy".into(), src, dest,
                "--transfers".into(), transfers.clone(),
                "--drive-chunk-size".into(), "64M".into(),
            ];
            let label = if total > 1 { format!("Uploading {name} ({}/{total})", i + 1) } else { format!("Uploading {name}") };
            let res = cloud_run_streamed(weak.clone(), args, label).await;
            if let Err(e) = res { all_ok = false; last_err = e.to_string(); }
        }
        cloud_log("upload", &format!("{total} item(s) → {remote}:{path}"), all_ok).await;
        // Detect a server-side read-only lock (OneDrive/SharePoint
        // `serviceReadOnly: Database Is Read Only`, GDrive 403, etc.). No client
        // method can write to a read-only store — surface an actionable hint so
        // the user switches remotes instead of re-linking/retrying in vain.
        let read_only = last_err.contains("serviceReadOnly")
            || last_err.to_lowercase().contains("read only")
            || last_err.to_lowercase().contains("read-only");
        let msg = if all_ok {
            format!("Uploaded {total} item(s) to {remote}")
        } else if read_only {
            format!("'{remote}' is read-only (check subscription/quota) — pick a writable remote")
        } else {
            format!("Upload failed: {last_err}")
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_op_active(false);
            w.set_cloud_status(msg.into());
            cloud_browse(w.as_weak());
        });
    });
}

/// Delete a cloud entry (`deletefile` for files, `purge` for dirs), then
/// re-browse the current directory.
fn cloud_delete_entry(weak: slint::Weak<MainWindow>, remote: String, rel: String, is_dir: bool) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let target = format!("{remote}:{rel}");
        let verb = if is_dir { "purge" } else { "deletefile" };
        let res = tokio::task::spawn_blocking(move || cloud::run(&[verb.into(), target]))
            .await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let msg = match res {
            Ok(_) => "Deleted.".to_string(),
            Err(e) => { tracing::error!(error=%e, "cloud delete"); format!("Delete failed: {e}") }
        };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
        cloud_browse(weak);
    });
}

// ── Cloud Sync & Tools handlers (np.p5.cloud.*) ─────────────────────────────

// Per-remote VFS cache preference (mode, max-age) consulted by cloud_mount_ensure.
static CLOUD_VFS: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, (String, String)>>> = std::sync::OnceLock::new();
fn cloud_vfs() -> &'static std::sync::Mutex<std::collections::HashMap<String, (String, String)>> {
    CLOUD_VFS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

fn cloud_interval_secs(s: &str) -> i64 {
    match s { "15m" => 900, "1h" => 3600, "6h" => 21600, "daily" => 86400, _ => 0 }
}
fn cloud_interval_label(secs: i64) -> &'static str {
    match secs { 900 => "every 15m", 3600 => "hourly", 21600 => "every 6h", 86400 => "daily", _ => "manual" }
}

/// Append a transfer-history row (best-effort, fire-and-forget).
async fn cloud_log(kind: &str, detail: &str, ok: bool) {
    if let Ok(pool) = pool_for("cloud").await {
        let _ = tulipix_cloud::jobs::log(&pool, kind, detail, ok).await;
    }
}

/// Build the rclone argv for a sync from the drawer/job fields.
fn cloud_build_sync_args(
    src: &str, dst: &str, bisync: bool, conflict: &str, bwlimit: &str,
    transfers: &str, includes: &str, excludes: &str, max_size: &str, min_age: &str,
) -> Vec<String> {
    use tulipix_cloud::{sync, selective};
    let dir = if bisync { sync::Direction::BiSync } else { sync::Direction::OneWay };
    let bw = sync::parse_bwlimit(bwlimit);
    let mut args = sync::sync_args(dir, src, dst, bw.as_deref());
    let t: u32 = transfers.trim().parse().unwrap_or(0);
    args.extend(sync::concurrency_flags(t, 0));
    if bisync {
        if let Some(c) = sync::Conflict::parse(conflict) { args.extend(c.flags()); }
    }
    let inc = selective::split_patterns(includes);
    let ex = selective::split_patterns(excludes);
    args.extend(selective::filter_flags(&inc, &ex, max_size, min_age));
    args
}

/// Run a sync from the current drawer fields (np.p5.cloud.bisync/throttle/filters).
fn cloud_run_sync(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let (src, dst) = (w.get_cloud_sync_src().to_string(), w.get_cloud_sync_dst().to_string());
    if src.trim().is_empty() || dst.trim().is_empty() {
        w.set_cloud_status("Sync needs a source and destination".into());
        return;
    }
    let bisync = w.get_cloud_sync_direction() == "bisync";
    let args = cloud_build_sync_args(
        &src, &dst, bisync, &w.get_cloud_sync_conflict(), &w.get_cloud_sync_bwlimit(),
        &w.get_cloud_sync_transfers(), &w.get_cloud_sync_includes(),
        &w.get_cloud_sync_excludes(), &w.get_cloud_sync_maxsize(), &w.get_cloud_sync_minage());
    w.set_cloud_status(format!("Syncing {src} → {dst}…").into());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let ok = res.is_ok();
        let detail = format!("{src} → {dst}");
        cloud_log(if bisync { "bisync" } else { "sync" }, &detail, ok).await;
        let msg = match res { Ok(_) => format!("Sync complete: {detail}"), Err(e) => format!("Sync failed: {e}") };
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_cloud_status(msg.into()); cloud_refresh_usage(w.as_weak()); });
    });
}

/// Persist the current sync fields as a saved (optionally scheduled) job.
fn cloud_save_job(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let (src, dst) = (w.get_cloud_sync_src().to_string(), w.get_cloud_sync_dst().to_string());
    if src.trim().is_empty() || dst.trim().is_empty() {
        w.set_cloud_status("Save a job: fill source and destination first".into());
        return;
    }
    let direction = if w.get_cloud_sync_direction() == "bisync" { "bisync" } else { "oneway" }.to_string();
    let bw = w.get_cloud_sync_bwlimit().to_string();
    let bw = if bw.trim().is_empty() { None } else { Some(bw) };
    let interval = cloud_interval_secs(&w.get_cloud_sync_interval());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("cloud").await {
            let _ = tulipix_cloud::jobs::save_job(&pool, &src, &dst, &direction, bw.as_deref(), interval).await;
        }
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_cloud_status("Saved sync job".into()); cloud_refresh_jobs(w.as_weak()); });
    });
}

/// Rebuild the `cloud-jobs` model from cloud.db.
fn cloud_refresh_jobs(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let jobs = match pool_for("cloud").await {
            Ok(pool) => tulipix_cloud::jobs::list_jobs(&pool).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<CloudJob> = jobs.into_iter().map(|j| CloudJob {
                id: j.id as i32,
                label: format!("{} {} {}", j.src, if j.direction == "bisync" { "⇄" } else { "→" }, j.dst).into(),
                schedule: format!("{} · {}", if j.direction == "bisync" { "two-way" } else { "one-way" }, cloud_interval_label(j.interval_s)).into(),
                enabled: j.enabled != 0,
            }).collect();
            w.set_cloud_jobs(slint::ModelRc::new(slint::VecModel::from(rows)));
        });
    });
}

fn cloud_job_toggle(weak: slint::Weak<MainWindow>, id: i32) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("cloud").await {
            if let Ok(jobs) = tulipix_cloud::jobs::list_jobs(&pool).await {
                if let Some(j) = jobs.iter().find(|j| j.id == id as i64) {
                    let _ = tulipix_cloud::jobs::set_enabled(&pool, j.id, j.enabled == 0).await;
                }
            }
        }
        let _ = weak.upgrade_in_event_loop(move |w| cloud_refresh_jobs(w.as_weak()));
    });
}

fn cloud_job_delete(weak: slint::Weak<MainWindow>, id: i32) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("cloud").await {
            let _ = tulipix_cloud::jobs::delete_job(&pool, id as i64).await;
        }
        let _ = weak.upgrade_in_event_loop(move |w| cloud_refresh_jobs(w.as_weak()));
    });
}

fn cloud_job_run(weak: slint::Weak<MainWindow>, id: i32) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let job = match pool_for("cloud").await {
            Ok(pool) => tulipix_cloud::jobs::list_jobs(&pool).await.unwrap_or_default()
                .into_iter().find(|j| j.id == id as i64),
            Err(_) => None,
        };
        let Some(job) = job else { return; };
        let bisync = job.direction == "bisync";
        let args = cloud_build_sync_args(
            &job.src, &job.dst, bisync, "newer", job.bwlimit.as_deref().unwrap_or(""),
            "4", "", "", "", "");
        let detail = format!("{} → {}", job.src, job.dst);
        let _ = weak.upgrade_in_event_loop({
            let detail = detail.clone();
            move |w| w.set_cloud_status(format!("Running job: {detail}…").into())
        });
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let ok = res.is_ok();
        cloud_log(if bisync { "bisync" } else { "sync" }, &detail, ok).await;
        if let Ok(pool) = pool_for("cloud").await { let _ = tulipix_cloud::jobs::mark_ran(&pool, job.id).await; }
        let msg = match res { Ok(_) => format!("Job complete: {detail}"), Err(e) => format!("Job failed: {e}") };
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_cloud_status(msg.into()); cloud_refresh_jobs(w.as_weak()); });
    });
}

/// Server-side remote-to-remote copy/move (np.p5.cloud.remote-copy).
fn cloud_run_copy(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let (src, dst) = (w.get_cloud_copy_src().to_string(), w.get_cloud_copy_dst().to_string());
    if src.trim().is_empty() || dst.trim().is_empty() {
        w.set_cloud_status("Copy needs a source and destination".into());
        return;
    }
    let move_files = w.get_cloud_copy_move();
    let server_side = w.get_cloud_copy_serverside();
    let args = tulipix_cloud::sync::copy_args(&src, &dst, move_files, server_side, None);
    w.set_cloud_status(format!("{} {src} → {dst}…", if move_files { "Moving" } else { "Copying" }).into());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let ok = res.is_ok();
        let detail = format!("{src} → {dst}");
        cloud_log(if move_files { "move" } else { "copy" }, &detail, ok).await;
        let msg = match res { Ok(_) => format!("Done: {detail}"), Err(e) => format!("Failed: {e}") };
        let _ = weak.upgrade_in_event_loop(move |w| { w.set_cloud_status(msg.into()); cloud_refresh_usage(w.as_weak()); });
    });
}

/// `rclone check` — hash-verify two trees (np.p5.cloud.verify).
fn cloud_run_verify(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let (src, dst) = (w.get_cloud_verify_src().to_string(), w.get_cloud_verify_dst().to_string());
    if src.trim().is_empty() || dst.trim().is_empty() {
        w.set_cloud_status("Verify needs a source and a target".into());
        return;
    }
    let args = tulipix_cloud::verify::check_args(&src, &dst, false, 8);
    w.set_cloud_status(format!("Verifying {src} ↔ {dst}…").into());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // `rclone check` exits non-zero when trees differ — that is a *result*,
        // not a tool failure, so surface its message either way.
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let (ok, msg) = match res {
            Ok(_) => (true, "Verify: trees match (0 differences)".to_string()),
            Err(e) => (false, format!("Verify: {e}")),
        };
        cloud_log("verify", &format!("{src} ↔ {dst}"), ok).await;
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
    });
}

/// `rclone dedupe` (np.p5.cloud.verify).
fn cloud_run_dedupe(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let target = w.get_cloud_dedupe_target().to_string();
    if target.trim().is_empty() { w.set_cloud_status("Dedupe needs a remote:path".into()); return; }
    let mode = tulipix_cloud::verify::DedupeMode::parse(&w.get_cloud_dedupe_mode())
        .unwrap_or(tulipix_cloud::verify::DedupeMode::Newest);
    let args = tulipix_cloud::verify::dedupe_args(&target, mode);
    w.set_cloud_status(format!("Deduping {target}…").into());
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let ok = res.is_ok();
        cloud_log("dedupe", &target, ok).await;
        let msg = match res { Ok(_) => format!("Dedupe complete: {target}"), Err(e) => format!("Dedupe failed: {e}") };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
    });
}

/// Query per-remote storage (`rclone about`) + load the transfer log
/// (np.p5.cloud.quota).
fn cloud_refresh_usage(weak: slint::Weak<MainWindow>) {
    let _ = weak.upgrade_in_event_loop(|w| w.set_cloud_status("Refreshing storage usage…".into()));
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let remotes = match pool_for("cloud").await {
            Ok(pool) => tulipix_cloud::remotes::list(&pool).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let names: Vec<String> = remotes.into_iter().map(|(n, _)| n).collect();
        let mut usage = Vec::new();
        for name in names {
            let n2 = name.clone();
            // Cap each `rclone about` at 15s — one unreachable remote must not
            // hang the whole refresh.
            let out = match tokio::time::timeout(
                std::time::Duration::from_secs(15),
                tokio::task::spawn_blocking(move || cloud::run(&tulipix_cloud::quota::about_args(&n2))),
            ).await {
                Ok(Ok(Ok(s))) => Some(s),
                _ => None,
            };
            let about = out.as_deref().and_then(tulipix_cloud::quota::parse_about);
            usage.push((name, about));
        }
        let log = match pool_for("cloud").await {
            Ok(pool) => tulipix_cloud::jobs::recent_log(&pool, 15).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let rows: Vec<CloudUsage> = usage.into_iter().map(|(remote, about)| match about {
                Some(a) => {
                    // Units only (KB/MB/GB) — `human_size` appends "(N bytes)" which
                    // the usage dashboard doesn't want.
                    let used = a.used.map(|b| cloud_human_size(b.max(0) as u64)).unwrap_or_else(|| "—".into());
                    let total = a.total.map(|b| cloud_human_size(b.max(0) as u64)).unwrap_or_else(|| "—".into());
                    let has_quota = a.fraction().is_some();
                    CloudUsage { remote: remote.into(), used: used.into(), total: total.into(),
                        fraction: a.fraction().unwrap_or(0.0) as f32, has_quota }
                }
                None => CloudUsage { remote: remote.into(), used: "n/a".into(), total: "".into(), fraction: 0.0, has_quota: false },
            }).collect();
            w.set_cloud_usage_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
            let xfers: Vec<CloudXfer> = log.into_iter().map(|(id, kind, detail, ok, at)| CloudXfer {
                id: id as i32, kind: kind.into(), detail: detail.into(), ok: ok != 0,
                when: cloud_relative_time(at).into(),
            }).collect();
            w.set_cloud_xfer_rows(slint::ModelRc::new(slint::VecModel::from(xfers)));
            w.set_cloud_status("Storage usage updated".into());
        });
    });
}

/// Re-run a logged transfer. For arrow-detail kinds (sync/bisync/copy/move) the
/// "src → dst" is parsed and the matching rclone op replayed; other kinds aren't
/// replayable from the log alone.
fn cloud_retry_xfer(weak: slint::Weak<MainWindow>, id: i64) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let row = match pool_for("cloud").await {
            Ok(pool) => tulipix_cloud::jobs::log_row(&pool, id).await.unwrap_or(None),
            Err(_) => None,
        };
        let Some((kind, detail)) = row else { return; };
        let sub = match kind.as_str() {
            "sync" => Some("sync"),
            "bisync" => Some("bisync"),
            "copy" => Some("copy"),
            "move" => Some("move"),
            _ => None,
        };
        let parts: Vec<&str> = detail.split(" → ").collect();
        let Some(sub) = sub.filter(|_| parts.len() == 2) else {
            let msg = format!("Retry not supported for {kind}");
            let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
            return;
        };
        let (src, dst) = (parts[0].trim().to_string(), parts[1].trim().to_string());
        let detail2 = format!("{src} → {dst}");
        let retrying = format!("Retrying {kind}: {detail2}");
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(retrying.into()));
        let args = vec![sub.to_string(), src, dst];
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let ok = res.is_ok();
        cloud_log(&kind, &detail2, ok).await;
        let msg = match res { Ok(_) => format!("Retried {kind}: {detail2}"), Err(e) => format!("Retry failed: {e}") };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
        cloud_reload_xfers(weak);
    });
}

/// Reload only the transfer-history list (no slow `rclone about`) so clear /
/// retry update the dashboard instantly.
fn cloud_reload_xfers(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let log = match pool_for("cloud").await {
            Ok(pool) => tulipix_cloud::jobs::recent_log(&pool, 15).await.unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        let _ = weak.upgrade_in_event_loop(move |w| {
            let xfers: Vec<CloudXfer> = log.into_iter().map(|(id, kind, detail, ok, at)| CloudXfer {
                id: id as i32, kind: kind.into(), detail: detail.into(), ok: ok != 0,
                when: cloud_relative_time(at).into(),
            }).collect();
            w.set_cloud_xfer_rows(slint::ModelRc::new(slint::VecModel::from(xfers)));
        });
    });
}

/// Delete one transfer-history row, then reload the list.
fn cloud_clear_xfer(weak: slint::Weak<MainWindow>, id: i64) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("cloud").await { let _ = tulipix_cloud::jobs::delete_log(&pool, id).await; }
        cloud_reload_xfers(weak);
    });
}

/// Wipe the whole transfer history, then empty the list.
fn cloud_clear_all_xfers(weak: slint::Weak<MainWindow>) {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        if let Ok(pool) = pool_for("cloud").await { let _ = tulipix_cloud::jobs::clear_log(&pool).await; }
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_cloud_status("Cleared transfer history".into());
            w.set_cloud_xfer_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<CloudXfer>::new())));
        });
    });
}

/// "5m ago" / "2h ago" / "3d ago" for the transfer log.
fn cloud_relative_time(at: i64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let d = (now - at).max(0);
    if d < 60 { "just now".into() }
    else if d < 3600 { format!("{}m ago", d / 60) }
    else if d < 86400 { format!("{}h ago", d / 3600) }
    else { format!("{}d ago", d / 86400) }
}

/// Wrap a remote in an rclone crypt remote (np.p5.cloud.crypt).
fn cloud_crypt_submit(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let name = w.get_cloud_crypt_name().to_string();
    let wrapped = w.get_cloud_crypt_wrapped().to_string();
    let pass = w.get_cloud_crypt_pass().to_string();
    let pass2 = w.get_cloud_crypt_pass2().to_string();
    if name.trim().is_empty() || wrapped.trim().is_empty() || pass.is_empty() {
        w.set_cloud_status("Crypt needs a name, a wrapped remote and a password".into());
        return;
    }
    let args = tulipix_cloud::encrypt::crypt_create_args(&name, &wrapped, &pass, &pass2);
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let name_c = name.clone();
        let res = tokio::task::spawn_blocking(move || cloud::run(&args)).await
            .unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        if res.is_ok() {
            // Mirror the new remote + keep the password in the OS keychain.
            if let Ok(pool) = pool_for("cloud").await { let _ = tulipix_cloud::remotes::upsert(&pool, &name_c, "crypt", true).await; }
            if let Ok(entry) = tulipix_cloud::encrypt::crypt_pass_entry(&name_c) { let _ = entry.set_password(&pass); }
        }
        let _ = weak.upgrade_in_event_loop(move |w| {
            match res {
                Ok(_) => { w.set_cloud_crypt_open(false); w.set_cloud_status(format!("Created crypt remote '{name}'").into()); cloud_refresh_remotes(w.as_weak()); }
                Err(e) => w.set_cloud_status(format!("Crypt failed: {e}").into()),
            }
        });
    });
}

/// Apply mount options: persist auto-mount + VFS cache, remount now if on
/// (np.p5.cloud.mount-cache).
fn cloud_mountopts_submit(weak: slint::Weak<MainWindow>) {
    let Some(w) = weak.upgrade() else { return; };
    let remote = w.get_cloud_mountopts_remote().to_string();
    if remote.trim().is_empty() { w.set_cloud_mountopts_open(false); return; }
    let auto = w.get_cloud_mountopts_auto();
    let cache = w.get_cloud_mountopts_cache().to_string();
    let max_age = w.get_cloud_mountopts_maxage().to_string();
    w.set_cloud_mountopts_open(false);
    // Stash the cache preference so cloud_mount_ensure picks it up.
    if let Ok(mut g) = cloud_vfs().lock() { g.insert(remote.clone(), (cache.clone(), max_age.clone())); }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let base = tulipix_core::paths::data_dir().map(|d| d.join("mounts").join(&remote))
            .map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Ok(pool) = pool_for("cloud").await {
            if let Ok(Some(id)) = tulipix_cloud::remotes::id_of(&pool, &remote).await {
                let kind = tulipix_cloud::mount::FsKind::for_os(std::env::consts::OS);
                let _ = tulipix_cloud::mount::set_auto(&pool, id, &base, kind, auto).await;
            }
        }
        // Remount now to apply the new cache mode (best-effort).
        let remote_c = remote.clone();
        let mounted = matches!(tokio::task::spawn_blocking(move || { cloud_force_remount(&remote_c) }).await, Ok(Ok(_)));
        let msg = if mounted { format!("Mounted '{remote}'") } else { format!("Saved options for '{remote}' (mount deferred)") };
        let active = remote.clone();
        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_cloud_status(msg.into());
            if w.get_cloud_active_remote().to_string() == active { w.set_cloud_active_mounted(mounted); }
            cloud_refresh_remotes(w.as_weak());
        });
    });
}

/// Kill any live mount for `remote` then re-mount with current VFS prefs.
fn cloud_force_remount(remote: &str) -> anyhow::Result<PathBuf> {
    if let Ok(mut g) = cloud_mounts().lock() {
        if let Some((mp, mut child)) = g.remove(remote) {
            let _ = child.kill();
            #[cfg(target_os = "linux")]
            { let _ = std::process::Command::new("fusermount").args(["-uz"]).arg(&mp).status(); }
            let _ = &mp;
        }
    }
    cloud_mount_ensure(remote)
}

/// Lightweight preview without mounting: copy the single file to a temp dir and
/// open it (image → in-app viewer, else OS default) (np.p5.cloud.preview).
fn cloud_preview(weak: slint::Weak<MainWindow>, remote: String, rel: String, name: String) {
    if let Some(w) = weak.upgrade() { w.set_cloud_status(format!("Fetching preview of {name}…").into()); }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let tmp = std::env::temp_dir().join("tulipix-cloud-preview");
        let _ = std::fs::create_dir_all(&tmp);
        let dest = tmp.join(&name);
        let src = format!("{remote}:{rel}");
        let dest_s = dest.to_string_lossy().into_owned();
        let res = tokio::task::spawn_blocking(move || cloud::run(&["copyto".into(), src, dest_s]))
            .await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        match res {
            Ok(_) => {
                let ext = dest.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
                if cloud_is_image(&ext) {
                    let _ = weak.upgrade_in_event_loop(move |w| { cloud_show_image(&w, &dest); w.set_cloud_status("".into()); });
                } else {
                    let d2 = dest.clone();
                    let _ = tokio::task::spawn_blocking(move || tulipix_platform::fm::open_default(&d2)).await;
                    let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Previewing {name}").into()));
                }
            }
            Err(e) => { let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Preview failed: {e}").into())); }
        }
    });
}

/// Import a cloud file into the local library: download it into a watched
/// library folder so the live FS watcher / next scan indexes it and routes it
/// to Photos/Videos/Music/Books by type (np.p5.cloud.import).
fn cloud_import(weak: slint::Weak<MainWindow>, remote: String, rel: String, name: String) {
    if let Some(w) = weak.upgrade() { w.set_cloud_status(format!("Importing {name}…").into()); }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        // Prefer the first existing watched library folder (its FS watcher will
        // pick the new file up); fall back to ~/Downloads if none configured.
        let dest_dir = load_watched_folders().into_iter().find(|p| p.is_dir())
            .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into())).join("Downloads"));
        let _ = std::fs::create_dir_all(&dest_dir);
        let dest = dest_dir.join(&name);
        let src = format!("{remote}:{rel}");
        let dest_s = dest.to_string_lossy().into_owned();
        let res = tokio::task::spawn_blocking(move || cloud::run(&["copyto".into(), src, dest_s]))
            .await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        let ok = res.is_ok();
        cloud_log("import", &name, ok).await;
        let where_ = dest_dir.file_name().and_then(|s| s.to_str()).unwrap_or("library").to_string();
        let msg = match res {
            Ok(_) => format!("Imported {name} → {where_} (will appear in its library)"),
            Err(e) => format!("Import failed: {e}"),
        };
        let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(msg.into()));
    });
}

/// Mint a provider-native public share link (`rclone link`) and record it
/// (np.p5.cloud.share-link).
fn cloud_share_link(weak: slint::Weak<MainWindow>, remote: String, rel: String) {
    if let Some(w) = weak.upgrade() { w.set_cloud_status("Creating share link…".into()); }
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        let (remote_c, rel_c) = (remote.clone(), rel.clone());
        let res = tokio::task::spawn_blocking(move || cloud::run(&tulipix_cloud::share::link_args(&remote_c, &rel_c)))
            .await.unwrap_or_else(|e| Err(anyhow::anyhow!(e)));
        match res {
            Ok(url) => {
                let url = url.trim().to_string();
                if let Ok(pool) = pool_for("cloud").await {
                    if let Ok(Some(id)) = tulipix_cloud::remotes::id_of(&pool, &remote).await {
                        let _ = tulipix_cloud::share::issue(&pool, id, &rel, &url).await;
                    }
                }
                cloud_log("share", &rel, true).await;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Share link: {url}").into()));
            }
            Err(e) => {
                cloud_log("share", &rel, false).await;
                let _ = weak.upgrade_in_event_loop(move |w| w.set_cloud_status(format!("Share link failed (backend may not support it): {e}").into()));
            }
        }
    });
}

// Music section (My Music/Podcasts/Radio/Audiobooks) extracted to tulipix_sec_music. See `use tulipix_sec_music::*`.

