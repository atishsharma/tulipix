//! `np.p4.music.yt-search` — in-Music YouTube/SoundCloud search via bundled
//! yt-dlp.
//!
//! Builds the yt-dlp argv for a search (`ytsearchN:` / `scsearchN:`) and parses
//! the `--dump-json` line stream into lightweight results. Stream + cache and
//! library-routing happen elsewhere; this owns argv construction + parsing.

use serde::Deserialize;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Source { YouTube, SoundCloud }

impl Source {
    fn prefix(self, n: u32) -> String {
        match self { Source::YouTube => format!("ytsearch{n}:"), Source::SoundCloud => format!("scsearch{n}:") }
    }
}

/// yt-dlp argv to enumerate `n` search hits as JSON (no download), audio-biased.
pub fn search_args(source: Source, query: &str, n: u32) -> Vec<String> {
    vec![
        "--dump-json".into(),
        "--flat-playlist".into(),
        "--no-warnings".into(),
        format!("{}{}", source.prefix(n), query),
    ]
}

/// argv to stream + cache the best audio of one URL to `out` as Opus.
pub fn stream_cache_args(url: &str, out: &str) -> Vec<String> {
    vec![
        "-f".into(), "bestaudio".into(),
        "-x".into(), "--audio-format".into(), "opus".into(),
        "-o".into(), out.into(),
        url.into(),
    ]
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct SearchHit {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub duration: Option<f64>,
}

/// Parse the newline-delimited JSON yt-dlp emits, skipping blank/garbage lines.
pub fn parse_dump_json(stdout: &str) -> Vec<SearchHit> {
    stdout.lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str::<SearchHit>(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_argv_uses_prefix() {
        let a = search_args(Source::YouTube, "miles davis", 5);
        assert!(a.contains(&"ytsearch5:miles davis".to_string()));
        let s = search_args(Source::SoundCloud, "lofi", 3);
        assert!(s.last().unwrap().starts_with("scsearch3:"));
    }

    #[test]
    fn cache_argv_extracts_opus() {
        let a = stream_cache_args("https://y/x", "/c/x.opus");
        assert!(a.contains(&"opus".to_string()));
        assert_eq!(a.last().unwrap(), "https://y/x");
    }

    #[test]
    fn parses_ndjson() {
        let out = "{\"id\":\"a\",\"title\":\"One\",\"duration\":100}\n\n{\"id\":\"b\",\"title\":\"Two\"}\ngarbage";
        let hits = parse_dump_json(out);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].duration, Some(100.0));
        assert_eq!(hits[1].id, "b");
    }
}
