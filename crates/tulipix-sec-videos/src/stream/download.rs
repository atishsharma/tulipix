//! Downloading a stream to disk, with progress reporting and cancellation.

use super::*;

/// Set while a download runs; flipping it true asks the loop to stop.
static DL_CANCEL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Guards against two downloads running at once.
static DL_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Where finished downloads land: `$XDG_DOWNLOAD_DIR`, else `~/Downloads`, else
/// the working directory.
fn download_dir() -> PathBuf {
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

/// Download the chosen stream to the Downloads folder, reporting progress.
///
/// Writes to a `.part` file and renames on success, so an interrupted download
/// never leaves something that looks complete.
pub fn stream_download(weak: slint::Weak<MainWindow>, index: i32) {
    use std::sync::atomic::Ordering;
    let Some(file) = with_state(|st| st.files.get(index.max(0) as usize).cloned()) else {
        set_status(&weak, "That stream is no longer available.", false);
        return;
    };
    if DL_BUSY.swap(true, Ordering::SeqCst) {
        set_status(&weak, "A download is already running.", false);
        return;
    }
    DL_CANCEL.store(false, Ordering::SeqCst);

    let (title, (season, episode)) =
        with_state(|st| (st.details.as_ref().map(|d| d.title.clone()).unwrap_or_default(), st.selection));
    let name = download_name(&title, season, episode, file.resolution);
    let dest = download_dir().join(&name);
    let part = dest.with_extension("mp4.part");

    let set_dl = {
        let weak = weak.clone();
        move |label: String, frac: f32, active: bool| {
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_video_stream_dl_label(label.into());
                w.set_video_stream_dl_frac(frac);
                w.set_video_stream_dl_active(active);
            });
        }
    };
    set_dl(format!("Starting {name}"), 0.0, true);

    let weak_hide = weak.clone();
    tokio::runtime::Handle::current().spawn(async move {
        let result = download_to(&file.url, &part, &set_dl).await;
        DL_BUSY.store(false, Ordering::SeqCst);

        match result {
            Ok(true) => {
                if tokio::fs::rename(&part, &dest).await.is_ok() {
                    set_dl(format!("Saved to {}", dest.display()), 1.0, false);
                } else {
                    set_dl("Downloaded, but could not be renamed.".into(), 1.0, false);
                }
            }
            Ok(false) => {
                let _ = tokio::fs::remove_file(&part).await;
                set_dl("Download cancelled.".into(), 0.0, false);
            }
            Err(e) => {
                let _ = tokio::fs::remove_file(&part).await;
                set_dl(format!("Download failed: {e}"), 0.0, false);
            }
        }
        // Auto-hide the row a few seconds after it finishes — but only if no new
        // download has started in the meantime (that would reset DL_BUSY true).
        tokio::time::sleep(std::time::Duration::from_secs(6)).await;
        if !DL_BUSY.load(Ordering::SeqCst) {
            let _ = weak_hide.upgrade_in_event_loop(|w| {
                w.set_video_stream_dl_label("".into());
                w.set_video_stream_dl_active(false);
                w.set_video_stream_dl_frac(0.0);
            });
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
