//! Tools-section wiring extracted from tulipix-app: declarative form bridge,
//! job-queue worker/executor glue, and every `window.on_tools_*` callback.
//! tulipix-app calls [`wire`] once at startup.

use slint::ComponentHandle;
use std::sync::OnceLock;
use tulipix_ui::*;
use tulipix_common::{on_path, pool_for};
// Static Tools catalog: (key, title, icon, ops). Each op is (label, kind) where
// `kind` is the job kind the executor + form bridge dispatch on. The detail
// body + search read from here.
type ToolOp = (&'static str, &'static str); // (label, kind)
const TOOLS_CATALOG: &[(&str, &str, &str, &[ToolOp])] = &[
    ("fileops", "File ops",  "📁", &[
        ("Rename", "rename"), ("Merge", "merge"), ("Split", "split"),
        ("Hash", "hash"), ("Folder diff", "folder_diff"),
        ("Media info", "mediainfo"), ("Clean cache", "cache_clean")]),
    ("video",   "Video",     "🎬", &[
        ("Compress video", "compress_video"), ("Trim", "trim"), ("Convert", "convert"),
        ("Thumbnail", "thumbnail"), ("Contact sheet", "contact_sheet"),
        ("Download", "download"), ("Playlist download", "download_playlist"),
        ("Live record", "download_live")]),
    ("audio",   "Audio",     "🎵", &[
        ("Compress audio", "compress_audio"), ("Normalise (R128)", "normalize"),
        ("Extract audio", "extract")]),
    ("photo",   "Photo",     "🖼", &[
        ("Compress photo", "compress_photo"), ("Resize", "resize"), ("Watermark", "watermark")]),
    ("subs",    "Subtitles", "💬", &[
        ("Transcribe", "transcribe"), ("Burn-in subtitles", "burn_subs")]),
];

/// How many operations the catalog offers, across every category. The Welcome
/// home layout's Tools tile shows this the way the other tiles show their own
/// counts; reading it off the catalog means the tile cannot drift from the
/// section.
pub fn op_total() -> usize {
    TOOLS_CATALOG.iter().map(|(_, _, _, ops)| ops.len()).sum()
}

/// Rebuild the Tools body from the window's category + query. Empty query shows
/// the selected category's ops; a query matches op names across every category.
fn tools_refresh(w: &MainWindow) {
    let cat = w.get_tools_category().to_string();
    let q = w.get_tools_query().to_string().trim().to_lowercase();
    let mut rows: Vec<ToolOpRow> = Vec::new();
    if q.is_empty() {
        if let Some((_, title, icon, ops)) = TOOLS_CATALOG.iter().find(|(k, ..)| *k == cat) {
            w.set_tools_cat_title((*title).into());
            w.set_tools_cat_icon((*icon).into());
            for (label, kind) in *ops {
                rows.push(ToolOpRow { cat: (*title).into(), label: (*label).into(), kind: (*kind).into() });
            }
        }
    } else {
        for (_, title, _, ops) in TOOLS_CATALOG {
            for (label, kind) in *ops {
                if label.to_lowercase().contains(&q) {
                    rows.push(ToolOpRow { cat: (*title).into(), label: (*label).into(), kind: (*kind).into() });
                }
            }
        }
    }
    w.set_tools_op_rows(slint::ModelRc::new(slint::VecModel::from(rows)));
}

// ── Tools worker + job execution ───────────────────────────────────────────
// One drainer task claims queued jobs (respecting the worker-slot cap that
// `queue::claim_next` enforces), runs each on its own task, and reports live
// progress back into tools.db. The GUI polls tools.db on a timer to render the
// Queue view, so CLI-submitted jobs show up live too.

/// In-memory edit buffer for the active tool's form: (kind, key→value).
static TOOLS_FORM: OnceLock<std::sync::Mutex<(String, std::collections::HashMap<String, String>)>> = OnceLock::new();
fn tools_form() -> &'static std::sync::Mutex<(String, std::collections::HashMap<String, String>)> {
    TOOLS_FORM.get_or_init(|| std::sync::Mutex::new((String::new(), std::collections::HashMap::new())))
}

// ── Per-job stop control ─────────────────────────────────────────────────────
// Cancel/Pause must actually stop the job's process, not just flip the DB row.
// Each running job registers a JobCtl; the queue actions fire it, the worker's
// child-wait selects on it (kills the process), and native loops poll it.

struct JobCtl { stop: std::sync::atomic::AtomicBool, notify: tokio::sync::Notify }
static JOB_CTLS: OnceLock<std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<JobCtl>>>> = OnceLock::new();
fn job_ctls() -> &'static std::sync::Mutex<std::collections::HashMap<i64, std::sync::Arc<JobCtl>>> {
    JOB_CTLS.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
/// The job's control handle (created on first use).
fn job_ctl(id: i64) -> std::sync::Arc<JobCtl> {
    job_ctls().lock().unwrap().entry(id).or_insert_with(|| std::sync::Arc::new(JobCtl {
        stop: std::sync::atomic::AtomicBool::new(false),
        notify: tokio::sync::Notify::new(),
    })).clone()
}
/// Signal the job to stop (kills its process, aborts native loops).
fn job_ctl_fire(id: i64) {
    let ctl = job_ctl(id);
    ctl.stop.store(true, std::sync::atomic::Ordering::Relaxed);
    ctl.notify.notify_waiters();
}
fn job_stopped(id: i64) -> bool {
    job_ctl(id).stop.load(std::sync::atomic::Ordering::Relaxed)
}
/// Drop the handle once the job task ends (fresh flag for a retry).
fn job_ctl_done(id: i64) { job_ctls().lock().unwrap().remove(&id); }

/// Stop a child gracefully: SIGINT first on unix (yt-dlp/ffmpeg finalize their
/// output files on it — a live recording stays playable), hard kill after 5 s
/// or on other platforms.
async fn graceful_kill(child: &mut tokio::process::Child) {
    #[cfg(unix)]
    if let Some(pid) = child.id() {
        unsafe { libc::kill(pid as i32, libc::SIGINT); }
        if tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await.is_ok() {
            return;
        }
    }
    let _ = child.kill().await;
}

/// Wait for a child process, racing the job's stop signal. On stop the process
/// is terminated and the step errors with "stopped" (queue state was already
/// set to canceled/paused by the action — `complete` won't overwrite it).
async fn wait_child(id: i64, child: &mut tokio::process::Child) -> anyhow::Result<std::process::ExitStatus> {
    let ctl = job_ctl(id);
    if ctl.stop.load(std::sync::atomic::Ordering::Relaxed) {
        graceful_kill(child).await;
        anyhow::bail!("stopped");
    }
    tokio::select! {
        st = child.wait() => Ok(st?),
        _ = ctl.notify.notified() => { graceful_kill(child).await; anyhow::bail!("stopped"); }
    }
}

/// Human label for a job kind (queue rows + recent strip).
fn tool_label(kind: &str) -> &'static str {
    match kind {
        "compress_video" => "Compress video", "compress_audio" => "Compress audio",
        "compress_photo" => "Compress photo", "convert" => "Convert",
        "trim" => "Trim", "resize" => "Resize", "thumbnail" => "Thumbnail",
        "extract" => "Extract audio", "normalize" => "Normalise", "watermark" => "Watermark",
        "burn_subs" => "Burn subtitles", "split" => "Split", "merge" => "Merge",
        "download" => "Download", "download_playlist" => "Playlist download",
        "download_live" => "Live record", "hash" => "Hash", "folder_diff" => "Folder diff",
        "rename" => "Rename", "transcribe" => "Transcribe",
        "mediainfo" => "Media info", "contact_sheet" => "Contact sheet", "cache_clean" => "Clean cache",
        _ => "Job",
    }
}

/// Directory downloads land in (yt-dlp writes here, relative out_template).
/// ~/Downloads, created on demand.
fn tools_download_dir() -> std::path::PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = home.join("Downloads");
    let _ = std::fs::create_dir_all(&d);
    d
}

/// Human byte size for the downloads list (compact).
fn human_bytes(n: u64) -> String {
    if n >= 1_073_741_824 { format!("{:.1} GB", n as f64 / 1_073_741_824.0) }
    else if n >= 1_048_576 { format!("{:.1} MB", n as f64 / 1_048_576.0) }
    else if n >= 1024 { format!("{:.0} KB", n as f64 / 1024.0) }
    else { format!("{n} B") }
}

/// The "Downloaded" list: ONLY files the app's download tools wrote (from the
/// tools.db `downloads` registry — a random file sitting in ~/Downloads must
/// not appear). Rows whose file vanished are pruned from the registry.
async fn list_downloaded() -> Vec<(String, String, String)> {
    let Ok(pool) = pool_for("tools").await else { return vec![]; };
    let paths = tulipix_tools::queue::downloads_list(&pool, 200).await.unwrap_or_default();
    let mut rows = Vec::new();
    for p in paths {
        let pb = std::path::PathBuf::from(&p);
        match std::fs::metadata(&pb) {
            Ok(md) if md.is_file() => {
                let name = pb.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone());
                rows.push((name, p, human_bytes(md.len())));
            }
            _ => { let _ = tulipix_tools::queue::forget_download(&pool, &p).await; }
        }
    }
    rows
}

/// One-paragraph "what it does + how to use it" blurb shown in the tool's info
/// box, above its form. Keep it concrete: name the inputs and the result.
fn tool_info(kind: &str) -> &'static str {
    match kind {
        "compress_video" => "Shrinks a video's file size by re-encoding it at a chosen quality. CRF (Constant Rate Factor) is the quality dial: the encoder keeps a constant visual quality and lets the bitrate vary to hit it. LOWER CRF = higher quality + bigger file; HIGHER CRF = smaller file + more quality loss. Each +6 roughly halves the size. Sweet spots: ~18 visually lossless, ~23 default, ~28 small. Pick the source video, set the CRF, then Run — the smaller copy is written beside the original.",
        "compress_audio" => "Re-encodes an audio file to a smaller size at a target bitrate. Choose the file and bitrate, then Run; the compressed copy lands beside the source.",
        "compress_photo" => "Reduces an image's file size by re-encoding it. Pick the photo and quality, then Run — a lighter copy is saved next to it.",
        "convert" => "Converts a media file from one container/codec to another. Choose the file and the target format, then Run.",
        "trim" => "Cuts a clip out of a video between a start and end time without re-encoding where possible. Pick the file, set start/end, then Run.",
        "resize" => "Scales a video or image to a new resolution. Pick the file, set the target width/height, then Run.",
        "thumbnail" => "Grabs a single still frame from a video as an image. Pick the video and the timestamp, then Run.",
        "extract" => "Pulls the audio track out of a video into a standalone audio file. Pick the video, choose the audio format, then Run.",
        "normalize" => "Levels a file's loudness to a standard target (EBU R128) so playback volume is consistent. Pick the file, then Run.",
        "watermark" => "Overlays an image or text watermark onto a video. Pick the video and the watermark, position it, then Run.",
        "burn_subs" => "Permanently renders a subtitle file into the video picture. Pick the video and the .srt/.ass, then Run.",
        "split" => "Splits one media file into multiple parts (by time or chapters). Pick the file, set how to split, then Run.",
        "merge" => "Joins several media files of the same type into one. Add the files in order, then Run to concatenate them.",
        "download" => "Downloads a single video/audio from a URL via yt-dlp. Paste the link, pick a format, then Run.",
        "download_playlist" => "Downloads an entire playlist via yt-dlp. Paste the playlist URL, choose a format, then Run — items queue up one by one.",
        "download_live" => "Records a live stream to disk via yt-dlp. Paste the stream URL, then Run; press Cancel on the queue row to stop — the recording is finalized and kept.",
        "hash" => "Computes checksums (e.g. SHA-256) for a file so you can verify its integrity. Pick the file, then Run; the digest shows in the queue row.",
        "folder_diff" => "Compares two folders and reports which files are added, removed, or changed between them. Choose folder A and folder B, then Run — the differences are listed in the result.",
        "rename" => "Batch-renames files in a folder using a pattern. Pick the folder, set the naming pattern, then Run.",
        "transcribe" => "Transcribes speech in an audio/video file to an SRT subtitle using Whisper. Leave language on auto-detect or pick one; flip \"Translate to English\" to turn any language straight into English subtitles. Pick the file, then Run.",
        "mediainfo" => "Reports detailed technical metadata (codecs, bitrate, streams, duration) for a media file. Pick the file, then Run — the report appears in the queue row.",
        "contact_sheet" => "Builds a grid of thumbnails sampled across a video (a contact sheet). Pick the video, set the grid size, then Run.",
        "cache_clean" => "Clears Tulipix's thumbnail cache to reclaim disk space. Thumbnails regenerate on demand the next time you browse. Just Run.",
        _ => "Set the options below, then Run. Progress shows in the Queue tab.",
    }
}

