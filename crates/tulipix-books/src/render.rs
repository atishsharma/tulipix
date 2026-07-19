//! Fixed-page formats (PDF / CBZ / CBR) — page counts + rasterized page
//! images, cached under `~/.cache/Tulipix/books/pages/<hash>/`.
//!
//! PDF uses poppler-utils (`pdfinfo` / `pdftoppm`); CBR uses `unrar`/`bsdtar`.
//! Missing tools degrade to an error string the UI shows on the paper card.

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

fn pages_dir(book_path: &Path) -> Result<PathBuf> {
    let root = crate::cache_dir().context("no cache dir")?;
    let d = root.join("pages").join(crate::short_hash(&book_path.display().to_string()));
    std::fs::create_dir_all(&d)?;
    Ok(d)
}

fn is_image_name(n: &str) -> bool {
    let l = n.to_ascii_lowercase();
    [".jpg", ".jpeg", ".png", ".webp", ".gif", ".bmp"].iter().any(|e| l.ends_with(e))
        && !l.split('/').next_back().unwrap_or("").starts_with('.')
}

/// Sorted comic page names inside a CBZ.
fn cbz_pages(path: &Path) -> Result<Vec<String>> {
    let f = std::fs::File::open(path)?;
    let mut z = zip::ZipArchive::new(f)?;
    let mut names: Vec<String> = (0..z.len())
        .filter_map(|i| z.by_index(i).ok().map(|e| e.name().to_string()))
        .filter(|n| is_image_name(n))
        .collect();
    names.sort();
    Ok(names)
}

/// Sorted comic page names inside a CBR (via `unrar` or `bsdtar` listing).
fn cbr_pages(path: &Path) -> Result<Vec<String>> {
    let try_list = |cmd: &str, args: &[&str]| -> Option<Vec<String>> {
        let out = Command::new(cmd).args(args).arg(path).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let mut v: Vec<String> = String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| is_image_name(l))
            .collect();
        v.sort();
        Some(v)
    };
    try_list("unrar", &["lb"])
        .or_else(|| try_list("bsdtar", &["-tf"]))
        .filter(|v| !v.is_empty())
        .context("cannot list CBR (need unrar or bsdtar)")
}

/// Archive page listings are expensive (full zip scan, or an `unrar` process)
/// and used to re-run on every page render — memoize per book path.
static ARCHIVE_NAMES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, Vec<String>>>,
> = std::sync::OnceLock::new();

fn archive_pages(path: &Path, format: &str) -> Result<Vec<String>> {
    let cache = ARCHIVE_NAMES.get_or_init(Default::default);
    if let Some(v) = cache.lock().ok().and_then(|m| m.get(path).cloned()) {
        return Ok(v);
    }
    let names = match format {
        "cbz" => cbz_pages(path)?,
        "cbr" => cbr_pages(path)?,
        _ => bail!("not an archive format: {format}"),
    };
    if let Ok(mut m) = cache.lock() {
        m.insert(path.to_path_buf(), names.clone());
    }
    Ok(names)
}

/// Total fixed pages for pdf/cbz/cbr.
pub fn page_count(path: &Path, format: &str) -> Result<usize> {
    match format {
        "pdf" => pdf_page_count(path),
        "cbz" | "cbr" => Ok(archive_pages(path, format)?.len()),
        _ => bail!("not a fixed-page format: {format}"),
    }
}

