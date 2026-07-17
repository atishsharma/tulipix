//! Minimal EPUB reader — zip + hand-rolled XML/HTML scanning (no XML dep).
//! Produces spine-ordered plain-text chapters for the paginator, plus the
//! hooks metadata/covers/toc need (OPF access, href resolution).

use anyhow::{Context, Result};
use std::io::Read;
use std::path::Path;

pub struct EpubDoc {
    /// Directory of the OPF inside the zip ("" or "OEBPS/" style, with slash).
    pub opf_dir: String,
    /// Raw OPF xml.
    pub opf: String,
    /// Spine-ordered chapter hrefs (zip paths, resolved).
    pub spine: Vec<String>,
    /// Manifest: (id, resolved zip path, media-type, properties).
    pub manifest: Vec<(String, String, String, String)>,
}

/// One spine chapter, flattened to plain text (paragraphs = blank line).
pub struct Chapter {
    pub href: String,
    pub title: String,
    pub text: String,
}

fn zip_open(path: &Path) -> Result<zip::ZipArchive<std::fs::File>> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(zip::ZipArchive::new(f)?)
}

fn zip_string(z: &mut zip::ZipArchive<std::fs::File>, name: &str) -> Option<String> {
    let mut e = z.by_name(name).ok()?;
    let mut s = String::new();
    e.read_to_string(&mut s).ok()?;
    Some(s)
}

pub fn zip_bytes(path: &Path, name: &str) -> Option<Vec<u8>> {
    let mut z = zip_open(path).ok()?;
    let mut e = z.by_name(name).ok()?;
    let mut v = Vec::new();
    e.read_to_end(&mut v).ok()?;
    Some(v)
}

/// Resolve `href` relative to `base_dir` (zip-internal, '/'-separated).
pub fn resolve(base_dir: &str, href: &str) -> String {
    let href = href.split('#').next().unwrap_or(href);
    let mut parts: Vec<&str> = base_dir.split('/').filter(|s| !s.is_empty()).collect();
    for seg in href.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

// ── tiny XML scanning helpers ────────────────────────────────────────────────

/// All raw start tags named `tag` (e.g. "item") — returns the inside of `<… >`.
pub fn tags<'a>(xml: &'a str, tag: &str) -> Vec<&'a str> {
    let mut out = Vec::new();
    let mut rest = xml;
    let open = format!("<{tag}");
    while let Some(i) = rest.find(&open) {
        let after = &rest[i + open.len()..];
        // must be followed by whitespace, '>' or '/' (not a longer tag name)
        let ok = after
            .chars()
            .next()
            .map(|c| c.is_whitespace() || c == '>' || c == '/')
            .unwrap_or(false);
        if let Some(end) = after.find('>') {
            if ok {
                out.push(after[..end].trim_end_matches('/'));
            }
            rest = &after[end + 1..];
        } else {
            break;
        }
    }
    out
}

/// Attribute value from a raw tag body.
pub fn attr(tag_body: &str, name: &str) -> Option<String> {
    let pat = format!("{name}=");
    let mut rest = tag_body;
    while let Some(i) = rest.find(&pat) {
        // ensure preceded by whitespace (or start) so `idref=` doesn't match `id=`.
        let at_start = i == 0
            || rest[..i]
                .chars()
                .next_back()
                .map(|c| c.is_whitespace())
                .unwrap_or(false);
        let after = &rest[i + pat.len()..];
        if at_start {
            let q = after.chars().next()?;
            if q == '"' || q == '\'' {
                return after[1..].split(q).next().map(unescape);
            }
        }
        rest = after;
    }
    None
}

/// Text content of the first `<tag …>text</tag>` occurrence.
pub fn tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}");
    let i = xml.find(&open)?;
    let after = &xml[i..];
    let gt = after.find('>')?;
    let body = &after[gt + 1..];
    let end = body.find(&close)?;
    let t = unescape(body[..end].trim());
    if t.is_empty() { None } else { Some(t) }
}

pub fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(semi) = rest[..rest.len().min(12)].find(';') else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..semi];
        match ent {
            "amp" => out.push('&'),
            "lt" => out.push('<'),
            "gt" => out.push('>'),
            "quot" => out.push('"'),
            "apos" => out.push('\''),
            "nbsp" => out.push(' '),
            "mdash" => out.push('—'),
            "ndash" => out.push('–'),
            "hellip" => out.push('…'),
            "rsquo" => out.push('’'),
            "lsquo" => out.push('‘'),
            "rdquo" => out.push('”'),
            "ldquo" => out.push('“'),
            _ => {
                let cp = if let Some(hex) = ent.strip_prefix("#x").or(ent.strip_prefix("#X")) {
                    u32::from_str_radix(hex, 16).ok()
                } else if let Some(dec) = ent.strip_prefix('#') {
                    dec.parse::<u32>().ok()
                } else {
                    None
                };
                match cp.and_then(char::from_u32) {
                    Some(c) => out.push(c),
                    None => {
                        out.push('&');
                        out.push_str(ent);
                        out.push(';');
                    }
                }
            }
        }
        rest = &rest[semi + 1..];
    }
    out.push_str(rest);
    out
}

