//! Settings' upkeep buttons, and the service keys that live in the system
//! keychain.
//!
//! A watched folder read again on its own, its thumbnails redrawn, the photo
//! search index rebuilt, a music folder moved to another shelf, the library
//! exported, another app's library brought in. Settings reaches all of it
//! through its own `Action` and `SetText` arms, so none of it is a bridge
//! command: everything here is `pub(crate)`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, UNIX_EPOCH};

use anyhow::Result;

use crate::api::settings::ServiceKeyRow;
use crate::api::shell::{load, save};

// ── service keys ────────────────────────────────────────────────────────────

/// The services with a key worth setting, in the order drawn. The first five
/// are the Slint build's Service keys page; the rest arrived with the service
/// integrations, and every one of them is a secret, so every one of them is
/// here rather than in settings.json. The others in `api_keys::SERVICES` are
/// keyless public APIs, or not keys at all.
const KEYED: [(&str, &str); 12] = [
    ("tmdb", "TMDB"),
    ("tvdb", "TheTVDB"),
    ("opensubtitles", "OpenSubtitles"),
    ("lastfm", "Last.fm"),
    ("libretranslate", "LibreTranslate"),
    ("discogs", "Discogs"),
    ("spotify_id", "Spotify client ID"),
    ("spotify_secret", "Spotify client secret"),
    ("youtube_data", "YouTube Data"),
    ("trakt", "Trakt client ID"),
    ("trakt_secret", "Trakt client secret"),
    // Not a key: AniDB registers a client by name, and every request carries
    // it. Same box, same keychain, so it is kept with the keys.
    ("anidb", "AniDB client name"),
];

fn label(service: &str) -> Option<&'static str> {
    KEYED.iter().find(|(s, _)| *s == service).map(|(_, l)| *l)
}

/// Which of them have a key, cached. A keychain read is a D-Bus round trip on
/// a thread of its own, and the settings snapshot is taken on every change and
/// every second of a model download; only a save or a removal changes this.
fn saved_cell() -> &'static Mutex<Option<Vec<bool>>> {
    static C: std::sync::OnceLock<Mutex<Option<Vec<bool>>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn forget_saved() {
    if let Ok(mut g) = saved_cell().lock() {
        *g = None;
    }
}

fn has_key(service: &str) -> bool {
    tulipix_core::api_keys::fetch(service)
        .ok()
        .flatten()
        .is_some_and(|k| !k.trim().is_empty())
}

pub(crate) fn service_keys() -> Vec<ServiceKeyRow> {
    let saved = {
        let mut g = saved_cell().lock().unwrap_or_else(|p| p.into_inner());
        g.get_or_insert_with(|| KEYED.iter().map(|(s, _)| has_key(s)).collect())
            .clone()
    };
    KEYED
        .iter()
        .zip(saved)
        .map(|((service, label), saved)| ServiceKeyRow {
            service: (*service).into(),
            label: (*label).into(),
            saved,
        })
        .collect()
}

/// Save a key. Only the five services above: the name is text from the page,
/// and the keychain is not a place to let it write under any name it likes.
pub(crate) fn store_key(service: &str, value: &str) -> String {
    let Some(name) = label(service) else {
        return "That is not a service Tulipix keeps a key for.".into();
    };
    let key = value.trim();
    if key.is_empty() {
        return format!("Paste a {name} key first.");
    }
    let r = tulipix_core::api_keys::store(service, key);
    forget_saved();
    match r {
        Ok(()) => format!("{name} key saved in the system keychain."),
        Err(e) => format!("Could not save the {name} key: {e}"),
    }
}

pub(crate) fn remove_key(service: &str) -> String {
    let Some(name) = label(service) else {
        return "That is not a service Tulipix keeps a key for.".into();
    };
    let r = tulipix_core::api_keys::delete(service);
    forget_saved();
    match r {
        Ok(()) => format!("{name} key removed."),
        Err(e) => format!("Could not remove the {name} key: {e}"),
    }
}

