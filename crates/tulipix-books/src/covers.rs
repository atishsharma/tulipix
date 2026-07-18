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
/// Follows the mockup's tilt + perspective (face measured from the frame's
/// alpha: left edge y 28→790, right edge y 0→829) so the art reaches the
/// frame's top and bottom instead of floating with gaps.
const QUAD_TILE: [(f32, f32); 4] = [(12.8, 26.5), (511.4, 0.0), (512.7, 829.0), (12.2, 791.0)];
/// Top-face quad of FRAME_HERO — art TL maps to the book's left corner.
const QUAD_HERO: [(f32, f32); 4] = [(46.1, 231.2), (251.6, 99.8), (552.5, 282.4), (340.7, 468.5)];

/// Average opaque colour of an image, as a hue-preserving multiplier
/// (max channel normalised to 1.0), saturation-boosted (gamma on the
/// normalised channels: max stays 1.0, the others sink) so near-white
/// covers still leave a visible colour on the spine/seam.
fn tint_of_boosted(img: &image::RgbaImage) -> [f32; 3] {
    let t = tint_of(img);
    [t[0].powf(1.8), t[1].powf(1.8), t[2].powf(1.8)]
}

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

/// Suffixes of the baked renditions — versioned: bumping one after any bake
/// change (quad, shadow, tint) makes every cover re-bake; stale
/// `<stem>_book*/_hero*.png` files are just ignored.
pub const BOOK_SUFFIX: &str = "_book5";
pub const HERO_SUFFIX: &str = "_hero2";

/// Cache location of a baked rendition (`suffix` = BOOK_SUFFIX | HERO_SUFFIX).
pub fn baked_path(flat: &Path, suffix: &str) -> PathBuf {
    let stem = flat.file_stem().and_then(|s| s.to_str()).unwrap_or("cover");
    flat.with_file_name(format!("{stem}{suffix}.png"))
}

/// Grid-tile bake: cover art wrapped onto the upright hardcover mockup.
/// Cached as `<stem>{BOOK_SUFFIX}.png`. The drop/shelf shadow is NOT baked —
/// it's theme-dependent, so the UI underlays a shared silhouette-shadow PNG
/// (resources/icons/bookhero/bookshadow{,-dark}.png, same canvas geometry)
/// and picks the light/dark variant at render time.
pub fn bake_book(flat: &Path) -> Result<PathBuf> {
    let out = baked_path(flat, BOOK_SUFFIX);
    if out.exists() {
        return Ok(out);
    }
    let src = image::open(flat).context("open flat cover")?.to_rgba8();
    let frame = image::load_from_memory(FRAME_TILE).context("frame")?.to_rgba8();
    let (fw, fh) = frame.dimensions();
    // Canvas keeps the shadow margin so the shared shadow PNG (628×858)
    // aligns 1:1 under the baked book when both are contain-fitted.
    const MARGIN: u32 = 28;
    let (cw, ch) = (fw + MARGIN, fh + MARGIN);
    let mut canvas = image::RgbaImage::new(cw, ch);

    // Frame onto the empty canvas (plain alpha-over).
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
    // Board tints (before the art warp, which overwrites the face): the bare
    // white strips read as cover-coloured boards — left spine, plus a thin
    // top strip and the outer right strip so the back cover appears to wrap
    // around behind the page block. The page block itself stays untinted.
    {
        let t = tint_of_boosted(&src);
        let tint_px = |canvas: &mut image::RgbaImage, x: u32, y: u32| {
            const S: f32 = 0.85;
            let px = canvas.get_pixel_mut(x, y);
            for c in 0..3 {
                px[c] = (px[c] as f32 * (1.0 - S + S * t[c])) as u8;
            }
        };
        // Left spine band (x ≤ 13, left of the art quad).
        let band = QUAD_TILE[0].0.max(QUAD_TILE[3].0) as u32 + 1;
        for y in 0..fh {
            for x in 0..band.min(fw) {
                if frame.get_pixel(x, y)[3] > 0 {
                    tint_px(&mut canvas, x, y);
                }
            }
        }
        // Top back-board strip: first ~9 rows of the silhouette per column.
        // On the face the art covers it back up bar a 1-2 px peek — exactly
        // the sliver of back cover a real book shows.
        for x in 0..fw {
            if let Some(y0) = (0..fh).find(|&y| frame.get_pixel(x, y)[3] > 10) {
                for y in y0..(y0 + 9).min(fh) {
                    if frame.get_pixel(x, y)[3] > 0 {
                        tint_px(&mut canvas, x, y);
                    }
                }
            }
        }
        // Right back-board strip: outermost ~12 px of rows that reach the
        // page block's outer edge (skip rows ending at the block's underside).
        for y in 0..fh {
            if let Some(x1) = (0..fw).rev().find(|&x| frame.get_pixel(x, y)[3] > 10) {
                if x1 >= 585 {
                    for x in x1.saturating_sub(11)..=x1 {
                        tint_px(&mut canvas, x, y);
                    }
                }
            }
        }
    }
    warp_onto(&mut canvas, &src, QUAD_TILE, Some(&frame));
    canvas.save(&out).context("save baked book")?;
    Ok(out)
}