// ── HTML → text ──────────────────────────────────────────────────────────────

/// Flatten chapter XHTML to plain text. Block boundaries become blank lines,
/// `<br>` a newline; scripts/styles/head dropped; entities decoded.
pub fn html_to_text(html: &str) -> String {
    // Drop head / script / style wholesale.
    let mut src = html.to_string();
    for t in ["head", "script", "style", "svg"] {
        loop {
            let Some(i) = find_ci(&src, &format!("<{t}")) else { break };
            let Some(j) = find_ci(&src[i..], &format!("</{t}")) else { break };
            let Some(k) = src[i + j..].find('>') else { break };
            src.replace_range(i..i + j + k + 1, "");
        }
    }
    let block = [
        "p", "div", "h1", "h2", "h3", "h4", "h5", "h6", "li", "tr", "blockquote",
        "section", "article", "figure", "figcaption", "table",
    ];
    let mut out = String::with_capacity(src.len() / 2);
    let mut rest = src.as_str();
    while let Some(i) = rest.find('<') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find('>') else { break };
        let tag = &rest[1..end];
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect::<String>()
            .to_ascii_lowercase();
        if name == "br" {
            out.push('\n');
        } else if block.contains(&name.as_str()) {
            out.push_str("\n\n");
        }
        rest = &rest[end + 1..];
    }
    out.push_str(rest);
    let decoded = unescape(&out);
    // Collapse intra-paragraph whitespace, keep paragraph breaks.
    let mut paras: Vec<String> = Vec::new();
    for p in decoded.split("\n\n") {
        let mut lines: Vec<String> = Vec::new();
        for l in p.split('\n') {
            let w = l.split_whitespace().collect::<Vec<_>>().join(" ");
            if !w.is_empty() {
                lines.push(w);
            }
        }
        if !lines.is_empty() {
            paras.push(lines.join("\n"));
        }
    }
    paras.join("\n\n")
}

fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| {
        h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
    })
}

// ── EPUB open / chapters ─────────────────────────────────────────────────────

pub fn open(path: &Path) -> Result<EpubDoc> {
    let mut z = zip_open(path)?;
    let container =
        zip_string(&mut z, "META-INF/container.xml").context("no META-INF/container.xml")?;
    let opf_path = tags(&container, "rootfile")
        .iter()
        .find_map(|t| attr(t, "full-path"))
        .context("no rootfile full-path")?;
    let opf = zip_string(&mut z, &opf_path).context("missing OPF")?;
    let opf_dir = match opf_path.rfind('/') {
        Some(i) => opf_path[..i].to_string(),
        None => String::new(),
    };

    let mut manifest = Vec::new();
    for t in tags(&opf, "item") {
        let (Some(id), Some(href)) = (attr(t, "id"), attr(t, "href")) else { continue };
        manifest.push((
            id,
            resolve(&opf_dir, &href),
            attr(t, "media-type").unwrap_or_default(),
            attr(t, "properties").unwrap_or_default(),
        ));
    }
    let mut spine = Vec::new();
    for t in tags(&opf, "itemref") {
        let Some(idref) = attr(t, "idref") else { continue };
        if attr(t, "linear").as_deref() == Some("no") {
            continue;
        }
        if let Some((_, path, mt, _)) = manifest.iter().find(|(id, ..)| *id == idref) {
            if mt.contains("html") || mt.contains("xml") {
                spine.push(path.clone());
            }
        }
    }
    Ok(EpubDoc { opf_dir, opf, spine, manifest })
}

/// Load every spine chapter as plain text.
pub fn load_chapters(path: &Path) -> Result<Vec<Chapter>> {
    let doc = open(path)?;
    let mut z = zip_open(path)?;
    let mut out = Vec::with_capacity(doc.spine.len());
    for href in &doc.spine {
        let Some(html) = zip_string(&mut z, href) else { continue };
        let title = tag_text(&html, "title")
            .or_else(|| tag_text(&html, "h1"))
            .or_else(|| tag_text(&html, "h2"))
            .unwrap_or_default();
        let text = html_to_text(&html);
        out.push(Chapter { href: href.clone(), title, text });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_helpers() {
        let x = r#"<item id="a" href="ch1.xhtml" media-type="application/xhtml+xml"/>"#;
        let t = tags(x, "item");
        assert_eq!(t.len(), 1);
        assert_eq!(attr(t[0], "href").as_deref(), Some("ch1.xhtml"));
        assert_eq!(attr(t[0], "id").as_deref(), Some("a"));
        assert_eq!(resolve("OEBPS/text", "../images/a.png"), "OEBPS/images/a.png");
    }

    #[test]
    fn html_flatten() {
        let h = "<html><head><title>T</title></head><body><h1>One</h1><p>Hello&nbsp;<b>world</b> &amp; you.</p><p>Two</p></body></html>";
        let t = html_to_text(h);
        assert_eq!(t, "One\n\nHello world & you.\n\nTwo");
    }
}