/// Ask the service whether it takes the saved key: one small request each,
/// the cheapest endpoint that checks a key. The Slint build's Test only asked
/// whether a key was saved, which says nothing about whether it works.
pub(crate) async fn test_key(service: &str) -> String {
    let Some(name) = label(service) else {
        return "That is not a service Tulipix keeps a key for.".into();
    };
    let Some(key) = tulipix_core::api_keys::fetch(service)
        .ok()
        .flatten()
        .filter(|k| !k.trim().is_empty())
    else {
        return format!("There is no {name} key saved to test.");
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Tulipix v", env!("CARGO_PKG_VERSION")))
        .build()
    {
        Ok(c) => c,
        Err(e) => return format!("Could not test the key: {e}"),
    };
    let key = key.trim();
    let req = match service {
        "tmdb" => client
            .get("https://api.themoviedb.org/3/configuration")
            .query(&[("api_key", key)]),
        "lastfm" => client.get("https://ws.audioscrobbler.com/2.0/").query(&[
            ("method", "chart.gettopartists"),
            ("limit", "1"),
            ("format", "json"),
            ("api_key", key),
        ]),
        "opensubtitles" => client
            .get("https://api.opensubtitles.com/api/v1/infos/formats")
            .header("Api-Key", key),
        "tvdb" => client
            .post("https://api4.thetvdb.com/v4/login")
            .json(&serde_json::json!({ "apikey": key })),
        // The integrations each know their own cheapest call, so the check
        // goes through their client rather than being spelled out twice.
        "discogs" => {
            return match tulipix_music::discogs::DiscogsClient::new(key).check().await {
                Ok(true) => "Discogs accepted the token.".into(),
                Ok(false) => "Discogs turned the token down. Check it was copied whole.".into(),
                Err(e) => format!("Could not reach Discogs: {e}"),
            };
        }
        // Spotify needs both halves, and the token call is what checks them.
        "spotify_id" | "spotify_secret" => {
            let (Some(id), Some(secret)) = (
                tulipix_core::api_keys::fetch("spotify_id").ok().flatten(),
                tulipix_core::api_keys::fetch("spotify_secret").ok().flatten(),
            ) else {
                return "Save both the client ID and the client secret, then test.".into();
            };
            return match tulipix_music::spotify_api::SpotifyClient::new(id.trim(), secret.trim())
                .check()
                .await
            {
                Ok(_) => "Spotify accepted the ID and secret.".into(),
                Err(e) => format!("Spotify turned them down: {e}"),
            };
        }
        "youtube_data" => {
            return match tulipix_music::youtube_data::YoutubeDataClient::new(key).check().await {
                Ok(true) => "YouTube accepted the key.".into(),
                Ok(false) => {
                    "YouTube turned the key down, or today's quota is spent.".into()
                }
                Err(e) => format!("Could not reach YouTube: {e}"),
            };
        }
        // Trakt's id is checked by asking for a device code, which is the
        // first thing Link does anyway.
        "trakt" | "trakt_secret" => {
            let Some(app) = crate::services::trakt_app() else {
                return "Save both the Trakt client ID and the client secret, then test.".into();
            };
            return match app.device_code().await {
                Ok(_) => "Trakt accepted the ID. Use Link to sign in.".into(),
                Err(e) => format!("Trakt turned the ID down: {e}"),
            };
        }
        // AniDB's client name is only checked by a real lookup, and their
        // terms are firm about how often that may happen.
        "anidb" => {
            return format!(
                "The AniDB client name is saved. AniDB allows one titles \
                 download a day, so {name} is checked the first time an anime \
                 is looked up."
            );
        }
        // Every LibreTranslate server is someone's own, with its own rules.
        _ => {
            return format!(
                "The {name} key is saved. LibreTranslate servers differ, so it is \
                 checked the first time something is translated."
            );
        }
    };
    match req.send().await {
        Ok(r) if r.status().is_success() => format!("{name} accepted the key."),
        Ok(r) if matches!(r.status().as_u16(), 401 | 403) => {
            format!("{name} turned the key down. Check it was copied whole.")
        }
        Ok(r) => format!("{name} answered {}. Try again in a while.", r.status()),
        Err(e) => format!("Could not reach {name}: {e}"),
    }
}

/// Keys typed into a text row, which sat in settings.json where nothing reads
/// them. Each one moves to the keychain, where the section that needs it
/// looks, and comes out of the file.
///
/// `api.tmdb` was the first: the Flutter build saved it and the videos read
/// the keychain, so a key typed there did nothing at all. The Spotify and
/// YouTube ones are the same mistake, found when the service integrations
/// went in -- and they are secrets, which settings.json is not for.
///
/// Runs on every snapshot and does nothing after the first: once a key has
/// moved, the file has no such entry to find.
pub(crate) fn rescue_typed_keys() {
    const MOVED: [(&str, &str); 4] = [
        ("api.tmdb", "tmdb"),
        ("api.spotify-id", "spotify_id"),
        ("api.spotify-secret", "spotify_secret"),
        ("api.youtube-data", "youtube_data"),
    ];
    let mut moved_any = false;
    for (old_key, service) in MOVED {
        let Some(typed) = load().advanced.get(old_key).map(|v| v.trim().to_string()) else {
            continue;
        };
        if !typed.is_empty() && !has_key(service) {
            // Left where it is if the keychain will not take it, and tried
            // again on the next snapshot, rather than lost.
            if tulipix_core::api_keys::store(service, &typed).is_err() {
                continue;
            }
            moved_any = true;
        }
        let mut s = load();
        s.advanced.remove(old_key);
        save(s);
    }
    if moved_any {
        forget_saved();
    }
}

// ── Trakt's device flow ─────────────────────────────────────────────────────
//
// The one integration with a sign-in. Link asks Trakt for a code, and the
// polling runs detached: the code is good for ten minutes and a settings
// dispatch that waited on it would hold the page for all of them. What it is
// doing rides the snapshot, the way a model download does.

pub(crate) enum TraktState {
    /// No client ID and secret saved yet, so there is nothing to link with.
    NoApp,
    /// Waiting for the user to type `code` at `url`.
    Waiting { code: String, url: String },
    Linked(String),
    NotLinked,
}

/// The code being waited on, and whether the poll is still running. Cleared
/// when the flow ends, however it ends.
fn trakt_pending() -> &'static Mutex<Option<(String, String)>> {
    static P: std::sync::OnceLock<Mutex<Option<(String, String)>>> = std::sync::OnceLock::new();
    P.get_or_init(|| Mutex::new(None))
}

