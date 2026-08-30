// The Tools section: a job queue over twenty-odd media operations.
//
// Every operation is the same three moves. `tulipix_tools::exec::plan` turns a
// kind and a JSON spec into a list of `Step`s — an ffmpeg argv, a yt-dlp argv,
// or a native op the runner implements itself. `tulipix_tools::queue` owns
// `tools.db`, which is the queue: submit, claim, progress, pause, cancel,
// retry, and the record of what was downloaded. This file is the worker that
// actually spawns the processes and reports what they say.
//
// The queue is a table rather than a channel on purpose: a transcode that was
// running when the app closed is still there when it opens again.

use anyhow::{Result, anyhow};
use flutter_rust_bridge::frb;

use crate::frb_generated::StreamSink;
use serde_json::{Value, json};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokio::process::Command;

use tulipix_tools::exec::{self, Native, Step};
use tulipix_tools::queue;

use crate::db::tools_pool;

// ------------------------------------------------------------------- state ---

/// One operation, as a card on the landing page.
pub struct OpRow {
    pub kind: String,
    pub label: String,
    pub category: String,
    pub info: String,
}

/// One row of the form the chosen operation asks for.
pub struct Field {
    pub key: String,
    pub label: String,
    /// file | files | folder | text | number | dropdown | slider | toggle.
    pub kind: String,
    pub value: String,
    pub options: Vec<String>,
    pub min: f64,
    pub max: f64,
    pub required: bool,
    pub hint: String,
}

/// A job in the queue.
pub struct Job {
    pub id: i64,
    pub kind: String,
    pub label: String,
    /// queued | running | paused | done | failed | cancelled.
    pub state: String,
    pub progress: f64,
    pub message: String,
}

/// One of the external binaries the section needs, and whether it is there.
pub struct ToolStatus {
    pub name: String,
    /// bundled | path | missing.
    pub source: String,
    pub detail: String,
    pub available: bool,
    pub updatable: bool,
    /// The installed version, where the tool reports one cheaply. Empty
    /// otherwise. yt-dlp's is the number that matters: a download failing with
    /// 403 is nearly always a binary some weeks old, and "bundled" alone never
    /// said that.
    pub version: String,
}

/// A file a download job produced.
pub struct DownloadRow {
    pub name: String,
    pub path: String,
    pub meta: String,
}

/// A key/value line of a media-info report.
pub struct InfoRow {
    pub section: String,
    pub key: String,
    pub value: String,
}

/// One difference between two folders.
pub struct DiffRow {
    /// only-a | only-b | differs | same.
    pub kind: String,
    pub path: String,
    pub detail: String,
}

pub struct ToolsState {
    /// fileops | video | audio | photo | subtitles | queue.
    pub category: String,
    pub query: String,
    pub ops: Vec<OpRow>,

    pub active_op: String,
    pub active_label: String,
    pub active_info: String,
    pub fields: Vec<Field>,

    pub jobs: Vec<Job>,
    pub queue_status: String,
    pub worker_slots: i64,

    pub statuses: Vec<ToolStatus>,
    pub downloads: Vec<DownloadRow>,

    /// Live output of whatever is running.
    pub log: String,
    pub error: String,

    // --- the result popup ---
    pub result_open: bool,
    /// info | diff | text.
    pub result_kind: String,
    pub result_title: String,
    pub result_info: Vec<InfoRow>,
    pub result_diff: Vec<DiffRow>,
    pub result_text: String,
}

// ---------------------------------------------------------------- commands ---

pub enum ToolsCmd {
    Refresh,
    SetCategory {
        name: String,
    },
    Search {
        text: String,
    },
    OpenTool {
        kind: String,
    },
    CloseTool,
    SetField {
        key: String,
        value: String,
    },
    Reset,
    Run,

    /// pause | resume | cancel | retry | remove | up | down.
    QueueAction {
        id: i64,
        action: String,
    },
    ClearQueue,
    SetWorkers {
        slots: i64,
    },
    ClearLog,

    ShowResult {
        id: i64,
    },
    CloseResult,

    ListDownloads,
    RemoveDownload {
        path: String,
    },

    /// Update one of the bundled tools in place. Only yt-dlp is updatable —
    /// it is the one that breaks on its own schedule as sites change, which is
    /// why it has a self-update channel separate from the app's.
    UpdateTool {
        name: String,
    },
}

pub enum ToolsEvent {
    Progress {
        id: i64,
        progress: f64,
        message: String,
    },
    Log {
        line: String,
    },
    Finished {
        id: i64,
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
    category: String,
    query: String,
    active: String,
    /// The form's current values for the active op.
    form: std::collections::BTreeMap<String, String>,
    error: String,
    result_open: bool,
    result_id: i64,
    downloads: Vec<(String, String, String)>,
}

fn session() -> &'static Mutex<Session> {
    static S: OnceLock<Mutex<Session>> = OnceLock::new();
    S.get_or_init(|| {
        Mutex::new(Session {
            category: "fileops".into(),
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

/// The running log, shared with whatever is on screen. Capped because a long
/// ffmpeg run writes a line a second for an hour and nobody scrolls back.
fn log() -> &'static Mutex<String> {
    static L: OnceLock<Mutex<String>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(String::new()))
}

const LOG_CAP: usize = 60_000;

fn log_push(line: &str) {
    if let Ok(mut g) = log().lock() {
        if g.len() > LOG_CAP {
            // Drop the oldest half rather than clearing: the tail is what
            // matters, and clearing loses the error that just scrolled past.
            let cut = g.len() / 2;
            let at = g[cut..].find('\n').map(|i| cut + i + 1).unwrap_or(cut);
            *g = g[at..].to_string();
        }
        g.push_str(line);
        g.push('\n');
    }
    emit(ToolsEvent::Log {
        line: line.to_string(),
    });
}

fn log_reset(header: &str) {
    if let Ok(mut g) = log().lock() {
        g.clear();
        g.push_str(header);
        g.push('\n');
    }
}

/// Jobs the user asked to stop. Checked between steps and on every progress
/// line — a cancelled transcode should not finish the file it is on.
fn stopped() -> &'static Mutex<std::collections::HashSet<i64>> {
    static S: OnceLock<Mutex<std::collections::HashSet<i64>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(std::collections::HashSet::new()))
}

fn is_stopped(id: i64) -> bool {
    stopped().lock().map(|g| g.contains(&id)).unwrap_or(false)
}

fn events() -> &'static OnceLock<StreamSink<ToolsEvent>> {
    static E: OnceLock<StreamSink<ToolsEvent>> = OnceLock::new();
    &E
}

