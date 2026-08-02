//! Dates, stored as ISO `YYYY-MM-DD` text.
//!
//! Text rather than an integer so a statement, a backup and a hand-written SQL
//! query all read the same, and because `occurred_on DESC` on ISO text sorts
//! correctly without a conversion.
//!
//! The interesting part here is [`advance`]. Rolling a monthly cycle forward is
//! not "add one month" — a bill anchored on the 31st has to land on the 28th in
//! February and back on the 31st in March. Clamping in place loses the anchor
//! permanently, which is the bug that turns a rent reminder into a 28th-of-the-
//! month reminder forever.

use anyhow::{Context, Result};
use chrono::{Datelike, Months, NaiveDate};

/// Today, local time. Local rather than UTC because a bill due "today" means
/// today where the user is, and a UTC day boundary would mark a bill overdue up
/// to five and a half hours early in India.
pub fn today() -> NaiveDate {
    chrono::Local::now().date_naive()
}

pub fn parse(s: &str) -> Result<NaiveDate> {
    NaiveDate::parse_from_str(s.trim(), "%Y-%m-%d").with_context(|| format!("bad date {s:?}"))
}

pub fn iso(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

/// `YYYY-MM`, the key `budgets.period` uses.
pub fn ym(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

/// First and last day of the `YYYY-MM` period, as ISO text.
pub fn month_bounds(period: &str) -> Result<(String, String)> {
    let (y, m) = period
        .split_once('-')
        .context("period must be YYYY-MM")?;
    let y: i32 = y.parse().context("bad year")?;
    let m: u32 = m.parse().context("bad month")?;
    let first = NaiveDate::from_ymd_opt(y, m, 1).context("bad period")?;
    let last = first.with_day(days_in_month(y, m)).context("bad period")?;
    Ok((iso(first), iso(last)))
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    // The first of next month, minus a day. Handles February, leap years and
    // December→January without a table.
    let (ny, nm) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    NaiveDate::from_ymd_opt(ny, nm, 1)
        .and_then(|d| d.pred_opt())
        .map(|d| d.day())
        .unwrap_or(28)
}

/// How often a recurrence repeats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cycle {
    Weekly,
    Monthly,
    Quarterly,
    Yearly,
    /// No predictable period. Never auto-advances; the user dates each instance.
    Irregular,
}

impl Cycle {
    pub fn as_str(self) -> &'static str {
        match self {
            Cycle::Weekly => "weekly",
            Cycle::Monthly => "monthly",
            Cycle::Quarterly => "quarterly",
            Cycle::Yearly => "yearly",
            Cycle::Irregular => "irregular",
        }
    }

    pub fn parse(s: &str) -> Cycle {
        match s {
            "weekly" => Cycle::Weekly,
            "quarterly" => Cycle::Quarterly,
            "yearly" => Cycle::Yearly,
            "irregular" => Cycle::Irregular,
            // Monthly is the overwhelming majority and the safe fallback: it
            // produces a due date, where irregular produces none.
            _ => Cycle::Monthly,
        }
    }

    /// Rough length, for annualising a subscription cost.
    pub fn per_year(self) -> i64 {
        match self {
            Cycle::Weekly => 52,
            Cycle::Monthly => 12,
            Cycle::Quarterly => 4,
            Cycle::Yearly => 1,
            Cycle::Irregular => 0,
        }
    }
}

/// Add `n` months to `d`, landing on `anchor` day-of-month, clamped to the
/// length of the target month.
///
/// `anchor` is carried separately from `d` on purpose: a bill anchored on the
/// 31st that got clamped to 28 February must return to 31 March, and it can only
/// do that if the 31 is remembered somewhere other than the last due date.
pub fn add_months_anchored(d: NaiveDate, n: u32, anchor: u32) -> NaiveDate {
    let anchor = anchor.clamp(1, 31);
    // Move from the 1st, so the clamping chrono does on its own never applies
    // and the anchor is the only thing that decides the day.
    let moved = d
        .with_day(1)
        .and_then(|f| f.checked_add_months(Months::new(n)))
        .unwrap_or(d);
    let dim = days_in_month(moved.year(), moved.month());
    moved.with_day(anchor.min(dim)).unwrap_or(moved)
}

/// The next due date after `from`.
///
/// `anchor` is the recurrence's `anchor_day`; when it is `None` the day of
/// `from` is used, which is what an imported or ad-hoc recurrence has.
pub fn advance(from: NaiveDate, cycle: Cycle, anchor: Option<u32>) -> NaiveDate {
    let anchor = anchor.unwrap_or_else(|| from.day());
    match cycle {
        // Weekly ignores the day-of-month anchor: a weekly thing is anchored to
        // a weekday, and forcing it to a date would drift it every month.
        Cycle::Weekly => from + chrono::Duration::days(7),
        Cycle::Monthly => add_months_anchored(from, 1, anchor),
        Cycle::Quarterly => add_months_anchored(from, 3, anchor),
        Cycle::Yearly => add_months_anchored(from, 12, anchor),
        Cycle::Irregular => from,
    }
}

/// Days from `a` to `b`, negative when `b` is earlier.
pub fn days_between(a: NaiveDate, b: NaiveDate) -> i64 {
    (b - a).num_days()
}

