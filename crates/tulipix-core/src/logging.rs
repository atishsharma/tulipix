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

impl Default for RollingJsonLog {
    fn default() -> Self {
        Self::new()
    }
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

    /// Drop the cached handle so the next line opens the file afresh. Deleting
    /// the logs leaves this struct holding an unlinked inode that still accepts
    /// writes, which would make the folder look empty for the rest of the day.
    pub fn reopen(&self) {
        *self.inner.lock().unwrap() = None;
    }
}

pub static LOG: RollingJsonLog = RollingJsonLog::new();

fn today_utc() -> String { date_utc(today_days()) }

fn today_days() -> i64 {
    (SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 86400) as i64
}

fn date_utc(days: i64) -> String {
    let (y, m, d) = days_to_ymd(days);
    format!("{y:04}-{m:02}-{d:02}")
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

/// Delete log files older than `keep_days`, and answer how many went.
///
/// Rotation is daily and nothing else ever removes a file, so without this
/// `<data>/logs/` grows for the life of the install. Called once at startup.
pub fn prune(keep_days: u32) -> usize {
    let cutoff = date_utc(today_days() - i64::from(keep_days));
    let mut gone = 0;
    for p in list_logs(usize::MAX) {
        let stem = p.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        // `YYYY-MM-DD` sorts lexicographically exactly as it sorts in time,
        // which is the whole reason the files are named that way -- no date
        // parsing, and `list_logs`'s plain `sort()` is already chronological.
        if stem.len() == 10 && stem.as_bytes()[4] == b'-' && stem < cutoff.as_str()
            && std::fs::remove_file(&p).is_ok()
        {
            gone += 1;
        }
    }
    gone
}

// ─── tracing-subscriber JSONL layer ─────────────────────────────────────

/// Copies every event the filter admits into the rolling JSONL file.
///
/// A second sink beside the console formatter, not a replacement: `fmt` keeps
/// stdout and this keeps the file that Settings › Data › Logs and the
/// bug-report bundler both read. Until this existed both read an empty folder
/// -- the sink was written, and nothing was wired to it.
///
/// ponytail: the write is synchronous, one `writeln!` under a mutex on the
/// thread that logged. At INFO that is a handful of lines a minute. If a
/// debug-level filter ever ships by default, put a channel and a writer
/// thread between the two.
pub struct FileLayer;

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for FileLayer {
    fn on_event(&self, event: &tracing::Event<'_>, _: tracing_subscriber::layer::Context<'_, S>) {
        let mut line = Line::default();
        event.record(&mut line);
        let meta = event.metadata();
        let _ = LOG.write_event(meta.level().as_str(), meta.target(), &line.finish());
    }
}

/// Flattens an event's fields into one line: the message first, then
/// `key=value` for the rest -- which is where `warn!(error = %e, "…")` keeps
/// the half that says what actually went wrong.
#[derive(Default)]
struct Line {
    message: String,
    fields: String,
}

impl Line {
    fn push(&mut self, name: &str, value: std::fmt::Arguments<'_>) {
        use std::fmt::Write;
        if name == "message" {
            let _ = write!(&mut self.message, "{value}");
        } else {
            let _ = write!(&mut self.fields, " {name}={value}");
        }
    }

    fn finish(mut self) -> String {
        self.message.push_str(&self.fields);
        self.message
    }
}

impl tracing::field::Visit for Line {
    fn record_debug(&mut self, f: &tracing::field::Field, v: &dyn std::fmt::Debug) {
        self.push(f.name(), format_args!("{v:?}"));
    }

    // Without this the default forwards to `record_debug`, which puts quotes
    // around every string field.
    fn record_str(&mut self, f: &tracing::field::Field, v: &str) {
        self.push(f.name(), format_args!("{v}"));
    }
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

/// Newest first, and case-insensitive on both substrings -- nobody searching
/// their own log remembers which half of `AniDB` was capitalised.
pub fn query(q: &LogQuery, max_files: usize) -> Vec<LogEntry> {
    let mut out = Vec::new();
    let want = q.level_at_least.as_deref().map(level_rank);
    let target_sub = q.target_substr.as_deref().map(str::to_lowercase);
    let msg_sub = q.message_substr.as_deref().map(str::to_lowercase);
    'files: for path in list_logs(max_files) {
        let Ok(text) = std::fs::read_to_string(&path) else { continue; };
        // Files arrive newest first; the lines inside one are oldest first, so
        // the rewind makes the whole walk newest first -- without it a `limit`
        // keeps the wrong end and the viewer opens on last week.
        for line in text.lines().rev() {
            let Ok(entry) = serde_json::from_str::<LogEntry>(line) else { continue; };
            if let Some(min) = want { if level_rank(&entry.level) < min { continue; } }
            if let Some(sub) = &target_sub { if !entry.target.to_lowercase().contains(sub) { continue; } }
            if let Some(sub) = &msg_sub { if !entry.msg.to_lowercase().contains(sub) { continue; } }
            out.push(entry);
            if let Some(lim) = q.limit { if out.len() >= lim { break 'files; } }
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
    /// What `prune` leans on instead of parsing a date: `YYYY-MM-DD` compares
    /// as text in the same order it compares as time, across month and year
    /// ends both.
    #[test] fn dates_sort_as_text_in_time_order() {
        let mut days: Vec<i64> = vec![0, 1, 59, 60, 19_000, 19_723, 20_000];
        days.sort();
        let dates: Vec<String> = days.iter().map(|d| date_utc(*d)).collect();
        let mut sorted = dates.clone();
        sorted.sort();
        assert_eq!(dates, sorted, "{dates:?}");
        assert_eq!(date_utc(0), "1970-01-01");
        assert_eq!(date_utc(19_723), "2024-01-01");
    }

    #[test] fn a_cutoff_is_keep_days_back() {
        let today = today_days();
        assert_eq!(date_utc(today), today_utc());
        assert!(date_utc(today - 30) < today_utc());
    }

    /// The message leads and the fields follow, so `warn!(error = %e, "…")`
    /// keeps the half that names the failure.
    #[test] fn a_line_is_the_message_then_its_fields() {
        let mut l = Line::default();
        l.push("message", format_args!("scan failed"));
        l.push("error", format_args!("no such file"));
        l.push("path", format_args!("/music"));
        assert_eq!(l.finish(), "scan failed error=no such file path=/music");
    }

}
