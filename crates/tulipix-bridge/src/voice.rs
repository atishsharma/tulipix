//! The Voice section's store, and the parts of a note that need no microphone:
//! reading whisper's timed lines, finding the tasks said aloud, and writing a
//! transcript out as text, subtitles or Markdown. `api::voice` records,
//! transcribes and maps what crosses.

use anyhow::Result;
use chrono::{Datelike, NaiveDate, TimeDelta, Weekday};
use sqlx::SqlitePool;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS notes (
    id         INTEGER PRIMARY KEY,
    title      TEXT    NOT NULL DEFAULT '',
    path       TEXT    NOT NULL DEFAULT '',   -- the 16 kHz WAV, in <data>/voice
    origin     TEXT    NOT NULL DEFAULT '',   -- an imported file, until converted
    source     TEXT    NOT NULL DEFAULT 'mic',-- mic | file
    created    INTEGER NOT NULL,
    duration_s REAL    NOT NULL DEFAULT 0,
    peaks      TEXT    NOT NULL DEFAULT '',
    state      TEXT    NOT NULL DEFAULT 'new',-- new | working | done | failed
    error      TEXT    NOT NULL DEFAULT '',
    transcript TEXT    NOT NULL DEFAULT '',
    summary    TEXT    NOT NULL DEFAULT '',   -- lines
    summary_by TEXT    NOT NULL DEFAULT '',   -- the model's name, or '' for extractive
    starred    INTEGER NOT NULL DEFAULT 0,
    journal_id INTEGER NOT NULL DEFAULT 0
);

-- whisper's timed lines, which is what playback highlights and a hit seeks to.
CREATE TABLE IF NOT EXISTS segments (
    note_id  INTEGER NOT NULL,
    idx      INTEGER NOT NULL,
    start_ms INTEGER NOT NULL,
    end_ms   INTEGER NOT NULL,
    text     TEXT    NOT NULL,
    PRIMARY KEY (note_id, idx)
);

-- Bookmarks pressed while recording.
CREATE TABLE IF NOT EXISTS marks (
    note_id INTEGER NOT NULL,
    at_ms   INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tasks (
    id      INTEGER PRIMARY KEY,
    note_id INTEGER NOT NULL,
    text    TEXT    NOT NULL,
    at_ms   INTEGER NOT NULL DEFAULT 0,
    due     TEXT    NOT NULL DEFAULT '',     -- ISO date, or ''
    done    INTEGER NOT NULL DEFAULT 0,
    created INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_note_idx ON tasks(note_id);

CREATE VIRTUAL TABLE IF NOT EXISTS seg_fts USING fts5(
    text, note_id UNINDEXED, start_ms UNINDEXED,
    tokenize = 'unicode61 remove_diacritics 2'
);
"#;

pub async fn apply_schema(pool: &SqlitePool) -> Result<()> {
    sqlx::raw_sql(SCHEMA).execute(pool).await?;
    Ok(())
}

pub fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

// ── whisper ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Seg {
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
}

/// `00:01:02.345` as milliseconds.
fn stamp(s: &str) -> Option<i64> {
    let (hms, ms) = s.trim().split_once('.')?;
    let mut parts = hms.split(':').map(|p| p.parse::<i64>().ok());
    let (h, m, sec) = (parts.next()??, parts.next()??, parts.next()??);
    Some(((h * 60 + m) * 60 + sec) * 1000 + ms.parse::<i64>().ok()?)
}

/// whisper-cli's stdout, `[00:00:00.000 --> 00:00:04.440]   Hello there.`, as
/// segments. Lines that are only a sound — `[BLANK_AUDIO]`, `(wind blowing)` —
/// are dropped: they are not words anybody said.
pub fn parse_whisper(out: &str) -> Vec<Seg> {
    out.lines()
        .filter_map(|l| {
            let l = l.trim();
            let rest = l.strip_prefix('[')?;
            let (times, text) = rest.split_once(']')?;
            let (a, b) = times.split_once("-->")?;
            let text = text.trim();
            let noise = (text.starts_with('[') && text.ends_with(']')) || (text.starts_with('(') && text.ends_with(')'));
            if text.is_empty() || noise {
                return None;
            }
            Some(Seg { start_ms: stamp(a)?, end_ms: stamp(b)?, text: text.to_string() })
        })
        .collect()
}

pub fn plain(segs: &[Seg]) -> String {
    segs.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" ")
}

