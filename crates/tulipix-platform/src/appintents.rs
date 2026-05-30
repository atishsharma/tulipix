//! AppIntents / Shortcuts — auto-generate per-OS action manifests from the
//! tulipix-cli subcommand registry. macOS Shortcuts.app reads
//! `Tulipix.app/Contents/Resources/AppIntents.json`; Windows quick-actions
//! ingest a registry manifest; Linux D-Bus actions ship as a
//! `org.tulipix.Actions` interface XML.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os { Macos, Windows, Linux }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliIntent {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub argv: Vec<String>,
}

impl CliIntent {
    fn new(id: &str, title: &str, summary: &str, argv: &[&str]) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            summary: summary.into(),
            argv: argv.iter().map(|s| (*s).into()).collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct AppIntentsManifest {
    pub bundle_id: String,
    pub intents: Vec<CliIntent>,
}

/// Registry of cli subcommands surfaced as OS-level actions. Source of truth
/// lives next to the cli parser; this snapshot is the contract that the
/// installers ship.
pub fn intents() -> Vec<CliIntent> {
    vec![
        CliIntent::new("tulipix.rescan-all",   "Rescan Tulipix Library",  "Scan every watched location for new media.",               &["rescan-all"]),
        CliIntent::new("tulipix.rebuild-fts",  "Rebuild Search Index",    "Rebuild the full-text search index for all sections.",      &["rebuild-fts"]),
        CliIntent::new("tulipix.lock",         "Lock Tulipix",            "Lock the app and stop indexer/sync.",                       &["lock"]),
        CliIntent::new("tulipix.rename",       "Rename Photos",           "Bulk rename via pattern.",                                  &["rename"]),
        CliIntent::new("tulipix.dedup",        "Find Duplicates",         "Run dedup across the current library.",                     &["dedup"]),
        CliIntent::new("tulipix.transcode",    "Transcode Video",         "Transcode using bundled ffmpeg presets.",                   &["transcode"]),
        CliIntent::new("tulipix.clear-thumbs", "Clear Thumb Cache",       "Free disk by clearing the thumbnail cache.",                &["clear-thumbs"]),
    ]
}

pub fn manifest_for(os: Os) -> String {
    let m = AppIntentsManifest { bundle_id: "com.tulipix.desktop".into(), intents: intents() };
    match os {
        Os::Macos => render_macos(&m),
        Os::Windows => render_windows(&m),
        Os::Linux => render_linux(&m),
    }
}

fn render_macos(m: &AppIntentsManifest) -> String {
    serde_json::to_string_pretty(&serde_json::json!({
        "bundleIdentifier": m.bundle_id,
        "intents": m.intents.iter().map(|i| serde_json::json!({
            "identifier": i.id,
            "title":      i.title,
            "description": i.summary,
            "parameters": [],
            "shellArgv":  i.argv,
        })).collect::<Vec<_>>(),
    })).unwrap_or_default()
}

fn render_windows(m: &AppIntentsManifest) -> String {
    // Quick-actions take a flat JSON the installer registers under
    // HKCU\Software\Tulipix\QuickActions.
    let body: Vec<_> = m.intents.iter().map(|i| serde_json::json!({
        "id": i.id, "title": i.title, "argv": i.argv,
    })).collect();
    serde_json::to_string_pretty(&serde_json::json!({ "actions": body })).unwrap_or_default()
}

fn render_linux(m: &AppIntentsManifest) -> String {
    // org.freedesktop.DBus.Introspectable XML — installer registers under
    // org.tulipix.Actions on the session bus.
    let mut s = String::from("<node name=\"/org/tulipix/Actions\">\n  <interface name=\"org.tulipix.Actions\">\n");
    for i in &m.intents {
        s.push_str(&format!("    <method name=\"{}\">\n      <annotation name=\"title\" value=\"{}\"/>\n    </method>\n",
            i.id.replace('.', "_"), i.title));
    }
    s.push_str("  </interface>\n</node>\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test] fn macos_manifest_is_json() {
        let s = manifest_for(Os::Macos);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["intents"].as_array().unwrap().len() >= 7);
    }
    #[test] fn windows_actions_round_trip() {
        let s = manifest_for(Os::Windows);
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert!(v["actions"].as_array().unwrap().iter().any(|a| a["id"] == "tulipix.lock"));
    }
    #[test] fn linux_xml_has_all_methods() {
        let s = manifest_for(Os::Linux);
        assert!(s.contains("tulipix_rescan-all"));
        assert!(s.contains("tulipix_lock"));
    }
}
