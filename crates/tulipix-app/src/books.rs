//! Books section, app layer: the file IO the `tulipix-books` crate deliberately
//! leaves out. EPUB and CBZ are both ZIP containers, so one `zip` reader covers
//! real cover extraction, comic page images, and EPUB chapter text. The crate
//! still owns the schema, paging math (`reader::ReaderState`), queries, TOC, and
//! progress; this only feeds it bytes.
//!
//! np.p5.books.pdf / .formats: PDF reads as reflowed text (pdf-extract), MOBI/
//! AZW3 via the `mobi` crate, FB2 as plain XML (zip-wrapped or bare), CBR by
//! extracting once with a system unarchiver (unrar/bsdtar/7z) into the cache.
//! DjVu (native lib) and DRM'd AZW stay honestly unsupported.

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
        Format::Cbr => {
            out.is_comic = true;
            let lower = path.to_string_lossy().to_ascii_lowercase();
            out.rtl = lower.contains("manga") || lower.contains("(jp)") || lower.contains("[jp]");
            if let Some(pages) = cbr_image_paths(path) {
                out.page_count = Some(pages.len() as i64);
                if let Some(first) = pages.first() {
                    if let Ok(bytes) = std::fs::read(first) {
                        out.cover = write_cover(&bytes, &key(path, "cover"));
                    }
                }
            }
        }
        Format::Mobi | Format::Azw3 => {
            if let Ok(m) = mobi::Mobi::from_path(path) {
                let t = m.title();
                if !t.trim().is_empty() { out.title = Some(t); }
                out.author = m.author();
                out.language = Some(format!("{:?}", m.language()).to_lowercase());
            }
        }
        Format::Fb2 => {
            if let Some(xml) = fb2_xml(path) {
                let tag = |t: &str| -> Option<String> {
                    let open = format!("<{t}");
                    let s = xml.find(&open)?;
                    let after = &xml[s..];
                    let gt = after.find('>')? + 1;
                    let close = after[gt..].find("</")? + gt;
                    let v = after[gt..close].trim();
                    if v.is_empty() { None } else { Some(v.to_string()) }
                };
                out.title = tag("book-title").or(out.title);
                let first = tag("first-name").unwrap_or_default();
                let last = tag("last-name").unwrap_or_default();
                let author = format!("{first} {last}").trim().to_string();
                if !author.is_empty() { out.author = Some(author); }
                out.language = tag("lang");
            }
        }
        // No text engine bundled; poppler (when installed) supplies the page
        // count + a first-page cover for the library card.
        Format::Pdf => {
            if let Some(n) = pdf_raster_page_count(path) {
                out.page_count = Some(n as i64);
            }
            if let Some(first) = pdf_raster_page_image(path, 0) {
                if let Ok(bytes) = std::fs::read(&first) {
                    out.cover = write_cover(&bytes, &key(path, "cover"));
                }
            }
        }
        Format::Djvu => {
            if let Some(n) = djvu_page_count(path) {
                out.page_count = Some(n as i64);
            }
            if let Some(first) = djvu_page_image(path, 0) {
                if let Ok(bytes) = std::fs::read(&first) {
                    out.cover = write_cover(&bytes, &key(path, "cover"));
                }
            }
        }
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

/// Comic/raster page count — CBZ via the zip listing, CBR via the extracted
/// dir, scanned PDF via pdfinfo, DjVu via djvused.
pub fn comic_page_count(path: &Path) -> usize {
    match format_from_ext(&path.to_string_lossy()) {
        Some(Format::Cbz) => open_zip(path).map(|mut z| zip_image_names(&mut z).len()).unwrap_or(0),
        Some(Format::Cbr) => cbr_image_paths(path).map(|p| p.len()).unwrap_or(0),
        Some(Format::Pdf) => pdf_raster_page_count(path).unwrap_or(0),
        Some(Format::Djvu) => djvu_page_count(path).unwrap_or(0),
        _ => 0,
    }
}

