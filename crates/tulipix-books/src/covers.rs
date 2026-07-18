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

// ── 3D bake: warp the flat cover onto blank-book mockups ──────────────────────
// The UI can't perspective-warp, so the "wrapped onto the book" look is baked
// into a cached PNG per cover: the art is quad-warped onto the mockup's face
// and the mockup's own lighting is multiplied back over it, so cover + book
// read as one object instead of a flat paste.

/// Grid-tile frame: upright hardcover, 600×830, transparent background.
const FRAME_TILE: &[u8] = include_bytes!("../../../resources/icons/bookhero/bookframe.png");
/// Hero frame: isometric flat book, 600×600, transparent background.
const FRAME_HERO: &[u8] = include_bytes!("../../../resources/icons/bookhero/continueframe.png");

/// Front-face quad of FRAME_TILE — [TL, TR, BR, BL] of the cover art.
const QUAD_TILE: [(f32, f32); 4] = [(12.8, 3.9), (511.4, 0.0), (512.7, 811.1), (12.2, 782.9)];
/// Top-face quad of FRAME_HERO — art TL maps to the book's left corner.
const QUAD_HERO: [(f32, f32); 4] = [(46.1, 231.2), (251.6, 99.8), (552.5, 282.4), (340.7, 468.5)];

/// Average opaque colour of an image, as a hue-preserving multiplier
/// (max channel normalised to 1.0).
fn tint_of(img: &image::RgbaImage) -> [f32; 3] {
    let (mut r, mut g, mut b, mut n) = (0f64, 0f64, 0f64, 0f64);
    for p in img.pixels() {
        if p[3] > 128 {
            r += p[0] as f64;
            g += p[1] as f64;
            b += p[2] as f64;
            n += 1.0;
        }
    }
    if n == 0.0 {
        return [1.0, 1.0, 1.0];
    }
    let (r, g, b) = ((r / n) as f32, (g / n) as f32, (b / n) as f32);
    let m = r.max(g).max(b).max(1.0);
    [r / m, g / m, b / m]
}

/// Forward-map `src` onto the bilinear patch `quad` ([TL,TR,BR,BL]) of
/// `canvas`, 2× oversampled so no holes remain. When `shade` is given
/// (the untouched frame), its luminance is multiplied over the art so the
/// mockup's highlights and edge shadows survive on top of the cover.
fn warp_onto(
    canvas: &mut image::RgbaImage,
    src: &image::RgbaImage,
    quad: [(f32, f32); 4],
    shade: Option<&image::RgbaImage>,
) {
    let (cw, ch) = canvas.dimensions();
    let (sw, sh) = src.dimensions();
    let len = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).hypot(a.1 - b.1);
    let su = (len(quad[0], quad[1]).max(len(quad[3], quad[2])) * 2.0) as u32;
    let sv = (len(quad[0], quad[3]).max(len(quad[1], quad[2])) * 2.0) as u32;
    for i in 0..=su {
        let u = i as f32 / su as f32;
        for j in 0..=sv {
            let v = j as f32 / sv as f32;
            let x = (1.0 - u) * (1.0 - v) * quad[0].0
                + u * (1.0 - v) * quad[1].0
                + u * v * quad[2].0
                + (1.0 - u) * v * quad[3].0;
            let y = (1.0 - u) * (1.0 - v) * quad[0].1
                + u * (1.0 - v) * quad[1].1
                + u * v * quad[2].1
                + (1.0 - u) * v * quad[3].1;
            let (cx, cy) = (x.round() as i64, y.round() as i64);
            if cx < 0 || cy < 0 || cx >= cw as i64 || cy >= ch as i64 {
                continue;
            }
            let sp = src.get_pixel(
                ((u * (sw - 1) as f32) as u32).min(sw - 1),
                ((v * (sh - 1) as f32) as u32).min(sh - 1),
            );
            let mut px = *sp;
            if let Some(fr) = shade {
                let f = fr.get_pixel(cx as u32, cy as u32);
                let lum = (f[0] as f32 * 0.299 + f[1] as f32 * 0.587 + f[2] as f32 * 0.114)
                    / 245.0;
                let l = lum.clamp(0.55, 1.08);
                for c in 0..3 {
                    px[c] = (px[c] as f32 * l).min(255.0) as u8;
                }
            }
            px[3] = 255;
            canvas.put_pixel(cx as u32, cy as u32, px);
        }
    }
}

/// Cache location of a baked rendition (`suffix` = "_book" | "_hero").
pub fn baked_path(flat: &Path, suffix: &str) -> PathBuf {
    let stem = flat.file_stem().and_then(|s| s.to_str()).unwrap_or("cover");
    flat.with_file_name(format!("{stem}{suffix}.png"))
}

