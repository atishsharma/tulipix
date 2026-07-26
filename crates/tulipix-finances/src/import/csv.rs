//! Delimited statement exports.
//!
//! Uses the `csv` crate rather than splitting on commas. Bank narrations contain
//! commas constantly — `UPI/DR/9928/SWIGGY, BANGALORE` — and a hand-rolled
//! splitter mangles exactly the rows whose merchant matters most for detection.
//!
//! Two shapes cover essentially every export: one signed amount column, or a
//! separate debit and credit pair. Indian bank exports use the pair, so it is not
//! an edge case.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::dates::{self, DateFormat};
use crate::money;

/// Which column is which. Stored as JSON in `import_presets.column_map`.
///
/// Column *names* rather than indices, because banks reorder columns between
/// statement versions and a saved index silently starts reading the wrong one.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ColumnMap {
    pub date: String,
    pub description: String,
    /// A single signed column: negative is money out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<String>,
    /// Or a pair, where whichever cell is filled decides the direction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credit: Option<String>,
}

impl ColumnMap {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(s: &str) -> Result<ColumnMap> {
        serde_json::from_str(s).context("stored column map is not valid JSON")
    }

    fn is_usable(&self) -> bool {
        !self.date.is_empty()
            && !self.description.is_empty()
            && (self.amount.is_some() || self.debit.is_some() || self.credit.is_some())
    }
}

/// Pick the column whose header contains any of `keys`, preferring earlier keys.
fn find(headers: &[String], keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(h) = headers.iter().find(|h| h.to_lowercase().contains(key)) {
            return Some(h.clone());
        }
    }
    None
}

/// Guess a column map from the header row.
///
/// Deliberately a guess the user confirms, not a decision. The mapping is shown
/// in the import sheet before anything is posted, because getting debit and credit
/// backwards inverts every row in the file and the totals still look plausible.
pub fn guess(headers: &[String]) -> ColumnMap {
    let mut m = ColumnMap {
        date: find(headers, &["value date", "txn date", "transaction date", "date"]).unwrap_or_default(),
        description: find(
            headers,
            &["narration", "description", "particulars", "details", "remarks", "payee"],
        )
        .unwrap_or_default(),
        ..Default::default()
    };
    // The pair first: a file with both `Withdrawal Amt.` and `Amount` should be
    // read as a pair, since the single column is usually a running total.
    let debit = find(headers, &["withdrawal", "debit", "paid out", "money out"]);
    let credit = find(headers, &["deposit", "credit", "paid in", "money in"]);
    if debit.is_some() || credit.is_some() {
        m.debit = debit;
        m.credit = credit;
    } else {
        m.amount = find(headers, &["amount", "value"]);
    }
    m
}

/// One row as the file states it, before it becomes a transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawRow {
    pub occurred_on: String,
    pub description: String,
    /// Always positive; `is_credit` carries the direction.
    pub amount_minor: i64,
    pub is_credit: bool,
}

/// Header row of a delimited file, for the mapping UI.
pub fn headers(text: &str) -> Result<Vec<String>> {
    let mut rdr = reader(text);
    Ok(rdr.headers()?.iter().map(|h| h.trim().to_string()).collect())
}

fn reader(text: &str) -> ::csv::Reader<&[u8]> {
    // Leading `::` on purpose: this module is also called `csv`.
    ::csv::ReaderBuilder::new()
        // Bank exports habitually have a ragged preamble and trailing summary
        // lines with fewer fields. Refusing the file over that would reject most
        // real statements.
        .flexible(true)
        .trim(::csv::Trim::All)
        .from_reader(text.as_bytes())
}

/// The first few dates in the file, for [`dates::sniff`].
pub fn date_samples(text: &str, map: &ColumnMap, limit: usize) -> Result<Vec<String>> {
    let mut rdr = reader(text);
    let head: Vec<String> = rdr.headers()?.iter().map(|h| h.trim().to_string()).collect();
    let Some(i) = head.iter().position(|h| h.eq_ignore_ascii_case(&map.date)) else {
        bail!("no column called {:?}", map.date);
    };
    let mut out = Vec::new();
    for rec in rdr.records() {
        let rec = rec?;
        if let Some(v) = rec.get(i)
            && !v.trim().is_empty()
        {
            out.push(v.trim().to_string());
        }
        if out.len() >= limit {
            break;
        }
    }
    Ok(out)
}