/// PDF page count, hardened against the "valid file won't open" bug: `pdfinfo`
/// with an empty user password (so empty-password-encrypted PDFs count), exit
/// status + stderr actually checked, and a pure-Rust `lopdf` fallback when
/// poppler yields nothing (broken xref / stderr-only output / missing binary).
fn pdf_page_count(path: &Path) -> Result<usize> {
    // 1) poppler pdfinfo — the fast path, now with -upw "" and status check.
    let pdfinfo_err = match Command::new("pdfinfo").args(["-upw", ""]).arg(path).output() {
        Ok(out) => {
            let text = String::from_utf8_lossy(&out.stdout);
            let n = text
                .lines()
                .find(|l| l.starts_with("Pages:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse::<usize>().ok());
            if let Some(n) = n.filter(|n| *n > 0) {
                return Ok(n);
            }
            format!(
                "pdfinfo status {}: {}",
                out.status,
                String::from_utf8_lossy(&out.stderr).trim()
            )
        }
        Err(e) => format!("pdfinfo not runnable ({e})"),
    };
    // 2) Fallback: parse the page tree ourselves.
    if let Some(n) = pdf_pages_lopdf(path) {
        return Ok(n);
    }
    bail!("could not read PDF page count ({pdfinfo_err}); install poppler-utils or the file may be corrupt")
}

/// Pure-Rust page count via `lopdf` (fallback when poppler fails).
fn pdf_pages_lopdf(path: &Path) -> Option<usize> {
    let doc = lopdf::Document::load(path).ok()?;
    let n = doc.get_pages().len();
    (n > 0).then_some(n)
}

/// Default rasterisation resolution (DPI) for fixed-page rendering.
pub const BASE_DPI: u32 = 150;

/// Render page `idx` (0-based) at [`BASE_DPI`] to a cached PNG/JPG.
pub fn page_image(path: &Path, format: &str, idx: usize) -> Result<PathBuf> {
    page_image_dpi(path, format, idx, BASE_DPI)
}

/// Render page `idx` (0-based) at `dpi` to a cached PNG/JPG; returns the cache
/// path. Higher `dpi` gives a crisp bitmap for deep zoom (PDF only — comics are
/// stored bitmaps, so `dpi` is ignored there). Cache is keyed by dpi so the
/// 150-DPI page and a zoomed 300-DPI page coexist.
pub fn page_image_dpi(path: &Path, format: &str, idx: usize, dpi: u32) -> Result<PathBuf> {
    let dir = pages_dir(path)?;
    // Comics keep the source entry's extension (a webp stored as ".png" relies
    // on the decoder sniffing content); PDFs always rasterise to PNG.
    let cached = if format == "pdf" {
        dir.join(format!("{idx:05}@{dpi}.png"))
    } else {
        let names = archive_pages(path, format)?;
        let name = names.get(idx).context("page out of range")?;
        let ext = name.rsplit('.').next().unwrap_or("png").to_ascii_lowercase();
        dir.join(format!("{idx:05}@{dpi}.{ext}"))
    };
    if cached.exists() {
        return Ok(cached);
    }
    match format {
        "pdf" => {
            // pdftoppm -png -f N -l N -r DPI -upw "" in.pdf out_prefix → out_prefix-N.png
            let prefix = dir.join(format!("p{idx:05}_{dpi}"));
            let n = (idx + 1).to_string();
            let r = dpi.to_string();
            let st = Command::new("pdftoppm")
                .args(["-png", "-upw", "", "-r", &r, "-f", &n, "-l", &n])
                .arg(path)
                .arg(&prefix)
                .status()
                .context("pdftoppm not found (install poppler-utils)")?;
            if !st.success() {
                bail!("pdftoppm failed on page {n}");
            }
            // poppler appends its own page-number suffix; find the produced file.
            let produced = std::fs::read_dir(&dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| {
                    p.file_name()
                        .and_then(|f| f.to_str())
                        .map(|f| f.starts_with(&format!("p{idx:05}_{dpi}-")))
                        .unwrap_or(false)
                })
                .context("pdftoppm produced no file")?;
            std::fs::rename(produced, &cached)?;
            Ok(cached)
        }
        "cbz" => {
            let names = archive_pages(path, format)?;
            let name = names.get(idx).context("page out of range")?;
            let f = std::fs::File::open(path)?;
            let mut z = zip::ZipArchive::new(f)?;
            let mut e = z.by_name(name)?;
            let mut buf = Vec::new();
            e.read_to_end(&mut buf)?;
            std::fs::write(&cached, &buf)?;
            Ok(cached)
        }
        "cbr" => {
            let names = archive_pages(path, format)?;
            let name = names.get(idx).context("page out of range")?;
            let ok = Command::new("unrar")
                .args(["p", "-inul"])
                .arg(path)
                .arg(name)
                .output()
                .ok()
                .filter(|o| o.status.success() && !o.stdout.is_empty())
                .map(|o| o.stdout)
                .or_else(|| {
                    Command::new("bsdtar")
                        .args(["-xOf"])
                        .arg(path)
                        .arg(name)
                        .output()
                        .ok()
                        .filter(|o| o.status.success() && !o.stdout.is_empty())
                        .map(|o| o.stdout)
                })
                .context("cannot extract CBR page (need unrar or bsdtar)")?;
            std::fs::write(&cached, &ok)?;
            Ok(cached)
        }
        _ => bail!("not a fixed-page format: {format}"),
    }
}

/// Inverted (night) variant of a rendered page, for the dark/OLED reading
/// themes — cached next to the day render. Falls back to the caller on any
/// decode failure (e.g. webp comics: the image crate here only has jpeg/png).
pub fn page_image_night(path: &Path, format: &str, idx: usize) -> Result<PathBuf> {
    let day = page_image(path, format, idx)?;
    let night = day.with_file_name(format!(
        "{}.night.png",
        day.file_stem().and_then(|s| s.to_str()).unwrap_or("page")
    ));
    if night.exists() {
        return Ok(night);
    }
    let mut img = image::open(&day)?.into_rgba8();
    for p in img.pixels_mut() {
        p.0[0] = 255 - p.0[0];
        p.0[1] = 255 - p.0[1];
        p.0[2] = 255 - p.0[2];
    }
    img.save(&night)?;
    Ok(night)
}

/// Prune the rendered-page cache to at most `cap_bytes`, deleting whole
/// per-book page dirs oldest-first (by mtime). Cheap bound so `~/.cache` doesn't
/// grow forever; call on reader open. ponytail: whole-dir LRU, not per-page —
/// good enough, upgrade to per-page LRU only if it ever matters.
pub fn prune_page_cache(cap_bytes: u64) {
    let Some(root) = crate::cache_dir().map(|d| d.join("pages")) else { return };
    let mut dirs: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
    let Ok(rd) = std::fs::read_dir(&root) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let mut size = 0u64;
        let mut newest = std::time::SystemTime::UNIX_EPOCH;
        if let Ok(inner) = std::fs::read_dir(&p) {
            for f in inner.filter_map(|f| f.ok()) {
                if let Ok(m) = f.metadata() {
                    size += m.len();
                    if let Ok(t) = m.modified() {
                        newest = newest.max(t);
                    }
                }
            }
        }
        dirs.push((newest, size, p));
    }
    let mut total: u64 = dirs.iter().map(|(_, s, _)| *s).sum();
    if total <= cap_bytes {
        return;
    }
    dirs.sort_by_key(|(t, _, _)| *t); // oldest first
    for (_, size, path) in dirs {
        if total <= cap_bytes {
            break;
        }
        if std::fs::remove_dir_all(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

/// First page bytes without caching as a page (cover extraction).
pub fn first_page_bytes(path: &Path, format: &str) -> Result<Vec<u8>> {
    let p = page_image(path, format, 0)?;
    Ok(std::fs::read(p)?)
}
