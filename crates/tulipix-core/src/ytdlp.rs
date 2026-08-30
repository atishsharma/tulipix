//! Everything the app knows about yt-dlp, in one place.
//!
//! There are fifteen yt-dlp call sites across six crates and two front-ends,
//! and before this module they agreed about nothing: one of them applied the
//! user's cookies, one rotated player clients, one translated YouTube's bot
//! check into a sentence a person could act on, and the other twelve did none
//! of it. A user who filled in Settings → "yt-dlp cookies" got them applied to
//! the Music Downloader and nowhere else.
//!
//! So: `bin()` resolves it, [`common_args`] is what every invocation carries,
//! [`friendly_error`] is the one translation of its stderr, and the update half
//! below is the standing cure for the failure that prompted all of this — a
//! bundled binary going stale until YouTube starts answering 403.
//!
//! The version arithmetic and the weekly timer live here rather than in
//! `tulipix-tools::ytdlp_update` (which now re-exports them) because
//! `tulipix-core` is the crate every caller already depends on, and the updater
//! needs them.

use crate::proc::NoWindow;
use std::path::{Path, PathBuf};

/// The resolved yt-dlp binary, by the app's usual tool-resolution order.
pub fn bin() -> PathBuf {
    crate::thumbs::tool_bin("yt-dlp")
}

// ── argv ────────────────────────────────────────────────────────────────────

fn cookie_cell() -> &'static std::sync::RwLock<Option<Vec<String>>> {
    static C: std::sync::OnceLock<std::sync::RwLock<Option<Vec<String>>>> =
        std::sync::OnceLock::new();
    C.get_or_init(|| std::sync::RwLock::new(None))
}

/// Cookie flags from the stored `ytdlp_cookies` setting: a cookies.txt path
/// becomes `--cookies <path>`, anything else is taken as a browser name for
/// `--cookies-from-browser`.
///
/// This is the only reliable way past YouTube's "confirm you're not a bot"
/// gate, which is why every caller wants it and not just the one that used to
/// have it.
///
/// Cached for the session, and that is not an optimisation to skip: the value
/// lives in the OS keyring, so reading it is a D-Bus round trip to the secret
/// service on Linux. It used to be read once per track download; it is now on
/// the path of every YouTube listing and search, which is exactly where a
/// per-call IPC hop would be felt. [`forget_cookies`] is what the settings
/// writer calls so a change still applies without a restart.
pub fn cookie_args() -> Vec<String> {
    if let Ok(g) = cookie_cell().read() {
        if let Some(v) = g.as_ref() {
            return v.clone();
        }
    }
    let built = match crate::api_keys::fetch("ytdlp_cookies") {
        Ok(Some(v)) if !v.trim().is_empty() => {
            let v = v.trim().to_string();
            if Path::new(&v).is_file() {
                vec!["--cookies".into(), v]
            } else {
                vec!["--cookies-from-browser".into(), v]
            }
        }
        _ => Vec::new(),
    };
    if let Ok(mut g) = cookie_cell().write() {
        *g = Some(built.clone());
    }
    built
}

/// Forget the cached cookie flags. Call after writing or clearing the
/// `ytdlp_cookies` key, or the change does not take until the next launch.
pub fn forget_cookies() {
    if let Ok(mut g) = cookie_cell().write() {
        *g = None;
    }
}

/// An optional player-client override, from the `ytdlp.player-clients` advanced
/// setting.
///
/// Deliberately **empty by default**. The Music Downloader used to hardcode
/// `default,tv,android` to dodge the bot gate, and on a current yt-dlp that is
/// now a liability rather than a fix: pinning a client list freezes one 2026-07
/// opinion into the app and stops it following upstream's, which is maintained
/// against whatever YouTube did this week. Measured on 2026-08-30, a current
/// yt-dlp downloads with no override at all, and the stale one failed *with*
/// the override — the binary was the variable, not the argv.
///
/// The setting stays because when YouTube next changes something, the fix
/// lands in yt-dlp's release before it lands in ours, and a user who reads a
/// workaround on the tracker should be able to apply it without a new build.
pub fn player_client_args() -> Vec<String> {
    let v = crate::settings::Settings::load()
        .map(|s| s.text("ytdlp.player-clients"))
        .unwrap_or_default();
    player_client_args_from(&v)
}

/// The argv for a given client list. Split out from the setting read so the
/// formatting is testable without a settings file.
fn player_client_args_from(v: &str) -> Vec<String> {
    let v = v.trim();
    if v.is_empty() {
        Vec::new()
    } else {
        vec!["--extractor-args".into(), format!("youtube:player_client={v}")]
    }
}

