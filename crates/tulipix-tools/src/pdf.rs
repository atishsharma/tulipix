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
mod document_tests {
    use super::*;

    #[test]
    fn a_booklet_puts_the_last_page_beside_the_first() {
        // Eight pages, two per sheet: the outer sheet carries 8 and 1 on one
        // side and 2 and 7 on the other. Fold the stack and it reads 1..8.
        assert_eq!(impose_order(8, Impose::Booklet), vec![8, 1, 2, 7, 6, 3, 4, 5]);
    }

    #[test]
    fn a_booklet_pads_up_to_a_whole_sheet_with_blanks() {
        // Six pages need eight slots; the two that do not exist are 0.
        let order = impose_order(6, Impose::Booklet);
        assert_eq!(order.len(), 8);
        assert_eq!(order.iter().filter(|n| **n == 0).count(), 2);
        // And every real page is placed exactly once.
        for n in 1..=6u32 {
            assert_eq!(order.iter().filter(|x| **x == n).count(), 1, "page {n}");
        }
    }

    #[test]
    fn n_up_stays_in_reading_order() {
        assert_eq!(impose_order(4, Impose::TwoUp), vec![1, 2, 3, 4]);
        assert_eq!(impose_order(5, Impose::FourUp), vec![1, 2, 3, 4, 5, 0, 0, 0]);
    }

    #[test]
    fn splitting_every_n_pages_covers_the_document_once() {
        assert_eq!(split_groups(10, &SplitAt::Every(4)), vec![(1, 4), (5, 8), (9, 10)]);
        assert_eq!(split_groups(4, &SplitAt::Every(4)), vec![(1, 4)]);
        assert_eq!(split_groups(0, &SplitAt::Every(4)), vec![]);
        // A zero would divide by zero; it means "one page each".
        assert_eq!(split_groups(2, &SplitAt::Every(0)), vec![(1, 1), (2, 2)]);
    }

    #[test]
    fn splitting_at_named_pages_starts_a_part_at_each_one() {
        assert_eq!(
            split_groups(10, &SplitAt::Pages(vec![4, 8])),
            vec![(1, 3), (4, 7), (8, 10)]
        );
        // Page 1 and anything past the end are not cuts.
        assert_eq!(split_groups(5, &SplitAt::Pages(vec![1, 99])), vec![(1, 5)]);
    }

    #[test]
    fn redaction_replaces_the_word_and_keeps_the_length() {
        let mut body = b"Contact ADA Lovelace at ada@example.com".to_vec();
        let hits = scrub(&mut body, &[b"ada".to_vec()]);
        // Twice: the name and the address, case-insensitively.
        assert_eq!(hits, 2);
        assert_eq!(body.len(), 39);
        assert!(!body.to_ascii_lowercase().windows(3).any(|w| w == b"ada"));
        assert!(body.starts_with(b"Contact     Lovelace"));
    }

    #[test]
    fn a_jpeg_gives_up_its_size_from_the_frame_header() {
        // SOI, an APP0 segment long enough to be skipped, then SOF0 640x480.
        let mut body = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x04, 0x00, 0x00];
        body.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x11, 0x08, 0x01, 0xE0, 0x02, 0x80]);
        assert_eq!(jpeg_size(&body), Some((640, 480)));
        assert_eq!(jpeg_size(b"not a jpeg"), None);
    }

    #[test]
    fn a_flattened_value_lands_inside_its_box() {
        let mut widget = lopdf::Dictionary::new();
        widget.set(
            "Rect",
            lopdf::Object::Array(vec![
                lopdf::Object::Real(100.0),
                lopdf::Object::Real(700.0),
                lopdf::Object::Real(300.0),
                lopdf::Object::Real(720.0),
            ]),
        );
        let (x, y) = rect_origin(&widget, 10.0).unwrap();
        assert!(x > 100.0 && x < 300.0);
        assert!(y > 700.0 && y < 720.0);
        // A widget with no rectangle is skipped rather than drawn at 0,0.
        assert_eq!(rect_origin(&lopdf::Dictionary::new(), 10.0), None);
    }
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


// ============================================================= documents ===
//
// Whole-file operations: putting PDFs together, taking one apart, and reading
// what is inside. Everything below matches on `Object`'s variants rather than
// calling `as_name`/`as_str` helpers — the variants have been stable across
// every lopdf release, and the helpers' return types have not.

/// The `/Type` of an object, or empty. Never fails: a dictionary with no
/// `/Type` is common and legal.
fn type_of(obj: &lopdf::Object) -> Vec<u8> {
    let Ok(dict) = obj.as_dict() else { return Vec::new() };
    match dict.get(b"Type") {
        Ok(lopdf::Object::Name(n)) => n.clone(),
        _ => Vec::new(),
    }
}

