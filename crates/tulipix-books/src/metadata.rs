//! Per-format metadata extraction. Best-effort — every field falls back to
//! something sane (title ← file stem) so a scan never fails on a bad file.

use crate::epub;
use anyhow::Result;
use std::path::Path;

#[derive(Debug, Default, Clone)]
pub struct BookMeta {
    pub title: String,
    pub author: String,
    pub genre: String,
    pub series: String,
    pub published: String,
    /// Right-to-left page order (manga). Only `ComicInfo.xml` sets this today;
    /// everything else leaves the user's toggle alone.
    pub rtl: bool,
    /// Publisher-supplied blurb (ComicInfo `<Summary>`), seeding `books.summary`
    /// so the detail popup has something before any online fetch.
    pub summary: String,
}

pub fn extract(path: &Path, format: &str) -> BookMeta {
    let mut m = match format {
        "epub" => epub_meta(path).unwrap_or_default(),
        "mobi" | "azw3" => mobi_meta(path).unwrap_or_default(),
        "pdf" => pdf_meta(path).unwrap_or_default(),
        "cbz" | "cbr" | "cb7" | "cbt" => comic_meta(path, format),
        "fb2" => fb2_meta(path).unwrap_or_default(),
        _ => BookMeta::default(),
    };
    // No embedded series (the norm for comics without ComicInfo, and for most
    // PDFs) — fall back to the filename, which is where manga libraries
    // actually keep it. Never overrides real metadata.
    if m.series.trim().is_empty() {
        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            let (series, _num) = series_from_filename(stem);
            m.series = series;
        }
    }
    if m.title.trim().is_empty() {
        m.title = title_from_stem(path);
    }
    m
}

/// Comic archives: `ComicInfo.xml` when present, else filename only.
fn comic_meta(path: &Path, format: &str) -> BookMeta {
    let Some(c) = crate::comicinfo::read(path, format) else { return BookMeta::default() };
    BookMeta {
        title: c.display_title(),
        author: c.writer.clone(),
        genre: c.genre.clone(),
        series: c.series.clone(),
        published: c.year.clone(),
        rtl: c.rtl,
        summary: c.summary.clone(),
    }
}

/// Series name + volume/chapter number guessed from a filename.
///
/// Comic archives usually carry no embedded metadata at all, so for manga the
/// filename *is* the catalogue: `Berserk v03`, `[Group] Naruto - 05 (2010)`,
/// `One Piece c1044`. Returns `("", "")` when nothing looks like a numbered
/// entry — a lone title must not be mistaken for a series.
pub fn series_from_filename(stem: &str) -> (String, String) {
    // Drop scanlation-group tags, years, and quality markers: [..] (..) {..}.
    let mut cleaned = String::with_capacity(stem.len());
    let mut depth = 0i32;
    for c in stem.chars() {
        match c {
            '[' | '(' | '{' => depth += 1,
            ']' | ')' | '}' => depth = (depth - 1).max(0),
            _ if depth == 0 => cleaned.push(if c == '_' || c == '.' { ' ' } else { c }),
            _ => {}
        }
    }
    let tokens: Vec<String> =
        cleaned.split_whitespace().map(|t| t.trim_matches('-').to_string()).filter(|t| !t.is_empty()).collect();
    if tokens.is_empty() {
        return (String::new(), String::new());
    }

    // A volume marker is either "<word><digits>" (v03, c1044, ch7) or a
    // separate keyword followed by a number ("Vol. 12", "Chapter 7"), or a
    // bare trailing number ("Naruto - 05").
    let keyword = |t: &str| {
        matches!(
            t.trim_end_matches('.').to_ascii_lowercase().as_str(),
            "v" | "vol" | "volume" | "c" | "ch" | "chapter" | "#"
        )
    };
    let glued = |t: &str| -> Option<String> {
        let low = t.to_ascii_lowercase();
        let prefix: String = low.chars().take_while(|c| c.is_ascii_alphabetic()).collect();
        let digits: String = low.chars().skip(prefix.len()).collect();
        (matches!(prefix.as_str(), "v" | "vol" | "c" | "ch")
            && !digits.is_empty()
            && digits.chars().all(|c| c.is_ascii_digit()))
        .then_some(digits)
    };

    // Scan from the end so a number inside the title ("Gantz 2") loses to a
    // real trailing marker.
    for i in (0..tokens.len()).rev() {
        let t = &tokens[i];
        if let Some(num) = glued(t) {
            return (join_series(&tokens[..i]), strip_zeros(&num));
        }
        if t.chars().all(|c| c.is_ascii_digit()) && !t.is_empty() {
            // "Vol 12" / "Chapter 7" — the keyword sits just before.
            let start = if i > 0 && keyword(&tokens[i - 1]) { i - 1 } else { i };
            // A bare number only counts when it ends the name, otherwise it is
            // part of the title ("Death Note 2 Special" is not volume 2).
            if start == i && i + 1 != tokens.len() {
                continue;
            }
            let series = join_series(&tokens[..start]);
            if series.is_empty() {
                return (String::new(), String::new());
            }
            return (series, strip_zeros(t));
        }
    }
    (String::new(), String::new())
}

