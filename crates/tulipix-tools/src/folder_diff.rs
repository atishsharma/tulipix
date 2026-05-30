//! `np.p4.tools.folder-diff` — diff two folders.
//!
//! Compares two file listings by relative path + content hash → only-in-A /
//! only-in-B / modified (same path, different hash). Drives per-row copy/sync
//! actions. Pure set logic over `(rel_path → hash)` maps.

use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diff {
    pub only_in_a: Vec<String>,
    pub only_in_b: Vec<String>,
    pub modified: Vec<String>,   // present in both, hashes differ
    pub identical: Vec<String>,
}

/// Diff two `rel_path → content_hash` maps.
pub fn diff(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> Diff {
    let mut d = Diff::default();
    for (path, ha) in a {
        match b.get(path) {
            None => d.only_in_a.push(path.clone()),
            Some(hb) if hb != ha => d.modified.push(path.clone()),
            Some(_) => d.identical.push(path.clone()),
        }
    }
    for path in b.keys() {
        if !a.contains_key(path) { d.only_in_b.push(path.clone()); }
    }
    d
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SyncAction { CopyAToB, CopyBToA, Skip }

/// Suggested action to make B mirror A for a path in a given diff bucket.
pub fn mirror_a_to_b(path: &str, diff: &Diff) -> SyncAction {
    if diff.only_in_a.iter().any(|p| p == path) || diff.modified.iter().any(|p| p == path) {
        SyncAction::CopyAToB
    } else if diff.only_in_b.iter().any(|p| p == path) {
        SyncAction::CopyBToA // would be a delete in strict mirror; copy-back is the safe default
    } else {
        SyncAction::Skip
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(p: &[(&str, &str)]) -> BTreeMap<String, String> {
        p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn classifies_buckets() {
        let a = m(&[("same.txt", "h1"), ("changed.txt", "h2"), ("onlya.txt", "h3")]);
        let b = m(&[("same.txt", "h1"), ("changed.txt", "hX"), ("onlyb.txt", "h4")]);
        let d = diff(&a, &b);
        assert_eq!(d.only_in_a, vec!["onlya.txt"]);
        assert_eq!(d.only_in_b, vec!["onlyb.txt"]);
        assert_eq!(d.modified, vec!["changed.txt"]);
        assert_eq!(d.identical, vec!["same.txt"]);
    }

    #[test]
    fn sync_actions() {
        let a = m(&[("onlya.txt", "h")]);
        let b = m(&[("onlyb.txt", "h")]);
        let d = diff(&a, &b);
        assert_eq!(mirror_a_to_b("onlya.txt", &d), SyncAction::CopyAToB);
        assert_eq!(mirror_a_to_b("onlyb.txt", &d), SyncAction::CopyBToA);
    }
}