/// Keep the last 200 chars of an error/message for the queue row.
fn truncate_msg(s: &str) -> String {
    let s = s.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() > 200 { chars[chars.len() - 200..].iter().collect() } else { s.to_string() }
}

/// Probe a media file's duration (seconds) via ffprobe; 0.0 if unknown.
async fn ffprobe_duration(path: &str) -> f64 {
    let bin = tulipix_core::thumbs::tool_bin("ffprobe");
    let out = tokio::process::Command::new(&bin)
        .args(["-v", "error", "-show_entries", "format=duration",
               "-of", "default=noprint_wrappers=1:nokey=1", path])
        .output().await;
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Everything about a clip that has to match before a concat can copy streams
/// instead of re-encoding them. `None` when ffprobe cannot read the file.
async fn ffprobe_clip_spec(path: &str) -> Option<tulipix_tools::merge::ClipSpec> {
    let bin = tulipix_core::thumbs::tool_bin("ffprobe");
    let out = tokio::process::Command::new(&bin)
        .args(["-v", "error", "-show_streams", "-of", "json", path])
        .output().await.ok()?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    Some(tulipix_tools::merge::ClipSpec::from_probe_json(&json))
}

/// Spawn the queue drainer. Idempotent-ish: call once at startup.
fn tools_start_worker() {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        loop {
            let pool = match pool_for("tools").await {
                Ok(p) => p,
                Err(_) => { tokio::time::sleep(std::time::Duration::from_secs(2)).await; continue; }
            };
            // A job canceled/paused outside the UI (CLI, direct DB edit) must
            // still stop its live process: fire the ctl of any registered job
            // whose row left 'running'.
            if let Ok(gone) = sqlx::query_scalar::<_, i64>(
                "SELECT id FROM jobs WHERE state IN ('canceled','paused')").fetch_all(&pool).await
            {
                let live: Vec<i64> = {
                    let g = job_ctls().lock().unwrap();
                    gone.iter().copied().filter(|id| g.contains_key(id)).collect()
                };
                for id in live { job_ctl_fire(id); }
            }
            match tulipix_tools::queue::claim_next(&pool).await {
                Ok(Some(id)) => {
                    // Fresh stop flag for this run — a cancel fired while the
                    // job sat queued must not poison a later retry.
                    job_ctl_done(id);
                    let pool2 = pool.clone();
                    tokio::spawn(async move {
                        let row: Option<(String, String)> =
                            sqlx::query_as("SELECT kind, spec_json FROM jobs WHERE id = ?")
                                .bind(id).fetch_optional(&pool2).await.ok().flatten();
                        let Some((kind, spec)) = row else { return; };
                        let res = tools_run_job(&pool2, id, &kind, &spec).await;
                        // `complete` only touches rows still 'running', so a job
                        // canceled/paused mid-run keeps that state. On success the
                        // message is left alone — it holds the hash digest / diff
                        // summary / download title ("Completed" used to clobber it).
                        let _ = match res {
                            Ok(())  => tulipix_tools::queue::complete(&pool2, id, true, None).await,
                            Err(e)  => tulipix_tools::queue::complete(&pool2, id, false, Some(truncate_msg(&e.to_string()).as_str())).await,
                        };
                        job_ctl_done(id);
                    });
                    // Let claim_next see the new 'running' count before retrying.
                    tokio::time::sleep(std::time::Duration::from_millis(80)).await;
                }
                _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
            }
        }
    });
}

/// Run a job: plan its steps and execute them in order, scaling progress.
async fn tools_run_job(pool: &sqlx::SqlitePool, id: i64, kind: &str, spec_json: &str) -> anyhow::Result<()> {
    // Only take over the shared console when no other job is mid-run —
    // resetting it would wipe a concurrent job's live output (workers ≥ 2).
    let running: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM jobs WHERE state = 'running'")
        .fetch_one(pool).await.unwrap_or(1);
    if running <= 1 { tools_log_reset(&format!("$ {kind}  (job #{id})")); }
    else { tools_log_push(&format!("$ {kind}  (job #{id})")); }
    let spec: serde_json::Value = serde_json::from_str(spec_json).unwrap_or_else(|_| serde_json::json!({}));
    let steps = tulipix_tools::exec::plan(kind, &spec)?;
    let n = steps.len().max(1);
    for (i, step) in steps.into_iter().enumerate() {
        tools_run_step(pool, id, step, i as f64 / n as f64, 1.0 / n as f64).await?;
    }
    Ok(())
}

/// Configure a Command to spawn silently on Windows (no console window).
fn quiet_cmd(bin: &std::path::Path) -> tokio::process::Command {
    let cmd = tokio::process::Command::new(bin);
    #[cfg(windows)]
    { let mut cmd = cmd; use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); return cmd; }
    #[cfg(not(windows))]
    cmd
}

/// Live console log of the currently-running tool's CLI output, surfaced under
/// the queue. One shared buffer (cleared per job); good enough for the common
/// 1–2 worker case. Capped so a chatty tool can't grow it without bound.
fn tools_log() -> &'static std::sync::Mutex<String> {
    static L: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    L.get_or_init(|| std::sync::Mutex::new(String::new()))
}
fn tools_log_snapshot() -> String { tools_log().lock().map(|g| g.clone()).unwrap_or_default() }
fn tools_log_reset(header: &str) {
    if let Ok(mut g) = tools_log().lock() { g.clear(); g.push_str(header); g.push('\n'); }
}
fn tools_log_push(line: &str) {
    if let Ok(mut g) = tools_log().lock() {
        g.push_str(line); g.push('\n');
        if g.len() > 24_000 { let start = g.len() - 18_000; *g = format!("…\n{}", &g[start..]); }
    }
}
/// Wipe the console buffer (Clear button).
fn tools_log_clear() { if let Ok(mut g) = tools_log().lock() { g.clear(); } }

// ── Rich result popup builders (Media info table · Folder diff list) ─────────
fn fmt_bytes(s: &str) -> String {
    let Ok(b) = s.parse::<f64>() else { return String::new(); };
    let u = ["B", "KB", "MB", "GB", "TB"]; let (mut x, mut i) = (b, 0usize);
    while x >= 1024.0 && i < u.len() - 1 { x /= 1024.0; i += 1; }
    format!("{x:.1} {}", u[i])
}
fn fmt_bitrate(s: &str) -> String {
    let Ok(b) = s.parse::<f64>() else { return String::new(); };
    if b >= 1_000_000.0 { format!("{:.1} Mbps", b / 1_000_000.0) } else { format!("{:.0} kbps", b / 1000.0) }
}
fn fmt_duration(s: &str) -> String {
    let Ok(sec) = s.parse::<f64>() else { return String::new(); };
    let t = sec as i64; format!("{:02}:{:02}:{:02}", t / 3600, (t % 3600) / 60, t % 60)
}
fn fmt_fps(s: &str) -> String {
    if let Some((n, d)) = s.split_once('/') {
        if let (Ok(n), Ok(d)) = (n.parse::<f64>(), d.parse::<f64>()) {
            if d > 0.0 { return format!("{:.3} fps", n / d).replace(".000 ", " "); }
        }
    }
    String::new()
}

/// Run ffprobe on `input` and flatten its JSON into categorized (section, key,
/// value) rows for the Media-info popup.
fn mediainfo_rows(input: &str) -> Vec<(String, String, String)> {
    let ff = tulipix_core::thumbs::tool_bin("ffprobe");
    let Ok(out) = std::process::Command::new(&ff)
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", input])
        .output() else { return vec![]; };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else { return vec![]; };
    let s = |x: &serde_json::Value, k: &str| match x.get(k) {
        Some(serde_json::Value::String(t)) => t.clone(),
        Some(other) => other.to_string(),
        None => String::new(),
    };
    let mut rows: Vec<(String, String, String)> = Vec::new();
    if let Some(f) = v.get("format") {
        rows.push(("General".into(), "Format".into(), s(f, "format_long_name")));
        rows.push(("General".into(), "Duration".into(), fmt_duration(&s(f, "duration"))));
        rows.push(("General".into(), "Size".into(), fmt_bytes(&s(f, "size"))));
        rows.push(("General".into(), "Bitrate".into(), fmt_bitrate(&s(f, "bit_rate"))));
        rows.push(("General".into(), "Streams".into(), s(f, "nb_streams")));
    }
    if let Some(streams) = v.get("streams").and_then(|x| x.as_array()) {
        let (mut nv, mut na, mut ns) = (0, 0, 0);
        let lang = |st: &serde_json::Value| st.get("tags").and_then(|t| t.get("language"))
            .and_then(|l| l.as_str()).unwrap_or("").to_string();
        for st in streams {
            match s(st, "codec_type").as_str() {
                "video" => { nv += 1; let sec = format!("Video {nv}");
                    rows.push((sec.clone(), "Codec".into(), s(st, "codec_name")));
                    let (w, h) = (s(st, "width"), s(st, "height"));
                    if !w.is_empty() { rows.push((sec.clone(), "Resolution".into(), format!("{w}×{h}"))); }
                    rows.push((sec.clone(), "Pixel format".into(), s(st, "pix_fmt")));
                    rows.push((sec.clone(), "Frame rate".into(), fmt_fps(&s(st, "r_frame_rate"))));
                    rows.push((sec, "Bitrate".into(), fmt_bitrate(&s(st, "bit_rate")))); }
                "audio" => { na += 1; let sec = format!("Audio {na}");
                    rows.push((sec.clone(), "Codec".into(), s(st, "codec_name")));
                    rows.push((sec.clone(), "Channels".into(), s(st, "channels")));
                    let sr = s(st, "sample_rate"); if !sr.is_empty() { rows.push((sec.clone(), "Sample rate".into(), format!("{sr} Hz"))); }
                    rows.push((sec.clone(), "Bitrate".into(), fmt_bitrate(&s(st, "bit_rate"))));
                    rows.push((sec, "Language".into(), lang(st))); }
                "subtitle" => { ns += 1; let sec = format!("Subtitle {ns}");
                    rows.push((sec.clone(), "Codec".into(), s(st, "codec_name")));
                    rows.push((sec, "Language".into(), lang(st))); }
                _ => {}
            }
        }
    }
    rows.retain(|(_, _, v)| !v.is_empty());
    rows
}

/// A preview thumbnail for any filetype: a real render for media, else an OS
/// file-type icon.
fn diff_thumb_path(p: &std::path::Path) -> Option<std::path::PathBuf> {
    let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let spec = tulipix_core::thumbs::ThumbSpec {
        kind: tulipix_core::thumbs::kind_for(&ext), width: 80, height: 80 };
    if let Ok(Some(r)) = tulipix_core::thumbs::render_or_cache(p, spec) {
        return Some(r.path);
    }
    tulipix_core::thumbs::os_icon_for(p)
}

