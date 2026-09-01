//! `np.p4.tools.pdf` — PDF tools (merge / split / OCR / extract images /
//! compress).
//!
//! Backed by lopdf. This owns the page-range parsing (the `"1-3,5,8-10"`
//! syntax everyone gets wrong) and, since the PDF operations landed, the page
//! surgery itself — keep, drop and turn.
//!
//! The surgery lives here rather than in the bridge for the same reason the
//! rename listing does: the preview counts the pages and marks which ones
//! survive, the run removes them, and a preview that disagreed with the run
//! about which page is page 3 would be worse than no preview.

use anyhow::{Context, Result, bail};

/// Parse a page-range spec like `"1-3,5,8-10"` into 1-based page numbers,
/// de-duplicated and sorted. Returns `None` on malformed input.
pub fn parse_ranges(spec: &str) -> Option<Vec<u32>> {
    let mut pages = std::collections::BTreeSet::new();
    for part in spec.split(',') {
        let part = part.trim();
        if part.is_empty() { continue; }
        match part.split_once('-') {
            Some((a, b)) => {
                let (a, b): (u32, u32) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
                if a == 0 || b < a { return None; }
                for p in a..=b { pages.insert(p); }
            }
            None => {
                let p: u32 = part.parse().ok()?;
                if p == 0 { return None; }
                pages.insert(p);
            }
        }
    }
    if pages.is_empty() { None } else { Some(pages.into_iter().collect()) }
}

/// Pages kept when splitting — clamped to the document's `page_count`.
pub fn clamp_pages(pages: &[u32], page_count: u32) -> Vec<u32> {
    pages.iter().copied().filter(|p| *p >= 1 && *p <= page_count).collect()
}

/// How many pages a document has, in the order its page tree says — which is
/// not the order its objects are stored in, in anything a report generator
/// wrote. `None` when the file cannot be opened at all.
pub fn page_count(path: &str) -> Option<u32> {
    let doc = lopdf::Document::load(path).ok()?;
    let n = doc.get_pages().len() as u32;
    (n > 0).then_some(n)
}

/// The page numbers an operation acts on: the parsed range clamped to the
/// document, or every page when the range is blank.
pub fn selected(ranges: &str, page_count: u32) -> Vec<u32> {
    if ranges.trim().is_empty() {
        return (1..=page_count).collect();
    }
    parse_ranges(ranges).map(|p| clamp_pages(&p, page_count)).unwrap_or_default()
}

/// Keep only `keep` (1-based), writing `out`. Returns how many pages survived.
pub fn keep_pages(input: &str, keep: &[u32], out: &str) -> Result<u32> {
    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let all: Vec<u32> = doc.get_pages().keys().copied().collect();
    let drop: Vec<u32> = all.iter().copied().filter(|p| !keep.contains(p)).collect();
    if drop.len() == all.len() {
        bail!("that range keeps no pages");
    }
    doc.delete_pages(&drop);
    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok((all.len() - drop.len()) as u32)
}

/// Drop `drop` (1-based), writing `out`. Returns how many pages were removed.
pub fn drop_pages(input: &str, drop: &[u32], out: &str) -> Result<u32> {
    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let all = doc.get_pages().len();
    if drop.is_empty() {
        bail!("that range names no pages");
    }
    if drop.len() >= all {
        bail!("that would delete every page");
    }
    doc.delete_pages(drop);
    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(drop.len() as u32)
}