fn emit(e: ToolsEvent) {
    if let Some(sink) = events().get() {
        let _ = sink.add(e);
    }
}

/// Cached results of the two jobs whose output is a report rather than a file.
fn reports() -> &'static Mutex<std::collections::HashMap<i64, Report>> {
    static R: OnceLock<Mutex<std::collections::HashMap<i64, Report>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[frb(ignore)]
#[derive(Debug, Clone)]
enum Report {
    Info(Vec<(String, String, String)>),
    Diff(Vec<(String, String, String)>),
    Text(String),
}

// ---------------------------------------------------------------- exported ---

pub async fn tools_dispatch(cmd: ToolsCmd) -> Result<ToolsState> {
    apply(cmd).await?;
    snapshot().await
}

#[frb(sync)]
pub fn tools_events(sink: StreamSink<ToolsEvent>) {
    let _ = events().set(sink);
}

/// Start the worker loop. Called once, after the section first opens — the
/// queue survives restarts, so there may be work waiting before anyone clicks
/// anything.
pub async fn tools_start_worker() -> Result<()> {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return Ok(());
    }
    let pool = tools_pool().await?;
    // A job left "running" by a process that died is not running. Put it back
    // in the queue before anything claims new work.
    queue::recover_stale(pool).await.ok();

    tokio::spawn(async move {
        loop {
            let Ok(pool) = tools_pool().await else {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                continue;
            };
            match queue::claim_next(pool).await {
                Ok(Some(id)) => {
                    let spec = queue::job_spec(pool, id).await.ok().flatten();
                    let kind = job_kind(pool, id).await.unwrap_or_default();
                    if let Some(spec) = spec {
                        let ok = run_job(pool, id, &kind, &spec).await;
                        let message = match &ok {
                            Ok(m) => m.clone(),
                            Err(e) => e.to_string(),
                        };
                        queue::complete(pool, id, ok.is_ok(), Some(&message))
                            .await
                            .ok();
                        stopped().lock().ok().map(|mut g| g.remove(&id));
                        emit(ToolsEvent::Finished {
                            id,
                            ok: ok.is_ok(),
                            message,
                        });
                    }
                }
                // Nothing to do. A poll rather than a notify because the queue
                // is a table other processes could also write to.
                _ => tokio::time::sleep(std::time::Duration::from_millis(700)).await,
            }
        }
    });
    Ok(())
}

async fn job_kind(pool: &sqlx::SqlitePool, id: i64) -> Result<String> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT kind FROM jobs WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?
            .unwrap_or_default(),
    )
}

// ------------------------------------------------------------------ runner ---

fn bin(name: &str) -> std::path::PathBuf {
    tulipix_core::thumbs::tool_bin(name)
}

/// `NoWindow` is implemented for `std::process::Command`; tokio's wraps one, so
/// the flag has to be set through the inner handle. A no-op off Windows.
fn no_window(c: &mut Command) {
    use tulipix_core::proc::NoWindow;
    c.as_std_mut().no_window();
}

fn quiet(path: &std::path::Path) -> Command {
    let mut c = Command::new(path);
    no_window(&mut c);
    c
}

/// Run one job to completion, reporting progress across its steps.
async fn run_job(pool: &sqlx::SqlitePool, id: i64, kind: &str, spec_json: &str) -> Result<String> {
    let spec: Value = serde_json::from_str(spec_json).unwrap_or(json!({}));
    let steps = exec::plan(kind, &spec)?;
    if steps.is_empty() {
        return Ok("Nothing to do.".into());
    }
    log_reset(&format!("── {} ──", label_of(kind)));

    // Progress is shared across the steps: a two-step job that jumped back to
    // zero halfway would look like it had restarted.
    let span = 1.0 / steps.len() as f64;
    let mut last = String::new();
    for (i, step) in steps.into_iter().enumerate() {
        if is_stopped(id) {
            return Err(anyhow!("cancelled"));
        }
        let base = i as f64 * span;
        last = run_step(pool, id, step, base, span).await?;
    }
    queue::set_progress(pool, id, 1.0, Some("done")).await.ok();
    Ok(if last.is_empty() {
        "Finished.".into()
    } else {
        last
    })
}

async fn run_step(
    pool: &sqlx::SqlitePool,
    id: i64,
    step: Step,
    base: f64,
    span: f64,
) -> Result<String> {
    match step {
        Step::Ffmpeg {
            args,
            duration_input,
            duration_s,
        } => {
            let total = match duration_s {
                Some(d) => d,
                None => match duration_input {
                    Some(p) => probe_duration(&p).await,
                    None => 0.0,
                },
            };
            let mut argv: Vec<String> = vec!["-hide_banner".into(), "-y".into()];
            argv.extend(args);
            argv.push("-progress".into());
            argv.push("pipe:1".into());
            argv.push("-nostats".into());
            spawn_tracked(pool, id, bin("ffmpeg"), &argv, base, span, move |line| {
                exec::parse_ffmpeg_progress(line, total)
            })
            .await?;
            Ok(String::new())
        }
        Step::YtDlp { args } => {
            let out = spawn_tracked(pool, id, bin("yt-dlp"), &args, base, span, |line| {
                exec::parse_ytdlp_progress(line)
            })
            .await?;
            // yt-dlp names the file it wrote; that is what the Downloads list
            // is built from, and it is the only place the name appears.
            if let Some(dest) = out.lines().rev().find_map(exec::parse_ytdlp_dest) {
                queue::record_download(pool, id, &dest).await.ok();
                return Ok(dest);
            }
            Ok(String::new())
        }
        Step::Native(n) => run_native(pool, id, n, base, span).await,
    }
}

