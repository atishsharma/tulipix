//! `np.p4.music.youtube` — the YouTube preferences both front ends persist.
//!
//! All four live in `Settings.advanced`, which is one file shared by the two
//! builds, so these have to agree on the key AND on what the value means: the
//! `-999` sentinel, the nine-entry CSV, the newest-first ordering. Keeping two
//! copies of that in two crates is how they drift.

use tulipix_core::settings::Settings;

/// "No default chosen" — the value that makes the quality picker appear.
/// Not `None`, because `0` is a real answer ("best available").
pub const RES_UNSET: i64 = -999;

/// Home-rail pins. Nine is what the rail fits.
pub const HOME_MAX: usize = 9;

fn get(key: &str) -> Option<String> {
    Settings::load().ok().and_then(|s| s.advanced.get(key).cloned())
}

fn put(key: &str, value: Option<&str>) {
    let mut s = Settings::load().unwrap_or_default();
    match value {
        Some(v) => {
            s.advanced.insert(key.to_string(), v.to_string());
        }
        None => {
            s.advanced.remove(key);
        }
    }
    let _ = s.save();
}

/// Preferred video height. `0` = best available, `< 0` = audio only,
/// [`RES_UNSET`] = ask every time.
pub fn default_res() -> i64 {
    get("yt.default-res")
        .and_then(|v| v.parse::<i64>().ok())
        .unwrap_or(RES_UNSET)
}

pub fn store_default_res(h: Option<i64>) {
    put("yt.default-res", h.map(|v| v.to_string()).as_deref());
}

/// Watch default. Until one is saved it follows the old download height, so a
/// user who told the picker "1080p" gets 1080p; with none (or "best", or
/// audio-only) it is 720p. Uncapped, YouTube's best is 4K60, which nothing
/// decodes cheaply on an ordinary laptop, and 720p H.264 is light enough for
/// any machine this runs on while still sharp in a window.
pub fn watch_pref() -> crate::youtube::formats::WatchPref {
    use crate::youtube::formats::WatchPref;
    match get("yt.watch-pref") {
        Some(v) => WatchPref::parse(&v),
        None => {
            let h = default_res();
            WatchPref { height: if h > 0 { h as u32 } else { 720 }, codec: "any".into() }
        }
    }
}

pub fn store_watch_pref(p: Option<&crate::youtube::formats::WatchPref>) {
    put("yt.watch-pref", p.map(|p| p.to_pref_string()).as_deref());
}

/// Download file-name template (yt-dlp `-o` syntax, `~` allowed).
pub fn download_template() -> String {
    get("yt.dl-template")
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| crate::youtube::dl_args::DEFAULT_TEMPLATE.to_string())
}

/// The last audio preset (`audio = true`) or video preset used. A key that no
/// longer parses, or one saved on the wrong side, falls back to the default.
pub fn download_preset(audio: bool) -> crate::youtube::dl_args::Preset {
    use crate::youtube::dl_args::{AudioPreset, Preset, VideoPreset};
    get(if audio { "yt.dl-audio" } else { "yt.dl-video" })
        .and_then(|k| Preset::parse(&k))
        .filter(|p| p.is_audio() == audio)
        .unwrap_or(if audio {
            Preset::Audio(AudioPreset::Opus)
        } else {
            Preset::Video(VideoPreset::Mp4(1080))
        })
}

pub fn download_extras() -> crate::youtube::dl_args::Extras {
    get("yt.dl-extras")
        .map(|v| crate::youtube::dl_args::Extras::parse(&v))
        .unwrap_or_default()
}

/// Remember a download's choices as the next one's defaults. `None` forgets
/// all three, which is what "reset video preferences" means.
pub fn store_download_spec(spec: Option<&crate::youtube::dl_args::DownloadSpec>) {
    match spec {
        Some(s) => {
            let key = if s.preset.is_audio() { "yt.dl-audio" } else { "yt.dl-video" };
            put(key, Some(&s.preset.key()));
            put("yt.dl-extras", Some(&s.extras.to_pref_string()));
            put("yt.dl-template", Some(&s.template));
        }
        None => {
            for key in ["yt.dl-audio", "yt.dl-video", "yt.dl-extras", "yt.dl-template"] {
                put(key, None);
            }
        }
    }
}

/// What streaming caches: `audio` (default), `all` (audio, and videos at the
/// watched format) or `never`.
pub fn cache_mode() -> String {
    get("yt.cache-mode")
        .filter(|v| v == "all" || v == "never")
        .unwrap_or_else(|| "audio".to_string())
}