/// Put several PDFs together, in the order given.
///
/// The object numbers of two documents collide by definition — both start at
/// 1 — so each is renumbered above the last before anything is copied. Then
/// one catalogue and one page tree survive and the rest are dropped: a merged
/// file with two `/Catalog` objects opens in nothing.
pub fn merge(inputs: &[String], out: &str) -> Result<u32> {
    use lopdf::{Document, Object, ObjectId};
    use std::collections::BTreeMap;

    if inputs.len() < 2 {
        bail!("merging needs at least two files");
    }

    let mut max_id = 1u32;
    let mut pages: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();

    for path in inputs {
        let mut doc = Document::load(path)
            .with_context(|| format!("{path} could not be opened as a PDF"))?;
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        for id in doc.get_pages().into_values() {
            if let Ok(page) = doc.get_object(id) {
                pages.insert(id, page.clone());
            }
        }
        objects.extend(doc.objects.clone());
    }
    if pages.is_empty() {
        bail!("none of those files had any pages");
    }

    let mut document = Document::with_version("1.5");
    let mut catalog: Option<(ObjectId, Object)> = None;
    let mut tree: Option<(ObjectId, Object)> = None;

    for (id, object) in objects.iter() {
        match type_of(object).as_slice() {
            b"Catalog" => {
                // The first document's catalogue wins, minus anything that
                // pointed into a page tree that is about to be rebuilt.
                if catalog.is_none() {
                    catalog = Some((*id, object.clone()));
                }
            }
            // Rebuilt below from the pages that survived, so the originals go.
            b"Pages" | b"Page" | b"Outlines" | b"Outline" => {
                if type_of(object).as_slice() == b"Pages" && tree.is_none() {
                    tree = Some((*id, object.clone()));
                }
            }
            _ => {
                document.objects.insert(*id, object.clone());
            }
        }
    }

    let (catalog_id, catalog_obj) = catalog.ok_or_else(|| anyhow::anyhow!("no PDF catalogue found"))?;
    let (tree_id, tree_obj) = tree.ok_or_else(|| anyhow::anyhow!("no page tree found"))?;

    // Every page now hangs off the one surviving tree.
    let count = pages.len() as u32;
    for (id, object) in pages.iter() {
        if let Ok(dict) = object.as_dict() {
            let mut dict = dict.clone();
            dict.set("Parent", Object::Reference(tree_id));
            document.objects.insert(*id, Object::Dictionary(dict));
        }
    }

    let mut tree_dict = tree_obj.as_dict().cloned().unwrap_or_default();
    tree_dict.set("Count", Object::Integer(count as i64));
    tree_dict.set(
        "Kids",
        Object::Array(pages.keys().map(|id| Object::Reference(*id)).collect()),
    );
    document.objects.insert(tree_id, Object::Dictionary(tree_dict));

    let mut catalog_dict = catalog_obj.as_dict().cloned().unwrap_or_default();
    catalog_dict.set("Pages", Object::Reference(tree_id));
    // An outline tree that pointed at pages from the second file would now
    // point at nothing, and a dangling /Outlines is worse than none.
    catalog_dict.remove(b"Outlines");
    document.objects.insert(catalog_id, Object::Dictionary(catalog_dict));

    document.trailer.set("Root", Object::Reference(catalog_id));
    document.max_id = max_id;
    document.renumber_objects();
    document.compress();
    document.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(count)
}

/// Where a split falls: every N pages, or at the page numbers given.
#[derive(Debug, Clone, PartialEq)]
pub enum SplitAt {
    Every(u32),
    Pages(Vec<u32>),
}

/// The page groups a split would produce, as 1-based inclusive ranges.
///
/// Pure arithmetic, so the preview and the run cannot disagree about which
/// pages land in part 3.
pub fn split_groups(total: u32, at: &SplitAt) -> Vec<(u32, u32)> {
    if total == 0 {
        return Vec::new();
    }
    match at {
        SplitAt::Every(n) => {
            let n = (*n).max(1);
            (0..total.div_ceil(n))
                .map(|i| (i * n + 1, ((i + 1) * n).min(total)))
                .collect()
        }
        SplitAt::Pages(cuts) => {
            // A cut "at page 5" means 5 opens the next part.
            let mut bounds: Vec<u32> = cuts.iter().copied().filter(|p| *p > 1 && *p <= total).collect();
            bounds.sort_unstable();
            bounds.dedup();
            let mut out = Vec::new();
            let mut start = 1u32;
            for cut in bounds {
                out.push((start, cut - 1));
                start = cut;
            }
            out.push((start, total));
            out
        }
    }
}

