//! Downloading a stream to disk, with progress reporting and cancellation.

use super::*;

/// Set while a download runs; flipping it true asks the loop to stop.
static DL_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Guards against two downloads running at once.
static DL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Where finished downloads land.
///
/// Default is the first configured video library, so a downloaded episode joins
/// the local grid with a thumbnail and its own resume instead of disappearing
/// into a folder the app never looks at. Setting `stream.dl_dest` to
/// `downloads` restores the old behaviour.
fn download_dir() -> PathBuf {
    if library_dest() {
        if let Some(lib) = first_video_library() {
            return lib.join("Stream");
        }
    }
    if let Ok(d) = std::env::var("XDG_DOWNLOAD_DIR") {
        if !d.trim().is_empty() {
            return PathBuf::from(d);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let d = PathBuf::from(home).join("Downloads");
        if d.is_dir() {
            return d;
        }
    }
    PathBuf::from(".")
}

/// Is the library the download target? Anything but an explicit `downloads`
/// means yes, so the useful behaviour is what a fresh install gets.
fn library_dest() -> bool {
    tulipix_core::settings::Settings::load().unwrap_or_default().text("stream.dl_dest").trim()
        != "downloads"
}

/// First configured video library folder, if there is one.
fn first_video_library() -> Option<PathBuf> {
    use tulipix_core::libraries::Section;
    let s = tulipix_core::settings::Settings::load().ok()?;
    s.libraries
        .libraries
        .iter()
        .find(|l| l.section == Section::Videos)
        .map(|l| l.path.clone())
        .filter(|p| p.is_dir())
}

/// Strip anything that cannot go in a filename, and collapse runs of spaces.
fn safe_filename(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '.' { c } else { ' ' })
        .collect();
    let joined = cleaned.split_whitespace().collect::<Vec<_>>().join("_");
    if joined.is_empty() { "stream".to_string() } else { joined }
}

/// `"Person_of_Interest_S01E05_1080p.mp4"` — enough to identify the file later.
fn download_name(title: &str, season: i64, episode: i64, resolution: i64) -> String {
    let mut name = safe_filename(title);
    if season > 0 || episode > 0 {
        name.push_str(&format!("_S{season:02}E{episode:02}"));
    }
    if resolution > 0 {
        name.push_str(&format!("_{resolution}p"));
    }
    name.push_str(".mp4");
    name
}

/// Queue the chosen stream. One download runs at a time; the rest wait in the
/// ledger, which survives a restart.
pub fn stream_download(weak: slint::Weak<MainWindow>, index: i32) {
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    let (subject, title, (season, episode)) = with_state(|st| {
        (
            st.open_id.clone(),
            st.details.as_ref().map(|d| d.title.clone()).unwrap_or_default(),
            st.selection,
        )
    });
    let name = download_name(&title, season, episode, file.resolution);
    let dest = download_dir().join(&name).display().to_string();
    // The language picked in the detail pane travels with the file.
    let caption = with_state(|st| st.sub_choice.and_then(|i| st.subs.get(i).cloned()));
    queue_subtitle(&dest, caption);
    let job = stream::downloads::Job {
        subject_id: subject,
        title: if title.is_empty() { name.clone() } else { title },
        season,
        episode,
        resolution: file.resolution,
        url: file.url,
        dest,
        ..Default::default()
    };
    enqueue(weak, job);
}

/// Fetch a caption and drop it next to the video that is being queued, so
/// playing the download later comes with the subtitles that were on screen when
/// it was asked for. Failure is silent: a missing subtitle must never stop a
/// download from being queued.
fn queue_subtitle(dest: &str, caption: Option<Caption>) {
    let Some(c) = caption else { return };
    let dest = PathBuf::from(dest);
    tokio::runtime::Handle::current().spawn(async move {
        let _ = save_sidecar_sub(&dest, &c).await;
    });
}

/// This episode's track in the language the user picked. A season download
/// resolves each episode on its own, so the tracks differ episode to episode
/// even though the chosen language does not.
fn caption_in(file: &stream::StreamFile, lang: Option<&str>) -> Option<Caption> {
    let want = lang?.trim();
    if want.is_empty() {
        return None;
    }
    file.captions.iter().find(|c| c.lang.eq_ignore_ascii_case(want)).cloned()
}

