//! Live TV — free-to-air channels from iptv-org.
//!
//! Ported from MovieBox-Tui v0.1.7 `src/providers/iptv_org` (MIT OR
//! Apache-2.0, https://github.com/mesamirh/MovieBox-Tui). Changes from upstream:
//!
//! * The playlist cache is keyed on the playlist URL rather than on its
//!   filename. Two iptv-org lists can share a basename (`us.m3u` exists under
//!   both `countries/` and, historically, `regions/`), and a filename key made
//!   one silently serve the other.
//! * Selected playlists persist through the app's own `Settings` rather than a
//!   private JSON file, so they round-trip with everything else the user has
//!   configured.
//!
//! Everything iptv-org publishes is a plain `.m3u`, so this is a text parser
//! and a day-long disk cache. There is no API, no key, and nothing to sign.

pub mod data;

use std::path::PathBuf;
use tulipix_core::settings::Settings;

/// Where the lists live. Whole thing is a static site on GitHub Pages.
const ROOT: &str = "https://iptv-org.github.io/iptv";

/// How long a downloaded playlist is trusted. iptv-org regenerates daily; a
/// shorter window means re-downloading a megabyte to learn nothing changed.
const CACHE_TTL_SECS: u64 = 24 * 60 * 60;

/// Settings key for the chosen playlists, newline-separated URLs.
pub const SELECTION_KEY: &str = "livetv.playlists";

/// One channel out of a playlist.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub logo: String,
    /// The `group-title` the playlist put it in — usually a genre.
    pub group: String,
    pub url: String,
}

/// How a playlist is picked in the UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Category,
    Language,
    Country,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Category => "Category",
            Kind::Language => "Language",
            Kind::Country => "Country",
        }
    }

    fn path(self) -> &'static str {
        match self {
            Kind::Category => "categories",
            Kind::Language => "languages",
            Kind::Country => "countries",
        }
    }
}

/// Every playlist that can be picked, as `(kind, display name, url)`.
///
/// Built from the vendored index rather than fetched: it is thousands of rows
/// and it changes about once a year.
pub fn catalogue() -> Vec<(Kind, String, String)> {
    let mut out = Vec::with_capacity(
        data::CATEGORIES.len() + data::LANGUAGES.len() + data::COUNTRIES.len(),
    );
    for name in data::CATEGORIES {
        out.push((Kind::Category, (*name).to_string(), url_for(Kind::Category, &name.to_lowercase())));
    }
    for (name, code) in data::LANGUAGES {
        out.push((Kind::Language, (*name).to_string(), url_for(Kind::Language, code)));
    }
    for (name, code) in data::COUNTRIES {
        out.push((Kind::Country, (*name).to_string(), url_for(Kind::Country, code)));
    }
    out
}

fn url_for(kind: Kind, code: &str) -> String {
    format!("{ROOT}/{}/{code}.m3u", kind.path())
}

// ---- selection ----

/// The playlists the user picked, in the order they were picked.
pub fn selection() -> Vec<String> {
    Settings::load()
        .unwrap_or_default()
        .text(SELECTION_KEY)
        .lines()
        .map(str::trim)
        .filter(|l| is_ours(l))
        .map(str::to_string)
        .collect()
}

/// Replace the selection. Entries that do not point at iptv-org are dropped
/// rather than saved — this list is fed straight to the downloader.
pub fn set_selection(urls: &[String]) -> Result<Vec<String>, String> {
    let mut clean: Vec<String> = Vec::new();
    for u in urls {
        let u = u.trim();
        if is_ours(u) && !clean.iter().any(|c| c == u) {
            clean.push(u.to_string());
        }
    }
    let mut s = Settings::load().unwrap_or_default();
    if clean.is_empty() {
        s.advanced.remove(SELECTION_KEY);
    } else {
        s.advanced.insert(SELECTION_KEY.to_string(), clean.join("\n"));
    }
    s.save().map_err(|e| format!("Could not save: {e}"))?;
    Ok(clean)
}

/// Is this one of iptv-org's own playlist URLs?
fn is_ours(url: &str) -> bool {
    url.starts_with(ROOT) && url.ends_with(".m3u")
}

// ---- fetching ----

/// Channels for every selected playlist, in selection order, de-duplicated on
/// stream URL.
///
/// A failed playlist is skipped rather than failing the batch: picking eight
/// countries and getting seven is better than getting an error.
pub async fn load(http: &reqwest::Client, urls: &[String]) -> Vec<Channel> {
    let mut out: Vec<Channel> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for url in urls {
        let text = match playlist(http, url).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(url = %url, error = %e, "livetv: playlist unavailable");
                continue;
            }
        };
        for ch in parse_m3u(&text) {
            if seen.insert(ch.url.clone()) {
                out.push(ch);
            }
        }
    }
    out
}

/// One playlist's text, from the day-old cache or from the network.
async fn playlist(http: &reqwest::Client, url: &str) -> Result<String, String> {
    if !is_ours(url) {
        return Err("not an iptv-org playlist".into());
    }
    let path = cache_path(url);
    if let Some(fresh) = read_fresh(&path) {
        return Ok(fresh);
    }
    let resp = http.get(url).send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status().as_u16()));
    }
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // A write failure only costs the cache, never the answer.
    let _ = std::fs::write(&path, &text);
    Ok(text)
}

fn read_fresh(path: &std::path::Path) -> Option<String> {
    let age = std::fs::metadata(path).ok()?.modified().ok()?.elapsed().ok()?;
    (age.as_secs() < CACHE_TTL_SECS).then(|| std::fs::read_to_string(path).ok())?
}

