//! Downloader tab glue — marshals `tulipix_mdl` (resolve + download) to the
//! Slint UI. Business logic stays in `tulipix-mdl`; this module only spawns the
//! work, tracks per-row selection + main-artist choice, pushes progress into
//! the UI models, ingests finished tracks straight into the library, and keeps
//! Download/Search history.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tulipix_mdl::types::{DownloadOptions, NameMethod, Playlist, Progress, Stage, Track};
use tulipix_ui::*;

/// Cancels the in-flight download when set.
static CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();
fn cancel_flag() -> &'static Arc<AtomicBool> {
    CANCEL.get_or_init(|| Arc::new(AtomicBool::new(false)))
}

/// Rust-side mirror of the on-screen worker rows.
#[derive(Clone)]
struct RowData {
    title: String,
    album: String,
    artists: Vec<String>,
    main_artist: String,
    stage: String,
    percent: f32,
    file: String,
    selected: bool,
}
static ROWS: OnceLock<Mutex<Vec<RowData>>> = OnceLock::new();
fn rows() -> &'static Mutex<Vec<RowData>> {
    ROWS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Last resolved playlist, so Download reuses it (respecting selection) instead
/// of re-resolving.
static RESOLVED: OnceLock<Mutex<Option<Playlist>>> = OnceLock::new();
fn resolved() -> &'static Mutex<Option<Playlist>> {
    RESOLVED.get_or_init(|| Mutex::new(None))
}
/// The raw URL the cached playlist was resolved from (guards against a stale
/// cache when the user changes the URL and hits Download without re-resolving).
static RESOLVED_URL: OnceLock<Mutex<String>> = OnceLock::new();
fn resolved_url() -> &'static Mutex<String> {
    RESOLVED_URL.get_or_init(|| Mutex::new(String::new()))
}

/// Maps a download worker's (filtered) track index back to its full-list row.
static INDEX_MAP: OnceLock<Mutex<Vec<usize>>> = OnceLock::new();
fn index_map() -> &'static Mutex<Vec<usize>> {
    INDEX_MAP.get_or_init(|| Mutex::new(Vec::new()))
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
    // Deduped union of every credited artist across the queue — the bulk
    // main-artist dropdown model.
    let mut union: Vec<String> = Vec::new();
    for r in &snapshot {
        for a in &r.artists {
            if !union.iter().any(|u| u == a) {
                union.push(a.clone());
            }
        }
    }
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_dl_all_artists(ModelRc::new(VecModel::from(
            union.iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
        let model: Vec<DownloaderRow> = snapshot
            .iter()
            .map(|r| DownloaderRow {
                title: SharedString::from(r.title.as_str()),
                album: SharedString::from(r.album.as_str()),
                artists: ModelRc::new(VecModel::from(
                    r.artists.iter().map(SharedString::from).collect::<Vec<_>>(),
                )),
                main_artist: SharedString::from(r.main_artist.as_str()),
                stage: SharedString::from(r.stage.as_str()),
                percent: r.percent,
                file: SharedString::from(r.file.as_str()),
                selected: r.selected,
                thumb: slint::Image::default(),
            })
            .collect();
        w.set_music_dl_rows(ModelRc::new(VecModel::from(model)));
    });
}

fn set_status(weak: &Weak<MainWindow>, status: &str) {
    let status = status.to_string();
    let _ = weak.upgrade_in_event_loop(move |w| w.set_music_dl_status(SharedString::from(status)));
}

fn seed_rows(pl: &Playlist) {
    let mut r = rows().lock().unwrap();
    *r = pl
        .tracks
        .iter()
        .map(|t| RowData {
            title: t.title.clone(),
            album: t.album.clone().unwrap_or_default(),
            artists: t.artists.clone(),
            main_artist: t.artists.first().cloned().unwrap_or_default(),
            stage: "queued".to_string(),
            percent: 0.0,
            file: String::new(),
            selected: true,
        })
        .collect();
}