/// Split `input` into one file per group. `template` gets the part number
/// substituted for `{n}`; a template without one has it appended, because
/// twelve parts written to one name is eleven files lost.
pub fn split(input: &str, at: &SplitAt, template: &str, out_dir: &str) -> Result<Vec<String>> {
    let total = page_count(input).unwrap_or(0);
    let groups = split_groups(total, at);
    if groups.len() < 2 {
        bail!("that leaves the document in one piece");
    }
    let stem = std::path::Path::new(input)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "part".into());
    let template = if template.contains("{n}") {
        template.to_string()
    } else {
        format!("{template}-{{n}}")
    };

    let mut written = Vec::new();
    for (i, (from, to)) in groups.iter().enumerate() {
        let name = template
            .replace("{n}", &format!("{:02}", i + 1))
            .replace("{name}", &stem);
        let path = format!("{}/{name}.pdf", out_dir.trim_end_matches('/'));
        let keep: Vec<u32> = (*from..=*to).collect();
        keep_pages(input, &keep, &path)?;
        written.push(path);
    }
    Ok(written)
}

/// The text of a document, page by page, separated by form feeds.
///
/// A PDF that was scanned rather than typeset has no text in it at all, and
/// this returns an empty string rather than pretending otherwise — which is
/// the signal that the answer is OCR, not this.
pub fn extract_text(input: &str, pages: &[u32]) -> Result<String> {
    let doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let numbers: Vec<u32> = if pages.is_empty() {
        doc.get_pages().keys().copied().collect()
    } else {
        pages.to_vec()
    };
    let mut out = String::new();
    for n in numbers {
        if let Ok(text) = doc.extract_text(&[n]) {
            out.push_str(text.trim());
        }
        out.push_str("\n\u{0c}\n");
    }
    Ok(out)
}

/// One picture found inside a PDF.
pub struct Embedded {
    pub page: u32,
    pub name: String,
    pub bytes: Vec<u8>,
    pub ext: &'static str,
}

/// Pull the embedded images out.
///
/// Only the ones already stored in a format that is a file on its own — JPEG
/// (`/DCTDecode`) and JPEG 2000 (`/JPXDecode`) — come out byte for byte. A
/// `/FlateDecode` image is raw samples plus a colour space, and turning that
/// back into a PNG is an encoder this does not have; those are counted and
/// reported so the number in the preview is the truth rather than a silence.
pub fn extract_images(input: &str, out_dir: &str) -> Result<(Vec<String>, u32)> {
    let doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let dir = out_dir.trim_end_matches('/');
    std::fs::create_dir_all(dir).ok();

    let mut written: Vec<String> = Vec::new();
    let mut skipped = 0u32;
    for (number, page_id) in doc.get_pages() {
        for found in page_images(&doc, page_id, number) {
            match found {
                Some(image) => {
                    let path = format!("{dir}/{}", image.name);
                    if std::fs::write(&path, &image.bytes).is_ok() {
                        written.push(path);
                    }
                }
                None => skipped += 1,
            }
        }
    }
    Ok((written, skipped))
}

/// `None` for an image whose encoding this cannot hand over as a file.
fn page_images(doc: &lopdf::Document, page_id: lopdf::ObjectId, number: u32) -> Vec<Option<Embedded>> {
    use lopdf::Object;

    let mut out = Vec::new();
    let Some(Object::Dictionary(dict)) = page_attr(doc, page_id, b"Resources") else { return out };
    let Ok(Object::Dictionary(xobjects)) = dict.get(b"XObject").map(resolve_shallow(doc)) else {
        return out;
    };

    for (key, value) in xobjects.iter() {
        let id = match value {
            Object::Reference(id) => *id,
            _ => continue,
        };
        let Ok(Object::Stream(stream)) = doc.get_object(id) else { continue };
        if !matches!(stream.dict.get(b"Subtype"), Ok(Object::Name(n)) if n == b"Image") {
            continue;
        }
        let ext = match stream.dict.get(b"Filter") {
            Ok(Object::Name(n)) if n == b"DCTDecode" => "jpg",
            Ok(Object::Name(n)) if n == b"JPXDecode" => "jp2",
            _ => {
                out.push(None);
                continue;
            }
        };
        let name = format!(
            "page-{number:03}-{}.{ext}",
            String::from_utf8_lossy(key).replace(|c: char| !c.is_ascii_alphanumeric(), "_")
        );
        out.push(Some(Embedded {
            page: number,
            name,
            bytes: stream.content.clone(),
            ext,
        }));
    }
    out
}

