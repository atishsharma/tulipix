//! `np.p4.tools.download` — URL downloader (bundled yt-dlp).
//!
//! YouTube/Vimeo/Twitch/SoundCloud/1800+ sites. Builds the yt-dlp argv from a
//! download spec: quality/format selection, subtitle + thumbnail embedding,
//! and cookies-from-browser for gated content.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DownloadSpec {
    pub url: String,
    /// Max video height (e.g. 1080); None = best.
    pub max_height: Option<u32>,
    pub audio_only: bool,
    pub embed_subs: bool,
    pub embed_thumbnail: bool,
    /// Browser name for `--cookies-from-browser` (e.g. "firefox"); None = off.
    pub cookies_from: Option<String>,
    /// Output template (yt-dlp `-o`).
    pub out_template: String,
}

impl DownloadSpec {
    pub fn format_selector(&self) -> String {
        if self.audio_only { return "bestaudio".into(); }
        match self.max_height {
            Some(h) => format!("bestvideo[height<=?{h}]+bestaudio/best[height<=?{h}]"),
            None => "bestvideo+bestaudio/best".into(),
        }
    }

    pub fn args(&self) -> Vec<String> {
        let mut a = vec!["-f".into(), self.format_selector(), "-o".into(), self.out_template.clone()];
        if self.audio_only { a.extend(["-x".into(), "--audio-format".into(), "opus".into()]); }
        if self.embed_subs { a.extend(["--write-subs".into(), "--embed-subs".into()]); }
        if self.embed_thumbnail { a.push("--embed-thumbnail".into()); }
        if let Some(b) = &self.cookies_from { a.push("--cookies-from-browser".into()); a.push(b.clone()); }
        a.push(self.url.clone());
        a
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> DownloadSpec {
        DownloadSpec { url: "https://y/x".into(), max_height: Some(1080), audio_only: false,
            embed_subs: true, embed_thumbnail: true, cookies_from: Some("firefox".into()),
            out_template: "%(title)s.%(ext)s".into() }
    }

    #[test]
    fn format_selector_respects_height() {
        assert!(spec().format_selector().contains("height<=?1080"));
        let mut s = spec(); s.audio_only = true;
        assert_eq!(s.format_selector(), "bestaudio");
    }

    #[test]
    fn argv_includes_options() {
        let a = spec().args();
        assert!(a.contains(&"--embed-subs".to_string()));
        assert!(a.contains(&"--embed-thumbnail".to_string()));
        assert!(a.windows(2).any(|w| w == ["--cookies-from-browser", "firefox"]));
        assert_eq!(a.last().unwrap(), "https://y/x");
    }
}