/// Queue every episode of the open season at the armed quality.
///
/// Each episode has to be resolved before it can be fetched, and resolving all
/// of them up front would stall on the slowest one, so this walks the season in
/// the background and appends as it goes.
pub fn stream_download_season(weak: slint::Weak<MainWindow>) {
    let Some(subject_id) = opened_subject() else { return };
    let (season, details) = with_state(|st| (st.selection.0, st.details.clone()));
    let Some(d) = details else { return };
    if !d.is_series {
        return stream_download_current(weak);
    }
    let max_ep = d
        .seasons
        .iter()
        .find(|s| s.number == season)
        .or_else(|| d.seasons.first())
        .map(|s| s.max_ep)
        .unwrap_or(0);
    if max_ep <= 0 {
        return;
    }
    let title = d.title.clone();
    let res = current_resolution();
    let sub_lang = with_state(|st| st.sub_lang.clone());
    set_status(&weak, format!("Queueing {max_ep} episodes…"), true);

    tokio::runtime::Handle::current().spawn(async move {
        let Ok(c) = client().await else { return };
        let sticky = stream::quality::sticky();
        let mut queued = 0;
        for ep in 1..=max_ep {
            let files = c
                .resources(&subject_id, season.max(0) as usize, ep.max(0) as usize, &res)
                .await
                .unwrap_or_default();
            let Some(pick) = stream::quality::pick(&files, sticky) else { continue };
            let Some(file) = files.get(pick) else { continue };
            let name = download_name(&title, season, ep, file.resolution);
            let dest = download_dir().join(&name).display().to_string();
            queue_subtitle(&dest, caption_in(file, sub_lang.as_deref()));
            enqueue(
                weak.clone(),
                stream::downloads::Job {
                    subject_id: subject_id.clone(),
                    title: title.clone(),
                    season,
                    episode: ep,
                    resolution: file.resolution,
                    url: file.url.clone(),
                    dest,
                    ..Default::default()
                },
            );
            queued += 1;
        }
        set_status(&weak, format!("Queued {queued} episodes."), false);
    });
}

/// Record a job and start the runner if it is idle.
fn enqueue(weak: slint::Weak<MainWindow>, job: stream::downloads::Job) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        if stream::downloads::enqueue(&pool, &job).await.is_err() {
            return;
        }
        stream_downloads_load(weak.clone());
        start_runner(weak);
    });
}

/// Start draining the queue unless something already is.
fn start_runner(weak: slint::Weak<MainWindow>) {
    use std::sync::atomic::Ordering;
    if DL_BUSY.swap(true, Ordering::SeqCst) {
        return; // a runner is already going; it will pick the new job up
    }
    DL_CANCEL.store(false, Ordering::SeqCst);
    tokio::runtime::Handle::current().spawn(run_queue(weak));
}

