// The Cloud section: an rclone front-end.
//
// rclone is a subprocess. `tulipix-cloud` is the crate that knows how to build
// its argv and read its output back — `lsjson` into entries, `config dump` into
// remotes, `config providers` into a form schema, `about` into a usage bar. It
// also owns `cloud.db`, which is everything the app knows on top of what rclone
// stores: which remotes are mounted, saved sync jobs, share links, the recycle
// bin.
//
// So this file spawns processes and keeps a session. It does not parse rclone
// output and it does not build rclone command lines, because the crate that
// does both of those is frozen and already tested.

use anyhow::{Result, anyhow};
use flutter_rust_bridge::frb;

use crate::frb_generated::StreamSink;
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::process::Command;

use tulipix_cloud::{browse, jobs, mount, providers, quota, remotes, sync, verify};

use crate::db::cloud_pool;

// ------------------------------------------------------------------- state ---

/// A configured remote, and whether the app currently has it mounted.
pub struct Remote {
    pub name: String,
    pub backend: String,
    pub mounted: bool,
    pub mount_path: String,
}

/// One row of a directory listing.
pub struct Entry {
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
    /// rclone's RFC-3339 ModTime, as it gives it.
    pub modified: String,
}

/// One backend rclone offers, for the connect wizard's first step.
pub struct BackendRow {
    pub name: String,
    pub description: String,
    pub oauth: bool,
}

/// One row of the generated config form.
pub struct OptRow {
    pub name: String,
    pub help: String,
    /// string | bool | int | SizeSuffix | …, straight from rclone.
    pub kind: String,
    pub value: String,
    pub required: bool,
    pub secret: bool,
    pub advanced: bool,
    /// Only these values are legal — render a closed dropdown.
    pub exclusive: bool,
    pub examples: Vec<String>,
}

/// A saved, scheduled sync.
pub struct SyncJobRow {
    pub id: i64,
    pub src: String,
    pub dst: String,
    /// oneway | bisync.
    pub direction: String,
    pub bwlimit: String,
    pub interval_s: i64,
    pub last_run: i64,
    pub enabled: bool,
}

/// A line in the activity log — what ran, and whether it worked.
pub struct XferRow {
    pub id: i64,
    pub kind: String,
    pub detail: String,
    pub ok: bool,
    pub at: i64,
}

/// The usage bar for one remote.
pub struct UsageRow {
    pub remote: String,
    pub used: i64,
    pub total: i64,
    pub free: i64,
    pub trashed: i64,
}

pub struct CloudState {
    /// Whether the rclone binary could be found at all. Everything here is
    /// unreachable without it, so the UI says so once rather than failing
    /// eleven times.
    pub rclone_ok: bool,
    pub rclone_version: String,

    pub remotes: Vec<Remote>,
    pub active_remote: String,
    pub active_mounted: bool,
    pub path: String,
    pub entries: Vec<Entry>,
    pub query: String,
    /// name | size | date.
    pub sort_key: String,
    pub sort_asc: bool,

    /// A long-running rclone call is in flight.
    pub busy: bool,
    pub status: String,

    // --- the connect wizard ---
    pub connect_open: bool,
    /// 0 pick a backend · 1 fill the form.
    pub connect_step: i64,
    pub connect_edit: bool,
    pub backend_query: String,
    pub backends: Vec<BackendRow>,
    pub form_name: String,
    pub form_backend: String,
    pub form_oauth: bool,
    pub authorized: bool,
    pub show_advanced: bool,
    pub opts: Vec<OptRow>,
    pub connect_error: String,

    // --- the tools drawer ---
    pub tools_open: bool,
    /// sync | copy | verify | dedupe | mount.
    pub tools_tab: String,
    pub sync_src: String,
    pub sync_dst: String,
    pub sync_direction: String,
    pub sync_bwlimit: String,
    pub sync_transfers: String,
    pub sync_interval: String,
    pub copy_src: String,
    pub copy_dst: String,
    pub copy_move: bool,
    pub copy_serverside: bool,
    pub verify_src: String,
    pub verify_dst: String,
    pub dedupe_target: String,
    pub dedupe_mode: String,

    pub jobs: Vec<SyncJobRow>,
    pub xfers: Vec<XferRow>,
    pub usage: Vec<UsageRow>,

    /// The last multi-line rclone output — a check report, a dedupe summary.
    pub output: String,
}

// ---------------------------------------------------------------- commands ---

pub enum CloudCmd {
    Refresh,

    // --- browsing ---
    OpenRemote {
        name: String,
    },
    Navigate {
        path: String,
    },
    Up,
    Search {
        text: String,
    },
    SetSort {
        key: String,
    },
    Reload,

    // --- one entry ---
    NewFolder {
        name: String,
    },
    Rename {
        from: String,
        to: String,
    },
    DeleteEntry {
        name: String,
        is_dir: bool,
    },
    DownloadEntry {
        name: String,
        dest: String,
    },
    UploadFile {
        local: String,
    },
    ShareLink {
        name: String,
    },

