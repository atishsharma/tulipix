//! `np.p4.music.listen-prefs` — the spoken-word settings that belong to the
//! section rather than to one show or one book.
//!
//! Same store as [`crate::yt_prefs`]: `Settings.advanced`, one file shared by
//! both front ends, so a limit set in Flutter is the limit Slint enforces.
//! Per-show and per-book settings are columns instead — those belong to the
//! row, not to the app.

use tulipix_core::settings::Settings;

fn get(key: &str) -> Option<String> {
    Settings::load().ok().and_then(|s| s.advanced.get(key).cloned())
}

fn put(key: &str, value: &str) {
    let mut s = Settings::load().unwrap_or_default();
    s.advanced.insert(key.to_string(), value.to_string());
    let _ = s.save();
}

fn flag(key: &str, default: bool) -> bool {
    get(key).map(|v| v == "1").unwrap_or(default)
}

fn set_flag(key: &str, on: bool) {
    put(key, if on { "1" } else { "0" });
}

// ------------------------------------------------------------- podcasts ----

/// How much disk the saved episodes may take, in GB. 0 = no ceiling, which is
/// what the meter reads as "of unlimited".
pub fn download_limit_gb() -> i64 {
    get("pod.dl-limit-gb").and_then(|v| v.parse().ok()).unwrap_or(6)
}

pub fn set_download_limit_gb(gb: i64) {
    put("pod.dl-limit-gb", &gb.clamp(0, 512).to_string());
}

/// Drop the audio file once the episode is marked played.
pub fn delete_when_played() -> bool {
    flag("pod.dl-when-played", false)
}

pub fn set_delete_when_played(on: bool) {
    set_flag("pod.dl-when-played", on);
}

/// Only auto-download on an unmetered connection. Advisory: the download
/// command honours it, a hand-pressed Download does not.
pub fn wifi_only() -> bool {
    flag("pod.dl-wifi-only", true)
}

pub fn set_wifi_only(on: bool) {
    set_flag("pod.dl-wifi-only", on);
}

/// New episodes of a pinned show join the queue as the feed brings them in.
pub fn queue_new_pinned() -> bool {
    flag("pod.queue-auto", false)
}

pub fn set_queue_new_pinned(on: bool) {
    set_flag("pod.queue-auto", on);
}

// ----------------------------------------------------------- audiobooks ----

/// What the two skip buttons move by, in seconds.
pub fn skip_seconds() -> i64 {
    get("book.skip-s").and_then(|v| v.parse().ok()).unwrap_or(30)
}

pub fn set_skip_seconds(secs: i64) {
    put("book.skip-s", &secs.clamp(5, 120).to_string());
}

/// Step back a few seconds when play resumes, so the sentence you stopped
/// mid-way starts again.
pub fn rewind_after_pause() -> bool {
    flag("book.rewind-pause", true)
}

pub fn set_rewind_after_pause(on: bool) {
    set_flag("book.rewind-pause", on);
}

pub fn trim_silence() -> bool {
    flag("book.trim-silence", false)
}

pub fn set_trim_silence(on: bool) {
    set_flag("book.trim-silence", on);
}

pub fn boost_voices() -> bool {
    flag("book.boost-voices", false)
}

pub fn set_boost_voices(on: bool) {
    set_flag("book.boost-voices", on);
}

// --------------------------------------------------------------- shared ----

/// Fade the last minute out rather than cutting when the sleep timer fires.
pub fn sleep_fade() -> bool {
    flag("listen.sleep-fade", true)
}

pub fn set_sleep_fade(on: bool) {
    set_flag("listen.sleep-fade", on);
}