/// A non-empty cell, or `None`.
///
/// A free function rather than a closure inside the loop: a closure returning a
/// reference borrowed from its own argument needs a named lifetime, which closure
/// syntax cannot express.
fn cell(rec: &::csv::StringRecord, i: Option<usize>) -> Option<&str> {
    let v = rec.get(i?)?.trim();
    if v.is_empty() { None } else { Some(v) }
}

/// Parse the whole file into rows.
///
/// Rows that cannot be read are skipped rather than failing the import: a
/// statement's trailing "Closing balance" line is not a transaction, and refusing
/// the file over it would mean no bank export ever imports. The count of skipped
/// rows is returned so the user is told rather than left to notice.
pub fn parse(text: &str, map: &ColumnMap, fmt: DateFormat, currency: &str) -> Result<(Vec<RawRow>, usize)> {
    if !map.is_usable() {
        bail!("the column mapping needs at least a date, a description and an amount");
    }
    let mut rdr = reader(text);
    let head: Vec<String> = rdr.headers()?.iter().map(|h| h.trim().to_string()).collect();

    let col = |name: &Option<String>| -> Option<usize> {
        let n = name.as_ref()?;
        head.iter().position(|h| h.eq_ignore_ascii_case(n))
    };
    let i_date = col(&Some(map.date.clone())).with_context(|| format!("no column {:?}", map.date))?;
    let i_desc = col(&Some(map.description.clone()))
        .with_context(|| format!("no column {:?}", map.description))?;
    let i_amount = col(&map.amount);
    let i_debit = col(&map.debit);
    let i_credit = col(&map.credit);

    let mut rows = Vec::new();
    let mut skipped = 0usize;

    for rec in rdr.records() {
        let Ok(rec) = rec else {
            skipped += 1;
            continue;
        };
        let raw_date = rec.get(i_date).unwrap_or("").trim();
        let description = rec.get(i_desc).unwrap_or("").trim().to_string();
        let Ok(on) = dates::parse(raw_date, fmt) else {
            skipped += 1;
            continue;
        };

        let (amount_minor, is_credit) = if let Some(v) = cell(&rec, i_amount) {
            match money::parse_amount(v, currency) {
                // A signed single column: negative is money leaving.
                Ok(a) => (a.abs(), a > 0),
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            }
        } else if let Some(v) = cell(&rec, i_debit) {
            match money::parse_amount(v, currency) {
                Ok(a) => (a.abs(), false),
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            }
        } else if let Some(v) = cell(&rec, i_credit) {
            match money::parse_amount(v, currency) {
                Ok(a) => (a.abs(), true),
                Err(_) => {
                    skipped += 1;
                    continue;
                }
            }
        } else {
            // No amount in any mapped column: a balance line or a blank row.
            skipped += 1;
            continue;
        };

        if amount_minor == 0 || description.is_empty() {
            skipped += 1;
            continue;
        }
        rows.push(RawRow {
            occurred_on: crate::date::iso(on),
            description,
            amount_minor,
            is_credit,
        });
    }
    Ok((rows, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An HDFC-shaped export: debit/credit pair, DD/MM/YYYY, a running balance
    /// column, and a trailing summary line.
    const HDFC: &str = "\
Date,Narration,Withdrawal Amt.,Deposit Amt.,Closing Balance
05/07/2026,\"UPI/DR/9928/SWIGGY, BANGALORE\",340.00,,10499.00
26/07/2026,NETFLIX 4471,649.00,,9850.00
01/07/2026,SALARY JULY,,50000.00,10839.00
,Closing balance,,,9850.00
";

    /// A US-shaped export: one signed amount column, MM/DD/YYYY.
    const US: &str = "\
Transaction Date,Description,Amount
07/26/2026,NETFLIX.COM,-9.99
07/01/2026,PAYROLL,3200.00
";

    fn h(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn a_debit_credit_pair_is_recognised_over_a_single_column() {
        let m = guess(&headers(HDFC).unwrap());
        assert_eq!(m.date, "Date");
        assert_eq!(m.description, "Narration");
        assert_eq!(m.debit.as_deref(), Some("Withdrawal Amt."));
        assert_eq!(m.credit.as_deref(), Some("Deposit Amt."));
        assert_eq!(m.amount, None, "the balance column must not be read as the amount");
    }

    #[test]
    fn a_single_signed_column_is_recognised() {
        let m = guess(&headers(US).unwrap());
        assert_eq!(m.date, "Transaction Date");
        assert_eq!(m.description, "Description");
        assert_eq!(m.amount.as_deref(), Some("Amount"));
        assert_eq!(m.debit, None);
    }

    #[test]
    fn commas_inside_a_narration_do_not_split_the_row() {
        let m = guess(&headers(HDFC).unwrap());
        let (rows, _) = parse(HDFC, &m, DateFormat::DayFirst, "INR").unwrap();
        let swiggy = rows.iter().find(|r| r.description.contains("SWIGGY")).unwrap();
        assert_eq!(swiggy.description, "UPI/DR/9928/SWIGGY, BANGALORE");
        assert_eq!(swiggy.amount_minor, 34_000);
        assert!(!swiggy.is_credit);
    }

    #[test]
    fn debit_and_credit_get_opposite_directions() {
        let m = guess(&headers(HDFC).unwrap());
        let (rows, _) = parse(HDFC, &m, DateFormat::DayFirst, "INR").unwrap();
        let salary = rows.iter().find(|r| r.description == "SALARY JULY").unwrap();
        assert!(salary.is_credit);
        assert_eq!(salary.amount_minor, 5_000_000);
        assert!(rows.iter().filter(|r| !r.is_credit).count() == 2);
    }

    #[test]
    fn a_negative_single_column_is_money_leaving() {
        let m = guess(&headers(US).unwrap());
        let (rows, _) = parse(US, &m, DateFormat::MonthFirst, "USD").unwrap();
        let netflix = rows.iter().find(|r| r.description == "NETFLIX.COM").unwrap();
        assert_eq!(netflix.amount_minor, 999);
        assert!(!netflix.is_credit);
        let pay = rows.iter().find(|r| r.description == "PAYROLL").unwrap();
        assert!(pay.is_credit);
    }

    #[test]
    fn a_trailing_summary_line_is_skipped_and_counted() {
        let m = guess(&headers(HDFC).unwrap());
        let (rows, skipped) = parse(HDFC, &m, DateFormat::DayFirst, "INR").unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(skipped, 1, "the closing-balance line is not a transaction");
    }

    #[test]
    fn the_date_format_changes_what_the_rows_mean() {
        let m = guess(&headers(HDFC).unwrap());
        let (day_first, _) = parse(HDFC, &m, DateFormat::DayFirst, "INR").unwrap();
        let (month_first, _) = parse(HDFC, &m, DateFormat::MonthFirst, "INR").unwrap();
        assert_eq!(day_first[0].occurred_on, "2026-07-05");
        assert_eq!(month_first[0].occurred_on, "2026-05-07");
        // 26/07 is not a valid month-first date, so that row drops out — which is
        // itself the signal the sniffer uses.
        assert!(month_first.len() < day_first.len());
    }

    #[test]
    fn date_sniffing_reads_the_real_column() {
        let m = guess(&headers(HDFC).unwrap());
        let samples = date_samples(HDFC, &m, 10).unwrap();
        assert_eq!(dates::sniff(&samples), Some(DateFormat::DayFirst));
    }

    #[test]
    fn an_unusable_mapping_is_refused_before_anything_posts() {
        let empty = ColumnMap::default();
        assert!(parse(HDFC, &empty, DateFormat::DayFirst, "INR").is_err());
        let no_amount =
            ColumnMap { date: "Date".into(), description: "Narration".into(), ..Default::default() };
        assert!(parse(HDFC, &no_amount, DateFormat::DayFirst, "INR").is_err());
    }

    #[test]
    fn a_mapping_naming_a_column_that_is_not_there_is_an_error_not_a_silent_skip() {
        let m = ColumnMap {
            date: "Nope".into(),
            description: "Narration".into(),
            amount: Some("Withdrawal Amt.".into()),
            ..Default::default()
        };
        assert!(parse(HDFC, &m, DateFormat::DayFirst, "INR").is_err());
    }

    #[test]
    fn column_maps_round_trip_through_json() {
        let m = guess(&headers(HDFC).unwrap());
        let back = ColumnMap::from_json(&m.to_json()).unwrap();
        assert_eq!(m, back);
        assert!(ColumnMap::from_json("not json").is_err());
    }

    #[test]
    fn header_matching_is_case_and_wording_tolerant() {
        let m = guess(&h(&["VALUE DATE", "Particulars", "Debit", "Credit"]));
        assert_eq!(m.date, "VALUE DATE");
        assert_eq!(m.description, "Particulars");
        assert_eq!(m.debit.as_deref(), Some("Debit"));
    }
}
