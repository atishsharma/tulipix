//! `np.p4.recent` — cross-section Recent rail.
//!
//! Each section tracks its own `last_accessed_at`; the rail merges those
//! streams into one most-recent-first list across photos/videos/music/books/
//! cloud. Sections feed in their recent entries (already queried from their own
//! DBs); this merges, sorts, and caps — DB-agnostic so it's unit-testable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecentEntry {
    pub section: String,
    pub item_id: i64,
    pub last_accessed: i64, // unix seconds
}

/// Merge per-section recents into one rail, newest first, capped at `limit`.
/// De-dups by (section, item_id) keeping the most recent timestamp.
pub fn merge(entries: Vec<RecentEntry>, limit: usize) -> Vec<RecentEntry> {
    use std::collections::HashMap;
    let mut best: HashMap<(String, i64), i64> = HashMap::new();
    for e in &entries {
        let k = (e.section.clone(), e.item_id);
        let v = best.entry(k).or_insert(e.last_accessed);
        if e.last_accessed > *v { *v = e.last_accessed; }
    }
    let mut out: Vec<RecentEntry> = best.into_iter()
        .map(|((section, item_id), last_accessed)| RecentEntry { section, item_id, last_accessed })
        .collect();
    out.sort_by(|a, b| b.last_accessed.cmp(&a.last_accessed));
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(sec: &str, id: i64, ts: i64) -> RecentEntry {
        RecentEntry { section: sec.into(), item_id: id, last_accessed: ts }
    }

    #[test]
    fn merges_newest_first_across_sections() {
        let r = merge(vec![e("photos", 1, 100), e("music", 2, 300), e("videos", 3, 200)], 10);
        assert_eq!(r[0].section, "music");
        assert_eq!(r[2].section, "photos");
    }

    #[test]
    fn dedup_keeps_latest_and_caps() {
        let r = merge(vec![e("photos", 1, 100), e("photos", 1, 500), e("music", 2, 50)], 1);
        assert_eq!(r.len(), 1);
        assert_eq!((r[0].item_id, r[0].last_accessed), (1, 500));
    }
}
