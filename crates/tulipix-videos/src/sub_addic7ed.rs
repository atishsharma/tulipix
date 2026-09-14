//! Addic7ed — the second subtitle source, for what OpenSubtitles does not
//! have. Addic7ed is where TV subtitles appear first, often the same night.
//!
//! There is no API. The client fetches the episode page and reads the version
//! table out of it:
//!
//! ```text
//! https://www.addic7ed.com/serie/<Show_Name>/<season>/<episode>/<lang id>
//! ```
//!
//! and each version's download link is `/original/<episode id>/<n>` or
//! `/updated/<episode id>/<n>`. Two rules the site enforces and a naive
//! client trips over: a browser `User-Agent` (a default one is answered with
//! an error page) and a `Referer` on the download (without it the link
//! returns the HTML page again, not the subtitle).
//!
//! ponytail: HTML scraped with regexes, so a redesign of their page breaks
//! the parse. The failure is a clean "no versions found" rather than a wrong
//! subtitle, and OpenSubtitles stays the first source. Upgrade path if it
//! turns out to break often: pin the parse to their `ajax_loadShow.php`
//! fragment, which changes less than the page around it.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const A7_BASE: &str = "https://www.addic7ed.com";

/// Addic7ed refuses a request that does not look like a browser.
pub const A7_USER_AGENT: &str =
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

/// Their language ids, for the ones Tulipix offers. Anything else asks for
/// every language and the rows are filtered by name instead.
pub const A7_LANGUAGES: &[(&str, i32, &str)] = &[
    ("en", 1, "English"),
    ("fr", 8, "French"),
    ("es", 4, "Spanish"),
    ("de", 11, "German"),
    ("it", 7, "Italian"),
    ("pt", 10, "Portuguese"),
    ("ru", 19, "Russian"),
    ("nl", 17, "Dutch"),
    ("pl", 21, "Polish"),
    ("tr", 16, "Turkish"),
    ("ar", 38, "Arabic"),
    ("hi", 51, "Hindi"),
];

pub fn language_id(code: &str) -> i32 {
    A7_LANGUAGES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, id, _)| *id)
        .unwrap_or(0)
}

pub fn language_name(code: &str) -> Option<&'static str> {
    A7_LANGUAGES
        .iter()
        .find(|(c, _, _)| *c == code)
        .map(|(_, _, n)| *n)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct A7Subtitle {
    /// The release the version was timed against ("Version DIMENSION").
    pub release: String,
    pub language: String,
    /// `/original/123456/1` — a path, not a URL: the download needs the base
    /// and a Referer that only the caller who searched knows.
    pub link: String,
    /// Addic7ed marks a version still being worked on; those are usually
    /// missing the last few lines.
    pub completed: bool,
    /// How many people have taken it, when the page says.
    pub downloads: i64,
    /// The episode page this came from, which is the Referer the download
    /// needs.
    pub referer: String,
}

/// "The Expanse" → "The_Expanse". Their URLs use underscores and keep the
/// punctuation, so only spaces are touched.
pub fn show_slug(title: &str) -> String {
    title.trim().replace(' ', "_")
}

pub fn episode_url(title: &str, season: i64, episode: i64, lang: &str) -> String {
    format!(
        "{A7_BASE}/serie/{}/{season}/{episode}/{}",
        show_slug(title),
        language_id(lang)
    )
}

pub struct Addic7edClient {
    http: reqwest::Client,
}

impl Default for Addic7edClient {
    fn default() -> Self {
        Self::new()
    }
}

impl Addic7edClient {
    pub fn new() -> Self {
        Self { http: tulipix_core::net::http().clone() }
    }

    /// Every version of one episode, best first. An unknown show, or a season
    /// and episode that do not exist, is an empty list rather than an error:
    /// this is the fallback source and the caller has already shown whatever
    /// OpenSubtitles had.
    pub async fn search(
        &self,
        title: &str,
        season: i64,
        episode: i64,
        lang: &str,
    ) -> Result<Vec<A7Subtitle>> {
        if title.trim().is_empty() || season <= 0 || episode <= 0 {
            return Ok(Vec::new());
        }
        let url = episode_url(title, season, episode, lang);
        let html = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, A7_USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let mut hits = parse_versions(&html, &url);
        if let Some(want) = language_name(lang) {
            hits.retain(|h| h.language.eq_ignore_ascii_case(want));
        }
        // Finished versions first, then the most-taken: on Addic7ed the
        // download count is the closest thing to a quality signal.
        hits.sort_by(|a, b| {
            b.completed
                .cmp(&a.completed)
                .then(b.downloads.cmp(&a.downloads))
        });
        Ok(hits)
    }

    /// Fetch one version. The Referer is not optional: without it Addic7ed
    /// answers with its "you must be logged in / wrong referer" page, which is
    /// HTML and would be saved as a subtitle.
    pub async fn download(&self, link: &str, referer: &str) -> Result<Vec<u8>> {
        let url = if link.starts_with("http") {
            link.to_string()
        } else {
            format!("{A7_BASE}{link}")
        };
        let resp = self
            .http
            .get(&url)
            .header(reqwest::header::USER_AGENT, A7_USER_AGENT)
            .header(reqwest::header::REFERER, referer)
            .send()
            .await?
            .error_for_status()?;
        let bytes = resp.bytes().await?.to_vec();
        if looks_like_html(&bytes) {
            anyhow::bail!(
                "Addic7ed sent its web page instead of the subtitle — it is usually \
                 their daily download limit. Try again later, or use OpenSubtitles."
            );
        }
        Ok(bytes)
    }

