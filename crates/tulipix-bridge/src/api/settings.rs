//! Settings: nine tabs over one `Settings` struct.
//!
//! Six of them are data-driven -- a list of rows, each a toggle, a text box, a
//! read-only status line or an action button, keyed by a stable setting id. New
//! settings need no schema change and no new command: they are a row in the
//! list below and a key in `flags` or `advanced`.
//!
//! The row *definitions* are the port. They live in the Slint build's
//! `main.rs`, which is a binary this cannot link, so they are transcribed here
//! -- same keys, same defaults, same copy. A key that drifted would silently
//! split one setting into two.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::api::shell::{load, put, save};

/// One row of a settings panel.
///
/// `kind` is what to draw: "header" | "toggle" | "text" | "status" | "action"
/// | "choice". `state` tints a status line: "ok" | "warn" | "error" | "busy" |
/// "muted".
pub struct SettingItem {
    pub key: String,
    pub kind: String,
    pub label: String,
    pub desc: String,
    /// The text value, the action's button label, or the status line's reading.
    pub value: String,
    pub on: bool,
    pub state: String,
    /// Choice rows only: what `value` may be set to.
    pub options: Vec<String>,
    /// "status-action" rows only: the trailing button's label. A status row
    /// and its Download button are one row, not two, because they describe
    /// one thing and drifted apart the moment they were separate.
    pub btn: String,
    /// 0..1 while a download is running, so the row can draw a determinate
    /// bar. Negative means "no progress to show".
    pub frac: f64,
}

/// One watched folder.
pub struct LibraryRow {
    pub path: String,
    pub exists: bool,
    /// Which sections found something under it.
    pub sections: Vec<String>,
}

/// The Home layout's card list — one switch per card.
pub struct HomeCardRow {
    pub key: String,
    pub label: String,
    pub on: bool,
}

/// One backup under `<data>/backups`. `id` is its folder name, the Unix time it
/// was made, and the only thing a Restore is allowed to name.
pub struct BackupRow {
    pub id: String,
    pub secs: i64,
    pub files: u32,
    pub bytes: u64,
}

pub struct SettingsState {
    /// "profile" | "libraries" | "playback" | "services" | "ai" | "security"
    /// | "data" | "advanced" | "status".
    pub tab: String,
    // You & Home.
    pub display_name: String,
    pub avatar_emoji: String,
    /// The cropped profile photo, or empty -- the emoji stands in.
    pub avatar_path: String,
    /// The cropped header cover, or empty -- the gradient stands in.
    pub cover_path: String,
    pub logo_choice: i32,
    pub theme: String,
    pub reduce_motion: bool,
    /// "classic" | "welcome" | "cinema" | "stream".
    pub home_layout: String,
    pub home_cards: Vec<HomeCardRow>,
    pub home_music_left: bool,
    pub app_version: String,
    // Libraries.
    pub libraries: Vec<LibraryRow>,
    pub default_cadence: String,
    // The data-driven panels.
    pub playback: Vec<SettingItem>,
    /// Library analysis: tracks measured, tracks there are to measure, and
    /// whether a pass is running now.
    pub analysed: i64,
    pub analysable: i64,
    pub analysing: bool,
    pub services: Vec<SettingItem>,
    pub ai: Vec<SettingItem>,
    /// The "Requirements" sheet behind the AI panel: what each model asks of
    /// this machine. Built off the same manifest as `ai`, so the two lists
    /// can never disagree about which models exist.
    pub ai_requirements: Vec<SettingItem>,
    pub security: Vec<SettingItem>,
    pub data: Vec<SettingItem>,
    /// Newest first.
    pub backups: Vec<BackupRow>,
    pub data_path: String,
    pub advanced: Vec<SettingItem>,
    /// What the last action did. Cleared by the next refresh.
    pub notice: String,
}

pub enum SettingsCmd {
    Refresh,
    SetTab { tab: String },
    Toggle { key: String, on: bool },
    SetText { key: String, value: String },
    /// A button row. The key names what to do; unknown keys are ignored rather
    /// than crashing a settings page.
    Action { key: String },
    /// `avatar` / `cover`: None leaves the picture alone, empty bytes remove
    /// it, anything else is a PNG the page already cropped.
    SaveProfile { name: String, emoji: String, logo: i32, avatar: Option<Vec<u8>>, cover: Option<Vec<u8>> },
    SetTheme { theme: String },
    SetReduceMotion { on: bool },
    SetHomeLayout { layout: String },
    HomeCardSet { key: String, on: bool },
    HomeCardsReset,
    LibAdd { path: String },
    LibRemove { path: String },
}