/// Rendered page for the raster document formats (scanned PDF / DjVu), with
/// the same invert-cache contract as the comic paths.
fn raster_page_image(path: &Path, index: usize, invert: bool) -> Option<PathBuf> {
    let base = match ext_of(&path.to_string_lossy()).as_str() {
        "pdf" => pdf_raster_page_image(path, index)?,
        _ => djvu_page_image(path, index)?,
    };
    if !invert {
        return Some(base);
    }
    let out = cache_dir()?.join(format!("{}-p{index}.png", key(path, "page-inv")));
    if out.exists() {
        return Some(out);
    }
    let mut buf = image::open(&base).ok()?.to_rgba8();
    image::imageops::invert(&mut buf);
    buf.save(&out).ok()?;
    Some(out)
}

/// Extract one comic page to a cached file and return its path (full
/// resolution). `invert` produces a colour-inverted copy (dark-mode scans).
pub fn comic_page_image(path: &Path, index: usize, invert: bool) -> Option<PathBuf> {
    if matches!(ext_of(&path.to_string_lossy()).as_str(), "pdf" | "djvu" | "djv") {
        return raster_page_image(path, index, invert);
    }
    if ext_of(&path.to_string_lossy()) == "cbr" {
        let pages = cbr_image_paths(path)?;
        let page = pages.get(index)?.clone();
        if !invert {
            return Some(page);
        }
        let cache = cache_dir()?;
        let out = cache.join(format!("{}-p{index}.png", key(path, "page-inv")));
        if out.exists() {
            return Some(out);
        }
        let mut buf = image::open(&page).ok()?.to_rgba8();
        image::imageops::invert(&mut buf);
        buf.save(&out).ok()?;
        return Some(out);
    }
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

/// Small cached thumbnail of one comic page for the reader's timeline strip.
/// Decoded once per page at ≤220×160 px, cached next to the full-page cache.
pub fn comic_page_thumb(path: &Path, index: usize) -> Option<PathBuf> {
    let cache = cache_dir()?;
    let out = cache.join(format!("{}-t{index}.png", key(path, "thumb")));
    if out.exists() {
        return Some(out);
    }
    let bytes = comic_page_bytes(path, index)?;
    let img = image::load_from_memory(&bytes).ok()?;
    img.thumbnail(220, 160).save(&out).ok()?;
    Some(out)
}

/// Raw bytes of one comic page, CBZ (zip), CBR (extracted dir), or a rendered
/// raster page (PDF/DjVu).
fn comic_page_bytes(path: &Path, index: usize) -> Option<Vec<u8>> {
    if matches!(ext_of(&path.to_string_lossy()).as_str(), "pdf" | "djvu" | "djv") {
        return std::fs::read(raster_page_image(path, index, false)?).ok();
    }
    if ext_of(&path.to_string_lossy()) == "cbr" {
        let pages = cbr_image_paths(path)?;
        return std::fs::read(pages.get(index)?).ok();
    }
    let mut zip = open_zip(path).ok()?;
    let names = zip_image_names(&mut zip);
    let name = names.get(index)?.clone();
    read_entry(&mut zip, &name)
}

// ── CBR (RAR) via a system unarchiver (np.p5.books.formats) ─────────────────

/// Extract a CBR once into the cache and return its page images, sorted.
/// Tries `unrar`, `bsdtar`, `7z` in that order; None if none is installed.
pub fn cbr_image_paths(path: &Path) -> Option<Vec<PathBuf>> {
    let dir = cache_dir()?.join(format!("{}-rar", key(path, "cbr")));
    if !dir.join(".done").exists() {
        std::fs::create_dir_all(&dir).ok()?;
        let tries: [(&str, Vec<String>); 3] = [
            ("unrar",  vec!["x".into(), "-inul".into(), "-o+".into(),
                            path.display().to_string(), format!("{}/", dir.display())]),
            ("bsdtar", vec!["-xf".into(), path.display().to_string(),
                            "-C".into(), dir.display().to_string()]),
            ("7z",     vec!["x".into(), "-y".into(), format!("-o{}", dir.display()),
                            path.display().to_string()]),
        ];
        let ok = tries.iter().any(|(bin, args)| {
            std::process::Command::new(bin).args(args)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status().map(|s| s.success()).unwrap_or(false)
        });
        if !ok { return None; }
        std::fs::write(dir.join(".done"), b"").ok();
    }
    let mut pages: Vec<PathBuf> = walkdir::WalkDir::new(&dir).into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file() && is_image(&e.file_name().to_string_lossy()))
        .map(|e| e.into_path())
        .collect();
    pages.sort();
    if pages.is_empty() { None } else { Some(pages) }
}