/// The account name behind the saved token, looked up once per run. `None`
/// means "not asked yet"; `Some(None)` means asked, and the token is no good.
fn trakt_user() -> &'static Mutex<Option<Option<String>>> {
    static U: std::sync::OnceLock<Mutex<Option<Option<String>>>> = std::sync::OnceLock::new();
    U.get_or_init(|| Mutex::new(None))
}

pub(crate) fn trakt_state() -> TraktState {
    if let Some((code, url)) = trakt_pending().lock().ok().and_then(|g| g.clone()) {
        return TraktState::Waiting { code, url };
    }
    if crate::services::key("trakt").is_none() || crate::services::key("trakt_secret").is_none() {
        return TraktState::NoApp;
    }
    if crate::services::key("trakt_token").is_none() {
        return TraktState::NotLinked;
    }
    // The name is filled in by the check `start_trakt_link` kicks off after a
    // successful link, and by the first Refresh after a restart.
    match trakt_user().lock().ok().and_then(|g| g.clone()) {
        Some(Some(user)) => TraktState::Linked(user),
        Some(None) => TraktState::NotLinked,
        None => {
            start_trakt_check();
            TraktState::Linked("…".into())
        }
    }
}

/// Ask Trakt whose token this is, once. A token they no longer accept comes
/// back as `Some(None)` and the row says "Not linked" rather than lying.
fn start_trakt_check() {
    let Some((client, token)) = crate::services::trakt() else { return };
    // Marked as asked at once, so a snapshot a second later does not start a
    // second lookup.
    if let Ok(mut g) = trakt_user().lock() {
        if g.is_some() {
            return;
        }
        *g = Some(Some("…".into()));
    }
    tokio::spawn(async move {
        let user = client.username(&token).await.ok().flatten();
        if let Ok(mut g) = trakt_user().lock() {
            *g = Some(user);
        }
    });
}

/// Link, cancel or unlink, depending on where the flow is. One button, because
/// there is only ever one thing to do next.
pub(crate) fn trakt_link_action() -> String {
    if trakt_pending().lock().is_ok_and(|g| g.is_some()) {
        if let Ok(mut g) = trakt_pending().lock() {
            *g = None;
        }
        return "Stopped waiting for Trakt.".into();
    }
    if crate::services::key("trakt_token").is_some() {
        let _ = tulipix_core::api_keys::delete("trakt_token");
        if let Ok(mut g) = trakt_user().lock() {
            *g = None;
        }
        forget_saved();
        return "Unlinked. Nothing more is sent to Trakt.".into();
    }
    let Some(app) = crate::services::trakt_app() else {
        return "Add a Trakt client ID and secret first — they come from \
                trakt.tv/oauth/applications."
            .into();
    };
    tokio::spawn(async move {
        let code = match app.device_code().await {
            Ok(c) => c,
            Err(e) => return done(format!("Trakt would not start the sign-in: {e}")),
        };
        if let Ok(mut g) = trakt_pending().lock() {
            *g = Some((code.user_code.clone(), code.verification_url.clone()));
        }
        let deadline = std::time::Instant::now() + Duration::from_secs(code.expires_in.max(60) as u64);
        let mut wait = Duration::from_secs(code.interval.max(1) as u64);
        let outcome = loop {
            tokio::time::sleep(wait).await;
            // Cancel is the pending slot going empty under us.
            if trakt_pending().lock().is_ok_and(|g| g.is_none()) {
                return;
            }
            if std::time::Instant::now() >= deadline {
                break "The Trakt code ran out. Press Link to get another.".to_string();
            }
            match app.poll_token(&code.device_code).await {
                Ok(tulipix_videos::trakt::DevicePoll::Token(token)) => {
                    break match tulipix_core::api_keys::store("trakt_token", &token) {
                        Ok(()) => {
                            if let Ok(mut g) = trakt_user().lock() {
                                *g = None;
                            }
                            forget_saved();
                            "Trakt is linked. What you finish watching goes to your history.".into()
                        }
                        Err(e) => format!("Trakt signed in, but the token could not be saved: {e}"),
                    };
                }
                Ok(tulipix_videos::trakt::DevicePoll::Pending) => {}
                Ok(tulipix_videos::trakt::DevicePoll::SlowDown) => wait += Duration::from_secs(1),
                Ok(tulipix_videos::trakt::DevicePoll::Expired) => {
                    break "The Trakt code ran out. Press Link to get another.".into()
                }
                Ok(tulipix_videos::trakt::DevicePoll::Denied) => {
                    break "The sign-in was turned down at trakt.tv.".into()
                }
                Err(e) => break format!("Trakt stopped answering: {e}"),
            }
        };
        if let Ok(mut g) = trakt_pending().lock() {
            *g = None;
        }
        done(outcome);
    });
    "Opening Trakt — type the code shown on this row at trakt.tv/activate.".into()
}