pub async fn settings_dispatch(cmd: SettingsCmd) -> Result<SettingsState> {
    let mut notice = String::new();
    match cmd {
        SettingsCmd::Refresh => {}
        SettingsCmd::SetTab { tab } => set_tab(tab),
        SettingsCmd::Toggle { key, on } => {
            let mut s = load();
            s.flags.insert(key, on);
            save(s);
        }
        SettingsCmd::SetText { key, value } => match key.as_str() {
            // Two settings are real fields on the struct rather than map
            // entries, and writing them into `advanced` would store a key
            // nothing reads.
            "idle_lock_secs" => {
                let mut s = load();
                s.idle_lock_secs = value.trim().parse().unwrap_or(0);
                save(s);
            }
            // The "Lock after" choice: a label, stored as the seconds it means.
            "lock.after" => {
                if let Some((_, secs)) = LOCK_AFTER.iter().find(|(l, _)| *l == value) {
                    let mut s = load();
                    s.idle_lock_secs = *secs;
                    save(s);
                }
            }
            // Never stored as text: the core salts and hashes it.
            "lock.pin" => {
                let pin = value.trim();
                notice = if (4..=8).contains(&pin.len()) && pin.chars().all(|c| c.is_ascii_digit()) {
                    tulipix_core::account::set_pin(pin);
                    crate::api::lock::remember_pin_len(pin.len());
                    "PIN set. The lock screen asks for it from now on.".into()
                } else {
                    "A PIN is four to eight digits.".into()
                };
            }
            _ => put(&key, &value),
        },
        SettingsCmd::Action { key } => notice = action(&key).await,
        SettingsCmd::SaveProfile { name, emoji, logo, avatar, cover } => {
            crate::api::shell::shell_dispatch(crate::api::shell::ShellCmd::SaveProfile {
                name,
                emoji,
                logo,
            })
            .await?;
            if let Some(png) = avatar {
                crate::api::shell::store_picture("avatar.png", &png)?;
            }
            if let Some(png) = cover {
                crate::api::shell::store_picture("cover.png", &png)?;
            }
        }
        SettingsCmd::SetTheme { theme } => {
            let mut s = load();
            s.theme = theme;
            save(s);
        }
        SettingsCmd::SetReduceMotion { on } => {
            let mut s = load();
            s.reduce_motion = on;
            save(s);
        }
        SettingsCmd::SetHomeLayout { layout } => put(HOME_LAYOUT_KEY, &layout),
        SettingsCmd::HomeCardSet { key, on } => {
            let mut s = load();
            s.flags.insert(format!("{HOME_CARD_PREFIX}{key}"), on);
            save(s);
        }
        SettingsCmd::HomeCardsReset => {
            let mut s = load();
            s.flags.retain(|k, _| !k.starts_with(HOME_CARD_PREFIX));
            save(s);
        }
        SettingsCmd::LibAdd { path } => {
            notice = match add_watched(Path::new(&path)) {
                true => format!("Watching {path}."),
                false => format!("{path} is already watched, or is not a folder."),
            };
        }
        SettingsCmd::LibRemove { path } => {
            remove_watched(Path::new(&path));
            notice = format!("Stopped watching {path}.");
        }
    }
    let mut state = snapshot().await;
    state.notice = notice;
    Ok(state)
}

// ── the snapshot ────────────────────────────────────────────────────────────

fn tab_cell() -> &'static std::sync::Mutex<String> {
    static T: std::sync::OnceLock<std::sync::Mutex<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| std::sync::Mutex::new("profile".into()))
}

fn set_tab(v: String) {
    if let Ok(mut g) = tab_cell().lock() {
        *g = v;
    }
}

const HOME_LAYOUT_KEY: &str = "home.layout";
const HOME_CARD_PREFIX: &str = "home.card.";

/// Every card the classic Home layout can draw, in the order it draws them.
const HOME_CARDS: [(&str, &str); 12] = [
    ("hero", "Greeting"),
    ("continue", "Continue"),
    ("player", "Music player"),
    ("quick", "Quick actions"),
    ("photos", "Photos"),
    ("videos", "Videos"),
    ("music", "Music"),
    ("books", "Books"),
    ("cloud", "Cloud"),
    ("tools", "Tools"),
    ("transfer", "Transfer"),
    ("finances", "Finances"),
];

/// The card keys Settings left switched on, for whichever Home layout is
/// drawing. Every card defaults to on: a fresh install shows the whole page,
/// and the switches are there to take things away.
pub(crate) fn enabled_cards(s: &tulipix_core::settings::Settings) -> Vec<String> {
    HOME_CARDS
        .iter()
        .filter(|(k, _)| s.flag(&format!("{HOME_CARD_PREFIX}{k}"), true))
        .map(|(k, _)| (*k).to_string())
        .collect()
}

async fn snapshot() -> SettingsState {
    let s = load();
    let (analysed, analysable, analysing) = crate::api::music::analyse_progress().await;
    SettingsState {
        tab: tab_cell().lock().map(|g| g.clone()).unwrap_or_else(|_| "profile".into()),
        display_name: s.text("profile.name"),
        avatar_emoji: s.text("profile.emoji"),
        avatar_path: crate::api::shell::picture("avatar.png"),
        cover_path: crate::api::shell::picture("cover.png"),
        logo_choice: s.text("profile.logo").parse().unwrap_or(0),
        theme: s.theme.clone(),
        reduce_motion: s.reduce_motion,
        home_layout: match s.text(HOME_LAYOUT_KEY) {
            l if l.is_empty() => "classic".into(),
            l => l,
        },
        home_cards: HOME_CARDS
            .iter()
            .map(|(key, label)| HomeCardRow {
                key: (*key).to_string(),
                label: (*label).to_string(),
                on: s.flag(&format!("{HOME_CARD_PREFIX}{key}"), true),
            })
            .collect(),
        home_music_left: s.flag("home.music-left", false),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        libraries: libraries(),
        default_cadence: match s.text("scan.cadence") {
            c if c.is_empty() => "manual".into(),
            c => c,
        },
        playback: playback(&s),
        analysed,
        analysable,
        analysing,
        services: services(&s),
        ai: ai(&s),
        ai_requirements: ai_requirement_rows(),
        security: security(&s),
        data: data(&s),
        backups: backups(),
        data_path: tulipix_core::paths::data_dir()
            .map(|d| d.display().to_string())
            .unwrap_or_default(),
        advanced: advanced(&s),
        notice: String::new(),
    }
}

// ── the panels ──────────────────────────────────────────────────────────────
//
// Transcribed from the Slint build's `wire_settings_panels`, key for key. The
// helpers below are the same four it uses, so a row reads the same on both
// sides and a copy edit is a one-line diff rather than a rewrite.

type S = tulipix_core::settings::Settings;

