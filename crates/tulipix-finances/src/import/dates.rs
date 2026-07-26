//! Statement dates, which are not ISO and are frequently ambiguous.
//!
//! `05/07/2026` is the 5th of July to an Indian bank and the 7th of May to an
//! American one, and nothing in the row says which. Guessing wrong shifts a whole
//! statement by months and it is not obvious from the result — the amounts all
//! look right.
//!
//! So the format is resolved once, over the whole file, before any row is parsed:
//! [`sniff`] looks for a component that can only be a day. When the file genuinely
//! cannot say, it says so, and the caller asks the user rather than guessing.

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;

/// How a statement writes its dates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateFormat {
    /// `05/07/2026` = 5 July. Indian and European statements.
    DayFirst,
    /// `05/07/2026` = 7 May. US statements.
    MonthFirst,
    /// `2026-07-05`. Unambiguous.
    Iso,
    /// `05 Jul 2026` / `05-JUL-2026`. Unambiguous because the month is named.
    NamedMonth,
}

impl DateFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            DateFormat::DayFirst => "DD/MM/YYYY",
            DateFormat::MonthFirst => "MM/DD/YYYY",
            DateFormat::Iso => "YYYY-MM-DD",
            DateFormat::NamedMonth => "DD MMM YYYY",
        }
    }

    pub fn parse_name(s: &str) -> Option<DateFormat> {
        match s.trim().to_uppercase().replace(['-', '.', ' '], "/").as_str() {
            "DD/MM/YYYY" => Some(DateFormat::DayFirst),
            "MM/DD/YYYY" => Some(DateFormat::MonthFirst),
            "YYYY/MM/DD" => Some(DateFormat::Iso),
            "DD/MMM/YYYY" => Some(DateFormat::NamedMonth),
            _ => None,
        }
    }
}

/// Split a date on any of the separators statements use.
fn parts(s: &str) -> Vec<&str> {
    s.trim().split(['/', '-', '.', ' ']).filter(|p| !p.is_empty()).collect()
}

fn month_from_name(s: &str) -> Option<u32> {
    let key: String = s.chars().take(3).flat_map(|c| c.to_uppercase()).collect();
    [
        "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
    ]
    .iter()
    .position(|m| *m == key)
    .map(|i| i as u32 + 1)
}

fn four_digit_year(s: &str) -> Option<i32> {
    let y: i32 = s.parse().ok()?;
    match s.len() {
        4 => Some(y),
        // Two-digit years: statements from this century. 26 → 2026, 99 → 1999.
        2 => Some(if y >= 70 { 1900 + y } else { 2000 + y }),
        _ => None,
    }
}

/// What can this column of dates possibly be?
///
/// Returns `None` when the sample is genuinely ambiguous — every row has both
/// components at 12 or below — so the caller can ask instead of guessing. That
/// happens with real files: a statement covering only the first twelve days of a
/// month says nothing about its own format.
pub fn sniff(samples: &[String]) -> Option<DateFormat> {
    let mut saw_named = false;
    let mut first_over_12 = false;
    let mut second_over_12 = false;
    let mut saw_iso = false;
    let mut saw_numeric = false;

    for s in samples {
        let p = parts(s);
        if p.len() < 3 {
            continue;
        }
        if p.iter().any(|c| month_from_name(c).is_some() && c.parse::<u32>().is_err()) {
            saw_named = true;
            continue;
        }
        // A four-digit leading component can only be a year.
        if p[0].len() == 4 && p[0].parse::<i32>().is_ok() {
            saw_iso = true;
            continue;
        }
        saw_numeric = true;
        if let Ok(a) = p[0].parse::<u32>()
            && a > 12
        {
            first_over_12 = true;
        }
        if let Ok(b) = p[1].parse::<u32>()
            && b > 12
        {
            second_over_12 = true;
        }
    }

    if saw_named {
        return Some(DateFormat::NamedMonth);
    }
    if saw_iso && !saw_numeric {
        return Some(DateFormat::Iso);
    }
    match (first_over_12, second_over_12) {
        // Only one of them can be a day, so the other is the month.
        (true, false) => Some(DateFormat::DayFirst),
        (false, true) => Some(DateFormat::MonthFirst),
        // Both over 12 is not a date at all; neither over 12 is undecidable.
        _ => None,
    }
}

