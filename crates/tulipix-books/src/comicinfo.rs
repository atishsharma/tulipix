//! `ComicInfo.xml` — the de-facto metadata standard shipped inside almost every
//! scanlated CBZ/CBR. Comic archives carry no other embedded metadata, so
//! without this a manga library is filename-only.
//!
//! Two things matter beyond the usual fields:
//!   * `<Manga>YesAndRightToLeft</Manga>` sets right-to-left page order, so the
//!     reader gets manga direction right without the user toggling it.
//!   * `<Pages>` can flag double-page spreads (or carry each page's pixel
//!     size), which is what lets the reader show a spread on its own instead of
//!     splitting it down the fold.

use crate::epub::{attr, tag_text, tags};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Default, Clone)]
pub struct ComicInfo {
    pub series: String,
    /// Issue / chapter number ("12", "12.5").
    pub number: String,
    pub volume: String,
    pub title: String,
    pub writer: String,
    pub publisher: String,
    pub summary: String,
    pub genre: String,
    pub year: String,
    /// Right-to-left page order (`Manga` = `YesAndRightToLeft`).
    pub rtl: bool,
    /// Per-page shape, index-aligned with the archive's sorted page list.
    /// Empty when `<Pages>` is absent — the reader then probes lazily.
    pub pages: Vec<PageInfo>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct PageInfo {
    pub width: u32,
    pub height: u32,
    /// Explicitly tagged `DoublePage="true"`.
    pub double: bool,
}

impl PageInfo {
    /// Does this page occupy a full two-page spread? Either the archive says so
    /// outright, or its own proportions do (landscape art in a portrait book).
    pub fn is_wide(&self) -> bool {
        self.double || (self.width > 0 && self.height > 0 && self.width > self.height)
    }
}

impl ComicInfo {
    /// Display title: an explicit `<Title>`, else "Series #Number", else series.
    pub fn display_title(&self) -> String {
        if !self.title.trim().is_empty() {
            return self.title.trim().to_string();
        }
        match (self.series.trim(), self.number.trim()) {
            ("", _) => String::new(),
            (s, "") => s.to_string(),
            (s, n) => format!("{s} #{n}"),
        }
    }
}

/// Read + parse `ComicInfo.xml` out of a comic archive. `None` when absent or
/// unreadable — callers fall back to filename parsing.
pub fn read(path: &Path, format: &str) -> Option<ComicInfo> {
    let xml = match format {
        "cbz" => from_zip(path),
        // rar / 7z / tar all go out to an external tool.
        "cbr" | "cb7" | "cbt" => from_external(path, format),
        _ => None,
    }?;
    Some(parse(&xml))
}

fn from_zip(path: &Path) -> Option<String> {
    use std::io::Read;
    let f = std::fs::File::open(path).ok()?;
    let mut z = zip::ZipArchive::new(f).ok()?;
    // The name is conventionally at the archive root, but casing varies and
    // some packers nest it — match on the file name only.
    let idx = (0..z.len()).find(|&i| {
        z.by_index(i)
            .ok()
            .map(|e| {
                e.name()
                    .rsplit('/')
                    .next()
                    .unwrap_or("")
                    .eq_ignore_ascii_case("comicinfo.xml")
            })
            .unwrap_or(false)
    })?;
    let mut e = z.by_index(idx).ok()?;
    let mut s = String::new();
    e.read_to_string(&mut s).ok()?;
    Some(s)
}

fn from_external(path: &Path, format: &str) -> Option<String> {
    // Same tool pair the page extractor uses; -inul silences unrar's banner.
    // bsdtar covers 7z and tar, and usually rar as well.
    let try_extract = |cmd: &str, args: &[&str], name: &str| -> Option<String> {
        let out = Command::new(cmd).args(args).arg(path).arg(name).output().ok()?;
        (out.status.success() && !out.stdout.is_empty())
            .then(|| String::from_utf8_lossy(&out.stdout).to_string())
    };
    for name in ["ComicInfo.xml", "comicinfo.xml"] {
        if format == "cbr" {
            if let Some(s) = try_extract("unrar", &["p", "-inul"], name) {
                return Some(s);
            }
        }
        if let Some(s) = try_extract("bsdtar", &["-xOf"], name) {
            return Some(s);
        }
    }
    None
}

pub fn parse(xml: &str) -> ComicInfo {
    let text = |t: &str| tag_text(xml, t).unwrap_or_default();
    let manga = text("Manga");
    let mut info = ComicInfo {
        series: text("Series"),
        number: text("Number"),
        volume: text("Volume"),
        title: text("Title"),
        // Writer is the usual credit; fall back through the other creator roles
        // so a book never shows a blank author when someone is credited.
        writer: [
            text("Writer"),
            text("Penciller"),
            text("Artist"),
            text("CoverArtist"),
        ]
        .into_iter()
        .find(|s| !s.trim().is_empty())
        .unwrap_or_default(),
        publisher: text("Publisher"),
        summary: text("Summary"),
        genre: text("Genre"),
        year: text("Year"),
        rtl: manga.eq_ignore_ascii_case("YesAndRightToLeft"),
        pages: Vec::new(),
    };

    // <Pages><Page Image="0" ImageWidth="1920" ImageHeight="1200"
    //             DoublePage="true" Type="FrontCover"/>…</Pages>
    // `Image` is the 0-based index into the archive's sorted page list.
    let raw = tags(xml, "Page");
    if !raw.is_empty() {
        let mut pages: Vec<PageInfo> = Vec::new();
        for t in raw {
            let idx = attr(t, "Image")
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(pages.len());
            let num = |n: &str| attr(t, n).and_then(|v| v.trim().parse::<u32>().ok()).unwrap_or(0);
            let double = attr(t, "DoublePage")
                .map(|v| v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let info = PageInfo { width: num("ImageWidth"), height: num("ImageHeight"), double };
            if idx >= pages.len() {
                pages.resize(idx + 1, PageInfo::default());
            }
            pages[idx] = info;
        }
        info.pages = pages;
    }
    info
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0"?>
<ComicInfo>
  <Series>Berserk</Series>
  <Number>12</Number>
  <Volume>3</Volume>
  <Writer>Kentaro Miura</Writer>
  <Publisher>Hakusensha</Publisher>
  <Genre>Dark Fantasy</Genre>
  <Year>1993</Year>
  <Manga>YesAndRightToLeft</Manga>
  <Pages>
    <Page Image="0" ImageWidth="1200" ImageHeight="1800" Type="FrontCover"/>
    <Page Image="1" ImageWidth="2400" ImageHeight="1800" DoublePage="true"/>
    <Page Image="2" ImageWidth="1200" ImageHeight="1800"/>
  </Pages>
</ComicInfo>"#;

    #[test]
    fn parses_fields_and_direction() {
        let c = parse(SAMPLE);
        assert_eq!(c.series, "Berserk");
        assert_eq!(c.number, "12");
        assert_eq!(c.writer, "Kentaro Miura");
        assert_eq!(c.genre, "Dark Fantasy");
        assert_eq!(c.year, "1993");
        assert!(c.rtl, "YesAndRightToLeft must set right-to-left order");
        assert_eq!(c.display_title(), "Berserk #12");
    }

    #[test]
    fn detects_wide_pages() {
        let c = parse(SAMPLE);
        assert_eq!(c.pages.len(), 3);
        assert!(!c.pages[0].is_wide());
        // Both the explicit flag and the landscape proportions agree here.
        assert!(c.pages[1].is_wide());
        assert!(!c.pages[2].is_wide());
    }

    #[test]
    fn landscape_without_flag_is_still_wide() {
        let c = parse(
            r#"<ComicInfo><Pages><Page Image="0" ImageWidth="2400" ImageHeight="1600"/></Pages></ComicInfo>"#,
        );
        assert!(c.pages[0].is_wide());
    }

    #[test]
    fn series_does_not_match_seriesgroup() {
        // Both are real ComicInfo fields and SeriesGroup can come first.
        let c = parse("<ComicInfo><SeriesGroup>Shonen</SeriesGroup><Series>Naruto</Series></ComicInfo>");
        assert_eq!(c.series, "Naruto");
    }

    #[test]
    fn missing_file_shape_is_empty_not_panic() {
        let c = parse("<ComicInfo><Series>X</Series></ComicInfo>");
        assert!(c.pages.is_empty());
        assert!(!c.rtl);
        assert_eq!(c.display_title(), "X");
    }
}