/// Cache file for a playlist URL.
///
/// Named from the URL's own path, not its basename: `countries/us.m3u` and
/// `languages/us.m3u` are different lists that share a filename, and one
/// serving the other is a bug that looks like bad data from iptv-org.
fn cache_path(url: &str) -> PathBuf {
    let slug: String = url
        .trim_start_matches(ROOT)
        .trim_start_matches('/')
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' })
        .collect();
    cache_dir().join(slug)
}

/// Falls back to the working directory when the platform has no cache dir —
/// the cache is an optimisation, and losing it must not stop playback.
fn cache_dir() -> PathBuf {
    tulipix_core::paths::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("livetv")
}

/// Drop every cached playlist, so the next load re-downloads.
pub fn clear_cache() -> std::io::Result<()> {
    match std::fs::remove_dir_all(cache_dir()) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

// ---- parsing ----

/// Extended-M3U → channels.
///
/// Format is one `#EXTINF:` line of attributes and a display name, then the
/// stream URL on the next non-comment line. Entries missing a URL are dropped;
/// entries missing a name keep their `tvg-id`, because a channel with no label
/// is still watchable.
pub fn parse_m3u(text: &str) -> Vec<Channel> {
    let mut out = Vec::new();
    let mut current = Channel::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(info) = line.strip_prefix("#EXTINF:") {
            current = Channel {
                id: attr(info, "tvg-id"),
                logo: attr(info, "tvg-logo"),
                group: attr(info, "group-title"),
                // Everything after the last comma is the display name.
                name: info.rsplit_once(',').map(|(_, n)| n.trim().to_string()).unwrap_or_default(),
                url: String::new(),
            };
            continue;
        }
        if line.starts_with('#') {
            continue;
        }
        // Only http(s): a playlist is a downloaded file, and a `file://` entry
        // in one would point the player at the local disk.
        if !(line.starts_with("http://") || line.starts_with("https://")) {
            current = Channel::default();
            continue;
        }
        current.url = line.to_string();
        if current.name.is_empty() {
            current.name = current.id.clone();
        }
        if current.id.is_empty() {
            current.id = current.name.clone();
        }
        if !current.name.is_empty() {
            out.push(std::mem::take(&mut current));
        } else {
            current = Channel::default();
        }
    }
    out
}

/// One `key="value"` attribute off an `#EXTINF:` line.
fn attr(line: &str, key: &str) -> String {
    let needle = format!("{key}=\"");
    let Some(at) = line.find(&needle) else { return String::new() };
    let rest = &line[at + needle.len()..];
    rest.split('"').next().unwrap_or_default().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAYLIST: &str = r#"#EXTM3U
#EXTINF:-1 tvg-id="BBCNews.uk" tvg-logo="https://logo/bbc.png" group-title="News",BBC News
https://cdn.example/bbc/index.m3u8
#EXTINF:-1 tvg-id="" tvg-logo="" group-title="Music",MTV Classic
https://cdn.example/mtv/index.m3u8
#EXTINF:-1 tvg-id="Broken.us" group-title="News",Broken
#EXTINF:-1 tvg-id="Local.us" group-title="News",Local
file:///etc/passwd
"#;

    #[test]
    fn parses_attributes_and_the_trailing_display_name() {
        let ch = parse_m3u(PLAYLIST);
        assert_eq!(ch.len(), 2);
        assert_eq!(ch[0].name, "BBC News");
        assert_eq!(ch[0].id, "BBCNews.uk");
        assert_eq!(ch[0].group, "News");
        assert_eq!(ch[0].logo, "https://logo/bbc.png");
        assert_eq!(ch[0].url, "https://cdn.example/bbc/index.m3u8");
    }

    #[test]
    fn an_entry_with_no_url_and_a_local_path_are_both_dropped() {
        let ch = parse_m3u(PLAYLIST);
        // "Broken" has no URL line; "Local" points at the local disk, which a
        // downloaded playlist has no business doing.
        assert!(ch.iter().all(|c| c.name != "Broken" && c.name != "Local"));
    }

    #[test]
    fn a_missing_id_falls_back_to_the_name() {
        let ch = parse_m3u(PLAYLIST);
        assert_eq!(ch[1].id, "MTV Classic");
    }

    #[test]
    fn playlist_urls_are_built_from_the_vendored_index() {
        let all = catalogue();
        assert!(all.len() > 200, "categories + languages + countries");
        let (_, name, url) = all.iter().find(|(k, n, _)| *k == Kind::Country && n == "India").unwrap();
        assert_eq!(name, "India");
        assert_eq!(url, "https://iptv-org.github.io/iptv/countries/in.m3u");
        let (_, _, cat) = all.iter().find(|(k, n, _)| *k == Kind::Category && n == "News").unwrap();
        assert_eq!(cat, "https://iptv-org.github.io/iptv/categories/news.m3u");
    }

    #[test]
    fn only_iptv_org_urls_are_accepted() {
        assert!(is_ours("https://iptv-org.github.io/iptv/countries/in.m3u"));
        assert!(!is_ours("https://evil.example/list.m3u"), "the selection feeds a downloader");
        assert!(!is_ours("https://iptv-org.github.io/iptv/countries/in.txt"));
    }

    #[test]
    fn two_lists_sharing_a_filename_get_separate_cache_files() {
        // `countries/us.m3u` and `languages/us.m3u` are different playlists.
        let a = cache_path("https://iptv-org.github.io/iptv/countries/us.m3u");
        let b = cache_path("https://iptv-org.github.io/iptv/languages/us.m3u");
        assert_ne!(a, b);
    }
}