/// Parse a statement date in a known format.
pub fn parse(s: &str, fmt: DateFormat) -> Result<NaiveDate> {
    let p = parts(s);
    if p.len() < 3 {
        bail!("{s:?} does not look like a date");
    }
    let (y, m, d) = match fmt {
        DateFormat::Iso => (
            four_digit_year(p[0]).context("bad year")?,
            p[1].parse::<u32>().context("bad month")?,
            p[2].parse::<u32>().context("bad day")?,
        ),
        DateFormat::DayFirst => (
            four_digit_year(p[2]).context("bad year")?,
            p[1].parse::<u32>().context("bad month")?,
            p[0].parse::<u32>().context("bad day")?,
        ),
        DateFormat::MonthFirst => (
            four_digit_year(p[2]).context("bad year")?,
            p[0].parse::<u32>().context("bad month")?,
            p[1].parse::<u32>().context("bad day")?,
        ),
        DateFormat::NamedMonth => {
            // The named component can be in either of the first two positions:
            // `05 Jul 2026` and `Jul 05 2026` both occur.
            let (day, month) = match month_from_name(p[1]) {
                Some(m) => (p[0].parse::<u32>().context("bad day")?, m),
                None => (
                    p[1].parse::<u32>().context("bad day")?,
                    month_from_name(p[0]).context("no month name")?,
                ),
            };
            (four_digit_year(p[2]).context("bad year")?, month, day)
        }
    };
    NaiveDate::from_ymd_opt(y, m, d).with_context(|| format!("{s:?} is not a real date"))
}

/// OFX timestamps: `YYYYMMDD`, optionally with a time and a bracketed zone.
pub fn parse_ofx(s: &str) -> Result<NaiveDate> {
    let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.len() < 8 {
        bail!("{s:?} is not an OFX date");
    }
    let y: i32 = digits[0..4].parse()?;
    let m: u32 = digits[4..6].parse()?;
    let d: u32 = digits[6..8].parse()?;
    NaiveDate::from_ymd_opt(y, m, d).with_context(|| format!("{s:?} is not a real date"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn a_day_over_twelve_settles_the_format() {
        assert_eq!(sniff(&s(&["05/07/2026", "26/07/2026"])), Some(DateFormat::DayFirst));
        assert_eq!(sniff(&s(&["07/05/2026", "07/26/2026"])), Some(DateFormat::MonthFirst));
    }

    #[test]
    fn a_file_that_cannot_say_is_not_guessed_at() {
        // Every row in the first twelve days of a month: genuinely undecidable,
        // and guessing would shift the whole statement silently.
        assert_eq!(sniff(&s(&["05/07/2026", "09/07/2026", "11/07/2026"])), None);
        assert_eq!(sniff(&s(&[])), None);
        assert_eq!(sniff(&s(&["not a date"])), None);
    }

    #[test]
    fn named_months_and_iso_need_no_guessing() {
        assert_eq!(sniff(&s(&["05 Jul 2026"])), Some(DateFormat::NamedMonth));
        assert_eq!(sniff(&s(&["05-JUL-2026"])), Some(DateFormat::NamedMonth));
        assert_eq!(sniff(&s(&["2026-07-05", "2026-07-09"])), Some(DateFormat::Iso));
    }

    #[test]
    fn the_same_string_parses_to_different_days_by_format() {
        // This is the bug the sniffer exists to prevent, stated as a test.
        let day = parse("05/07/2026", DateFormat::DayFirst).unwrap();
        let month = parse("05/07/2026", DateFormat::MonthFirst).unwrap();
        assert_eq!(day.to_string(), "2026-07-05");
        assert_eq!(month.to_string(), "2026-05-07");
        assert_ne!(day, month);
    }

    #[test]
    fn separators_do_not_matter() {
        for raw in ["05/07/2026", "05-07-2026", "05.07.2026", "05 07 2026"] {
            assert_eq!(parse(raw, DateFormat::DayFirst).unwrap().to_string(), "2026-07-05");
        }
    }

    #[test]
    fn named_months_in_either_position() {
        assert_eq!(parse("05 Jul 2026", DateFormat::NamedMonth).unwrap().to_string(), "2026-07-05");
        assert_eq!(parse("Jul 05 2026", DateFormat::NamedMonth).unwrap().to_string(), "2026-07-05");
        assert_eq!(parse("05-SEPTEMBER-2026", DateFormat::NamedMonth).unwrap().to_string(), "2026-09-05");
    }

    #[test]
    fn two_digit_years() {
        assert_eq!(parse("05/07/26", DateFormat::DayFirst).unwrap().to_string(), "2026-07-05");
        assert_eq!(parse("05/07/99", DateFormat::DayFirst).unwrap().to_string(), "1999-07-05");
    }

    #[test]
    fn impossible_dates_are_refused_rather_than_clamped() {
        assert!(parse("31/02/2026", DateFormat::DayFirst).is_err());
        assert!(parse("00/07/2026", DateFormat::DayFirst).is_err());
        assert!(parse("05/13/2026", DateFormat::MonthFirst).is_err());
        assert!(parse("05/07", DateFormat::DayFirst).is_err());
    }

    #[test]
    fn ofx_timestamps() {
        assert_eq!(parse_ofx("20260705").unwrap().to_string(), "2026-07-05");
        assert_eq!(parse_ofx("20260705120000[-5:EST]").unwrap().to_string(), "2026-07-05");
        assert!(parse_ofx("2026").is_err());
    }

    #[test]
    fn format_names_round_trip() {
        for f in [DateFormat::DayFirst, DateFormat::MonthFirst, DateFormat::Iso, DateFormat::NamedMonth] {
            assert_eq!(DateFormat::parse_name(f.as_str()), Some(f), "{}", f.as_str());
        }
    }
}