/// Rows for the folder-diff result popup: the cached triples from the job run
/// when available (no re-hash), else a fresh diff (job from a past session).
/// Thumb resolution runs off the async runtime (decodes media).
async fn folderdiff_rows(job_id: i64, a: &str, b: &str) -> Vec<(String, String, String, Option<std::path::PathBuf>)> {
    let cached = diff_cache().lock().unwrap().get(&job_id).cloned();
    let triples: Vec<(String, String, String)> = match cached {
        Some(t) => t,
        None => {
            let ma = folder_hash_map(a).await;
            let mb = folder_hash_map(b).await;
            let d = tulipix_tools::folder_diff::diff(&ma, &mb);
            let mut out = Vec::new();
            let push = |out: &mut Vec<(String, String, String)>, kind: &str, rel: &str, root: &str| {
                let full = std::path::Path::new(root).join(rel);
                out.push((kind.to_string(), rel.to_string(), full.to_string_lossy().into_owned()));
            };
            for rel in d.only_in_b.iter().take(120) { push(&mut out, "only-b", rel, b); }
            for rel in d.modified.iter().take(120) { push(&mut out, "modified", rel, b); }
            for rel in d.only_in_a.iter().take(120) { push(&mut out, "only-a", rel, a); }
            out
        }
    };
    tokio::task::spawn_blocking(move || {
        triples.into_iter().map(|(kind, rel, full)| {
            let thumb = diff_thumb_path(std::path::Path::new(&full));
            (kind, rel, full, thumb)
        }).collect()
    }).await.unwrap_or_default()
}

// ── Trim timeline editor (filmstrip + waveform + multi-segment cut/join) ─────
static TRIM_SEGS: std::sync::OnceLock<std::sync::Mutex<Vec<(f32, f32)>>> = std::sync::OnceLock::new();
fn trim_segs() -> &'static std::sync::Mutex<Vec<(f32, f32)>> {
    TRIM_SEGS.get_or_init(|| std::sync::Mutex::new(Vec::new()))
}
fn trim_push_segments(w: &MainWindow) {
    let segs: Vec<TrimSeg> = trim_segs().lock()
        .map(|g| g.iter().map(|(s, e)| TrimSeg { start: *s, end: *e }).collect())
        .unwrap_or_default();
    w.set_tools_trim_segments(slint::ModelRc::new(slint::VecModel::from(segs)));
}
fn trim_probe_duration(input: &str) -> f64 {
    let ff = tulipix_core::thumbs::tool_bin("ffprobe");
    std::process::Command::new(&ff)
        .args(["-v", "quiet", "-show_entries", "format=duration", "-of", "default=nw=1:nk=1", input])
        .output().ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse::<f64>().ok())
        .unwrap_or(0.0)
}
/// A single wide PNG of ~20 frames tiled across the video's duration.
fn trim_gen_filmstrip(input: &str, dur: f64) -> Option<std::path::PathBuf> {
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let out = std::env::temp_dir().join(format!("tulipix-trim-strip-{}.png", std::process::id()));
    let fps = if dur > 1.0 { format!("{:.5}", 20.0 / dur) } else { "1".to_string() };
    let vf = format!("fps={fps},scale=-1:104,tile=20x1");
    let ok = std::process::Command::new(&ff)
        .args(["-y", "-i", input, "-vf", &vf, "-frames:v", "1", "-update", "1"]).arg(&out)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    (ok && out.exists()).then_some(out)
}
/// A waveform overview PNG (empty if the file has no audio).
fn trim_gen_waveform(input: &str) -> Option<std::path::PathBuf> {
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let out = std::env::temp_dir().join(format!("tulipix-trim-wave-{}.png", std::process::id()));
    let ok = std::process::Command::new(&ff)
        .args(["-y", "-i", input, "-filter_complex", "showwavespic=s=1600x96:colors=0x06b6d4", "-frames:v", "1"]).arg(&out)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    (ok && out.exists()).then_some(out)
}
/// Cut each keep-segment (re-encoded for frame accuracy + uniform codecs) then
/// concat-demux them into `output`.
fn trim_join_run(input: &str, segs: &[(f32, f32)], output: &str) -> Result<(), String> {
    let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
    let tmp = std::env::temp_dir();
    let pid = std::process::id();
    let mut parts = Vec::new();
    for (i, (s, e)) in segs.iter().enumerate() {
        let part = tmp.join(format!("tulipix-trim-part-{pid}-{i}.mp4"));
        let st = std::process::Command::new(&ff)
            .args(["-y", "-ss", &format!("{s}"), "-to", &format!("{e}"), "-i", input,
                   "-c:v", "libx264", "-preset", "veryfast", "-c:a", "aac"]).arg(&part)
            .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().map_err(|e| e.to_string())?;
        if !st.success() { return Err(format!("cut {} failed", i + 1)); }
        parts.push(part);
    }
    let list = tmp.join(format!("tulipix-trim-list-{pid}.txt"));
    let body = parts.iter().map(|p| format!("file '{}'", p.display())).collect::<Vec<_>>().join("\n");
    std::fs::write(&list, body).map_err(|e| e.to_string())?;
    let st = std::process::Command::new(&ff)
        .args(["-y", "-f", "concat", "-safe", "0", "-i"]).arg(&list).args(["-c", "copy"]).arg(output)
        .stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
        .status().map_err(|e| e.to_string())?;
    for p in &parts { let _ = std::fs::remove_file(p); }
    let _ = std::fs::remove_file(&list);
    if st.success() { Ok(()) } else { Err("concat failed".into()) }
}

/// Open a job's output file in the system default app. Download jobs open the
/// actual downloaded FILE (system player), not the save folder; other tools
/// read the spec's `output` path — so a halted/partial job (e.g. an aborted
/// transcribe with a half-written .srt) is still openable.
async fn tools_open_job_output(pool: &sqlx::SqlitePool, id: i64) {
    if let Ok(Some(p)) = tulipix_tools::queue::download_for_job(pool, id).await {
        let pb = std::path::PathBuf::from(&p);
        if pb.is_file() {
            std::thread::spawn(move || { let _ = tulipix_platform::fm::open_default(&pb); });
            return;
        }
    }
    let Ok(Some(spec)) = tulipix_tools::queue::job_spec(pool, id).await else { return; };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&spec) else { return; };
    if let Some(out) = v.get("output").and_then(|x| x.as_str()) {
        let p = std::path::PathBuf::from(out);
        if p.exists() {
            std::thread::spawn(move || { let _ = tulipix_platform::fm::open_default(&p); });
        }
    }
}

/// Drain stderr concurrently (avoids a full-pipe deadlock), streaming each line
/// to the live console log so the running tool's output shows up under the queue
/// in real time. Returns the full stderr for error reporting.
fn spawn_stderr_log_drain(child: &mut tokio::process::Child) -> tokio::task::JoinHandle<String> {
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut full = String::new();
        if let Some(se) = stderr {
            let mut lines = tokio::io::BufReader::new(se).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tools_log_push(&line);
                full.push_str(&line); full.push('\n');
            }
        }
        full
    })
}

/// Stream a child's stdout through `parse`, writing scaled progress to the job
/// row. Monotonic — yt-dlp's audio phase restarts its percentage at 0, which
/// used to make the bar jump backwards mid-download.
fn spawn_stdout_progress(
    child: &mut tokio::process::Child,
    pool: sqlx::SqlitePool,
    id: i64, base: f64, span: f64,
    mut parse: impl FnMut(&str) -> Option<f64> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    let stdout = child.stdout.take();
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let Some(so) = stdout else { return; };
        let mut lines = tokio::io::BufReader::new(so).lines();
        let mut last = 0.0f64;
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(f) = parse(&line) {
                if f > last {
                    last = f;
                    let _ = tulipix_tools::queue::set_progress(&pool, id, base + f * span, None).await;
                }
            }
        }
    })
}

async fn tools_run_step(pool: &sqlx::SqlitePool, id: i64, step: tulipix_tools::exec::Step, base: f64, span: f64) -> anyhow::Result<()> {
    use tulipix_tools::exec::Step;
    match step {
        Step::Ffmpeg { args, duration_input, duration_s } => {
            let bin = tulipix_core::thumbs::tool_bin("ffmpeg");
            let dur = match duration_s {
                Some(d) => d,
                None => match &duration_input { Some(p) => ffprobe_duration(p).await, None => 0.0 },
            };
            let mut cmd = quiet_cmd(&bin);
            cmd.arg("-y").arg("-nostdin");
            cmd.args(&args);
            cmd.arg("-progress").arg("pipe:1").arg("-nostats");
            cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("ffmpeg not found ({e}) — set Tools directory in Settings"))?;
            let err_task = spawn_stderr_log_drain(&mut child);
            spawn_stdout_progress(&mut child, pool.clone(), id, base, span,
                move |l| tulipix_tools::exec::parse_ffmpeg_progress(l, dur));
            let status = wait_child(id, &mut child).await?;
            let err = err_task.await.unwrap_or_default();
            if !status.success() { anyhow::bail!("ffmpeg failed: {}", truncate_msg(&err)); }
        }
        Step::YtDlp { args } => {
            let bin = tulipix_core::ytdlp::bin();
            let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
            // Best-effort: resolve the video title up front so the queue row
            // names the actual video (not just "Download"). The URL is the last
            // argv element (DownloadSpec pushes it last).
            let mut title = String::new();
            if let Some(url) = args.last() {
                if let Ok(out) = quiet_cmd(&bin)
                    .args(["--no-warnings", "--skip-download", "--playlist-items", "1",
                           "--print", "%(title)s"])
                    .args(tulipix_core::ytdlp::common_args())
                    .arg(url)
                    .output().await
                {
                    if out.status.success() {
                        let t = String::from_utf8_lossy(&out.stdout)
                            .lines().next().unwrap_or("").trim().to_string();
                        if !t.is_empty() {
                            let _ = tulipix_tools::queue::set_message(pool, id, &t).await;
                            title = t;
                        }
                    }
                }
            }
            let mut cmd = quiet_cmd(&bin);
            // Land downloads in ~/Downloads (the out_template is relative) so the
            // in-tool "Downloaded" list can find + open them.
            cmd.current_dir(tools_download_dir());
            cmd.arg("--newline");
            // Only pin --ffmpeg-location when ffmpeg is an absolute path (bundled
            // / Tools dir). For a PATH ffmpeg, tool_bin returns the bare name and
            // its parent is "" — passing that broke the merge (separate video +
            // audio kept, no final file). Empty ⇒ let yt-dlp find ffmpeg on PATH.
            if ff.is_absolute() { if let Some(dir) = ff.parent() { cmd.arg("--ffmpeg-location").arg(dir); } }
            cmd.args(&args);
            cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("yt-dlp not found ({e}) — set Tools directory in Settings"))?;
            let err_task = spawn_stderr_log_drain(&mut child);
            // Rich live progress: overall % (playlist-aware — each item's 0–100%
            // scales into its slice of the batch), downloaded / total size, and
            // speed, written into the job message so the queue row + header show
            // real numbers instead of a 0→100 jump.
            let dests: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
            {
                let stdout = child.stdout.take();
                let (pool2, title2, dests2) = (pool.clone(), title.clone(), dests.clone());
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let Some(so) = stdout else { return; };
                    let mut lines = tokio::io::BufReader::new(so).lines();
                    let (mut item, mut items_total) = (0u32, 1u32);
                    let mut last_frac = 0.0f64;
                    let mut last_write = std::time::Instant::now() - std::time::Duration::from_secs(2);
                    while let Ok(Some(line)) = lines.next_line().await {
                        if let Some(p) = tulipix_tools::exec::parse_ytdlp_dest(&line) {
                            if let Ok(mut g) = dests2.lock() { g.push(p); }
                            continue;
                        }
                        if let Some((n, m)) = tulipix_tools::exec::parse_ytdlp_item(&line) {
                            item = n; items_total = m.max(1);
                            continue;
                        }
                        let Some(st) = tulipix_tools::exec::parse_ytdlp_stats(&line) else { continue; };
                        let overall = (item.saturating_sub(1) as f64 + st.frac) / items_total as f64;
                        // Throttle DB writes; always let a forward jump through.
                        let due = last_write.elapsed().as_millis() >= 250;
                        if overall < last_frac || (!due && overall == last_frac) { continue; }
                        last_frac = overall;
                        last_write = std::time::Instant::now();
                        let mut parts = vec![format!("{:.0}%", overall * 100.0)];
                        if items_total > 1 { parts.push(format!("item {item}/{items_total}")); }
                        if let Some(t) = st.total_bytes {
                            parts.push(format!("{} / {}", human_bytes((st.frac * t as f64) as u64), human_bytes(t)));
                        }
                        if let Some(sp) = st.bytes_per_s { parts.push(format!("{}/s", human_bytes(sp))); }
                        let mut msg = parts.join(" · ");
                        if !title2.is_empty() { msg.push_str(" — "); msg.push_str(&title2); }
                        let _ = tulipix_tools::queue::set_progress(&pool2, id, base + overall * span, Some(&msg)).await;
                    }
                });
            }
            let status = wait_child(id, &mut child).await?;
            let err = err_task.await.unwrap_or_default();
            // A stopped live recording is success from the user's side: yt-dlp
            // exits non-zero on SIGINT but the finalized file is on disk.
            if !status.success() { anyhow::bail!("yt-dlp failed: {}", truncate_msg(&err)); }
            // Register the files this job wrote. Every Destination/Merger path was
            // collected; fragments were deleted by yt-dlp after the merge, so
            // "still on disk" filters the list down to the final outputs.
            {
                let mut seen = std::collections::HashSet::new();
                let paths: Vec<String> = dests.lock().map(|g| g.clone()).unwrap_or_default();
                for p in paths.into_iter().rev() { // newest mention first
                    if seen.insert(p.clone()) && std::path::Path::new(&p).is_file() {
                        let _ = tulipix_tools::queue::record_download(pool, id, &p).await;
                    }
                }
            }
            // Settle the row message back to the plain title — a stale
            // "97% · … · MB/s" reads wrong on a finished job.
            if !title.is_empty() { let _ = tulipix_tools::queue::set_message(pool, id, &title).await; }
        }
        // This crate's catalogue is a subset of `tulipix-tools`': nothing it
        // offers plans a `Tool` step, and the four binaries behind one are not
        // bundled. The jobs table is shared, though, so a row enqueued
        // elsewhere can still be claimed here — fail it by name rather than
        // panic the worker loop.
        Step::Tool { bin, .. } => anyhow::bail!("{bin} is not bundled with this build"),
        Step::Native(n) => tools_run_native(pool, id, n, base, span).await?,
    }
    let _ = tulipix_tools::queue::set_progress(pool, id, base + span, None).await;
    Ok(())
}