    // --- remotes ---
    ConnectOpen,
    ConnectClose,
    BackendSearch {
        text: String,
    },
    BackendPick {
        name: String,
    },
    SetFormName {
        name: String,
    },
    SetOpt {
        name: String,
        value: String,
    },
    ToggleAdvanced,
    Authorize,
    SaveRemote,
    EditRemote {
        name: String,
    },
    DeleteRemote {
        name: String,
    },

    // --- mounts ---
    Mount {
        name: String,
        path: String,
        cache: String,
        max_age: String,
    },
    Unmount {
        name: String,
    },

    // --- the tools drawer ---
    ToolsToggle,
    SetToolsTab {
        name: String,
    },
    SetField {
        key: String,
        value: String,
    },
    RunSync,
    RunCopy,
    RunVerify,
    RunDedupe,
    SaveJob,
    DeleteJob {
        id: i64,
    },
    ToggleJob {
        id: i64,
        enabled: bool,
    },
    RunJob {
        id: i64,
    },
    LoadUsage,
    ClearLog,
    DeleteLogRow {
        id: i64,
    },
}

/// Progress and news from a running rclone call.
pub enum CloudEvent {
    /// 0..1 where rclone reports one, otherwise -1 for indeterminate.
    Progress {
        label: String,
        frac: f64,
    },
    Done {
        label: String,
        ok: bool,
        message: String,
    },
    Failed {
        message: String,
    },
}

// ----------------------------------------------------------------- session ---

#[frb(ignore)]
#[derive(Debug, Default)]
struct Session {
    active: String,
    path: String,
    query: String,
    sort_key: String,
    sort_asc: bool,
    status: String,
    busy: bool,
    output: String,

    entries: Vec<browse::Entry>,

    connect_open: bool,
    connect_step: i64,
    connect_edit: bool,
    backend_query: String,
    form_name: String,
    form_backend: String,
    authorized: bool,
    show_advanced: bool,
    /// name → value, for whatever the form currently holds.
    form: std::collections::BTreeMap<String, String>,
    connect_error: String,

    tools_open: bool,
    tools_tab: String,
    fields: std::collections::BTreeMap<String, String>,

    usage: Vec<UsageCache>,
    /// remote → mount point, for everything this process mounted.
    mounts: std::collections::BTreeMap<String, String>,
}

#[frb(ignore)]
#[derive(Debug, Clone)]
struct UsageCache {
    remote: String,
    used: i64,
    total: i64,
    free: i64,
    trashed: i64,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            sort_key: "name".into(),
            sort_asc: true,
            tools_tab: "sync".into(),
            ..Default::default()
        })
    })
}

fn lock() -> MutexGuard<'static, Session> {
    match session().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// rclone's own provider schema, fetched once. It is a megabyte of JSON and it
/// does not change while the app is running.
fn backends_cache() -> &'static Mutex<Vec<providers::Backend>> {
    static B: OnceLock<Mutex<Vec<providers::Backend>>> = OnceLock::new();
    B.get_or_init(|| Mutex::new(Vec::new()))
}

fn events() -> &'static OnceLock<StreamSink<CloudEvent>> {
    static E: OnceLock<StreamSink<CloudEvent>> = OnceLock::new();
    &E
}

fn emit(e: CloudEvent) {
    if let Some(sink) = events().get() {
        let _ = sink.add(e);
    }
}

// ---------------------------------------------------------------- exported ---

pub async fn cloud_dispatch(cmd: CloudCmd) -> Result<CloudState> {
    apply(cmd).await?;
    snapshot().await
}

#[frb(sync)]
pub fn cloud_events(sink: StreamSink<CloudEvent>) {
    let _ = events().set(sink);
}

/// Unmount everything this process mounted. Leaving a FUSE mount behind after
/// the window closes makes the next `rclone mount` on that directory fail, and
/// the directory itself unreadable until someone runs `fusermount -u` by hand.
/// Called from the app's exit hook.
pub async fn cloud_shutdown() -> Result<()> {
    let mounts: Vec<String> = lock().mounts.values().cloned().collect();
    for path in mounts {
        unmount_path(&path).await;
    }
    lock().mounts.clear();
    Ok(())
}

/// The `rclone mount` processes this run started, by mount point. On Linux and
/// macOS the OS unmount ends them; Windows has no unmount call for a WinFsp
/// mount, so ending the process is the unmount there.
fn mount_children() -> MutexGuard<'static, std::collections::HashMap<String, tokio::process::Child>> {
    static C: OnceLock<Mutex<std::collections::HashMap<String, tokio::process::Child>>> = OnceLock::new();
    C.get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}

/// Once per process, before the first snapshot: mounts the database says are
/// up. One still up (the app crashed, or was killed) goes back into the
/// session, so Unmount can find it; one that is gone is marked unmounted.
async fn reconcile_mounts() {
    static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if DONE.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    let Ok(pool) = cloud_pool().await else { return };
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT m.remote_id, r.name, m.mount_path FROM mounts m \
         JOIN remotes r ON r.id = m.remote_id WHERE m.status = 'mounted'",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    for (id, name, path) in rows {
        if is_mounted(&path) {
            lock().mounts.insert(name, path);
        } else {
            mount::set_status(pool, id, "unmounted").await.ok();
        }
    }
}

