//! Every Stream Plus setting, in one place, typed.
//!
//! The app stores settings as two string-keyed maps (`flags` for bools,
//! `advanced` for text). Reading them inline everywhere is how key typos become
//! silent behaviour changes, so nothing outside this file spells a key.

use tulipix_core::settings::Settings;

fn flag(key: &str, default: bool) -> bool {
    Settings::load().map(|s| s.flag(key, default)).unwrap_or(default)
}

fn text(key: &str) -> String {
    Settings::load().map(|s| s.text(key)).unwrap_or_default()
}

fn set_flag(key: &str, on: bool) {
    if let Ok(mut s) = Settings::load() {
        s.flags.insert(key.to_string(), on);
        if let Err(e) = s.save() {
            tracing::warn!(key, error = %e, "splus: could not save setting");
        }
    }
}

fn set_text(key: &str, value: &str) {
    if let Ok(mut s) = Settings::load() {
        s.advanced.insert(key.to_string(), value.to_string());
        if let Err(e) = s.save() {
            tracing::warn!(key, error = %e, "splus: could not save setting");
        }
    }
}

fn num(key: &str, default: i64) -> i64 {
    let t = text(key);
    t.trim().parse::<i64>().unwrap_or(default)
}

// ── sources ───────────────────────────────────────────────────────────────
pub fn source_enabled(id: &str) -> bool {
    flag(&format!("splus.source_enabled.{id}"), true)
}
pub fn set_source_enabled(id: &str, on: bool) {
    set_flag(&format!("splus.source_enabled.{id}"), on);
}
/// Comma-separated source ids, first tried first. Empty means declaration order.
pub fn source_order() -> Vec<String> {
    text("splus.source_order")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}
pub fn set_source_order(ids: &[String]) {
    set_text("splus.source_order", &ids.join(","));
}
/// Extra hosts the user pasted into the Servers modal, one per line.
pub fn extra_hosts() -> Vec<String> {
    text("splus.hosts")
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("http"))
        .collect()
}
pub fn set_extra_hosts(text_block: &str) {
    set_text("splus.hosts", text_block);
}

// ── anime ─────────────────────────────────────────────────────────────────
pub fn audio_pref() -> super::AudioPref {
    super::AudioPref::from_setting(&text("splus.audio_pref"))
}
pub fn set_audio_pref(v: &str) {
    set_text("splus.audio_pref", v);
}
pub fn skip_op_ed() -> bool {
    flag("splus.skip_op_ed", true)
}
pub fn set_skip_op_ed(on: bool) {
    set_flag("splus.skip_op_ed", on);
}
/// Progress fraction past which an episode counts as finished.
pub fn watched_threshold() -> f32 {
    (num("splus.watched_threshold", 90) as f32 / 100.0).clamp(0.5, 1.0)
}
pub fn set_watched_threshold(pct: i64) {
    set_text("splus.watched_threshold", &pct.clamp(50, 100).to_string());
}
/// Seconds of countdown before the next episode. 0 disables autoplay.
pub fn autoplay_seconds() -> i64 {
    num("splus.autoplay_seconds", 10).clamp(0, 60)
}
pub fn set_autoplay_seconds(s: i64) {
    set_text("splus.autoplay_seconds", &s.clamp(0, 60).to_string());
}
pub fn hide_finished() -> bool {
    flag("splus.hide_finished", false)
}
pub fn set_hide_finished(on: bool) {
    set_flag("splus.hide_finished", on);
}

// ── subtitles ─────────────────────────────────────────────────────────────
pub fn subs_wyzie() -> bool {
    flag("splus.subs.wyzie", true)
}
pub fn set_subs_wyzie(on: bool) {
    set_flag("splus.subs.wyzie", on);
}
pub fn subs_subdl() -> bool {
    flag("splus.subs.subdl", false)
}
pub fn set_subs_subdl(on: bool) {
    set_flag("splus.subs.subdl", on);
}
pub fn subs_lang() -> String {
    let l = text("splus.subs.lang");
    if l.is_empty() { "en".into() } else { l }
}
pub fn set_subs_lang(l: &str) {
    set_text("splus.subs.lang", l);
}
pub fn subs_with_file() -> bool {
    flag("splus.subs.with_file", true)
}
pub fn set_subs_with_file(on: bool) {
    set_flag("splus.subs.with_file", on);
}

// ── player / downloads ────────────────────────────────────────────────────
/// Preferred vertical resolution. The picker takes the closest at or below it.
pub fn preferred_height() -> i32 {
    num("splus.quality", 1080) as i32
}
pub fn set_preferred_height(h: i32) {
    set_text("splus.quality", &h.to_string());
}
pub fn into_library() -> bool {
    flag("splus.into_library", true)
}
pub fn set_into_library(on: bool) {
    set_flag("splus.into_library", on);
}
pub fn download_dir() -> std::path::PathBuf {
    let t = text("splus.download_dir");
    if !t.is_empty() {
        return std::path::PathBuf::from(t);
    }
    tulipix_core::paths::data_dir()
        .map(|d| d.join("stream-plus"))
        .unwrap_or_else(|| std::path::PathBuf::from("stream-plus"))
}
pub fn set_download_dir(p: &str) {
    set_text("splus.download_dir", p);
}
/// How many downloads run at once. More slots is not faster on one host.
pub fn download_slots() -> usize {
    num("splus.slots", 2).clamp(1, 6) as usize
}
pub fn set_download_slots(n: i64) {
    set_text("splus.slots", &n.clamp(1, 6).to_string());
}

// ── age gate ──────────────────────────────────────────────────────────────
/// Which certification system the limit is expressed in. Kept for the UI label;
/// the comparison itself is on `parental::Rating`'s severity, which is
/// system-agnostic.
pub fn age_system() -> String {
    let s = text("splus.age.system");
    if s.is_empty() { "US".into() } else { s }
}
pub fn set_age_system(s: &str) {
    set_text("splus.age.system", s);
}
/// Highest certification allowed, e.g. "PG-13". Empty means no limit.
pub fn age_max() -> String {
    text("splus.age.max")
}
pub fn set_age_max(s: &str) {
    set_text("splus.age.max", s);
}
/// Adult titles are gated by their own flag rather than a certification, since
/// no board rates them on the same scale.
pub fn allow_adult() -> bool {
    flag("splus.age.allow_adult", false)
}
pub fn set_allow_adult(on: bool) {
    set_flag("splus.age.allow_adult", on);
}

// ── notifications ─────────────────────────────────────────────────────────
pub fn notify_download_done() -> bool {
    flag("splus.notify.download_done", true)
}
pub fn set_notify_download_done(on: bool) {
    set_flag("splus.notify.download_done", on);
}
pub fn notify_new_episode() -> bool {
    flag("splus.notify.new_episode", true)
}
pub fn set_notify_new_episode(on: bool) {
    set_flag("splus.notify.new_episode", on);
}

// ── SubDL key ─────────────────────────────────────────────────────────────
/// Kept in the keychain-backed key store, never in settings.json.
pub fn subdl_key() -> String {
    tulipix_core::api_keys::fetch("subdl").ok().flatten().unwrap_or_default()
}