async fn tools_run_native(pool: &sqlx::SqlitePool, id: i64, n: tulipix_tools::exec::Native, base: f64, span: f64) -> anyhow::Result<()> {
    use tulipix_tools::exec::Native;
    use tulipix_tools::hash::{manifest_line, sha256_file_hex_with, Algo};
    match n {
        Native::Hash { files, manifest, .. } => {
            let total = files.len().max(1);
            let mut lines = Vec::new();
            for (i, f) in files.iter().enumerate() {
                // Streaming hash off the async runtime — constant memory even
                // for multi-GB files, and cancelable between chunks.
                let f2 = f.clone();
                let hex = tokio::task::spawn_blocking(move || {
                    sha256_file_hex_with(&f2, || job_stopped(id))
                }).await?.map_err(|e| anyhow::anyhow!("read {f}: {e}"))?;
                let Some(hex) = hex else { anyhow::bail!("stopped"); };
                lines.push(manifest_line(Algo::Sha256, &hex, f));
                let _ = tulipix_tools::queue::set_progress(pool, id, base + ((i + 1) as f64 / total as f64) * span, None).await;
            }
            if let Some(m) = manifest { tokio::fs::write(&m, lines.join("\n")).await?; }
            else { let _ = tulipix_tools::queue::set_progress(pool, id, base + span, Some(lines.join(" · ").as_str())).await; }
        }
        Native::FolderDiff { a, b } => {
            let ma = folder_hash_map_prog(&a, pool, id, base, span * 0.5).await?;
            let mb = folder_hash_map_prog(&b, pool, id, base + span * 0.5, span * 0.5).await?;
            let d = tulipix_tools::folder_diff::diff(&ma, &mb);
            // Cache the row triples so the result popup doesn't re-hash both
            // folders from scratch just to display them.
            let mut rows: Vec<(String, String, String)> = Vec::new();
            let push = |rows: &mut Vec<(String, String, String)>, kind: &str, rel: &str, root: &str| {
                let full = std::path::Path::new(root).join(rel);
                rows.push((kind.into(), rel.into(), full.to_string_lossy().into_owned()));
            };
            for rel in d.only_in_b.iter().take(120) { push(&mut rows, "only-b", rel, &b); }
            for rel in d.modified.iter().take(120) { push(&mut rows, "modified", rel, &b); }
            for rel in d.only_in_a.iter().take(120) { push(&mut rows, "only-a", rel, &a); }
            diff_cache().lock().unwrap().insert(id, rows);
            let msg = format!("only-in-A {} · only-in-B {} · modified {} · same {}",
                d.only_in_a.len(), d.only_in_b.len(), d.modified.len(), d.identical.len());
            let _ = tulipix_tools::queue::set_progress(pool, id, base + span, Some(msg.as_str())).await;
        }
        Native::Rename { dir, pattern, start } => {
            let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
                .map_err(|e| anyhow::anyhow!("read dir {dir}: {e}"))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file()).collect();
            entries.sort();
            let total = entries.len().max(1);
            for (i, src) in entries.iter().enumerate() {
                if job_stopped(id) { anyhow::bail!("stopped"); }
                let src_s = src.to_string_lossy().to_string();
                // `{name}` = the original stem — the default pattern uses it, and
                // an empty tag map used to rename everything to "001_.mp4".
                let mut tags = std::collections::HashMap::new();
                if let Some(stem) = src.file_stem().and_then(|s| s.to_str()) {
                    tags.insert("name".to_string(), stem.to_string());
                }
                let newname = tulipix_tools::rename::expand(&pattern, &tags, start + i, &src_s);
                if let Some(parent) = src.parent() {
                    let dst = parent.join(&newname);
                    // Never clobber: renaming onto an existing file destroys it.
                    if dst != *src && !dst.exists() { let _ = std::fs::rename(src, &dst); }
                }
                let _ = tulipix_tools::queue::set_progress(pool, id, base + ((i + 1) as f64 / total as f64) * span, None).await;
            }
        }
        Native::Merge { inputs, output } => {
            // The concat demuxer writes the first input's header and appends
            // everyone else's packets under it, so clips that disagree copy
            // into a file that describes only the first of them. Same check the
            // GUI worker makes; a job that fails beats a file that plays wrong.
            let mut specs = Vec::new();
            for p in &inputs {
                specs.push(ffprobe_clip_spec(p).await
                    .ok_or_else(|| anyhow::anyhow!("could not read {p}"))?);
            }
            if !tulipix_tools::merge::can_stream_copy(&specs) {
                anyhow::bail!("merge copies streams rather than re-encoding them, \
                               and these files do not match — convert them first");
            }
            let refs: Vec<&str> = inputs.iter().map(|s| s.as_str()).collect();
            let list = tulipix_tools::merge::concat_list(&refs);
            let tmp = std::env::temp_dir().join(format!("tulipix-merge-{id}.txt"));
            tokio::fs::write(&tmp, list).await?;
            let bin = tulipix_core::thumbs::tool_bin("ffmpeg");
            let args = tulipix_tools::merge::concat_copy_args(&tmp.to_string_lossy(), &output);
            let mut cmd = quiet_cmd(&bin);
            cmd.arg("-y");
            cmd.args(&args);
            cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("ffmpeg not found ({e})"))?;
            let err_task = spawn_stderr_log_drain(&mut child);
            let status = wait_child(id, &mut child).await?;
            let err = err_task.await.unwrap_or_default();
            let _ = tokio::fs::remove_file(&tmp).await;
            if !status.success() { anyhow::bail!("merge failed: {}", truncate_msg(&err)); }
        }
        Native::Transcribe { input, output, translate, language } => {
            // 1. Extract 16 kHz mono PCM wav (what whisper.cpp expects).
            let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
            let media_dur = ffprobe_duration(&input).await; // for whisper progress
            let wav = std::env::temp_dir().join(format!("tulipix-whisper-{id}.wav"));
            let mut c = quiet_cmd(&ff);
            c.arg("-y").args(["-i", &input, "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"]);
            c.arg(&wav).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
            let mut child = c.spawn().map_err(|e| anyhow::anyhow!("ffmpeg not found ({e})"))?;
            let err_task = spawn_stderr_log_drain(&mut child);
            let status = wait_child(id, &mut child).await?;
            let err = err_task.await.unwrap_or_default();
            if !status.success() { anyhow::bail!("audio extract failed: {}", truncate_msg(&err)); }
            let _ = tulipix_tools::queue::set_progress(pool, id, base + span * 0.2, None).await;
            // 2. whisper.cpp → SRT.
            let model = whisper_model().ok_or_else(|| anyhow::anyhow!(
                "whisper model (ggml-tiny-1.0.bin) not found — run `just fetch`, \
                 or drop it in the Tools directory set in Settings"))?;
            let whisper = whisper_bin().ok_or_else(|| { anyhow::anyhow!(
                "whisper.cpp CLI not found — install whisper.cpp (provides `whisper-cli`), \
                 or drop the binary in the Tools directory set in Settings") })?;
            let out_prefix = output.strip_suffix(".srt").unwrap_or(&output).to_string();
            let args = tulipix_tools::transcribe::args(&model.to_string_lossy(), &wav.to_string_lossy(), &out_prefix, 4, translate, &language);
            let mut c = quiet_cmd(&whisper);
            c.args(&args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            let mut child = c.spawn().map_err(|e| anyhow::anyhow!("whisper failed to start ({e})"))?;
            // whisper.cpp prints transcribed segments to stdout — stream them to
            // the console log AND turn their timestamps into live progress (the
            // bar used to sit frozen for the whole transcription).
            if let Some(out) = child.stdout.take() {
                let (pool2, prog_base, prog_span) = (pool.clone(), base + span * 0.2, span * 0.8);
                tokio::spawn(async move {
                    use tokio::io::AsyncBufReadExt;
                    let mut lines = tokio::io::BufReader::new(out).lines();
                    while let Ok(Some(line)) = lines.next_line().await {
                        tools_log_push(&line);
                        if media_dur > 0.0 {
                            if let Some(ts) = parse_whisper_ts(&line) {
                                let f = (ts / media_dur).clamp(0.0, 1.0);
                                let _ = tulipix_tools::queue::set_progress(&pool2, id, prog_base + f * prog_span, None).await;
                            }
                        }
                    }
                });
            }
            let err_task = spawn_stderr_log_drain(&mut child);
            let status = wait_child(id, &mut child).await;
            let err = err_task.await.unwrap_or_default();
            let _ = tokio::fs::remove_file(&wav).await;
            if !status?.success() { anyhow::bail!("whisper failed: {}", truncate_msg(&err)); }
        }
        Native::MediaInfo { input, output } => {
            let bin = tulipix_core::thumbs::tool_bin("ffprobe");
            let out = tokio::process::Command::new(&bin)
                .args(["-v", "quiet", "-print_format", "default", "-show_format", "-show_streams", &input])
                .output().await.map_err(|e| anyhow::anyhow!("ffprobe not found ({e})"))?;
            if !out.status.success() {
                anyhow::bail!("ffprobe failed: {}", truncate_msg(&String::from_utf8_lossy(&out.stderr)));
            }
            let report = String::from_utf8_lossy(&out.stdout).to_string();
            tokio::fs::write(&output, &report).await.map_err(|e| anyhow::anyhow!("write {output}: {e}"))?;
            // Surface a one-line summary in the queue row.
            let codec = report.lines().find(|l| l.trim_start().starts_with("codec_name="))
                .and_then(|l| l.split('=').nth(1)).unwrap_or("");
            let dur = report.lines().find(|l| l.trim_start().starts_with("duration="))
                .and_then(|l| l.split('=').nth(1)).unwrap_or("");
            let _ = tulipix_tools::queue::set_progress(pool, id, base + span,
                Some(format!("{codec} · {dur}s → {output}").as_str())).await;
        }
        Native::ContactSheet { input, output, cols, rows } => {
            let dur = ffprobe_duration(&input).await;
            let filter = tulipix_tools::thumbnail::contact_sheet_filter(dur, cols, rows, 320);
            let bin = tulipix_core::thumbs::tool_bin("ffmpeg");
            let mut c = quiet_cmd(&bin);
            c.arg("-y").args(["-i", &input, "-vf", &filter, "-frames:v", "1"]);
            c.arg(&output).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
            let mut child = c.spawn().map_err(|e| anyhow::anyhow!("ffmpeg not found ({e})"))?;
            let err_task = spawn_stderr_log_drain(&mut child);
            let status = wait_child(id, &mut child).await?;
            let err = err_task.await.unwrap_or_default();
            if !status.success() { anyhow::bail!("contact sheet failed: {}", truncate_msg(&err)); }
        }
        Native::CacheClean => {
            let dir = tulipix_core::paths::thumbs_dir();
            let (mut freed, mut count) = (0u64, 0u64);
            if let Some(dir) = dir {
                let d = dir.clone();
                let res = tokio::task::spawn_blocking(move || {
                    let (mut bytes, mut n) = (0u64, 0u64);
                    for entry in walkdir::WalkDir::new(&d).into_iter().filter_map(|e| e.ok()) {
                        if entry.file_type().is_file() {
                            if let Ok(m) = entry.metadata() { bytes += m.len(); }
                            if std::fs::remove_file(entry.path()).is_ok() { n += 1; }
                        }
                    }
                    (bytes, n)
                }).await.unwrap_or((0, 0));
                freed = res.0; count = res.1;
            }
            let mb = freed as f64 / (1024.0 * 1024.0);
            let _ = tulipix_tools::queue::set_progress(pool, id, base + span,
                Some(format!("cleared {count} cached files · {mb:.1} MB freed").as_str())).await;
        }
        // Same subset rule as `tools_run_step`: the archive, crypt, PDF and
        // folder-chore ops are not in this crate's catalogue, so no job started
        // here reaches them. Deliberately no `{:?}` on the variant — `Crypt`
        // carries the passphrase, and this message is stored on the job row.
        _ => anyhow::bail!("this operation is not available in this build"),
    }
    Ok(())
}