/// Whether something is mounted at `path` right now.
fn is_mounted(path: &str) -> bool {
    #[cfg(target_os = "linux")]
    {
        // Field 5 of mountinfo is the mount point, with spaces as \040.
        let want = path.trim_end_matches('/').replace(' ', "\\040");
        std::fs::read_to_string("/proc/self/mountinfo")
            .map(|t| t.lines().any(|l| l.split(' ').nth(4) == Some(want.as_str())))
            .unwrap_or(false)
    }
    #[cfg(target_os = "macos")]
    {
        let needle = format!(" on {} (", path.trim_end_matches('/'));
        std::process::Command::new("mount")
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).contains(&needle))
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        // A WinFsp mount point exists only while rclone holds it.
        std::path::Path::new(path).exists()
    }
}

// ------------------------------------------------------------------ rclone ---

fn rclone_bin() -> std::path::PathBuf {
    tulipix_core::thumbs::tool_bin("rclone")
}

/// `NoWindow` is implemented for `std::process::Command`; tokio's wraps one, so
/// the flag has to be set through the inner handle. A no-op off Windows.
fn no_window(c: &mut Command) {
    use tulipix_core::proc::NoWindow;
    c.as_std_mut().no_window();
}

fn cmd() -> Command {
    let mut c = Command::new(rclone_bin());
    no_window(&mut c);
    c
}