/// The next occurrence of day-of-month `day`, on or after `from`.
///
/// Clamped to the length of whichever month it lands in, so a statement day of
/// 31 falls on 28 February rather than vanishing. Returns `None` only for a day
/// outside 1-31, which is not a day.
pub fn next_on_day(from: NaiveDate, day: i64) -> Option<String> {
    let day = u32::try_from(day).ok().filter(|d| (1..=31).contains(d))?;
    let this = {
        let dim = days_in_month(from.year(), from.month());
        from.with_day(day.min(dim))?
    };
    if this >= from {
        return Some(iso(this));
    }
    Some(iso(add_months_anchored(from, 1, day)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(s: &str) -> NaiveDate {
        parse(s).unwrap()
    }

    #[test]
    fn month_lengths_including_leap_february() {
        assert_eq!(days_in_month(2026, 2), 28);
        assert_eq!(days_in_month(2028, 2), 29, "2028 is a leap year");
        assert_eq!(days_in_month(2100, 2), 28, "2100 is not, despite being divisible by 4");
        assert_eq!(days_in_month(2026, 12), 31);
        assert_eq!(days_in_month(2026, 4), 30);
    }

    #[test]
    fn the_next_statement_day_is_this_month_or_the_next_one() {
        // Still to come this month.
        assert_eq!(next_on_day(d("2026-07-02"), 5).as_deref(), Some("2026-07-05"));
        // Today counts as on or after itself.
        assert_eq!(next_on_day(d("2026-07-05"), 5).as_deref(), Some("2026-07-05"));
        // Already gone, so it rolls forward.
        assert_eq!(next_on_day(d("2026-07-20"), 5).as_deref(), Some("2026-08-05"));
        // A 31st statement day in a 30-day month lands on the 30th, and comes
        // back to the 31st afterwards — the anchor rule, applied here too.
        assert_eq!(next_on_day(d("2026-04-01"), 31).as_deref(), Some("2026-04-30"));
        assert_eq!(next_on_day(d("2026-05-01"), 31).as_deref(), Some("2026-05-31"));
        // Not a day of any month.
        assert_eq!(next_on_day(d("2026-07-02"), 0), None);
        assert_eq!(next_on_day(d("2026-07-02"), 32), None);
    }

    #[test]
    fn a_31st_anchor_survives_february() {
        // This is the whole reason anchor_day is a column.
        let jan = d("2026-01-31");
        let feb = advance(jan, Cycle::Monthly, Some(31));
        assert_eq!(iso(feb), "2026-02-28");
        let mar = advance(feb, Cycle::Monthly, Some(31));
        assert_eq!(iso(mar), "2026-03-31", "the anchor must come back, not stay clamped");
        let apr = advance(mar, Cycle::Monthly, Some(31));
        assert_eq!(iso(apr), "2026-04-30");
    }

    #[test]
    fn a_29th_anchor_in_a_leap_year() {
        let feb = advance(d("2028-01-29"), Cycle::Monthly, Some(29));
        assert_eq!(iso(feb), "2028-02-29");
        let feb = advance(d("2026-01-29"), Cycle::Monthly, Some(29));
        assert_eq!(iso(feb), "2026-02-28");
    }

    #[test]
    fn cycles_advance_by_their_own_period() {
        assert_eq!(iso(advance(d("2026-07-26"), Cycle::Weekly, Some(26))), "2026-08-02");
        assert_eq!(iso(advance(d("2026-07-26"), Cycle::Monthly, Some(26))), "2026-08-26");
        assert_eq!(iso(advance(d("2026-07-26"), Cycle::Quarterly, Some(26))), "2026-10-26");
        assert_eq!(iso(advance(d("2026-07-26"), Cycle::Yearly, Some(26))), "2027-07-26");
        // Irregular has no period, so it must not invent one.
        assert_eq!(iso(advance(d("2026-07-26"), Cycle::Irregular, Some(26))), "2026-07-26");
    }

    #[test]
    fn year_end_rolls_over() {
        assert_eq!(iso(advance(d("2026-12-15"), Cycle::Monthly, Some(15))), "2027-01-15");
        assert_eq!(iso(advance(d("2026-11-30"), Cycle::Quarterly, Some(30))), "2027-02-28");
    }

    #[test]
    fn period_bounds_cover_the_whole_month() {
        assert_eq!(month_bounds("2026-02").unwrap(), ("2026-02-01".into(), "2026-02-28".into()));
        assert_eq!(month_bounds("2028-02").unwrap(), ("2028-02-01".into(), "2028-02-29".into()));
        assert_eq!(month_bounds("2026-12").unwrap(), ("2026-12-01".into(), "2026-12-31".into()));
        assert!(month_bounds("2026").is_err());
    }

    #[test]
    fn iso_text_sorts_chronologically() {
        let mut v = ["2026-10-01", "2026-02-28", "2026-02-05"];
        v.sort_unstable();
        assert_eq!(v, ["2026-02-05", "2026-02-28", "2026-10-01"]);
    }

    #[test]
    fn day_gaps() {
        assert_eq!(days_between(d("2026-07-26"), d("2026-08-02")), 7);
        assert_eq!(days_between(d("2026-08-02"), d("2026-07-26")), -7);
    }
}
