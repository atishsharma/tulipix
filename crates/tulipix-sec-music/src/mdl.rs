//! Downloader tab glue — marshals `tulipix_mdl` (resolve + download) to the
//! Slint UI. Business logic stays in `tulipix-mdl`; this module only spawns the
//! work, tracks per-row selection + main-artist choice, pushes progress into
//! the UI models, ingests finished tracks straight into the library, and keeps
//! Download/Search history.

use slint::{ComponentHandle, ModelRc, SharedString, VecModel, Weak};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tulipix_mdl::types::{DownloadOptions, NameMethod, Playlist, Progress, ProviderId, Stage, Track};
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
    /// Decoded cover thumbnail (RGBA8 pixels + w/h). Kept as raw bytes — not a
    /// `slint::Image` — because `Image` is `!Send` and these rows live in a
    /// cross-thread static; the image is rebuilt on the UI thread in `push_rows`.
    thumb_rgba: Option<(Vec<u8>, u32, u32)>,
}

/// Bumped on every resolve so a slow background art-fetch from a previous URL
/// can detect it's stale and stop pushing thumbs into the current queue.
static ART_GEN: OnceLock<std::sync::atomic::AtomicU64> = OnceLock::new();
fn art_gen() -> &'static std::sync::atomic::AtomicU64 {
    ART_GEN.get_or_init(|| std::sync::atomic::AtomicU64::new(0))
}

/// Decode arbitrary cover bytes into a small RGBA thumbnail for a queue row.
fn decode_thumb(bytes: &[u8]) -> Option<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail(120, 120).to_rgba8();
    let (w, h) = small.dimensions();
    Some((small.into_raw(), w, h))
}

// ── Persistent art cache ─────────────────────────────────────────────────────
// Downloader queue thumbs are cached on disk so re-resolving a URL (e.g. picking
// it again from Search history) reuses the art instead of re-downloading — and
// for playlists reuses the resolved per-track art URL instead of re-fetching
// each track's page. Two tiny caches under <cache>/mdl_art: `t_<hash>` = original
// cover bytes keyed by art URL; `u_<hash>.txt` = resolved real art URL keyed by
// the track's source URL.

fn art_cache_root() -> Option<std::path::PathBuf> {
    let d = tulipix_core::paths::cache_dir()?.join("mdl_art");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

fn art_key(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    h.finalize().iter().take(12).map(|b| format!("{b:02x}")).collect()
}

/// Resolved real art URL previously cached for a track source URL.
fn cached_art_url(src: &str) -> Option<String> {
    let p = art_cache_root()?.join(format!("u_{}.txt", art_key(src)));
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn store_art_url(src: &str, url: &str) {
    if let Some(root) = art_cache_root() {
        let _ = std::fs::write(root.join(format!("u_{}.txt", art_key(src))), url);
    }
}

/// Fetch a cover URL and decode it to a queue thumbnail, backed by an on-disk
/// byte cache so the same art is never downloaded twice. Blocking IO/decode runs
/// off the async runtime.
async fn fetch_thumb(client: &reqwest::Client, url: &str) -> Option<(Vec<u8>, u32, u32)> {
    // Disk hit — decode the cached original bytes, no network.
    let u = url.to_string();
    if let Ok(Some(t)) = tokio::task::spawn_blocking(move || {
        let p = art_cache_root()?.join(format!("t_{}", art_key(&u)));
        let bytes = std::fs::read(p).ok()?;
        decode_thumb(&bytes)
    })
    .await
    {
        return Some(t);
    }
    // Miss — download, persist the original bytes, decode.
    let bytes = client
        .get(url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?
        .to_vec();
    let u = url.to_string();
    tokio::task::spawn_blocking(move || {
        if let Some(root) = art_cache_root() {
            let _ = std::fs::write(root.join(format!("t_{}", art_key(&u))), &bytes);
        }
        decode_thumb(&bytes)
    })
    .await
    .ok()?
}
static ROWS: OnceLock<Mutex<Vec<RowData>>> = OnceLock::new();
fn rows() -> &'static Mutex<Vec<RowData>> {
    ROWS.get_or_init(|| Mutex::new(Vec::new()))
}

/// Per-track cards shown per queue page. Only this page's rows are pushed to the
/// UI model (huge playlists stay responsive); `abs_index` maps a card back to
/// its full-list row.
const QUEUE_PAGE_SIZE: usize = 25;
static QUEUE_PAGE: OnceLock<Mutex<usize>> = OnceLock::new();
fn queue_page() -> &'static Mutex<usize> {
    QUEUE_PAGE.get_or_init(|| Mutex::new(0))
}

/// Set the visible queue page (clamped in `push_rows`) and re-push.
pub fn set_queue_page(weak: Weak<MainWindow>, page: i32) {
    *queue_page().lock().unwrap() = page.max(0) as usize;
    push_rows(&weak);
}

/// Current queue sort — (key, dir) where dir 1 = asc, 2 = desc.
static SORT_STATE: OnceLock<Mutex<(String, i32)>> = OnceLock::new();
fn sort_state() -> &'static Mutex<(String, i32)> {
    SORT_STATE.get_or_init(|| Mutex::new((String::new(), 1)))
}