/// Drain the queue one job at a time until it is empty or cancelled.
///
/// Writes to a `.part` file and renames on success, so an interrupted download
/// never leaves something that looks complete.
async fn run_queue(weak: slint::Weak<MainWindow>) {
    use std::sync::atomic::Ordering;
    use stream::downloads::State;

    let Ok(pool) = pool_for("videos").await else {
        DL_BUSY.store(false, Ordering::SeqCst);
        return;
    };
    while let Some(job) = stream::downloads::next_queued(&pool).await {
        if DL_CANCEL.swap(false, Ordering::SeqCst) {
            let _ = stream::downloads::cancel_waiting(&pool).await;
            break;
        }
        let _ = stream::downloads::set_state(&pool, job.id, State::Running, "").await;
        stream_downloads_load(weak.clone());

        let dest = PathBuf::from(&job.dest);
        let part = dest.with_extension("mp4.part");
        let left = stream::downloads::active_count(&pool).await.saturating_sub(1);
        let label = job_label(&job);
        set_dl(&weak, format!("{label}{}", waiting_suffix(left)), 0.0, true);

        // Progress goes to the ledger as well as the header strip, so the
        // Downloads page shows a live bar for the job in flight.
        let report = {
            let weak = weak.clone();
            let pool = pool.clone();
            let label = label.clone();
            let id = job.id;
            move |text: String, frac: f32, done: u64, total: u64| {
                let _ = weak.upgrade_in_event_loop({
                    let header = format!("{text}{}", waiting_suffix(left));
                    let detail = text;
                    move |w| {
                        w.set_video_stream_dl_label(header.into());
                        w.set_video_stream_dl_frac(frac);
                        w.set_video_stream_dl_active(true);
                        // The Downloads page has its own bar per job. Reloading
                        // the whole ledger on every tick would be wasteful, so
                        // the running row is patched in place.
                        patch_row(&w, id, frac, &detail);
                    }
                });
                let _ = label;
                let pool = pool.clone();
                tokio::runtime::Handle::current().spawn(async move {
                    let _ = stream::downloads::set_progress(&pool, id, done as i64, total as i64)
                        .await;
                });
            }
        };

        match download_to(&job.url, &part, &report).await {
            Ok(true) => {
                if tokio::fs::rename(&part, &dest).await.is_ok() {
                    let _ = stream::downloads::set_state(&pool, job.id, State::Done, "").await;
                    set_dl(&weak, format!("Saved {label}"), 1.0, true);
                    rescan_library(&dest);
                } else {
                    let _ = stream::downloads::set_state(
                        &pool,
                        job.id,
                        State::Failed,
                        "could not be moved into place",
                    )
                    .await;
                    set_dl(&weak, "Downloaded, but could not be renamed.".into(), 1.0, true);
                }
            }
            Ok(false) => {
                let _ = tokio::fs::remove_file(&part).await;
                let _ = stream::downloads::set_state(&pool, job.id, State::Cancelled, "").await;
                let _ = stream::downloads::cancel_waiting(&pool).await;
                set_dl(&weak, "Download cancelled.".into(), 0.0, false);
                stream_downloads_load(weak.clone());
                break;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                let _ = stream::downloads::set_state(&pool, job.id, State::Failed, &e).await;
                set_dl(&weak, format!("Download failed: {e}"), 0.0, true);
            }
        }
        stream_downloads_load(weak.clone());
    }
    DL_BUSY.store(false, Ordering::SeqCst);

    // Auto-hide the strip a few seconds after the queue empties — unless
    // another download started in the meantime.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    if !DL_BUSY.load(Ordering::SeqCst) {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_dl_label("".into());
            w.set_video_stream_dl_active(false);
            w.set_video_stream_dl_frac(0.0);
        });
    }
}

/// Move one Downloads row's bar without rebuilding the list. Does nothing when
/// the page is not showing the job — the next full load will catch it up.
fn patch_row(w: &MainWindow, id: i64, frac: f32, detail: &str) {
    use slint::Model as _;
    let rows = w.get_video_stream_downloads();
    for i in 0..rows.row_count() {
        let Some(mut r) = rows.row_data(i) else { continue };
        if r.id == id as i32 {
            r.progress = frac;
            r.detail = detail.into();
            rows.set_row_data(i, r);
            return;
        }
    }
}

/// "Severance S01E04 · 720p" — one line identifying a job.
fn job_label(job: &stream::downloads::Job) -> String {
    let mut out = job.title.clone();
    if job.season > 0 || job.episode > 0 {
        out.push_str(&format!("  ·  S{:02}E{:02}", job.season.max(1), job.episode.max(1)));
    }
    if job.resolution > 0 {
        out.push_str(&format!("  ·  {}p", job.resolution));
    }
    out
}

fn waiting_suffix(left: i64) -> String {
    if left > 0 { format!("  ·  {left} waiting") } else { String::new() }
}

/// `frac < 0` leaves the bar where it is — used for queue-length notices that
/// should not disturb a download already in flight.
fn set_dl(weak: &slint::Weak<MainWindow>, label: String, frac: f32, active: bool) {
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_video_stream_dl_label(label.into());
        if frac >= 0.0 {
            w.set_video_stream_dl_frac(frac);
        }
        w.set_video_stream_dl_active(active);
    });
}