/// Toggle one row's checkbox.
pub fn toggle_row(weak: Weak<MainWindow>, index: i32) {
    if let Some(r) = rows().lock().unwrap().get_mut(index as usize) {
        r.selected = !r.selected;
    }
    push_rows(&weak);
}

/// Select or deselect every row.
pub fn select_all(weak: Weak<MainWindow>, all: bool) {
    for r in rows().lock().unwrap().iter_mut() {
        r.selected = all;
    }
    push_rows(&weak);
}

/// Set the main (primary) artist for one track row.
pub fn set_main_artist(weak: Weak<MainWindow>, index: i32, artist: String) {
    if let Some(r) = rows().lock().unwrap().get_mut(index as usize) {
        if r.artists.iter().any(|a| a == &artist) {
            r.main_artist = artist;
        }
    }
    push_rows(&weak);
}

/// Bulk-set the main artist across the queue: every row that lists `artist`
/// gets it as its primary. `scope` (album|playlist) is informational — both
/// apply across the current queue where the artist appears.
pub fn bulk_main_artist(weak: Weak<MainWindow>, _scope: String, artist: String) {
    for r in rows().lock().unwrap().iter_mut() {
        if r.artists.iter().any(|a| a == &artist) {
            r.main_artist = artist.clone();
        }
    }
    push_rows(&weak);
}

/// Reorder `track.artists` so `main` is first (if present).
fn apply_main(track: &mut Track, main: &str) {
    if main.is_empty() {
        return;
    }
    if let Some(pos) = track.artists.iter().position(|a| a == main) {
        if pos != 0 {
            let a = track.artists.remove(pos);
            track.artists.insert(0, a);
        }
    }
}

/// Rough resolve-kind tag for the search history: single track vs album vs
/// playlist (albums resolve as playlists, so sniff the URL).
fn resolve_kind(pl: &Playlist) -> &'static str {
    if pl.tracks.len() == 1 {
        "track"
    } else if pl.source_url.contains("/album") {
        "album"
    } else {
        "playlist"
    }
}

fn record_search_bg(pl: &Playlist, url: String) {
    let (kind, title, provider) = (
        resolve_kind(pl).to_string(),
        pl.title.clone(),
        pl.provider.display_name().to_string(),
    );
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = tulipix_common::pool_for("music").await {
            let _ = tulipix_music::dl_history::record_search(
                &pool, &url, &kind, Some(title.as_str()), Some(provider.as_str()),
            )
            .await;
        }
    });
}

/// Resolve a URL and show the tracklist preview (all rows selected).
pub fn start_resolve(weak: Weak<MainWindow>, url: String) {
    set_status(&weak, "resolving");
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        match tulipix_mdl::resolve_url(&client, &url).await {
            Ok(pl) => {
                seed_rows(&pl);
                record_search_bg(&pl, url.clone());
                *resolved().lock().unwrap() = Some(pl);
                *resolved_url().lock().unwrap() = url.clone();
                push_rows(&weak);
                set_status(&weak, "resolved");
            }
            Err(e) => set_status(&weak, &format!("error: {e}")),
        }
    });
}

/// Cancel the current download.
pub fn cancel() {
    cancel_flag().store(true, Ordering::Relaxed);
}

/// Ingest a finished track directly into the library (no filename re-scan) and
/// log it to download history, then refresh the music views live.
fn ingest_completed(
    weak: Weak<MainWindow>,
    track: Track,
    abs_path: String,
    provider: String,
) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = tulipix_common::pool_for("music").await else { return; };
        let album = track.album.as_deref();
        if let Err(e) = tulipix_music::dl_history::ingest_downloaded_track(
            &pool, &abs_path, &track.title, &track.artists, album, track.duration_ms,
        )
        .await
        {
            tracing::warn!(error = %e, "mdl: direct ingest failed");
        }
        let _ = tulipix_music::dl_history::record_download(
            &pool,
            &track.title,
            &track.artists.join(", "),
            album,
            Some(provider.as_str()),
            &abs_path,
        )
        .await;
        let _ = weak.upgrade_in_event_loop(|w| crate::populate_music_views(w.as_weak()));
    });
}

