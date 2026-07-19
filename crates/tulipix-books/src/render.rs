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

/// Sorted comic page names inside an archive we read through an external tool:
/// CBR (rar) via `unrar`, CB7 (7z) and CBT (tar) via `bsdtar`. `bsdtar` also
/// backs up `unrar`, since libarchive can usually read rar too.
fn external_pages(path: &Path, format: &str) -> Result<Vec<String>> {
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
    let rar_first = format == "cbr";
    rar_first
        .then(|| try_list("unrar", &["lb"]))
        .flatten()
        .or_else(|| try_list("bsdtar", &["-tf"]))
        .filter(|v| !v.is_empty())
        .with_context(|| {
            if rar_first {
                "cannot list CBR (need unrar or bsdtar)".to_string()
            } else {
                format!("cannot list {} (need bsdtar)", format.to_uppercase())
            }
        })
}

/// Extract one entry from an external-tool archive, as bytes.
fn external_entry(path: &Path, format: &str, name: &str) -> Result<Vec<u8>> {
    let run = |cmd: &str, args: &[&str]| -> Option<Vec<u8>> {
        Command::new(cmd)
            .args(args)
            .arg(path)
            .arg(name)
            .output()
            .ok()
            .filter(|o| o.status.success() && !o.stdout.is_empty())
            .map(|o| o.stdout)
    };
    (format == "cbr")
        .then(|| run("unrar", &["p", "-inul"]))
        .flatten()
        .or_else(|| run("bsdtar", &["-xOf"]))
        .context("cannot extract archive page (need unrar or bsdtar)")
}

/// Archive page listings are expensive (full zip scan, or an `unrar` process)
/// and used to re-run on every page render — memoize per book path, stamped
/// with the file's mtime so editing a comic in place doesn't serve a stale
/// page list until restart.
static ARCHIVE_NAMES: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashMap<PathBuf, (u64, Vec<String>)>>,
> = std::sync::OnceLock::new();

/// Cached archives kept before the map is dropped. Only the open book (and
/// whatever the user just closed) matters, so a plain clear beats an LRU.
const ARCHIVE_CACHE_MAX: usize = 8;

fn mtime_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn archive_pages(path: &Path, format: &str) -> Result<Vec<String>> {
    let cache = ARCHIVE_NAMES.get_or_init(Default::default);
    let stamp = mtime_of(path);
    if let Ok(m) = cache.lock() {
        if let Some((t, v)) = m.get(path) {
            if *t == stamp {
                return Ok(v.clone());
            }
        }
    }
    let names = match format {
        "cbz" => cbz_pages(path)?,
        "cbr" | "cb7" | "cbt" => external_pages(path, format)?,
        // Fixed-layout EPUB: an EPUB is a zip, so its page images extract
        // through the same path as a CBZ — only the page list differs (spine
        // order, not filename order).
        "epubfx" => crate::epub::fixed_layout_pages(path)?,
        _ => bail!("not an archive format: {format}"),
    };
    if let Ok(mut m) = cache.lock() {
        if m.len() >= ARCHIVE_CACHE_MAX {
            m.clear();
        }
        m.insert(path.to_path_buf(), (stamp, names.clone()));
    }
    Ok(names)
}

/// Total fixed pages for pdf/cbz/cbr.
pub fn page_count(path: &Path, format: &str) -> Result<usize> {
    match format {
        "pdf" => pdf_page_count(path),
        "djvu" => djvu_page_count(path),
        "cbz" | "cbr" | "cb7" | "cbt" | "epubfx" => Ok(archive_pages(path, format)?.len()),
        _ => bail!("not a fixed-page format: {format}"),
    }
}

