//! `np.p4.tools.folder-diff` — diff two folders.
//!
//! Compares two file listings by relative path + content hash → only-in-A /
//! only-in-B / modified (same path, different hash). Drives per-row copy/sync
//! actions. Pure set logic over `(rel_path → hash)` maps.
//!
//! Mirroring sits here too, as a *plan* rather than an action: [`plan_mirror`]
//! works out every copy and every delete and touches nothing. The caller runs
//! them one at a time so a cancel lands between files, and the preview shows
//! the same list before Start exists — which is the whole point of a
//! destructive tool that names what it will delete.

use std::collections::BTreeMap;
use std::path::Path;

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

// ------------------------------------------------------------------ mirror ---

/// Every file under `root`, as `relative path → (bytes, modified seconds)`.
///
/// Size and mtime rather than a hash: this runs on a keystroke in the preview,
/// and reading every byte of a photo library to redraw a pane is not a
/// preview. The comparison it feeds says so out loud.
pub fn walk(root: &str, cap: usize) -> BTreeMap<String, (u64, i64)> {
    let mut out = BTreeMap::new();
    let base = Path::new(root);
    let mut stack = vec![base.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for entry in rd.filter_map(|e| e.ok()) {
            if out.len() >= cap { return out; }
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(rel) = path.strip_prefix(base) {
                let meta = entry.metadata();
                let bytes = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                let mtime = meta.ok()
                    .and_then(|m| m.modified().ok())
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                out.insert(rel.to_string_lossy().to_string(), (bytes, mtime));
            }
        }
    }
    out
}

/// One thing a mirror does. Both carry the relative path, because that is what
/// a person reads, and the byte count, because that is what they weigh.
#[derive(Debug, Clone, PartialEq)]
pub enum MirrorAction {
    Copy { from: String, to: String, rel: String, bytes: u64 },
    Delete { path: String, rel: String, bytes: u64 },
}

impl MirrorAction {
    pub fn rel(&self) -> &str {
        match self { MirrorAction::Copy { rel, .. } | MirrorAction::Delete { rel, .. } => rel }
    }
    pub fn bytes(&self) -> u64 {
        match self { MirrorAction::Copy { bytes, .. } | MirrorAction::Delete { bytes, .. } => *bytes }
    }
}

/// What making `b` match `a` involves. Copies first, then deletes: a run that
/// stops halfway should have written the new files rather than removed the old
/// ones.
pub fn plan_mirror(a: &str, b: &str, delete_extra: bool, cap: usize) -> Vec<MirrorAction> {
    let left = walk(a, cap);
    let right = walk(b, cap);
    let mut copies = Vec::new();
    for (rel, (bytes, mtime)) in &left {
        // Same size and same second: as close to "unchanged" as this can get
        // without reading the file.
        if right.get(rel).is_some_and(|(rb, rm)| rb == bytes && rm == mtime) { continue; }
        copies.push(MirrorAction::Copy {
            from: Path::new(a).join(rel).to_string_lossy().to_string(),
            to: Path::new(b).join(rel).to_string_lossy().to_string(),
            rel: rel.clone(),
            bytes: *bytes,
        });
    }
    let mut deletes = Vec::new();
    if delete_extra {
        for (rel, (bytes, _)) in &right {
            if left.contains_key(rel) { continue; }
            deletes.push(MirrorAction::Delete {
                path: Path::new(b).join(rel).to_string_lossy().to_string(),
                rel: rel.clone(),
                bytes: *bytes,
            });
        }
    }
    copies.extend(deletes);
    copies
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    fn write(dir: &Path, name: &str, body: &[u8]) {
        if let Some(parent) = Path::new(name).parent() {
            std::fs::create_dir_all(dir.join(parent)).unwrap();
        }
        std::fs::write(dir.join(name), body).unwrap();
    }

    #[test]
    fn copies_what_is_missing_or_different_and_leaves_the_rest() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "keep.txt", b"same");
        write(b.path(), "keep.txt", b"same");
        write(a.path(), "sub/new.txt", b"new");
        write(a.path(), "changed.txt", b"longer body");
        write(b.path(), "changed.txt", b"short");
        write(b.path(), "extra.txt", b"gone soon");

        let plan = plan_mirror(
            &a.path().to_string_lossy(),
            &b.path().to_string_lossy(),
            false,
            1000,
        );
        let rels: Vec<&str> = plan.iter().map(|x| x.rel()).collect();
        assert!(rels.contains(&"changed.txt"));
        assert!(rels.iter().any(|r| r.ends_with("new.txt")));
        // Identical by size and mtime, so it is not copied again.
        assert!(!rels.contains(&"keep.txt"));
        // Without the toggle, nothing is deleted.
        assert!(!plan.iter().any(|x| matches!(x, MirrorAction::Delete { .. })));
    }

    #[test]
    fn deletes_come_last_and_only_when_asked() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write(a.path(), "one.txt", b"x");
        write(b.path(), "extra.txt", b"y");
        let plan = plan_mirror(
            &a.path().to_string_lossy(),
            &b.path().to_string_lossy(),
            true,
            1000,
        );
        assert_eq!(plan.len(), 2);
        assert!(matches!(plan[0], MirrorAction::Copy { .. }));
        // A run that stops halfway has written the new file, not removed the old.
        assert!(matches!(plan[1], MirrorAction::Delete { .. }));
    }
}