/// "1:48", or "1:02:05" past the hour.
pub fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

fn srt_stamp(ms: i64) -> String {
    let ms = ms.max(0);
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

pub fn srt(segs: &[Seg]) -> String {
    segs.iter()
        .enumerate()
        .map(|(i, s)| format!("{}\n{} --> {}\n{}\n", i + 1, srt_stamp(s.start_ms), srt_stamp(s.end_ms), s.text))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A note as a Markdown page: title, when, the summary, the tasks and the
/// transcript with a time at each line.
pub fn markdown(title: &str, when: &str, summary: &[String], tasks: &[(String, bool)], segs: &[Seg]) -> String {
    let mut md = format!("# {title}\n\n_{when}_\n");
    if !summary.is_empty() {
        md.push_str("\n## Summary\n\n");
        md.push_str(&summary.join(" "));
        md.push('\n');
    }
    if !tasks.is_empty() {
        md.push_str("\n## Tasks\n\n");
        for (t, done) in tasks {
            md.push_str(&format!("- [{}] {t}\n", if *done { "x" } else { " " }));
        }
    }
    md.push_str("\n## Transcript\n\n");
    for s in segs {
        md.push_str(&format!("**{}** {}\n\n", clock(s.start_ms), s.text));
    }
    md
}

/// A title from the first words said, until somebody names the note.
pub fn auto_title(text: &str) -> String {
    let first = text.split(['.', '?', '!']).map(str::trim).find(|s| !s.is_empty()).unwrap_or("");
    let words: Vec<&str> = first.split_whitespace().take(7).collect();
    let mut t = words.join(" ").trim_end_matches([',', ';', ':']).to_string();
    if first.split_whitespace().count() > 7 {
        t.push('…');
    }
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

// ── tasks ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub text: String,
    pub at_ms: i64,
    pub due: Option<NaiveDate>,
}

/// What makes a sentence a task, and whether the phrase itself is dropped
/// from the task's text ("remind me to call" is a task to *call*).
const TRIGGERS: &[(&str, bool)] = &[
    ("remind me to ", true),
    ("don't forget to ", true),
    ("dont forget to ", true),
    ("remember to ", true),
    ("i need to ", true),
    ("we need to ", true),
    ("i have to ", true),
    ("i must ", true),
    ("to do ", true),
    ("todo ", true),
    ("i should ", true),
];

/// Promises only count with a day attached: "I'll chase them tomorrow" is a
/// task, "I'll think about it" is not.
const PROMISES: &[&str] = &["i'll ", "i will ", "we'll ", "i'm going to "];

const DAYS: [(&str, Weekday); 7] = [
    ("monday", Weekday::Mon),
    ("tuesday", Weekday::Tue),
    ("wednesday", Weekday::Wed),
    ("thursday", Weekday::Thu),
    ("friday", Weekday::Fri),
    ("saturday", Weekday::Sat),
    ("sunday", Weekday::Sun),
];

/// The day a sentence names, from `today`: today, tomorrow, next week (its
/// Monday) or a weekday (the next one; today's name means a week on).
pub fn due_in(sentence: &str, today: NaiveDate) -> Option<NaiveDate> {
    let s = format!(" {} ", sentence.to_lowercase().replace([',', '.', '!', '?'], " "));
    if s.contains(" today ") || s.contains(" tonight ") || s.contains(" this evening ") {
        return Some(today);
    }
    if s.contains(" tomorrow ") {
        return Some(today + TimeDelta::days(1));
    }
    if s.contains(" next week ") {
        let to_monday = 7 - today.weekday().num_days_from_monday() as i64;
        return Some(today + TimeDelta::days(to_monday));
    }
    for (name, wd) in DAYS {
        if s.contains(&format!(" {name} ")) {
            let ahead = (wd.num_days_from_monday() as i64 - today.weekday().num_days_from_monday() as i64).rem_euclid(7);
            return Some(today + TimeDelta::days(if ahead == 0 { 7 } else { ahead }));
        }
    }
    None
}

/// The tasks said in a note: sentences that start one ("remind me to…",
/// "I need to…", "buy…"), and promises with a day on them.
///
/// ponytail: phrase rules, English only. A local model can take this over
/// when one is set up, as it does the summary.
pub fn find_tasks(segs: &[Seg], today: NaiveDate) -> Vec<Found> {
    let mut out = Vec::new();
    for seg in segs {
        for sentence in seg.text.split_inclusive(['.', '?', '!']) {
            let raw = sentence.trim();
            let low = raw.to_lowercase();
            let due = due_in(raw, today);
            let hit = TRIGGERS.iter().find_map(|(t, drop)| low.find(t).map(|at| (at, t.len(), *drop)));
            // Offsets come from the lowercased copy; `get` rather than slicing,
            // since lowercasing can change a letter's length outside ASCII.
            let text = if let Some((at, len, drop)) = hit {
                raw.get(if drop { at + len } else { at }..)
            } else if low.starts_with("buy ") || low.starts_with("pick up ") || low.starts_with("call ") || low.starts_with("email ") {
                Some(raw)
            } else if due.is_some() && let Some(at) = PROMISES.iter().find_map(|p| low.find(p).map(|at| at + p.len())) {
                raw.get(at..)
            } else {
                continue;
            };
            let Some(text) = text else { continue };
            let text = text.trim().trim_end_matches(['.', '?', '!', ',']).trim();
            if text.split_whitespace().count() < 2 {
                continue;
            }
            let mut c = text.chars();
            let text: String = match c.next() {
                Some(f) => f.to_uppercase().chain(c).collect(),
                None => continue,
            };
            out.push(Found { text, at_ms: seg.start_ms, due });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const OUT: &str = "\
[00:00:00.000 --> 00:00:04.440]   So flights are sorted for the ninth.
[00:00:04.440 --> 00:00:05.000]   [BLANK_AUDIO]
[00:01:48.120 --> 00:01:52.000]   Remind me to call the plumber on Tuesday about the boiler.
[01:00:00.000 --> 01:00:02.500]   (wind blowing)
";

    #[test]
    fn whisper_lines_become_segments() {
        let s = parse_whisper(OUT);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0], Seg { start_ms: 0, end_ms: 4440, text: "So flights are sorted for the ninth.".into() });
        assert_eq!(s[1].start_ms, 108_120);
        assert_eq!(clock(s[1].start_ms), "1:48");
        assert_eq!(clock(3_725_000), "1:02:05");
        assert!(srt(&s).contains("00:01:48,120 --> 00:01:52,000"));
    }

    #[test]
    fn tasks_and_their_days() {
        // A Friday.
        let fri = NaiveDate::from_ymd_opt(2026, 9, 25).unwrap();
        let segs = vec![
            Seg { start_ms: 0, end_ms: 1, text: "Remind me to call the plumber on Tuesday about the boiler.".into() },
            Seg { start_ms: 5000, end_ms: 1, text: "Not yet, I'll chase them tomorrow. I'll think about it.".into() },
            Seg { start_ms: 9000, end_ms: 1, text: "Buy onions and bin bags. The weather was nice.".into() },
            Seg { start_ms: 12000, end_ms: 1, text: "I need to book Sintra next week, and it's Friday again on Friday.".into() },
        ];
        let t = find_tasks(&segs, fri);
        let got: Vec<(&str, Option<NaiveDate>)> = t.iter().map(|f| (f.text.as_str(), f.due)).collect();
        let d = |m, day| NaiveDate::from_ymd_opt(2026, m, day);
        assert_eq!(
            got,
            vec![
                ("Call the plumber on Tuesday about the boiler", d(9, 29)),
                ("Chase them tomorrow", d(9, 26)),
                ("Buy onions and bin bags", None),
                ("Book Sintra next week, and it's Friday again on Friday", d(9, 28)),
            ]
        );
        assert_eq!(t[1].at_ms, 5000);
        // Today's own weekday means next week's.
        assert_eq!(due_in("see you friday", fri), d(10, 2));
    }

    #[test]
    fn a_title_from_the_first_words() {
        assert_eq!(auto_title("so flights are sorted for the ninth, the early one. Then"), "So flights are sorted for the ninth…");
        assert_eq!(auto_title("Shopping."), "Shopping");
        assert_eq!(auto_title(""), "");
    }
}
