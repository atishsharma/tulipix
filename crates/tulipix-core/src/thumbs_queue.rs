//! Viewport-intersection thumb generation queue + LRU eviction.
//!
//! Producer side: the grid view tells us which item ids are visible
//! (`set_viewport`) and which is hovered (`hover`). Consumer side: the
//! thumb renderer pops the next id off the queue. Priority order:
//!   1. Hovered id (highest — bump to front)
//!   2. Visible viewport, near-centre first
//!   3. Predicted-scroll lookahead (driven by tulipix-core::perf::prefetch)
//!   4. Stale (already rendered but item rev bumped)
//!
//! Eviction runs every time the cache crosses `CACHE_CAP_BYTES` (5 GB).
//! Oldest-access-first removal until usage drops to 90 % of the cap.

use crate::thumbs::CACHE_CAP_BYTES;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

pub type ItemId = i64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    Stale       = 0,
    Lookahead   = 1,
    Viewport    = 2,
    Hover       = 3,
}

#[derive(Debug, Clone)]
struct Entry { id: ItemId, prio: Priority, distance_from_centre: u32 }

#[derive(Debug, Default)]
pub struct LazyQueue {
    pending: Vec<Entry>,
    queued_ids: HashSet<ItemId>,
    in_flight: HashSet<ItemId>,
}

impl LazyQueue {
    pub fn new() -> Self { Self::default() }

    pub fn set_viewport(&mut self, ids: &[ItemId]) {
        let centre = (ids.len() / 2) as u32;
        for (i, &id) in ids.iter().enumerate() {
            self.enqueue(Entry { id, prio: Priority::Viewport, distance_from_centre: (i as i64 - centre as i64).unsigned_abs() as u32 });
        }
    }

    pub fn enqueue_lookahead(&mut self, ids: &[ItemId]) {
        for &id in ids { self.enqueue(Entry { id, prio: Priority::Lookahead, distance_from_centre: u32::MAX }); }
    }

    pub fn hover(&mut self, id: ItemId) {
        // Bump (or insert) to top priority.
        self.queued_ids.remove(&id);
        self.pending.retain(|e| e.id != id);
        self.enqueue(Entry { id, prio: Priority::Hover, distance_from_centre: 0 });
    }

    pub fn cancel_outside_viewport(&mut self, visible: &HashSet<ItemId>) {
        self.pending.retain(|e| e.prio == Priority::Hover || visible.contains(&e.id));
        self.queued_ids.retain(|id| visible.contains(id));
    }

    fn enqueue(&mut self, e: Entry) {
        if self.in_flight.contains(&e.id) { return; }
        if self.queued_ids.insert(e.id) {
            self.pending.push(e);
        } else if let Some(existing) = self.pending.iter_mut().find(|x| x.id == e.id) {
            if e.prio > existing.prio { existing.prio = e.prio; existing.distance_from_centre = e.distance_from_centre.min(existing.distance_from_centre); }
        }
    }

    /// Pop the next id to render. None = queue empty.
    pub fn next(&mut self) -> Option<ItemId> {
        if self.pending.is_empty() { return None; }
        let mut best = 0usize;
        for i in 1..self.pending.len() {
            let a = &self.pending[i];
            let b = &self.pending[best];
            if (a.prio, std::cmp::Reverse(a.distance_from_centre)) > (b.prio, std::cmp::Reverse(b.distance_from_centre)) {
                best = i;
            }
        }
        let e = self.pending.swap_remove(best);
        self.queued_ids.remove(&e.id);
        self.in_flight.insert(e.id);
        Some(e.id)
    }

    pub fn complete(&mut self, id: ItemId) { self.in_flight.remove(&id); }
    pub fn pending_len(&self) -> usize { self.pending.len() }
}

// ─── LRU eviction ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct LruEntry { pub path: PathBuf, pub bytes: u64, pub last_access_unix: u64 }

#[derive(Debug, Default)]
pub struct LruIndex { entries: HashMap<PathBuf, LruEntry>, total: u64 }