/// Grid-tile bake: cover art wrapped onto the upright hardcover mockup, with
/// a soft drop shadow to the right + below (follows the book silhouette) so
/// the book looks placed into the card. Cached as `<stem>_book.png`.
pub fn bake_book(flat: &Path) -> Result<PathBuf> {
    let out = baked_path(flat, "_book");
    if out.exists() {
        return Ok(out);
    }
    let src = image::open(flat).context("open flat cover")?.to_rgba8();
    let frame = image::load_from_memory(FRAME_TILE).context("frame")?.to_rgba8();
    let (fw, fh) = frame.dimensions();
    // Canvas grows right+bottom so the offset shadow has room.
    const MARGIN: u32 = 28;
    const DX: u32 = 10; // shadow offset right
    const DY: u32 = 12; // shadow offset down
    const BLUR: u32 = 8;
    const ALPHA: f32 = 0.34;
    let (cw, ch) = (fw + MARGIN, fh + MARGIN);
    let mut canvas = image::RgbaImage::new(cw, ch);

    // Shadow = frame alpha, offset, box-blurred twice (≈ gaussian).
    let mut a = vec![0f32; (cw * ch) as usize];
    for y in 0..fh {
        for x in 0..fw {
            a[((y + DY) * cw + x + DX) as usize] = frame.get_pixel(x, y)[3] as f32 / 255.0;
        }
    }
    for _ in 0..2 {
        // horizontal then vertical box pass
        let mut b = vec![0f32; a.len()];
        for y in 0..ch {
            for x in 0..cw {
                let (mut s, mut n) = (0f32, 0f32);
                for k in x.saturating_sub(BLUR)..(x + BLUR + 1).min(cw) {
                    s += a[(y * cw + k) as usize];
                    n += 1.0;
                }
                b[(y * cw + x) as usize] = s / n;
            }
        }
        for x in 0..cw {
            for y in 0..ch {
                let (mut s, mut n) = (0f32, 0f32);
                for k in y.saturating_sub(BLUR)..(y + BLUR + 1).min(ch) {
                    s += b[(k * cw + x) as usize];
                    n += 1.0;
                }
                a[(y * cw + x) as usize] = s / n;
            }
        }
    }
    for y in 0..ch {
        for x in 0..cw {
            let v = a[(y * cw + x) as usize];
            if v > 0.003 {
                canvas.put_pixel(x, y, image::Rgba([22, 22, 45, (v * ALPHA * 255.0) as u8]));
            }
        }
    }
    // Frame over the shadow (plain alpha-over).
    for y in 0..fh {
        for x in 0..fw {
            let p = frame.get_pixel(x, y);
            if p[3] == 255 {
                canvas.put_pixel(x, y, *p);
            } else if p[3] > 0 {
                let q = canvas.get_pixel(x, y);
                let fa = p[3] as f32 / 255.0;
                let out_px = image::Rgba([
                    (p[0] as f32 * fa + q[0] as f32 * (1.0 - fa)) as u8,
                    (p[1] as f32 * fa + q[1] as f32 * (1.0 - fa)) as u8,
                    (p[2] as f32 * fa + q[2] as f32 * (1.0 - fa)) as u8,
                    ((fa + q[3] as f32 / 255.0 * (1.0 - fa)) * 255.0) as u8,
                ]);
                canvas.put_pixel(x, y, out_px);
            }
        }
    }
    warp_onto(&mut canvas, &src, QUAD_TILE, Some(&frame));
    canvas.save(&out).context("save baked book")?;
    Ok(out)
}

/// Hero bake: cover art wrapped onto the isometric flat-book mockup; the
/// page-block seam (and soft shadow) is tinted with a vertical gradient of
/// the cover's own average colour. Cached as `<stem>_hero.png`.
pub fn bake_hero(flat: &Path) -> Result<PathBuf> {
    let out = baked_path(flat, "_hero");
    if out.exists() {
        return Ok(out);
    }
    let src = image::open(flat).context("open flat cover")?.to_rgba8();
    let mut frame = image::load_from_memory(FRAME_HERO).context("frame")?.to_rgba8();
    let tint = tint_of(&src);
    let h = frame.height().max(1) as f32;
    for (_, y, p) in frame.enumerate_pixels_mut() {
        if p[3] == 0 {
            continue;
        }
        // Tint strength grows toward the bottom (deeper pages darker).
        let s = 0.35 + 0.5 * (y as f32 / h);
        for c in 0..3 {
            let f = 1.0 - s + s * tint[c];
            p[c] = (p[c] as f32 * f) as u8;
        }
    }
    warp_onto(&mut frame, &src, QUAD_HERO, None);
    frame.save(&out).context("save baked hero")?;
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
