//! Tools-section wiring extracted from tulipix-app: declarative form bridge,
//! job-queue worker/executor glue, and every `window.on_tools_*` callback.
//! tulipix-app calls [`wire`] once at startup.

use slint::ComponentHandle;
use std::sync::OnceLock;
use tulipix_ui::*;
use tulipix_common::{bundled_bin_dir, bundled_present, dirs_default, on_path, pool_for};
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
        ("Download", "download"), ("Live record", "download_live")]),
    ("audio",   "Audio",     "🎵", &[
        ("Compress audio", "compress_audio"), ("Normalise (R128)", "normalize"),
        ("Extract audio", "extract")]),
    ("photo",   "Photo",     "🖼", &[
        ("Compress photo", "compress_photo"), ("Resize", "resize"), ("Watermark", "watermark")]),
    ("subs",    "Subtitles", "💬", &[
        ("Transcribe", "transcribe"), ("Burn-in subtitles", "burn_subs")]),
];

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
        "rename" => "Rename", "transcribe" => "Transcribe", "pdf" => "PDF",
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

/// List media files in the downloads dir, newest first, as (name, abs, meta).
fn list_downloaded() -> Vec<(String, String, String)> {
    let dir = tools_download_dir();
    let mut items: Vec<(std::time::SystemTime, String, String, String)> = std::fs::read_dir(&dir)
        .into_iter().flatten().flatten()
        .filter_map(|e| {
            let p = e.path();
            if !p.is_file() { return None; }
            // Skip yt-dlp partials.
            let name = p.file_name()?.to_str()?.to_string();
            if name.ends_with(".part") || name.ends_with(".ytdl") { return None; }
            let md = e.metadata().ok()?;
            let mtime = md.modified().unwrap_or(std::time::UNIX_EPOCH);
            Some((mtime, name, p.to_string_lossy().into_owned(), human_bytes(md.len())))
        })
        .collect();
    items.sort_by(|a, b| b.0.cmp(&a.0));
    items.into_iter().take(200).map(|(_, n, p, sz)| (n, p, sz)).collect()
}

