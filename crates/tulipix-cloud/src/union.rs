//! `np.p4.cloud.union` — Union / Merge Remotes UI.
//!
//! Visual builder for rclone's `union` backend: pool several free cloud drives
//! into one virtual drive. Builds the `config create` argv for a union remote
//! and the `upstreams` string with per-upstream policy suffixes (`:ro`, `:nc`).

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum UpstreamPolicy { ReadWrite, ReadOnly, NoCreate }

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Upstream {
    pub remote: String,   // e.g. "gdrive:media"
    pub policy: UpstreamPolicy,
}

impl Upstream {
    fn spec(&self) -> String {
        match self.policy {
            UpstreamPolicy::ReadWrite => self.remote.clone(),
            UpstreamPolicy::ReadOnly => format!("{}:ro", self.remote),
            UpstreamPolicy::NoCreate => format!("{}:nc", self.remote),
        }
    }
}

/// Space-separated `upstreams` value for an rclone union remote.
pub fn upstreams_value(ups: &[Upstream]) -> String {
    ups.iter().map(|u| u.spec()).collect::<Vec<_>>().join(" ")
}

/// `rclone config create <name> union upstreams="..." ...` argv.
pub fn create_args(name: &str, ups: &[Upstream], create_policy: &str) -> Vec<String> {
    vec![
        "config".into(), "create".into(), name.into(), "union".into(),
        format!("upstreams={}", upstreams_value(ups)),
        format!("create_policy={create_policy}"),
        "--non-interactive".into(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upstreams_carry_policy_suffix() {
        let ups = vec![
            Upstream { remote: "gdrive:".into(), policy: UpstreamPolicy::ReadWrite },
            Upstream { remote: "box:archive".into(), policy: UpstreamPolicy::ReadOnly },
            Upstream { remote: "mega:".into(), policy: UpstreamPolicy::NoCreate },
        ];
        assert_eq!(upstreams_value(&ups), "gdrive: box:archive:ro mega::nc");
    }

    #[test]
    fn create_argv() {
        let ups = vec![Upstream { remote: "a:".into(), policy: UpstreamPolicy::ReadWrite }];
        let a = create_args("pool", &ups, "mfs");
        assert_eq!(a[3], "union");
        assert!(a.contains(&"create_policy=mfs".to_string()));
        assert!(a.iter().any(|s| s.starts_with("upstreams=")));
    }
}
