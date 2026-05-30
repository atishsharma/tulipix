//! `np.p4.books.reader` — native Slint reader paging model.
//!
//! GPU painting lives in the Slint view; the page-advance logic does not, and
//! it's the part that's easy to get wrong: two-page comic spreads, manga RTL
//! (turn pages right-to-left), and which physical pages a spread shows. Image
//! dark-mode inversion is a render flag carried here.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SpreadMode { Single, Double }

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ReaderState {
    pub page: usize,           // current left/primary page index (0-based)
    pub total: usize,
    pub spread: SpreadMode,
    pub rtl: bool,             // manga right-to-left
    pub invert_images: bool,   // dark-mode invert for comics/scans
}

impl ReaderState {
    pub fn new(total: usize, comic: bool, rtl: bool) -> Self {
        Self { page: 0, total, spread: if comic { SpreadMode::Double } else { SpreadMode::Single }, rtl, invert_images: false }
    }

    fn stride(&self) -> usize { match self.spread { SpreadMode::Single => 1, SpreadMode::Double => 2 } }

    /// Advance toward the *next* reading page (handles RTL: "next" still moves
    /// forward through the book; the renderer mirrors layout).
    pub fn next(&mut self) {
        let step = self.stride();
        if self.page + step < self.total { self.page += step; }
        else if self.total > 0 { self.page = self.total.saturating_sub(if self.total >= step { step } else { 1 }).min(self.total - 1); }
    }

    pub fn prev(&mut self) {
        self.page = self.page.saturating_sub(self.stride());
    }

    /// Physical page indices visible right now, in *visual* left→right order.
    /// In double mode under RTL the higher page sits on the left.
    pub fn visible_pages(&self) -> Vec<usize> {
        match self.spread {
            SpreadMode::Single => vec![self.page],
            SpreadMode::Double => {
                let a = self.page;
                let b = self.page + 1;
                if b >= self.total { return vec![a]; }
                if self.rtl { vec![b, a] } else { vec![a, b] }
            }
        }
    }

    pub fn toggle_invert(&mut self) { self.invert_images = !self.invert_images; }

    /// 0..1 fraction read through the book.
    pub fn fraction(&self) -> f64 {
        if self.total == 0 { 0.0 } else { self.page as f64 / (self.total - 1).max(1) as f64 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_spread_pairs_pages() {
        let mut r = ReaderState::new(10, true, false);
        assert_eq!(r.visible_pages(), vec![0, 1]);
        r.next();
        assert_eq!(r.visible_pages(), vec![2, 3]);
    }

    #[test]
    fn rtl_reverses_visual_order() {
        let r = ReaderState { page: 2, total: 10, spread: SpreadMode::Double, rtl: true, invert_images: false };
        assert_eq!(r.visible_pages(), vec![3, 2]); // higher page on the left
    }

    #[test]
    fn single_mode_steps_one() {
        let mut r = ReaderState::new(5, false, false);
        r.next(); r.next();
        assert_eq!(r.page, 2);
        r.prev();
        assert_eq!(r.page, 1);
        r.toggle_invert();
        assert!(r.invert_images);
    }

    #[test]
    fn does_not_run_past_end() {
        let mut r = ReaderState::new(3, false, false);
        for _ in 0..10 { r.next(); }
        assert!(r.page < r.total);
    }
}
