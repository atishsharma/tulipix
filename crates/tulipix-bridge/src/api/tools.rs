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

use tulipix_tools::catalog;
use tulipix_tools::exec::{self, Native, Step};
use tulipix_tools::preview;
use tulipix_tools::queue;

use crate::db::tools_pool;

// ------------------------------------------------------------------- state ---

/// One operation, as a card on the landing page.
pub struct OpRow {
    pub kind: String,
    pub label: String,
    pub category: String,
    pub info: String,
    /// The binary this operation needs and cannot find — empty when it can run.
    /// The tile greys out and says the name; the alternative is a job that
    /// queues, starts, and dies on the first spawn.
    pub missing: String,
}

/// One row of the form the chosen operation asks for.
pub struct Field {
    pub key: String,
    pub label: String,
    /// file | files | folder | save | text | number | dropdown | slider |
    /// toggle.
    pub kind: String,
    pub value: String,
    pub options: Vec<String>,
    pub min: f64,
    pub max: f64,
    pub required: bool,
    pub hint: String,
    /// What the file dialog filters to, without the dot. Empty means anything.
    pub ext: Vec<String>,
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
    /// One of `catalog::Category::id` — fileops | video | audio | photo |
    /// subtitles.
    pub category: String,
    pub query: String,
    pub ops: Vec<OpRow>,