/// Run rclone and collect everything it said. For calls whose whole point is
/// the output — a listing, a config dump, a check report.
async fn run(args: &[String]) -> Result<String> {
    let out = cmd().args(args).output().await?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    if out.status.success() {
        return Ok(stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
    // rclone puts the useful sentence on stderr and exits non-zero; the
    // stdout it did manage is usually still worth keeping for the report.
    Err(anyhow!(if stderr.is_empty() {
        format!("rclone exited {}", out.status)
    } else {
        stderr
    }))
}

/// Run rclone with `--progress`, reporting as it goes. For the calls that move
/// bytes.
async fn run_progress(label: &str, args: &[String]) -> Result<String> {
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut child = cmd()
        .args(args)
        .arg("--stats=1s")
        .arg("--stats-one-line")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let mut log = String::new();
    if let Some(err) = child.stderr.take() {
        let mut lines = BufReader::new(err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            // rclone's one-line stats carry a percentage; anything else is a
            // message worth keeping but not worth a progress event.
            if let Some(frac) = parse_percent(&line) {
                emit(CloudEvent::Progress {
                    label: label.to_string(),
                    frac,
                });
            }
            if log.len() < LOG_CAP {
                log.push_str(&line);
                log.push('\n');
            }
        }
    }
    let status = child.wait().await?;
    if let Some(out) = child.stdout.take() {
        let mut lines = BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if log.len() < LOG_CAP {
                log.push_str(&line);
                log.push('\n');
            }
        }
    }
    emit(CloudEvent::Done {
        label: label.to_string(),
        ok: status.success(),
        message: last_line(&log),
    });
    if status.success() {
        Ok(log)
    } else {
        Err(anyhow!(last_line(&log)))
    }
}

/// The log a report popup shows. Past this, a `check` over a large tree is
/// megabytes of "identical" lines that nobody scrolls to the end of.
const LOG_CAP: usize = 200_000;

fn last_line(s: &str) -> String {
    s.lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim()
        .to_string()
}

/// `… 43% …` out of an rclone stats line, as 0..1.
fn parse_percent(line: &str) -> Option<f64> {
    let at = line.find('%')?;
    let head = &line[..at];
    let start = head
        .rfind(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|i| i + 1)
        .unwrap_or(0);
    head[start..]
        .parse::<f64>()
        .ok()
        .map(|p| (p / 100.0).clamp(0.0, 1.0))
}

async fn unmount_path(path: &str) {
    // Platform-specific and best-effort: a mount that is already gone is the
    // outcome we wanted anyway.
    #[cfg(target_os = "linux")]
    {
        let mut c = Command::new("fusermount");
        no_window(&mut c);
        let _ = c.arg("-uz").arg(path).status().await;
    }
    #[cfg(target_os = "macos")]
    {
        let mut c = Command::new("umount");
        no_window(&mut c);
        let _ = c.arg(path).status().await;
    }
    // After the OS unmount, never before: a FUSE daemon killed first leaves
    // "transport endpoint is not connected" until something unmounts it.
    if let Some(mut child) = mount_children().remove(path) {
        let _ = child.start_kill();
    }
}

/// `remote:path`, with the colon in the one place rclone accepts it.
fn full_path(remote: &str, path: &str) -> String {
    let p = path.trim_start_matches('/');
    if p.is_empty() {
        format!("{remote}:")
    } else {
        format!("{remote}:{p}")
    }
}

// ---------------------------------------------------------------- commands ---

async fn apply(cmd_in: CloudCmd) -> Result<()> {
    match cmd_in {
        CloudCmd::Refresh => {
            sync_remotes().await?;
            reconcile_mounts().await;
        }

        // --- browsing ---
        CloudCmd::OpenRemote { name } => {
            {
                let mut s = lock();
                s.active = name;
                s.path = String::new();
                s.query.clear();
            }
            list_current().await?;
        }
        CloudCmd::Navigate { path } => {
            lock().path = path;
            list_current().await?;
        }
        CloudCmd::Up => {
            {
                let mut s = lock();
                let p = s.path.trim_end_matches('/').to_string();
                s.path = match p.rfind('/') {
                    Some(i) => p[..i].to_string(),
                    None => String::new(),
                };
            }
            list_current().await?;
        }
        CloudCmd::Search { text } => lock().query = text,
        CloudCmd::SetSort { key } => {
            let mut s = lock();
            // Clicking the column that is already sorted reverses it.
            if s.sort_key == key {
                s.sort_asc = !s.sort_asc;
            } else {
                s.sort_key = key;
                s.sort_asc = true;
            }
        }
        CloudCmd::Reload => list_current().await?,

        // --- one entry ---
        CloudCmd::NewFolder { name } => {
            let (remote, path) = here();
            let target = full_path(&remote, &join(&path, &name));
            run(&["mkdir".into(), target]).await?;
            list_current().await?;
            lock().status = format!("Created {name}.");
        }
        CloudCmd::Rename { from, to } => {
            let (remote, path) = here();
            run(&[
                "moveto".into(),
                full_path(&remote, &join(&path, &from)),
                full_path(&remote, &join(&path, &to)),
            ])
            .await?;
            list_current().await?;
        }
        CloudCmd::DeleteEntry { name, is_dir } => {
            let (remote, path) = here();
            let target = full_path(&remote, &join(&path, &name));
            // `purge` removes a directory and its contents; `deletefile`
            // refuses to touch a directory, which is the safety we want.
            run(&[
                if is_dir {
                    "purge".into()
                } else {
                    "deletefile".to_string()
                },
                target,
            ])
            .await?;
            list_current().await?;
            lock().status = format!("Deleted {name}.");
        }
        CloudCmd::DownloadEntry { name, dest } => {
            let (remote, path) = here();
            let out = run_progress(
                &format!("Downloading {name}"),
                &[
                    "copy".into(),
                    full_path(&remote, &join(&path, &name)),
                    dest.clone(),
                    "--progress".into(),
                ],
            )
            .await?;
            log_it("download", &format!("{name} → {dest}"), true).await;
            lock().output = out;
        }
        CloudCmd::UploadFile { local } => {
            let (remote, path) = here();
            let out = run_progress(
                "Uploading",
                &[
                    "copy".into(),
                    local.clone(),
                    full_path(&remote, &path),
                    "--progress".into(),
                ],
            )
            .await?;
            log_it("upload", &format!("{local} → {remote}:{path}"), true).await;
            lock().output = out;
            list_current().await?;
        }
        CloudCmd::ShareLink { name } => {
            let (remote, path) = here();
            let url = run(&tulipix_cloud::share::link_args(
                &remote,
                &join(&path, &name),
            ))
            .await?;
            let url = url.trim().to_string();
            if !url.is_empty() {
                let pool = cloud_pool().await?;
                if let Ok(Some(id)) = remotes::id_of(pool, &remote).await {
                    tulipix_cloud::share::issue(pool, id, &join(&path, &name), &url)
                        .await
                        .ok();
                }
            }
            // The link itself is the answer, so it goes where the user can read
            // and copy it rather than into a toast that vanishes.
            lock().output = url.clone();
            lock().status = if url.is_empty() {
                "This backend does not issue share links.".into()
            } else {
                url
            };
        }

        // --- remotes ---
        CloudCmd::ConnectOpen => {
            load_backends().await?;
            let mut s = lock();
            s.connect_open = true;
            s.connect_step = 0;
            s.connect_edit = false;
            s.form.clear();
            s.form_name.clear();
            s.form_backend.clear();
            s.authorized = false;
            s.connect_error.clear();
        }
        CloudCmd::ConnectClose => {
            let mut s = lock();
            s.connect_open = false;
            s.connect_error.clear();
        }
        CloudCmd::BackendSearch { text } => lock().backend_query = text,
        CloudCmd::BackendPick { name } => {
            let defaults = {
                let cache = backends_cache().lock().ok();
                cache
                    .and_then(|c| providers::find(&c, &name).cloned())
                    .map(|b| {
                        b.options
                            .iter()
                            .filter(|o| !o.default.is_empty())
                            .map(|o| (o.name.clone(), o.default.clone()))
                            .collect::<std::collections::BTreeMap<_, _>>()
                    })
                    .unwrap_or_default()
            };
            let mut s = lock();
            s.form_backend = name;
            s.connect_step = 1;
            s.form = defaults;
            s.authorized = false;
            s.connect_error.clear();
        }
        CloudCmd::SetFormName { name } => lock().form_name = name,
        CloudCmd::SetOpt { name, value } => {
            lock().form.insert(name, value);
        }
        CloudCmd::ToggleAdvanced => {
            let mut s = lock();
            s.show_advanced = !s.show_advanced;
        }
        CloudCmd::Authorize => {
            let backend = lock().form_backend.clone();
            if backend.is_empty() {
                anyhow::bail!("pick a backend first");
            }
            // rclone opens the browser itself and prints the token when the
            // round trip finishes. This blocks until then, which is correct:
            // there is nothing else to do in this dialog meanwhile.
            let out = run(&providers::authorize_args(&backend)).await?;
            match providers::parse_authorize(&out) {
                Some(token) => {
                    let mut s = lock();
                    s.form.insert("token".into(), token);
                    s.authorized = true;
                    s.connect_error.clear();
                }
                None => {
                    lock().connect_error = "Authorisation did not return a token.".into();
                }
            }
        }
        CloudCmd::SaveRemote => {
            let (name, backend, form, editing) = {
                let s = lock();
                (
                    s.form_name.trim().to_string(),
                    s.form_backend.clone(),
                    s.form.clone(),
                    s.connect_edit,
                )
            };
            if name.is_empty() {
                lock().connect_error = "Give the remote a name.".into();
                return Ok(());
            }
            if backend.is_empty() {
                lock().connect_error = "Pick a backend.".into();
                return Ok(());
            }
            let pairs: Vec<(&str, &str)> = form
                .iter()
                .filter(|(_, v)| !v.trim().is_empty())
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            let args = if editing {
                providers::update_args(&name, &backend, &pairs)
            } else {
                remotes::create_args(&name, &backend, &pairs)
            };
            match run(&args).await {
                Ok(_) => {
                    let pool = cloud_pool().await?;
                    remotes::upsert(pool, &name, &backend, backend == "crypt").await?;
                    let mut s = lock();
                    s.connect_open = false;
                    s.connect_error.clear();
                    s.status = format!("Saved {name}.");
                }
                Err(e) => {
                    lock().connect_error = e.to_string();
                    return Ok(());
                }
            }
            sync_remotes().await?;
        }
        CloudCmd::EditRemote { name } => {
            load_backends().await?;
            let dump = run(&remotes::dump_args()).await.unwrap_or_default();
            let existing = remotes::parse_dump_one(&dump, &name);
            let mut s = lock();
            s.connect_open = true;
            s.connect_edit = true;
            s.connect_step = 1;
            s.form_name = name;
            match existing {
                Some((backend, opts)) => {
                    s.form_backend = backend;
                    s.form = opts;
                    s.authorized = s.form.contains_key("token");
                }
                None => {
                    s.form.clear();
                    s.authorized = false;
                }
            }
            s.connect_error.clear();
        }
        CloudCmd::DeleteRemote { name } => {
            run(&remotes::delete_args(&name)).await?;
            let pool = cloud_pool().await?;
            remotes::remove(pool, &name).await.ok();
            {
                let mut s = lock();
                if s.active == name {
                    s.active.clear();
                    s.path.clear();
                    s.entries.clear();
                }
                s.status = format!("Removed {name}.");
            }
            sync_remotes().await?;
        }

        // --- mounts ---
        CloudCmd::Mount {
            name,
            path,
            cache,
            max_age,
        } => {
            std::fs::create_dir_all(&path).ok();
            let kind = native_fs();
            let vfs = match cache.as_str() {
                "off" => mount::VfsCache::Off,
                "minimal" => mount::VfsCache::Minimal,
                "writes" => mount::VfsCache::Writes,
                _ => mount::VfsCache::Full,
            };
            let args = mount::mount_args_cached(&name, &path, kind, vfs, &max_age);
            // `mount` never returns while the mount is up, so it is spawned and
            // kept: `unmount_path` ends it, which on Windows is the unmount.
            let child = cmd().args(&args).spawn()?;
            mount_children().insert(path.clone(), child);
            let pool = cloud_pool().await?;
            if let Ok(Some(id)) = remotes::id_of(pool, &name).await {
                mount::record(pool, id, &path, kind, false).await.ok();
                mount::set_vfs(pool, id, &path, kind, &cache, &max_age)
                    .await
                    .ok();
                mount::set_status(pool, id, "mounted").await.ok();
            }
            let mut s = lock();
            s.mounts.insert(name.clone(), path.clone());
            s.status = format!("{name} mounted at {path}.");
        }
        CloudCmd::Unmount { name } => {
            let path = lock().mounts.get(&name).cloned();
            if let Some(path) = path {
                unmount_path(&path).await;
                let pool = cloud_pool().await?;
                if let Ok(Some(id)) = remotes::id_of(pool, &name).await {
                    mount::set_status(pool, id, "unmounted").await.ok();
                }
                let mut s = lock();
                s.mounts.remove(&name);
                s.status = format!("{name} unmounted.");
            }
        }

        // --- the tools drawer ---
        CloudCmd::ToolsToggle => {
            let mut s = lock();
            s.tools_open = !s.tools_open;
        }
        CloudCmd::SetToolsTab { name } => lock().tools_tab = name,
        CloudCmd::SetField { key, value } => {
            lock().fields.insert(key, value);
        }
        CloudCmd::RunSync => {
            let f = fields();
            let (src, dst) = (need(&f, "sync_src")?, need(&f, "sync_dst")?);
            let dir = if f.get("sync_direction").map(String::as_str) == Some("bisync") {
                sync::Direction::BiSync
            } else {
                sync::Direction::OneWay
            };
            let bw = f.get("sync_bwlimit").and_then(|s| sync::parse_bwlimit(s));
            let mut args = sync::sync_args(dir, &src, &dst, bw.as_deref());
            let transfers = f
                .get("sync_transfers")
                .and_then(|s| s.trim().parse::<u32>().ok())
                .unwrap_or(4);
            args.extend(sync::concurrency_flags(transfers, transfers.max(2)));
            args.push("--progress".into());
            let out = run_progress("Sync", &args).await;
            finish("sync", &format!("{src} → {dst}"), out).await;
        }
        CloudCmd::RunCopy => {
            let f = fields();
            let (src, dst) = (need(&f, "copy_src")?, need(&f, "copy_dst")?);
            let move_files = f.get("copy_move").map(String::as_str) == Some("true");
            let server_side = f.get("copy_serverside").map(String::as_str) != Some("false");
            let mut args = sync::copy_args(&src, &dst, move_files, server_side, None);
            args.push("--progress".into());
            let out = run_progress(if move_files { "Move" } else { "Copy" }, &args).await;
            finish("copy", &format!("{src} → {dst}"), out).await;
        }
        CloudCmd::RunVerify => {
            let f = fields();
            let (src, dst) = (need(&f, "verify_src")?, need(&f, "verify_dst")?);
            let args = verify::check_args(&src, &dst, false, 8);
            let out = run_progress("Verify", &args).await;
            finish("verify", &format!("{src} ⇄ {dst}"), out).await;
        }
        CloudCmd::RunDedupe => {
            let f = fields();
            let target = need(&f, "dedupe_target")?;
            let mode = match f.get("dedupe_mode").map(String::as_str) {
                Some("first") => verify::DedupeMode::First,
                Some("oldest") => verify::DedupeMode::Oldest,
                Some("largest") => verify::DedupeMode::Largest,
                Some("smallest") => verify::DedupeMode::Smallest,
                Some("rename") => verify::DedupeMode::Rename,
                Some("skip") => verify::DedupeMode::Skip,
                _ => verify::DedupeMode::Newest,
            };
            let out = run_progress("Dedupe", &verify::dedupe_args(&target, mode)).await;
            finish("dedupe", &target, out).await;
        }
        CloudCmd::SaveJob => {
            let f = fields();
            let (src, dst) = (need(&f, "sync_src")?, need(&f, "sync_dst")?);
            let direction = f
                .get("sync_direction")
                .cloned()
                .unwrap_or_else(|| "oneway".into());
            let bw = f.get("sync_bwlimit").and_then(|s| sync::parse_bwlimit(s));
            let interval = match f.get("sync_interval").map(String::as_str) {
                Some("hourly") => 3600,
                Some("daily") => 86_400,
                Some("weekly") => 604_800,
                // "manual" is stored as zero: a job that is never due, but is
                // still a saved pair you can run with one click.
                _ => 0,
            };
            jobs::save_job(
                cloud_pool().await?,
                &src,
                &dst,
                &direction,
                bw.as_deref(),
                interval,
            )
            .await?;
            lock().status = "Saved.".into();
        }
        CloudCmd::DeleteJob { id } => {
            jobs::delete_job(cloud_pool().await?, id).await?;
        }
        CloudCmd::ToggleJob { id, enabled } => {
            jobs::set_enabled(cloud_pool().await?, id, enabled).await?;
        }
        CloudCmd::RunJob { id } => {
            let pool = cloud_pool().await?;
            let job = jobs::list_jobs(pool)
                .await?
                .into_iter()
                .find(|j| j.id == id)
                .ok_or_else(|| anyhow!("no such job"))?;
            let dir = if job.direction == "bisync" {
                sync::Direction::BiSync
            } else {
                sync::Direction::OneWay
            };
            let mut args = sync::sync_args(dir, &job.src, &job.dst, job.bwlimit.as_deref());
            args.push("--progress".into());
            let out = run_progress("Sync", &args).await;
            let ok = out.is_ok();
            finish("sync", &format!("{} → {}", job.src, job.dst), out).await;
            if ok {
                jobs::mark_ran(pool, id).await.ok();
            }
        }
        CloudCmd::LoadUsage => {
            let names: Vec<String> = lock()
                .usage
                .iter()
                .map(|u| u.remote.clone())
                .collect::<Vec<_>>();
            let _ = names;
            let pool = cloud_pool().await?;
            let all = remotes::list(pool).await.unwrap_or_default();
            let mut out = Vec::new();
            for (name, _) in all {
                // `about` is a network round trip per remote and not every
                // backend implements it — a failure is a blank bar, not an
                // error banner.
                if let Ok(json) = run(&quota::about_args(&name)).await {
                    if let Some(a) = quota::parse_about(&json) {
                        out.push(UsageCache {
                            remote: name,
                            used: a.used.unwrap_or(0),
                            total: a.total.unwrap_or(0),
                            free: a.free.unwrap_or(0),
                            trashed: a.trashed.unwrap_or(0),
                        });
                    }
                }
            }
            lock().usage = out;
        }
        CloudCmd::ClearLog => {
            jobs::clear_log(cloud_pool().await?).await?;
        }
        CloudCmd::DeleteLogRow { id } => {
            jobs::delete_log(cloud_pool().await?, id).await?;
        }
    }
    Ok(())
}

/// Where the browser currently is.
fn here() -> (String, String) {
    let s = lock();
    (s.active.clone(), s.path.clone())
}

fn fields() -> std::collections::BTreeMap<String, String> {
    lock().fields.clone()
}

fn need(f: &std::collections::BTreeMap<String, String>, key: &str) -> Result<String> {
    f.get(key)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow!("{} is required", key.replace('_', " ")))
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else {
        format!("{}/{}", dir.trim_end_matches('/'), name)
    }
}

