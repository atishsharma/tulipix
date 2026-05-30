//! Library export to portable JSON. Per-section dump of proxy rows
//! plus relationships (albums ↔ items, playlists ↔ tracks, tags,
//! ratings). No asset bytes — just rows. Round-trip target: import
//! into a fresh Tulipix install and recover library state without
//! touching the original disks.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Section { Photos, Videos, Music, Books, Cloud }

impl Section {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Photos => "photos", Self::Videos => "videos", Self::Music => "music",
            Self::Books => "books", Self::Cloud => "cloud",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ItemRow {
    pub id: i64,
    pub abs_path: String,
    pub sha256: Option<String>,
    pub size: i64,
    pub mtime: i64,
    pub added: i64,
    pub missing_since: Option<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Relationship {
    pub kind: String, // album, playlist, tag, rating, face-cluster
    pub label: String,
    pub item_ids: Vec<i64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SectionDump {
    pub items: Vec<ItemRow>,
    pub relationships: Vec<Relationship>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportEnvelope {
    pub format: String,
    pub format_version: u32,
    pub app_version: String,
    pub created_at_unix: u64,
    pub sections: std::collections::BTreeMap<String, SectionDump>,
}

pub const FORMAT: &str = "tulipix-library-export";
pub const FORMAT_VERSION: u32 = 1;

pub fn empty() -> ExportEnvelope {
    ExportEnvelope {
        format: FORMAT.into(),
        format_version: FORMAT_VERSION,
        app_version: env!("CARGO_PKG_VERSION").into(),
        created_at_unix: now_unix(),
        sections: Default::default(),
    }
}

pub fn add_section(env: &mut ExportEnvelope, section: Section, dump: SectionDump) {
    env.sections.insert(section.as_str().into(), dump);
}

pub fn to_json(env: &ExportEnvelope) -> Result<Vec<u8>> { Ok(serde_json::to_vec_pretty(env)?) }

pub fn from_json(bytes: &[u8]) -> Result<ExportEnvelope> { Ok(serde_json::from_slice(bytes)?) }

/// Cross-section invariants checked at import time. Returns warning
/// strings — empty Vec means clean.
pub fn validate(env: &ExportEnvelope) -> Vec<String> {
    let mut warns = Vec::new();
    if env.format != FORMAT { warns.push(format!("unexpected format: {}", env.format)); }
    if env.format_version != FORMAT_VERSION {
        warns.push(format!("format version mismatch: have {}, expected {}", env.format_version, FORMAT_VERSION));
    }
    for (name, dump) in &env.sections {
        let valid_ids: std::collections::HashSet<i64> = dump.items.iter().map(|i| i.id).collect();
        for rel in &dump.relationships {
            for id in &rel.item_ids {
                if !valid_ids.contains(id) {
                    warns.push(format!("{name}/{}/{} references missing item id {id}", rel.kind, rel.label));
                }
            }
        }
        // Duplicate path inside a section breaks the proxy uniqueness.
        let mut seen = std::collections::HashSet::new();
        for it in &dump.items {
            if !seen.insert(it.abs_path.clone()) {
                warns.push(format!("{name}: duplicate abs_path {}", it.abs_path));
            }
        }
    }
    warns
}

fn now_unix() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn item(id: i64, path: &str) -> ItemRow {
        ItemRow { id, abs_path: path.into(), sha256: None, size: 0, mtime: 0, added: 0, missing_since: None }
    }
    #[test] fn round_trip_json() {
        let mut env = empty();
        let dump = SectionDump {
            items: vec![item(1, "/a.jpg"), item(2, "/b.jpg")],
            relationships: vec![Relationship { kind: "album".into(), label: "Trip".into(), item_ids: vec![1, 2] }],
        };
        add_section(&mut env, Section::Photos, dump);
        let bytes = to_json(&env).unwrap();
        let parsed = from_json(&bytes).unwrap();
        assert_eq!(parsed.format, FORMAT);
        assert_eq!(parsed.sections["photos"].items.len(), 2);
        assert!(validate(&parsed).is_empty());
    }
    #[test] fn missing_item_id_warns() {
        let mut env = empty();
        let dump = SectionDump {
            items: vec![item(1, "/a.jpg")],
            relationships: vec![Relationship { kind: "album".into(), label: "Trip".into(), item_ids: vec![1, 99] }],
        };
        add_section(&mut env, Section::Photos, dump);
        let w = validate(&env);
        assert!(w.iter().any(|s| s.contains("missing item id 99")));
    }
    #[test] fn duplicate_path_warns() {
        let mut env = empty();
        let dump = SectionDump {
            items: vec![item(1, "/a.jpg"), item(2, "/a.jpg")],
            relationships: vec![],
        };
        add_section(&mut env, Section::Music, dump);
        assert!(validate(&env).iter().any(|s| s.contains("duplicate abs_path")));
    }
    #[test] fn format_version_mismatch_warns() {
        let mut env = empty();
        env.format_version = 999;
        assert!(validate(&env).iter().any(|s| s.contains("format version mismatch")));
    }
}