/// Spawn a child, stream both pipes into the log, and turn whatever the
/// `progress` closure recognises into a queue update.
async fn spawn_tracked<F>(
    pool: &sqlx::SqlitePool,
    id: i64,
    exe: std::path::PathBuf,
    args: &[String],
    base: f64,
    span: f64,
    progress: F,
) -> Result<String>
where
    F: Fn(&str) -> Option<f64> + Send + 'static,
{
    use tokio::io::{AsyncBufReadExt, BufReader};

    let mut child = quiet(&exe)
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow!("{} could not be started: {e}", exe.display()))?;

    let mut collected = String::new();
    let stderr = child.stderr.take();
    let stdout = child.stdout.take();

    // stderr in the background: ffmpeg says everything interesting there, and
    // a full pipe buffer deadlocks the child.
    let err_task = stderr.map(|e| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(e).lines();
            let mut tail = String::new();
            while let Ok(Some(line)) = lines.next_line().await {
                log_push(&line);
                tail = line;
            }
            tail
        })
    });

    if let Some(out) = stdout {
        let mut lines = BufReader::new(out).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            if is_stopped(id) {
                child.start_kill().ok();
                break;
            }
            if let Some(frac) = progress(&line) {
                let at = (base + frac.clamp(0.0, 1.0) * span).clamp(0.0, 1.0);
                queue::set_progress(pool, id, at, None).await.ok();
                emit(ToolsEvent::Progress {
                    id,
                    progress: at,
                    message: String::new(),
                });
            } else {
                log_push(&line);
            }
            if collected.len() < LOG_CAP {
                collected.push_str(&line);
                collected.push('\n');
            }
        }
    }

    let status = child.wait().await?;
    let tail = match err_task {
        Some(t) => t.await.unwrap_or_default(),
        None => String::new(),
    };
    if is_stopped(id) {
        return Err(anyhow!("cancelled"));
    }
    if !status.success() {
        return Err(anyhow!(if tail.is_empty() {
            format!("{} exited {}", exe.display(), status)
        } else {
            tail
        }));
    }
    Ok(collected)
}