fn si(key: &str, kind: &str, label: &str, desc: &str, value: &str, on: bool, state: &str) -> SettingItem {
    SettingItem {
        key: key.into(),
        kind: kind.into(),
        label: label.into(),
        desc: desc.into(),
        value: value.into(),
        on,
        state: state.into(),
        options: Vec::new(),
        btn: String::new(),
        frac: -1.0,
    }
}

fn hdr(label: &str) -> SettingItem {
    si("", "header", label, "", "", false, "")
}

fn tog(s: &S, key: &str, def: bool, label: &str, desc: &str) -> SettingItem {
    si(key, "toggle", label, desc, "", s.flag(key, def), "")
}

fn txt(s: &S, key: &str, label: &str, desc: &str) -> SettingItem {
    si(key, "text", label, desc, &s.text(key), false, "")
}

fn stat(label: &str, value: &str, state: &str) -> SettingItem {
    si("", "status", label, "", value, false, state)
}

fn act(key: &str, label: &str, desc: &str, btn: &str) -> SettingItem {
    si(key, "action", label, desc, btn, false, "")
}

/// A status reading and its action button in one row. `value` is the reading
/// ("Installed", "Downloading"), `btn` the button ("Download", "Verify").
fn statact(key: &str, label: &str, desc: &str, value: &str, state: &str, btn: &str) -> SettingItem {
    let mut r = si(key, "status-action", label, desc, value, false, state);
    r.btn = btn.into();
    r
}

fn choice(s: &S, key: &str, label: &str, desc: &str, options: &[&str]) -> SettingItem {
    let mut row = si(key, "choice", label, desc, &s.text(key), false, "");
    row.options = options.iter().map(|o| (*o).to_string()).collect();
    if row.value.is_empty() {
        row.value = options.first().map(|o| (*o).to_string()).unwrap_or_default();
    }
    row
}

fn playback(s: &S) -> Vec<SettingItem> {
    vec![
        hdr("SUBTITLES"),
        txt(s, "playback.sub-size", "Subtitle size", "In pixels — 28 if left blank"),
        txt(s, "playback.sub-color", "Subtitle colour", "A colour code like #ffffff — applies on the next play"),
        stat("Subtitles next to the video", "Loaded automatically (.srt / .vtt / .ass)", "ok"),
        hdr("VIDEO"),
        tog(s, "playback.interpolation", false, "Smoother motion", "Frame interpolation — can be heavy on laptop graphics"),
        tog(s, "playback.upscale", false, "Upscale shaders (Anime4K)", "Sharper upscaling — drop .glsl shader files in the folder below"),
        stat("Keep display awake", "While a video plays", "ok"),
        hdr("AUDIO & MUSIC"),
        tog(s, "playback.audio-exclusive", false, "Exclusive audio output", "Bit-perfect output straight to the audio device — silences other apps"),
        txt(s, "music.eq-preset", "Music equalizer preset", "flat · rock · pop · jazz · bass · treble — applies on the next track"),
        tog(s, "music.autoplay", false, "Keep playing when the queue ends",
            "Builds a run from the last few tracks — their artists, genres, tempo and key — drawn only from your own library. Needs the analysis below to have run"),
        hdr("LIBRARY ANALYSIS"),
        act("music-analyse", "Measure tempo, key and dynamic range",
            "Reads every track once to work out its BPM, musical key, how compressed it is, and what it sounds like — which is what song matching, duplicate detection and keep-playing all read.              Runs in the background and can be stopped; already-measured tracks are skipped",
            "Analyse"),
        act("music-analyse-stop", "Stop measuring",
            "Finishes the track it is on and leaves the rest for next time", "Stop"),
    ]
}

fn services(s: &S) -> Vec<SettingItem> {
    vec![
        hdr("OPTIONAL SERVICE KEYS"),
        txt(s, "api.tmdb", "TMDB API key", "Movie and show artwork, cast and summaries. Free key from themoviedb.org"),
        txt(s, "api.spotify-id", "Spotify client ID", "Better music search and recommendations. Free key from developer.spotify.com"),
        txt(s, "api.spotify-secret", "Spotify client secret", "Goes together with the client ID above"),
        txt(s, "api.youtube-data", "YouTube API key", "Richer YouTube search results and video details"),
        txt(s, "api.piped-instance", "YouTube backend server (Piped)", "The server used to browse YouTube. Leave blank for the default"),
        hdr("EXTRA METADATA SOURCES"),
        tog(s, "api.discogs", false, "Discogs", "Extra music metadata when MusicBrainz has no match"),
        tog(s, "api.anidb", false, "AniDB", "Anime titles, episodes, and ratings"),
        tog(s, "api.anilist", false, "AniList", "A second anime source when AniDB misses"),
        tog(s, "api.subscene", false, "Subscene / Addic7ed", "Backup subtitle sources when OpenSubtitles is busy"),
        tog(s, "api.trakt", false, "Trakt.tv", "Track what you watch with a Trakt account"),
        tog(s, "api.listenbrainz", false, "ListenBrainz", "Scrobble played music — the open Last.fm alternative"),
        hdr("SELF-HOSTED SERVERS (ADVANCED)"),
        txt(s, "api.update-channel", "Update server", "Only needed for mirrors or offline networks"),
        txt(s, "api.sentry", "Crash-report server", "Send crash reports to your own Sentry server"),
        txt(s, "api.nominatim", "Place-name server", "Turns photo GPS coordinates into place names"),
        txt(s, "api.radio-browser", "Radio station server", "Mirror for the internet-radio directory"),
        txt(s, "api.autoeq", "Headphone EQ database", "Mirror for AutoEq headphone profiles"),
        txt(s, "api.tmdb-image-base", "Poster artwork server", "Mirror for movie and show artwork"),
    ]
}

