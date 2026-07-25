//! File-manager integration — Reveal, Open-with, Drag-out promises,
//! Move-to-Trash, and per-MIME user defaults.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

// ─── Escaping for the few places a script string is unavoidable ──────────────
//
// Filenames are attacker-controlled in the ordinary case — a download, a shared
// drive, a synced cloud folder — so anything interpolated into a script must be
// escaped for that script's rules. These helpers are compiled and tested on
// every platform even though each is called from a single per-OS branch: an
// escaping bug here is a command-execution bug, and it should not be
// discoverable only on the OS that ships it.

/// Escape for a double-quoted AppleScript literal, where `"` and `\` are live.
/// Backslash first, or it escapes the escapes.
#[allow(dead_code)]
fn applescript_quote(s: &str) -> String {
    s.replace('\\', r"\\").replace('"', "\\\"")
}

/// Escape for a single-quoted PowerShell literal. Such a string expands
/// nothing, so the quote is the only metacharacter, and doubling it is
/// PowerShell's escape.
#[allow(dead_code)]
fn powershell_quote(s: &str) -> String {
    s.replace('\'', "''")
}

/// Whether a token may be interpolated into a `cmd.exe` command line.
///
/// cmd re-parses its command line *after* argv splitting and offers no quoting
/// that reliably survives it, so escaping is not an option: only inert tokens
/// may go through. `extra` lists punctuation allowed beyond ASCII alphanumerics.
#[allow(dead_code)]
fn cmd_token_ok(s: &str, extra: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(c))
}

/// Reveal a path in the OS file manager (file selected, not just folder opened).
pub fn reveal_in_file_manager(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).status()?;
        return Ok(());
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // `explorer /select,<path>` selects the file in its folder. Two Windows
        // quirks to work around:
        //  1. Passed as a normal arg, Rust quotes it when the path has spaces
        //     ("…\Artist - Song.opus"), and explorer then fails to parse the
        //     switch and opens a *default* folder (the "random directory" bug).
        //     `raw_arg` writes the command line verbatim so the quoting is ours.
        //  2. explorer almost always exits non-zero even on success, so the exit
        //     status is deliberately ignored rather than surfaced as an error.
        let mut c = Command::new("explorer");
        c.raw_arg(format!("/select,\"{}\"", path.display()));
        let _ = c.status();
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        let uri = format!("file://{}", path.display());
        let dbus = Command::new("dbus-send")
            .args([
                "--session",
                "--print-reply",
                "--dest=org.freedesktop.FileManager1",
                "--type=method_call",
                "/org/freedesktop/FileManager1",
                "org.freedesktop.FileManager1.ShowItems",
                &format!("array:string:{uri}"),
                "string:",
            ])
            .status();
        if matches!(dbus, Ok(s) if s.success()) { return Ok(()); }
        let parent = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(parent).status()?;
        Ok(())
    }
}

