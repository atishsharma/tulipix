//! `np.p4.search.universal` — single search box fanning out across sections.
//!
//! Each section (photos/videos/music/books/cloud) runs its own FTS search and
//! returns ranked [`SearchHit`]s; this merges them into one relevance-ordered
//! list, applies the `type:` filter from the parsed query, de-duplicates by
//! (section, item_id), and caps the result count. The per-section search fns
//! are injected so this stays DB-agnostic and unit-testable.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub section: String,   // "photos" | "videos" | "music" | "books" | "cloud"
    pub item_id: i64,
    pub title: String,
    /// Lower rank = better match (e.g. SQLite FTS `bm25`).
    pub rank: f64,
}

/// Map a `type:` operator value to the section it scopes to.
pub fn type_to_section(ty: &str) -> Option<&'static str> {
    Some(match ty.to_ascii_lowercase().as_str() {
        "photo" | "photos" | "image" => "photos",
        "video" | "videos" | "movie" => "videos",
        "audio" | "music" | "song" | "track" => "music",
        "book" | "books" | "comic" | "ebook" => "books",
        "cloud" | "remote" => "cloud",
        _ => return None,
    })
}

/// Merge per-section results into one ranked list. `type_filter` (from the
/// parsed query's `type:`) restricts to one section when set. De-dups by
/// (section, item_id), keeping the better rank.
pub fn merge(mut hits: Vec<SearchHit>, type_filter: Option<&str>, limit: usize) -> Vec<SearchHit> {
    if let Some(ty) = type_filter.and_then(type_to_section) {
        hits.retain(|h| h.section == ty);
    }
    // dedup keeping best (lowest) rank
    hits.sort_by(|a, b| a.rank.partial_cmp(&b.rank).unwrap_or(std::cmp::Ordering::Equal));
    let mut seen = std::collections::HashSet::new();
    hits.retain(|h| seen.insert((h.section.clone(), h.item_id)));
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(sec: &str, id: i64, rank: f64) -> SearchHit {
        SearchHit { section: sec.into(), item_id: id, title: format!("{sec}-{id}"), rank }
    }

    #[test]
    fn merges_and_ranks_across_sections() {
        let hits = vec![hit("photos", 1, 3.0), hit("music", 2, 1.0), hit("videos", 3, 2.0)];
        let m = merge(hits, None, 10);
        assert_eq!(m[0].section, "music"); // best rank first
        assert_eq!(m.len(), 3);
    }

    #[test]
    fn type_filter_restricts_section() {
        let hits = vec![hit("photos", 1, 1.0), hit("music", 2, 2.0)];
        let m = merge(hits, Some("audio"), 10);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].section, "music");
    }

    #[test]
    fn dedups_keeping_best_rank() {
        let hits = vec![hit("photos", 1, 5.0), hit("photos", 1, 2.0)];
        let m = merge(hits, None, 10);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].rank, 2.0);
    }
}