/// Turn `pages` (1-based; empty means every page) by `degrees`, writing `out`.
///
/// `/Rotate` is a property of the page and it is cumulative: a page already
/// turned 90° and turned 90° again is at 180°, not back at 90°.
pub fn rotate_pages(input: &str, pages: &[u32], degrees: i64, out: &str) -> Result<u32> {
    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let targets: Vec<lopdf::ObjectId> = doc
        .get_pages()
        .into_iter()
        .filter(|(n, _)| pages.is_empty() || pages.contains(n))
        .map(|(_, id)| id)
        .collect();
    if targets.is_empty() {
        bail!("that range names no pages");
    }
    let mut turned = 0u32;
    for id in targets {
        let existing = doc
            .get_object(id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .and_then(|d| d.get(b"Rotate").ok())
            .and_then(|o| o.as_i64().ok())
            .unwrap_or(0);
        let next = normalise_turn(existing + degrees);
        if let Ok(dict) = doc.get_object_mut(id).and_then(|o| o.as_dict_mut()) {
            dict.set("Rotate", lopdf::Object::Integer(next));
            turned += 1;
        }
    }
    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(turned)
}

/// Where a stamp sits on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Corner { BottomCentre, BottomRight, BottomLeft, TopCentre, TopRight }

impl Corner {
    pub fn parse(s: &str) -> Corner {
        match s.trim().to_ascii_lowercase().replace([' ', '-'], "_").as_str() {
            "bottom_right" => Corner::BottomRight,
            "bottom_left" => Corner::BottomLeft,
            "top_centre" | "top_center" => Corner::TopCentre,
            "top_right" => Corner::TopRight,
            _ => Corner::BottomCentre,
        }
    }
}

/// Margin from the page edge, in points. Half an inch, which is what a printer
/// will not clip.
const STAMP_MARGIN: f64 = 36.0;

/// Where to put the text baseline on a `w`×`h` page.
///
/// The horizontal placement guesses the string's width at half an em per
/// character, which is close for Helvetica digits and close enough for a short
/// stamp. Measuring properly means parsing the font's widths table, for a
/// nudge nobody will see.
pub fn stamp_xy(corner: Corner, w: f64, h: f64, chars: usize, size: f64) -> (f64, f64) {
    let width = chars as f64 * size * 0.5;
    let (x, y) = match corner {
        Corner::BottomCentre => (w / 2.0 - width / 2.0, STAMP_MARGIN),
        Corner::BottomRight => (w - STAMP_MARGIN - width, STAMP_MARGIN),
        Corner::BottomLeft => (STAMP_MARGIN, STAMP_MARGIN),
        Corner::TopCentre => (w / 2.0 - width / 2.0, h - STAMP_MARGIN - size),
        Corner::TopRight => (w - STAMP_MARGIN - width, h - STAMP_MARGIN - size),
    };
    (x.max(0.0), y.max(0.0))
}

/// `{n}` is the page number and `{total}` is how many there are. Everything
/// else in the pattern is left alone, so "Page {n} of {total}" and a plain
/// "DRAFT" are both just patterns.
pub fn stamp_text(pattern: &str, page: u32, total: u32) -> String {
    pattern
        .replace("{n}", &page.to_string())
        .replace("{total}", &total.to_string())
}

/// Draw `pattern` on `pages` (1-based; empty means every page), writing `out`.
///
/// The text goes on top of whatever is already there, inside its own `q`/`Q`
/// pair so it cannot leak graphics state into the page's own drawing. The font
/// is Helvetica, one of the fourteen every reader has built in, so nothing is
/// embedded and the file barely grows.
pub fn stamp_pages(
    input: &str,
    pages: &[u32],
    pattern: &str,
    corner: Corner,
    size: f64,
    out: &str,
) -> Result<u32> {
    use lopdf::content::{Content, Operation};
    use lopdf::{Dictionary, Object};

    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let all = doc.get_pages();
    let total = all.len() as u32;
    let targets: Vec<(u32, lopdf::ObjectId)> = all
        .into_iter()
        .filter(|(n, _)| pages.is_empty() || pages.contains(n))
        .collect();
    if targets.is_empty() {
        bail!("that range names no pages");
    }
    let size = size.clamp(4.0, 96.0);

    let mut font = Dictionary::new();
    font.set("Type", Object::Name(b"Font".to_vec()));
    font.set("Subtype", Object::Name(b"Type1".to_vec()));
    font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
    let font_id = doc.add_object(font);

    let mut stamped = 0u32;
    for (number, page_id) in targets {
        let (w, h) = page_size(&doc, page_id);
        let text = stamp_text(pattern, number, total);
        let (x, y) = stamp_xy(corner, w, h, text.chars().count(), size);

        // The font has to be reachable from this page's own resources, or the
        // reader has no idea what /TPX means.
        if let Ok(resources) = doc.get_or_create_resources(page_id).and_then(|r| r.as_dict_mut()) {
            if resources.get(b"Font").and_then(|f| f.as_dict()).is_err() {
                resources.set("Font", Dictionary::new());
            }
            if let Ok(fonts) = resources.get_mut(b"Font").and_then(|f| f.as_dict_mut()) {
                fonts.set("TPX", Object::Reference(font_id));
            }
        }

        let Ok(mut content) = doc.get_and_decode_page_content(page_id) else { continue };
        content.operations.extend([
            Operation::new("q", vec![]),
            Operation::new("BT", vec![]),
            Operation::new(
                "Tf",
                vec![Object::Name(b"TPX".to_vec()), Object::Real(size as f32)],
            ),
            Operation::new("Td", vec![Object::Real(x as f32), Object::Real(y as f32)]),
            Operation::new("Tj", vec![Object::string_literal(text)]),
            Operation::new("ET", vec![]),
            Operation::new("Q", vec![]),
        ]);
        let Ok(encoded) = Content::encode(&content) else { continue };
        if doc.change_page_content(page_id, encoded).is_ok() {
            stamped += 1;
        }
    }
    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(stamped)
}

/// A page's width and height in points.
///
/// `/MediaBox` is inheritable, so a document that sets it once on the page
/// tree root has pages with no MediaBox of their own; this walks up `/Parent`
/// for it. US Letter is the fallback, because a stamp in roughly the right
/// place beats no stamp.
fn page_size(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> (f64, f64) {
    let mut id = page_id;
    for _ in 0..8 {
        let Ok(dict) = doc.get_object(id).and_then(|o| o.as_dict()) else { break };
        if let Ok(media) = dict.get(b"MediaBox").and_then(|o| o.as_array()) {
            let n: Vec<f64> = media.iter().filter_map(|v| as_number(v)).collect();
            if n.len() == 4 {
                return ((n[2] - n[0]).abs().max(1.0), (n[3] - n[1]).abs().max(1.0));
            }
        }
        match dict.get(b"Parent").and_then(|o| o.as_reference()) {
            Ok(parent) => id = parent,
            Err(_) => break,
        }
    }
    (612.0, 792.0)
}

fn as_number(v: &lopdf::Object) -> Option<f64> {
    v.as_f32().map(|f| f as f64).ok().or_else(|| v.as_i64().ok().map(|i| i as f64))
}

/// A `/Rotate` value PDF readers accept: a multiple of 90 in 0..360.
pub fn normalise_turn(degrees: i64) -> i64 {
    let step = (degrees as f64 / 90.0).round() as i64;
    step.rem_euclid(4) * 90
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        assert_eq!(parse_ranges("1-3,5,8-10").unwrap(), vec![1, 2, 3, 5, 8, 9, 10]);
        assert_eq!(parse_ranges("3,1,2").unwrap(), vec![1, 2, 3]); // sorted + deduped
        assert!(parse_ranges("5-1").is_none());  // reversed
        assert!(parse_ranges("0").is_none());    // zero page
        assert!(parse_ranges("abc").is_none());
    }

    #[test]
    fn clamp_to_doc() {
        assert_eq!(clamp_pages(&[1, 5, 99], 10), vec![1, 5]);
    }

    #[test]
    fn a_blank_range_means_every_page() {
        assert_eq!(selected("", 3), vec![1, 2, 3]);
        assert_eq!(selected("  ", 2), vec![1, 2]);
        assert_eq!(selected("2-4", 3), vec![2, 3]);
        // Unparseable is not "everything" — that would delete a document.
        assert!(selected("abc", 9).is_empty());
    }

    #[test]
    fn a_stamp_pattern_knows_where_it_is() {
        assert_eq!(stamp_text("Page {n} of {total}", 3, 12), "Page 3 of 12");
        // A pattern with no tokens is a stamp rather than a number.
        assert_eq!(stamp_text("DRAFT", 3, 12), "DRAFT");
    }

    #[test]
    fn stamps_sit_inside_the_page() {
        let (w, h) = (612.0, 792.0);
        let (x, y) = stamp_xy(Corner::BottomRight, w, h, 4, 10.0);
        assert!(x > 0.0 && x < w, "{x}");
        assert!(y > 0.0 && y < h / 2.0, "{y}");
        let (tx, ty) = stamp_xy(Corner::TopRight, w, h, 4, 10.0);
        assert!(ty > h / 2.0, "top means top");
        assert!((tx - x).abs() < 0.01, "the two right corners share a column");
        // A pathologically long stamp is clamped onto the page, not off it.
        let (nx, _) = stamp_xy(Corner::BottomRight, w, h, 500, 20.0);
        assert_eq!(nx, 0.0);
        assert_eq!(Corner::parse("Top Right"), Corner::TopRight);
        assert_eq!(Corner::parse("nonsense"), Corner::BottomCentre);
    }

    #[test]
    fn turns_stay_on_the_ninety_degree_grid() {
        assert_eq!(normalise_turn(90), 90);
        assert_eq!(normalise_turn(450), 90);
        assert_eq!(normalise_turn(-90), 270);
        assert_eq!(normalise_turn(360), 0);
        // Junk from a malformed /Rotate snaps to the nearest quarter.
        assert_eq!(normalise_turn(100), 90);
    }
}