// ── the running job ─────────────────────────────────────────────────────────
//
// One long job at a time, as the Slint build's maintenance bar had one: a
// folder read again, the search index rebuilt, every folder rescanned. A
// Settings job runs detached -- a dispatch that waited on it would stop the
// page polling -- and what it is doing, and how far, rides every snapshot.
// What it said when it finished is the next snapshot's notice.

// Not `Job`: flutter_rust_bridge resolves types by name, and the Tools
// queue's `Job` would bind to this one.
struct Slot {
    key: String,
    label: String,
    frac: f64,
}

fn job_cell() -> &'static Mutex<Option<Slot>> {
    static C: std::sync::OnceLock<Mutex<Option<Slot>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

fn done_cell() -> &'static Mutex<Option<String>> {
    static C: std::sync::OnceLock<Mutex<Option<String>>> = std::sync::OnceLock::new();
    C.get_or_init(|| Mutex::new(None))
}

/// The job running now: its key, what to call it, and 0..1 -- or -1 where
/// there is no honest fraction. An empty key: nothing is running.
pub(crate) fn job() -> (String, String, f64) {
    job_cell()
        .lock()
        .ok()
        .and_then(|g| g.as_ref().map(|j| (j.key.clone(), j.label.clone(), j.frac)))
        .unwrap_or_default()
}

/// What the last job said when it ended. Given out once.
pub(crate) fn take_done() -> Option<String> {
    done_cell().lock().ok().and_then(|mut g| g.take())
}

/// Holds the job slot. Dropping it frees the slot, however the job ended.
pub(crate) struct JobGuard;

impl Drop for JobGuard {
    fn drop(&mut self) {
        if let Ok(mut g) = job_cell().lock() {
            *g = None;
        }
    }
}

/// Take the slot, or None while another job has it.
pub(crate) fn begin(key: &str, label: &str) -> Option<JobGuard> {
    let mut g = job_cell().lock().ok()?;
    if g.is_some() {
        return None;
    }
    *g = Some(Slot { key: key.into(), label: label.into(), frac: -1.0 });
    Some(JobGuard)
}

fn progress(frac: f64) {
    if let Ok(mut g) = job_cell().lock()
        && let Some(j) = g.as_mut()
    {
        j.frac = frac;
    }
}

fn done(msg: String) {
    if let Ok(mut g) = done_cell().lock() {
        *g = Some(msg);
    }
}

fn busy() -> String {
    let (_, label, _) = job();
    format!("Wait for “{label}” to finish first.")
}

fn folder_name(dir: &Path) -> String {
    dir.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.display().to_string())
}

/// Read one folder again, detached. The page's bar says it is running; the
/// notice after says how it went.
pub(crate) fn start_rescan(dir: PathBuf) -> String {
    let key = format!("lib-rescan:{}", dir.display());
    let Some(guard) = begin(&key, &format!("Reading {} again", folder_name(&dir))) else {
        return busy();
    };
    tokio::spawn(async move {
        let msg = rescan_folder(&dir).await;
        stamp_scanned(std::slice::from_ref(&dir));
        refresh_counts().await;
        done(msg);
        drop(guard);
    });
    String::new()
}

pub(crate) fn start_rebuild_search() -> String {
    let Some(guard) = begin("rebuild-search", "Rebuilding the photo search index") else {
        return busy();
    };
    tokio::spawn(async move {
        let msg = match rebuild_search().await {
            Ok(n) => format!("Search index rebuilt: {n} photo{}.", if n == 1 { "" } else { "s" }),
            Err(e) => format!("Could not rebuild the search index: {e}"),
        };
        done(msg);
        drop(guard);
    });
    String::new()
}

// ── tool updates ────────────────────────────────────────────────────────────

/// Where each tool's new releases are. Only yt-dlp has an update channel the
/// app can use (`updater::update_ytdlp_now`); a bundled or system binary of
/// the rest is not the app's to replace, so their Update opens this page.
const TOOL_PAGES: [(&str, &str); 6] = [
    ("ffmpeg", "https://ffmpeg.org/download.html"),
    ("ffprobe", "https://ffmpeg.org/download.html"),
    ("rclone", "https://rclone.org/downloads/"),
    ("whisper-cli", "https://github.com/ggml-org/whisper.cpp/releases"),
    ("mpv", "https://mpv.io/installation/"),
    ("exiftool", "https://exiftool.org/"),
];

/// Update one tool from Settings › Advanced. yt-dlp downloads its new release,
/// detached in the job slot; the others open their download page.
pub(crate) fn update_tool(name: &str) -> String {
    if name == "yt-dlp" {
        let Some(guard) = begin("tool-update:yt-dlp", "Updating yt-dlp") else {
            return busy();
        };
        tokio::spawn(async move {
            // Blocking reqwest and a 30 MB download: off the async threads.
            let msg = match tokio::task::spawn_blocking(tulipix_core::updater::update_ytdlp_now).await {
                Ok(Ok(Some(v))) => format!("yt-dlp updated to {v}."),
                Ok(Ok(None)) => "yt-dlp is already up to date.".into(),
                Ok(Err(e)) => format!("yt-dlp update failed: {e}"),
                Err(e) => format!("yt-dlp update failed: {e}"),
            };
            done(msg);
            drop(guard);
        });
        return String::new();
    }
    let Some((_, url)) = TOOL_PAGES.iter().find(|(n, _)| *n == name) else {
        return "That is not a tool Tulipix runs.".into();
    };
    crate::api::transfer::open_url(url);
    if name == "mpv" {
        // libmpv is linked, not looked up: the tools folder does not apply.
        "Opened the mpv download page. libmpv comes with Tulipix, or on Linux \
         from your system's packages."
            .into()
    } else {
        format!(
            "Opened the {name} download page. Put the new copy in your tools \
             folder and Tulipix uses it at once."
        )
    }
}

