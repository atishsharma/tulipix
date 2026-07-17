//! Cover extraction — one downscaled PNG per book under
//! `~/.cache/Tulipix/books/covers/<hash>.png`. Empty string = no cover
//! (the UI shows a title-on-spine placeholder card).

use crate::{epub, render};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const MAX_EDGE: u32 = 480;

fn cover_path_for(book_path: &Path) -> Result<PathBuf> {
    let root = crate::cache_dir().context("no cache dir")?;
    let d = root.join("covers");
    std::fs::create_dir_all(&d)?;
    Ok(d.join(format!("{}.png", crate::short_hash(&book_path.display().to_string()))))
}

/// Extract (or reuse) the cover for a book. Returns the cached PNG path,
/// or Err when the format yields no image.
pub fn extract(book_path: &Path, format: &str) -> Result<PathBuf> {
    let out = cover_path_for(book_path)?;
    if out.exists() {
        return Ok(out);
    }
    let bytes = match format {
        "epub" => epub_cover_bytes(book_path)?,
        "pdf" | "cbz" | "cbr" => render::first_page_bytes(book_path, format)?,
        "mobi" | "azw3" => mobi_cover_bytes(book_path)?,
        _ => anyhow::bail!("unknown format {format}"),
    };
    let img = image::load_from_memory(&bytes).context("decode cover")?;
    let img = img.thumbnail(MAX_EDGE, MAX_EDGE * 2);
    img.save(&out).context("save cover png")?;
    Ok(out)
}

fn epub_cover_bytes(path: &Path) -> Result<Vec<u8>> {
    let doc = epub::open(path)?;
    // EPUB3: manifest item with properties="cover-image".
    let by_props = doc
        .manifest
        .iter()
        .find(|(_, _, _, props)| props.split_whitespace().any(|p| p == "cover-image"))
        .map(|(_, p, _, _)| p.clone());
    // EPUB2: <meta name="cover" content="item-id"/>.
    let by_meta = || -> Option<String> {
        let id = epub::tags(&doc.opf, "meta")
            .iter()
            .find(|t| epub::attr(t, "name").as_deref() == Some("cover"))
            .and_then(|t| epub::attr(t, "content"))?;
        doc.manifest
            .iter()
            .find(|(mid, _, mt, _)| *mid == id && mt.starts_with("image/"))
            .map(|(_, p, _, _)| p.clone())
    };
    // Last resort: any manifest image whose id/path mentions "cover".
    let by_name = || -> Option<String> {
        doc.manifest
            .iter()
            .find(|(id, p, mt, _)| {
                mt.starts_with("image/")
                    && (id.to_ascii_lowercase().contains("cover")
                        || p.to_ascii_lowercase().contains("cover"))
            })
            .map(|(_, p, _, _)| p.clone())
    };
    let img_path = by_props
        .or_else(by_meta)
        .or_else(by_name)
        .context("epub has no identifiable cover")?;
    epub::zip_bytes(path, &img_path).context("cover entry missing in zip")
}

fn mobi_cover_bytes(path: &Path) -> Result<Vec<u8>> {
    let m = mobi::Mobi::from_path(path)?;
    let recs = m.image_records();
    // Largest image record is almost always the cover.
    recs.iter()
        .max_by_key(|r| r.content.len())
        .map(|r| r.content.to_vec())
        .context("mobi has no image records")
}
