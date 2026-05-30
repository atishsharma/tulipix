//! `np.p4.tools.pdf` — PDF tools (merge / split / OCR / extract images /
//! compress).
//!
//! Backed by lopdf + pdfium-render; this owns the page-range parsing (the
//! `"1-3,5,8-10"` syntax everyone gets wrong) and the operation specs the
//! backend executes.

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

#[derive(Debug, Clone, PartialEq)]
pub enum PdfOp {
    Merge { inputs: Vec<String>, out: String },
    Split { input: String, ranges: Vec<u32>, out: String },
    OcrOverlay { input: String, lang: String, out: String },
    ExtractImages { input: String, out_dir: String },
    Compress { input: String, out: String },
}

/// Pages kept when splitting — clamped to the document's `page_count`.
pub fn clamp_pages(pages: &[u32], page_count: u32) -> Vec<u32> {
    pages.iter().copied().filter(|p| *p >= 1 && *p <= page_count).collect()
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
}
