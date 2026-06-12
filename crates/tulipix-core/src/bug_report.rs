//! Bug-report-with-logs bundler. Collects the last `window_days`
//! days of JSONL logs from `<data>/logs/`, strips obvious PII
//! (absolute paths, emails, API keys, bearer tokens, UUIDs and 32+
//! char hex/base64 blobs), prefixes a system-info block, and returns
//! the bundle as in-memory files. The app layer writes them to disk
//! or POSTs to a configurable endpoint.

use crate::logging;
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SystemInfo {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub locale: String,
    pub theme: String,
    pub cpu_count: usize,
    pub generated_at_unix: u64,
}

impl SystemInfo {
    pub fn detect() -> Self {
        Self {
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            locale: std::env::var("LANG").unwrap_or_default(),
            theme: String::new(),
            cpu_count: std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1),
            generated_at_unix: now_unix(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BugReport {
    pub system: SystemInfo,
    pub description: String,
    pub log_file_count: usize,
}

#[derive(Debug, Clone)]
pub struct BundleFile { pub name: String, pub bytes: Vec<u8> }

#[derive(Debug, Clone, Default)]
pub struct Bundle { pub files: Vec<BundleFile> }

pub fn collect(description: &str, window_days: u32) -> Result<Bundle> {
    let logs = logging::list_logs(64);
    let cutoff = now_unix().saturating_sub(window_days as u64 * 86_400);
    let mut keep: Vec<PathBuf> = Vec::new();
    for p in logs {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if stem_to_unix(stem).map(|t| t >= cutoff).unwrap_or(true) { keep.push(p); }
    }

    let mut files = Vec::with_capacity(keep.len() + 2);
    let report = BugReport {
        system: SystemInfo::detect(),
        description: redact(description),
        log_file_count: keep.len(),
    };
    files.push(BundleFile {
        name: "report.json".into(),
        bytes: serde_json::to_vec_pretty(&report)?,
    });

    for p in keep {
        let name = format!("logs/{}", p.file_name().and_then(|s| s.to_str()).unwrap_or("log.jsonl"));
        let bytes = std::fs::read(&p).unwrap_or_default();
        let scrubbed = redact_bytes(&bytes);
        files.push(BundleFile { name, bytes: scrubbed });
    }

    Ok(Bundle { files })
}

/// Submit the bundle to a configured endpoint. The caller chooses the
/// transport (multipart/form-data, base64-in-JSON, S3 presigned URL);
/// this signature only fixes the contract.
pub trait BundleSink: Send + Sync {
    fn submit(&self, endpoint: &str, bundle: &Bundle) -> Result<String>;
}

pub struct StubSink;
impl BundleSink for StubSink {
    fn submit(&self, _endpoint: &str, _bundle: &Bundle) -> Result<String> {
        Ok("stub-receipt".into())
    }
}

pub fn redact(input: &str) -> String { String::from_utf8_lossy(&redact_bytes(input.as_bytes())).into_owned() }

pub fn redact_bytes(input: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(input);
    let mut out = String::with_capacity(text.len());
    for line in text.lines() {
        out.push_str(&redact_line(line));
        out.push('\n');
    }
    out.into_bytes()
}

fn redact_line(line: &str) -> String {
    let mut s = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    let mut buf = String::new();
    for c in chars {
        if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.' || c == '/' || c == '\\' || c == ':' || c == '@' || c == '+' || c == '=' {
            buf.push(c);
            continue;
        }
        if !buf.is_empty() { s.push_str(&redact_token(&buf)); buf.clear(); }
        s.push(c);
    }
    if !buf.is_empty() { s.push_str(&redact_token(&buf)); }
    s
}

fn redact_token(tok: &str) -> String {
    // Email-shaped → <email>
    if tok.contains('@') && tok.contains('.') && tok.matches('@').count() == 1 {
        let (l, r) = tok.split_once('@').unwrap();
        if !l.is_empty() && r.contains('.') { return "<email>".into(); }
    }
    // Absolute paths → <path>
    if tok.starts_with('/') || (tok.len() >= 3 && &tok.as_bytes()[1..3] == b":\\") || tok.starts_with("\\\\") {
        return "<path>".into();
    }
    // Long secret-shaped blobs (hex/base64-ish, 32+ chars, no dots) → <redacted>
    if tok.len() >= 32 && !tok.contains('.') && tok.chars().all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=' || c == '-' || c == '_') {
        return "<redacted>".into();
    }
    tok.to_string()
}

fn stem_to_unix(stem: &str) -> Option<u64> {
    // stem is YYYY-MM-DD
    let mut parts = stem.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() { return None; }
    // civil-from-days (inverse of logging::days_to_ymd) — good enough for cutoff math.
    let y2 = if m <= 2 { y - 1 } else { y };
    let era = if y2 >= 0 { y2 } else { y2 - 399 } / 400;
    let yoe = (y2 - era * 400);
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86_400) as u64)
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn redacts_email_and_path_and_secret() {
        let s = redact("user alice@example.com hit /home/alice/foo.jpg with token abcdefABCDEF0123456789abcdef0123");
        assert!(s.contains("<email>"));
        assert!(s.contains("<path>"));
        assert!(s.contains("<redacted>"));
        assert!(!s.contains("alice@example.com"));
        assert!(!s.contains("/home/alice"));
    }
    #[test] fn keeps_short_words_intact() {
        let s = redact("hello world");
        assert_eq!(s.trim(), "hello world");
    }
    #[test] fn windows_path_redacted() {
        let s = redact(r"C:\Users\bob\file.txt");
        assert!(s.contains("<path>"));
    }
    #[test] fn collect_builds_report_json() {
        let b = collect("phone in case it matters: alice@example.com", 7).unwrap();
        let report = b.files.iter().find(|f| f.name == "report.json").unwrap();
        let parsed: serde_json::Value = serde_json::from_slice(&report.bytes).unwrap();
        assert!(parsed["description"].as_str().unwrap().contains("<email>"));
        assert!(parsed["system"]["os"].is_string());
    }
    #[test] fn stub_sink_returns_receipt() {
        let b = Bundle::default();
        assert_eq!(StubSink.submit("http://x", &b).unwrap(), "stub-receipt");
    }
    #[test] fn stem_to_unix_round_trip_2024_01_01() {
        // 2024-01-01 = 19723 days since 1970-01-01 = 1704067200
        assert_eq!(stem_to_unix("2024-01-01"), Some(1_704_067_200));
    }
}