/// Download the SELECTED tracks into `dest` using `format` + naming `method`,
/// `parallel` concurrent tracks and `threads` fragments per download. Reuses
/// the resolved playlist if present, else resolves `url` first.
#[allow(clippy::too_many_arguments)]
pub fn start_download(
    weak: Weak<MainWindow>,
    url: String,
    dest: PathBuf,
    format: String,
    name_method_label: String,
    parallel: i32,
    threads: i32,
) {
    cancel_flag().store(false, Ordering::Relaxed);
    tulipix_common::add_watched_folder(&default_music_dir());
    tulipix_common::add_watched_folder(&dest);
    let method = NameMethod::from_label(&name_method_label);
    let parallel = parallel.clamp(1, 4) as usize;
    let threads = threads.clamp(1, 8) as usize;

    set_status(&weak, "resolving");
    let cancel = cancel_flag().clone();
    tokio::runtime::Handle::current().spawn(async move {
        // Reuse the resolved playlist only if it matches the current URL;
        // otherwise resolve fresh (user changed the URL or skipped Resolve).
        let fresh = *resolved_url().lock().unwrap() == url;
        let base = if fresh { resolved().lock().unwrap().clone() } else { None };
        let playlist = match base {
            Some(pl) => pl,
            None => {
                let client = reqwest::Client::new();
                match tulipix_mdl::resolve_url(&client, &url).await {
                    Ok(pl) => {
                        seed_rows(&pl);
                        record_search_bg(&pl, url.clone());
                        *resolved().lock().unwrap() = Some(pl.clone());
                        *resolved_url().lock().unwrap() = url.clone();
                        push_rows(&weak);
                        pl
                    }
                    Err(e) => {
                        set_status(&weak, &format!("error: {e}"));
                        return;
                    }
                }
            }
        };

        // Keep only the selected rows; map filtered index -> full-list row, and
        // apply each row's chosen main artist (reorder so it is artists[0]).
        let (selected, mains): (Vec<usize>, Vec<String>) = {
            let r = rows().lock().unwrap();
            let mut idx = Vec::new();
            let mut mains = Vec::new();
            for i in 0..playlist.tracks.len() {
                let row = r.get(i);
                if row.map(|row| row.selected).unwrap_or(true) {
                    idx.push(i);
                    mains.push(row.map(|row| row.main_artist.clone()).unwrap_or_default());
                }
            }
            (idx, mains)
        };
        if selected.is_empty() {
            set_status(&weak, "nothing selected");
            return;
        }
        *index_map().lock().unwrap() = selected.clone();
        let tracks: Vec<Track> = selected
            .iter()
            .zip(mains.iter())
            .map(|(i, main)| {
                let mut t = playlist.tracks[*i].clone();
                apply_main(&mut t, main);
                t
            })
            .collect();
        let filtered = Playlist { tracks: tracks.clone(), ..playlist.clone() };
        // Metadata used by the completion ingest (indexed by filtered position).
        let ingest_tracks = tracks;
        let provider = playlist.provider.display_name().to_string();

        set_status(&weak, "downloading");
        let client = reqwest::Client::new();
        let opts = DownloadOptions {
            dest_dir: dest.clone(),
            parallelism: parallel,
            threads_per_download: threads,
            format,
            name_method: method,
        };
        let progress_weak = weak.clone();
        let dest_for_ingest = dest.clone();
        let on_progress = move |p: Progress| {
            // Map the filtered worker index back to its full-list row.
            let row_idx = index_map()
                .lock()
                .unwrap()
                .get(p.track_index.saturating_sub(1))
                .copied();
            if let Some(ri) = row_idx {
                let mut r = rows().lock().unwrap();
                if let Some(row) = r.get_mut(ri) {
                    row.stage = if matches!(p.stage, Stage::Failed) {
                        let msg: String = p.message.chars().take(60).collect();
                        format!("failed — {msg}")
                    } else {
                        stage_label(p.stage).to_string()
                    };
                    row.percent = p.percent;
                    if let Some(f) = &p.file_name {
                        row.file = f.clone();
                    }
                }
            }
            push_rows(&progress_weak);
            // On completion, ingest the finished file straight into the library.
            if matches!(p.stage, Stage::Completed) {
                if let (Some(file), Some(track)) = (
                    p.file_name.as_ref(),
                    ingest_tracks.get(p.track_index.saturating_sub(1)),
                ) {
                    let abs = dest_for_ingest.join(file).to_string_lossy().to_string();
                    ingest_completed(progress_weak.clone(), track.clone(), abs, provider.clone());
                }
            }
            let (done, skipped, failed) = (p.downloaded, p.skipped, p.failed);
            let _ = progress_weak.upgrade_in_event_loop(move |w| {
                w.set_music_dl_done(done as i32);
                w.set_music_dl_skipped(skipped as i32);
                w.set_music_dl_failed(failed as i32);
            });
        };

        let summary = tulipix_mdl::download::download_playlist(
            &client, &filtered, &opts, cancel, on_progress,
        )
        .await;

        let _ = weak.upgrade_in_event_loop(move |w| {
            w.set_music_dl_done(summary.downloaded as i32);
            w.set_music_dl_skipped(summary.skipped as i32);
            w.set_music_dl_failed(summary.failed.len() as i32);
        });
        set_status(&weak, "done");
    });
}

