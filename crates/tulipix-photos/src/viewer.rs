//! Full-screen viewer state machine.
//!
//! The Slint viewer is a GPU node fed by a `wgpu` texture (per `np.p2.viewer`
//! in the plan — explicitly NOT a webview). This module owns the data side:
//! which item is current, prev/next navigation, prefetch hints, and a small
//! ring buffer of preloaded thumbnail keys that the renderer can stream.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::VecDeque;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewerItem {
    pub item_id: i64,
    pub path: PathBuf,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub orientation: Option<i64>,
    pub taken_at: Option<i64>,
}

/// Stateful viewer cursor: holds the entire ordered ID list for the active
/// "scope" (album, folder, search, or timeline page) plus a small prefetch
/// ring. Re-entering the viewer with a different scope drops + rebuilds.
#[derive(Debug, Clone)]
pub struct ViewerCursor {
    ids: Vec<i64>,
    pos: usize,
    /// Item IDs whose images should already be decoded into texture cache.
    prefetch: VecDeque<i64>,
    prefetch_radius: usize,
}

impl ViewerCursor {
    pub fn new(ids: Vec<i64>, start: i64) -> Self {
        let pos = ids.iter().position(|&i| i == start).unwrap_or(0);
        let mut c = Self { ids, pos, prefetch: VecDeque::new(), prefetch_radius: 3 };
        c.refresh_prefetch();
        c
    }
    pub fn len(&self) -> usize { self.ids.len() }
    pub fn is_empty(&self) -> bool { self.ids.is_empty() }
    pub fn position(&self) -> usize { self.pos }
    pub fn current_id(&self) -> Option<i64> { self.ids.get(self.pos).copied() }

    pub fn next(&mut self) -> Option<i64> {
        if self.pos + 1 < self.ids.len() {
            self.pos += 1;
            self.refresh_prefetch();
            self.current_id()
        } else { None }
    }
    pub fn prev(&mut self) -> Option<i64> {
        if self.pos > 0 {
            self.pos -= 1;
            self.refresh_prefetch();
            self.current_id()
        } else { None }
    }
    pub fn jump(&mut self, item_id: i64) -> bool {
        if let Some(p) = self.ids.iter().position(|&i| i == item_id) {
            self.pos = p;
            self.refresh_prefetch();
            true
        } else { false }
    }
    pub fn prefetch_targets(&self) -> &VecDeque<i64> { &self.prefetch }

    fn refresh_prefetch(&mut self) {
        self.prefetch.clear();
        let lo = self.pos.saturating_sub(self.prefetch_radius);
        let hi = (self.pos + self.prefetch_radius).min(self.ids.len().saturating_sub(1));
        for i in lo..=hi {
            if i != self.pos {
                self.prefetch.push_back(self.ids[i]);
            }
        }
    }
}

/// Resolve a single item's viewer-side metadata.
pub async fn load_item(pool: &SqlitePool, item_id: i64) -> Result<Option<ViewerItem>> {
    let row: Option<(String, Option<i64>, Option<i64>, Option<i64>, Option<i64>)> = sqlx::query_as(
        "SELECT items.abs_path,
                photo_meta.width, photo_meta.height,
                photo_meta.orientation, photo_meta.taken_at
         FROM items LEFT JOIN photo_meta ON photo_meta.item_id = items.id
         WHERE items.id = ?",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|(p, w, h, o, t)| ViewerItem {
        item_id, path: PathBuf::from(p),
        width: w, height: h, orientation: o, taken_at: t,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_navigates_and_prefetches() {
        let mut c = ViewerCursor::new(vec![10, 20, 30, 40, 50], 30);
        assert_eq!(c.position(), 2);
        assert_eq!(c.current_id(), Some(30));
        // prefetch radius 3 → [10,20,40,50] (excludes current)
        let pf: Vec<_> = c.prefetch_targets().iter().copied().collect();
        assert!(pf.contains(&10) && pf.contains(&50));
        assert_eq!(c.next(), Some(40));
        assert_eq!(c.next(), Some(50));
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), Some(40));
        assert!(c.jump(20));
        assert_eq!(c.current_id(), Some(20));
        assert!(!c.jump(9999));
    }
    #[test]
    fn empty_cursor_returns_none() {
        let mut c = ViewerCursor::new(vec![], 0);
        assert!(c.is_empty());
        assert_eq!(c.current_id(), None);
        assert_eq!(c.next(), None);
        assert_eq!(c.prev(), None);
    }
}
