//! Books section, app layer: the file IO the `tulipix-books` crate deliberately
//! leaves out. EPUB and CBZ are both ZIP containers, so one `zip` reader covers
//! real cover extraction, comic page images, and EPUB chapter text. The crate
//! still owns the schema, paging math (`reader::ReaderState`), queries, TOC, and
//! progress; this only feeds it bytes. CBR (RAR) and PDF have no pure-Rust
//! engine bundled, so they ingest as metadata-only and the reader shows an
//! honest "needs an engine" notice.

use anyhow::{Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use tulipix_books::scan::{format_from_ext, Format};

const IMG_EXT: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

fn ext_of(name: &str) -> String {
    name.rsplit('.').next().unwrap_or("").to_ascii_lowercase()
}
fn is_image(name: &str) -> bool {
    IMG_EXT.contains(&ext_of(name).as_str())
}

fn cache_dir() -> Option<PathBuf> {
    let d = crate::dirs_default()?.join("cache").join("books");
    std::fs::create_dir_all(&d).ok();
    Some(d)
}

/// Stable short hash of (path, mtime, salt) → a cache filename stem.
fn key(path: &Path, salt: &str) -> String {
    use sha2::{Digest, Sha256};
    let mtime = std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut h = Sha256::new();
    h.update(path.display().to_string().as_bytes());
    h.update(format!(":{mtime}:{salt}").as_bytes());
    let d = h.finalize();
    d.iter().take(8).map(|b| format!("{b:02x}")).collect()
}

/// Decode arbitrary image bytes and write a cover-sized JPEG into the cache.
fn write_cover(bytes: &[u8], stem: &str) -> Option<PathBuf> {
    let img = image::load_from_memory(bytes).ok()?;
    let thumb = img.thumbnail(400, 600);
    let out = cache_dir()?.join(format!("{stem}.jpg"));
    let mut w = std::io::BufWriter::new(std::fs::File::create(&out).ok()?);
    thumb.to_rgb8().write_to(&mut w, image::ImageFormat::Jpeg).ok()?;
    Some(out)
}

// ── ZIP helpers ────────────────────────────────────────────────────────────

type Zip = zip::ZipArchive<std::fs::File>;

fn open_zip(path: &Path) -> Result<Zip> {
    let f = std::fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    Ok(zip::ZipArchive::new(f)?)
}

fn read_entry(zip: &mut Zip, name: &str) -> Option<Vec<u8>> {
    let mut f = zip.by_name(name).ok()?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf).ok()?;
    Some(buf)
}

/// Sorted list of image entry names in a CBZ (natural-ish: lexical works for
/// the usual zero-padded page numbering).
fn zip_image_names(zip: &mut Zip) -> Vec<String> {
    let mut names: Vec<String> = (0..zip.len())
        .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
        .filter(|n| !n.ends_with('/') && is_image(n))
        .collect();
    names.sort();
    names
}

// ── Public ingest result ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct Ingest {
    pub format: &'static str,
    pub title: Option<String>,
    pub author: Option<String>,
    pub language: Option<String>,
    pub is_comic: bool,
    pub rtl: bool,
    pub page_count: Option<i64>,
    pub cover: Option<PathBuf>,
}