// ── History popups ─────────────────────────────────────────────────────────

fn fmt_when(ts: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(ts);
    let d = (now - ts).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", d / 60),
        3600..=86399 => format!("{}h ago", d / 3600),
        _ => format!("{}d ago", d / 86400),
    }
}

/// Fetch one page of download history and push it into the popup model.
pub fn load_history(weak: Weak<MainWindow>, page: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = tulipix_common::pool_for("music").await else { return; };
        let (rows, pages) = tulipix_music::dl_history::history_page(&pool, page as i64)
            .await
            .unwrap_or_default();
        let page = (page as i64).clamp(0, (pages - 1).max(0));
        let _ = weak.upgrade_in_event_loop(move |w| {
            let model: Vec<DlHistoryEntry> = rows
                .iter()
                .map(|r| DlHistoryEntry {
                    title: r.title.as_str().into(),
                    artists: r.artists.as_str().into(),
                    album: r.album.clone().unwrap_or_default().into(),
                    provider: r.provider.clone().unwrap_or_default().into(),
                    when: fmt_when(r.downloaded_at).into(),
                    path: r.abs_path.as_str().into(),
                })
                .collect();
            w.set_music_dl_history(ModelRc::new(VecModel::from(model)));
            w.set_music_dl_history_page(page as i32);
            w.set_music_dl_history_pages(pages as i32);
        });
    });
}

/// Fetch one page of search history and push it into the popup model.
pub fn load_searches(weak: Weak<MainWindow>, page: i32) {
    tokio::runtime::Handle::current().spawn(async move {
        let Ok(pool) = tulipix_common::pool_for("music").await else { return; };
        let (rows, pages) = tulipix_music::dl_history::searches_page(&pool, page as i64)
            .await
            .unwrap_or_default();
        let page = (page as i64).clamp(0, (pages - 1).max(0));
        let _ = weak.upgrade_in_event_loop(move |w| {
            let model: Vec<DlSearchEntry> = rows
                .iter()
                .map(|r| DlSearchEntry {
                    url: r.url.as_str().into(),
                    kind: r.kind.as_str().into(),
                    title: r.title.clone().unwrap_or_default().into(),
                    provider: r.provider.clone().unwrap_or_default().into(),
                    when: fmt_when(r.searched_at).into(),
                })
                .collect();
            w.set_music_dl_searches(ModelRc::new(VecModel::from(model)));
            w.set_music_dl_search_page(page as i32);
            w.set_music_dl_search_pages(pages as i32);
        });
    });
}

/// Play a downloaded track (from history) in-app.
pub fn play_history(w: &MainWindow, path: String, title: String, sub: String) {
    crate::play_music_file(w, &path, &title, &sub);
}