async fn probe_duration(path: &str) -> f64 {
    let out = quiet(&bin("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            path,
        ])
        .output()
        .await;
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// The operations the worker implements itself rather than by spawning a
/// dedicated tool.
async fn run_native(
    pool: &sqlx::SqlitePool,
    id: i64,
    n: Native,
    base: f64,
    span: f64,
) -> Result<String> {
    match n {
        Native::Hash {
            files,
            algo,
            manifest,
        } => {
            let total = files.len().max(1);
            let mut lines = Vec::new();
            for (i, f) in files.iter().enumerate() {
                if is_stopped(id) {
                    return Err(anyhow!("cancelled"));
                }
                // The crate hashes by path and only speaks SHA-256; `algo` is
                // carried through the spec for the manifest header.
                let digest =
                    tulipix_tools::hash::sha256_file_hex(f).map_err(|e| anyhow!("{f}: {e}"))?;
                let _ = &algo;
                lines.push(format!("{digest}  {f}"));
                log_push(lines.last().map(String::as_str).unwrap_or(""));
                let at = base + (i + 1) as f64 / total as f64 * span;
                queue::set_progress(pool, id, at.clamp(0.0, 1.0), None)
                    .await
                    .ok();
            }
            let body = lines.join("\n");
            match manifest {
                Some(path) if !path.trim().is_empty() => {
                    std::fs::write(&path, &body)?;
                    Ok(format!("Wrote {path}"))
                }
                _ => {
                    remember(id, Report::Text(body));
                    Ok("Hashed.".into())
                }
            }
        }
        Native::FolderDiff { a, b } => {
            let rows = folder_diff(&a, &b).await;
            let n = rows.len();
            remember(id, Report::Diff(rows));
            Ok(format!("{n} differences."))
        }
        Native::Rename {
            dir,
            pattern,
            start,
        } => {
            // The crate plans; it deliberately does not touch the disk. Sorted
            // by name so `{n}` numbers the way the folder reads.
            let mut files: Vec<(String, std::collections::HashMap<String, String>)> = Vec::new();
            let mut names: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
                .map_err(|e| anyhow!("{dir}: {e}"))?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect();
            names.sort();
            for p in &names {
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                let mut tags = std::collections::HashMap::new();
                tags.insert("name".to_string(), stem);
                files.push((p.to_string_lossy().to_string(), tags));
            }
            let preview = tulipix_tools::rename::dry_run(&files, &pattern, start);
            if !preview.collisions.is_empty() {
                // Renaming into a collision loses a file. The plan is rejected
                // whole rather than half-applied.
                return Err(anyhow!(
                    "{} names would collide — adjust the pattern",
                    preview.collisions.len()
                ));
            }
            let root = std::path::Path::new(&dir);
            let mut done = 0usize;
            for (from, to) in &preview.renames {
                if is_stopped(id) {
                    return Err(anyhow!("cancelled"));
                }
                if std::fs::rename(from, root.join(to)).is_ok() {
                    done += 1;
                }
            }
            Ok(format!("Renamed {done} of {}.", preview.renames.len()))
        }
        Native::Merge { inputs, output } => {
            // ffmpeg's concat demuxer needs a list file, not an argv.
            let list = std::env::temp_dir().join(format!("tulipix-merge-{id}.txt"));
            let body: String = inputs
                .iter()
                .map(|p| format!("file '{}'\n", p.replace('\'', "'\\''")))
                .collect();
            std::fs::write(&list, body)?;
            let args: Vec<String> = vec![
                "-hide_banner".into(),
                "-y".into(),
                "-f".into(),
                "concat".into(),
                "-safe".into(),
                "0".into(),
                "-i".into(),
                list.to_string_lossy().to_string(),
                "-c".into(),
                "copy".into(),
                output.clone(),
                "-progress".into(),
                "pipe:1".into(),
                "-nostats".into(),
            ];
            let r = spawn_tracked(pool, id, bin("ffmpeg"), &args, base, span, |_| None).await;
            std::fs::remove_file(&list).ok();
            r?;
            Ok(output)
        }
        Native::Transcribe {
            input,
            output,
            translate,
            language,
        } => {
            // Whisper wants 16 kHz mono PCM; anything else it resamples badly.
            let wav = std::env::temp_dir().join(format!("tulipix-stt-{id}.wav"));
            let wav_s = wav.to_string_lossy().to_string();
            let pre: Vec<String> = vec![
                "-hide_banner".into(),
                "-y".into(),
                "-i".into(),
                input.clone(),
                "-ar".into(),
                "16000".into(),
                "-ac".into(),
                "1".into(),
                "-c:a".into(),
                "pcm_s16le".into(),
                wav_s.clone(),
                "-progress".into(),
                "pipe:1".into(),
                "-nostats".into(),
            ];
            spawn_tracked(pool, id, bin("ffmpeg"), &pre, base, span * 0.3, |_| None).await?;

            let model = whisper_model().ok_or_else(|| anyhow!("no whisper model is bundled"))?;
            let mut args: Vec<String> = vec![
                "-m".into(),
                model.to_string_lossy().to_string(),
                "-f".into(),
                wav_s.clone(),
                "-osrt".into(),
                "-of".into(),
                output.trim_end_matches(".srt").to_string(),
            ];
            if translate {
                args.push("-tr".into());
            }
            if language != "auto" {
                args.push("-l".into());
                args.push(language);
            }
            let r = spawn_tracked(
                pool,
                id,
                bin("whisper-cli"),
                &args,
                base + span * 0.3,
                span * 0.7,
                |_| None,
            )
            .await;
            std::fs::remove_file(&wav).ok();
            r?;
            Ok(output)
        }
        Native::MediaInfo { input, output } => {
            let rows = media_info(&input).await;
            let body: String = rows
                .iter()
                .map(|(s, k, v)| format!("{s}\t{k}\t{v}\n"))
                .collect();
            if !output.trim().is_empty() {
                std::fs::write(&output, &body).ok();
            }
            remember(id, Report::Info(rows));
            Ok(if output.trim().is_empty() {
                "Inspected.".into()
            } else {
                output
            })
        }
        Native::ContactSheet {
            input,
            output,
            cols,
            rows,
        } => {
            let dur = probe_duration(&input).await;
            let tiles = (cols * rows).max(1);
            // One frame per even slice of the running time, so the sheet is a
            // summary rather than the first N seconds.
            let every = if dur > 0.0 { dur / tiles as f64 } else { 1.0 };
            let filter = format!("fps=1/{every:.4},scale=320:-1,tile={cols}x{rows}");
            let args: Vec<String> = vec![
                "-hide_banner".into(),
                "-y".into(),
                "-i".into(),
                input,
                "-vf".into(),
                filter,
                "-frames:v".into(),
                "1".into(),
                output.clone(),
                "-progress".into(),
                "pipe:1".into(),
                "-nostats".into(),
            ];
            spawn_tracked(pool, id, bin("ffmpeg"), &args, base, span, |_| None).await?;
            Ok(output)
        }
        Native::CacheClean => {
            let freed = clean_cache();
            Ok(format!("Freed {}.", human_bytes(freed)))
        }
    }
}

fn remember(id: i64, r: Report) {
    if let Ok(mut g) = reports().lock() {
        g.insert(id, r);
    }
}

fn whisper_model() -> Option<std::path::PathBuf> {
    let dir = bin("whisper-cli").parent()?.to_path_buf();
    std::fs::read_dir(&dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().map(|e| e == "bin").unwrap_or(false))
}

async fn media_info(input: &str) -> Vec<(String, String, String)> {
    let out = quiet(&bin("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
            input,
        ])
        .output()
        .await;
    let Some(json) = out
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
    else {
        return Vec::new();
    };
    let mut rows = Vec::new();
    if let Some(f) = json.get("format").and_then(|v| v.as_object()) {
        for (k, v) in f {
            if let Some(text) = scalar(v) {
                rows.push(("Format".to_string(), k.clone(), text));
            }
        }
    }
    if let Some(streams) = json.get("streams").and_then(|v| v.as_array()) {
        for (i, st) in streams.iter().enumerate() {
            let kind = st
                .get("codec_type")
                .and_then(|v| v.as_str())
                .unwrap_or("stream");
            let section = format!("{} {i}", title_case(kind));
            if let Some(o) = st.as_object() {
                for (k, v) in o {
                    if let Some(text) = scalar(v) {
                        rows.push((section.clone(), k.clone(), text));
                    }
                }
            }
        }
    }
    rows
}

fn scalar(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn title_case(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

async fn folder_diff(a: &str, b: &str) -> Vec<(String, String, String)> {
    let map_a = hash_tree(a);
    let map_b = hash_tree(b);
    let mut rows = Vec::new();
    for (rel, digest) in &map_a {
        match map_b.get(rel) {
            None => rows.push(("only-a".into(), rel.clone(), "only in A".into())),
            Some(other) if other != digest => {
                rows.push(("differs".into(), rel.clone(), "contents differ".into()))
            }
            _ => {}
        }
    }
    for rel in map_b.keys() {
        if !map_a.contains_key(rel) {
            rows.push(("only-b".into(), rel.clone(), "only in B".into()));
        }
    }
    rows.sort_by(|x, y| x.1.cmp(&y.1));
    rows
}

/// relative path → digest, for every file under a root.
fn hash_tree(root: &str) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let base = std::path::Path::new(root);
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(base) {
                if let Ok(d) = tulipix_tools::hash::sha256_file_hex(&path.to_string_lossy()) {
                    out.insert(rel.to_string_lossy().to_string(), d);
                }
            }
        }
    }
    out
}

fn clean_cache() -> u64 {
    let Some(dir) = tulipix_core::paths::cache_dir() else {
        return 0;
    };
    let mut freed = 0u64;
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return 0;
    };
    for entry in rd.filter_map(|e| e.ok()) {
        let path = entry.path();
        let size = entry.metadata().map(|m| m.len()).unwrap_or(0);
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path).is_ok()
        } else {
            std::fs::remove_file(&path).is_ok()
        };
        if removed {
            freed += size;
        }
    }
    freed
}

fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    format!("{v:.1} {}", UNITS[u])
}

// ---------------------------------------------------------------- commands ---

