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

/// One queued download: everything needed to fetch it without touching the
/// shared state again, since by the time it runs the user has moved on.
#[derive(Debug, Clone)]
struct Job {
    url: String,
    name: String,
    /// Shown while it waits its turn.
    label: String,
}

static QUEUE: std::sync::OnceLock<Mutex<std::collections::VecDeque<Job>>> =
    std::sync::OnceLock::new();
fn queue() -> &'static Mutex<std::collections::VecDeque<Job>> {
    QUEUE.get_or_init(|| Mutex::new(std::collections::VecDeque::new()))
}

/// Queue the chosen stream. One download runs at a time; the rest wait.
pub fn stream_download(weak: slint::Weak<MainWindow>, index: i32) {
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    let (title, (season, episode)) =
        with_state(|st| (st.details.as_ref().map(|d| d.title.clone()).unwrap_or_default(), st.selection));
    let name = download_name(&title, season, episode, file.resolution);
    enqueue(
        weak,
        Job { url: file.url, label: format!("Queued {name}"), name },
    );
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
            enqueue(
                weak.clone(),
                Job { url: file.url.clone(), label: format!("Queued {name}"), name },
            );
            queued += 1;
        }
        set_status(&weak, format!("Queued {queued} episodes."), false);
    });
}

/// Add a job and start the runner if it is idle.
fn enqueue(weak: slint::Weak<MainWindow>, job: Job) {
    use std::sync::atomic::Ordering;
    let waiting = {
        let Ok(mut q) = queue().lock() else { return };
        q.push_back(job);
        q.len()
    };
    if DL_BUSY.swap(true, Ordering::SeqCst) {
        // A runner is already draining the queue; it will pick this up.
        set_dl(&weak, format!("{waiting} in queue"), -1.0, true);
        return;
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
    loop {
        let next = queue().lock().ok().and_then(|mut q| q.pop_front());
        let Some(job) = next else { break };
        let left = queue().lock().map(|q| q.len()).unwrap_or(0);

        let dest = download_dir().join(&job.name);
        let part = dest.with_extension("mp4.part");
        let suffix = if left > 0 { format!(" · {left} waiting") } else { String::new() };
        set_dl(&weak, format!("{}{suffix}", job.label), 0.0, true);

        let report = {
            let weak = weak.clone();
            let suffix = suffix.clone();
            move |label: String, frac: f32, active: bool| {
                // Cloned per call: `download_to` reports many times, so the
                // closure has to stay `Fn` rather than consuming the suffix.
                let suffix = suffix.clone();
                let _ = weak.upgrade_in_event_loop(move |w| {
                    w.set_video_stream_dl_label(format!("{label}{suffix}").into());
                    w.set_video_stream_dl_frac(frac);
                    w.set_video_stream_dl_active(active);
                });
            }
        };

        match download_to(&job.url, &part, &report).await {
            Ok(true) => {
                if tokio::fs::rename(&part, &dest).await.is_ok() {
                    set_dl(&weak, format!("Saved to {}", dest.display()), 1.0, true);
                    rescan_library(&dest);
                } else {
                    set_dl(&weak, "Downloaded, but could not be renamed.".into(), 1.0, true);
                }
            }
            Ok(false) => {
                let _ = tokio::fs::remove_file(&part).await;
                // Cancel means the whole queue, not just this one.
                if let Ok(mut q) = queue().lock() {
                    q.clear();
                }
                set_dl(&weak, "Download cancelled.".into(), 0.0, false);
                break;
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                set_dl(&weak, format!("Download failed: {e}"), 0.0, true);
            }
        }
    }
    DL_BUSY.store(false, Ordering::SeqCst);

    // Auto-hide the row a few seconds after the queue empties — unless another
    // download started in the meantime.
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    if !DL_BUSY.load(Ordering::SeqCst) {
        let _ = weak.upgrade_in_event_loop(|w| {
            w.set_video_stream_dl_label("".into());
            w.set_video_stream_dl_active(false);
            w.set_video_stream_dl_frac(0.0);
        });
    }
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
    report: &impl Fn(String, f32, bool),
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
            report(progress_label(done, total), frac.clamp(0.0, 1.0), true);
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