/// Rescan the video library a download just landed in, so the file shows up in
/// the local grid — with a thumbnail and its own resume — without the user
/// having to ask for a scan.
fn rescan_library(dest: &std::path::Path) {
    use tulipix_core::libraries::Section;
    if !library_dest() {
        return;
    }
    let dest = dest.to_path_buf();
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(settings) = tulipix_core::settings::Settings::load() else { return };
        // The library that contains it — the download folder is a subfolder of
        // one, so a prefix match is what identifies it.
        let Some(lib) = settings
            .libraries
            .libraries
            .iter()
            .find(|l| l.section == Section::Videos && dest.starts_with(&l.path))
        else {
            return;
        };
        if let Ok(pool) = pool_for("videos").await {
            let _ = tulipix_videos::scan::scan_library(&pool, lib).await;
        }
    });
}

/// Manually dismiss the download row (the X on it).
pub fn stream_download_dismiss(weak: slint::Weak<MainWindow>) {
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_video_stream_dl_label("".into());
        w.set_video_stream_dl_active(false);
        w.set_video_stream_dl_frac(0.0);
    });
}

/// Info-box Download — downloads the currently selected stream.
pub fn stream_download_current(weak: slint::Weak<MainWindow>) {
    let i = current_index();
    if i >= 0 {
        stream_download(weak, i);
    }
}

/// Stream `url` into `part`. `Ok(false)` means the user cancelled.
async fn download_to(
    url: &str,
    part: &std::path::Path,
    report: &impl Fn(String, f32, u64, u64),
) -> Result<bool, String> {
    use std::sync::atomic::Ordering;
    use tokio::io::AsyncWriteExt;

    if let Some(parent) = part.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let mut resp = reqwest::Client::new()
        .get(url)
        .send()
        .await
        .map_err(|e| e.to_string())?
        .error_for_status()
        .map_err(|e| e.to_string())?;

    let total = resp.content_length().unwrap_or(0);
    let mut out = tokio::fs::File::create(part).await.map_err(|e| e.to_string())?;
    let mut done: u64 = 0;
    let mut last_tick = std::time::Instant::now();

    while let Some(chunk) = resp.chunk().await.map_err(|e| e.to_string())? {
        if DL_CANCEL.load(Ordering::SeqCst) {
            return Ok(false);
        }
        out.write_all(&chunk).await.map_err(|e| e.to_string())?;
        done += chunk.len() as u64;

        // Repainting on every chunk would flood the event loop.
        if last_tick.elapsed() >= std::time::Duration::from_millis(200) {
            last_tick = std::time::Instant::now();
            let frac = if total > 0 { done as f32 / total as f32 } else { 0.0 };
            report(progress_label(done, total), frac.clamp(0.0, 1.0), done, total);
        }
    }
    out.flush().await.map_err(|e| e.to_string())?;
    Ok(true)
}

fn progress_label(done: u64, total: u64) -> String {
    let mb = |b: u64| b as f64 / 1024.0 / 1024.0;
    if total > 0 {
        format!("{:.0} MB of {:.0} MB ({:.0}%)", mb(done), mb(total), done as f64 / total as f64 * 100.0)
    } else {
        format!("{:.0} MB", mb(done))
    }
}

