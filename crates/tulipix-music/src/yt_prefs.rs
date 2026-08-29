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
