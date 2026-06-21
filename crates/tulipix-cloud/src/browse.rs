//! `np.p4.cloud.browse` — native remote-tree browser (lazy expand, sort).
//!
//! Backs the Slint tree widget. Builds `rclone lsjson` argv for one directory
//! level (lazy expand: only the clicked node), parses the JSON array, and sorts
//! entries the way the column header asks (dirs-first, then key).

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct Entry {
    #[serde(rename = "Name")] pub name: String,
    #[serde(rename = "Size", default)] pub size: i64,
    #[serde(rename = "IsDir", default)] pub is_dir: bool,
    #[serde(rename = "ModTime", default)] pub mod_time: String,
    // Populated by `--recursive` listings: path relative to the lsjson root.
    // Empty for single-level listings (rclone omits it without `--recursive`).
    #[serde(rename = "Path", default)] pub path: String,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SortKey { Name, Size, Date }

/// `rclone lsjson remote:path` argv — one level only (no `--recursive`).
pub fn lsjson_args(remote: &str, path: &str) -> Vec<String> {
    vec!["lsjson".into(), format!("{remote}:{path}")]
}

/// `rclone lsjson --recursive remote:path` argv — every object under the path,
/// each carrying a `Path` relative to `remote:path`. Backs whole-remote search.
pub fn lsjson_recursive_args(remote: &str, path: &str) -> Vec<String> {
    vec!["lsjson".into(), "--recursive".into(), format!("{remote}:{path}")]
}

pub fn parse_lsjson(json: &str) -> Result<Vec<Entry>> {
    Ok(serde_json::from_str(json)?)
}

/// Sort in place: directories first, then by the chosen key (ascending).
pub fn sort_entries(entries: &mut [Entry], key: SortKey) {
    entries.sort_by(|a, b| {
        b.is_dir.cmp(&a.is_dir) // dirs (true) first
            .then_with(|| match key {
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::Size => a.size.cmp(&b.size),
                SortKey::Date => a.mod_time.cmp(&b.mod_time),
            })
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const JSON: &str = r#"[
      {"Name":"zeta.txt","Size":30,"IsDir":false,"ModTime":"2024-03-01T00:00:00Z"},
      {"Name":"alpha","Size":0,"IsDir":true,"ModTime":"2024-01-01T00:00:00Z"},
      {"Name":"beta.txt","Size":10,"IsDir":false,"ModTime":"2024-02-01T00:00:00Z"}
    ]"#;

    #[test]
    fn argv_single_level() {
        assert_eq!(lsjson_args("gdrive", "Photos"), vec!["lsjson", "gdrive:Photos"]);
    }

    #[test]
    fn dirs_first_then_name() {
        let mut e = parse_lsjson(JSON).unwrap();
        sort_entries(&mut e, SortKey::Name);
        assert_eq!(e[0].name, "alpha");      // dir first
        assert_eq!(e[1].name, "beta.txt");
        assert_eq!(e[2].name, "zeta.txt");
    }

    #[test]
    fn sort_by_size() {
        let mut e = parse_lsjson(JSON).unwrap();
        sort_entries(&mut e, SortKey::Size);
        assert_eq!(e[0].name, "alpha"); // dir
        assert_eq!(e[1].name, "beta.txt"); // smaller file first
    }
}