/// What every yt-dlp invocation in the app carries: the user's cookies, and any
/// player-client override they have set.
///
/// Nothing here writes to stdout, which matters — half the callers parse
/// `-J` / `--dump-json` output and a stray line would break them.
pub fn common_args() -> Vec<String> {
    let mut a = cookie_args();
    a.extend(player_client_args());
    a
}

// ── errors ──────────────────────────────────────────────────────────────────

/// Does this stderr mean "YouTube refused us", as opposed to a bad URL or a
/// missing file?
///
/// The three shapes are one problem wearing three hats: a 403 on the media
/// fetch, the bot gate on the metadata fetch, and the sign-in wall on
/// age/region-gated videos. All three are answered by the same two things —
/// a current binary and cookies — so they get one message.
pub fn is_access_error(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("http error 403")
        || s.contains("forbidden")
        || s.contains("confirm you")
        || s.contains("not a bot")
        || s.contains("sign in to confirm")
        || s.contains("sign in")
}

/// Turn yt-dlp's stderr into one sentence worth showing.
///
/// Callers used to surface "yt-dlp could not resolve a stream for dQw4w9WgXcQ",
/// which tells the user nothing they can act on. The access case names the two
/// things that actually fix it; everything else falls through to yt-dlp's own
/// last line, which is usually the useful one.
pub fn friendly_error(stderr: &str) -> String {
    if is_access_error(stderr) {
        return "The site refused the request (403 / sign-in check). Update \
                yt-dlp in Settings → Tools, and if it keeps happening set \
                browser cookies in Settings → yt-dlp cookies."
            .to_string();
    }
    let last = stderr
        .lines()
        .rev()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("yt-dlp failed");
    last.trim_start_matches("ERROR: ").to_string()
}

// ── version ─────────────────────────────────────────────────────────────────

/// argv to print the installed version.
pub fn version_args() -> Vec<&'static str> {
    vec!["--version"]
}

fn version_cell() -> &'static std::sync::RwLock<Option<Option<String>>> {
    static V: std::sync::OnceLock<std::sync::RwLock<Option<Option<String>>>> =
        std::sync::OnceLock::new();
    V.get_or_init(|| std::sync::RwLock::new(None))
}

/// The installed version (`YYYY.MM.DD[.N]`), or None if it could not be run.
///
/// Cached, because this spawns a process and the Settings panel, the status
/// page and the update check all ask. An update calls [`forget_version`], so
/// the cache is a `RwLock` rather than a `OnceLock`: a version that cannot be
/// re-read after the thing it describes has been replaced is a stale answer
/// with no way back.
pub fn installed_version() -> Option<String> {
    if let Ok(g) = version_cell().read() {
        if let Some(v) = g.as_ref() {
            return v.clone();
        }
    }
    let read = (|| {
        let out = std::process::Command::new(bin())
            .args(version_args())
            .no_window()
            .output()
            .ok()?;
        let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if v.is_empty() { None } else { Some(v) }
    })();
    if let Ok(mut g) = version_cell().write() {
        *g = Some(read.clone());
    }
    read
}

/// Forget the cached version, after an update has replaced the binary.
pub fn forget_version() {
    if let Ok(mut g) = version_cell().write() {
        *g = None;
    }
}

/// Is `remote` newer than `local`? yt-dlp versions are date-based
/// `YYYY.MM.DD[.N]`; compare component-wise numerically.
pub fn is_newer(local: &str, remote: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> {
        v.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().unwrap_or(0))
            .collect()
    }
    let (l, r) = (parts(local), parts(remote));
    for i in 0..l.len().max(r.len()) {
        let a = l.get(i).copied().unwrap_or(0);
        let b = r.get(i).copied().unwrap_or(0);
        if b != a {
            return b > a;
        }
    }
    false
}

/// How often the app looks for a new yt-dlp.
pub const CHECK_INTERVAL_S: i64 = 7 * 86_400;

pub fn is_check_due(last_check_unix: i64, now_unix: i64) -> bool {
    now_unix >= last_check_unix + CHECK_INTERVAL_S
}

// ── where an update lands ───────────────────────────────────────────────────

/// The app-managed tools directory: `<data>/tools`.
///
/// A self-update cannot always write next to the bundled copy — a packaged
/// install puts `resources/bin/` somewhere root owns — so updates land here and
/// [`crate::thumbs::tool_bin`] prefers this over the bundle. That ordering is
/// the whole point: an update nobody can reach is not an update.
pub fn managed_dir() -> Option<PathBuf> {
    crate::thumbs::managed_dir()
}

/// The managed copy's path, whether or not it exists yet.
pub fn managed_bin() -> Option<PathBuf> {
    let ext = if cfg!(target_os = "windows") { ".exe" } else { "" };
    managed_dir().map(|d| d.join(format!("yt-dlp{ext}")))
}