fn ai(s: &S) -> Vec<SettingItem> {
    // Manifest-driven: ONE row per model -- status dot and Download/Verify
    // button together. Two separate rows is what the Slint build started with,
    // and the pair drifted the first time a model was renamed.
    let mut rows = vec![hdr("ON-DEVICE MODELS")];
    for m in &ai_manifest().models {
        let installed = tulipix_photos::ai::models::is_installed(m);
        let mb = m.size_bytes / 1_000_000;
        let (nice, purpose) = model_display(&m.name, &m.cap);
        let dl = ai_dl_progress().lock().ok().and_then(|g| g.get(m.name.as_str()).copied());
        let mut row = statact(
            &format!("ai-dl-{}", m.name),
            &format!("{nice} · {mb} MB"),
            purpose,
            if dl.is_some() { "Downloading" } else if installed { "Installed" } else { "Not downloaded" },
            if dl.is_some() { "busy" } else if installed { "ok" } else { "muted" },
            if installed { "Verify" } else { "Download" },
        );
        if let Some(f) = dl {
            row.frac = f as f64;
        }
        rows.push(row);
    }
    {
        let mut chk = act("ai-update-check", "Check for model updates",
            "Compare installed models against the latest versions", "Check");
        if AI_CHECK_BUSY.load(std::sync::atomic::Ordering::Relaxed) {
            chk.state = "busy".into();
        }
        rows.push(chk);
    }
    rows.extend([
        hdr("VOICE RECOGNITION"),
        choice(s, "ai.voice-lang", "Spoken language",
            "What the mic listens for — pinning a language beats auto-detect on short clips",
            &["en", "hi", "de", "fr", "es", "ru", "it", "auto"]),
        choice(s, "ai.model.voice", "Voice search",
            "Used by the mic button in search fields — Tiny answers fastest",
            &["tiny", "base", "small", "turbo"]),
        choice(s, "ai.model.transcribe", "Transcribe & subtitles",
            "Used by the Tools transcriber and video subtitles — bigger models catch more words",
            &["tiny", "base", "small", "turbo"]),
        stat("Model sizes", "Tiny bundled · Base 60 MB · Small 190 MB · Turbo 574 MB", "muted"),
        hdr("FEATURES"),
        tog(s, "ai.captions", false, "Describe photos automatically", "Writes captions and alt-text for new photos as they are added"),
        tog(s, "ai.voice", true, "Voice search", "The mic button in search bars — speak instead of typing"),
        tog(s, "ai.chat", false, "Chat assistant (Ctrl+J)", "Ask questions about your library in plain language"),
        hdr("BOOK READ-ALOUD"),
        tog(s, "books.tts.neural", true, "Use neural voice (Kokoro)",
            "Natural AI voice for the reader's Read Aloud. Off = robotic espeak voice (no model, needs espeak-ng)"),
        hdr("SCROBBLING"),
        tog(s, "api.listenbrainz", false, "Send listens to ListenBrainz",
            "The open listening record. Plays are queued while you are offline and sent when you are back"),
        txt(s, "api.listenbrainz-token", "ListenBrainz token",
            "From listenbrainz.org → Settings. Nothing is sent until this is filled in"),
        act("scrobble-flush", "Send queued listens now",
            "Plays are queued while you are offline and go out with the next one. This sends them immediately",
            "Send"),
        hdr("CLOUD"),
        tog(s, "ai.cloud-offload", false, "Allow cloud AI help", "Send selected questions to a cloud AI service. Off = everything stays on-device"),
    ]);
    rows
}

// ── AI model manifest ───────────────────────────────────────────────────────

/// Compiled-in AI model manifest (resources/ai-models.toml) -- parsed once.
/// The SAME file the Slint build reads, by `include_str!` rather than a copy:
/// two transcriptions of a model list is two lists that disagree.
fn ai_manifest() -> &'static tulipix_core::ai_models::Manifest {
    static M: std::sync::OnceLock<tulipix_core::ai_models::Manifest> = std::sync::OnceLock::new();
    M.get_or_init(|| {
        tulipix_core::ai_models::Manifest::from_toml(include_str!(
            "../../../../resources/ai-models.toml"
        ))
        .unwrap_or_else(|e| {
            tracing::error!(error = %e, "ai-models.toml parse");
            Default::default()
        })
    })
}

/// Human name + purpose for a manifest model, keyed off its capability gate so
/// the row says what the model DOES rather than naming its file.
fn model_display<'a>(name: &'a str, cap: &str) -> (&'a str, &'static str) {
    match cap {
        "photos.ai.faces" => ("Face detection", "Finds faces in photos so people can be grouped"),
        "photos.ai.heal" => ("Magic eraser", "Removes unwanted objects in the photo editor"),
        "photos.ai.sky" => ("Smart select", "Selects sky / objects for one-tap edits"),
        "voice.balanced" => ("Whisper Base (balanced)", "Good accuracy at near-instant speed — best all-rounder"),
        "voice.accurate" => ("Whisper Small (accurate)", "Catches names and accents — great for subtitles"),
        "voice.best" => ("Whisper Turbo (best)", "Top accuracy for dictation-grade transcription"),
        _ => (name, ""),
    }
}

/// "Check for model updates" in flight -- renders the button as a busy pill.
static AI_CHECK_BUSY: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Live model-download progress (model name → 0..1). The download runs
/// detached, so this map is how a later `Refresh` learns how far it got --
/// there is no stream back to Dart and a download does not need one.
fn ai_dl_progress() -> &'static std::sync::Mutex<std::collections::HashMap<String, f32>> {
    static M: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f32>>> =
        std::sync::OnceLock::new();
    M.get_or_init(Default::default)
}