    /// Download and write beside the video, the way the OpenSubtitles client
    /// does, so `sub_local` finds it on the next launch.
    pub async fn save_as_sibling(
        &self,
        anchor: &Path,
        hit: &A7Subtitle,
        lang: &str,
    ) -> Result<PathBuf> {
        let bytes = self.download(&hit.link, &hit.referer).await?;
        let stem = anchor
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("subtitle");
        let dir = anchor.parent().unwrap_or(Path::new("."));
        let out = dir.join(format!("{stem}.{lang}.srt"));
        std::fs::write(&out, &bytes).with_context(|| format!("write {}", out.display()))?;
        Ok(out)
    }
}

/// An error page is HTML; a subtitle never is. `.srt` opens with an index or
/// a BOM, `.ass` with `[Script Info]`.
fn looks_like_html(bytes: &[u8]) -> bool {
    let head = String::from_utf8_lossy(&bytes[..bytes.len().min(512)]).to_ascii_lowercase();
    head.contains("<!doctype html") || head.contains("<html")
}

/// The version table. Each version opens with a `NewsTitle` cell and holds one
/// language cell and one download link; the chunk between two `NewsTitle`s is
/// therefore exactly one version.
pub fn parse_versions(html: &str, referer: &str) -> Vec<A7Subtitle> {
    use std::sync::OnceLock;
    static RE: OnceLock<Res> = OnceLock::new();
    struct Res {
        version: regex::Regex,
        language: regex::Regex,
        link: regex::Regex,
        completed: regex::Regex,
        downloads: regex::Regex,
    }
    let re = RE.get_or_init(|| Res {
        // "Version DIMENSION, 1.15 MBs" — everything up to the size.
        version: regex::Regex::new(r"(?is)(Version\s+[^,<]{1,80})").unwrap(),
        language: regex::Regex::new(r#"(?is)<td[^>]*class="language"[^>]*>(.*?)</td>"#).unwrap(),
        link: regex::Regex::new(r#"(?i)href="(/(?:original|updated)/\d+/\d+)""#).unwrap(),
        completed: regex::Regex::new(r"(?i)\b(\d{1,3})%\s*Completed").unwrap(),
        downloads: regex::Regex::new(r"(?i)([\d,]+)\s*Downloads").unwrap(),
    });

    let mut out = Vec::new();
    // Split on the version header, dropping whatever came before the first.
    for chunk in html.split("NewsTitle").skip(1) {
        let Some(link) = re.link.captures(chunk).map(|c| c[1].to_string()) else {
            continue;
        };
        let release = re
            .version
            .captures(chunk)
            .map(|c| c[1].trim().to_string())
            .unwrap_or_else(|| "Unknown release".into());
        let language = re
            .language
            .captures(chunk)
            .map(|c| strip_tags(&c[1]))
            .unwrap_or_default();
        // The page says "80% Completed" only while a version is unfinished;
        // a finished one says "Completed" with no number.
        let completed = re
            .completed
            .captures(chunk)
            .and_then(|c| c[1].parse::<i32>().ok())
            .is_none_or(|pct| pct >= 100);
        let downloads = re
            .downloads
            .captures(chunk)
            .map(|c| c[1].replace(',', ""))
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        out.push(A7Subtitle {
            release,
            language,
            link,
            completed,
            downloads,
            referer: referer.to_string(),
        });
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_tag = false;
    for c in s.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            c if !in_tag => out.push(c),
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_and_url() {
        assert_eq!(show_slug(" The Expanse "), "The_Expanse");
        assert_eq!(
            episode_url("The Expanse", 3, 7, "en"),
            "https://www.addic7ed.com/serie/The_Expanse/3/7/1"
        );
        // A language they have no id for asks for all of them.
        assert!(episode_url("X", 1, 1, "zz").ends_with("/1/1/0"));
    }

    const PAGE: &str = r#"
    <table>
      <tr><td class="NewsTitle" colspan="3">Version DIMENSION, 1.15 MBs</td></tr>
      <tr><td class="language">English</td><td>Completed</td><td>1,204 Downloads</td>
          <td class="buttonDownload"><a href="/original/123456/1">Download</a></td></tr>
      <tr><td class="NewsTitle" colspan="3">Version WEB-DL, 1.02 MBs</td></tr>
      <tr><td class="language">  French  </td><td>80% Completed</td><td>12 Downloads</td>
          <td class="buttonDownload"><a href="/updated/123456/2">Download</a></td></tr>
      <tr><td class="NewsTitle" colspan="3">Version NOLINK, 0 MBs</td></tr>
    </table>"#;

    #[test]
    fn versions_are_read_with_their_language_state_and_count() {
        let v = parse_versions(PAGE, "https://ref");
        // The third has no download link, so it is not a version.
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].release, "Version DIMENSION");
        assert_eq!(v[0].language, "English");
        assert_eq!(v[0].link, "/original/123456/1");
        assert!(v[0].completed);
        assert_eq!(v[0].downloads, 1204);
        assert_eq!(v[1].language, "French");
        assert!(!v[1].completed, "80% is not finished");
        assert_eq!(v[1].referer, "https://ref");
    }

    #[test]
    fn html_is_recognised_so_it_is_never_written_as_a_subtitle() {
        assert!(looks_like_html(b"<!DOCTYPE html><html>"));
        assert!(!looks_like_html(b"1\n00:00:01,000 --> 00:00:02,000\nHello\n"));
        assert!(!looks_like_html(b""));
    }

    #[test]
    fn tags_come_out_of_a_cell() {
        assert_eq!(strip_tags("  <b>English</b> \n"), "English");
    }
}
