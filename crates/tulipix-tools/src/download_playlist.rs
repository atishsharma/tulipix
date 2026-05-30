//! `np.p4.tools.download.playlist` — playlist/channel batch download.
//!
//! Count/date filters, regex on title, resumable, dedup by URL hash. Builds
//! the yt-dlp argv for a batch and owns the client-side dedup (so a re-run
//! skips already-downloaded entries by URL hash).

use sha2::{Digest, Sha256};
use std::collections::HashSet;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PlaylistFilter {
    pub max_items: Option<u32>,
    /// `playlist_items` range like "1-50" or None.
    pub items_range: Option<String>,
    /// Only entries after this upload date (yt-dlp `--dateafter`, YYYYMMDD).
    pub date_after: Option<String>,
    /// Title regex (`--match-title`).
    pub title_regex: Option<String>,
}

/// Build yt-dlp argv for the batch (resumable via `--download-archive`).
pub fn args(url: &str, filter: &PlaylistFilter, archive_file: &str, out_template: &str) -> Vec<String> {
    let mut a = vec![
        "--yes-playlist".into(),
        "--download-archive".into(), archive_file.into(),
        "-o".into(), out_template.into(),
    ];
    if let Some(n) = filter.max_items { a.push("--max-downloads".into()); a.push(n.to_string()); }
    if let Some(r) = &filter.items_range { a.push("--playlist-items".into()); a.push(r.clone()); }
    if let Some(d) = &filter.date_after { a.push("--dateafter".into()); a.push(d.clone()); }
    if let Some(re) = &filter.title_regex { a.push("--match-title".into()); a.push(re.clone()); }
    a.push(url.into());
    a
}

/// Stable dedup key for an entry URL.
pub fn url_hash(url: &str) -> String {
    Sha256::digest(url.as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Filter out URLs whose hash is already in `seen`; returns the new ones and
/// updates `seen`.
pub fn dedup<'a>(urls: &'a [String], seen: &mut HashSet<String>) -> Vec<&'a String> {
    urls.iter().filter(|u| seen.insert(url_hash(u))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_carries_filters() {
        let f = PlaylistFilter { max_items: Some(10), date_after: Some("20240101".into()), title_regex: Some("Live".into()), ..Default::default() };
        let a = args("https://y/pl", &f, "arch.txt", "%(title)s.%(ext)s");
        assert!(a.windows(2).any(|w| w == ["--max-downloads", "10"]));
        assert!(a.windows(2).any(|w| w == ["--dateafter", "20240101"]));
        assert!(a.contains(&"--download-archive".to_string()));
    }

    #[test]
    fn dedup_skips_seen() {
        let mut seen = HashSet::new();
        let urls = vec!["a".to_string(), "b".to_string(), "a".to_string()];
        let new = dedup(&urls, &mut seen);
        assert_eq!(new.len(), 2);
        // re-run sees nothing new
        assert!(dedup(&urls, &mut seen).is_empty());
    }
}