/// Per manifest entry: up-to-date, an older install awaiting update, or absent.
fn ai_update_summary() -> String {
    let (mut current, mut missing) = (0, 0);
    let mut stale: Vec<String> = Vec::new();
    let root = tulipix_photos::ai::models::models_root();
    for m in &ai_manifest().models {
        if tulipix_photos::ai::models::is_installed(m) {
            current += 1;
            continue;
        }
        // Any older "<name>-<version>" install dir → an update, not a gap.
        let older = root
            .as_deref()
            .and_then(|r| std::fs::read_dir(r).ok())
            .into_iter()
            .flatten()
            .flatten()
            .any(|e| e.file_name().to_string_lossy().starts_with(&format!("{}-", m.name)));
        if older {
            stale.push(format!("{} → v{}", m.name, m.version));
        } else {
            missing += 1;
        }
    }
    if stale.is_empty() {
        format!("Models: {current} up-to-date · {missing} not installed — nothing to update.")
    } else {
        format!("Updates available: {} · {current} current · {missing} not installed.", stale.join(", "))
    }
}

/// Installed RAM in MB, or 0 where we cannot tell -- a machine that cannot be
/// measured gets a neutral row rather than a guess.
fn total_ram_mb() -> u64 {
    #[cfg(target_os = "linux")]
    {
        std::fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|s| {
                s.lines()
                    .find(|l| l.starts_with("MemTotal:"))
                    .and_then(|l| l.split_whitespace().nth(1))
                    .and_then(|kb| kb.parse::<u64>().ok())
                    .map(|kb| kb / 1024)
            })
            .unwrap_or(0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        0
    }
}

/// One row per on-device model saying what it wants from this machine.
///
/// The RAM figure is a recommendation, not a measurement: weights have to be
/// resident and the runtime needs working buffers on top, which in practice
/// lands near three times the file -- floored at 512 MB so a tiny model does
/// not read as free.
fn ai_requirement_rows() -> Vec<SettingItem> {
    let ram = total_ram_mb();
    let mut rows = vec![hdr("THIS COMPUTER")];
    rows.push(stat(
        "Installed memory",
        &if ram > 0 { format!("{:.1} GB", ram as f64 / 1024.0) } else { "Unknown".to_string() },
        if ram == 0 { "muted" } else if ram >= 8192 { "ok" } else { "warn" },
    ));
    rows.push(stat(
        "Graphics acceleration",
        if cfg!(feature = "ai-onnx") {
            "ONNX runtime compiled in — GPU used when available"
        } else {
            "CPU only in this build"
        },
        if cfg!(feature = "ai-onnx") { "ok" } else { "muted" },
    ));

    rows.push(hdr("ON-DEVICE MODELS"));
    for m in &ai_manifest().models {
        let mb = m.size_bytes / 1_000_000;
        let need = (mb * 3).max(512);
        let (nice, purpose) = model_display(&m.name, &m.cap);
        // What the row asks for, in the order it matters: memory, then disk,
        // then whether a GPU is required or merely welcome.
        let gpu = if m.min_vram_mb > 0 {
            format!(" · {} MB VRAM", m.min_vram_mb)
        } else {
            String::new()
        };
        rows.push(si(
            "",
            "status",
            nice,
            purpose,
            &format!("{need} MB RAM · {mb} MB disk{gpu}"),
            false,
            if ram == 0 { "muted" } else if ram >= need { "ok" } else { "warn" },
        ));
    }

    rows.push(hdr("NOTES"));
    rows.push(stat("Only what you use is loaded", "Models load on demand and unload after", "muted"));
    rows.push(stat("Transcription is CPU-heavy", "Expect roughly real-time on four cores", "muted"));
    rows
}

/// "Lock after": each choice and the seconds it means.
const LOCK_AFTER: [(&str, u64); 7] = [
    ("1 minute", 60),
    ("2 minutes", 120),
    ("5 minutes", 300),
    ("10 minutes", 600),
    ("15 minutes", 900),
    ("30 minutes", 1800),
    ("1 hour", 3600),
];

/// The choice nearest `secs`, so a value typed into the old seconds box that
/// is not on the list still shows as something rather than as nothing.
fn lock_after_label(secs: u64) -> &'static str {
    LOCK_AFTER
        .iter()
        .min_by_key(|(_, s)| s.abs_diff(secs))
        .map(|(l, _)| *l)
        .unwrap_or("10 minutes")
}

fn security(s: &S) -> Vec<SettingItem> {
    let has_pin = tulipix_core::account::has_pin();
    let mut after = choice(s, "lock.after", "Lock after",
        "How long without a key press or the pointer moving. A playing video never locks",
        &LOCK_AFTER.map(|(l, _)| l));
    after.value = lock_after_label(crate::api::lock::idle_secs(s)).into();
    let mut rows = vec![
        hdr("LOCK"),
        tog(s, "autolock", false, "Lock when idle",
            "Shows the lock screen after a while without input, or now with Ctrl+L. Music keeps playing"),
        after,
        statact("lock-pin", "PIN",
            "Four to eight digits, asked for on the lock screen. Without one, a click unlocks",
            if has_pin { "Set" } else { "Not set" },
            if has_pin { "ok" } else { "muted" },
            if has_pin { "Change" } else { "Set a PIN" }),
    ];
    if has_pin {
        rows.push(act("lock-pin-clear", "Remove the PIN",
            "The lock screen goes back to unlocking with a click", "Remove"));
    }
    rows.extend([
        hdr("ON THE LOCK SCREEN"),
        tog(s, "lock.show-music", true, "What's playing", "Artwork, title and artist, lit in the cover's colours"),
        tog(s, "lock.media-controls", true, "Music controls without unlocking", "Play, pause, skip and love from the lock screen"),
        tog(s, "lock.show-lyrics", true, "Lyrics", "The synced line at the playhead, when the track has them"),
        tog(s, "lock.show-video", true, "A paused video", "Its frame and title. Resuming asks for the PIN"),
        tog(s, "lock.show-glance", true, "Glance cards", "Continue watching, bills due (a count, never amounts) and the library, when nothing is playing"),
        tog(s, "lock.motion", true, "Moving smoke",
            "A GPU shader drawn at a third of the window's size, twenty frames a second. Off: one still frame"),
        txt(s, "lock.wallpapers", "Wallpapers folder",
            "Pictures for when nothing is playing — the first ten, one every twelve seconds. Blank = the gradient"),
        act("lock-wallpapers-browse", "Choose the wallpapers folder", "Opens a folder picker", "Choose"),
        hdr("UNLOCK"),
        tog(s, "passkey", false, "Unlock with a passkey", "Use a security key or fingerprint instead of a password"),
        hdr("ENCRYPTION"),
        tog(s, "db-encrypt", false, "Encrypt the library database", "Protects your library index if the disk is stolen — applies on next launch"),
    ]);
    rows
}

