//! A calendar file any calendar app imports: Papers' "Plan renewal" and
//! Kitchen's week of meals. All-day events only — neither knows a time.
//!
//! The file is written under the cache folder and handed to the system, which
//! opens it in the default calendar and asks to add the events there. Tulipix
//! has no calendar of its own, and this is the one every desktop already has.

use anyhow::{Result, anyhow};
use chrono::NaiveDate;

pub struct Event {
    /// Stable across exports, so importing the same week twice updates it
    /// rather than doubling it.
    pub uid: String,
    pub day: NaiveDate,
    pub title: String,
    pub note: String,
}

/// RFC 5545 text: backslash, semicolon, comma and newline escaped.
fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n")
}

pub fn calendar(events: &[Event]) -> String {
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
    let mut out = String::from("BEGIN:VCALENDAR\r\nVERSION:2.0\r\nPRODID:-//Tulipix//EN\r\n");
    for e in events {
        let next = e.day.succ_opt().unwrap_or(e.day);
        out.push_str(&format!(
            "BEGIN:VEVENT\r\nUID:{}@tulipix\r\nDTSTAMP:{stamp}\r\nDTSTART;VALUE=DATE:{}\r\nDTEND;VALUE=DATE:{}\r\nSUMMARY:{}\r\n",
            esc(&e.uid),
            e.day.format("%Y%m%d"),
            next.format("%Y%m%d"),
            esc(&e.title),
        ));
        if !e.note.is_empty() {
            out.push_str(&format!("DESCRIPTION:{}\r\n", esc(&e.note)));
        }
        out.push_str("END:VEVENT\r\n");
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

/// Write `events` to `<cache>/calendar/<name>.ics` and open it.
pub fn open(name: &str, events: &[Event]) -> Result<()> {
    let dir = tulipix_core::paths::cache_dir().ok_or_else(|| anyhow!("no cache folder"))?.join("calendar");
    std::fs::create_dir_all(&dir)?;
    let file = dir.join(format!("{name}.ics"));
    std::fs::write(&file, calendar(events))?;
    crate::api::transfer::open_url(&file.to_string_lossy());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_all_day_event_ends_the_next_day_and_escapes_text() {
        let d = NaiveDate::from_ymd_opt(2026, 8, 22).unwrap();
        let s = calendar(&[Event { uid: "p1".into(), day: d, title: "Renew passport, now".into(), note: String::new() }]);
        assert!(s.contains("DTSTART;VALUE=DATE:20260822\r\n"));
        assert!(s.contains("DTEND;VALUE=DATE:20260823\r\n"));
        assert!(s.contains("SUMMARY:Renew passport\\, now\r\n"));
        assert!(!s.contains("DESCRIPTION"));
        assert!(s.starts_with("BEGIN:VCALENDAR\r\n") && s.ends_with("END:VCALENDAR\r\n"));
    }
}
