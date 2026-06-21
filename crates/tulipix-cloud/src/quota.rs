//! `np.p5.cloud.quota` — usage dashboard: per-remote storage used / quota.
//!
//! `rclone about <remote>: --json` reports backend storage totals. Not every
//! backend supports it (e.g. plain HTTP/WebDAV), so the parse is fully
//! optional-field. Argv + parse here; the subprocess spawn lives in the app.

use serde::Deserialize;

/// `rclone about <remote>: --json`.
pub fn about_args(remote: &str) -> Vec<String> {
    vec!["about".into(), format!("{remote}:"), "--json".into()]
}

/// Parsed `rclone about` totals (bytes). Every field is optional — backends
/// report whatever subset they can.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct About {
    pub total: Option<i64>,
    pub used: Option<i64>,
    pub free: Option<i64>,
    pub trashed: Option<i64>,
    pub other: Option<i64>,
    pub objects: Option<i64>,
}

impl About {
    /// Used / total as a 0.0–1.0 fraction, when both are known and total > 0.
    pub fn fraction(&self) -> Option<f64> {
        match (self.used, self.total) {
            (Some(u), Some(t)) if t > 0 => Some((u as f64 / t as f64).clamp(0.0, 1.0)),
            _ => None,
        }
    }
}

/// Parse `rclone about --json` output. Returns `None` on non-JSON / error
/// output (e.g. a backend that does not implement `About`).
pub fn parse_about(json: &str) -> Option<About> {
    serde_json::from_str(json.trim()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_argv() {
        assert_eq!(about_args("gdrive"), ["about", "gdrive:", "--json"]);
    }

    #[test]
    fn parses_full_and_partial() {
        let full = r#"{"total":16106127360,"used":8053063680,"free":8053063680,"trashed":1024,"objects":42}"#;
        let a = parse_about(full).unwrap();
        assert_eq!(a.total, Some(16106127360));
        assert_eq!(a.used, Some(8053063680));
        assert!((a.fraction().unwrap() - 0.5).abs() < 1e-9);

        // Backends that only report "used" still parse; fraction is None.
        let partial = r#"{"used":1000}"#;
        let b = parse_about(partial).unwrap();
        assert_eq!(b.used, Some(1000));
        assert_eq!(b.total, None);
        assert_eq!(b.fraction(), None);

        assert!(parse_about("Error: command about not found").is_none());
    }
}