fn data(s: &S) -> Vec<SettingItem> {
    vec![
        hdr("BACKUP"),
        act("backup", "Back up settings", "Saves your settings and folder list so you can restore them later", "Back up"),
        hdr("PROBLEMS"),
        act("open-logs", "Open the log folder", "tracing JSON logs with daily rotation", "Open"),
        act("open-data", "Open the data folder", "Where the section databases live", "Open"),
        tog(s, "multi-user", false, "Separate library per computer user", "Each OS account gets its own Tulipix library"),
    ]
}

fn advanced(s: &S) -> Vec<SettingItem> {
    let cache_mb = tulipix_core::thumbs::cache_size().map(|b| b / (1024 * 1024)).unwrap_or(0);
    vec![
        hdr("PERFORMANCE"),
        tog(s, "power-aware", true, "Battery / network aware", "Pause background scanning on battery or metered connections"),
        stat("Thumbnail cache", &format!("{cache_mb} MB"), "muted"),
        act("clear-thumbs", "Clear the thumbnail cache", "Every thumbnail is redrawn the next time it is needed", "Clear"),
        hdr("BUNDLED TOOLS"),
        txt(s, "tools.bin-dir", "Tools directory",
            "Folder holding yt-dlp, ffmpeg… — checked before bundled + PATH (blank = off)"),
        tool_row("ffmpeg"),
        tool_row("ffprobe"),
        tool_row("rclone"),
        tool_row("yt-dlp"),
        tog(s, tulipix_core::ytdlp::AUTO_UPDATE_FLAG, true, "Keep yt-dlp up to date",
            "Checks weekly. Sites change constantly and a yt-dlp a few weeks old \
             starts failing downloads with 403"),
        txt(s, "ytdlp.player-clients", "yt-dlp player clients",
            "Advanced, blank = yt-dlp's own defaults. Only set this if a yt-dlp \
             issue tells you to, e.g. default,tv,android"),
        // Not a program to find: libmpv is loaded into the app by media_kit.
        // It ships inside the app on Windows and macOS; Linux links the
        // system's libmpv.
        stat("mpv", if cfg!(target_os = "linux") { "System library" } else { "Bundled" }, "ok"),
        tool_row("exiftool"),
        hdr("PLATFORM"),
        tog(s, "notifications", true, "Actionable notifications", "Snooze / mark-played / open-version actions"),
        tog(s, "crash-upload", false, "Opt-in crash uploader", "Send minidumps to the configured Sentry DSN"),
        hdr("BUILD"),
        stat("Renderer", "Flutter", "ok"),
        stat("Player embedding", "libmpv inside the app, through media_kit", "ok"),
        stat("Audio formats", &tulipix_music::formats::FORMATS
            .iter()
            .map(|f| f.ext)
            .collect::<Vec<_>>()
            .join(" · "), "ok"),
    ]
}

/// Where one external tool is coming from. The order is the one the app
/// actually resolves in: the override folder, then the app's own updated copy,
/// then the bundled copy, then PATH -- so "System" is a warning, not a pass: a
/// system binary is whatever version happens to be installed.
///
/// yt-dlp additionally prints its version, because that is the number anyone
/// looking at this row is actually there to check: a download failing with 403
/// is nearly always a binary some weeks old, and "Bundled" never said that.
fn tool_row(name: &str) -> SettingItem {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let file = format!("{name}{ext}");
    let version = if name == "yt-dlp" {
        tulipix_core::ytdlp::installed_version()
    } else {
        None
    };
    let with_version = |source: &str| -> String {
        match &version {
            Some(v) => format!("{source} · {v}"),
            None => source.to_string(),
        }
    };
    let dir = load().text("tools.bin-dir");
    let dir = dir.trim();
    if !dir.is_empty() && Path::new(dir).join(&file).exists() {
        return stat(name, &with_version("Tools directory"), "ok");
    }
    if tulipix_core::ytdlp::managed_dir().is_some_and(|d| d.join(&file).exists()) {
        return stat(name, &with_version("Updated"), "ok");
    }
    if tulipix_core::thumbs::bundled_file(&file).is_some() {
        return stat(name, &with_version("Bundled"), "ok");
    }
    if on_path(&file) {
        return stat(name, &with_version("System"), "warn");
    }
    stat(name, "Missing", "error")
}

fn on_path(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&paths).any(|d| d.join(name).exists())
}

// ── actions ─────────────────────────────────────────────────────────────────

