//! EPUB table of contents — EPUB3 nav.xhtml first, NCX fallback. Entries map
//! to spine chapter indexes so the reader can jump / highlight current.

use crate::epub;
use anyhow::Result;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct TocEntry {
    pub label: String,
    pub depth: i32,
    /// Index into the spine chapter list (-1 when the href isn't in the spine).
    pub chapter: i32,
}

pub fn load(path: &Path) -> Result<Vec<TocEntry>> {
    let doc = epub::open(path)?;
    // EPUB3 nav document (manifest properties contains "nav").
    let nav = doc
        .manifest
        .iter()
        .find(|(_, _, _, props)| props.split_whitespace().any(|p| p == "nav"))
        .map(|(_, p, _, _)| p.clone());
    if let Some(nav_path) = nav {
        if let Some(xhtml) = epub::zip_bytes(path, &nav_path)
            .and_then(|b| String::from_utf8(b).ok())
        {
            let dir = nav_path.rfind('/').map(|i| nav_path[..i].to_string()).unwrap_or_default();
            let entries = parse_nav(&xhtml, &dir, &doc.spine);
            if !entries.is_empty() {
                return Ok(entries);
            }
        }
    }
    // NCX fallback (EPUB2): manifest media-type application/x-dtbncx+xml.
    if let Some((_, ncx_path, _, _)) = doc
        .manifest
        .iter()
        .find(|(_, _, mt, _)| mt.contains("dtbncx"))
    {
        if let Some(xml) = epub::zip_bytes(path, ncx_path)
            .and_then(|b| String::from_utf8(b).ok())
        {
            let dir = ncx_path.rfind('/').map(|i| ncx_path[..i].to_string()).unwrap_or_default();
            return Ok(parse_ncx(&xml, &dir, &doc.spine));
        }
    }
    Ok(Vec::new())
}

/// PDF outline (document bookmarks) → TOC entries. `chapter` carries the
/// 0-based PAGE index (fixed-page books have no spine); empty when the PDF
/// has no outline or lopdf can't parse it.
pub fn pdf_outline(path: &Path) -> Vec<TocEntry> {
    let Ok(doc) = lopdf::Document::load(path) else { return Vec::new() };
    let Ok(toc) = doc.get_toc() else { return Vec::new() };
    toc.toc
        .into_iter()
        .filter(|e| !e.title.trim().is_empty())
        .map(|e| TocEntry {
            label: e.title,
            depth: (e.level as i32 - 1).max(0),
            chapter: (e.page as i32 - 1).max(0),
        })
        .collect()
}

fn chapter_of(spine: &[String], resolved: &str) -> i32 {
    spine.iter().position(|s| s == resolved).map(|i| i as i32).unwrap_or(-1)
}

/// nav.xhtml: walk `<a href>` inside the toc nav, depth = nested `<ol>` level.
fn parse_nav(xhtml: &str, dir: &str, spine: &[String]) -> Vec<TocEntry> {
    // Slice out the toc <nav …epub:type="toc"…> … </nav> block when present.
    let body = match xhtml.find("epub:type=\"toc\"") {
        Some(i) => {
            let start = xhtml[..i].rfind("<nav").unwrap_or(0);
            let end = xhtml[start..].find("</nav>").map(|e| start + e).unwrap_or(xhtml.len());
            &xhtml[start..end]
        }
        None => xhtml,
    };
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut rest = body;
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        let lower = tag.to_ascii_lowercase();
        if lower.starts_with("ol") {
            depth += 1;
        } else if lower.starts_with("/ol") {
            depth -= 1;
        } else if lower.starts_with("a ") || lower == "a" {
            if let Some(href) = epub::attr(tag, "href") {
                let after = &rest[end + 1..];
                let label = after
                    .find("</a")
                    .map(|e| epub::unescape(&strip_tags(&after[..e])))
                    .unwrap_or_default();
                let label = label.split_whitespace().collect::<Vec<_>>().join(" ");
                if !label.is_empty() {
                    out.push(TocEntry {
                        label,
                        depth: (depth - 1).max(0),
                        chapter: chapter_of(spine, &epub::resolve(dir, &href)),
                    });
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out
}

/// NCX: `<navPoint>` nesting depth, `<navLabel><text>` + `<content src>`.
fn parse_ncx(xml: &str, dir: &str, spine: &[String]) -> Vec<TocEntry> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut rest = xml;
    let mut pending: Option<String> = None; // label waiting for its content src
    while let Some(i) = rest.find('<') {
        rest = &rest[i..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        if tag.starts_with("navPoint") {
            depth += 1;
        } else if tag.starts_with("/navPoint") {
            depth -= 1;
        } else if tag.starts_with("text") && !tag.starts_with("text/") {
            let after = &rest[end + 1..];
            if let Some(e) = after.find("</text") {
                pending = Some(epub::unescape(after[..e].trim()));
            }
        } else if tag.starts_with("content") {
            if let (Some(label), Some(src)) = (pending.take(), epub::attr(tag, "src")) {
                if !label.is_empty() {
                    out.push(TocEntry {
                        label,
                        depth: (depth - 1).max(0),
                        chapter: chapter_of(spine, &epub::resolve(dir, &src)),
                    });
                }
            }
        }
        rest = &rest[end + 1..];
    }
    out
}

fn strip_tags(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        match rest[i..].find('>') {
            Some(e) => rest = &rest[i + e + 1..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ncx_parse() {
        let xml = r#"<ncx><navMap>
            <navPoint id="a"><navLabel><text>Intro</text></navLabel><content src="ch1.xhtml"/>
              <navPoint id="b"><navLabel><text>Sub</text></navLabel><content src="ch1.xhtml#s"/></navPoint>
            </navPoint>
        </navMap></ncx>"#;
        let spine = vec!["ch1.xhtml".to_string()];
        let t = parse_ncx(xml, "", &spine);
        assert_eq!(t.len(), 2);
        assert_eq!(t[0].label, "Intro");
        assert_eq!(t[0].depth, 0);
        assert_eq!(t[1].depth, 1);
        assert_eq!(t[1].chapter, 0);
    }
}