/// `[00:01:23.400 --> …]` whisper segment line → seconds of the segment start.
fn parse_whisper_ts(line: &str) -> Option<f64> {
    let rest = line.trim_start().strip_prefix('[')?;
    let (h, rest) = rest.split_once(':')?;
    let (m, rest) = rest.split_once(':')?;
    let s: f64 = rest.split([' ', ']']).next()?.parse().ok()?;
    Some(h.trim().parse::<f64>().ok()? * 3600.0 + m.parse::<f64>().ok()? * 60.0 + s)
}

/// Build a `rel_path → sha256` map for a folder (recursive, streaming hashes).
async fn folder_hash_map(root: &str) -> std::collections::BTreeMap<String, String> {
    let root = root.to_string();
    tokio::task::spawn_blocking(move || {
        let mut m = std::collections::BTreeMap::new();
        let base = std::path::Path::new(&root);
        for entry in walkdir::WalkDir::new(base).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() { continue; }
            let p = entry.path();
            let rel = p.strip_prefix(base).unwrap_or(p).to_string_lossy().to_string();
            if let Ok(Some(hex)) = tulipix_tools::hash::sha256_file_hex_with(&p.to_string_lossy(), || false) {
                m.insert(rel, hex);
            }
        }
        m
    }).await.unwrap_or_default()
}

/// Like `folder_hash_map`, but reports progress into the job row (folder-diff
/// used to sit at 0% for the whole multi-GB hash) and aborts on job stop.
async fn folder_hash_map_prog(
    root: &str, pool: &sqlx::SqlitePool, id: i64, base: f64, span: f64,
) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let root = root.to_string();
    let pool = pool.clone();
    let handle = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || {
        let base_p = std::path::Path::new(&root);
        let files: Vec<std::path::PathBuf> = walkdir::WalkDir::new(base_p).into_iter()
            .filter_map(|e| e.ok()).filter(|e| e.file_type().is_file())
            .map(|e| e.into_path()).collect();
        let total = files.len().max(1);
        let mut m = std::collections::BTreeMap::new();
        let mut last_report = std::time::Instant::now();
        for (i, p) in files.iter().enumerate() {
            if job_stopped(id) { anyhow::bail!("stopped"); }
            let rel = p.strip_prefix(base_p).unwrap_or(p).to_string_lossy().to_string();
            match tulipix_tools::hash::sha256_file_hex_with(&p.to_string_lossy(), || job_stopped(id)) {
                Ok(Some(hex)) => { m.insert(rel, hex); }
                Ok(None) => anyhow::bail!("stopped"),
                Err(_) => {} // unreadable file — skip, like the diff always has
            }
            if last_report.elapsed().as_millis() >= 300 {
                last_report = std::time::Instant::now();
                let f = base + ((i + 1) as f64 / total as f64) * span;
                let _ = handle.block_on(tulipix_tools::queue::set_progress(&pool, id, f, None));
            }
        }
        Ok(m)
    }).await?
}

/// Folder-diff result rows cached at run time, keyed by job id, so the result
/// popup doesn't re-hash both folders just to render the list.
fn diff_cache() -> &'static std::sync::Mutex<std::collections::HashMap<i64, Vec<(String, String, String)>>> {
    static C: OnceLock<std::sync::Mutex<std::collections::HashMap<i64, Vec<(String, String, String)>>>> = OnceLock::new();
    C.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

/// Read tools.db jobs into the Queue model (newest activity first).
async fn tools_refresh_queue(weak: slint::Weak<MainWindow>) {
    let Ok(pool) = pool_for("tools").await else { return; };
    let rows: Vec<(i64, String, String, f64, Option<String>)> = sqlx::query_as(
        "SELECT id, kind, state, progress, message FROM jobs \
         ORDER BY (state IN ('running','queued','paused')) DESC, updated DESC LIMIT 100")
        .fetch_all(&pool).await.unwrap_or_default();
    let summary = {
        let running = rows.iter().filter(|r| r.2 == "running").count();
        let queued = rows.iter().filter(|r| r.2 == "queued").count();
        if running + queued == 0 { "Idle".to_string() }
        else { format!("{running} running · {queued} queued") }
    };
    let log = tools_log_snapshot();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_tools_log(log.into());
        // Header: when a tool is open, show it + its newest job's live progress;
        // otherwise the queue summary.
        let active = w.get_tools_active_op().to_string();
        let header = if !active.is_empty() {
            let label = w.get_tools_active_label().to_string();
            match rows.iter().find(|(_, k, ..)| *k == active) {
                // Downloads write "42% · 4.4 MB / 10.5 MB · 2.1 MB/s — title"
                // into the message — the pill shows only the stats before the
                // " — " (title stays on the queue row); else plain percent.
                Some((_, _, state, progress, msg)) if state == "running" =>
                    match msg.as_deref().map(str::trim).filter(|m| !m.is_empty()) {
                        Some(m) => format!("{label} · {}", m.split(" — ").next().unwrap_or(m).trim()),
                        None => format!("{label} · {}%", (progress * 100.0).round() as i32),
                    },
                Some((_, _, state, ..)) => format!("{label} · {state}"),
                None => label,
            }
        } else { summary.clone() };
        // Queue is scoped to the open tool: show only this tool's jobs (when one
        // is open), so each tool's window carries its own independent queue.
        let jobs: Vec<QueueJob> = rows.into_iter()
            .filter(|(_, kind, ..)| active.is_empty() || *kind == active)
            .map(|(id, kind, state, progress, message)| QueueJob {
                id: id as i32,
                name: tool_label(&kind).into(),
                kind: kind.into(),
                state: state.into(),
                progress: progress as f32,
                message: message.unwrap_or_default().into(),
            }).collect();
        w.set_queue_jobs(slint::ModelRc::new(slint::VecModel::from(jobs)));
        w.set_tools_queue_status(header.into());
    });
}

/// Build one ToolField. `opts`/`min`/`max`/`hint` default to empty/0.
#[allow(clippy::too_many_arguments)]
fn mk_field(key: &str, label: &str, kind: &str, value: &str, required: bool, hint: &str, opts: &[&str], min: f32, max: f32) -> ToolField {
    ToolField {
        key: key.into(), label: label.into(), kind: kind.into(), value: value.into(),
        required, hint: hint.into(), min, max,
        options: slint::ModelRc::new(slint::VecModel::from(
            opts.iter().map(|s| slint::SharedString::from(*s)).collect::<Vec<_>>())),
    }
}