/// Open a file with the OS default app.
pub fn open_default(path: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    { Command::new("open").arg(path).status()?; }
    // `cmd /C start "" <path>` used to run this. cmd re-parses its command line
    // after argv splitting, so a file named `a&calc.mp3` — an ordinary name for
    // a download or a file off a shared drive — ran `calc`. explorer.exe opens
    // the same default-app path but receives the argument through CreateProcess
    // with no shell to reinterpret it.
    #[cfg(target_os = "windows")]
    { Command::new("explorer.exe").arg(path).status()?; }
    #[cfg(target_os = "linux")]
    { Command::new("xdg-open").arg(path).status()?; }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenWithApp { pub name: String, pub exec: Option<PathBuf>, pub mime: String }

/// Enumerate apps registered for a file's MIME type.
/// Per-OS: LaunchServices (`lsregister -dump`), `ftype` (`assoc`) on Windows,
/// `xdg-mime` + `.desktop` scanning on Linux. Each branch is best-effort and
/// degrades to `System default` when the host tool is missing.
pub fn enumerate_open_with(path: &Path) -> Vec<OpenWithApp> {
    let mime = guess_mime(path);
    let mut apps = vec![OpenWithApp { name: "System default".into(), exec: None, mime: mime.clone() }];

    #[cfg(target_os = "macos")]
    {
        let out = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister")
            .args(["-dump", "-h"]).output();
        if let Ok(out) = out {
            for line in String::from_utf8_lossy(&out.stdout).lines() {
                if let Some(rest) = line.trim().strip_prefix("path:") {
                    let p = PathBuf::from(rest.trim());
                    if p.extension().and_then(|s| s.to_str()) == Some("app") {
                        let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or("App").to_string();
                        apps.push(OpenWithApp { name, exec: Some(p), mime: mime.clone() });
                    }
                }
            }
        }
    }
    #[cfg(target_os = "linux")]
    {
        let out = Command::new("xdg-mime").args(["query", "default", &mime]).output();
        if let Ok(out) = out {
            let desktop = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !desktop.is_empty() {
                for dir in ["/usr/share/applications", "/usr/local/share/applications"] {
                    let p = Path::new(dir).join(&desktop);
                    if p.exists() {
                        let name = desktop.trim_end_matches(".desktop").to_string();
                        apps.push(OpenWithApp { name, exec: Some(p), mime: mime.clone() });
                    }
                }
            }
        }
    }
    #[cfg(target_os = "windows")]
    {
        // `cmd /c assoc .<ext>` → `.<ext>=PerceivedType`; `ftype <key>` → exec.
        // Both are cmd builtins, so cmd is unavoidable here — which means both
        // interpolated values must be inert first. An extension is whatever
        // follows the last dot in a filename the user did not choose, so
        // `x.mp4&calc` would otherwise run `calc`.
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        if cmd_token_ok(ext, "") {
            let assoc = Command::new("cmd").args(["/C", &format!("assoc .{ext}")]).output();
            if let Ok(a) = assoc {
                if let Some(progid) = String::from_utf8_lossy(&a.stdout).split('=').nth(1)
                    .map(|s| s.trim().to_string())
                    // The ProgID comes back from the registry, which a local
                    // installer writes — hold it to the same bar before it goes
                    // back through cmd.
                    .filter(|p| cmd_token_ok(p, ".-_"))
                {
                    let ft = Command::new("cmd").args(["/C", &format!("ftype {progid}")]).output();
                    if let Ok(ft) = ft {
                        let exec = String::from_utf8_lossy(&ft.stdout).split('=').nth(1).map(|s| s.trim().to_string()).unwrap_or_default();
                        if !exec.is_empty() {
                            apps.push(OpenWithApp { name: progid, exec: Some(PathBuf::from(exec)), mime: mime.clone() });
                        }
                    }
                }
            }
        }
    }
    dedupe(&mut apps);
    apps
}

fn dedupe(apps: &mut Vec<OpenWithApp>) {
    let mut seen = std::collections::HashSet::new();
    apps.retain(|a| seen.insert(a.name.clone()));
}

pub fn guess_mime(path: &Path) -> String {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png"  => "image/png",
        "webp" => "image/webp",
        "mp4"  => "video/mp4",
        "mkv"  => "video/x-matroska",
        "mp3"  => "audio/mpeg",
        "flac" => "audio/flac",
        "epub" => "application/epub+zip",
        "pdf"  => "application/pdf",
        _      => "application/octet-stream",
    }.into()
}

// ─── User-default open-with persistence ─────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OpenWithDefaults {
    /// MIME → exec path. Tulipix uses this in preference to the OS default
    /// when the user has explicitly picked an app from the Open-with menu.
    pub by_mime: BTreeMap<String, PathBuf>,
}

impl OpenWithDefaults {
    pub fn set(&mut self, mime: impl Into<String>, exec: PathBuf) { self.by_mime.insert(mime.into(), exec); }
    pub fn clear(&mut self, mime: &str) { self.by_mime.remove(mime); }
    pub fn lookup(&self, mime: &str) -> Option<&PathBuf> { self.by_mime.get(mime) }
}

pub fn open_with(path: &Path, app_exec: &Path) -> Result<()> {
    if !app_exec.exists() { return Err(anyhow!("open-with target not found: {}", app_exec.display())); }
    #[cfg(target_os = "macos")]
    { Command::new("open").args(["-a", &app_exec.display().to_string(), &path.display().to_string()]).status()?; }
    #[cfg(target_os = "linux")]
    { Command::new(app_exec).arg(path).status()?; }
    #[cfg(target_os = "windows")]
    { Command::new(app_exec).arg(path).status()?; }
    Ok(())
}

// ─── Drag-out promises ──────────────────────────────────────────────────