/// One-paragraph "what it does + how to use it" blurb shown in the tool's info
/// box, above its form. Keep it concrete: name the inputs and the result.
fn tool_info(kind: &str) -> &'static str {
    match kind {
        "compress_video" => "Shrinks a video's file size by re-encoding it at a chosen quality. Pick the source video, set the quality/CRF, then Run — the smaller copy is written beside the original.",
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
        "download_live" => "Records a live stream to disk via yt-dlp until you stop it. Paste the stream URL, then Run.",
        "hash" => "Computes checksums (e.g. SHA-256) for a file so you can verify its integrity. Pick the file, then Run; the digest shows in the queue row.",
        "folder_diff" => "Compares two folders and reports which files are added, removed, or changed between them. Choose folder A and folder B, then Run — the differences are listed in the result.",
        "rename" => "Batch-renames files in a folder using a pattern. Pick the folder, set the naming pattern, then Run.",
        "transcribe" => "Transcribes speech in an audio/video file to a text/subtitle file using Whisper. Pick the file, choose the model, then Run.",
        "pdf" => "Runs a PDF operation (merge/split/compress). Pick the PDF(s), choose the action, then Run.",
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

/// Spawn the queue drainer. Idempotent-ish: call once at startup.
fn tools_start_worker() {
    let handle = tokio::runtime::Handle::current();
    handle.spawn(async move {
        loop {
            let pool = match pool_for("tools").await {
                Ok(p) => p,
                Err(_) => { tokio::time::sleep(std::time::Duration::from_secs(2)).await; continue; }
            };
            match tulipix_tools::queue::claim_next(&pool).await {
                Ok(Some(id)) => {
                    let pool2 = pool.clone();
                    tokio::spawn(async move {
                        let row: Option<(String, String)> =
                            sqlx::query_as("SELECT kind, spec_json FROM jobs WHERE id = ?")
                                .bind(id).fetch_optional(&pool2).await.ok().flatten();
                        let Some((kind, spec)) = row else { return; };
                        let res = tools_run_job(&pool2, id, &kind, &spec).await;
                        let _ = match res {
                            Ok(msg) => tulipix_tools::queue::complete(&pool2, id, true, Some(msg.as_str())).await,
                            Err(e)  => tulipix_tools::queue::complete(&pool2, id, false, Some(truncate_msg(&e.to_string()).as_str())).await,
                        };
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
async fn tools_run_job(pool: &sqlx::SqlitePool, id: i64, kind: &str, spec_json: &str) -> anyhow::Result<String> {
    let spec: serde_json::Value = serde_json::from_str(spec_json).unwrap_or_else(|_| serde_json::json!({}));
    let steps = tulipix_tools::exec::plan(kind, &spec)?;
    let n = steps.len().max(1);
    for (i, step) in steps.into_iter().enumerate() {
        tools_run_step(pool, id, step, i as f64 / n as f64, 1.0 / n as f64).await?;
    }
    Ok("Completed".into())
}

/// Configure a Command to spawn silently on Windows (no console window).
fn quiet_cmd(bin: &std::path::Path) -> tokio::process::Command {
    let cmd = tokio::process::Command::new(bin);
    #[cfg(windows)]
    { let mut cmd = cmd; use std::os::windows::process::CommandExt; cmd.creation_flags(0x0800_0000); return cmd; }
    #[cfg(not(windows))]
    cmd
}

/// Drain stderr concurrently (avoids a full-pipe deadlock) and return it.
fn spawn_stderr_drain(child: &mut tokio::process::Child) -> tokio::task::JoinHandle<String> {
    let stderr = child.stderr.take();
    tokio::spawn(async move {
        use tokio::io::AsyncReadExt;
        let mut s = String::new();
        if let Some(mut se) = stderr { let _ = se.read_to_string(&mut s).await; }
        s
    })
}

async fn tools_run_step(pool: &sqlx::SqlitePool, id: i64, step: tulipix_tools::exec::Step, base: f64, span: f64) -> anyhow::Result<()> {
    use tokio::io::AsyncBufReadExt;
    use tulipix_tools::exec::Step;
    match step {
        Step::Ffmpeg { args, duration_input } => {
            let bin = tulipix_core::thumbs::tool_bin("ffmpeg");
            let dur = match &duration_input { Some(p) => ffprobe_duration(p).await, None => 0.0 };
            let mut cmd = quiet_cmd(&bin);
            cmd.arg("-y").arg("-nostdin");
            cmd.args(&args);
            cmd.arg("-progress").arg("pipe:1").arg("-nostats");
            cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("ffmpeg not found ({e}) — set Tools directory in Settings"))?;
            let err_task = spawn_stderr_drain(&mut child);
            if let Some(stdout) = child.stdout.take() {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(f) = tulipix_tools::exec::parse_ffmpeg_progress(&line, dur) {
                        let _ = tulipix_tools::queue::set_progress(pool, id, base + f * span, None).await;
                    }
                }
            }
            let status = child.wait().await?;
            let err = err_task.await.unwrap_or_default();
            if !status.success() { anyhow::bail!("ffmpeg failed: {}", truncate_msg(&err)); }
        }
        Step::YtDlp { args } => {
            let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
            let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
            let mut cmd = quiet_cmd(&bin);
            // Land downloads in ~/Downloads (the out_template is relative) so the
            // in-tool "Downloaded" list can find + open them.
            cmd.current_dir(tools_download_dir());
            cmd.arg("--newline");
            if let Some(dir) = ff.parent() { cmd.arg("--ffmpeg-location").arg(dir); }
            cmd.args(&args);
            cmd.stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
            let mut child = cmd.spawn().map_err(|e| anyhow::anyhow!("yt-dlp not found ({e}) — set Tools directory in Settings"))?;
            let err_task = spawn_stderr_drain(&mut child);
            if let Some(stdout) = child.stdout.take() {
                let mut lines = tokio::io::BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(f) = tulipix_tools::exec::parse_ytdlp_progress(&line) {
                        let _ = tulipix_tools::queue::set_progress(pool, id, base + f * span, None).await;
                    }
                }
            }
            let status = child.wait().await?;
            let err = err_task.await.unwrap_or_default();
            if !status.success() { anyhow::bail!("yt-dlp failed: {}", truncate_msg(&err)); }
        }
        Step::Native(n) => tools_run_native(pool, id, n, base, span).await?,
    }
    let _ = tulipix_tools::queue::set_progress(pool, id, base + span, None).await;
    Ok(())
}

async fn tools_run_native(pool: &sqlx::SqlitePool, id: i64, n: tulipix_tools::exec::Native, base: f64, span: f64) -> anyhow::Result<()> {
    use tulipix_tools::exec::Native;
    use tulipix_tools::hash::{manifest_line, sha256_hex, Algo};
    match n {
        Native::Hash { files, manifest, .. } => {
            let total = files.len().max(1);
            let mut lines = Vec::new();
            for (i, f) in files.iter().enumerate() {
                let bytes = tokio::fs::read(f).await.map_err(|e| anyhow::anyhow!("read {f}: {e}"))?;
                let hex = sha256_hex(&bytes);
                lines.push(manifest_line(Algo::Sha256, &hex, f));
                let _ = tulipix_tools::queue::set_progress(pool, id, base + ((i + 1) as f64 / total as f64) * span, None).await;
            }
            if let Some(m) = manifest { tokio::fs::write(&m, lines.join("\n")).await?; }
            else { let _ = tulipix_tools::queue::set_progress(pool, id, base + span, Some(lines.join(" · ").as_str())).await; }
        }
        Native::FolderDiff { a, b } => {
            let ma = folder_hash_map(&a).await;
            let mb = folder_hash_map(&b).await;
            let d = tulipix_tools::folder_diff::diff(&ma, &mb);
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
            let empty = std::collections::HashMap::new();
            let total = entries.len().max(1);
            for (i, src) in entries.iter().enumerate() {
                let src_s = src.to_string_lossy().to_string();
                let newname = tulipix_tools::rename::expand(&pattern, &empty, start + i, &src_s);
                if let Some(parent) = src.parent() {
                    let dst = parent.join(&newname);
                    if dst != *src { let _ = std::fs::rename(src, &dst); }
                }
                let _ = tulipix_tools::queue::set_progress(pool, id, base + ((i + 1) as f64 / total as f64) * span, None).await;
            }
        }
        Native::Merge { inputs, output } => {
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
            let err_task = spawn_stderr_drain(&mut child);
            let status = child.wait().await?;
            let err = err_task.await.unwrap_or_default();
            let _ = tokio::fs::remove_file(&tmp).await;
            if !status.success() { anyhow::bail!("merge failed: {}", truncate_msg(&err)); }
        }
        Native::Transcribe { input, output } => {
            // 1. Extract 16 kHz mono PCM wav (what whisper.cpp expects).
            let ff = tulipix_core::thumbs::tool_bin("ffmpeg");
            let wav = std::env::temp_dir().join(format!("tulipix-whisper-{id}.wav"));
            let mut c = quiet_cmd(&ff);
            c.arg("-y").args(["-i", &input, "-ar", "16000", "-ac", "1", "-c:a", "pcm_s16le"]);
            c.arg(&wav).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
            let mut child = c.spawn().map_err(|e| anyhow::anyhow!("ffmpeg not found ({e})"))?;
            let err_task = spawn_stderr_drain(&mut child);
            let status = child.wait().await?;
            let err = err_task.await.unwrap_or_default();
            if !status.success() { anyhow::bail!("audio extract failed: {}", truncate_msg(&err)); }
            let _ = tulipix_tools::queue::set_progress(pool, id, base + span * 0.4, None).await;
            // 2. whisper-cli → SRT.
            let model = bundled_bin_dir().join("ggml-tiny-1.0.bin");
            let whisper = tulipix_core::thumbs::tool_bin("whisper-cli");
            let out_prefix = output.strip_suffix(".srt").unwrap_or(&output).to_string();
            let args = tulipix_tools::transcribe::args(&model.to_string_lossy(), &wav.to_string_lossy(), &out_prefix, 4);
            let mut c = quiet_cmd(&whisper);
            c.args(&args).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped());
            let mut child = c.spawn().map_err(|e| anyhow::anyhow!("whisper-cli not found ({e}) — set Tools directory in Settings"))?;
            let err_task = spawn_stderr_drain(&mut child);
            let status = child.wait().await?;
            let err = err_task.await.unwrap_or_default();
            let _ = tokio::fs::remove_file(&wav).await;
            if !status.success() { anyhow::bail!("whisper failed: {}", truncate_msg(&err)); }
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
            let err_task = spawn_stderr_drain(&mut child);
            let status = child.wait().await?;
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
    }
    Ok(())
}

/// Build a `rel_path → sha256` map for a folder (recursive), for folder-diff.
async fn folder_hash_map(root: &str) -> std::collections::BTreeMap<String, String> {
    let root = root.to_string();
    tokio::task::spawn_blocking(move || {
        let mut m = std::collections::BTreeMap::new();
        let base = std::path::Path::new(&root);
        for entry in walkdir::WalkDir::new(base).into_iter().filter_map(|e| e.ok()) {
            if !entry.file_type().is_file() { continue; }
            let p = entry.path();
            let rel = p.strip_prefix(base).unwrap_or(p).to_string_lossy().to_string();
            if let Ok(bytes) = std::fs::read(p) {
                m.insert(rel, tulipix_tools::hash::sha256_hex(&bytes));
            }
        }
        m
    }).await.unwrap_or_default()
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
    let _ = weak.upgrade_in_event_loop(move |w| {
        // Header: when a tool is open, show it + its newest job's live progress;
        // otherwise the queue summary.
        let active = w.get_tools_active_op().to_string();
        let header = if !active.is_empty() {
            let label = w.get_tools_active_label().to_string();
            match rows.iter().find(|(_, k, ..)| *k == active) {
                Some((_, _, state, progress, _)) if state == "running" =>
                    format!("{label} · {}%", (progress * 100.0).round() as i32),
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
            mk_field("crf", "Quality — CRF (lower = better, bigger)", "slider", "23", false, "18 great · 28 small", &[], 18.0, 35.0),
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
            file("input", "Source image"),
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
        "download" | "download_live" => vec![
            mk_field("url", "URL", "text", "", true, "YouTube / Vimeo / 1800+ sites", &[], 0.0, 0.0),
            mk_field("audio_only", "Audio only", "toggle", "false", false, "extract audio", &[], 0.0, 0.0),
            mk_field("max_height", "Max height (px, blank = best)", "number", "", false, "e.g. 1080", &[], 0.0, 0.0),
            mk_field("embed_subs", "Embed subtitles", "toggle", "false", false, "", &[], 0.0, 0.0),
            mk_field("output", "Save folder (blank = Downloads)", "folder", "", false, "", &[], 0.0, 0.0),
        ],
        "transcribe" => vec![
            file("input", "Audio/video file"),
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
            mk_field("pattern", "Pattern", "text", "{n:03}_{name}", true, "{n}, {n:03} = sequence", &[], 0.0, 0.0),
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

/// `dir/stem.suffix.ext` next to `input`.
fn beside_source(input: &str, suffix: &str, ext: &str) -> String {
    let p = std::path::Path::new(input);
    let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let parent = p.parent().map(|x| x.to_path_buf()).unwrap_or_default();
    parent.join(format!("{stem}.{suffix}.{ext}")).to_string_lossy().to_string()
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
            "extract" => set_out(&mut o, beside_source(&input, "track", "m4a")),
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
    // download: blank "output" folder → into the user's Downloads.
    if kind == "download" || kind == "download_live" {
        let folder = o.get("output").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
        let base = if folder.is_empty() {
            dirs_default().map(|d| d.join("Downloads")).unwrap_or_else(|| std::path::PathBuf::from("."))
        } else { std::path::PathBuf::from(&folder) };
        let tmpl = base.join("%(title)s.%(ext)s").to_string_lossy().to_string();
        o.insert("out_template".into(), serde_json::Value::String(tmpl));
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
    else if bundled_present(name) || bundled_present(&format!("{name}{ext}")) { "Bundled" }
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
    // Bundled AI model (not a binary).
    let have_model = bundled_present("ggml-tiny-1.0.bin");
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
        for f in tool_fields(&kind) {
            if f.required && map.get(f.key.as_str()).map(|v| v.trim().is_empty()).unwrap_or(true) {
                w0.set_tools_error(format!("{} is required", f.label).into());
                return;
            }
        }
        w0.set_tools_error("".into());
        let spec = tools_build_spec(&kind, &map).to_string();
        let weak = w0.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
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
                let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
                let _ = tokio::process::Command::new(&bin).arg("-U").output().await;
            }
            tools_refresh_status(weak).await;
        });
    });
    // Download tool: list finished downloads + open one in the OS default app.
    let w = window.as_weak();
    window.on_tools_list_downloads(move || {
        let Some(w0) = w.upgrade() else { return; };
        let rows: Vec<DownloadItem> = list_downloaded().into_iter()
            .map(|(name, path, meta)| DownloadItem { name: name.into(), path: path.into(), meta: meta.into() })
            .collect();
        w0.set_tools_downloads(slint::ModelRc::new(slint::VecModel::from(rows)));
    });
    window.on_tools_open_download(move |path| {
        let p = std::path::PathBuf::from(path.to_string());
        std::thread::spawn(move || { let _ = tulipix_platform::fm::open_default(&p); });
    });
    // Queue row actions + worker-slot slider.
    let w = window.as_weak();
    window.on_queue_action(move |id, act| {
        let Some(w0) = w.upgrade() else { return; };
        let weak = w0.as_weak();
        let (id, act) = (id as i64, act.to_string());
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
                let _ = match act.as_str() {
                    "pause"  => tulipix_tools::queue::pause(&pool, id).await,
                    "resume" => tulipix_tools::queue::resume(&pool, id).await,
                    "cancel" => tulipix_tools::queue::cancel(&pool, id).await,
                    "retry"  => tulipix_tools::queue::retry(&pool, id).await,
                    "remove" => tulipix_tools::queue::remove(&pool, id).await,
                    "clear"  => tulipix_tools::queue::clear_all(&pool).await,
                    "up"     => tulipix_tools::queue::reorder(&pool, id, true).await,
                    "down"   => tulipix_tools::queue::reorder(&pool, id, false).await,
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
    tools_refresh(window);
    // Load persisted worker-slot count + start the queue drainer.
    {
        let weak = window.as_weak();
        tokio::runtime::Handle::current().spawn(async move {
            if let Ok(pool) = pool_for("tools").await {
                let n = tulipix_tools::queue::worker_slots(&pool).await.unwrap_or(2);
                let _ = weak.upgrade_in_event_loop(move |w| w.set_tools_worker_slots(n as i32));
            }
        });
        tools_start_worker();
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

