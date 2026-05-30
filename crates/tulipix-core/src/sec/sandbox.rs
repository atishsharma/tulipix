//! OS sandbox compliance. Tulipix targets first-party stores so the
//! installer ships the strictest sandbox manifest each OS supports.
//!
//!   * macOS:    `com.apple.security.app-sandbox` + security-scoped
//!               bookmarks for user-chosen folders.
//!   * Windows:  AppContainer manifest with `runFullTrust` only for the
//!               packaged ffmpeg launcher (signed by us).
//!   * Linux:    Flatpak portal-only manifest (no `--filesystem=host`).
//!
//! The render functions emit the per-OS manifest from a single source so the
//! manifest can't drift between platforms.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct SandboxSpec {
    pub bundle_id: &'static str,
    pub needs_camera: bool,
    pub needs_mic: bool,
    pub needs_network: bool,
}

pub const SPEC: SandboxSpec = SandboxSpec {
    bundle_id: "com.tulipix.desktop",
    needs_camera: false,
    needs_mic: true,            // voice search (cap-gated ai.voice)
    needs_network: true,        // optional sync + AI cloud offload
};

pub fn macos_entitlements(spec: &SandboxSpec) -> String {
    let mut keys = String::new();
    let mut add = |k: &str, v: bool| {
        keys.push_str(&format!("    <key>{k}</key>\n    <{}/>\n", if v { "true" } else { "false" }));
    };
    add("com.apple.security.app-sandbox", true);
    add("com.apple.security.files.user-selected.read-write", true);
    add("com.apple.security.files.bookmarks.app-scope", true);
    add("com.apple.security.device.audio-input", spec.needs_mic);
    add("com.apple.security.device.camera", spec.needs_camera);
    add("com.apple.security.network.client", spec.needs_network);
    add("com.apple.security.network.server", false);
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"https://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n  <dict>\n{keys}  </dict>\n</plist>\n")
}

pub fn windows_appx_manifest(spec: &SandboxSpec) -> String {
    let caps = [
        ("internetClient", spec.needs_network),
        ("microphone",     spec.needs_mic),
        ("webcam",         spec.needs_camera),
        ("removableStorage", true),
        ("picturesLibrary",  true),
        ("videosLibrary",    true),
        ("musicLibrary",     true),
    ];
    let mut s = String::new();
    s.push_str("<Package xmlns=\"http://schemas.microsoft.com/appx/manifest/foundation/windows10\">\n");
    s.push_str(&format!("  <Identity Name=\"{}\"/>\n", spec.bundle_id));
    s.push_str("  <Capabilities>\n");
    for (c, on) in caps {
        if on { s.push_str(&format!("    <Capability Name=\"{c}\"/>\n")); }
    }
    s.push_str("  </Capabilities>\n</Package>\n");
    s
}

pub fn flatpak_manifest(spec: &SandboxSpec) -> String {
    let mut finish_args = vec![
        "--share=ipc", "--socket=wayland", "--socket=fallback-x11",
        "--device=dri", "--filesystem=xdg-pictures", "--filesystem=xdg-videos",
        "--filesystem=xdg-music", "--filesystem=xdg-documents",
        "--talk-name=org.freedesktop.portal.*",
    ];
    if spec.needs_network { finish_args.push("--share=network"); }
    if spec.needs_mic     { finish_args.push("--socket=pulseaudio"); }
    serde_json::to_string_pretty(&serde_json::json!({
        "id": spec.bundle_id,
        "runtime": "org.freedesktop.Platform",
        "runtime-version": "24.08",
        "sdk": "org.freedesktop.Sdk",
        "command": "tulipix",
        "finish-args": finish_args,
    })).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn macos_has_app_sandbox_key() {
        let s = macos_entitlements(&SPEC);
        assert!(s.contains("com.apple.security.app-sandbox"));
        assert!(s.contains("audio-input"));
        assert!(!s.contains("com.apple.security.network.server</key>\n    <true"));
    }
    #[test] fn appx_lists_picture_lib() {
        let s = windows_appx_manifest(&SPEC);
        assert!(s.contains("picturesLibrary"));
        assert!(s.contains("microphone"));
    }
    #[test] fn flatpak_no_host_filesystem() {
        let s = flatpak_manifest(&SPEC);
        assert!(s.contains("xdg-pictures"));
        assert!(!s.contains("--filesystem=host"));
    }
}