async fn apply(cmd: ToolsCmd) -> Result<()> {
    match cmd {
        ToolsCmd::Refresh => {
            tools_start_worker().await.ok();
        }
        ToolsCmd::SetCategory { name } => {
            let mut s = lock();
            s.category = name;
            s.query.clear();
        }
        ToolsCmd::Search { text } => lock().query = text,
        ToolsCmd::OpenTool { kind } => {
            let defaults: std::collections::BTreeMap<String, String> = fields_for(&kind)
                .into_iter()
                .map(|f| (f.key, f.value))
                .collect();
            let mut s = lock();
            s.active = kind;
            s.form = defaults;
            s.error.clear();
        }
        ToolsCmd::CloseTool => {
            let mut s = lock();
            s.active.clear();
            s.form.clear();
            s.error.clear();
        }
        ToolsCmd::SetField { key, value } => {
            lock().form.insert(key, value);
        }
        ToolsCmd::Reset => {
            let kind = lock().active.clone();
            let defaults: std::collections::BTreeMap<String, String> = fields_for(&kind)
                .into_iter()
                .map(|f| (f.key, f.value))
                .collect();
            let mut s = lock();
            s.form = defaults;
            s.error.clear();
        }
        ToolsCmd::Run => {
            let (kind, form) = {
                let s = lock();
                (s.active.clone(), s.form.clone())
            };
            if kind.is_empty() {
                return Ok(());
            }
            // Everything the form is missing, before anything is queued: a job
            // that fails on its first step for a blank field should never have
            // been accepted.
            let schema = fields_for(&kind);
            for f in &schema {
                if f.required
                    && form
                        .get(&f.key)
                        .map(|v| v.trim().is_empty())
                        .unwrap_or(true)
                {
                    lock().error = format!("{} is required.", f.label);
                    return Ok(());
                }
            }
            let spec = spec_from(&kind, &form, &schema);
            let spec_json = serde_json::to_string(&spec)?;
            // Reject the plan here too: `plan` is where a bad number or an
            // unreadable path is noticed, and noticing it now means an error
            // under the form rather than a red row in the queue.
            exec::plan(&kind, &spec).map_err(|e| {
                lock().error = e.to_string();
                e
            })?;

            let pool = tools_pool().await?;
            if queue::has_active_duplicate(pool, &kind, &spec_json).await? {
                lock().error = "That exact job is already queued.".into();
                return Ok(());
            }
            queue::submit(pool, &kind, &spec_json, 0).await?;
            tools_start_worker().await.ok();
            let mut s = lock();
            s.error.clear();
            s.active.clear();
        }

        ToolsCmd::QueueAction { id, action } => {
            let pool = tools_pool().await?;
            match action.as_str() {
                "pause" => queue::pause(pool, id).await?,
                "resume" => queue::resume(pool, id).await?,
                "cancel" => {
                    stopped().lock().ok().map(|mut g| g.insert(id));
                    queue::cancel(pool, id).await?;
                }
                "retry" => {
                    stopped().lock().ok().map(|mut g| g.remove(&id));
                    queue::retry(pool, id).await?;
                }
                "remove" => {
                    stopped().lock().ok().map(|mut g| g.insert(id));
                    queue::remove(pool, id).await?;
                }
                "up" => queue::reorder(pool, id, true).await?,
                "down" => queue::reorder(pool, id, false).await?,
                other => anyhow::bail!("unknown queue action: {other}"),
            }
        }
        ToolsCmd::ClearQueue => {
            queue::clear_all(tools_pool().await?).await?;
        }
        ToolsCmd::SetWorkers { slots } => {
            queue::set_worker_slots(tools_pool().await?, slots.clamp(1, 8)).await?;
        }
        ToolsCmd::ClearLog => {
            if let Ok(mut g) = log().lock() {
                g.clear();
            }
        }

        ToolsCmd::ShowResult { id } => {
            let mut s = lock();
            s.result_open = true;
            s.result_id = id;
        }
        ToolsCmd::CloseResult => {
            let mut s = lock();
            s.result_open = false;
            s.result_id = 0;
        }

        ToolsCmd::ListDownloads => {
            let pool = tools_pool().await?;
            let paths = queue::downloads_list(pool, 200).await.unwrap_or_default();
            let rows = paths
                .into_iter()
                .filter(|p| std::path::Path::new(p).exists())
                .map(|p| {
                    let path = std::path::Path::new(&p);
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_else(|| p.clone());
                    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
                    (name, p.clone(), human_bytes(size))
                })
                .collect();
            lock().downloads = rows;
        }
        ToolsCmd::RemoveDownload { path } => {
            std::fs::remove_file(&path).ok();
            queue::forget_download(tools_pool().await?, &path)
                .await
                .ok();
            lock().downloads.retain(|(_, p, _)| p != &path);
        }
        ToolsCmd::UpdateTool { name } => {
            if name != "yt-dlp" {
                anyhow::bail!("{name} has no update channel");
            }
            // `spawn_blocking`: the updater is blocking reqwest plus a 30 MB
            // download, and this is an async dispatch handler.
            //
            // Nothing is written to the session on success. `error` is drawn as
            // a red banner, and "updated to 2026.08.19" is not an error; the
            // snapshot this dispatch returns carries the new `version` on the
            // yt-dlp row, which is the same fact in the place that already
            // shows it. A failure does belong in the banner.
            match tokio::task::spawn_blocking(tulipix_core::updater::update_ytdlp_now).await {
                Ok(Ok(_)) => {}
                Ok(Err(e)) => lock().error = format!("yt-dlp update failed: {e}"),
                Err(e) => lock().error = format!("yt-dlp update failed: {e}"),
            }
        }
    }
    Ok(())
}

/// Turn the form's flat strings into the JSON the planner expects, using the
/// schema to decide what each one actually is.
fn spec_from(
    kind: &str,
    form: &std::collections::BTreeMap<String, String>,
    schema: &[Field],
) -> Value {
    let mut map = serde_json::Map::new();
    for f in schema {
        let Some(raw) = form.get(&f.key) else {
            continue;
        };
        let raw = raw.trim();
        if raw.is_empty() {
            continue;
        }
        let value = match f.kind.as_str() {
            "toggle" => Value::Bool(raw == "true"),
            "number" | "slider" => raw
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or_else(|| Value::String(raw.to_string())),
            // A multi-file picker hands back one newline-joined string; the
            // planner wants an array.
            "files" => Value::Array(
                raw.lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .map(|l| Value::String(l.to_string()))
                    .collect(),
            ),
            _ => Value::String(raw.to_string()),
        };
        map.insert(f.key.clone(), value);
    }
    let _ = kind;
    Value::Object(map)
}

// ------------------------------------------------------------------ schema ---