/// The cache's byte budget, 5 GB until changed. Both builds evict to it.
pub fn cache_cap_bytes() -> i64 {
    get("yt.cache-cap")
        .and_then(|v| v.parse::<i64>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(5 * 1024 * 1024 * 1024)
}

pub fn cache_evict() -> crate::youtube::store::Evict {
    crate::youtube::store::Evict::parse(&get("yt.cache-evict").unwrap_or_default())
}

/// Days unplayed before a cached file goes regardless of space; 0 = never.
pub fn cache_idle_days() -> u32 {
    get("yt.cache-idle-days")
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(30)
}

pub fn store_cache_policy(mode: &str, cap_bytes: i64, evict: crate::youtube::store::Evict, idle_days: u32) {
    put("yt.cache-mode", Some(mode));
    put("yt.cache-cap", Some(&cap_bytes.to_string()));
    put("yt.cache-evict", Some(evict.key()));
    put("yt.cache-idle-days", Some(&idle_days.to_string()));
}

/// User-pinned Home-rail channels, newest first.
pub fn home_channels() -> Vec<String> {
    get("yt.home-channels")
        .map(|v| {
            v.split(',')
                .map(|x| x.trim().to_string())
                .filter(|x| !x.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

pub fn add_home_channel(id: &str) {
    let mut list = home_channels();
    list.retain(|x| x != id);
    list.insert(0, id.to_string());
    list.truncate(HOME_MAX);
    put("yt.home-channels", Some(&list.join(",")));
}

pub fn remove_home_channel(id: &str) {
    let mut list = home_channels();
    list.retain(|x| x != id);
    put("yt.home-channels", Some(&list.join(",")));
}

/// Subscriptions page: "sub" (default) or "unsub".
pub fn subs_filter() -> String {
    get("yt.subs-filter")
        .filter(|v| v == "unsub")
        .unwrap_or_else(|| "sub".to_string())
}

pub fn store_subs_filter(v: &str) {
    put("yt.subs-filter", Some(v));
}

/// Caption language for watching and for downloads' embedded subtitles: a
/// language code, from the system locale until one is chosen.
pub fn caption_lang() -> String {
    get("yt.caption-lang")
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| {
            std::env::var("LANG")
                .ok()
                .and_then(|l| l.split(['_', '.', '-']).next().map(str::to_lowercase))
                .filter(|l| l.len() == 2 || l.len() == 3)
                .filter(|l| l != "c")
                .unwrap_or_else(|| "en".to_string())
        })
}

pub fn store_caption_lang(v: &str) {
    put("yt.caption-lang", Some(v.trim()).filter(|x| !x.is_empty()));
}

/// Skip sponsor segments while playing. On until turned off.
pub fn sponsor_skip() -> bool {
    get("yt.sponsor-skip").is_none_or(|v| v != "0")
}

pub fn store_sponsor_skip(on: bool) {
    put("yt.sponsor-skip", Some(if on { "1" } else { "0" }));
}

/// Leave watched videos out of the feeds and channel pages.
pub fn hide_watched() -> bool {
    get("yt.hide-watched").is_some_and(|v| v == "1")
}

pub fn store_hide_watched(on: bool) {
    put("yt.hide-watched", Some(if on { "1" } else { "0" }));
}

/// Listing backend: "auto" (default), "piped" or "ytdlp".
pub fn fetcher() -> String {
    get("music.yt.fetcher")
        .filter(|v| v == "piped" || v == "ytdlp")
        .unwrap_or_else(|| "auto".to_string())
}

pub fn store_fetcher(v: &str) {
    put("music.yt.fetcher", Some(v));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pinning is de-duping, newest-first and capped — the three things a
    /// second copy of this logic in another crate would get subtly wrong.
    #[test]
    fn home_pin_order_is_newest_first_deduped_and_capped() {
        let apply = |mut list: Vec<String>, id: &str| {
            list.retain(|x| x != id);
            list.insert(0, id.to_string());
            list.truncate(HOME_MAX);
            list
        };
        let mut list: Vec<String> = Vec::new();
        for i in 0..12 {
            list = apply(list, &format!("c{i}"));
        }
        assert_eq!(list.len(), HOME_MAX, "capped");
        assert_eq!(list[0], "c11", "newest first");
        assert!(!list.contains(&"c0".to_string()), "oldest dropped");

        // Re-pinning an existing channel moves it to the top, not a duplicate.
        list = apply(list, "c5");
        assert_eq!(list[0], "c5");
        assert_eq!(list.iter().filter(|x| *x == "c5").count(), 1);
        assert_eq!(list.len(), HOME_MAX);
    }
}