    pub active_op: String,
    pub active_label: String,
    pub active_info: String,
    /// `OpDef::preview` for the open tool: image | video | wave | pages | cues
    /// | dryrun | convert | plain. Empty when no tool is open.
    pub active_preview: String,
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

// ----------------------------------------------------------------- preview ---

/// One line of a dry run.
pub struct PreviewRow {
    /// plain | add | remove | change | warn. A colour, not a meaning — the
    /// words are in `note`.
    pub kind: String,
    pub left: String,
    pub right: String,
    pub note: String,
}

/// One subtitle cue, in milliseconds.
pub struct PreviewCue {
    pub index: i64,
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// One page of a document, for the `pages` preview.
pub struct PreviewPage {
    pub page: i64,
    /// False for a page the operation removes.
    pub kept: bool,
    /// Degrees it ends up turned by, on top of what it already carried.
    pub turned: i64,
}

/// What the run is likely to produce. Always a guess, and the footer says so.
pub struct PreviewEstimate {
    pub src: i64,
    pub out: i64,
    /// Encode seconds. Zero means not known yet.
    pub secs: f64,
}

pub struct PreviewResult {
    /// The request this answers. Anything older than what the caller has
    /// already drawn is thrown away rather than drawn over it.
    pub epoch: i64,
    /// dryrun | cues | pages | image | wave | waiting | none | stale.
    pub kind: String,
    /// The op it describes, so a late answer cannot be shown under a different
    /// tool.
    pub op: String,
    pub title: String,
    pub note: String,
    pub rows: Vec<PreviewRow>,
    pub cues: Vec<PreviewCue>,
    pub pages: Vec<PreviewPage>,
    /// Rows the budget cut. `rows.len() + more` is the real total.
    pub more: i64,
    /// A rendered picture in the preview cache, for `kind == "image"`.
    pub image: String,
    /// The untouched side of a before/after. Empty when the render stands
    /// alone — a thumbnail has no "before".
    pub before: String,
    /// One 0..1 peak per bucket, for `kind == "wave"`.
    pub peaks: Vec<f64>,
    pub estimate: Option<PreviewEstimate>,
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
    /// The newest preview request. Anything older that comes back late is
    /// dropped rather than drawn over the answer the user is looking at.
    preview_epoch: i64,
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

/// What the open tool is about to do, drawn while the form is still being
/// filled in.
///
/// A third entry point rather than another field on `ToolsState`, because a
/// preview is expensive, per-keystroke and droppable, and a snapshot is none of
/// those. The spec comes from the session rather than the caller: it is the
/// same form `Run` submits, so the preview cannot describe a job different
/// from the one that would be queued.
///
/// `epoch` counts up on the Flutter side. The newest one wins; an answer that
/// finishes after a newer request started comes back marked `stale`.
pub async fn tools_preview(epoch: i64) -> Result<PreviewResult> {
    let (kind, form) = {
        let mut s = lock();
        if epoch > s.preview_epoch {
            s.preview_epoch = epoch;
        }
        (s.active.clone(), s.form.clone())
    };
    if kind.is_empty() {
        return Ok(blank_preview(epoch, "none", &kind));
    }

    // A tool whose binary is missing previews as the reason, not as an error.
    // The tile is greyed already; this is what the pane says if one is opened
    // from a search result anyway.
    if let Some(op) = catalog::get(&kind) {
        let absent = missing_for(op, &installed_binaries());
        if !absent.is_empty() {
            let mut waiting = blank_preview(epoch, "waiting", &kind);
            waiting.note = format!("Needs {absent}, and it is not installed.");
            return Ok(waiting);
        }
    }

    let schema = fields_for(&kind);
    let spec = spec_from(&kind, &form, &schema);
    let input = spec
        .get("input")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    // One ffprobe per source, ever. Duration, dimensions and codec names are
    // what turn Convert and Split from guesses into statements, and they are
    // also where a frame is worth grabbing from. The dry-run previews need
    // none of it and do not pay for it.
    let probe = if input.is_empty() || !wants_probe(&kind) {
        None
    } else {
        probe_of(&input).await
    };

    // Walking a folder of ten thousand files is not something to do on the
    // async runtime's own thread, however fast it usually is.
    let budget = preview::Budget {
        cache: preview_cache().unwrap_or_default(),
        ..Default::default()
    };
    let planning = kind.clone();
    let planning_probe = probe;
    let (plan, estimate) = tokio::task::spawn_blocking(move || {
        let plan = preview::plan_preview(&planning, &spec, &budget, planning_probe.as_ref());
        let src = spec
            .get("input")
            .and_then(|v| v.as_str())
            .and_then(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .unwrap_or(0);
        let estimate = preview::estimate(&planning, &spec, src, planning_probe.as_ref());
        (plan, estimate)
    })
    .await?;

    let (data, image, before, peaks) = match plan {
        preview::PreviewPlan::Pure(d) => (d, String::new(), String::new(), Vec::new()),
        preview::PreviewPlan::Render(r) => match render(r, epoch).await {
            Ok(done) => done,
            Err(e) => {
                if lock().preview_epoch > epoch {
                    return Ok(blank_preview(epoch, "stale", &kind));
                }
                return Ok(PreviewResult {
                    note: e.to_string(),
                    ..blank_preview(epoch, "waiting", &kind)
                });
            }
        },
    };

    // The form moved on while we were reading the disk. Whatever is on screen
    // is newer than this.
    if lock().preview_epoch > epoch {
        return Ok(blank_preview(epoch, "stale", &kind));
    }

    Ok(PreviewResult {
        epoch,
        kind: data.kind.to_string(),
        op: kind,
        image,
        before,
        peaks,
        title: data.title,
        note: data.note,
        rows: data
            .rows
            .into_iter()
            .map(|r| PreviewRow {
                kind: r.kind.as_str().to_string(),
                left: r.left,
                right: r.right,
                note: r.note,
            })
            .collect(),
        cues: data
            .cues
            .into_iter()
            .map(|c| PreviewCue {
                index: c.index as i64,
                start_ms: c.start_ms as i64,
                end_ms: c.end_ms as i64,
                text: c.text,
            })
            .collect(),
        pages: data
            .pages
            .into_iter()
            .map(|p| PreviewPage {
                page: p.page as i64,
                kept: p.kept,
                turned: p.turned,
            })
            .collect(),
        more: data.more as i64,
        estimate: estimate.map(|e| PreviewEstimate {
            src: e.src as i64,
            out: e.out as i64,
            secs: e.secs,
        }),
    })
}

/// How many peaks a waveform is reduced to. Wider than any pane it is drawn
/// in, so the drawing never has to interpolate upwards.
const PEAK_BUCKETS: usize = 480;

/// The preview cache holds this much before the oldest renders are dropped.
const PREVIEW_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// Where rendered previews live. Under the app's own cache directory, which
/// means Clean cache already clears them and there is no second broom.
fn preview_cache() -> Option<std::path::PathBuf> {
    let dir = tulipix_core::paths::cache_dir()?.join("tools-preview");
    std::fs::create_dir_all(&dir).ok()?;
    Some(dir)
}

/// Which previews are worth an ffprobe. The dry runs are about files on disk,
/// not about what is inside them.
fn wants_probe(kind: &str) -> bool {
    catalog::get(kind)
        .map(|op| {
            matches!(
                op.preview,
                catalog::Preview::Image
                    | catalog::Preview::Video
                    | catalog::Preview::Convert
                    | catalog::Preview::Pages
            ) || op.kind == "split"
        })
        // ffprobe has nothing to say about a .docx or an .epub. The two
        // document converters draw the same Convert pane and are read by
        // pandoc and Calibre, not by ffmpeg.
        .map(|wants| {
            wants
                && catalog::get(kind)
                    .map(|op| op.needs.contains(&"ffmpeg") || op.needs.contains(&"ffprobe"))
                    .unwrap_or(false)
        })
        .unwrap_or(false)
}

/// One ffprobe per source per session, keyed by path, size and mtime — so
/// dragging a CRF slider for a minute costs exactly one.
fn probes() -> &'static Mutex<std::collections::HashMap<String, preview::Probe>> {
    static P: OnceLock<Mutex<std::collections::HashMap<String, preview::Probe>>> = OnceLock::new();
    P.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

async fn probe_of(path: &str) -> Option<preview::Probe> {
    let meta = std::fs::metadata(path).ok()?;
    let stamp = format!(
        "{path}|{}|{}",
        meta.len(),
        meta.modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    if let Some(hit) = probes().lock().ok().and_then(|g| g.get(&stamp).cloned()) {
        return Some(hit);
    }
    let out = quiet(&bin("ffprobe"))
        .args([
            "-v",
            "error",
            "-show_format",
            "-show_streams",
            "-of",
            "json",
            path,
        ])
        .output()
        .await
        .ok()?;
    let json: Value = serde_json::from_slice(&out.stdout).ok()?;
    let probe = parse_probe(&json, meta.len());
    if let Ok(mut g) = probes().lock() {
        g.insert(stamp, probe.clone());
    }
    Some(probe)
}

fn parse_probe(v: &Value, bytes: u64) -> preview::Probe {
    let mut p = preview::Probe {
        bytes,
        // A still image has no duration, which is the right answer for it.
        duration_s: v["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0),
        ..Default::default()
    };
    for stream in v["streams"].as_array().into_iter().flatten() {
        let name = stream["codec_name"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        match stream["codec_type"].as_str() {
            Some("video") if p.v_codec.is_empty() => {
                p.v_codec = name;
                p.width = stream["width"].as_u64().unwrap_or(0) as u32;
                p.height = stream["height"].as_u64().unwrap_or(0) as u32;
            }
            Some("audio") if p.a_codec.is_empty() => p.a_codec = name,
            _ => {}
        }
    }
    p
}

/// One preview render at a time, process-wide.
///
/// This is the whole of the "no more than one ffmpeg alive" rule. Without it,
/// a slider dragged across its range forks one encoder per stop and they all
/// finish at once, several seconds after the value they describe is gone.
fn render_gate() -> &'static tokio::sync::Mutex<()> {
    static G: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    G.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Execute a render plan and report what it produced.
async fn render(
    plan: preview::RenderPlan,
    epoch: i64,
) -> Result<(preview::PreviewData, String, String, Vec<f64>)> {
    run_preview_steps(plan.steps, epoch).await?;
    if plan.kind == "wave" {
        // The artifact is raw PCM, which is only a preview once it has been
        // reduced to peaks.
        let peaks = tokio::fs::read(&plan.out)
            .await
            .map(|bytes| {
                preview::peaks_from_pcm(&bytes, PEAK_BUCKETS)
                    .into_iter()
                    .map(f64::from)
                    .collect::<Vec<f64>>()
            })
            .unwrap_or_default();
        return Ok((
            preview::PreviewData::rendered("wave", plan.note),
            String::new(),
            String::new(),
            peaks,
        ));
    }
    Ok((
        preview::PreviewData::rendered("image", plan.note),
        plan.out,
        plan.before,
        Vec::new(),
    ))
}

async fn run_preview_steps(steps: Vec<Step>, epoch: i64) -> Result<()> {
    if steps.is_empty() {
        return Ok(());
    }
    let _gate = render_gate().lock().await;
    for step in steps {
        // The gate is a queue: by the time our turn comes the form may have
        // moved on twice.
        if lock().preview_epoch > epoch {
            return Err(anyhow!("superseded"));
        }
        let Step::Ffmpeg { args, .. } = step else {
            continue;
        };
        // `-nostdin` because a preview must never take the terminal, and
        // `-loglevel error` because nothing here is worth a line in the
        // console the user is watching a real job in.
        let mut argv: Vec<String> = vec![
            "-hide_banner".into(),
            "-nostdin".into(),
            "-loglevel".into(),
            "error".into(),
            "-y".into(),
        ];
        argv.extend(args);
        spawn_preview(bin("ffmpeg"), &argv, epoch).await?;
    }
    Ok(())
}

/// Spawn one short-lived child and watch the epoch while it runs. A render
/// whose answer nobody wants any more is killed, not waited for.
async fn spawn_preview(exe: std::path::PathBuf, args: &[String], epoch: i64) -> Result<()> {
    let mut child = quiet(&exe)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| anyhow!("{} could not be started: {e}", exe.display()))?;
    loop {
        tokio::select! {
            // `Child::wait` is cancel-safe, so losing this race costs nothing.
            status = child.wait() => {
                return if status?.success() {
                    Ok(())
                } else {
                    Err(anyhow!("This one could not be previewed."))
                };
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(120)) => {
                if lock().preview_epoch > epoch {
                    child.kill().await.ok();
                    return Err(anyhow!("superseded"));
                }
            }
        }
    }
}

fn blank_preview(epoch: i64, kind: &str, op: &str) -> PreviewResult {
    PreviewResult {
        epoch,
        kind: kind.to_string(),
        op: op.to_string(),
        title: String::new(),
        note: String::new(),
        rows: Vec::new(),
        cues: Vec::new(),
        pages: Vec::new(),
        more: 0,
        image: String::new(),
        before: String::new(),
        peaks: Vec::new(),
        estimate: None,
    }
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

    // Once, here, rather than after every render: a render that stops to
    // delete things is a render the user is waiting on.
    if let Some(dir) = preview_cache() {
        tokio::task::spawn_blocking(move || preview::sweep_cache(&dir, PREVIEW_CACHE_BYTES))
            .await
            .ok();
    }

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
        Step::Tool { bin: name, args } => {
            // No progress parser: see `Step::Tool`. The step's share of the bar
            // is claimed up front so a four-minute demucs run does not look
            // stalled at whatever the previous step left behind.
            queue::set_progress(pool, id, base, Some(name)).await.ok();
            spawn_tracked(pool, id, bin(name), &args, base, span, |_| None).await?;
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
            // Sorted by name so `{n}` numbers the way the folder reads. The
            // listing is `rename::scan` rather than a read_dir here, because
            // the preview pane runs the same one — a preview that numbers the
            // files differently from the run is worse than no preview.
            let files = tulipix_tools::rename::scan(&dir).map_err(|e| anyhow!("{dir}: {e}"))?;
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
        Native::Mirror { a, b, delete_extra } => {
            // The same plan the preview showed, built the same way. The worker
            // only carries it out — one action at a time, so cancelling lands
            // between files rather than halfway through one.
            let actions = tulipix_tools::folder_diff::plan_mirror(&a, &b, delete_extra, MIRROR_CAP);
            if actions.is_empty() {
                return Ok("Already identical.".into());
            }
            let total = actions.len();
            let (mut copied, mut deleted) = (0usize, 0usize);
            for (i, action) in actions.into_iter().enumerate() {
                if is_stopped(id) {
                    return Err(anyhow!("cancelled"));
                }
                match action {
                    tulipix_tools::folder_diff::MirrorAction::Copy { from, to, rel, .. } => {
                        if let Some(parent) = std::path::Path::new(&to).parent() {
                            std::fs::create_dir_all(parent).ok();
                        }
                        match std::fs::copy(&from, &to) {
                            Ok(_) => {
                                copied += 1;
                                log_push(&format!("copy  {rel}"));
                            }
                            Err(e) => log_push(&format!("skip  {rel}: {e}")),
                        }
                    }
                    tulipix_tools::folder_diff::MirrorAction::Delete { path, rel, .. } => {
                        match std::fs::remove_file(&path) {
                            Ok(()) => {
                                deleted += 1;
                                log_push(&format!("del   {rel}"));
                            }
                            Err(e) => log_push(&format!("skip  {rel}: {e}")),
                        }
                    }
                }
                let at = base + (i + 1) as f64 / total as f64 * span;
                queue::set_progress(pool, id, at.clamp(0.0, 1.0), None)
                    .await
                    .ok();
                emit(ToolsEvent::Progress {
                    id,
                    progress: at.clamp(0.0, 1.0),
                    message: String::new(),
                });
            }
            Ok(format!("Copied {copied}, deleted {deleted}."))
        }
        Native::ArchiveCreate {
            root,
            files,
            output,
            store,
        } => {
            // The same member list the preview showed, from the same function.
            let members = tulipix_tools::archive::members(&root, &files, ARCHIVE_CAP);
            let total = members.len();
            let count = archive_step(pool, id, base, span, total, move |tick| {
                tulipix_tools::archive::create(&members, &output, store, tick)
            })
            .await?;
            Ok(format!("Put {count} files in the archive."))
        }
        Native::ArchiveExtract { input, dir } => {
            let total = tulipix_tools::archive::list(&input)
                .map(|e| e.len())
                .unwrap_or(0);
            let (written, refused) = archive_step(pool, id, base, span, total, move |tick| {
                tulipix_tools::archive::extract(&input, &dir, tick)
            })
            .await?;
            Ok(if refused == 0 {
                format!("Unpacked {written} files.")
            } else {
                // Not an error: one member trying to climb out of the folder
                // should not lose the other four hundred.
                format!("Unpacked {written} files. {refused} were skipped as unsafe or unreadable.")
            })
        }
        Native::ArchiveRepack {
            input,
            output,
            store,
        } => {
            let total = tulipix_tools::archive::list(&input)
                .map(|e| e.len())
                .unwrap_or(0);
            let count = archive_step(pool, id, base, span, total, move |tick| {
                tulipix_tools::archive::repack(&input, &output, store, tick)
            })
            .await?;
            Ok(format!("Repacked {count} files."))
        }
        Native::PdfStamp {
            input,
            output,
            ranges,
            pattern,
            corner,
            size,
        } => {
            use tulipix_tools::pdf;
            let count = pdf::page_count(&input)
                .ok_or_else(|| anyhow!("{input} could not be opened as a PDF"))?;
            let pages = pdf::selected(&ranges, count);
            let corner = pdf::Corner::parse(&corner);
            let done = tokio::task::spawn_blocking(move || {
                pdf::stamp_pages(&input, &pages, &pattern, corner, size, &output)
            })
            .await??;
            let _ = (base, span);
            Ok(format!("Stamped {done} of {count} pages."))
        }
        Native::PdfPages {
            op,
            input,
            output,
            ranges,
            turn,
        } => {
            use tulipix_tools::exec::PdfPageOp;
            use tulipix_tools::pdf;
            let count = pdf::page_count(&input)
                .ok_or_else(|| anyhow!("{input} could not be opened as a PDF"))?;
            let pages = pdf::selected(&ranges, count);
            if pages.is_empty() && op != PdfPageOp::Rotate {
                return Err(anyhow!(
                    "that range names no page in a {count}-page document"
                ));
            }
            // lopdf holds the whole document, so this is one blocking chunk of
            // work rather than something to report progress across.
            let done = tokio::task::spawn_blocking(move || match op {
                PdfPageOp::Keep => pdf::keep_pages(&input, &pages, &output),
                PdfPageOp::Drop => pdf::drop_pages(&input, &pages, &output),
                PdfPageOp::Rotate => pdf::rotate_pages(&input, &pages, turn, &output),
            })
            .await??;
            let _ = (base, span);
            Ok(match op {
                PdfPageOp::Keep => format!("Kept {done} of {count} pages."),
                PdfPageOp::Drop => format!("Deleted {done} of {count} pages."),
                PdfPageOp::Rotate => format!("Turned {done} of {count} pages by {turn}°."),
            })
        }
    }
}

/// An archive job will not gather more members than this.
const ARCHIVE_CAP: usize = 200_000;

/// Run one archive operation on a blocking thread, reporting progress from
/// inside it and stopping when the job is cancelled.
///
/// The archive crate takes a `FnMut(done, total) -> bool` rather than knowing
/// about the queue, so this is the only place the two meet. Progress is sent
/// through a channel because the closure runs off the async runtime.
async fn archive_step<T, F>(
    pool: &sqlx::SqlitePool,
    id: i64,
    base: f64,
    span: f64,
    total: usize,
    work: F,
) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut dyn FnMut(usize, usize) -> bool) -> Result<T> + Send + 'static,
{
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<usize>();
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = stop.clone();
    let handle = tokio::task::spawn_blocking(move || {
        let mut tick = |done: usize, _total: usize| {
            let _ = tx.send(done);
            !flag.load(std::sync::atomic::Ordering::Relaxed)
        };
        work(&mut tick)
    });

    // Drain progress while it runs, and tell it to stop when the queue says so.
    let total = total.max(1);
    while let Some(done) = rx.recv().await {
        if is_stopped(id) {
            stop.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        let at = (base + done as f64 / total as f64 * span).clamp(0.0, 1.0);
        queue::set_progress(pool, id, at, None).await.ok();
        emit(ToolsEvent::Progress {
            id,
            progress: at,
            message: String::new(),
        });
    }
    handle.await?
}

/// A mirror will not walk more files than this. A photo library is a plausible
/// source; a whole home directory chosen by accident is not something to
/// enumerate before saying so.
const MIRROR_CAP: usize = 200_000;

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

/// The label a job carries in the queue and the log. Everything else the
/// catalogue provides is read straight off the `OpDef` in `snapshot`.
fn label_of(kind: &str) -> &'static str {
    catalog::get(kind).map(|o| o.label).unwrap_or("Job")
}

/// How many operations the catalogue offers. Home's hub tile prints it, and it
/// must be the same number the Tools page lists rather than a second count.
pub(crate) fn op_count() -> i64 {
    catalog::CATALOG.len() as i64
}

/// The form one operation asks for, as the UI's `Field` rather than the
/// catalogue's `FieldDef`. Values are filled in from the session afterwards.
fn fields_for(kind: &str) -> Vec<Field> {
    let Some(op) = catalog::get(kind) else {
        return Vec::new();
    };
    op.fields
        .iter()
        .map(|d| Field {
            key: d.key.to_string(),
            label: d.label.to_string(),
            kind: d.kind.as_str().to_string(),
            value: d.value.to_string(),
            options: d.options.iter().map(|s| (*s).to_string()).collect(),
            min: d.min,
            max: d.max,
            required: d.required,
            hint: d.hint.to_string(),
            ext: d.ext.iter().map(|s| (*s).to_string()).collect(),
        })
        .collect()
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
    // Not bundled, and not going to be. Each is large, each is packaged
    // everywhere, and the four operations that use them grey out until one
    // shows up on PATH.
    ("pandoc", "Document converter", false),
    ("ebook-convert", "Ebook converter — part of Calibre", false),
    ("demucs", "Splits a track into stems", false),
    ("ffsubsync", "Lines subtitles up with the audio", false),
];

/// Whether a binary can be run at all: bundled with the app, or on PATH.
///
/// Cheap, but not free — it stats a handful of directories. `snapshot` asks it
/// once per distinct binary rather than once per tile.
fn have(name: &str) -> bool {
    tulipix_core::thumbs::tool_bin(name).exists() || which_on_path(name)
}

/// Every binary the catalogue names, resolved once.
///
/// `tool_bin` is not free — for a bundled candidate it will launch the thing to
/// see whether it starts — so a snapshot asks about each distinct name once
/// rather than once per tile. Eight names, forty-two tiles.
fn installed_binaries() -> Vec<(&'static str, bool)> {
    let mut names: Vec<&'static str> = Vec::new();
    for op in catalog::CATALOG {
        for n in op.needs {
            if !names.contains(n) {
                names.push(*n);
            }
        }
    }
    names.into_iter().map(|n| (n, have(n))).collect()
}

/// The first binary an operation needs and cannot find.
fn missing_for(op: &catalog::OpDef, known: &[(&'static str, bool)]) -> String {
    op.needs
        .iter()
        .copied()
        .find(|n| !known.iter().any(|(name, ok)| name == n && *ok))
        .map(str::to_string)
        .unwrap_or_default()
}

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
    let installed = installed_binaries();
    let ops: Vec<OpRow> = catalog::CATALOG
        .iter()
        .filter(|op| {
            // A search crosses categories: if you know what you want, the tab
            // you happen to be on should not hide it.
            if !needle.is_empty() {
                op.label.to_lowercase().contains(needle.as_str())
                    || op.kind.contains(needle.as_str())
            } else {
                op.cat.id() == s.category
            }
        })
        .map(|op| OpRow {
            kind: op.kind.to_string(),
            label: op.label.to_string(),
            category: op.cat.id().to_string(),
            info: op.info.to_string(),
            missing: missing_for(op, &installed),
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
        active_info: catalog::get(&s.active)
            .map(|op| op.info.to_string())
            .unwrap_or_default(),
        active_preview: catalog::get(&s.active)
            .map(|op| op.preview.id().to_string())
            .unwrap_or_default(),
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
