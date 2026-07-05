//! Downloader tab glue — marshals `tulipix_mdl` (resolve + download) to the
//! Slint UI. Business logic stays in `tulipix-mdl`; this module only spawns the
//! work, pushes progress into the UI models, and keeps the library live.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tulipix_mdl::types::{DownloadOptions, Progress, Stage};
use tulipix_ui::*;

/// Cancels the in-flight download when set.
static CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn cancel_flag() -> &'static Arc<AtomicBool> {
    CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// Rust-side mirror of the on-screen worker rows, rebuilt into a Slint model on
/// every progress event.
type RowData = (String, String, f32, String); // title, stage, percent, file
static ROWS: OnceLock<Mutex<Vec<RowData>>> = OnceLock::new();
fn rows() -> &'static Mutex<Vec<RowData>> {
    ROWS.get_or_init(|| Mutex::new(Vec::new()))
}

/// System Music dir — default download destination.
pub fn default_music_dir() -> PathBuf {
    tulipix_common::dirs_default_music()
}

/// Provider display name for a URL, or "" if unsupported. Drives the badge.
pub fn detect(url: &str) -> String {
    tulipix_mdl::detect_provider(url)
        .map(|p| p.display_name().to_string())
        .unwrap_or_default()
}

fn stage_label(stage: Stage) -> &'static str {
    match stage {
        Stage::Initializing => "queued",
        Stage::SearchingYoutube => "searching",
        Stage::DownloadingAudio => "downloading",
        Stage::WritingMetadata => "tagging",
        Stage::WritingManifest => "saving",
        Stage::Skipped => "skipped",
        Stage::Failed => "failed",
        Stage::Completed => "done",
    }
}

fn push_rows(weak: &Weak<MainWindow>) {
    let snapshot = rows().lock().unwrap().clone();
    let _ = weak.upgrade_in_event_loop(move |w| {
        let model: Vec<DownloaderRow> = snapshot
            .iter()
            .map(|(title, stage, percent, file)| DownloaderRow {
                title: SharedString::from(title.as_str()),
                stage: SharedString::from(stage.as_str()),
                percent: *percent,
                file: SharedString::from(file.as_str()),
            })
            .collect();
        w.set_music_dl_rows(ModelRc::new(VecModel::from(model)));
    });
}

fn set_status(weak: &Weak<MainWindow>, status: &str) {
    let status = status.to_string();
    let _ = weak.upgrade_in_event_loop(move |w| w.set_music_dl_status(SharedString::from(status)));
}

/// Resolve a URL and show the tracklist preview (no download yet).
pub fn start_resolve(weak: Weak<MainWindow>, url: String) {
    set_status(&weak, "resolving");
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        match tulipix_mdl::resolve_url(&client, &url).await {
            Ok(pl) => {
                let mut r = rows().lock().unwrap();
                *r = pl
                    .tracks
                    .iter()
                    .map(|t| {
                        (
                            format!("{} — {}", t.artists.join(", "), t.title),
                            "queued".to_string(),
                            0.0f32,
                            String::new(),
                        )
                    })
                    .collect();
                drop(r);
                push_rows(&weak);
                set_status(&weak, "resolved");
            }
            Err(e) => {
                set_status(&weak, &format!("error: {e}"));
            }
        }
    });
}

/// Cancel the current download.
pub fn cancel() {
    cancel_flag().store(true, Ordering::Relaxed);
}

/// Resolve (if needed) then download the whole playlist into `dest`, updating
/// the UI live and keeping both the default Music dir and `dest` in the
/// watched-folders set so tracks show up in the library.
pub fn start_download(weak: Weak<MainWindow>, url: String, dest: PathBuf) {
    cancel_flag().store(false, Ordering::Relaxed);
    // Both destinations become permanent library roots.
    tulipix_common::add_watched_folder(&default_music_dir());
    tulipix_common::add_watched_folder(&dest);

    set_status(&weak, "resolving");
    let cancel = cancel_flag().clone();
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        let playlist = match tulipix_mdl::resolve_url(&client, &url).await {
            Ok(pl) => pl,
            Err(e) => {
                set_status(&weak, &format!("error: {e}"));
                return;
            }
        };

        // Seed one row per track.
        {
            let mut r = rows().lock().unwrap();
            *r = playlist
                .tracks
                .iter()
                .map(|t| {
                    (
                        format!("{} — {}", t.artists.join(", "), t.title),
                        "queued".to_string(),
                        0.0f32,
                        String::new(),
                    )
                })
                .collect();
        }
        push_rows(&weak);
        set_status(&weak, "downloading");

        let opts = DownloadOptions { dest_dir: dest.clone(), parallelism: 5 };
        let progress_weak = weak.clone();
        let on_progress = move |p: Progress| {
            {
                let mut r = rows().lock().unwrap();
                if let Some(row) = r.get_mut(p.track_index.saturating_sub(1)) {
                    row.1 = stage_label(p.stage).to_string();
                    row.2 = p.percent;
                    if let Some(f) = &p.file_name {
                        row.3 = f.clone();
                    }
                }
            }
            push_rows(&progress_weak);
            // Counts + a completed track -> refresh the library live.
            let (done, skipped, failed, completed) =
                (p.downloaded, p.skipped, p.failed, matches!(p.stage, Stage::Completed));
            let _ = progress_weak.upgrade_in_event_loop(move |w| {
                w.set_music_dl_done(done as i32);
                w.set_music_dl_skipped(skipped as i32);
                w.set_music_dl_failed(failed as i32);
                if completed {
                    w.invoke_music_dl_refresh_library();
                }
            });
        };

        let summary =
            tulipix_mdl::download::download_playlist(&client, &playlist, &opts, cancel, on_progress)
                .await;

        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_dl_done(summary.downloaded as i32);
            w.set_music_dl_skipped(summary.skipped as i32);
            w.set_music_dl_failed(summary.failed.len() as i32);
            w.invoke_music_dl_refresh_library();
        });
        set_status(&weak, "done");
    });
}
