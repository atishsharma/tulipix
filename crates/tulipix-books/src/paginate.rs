//! `np.p4.books.reader` text pagination — split a chapter's plain text into
//! discrete screen-sized pages so the EPUB reader shows real, turnable pages
//! instead of one long scroll.
//!
//! Slint does not expose glyph-level text measurement we can paginate against,
//! so we estimate a character budget per page from the viewport geometry and
//! the active typography, then fill pages on word/paragraph boundaries. The
//! estimate is intentionally conservative (a fill factor < 1) so a page never
//! overflows its frame; the cost is occasional slightly-short pages, which read
//! fine. Re-run [`paginate`] whenever the viewport, font size, line height, or
//! family changes ([`reflow_anchor`] keeps the reader near the same spot).

use crate::typography::FontFamily;

/// Average glyph advance as a fraction of the font pixel size, per family.
/// Serif is tightest; OpenDyslexic is widest by design.
fn char_ratio(family: FontFamily) -> f32 {
    match family {
        FontFamily::Serif => 0.50,
        FontFamily::SansSerif => 0.52,
        FontFamily::OpenDyslexic => 0.58,
    }
}

/// Characters that fit on one page for the given stage geometry + typography.
/// `page_w`/`page_h` are the *text column* pixels (already inset by margins).
pub fn chars_per_page(
    page_w: f32,
    page_h: f32,
    font_px: f32,
    line_height: f32,
    family: FontFamily,
) -> usize {
    if font_px < 1.0 || page_w < 8.0 || page_h < 8.0 {
        return 900; // sane default before the stage has a real size
    }
    let char_w = (font_px * char_ratio(family)).max(1.0);
    let cols = (page_w / char_w).max(1.0);
    let line_px = (font_px * line_height.max(1.0)).max(1.0);
    let rows = (page_h / line_px).max(1.0);
    // Word-wrap ragged edges + paragraph breaks waste ~12% of the grid.
    ((cols * rows) * 0.88) as usize
}

/// Split `text` into pages of at most `cap` characters, breaking on word and
/// paragraph boundaries. Paragraph breaks (`\n`) are preserved inside a page;
/// a blank paragraph never starts a page. Always returns at least one page.
pub fn paginate(text: &str, cap: usize) -> Vec<String> {
    let cap = cap.max(8); // floor only guards against a zero/degenerate cap
    let mut pages: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut cur_len = 0usize; // char count of `cur`

    let flush = |pages: &mut Vec<String>, cur: &mut String, cur_len: &mut usize| {
        let t = cur.trim_end_matches('\n');
        if !t.is_empty() {
            pages.push(t.to_string());
        }
        cur.clear();
        *cur_len = 0;
    };

    for para in text.split('\n') {
        let para = para.trim_end();
        if para.is_empty() {
            // Paragraph gap: keep a single blank line inside a page, but never
            // let a page begin with blank space.
            if cur_len > 0 {
                cur.push('\n');
            }
            continue;
        }
        let para_len = para.chars().count();
        // Whole paragraph fits in the remaining room → drop it in.
        if cur_len + para_len < cap {
            if cur_len > 0 {
                cur.push('\n');
                cur_len += 1;
            }
            cur.push_str(para);
            cur_len += para_len;
            continue;
        }
        // Paragraph too big for the remaining room — pour it word by word.
        if cur_len > 0 {
            cur.push('\n');
            cur_len += 1;
        }
        for word in para.split(' ') {
            let wlen = word.chars().count();
            let sep = if cur_len > 0 && !cur.ends_with('\n') { 1 } else { 0 };
            if cur_len + sep + wlen > cap && cur_len > 0 {
                flush(&mut pages, &mut cur, &mut cur_len);
            }
            if cur_len > 0 && !cur.ends_with('\n') {
                cur.push(' ');
                cur_len += 1;
            }
            // A single word longer than a whole page: hard-split it.
            if wlen > cap {
                for ch in word.chars() {
                    if cur_len >= cap {
                        flush(&mut pages, &mut cur, &mut cur_len);
                    }
                    cur.push(ch);
                    cur_len += 1;
                }
            } else {
                cur.push_str(word);
                cur_len += wlen;
            }
        }
    }
    flush(&mut pages, &mut cur, &mut cur_len);
    if pages.is_empty() {
        pages.push(String::new());
    }
    pages
}

/// After a reflow changes the page count, map an old page index to the nearest
/// equivalent in the new pagination, preserving read position by fraction.
pub fn reflow_anchor(old_page: usize, old_total: usize, new_total: usize) -> usize {
    if old_total <= 1 || new_total == 0 {
        return 0;
    }
    let frac = old_page as f64 / (old_total - 1) as f64;
    ((frac * (new_total - 1) as f64).round() as usize).min(new_total - 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pages_respect_cap() {
        let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
        let pages = paginate(text, 20);
        assert!(pages.len() > 1);
        for p in &pages {
            assert!(p.chars().count() <= 20, "page over cap: {p:?}");
        }
    }

    #[test]
    fn preserves_words_no_loss() {
        let text = "one two three four five six seven eight nine ten";
        let joined = paginate(text, 12).join(" ").replace('\n', " ");
        let norm: Vec<&str> = joined.split_whitespace().collect();
        assert_eq!(norm, text.split(' ').collect::<Vec<_>>());
    }

    #[test]
    fn blank_never_starts_page() {
        let text = "para one\n\n\npara two that is a fair bit longer than the cap here";
        for p in paginate(text, 15) {
            assert!(!p.starts_with('\n'));
            assert!(!p.trim().is_empty());
        }
    }

    #[test]
    fn long_word_hard_splits() {
        let text = "supercalifragilisticexpialidocious";
        let pages = paginate(text, 10);
        assert!(pages.len() >= 3);
        assert_eq!(pages.concat(), text);
    }

    #[test]
    fn always_one_page() {
        assert_eq!(paginate("", 100).len(), 1);
    }

    #[test]
    fn anchor_keeps_fraction() {
        assert_eq!(reflow_anchor(0, 10, 20), 0);
        assert_eq!(reflow_anchor(9, 10, 20), 19);
        assert_eq!(reflow_anchor(5, 11, 21), 10);
    }

    #[test]
    fn chars_per_page_scales_with_size() {
        let big = chars_per_page(800.0, 1000.0, 16.0, 1.5, FontFamily::Serif);
        let small = chars_per_page(800.0, 1000.0, 32.0, 1.5, FontFamily::Serif);
        assert!(big > small);
        assert!(small > 0);
    }
}