fn label_of(kind: &str) -> &'static str {
    match kind {
        "compress_video" => "Compress video",
        "compress_audio" => "Compress audio",
        "compress_photo" => "Compress photo",
        "convert" => "Convert",
        "trim" => "Trim",
        "resize" => "Resize",
        "thumbnail" => "Thumbnail",
        "extract" => "Extract audio",
        "normalize" => "Normalise",
        "watermark" => "Watermark",
        "burn_subs" => "Burn subtitles",
        "split" => "Split",
        "merge" => "Merge",
        "download" => "Download",
        "download_playlist" => "Playlist download",
        "download_live" => "Live record",
        "hash" => "Hash",
        "folder_diff" => "Folder diff",
        "rename" => "Rename",
        "transcribe" => "Transcribe",
        "mediainfo" => "Media info",
        "contact_sheet" => "Contact sheet",
        "cache_clean" => "Clean cache",
        _ => "Job",
    }
}

fn info_of(kind: &str) -> &'static str {
    match kind {
        "compress_video" => {
            "Re-encode to a smaller file. CRF is the quality dial: lower is better and bigger, and every +6 roughly halves the size."
        }
        "compress_audio" => {
            "Re-encode audio at a chosen bitrate. Opus is the best of these per kilobit; mp3 is the one everything plays."
        }
        "compress_photo" => {
            "Re-encode an image. AVIF is smallest, WebP is widely supported, JPEG is universal."
        }
        "convert" => {
            "Change container or codec. Picking an audio extension on a video file extracts the audio."
        }
        "trim" => {
            "Cut a section out. Lossless snaps to keyframes and does not re-encode, so it is fast and the cut may land a moment early."
        }
        "resize" => {
            "Scale an image or video. Fit keeps the aspect ratio inside the box; exact will distort."
        }
        "thumbnail" => "Grab one frame as an image.",
        "extract" => "Pull an audio or subtitle track out into its own file.",
        "normalize" => "Level the loudness to a target. -23 LUFS is the broadcast standard.",
        "watermark" => "Burn a line of text into the bottom-right corner.",
        "burn_subs" => "Render a subtitle file into the picture, permanently.",
        "split" => "Cut into fixed-length segments.",
        "merge" => {
            "Join files end to end. They must share a codec — this copies streams rather than re-encoding."
        }
        "download" => "Fetch a video from any of the sites yt-dlp supports.",
        "download_playlist" => "Fetch every video in a playlist.",
        "download_live" => "Record a live stream until it ends or you cancel.",
        "hash" => "SHA-256 every file, to a manifest or to the report.",
        "folder_diff" => "Compare two folders by content, not by timestamp.",
        "rename" => {
            "Rename in bulk from a pattern. Collisions are refused rather than half-applied."
        }
        "transcribe" => "Speech to an SRT subtitle file, locally, with Whisper.",
        "mediainfo" => "Report every stream, codec and bitrate.",
        "contact_sheet" => "One image of evenly spaced frames.",
        "cache_clean" => "Delete the app's thumbnail and temporary files.",
        _ => "",
    }
}

fn category_of(kind: &str) -> &'static str {
    match kind {
        "rename" | "merge" | "split" | "hash" | "folder_diff" | "cache_clean" => "fileops",
        "compress_video" | "trim" | "convert" | "thumbnail" | "resize" | "contact_sheet"
        | "download" | "download_live" | "download_playlist" => "video",
        "compress_audio" | "normalize" | "extract" => "audio",
        "compress_photo" | "watermark" => "photo",
        "transcribe" | "burn_subs" => "subtitles",
        _ => "queue",
    }
}

/// How many operations the catalogue offers. Home's hub tile prints it, and it
/// must be the same number the Tools page lists rather than a second count.
pub(crate) fn op_count() -> i64 {
    ALL_OPS.len() as i64
}

const ALL_OPS: &[&str] = &[
    "rename",
    "merge",
    "split",
    "hash",
    "folder_diff",
    "cache_clean",
    "compress_video",
    "trim",
    "convert",
    "thumbnail",
    "resize",
    "contact_sheet",
    "download",
    "download_live",
    "download_playlist",
    "compress_audio",
    "normalize",
    "extract",
    "compress_photo",
    "watermark",
    "transcribe",
    "burn_subs",
    "mediainfo",
];

fn f(
    key: &str,
    label: &str,
    kind: &str,
    value: &str,
    required: bool,
    hint: &str,
    options: &[&str],
    min: f64,
    max: f64,
) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        kind: kind.into(),
        value: value.into(),
        options: options.iter().map(|s| s.to_string()).collect(),
        min,
        max,
        required,
        hint: hint.into(),
    }
}

