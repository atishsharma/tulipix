//! `np.p4.books.continuous` — continuous vertical scroll (webtoon) mode.
//!
//! Comics can read as a paginated spread or as one tall scroll (webtoon
//! style). This owns the scroll model: given per-page heights stacked
//! vertically, map a scroll offset → current page (and back) so switching
//! modes keeps the reader on the same page, and report progress.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum ReadMode { Paginated, Continuous }

/// Cumulative top offsets for each page given their heights (+ optional gap).
pub fn page_offsets(heights: &[f64], gap: f64) -> Vec<f64> {
    let mut offs = Vec::with_capacity(heights.len());
    let mut y = 0.0;
    for h in heights {
        offs.push(y);
        y += h + gap;
    }
    offs
}

/// Total scrollable height.
pub fn total_height(heights: &[f64], gap: f64) -> f64 {
    let sum: f64 = heights.iter().sum();
    sum + gap * heights.len().saturating_sub(1) as f64
}

/// Page whose band contains scroll offset `y` (the last page starting ≤ y).
pub fn page_at_offset(offsets: &[f64], y: f64) -> usize {
    if offsets.is_empty() { return 0; }
    offsets.iter().rposition(|o| *o <= y.max(0.0)).unwrap_or(0)
}

/// Scroll offset that puts `page` at the top — used when switching from
/// paginated to continuous so the reader doesn't lose their place.
pub fn offset_for_page(offsets: &[f64], page: usize) -> f64 {
    offsets.get(page).copied().unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_stack_with_gap() {
        let h = [100.0, 200.0, 50.0];
        let offs = page_offsets(&h, 10.0);
        assert_eq!(offs, vec![0.0, 110.0, 320.0]);
        assert_eq!(total_height(&h, 10.0), 370.0);
    }

    #[test]
    fn offset_maps_to_page_and_back() {
        let h = [100.0, 200.0, 50.0];
        let offs = page_offsets(&h, 10.0);
        assert_eq!(page_at_offset(&offs, 0.0), 0);
        assert_eq!(page_at_offset(&offs, 150.0), 1);
        assert_eq!(page_at_offset(&offs, 999.0), 2);
        // round-trip: switching modes keeps the page
        let p = 1;
        assert_eq!(page_at_offset(&offs, offset_for_page(&offs, p)), p);
    }
}