/// The declarative form for a tool kind. Output fields are blank by default →
/// the spec builder fills "beside source + suffix".
fn tool_fields(kind: &str) -> Vec<ToolField> {
    let file = |k: &str, l: &str| mk_field(k, l, "file", "", true, "", &[], 0.0, 0.0);
    let out = || mk_field("output", "Output (blank = beside source)", "text", "", false, "auto-named next to the source", &[], 0.0, 0.0);
    match kind {
        "compress_video" => vec![
            file("input", "Source video"),
            mk_field("codec", "Codec", "dropdown", "h264", true, "", &["h264", "h265", "av1"], 0.0, 0.0),
            mk_field("crf", "Quality — CRF (lower = better, bigger)", "slider", "23", false, "Constant Rate Factor · ~18 near-lossless · 23 default · 28 small · each +6 ≈ half the size", &[], 18.0, 35.0),
            out(),
        ],
        "compress_audio" => vec![
            file("input", "Source audio"),
            mk_field("codec", "Codec", "dropdown", "mp3", true, "", &["mp3", "aac", "opus", "vorbis"], 0.0, 0.0),
            mk_field("kbps", "Bitrate (kbps)", "slider", "192", false, "", &[], 64.0, 320.0),
            out(),
        ],
        "compress_photo" => vec![
            file("input", "Source image"),
            mk_field("format", "Format", "dropdown", "jpeg", true, "", &["jpeg", "webp", "avif"], 0.0, 0.0),
            mk_field("quality", "Quality", "slider", "82", false, "1–100", &[], 1.0, 100.0),
            out(),
        ],
        "convert" => vec![
            file("input", "Source file"),
            mk_field("target_ext", "Convert to", "dropdown", "mp4", true, "", &["mp4", "mkv", "webm", "mp3", "m4a", "opus", "flac", "png", "jpg", "webp"], 0.0, 0.0),
            out(),
        ],
        "trim" => vec![
            file("input", "Source video"),
            mk_field("start_s", "Start (seconds)", "number", "0", true, "e.g. 12.5", &[], 0.0, 0.0),
            mk_field("end_s", "End (seconds)", "number", "", true, "e.g. 48", &[], 0.0, 0.0),
            mk_field("lossless", "Lossless cut (keyframe-aligned)", "toggle", "true", false, "fast, no re-encode", &[], 0.0, 0.0),
            out(),
        ],
        "resize" => vec![
            file("input", "Source image/video"),
            mk_field("mode", "Aspect", "dropdown", "fit", false,
                "fit = keep aspect inside W×H · exact may distort · width/height scale one edge",
                &["fit", "exact", "width", "height"], 0.0, 0.0),
            mk_field("w", "Width (px)", "number", "1920", true, "", &[], 0.0, 0.0),
            mk_field("h", "Height (px)", "number", "1080", true, "", &[], 0.0, 0.0),
            out(),
        ],
        "thumbnail" => vec![
            file("input", "Source video"),
            mk_field("at_s", "At timestamp (seconds)", "number", "1", false, "", &[], 0.0, 0.0),
            mk_field("width", "Thumbnail width (px)", "number", "320", false, "", &[], 0.0, 0.0),
            out(),
        ],
        "extract" => vec![
            file("input", "Source video"),
            mk_field("stream", "Stream", "dropdown", "audio", true, "", &["audio", "subtitle"], 0.0, 0.0),
            mk_field("index", "Track index", "number", "0", false, "0 = first", &[], 0.0, 0.0),
            out(),
        ],
        "normalize" => vec![
            file("input", "Source audio/video"),
            mk_field("lufs", "Target loudness (LUFS)", "number", "-23", false, "EBU R128 = -23", &[], 0.0, 0.0),
            out(),
        ],
        "watermark" => vec![
            file("input", "Source image/video"),
            mk_field("text", "Watermark text", "text", "", true, "shown bottom-right", &[], 0.0, 0.0),
            out(),
        ],
        "burn_subs" => vec![
            file("input", "Source video"),
            mk_field("sub", "Subtitle file (.srt/.ass)", "file", "", true, "", &[], 0.0, 0.0),
            out(),
        ],
        "split" => vec![
            file("input", "Source video"),
            mk_field("every_s", "Segment length (seconds)", "number", "60", true, "", &[], 0.0, 0.0),
            mk_field("output", "Output template (blank = beside source)", "text", "", false, "", &[], 0.0, 0.0),
        ],
        "merge" => vec![
            mk_field("inputs", "Source files (pick several)", "files", "", true, "", &[], 0.0, 0.0),
            mk_field("output", "Output file", "text", "", true, "", &[], 0.0, 0.0),
        ],
        "download" | "download_playlist" | "download_live" => vec![
            mk_field("url", "URL", "text", "", true, "YouTube / Vimeo / 1800+ sites", &[], 0.0, 0.0),
            mk_field("audio_only", "Audio only", "toggle", "false", false, "extract audio", &[], 0.0, 0.0),
            mk_field("max_height", "Resolution", "dropdown", "1080",
                false, "“Best” grabs the highest available",
                &["Best", "2160", "1440", "1080", "720", "480", "360", "240"], 0.0, 0.0),
            mk_field("embed_subs", "Embed subtitles", "toggle", "false", false, "", &[], 0.0, 0.0),
            mk_field("embed_thumbnail", "Embed thumbnail", "toggle", "false", false, "cover art from the video thumbnail", &[], 0.0, 0.0),
            mk_field("output", "Save folder (blank = Downloads)", "folder", "", false, "", &[], 0.0, 0.0),
        ],
        "transcribe" => vec![
            file("input", "Audio/video file"),
            mk_field("language", "Source language", "dropdown", "auto", false,
                "auto-detect, or pick to sharpen accuracy",
                &["auto", "en", "es", "fr", "de", "it", "pt", "nl", "ru", "uk", "pl", "tr", "ar", "fa", "hi", "ur", "bn", "ta", "th", "vi", "id", "ja", "ko", "zh"],
                0.0, 0.0),
            mk_field("translate", "Translate to English", "toggle", "false", false,
                "transcribe any language straight to English (Whisper built-in)", &[], 0.0, 0.0),
            mk_field("output", "Output .srt (blank = beside source)", "text", "", false, "", &[], 0.0, 0.0),
        ],
        "hash" => vec![
            mk_field("files", "Files to hash (pick several)", "files", "", true, "SHA-256", &[], 0.0, 0.0),
            mk_field("manifest", "Save manifest to (blank = show inline)", "text", "", false, "", &[], 0.0, 0.0),
        ],
        "folder_diff" => vec![
            mk_field("a", "Folder A", "folder", "", true, "", &[], 0.0, 0.0),
            mk_field("b", "Folder B", "folder", "", true, "", &[], 0.0, 0.0),
        ],
        "rename" => vec![
            mk_field("dir", "Folder", "folder", "", true, "", &[], 0.0, 0.0),
            mk_field("pattern", "Pattern", "text", "{n:03}_{name}", true, "{n}, {n:03} = sequence · {name} = original name", &[], 0.0, 0.0),
            mk_field("start", "Start number", "number", "1", false, "", &[], 0.0, 0.0),
        ],
        "mediainfo" => vec![
            file("input", "File to inspect"),
            mk_field("output", "Report .txt (blank = beside source)", "text", "", false, "codec/stream/bitrate report", &[], 0.0, 0.0),
        ],
        "contact_sheet" => vec![
            file("input", "Source video"),
            mk_field("cols", "Columns", "number", "4", false, "", &[], 0.0, 0.0),
            mk_field("rows", "Rows", "number", "4", false, "", &[], 0.0, 0.0),
            mk_field("output", "Output .jpg (blank = beside source)", "text", "", false, "", &[], 0.0, 0.0),
        ],
        "cache_clean" => vec![],
        _ => vec![],
    }
}

/// Extension of a path (lowercase), or fallback.
fn ext_of(path: &str) -> String {
    std::path::Path::new(path).extension().and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase()).unwrap_or_else(|| "out".into())
}

/// `dir/stem.suffix.ext` next to `input` (`dir/stem.ext` for an empty suffix —
/// transcribe used to produce "movie..srt").
fn beside_source(input: &str, suffix: &str, ext: &str) -> String {
    let p = std::path::Path::new(input);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let parent = p.parent().map(|x| x.to_path_buf()).unwrap_or_default();
    let name = if suffix.is_empty() { format!("{stem}.{ext}") } else { format!("{stem}.{suffix}.{ext}") };
    parent.join(name).to_string_lossy().to_string()
}

/// Convert the active form's string map into a typed JSON spec, filling the
/// output path (beside source + suffix) when the user left it blank.
fn tools_build_spec(kind: &str, map: &std::collections::HashMap<String, String>) -> serde_json::Value {
    let fields = tool_fields(kind);
    let mut o = serde_json::Map::new();
    for f in &fields {
        let key = f.key.to_string();
        let v = map.get(&key).cloned().unwrap_or_else(|| f.value.to_string());
        let val = match f.kind.as_str() {
            "files" => serde_json::Value::Array(
                v.split('\n').filter(|s| !s.trim().is_empty())
                    .map(|s| serde_json::Value::String(s.trim().to_string())).collect()),
            "number" | "slider" => v.trim().parse::<f64>().ok()
                .map(|n| serde_json::json!(n)).unwrap_or(serde_json::Value::String(v.clone())),
            "toggle" => serde_json::Value::Bool(v == "true"),
            _ => serde_json::Value::String(v),
        };
        o.insert(key, val);
    }
    // Output defaults.
    let input = map.get("input").cloned().unwrap_or_default();
    let blank_out = o.get("output").and_then(|v| v.as_str()).map(|s| s.trim().is_empty()).unwrap_or(true);
    let set_out = |o: &mut serde_json::Map<String, serde_json::Value>, path: String| {
        o.insert("output".into(), serde_json::Value::String(path));
    };
    if blank_out && !input.is_empty() {
        match kind {
            "compress_video" | "trim" | "normalize" | "watermark" | "burn_subs" =>
                set_out(&mut o, beside_source(&input, "tulipix", &ext_of(&input))),
            "compress_audio" => {
                let ext = match map.get("codec").map(|s| s.as_str()) {
                    Some("aac") => "m4a", Some("opus") => "opus", Some("vorbis") => "ogg", _ => "mp3" };
                set_out(&mut o, beside_source(&input, "tulipix", ext));
            }
            "compress_photo" => {
                let ext = match map.get("format").map(|s| s.as_str()) {
                    Some("webp") => "webp", Some("avif") => "avif", _ => "jpg" };
                set_out(&mut o, beside_source(&input, "tulipix", ext));
            }
            "convert" => {
                let ext = map.get("target_ext").cloned().unwrap_or_else(|| "mp4".into());
                set_out(&mut o, beside_source(&input, "tulipix", &ext));
            }
            "resize" => set_out(&mut o, beside_source(&input, "resized", &ext_of(&input))),
            "thumbnail" => set_out(&mut o, beside_source(&input, "thumb", "jpg")),
            "mediainfo" => set_out(&mut o, beside_source(&input, "info", "txt")),
            "contact_sheet" => set_out(&mut o, beside_source(&input, "sheet", "jpg")),
            // .mka holds any audio codec losslessly (-c copy); .m4a rejected
            // opus/ac3/dts tracks. Subtitles land as .srt (re-encoded).
            "extract" => {
                let ext = if map.get("stream").map(|s| s.as_str()) == Some("subtitle") { "srt" } else { "mka" };
                set_out(&mut o, beside_source(&input, "track", ext));
            }
            "transcribe" => set_out(&mut o, beside_source(&input, "", "srt")),
            "split" => {
                let p = std::path::Path::new(&input);
                let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
                let ext = ext_of(&input);
                let tmpl = p.parent().unwrap_or_else(|| std::path::Path::new("."))
                    .join(format!("{stem}_%03d.{ext}"));
                set_out(&mut o, tmpl.to_string_lossy().to_string());
            }
            _ => {}
        }
    }
    // downloads: blank "output" folder → into the user's ~/Downloads (the same
    // dir the "Downloaded" list reads — dirs_default() is the app CONFIG dir,
    // and files written there were invisible to the user). Playlists get their
    // own subfolder so 50 items don't flood the folder root.
    if matches!(kind, "download" | "download_playlist" | "download_live") {
        let folder = o.get("output").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let base = if folder.is_empty() { tools_download_dir() } else { std::path::PathBuf::from(&folder) };
        let tmpl = if kind == "download_playlist" {
            base.join("%(playlist_title)s").join("%(title)s.%(ext)s")
        } else {
            base.join("%(title)s.%(ext)s")
        };
        // Resolved folder into "output" so the queue row's Open button works
        // (it opens the spec's `output` path — blank meant a dead button).
        o.insert("output".into(), serde_json::Value::String(base.to_string_lossy().to_string()));
        o.insert("out_template".into(), serde_json::Value::String(tmpl.to_string_lossy().to_string()));
    }
    serde_json::Value::Object(o)
}

/// Rebuild the active tool's field model, overlaying current edits.
fn tools_apply_form(w: &MainWindow) {
    let (kind, map) = { let g = tools_form().lock().unwrap(); (g.0.clone(), g.1.clone()) };
    let fields: Vec<ToolField> = tool_fields(&kind).into_iter().map(|mut f| {
        if let Some(v) = map.get(f.key.as_str()) { f.value = v.as_str().into(); }
        f
    }).collect();
    w.set_tools_fields(slint::ModelRc::new(slint::VecModel::from(fields)));
}