// ── scanning on a schedule, and saying so ───────────────────────────────────

/// Settings › Libraries' "Skip these": glob patterns every scan passes over,
/// comma- or line-separated. The populator matches them against whole paths,
/// on top of its own defaults.
pub(crate) fn exclusions() -> Vec<String> {
    load()
        .text("scan.exclude")
        .split([',', '\n'])
        .map(str::trim)
        .filter(|g| !g.is_empty())
        .map(String::from)
        .collect()
}

fn cadence(s: &str) -> Option<tulipix_core::libraries::ScanCadence> {
    use tulipix_core::libraries::ScanCadence::*;
    Some(match s.trim() {
        "manual" => Manual,
        "hourly" => Hourly,
        "daily" => Daily,
        "weekly" => Weekly,
        _ => return None,
    })
}

/// Start the auto-rescan loop, once per process; the shell's first snapshot
/// calls it. Every ten minutes each watched folder goes through
/// `scheduler::due_now` with its own cadence (or the default) and the time it
/// was last read, and the due ones are read again in the job slot.
pub(crate) fn start_auto_rescan() {
    static STARTED: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    tokio::spawn(async {
        // Not in the first minutes: launch has enough to do.
        tokio::time::sleep(Duration::from_secs(120)).await;
        let mut every = tokio::time::interval(Duration::from_secs(600));
        loop {
            every.tick().await;
            auto_rescan_tick().await;
        }
    });
}

async fn auto_rescan_tick() {
    use tulipix_core::libraries::{Library, ScanCadence, Section};
    use tulipix_core::power_aware as pa;
    let s = load();
    let default = cadence(&s.text("scan.cadence")).unwrap_or(ScanCadence::Manual);
    // "Battery / network aware": held back on a low battery, as the core
    // decides for the indexer. A scan reads the local disk, so the network
    // half does not come into it.
    if s.flag("power-aware", true) {
        let snap = pa::PowerSnapshot {
            source: pa::power_source(),
            battery: pa::battery_percent(),
            network: pa::NetworkCost::Unknown,
        };
        if pa::should_pause(pa::WorkerClass::Indexer, snap, pa::PowerOverrides::default()) {
            return;
        }
    }
    let now = std::time::SystemTime::now();
    let due: Vec<PathBuf> = tulipix_common::load_watched_folders()
        .into_iter()
        .filter(|dir| dir.exists())
        .filter(|dir| {
            let key = dir.display().to_string();
            let lib = Library {
                id: key.clone(),
                path: dir.clone(),
                section: Section::Photos,
                last_scan: s
                    .text(&format!("lib.scanned.{key}"))
                    .parse::<u64>()
                    .ok()
                    .map(|t| UNIX_EPOCH + Duration::from_secs(t)),
                item_count: 0,
                size_bytes: 0,
                exclude_globs: Vec::new(),
                cadence_override: cadence(&s.text(&format!("lib.cadence.{key}"))),
                realtime_notify: false,
            };
            matches!(
                tulipix_core::scheduler::due_now(&lib, default, now),
                tulipix_core::scheduler::Tick::Fire
            )
        })
        .collect();
    if due.is_empty() {
        return;
    }
    // A job you started is left alone; the next tick tries again.
    let Some(_guard) = begin("auto-rescan", "Reading folders due a rescan") else {
        return;
    };
    let total = due.len() as f64;
    for (i, dir) in due.iter().enumerate() {
        progress(i as f64 / total);
        rescan_folder(dir).await;
    }
    stamp_scanned(&due);
    refresh_counts().await;
}

/// A desktop notification, while Settings › Advanced › Notifications is on.
/// Linux's `notify-send` carries no buttons, so these say what happened and a
/// click opens nothing.
pub(crate) fn notify(title: &str, body: &str) {
    if !load().flag("notifications", true) {
        return;
    }
    // The app's mark by path: an uninstalled build has no `tulipix` icon in
    // any theme for the desktop to find by name.
    static ICON: std::sync::OnceLock<()> = std::sync::OnceLock::new();
    ICON.get_or_init(|| {
        let Some(p) = tulipix_core::paths::data_dir().map(|d| d.join("notify-icon.png")) else {
            return;
        };
        if p.exists()
            || std::fs::write(&p, include_bytes!("../../../../resources/icons/tulipix-64.png")).is_ok()
        {
            tulipix_platform::notify::set_icon(p);
        }
    });
    let id = format!("tulipix-{}", crate::api::home::now_secs());
    tulipix_platform::notify::notify(&tulipix_platform::notify::Notification::new(id, title, body));
}

