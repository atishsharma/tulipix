//! Crash reports, kept on this machine.
//!
//! A panic writes one JSON file into `<data>/crashes/`. Nothing is ever
//! uploaded: Settings › Data lists what is there, and Report opens a
//! pre-filled issue on the tracker in the browser, so the user posts it
//! themselves or not at all. The panic text and location go through the
//! bug-report redactor first, which is what strips absolute paths, emails
//! and secret-shaped blobs.

use crate::bug_report::redact;
use crate::paths;
use std::panic::PanicHookInfo;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Where the tracker lives. The Report button opens `.../issues/new` here.
pub const ISSUES_URL: &str = "https://github.com/atishsharma/tulipix/issues/new";

#[derive(Debug, Clone)]
pub struct CrashEntry {
    /// The file's stem, `crash-<unix secs>`. The only thing an action is
    /// allowed to name.
    pub id: String,
    pub secs: i64,
    /// The panic message, one line.
    pub headline: String,
    /// `file:line:col` of the panic, or empty.
    pub location: String,
    pub version: String,
    pub path: PathBuf,
}

pub fn dir() -> Option<PathBuf> {
    paths::data_dir().map(|d| d.join("crashes"))
}

pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Err(e) = write_crash_dump(info) {
            tracing::error!(error = %e, "write_crash_dump failed");
        }
        default(info);
    }));
}

/// The panic payload as a line. `panic!("{x}")` carries a `String` and
/// `panic!("literal")` a `&str`; missing the first left most crashes headed
/// "Tulipix crashed".
fn headline_of(info: &PanicHookInfo<'_>) -> String {
    let p = info.payload();
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "Tulipix crashed".into())
}

fn write_crash_dump(info: &PanicHookInfo<'_>) -> std::io::Result<()> {
    let Some(crash_dir) = dir() else { return Ok(()) };
    std::fs::create_dir_all(&crash_dir)?;
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = crash_dir.join(format!("crash-{secs}.json"));
    let dump = serde_json::json!({
        "ts": secs,
        "version": env!("CARGO_PKG_VERSION"),
        "headline": headline_of(info),
        "location": info.location().map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column())).unwrap_or_default(),
    });
    std::fs::write(&path, serde_json::to_vec_pretty(&dump)?)
}

/// What is in the crash folder, newest first. A file that is not a dump, or
/// is half-written, is left out rather than shown as a blank row.
pub fn list() -> Vec<CrashEntry> {
    let Some(crash_dir) = dir() else { return Vec::new() };
    let Ok(rd) = std::fs::read_dir(&crash_dir) else { return Vec::new() };
    let mut out: Vec<CrashEntry> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("json"))
        .filter_map(|p| {
            let id = p.file_stem()?.to_str()?.to_string();
            let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some(CrashEntry {
                secs: v["ts"].as_i64().unwrap_or(0),
                headline: v["headline"].as_str().unwrap_or("Tulipix crashed").to_string(),
                location: v["location"].as_str().unwrap_or_default().to_string(),
                version: v["version"].as_str().unwrap_or_default().to_string(),
                id,
                path: p,
            })
        })
        .collect();
    out.sort_by(|a, b| b.secs.cmp(&a.secs));
    out
}

pub fn find(id: &str) -> Option<CrashEntry> {
    list().into_iter().find(|c| c.id == id)
}

/// Delete every dump. Returns how many went.
pub fn clear() -> usize {
    list()
        .into_iter()
        .filter(|c| std::fs::remove_file(&c.path).is_ok())
        .count()
}

/// A pre-filled issue on the tracker: title, and a body holding the version,
/// the machine, the panic and where it happened. Redacted, because the body
/// is about to be pasted into a public issue.
pub fn issue_url(c: &CrashEntry) -> String {
    let title = format!("Crash: {}", one_line(&redact(&c.headline), 90));
    let body = format!(
        "**What I was doing:**\n\n\n---\n\
         Tulipix {} · {} {}\n\
         Panic: {}\n\
         At: {}\n",
        if c.version.is_empty() { env!("CARGO_PKG_VERSION") } else { &c.version },
        std::env::consts::OS,
        std::env::consts::ARCH,
        one_line(&redact(&c.headline), 400),
        one_line(&redact(&c.location), 200),
    );
    format!("{ISSUES_URL}?title={}&body={}", enc(&title), enc(&body))
}

fn one_line(s: &str, max: usize) -> String {
    let flat: String = s.trim().chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    match flat.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &flat[..i]),
        None => flat,
    }
}

/// Percent-encode a query value. Unreserved characters through, everything
/// else as %XX — a handful of lines rather than a dependency for two calls.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len() * 2);
    for b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(*b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enc_escapes_space_and_newline_and_keeps_unreserved() {
        assert_eq!(enc("a b"), "a%20b");
        assert_eq!(enc("a\nb"), "a%0Ab");
        assert_eq!(enc("Az0-_.~"), "Az0-_.~");
    }

    #[test]
    fn one_line_flattens_and_truncates() {
        assert_eq!(one_line("  a\nb  ", 90), "a b");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }

    #[test]
    fn issue_url_redacts_and_carries_both_fields() {
        let c = CrashEntry {
            id: "crash-1".into(),
            secs: 1,
            headline: "failed on /home/alice/secret.jpg".into(),
            location: "crates/x/src/y.rs:3:4".into(),
            version: "1.0.1".into(),
            path: PathBuf::from("/tmp/crash-1.json"),
        };
        let u = issue_url(&c);
        assert!(u.starts_with(ISSUES_URL));
        assert!(u.contains("title="), "{u}");
        assert!(u.contains("&body="), "{u}");
        // The home path never reaches the query.
        assert!(!u.contains("alice"), "{u}");
        assert!(u.contains(&enc("<path>")), "{u}");
    }
}