fn join_series(tokens: &[String]) -> String {
    tokens.join(" ").trim().trim_end_matches(['-', '–', ',']).trim().to_string()
}

/// "03" → "3" (but "0" stays "0").
fn strip_zeros(n: &str) -> String {
    let t = n.trim_start_matches('0');
    if t.is_empty() { "0".into() } else { t.into() }
}

/// "A_Brief-History.of.Time" → "A Brief History of Time".
fn title_from_stem(path: &Path) -> String {
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Untitled");
    let cleaned: String = stem
        .chars()
        .map(|c| if c == '_' || c == '.' || c == '-' { ' ' } else { c })
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.is_empty() { "Untitled".into() } else { cleaned }
}

fn epub_meta(path: &Path) -> Result<BookMeta> {
    let doc = epub::open(path)?;
    let opf = &doc.opf;
    let mut m = BookMeta {
        title: epub::tag_text(opf, "dc:title").unwrap_or_default(),
        author: epub::tag_text(opf, "dc:creator").unwrap_or_default(),
        genre: epub::tag_text(opf, "dc:subject").unwrap_or_default(),
        series: String::new(),
        published: epub::tag_text(opf, "dc:date")
            .map(|d| d.chars().take(10).collect())
            .unwrap_or_default(),
        ..Default::default()
    };
    // Calibre series convention: <meta name="calibre:series" content="…"/>
    for t in epub::tags(opf, "meta") {
        if epub::attr(t, "name").as_deref() == Some("calibre:series") {
            m.series = epub::attr(t, "content").unwrap_or_default();
        }
    }
    Ok(m)
}

fn fb2_meta(path: &Path) -> Result<BookMeta> {
    let xml = crate::fb2::read_xml(path)?;
    let m = crate::fb2::meta(&xml);
    Ok(BookMeta {
        title: m.title,
        author: m.author,
        genre: m.genre,
        series: m.series,
        published: m.published,
        ..Default::default()
    })
}

fn mobi_meta(path: &Path) -> Result<BookMeta> {
    let m = mobi::Mobi::from_path(path)?;
    Ok(BookMeta {
        title: m.title(),
        author: m.author().unwrap_or_default(),
        genre: m.metadata.subjects().map(|s| s.join(", ")).unwrap_or_default(),
        series: String::new(),
        published: m.publish_date().map(|d| d.chars().take(10).collect()).unwrap_or_default(),
        ..Default::default()
    })
}

/// PDF: `pdfinfo` when available (poppler-utils); otherwise stem only.
fn pdf_meta(path: &Path) -> Result<BookMeta> {
    let out = std::process::Command::new("pdfinfo").arg(path).output()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let field = |k: &str| -> String {
        text.lines()
            .find(|l| l.starts_with(k))
            .and_then(|l| l.split_once(':'))
            .map(|(_, v)| v.trim().to_string())
            .unwrap_or_default()
    };
    Ok(BookMeta {
        title: field("Title"),
        author: field("Author"),
        genre: field("Subject"),
        series: String::new(),
        published: field("CreationDate")
            .split_whitespace()
            .skip(1)
            .take(3)
            .collect::<Vec<_>>()
            .join(" "),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_series_and_volume() {
        let cases = [
            ("Berserk v03", "Berserk", "3"),
            ("Berserk Vol. 12", "Berserk", "12"),
            ("One Piece c1044", "One Piece", "1044"),
            ("Akira Chapter 7", "Akira", "7"),
            ("[Group] Naruto - 05 (2010)", "Naruto", "5"),
            ("Vinland_Saga_v02", "Vinland Saga", "2"),
            ("Attack on Titan ch09", "Attack on Titan", "9"),
        ];
        for (stem, series, num) in cases {
            assert_eq!(series_from_filename(stem), (series.into(), num.into()), "stem: {stem}");
        }
    }

    #[test]
    fn plain_titles_are_not_invented_into_series() {
        // No number → not a series entry.
        assert_eq!(series_from_filename("Dune"), (String::new(), String::new()));
        assert_eq!(series_from_filename("A Brief History of Time"), (String::new(), String::new()));
        // A number mid-title isn't a volume marker.
        assert_eq!(series_from_filename("Death Note 2 Special"), (String::new(), String::new()));
        // A bare number with nothing before it has no series to name.
        assert_eq!(series_from_filename("05"), (String::new(), String::new()));
    }

    #[test]
    fn stem_title() {
        assert_eq!(
            title_from_stem(Path::new("/x/A_Brief-History.of.Time.epub")),
            "A Brief History of Time"
        );
    }
}