// ── what each folder holds, and when it was read ────────────────────────────

fn counts_cell() -> &'static Mutex<std::collections::HashMap<String, i64>> {
    static C: std::sync::OnceLock<Mutex<std::collections::HashMap<String, i64>>> =
        std::sync::OnceLock::new();
    C.get_or_init(Default::default)
}

/// Items indexed under one watched folder, across photos, videos and music,
/// as last counted. None until the first count.
pub(crate) fn folder_items(path: &str) -> Option<i64> {
    counts_cell().lock().ok()?.get(path).copied()
}

/// Count what every watched folder holds. Done when the Libraries tab opens
/// and after every scan, not in the snapshot: a prefix match per folder per
/// database is not work for a snapshot taken every second of a download.
/// Books keep their own tables and are not in the figure.
pub(crate) async fn refresh_counts() {
    let mut pools: Vec<(&'static sqlx::SqlitePool, &str)> = Vec::new();
    if let Ok(p) = crate::db::photos_pool().await {
        pools.push((p, "photos"));
    }
    if let Ok(p) = crate::api::videos::pool().await {
        pools.push((p, "videos"));
    }
    if let Ok(p) = crate::db::music_pool().await {
        pools.push((p, "music"));
    }
    let mut map = std::collections::HashMap::new();
    for root in tulipix_common::load_watched_folders() {
        let path = root.to_string_lossy().into_owned();
        let prefix = format!(
            "{}{}%",
            path.trim_end_matches(['/', '\\']),
            std::path::MAIN_SEPARATOR
        );
        let mut n = 0i64;
        for (pool, section) in &pools {
            n += sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM items WHERE section = ? AND missing_since IS NULL \
                 AND abs_path LIKE ?",
            )
            .bind(*section)
            .bind(&prefix)
            .fetch_one(*pool)
            .await
            .unwrap_or(0);
        }
        map.insert(path, n);
    }
    if let Ok(mut g) = counts_cell().lock() {
        *g = map;
    }
}

/// When each folder was last read, whole or on its own: `lib.scanned.<path>`.
/// Nothing kept this for photos and videos; music's own per-folder stamp only
/// covers its walk.
pub(crate) fn stamp_scanned(dirs: &[PathBuf]) {
    let now = crate::api::home::now_secs().to_string();
    let mut s = load();
    for d in dirs {
        s.advanced.insert(format!("lib.scanned.{}", d.display()), now.clone());
    }
    save(s);
}

// ── one folder ──────────────────────────────────────────────────────────────

/// Read one watched folder again in every section. Books keep their own list
/// of folders and read them together, as Rescan all does.
pub(crate) async fn rescan_folder(dir: &Path) -> String {
    // A section switched Off in Settings → Sections is not scanned. This is
    // what makes Off different from Hidden: hiding Finances tidies the sidebar,
    // switching it off stops its work. Four sections share this one walk, so
    // this is the one place that decides for all of them.
    let s = load();
    let on = |id: &str| {
        tulipix_core::sections::mode_of(&s, id).works()
    };
    let mut skipped = Vec::new();
    if on("photos") {
        if let Ok(pool) = crate::db::photos_pool().await {
            crate::api::photos::scan_one(pool, dir).await;
        }
    } else {
        skipped.push("photos");
    }
    if on("videos") {
        crate::api::videos::scan_one(dir).await;
    } else {
        skipped.push("videos");
    }
    let mut trouble = Vec::new();
    if on("music") {
        if let Err(e) = crate::api::music::scan_one(dir).await {
            tracing::warn!(error = %e, dir = %dir.display(), "music rescan of one folder");
            trouble.push(format!("music ({e})"));
        }
    } else {
        skipped.push("music");
    }
    if on("books") {
        if let Ok(pool) = crate::db::books_pool().await
            && let Err(e) = crate::api::books::run_scan(pool).await
        {
            trouble.push(format!("books ({e})"));
        }
    } else {
        skipped.push("books");
    }
    let name = folder_name(dir);
    let mut out = if trouble.is_empty() {
        format!("Read {name} again.")
    } else {
        format!("Read {name} again, except {}.", trouble.join(" and "))
    };
    // Said rather than silent: a folder that "read again" but produced nothing
    // in Music is otherwise indistinguishable from a broken scan.
    if !skipped.is_empty() {
        out.push_str(&format!(" {} switched off.", skipped.join(", ")));
    }
    out
}

