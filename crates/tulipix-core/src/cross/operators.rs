//! `np.p4.search.operators` — operator parser.
//!
//! Parses a search box string into free-text terms + structured filters:
//! `type:` / `before:` / `after:` / `has:gps` / `face:` / `album:` /
//! `in:shared`. Quoted values are supported (`album:"Road Trip"`). The parsed
//! form drives the universal fan-out and per-section SQL.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ParsedQuery {
    pub terms: Vec<String>,
    pub r#type: Option<String>,   // photo|video|audio|book|...
    pub before: Option<String>,   // date string (yyyy-mm-dd)
    pub after: Option<String>,
    pub has: Vec<String>,         // gps, faces, ...
    pub face: Option<String>,
    pub album: Option<String>,
    pub in_scope: Option<String>, // shared, trash, ...
}

/// Split respecting double-quoted spans so `album:"Road Trip"` is one token.
fn tokenize(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for c in input.chars() {
        match c {
            '"' => in_q = !in_q,
            c if c.is_whitespace() && !in_q => { if !cur.is_empty() { out.push(std::mem::take(&mut cur)); } }
            c => cur.push(c),
        }
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

pub fn parse(input: &str) -> ParsedQuery {
    let mut q = ParsedQuery::default();
    for tok in tokenize(input) {
        match tok.split_once(':') {
            Some((key, val)) if !val.is_empty() => match key.to_ascii_lowercase().as_str() {
                "type"  => q.r#type = Some(val.to_string()),
                "before" => q.before = Some(val.to_string()),
                "after"  => q.after = Some(val.to_string()),
                "has"    => q.has.push(val.to_ascii_lowercase()),
                "face"   => q.face = Some(val.to_string()),
                "album"  => q.album = Some(val.to_string()),
                "in"     => q.in_scope = Some(val.to_ascii_lowercase()),
                _ => q.terms.push(tok), // unknown operator → treat as text
            },
            _ => q.terms.push(tok),
        }
    }
    q
}

impl ParsedQuery {
    /// Free-text joined for an FTS MATCH (empty → match-all handled by caller).
    pub fn fts_query(&self) -> String { self.terms.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_mixed_operators() {
        let q = parse(r#"sunset type:photo has:gps album:"Road Trip" before:2020-01-01"#);
        assert_eq!(q.terms, vec!["sunset"]);
        assert_eq!(q.r#type.as_deref(), Some("photo"));
        assert_eq!(q.album.as_deref(), Some("Road Trip"));
        assert_eq!(q.before.as_deref(), Some("2020-01-01"));
        assert!(q.has.contains(&"gps".to_string()));
    }

    #[test]
    fn in_shared_and_face() {
        let q = parse("face:Alice in:shared");
        assert_eq!(q.face.as_deref(), Some("Alice"));
        assert_eq!(q.in_scope.as_deref(), Some("shared"));
        assert!(q.terms.is_empty());
    }

    #[test]
    fn bare_text_only() {
        let q = parse("holiday beach");
        assert_eq!(q.fts_query(), "holiday beach");
        assert!(q.r#type.is_none());
    }
}