async fn action(key: &str) -> String {
    match key {
        "backup" => match backup() {
            Ok(dir) => format!("Backed up to {}.", dir.display()),
            Err(e) => format!("Backup failed: {e}"),
        },
        "open-logs" => open(tulipix_core::paths::data_dir().map(|d| d.join("logs"))),
        "open-data" => open(tulipix_core::paths::data_dir()),
        // Where the upscale chain is read from (`vmpv::glsl_chain`).
        "open-shaders" => open(tulipix_core::paths::config_dir().map(|d| d.join("shaders"))),
        // Only a folder that is on the watched list: the key is text from the
        // page, and this opens whatever it names.
        k if k.starts_with("lib-open:") => {
            let dir = PathBuf::from(&k["lib-open:".len()..]);
            if watched().contains(&dir) { open(Some(dir)) } else { "That folder is not watched.".into() }
        }
        k if k.starts_with("backup-restore-") => {
            let id = &k["backup-restore-".len()..];
            match restore(id) {
                Ok(()) => "Restored. Your settings from before are a backup of their own now. \
                           Some changes show after a restart."
                    .into(),
                Err(e) => format!("Could not restore: {e}"),
            }
        }
        "music-analyse" => {
            let n = crate::api::music::music_analyse_all().await.unwrap_or(0);
            match n {
                0 => "Every track has already been measured.".into(),
                1 => "Measuring 1 track…".into(),
                n => format!("Measuring {n} tracks… you can keep using the app."),
            }
        }
        "music-analyse-stop" => {
            let _ = crate::api::music::music_analyse_stop().await;
            "Stopping after the current track.".into()
        }
        "scrobble-flush" => {
            let waiting = crate::api::scrobble::scrobble_pending_count()
                .await
                .unwrap_or(0);
            if waiting == 0 {
                return "Nothing is waiting to be sent.".into();
            }
            match crate::api::scrobble::scrobble_flush().await {
                Ok(0) => format!(
                    "{waiting} listen{} still waiting — check the token, or that you are online.",
                    if waiting == 1 { "" } else { "s" }
                ),
                Ok(n) => format!("Sent {n} listen{}.", if n == 1 { "" } else { "s" }),
                Err(e) => format!("Could not send: {e}"),
            }
        }
        "lock-pin-clear" => {
            tulipix_core::account::set_pin("");
            crate::api::lock::remember_pin_len(0);
            "PIN removed. A click unlocks now.".into()
        }
        "clear-thumbs" => match tulipix_core::paths::thumbs_dir() {
            Some(dir) => {
                std::fs::remove_dir_all(&dir).ok();
                std::fs::create_dir_all(&dir).ok();
                "Thumbnail cache cleared.".into()
            }
            None => "No thumbnail cache to clear.".into(),
        },
        // Manifest vs what is on disk. Cheap enough to await inline -- it is
        // a directory listing, not a network call.
        "ai-update-check" => {
            AI_CHECK_BUSY.store(true, std::sync::atomic::Ordering::Relaxed);
            let msg = ai_update_summary();
            AI_CHECK_BUSY.store(false, std::sync::atomic::Ordering::Relaxed);
            msg
        }
        // Model download / verify. The download is detached and reports into
        // `ai_dl_progress`: a 574 MB blob cannot be awaited inside a settings
        // dispatch, and Dart polls `Refresh` while any row reads "busy".
        k if k.starts_with("ai-dl-") => {
            let name = k.trim_start_matches("ai-dl-").to_string();
            let Some(entry) = ai_manifest().find(&name).cloned() else {
                return format!("No model called “{name}” is in the manifest.");
            };
            // A second click while it is already running is a no-op, not a
            // second download writing over the same file.
            if ai_dl_progress().lock().map(|g| g.contains_key(&name)).unwrap_or(false) {
                return format!("{name} is already downloading.");
            }
            if let Ok(mut g) = ai_dl_progress().lock() {
                g.insert(name.clone(), 0.0);
            }
            let started = name.clone();
            tokio::spawn(async move {
                // Record every >=1% step; the poll reads whatever is current,
                // so finer granularity would only cost locks.
                let mut last = -1.0f32;
                let prog_name = name.clone();
                let res = tulipix_photos::ai::models::download_with_progress(&entry, move |f| {
                    if f - last >= 0.01 || f >= 1.0 {
                        last = f;
                        if let Ok(mut g) = ai_dl_progress().lock() {
                            g.insert(prog_name.clone(), f);
                        }
                    }
                })
                .await;
                // Cleared on both paths: a failed download left in the map
                // would stick the row on "Downloading" for the session.
                if let Ok(mut g) = ai_dl_progress().lock() {
                    g.remove(&name);
                }
                match res {
                    Ok(_) => tracing::info!(%name, "model installed"),
                    Err(e) => tracing::error!(%name, error = %e, "model download"),
                }
            });
            format!("Downloading {started}…")
        }
        // An unknown key is a row that has a button and no handler yet. Saying
        // so beats a button that silently does nothing.
        other => format!("Nothing is wired to “{other}” yet."),
    }
}

fn open(dir: Option<PathBuf>) -> String {
    let Some(dir) = dir else { return "That folder does not exist on this system.".into() };
    std::fs::create_dir_all(&dir).ok();
    match tulipix_platform::fm::open_default(&dir) {
        Ok(()) => format!("Opened {}.", dir.display()),
        Err(e) => format!("Could not open {}: {e}", dir.display()),
    }
}

/// Minimal backup: settings.json and watched_folders.json into a timestamped
/// folder under the data dir. The databases are not in it -- they are the part
/// a rescan can rebuild, and the part a copy could catch mid-write.
fn backup() -> Result<PathBuf> {
    let data = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory on this system"))?;
    let dest = data.join("backups").join(crate::api::home::now_secs().to_string());
    std::fs::create_dir_all(&dest)?;
    if let Some(cfg) = tulipix_core::paths::config_dir() {
        for f in ["settings.json", "watched_folders.json"] {
            let src = cfg.join(f);
            if src.exists() {
                std::fs::copy(&src, dest.join(f))?;
            }
        }
    }
    Ok(dest)
}

