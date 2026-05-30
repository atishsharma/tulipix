//! `np.p4.music.grid-density` — adjustable thumb grid density.
//!
//! The cover grid's slider runs 100→300 px target thumb size. To re-flow live
//! without a scroll jump we compute the column count + actual cell size for a
//! container width, and map the scroll anchor (first visible item) to the new
//! layout so the same item stays in view.

pub const MIN_PX: f64 = 100.0;
pub const MAX_PX: f64 = 300.0;

pub fn clamp_target(px: f64) -> f64 { px.clamp(MIN_PX, MAX_PX) }

/// Columns that fit in `container_w` for a `target_px` cell with `gap` between.
/// Always ≥ 1.
pub fn columns(container_w: f64, target_px: f64, gap: f64) -> u32 {
    let t = clamp_target(target_px);
    (((container_w + gap) / (t + gap)).floor() as i64).max(1) as u32
}

/// Actual cell size once columns are chosen (cells stretch to fill the row).
pub fn cell_size(container_w: f64, cols: u32, gap: f64) -> f64 {
    let cols = cols.max(1) as f64;
    (container_w - gap * (cols - 1.0)) / cols
}

/// Re-flow without scroll jump: given the index of the first visible item and
/// the new layout, return the scroll-Y that keeps that item's row at the top.
pub fn scroll_anchor(first_visible_index: usize, cols: u32, cell: f64, gap: f64) -> f64 {
    let row = (first_visible_index as u32 / cols.max(1)) as f64;
    row * (cell + gap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_fit_container() {
        // 1000 wide, 100px cells, 20 gap → (1000+20)/120 = 8.5 → 8
        assert_eq!(columns(1000.0, 100.0, 20.0), 8);
        // bigger thumbs → fewer columns
        assert_eq!(columns(1000.0, 300.0, 20.0), 3);
        // never zero
        assert_eq!(columns(50.0, 300.0, 20.0), 1);
    }

    #[test]
    fn cells_stretch_to_fill() {
        let c = cell_size(1000.0, 8, 20.0);
        assert!((c - 107.5).abs() < 0.01);
    }

    #[test]
    fn anchor_keeps_item_row() {
        // item 17 with 8 cols is on row 2 → y = 2*(100+20)
        let y = scroll_anchor(17, 8, 100.0, 20.0);
        assert!((y - 240.0).abs() < 1e-6);
    }

    #[test]
    fn target_clamps() {
        assert_eq!(clamp_target(50.0), MIN_PX);
        assert_eq!(clamp_target(999.0), MAX_PX);
    }
}
