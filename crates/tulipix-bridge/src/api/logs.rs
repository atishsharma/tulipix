//! Settings › Data › Logs — the in-app log viewer.
//!
//! The rolling JSONL files under `<data>/logs/` are the same ones the
//! bug-report bundler collects; this is the surface that reads them back
//! without leaving the app. Lines come newest first, filtered by level and by
//! a substring that matches either the message or the target.
//!
//! Redaction is applied on the way *out* of the machine, not on the way to the
//! screen: the viewer shows real paths because it is your own computer and the
//! folder that failed to scan is the answer you came for. Copy runs the same
//! lines through `bug_report::redact`, because that is the trip that ends in
//! an issue tracker.

use tulipix_core::{bug_report, logging};

pub struct LogLine {
    pub secs: i64,
    /// "TRACE" | "DEBUG" | "INFO" | "WARN" | "ERROR".
    pub level: String,
    /// The emitting module, e.g. `tulipix_videos::anime`.
    pub target: String,
    pub message: String,
}

pub struct LogPage {
    pub lines: Vec<LogLine>,
    /// Log files on disk, whatever the filter matched — the viewer says how
    /// many days it is reading, and an empty page needs to distinguish "no
    /// logs yet" from "nothing matched".
    pub files: u32,
    /// The filter had more to give and `limit` cut it off.
    pub truncated: bool,
}

/// How many days back a single read will go. Rotation is daily and `prune`
/// keeps a fortnight, so this reads everything that is there.
const MAX_FILES: usize = 14;

fn matches(line: &logging::LogEntry, needle: &str) -> bool {
    needle.is_empty()
        || line.msg.to_lowercase().contains(needle)
        || line.target.to_lowercase().contains(needle)
}

fn read(level: &str, search: &str, limit: u32) -> (Vec<logging::LogEntry>, bool) {
    let needle = search.to_lowercase();
    // The level filter is cheap and lives in core; the one-box search is this
    // crate's idea of matching, so it reads without a limit and stops itself.
    let q = logging::LogQuery {
        level_at_least: (!level.is_empty()).then(|| level.to_string()),
        limit: None,
        ..Default::default()
    };
    let want = limit as usize;
    let mut out = Vec::with_capacity(want.min(512));
    let mut truncated = false;
    for entry in logging::query(&q, MAX_FILES) {
        if !matches(&entry, &needle) {
            continue;
        }
        if out.len() == want {
            truncated = true;
            break;
        }
        out.push(entry);
    }
    (out, truncated)
}

/// Newest first. `level` is one of TRACE/DEBUG/INFO/WARN/ERROR, or empty for
/// everything; `search` matches the message or the target, either case.
pub fn logs_read(level: String, search: String, limit: u32) -> LogPage {
    let (entries, truncated) = read(&level, &search, limit);
    LogPage {
        lines: entries
            .into_iter()
            .map(|e| LogLine {
                secs: e.ts as i64,
                level: e.level,
                target: e.target,
                message: e.msg,
            })
            .collect(),
        files: logging::list_logs(MAX_FILES).len() as u32,
        truncated,
    }
}

/// The same lines as one block of text, oldest first — the order a log reads
/// in — with every absolute path, address and secret-shaped blob replaced.
/// This is what the Copy button puts on the clipboard.
pub fn logs_copy(level: String, search: String, limit: u32) -> String {
    let (entries, _) = read(&level, &search, limit);
    let mut text = String::new();
    for e in entries.iter().rev() {
        text.push_str(&format!("{} {:<5} {} {}\n", e.ts, e.level, e.target, e.msg));
    }
    bug_report::redact(&text)
}

/// Delete every log file, and answer how many went.
///
/// `reopen` afterwards because the sink caches today's open handle: on Unix
/// the unlinked file stays writable, so without it every line logged for the
/// rest of the day would go to a deleted inode and the folder would look
/// permanently empty.
pub fn logs_clear() -> u32 {
    let mut gone = 0;
    for p in logging::list_logs(usize::MAX) {
        if std::fs::remove_file(&p).is_ok() {
            gone += 1;
        }
    }
    logging::LOG.reopen();
    gone
}