/// Resolve the whisper.cpp CLI. The upstream binary was renamed across releases
/// (`main` → `whisper` → `whisper-cli`) and distros package it under a couple of
/// names, so try each: the Tools dir / bundled copy first (tool_bin), then PATH.
fn whisper_bin() -> Option<std::path::PathBuf> {
    for name in ["whisper-cli", "whisper-cpp"] {
        let p = tulipix_core::thumbs::tool_bin(name);
        if p.is_absolute() && p.exists() { return Some(p); }
        if on_path(name) { return Some(std::path::PathBuf::from(name)); }
    }
    None
}

/// Resolve the whisper model at runtime, honouring the per-task model choice
/// in Settings → AI Features (`ai.model.transcribe`): a downloaded base /
/// small / turbo model when present, else the bundled tiny model.
fn whisper_model() -> Option<std::path::PathBuf> {
    tulipix_core::ai_models::whisper_model_for("transcribe")
}

/// Where a tool binary resolves from, for the Settings tab.
fn tool_source(name: &str) -> &'static str {
    // Mirror the main Settings page (tool_row): user Tools dir → bundled → PATH.
    // The old path-equality check against tool_bin() reported false "Missing" for
    // tools like exiftool (perl script / resolved path differs from the join).
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let in_tools_dir = tulipix_core::settings::Settings::load().ok()
        .map(|s| s.text("tools.bin-dir")).filter(|d| !d.trim().is_empty())
        .map(|d| std::path::Path::new(d.trim()).join(format!("{name}{ext}")).exists())
        .unwrap_or(false);
    if in_tools_dir { "Tools dir" }
    else if tulipix_core::thumbs::bundled_file(&format!("{name}{ext}")).is_some() { "Bundled" }
    else if on_path(name) || on_path(&format!("{name}{ext}")) { "System PATH" }
    else { "Missing" }
}

/// First line of `<bin> --version` (or `-version`), trimmed.
async fn tool_version(name: &str) -> String {
    let bin = tulipix_core::thumbs::tool_bin(name);
    for flag in ["--version", "-version"] {
        if let Ok(o) = tokio::process::Command::new(&bin).arg(flag).output().await {
            if o.status.success() {
                let s = String::from_utf8_lossy(&o.stdout);
                let line = s.lines().next().unwrap_or("").trim();
                if !line.is_empty() { return line.chars().take(64).collect(); }
            }
        }
    }
    String::new()
}