fn emit_sort(weak: &Weak<MainWindow>, key: &str, dir: i32) {
    let key = key.to_string();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_dl_sort(SharedString::from(key.as_str()));
        w.set_music_dl_sort_dir(dir);
    });
}

/// Sort the parsed queue by `key` (name|length|artist). Re-clicking the active
/// key flips asc⇄desc. Reorders BOTH the UI rows and the cached playlist tracks
/// by the same permutation so per-row indices (download / retry / art) stay
/// aligned.
pub fn sort_queue(weak: Weak<MainWindow>, key: String) {
    let dir = {
        let mut st = sort_state().lock().unwrap();
        let d = if st.0 == key { if st.1 == 1 { 2 } else { 1 } } else { 1 };
        *st = (key.clone(), d);
        d
    };
    {
        let mut rows_g = rows().lock().unwrap();
        let n = rows_g.len();
        // Durations pulled from the cached tracks, parallel to rows.
        let durations: Vec<u64> = {
            let r = resolved().lock().unwrap();
            (0..n)
                .map(|i| {
                    r.as_ref()
                        .and_then(|p| p.tracks.get(i))
                        .and_then(|t| t.duration_ms)
                        .unwrap_or(0)
                })
                .collect()
        };
        let mut idx: Vec<usize> = (0..n).collect();
        idx.sort_by(|&a, &b| {
            let ord = match key.as_str() {
                "length" => durations[a].cmp(&durations[b]),
                "artist" => rows_g[a]
                    .main_artist
                    .to_lowercase()
                    .cmp(&rows_g[b].main_artist.to_lowercase()),
                _ => rows_g[a].title.to_lowercase().cmp(&rows_g[b].title.to_lowercase()),
            };
            if dir == 2 { ord.reverse() } else { ord }
        });
        let new_rows: Vec<RowData> = idx.iter().map(|&i| rows_g[i].clone()).collect();
        *rows_g = new_rows;
        if let Some(pl) = resolved().lock().unwrap().as_mut() {
            let nt: Vec<Track> = idx.iter().filter_map(|&i| pl.tracks.get(i).cloned()).collect();
            if nt.len() == pl.tracks.len() {
                pl.tracks = nt;
            }
        }
    }
    *queue_page().lock().unwrap() = 0;
    emit_sort(&weak, &key, dir);
    push_rows(&weak);
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

/// Options + destination of the last download run, so per-row Retry can rebuild
/// the same pipeline for a single failed track.
#[derive(Clone)]
struct DlCtx {
    dest: PathBuf,
    format: String,
    bitrate: u32,
    method: NameMethod,
    threads: usize,
}
static LAST_CTX: OnceLock<Mutex<Option<DlCtx>>> = OnceLock::new();
fn last_ctx() -> &'static Mutex<Option<DlCtx>> {
    LAST_CTX.get_or_init(|| Mutex::new(None))
}

/// Rolling CLI/activity log for the downloader — what it's doing in the
/// background. Cleared on each new download and empty at app start.
static CLI_LOG: OnceLock<Mutex<String>> = OnceLock::new();
fn cli_log() -> &'static Mutex<String> {
    CLI_LOG.get_or_init(|| Mutex::new(String::new()))
}