pub fn stream_download_cancel(weak: slint::Weak<MainWindow>) {
    DL_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    set_status(&weak, "Cancelling…", false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_names_are_filesystem_safe_and_identify_the_episode() {
        assert_eq!(
            download_name("Person of Interest", 1, 5, 1080),
            "Person_of_Interest_S01E05_1080p.mp4"
        );
        assert_eq!(download_name("Dune: Part Two", 0, 0, 720), "Dune_Part_Two_720p.mp4");
        // path separators and quotes must never survive into a filename
        assert_eq!(download_name("../../etc/passwd", 0, 0, 0), "etc_passwd.mp4");
        assert_eq!(download_name("", 0, 0, 0), "stream.mp4");
        assert!(!download_name("a/b\\c:d", 2, 10, 480).contains(['/', '\\', ':']));
    }

    #[test]
    fn progress_label_handles_unknown_length() {
        assert_eq!(progress_label(5 * 1024 * 1024, 10 * 1024 * 1024), "5 MB of 10 MB (50%)");
        assert_eq!(progress_label(3 * 1024 * 1024, 0), "3 MB");
    }
}

// ---- Downloads page ----

/// How many rows the Downloads page lists.
const LEDGER_LIMIT: i64 = 200;

/// Fill the Downloads page and the tab badge.
pub fn stream_downloads_load(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let jobs = stream::downloads::list(&pool, LEDGER_LIMIT).await;
        let active = stream::downloads::active_count(&pool).await as i32;
        // `file_present` touches the filesystem, so it is resolved here rather
        // than inside the event-loop closure.
        let rows: Vec<(stream::downloads::Job, bool)> =
            jobs.into_iter().map(|j| { let present = j.file_present(); (j, present) }).collect();

        let _ = weak.upgrade_in_event_loop(move |w| {
            let list: Vec<StreamDownloadRow> = rows
                .into_iter()
                .map(|(j, present)| StreamDownloadRow {
                    id: j.id as i32,
                    title: job_label(&j).into(),
                    // A finished file that has since been deleted elsewhere is
                    // reported as missing rather than offered for playing.
                    state: state_label(&j, present).into(),
                    detail: state_detail(&j, present).into(),
                    dest: j.dest.clone().into(),
                    progress: j.fraction() as f32,
                    active: j.state().active(),
                    done: j.state() == stream::downloads::State::Done && present,
                    failed: matches!(
                        j.state(),
                        stream::downloads::State::Failed | stream::downloads::State::Cancelled
                    ) || (j.state() == stream::downloads::State::Done && !present),
                })
                .collect();
            w.set_video_stream_downloads(slint::ModelRc::new(slint::VecModel::from(list)));
            w.set_video_stream_dl_pending(active);
        });
    });
}

/// "Downloading" / "Waiting" / "Saved" / "Missing" / "Failed".
fn state_label(job: &stream::downloads::Job, present: bool) -> String {
    use stream::downloads::State;
    match job.state() {
        State::Running => "Downloading".into(),
        State::Queued => "Waiting".into(),
        State::Cancelled => "Cancelled".into(),
        State::Failed => "Failed".into(),
        State::Done if present => "Saved".into(),
        State::Done => "Missing".into(),
    }
}

/// The second line: size progress, the error, or where the file went.
fn state_detail(job: &stream::downloads::Job, present: bool) -> String {
    use stream::downloads::State;
    match job.state() {
        State::Running => progress_label(job.done_bytes.max(0) as u64, job.total_bytes.max(0) as u64),
        State::Queued => "in the queue".into(),
        State::Failed | State::Cancelled if !job.error.is_empty() => job.error.clone(),
        State::Failed => "download did not finish".into(),
        State::Cancelled => "stopped before it finished".into(),
        State::Done if present => job.dest.clone(),
        State::Done => format!("no longer at {}", job.dest),
    }
}

/// Play a finished download in the same external mpv the streams use.
pub fn stream_download_play(weak: slint::Weak<MainWindow>, id: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let Some(job) = stream::downloads::get(&pool, id as i64).await else { return };
        if !job.file_present() {
            return set_status(&weak, "That file is no longer on disk.", false);
        }
        // The subtitle saved beside it when it was queued goes back on.
        let dest = PathBuf::from(&job.dest);
        let args = sidecar_sub_args(&dest);
        let subbed = !args.is_empty();
        spawn_mpv_windowed_tracked(dest, None, None, args, None);
        let label = job_label(&job);
        let msg = if subbed {
            format!("Playing {label} · with subtitles")
        } else {
            format!("Playing {label}")
        };
        set_status(&weak, msg, false);
    });
}

/// Retry a failed job by putting it back in the queue.
pub fn stream_download_retry(weak: slint::Weak<MainWindow>, id: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let _ =
            stream::downloads::set_state(&pool, id as i64, stream::downloads::State::Queued, "")
                .await;
        stream_downloads_load(weak.clone());
        start_runner(weak);
    });
}