/// Where an update should be written.
///
/// In place when the bundled copy's directory is writable — that keeps one copy
/// on disk and the dev tree behaving as it always has — and in the managed
/// directory otherwise.
pub fn update_dest() -> Option<PathBuf> {
    let current = bin();
    if current.is_absolute() {
        if let Some(dir) = current.parent() {
            if dir_is_writable(dir) {
                return Some(current);
            }
        }
    }
    let dest = managed_bin()?;
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).ok()?;
    }
    Some(dest)
}

/// Can we create a file in this directory? Asked by trying, because the
/// permission bits lie about read-only mounts, ACLs and containers.
fn dir_is_writable(dir: &Path) -> bool {
    let probe = dir.join(".tulipix-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// ── the stored check state ──────────────────────────────────────────────────

/// `advanced` keys the updater keeps: when it last looked, and what it saw.
pub const LAST_CHECK_KEY: &str = "ytdlp.last-check";
pub const LAST_SEEN_KEY: &str = "ytdlp.last-seen";
/// Off switch for the weekly check (the manual button still works).
pub const AUTO_UPDATE_FLAG: &str = "ytdlp.auto-update";

pub fn auto_update_enabled() -> bool {
    crate::settings::Settings::load()
        .map(|s| s.flag(AUTO_UPDATE_FLAG, true))
        .unwrap_or(true)
}

pub fn last_check() -> i64 {
    crate::settings::Settings::load()
        .ok()
        .and_then(|s| s.text(LAST_CHECK_KEY).parse().ok())
        .unwrap_or(0)
}

pub fn record_check(now_unix: i64, seen_version: &str) {
    let Ok(mut s) = crate::settings::Settings::load() else { return };
    s.advanced.insert(LAST_CHECK_KEY.into(), now_unix.to_string());
    if !seen_version.is_empty() {
        s.advanced.insert(LAST_SEEN_KEY.into(), seen_version.to_string());
    }
    if let Err(e) = s.save() {
        tracing::warn!(error = %e, "yt-dlp: could not record the update check");
    }
}

pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("2024.01.01", "2024.03.10"));
        assert!(is_newer("2024.03.10", "2024.03.10.1"));
        assert!(!is_newer("2024.03.10", "2024.03.10"));
        assert!(!is_newer("2024.04.01", "2024.03.10"));
        // A local nightly must not be "updated" back to the older stable.
        assert!(!is_newer("2026.08.19.232815", "2026.08.19"));
        // Tag names sometimes carry a leading v.
        assert!(is_newer("2026.07.04", "v2026.08.19"));
    }

    #[test]
    fn check_timer() {
        assert!(is_check_due(0, CHECK_INTERVAL_S));
        assert!(!is_check_due(0, CHECK_INTERVAL_S - 1));
    }

    #[test]
    fn access_errors_are_recognised() {
        assert!(is_access_error("ERROR: unable to download video data: HTTP Error 403: Forbidden"));
        assert!(is_access_error("Sign in to confirm you're not a bot"));
        assert!(!is_access_error("ERROR: [youtube] xyz: Video unavailable"));
    }

    #[test]
    fn friendly_error_names_the_two_fixes() {
        let m = friendly_error("ERROR: unable to download video data: HTTP Error 403: Forbidden");
        assert!(m.contains("Update"));
        assert!(m.contains("cookies"));
        // Anything else keeps yt-dlp's own last line, without the ERROR: prefix.
        assert_eq!(
            friendly_error("[youtube] x: Downloading\nERROR: Video unavailable"),
            "Video unavailable"
        );
        assert_eq!(friendly_error("   "), "yt-dlp failed");
    }

    #[test]
    fn player_clients_are_off_unless_set() {
        // Blank is the documented default: follow yt-dlp's own client choice.
        assert!(player_client_args_from("").is_empty());
        assert!(player_client_args_from("   ").is_empty());
        assert_eq!(
            player_client_args_from("default,tv,android"),
            vec![
                "--extractor-args".to_string(),
                "youtube:player_client=default,tv,android".to_string()
            ]
        );
    }

    #[test]
    fn cookie_args_are_a_pair_or_nothing() {
        // Whatever the keyring holds in this environment, the shape is fixed:
        // a flag and its value, or nothing at all. A lone flag would make
        // yt-dlp swallow the next argument as its value.
        let a = cookie_args();
        assert!(a.is_empty() || a.len() == 2);
        if let Some(flag) = a.first() {
            assert!(flag == "--cookies" || flag == "--cookies-from-browser");
        }
    }
}