/// Follow one level of indirection, which is all `/XObject` ever needs.
fn resolve_shallow(doc: &lopdf::Document) -> impl Fn(&lopdf::Object) -> lopdf::Object + '_ {
    move |obj| match obj {
        lopdf::Object::Reference(id) => doc.get_object(*id).cloned().unwrap_or(lopdf::Object::Null),
        other => other.clone(),
    }
}

/// A page attribute, following `/Parent` for the inheritable ones.
///
/// `/Resources`, `/MediaBox`, `/CropBox` and `/Rotate` may all be set once on
/// the page tree root, and a document that does so has pages carrying none of
/// them. References are resolved, so the caller gets the value rather than a
/// pointer to it.
fn page_attr(doc: &lopdf::Document, page_id: lopdf::ObjectId, key: &[u8]) -> Option<lopdf::Object> {
    let mut id = page_id;
    for _ in 0..8 {
        let dict = doc.get_object(id).ok()?.as_dict().ok()?;
        if let Ok(value) = dict.get(key) {
            return Some(resolve_shallow(doc)(value));
        }
        match dict.get(b"Parent") {
            Ok(lopdf::Object::Reference(parent)) => id = *parent,
            _ => break,
        }
    }
    None
}

// ================================================================ layout ===

/// How pages are laid onto sheets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Impose {
    /// Saddle-stitch: print double-sided, fold the stack once, staple the
    /// spine. The page order is the whole trick — page 1 sits beside the last
    /// page on the same sheet.
    Booklet,
    /// Two pages per landscape sheet, in reading order.
    TwoUp,
    /// Four pages per sheet, 2x2, in reading order.
    FourUp,
}

impl Impose {
    pub fn parse(s: &str) -> Impose {
        match s {
            "2up" => Impose::TwoUp,
            "4up" => Impose::FourUp,
            _ => Impose::Booklet,
        }
    }

    /// Columns and rows per sheet.
    pub fn grid(self) -> (u32, u32) {
        match self {
            Impose::Booklet | Impose::TwoUp => (2, 1),
            Impose::FourUp => (2, 2),
        }
    }
}

/// The source page for every slot on every sheet, in order. `0` is a blank.
///
/// Booklet padding rounds up to a multiple of four, because a folded sheet is
/// four pages whether or not they are printed on.
pub fn impose_order(total: u32, how: Impose) -> Vec<u32> {
    let (cols, rows) = how.grid();
    let per_sheet = cols * rows;
    match how {
        Impose::TwoUp | Impose::FourUp => {
            let padded = total.div_ceil(per_sheet) * per_sheet;
            (1..=padded).map(|n| if n <= total { n } else { 0 }).collect()
        }
        Impose::Booklet => {
            let padded = total.div_ceil(4) * 4;
            let mut out = Vec::with_capacity(padded as usize);
            let (mut left, mut right) = (1u32, padded);
            // Front of sheet: last, first. Back of sheet: second, second-last.
            // Then inwards. Every reader who has folded a zine knows this
            // sequence; nobody can derive it at the printer.
            while left < right {
                out.push(right);
                out.push(left);
                left += 1;
                right -= 1;
                if left < right {
                    out.push(left);
                    out.push(right);
                    left += 1;
                    right -= 1;
                }
            }
            out.into_iter().map(|n| if n <= total { n } else { 0 }).collect()
        }
    }
}