/// Drag-out promise: returns file:// URIs (zero-copy).
pub fn drag_uris(paths: &[PathBuf]) -> Vec<String> {
    paths.iter().map(|p| format!("file://{}", p.display())).collect()
}

#[derive(Debug, Clone)]
pub struct PasteboardPayload {
    /// `text/uri-list` payload bytes — Linux drag protocol + clipboard fallback.
    pub uri_list: Vec<u8>,
    /// Concatenated NUL-terminated wide-char block — Windows `CF_HDROP` DROPFILES tail.
    pub cf_hdrop_paths: Vec<PathBuf>,
    /// JSON array of file URIs — macOS NSPasteboard `public.file-url` driver
    /// re-encodes each entry into an `NSURL`.
    pub ns_pasteboard_json: Vec<u8>,
}

pub fn drag_payload(paths: &[PathBuf]) -> PasteboardPayload {
    let uri_list_text: String = paths.iter().map(|p| format!("file://{}\r\n", p.display())).collect();
    let ns_json = serde_json::to_vec(&paths.iter().map(|p| format!("file://{}", p.display())).collect::<Vec<_>>())
        .unwrap_or_default();
    PasteboardPayload {
        uri_list: uri_list_text.into_bytes(),
        cf_hdrop_paths: paths.to_vec(),
        ns_pasteboard_json: ns_json,
    }
}

// ─── Trash ──────────────────────────────────────────────────────────────