/// FUSE on Linux, WinFsp on Windows, macFUSE on macOS — the crate's enum only
/// distinguishes the two families plus the no-mount HTTP fallback.
fn native_fs() -> mount::FsKind {
    if cfg!(target_os = "windows") {
        mount::FsKind::WinFsp
    } else {
        mount::FsKind::Fuse
    }
}

/// Record what a long call did, and put its output where the report popup can
/// find it.
async fn finish(kind: &str, detail: &str, out: Result<String>) {
    let ok = out.is_ok();
    let text = match out {
        Ok(t) => t,
        Err(e) => e.to_string(),
    };
    log_it(kind, detail, ok).await;
    let mut s = lock();
    s.output = text;
    s.status = if ok {
        format!("{kind} finished.")
    } else {
        format!("{kind} failed.")
    };
}

async fn log_it(kind: &str, detail: &str, ok: bool) {
    if let Ok(pool) = cloud_pool().await {
        jobs::log(pool, kind, detail, ok).await.ok();
    }
}

/// `config dump` → cloud.db, so the app's own bookkeeping never drifts from
/// rclone's config file.
async fn sync_remotes() -> Result<()> {
    let dump = run(&remotes::dump_args()).await.unwrap_or_default();
    let pool = cloud_pool().await?;
    for (name, backend) in remotes::parse_dump(&dump) {
        remotes::upsert(pool, &name, &backend, backend == "crypt")
            .await
            .ok();
    }
    Ok(())
}