/// Lay the pages of `input` onto larger sheets.
///
/// Each source page becomes a Form XObject — its own content stream and its
/// own resources, in a box — which is then drawn onto the sheet with a scale
/// and an offset. That keeps the text as text: a version that rendered pages
/// to images would print a booklet nobody could search or select from.
pub fn impose(input: &str, how: Impose, out: &str) -> Result<u32> {
    use lopdf::content::{Content, Operation};
    use lopdf::{Dictionary, Object, Stream};

    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let pages: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
    let total = pages.len() as u32;
    if total < 2 {
        bail!("there is nothing to impose in a one-page document");
    }

    // The sheet takes its size from the first page, which is what every
    // document that is not a scrapbook uses throughout.
    let (pw, ph) = page_size(&doc, pages[0].1);
    let (cols, rows) = how.grid();
    let (sheet_w, sheet_h) = (pw * cols as f64, ph * rows as f64);

    // Turn every source page into a form, before any of them are placed.
    let mut forms: std::collections::BTreeMap<u32, lopdf::ObjectId> = Default::default();
    for (number, page_id) in &pages {
        // Infallible in lopdf 0.44: an undecodable stream comes back empty,
        // which is a blank form and still holds its slot on the sheet.
        let content = doc.get_page_content(*page_id);
        let resources = page_attr(&doc, *page_id, b"Resources")
            .unwrap_or(Object::Dictionary(Dictionary::new()));
        let (w, h) = page_size(&doc, *page_id);
        let mut dict = Dictionary::new();
        dict.set("Type", Object::Name(b"XObject".to_vec()));
        dict.set("Subtype", Object::Name(b"Form".to_vec()));
        dict.set(
            "BBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(w as f32),
                Object::Real(h as f32),
            ]),
        );
        dict.set("Resources", resources);
        forms.insert(*number, doc.add_object(Stream::new(dict, content)));
    }

    let order = impose_order(total, how);
    let per_sheet = (cols * rows) as usize;
    let mut sheet_ids: Vec<Object> = Vec::new();
    let pages_id = doc.new_object_id();

    for slots in order.chunks(per_sheet) {
        let mut xobjects = Dictionary::new();
        let mut ops: Vec<Operation> = Vec::new();
        for (slot, source) in slots.iter().enumerate() {
            if *source == 0 {
                continue;
            }
            let Some(form_id) = forms.get(source) else { continue };
            let (sw, sh) = (pw, ph);
            let col = slot as u32 % cols;
            // Row 0 is the top of the sheet, and PDF's origin is the bottom.
            let row = rows - 1 - (slot as u32 / cols);
            let (tx, ty) = (col as f64 * sw, row as f64 * sh);
            let name = format!("P{slot}");
            xobjects.set(name.clone(), Object::Reference(*form_id));
            ops.extend([
                Operation::new("q", vec![]),
                // No scaling: the slot is exactly one source page wide, so the
                // form drops in at 1:1 and only moves.
                Operation::new(
                    "cm",
                    vec![
                        Object::Real(1.0),
                        Object::Real(0.0),
                        Object::Real(0.0),
                        Object::Real(1.0),
                        Object::Real(tx as f32),
                        Object::Real(ty as f32),
                    ],
                ),
                Operation::new("Do", vec![Object::Name(name.into_bytes())]),
                Operation::new("Q", vec![]),
            ]);
        }

        let content = Content { operations: ops };
        let stream_id = doc.add_object(Stream::new(Dictionary::new(), Content::encode(&content)?));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let mut sheet = Dictionary::new();
        sheet.set("Type", Object::Name(b"Page".to_vec()));
        sheet.set("Parent", Object::Reference(pages_id));
        sheet.set("Contents", Object::Reference(stream_id));
        sheet.set("Resources", Object::Dictionary(resources));
        sheet.set(
            "MediaBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(sheet_w as f32),
                Object::Real(sheet_h as f32),
            ]),
        );
        sheet_ids.push(Object::Reference(doc.add_object(sheet)));
    }

    let count = sheet_ids.len() as u32;
    let mut tree = Dictionary::new();
    tree.set("Type", Object::Name(b"Pages".to_vec()));
    tree.set("Count", Object::Integer(count as i64));
    tree.set("Kids", Object::Array(sheet_ids));
    doc.objects.insert(pages_id, Object::Dictionary(tree));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(count)
}

// =============================================================== redact ===