/// Append a line to the CLI log and push the buffer to the UI (capped).
fn cli_push(weak: &Weak<MainWindow>, line: &str) {
    let snap = {
        let mut g = cli_log().lock().unwrap();
        g.push_str(line);
        g.push('\n');
        // Keep the tail bounded so a huge playlist can't grow the buffer forever.
        if g.len() > 40_000 {
            let cut = g.len() - 40_000;
            *g = g[cut..].to_string();
        }
        g.clone()
    };
    let _ = weak.upgrade_in_event_loop(move |w| w.set_music_dl_cli(SharedString::from(snap.as_str())));
}

/// Clear the CLI log (new download / user "Clear").
pub fn clear_cli(weak: Weak<MainWindow>) {
    cli_log().lock().unwrap().clear();
    let _ = weak.upgrade_in_event_loop(|w| w.set_music_dl_cli(SharedString::new()));
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
    // Window the full queue to the current page; abs_index maps each card back.
    let total = snapshot.len();
    let selected = snapshot.iter().filter(|r| r.selected).count();
    let pages = total.div_ceil(QUEUE_PAGE_SIZE).max(1);
    let page = {
        let mut g = queue_page().lock().unwrap();
        *g = (*g).min(pages - 1);
        *g
    };
    let start = page * QUEUE_PAGE_SIZE;
    let end = (start + QUEUE_PAGE_SIZE).min(total);
    let window: Vec<(usize, RowData)> = snapshot
        .get(start..end)
        .unwrap_or(&[])
        .iter()
        .cloned()
        .enumerate()
        .map(|(off, r)| (start + off, r))
        .collect();
    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_dl_all_artists(ModelRc::new(VecModel::from(
            union.iter().map(SharedString::from).collect::<Vec<_>>(),
        )));
        w.set_music_dl_total(total as i32);
        w.set_music_dl_selected(selected as i32);
        w.set_music_dl_queue_page(page as i32);
        w.set_music_dl_queue_pages(pages as i32);
        let model: Vec<DownloaderRow> = window
            .iter()
            .map(|(abs, r)| DownloaderRow {
                abs_index: *abs as i32,
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
                thumb: r
                    .thumb_rgba
                    .as_ref()
                    .map(|(px, w, h)| {
                        let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(px, *w, *h);
                        slint::Image::from_rgba8(buf)
                    })
                    .unwrap_or_default(),
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
            thumb_rgba: None,
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

/// Push the resolved-collection kind (track|album|playlist) so the UI can
/// enable the matching "Per Album" / "Per Playlist" bulk button.
fn push_kind(weak: &Weak<MainWindow>, kind: &str) {
    let kind = kind.to_string();
    let _ = weak.upgrade_in_event_loop(move |w| w.set_music_dl_kind(SharedString::from(kind.as_str())));
}

/// Background: fetch per-track cover art after a resolve and stream the decoded
/// thumbnails into the queue rows. A single track or an album legitimately
/// shares one cover, so it is fetched once and reused. A playlist mixes releases,
/// so each track's real cover is fetched by re-resolving its source URL (bounded
/// concurrency); that real URL also replaces the row's `artwork_url` in the
/// cached playlist so the eventual download embeds the correct art. Aborts if a
/// newer resolve superseded this one.
fn stream_art(weak: Weak<MainWindow>, playlist: Playlist, kind: String, generation: u64) {
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        if kind != "playlist" {
            let url = playlist
                .artwork_url
                .clone()
                .or_else(|| playlist.tracks.first().and_then(|t| t.artwork_url.clone()));
            let Some(url) = url else { return; };
            let Some(thumb) = fetch_thumb(&client, &url).await else { return; };
            if art_gen().load(Ordering::Relaxed) != generation {
                return;
            }
            {
                let mut rows = rows().lock().unwrap();
                for row in rows.iter_mut() {
                    row.thumb_rgba = Some(thumb.clone());
                }
            }
            push_rows(&weak);
            return;
        }
        // Playlist — real per-track covers, bounded concurrency.
        let sem = Arc::new(tokio::sync::Semaphore::new(4));
        let mut handles = Vec::new();
        for (i, track) in playlist.tracks.iter().cloned().enumerate() {
            let sem = sem.clone();
            let client = client.clone();
            let weak = weak.clone();
            handles.push(tokio::spawn(async move {
                let _permit = match sem.acquire().await {
                    Ok(p) => p,
                    Err(_) => return,
                };
                if art_gen().load(Ordering::Relaxed) != generation {
                    return;
                }
                // Real per-track cover: reuse the previously resolved art URL from
                // the cache; otherwise re-resolve the track page once and cache it.
                let real_url = match &track.source_url {
                    Some(src) => match cached_art_url(src) {
                        Some(u) => Some(u),
                        None => {
                            let u = tulipix_mdl::resolve_url(&client, src)
                                .await
                                .ok()
                                .and_then(|pl| pl.tracks.into_iter().next())
                                .and_then(|t| t.artwork_url);
                            if let Some(ref uu) = u {
                                store_art_url(src, uu);
                            }
                            u
                        }
                    },
                    None => None,
                };
                let url = real_url.clone().or_else(|| track.artwork_url.clone());
                let Some(url) = url else { return; };
                if art_gen().load(Ordering::Relaxed) != generation {
                    return;
                }
                // Persist the real per-track art into the cached playlist so the
                // download embeds the correct cover, not the playlist thumbnail.
                if let Some(real) = real_url {
                    if let Some(pl) = resolved().lock().unwrap().as_mut() {
                        if let Some(t) = pl.tracks.get_mut(i) {
                            t.artwork_url = Some(real);
                        }
                    }
                }
                let Some(thumb) = fetch_thumb(&client, &url).await else { return; };
                if art_gen().load(Ordering::Relaxed) != generation {
                    return;
                }
                if let Some(row) = rows().lock().unwrap().get_mut(i) {
                    row.thumb_rgba = Some(thumb);
                }
                push_rows(&weak);
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    });
}

/// Search mode (np.p6.mdl.search): yt-dlp `ytsearch` on YouTube Music — the
/// results land in the same queue/tag/download pipeline as a resolved URL.
pub fn start_search(weak: Weak<MainWindow>, query: String) {
    set_status(&weak, "resolving");
    let _ = weak.upgrade_in_event_loop(|w| w.set_music_dl_yt_warn(false));
    cli_push(&weak, &format!("▸ searching YouTube Music: {query}"));
    tokio::runtime::Handle::current().spawn(async move {
        match tulipix_mdl::search_ytmusic(&query, 12).await {
            Ok(pl) => {
                cli_push(&weak, &format!("  found {} result(s)", pl.tracks.len()));
                seed_rows(&pl);
                record_search_bg(&pl, query.clone());
                let kind = resolve_kind(&pl).to_string();
                *resolved().lock().unwrap() = Some(pl.clone());
                // Store the raw query as the "resolved URL" — Download passes the
                // same field text back, so the playlist is reused, not re-resolved.
                *resolved_url().lock().unwrap() = query.clone();
                *queue_page().lock().unwrap() = 0;
                *sort_state().lock().unwrap() = (String::new(), 1);
                emit_sort(&weak, "", 1);
                push_kind(&weak, &kind);
                push_rows(&weak);
                set_status(&weak, "resolved");
                let generation = art_gen().fetch_add(1, Ordering::Relaxed) + 1;
                stream_art(weak.clone(), pl, kind, generation);
            }
            Err(e) => set_status(&weak, &format!("error: {e}")),
        }
    });
}

/// Resolve a URL and show the tracklist preview (all rows selected).
pub fn start_resolve(weak: Weak<MainWindow>, url: String) {
    set_status(&weak, "resolving");
    let _ = weak.upgrade_in_event_loop(|w| w.set_music_dl_yt_warn(false));
    cli_push(&weak, &format!("▸ resolving {url}"));
    tokio::runtime::Handle::current().spawn(async move {
        let client = reqwest::Client::new();
        match tulipix_mdl::resolve_url(&client, &url).await {
            Ok(pl) => {
                cli_push(&weak, &format!("  resolved: {} — {} track(s) [{}]", pl.title, pl.tracks.len(), pl.provider.display_name()));
                seed_rows(&pl);
                record_search_bg(&pl, url.clone());
                let kind = resolve_kind(&pl).to_string();
                *resolved().lock().unwrap() = Some(pl.clone());
                *resolved_url().lock().unwrap() = url.clone();
                *queue_page().lock().unwrap() = 0;
                *sort_state().lock().unwrap() = (String::new(), 1);
                emit_sort(&weak, "", 1);
                push_kind(&weak, &kind);
                push_rows(&weak);
                set_status(&weak, "resolved");
                // YouTube link that doesn't look like tagged music — warn (but
                // still resolve). music.youtube.com URLs, or any track carrying a
                // real album tag, are treated as genuine music; a bare youtube.com
                // video with no music metadata trips the warning.
                let yt_warn = pl.provider == ProviderId::YoutubeMusic
                    && !url.contains("music.youtube.com")
                    && !pl.tracks.iter().any(|t| t.album.is_some());
                let _ = weak.upgrade_in_event_loop(move |w| w.set_music_dl_yt_warn(yt_warn));
                // Kick off background per-track art fetch for the queue thumbs.
                let generation = art_gen().fetch_add(1, Ordering::Relaxed) + 1;
                stream_art(weak.clone(), pl, kind, generation);
            }
            Err(e) => set_status(&weak, &format!("error: {e}")),
        }
    });
}

/// Clear the resolved playlist + queue rows (title-row "Clear" button).
pub fn clear_all(weak: Weak<MainWindow>) {
    art_gen().fetch_add(1, Ordering::Relaxed); // supersede any in-flight art fetch
    rows().lock().unwrap().clear();
    *resolved().lock().unwrap() = None;
    resolved_url().lock().unwrap().clear();
    *queue_page().lock().unwrap() = 0;
    *sort_state().lock().unwrap() = (String::new(), 1);
    let _ = weak.upgrade_in_event_loop(|w| {
        w.set_music_dl_rows(ModelRc::new(VecModel::from(Vec::<DownloaderRow>::new())));
        w.set_music_dl_all_artists(ModelRc::new(VecModel::from(Vec::<SharedString>::new())));
        w.set_music_dl_kind(SharedString::new());
        w.set_music_dl_sort(SharedString::new());
        w.set_music_dl_sort_dir(1);
        w.set_music_dl_total(0);
        w.set_music_dl_selected(0);
        w.set_music_dl_queue_page(0);
        w.set_music_dl_queue_pages(1);
        w.set_music_dl_done(0);
        w.set_music_dl_skipped(0);
        w.set_music_dl_failed(0);
        w.set_music_dl_status(SharedString::from("idle"));
        w.set_music_dl_yt_warn(false);
    });
}

/// Wipe the download-history log (records only — files on disk are kept), then
/// reload the popup's first page so the list + counter reset live.
pub fn clear_history(weak: Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = tulipix_common::pool_for("music").await {
            let _ = tulipix_music::dl_history::clear_history(&pool).await;
        }
        load_history(weak, 0);
    });
}

/// Wipe the search-history log, then reload the popup's first page.
pub fn clear_searches(weak: Weak<MainWindow>) {
    tokio::runtime::Handle::current().spawn(async move {
        if let Ok(pool) = tulipix_common::pool_for("music").await {
            let _ = tulipix_music::dl_history::clear_searches(&pool).await;
        }
        load_searches(weak, 0);
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
        // Render (or reuse) the embedded-cover thumbnail so the new tile AND the
        // now-playing player show real art immediately — no restart/rescan.
        let path = PathBuf::from(&abs_path);
        let thumb = tulipix_core::thumbs::render_or_cache(
            &path, tulipix_core::thumbs::ThumbSpec {
                kind: tulipix_core::thumbs::ThumbKind::Audio, width: 320, height: 320 })
            .ok().flatten().map(|t| t.path).unwrap_or_default();
        let label = path.file_stem().and_then(|s| s.to_str())
            .unwrap_or(track.title.as_str()).to_string();
        let _ = weak.upgrade_in_event_loop(move |w| {
            // Add to the in-memory accumulator (dedup by abs path) so the track
            // enters music_paths → Songs/Albums/Artists/Folders show it live and
            // play_history resolves its library position (cover art in the player)
            // WITHOUT waiting for an app restart / folder rescan.
            if let Ok(mut g) = crate::music_full().lock() {
                if !g.iter().any(|(_, o, _)| o == &path) {
                    g.push((label, path, thumb));
                }
            }
            crate::rebuild_music_tiles(&w);
            crate::populate_music_views(w.as_weak());
            crate::populate_folder_roots(&w);
        });
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
    bitrate: i32,
) {
    cancel_flag().store(false, Ordering::Relaxed);
    tulipix_common::add_watched_folder(&default_music_dir());
    tulipix_common::add_watched_folder(&dest);
    let method = NameMethod::from_label(&name_method_label);
    let parallel = parallel.clamp(1, 4) as usize;
    let threads = threads.clamp(1, 8) as usize;
    let bitrate = bitrate.clamp(0, 320) as u32;
    // Fresh CLI log per download run.
    clear_cli(weak.clone());
    cli_push(&weak, &format!("$ mdl download → {}", dest.display()));
    cli_push(&weak, &format!("  format={format} bitrate={bitrate}k name=\"{name_method_label}\" parallel={parallel} threads={threads}"));

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
        let tracks: Vec<Track> = selected
            .iter()
            .zip(mains.iter())
            .map(|(i, main)| {
                let mut t = playlist.tracks[*i].clone();
                apply_main(&mut t, main);
                t
            })
            .collect();

        // Skip tracks already owned in the library — mark the row "in library"
        // and never re-fetch them.
        let pool = tulipix_common::pool_for("music").await.ok();
        let mut keep_tracks: Vec<Track> = Vec::new();
        let mut keep_idx: Vec<usize> = Vec::new();
        let mut base_skipped = 0usize;
        for (t, ri) in tracks.into_iter().zip(selected.iter().copied()) {
            let main = t.artists.first().cloned().unwrap_or_default();
            let in_lib = match &pool {
                Some(p) => tulipix_music::dl_history::track_in_library(p, &t.title, &main).await,
                None => false,
            };
            if in_lib {
                base_skipped += 1;
                if let Some(row) = rows().lock().unwrap().get_mut(ri) {
                    row.stage = "in library".into();
                    row.percent = 100.0;
                    row.file = "Already in library".into();
                }
                cli_push(&weak, &format!("- in library » {}", t.title));
            } else {
                keep_tracks.push(t);
                keep_idx.push(ri);
            }
        }
        push_rows(&weak);
        if keep_tracks.is_empty() {
            let _ = weak.upgrade_in_event_loop(move |w| {
                w.set_music_dl_skipped(base_skipped as i32);
            });
            set_status(&weak, "all in library");
            return;
        }

        let filtered = Playlist { tracks: keep_tracks.clone(), ..playlist.clone() };
        let provider = playlist.provider.display_name().to_string();
        let opts = DownloadOptions {
            dest_dir: dest.clone(),
            parallelism: parallel,
            threads_per_download: threads,
            format,
            bitrate,
            name_method: method,
        };
        run_download(weak, filtered, keep_idx, keep_tracks, provider, opts, cancel, base_skipped).await;
    });
}

/// Re-download a single failed track (its full-list row `row_index`) using the
/// last run's destination/format/naming. No-op if there is no cached context.
pub fn retry_track(weak: Weak<MainWindow>, row_index: i32) {
    let ri = row_index.max(0) as usize;
    let Some(ctx) = last_ctx().lock().unwrap().clone() else { return; };
    let playlist = { resolved().lock().unwrap().clone() };
    let Some(playlist) = playlist else { return; };
    let Some(base) = playlist.tracks.get(ri).cloned() else { return; };
    // Apply the row's current main-artist choice.
    let main = rows()
        .lock()
        .unwrap()
        .get(ri)
        .map(|r| r.main_artist.clone())
        .unwrap_or_default();
    let mut track = base;
    apply_main(&mut track, &main);
    // Reset the row to "queued" so the pill + fill restart.
    if let Some(r) = rows().lock().unwrap().get_mut(ri) {
        r.stage = "queued".into();
        r.percent = 0.0;
        r.file.clear();
    }
    push_rows(&weak);
    cli_push(&weak, &format!("↻ retry ▸ {}", track.title));

    let provider = playlist.provider.display_name().to_string();
    let opts = DownloadOptions {
        dest_dir: ctx.dest.clone(),
        parallelism: 1,
        threads_per_download: ctx.threads,
        format: ctx.format.clone(),
        bitrate: ctx.bitrate,
        name_method: ctx.method,
    };
    let filtered = Playlist { tracks: vec![track.clone()], ..playlist.clone() };
    cancel_flag().store(false, Ordering::Relaxed);
    let cancel = cancel_flag().clone();
    tokio::runtime::Handle::current().spawn(async move {
        run_download(weak, filtered, vec![ri], vec![track], provider, opts, cancel, 0).await;
    });
}

/// Drive one `download_playlist` run: pumps per-track progress into the UI rows,
/// logs CLI activity, ingests finished tracks into the library, and mirrors the
/// counters. Shared by a full download and a single-track Retry.
///
/// `index_map_vec[k]` is the full-list row that worker track `k+1` maps back to;
/// `ingest_tracks[k]` is that track's metadata (main artist first) for ingest.
#[allow(clippy::too_many_arguments)]
async fn run_download(
    weak: Weak<MainWindow>,
    filtered: Playlist,
    index_map_vec: Vec<usize>,
    ingest_tracks: Vec<Track>,
    provider: String,
    opts: DownloadOptions,
    cancel: Arc<AtomicBool>,
    // Tracks already skipped as "in library" before the run — added to the
    // displayed skipped counter so the progress pill reflects them.
    base_skipped: usize,
) {
    *index_map().lock().unwrap() = index_map_vec;
    *last_ctx().lock().unwrap() = Some(DlCtx {
        dest: opts.dest_dir.clone(),
        format: opts.format.clone(),
        bitrate: opts.bitrate,
        method: opts.name_method,
        threads: opts.threads_per_download,
    });
    set_status(&weak, "downloading");
    let client = reqwest::Client::new();
    let dest_for_ingest = opts.dest_dir.clone();
    let progress_weak = weak.clone();
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
                // Keep the stage a clean single word ("failed") so the UI can
                // match it for the pill + Retry button; stash the reason in file.
                row.stage = stage_label(p.stage).to_string();
                row.percent = p.percent;
                if matches!(p.stage, Stage::Failed) {
                    row.file = p.message.chars().take(80).collect();
                } else if let Some(f) = &p.file_name {
                    row.file = f.clone();
                }
            }
        }
        push_rows(&progress_weak);
        // CLI activity line per meaningful stage transition.
        let cli = match p.stage {
            Stage::SearchingYoutube => format!("[{}/{}] search  ▸ {}", p.track_index, p.total, p.title),
            Stage::WritingMetadata => format!("[{}/{}] tag     · {}", p.track_index, p.total, p.title),
            Stage::Completed => format!("[{}/{}] done    ✓ {}", p.track_index, p.total, p.file_name.clone().unwrap_or_default()),
            Stage::Failed => format!("[{}/{}] FAILED  ✗ {} — {}", p.track_index, p.total, p.title, p.message),
            Stage::Skipped => format!("[{}/{}] skip    » {}", p.track_index, p.total, p.title),
            _ => String::new(),
        };
        if !cli.is_empty() {
            cli_push(&progress_weak, &cli);
        }
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
        let (done, skipped, failed) = (p.downloaded, p.skipped + base_skipped, p.failed);
        let _ = progress_weak.upgrade_in_event_loop(move |w| {
            w.set_music_dl_done(done as i32);
            w.set_music_dl_skipped(skipped as i32);
            w.set_music_dl_failed(failed as i32);
        });
    };

    let summary =
        tulipix_mdl::download::download_playlist(&client, &filtered, &opts, cancel, on_progress).await;

    let _ = weak.upgrade_in_event_loop(move |w| {
        w.set_music_dl_done(summary.downloaded as i32);
        w.set_music_dl_skipped((summary.skipped + base_skipped) as i32);
        w.set_music_dl_failed(summary.failed.len() as i32);
    });
    set_status(&weak, "done");
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

/// Play a downloaded track (from history) in-app. Downloaded tracks are ingested
/// into the library, so prefer the real library play path: it loads the cover
/// art and sets the now-playing index so clicking the title/artist in the player
/// opens the *correct* album/artist page. Falls back to a raw file stream only
/// if the track isn't in the current library list.
pub fn play_history(w: &MainWindow, path: String, title: String, sub: String) {
    let pos = crate::music_paths()
        .lock()
        .ok()
        .and_then(|g| g.iter().position(|p| p.to_string_lossy() == path));
    match pos {
        Some(i) => crate::play_music_at(w, i as i32),
        None => crate::play_music_file(w, &path, &title, &sub),
    }
}