/// Hero bake: cover art wrapped onto the isometric flat-book mockup; the
/// page-block seam (and soft shadow) is tinted with a vertical gradient of
/// the cover's own average colour. Cached as `<stem>{HERO_SUFFIX}.png`.
pub fn bake_hero(flat: &Path) -> Result<PathBuf> {
    let out = baked_path(flat, HERO_SUFFIX);
    if out.exists() {
        return Ok(out);
    }
    let src = image::open(flat).context("open flat cover")?.to_rgba8();
    let mut frame = image::load_from_memory(FRAME_HERO).context("frame")?.to_rgba8();
    // Boosted tint: plain averages of light covers are near-white and left
    // the page-block seam looking blank — the gamma'd tint keeps a visible
    // colour gradient on the seam for any cover.
    let tint = tint_of_boosted(&src);
    let h = frame.height().max(1) as f32;
    for (_, y, p) in frame.enumerate_pixels_mut() {
        if p[3] == 0 {
            continue;
        }
        // Tint strength grows toward the bottom (deeper pages darker).
        let s = 0.55 + 0.4 * (y as f32 / h);
        for c in 0..3 {
            let f = 1.0 - s + s * tint[c];
            p[c] = (p[c] as f32 * f) as u8;
        }
    }
    warp_onto(&mut frame, &src, QUAD_HERO, None);
    frame.save(&out).context("save baked hero")?;
    Ok(out)
}

// ── Generated placeholder covers (books with no extractable art) ─────────────
// A flat title/author card is rendered in Rust (Sora, on the book's hash hue)
// and then baked through the same warp pipeline as real covers, so coverless
// books get the identical wrapped-on-mockup look instead of a flat overlay.

const FONT_SORA: &[u8] = include_bytes!("../../../resources/fonts/Sora[wght].ttf");

/// Cache path of the generated flat placeholder for `book_path`
/// (`<hash>_ph2.png` next to the real extracted covers — versioned like the
/// bake suffixes; bump on style changes so old flats regenerate).
pub fn placeholder_path(book_path: &Path) -> PathBuf {
    let hash = crate::short_hash(&book_path.display().to_string());
    crate::cache_dir()
        .map(|d| d.join("covers").join(format!("{hash}_ph2.png")))
        .unwrap_or_else(|| PathBuf::from(format!("{hash}_ph2.png")))
}

