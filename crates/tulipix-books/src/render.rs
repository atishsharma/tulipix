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

/// Total fixed pages for pdf/cbz/cbr.
pub fn page_count(path: &Path, format: &str) -> Result<usize> {
    match format {
        "pdf" => {
            let out = Command::new("pdfinfo").arg(path).output()
                .context("pdfinfo not found (install poppler-utils)")?;
            let text = String::from_utf8_lossy(&out.stdout).to_string();
            text.lines()
                .find(|l| l.starts_with("Pages:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|n| n.parse().ok())
                .context("pdfinfo gave no page count")
        }
        "cbz" => Ok(cbz_pages(path)?.len()),
        "cbr" => Ok(cbr_pages(path)?.len()),
        _ => bail!("not a fixed-page format: {format}"),
    }
}

/// Render page `idx` (0-based) to a cached PNG/JPG; returns the cache path.
pub fn page_image(path: &Path, format: &str, idx: usize) -> Result<PathBuf> {
    let dir = pages_dir(path)?;
    let cached = dir.join(format!("{idx:05}.png"));
    if cached.exists() {
        return Ok(cached);
    }
    match format {
        "pdf" => {
            // pdftoppm -png -f N -l N -r 150 in.pdf out_prefix → out_prefix-N.png
            let prefix = dir.join(format!("p{idx:05}"));
            let n = (idx + 1).to_string();
            let st = Command::new("pdftoppm")
                .args(["-png", "-r", "150", "-f", &n, "-l", &n])
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
                        .map(|f| f.starts_with(&format!("p{idx:05}-")))
                        .unwrap_or(false)
                })
                .context("pdftoppm produced no file")?;
            std::fs::rename(produced, &cached)?;
            Ok(cached)
        }
        "cbz" => {
            let names = cbz_pages(path)?;
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
            let names = cbr_pages(path)?;
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

/// First page bytes without caching as a page (cover extraction).
pub fn first_page_bytes(path: &Path, format: &str) -> Result<Vec<u8>> {
    let p = page_image(path, format, 0)?;
    Ok(std::fs::read(p)?)
}