/// Take words out of a PDF's text, everywhere they appear.
///
/// This removes the characters rather than covering them. A black rectangle
/// drawn over a word is the classic redaction failure — the text is still in
/// the file and any reader will copy it straight back out — so nothing here
/// draws one. What comes out is a document whose content stream no longer
/// contains the word at all.
///
/// ponytail: matches the bytes in the content stream, so it finds text in the
/// standard encodings and misses a subset CID font that maps its own glyph
/// codes. That is why the preview reports the hit count per page: nought found
/// is visible, where a silent miss would not be. Font-aware matching is the
/// upgrade, and it needs the font's /ToUnicode map.
pub fn redact(input: &str, words: &[String], out: &str) -> Result<Vec<(u32, u32)>> {
    use lopdf::Object;
    use lopdf::content::Content;

    let needles: Vec<Vec<u8>> = words
        .iter()
        .map(|w| w.trim().to_lowercase().into_bytes())
        .filter(|w| !w.is_empty())
        .collect();
    if needles.is_empty() {
        bail!("nothing to redact — name at least one word");
    }

    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let mut report: Vec<(u32, u32)> = Vec::new();

    for (number, page_id) in doc.get_pages() {
        let Ok(mut content) = doc.get_and_decode_page_content(page_id) else { continue };
        let mut hits = 0u32;
        for op in content.operations.iter_mut() {
            match op.operator.as_str() {
                "Tj" | "'" | "\"" => {
                    for operand in op.operands.iter_mut() {
                        if let Object::String(bytes, _) = operand {
                            hits += scrub(bytes, &needles);
                        }
                    }
                }
                "TJ" => {
                    for operand in op.operands.iter_mut() {
                        if let Object::Array(items) = operand {
                            for item in items.iter_mut() {
                                if let Object::String(bytes, _) = item {
                                    hits += scrub(bytes, &needles);
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        report.push((number, hits));
        if hits > 0 {
            if let Ok(encoded) = Content::encode(&content) {
                doc.change_page_content(page_id, encoded).ok();
            }
        }
    }

    // Metadata carries the title and often the author of the thing being
    // redacted, and no amount of scrubbing the pages touches it.
    doc.trailer.remove(b"Info");
    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(report)
}

/// Replace every occurrence, case-insensitively, with spaces.
///
/// Same length on purpose: the surrounding text has positioning baked into the
/// stream, and a shorter string would shift the rest of the line.
fn scrub(haystack: &mut [u8], needles: &[Vec<u8>]) -> u32 {
    let lower: Vec<u8> = haystack.to_ascii_lowercase();
    let mut hits = 0u32;
    for needle in needles {
        let mut from = 0usize;
        while from + needle.len() <= lower.len() {
            match lower[from..].windows(needle.len()).position(|w| w == needle.as_slice()) {
                Some(at) => {
                    let start = from + at;
                    for byte in haystack[start..start + needle.len()].iter_mut() {
                        *byte = b' ';
                    }
                    hits += 1;
                    from = start + needle.len();
                }
                None => break,
            }
        }
    }
    hits
}

// ================================================================ forms ===

/// One fillable field, as it appears on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct Field {
    pub page: u32,
    pub name: String,
    /// `Tx` text, `Btn` button or checkbox, `Ch` choice, `Sig` signature.
    pub kind: String,
    pub value: String,
}

/// Every widget on every page, in page order.
///
/// Read off the pages' `/Annots` rather than walked down the `/AcroForm` tree:
/// a widget has to be on a page to be filled in, the page number is the thing
/// the preview wants to show, and the two lists agree in every document that
/// is not already broken.
pub fn form_fields(input: &str) -> Result<Vec<Field>> {
    let doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let mut out = Vec::new();
    for (number, page_id) in doc.get_pages() {
        for (_, widget) in widgets(&doc, page_id) {
            out.push(Field {
                page: number,
                name: field_name(&doc, &widget),
                kind: name_entry(&doc, &widget, b"FT"),
                value: string_entry(&doc, &widget, b"V"),
            });
        }
    }
    Ok(out)
}

/// Fill a form in, and optionally print the answers onto the page.
///
/// Filling alone sets `/V` and asks the reader to build the appearance, which
/// every reader does and no two do identically. Flattening draws the value
/// with the same Helvetica the page stamp uses and then removes the
/// annotations altogether — after which the document is not a form any more,
/// which is the point of the word.
pub fn fill_form(
    input: &str,
    values: &[(String, String)],
    flatten: bool,
    size: f64,
    out: &str,
) -> Result<u32> {
    use lopdf::content::{Content, Operation};
    use lopdf::{Dictionary, Object};

    let mut doc = lopdf::Document::load(input)
        .with_context(|| format!("{input} could not be opened as a PDF"))?;
    let size = size.clamp(4.0, 48.0);

    let font_id = {
        let mut font = Dictionary::new();
        font.set("Type", Object::Name(b"Font".to_vec()));
        font.set("Subtype", Object::Name(b"Type1".to_vec()));
        font.set("BaseFont", Object::Name(b"Helvetica".to_vec()));
        doc.add_object(font)
    };

    let mut filled = 0u32;
    let pages: Vec<lopdf::ObjectId> = doc.get_pages().into_values().collect();
    for page_id in pages {
        let found = widgets(&doc, page_id);
        let mut drawn: Vec<(f64, f64, String)> = Vec::new();

        for (annot_id, widget) in &found {
            let name = field_name(&doc, widget);
            let Some((_, value)) = values.iter().find(|(k, _)| *k == name) else { continue };
            filled += 1;

            if let Ok(dict) = doc.get_object_mut(*annot_id).and_then(|o| o.as_dict_mut()) {
                dict.set("V", Object::string_literal(value.clone()));
                // A checkbox takes a name, not a string: /Yes and /Off are the
                // two states every writer produces.
                if name_entry_dict(widget, b"FT") == "Btn" {
                    let on = !matches!(value.to_lowercase().as_str(), "" | "off" | "no" | "false");
                    let state: &[u8] = if on { b"Yes" } else { b"Off" };
                    dict.set("V", Object::Name(state.to_vec()));
                    dict.set("AS", Object::Name(state.to_vec()));
                }
            }
            if flatten {
                if let Some((x, y)) = rect_origin(widget, size) {
                    drawn.push((x, y, value.clone()));
                }
            }
        }

        if !flatten || drawn.is_empty() {
            continue;
        }
        if let Ok(resources) = doc.get_or_create_resources(page_id).and_then(|r| r.as_dict_mut()) {
            if resources.get(b"Font").and_then(|f| f.as_dict()).is_err() {
                resources.set("Font", Dictionary::new());
            }
            if let Ok(fonts) = resources.get_mut(b"Font").and_then(|f| f.as_dict_mut()) {
                fonts.set("TPXF", Object::Reference(font_id));
            }
        }
        let Ok(mut content) = doc.get_and_decode_page_content(page_id) else { continue };
        for (x, y, value) in drawn {
            content.operations.extend([
                Operation::new("q", vec![]),
                Operation::new("BT", vec![]),
                Operation::new(
                    "Tf",
                    vec![Object::Name(b"TPXF".to_vec()), Object::Real(size as f32)],
                ),
                Operation::new("Td", vec![Object::Real(x as f32), Object::Real(y as f32)]),
                Operation::new("Tj", vec![Object::string_literal(value)]),
                Operation::new("ET", vec![]),
                Operation::new("Q", vec![]),
            ]);
        }
        if let Ok(encoded) = Content::encode(&content) {
            doc.change_page_content(page_id, encoded).ok();
        }
        if let Ok(dict) = doc.get_object_mut(page_id).and_then(|o| o.as_dict_mut()) {
            dict.remove(b"Annots");
        }
    }

    if flatten {
        // Without this the reader still believes there is a form, and offers
        // to fill in fields that are no longer there.
        if let Ok(catalog) = doc.catalog_mut() {
            catalog.remove(b"AcroForm");
        }
    }
    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(filled)
}

fn widgets(doc: &lopdf::Document, page_id: lopdf::ObjectId) -> Vec<(lopdf::ObjectId, lopdf::Dictionary)> {
    use lopdf::Object;

    let Some(Object::Array(annots)) = page_attr(doc, page_id, b"Annots") else { return Vec::new() };
    annots
        .iter()
        .filter_map(|entry| {
            let Object::Reference(id) = entry else { return None };
            let dict = doc.get_object(*id).ok()?.as_dict().ok()?.clone();
            match dict.get(b"Subtype") {
                Ok(Object::Name(n)) if n == b"Widget" => Some((*id, dict)),
                _ => None,
            }
        })
        .collect()
}

/// A widget's field name, following `/Parent` for the radio groups that keep
/// it one level up.
fn field_name(doc: &lopdf::Document, widget: &lopdf::Dictionary) -> String {
    let own = string_entry(doc, widget, b"T");
    if !own.is_empty() {
        return own;
    }
    if let Ok(lopdf::Object::Reference(parent)) = widget.get(b"Parent") {
        if let Ok(dict) = doc.get_object(*parent).and_then(|o| o.as_dict()) {
            return string_entry(doc, dict, b"T");
        }
    }
    String::new()
}

fn string_entry(doc: &lopdf::Document, dict: &lopdf::Dictionary, key: &[u8]) -> String {
    match dict.get(key).map(resolve_shallow(doc)) {
        Ok(lopdf::Object::String(bytes, _)) => String::from_utf8_lossy(&bytes).to_string(),
        Ok(lopdf::Object::Name(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
        _ => String::new(),
    }
}

fn name_entry(doc: &lopdf::Document, dict: &lopdf::Dictionary, key: &[u8]) -> String {
    match dict.get(key).map(resolve_shallow(doc)) {
        Ok(lopdf::Object::Name(bytes)) => String::from_utf8_lossy(&bytes).to_string(),
        _ => String::new(),
    }
}

fn name_entry_dict(dict: &lopdf::Dictionary, key: &[u8]) -> String {
    match dict.get(key) {
        Ok(lopdf::Object::Name(bytes)) => String::from_utf8_lossy(bytes).to_string(),
        _ => String::new(),
    }
}

/// Where to start drawing a flattened value: the field's lower-left corner,
/// nudged in and up so the text sits on the line rather than under it.
fn rect_origin(widget: &lopdf::Dictionary, size: f64) -> Option<(f64, f64)> {
    let lopdf::Object::Array(rect) = widget.get(b"Rect").ok()? else { return None };
    let n: Vec<f64> = rect.iter().filter_map(as_number).collect();
    if n.len() != 4 {
        return None;
    }
    let (x0, y0, _, y1) = (n[0].min(n[2]), n[1].min(n[3]), n[0].max(n[2]), n[1].max(n[3]));
    let height = y1 - y0;
    Some((x0 + 2.0, y0 + (height - size).max(0.0) / 2.0 + size * 0.2))
}

// ========================================================== images to pdf ===

/// One JPEG per page, each page the size of its picture.
///
/// JPEG only, and deliberately: `/DCTDecode` means the file's own bytes go
/// into the PDF untouched, with no decoding, no re-encoding and no quality
/// lost. Anything else is converted to JPEG by the step before this one, which
/// is ffmpeg's job rather than this module's.
pub fn from_jpegs(inputs: &[String], out: &str) -> Result<u32> {
    use lopdf::content::{Content, Operation};
    use lopdf::{Dictionary, Document, Object, Stream};

    if inputs.is_empty() {
        bail!("no pictures to put in a PDF");
    }
    let mut doc = Document::with_version("1.5");
    let pages_id = doc.new_object_id();
    let mut kids: Vec<Object> = Vec::new();

    for path in inputs {
        let bytes = std::fs::read(path).with_context(|| format!("cannot read {path}"))?;
        let (w, h) = jpeg_size(&bytes)
            .ok_or_else(|| anyhow::anyhow!("{path} is not a JPEG this can read"))?;

        let mut image = Dictionary::new();
        image.set("Type", Object::Name(b"XObject".to_vec()));
        image.set("Subtype", Object::Name(b"Image".to_vec()));
        image.set("Width", Object::Integer(w as i64));
        image.set("Height", Object::Integer(h as i64));
        image.set("ColorSpace", Object::Name(b"DeviceRGB".to_vec()));
        image.set("BitsPerComponent", Object::Integer(8));
        image.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        let image_id = doc.add_object(Stream::new(image, bytes));

        // One pixel to one point: a 1200x1800 photo becomes a page that size,
        // which prints at the right proportions whatever paper it lands on.
        let content = Content {
            operations: vec![
                Operation::new("q", vec![]),
                Operation::new(
                    "cm",
                    vec![
                        Object::Real(w as f32),
                        Object::Real(0.0),
                        Object::Real(0.0),
                        Object::Real(h as f32),
                        Object::Real(0.0),
                        Object::Real(0.0),
                    ],
                ),
                Operation::new("Do", vec![Object::Name(b"Im0".to_vec())]),
                Operation::new("Q", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(Dictionary::new(), Content::encode(&content)?));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im0", Object::Reference(image_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set("Contents", Object::Reference(content_id));
        page.set("Resources", Object::Dictionary(resources));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(w as f32),
                Object::Real(h as f32),
            ]),
        );
        kids.push(Object::Reference(doc.add_object(page)));
    }

    let count = kids.len() as u32;
    let mut tree = Dictionary::new();
    tree.set("Type", Object::Name(b"Pages".to_vec()));
    tree.set("Count", Object::Integer(count as i64));
    tree.set("Kids", Object::Array(kids));
    doc.objects.insert(pages_id, Object::Dictionary(tree));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", Object::Reference(catalog_id));

    doc.save(out).with_context(|| format!("{out} could not be written"))?;
    Ok(count)
}

/// Width and height from a JPEG's start-of-frame marker.
///
/// Walks the segment chain rather than scanning for the two bytes, because
/// `FFC0` appears inside compressed data often enough to matter.
pub fn jpeg_size(bytes: &[u8]) -> Option<(u32, u32)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2usize;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = bytes[i + 1];
        // Standalone markers carry no length.
        if marker == 0xD8 || marker == 0x01 || (0xD0..=0xD7).contains(&marker) {
            i += 2;
            continue;
        }
        if marker == 0xD9 || marker == 0xDA {
            break;
        }
        let length = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
        // SOF0..SOF15, minus the two that are not frame headers.
        if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
            // The last byte read is `i + 8`, so that one has to be in range —
            // `i + 9` rejected a frame header that ends the file.
            if i + 8 >= bytes.len() {
                return None;
            }
            let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
            let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
            return Some((w, h));
        }
        i += 2 + length.max(2);
    }
    None
}