/// DjVu page count via `djvused` (djvulibre). Scanned books and older
/// magazines are commonly distributed this way.
fn djvu_page_count(path: &Path) -> Result<usize> {
    let out = Command::new("djvused")
        .arg(path)
        .args(["-e", "n"])
        .output()
        .context("djvused not found (install djvulibre)")?;
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .find_map(|l| l.trim().parse::<usize>().ok())
        .filter(|n| *n > 0)
        .context("could not read DjVu page count")
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
    // Rasterised formats always land as PNG; archives keep the stored entry's
    // extension (a webp saved as ".png" relies on the decoder sniffing content).
    let cached = if matches!(format, "pdf" | "djvu") {
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
            // The page-number suffix poppler appends is zero-padded to the
            // document's digit count, so we can't name the output up front —
            // instead render into a private scratch dir, which by construction
            // holds exactly the one file we just produced. (Scanning the page
            // dir for it meant walking thousands of cached pages per render.)
            let scratch = dir.join(format!(".r{idx:05}_{dpi}"));
            let _ = std::fs::remove_dir_all(&scratch);
            std::fs::create_dir_all(&scratch)?;
            let n = (idx + 1).to_string();
            let r = dpi.to_string();
            let st = Command::new("pdftoppm")
                .args(["-png", "-upw", "", "-r", &r, "-f", &n, "-l", &n])
                .arg(path)
                .arg(scratch.join("p"))
                .status()
                .context("pdftoppm not found (install poppler-utils)")?;
            if !st.success() {
                let _ = std::fs::remove_dir_all(&scratch);
                bail!("pdftoppm failed on page {n}");
            }
            let produced = std::fs::read_dir(&scratch)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .find(|p| p.is_file());
            let res = match produced {
                Some(p) => std::fs::rename(p, &cached).map_err(anyhow::Error::from),
                None => Err(anyhow::anyhow!("pdftoppm produced no file")),
            };
            let _ = std::fs::remove_dir_all(&scratch);
            res?;
            Ok(cached)
        }
        // Both are zips; only the page list differs (see `archive_pages`).
        "cbz" | "epubfx" => {
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
        "cbr" | "cb7" | "cbt" => {
            let names = archive_pages(path, format)?;
            let name = names.get(idx).context("page out of range")?;
            let bytes = external_entry(path, format, name)?;
            std::fs::write(&cached, &bytes)?;
            Ok(cached)
        }
        "djvu" => {
            // ddjvu has no PNG output, so it renders to PPM and we transcode.
            // `-scale` is DPI, matching the PDF path's resolution knob.
            let ppm = dir.join(format!("d{idx:05}_{dpi}.ppm"));
            let st = Command::new("ddjvu")
                .args([
                    "-format=ppm",
                    &format!("-page={}", idx + 1),
                    &format!("-scale={dpi}"),
                ])
                .arg(path)
                .arg(&ppm)
                .status()
                .context("ddjvu not found (install djvulibre)")?;
            if !st.success() {
                let _ = std::fs::remove_file(&ppm);
                bail!("ddjvu failed on page {}", idx + 1);
            }
            let res = image::open(&ppm)
                .context("decode djvu page")
                .and_then(|img| img.save(&cached).context("save djvu page"));
            let _ = std::fs::remove_file(&ppm);
            res?;
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

/// Bounding box of real content on a page, ignoring a uniform border.
/// Scanned comics and books carry wide, near-uniform margins that waste 15–25%
/// of the screen. `None` when there is nothing worth cropping.
///
/// The background level is taken from the corners rather than assumed white,
/// so inverted (night) renders and black-bordered scans work too.
fn content_bbox(img: &image::GrayImage) -> Option<(u32, u32, u32, u32)> {
    const TOL: i32 = 14;
    let (w, h) = img.dimensions();
    if w < 32 || h < 32 {
        return None;
    }
    // Median of the four corners — robust to one stray dark corner.
    let mut corners = [
        img.get_pixel(0, 0)[0],
        img.get_pixel(w - 1, 0)[0],
        img.get_pixel(0, h - 1)[0],
        img.get_pixel(w - 1, h - 1)[0],
    ];
    corners.sort_unstable();
    let bg = ((corners[1] as u16 + corners[2] as u16) / 2) as i32;
    // Sample rather than test every pixel: margins are uniform by definition,
    // and this runs per page.
    let step = (w.max(h) / 256).max(1);
    let uniform_row = |y: u32| {
        (0..w).step_by(step as usize).all(|x| (img.get_pixel(x, y)[0] as i32 - bg).abs() <= TOL)
    };
    let uniform_col = |x: u32| {
        (0..h).step_by(step as usize).all(|y| (img.get_pixel(x, y)[0] as i32 - bg).abs() <= TOL)
    };
    let top = (0..h).find(|&y| !uniform_row(y))?;
    let bottom = (0..h).rev().find(|&y| !uniform_row(y))?;
    let left = (0..w).find(|&x| !uniform_col(x))?;
    let right = (0..w).rev().find(|&x| !uniform_col(x))?;
    if right <= left || bottom <= top {
        return None;
    }
    let (cw, ch) = (right - left + 1, bottom - top + 1);
    // Refuse to crop when there's barely a margin (not worth a re-encode) or
    // when the "content" is a tiny fraction — that means the heuristic lost,
    // and cropping to it would destroy the page.
    let area_ratio = (cw as f32 * ch as f32) / (w as f32 * h as f32);
    if area_ratio > 0.97 || area_ratio < 0.25 {
        return None;
    }
    Some((left, top, cw, ch))
}

/// Margin-trimmed variant of a rendered page, cached next to it. Falls back to
/// the untrimmed path when there is no worthwhile crop.
pub fn page_image_trimmed(path: &Path, format: &str, idx: usize, night: bool) -> Option<PathBuf> {
    let src = page_image_for(path, format, idx, night)?;
    let out = src.with_file_name(format!(
        "{}.trim.png",
        src.file_stem().and_then(|s| s.to_str()).unwrap_or("page")
    ));
    if out.exists() {
        return Some(out);
    }
    let img = image::open(&src).ok()?;
    let (x, y, w, h) = content_bbox(&img.to_luma8())?;
    image::imageops::crop_imm(&img.to_rgba8(), x, y, w, h)
        .to_image()
        .save(&out)
        .ok()?;
    Some(out)
}

/// The page exactly as the reader is currently configured to show it: night
/// inversion and margin trimming applied, each falling back cleanly when it
/// can't be done. Display and prefetch both go through here so they can never
/// warm different variants.
pub fn page_image_view(
    path: &Path,
    format: &str,
    idx: usize,
    night: bool,
    trim: bool,
) -> Option<PathBuf> {
    if trim {
        if let Some(p) = page_image_trimmed(path, format, idx, night) {
            return Some(p);
        }
    }
    page_image_for(path, format, idx, night)
}

/// Long edge of a page thumbnail, in px.
const THUMB_W: u32 = 150;
/// Rasterisation DPI for PDF thumbnails — a fraction of [`BASE_DPI`], because
/// a thumbnail rendered at reading resolution and then thrown away is most of
/// the cost of the whole panel.
const THUMB_DPI: u32 = 24;

/// Small preview of a page for the thumbnail navigator, cached alongside the
/// full-size renders as `t<idx>.jpg`.
pub fn page_thumb(path: &Path, format: &str, idx: usize) -> Result<PathBuf> {
    let dir = pages_dir(path)?;
    let out = dir.join(format!("t{idx:05}.jpg"));
    if out.exists() {
        return Ok(out);
    }
    // PDFs re-rasterise cheaply at low DPI. Archive pages are stored bitmaps,
    // so the full page has to come out either way — but it is usually already
    // in the cache from reading.
    let src = if format == "pdf" {
        page_image_dpi(path, format, idx, THUMB_DPI)?
    } else {
        page_image(path, format, idx)?
    };
    let img = image::open(&src).context("open page for thumb")?;
    let img = img.thumbnail(THUMB_W, THUMB_W * 3);
    image::DynamicImage::from(img.to_rgb8()).save(&out).context("save thumb")?;
    Ok(out)
}

/// Rendered page in whichever variant the reader is currently showing:
/// inverted for the dark/OLED themes, falling back to the day raster when
/// inversion isn't possible (e.g. webp comics — the `image` build here only
/// decodes jpeg/png). Both the display path and the prefetch warm-up go
/// through this, so paging ahead in a dark theme caches what will be shown.
pub fn page_image_for(path: &Path, format: &str, idx: usize, night: bool) -> Option<PathBuf> {
    if night {
        if let Ok(p) = page_image_night(path, format, idx) {
            return Some(p);
        }
    }
    page_image(path, format, idx).ok()
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