/// The form one operation asks for. Kept here rather than derived from the
/// planner because the planner reads a spec and does not describe one.
fn fields_for(kind: &str) -> Vec<Field> {
    let file = |k: &str, l: &str| f(k, l, "file", "", true, "", &[], 0.0, 0.0);
    let out = || {
        f(
            "output",
            "Output (blank = beside source)",
            "text",
            "",
            false,
            "auto-named next to the source",
            &[],
            0.0,
            0.0,
        )
    };
    match kind {
        "compress_video" => vec![
            file("input", "Source video"),
            f(
                "codec",
                "Codec",
                "dropdown",
                "h264",
                true,
                "",
                &["h264", "h265", "av1"],
                0.0,
                0.0,
            ),
            f(
                "crf",
                "Quality — CRF (lower = better, bigger)",
                "slider",
                "23",
                false,
                "~18 near-lossless · 23 default · 28 small · each +6 ≈ half the size",
                &[],
                18.0,
                35.0,
            ),
            out(),
        ],
        "compress_audio" => vec![
            file("input", "Source audio"),
            f(
                "codec",
                "Codec",
                "dropdown",
                "mp3",
                true,
                "",
                &["mp3", "aac", "opus", "vorbis"],
                0.0,
                0.0,
            ),
            f(
                "kbps",
                "Bitrate (kbps)",
                "slider",
                "192",
                false,
                "",
                &[],
                64.0,
                320.0,
            ),
            out(),
        ],
        "compress_photo" => vec![
            file("input", "Source image"),
            f(
                "format",
                "Format",
                "dropdown",
                "jpeg",
                true,
                "",
                &["jpeg", "webp", "avif"],
                0.0,
                0.0,
            ),
            f(
                "quality",
                "Quality",
                "slider",
                "82",
                false,
                "1–100",
                &[],
                1.0,
                100.0,
            ),
            out(),
        ],
        "convert" => vec![
            file("input", "Source file"),
            f(
                "target_ext",
                "Convert to",
                "dropdown",
                "mp4",
                true,
                "",
                &[
                    "mp4", "mkv", "webm", "mp3", "m4a", "opus", "flac", "png", "jpg", "webp",
                ],
                0.0,
                0.0,
            ),
            out(),
        ],
        "trim" => vec![
            file("input", "Source video"),
            f(
                "start_s",
                "Start (seconds)",
                "number",
                "0",
                true,
                "e.g. 12.5",
                &[],
                0.0,
                0.0,
            ),
            f(
                "end_s",
                "End (seconds)",
                "number",
                "",
                true,
                "e.g. 48",
                &[],
                0.0,
                0.0,
            ),
            f(
                "lossless",
                "Lossless cut (keyframe-aligned)",
                "toggle",
                "true",
                false,
                "fast, no re-encode",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "resize" => vec![
            file("input", "Source image/video"),
            f(
                "mode",
                "Aspect",
                "dropdown",
                "fit",
                false,
                "fit keeps the aspect inside W×H · exact may distort · width/height scale one edge",
                &["fit", "exact", "width", "height"],
                0.0,
                0.0,
            ),
            f("w", "Width (px)", "number", "1920", true, "", &[], 0.0, 0.0),
            f(
                "h",
                "Height (px)",
                "number",
                "1080",
                true,
                "",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "thumbnail" => vec![
            file("input", "Source video"),
            f(
                "at_s",
                "At timestamp (seconds)",
                "number",
                "1",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
            f(
                "width",
                "Thumbnail width (px)",
                "number",
                "320",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "extract" => vec![
            file("input", "Source video"),
            f(
                "stream",
                "Stream",
                "dropdown",
                "audio",
                true,
                "",
                &["audio", "subtitle"],
                0.0,
                0.0,
            ),
            f(
                "index",
                "Track index",
                "number",
                "0",
                false,
                "0 = first",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "normalize" => vec![
            file("input", "Source audio/video"),
            f(
                "lufs",
                "Target loudness (LUFS)",
                "number",
                "-23",
                false,
                "EBU R128 = -23",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "watermark" => vec![
            file("input", "Source image/video"),
            f(
                "text",
                "Watermark text",
                "text",
                "",
                true,
                "shown bottom-right",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "burn_subs" => vec![
            file("input", "Source video"),
            f(
                "sub",
                "Subtitle file (.srt/.ass)",
                "file",
                "",
                true,
                "",
                &[],
                0.0,
                0.0,
            ),
            out(),
        ],
        "split" => vec![
            file("input", "Source video"),
            f(
                "every_s",
                "Segment length (seconds)",
                "number",
                "60",
                true,
                "",
                &[],
                0.0,
                0.0,
            ),
            f(
                "output",
                "Output template (blank = beside source)",
                "text",
                "",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
        ],
        "merge" => vec![
            f(
                "inputs",
                "Source files (one path per line)",
                "files",
                "",
                true,
                "",
                &[],
                0.0,
                0.0,
            ),
            f("output", "Output file", "text", "", true, "", &[], 0.0, 0.0),
        ],
        "download" | "download_playlist" | "download_live" => vec![
            f(
                "url",
                "URL",
                "text",
                "",
                true,
                "YouTube / Vimeo / 1800+ sites",
                &[],
                0.0,
                0.0,
            ),
            f(
                "audio_only",
                "Audio only",
                "toggle",
                "false",
                false,
                "extract audio",
                &[],
                0.0,
                0.0,
            ),
            f(
                "max_height",
                "Resolution",
                "dropdown",
                "1080",
                false,
                "“Best” grabs the highest available",
                &["Best", "2160", "1440", "1080", "720", "480", "360", "240"],
                0.0,
                0.0,
            ),
            f(
                "embed_subs",
                "Embed subtitles",
                "toggle",
                "false",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
            f(
                "embed_thumbnail",
                "Embed thumbnail",
                "toggle",
                "false",
                false,
                "cover art from the video thumbnail",
                &[],
                0.0,
                0.0,
            ),
            f(
                "output",
                "Save folder (blank = Downloads)",
                "folder",
                "",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
        ],
        "transcribe" => vec![
            file("input", "Audio/video file"),
            f(
                "language",
                "Source language",
                "dropdown",
                "auto",
                false,
                "auto-detect, or pick to sharpen accuracy",
                &[
                    "auto", "en", "es", "fr", "de", "it", "pt", "nl", "ru", "uk", "pl", "tr", "ar",
                    "fa", "hi", "ur", "bn", "ta", "th", "vi", "id", "ja", "ko", "zh",
                ],
                0.0,
                0.0,
            ),
            f(
                "translate",
                "Translate to English",
                "toggle",
                "false",
                false,
                "transcribe any language straight to English",
                &[],
                0.0,
                0.0,
            ),
            f(
                "output",
                "Output .srt (blank = beside source)",
                "text",
                "",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
        ],
        "hash" => vec![
            f(
                "files",
                "Files to hash (one path per line)",
                "files",
                "",
                true,
                "SHA-256",
                &[],
                0.0,
                0.0,
            ),
            f(
                "manifest",
                "Save manifest to (blank = show inline)",
                "text",
                "",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
        ],
        "folder_diff" => vec![
            f("a", "Folder A", "folder", "", true, "", &[], 0.0, 0.0),
            f("b", "Folder B", "folder", "", true, "", &[], 0.0, 0.0),
        ],
        "rename" => vec![
            f("dir", "Folder", "folder", "", true, "", &[], 0.0, 0.0),
            f(
                "pattern",
                "Pattern",
                "text",
                "{n:03}_{name}",
                true,
                "{n}, {n:03} = sequence · {name} = original name",
                &[],
                0.0,
                0.0,
            ),
            f(
                "start",
                "Start number",
                "number",
                "1",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
        ],
        "mediainfo" => vec![
            file("input", "File to inspect"),
            f(
                "output",
                "Report .txt (blank = show inline)",
                "text",
                "",
                false,
                "codec/stream/bitrate report",
                &[],
                0.0,
                0.0,
            ),
        ],
        "contact_sheet" => vec![
            file("input", "Source video"),
            f("cols", "Columns", "number", "4", false, "", &[], 0.0, 0.0),
            f("rows", "Rows", "number", "4", false, "", &[], 0.0, 0.0),
            f(
                "output",
                "Output .jpg (blank = beside source)",
                "text",
                "",
                false,
                "",
                &[],
                0.0,
                0.0,
            ),
        ],
        _ => Vec::new(),
    }
}

// ---------------------------------------------------------------- snapshot ---

/// The external binaries the section shells out to. Reported once so a missing
/// one is a row that says so, rather than twenty jobs that fail identically.
const TOOLCHAIN: &[(&str, &str, bool)] = &[
    (
        "ffmpeg",
        "Encoding, trimming, resizing, everything media",
        false,
    ),
    ("ffprobe", "Durations and stream reports", false),
    ("yt-dlp", "Downloads", true),
    ("whisper-cli", "Local speech to text", false),
];

async fn snapshot() -> Result<ToolsState> {
    let pool = tools_pool().await?;

    // Both queries run before the session lock is taken. A `MutexGuard` is not
    // `Send`, and holding one across an `.await` makes the whole dispatch
    // future non-`Send` — which frb's handler will not accept.
    let job_rows: Vec<(i64, String, String, f64, String)> = sqlx::query_as(
        "SELECT id, kind, state, COALESCE(progress, 0.0), COALESCE(message, '')
         FROM jobs ORDER BY
           CASE state WHEN 'running' THEN 0 WHEN 'queued' THEN 1
                      WHEN 'paused' THEN 2 ELSE 3 END,
           id DESC
         LIMIT 200",
    )
    .fetch_all(pool)
    .await
    .unwrap_or_default();
    let slots = queue::worker_slots(pool).await.unwrap_or(2);

    let s = lock();

    let needle = s.query.trim().to_lowercase();
    let ops: Vec<OpRow> = ALL_OPS
        .iter()
        .filter(|k| {
            // A search crosses categories: if you know what you want, the tab
            // you happen to be on should not hide it.
            if !needle.is_empty() {
                label_of(k).to_lowercase().contains(&needle) || k.to_lowercase().contains(&needle)
            } else {
                category_of(k) == s.category
            }
        })
        .map(|k| OpRow {
            kind: (*k).to_string(),
            label: label_of(k).to_string(),
            category: category_of(k).to_string(),
            info: info_of(k).to_string(),
        })
        .collect();

    let fields: Vec<Field> = if s.active.is_empty() {
        Vec::new()
    } else {
        fields_for(&s.active)
            .into_iter()
            .map(|mut f| {
                if let Some(v) = s.form.get(&f.key) {
                    f.value = v.clone();
                }
                f
            })
            .collect()
    };

    let jobs: Vec<Job> = job_rows
        .into_iter()
        .map(|(id, kind, state, progress, message)| Job {
            label: label_of(&kind).to_string(),
            id,
            kind,
            state,
            progress,
            message,
        })
        .collect();

    let running = jobs.iter().filter(|j| j.state == "running").count();
    let queued = jobs.iter().filter(|j| j.state == "queued").count();
    let queue_status = if running == 0 && queued == 0 {
        "Idle".to_string()
    } else {
        format!("{running} running · {queued} queued")
    };

    let statuses: Vec<ToolStatus> = TOOLCHAIN
        .iter()
        .map(|(name, detail, updatable)| {
            let bundled = tulipix_core::thumbs::tool_bin(name);
            let has_bundled = bundled.exists();
            let on_path = which_on_path(name);
            ToolStatus {
                name: (*name).to_string(),
                source: if has_bundled {
                    "bundled".into()
                } else if on_path {
                    "path".into()
                } else {
                    "missing".into()
                },
                detail: (*detail).to_string(),
                available: has_bundled || on_path,
                updatable: *updatable,
                version: if *name == "yt-dlp" {
                    tulipix_core::ytdlp::installed_version().unwrap_or_default()
                } else {
                    String::new()
                },
            }
        })
        .collect();

    let (result_kind, result_title, result_info, result_diff, result_text) = if s.result_open {
        let report = reports()
            .lock()
            .ok()
            .and_then(|g| g.get(&s.result_id).cloned());
        match report {
            Some(Report::Info(rows)) => (
                "info".to_string(),
                "Media info".to_string(),
                rows.into_iter()
                    .map(|(section, key, value)| InfoRow {
                        section,
                        key,
                        value,
                    })
                    .collect(),
                Vec::new(),
                String::new(),
            ),
            Some(Report::Diff(rows)) => (
                "diff".to_string(),
                "Folder diff".to_string(),
                Vec::new(),
                rows.into_iter()
                    .map(|(kind, path, detail)| DiffRow { kind, path, detail })
                    .collect(),
                String::new(),
            ),
            Some(Report::Text(body)) => (
                "text".to_string(),
                "Result".to_string(),
                Vec::new(),
                Vec::new(),
                body,
            ),
            None => (
                "text".to_string(),
                "Result".to_string(),
                Vec::new(),
                Vec::new(),
                "This job did not produce a report.".to_string(),
            ),
        }
    } else {
        (
            String::new(),
            String::new(),
            Vec::new(),
            Vec::new(),
            String::new(),
        )
    };

    Ok(ToolsState {
        category: s.category.clone(),
        query: s.query.clone(),
        ops,
        active_op: s.active.clone(),
        active_label: if s.active.is_empty() {
            String::new()
        } else {
            label_of(&s.active).to_string()
        },
        active_info: if s.active.is_empty() {
            String::new()
        } else {
            info_of(&s.active).to_string()
        },
        fields,
        jobs,
        queue_status,
        worker_slots: slots,
        statuses,
        downloads: s
            .downloads
            .iter()
            .map(|(name, path, meta)| DownloadRow {
                name: name.clone(),
                path: path.clone(),
                meta: meta.clone(),
            })
            .collect(),
        log: log().lock().map(|g| g.clone()).unwrap_or_default(),
        error: s.error.clone(),
        result_open: s.result_open,
        result_kind,
        result_title,
        result_info,
        result_diff,
        result_text,
    })
}

fn which_on_path(bin: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&paths).any(|dir| {
        let p = dir.join(bin);
        p.exists() || p.with_extension("exe").exists()
    })
}