/// Move to OS-native Trash. Best-effort per-OS via subprocess so we don't
/// pull in the `trash` crate (and its libdbus dep on Linux). Returns
/// `Err` when the OS-native trash tool is missing.
pub fn move_to_trash(path: &Path) -> Result<()> {
    if !path.exists() { return Err(anyhow!("trash target missing: {}", path.display())); }
    #[cfg(target_os = "macos")]
    {
        // Without escaping, a file named `x" & (do shell script "…") & "`
        // closes the literal and runs whatever it likes.
        let quoted = applescript_quote(&path.display().to_string());
        let osa = format!("tell application \"Finder\" to delete POSIX file \"{quoted}\"");
        let status = Command::new("osascript").args(["-e", &osa]).status()?;
        if !status.success() { return Err(anyhow!("osascript trash failed")); }
        return Ok(());
    }
    #[cfg(target_os = "linux")]
    {
        // gio trash (GNOME) → kioclient5 mv … trash:/ (KDE) fallback.
        let g = Command::new("gio").args(["trash", &path.display().to_string()]).status();
        if matches!(g, Ok(s) if s.success()) { return Ok(()); }
        let k = Command::new("kioclient5").args(["mv", &path.display().to_string(), "trash:/"]).status();
        if matches!(k, Ok(s) if s.success()) { return Ok(()); }
        return Err(anyhow!("no trash tool found (gio / kioclient5)"));
    }
    #[cfg(target_os = "windows")]
    {
        // PowerShell shell COM call. The `recyclebin` verb deletes via the Explorer
        // shell so the file lands in Recycle Bin with full restore metadata.
        //
        // Without escaping, a file named `x'; calc; '.mp3` closed the literal and
        // ran arbitrary commands.
        //
        // SHFileOperationW with FOF_ALLOWUNDO would remove the shell from this
        // path entirely; it needs the Win32_UI_Shell feature on the `windows`
        // crate and careful double-NUL buffer handling.
        let ps = format!(
            "Add-Type -AssemblyName Microsoft.VisualBasic; [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile('{}', 'OnlyErrorDialogs', 'SendToRecycleBin')",
            powershell_quote(&path.display().to_string())
        );
        let status = Command::new("powershell").args(["-NoProfile", "-Command", &ps]).status()?;
        if !status.success() { return Err(anyhow!("PowerShell recycle bin call failed")); }
        return Ok(());
    }
    #[allow(unreachable_code)]
    Err(anyhow!("trash unsupported on this target"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn drag_uris_format() {
        let v = drag_uris(&[PathBuf::from("/a/b.jpg")]);
        assert_eq!(v, vec!["file:///a/b.jpg".to_string()]);
    }
    #[test] fn drag_payload_emits_all_three_envelopes() {
        let payload = drag_payload(&[PathBuf::from("/a.jpg"), PathBuf::from("/b.jpg")]);
        let uri_list = String::from_utf8(payload.uri_list).unwrap();
        assert!(uri_list.contains("file:///a.jpg\r\n"));
        assert!(uri_list.contains("file:///b.jpg\r\n"));
        assert_eq!(payload.cf_hdrop_paths.len(), 2);
        let json: Vec<String> = serde_json::from_slice(&payload.ns_pasteboard_json).unwrap();
        assert_eq!(json, vec!["file:///a.jpg".to_string(), "file:///b.jpg".to_string()]);
    }
    #[test] fn guess_mime_table() {
        assert_eq!(guess_mime(Path::new("/x.JPG")), "image/jpeg");
        assert_eq!(guess_mime(Path::new("/x.unknown")), "application/octet-stream");
    }
    #[test] fn defaults_round_trip() {
        let mut d = OpenWithDefaults::default();
        d.set("image/jpeg", PathBuf::from("/usr/bin/gthumb"));
        assert_eq!(d.lookup("image/jpeg"), Some(&PathBuf::from("/usr/bin/gthumb")));
        d.clear("image/jpeg");
        assert!(d.lookup("image/jpeg").is_none());
    }
    #[test] fn dedupe_keeps_first_occurrence() {
        let mut v = vec![
            OpenWithApp { name: "A".into(), exec: None, mime: "x".into() },
            OpenWithApp { name: "A".into(), exec: Some(PathBuf::from("/y")), mime: "x".into() },
            OpenWithApp { name: "B".into(), exec: None, mime: "x".into() },
        ];
        dedupe(&mut v);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "A");
        assert_eq!(v[1].name, "B");
    }
    #[test] fn enumerate_open_with_includes_system_default() {
        let apps = enumerate_open_with(Path::new("/tmp/x.jpg"));
        assert!(!apps.is_empty());
        assert_eq!(apps[0].name, "System default");
    }

    // The trash paths hand a filename to a script interpreter, so a name that
    // closes the literal used to run commands. These run on every platform, not
    // just the one whose branch calls them.
    /// Quotes that actually terminate a literal — escaped ones skipped, and a
    /// `\\` pair consumed so it cannot be mistaken for an escape.
    fn unescaped_quotes(s: &str) -> usize {
        let b = s.as_bytes();
        let (mut n, mut i) = (0, 0);
        while i < b.len() {
            match b[i] {
                b'\\' => { i += 2; continue; }
                b'"' => n += 1,
                _ => {}
            }
            i += 1;
        }
        n
    }

    #[test] fn applescript_quote_neutralises_a_breakout_filename() {
        let evil = r#"x" & (do shell script "calc") & ""#;
        assert_eq!(unescaped_quotes(evil), 4, "payload should break out unescaped");
        // Escaped, none of its quotes can close the literal.
        assert_eq!(unescaped_quotes(&applescript_quote(evil)), 0);
        // Assembled, only the two quote pairs the template itself adds remain.
        let script = format!(
            "tell application \"Finder\" to delete POSIX file \"{}\"",
            applescript_quote(evil)
        );
        assert_eq!(unescaped_quotes(&script), 4, "{script}");
    }

    #[test] fn applescript_quote_escapes_backslashes_before_quotes() {
        // A trailing backslash must not end up escaping the closing quote the
        // caller appends.
        assert_eq!(applescript_quote(r"C:\dir\"), r"C:\\dir\\");
        assert_eq!(applescript_quote(r#"a\"b"#), r#"a\\\"b"#);
    }

    #[test] fn powershell_quote_doubles_single_quotes() {
        assert_eq!(powershell_quote("x'; calc; '.mp3"), "x''; calc; ''.mp3");
        assert_eq!(powershell_quote("plain.mp3"), "plain.mp3");
    }

    #[test] fn cmd_token_rejects_metacharacters() {
        assert!(cmd_token_ok("mp4", ""));
        assert!(!cmd_token_ok("mp4&calc", ""));
        assert!(!cmd_token_ok("mp4|calc", ""));
        assert!(!cmd_token_ok("", ""), "empty must not reach cmd");
        // ProgIDs legitimately carry dots, dashes and underscores.
        assert!(cmd_token_ok("WMP11.AssocFile.MP4", ".-_"));
        assert!(!cmd_token_ok("WMP11.AssocFile.MP4&calc", ".-_"));
    }
}