fn stem_title(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.replace(['_', '.'], " ").trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Read a book file and produce metadata + an extracted cover. `None` if the
/// extension isn't a known book format.
pub fn ingest_file(path: &Path) -> Option<Ingest> {
    let fmt = format_from_ext(&path.to_string_lossy())?;
    let mut out = Ingest { format: fmt.as_str(), title: stem_title(path), ..Default::default() };
    match fmt {
        Format::Cbz => {
            out.is_comic = true;
            // Manga RTL heuristic: filename hints.
            let lower = path.to_string_lossy().to_ascii_lowercase();
            out.rtl = lower.contains("manga") || lower.contains("(jp)") || lower.contains("[jp]");
            if let Ok(mut zip) = open_zip(path) {
                let names = zip_image_names(&mut zip);
                out.page_count = Some(names.len() as i64);
                if let Some(first) = names.first().cloned() {
                    if let Some(bytes) = read_entry(&mut zip, &first) {
                        out.cover = write_cover(&bytes, &key(path, "cover"));
                    }
                }
            }
        }
        Format::Epub => {
            if let Ok((meta, cover_bytes, spine_len)) = read_epub(path) {
                out.title = meta.title.or(out.title);
                out.author = meta.author;
                out.language = meta.language;
                out.page_count = Some(spine_len as i64);
                if let Some(b) = cover_bytes {
                    out.cover = write_cover(&b, &key(path, "cover"));
                }
            }
        }
        // No bundled engine — record format + stem title; reader is honest.
        Format::Cbr | Format::Pdf => {}
    }
    Some(out)
}

// ── EPUB parsing (string-based, no XML dep) ─────────────────────────────────

/// Find an XML attribute value following the first occurrence of `attr=`.
fn attr_after(hay: &str, at: usize, attr: &str) -> Option<String> {
    let region = &hay[at..];
    let p = region.find(attr)?;
    let after = &region[p + attr.len()..];
    let after = after.trim_start();
    let after = after.strip_prefix('=')?.trim_start();
    let q = after.chars().next()?;
    if q != '"' && q != '\'' { return None; }
    let rest = &after[1..];
    let end = rest.find(q)?;
    Some(rest[..end].to_string())
}

/// Locate the OPF path from META-INF/container.xml.
fn opf_path(zip: &mut Zip) -> Option<String> {
    let xml = read_entry(zip, "META-INF/container.xml")?;
    let xml = String::from_utf8_lossy(&xml);
    let at = xml.find("<rootfile")?;
    attr_after(&xml, at, "full-path")
}

/// Resolve a manifest href relative to the OPF's directory, normalising `../`.
fn resolve(opf: &str, href: &str) -> String {
    let base = match opf.rfind('/') {
        Some(i) => &opf[..i],
        None => "",
    };
    let joined = if base.is_empty() { href.to_string() } else { format!("{base}/{href}") };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => { parts.pop(); }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

struct Manifest {
    /// id → href (resolved to a full zip path)
    items: std::collections::HashMap<String, String>,
    /// spine itemref idrefs, in order
    spine: Vec<String>,
    cover_id: Option<String>,
    cover_prop_href: Option<String>,
}

fn parse_manifest(opf: &str, opf_xml: &str) -> Manifest {
    let mut items = std::collections::HashMap::new();
    let mut cover_prop_href = None;
    // <item id=".." href=".." media-type=".." properties=".."/>
    let mut idx = 0;
    while let Some(rel) = opf_xml[idx..].find("<item ") {
        let at = idx + rel;
        let end = opf_xml[at..].find('>').map(|e| at + e).unwrap_or(opf_xml.len());
        let tag = &opf_xml[at..end];
        if let (Some(id), Some(href)) = (attr_after(tag, 0, "id"), attr_after(tag, 0, "href")) {
            let full = resolve(opf, &href);
            if tag.contains("cover-image") {
                cover_prop_href = Some(full.clone());
            }
            items.insert(id, full);
        }
        idx = end + 1;
    }
    // spine
    let mut spine = Vec::new();
    let mut sidx = 0;
    while let Some(rel) = opf_xml[sidx..].find("<itemref ") {
        let at = sidx + rel;
        let end = opf_xml[at..].find('>').map(|e| at + e).unwrap_or(opf_xml.len());
        let tag = &opf_xml[at..end];
        if let Some(idref) = attr_after(tag, 0, "idref") {
            spine.push(idref);
        }
        sidx = end + 1;
    }
    // <meta name="cover" content="ID"/>
    let cover_id = opf_xml.find("name=\"cover\"").and_then(|p| {
        let end = opf_xml[p..].find('>').map(|e| p + e).unwrap_or(opf_xml.len());
        attr_after(&opf_xml[p..end], 0, "content")
    });
    Manifest { items, spine, cover_id, cover_prop_href }
}

struct EpubMeta {
    title: Option<String>,
    author: Option<String>,
    language: Option<String>,
}

/// Returns (metadata, cover bytes, spine length).
fn read_epub(path: &Path) -> Result<(EpubMeta, Option<Vec<u8>>, usize)> {
    let mut zip = open_zip(path)?;
    let opf = opf_path(&mut zip).context("no OPF in container.xml")?;
    let opf_bytes = read_entry(&mut zip, &opf).context("OPF unreadable")?;
    let opf_xml = String::from_utf8_lossy(&opf_bytes).into_owned();

    let bm = tulipix_books::scan::parse_opf(&opf_xml);
    let meta = EpubMeta { title: bm.title, author: bm.author, language: bm.language };
    let man = parse_manifest(&opf, &opf_xml);

    // Cover: <meta name=cover> → manifest item, else properties=cover-image,
    // else first image-looking manifest href.
    let cover_href = man
        .cover_id
        .as_ref()
        .and_then(|id| man.items.get(id).cloned())
        .or_else(|| man.cover_prop_href.clone())
        .or_else(|| man.items.values().find(|h| is_image(h)).cloned());
    let cover = cover_href.and_then(|h| read_entry(&mut zip, &h));

    Ok((meta, cover, man.spine.len().max(1)))
}

// ── Reader data sources ──────────────────────────────────────────────────────

/// CBZ page count (0 for non-zip formats).
pub fn comic_page_count(path: &Path) -> usize {
    if format_from_ext(&path.to_string_lossy()) != Some(Format::Cbz) {
        return 0;
    }
    open_zip(path).map(|mut z| zip_image_names(&mut z).len()).unwrap_or(0)
}

/// Extract one CBZ page to a cached file and return its path (full resolution).
/// `invert` produces a colour-inverted copy (dark-mode reading of scans).
pub fn comic_page_image(path: &Path, index: usize, invert: bool) -> Option<PathBuf> {
    let mut zip = open_zip(path).ok()?;
    let names = zip_image_names(&mut zip);
    let name = names.get(index)?.clone();
    let cache = cache_dir()?;
    let salt = if invert { "page-inv" } else { "page" };
    let ext = if invert { "png" } else { &ext_of(&name) };
    let out = cache.join(format!("{}-p{index}.{ext}", key(path, salt)));
    if out.exists() {
        return Some(out);
    }
    let bytes = read_entry(&mut zip, &name)?;
    if invert {
        let mut buf = image::load_from_memory(&bytes).ok()?.to_rgba8();
        image::imageops::invert(&mut buf);
        buf.save(&out).ok()?;
    } else {
        std::fs::write(&out, &bytes).ok()?;
    }
    Some(out)
}

/// Strip HTML/XML tags and decode the handful of entities that matter, leaving
/// paragraph breaks. Crude but enough for a readable text view.
fn strip_html(html: &str) -> String {
    // Drop <style>/<script>/<head> blocks wholesale.
    let mut rest = html.to_string();
    for tag in ["style", "script", "head"] {
        let open = format!("<{tag}");
        let close = format!("</{tag}>");
        let mut cleaned = String::new();
        let mut r = rest.as_str();
        while let Some(o) = r.to_ascii_lowercase().find(&open) {
            cleaned.push_str(&r[..o]);
            if let Some(c) = r[o..].to_ascii_lowercase().find(&close) {
                r = &r[o + c + close.len()..];
            } else {
                r = "";
                break;
            }
        }
        cleaned.push_str(r);
        rest = cleaned;
    }
    let mut s = String::with_capacity(rest.len());
    let mut in_tag = false;
    for ch in rest.chars() {
        match ch {
            '<' => {
                in_tag = true;
                // paragraph-ish breaks
                s.push('\n');
            }
            '>' => in_tag = false,
            c if !in_tag => s.push(c),
            _ => {}
        }
    }
    let s = s
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&rsquo;", "'")
        .replace("&lsquo;", "'")
        .replace("&rdquo;", "\"")
        .replace("&ldquo;", "\"")
        .replace("&mdash;", "—");
    // collapse runs of blank lines / spaces
    let mut out = String::with_capacity(s.len());
    let mut blanks = 0;
    for line in s.lines() {
        let t = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if t.is_empty() {
            blanks += 1;
            if blanks <= 1 {
                out.push('\n');
            }
        } else {
            blanks = 0;
            out.push_str(&t);
            out.push('\n');
        }
    }
    out.trim().to_string()
}

/// EPUB chapter plain-text in spine order. Each entry is one chapter.
pub fn epub_chapters_text(path: &Path) -> Vec<String> {
    let Ok(mut zip) = open_zip(path) else { return Vec::new() };
    let Some(opf) = opf_path(&mut zip) else { return Vec::new() };
    let Some(opf_bytes) = read_entry(&mut zip, &opf) else { return Vec::new() };
    let opf_xml = String::from_utf8_lossy(&opf_bytes).into_owned();
    let man = parse_manifest(&opf, &opf_xml);
    let mut out = Vec::new();
    for idref in &man.spine {
        if let Some(href) = man.items.get(idref) {
            if let Some(bytes) = read_entry(&mut zip, href) {
                let text = strip_html(&String::from_utf8_lossy(&bytes));
                if !text.trim().is_empty() {
                    out.push(text);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_normalises_dotdot() {
        assert_eq!(resolve("OEBPS/content.opf", "images/c.jpg"), "OEBPS/images/c.jpg");
        assert_eq!(resolve("OEBPS/content.opf", "../cover.jpg"), "cover.jpg");
        assert_eq!(resolve("content.opf", "ch1.xhtml"), "ch1.xhtml");
    }

    #[test]
    fn attr_extracts_quoted_value() {
        let tag = r#"<item id="cov" href="cover.jpg" media-type="image/jpeg"/>"#;
        assert_eq!(attr_after(tag, 0, "id").as_deref(), Some("cov"));
        assert_eq!(attr_after(tag, 0, "href").as_deref(), Some("cover.jpg"));
    }

    #[test]
    fn strip_removes_tags_and_entities() {
        let html = "<html><head><title>x</title></head><body><p>Hello &amp; bye</p><p>Two</p></body></html>";
        let t = strip_html(html);
        assert!(t.contains("Hello & bye"));
        assert!(t.contains("Two"));
        assert!(!t.contains('<'));
    }

    #[test]
    fn stem_title_cleans() {
        assert_eq!(stem_title(Path::new("/b/The_Hobbit.epub")).as_deref(), Some("The Hobbit"));
    }
}