/// Drop one row. A finished download's file is left alone — this is the
/// ledger's row, not the video.
pub fn stream_download_forget(weak: slint::Weak<MainWindow>, id: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        // Removing the job that is running also stops it.
        if let Some(job) = stream::downloads::get(&pool, id as i64).await {
            if job.state() == stream::downloads::State::Running {
                DL_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
            }
        }
        let _ = stream::downloads::remove(&pool, id as i64).await;
        stream_downloads_load(weak);
    });
}

/// Delete the downloaded file as well as the row.
pub fn stream_download_delete_file(weak: slint::Weak<MainWindow>, id: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        if let Some(job) = stream::downloads::get(&pool, id as i64).await {
            let dest = PathBuf::from(&job.dest);
            let _ = tokio::fs::remove_file(&dest).await;
            // The subtitle saved with it is part of the download, not a file the
            // user put there — it goes too.
            for ext in ["srt", "vtt", "ass", "ssa", "sub"] {
                let _ = tokio::fs::remove_file(dest.with_extension(ext)).await;
            }
        }
        let _ = stream::downloads::remove(&pool, id as i64).await;
        stream_downloads_load(weak);
    });
}

/// Clear every finished, failed and cancelled row. Files stay where they are.
pub fn stream_downloads_clear(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let _ = stream::downloads::clear_finished(&pool).await;
        stream_downloads_load(weak);
    });
}

/// Stop the job in flight and drop everything still waiting.
pub fn stream_downloads_cancel_all(weak: slint::Weak<MainWindow>) {
    DL_CANCEL.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = pool_for("videos").await {
            let _ = stream::downloads::cancel_waiting(&pool).await;
        }
        stream_downloads_load(weak);
    });
}

/// Show the finished file in the desktop file manager.
pub fn stream_download_reveal(weak: slint::Weak<MainWindow>, id: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let Some(job) = stream::downloads::get(&pool, id as i64).await else { return };
        let dest = PathBuf::from(&job.dest);
        let dir = dest.parent().map(|p| p.to_path_buf()).unwrap_or_else(|| dest.clone());
        match reveal_in_file_manager(&dest, &dir) {
            Ok(()) => set_status(&weak, format!("Opened {}", dir.display()), false),
            Err(e) => set_status(&weak, format!("Could not open the folder: {e}"), false),
        }
    });
}

/// Show `file` in the desktop's file manager, with its window in front.
///
/// `xdg-open` on the folder hands the desktop a plain "open this" with no
/// activation token, so the file manager comes up behind the app that asked for
/// it. The `org.freedesktop.FileManager1` interface is the one meant for this:
/// it selects the file and raises the window. It is asked first, and the plain
/// launcher is the fallback for desktops that do not export it.
#[cfg(target_os = "linux")]
fn reveal_in_file_manager(file: &std::path::Path, dir: &std::path::Path) -> Result<(), String> {
    let uri = format!("file://{}", file.display());
    let shown = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.FileManager1",
            "--object-path",
            "/org/freedesktop/FileManager1",
            "--method",
            "org.freedesktop.FileManager1.ShowItems",
            &format!("['{uri}']"),
            "",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if shown {
        return Ok(());
    }
    std::process::Command::new("xdg-open").arg(dir).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[cfg(not(target_os = "linux"))]
fn reveal_in_file_manager(file: &std::path::Path, dir: &std::path::Path) -> Result<(), String> {
    // Both of these select the file and bring their window forward already.
    #[cfg(target_os = "macos")]
    let cmd = std::process::Command::new("open").arg("-R").arg(file).spawn();
    #[cfg(target_os = "windows")]
    let cmd = std::process::Command::new("explorer").arg(format!("/select,{}", file.display())).spawn();
    let _ = dir;
    cmd.map(|_| ()).map_err(|e| e.to_string())
}

/// Anything left `running` when the app closed is not running now; put it back
/// in the queue and pick it up. Called when the tab opens.
pub fn stream_downloads_resume(weak: slint::Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = pool_for("videos").await else { return };
        let _ = stream::downloads::reset_orphans(&pool).await;
        if stream::downloads::active_count(&pool).await > 0 {
            start_runner(weak.clone());
        }
        stream_downloads_load(weak);
    });
}
