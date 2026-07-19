//! FictionBook 2 (`.fb2`, `.fb2.zip`) — a single XML file holding metadata,
//! the body text, and any images inline as base64. Dominant in Eastern
//! European ebook libraries and reflows through the normal paginator.
//!
//! Parsed with the same hand-rolled scanners as `epub` (no XML dependency).

use crate::epub::{attr, html_to_text, tag_text, tags};
use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

/// Whole-file XML for an FB2, transparently unzipping a `.fb2.zip`.
pub fn read_xml(path: &Path) -> Result<String> {
    let is_zip = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("zip"))
        .unwrap_or(false);
    if !is_zip {
        return Ok(String::from_utf8_lossy(&std::fs::read(path)?).into_owned());
    }
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    // Conventionally one .fb2 inside; take the first regardless of name.
    let idx = (0..z.len())
        .find(|&i| {
            z.by_index(i)
                .ok()
                .map(|e| e.name().to_ascii_lowercase().ends_with(".fb2"))
                .unwrap_or(false)
        })
        .context("no .fb2 inside archive")?;
    let mut e = z.by_index(idx)?;
    let mut buf = Vec::new();
    e.read_to_end(&mut buf)?;
    // FB2 is often windows-1251; from_utf8_lossy keeps the ASCII structure
    // readable either way, which is all the scanners below need.
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

#[derive(Debug, Default, Clone)]
pub struct Fb2Meta {
    pub title: String,
    pub author: String,
    pub genre: String,
    pub series: String,
    pub published: String,
}

/// `<description><title-info>` fields.
pub fn meta(xml: &str) -> Fb2Meta {
    // Restrict to <description> so a chapter's <title> can't win over the
    // book title (both use the same element name).
    let desc = section_of(xml, "description").unwrap_or_else(|| xml.to_string());
    let author = {
        let first = tag_text(&desc, "first-name").unwrap_or_default();
        let last = tag_text(&desc, "last-name").unwrap_or_default();
        let joined = format!("{first} {last}");
        let joined = joined.trim().to_string();
        if joined.is_empty() {
            tag_text(&desc, "nickname").unwrap_or_default()
        } else {
            joined
        }
    };
    Fb2Meta {
        title: tag_text(&desc, "book-title").unwrap_or_default(),
        author,
        genre: tag_text(&desc, "genre").unwrap_or_default(),
        // <sequence name="Series" number="3"/>
        series: tags(&desc, "sequence")
            .first()
            .and_then(|t| attr(t, "name"))
            .unwrap_or_default(),
        published: tags(&desc, "date")
            .first()
            .and_then(|t| attr(t, "value"))
            .or_else(|| tag_text(&desc, "date"))
            .unwrap_or_default(),
    }
}

/// Inner XML of the first `<tag>…</tag>`, tag excluded.
fn section_of(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let i = xml.find(&open)?;
    let after = &xml[i..];
    let gt = after.find('>')?;
    let body = &after[gt + 1..];
    let end = body.find(&close)?;
    Some(body[..end].to_string())
}

/// Chapters as `(title, text)`, one per top-level `<section>` of `<body>`.
/// A book with no sections becomes a single chapter, so it still reads.
pub fn chapters(xml: &str) -> Vec<(String, String)> {
    let Some(body) = section_of(xml, "body") else { return Vec::new() };
    let parts = split_sections(&body);
    let mut out: Vec<(String, String)> = parts
        .into_iter()
        .map(|s| {
            let title = section_of(&s, "title").map(|t| html_to_text(&t)).unwrap_or_default();
            // First line only — a <title> can hold several <p>.
            let title = title.lines().next().unwrap_or("").trim().to_string();
            (title, html_to_text(&s))
        })
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    if out.is_empty() {
        let text = html_to_text(&body);
        if !text.trim().is_empty() {
            out.push((String::new(), text));
        }
    }
    out
}

/// Split a `<body>` into its *top-level* `<section>` blocks, tracking nesting
/// so a subsection never ends its parent early.
fn split_sections(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut rest = body;
    let mut base = 0usize;
    while let Some(rel) = rest.find('<') {
        let at = base + rel;
        let tail = &body[at..];
        let Some(close_rel) = tail.find('>') else { break };
        let tag = &tail[1..close_rel];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect::<String>()
            .to_ascii_lowercase();
        if name == "section" && !tag.starts_with('/') && !tag.ends_with('/') {
            if depth == 0 {
                start = at;
            }
            depth += 1;
        } else if name == "section" && tag.starts_with('/') {
            depth = depth.saturating_sub(1);
            if depth == 0 {
                out.push(body[start..at + close_rel + 1].to_string());
            }
        }
        base = at + close_rel + 1;
        rest = &body[base..];
    }
    out
}

/// Cover image bytes: `<coverpage><image l:href="#id"/></coverpage>` resolved
/// against the matching `<binary id="id">base64</binary>`.
pub fn cover_bytes(xml: &str) -> Option<Vec<u8>> {
    let cover = section_of(xml, "coverpage")?;
    let href = tags(&cover, "image")
        .first()
        .and_then(|t| attr(t, "l:href").or_else(|| attr(t, "xlink:href")).or_else(|| attr(t, "href")))?;
    let id = href.trim_start_matches('#');
    // Find the <binary> whose id matches, then take its element text.
    let mut base = 0usize;
    while let Some(rel) = xml[base..].find("<binary") {
        let at = base + rel;
        let tail = &xml[at..];
        let gt = tail.find('>')?;
        if attr(&tail[1..gt], "id").as_deref() == Some(id) {
            let body = &tail[gt + 1..];
            let end = body.find("</binary")?;
            return Some(crate::b64_decode(&body[..end]));
        }
        base = at + gt + 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOK: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook>
 <description><title-info>
   <genre>sf</genre>
   <author><first-name>Arkady</first-name><last-name>Strugatsky</last-name></author>
   <book-title>Roadside Picnic</book-title>
   <date value="1972"/>
   <sequence name="Noon Universe" number="3"/>
 </title-info></description>
 <body>
  <section><title><p>One</p></title><p>First para.</p><p>Second para.</p>
    <section><title><p>Nested</p></title><p>Inner text.</p></section>
  </section>
  <section><title><p>Two</p></title><p>Chapter two body.</p></section>
 </body>
</FictionBook>"#;

    #[test]
    fn reads_metadata() {
        let m = meta(BOOK);
        assert_eq!(m.title, "Roadside Picnic");
        assert_eq!(m.author, "Arkady Strugatsky");
        assert_eq!(m.genre, "sf");
        assert_eq!(m.series, "Noon Universe");
        assert_eq!(m.published, "1972");
    }

    #[test]
    fn splits_top_level_sections_only() {
        let ch = chapters(BOOK);
        // Two top-level chapters — the nested section stays inside the first.
        assert_eq!(ch.len(), 2, "nested sections must not split their parent");
        assert_eq!(ch[0].0, "One");
        assert_eq!(ch[1].0, "Two");
        assert!(ch[0].1.contains("First para."));
        assert!(ch[0].1.contains("Inner text."), "nested body belongs to its parent");
        assert!(ch[1].1.contains("Chapter two body."));
    }

    #[test]
    fn book_without_sections_still_reads() {
        let ch = chapters("<FictionBook><body><p>Just prose.</p></body></FictionBook>");
        assert_eq!(ch.len(), 1);
        assert!(ch[0].1.contains("Just prose."));
    }
}
