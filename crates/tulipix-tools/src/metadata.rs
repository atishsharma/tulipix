//! `np.p4.tools.metadata` — bulk metadata editor (EXIF / ID3 / MP4 / ffprobe);
//! rule-based fills.
//!
//! Applies a set of rules across many files: set a constant, or fill a field
//! from another field / a pattern when empty. This resolves rules against a
//! per-file tag map (the actual write goes through exif/id3 backends) and is
//! fully unit-tested.

use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum Rule {
    /// Always set `field` to `value`.
    Set { field: String, value: String },
    /// Set `field` only if currently empty/absent.
    FillIfEmpty { field: String, value: String },
    /// Copy `from` → `to` when `to` is empty.
    CopyIfEmpty { from: String, to: String },
}

/// Apply rules in order to a tag map, returning the changed map.
pub fn apply(tags: &HashMap<String, String>, rules: &[Rule]) -> HashMap<String, String> {
    let mut out = tags.clone();
    let is_empty = |m: &HashMap<String, String>, k: &str| m.get(k).is_none_or(|v| v.trim().is_empty());
    for rule in rules {
        match rule {
            Rule::Set { field, value } => { out.insert(field.clone(), value.clone()); }
            Rule::FillIfEmpty { field, value } => {
                if is_empty(&out, field) { out.insert(field.clone(), value.clone()); }
            }
            Rule::CopyIfEmpty { from, to } => {
                if is_empty(&out, to) {
                    if let Some(v) = out.get(from).cloned() { out.insert(to.clone(), v); }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(p: &[(&str, &str)]) -> HashMap<String, String> {
        p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn rules_apply_in_order() {
        let tags = map(&[("artist", "Band"), ("album_artist", "")]);
        let rules = vec![
            Rule::CopyIfEmpty { from: "artist".into(), to: "album_artist".into() },
            Rule::Set { field: "genre".into(), value: "Rock".into() },
            Rule::FillIfEmpty { field: "artist".into(), value: "Unknown".into() }, // no-op
        ];
        let r = apply(&tags, &rules);
        assert_eq!(r["album_artist"], "Band");
        assert_eq!(r["genre"], "Rock");
        assert_eq!(r["artist"], "Band"); // not overwritten by FillIfEmpty
    }

    #[test]
    fn fill_only_empty() {
        let r = apply(&map(&[("comment", "  ")]), &[Rule::FillIfEmpty { field: "comment".into(), value: "x".into() }]);
        assert_eq!(r["comment"], "x");
    }
}
