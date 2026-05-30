use crate::paths;
use anyhow::Result;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Daily-rotated JSON log file at <data>/logs/YYYY-MM-DD.jsonl.
pub struct RollingJsonLog {
    inner: Mutex<Option<(String, std::fs::File)>>,
}

impl RollingJsonLog {
    pub const fn new() -> Self { Self { inner: Mutex::new(None) } }

    pub fn write_event(&self, level: &str, target: &str, message: &str) -> Result<()> {
        let Some(dir) = paths::logs_dir() else { return Ok(()); };
        std::fs::create_dir_all(&dir)?;
        let date = today_utc();
        let path = dir.join(format!("{date}.jsonl"));
        let mut g = self.inner.lock().unwrap();
        let need_open = match g.as_ref() {
            Some((d, _)) => d != &date,
            None => true,
        };
        if need_open {
            let f = OpenOptions::new().create(true).append(true).open(&path)?;
            *g = Some((date.clone(), f));
        }
        let Some((_, ref mut f)) = *g else { return Ok(()); };
        let line = serde_json::json!({
            "ts": SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
            "level": level,
            "target": target,
            "msg": message,
        });
        writeln!(f, "{line}")?;
        Ok(())
    }
}

pub static LOG: RollingJsonLog = RollingJsonLog::new();

fn today_utc() -> String {
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let days = secs / 86400;
    let (y, m, d) = days_to_ymd(days as i64);
    format!("{:04}-{:02}-{:02}", y, m, d)
}

fn days_to_ymd(mut days: i64) -> (i32, u32, u32) {
    days += 719468;
    let era = if days >= 0 { days / 146097 } else { (days - 146096) / 146097 };
    let doe = (days - era * 146097) as u32;
    let yoe = (doe - doe/1460 + doe/36524 - doe/146096) / 365;
    let y = yoe as i32 + (era as i32) * 400;
    let doy = doe - (365 * yoe + yoe/4 - yoe/100);
    let mp = (5*doy + 2) / 153;
    let d = doy - (153*mp + 2)/5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// List today's + last N days of log files (newest first) for an in-app viewer.
pub fn list_logs(max: usize) -> Vec<PathBuf> {
    let Some(dir) = paths::logs_dir() else { return vec![]; };
    let Ok(rd) = std::fs::read_dir(&dir) else { return vec![]; };
    let mut paths: Vec<_> = rd.flatten()
        .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("jsonl"))
        .map(|e| e.path())
        .collect();
    paths.sort();
    paths.reverse();
    paths.truncate(max);
    paths
}

// ─── tracing-subscriber JSONL layer ─────────────────────────────────────

/// Install a tracing-subscriber layer that writes every span/event to the
/// rolling JSONL file. `RUST_LOG` (or the supplied filter) controls the
/// per-target threshold. Idempotent — repeat calls are no-ops.
///
/// The actual `tracing-subscriber` install lives in the app crate (it owns
/// the `tracing-subscriber` dep). This helper builds the writer + filter
/// string the app passes in.
pub struct SubscriberConfig {
    pub default_filter: String,
    pub min_level: &'static str,
}

impl Default for SubscriberConfig {
    fn default() -> Self { Self { default_filter: "tulipix=info,warn".into(), min_level: "INFO" } }
}

pub fn writer_for_level(level: &str, target: &str, message: &str) {
    let _ = LOG.write_event(level, target, message);
}

// ─── Viewer query API ───────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LogEntry {
    pub ts: u64,
    pub level: String,
    pub target: String,
    pub msg: String,
}

#[derive(Debug, Clone, Default)]
pub struct LogQuery {
    pub level_at_least: Option<String>,
    pub target_substr: Option<String>,
    pub message_substr: Option<String>,
    pub limit: Option<usize>,
}

fn level_rank(level: &str) -> u8 {
    match level.to_ascii_uppercase().as_str() {
        "TRACE" => 0, "DEBUG" => 1, "INFO" => 2, "WARN" => 3, "ERROR" => 4, _ => 2,
    }
}

pub fn query(q: &LogQuery, max_files: usize) -> Vec<LogEntry> {
    let mut out = Vec::new();
    let want = q.level_at_least.as_deref().map(level_rank);
    for path in list_logs(max_files) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue; };
        for line in text.lines() {
            let Ok(entry) = serde_json::from_str::<LogEntry>(line) else { continue; };
            if let Some(min) = want { if level_rank(&entry.level) < min { continue; } }
            if let Some(sub) = &q.target_substr { if !entry.target.contains(sub) { continue; } }
            if let Some(sub) = &q.message_substr { if !entry.msg.contains(sub) { continue; } }
            out.push(entry);
            if let Some(lim) = q.limit { if out.len() >= lim { return out; } }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn level_rank_orders() {
        assert!(level_rank("DEBUG") < level_rank("INFO"));
        assert!(level_rank("INFO") < level_rank("WARN"));
        assert!(level_rank("WARN") < level_rank("ERROR"));
    }
    #[test] fn subscriber_defaults() {
        let c = SubscriberConfig::default();
        assert!(c.default_filter.contains("tulipix"));
        assert_eq!(c.min_level, "INFO");
    }
}