/// Delete every cached thumbnail of every file under `dir`: the shared cache's
/// one, and each of the photo pipeline's sizes. They are drawn again as they
/// come into view. Returns how many files had one.
///
/// Walks the disk rather than a database so all four sections are covered,
/// and keys each file exactly as its thumbnail was keyed: path, whole-second
/// mtime, size.
pub(crate) fn clear_folder_thumbs(dir: &Path) -> usize {
    let mut cleared = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else { continue };
        for e in entries.flatten() {
            let Ok(ft) = e.file_type() else { continue };
            // Not into links: one pointing back up the tree walks forever.
            if ft.is_symlink() {
                continue;
            }
            let p = e.path();
            if ft.is_dir() {
                stack.push(p);
                continue;
            }
            let Ok(meta) = e.metadata() else { continue };
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let size = meta.len();
            let key = tulipix_core::thumbs::key(&p, mtime, size);
            let mut cached: Vec<PathBuf> = tulipix_core::thumbs::thumb_path(&key).into_iter().collect();
            cached.extend(
                tulipix_photos::thumbs::SIZES
                    .iter()
                    .filter_map(|dim| tulipix_photos::thumbs::thumb_path(&p, mtime, size, *dim)),
            );
            // Every size is tried; a file counts once, however many it had.
            if cached.iter().fold(false, |any, t| std::fs::remove_file(t).is_ok() || any) {
                cleared += 1;
            }
        }
    }
    cleared
}

/// Re-index every photo into the full-text search table, so search matches the
/// library, its tags and its people again. Returns how many photos.
pub(crate) async fn rebuild_search() -> Result<usize> {
    let pool = crate::db::photos_pool().await?;
    let ids: Vec<i64> = sqlx::query_scalar(
        "SELECT id FROM items WHERE section = 'photos' AND missing_since IS NULL",
    )
    .fetch_all(pool)
    .await?;
    let total = ids.len().max(1) as f64;
    for (i, id) in ids.iter().enumerate() {
        if let Err(e) = tulipix_photos::search::index_item(pool, *id).await {
            tracing::debug!(error = %e, id, "search index");
        }
        if i % 16 == 0 {
            progress(i as f64 / total);
        }
    }
    Ok(ids.len())
}

/// Put a folder's music on one of the five shelves. The tag and the
/// audiobook flag move together, as the Books shelf's own switch does:
/// `apply_folder_sections` re-asserts the tag after every scan, so a flag set
/// alone would be undone by the next one.
pub(crate) async fn set_music_section(folder: &str, key: &str) -> Result<&'static str> {
    anyhow::ensure!(
        matches!(key, "mymusic" | "podcasts" | "audiobooks" | "radio" | "youtube"),
        "there is no shelf called “{key}”"
    );
    tulipix_common::set_folder_section(folder, key);
    let pool = crate::db::music_pool().await?;
    tulipix_music::audiobooks::set_folder_flag(
        pool,
        folder.trim_end_matches(['/', '\\']),
        key == "audiobooks",
    )
    .await?;
    Ok(tulipix_common::music_section_label(key))
}

// ── export and import ───────────────────────────────────────────────────────

/// The library as portable JSON: every item in photos, videos and music, and
/// what was done with them -- photo albums, tags you gave and stars; music
/// playlists, loved tracks and ratings. Paths and rows, never the files. The
/// Slint build's Export wrote the envelope with nothing in it.
pub(crate) async fn export_library() -> Result<PathBuf> {
    use tulipix_core::export::{self, Section, SectionDump};
    let mut env = export::empty();
    if let Ok(pool) = crate::db::photos_pool().await {
        let mut relationships = grouped(
            pool,
            "album",
            "SELECT a.name, ai.item_id FROM album_items ai JOIN albums a ON a.id = ai.album_id \
             ORDER BY a.id, ai.sort_key",
        )
        .await;
        relationships.extend(
            grouped(
                pool,
                "tag",
                "SELECT t.name, it.item_id FROM item_tags it JOIN tags t ON t.id = it.tag_id \
                 WHERE it.source = 'user' ORDER BY t.name",
            )
            .await,
        );
        relationships.extend(
            grouped(pool, "rating", "SELECT 'starred', item_id FROM photo_meta WHERE starred = 1")
                .await,
        );
        let dump = SectionDump { items: items(pool, "photos").await?, relationships };
        export::add_section(&mut env, Section::Photos, dump);
    }
    if let Ok(pool) = crate::api::videos::pool().await {
        let dump = SectionDump { items: items(pool, "videos").await?, relationships: Vec::new() };
        export::add_section(&mut env, Section::Videos, dump);
    }
    if let Ok(pool) = crate::db::music_pool().await {
        let mut relationships = grouped(
            pool,
            "playlist",
            "SELECT p.name, pi.item_id FROM playlist_items pi JOIN playlists p ON p.id = pi.playlist_id \
             WHERE p.is_smart = 0 ORDER BY p.id, pi.position",
        )
        .await;
        relationships.extend(
            grouped(pool, "loved", "SELECT 'loved', item_id FROM track_meta WHERE loved = 1").await,
        );
        relationships.extend(
            grouped(
                pool,
                "rating",
                "SELECT CAST(rating AS TEXT) || ' stars', item_id FROM track_meta \
                 WHERE rating > 0 ORDER BY rating",
            )
            .await,
        );
        let dump = SectionDump { items: items(pool, "music").await?, relationships };
        export::add_section(&mut env, Section::Music, dump);
    }
    let dir = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory on this system"))?
        .join("exports");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("tulipix-export-{}.json", crate::api::home::now_secs()));
    std::fs::write(&file, export::to_json(&env)?)?;
    Ok(file)
}