/// What `backup` has written, newest first. Folders not named by a number are
/// not backups and are left out.
fn backups() -> Vec<BackupRow> {
    let Some(root) = tulipix_core::paths::data_dir().map(|d| d.join("backups")) else {
        return Vec::new();
    };
    let mut out: Vec<BackupRow> = std::fs::read_dir(&root)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let id = e.file_name().to_string_lossy().into_owned();
            let secs = id.parse::<i64>().ok()?;
            let (mut files, mut bytes) = (0u32, 0u64);
            for f in std::fs::read_dir(e.path()).ok()?.flatten() {
                if let Ok(m) = f.metadata()
                    && m.is_file()
                {
                    files += 1;
                    bytes += m.len();
                }
            }
            Some(BackupRow { id, secs, files, bytes })
        })
        .collect();
    out.sort_by(|a, b| b.secs.cmp(&a.secs));
    out
}

/// Copy a backup's files back over the live ones. The live ones are backed up
/// first, so a restore is itself undoable.
fn restore(id: &str) -> Result<()> {
    // The id becomes a path, so it has to be the bare number `backup` names
    // its folders with -- never a `..` or a separator.
    anyhow::ensure!(!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()), "that is not a backup");
    let data = tulipix_core::paths::data_dir()
        .ok_or_else(|| anyhow::anyhow!("no data directory on this system"))?;
    let src = data.join("backups").join(id);
    anyhow::ensure!(src.is_dir(), "that backup is no longer there");
    let cfg = tulipix_core::paths::config_dir()
        .ok_or_else(|| anyhow::anyhow!("no config directory on this system"))?;
    backup()?;
    std::fs::create_dir_all(&cfg)?;
    for f in ["settings.json", "watched_folders.json"] {
        let from = src.join(f);
        if from.exists() {
            std::fs::copy(&from, cfg.join(f))?;
        }
    }
    Ok(())
}

// ── watched folders ─────────────────────────────────────────────────────────

fn watched_path() -> Option<PathBuf> {
    tulipix_core::paths::config_dir().map(|d| d.join("watched_folders.json"))
}

fn watched() -> Vec<PathBuf> {
    let Some(p) = watched_path() else { return Vec::new() };
    std::fs::read_to_string(p)
        .ok()
        .and_then(|b| serde_json::from_str::<Vec<String>>(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .map(PathBuf::from)
        .collect()
}

fn write_watched(list: &[PathBuf]) -> bool {
    let Some(p) = watched_path() else { return false };
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let out: Vec<String> = list.iter().map(|x| x.to_string_lossy().into_owned()).collect();
    match serde_json::to_string_pretty(&out) {
        Ok(body) => std::fs::write(&p, body).is_ok(),
        Err(e) => {
            tracing::warn!(error = %e, "settings: serialise watched folders");
            false
        }
    }
}

fn add_watched(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    let mut list = watched();
    if list.iter().any(|x| x == dir) {
        return false;
    }
    list.push(dir.to_path_buf());
    write_watched(&list)
}

fn remove_watched(dir: &Path) {
    let mut list = watched();
    list.retain(|x| x != dir);
    write_watched(&list);
}

/// The watched list, with whether each folder is still on disk. Removing a
/// folder here does not delete anything indexed from it -- the next scan marks
/// those rows missing, which is reversible; a delete is not.
fn libraries() -> Vec<LibraryRow> {
    watched()
        .into_iter()
        .map(|p| LibraryRow {
            exists: p.exists(),
            path: p.to_string_lossy().into_owned(),
            sections: Vec::new(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every panel opens with a header, or the first rows sit under nothing.
    #[test]
    fn every_panel_starts_with_a_header() {
        let s = S::default();
        for (name, rows) in [
            ("playback", playback(&s)),
            ("services", services(&s)),
            ("ai", ai(&s)),
            ("security", security(&s)),
            ("data", data(&s)),
            ("advanced", advanced(&s)),
        ] {
            assert_eq!(rows[0].kind, "header", "{name} does not open with a header");
        }
    }

    /// A toggle or a text row with no key writes into nothing. Headers and
    /// status lines are the only rows allowed to be keyless.
    #[test]
    fn every_writable_row_carries_a_key() {
        let s = S::default();
        for rows in [playback(&s), services(&s), ai(&s), security(&s), data(&s), advanced(&s)] {
            for r in rows {
                let writable = matches!(r.kind.as_str(), "toggle" | "text" | "choice" | "action");
                assert!(!writable || !r.key.is_empty(), "{} has no key", r.label);
            }
        }
    }

    /// A choice row with no stored value must offer its first option rather
    /// than an empty selection the picker cannot show.
    #[test]
    fn a_choice_defaults_to_its_first_option() {
        let row = choice(&S::default(), "ai.model.voice", "Voice", "", &["tiny", "base"]);
        assert_eq!(row.value, "tiny");
        assert_eq!(row.options.len(), 2);
    }

    /// Every "Lock after" choice reads back as itself, and a value that is not
    /// on the list shows as the nearest one.
    #[test]
    fn lock_after_labels_round_trip() {
        for (label, secs) in LOCK_AFTER {
            assert_eq!(lock_after_label(secs), label);
        }
        assert_eq!(lock_after_label(700), "10 minutes");
        assert_eq!(lock_after_label(100_000), "1 hour");
    }

    /// A restore names a folder, so anything but a backup's bare number is
    /// refused before it can become a path.
    #[test]
    fn a_restore_takes_only_a_backup_id() {
        for bad in ["", "..", "../settings", "12/../../x", "abc"] {
            assert!(restore(bad).is_err(), "{bad:?} was accepted");
        }
    }

    /// Home cards default to on: a fresh install shows the whole page, and the
    /// switches are there to take things away.
    #[test]
    fn home_cards_start_on() {
        let s = S::default();
        assert!(HOME_CARDS.iter().all(|(k, _)| s.flag(&format!("{HOME_CARD_PREFIX}{k}"), true)));
    }
}