/// Greedy word wrap at `max_w` px for `px` font size; `None` when any single
/// word is wider than the line (word must never be cut → caller shrinks font).
fn wrap_words<F: ab_glyph::Font>(
    font: &F,
    px: f32,
    text: &str,
    max_w: f32,
) -> Option<Vec<String>> {
    use ab_glyph::ScaleFont;
    let sf = font.as_scaled(px);
    let width = |s: &str| -> f32 {
        s.chars().map(|c| sf.h_advance(sf.scaled_glyph(c).id)).sum()
    };
    let space = width(" ");
    let mut lines: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0.0f32;
    for word in text.split_whitespace() {
        let w = width(word);
        if w > max_w {
            return None;
        }
        if cur.is_empty() {
            cur = word.to_string();
            cur_w = w;
        } else if cur_w + space + w <= max_w {
            cur.push(' ');
            cur.push_str(word);
            cur_w += space + w;
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
            cur_w = w;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    Some(lines)
}

/// Raster `text` centered at baseline `y` (alpha-blended `color`). `bold`
/// double-strikes each glyph 1 px apart (Sora ships one weight; this fakes
/// the heavier cut the covers want).
fn draw_line<F: ab_glyph::Font>(
    img: &mut image::RgbaImage,
    font: &F,
    px: f32,
    y: f32,
    text: &str,
    color: [u8; 3],
    alpha: f32,
    bold: bool,
) {
    draw_line_at(img, font, px, y, 0.0, text, color, alpha);
    if bold {
        draw_line_at(img, font, px, y, 1.0, text, color, alpha);
    }
}

fn draw_line_at<F: ab_glyph::Font>(
    img: &mut image::RgbaImage,
    font: &F,
    px: f32,
    y: f32,
    x_off: f32,
    text: &str,
    color: [u8; 3],
    alpha: f32,
) {
    use ab_glyph::ScaleFont;
    let sf = font.as_scaled(px);
    let total: f32 = text.chars().map(|c| sf.h_advance(sf.scaled_glyph(c).id)).sum();
    let (iw, ih) = img.dimensions();
    let mut pen = (iw as f32 - total) / 2.0 + x_off;
    for ch in text.chars() {
        let glyph = sf.scaled_glyph(ch);
        let advance = sf.h_advance(glyph.id);
        let mut g = glyph;
        g.position = ab_glyph::point(pen, y);
        if let Some(outline) = font.outline_glyph(g) {
            let bb = outline.px_bounds();
            outline.draw(|gx, gy, c| {
                let (x, yy) = (bb.min.x as i64 + gx as i64, bb.min.y as i64 + gy as i64);
                if x < 0 || yy < 0 || x >= iw as i64 || yy >= ih as i64 {
                    return;
                }
                let p = img.get_pixel_mut(x as u32, yy as u32);
                let a = c * alpha;
                for i in 0..3 {
                    p[i] = (color[i] as f32 * a + p[i] as f32 * (1.0 - a)) as u8;
                }
            });
        }
        pen += advance;
    }
}

/// Generate (or reuse) the flat placeholder cover: vertical hue gradient,
/// auto-sized centered title (word-wrapped, words never cut), divider, author.
pub fn placeholder_flat(
    book_path: &Path,
    title: &str,
    author: &str,
    hue: [u8; 3],
) -> Result<PathBuf> {
    let out = placeholder_path(book_path);
    if out.exists() {
        return Ok(out);
    }
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let font = ab_glyph::FontRef::try_from_slice(FONT_SORA)
        .map_err(|e| anyhow::anyhow!("font: {e}"))?;
    // Canvas matches the mockup face's aspect (≈500×800) so the inner border
    // keeps equal margins after the quad warp instead of stretching unevenly.
    let (w, h) = (500u32, 800u32);
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        // Darkens toward the bottom, like a lit cover.
        let f = 1.0 - 0.35 * (y as f32 / h as f32);
        for x in 0..w {
            img.put_pixel(
                x,
                y,
                image::Rgba([
                    (hue[0] as f32 * f) as u8,
                    (hue[1] as f32 * f) as u8,
                    (hue[2] as f32 * f) as u8,
                    255,
                ]),
            );
        }
    }
    // Thin inner border, classic hardcover print look.
    let m = 24u32;
    for x in m..w - m {
        for &y in &[m, h - m - 1] {
            img.put_pixel(x, y, image::Rgba([255, 255, 255, 110]));
        }
    }
    for y in m..h - m {
        for &x in &[m, w - m - 1] {
            img.put_pixel(x, y, image::Rgba([255, 255, 255, 110]));
        }
    }
    // Title: BOLD UPPERCASE, one word per line, the whole text group centered
    // on the cover. Font auto-sizes down until the widest word fits the line
    // and the group fits its zone — a word is never cut at the edge.
    let title = if title.trim().is_empty() { "Untitled" } else { title };
    let words: Vec<String> = title
        .to_uppercase()
        .split_whitespace()
        .take(8)
        .map(String::from)
        .collect();
    let max_w = w as f32 - 110.0;
    let widest = |px: f32| -> f32 {
        use ab_glyph::{Font, ScaleFont};
        let sf = font.as_scaled(px);
        words
            .iter()
            .map(|wd| wd.chars().map(|c| sf.h_advance(sf.scaled_glyph(c).id)).sum::<f32>())
            .fold(0.0, f32::max)
    };
    let mut px = 60.0f32;
    while px > 18.0 {
        let fits_w = widest(px) + 1.0 <= max_w; // +1 for the bold strike
        let fits_h = words.len() as f32 * px * 1.3 <= h as f32 * 0.62;
        if fits_w && fits_h {
            break;
        }
        px -= 2.0;
    }
    let line_h = px * 1.3;
    let title_h = words.len() as f32 * line_h;
    // Author block (small, under a divider) is part of the centered group.
    let apx = 22.0f32;
    let author_lines: Vec<String> = if author.trim().is_empty() {
        Vec::new()
    } else {
        wrap_words(&font, apx, author, max_w)
            .unwrap_or_default()
            .into_iter()
            .take(2)
            .collect()
    };
    let author_h = if author_lines.is_empty() {
        0.0
    } else {
        30.0 + author_lines.len() as f32 * apx * 1.3 // divider gap + lines
    };
    let group_h = title_h + author_h;
    let mut y = (h as f32 - group_h) / 2.0 + px * 0.9; // baseline of first line
    for wd in &words {
        draw_line(&mut img, &font, px, y, wd, [255, 255, 255], 1.0, true);
        y += line_h;
    }
    if !author_lines.is_empty() {
        let dv_y = ((y - px * 0.9 + 8.0) as u32).min(h - 2);
        for x in (w / 2 - 26)..(w / 2 + 26) {
            img.put_pixel(x, dv_y, image::Rgba([255, 255, 255, 130]));
        }
        let mut ay = dv_y as f32 + 14.0 + apx;
        for line in &author_lines {
            draw_line(&mut img, &font, apx, ay, line, [255, 255, 255], 0.85, false);
            ay += apx * 1.3;
        }
    }
    img.save(&out).context("save placeholder cover")?;
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