async fn load_backends() -> Result<()> {
    if !backends_cache()
        .lock()
        .map(|c| c.is_empty())
        .unwrap_or(true)
    {
        return Ok(());
    }
    let json = run(&providers::providers_args()).await?;
    let parsed = providers::parse_providers(&json);
    if let Ok(mut c) = backends_cache().lock() {
        *c = parsed;
    }
    Ok(())
}

async fn list_current() -> Result<()> {
    let (remote, path) = here();
    if remote.is_empty() {
        lock().entries.clear();
        return Ok(());
    }
    lock().busy = true;
    let result = run(&browse::lsjson_args(&remote, &path)).await;
    lock().busy = false;
    match result {
        Ok(json) => {
            let entries = browse::parse_lsjson(&json)?;
            let mut s = lock();
            s.entries = entries;
            s.status.clear();
        }
        Err(e) => {
            let mut s = lock();
            s.entries.clear();
            s.status = e.to_string();
        }
    }
    Ok(())
}

// ---------------------------------------------------------------- snapshot ---

async fn snapshot() -> Result<CloudState> {
    let bin = rclone_bin();
    let rclone_ok = bin.exists() || which_on_path("rclone");
    let version = if rclone_ok {
        run(&["version".into()])
            .await
            .ok()
            .and_then(|v| v.lines().next().map(|l| l.trim().to_string()))
            .unwrap_or_default()
    } else {
        String::new()
    };

    let pool = cloud_pool().await?;
    let configured = remotes::list(pool).await.unwrap_or_default();

    // Every query runs before the session lock is taken. A `MutexGuard` is not
    // `Send`, and holding one across an `.await` makes the whole dispatch
    // future non-`Send` — which frb's handler will not accept.
    let jobs_rows = jobs::list_jobs(pool).await.unwrap_or_default();
    let log_rows = jobs::recent_log(pool, 60).await.unwrap_or_default();

    let s = lock();

    let remotes_out: Vec<Remote> = configured
        .into_iter()
        .map(|(name, backend)| {
            let mount_path = s.mounts.get(&name).cloned().unwrap_or_default();
            Remote {
                mounted: !mount_path.is_empty(),
                mount_path,
                name,
                backend,
            }
        })
        .collect();

    // Filter, then sort. The other order would sort rows nobody is going to
    // see, which on a bucket with ten thousand objects is the whole cost.
    let needle = s.query.trim().to_lowercase();
    let mut entries: Vec<browse::Entry> = s
        .entries
        .iter()
        .filter(|e| needle.is_empty() || e.name.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    let key = match s.sort_key.as_str() {
        "size" => browse::SortKey::Size,
        "date" => browse::SortKey::Date,
        _ => browse::SortKey::Name,
    };
    browse::sort_entries(&mut entries, key);
    if !s.sort_asc {
        entries.reverse();
    }
    // Directories first regardless of the sort — a listing that interleaves
    // them is a listing you have to read twice.
    entries.sort_by_key(|e| !e.is_dir);

    let backends: Vec<BackendRow> = {
        let cache = backends_cache().lock().ok();
        let q = s.backend_query.trim().to_lowercase();
        cache
            .map(|c| {
                c.iter()
                    .filter(|b| {
                        q.is_empty()
                            || b.name.to_lowercase().contains(&q)
                            || b.description.to_lowercase().contains(&q)
                    })
                    .map(|b| BackendRow {
                        name: b.name.clone(),
                        description: b.description.clone(),
                        oauth: b.is_oauth(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    };

    let (opts, form_oauth) = form_rows(&s);

    let jobs_out: Vec<SyncJobRow> = jobs_rows
        .into_iter()
        .map(|j| SyncJobRow {
            id: j.id,
            src: j.src,
            dst: j.dst,
            direction: j.direction,
            bwlimit: j.bwlimit.unwrap_or_default(),
            interval_s: j.interval_s,
            last_run: j.last_run,
            enabled: j.enabled != 0,
        })
        .collect();

    let xfers: Vec<XferRow> = log_rows
        .into_iter()
        .map(|(id, kind, detail, ok, at)| XferRow {
            id,
            kind,
            detail,
            ok: ok != 0,
            at,
        })
        .collect();

    let f = &s.fields;
    let field = |k: &str, d: &str| f.get(k).cloned().unwrap_or_else(|| d.to_string());

    Ok(CloudState {
        rclone_ok,
        rclone_version: version,
        remotes: remotes_out,
        active_remote: s.active.clone(),
        active_mounted: s.mounts.contains_key(&s.active),
        path: s.path.clone(),
        entries: entries
            .into_iter()
            .map(|e| Entry {
                name: e.name,
                size: e.size,
                is_dir: e.is_dir,
                modified: e.mod_time,
            })
            .collect(),
        query: s.query.clone(),
        sort_key: s.sort_key.clone(),
        sort_asc: s.sort_asc,
        busy: s.busy,
        status: s.status.clone(),

        connect_open: s.connect_open,
        connect_step: s.connect_step,
        connect_edit: s.connect_edit,
        backend_query: s.backend_query.clone(),
        backends,
        form_name: s.form_name.clone(),
        form_backend: s.form_backend.clone(),
        form_oauth,
        authorized: s.authorized,
        show_advanced: s.show_advanced,
        opts,
        connect_error: s.connect_error.clone(),

        tools_open: s.tools_open,
        tools_tab: s.tools_tab.clone(),
        sync_src: field("sync_src", ""),
        sync_dst: field("sync_dst", ""),
        sync_direction: field("sync_direction", "oneway"),
        sync_bwlimit: field("sync_bwlimit", ""),
        sync_transfers: field("sync_transfers", "4"),
        sync_interval: field("sync_interval", "manual"),
        copy_src: field("copy_src", ""),
        copy_dst: field("copy_dst", ""),
        copy_move: field("copy_move", "false") == "true",
        copy_serverside: field("copy_serverside", "true") == "true",
        verify_src: field("verify_src", ""),
        verify_dst: field("verify_dst", ""),
        dedupe_target: field("dedupe_target", ""),
        dedupe_mode: field("dedupe_mode", "newest"),

        jobs: jobs_out,
        xfers,
        usage: s
            .usage
            .iter()
            .map(|u| UsageRow {
                remote: u.remote.clone(),
                used: u.used,
                total: u.total,
                free: u.free,
                trashed: u.trashed,
            })
            .collect(),
        output: s.output.clone(),
    })
}

/// The rows the connect form should show: rclone's schema for the chosen
/// backend, narrowed by the gate option and by whether advanced is on.
fn form_rows(s: &Session) -> (Vec<OptRow>, bool) {
    let Ok(cache) = backends_cache().lock() else {
        return (Vec::new(), false);
    };
    let Some(backend) = providers::find(&cache, &s.form_backend) else {
        return (Vec::new(), false);
    };
    // On s3 and its siblings the `provider` option decides which of the rest
    // even apply, so the form has to be re-narrowed every time it changes.
    let gate = backend
        .gate()
        .and_then(|o| s.form.get(&o.name).cloned())
        .unwrap_or_default();
    let rows = providers::visible(backend, &gate, s.show_advanced)
        .into_iter()
        .map(|o| OptRow {
            value: s
                .form
                .get(&o.name)
                .cloned()
                .unwrap_or_else(|| o.default.clone()),
            name: o.name.clone(),
            help: o.help.clone(),
            kind: o.kind.clone(),
            required: o.required,
            secret: o.secret,
            advanced: o.advanced,
            exclusive: o.exclusive,
            examples: o
                .examples
                .iter()
                .map(|(value, _help)| value.clone())
                .collect(),
        })
        .collect();
    (rows, backend.is_oauth())
}

/// Is the tool resolvable at all? `tool_bin` returns a bundled path that may
/// not exist, in which case the PATH is the fallback.
fn which_on_path(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(bin);
        p.exists() || p.with_extension("exe").exists()
    })
}
