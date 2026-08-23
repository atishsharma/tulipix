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

pub struct SettingsState {
    /// "profile" | "libraries" | "playback" | "services" | "ai" | "security"
    /// | "data" | "advanced" | "status".
    pub tab: String,
    // You & Home.
    pub display_name: String,
    pub avatar_emoji: String,
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
    pub services: Vec<SettingItem>,
    pub ai: Vec<SettingItem>,
    pub security: Vec<SettingItem>,
    pub data: Vec<SettingItem>,
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
    SaveProfile { name: String, emoji: String, logo: i32 },
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
            _ => put(&key, &value),
        },
        SettingsCmd::Action { key } => notice = action(&key).await,
        SettingsCmd::SaveProfile { name, emoji, logo } => {
            crate::api::shell::shell_dispatch(crate::api::shell::ShellCmd::SaveProfile {
                name,
                emoji,
                logo,
            })
            .await?;
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
    ("hero", "Greeting and library totals"),
    ("continue", "Continue where you left off"),
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
    SettingsState {
        tab: tab_cell().lock().map(|g| g.clone()).unwrap_or_else(|_| "profile".into()),
        display_name: s.text("profile.name"),
        avatar_emoji: s.text("profile.emoji"),
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
        services: services(&s),
        ai: ai(&s),
        security: security(&s),
        data: data(&s),
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
    vec![
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
        tog(s, "ai.chat", false, "Chat assistant", "Ask questions about your library in plain language"),
        hdr("BOOK READ-ALOUD"),
        tog(s, "books.tts.neural", true, "Use neural voice (Kokoro)",
            "Natural AI voice for the reader's Read Aloud. Off = robotic espeak voice (no model, needs espeak-ng)"),
        hdr("CLOUD"),
        tog(s, "ai.cloud-offload", false, "Allow cloud AI help", "Send selected questions to a cloud AI service. Off = everything stays on-device"),
    ]
}

fn security(s: &S) -> Vec<SettingItem> {
    let idle = match s.idle_lock_secs {
        0 => String::new(),
        n => n.to_string(),
    };
    vec![
        hdr("LOCK"),
        tog(s, "autolock", false, "Auto-lock when idle", "Lock the app and show the screensaver after a period of no activity"),
        si("idle_lock_secs", "text", "Idle timeout (seconds)",
            "How long before auto-lock kicks in — blank means 600 (ten minutes)", &idle, false, ""),
        txt(s, "lock.wallpapers", "Lock screen wallpapers",
            "Folder of pictures for the lock screen slideshow — the first ten are used, one every twelve seconds. Blank = the gradient"),
        hdr("UNLOCK"),
        tog(s, "passkey", false, "Unlock with a passkey", "Use a security key or fingerprint instead of a password"),
        hdr("ENCRYPTION"),
        tog(s, "db-encrypt", false, "Encrypt the library database", "Protects your library index if the disk is stolen — applies on next launch"),
    ]
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
            "Folder holding mpv, yt-dlp, ffmpeg… — checked before bundled + PATH (blank = off)"),
        tool_row("ffmpeg"),
        tool_row("ffprobe"),
        tool_row("rclone"),
        tool_row("yt-dlp"),
        tool_row("mpv"),
        tool_row("exiftool"),
        hdr("PLATFORM"),
        tog(s, "notifications", true, "Actionable notifications", "Snooze / mark-played / open-version actions"),
        tog(s, "crash-upload", false, "Opt-in crash uploader", "Send minidumps to the configured Sentry DSN"),
        hdr("BUILD"),
        stat("Renderer", "Flutter", "ok"),
        stat("Player embedding", "Out-of-process mpv — isolation by design", "ok"),
        stat("Audio formats", &tulipix_music::formats::FORMATS
            .iter()
            .map(|f| f.ext)
            .collect::<Vec<_>>()
            .join(" · "), "ok"),
    ]
}

/// Where one external tool is coming from. The order is the one the app
/// actually resolves in: the override folder, then the bundled copy, then
/// PATH -- so "System" is a warning, not a pass: a system binary is whatever
/// version happens to be installed.
fn tool_row(name: &str) -> SettingItem {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    let dir = load().text("tools.bin-dir");
    let dir = dir.trim();
    if !dir.is_empty() && Path::new(dir).join(format!("{name}{ext}")).exists() {
        return stat(name, "Tools directory", "ok");
    }
    if tulipix_core::thumbs::bundled_file(&format!("{name}{ext}")).is_some() {
        return stat(name, "Bundled", "ok");
    }
    if on_path(&format!("{name}{ext}")) {
        return stat(name, "System", "warn");
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
        "clear-thumbs" => match tulipix_core::paths::thumbs_dir() {
            Some(dir) => {
                std::fs::remove_dir_all(&dir).ok();
                std::fs::create_dir_all(&dir).ok();
                "Thumbnail cache cleared.".into()
            }
            None => "No thumbnail cache to clear.".into(),
        },
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

    /// Home cards default to on: a fresh install shows the whole page, and the
    /// switches are there to take things away.
    #[test]
    fn home_cards_start_on() {
        let s = S::default();
        assert!(HOME_CARDS.iter().all(|(k, _)| s.flag(&format!("{HOME_CARD_PREFIX}{k}"), true)));
    }
}