impl LruIndex {
    pub fn new() -> Self { Self::default() }
    pub fn touch(&mut self, path: PathBuf, bytes: u64) {
        let now = now_unix();
        if let Some(prev) = self.entries.insert(path.clone(), LruEntry { path, bytes, last_access_unix: now }) {
            self.total = self.total.saturating_sub(prev.bytes);
        }
        self.total += bytes;
    }
    pub fn total_bytes(&self) -> u64 { self.total }
    pub fn at_threshold(&self) -> bool { self.total >= CACHE_CAP_BYTES }

    /// Returns the list of paths the caller should delete. Plans an
    /// eviction so the cache lands at <= 90 % of `CACHE_CAP_BYTES`.
    pub fn plan_eviction(&mut self) -> Vec<PathBuf> {
        let target = (CACHE_CAP_BYTES as f64 * 0.9) as u64;
        if self.total <= target { return Vec::new(); }
        let mut sorted: Vec<&LruEntry> = self.entries.values().collect();
        sorted.sort_by_key(|e| e.last_access_unix);
        let mut out = Vec::new();
        let mut freed = 0u64;
        let need = self.total - target;
        for e in sorted {
            if freed >= need { break; }
            freed += e.bytes;
            out.push(e.path.clone());
        }
        for p in &out {
            if let Some(e) = self.entries.remove(p) { self.total = self.total.saturating_sub(e.bytes); }
        }
        out
    }
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn hover_beats_viewport() {
        let mut q = LazyQueue::new();
        q.set_viewport(&[10, 11, 12, 13, 14]);
        q.hover(99);
        assert_eq!(q.next(), Some(99));
    }
    #[test] fn viewport_centre_first() {
        let mut q = LazyQueue::new();
        q.set_viewport(&[1, 2, 3, 4, 5]); // centre = 3
        assert_eq!(q.next(), Some(3));
    }
    #[test] fn cancel_outside_viewport_drops_stale_ids() {
        let mut q = LazyQueue::new();
        q.set_viewport(&[1, 2, 3]);
        let visible: HashSet<ItemId> = [2, 3].iter().copied().collect();
        q.cancel_outside_viewport(&visible);
        let popped: Vec<_> = std::iter::from_fn(|| q.next()).collect();
        assert!(!popped.contains(&1));
        assert!(popped.contains(&2));
    }
    #[test] fn lookahead_lower_than_viewport() {
        let mut q = LazyQueue::new();
        q.set_viewport(&[1]);
        q.enqueue_lookahead(&[99]);
        assert_eq!(q.next(), Some(1));
        assert_eq!(q.next(), Some(99));
    }
    #[test] fn complete_clears_in_flight() {
        let mut q = LazyQueue::new();
        q.set_viewport(&[7]);
        assert_eq!(q.next(), Some(7));
        // While in flight, re-enqueueing the same id is dropped.
        q.set_viewport(&[7]);
        assert_eq!(q.next(), None);
        q.complete(7);
        q.set_viewport(&[7]);
        assert_eq!(q.next(), Some(7));
    }
    #[test] fn lru_evicts_oldest_first() {
        let mut idx = LruIndex::new();
        // Total = 6 GB → over 5 GB cap → evict to 0.9 * 5 GB = 4.5 GB.
        idx.touch(PathBuf::from("/old"),  3 * 1024 * 1024 * 1024);
        idx.entries.get_mut(std::path::Path::new("/old")).unwrap().last_access_unix = 100;
        idx.touch(PathBuf::from("/new"),  3 * 1024 * 1024 * 1024);
        idx.entries.get_mut(std::path::Path::new("/new")).unwrap().last_access_unix = 200;
        assert!(idx.at_threshold());
        let plan = idx.plan_eviction();
        assert_eq!(plan, vec![PathBuf::from("/old")]);
        assert!(idx.total_bytes() < CACHE_CAP_BYTES);
    }
    #[test] fn lru_no_op_under_threshold() {
        let mut idx = LruIndex::new();
        idx.touch(PathBuf::from("/a"), 1024);
        assert!(idx.plan_eviction().is_empty());
    }
}