// ── Raster documents: scanned PDFs + DjVu (np.p5.books.pdf / .formats) ──────
// Same pattern as CBR: lean on a system tool when present (poppler's
// pdftoppm/pdfinfo, djvulibre's ddjvu/djvused), render pages on demand into
// the cache, honest notice when the tool is missing.

fn run_capture(bin: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(bin).args(args)
        .stderr(std::process::Stdio::null())
        .output().ok()?;
    if !out.status.success() { return None; }
    Some(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Page count of a PDF via `pdfinfo` ("Pages: N"). None when poppler is absent.
pub fn pdf_raster_page_count(path: &Path) -> Option<usize> {
    let info = run_capture("pdfinfo", &[&path.display().to_string()])?;
    info.lines()
        .find_map(|l| l.strip_prefix("Pages:"))
        .and_then(|v| v.trim().parse().ok())
}

/// Render one PDF page to a cached PNG via `pdftoppm` (150 dpi reads crisply
/// without ballooning the cache). 0-based index; pdftoppm pages are 1-based.
pub fn pdf_raster_page_image(path: &Path, index: usize) -> Option<PathBuf> {
    let cache = cache_dir()?;
    let stem = format!("{}-r{index}", key(path, "pdfpage"));
    let out = cache.join(format!("{stem}.png"));
    if out.exists() {
        return Some(out);
    }
    let n = (index + 1).to_string();
    let prefix = cache.join(&stem);
    let ok = std::process::Command::new("pdftoppm")
        .args(["-png", "-r", "150", "-f", &n, "-l", &n, "-singlefile"])
        .arg(path).arg(&prefix)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    if !ok || !out.exists() { return None; }
    Some(out)
}

/// Page count of a DjVu document via `djvused -e n`.
pub fn djvu_page_count(path: &Path) -> Option<usize> {
    let out = run_capture("djvused", &[&path.display().to_string(), "-e", "n"])?;
    out.trim().parse().ok()
}

/// Render one DjVu page to a cached PNG via `ddjvu`. ddjvu has no PNG writer,
/// so it emits PNM into the cache and the `image` crate converts.
pub fn djvu_page_image(path: &Path, index: usize) -> Option<PathBuf> {
    let cache = cache_dir()?;
    let out = cache.join(format!("{}-r{index}.png", key(path, "djvupage")));
    if out.exists() {
        return Some(out);
    }
    let pnm = cache.join(format!("{}-r{index}.pnm", key(path, "djvupage")));
    let ok = std::process::Command::new("ddjvu")
        .args(["-format=pnm", &format!("-page={}", index + 1), "-scale=150"])
        .arg(path).arg(&pnm)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false);
    if !ok || !pnm.exists() { return None; }
    let img = image::open(&pnm).ok()?;
    let _ = std::fs::remove_file(&pnm);
    img.save(&out).ok()?;
    Some(out)
}

// ── Reflowable text sources (np.p5.books.pdf / .formats) ────────────────────

/// PDF plain text as one pseudo-chapter (pdf-extract; reflow-only — raster
/// rendering of scanned PDFs needs a real PDF engine and stays TBD).
/// catch_unwind because pdf-extract panics on some malformed files.
pub fn pdf_chapters_text(path: &Path) -> Vec<String> {
    let p = path.to_path_buf();
    let text = std::panic::catch_unwind(move || pdf_extract::extract_text(&p).ok())
        .ok().flatten().unwrap_or_default();
    let text = text.trim();
    if text.is_empty() { Vec::new() } else {
        // Form feeds mark page breaks in some extractors; use them as chapter
        // seams when present so the TOC/scrub get useful granularity.
        let parts: Vec<String> = text.split('\u{c}')
            .map(|c| c.trim().to_string()).filter(|c| !c.is_empty()).collect();
        if parts.is_empty() { vec![text.to_string()] } else { parts }
    }
}

/// MOBI/AZW3 chapters: split the HTML on Mobipocket page breaks, strip tags.
pub fn mobi_chapters_text(path: &Path) -> Vec<String> {
    let Ok(m) = mobi::Mobi::from_path(path) else { return Vec::new() };
    let html = m.content_as_string_lossy();
    let chapters: Vec<String> = html.split("<mbp:pagebreak")
        .map(strip_html)
        .filter(|c| !c.trim().is_empty())
        .collect();
    chapters
}

/// The FB2 XML document — bare `.fb2` or the common `.fb2.zip` wrapper.
fn fb2_xml(path: &Path) -> Option<String> {
    let lower = path.to_string_lossy().to_ascii_lowercase();
    if lower.ends_with(".zip") {
        let mut zip = open_zip(path).ok()?;
        let name = (0..zip.len())
            .filter_map(|i| zip.by_index(i).ok().map(|f| f.name().to_string()))
            .find(|n| n.to_ascii_lowercase().ends_with(".fb2"))?;
        let bytes = read_entry(&mut zip, &name)?;
        return Some(String::from_utf8_lossy(&bytes).into_owned());
    }
    std::fs::read(path).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
}

/// FB2 chapters: one per top-level `<section>` of the body, tags stripped.
pub fn fb2_chapters_text(path: &Path) -> Vec<String> {
    let Some(xml) = fb2_xml(path) else { return Vec::new() };
    let body = xml.find("<body").map(|i| &xml[i..]).unwrap_or(&xml);
    let chapters: Vec<String> = body.split("<section")
        .skip(1) // before the first <section is the body header
        .map(strip_html)
        .filter(|c| !c.trim().is_empty())
        .collect();
    if chapters.is_empty() {
        let all = strip_html(body);
        if all.trim().is_empty() { Vec::new() } else { vec![all] }
    } else {
        chapters
    }
}

// ── Comic guided view (np.p5.books.comic-guided) ─────────────────────────────

/// Detect panel rectangles on a comic page by white-gutter splitting: rows that
/// are ≥95% light split the page into strips, then light columns split each
/// strip into cells. Reading order top→bottom then left→right (reversed per
/// strip for RTL/manga). Falls back to the whole page when nothing splits.
pub fn detect_panels(page_img: &Path, rtl: bool) -> Vec<(u32, u32, u32, u32)> {
    let Ok(img) = image::open(page_img) else { return Vec::new() };
    let gray = img.to_luma8();
    let (w, h) = gray.dimensions();
    if w < 32 || h < 32 {
        return vec![(0, 0, w, h)];
    }
    let light = |v: u8| v >= 200;
    let row_light = |y: u32| -> bool {
        let lit = (0..w).filter(|&x| light(gray.get_pixel(x, y)[0])).count();
        lit as f32 / w as f32 >= 0.95
    };
    // Horizontal strips between gutter row-runs.
    let mut strips: Vec<(u32, u32)> = Vec::new(); // (y0, y1)
    let mut y0: Option<u32> = None;
    for y in 0..h {
        if row_light(y) {
            if let Some(s) = y0.take() {
                if y - s >= h / 12 { strips.push((s, y)); }
            }
        } else if y0.is_none() {
            y0 = Some(y);
        }
    }
    if let Some(s) = y0 {
        if h - s >= h / 12 { strips.push((s, h)); }
    }
    if strips.is_empty() { strips.push((0, h)); }
    // Vertical cells within each strip.
    let mut panels: Vec<(u32, u32, u32, u32)> = Vec::new();
    for &(sy0, sy1) in &strips {
        let col_light = |x: u32| -> bool {
            let lit = (sy0..sy1).filter(|&y| light(gray.get_pixel(x, y)[0])).count();
            lit as f32 / (sy1 - sy0) as f32 >= 0.95
        };
        let mut cells: Vec<(u32, u32)> = Vec::new();
        let mut x0: Option<u32> = None;
        for x in 0..w {
            if col_light(x) {
                if let Some(s) = x0.take() {
                    if x - s >= w / 12 { cells.push((s, x)); }
                }
            } else if x0.is_none() {
                x0 = Some(x);
            }
        }
        if let Some(s) = x0 {
            if w - s >= w / 12 { cells.push((s, w)); }
        }
        if cells.is_empty() { cells.push((0, w)); }
        if rtl { cells.reverse(); }
        for (cx0, cx1) in cells {
            panels.push((cx0, sy0, cx1 - cx0, sy1 - sy0));
        }
    }
    if panels.is_empty() { panels.push((0, 0, w, h)); }
    panels
}

/// Crop one detected panel to a cached PNG (small breathing margin added).
pub fn comic_panel_image(
    path: &Path, page: usize, panel_idx: usize,
    rect: (u32, u32, u32, u32), invert: bool,
) -> Option<PathBuf> {
    let cache = cache_dir()?;
    let salt = if invert { "panel-inv" } else { "panel" };
    let out = cache.join(format!("{}-p{page}-i{panel_idx}.png", key(path, salt)));
    if out.exists() {
        return Some(out);
    }
    let full = comic_page_image(path, page, false)?;
    let img = image::open(&full).ok()?;
    let (w, h) = (img.width(), img.height());
    let pad = (w / 100).max(4);
    let x = rect.0.saturating_sub(pad);
    let y = rect.1.saturating_sub(pad);
    let cw = (rect.2 + 2 * pad).min(w - x);
    let ch = (rect.3 + 2 * pad).min(h - y);
    let mut crop = img.crop_imm(x, y, cw, ch).to_rgba8();
    if invert {
        image::imageops::invert(&mut crop);
    }
    crop.save(&out).ok()?;
    Some(out)
}

/// Find `<tag` at a real tag boundary (followed by `>`, whitespace or `/`) so
/// that searching for `<head` can never match `<header>`, `<script` never
/// matches a hypothetical `<scripted>`, etc.
fn find_tag_open(hay_lower: &str, tag: &str, from: usize) -> Option<usize> {
    let pat = format!("<{tag}");
    let mut i = from;
    while let Some(rel) = hay_lower.get(i..)?.find(&pat) {
        let at = i + rel;
        match hay_lower.as_bytes().get(at + pat.len()) {
            None | Some(b'>') | Some(b' ') | Some(b'\t') | Some(b'\n') | Some(b'\r') | Some(b'/') => {
                return Some(at);
            }
            _ => i = at + pat.len(),
        }
    }
    None
}

/// Strip HTML/XML tags and decode the handful of entities that matter, leaving
/// paragraph breaks. Crude but enough for a readable text view.
fn strip_html(html: &str) -> String {
    // Drop <style>/<script>/<head> blocks wholesale. Tag-boundary matching is
    // load-bearing: EPUB3 chapters routinely use <header>, and a prefix match
    // on "<head" would eat the whole chapter body looking for a "</head>"
    // that never comes.
    let mut rest = html.to_string();
    for tag in ["style", "script", "head"] {
        let close = format!("</{tag}>");
        let mut cleaned = String::new();
        let mut r = rest.as_str();
        while let Some(o) = find_tag_open(&r.to_ascii_lowercase(), tag, 0) {
            cleaned.push_str(&r[..o]);
            if let Some(c) = r[o..].to_ascii_lowercase().find(&close) {
                r = &r[o + c + close.len()..];
            } else {
                // Unclosed block: drop only the open tag itself, keep content.
                let tag_end = r[o..].find('>').map(|e| o + e + 1).unwrap_or(r.len());
                r = &r[tag_end..];
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

    #[test]
    fn strip_keeps_epub3_header_content() {
        // "<head" must not prefix-match <header> — that used to delete the
        // whole chapter body.
        let html = "<html><head><title>x</title></head><body>\
                    <header><h1>Chapter 1</h1></header><p>The actual story.</p></body></html>";
        let t = strip_html(html);
        assert!(t.contains("Chapter 1"), "header content lost: {t:?}");
        assert!(t.contains("The actual story."), "body lost: {t:?}");
        assert!(!t.contains("x"), "head title should be stripped: {t:?}");
    }

    #[test]
    fn strip_unclosed_block_keeps_content() {
        let html = "<body><style>p { color: red }</style><p>kept</p><script>boom(";
        let t = strip_html(html);
        assert!(t.contains("kept"));
        assert!(!t.contains("color"));
        // Unclosed <script>: only the tag is dropped, not the rest of the doc.
        assert!(t.contains("boom("));
    }
}