async fn items(pool: &sqlx::SqlitePool, section: &str) -> Result<Vec<tulipix_core::export::ItemRow>> {
    let rows: Vec<(i64, String, Option<String>, i64, i64, i64, Option<i64>)> = sqlx::query_as(
        "SELECT id, abs_path, sha256, size, mtime, added, missing_since FROM items \
         WHERE section = ? ORDER BY id",
    )
    .bind(section)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, abs_path, sha256, size, mtime, added, missing_since)| {
            tulipix_core::export::ItemRow { id, abs_path, sha256, size, mtime, added, missing_since }
        })
        .collect())
}

/// `(label, item_id)` rows, ordered by label, folded into one relationship per
/// run of the same label. A table a database does not have yet is no
/// relationships, not a failed export. `'static`: sqlx 0.9 takes a query only
/// as a literal, or wrapped in `AssertSqlSafe`.
async fn grouped(pool: &sqlx::SqlitePool, kind: &str, sql: &'static str) -> Vec<tulipix_core::export::Relationship> {
    let rows: Vec<(String, i64)> = sqlx::query_as(sql).fetch_all(pool).await.unwrap_or_default();
    let mut out: Vec<tulipix_core::export::Relationship> = Vec::new();
    for (label, id) in rows {
        match out.last_mut() {
            Some(r) if r.label == label => r.item_ids.push(id),
            _ => out.push(tulipix_core::export::Relationship {
                kind: kind.into(),
                label,
                item_ids: vec![id],
            }),
        }
    }
    out
}

/// Bring in what another app knew. Picasa stars land on the matching photos
/// and iTunes play counts and ratings on the matching tracks; Plex, Lightroom
/// and foobar2000 are recognised and said so, because their importers are not
/// written yet. A port of the Slint build's Import, minus its file dialog --
/// the page picks the file.
pub(crate) async fn import_from(path: &Path) -> String {
    use tulipix_core::migration::{self, SourceKind};
    let Some(kind) = SourceKind::detect(path) else {
        return "That is not a file Tulipix can import. It takes a Plex library.db, a \
                .picasa.ini, an iTunes Library.xml, a Lightroom .lrcat or a foobar2000 \
                library."
            .into();
    };
    let plan = match migration::dry_run(kind, path) {
        Ok(p) => p,
        Err(e) => return format!("Could not read it: {e}"),
    };
    for w in &plan.warnings {
        tracing::warn!(%w, "import");
    }
    match kind {
        SourceKind::Picasa => {
            let n = picasa_stars(path).await;
            format!(
                "Picasa: {} photos listed, {} starred. {n} of the stars landed on photos in \
                 your library.",
                plan.photos, plan.ratings
            )
        }
        SourceKind::ITunesXml => {
            let merged = match (std::fs::read_to_string(path), crate::db::music_pool().await) {
                (Ok(xml), Ok(pool)) => {
                    let tracks = tulipix_music::import::parse_itunes(&xml);
                    tulipix_music::import::merge(pool, &tracks).await.unwrap_or(0)
                }
                _ => 0,
            };
            format!(
                "iTunes: {} tracks and {} playlists. Play counts and ratings merged into \
                 {merged} of your tracks.",
                plan.music_tracks, plan.playlists
            )
        }
        SourceKind::Plex => "A Plex library. Tulipix recognises it, but cannot import from it yet.".into(),
        SourceKind::Lightroom => {
            "A Lightroom catalog. Tulipix recognises it, but cannot import its collections yet.".into()
        }
        SourceKind::Foobar2000 => {
            "A foobar2000 library. Tulipix recognises it, but cannot read its format yet.".into()
        }
    }
}

/// Picasa's `star=yes`: each starred section is a bare file name, matched in
/// the .ini's own folder first and then by name anywhere. Against `abs_path`,
/// the column the photos database has.
async fn picasa_stars(ini: &Path) -> u32 {
    let Ok(text) = std::fs::read_to_string(ini) else { return 0 };
    let dir = ini.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut starred = Vec::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if let Some(inner) = t.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
            current = Some(inner.to_string());
            continue;
        }
        if t.starts_with("star=yes")
            && let Some(name) = &current
        {
            starred.push(name.clone());
        }
    }
    let Ok(pool) = crate::db::photos_pool().await else { return 0 };
    let mut n = 0;
    for name in starred {
        let exact = dir.join(&name).to_string_lossy().into_owned();
        let res = sqlx::query(
            "UPDATE photo_meta SET starred = 1 WHERE item_id IN \
             (SELECT id FROM items WHERE abs_path = ?1 \
              OR abs_path LIKE '%/' || ?2 OR abs_path LIKE '%\\' || ?2)",
        )
        .bind(&exact)
        .bind(&name)
        .execute(pool)
        .await;
        if let Ok(r) = res
            && r.rows_affected() > 0
        {
            n += 1;
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The service name is text from the page. Anything outside the five is
    /// refused before the keychain is touched.
    #[test]
    fn only_the_five_services_reach_the_keychain() {
        for s in ["", "github_oauth", "ytdlp_cookies", "tmdb/../x"] {
            assert!(store_key(s, "k").contains("not a service"), "{s} was let through");
            assert!(remove_key(s).contains("not a service"), "{s} was let through");
        }
    }
}
