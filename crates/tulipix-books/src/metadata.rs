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
}

pub fn extract(path: &Path, format: &str) -> BookMeta {
    let mut m = match format {
        "epub" => epub_meta(path).unwrap_or_default(),
        "mobi" | "azw3" => mobi_meta(path).unwrap_or_default(),
        "pdf" => pdf_meta(path).unwrap_or_default(),
        _ => BookMeta::default(), // cbz/cbr: filename only
    };
    if m.title.trim().is_empty() {
        m.title = title_from_stem(path);
    }
    m
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
    };
    // Calibre series convention: <meta name="calibre:series" content="…"/>
    for t in epub::tags(opf, "meta") {
        if epub::attr(t, "name").as_deref() == Some("calibre:series") {
            m.series = epub::attr(t, "content").unwrap_or_default();
        }
    }
    Ok(m)
}

fn mobi_meta(path: &Path) -> Result<BookMeta> {
    let m = mobi::Mobi::from_path(path)?;
    Ok(BookMeta {
        title: m.title(),
        author: m.author().unwrap_or_default(),
        genre: m.metadata.subjects().map(|s| s.join(", ")).unwrap_or_default(),
        series: String::new(),
        published: m.publish_date().map(|d| d.chars().take(10).collect()).unwrap_or_default(),
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stem_title() {
        assert_eq!(
            title_from_stem(Path::new("/x/A_Brief-History.of.Time.epub")),
            "A Brief History of Time"
        );
    }
}