/// Probe every external tool the app uses → the Settings-tab model.
async fn tools_refresh_status(weak: slint::Weak<MainWindow>) {
    // (name, what it powers)
    const TOOLS: &[(&str, &str)] = &[
        ("ffmpeg", "transcode · compress · convert"),
        ("ffprobe", "media inspection"),
        ("yt-dlp", "URL downloads"),
        ("whisper-cli", "transcription"),
        ("mpv", "playback"),
        ("rclone", "cloud sync"),
        ("exiftool", "EXIF metadata"),
    ];
    let mut rows: Vec<ToolStatus> = Vec::new();
    for (name, role) in TOOLS {
        let source = tool_source(name);
        let detail = if source == "Missing" { role.to_string() } else {
            let v = tool_version(name).await;
            if v.is_empty() { role.to_string() } else { v }
        };
        rows.push(ToolStatus {
            name: (*name).into(),
            source: source.into(),
            detail: detail.into(),
            available: source != "Missing",
            updatable: *name == "yt-dlp" && source != "Missing",
        });
    }
    // Bundled AI model (not a binary) — resolved at runtime like the job does.
    let have_model = whisper_model().is_some();
    rows.push(ToolStatus {
        name: "ggml-tiny (whisper model)".into(),
        source: if have_model { "Bundled".into() } else { "Missing".into() },
        detail: if have_model { "75 MB · transcription model".into() } else { "transcription model".into() },
        available: have_model, updatable: false,
    });
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_tool_statuses(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
}

/// Register all Tools-section callbacks + start the queue worker/poller.
pub fn wire(window: &MainWindow) {
    // ── Tools: category tabs + op search ──
    let w = window.as_weak();
    window.on_tools_set_category(move |c| {
        if let Some(w0) = w.upgrade() {
            w0.set_tools_category(c.clone());
            tools_refresh(&w0);
            if c == "settings" {
                let weak = w0.as_weak();
                tokio::runtime::Handle::current().spawn(async move { tools_refresh_status(weak).await; });
            }
        }
    });
    let w = window.as_weak();
    window.on_tools_search(move |_q| {
        if let Some(w0) = w.upgrade() { tools_refresh(&w0); }
    });
    // Open a tool → seed its form defaults + show the inline detail panel.
    let w = window.as_weak();
    window.on_tools_open(move |kind| {
        let Some(w0) = w.upgrade() else { return; };
        let kind = kind.to_string();
        let mut map = std::collections::HashMap::new();
        for f in tool_fields(&kind) { map.insert(f.key.to_string(), f.value.to_string()); }
        *tools_form().lock().unwrap() = (kind.clone(), map);
        w0.set_tools_active_op(kind.as_str().into());
        w0.set_tools_active_label(tool_label(&kind).into());
        w0.set_tools_active_info(tool_info(&kind).into());
        w0.set_tools_queue_status(tool_label(&kind).into()); // header shows the tool immediately
        w0.set_tools_error("".into());
        tools_apply_form(&w0);
        // Populate the in-window queue with any jobs already running/queued.
        let weak = w0.as_weak();
        tokio::runtime::Handle::current().spawn(async move { tools_refresh_queue(weak).await; });
    });
    // Field edits (no model rebuild — the control holds its own value).
    window.on_tools_field_set(move |key, value| {
        tools_form().lock().unwrap().1.insert(key.to_string(), value.to_string());
    });
    // File/folder pickers → write the path back + rebuild the model.
    let w = window.as_weak();
    window.on_tools_field_pick(move |key| {
        let Some(w0) = w.upgrade() else { return; };
        let key = key.to_string();
        let active = w0.get_tools_active_op().to_string();
        let fkind = tool_fields(&active).into_iter().find(|f| f.key == key)
            .map(|f| f.kind.to_string()).unwrap_or_default();
        let picked: Option<String> = match fkind.as_str() {
            "folder" => rfd::FileDialog::new().pick_folder().map(|p| p.to_string_lossy().to_string()),
            "files"  => rfd::FileDialog::new().pick_files().map(|ps|
                ps.iter().map(|p| p.to_string_lossy().to_string()).collect::<Vec<_>>().join("\n")),
            _ => rfd::FileDialog::new().pick_file().map(|p| p.to_string_lossy().to_string()),
        };
        if let Some(v) = picked {
            tools_form().lock().unwrap().1.insert(key, v);
            tools_apply_form(&w0);
        }
    });
    // Run → validate required fields, build the spec, submit. The tool stays
    // open; the job appears in the in-window queue (below the form) with live
    // progress. (Keeping the panel mounted also sidesteps the live-preview
    // interpreter's "deleted parent" panic, #6426 — the Run button no longer
    // removes the element it fires from.)
    let w = window.as_weak();
    window.on_tools_run(move || {
        let Some(w0) = w.upgrade() else { return; };
        let (kind, map) = { let g = tools_form().lock().unwrap(); (g.0.clone(), g.1.clone()) };
        if kind.is_empty() { return; }
        // Validate up front — a typo'd path used to fail minutes later with a
        // raw ffmpeg stderr blob, and "abc" in a number field was silently
        // swapped for the default.
        for f in tool_fields(&kind) {
            let val = map.get(f.key.as_str()).map(|v| v.trim().to_string()).unwrap_or_default();
            if f.required && val.is_empty() {
                w0.set_tools_error(format!("{} is required", f.label).into());
                return;
            }
            if val.is_empty() { continue; }
            match f.kind.as_str() {
                "number" | "slider" => if val.parse::<f64>().is_err() {
                    w0.set_tools_error(format!("{} must be a number", f.label).into());
                    return;
                },
                "file" | "folder" => if !std::path::Path::new(&val).exists() {
                    w0.set_tools_error(format!("{}: not found — {}", f.label, val).into());
                    return;
                },
                "files" => for line in val.lines().map(str::trim).filter(|s| !s.is_empty()) {
                    if !std::path::Path::new(line).exists() {
                        w0.set_tools_error(format!("File not found — {line}").into());
                        return;
                    }
                },
                _ => {}
            }
        }
        let spec_v = tools_build_spec(&kind, &map);
        // The worker runs ffmpeg with -y; an output equal to a source would
        // silently destroy the original file.
        if let Some(out) = spec_v.get("output").and_then(|v| v.as_str()) {
            let same = |i: &str| !i.is_empty() && std::path::Path::new(i) == std::path::Path::new(out);
            let clash = spec_v.get("input").and_then(|v| v.as_str()).map(same).unwrap_or(false)
                || spec_v.get("sub").and_then(|v| v.as_str()).map(same).unwrap_or(false)
                || spec_v.get("inputs").and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str()).any(same)).unwrap_or(false);
            if clash {
                w0.set_tools_error("Output path equals the source — pick a different output.".into());
                return;
            }
        }
        w0.set_tools_error("".into());
        let spec = spec_v.to_string();
        let weak = w0.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
                // Double-click Run guard: with 2 workers, twin jobs would write
                // the same output file concurrently.
                if let Ok(true) = tulipix_tools::queue::has_active_duplicate(&pool, &kind, &spec).await {
                    let _ = weak.upgrade_in_event_loop(|w| {
                        w.set_tools_error("This exact job is already queued or running.".into());
                    });
                    return;
                }
                let _ = tulipix_tools::queue::submit(&pool, &kind, &spec, 0).await;
                tools_refresh_queue(weak).await;
            }
        });
    });
    // Reset → re-seed the active tool's defaults.
    let w = window.as_weak();
    window.on_tools_reset(move || {
        let Some(w0) = w.upgrade() else { return; };
        let kind = w0.get_tools_active_op().to_string();
        let mut map = std::collections::HashMap::new();
        for f in tool_fields(&kind) { map.insert(f.key.to_string(), f.value.to_string()); }
        *tools_form().lock().unwrap() = (kind, map);
        w0.set_tools_error("".into());
        tools_apply_form(&w0);
    });
    let w = window.as_weak();
    window.on_tools_close(move || {
        if let Some(w0) = w.upgrade() {
            // Same as Run: defer clearing active-op (the Close button lives in
            // the panel it removes) to dodge interpreter #6426.
            let weak_v = w0.as_weak();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w0) = weak_v.upgrade() { w0.set_tools_active_op("".into()); }
            });
            let weak = w0.as_weak();
            tokio::runtime::Handle::current().spawn(async move { tools_refresh_queue(weak).await; });
        }
    });
    // Settings tab: self-update a tool (yt-dlp -U), then re-probe statuses.
    let w = window.as_weak();
    window.on_tools_update_tool(move |name| {
        let Some(w0) = w.upgrade() else { return; };
        let weak = w0.as_weak();
        let name = name.to_string();
        tokio::runtime::Handle::current().spawn(async move {
            if name == "yt-dlp" {
                // The app's updater, not yt-dlp's own `-U`: `-U` overwrites the
                // running binary in place and fails silently on a packaged
                // install where `resources/bin/` is not writable — which is how
                // the bundled copy went eight weeks stale and started answering
                // 403. `update_ytdlp_now` picks a destination it has checked it
                // can write and the tool resolver prefers.
                let r = tokio::task::spawn_blocking(tulipix_core::updater::update_ytdlp_now).await;
                match r {
                    Ok(Ok(Some(v))) => tracing::info!(version = %v, "yt-dlp updated"),
                    Ok(Ok(None)) => tracing::info!("yt-dlp already current"),
                    Ok(Err(e)) => tracing::warn!(error = %e, "yt-dlp update failed"),
                    Err(e) => tracing::warn!(error = %e, "yt-dlp update task failed"),
                }
            }
            tools_refresh_status(weak).await;
        });
    });
    // Download tool: list finished downloads + open one in the OS default app.
    fn push_downloads(weak: slint::Weak<MainWindow>) {
        tokio::runtime::Handle::current().spawn(async move {
            let rows = list_downloaded().await;
            let _ = weak.upgrade_in_event_loop(move |w| {
                let model: Vec<DownloadItem> = rows.into_iter()
                    .map(|(name, path, meta)| DownloadItem { name: name.into(), path: path.into(), meta: meta.into() })
                    .collect();
                w.set_tools_downloads(slint::ModelRc::new(slint::VecModel::from(model)));
            });
        });
    }
    let w = window.as_weak();
    window.on_tools_list_downloads(move || { push_downloads(w.clone()); });
    window.on_tools_open_download(move |path| {
        let p = std::path::PathBuf::from(path.to_string());
        std::thread::spawn(move || { let _ = tulipix_platform::fm::open_default(&p); });
    });
    let w = window.as_weak();
    window.on_tools_remove_download(move |path| {
        let weak = w.clone();
        let path = path.to_string();
        tokio::runtime::Handle::current().spawn(async move {
            let _ = std::fs::remove_file(std::path::PathBuf::from(&path));
            if let Ok(pool) = pool_for("tools").await {
                let _ = tulipix_tools::queue::forget_download(&pool, &path).await;
            }
            push_downloads(weak);
        });
    });
    // "Open folder" next to Clear all: the tool's save/source folder on disk.
    window.on_tools_open_folder(move || {
        let (kind, map) = { let g = tools_form().lock().unwrap(); (g.0.clone(), g.1.clone()) };
        let custom = map.get("output").map(|s| s.trim().to_string()).unwrap_or_default();
        let dir = if !custom.is_empty() && std::path::Path::new(&custom).is_dir() {
            std::path::PathBuf::from(custom)
        } else if matches!(kind.as_str(), "download" | "download_playlist" | "download_live") {
            tools_download_dir()
        } else {
            // Other tools: the source file's folder (outputs land beside it).
            map.get("input").map(|i| std::path::Path::new(i.trim()).parent()
                    .map(|p| p.to_path_buf()).unwrap_or_default())
                .filter(|p| p.is_dir())
                .unwrap_or_else(tools_download_dir)
        };
        std::thread::spawn(move || { let _ = tulipix_platform::fm::open_default(&dir); });
    });
    // Queue row actions + worker-slot slider.
    let w = window.as_weak();
    window.on_queue_action(move |id, act| {
        let Some(w0) = w.upgrade() else { return; };
        let weak = w0.as_weak();
        let (id, act) = (id as i64, act.to_string());
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
                // Mark the state first (so the worker's `complete` can't
                // overwrite it), then stop the live process.
                let _ = match act.as_str() {
                    "pause"  => {
                        let r = tulipix_tools::queue::pause(&pool, id).await;
                        job_ctl_fire(id);
                        r
                    }
                    "resume" => tulipix_tools::queue::resume(&pool, id).await,
                    "cancel" => {
                        let r = tulipix_tools::queue::cancel(&pool, id).await;
                        job_ctl_fire(id);
                        r
                    }
                    "retry"  => tulipix_tools::queue::retry(&pool, id).await,
                    "remove" => tulipix_tools::queue::remove(&pool, id).await,
                    "clear"  => tulipix_tools::queue::clear_all(&pool).await,
                    "up"     => tulipix_tools::queue::reorder(&pool, id, true).await,
                    "down"   => tulipix_tools::queue::reorder(&pool, id, false).await,
                    // Open the job's output in the system default app — works even
                    // for a halted/partial job, as long as the file exists on disk.
                    "open"   => { tools_open_job_output(&pool, id).await; Ok(()) }
                    _ => Ok(()),
                };
                tools_refresh_queue(weak).await;
            }
        });
    });
    window.on_tools_set_workers(move |n| {
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
                let _ = tulipix_tools::queue::set_worker_slots(&pool, n as i64).await;
            }
        });
    });
    window.on_tools_clear_console({
        let w = window.as_weak();
        move || {
            tools_log_clear();
            if let Some(w0) = w.upgrade() { w0.set_tools_log(slint::SharedString::new()); }
        }
    });
    window.on_tools_result({
        let w = window.as_weak();
        move |id| {
            let weak = w.clone();
            let id = id as i64;
            tokio::runtime::Handle::current().spawn(async move {
                let Ok(pool) = pool_for("tools").await else { return; };
                let row: Option<(String, String)> = sqlx::query_as("SELECT kind, spec_json FROM jobs WHERE id = ?")
                    .bind(id).fetch_optional(&pool).await.ok().flatten();
                let Some((kind, spec)) = row else { return; };
                let v: serde_json::Value = serde_json::from_str(&spec).unwrap_or_default();
                if kind == "mediainfo" {
                    let input = v.get("input").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let title = std::path::Path::new(&input).file_name()
                        .map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "Media info".into());
                    let rows = tokio::task::spawn_blocking(move || mediainfo_rows(&input)).await.unwrap_or_default();
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        let model: Vec<ToolInfoRow> = rows.into_iter()
                            .map(|(section, key, value)| ToolInfoRow { section: section.into(), key: key.into(), value: value.into() })
                            .collect();
                        w.set_tools_result_info(slint::ModelRc::new(slint::VecModel::from(model)));
                        w.set_tools_result_diff(slint::ModelRc::new(slint::VecModel::from(Vec::<ToolDiffRow>::new())));
                        w.set_tools_result_kind("info".into());
                        w.set_tools_result_title(title.into());
                        w.set_tools_result_open(true);
                    });
                } else if kind == "folder_diff" {
                    let a = v.get("a").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let b = v.get("b").and_then(|x| x.as_str()).unwrap_or("").to_string();
                    let rows = folderdiff_rows(id, &a, &b).await;
                    let title = format!("Folder diff · {} changes", rows.len());
                    let _ = weak.upgrade_in_event_loop(move |w| {
                        let model: Vec<ToolDiffRow> = rows.into_iter().map(|(kind, path, detail, thumb)| ToolDiffRow {
                            kind: kind.into(), path: path.into(), detail: detail.into(),
                            thumb: thumb.and_then(|p| slint::Image::load_from_path(&p).ok()).unwrap_or_default(),
                        }).collect();
                        w.set_tools_result_diff(slint::ModelRc::new(slint::VecModel::from(model)));
                        w.set_tools_result_info(slint::ModelRc::new(slint::VecModel::from(Vec::<ToolInfoRow>::new())));
                        w.set_tools_result_kind("diff".into());
                        w.set_tools_result_title(title.into());
                        w.set_tools_result_open(true);
                    });
                }
            });
        }
    });
    window.on_tools_result_close({
        let w = window.as_weak();
        move || { if let Some(w0) = w.upgrade() { w0.set_tools_result_open(false); } }
    });
    window.on_tools_trim_editor({
        let w = window.as_weak();
        move || {
            let input = { let g = tools_form().lock().unwrap(); g.1.get("input").cloned().unwrap_or_default() };
            let Some(w0) = w.upgrade() else { return; };
            if input.is_empty() {
                // Surface why nothing happened instead of failing silently.
                w0.set_tools_trim_path(slint::SharedString::new());
                w0.set_tools_trim_status("Pick a source video first (use Choose… above), then Open timeline editor.".into());
                w0.set_tools_trim_open(true);
                return;
            }
            if let Ok(mut g) = trim_segs().lock() { g.clear(); }
            // Open the editor IMMEDIATELY with a loading state — filmstrip + full
            // waveform decode can take seconds, and a delayed modal looks dead.
            w0.set_tools_trim_path(input.clone().into());
            w0.set_tools_trim_duration(0.0);
            w0.set_tools_trim_playhead(0.0);
            w0.set_tools_trim_filmstrip(slint::Image::default());
            w0.set_tools_trim_waveform(slint::Image::default());
            trim_push_segments(&w0);
            w0.set_tools_trim_status("Generating timeline… (decoding video + audio)".into());
            w0.set_tools_trim_open(true);
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let inp = input.clone();
                let (dur, strip, wave) = tokio::task::spawn_blocking(move || {
                    let d = trim_probe_duration(&inp);
                    (d, trim_gen_filmstrip(&inp, d), trim_gen_waveform(&inp))
                }).await.unwrap_or((0.0, None, None));
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_tools_trim_duration(dur as f32);
                    if let Some(p) = strip { w.set_tools_trim_filmstrip(slint::Image::load_from_path(&p).unwrap_or_default()); }
                    if let Some(p) = wave { w.set_tools_trim_waveform(slint::Image::load_from_path(&p).unwrap_or_default()); }
                    w.set_tools_trim_status(if dur > 0.0 { slint::SharedString::new() }
                        else { "Could not read this file (is it a valid video?)".into() });
                });
            });
        }
    });
    window.on_tools_trim_add_seg({
        let w = window.as_weak();
        move |s, e| {
            if let Ok(mut g) = trim_segs().lock() { g.push((s, e)); }
            if let Some(w0) = w.upgrade() { trim_push_segments(&w0); }
        }
    });
    window.on_tools_trim_del_seg({
        let w = window.as_weak();
        move |i| {
            if let Ok(mut g) = trim_segs().lock() { let i = i as usize; if i < g.len() { g.remove(i); } }
            if let Some(w0) = w.upgrade() { trim_push_segments(&w0); }
        }
    });
    window.on_tools_trim_close({
        let w = window.as_weak();
        move || { if let Some(w0) = w.upgrade() { w0.set_tools_trim_open(false); } }
    });
    window.on_tools_trim_join({
        let w = window.as_weak();
        move || {
            let input = { let g = tools_form().lock().unwrap(); g.1.get("input").cloned().unwrap_or_default() };
            let segs: Vec<(f32, f32)> = trim_segs().lock().map(|g| g.clone()).unwrap_or_default();
            if input.is_empty() || segs.is_empty() { return; }
            let out = beside_source(&input, "trimmed", &ext_of(&input));
            if let Some(w0) = w.upgrade() { w0.set_tools_trim_status("Joining…".into()); }
            let weak = w.clone();
            tokio::runtime::Handle::current().spawn(async move {
                let (inp, out2) = (input, out.clone());
                let res = tokio::task::spawn_blocking(move || trim_join_run(&inp, &segs, &out2))
                    .await.unwrap_or_else(|_| Err("join task panicked".into()));
                let _ = weak.upgrade_in_event_loop(move |w| {
                    match res {
                        Ok(()) => w.set_tools_trim_status(format!("Saved → {out}").into()),
                        Err(e) => w.set_tools_trim_status(format!("Failed: {e}").into()),
                    }
                });
            });
        }
    });
    tools_refresh(window);
    // Load persisted worker-slot count + start the queue drainer. Jobs left
    // 'running' by a crash/quit are re-queued first — they'd otherwise occupy
    // worker slots forever and silently stall the queue.
    {
        let weak = window.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
                let _ = tulipix_tools::queue::recover_stale(&pool).await;
                let n = tulipix_tools::queue::worker_slots(&pool).await.unwrap_or(2);
                let _ = weak.upgrade_in_event_loop(move |w| w.set_tools_worker_slots(n as i32));
            }
            tools_start_worker();
        });
    }
    // Poll tools.db into the Queue model (also catches CLI-submitted jobs).
    {
        let weak = window.as_weak();
        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, std::time::Duration::from_millis(750), move || {
            let w = weak.clone();
            tokio::runtime::Handle::current().spawn(async move { tools_refresh_queue(w).await; });
        });
        std::mem::forget(timer); // fire for the app's lifetime
    }
}

