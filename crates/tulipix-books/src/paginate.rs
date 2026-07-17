//! Deterministic text pagination for the two-page-spread reader.
//!
//! Pure functions: chapter texts + page geometry in, pages out. Character
//! metrics are an approximation (Slint gives us no per-glyph measure from
//! Rust), tuned for the serif body face; a `Layout` change (font size, line
//! height, page size) simply re-runs `paginate` and the caller re-locates the
//! reader by char offset.

/// Page geometry + typography knobs (Aa popover).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Layout {
    pub page_w_px: f32,
    pub page_h_px: f32,
    pub font_px: f32,
    /// Line height multiplier (e.g. 1.55).
    pub line_height: f32,
    /// Horizontal padding inside a page, both sides combined.
    pub pad_x_px: f32,
    /// Vertical padding inside a page, both edges combined (incl. folio row).
    pub pad_y_px: f32,
    /// Average glyph width as a fraction of the em (typeface-dependent: serif
    /// ~0.5, mono ~0.6). Drives `chars_per_line`.
    pub glyph_em: f32,
}

impl Default for Layout {
    fn default() -> Self {
        Self {
            page_w_px: 480.0,
            page_h_px: 640.0,
            font_px: 17.0,
            line_height: 1.55,
            pad_x_px: 96.0,
            pad_y_px: 120.0,
            glyph_em: AVG_GLYPH_EM,
        }
    }
}

/// One laid-out page: chapter index, absolute char offset of its first char
/// (over the concatenated book text), and the page's text.
#[derive(Debug, Clone)]
pub struct Page {
    pub chapter: usize,
    pub char_start: usize,
    pub text: String,
}

// ponytail: average-glyph-width heuristic (0.5 em serif); good enough for a
// justified column — upgrade path is real text measurement via Slint when the
// API exists.
const AVG_GLYPH_EM: f32 = 0.5;

/// Lines reserved on a chapter's first page for the centered title + ornamental
/// divider the reader draws there. Chapters without a title just get a slightly
/// shorter opener (harmless whitespace) — the reader can't lose text to a clip.
const HEADING_RESERVE_LINES: usize = 4;

impl Layout {
    pub fn chars_per_line(&self) -> usize {
        let usable = (self.page_w_px - self.pad_x_px).max(50.0);
        ((usable / (self.font_px * self.glyph_em)) as usize).max(10)
    }
    pub fn lines_per_page(&self) -> usize {
        let usable = (self.page_h_px - self.pad_y_px).max(80.0);
        ((usable / (self.font_px * self.line_height)) as usize).max(4)
    }
}

/// Greedy word wrap of one paragraph into lines of ≤ `width` chars.
fn wrap(par: &str, width: usize) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in par.split_whitespace() {
        if cur.is_empty() {
            cur = word.to_string();
        } else if cur.chars().count() + 1 + word.chars().count() <= width {
            cur.push(' ');
            cur.push_str(word);
        } else {
            lines.push(std::mem::take(&mut cur));
            cur = word.to_string();
        }
        // Pathological single "word" longer than the line: hard-split.
        while cur.chars().count() > width {
            let head: String = cur.chars().take(width).collect();
            let tail: String = cur.chars().skip(width).collect();
            lines.push(head);
            cur = tail;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

/// Paginate chapter texts. Every chapter starts on a fresh page (book
/// convention). Absolute char offsets accumulate across chapters so progress
/// survives repagination.
pub fn paginate(chapters: &[String], layout: Layout) -> Vec<Page> {
    let width = layout.chars_per_line();
    let height = layout.lines_per_page();
    let mut pages = Vec::new();
    let mut abs: usize = 0;

    for (ci, text) in chapters.iter().enumerate() {
        let mut lines: Vec<(usize, String)> = Vec::new(); // (abs char start, line)
        let mut off = abs;
        for par in text.split("\n\n") {
            for l in wrap(par, width) {
                let n = l.chars().count() + 1;
                lines.push((off, l));
                off += n;
            }
            if !lines.is_empty() {
                lines.push((off, String::new())); // paragraph gap line
            }
        }
        while lines.last().map(|(_, l)| l.is_empty()).unwrap_or(false) {
            lines.pop();
        }
        if lines.is_empty() {
            lines.push((off, String::new()));
        }
        // The chapter opener (first page) reserves room for the title/divider,
        // so fewer body lines land there; later pages use the full height.
        let mut i = 0;
        let mut first = true;
        while i < lines.len() {
            let h = if first {
                height.saturating_sub(HEADING_RESERVE_LINES).max(1)
            } else {
                height
            };
            let end = (i + h).min(lines.len());
            let chunk = &lines[i..end];
            pages.push(Page {
                chapter: ci,
                char_start: chunk[0].0,
                text: chunk.iter().map(|(_, l)| l.as_str()).collect::<Vec<_>>().join("\n"),
            });
            i = end;
            first = false;
        }
        abs = off + 1;
    }
    if pages.is_empty() {
        pages.push(Page { chapter: 0, char_start: 0, text: String::new() });
    }
    pages
}

/// Page index containing `char_offset` (position restore after repaginate).
pub fn page_at_offset(pages: &[Page], char_offset: usize) -> usize {
    match pages.binary_search_by(|p| p.char_start.cmp(&char_offset)) {
        Ok(i) => i,
        Err(0) => 0,
        Err(i) => i - 1,
    }
}

/// First page of `chapter` (TOC jump).
pub fn page_of_chapter(pages: &[Page], chapter: usize) -> usize {
    pages.iter().position(|p| p.chapter == chapter).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small() -> Layout {
        Layout { page_w_px: 200.0, page_h_px: 200.0, font_px: 10.0, line_height: 1.5, pad_x_px: 20.0, pad_y_px: 20.0, glyph_em: 0.5 }
    }

    #[test]
    fn paginates_and_restores() {
        let ch = vec![
            "one two three four five six seven eight nine ten. ".repeat(30),
            "second chapter text here. ".repeat(10),
        ];
        let pages = paginate(&ch, small());
        assert!(pages.len() > 2);
        // chapter 2 starts on a fresh page
        let c2 = page_of_chapter(&pages, 1);
        assert_eq!(pages[c2].chapter, 1);
        assert!(c2 > 0 && pages[c2 - 1].chapter == 0);
        // offset restore: the page found for an offset contains that offset
        let off = pages[3].char_start + 5;
        assert_eq!(page_at_offset(&pages, off), 3);
        // repaginate at a different size still resolves a valid page
        let bigger = Layout { font_px: 14.0, ..small() };
        let pages2 = paginate(&ch, bigger);
        let p2 = page_at_offset(&pages2, off);
        assert!(p2 < pages2.len());
    }

    #[test]
    fn wrap_hard_splits_long_words() {
        let lines = wrap("abcdefghijklmnop", 5);
        assert!(lines.iter().all(|l| l.chars().count() <= 5));
        assert_eq!(lines.join(""), "abcdefghijklmnop");
    }
}
