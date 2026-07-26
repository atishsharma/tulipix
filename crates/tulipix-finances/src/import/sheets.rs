//! Statement spreadsheets — `.xlsx` and the legacy binary `.xls` — as CSV text.
//!
//! Same approach as [`super::pdf`]: convert to CSV and let the existing pipeline do
//! the rest, rather than growing a second importer with its own column mapping and
//! its own date bugs.
//!
//! A spreadsheet is easier than a PDF, because it really does have cells. Two things
//! still need care:
//!
//! **Dates.** A spreadsheet date is a number of days since 1899-12-30, and the cell
//! knows it is a date only through its format. `calamine` resolves that, and the ISO
//! text it produces is handed on — so a `.xlsx` never goes through the
//! day-first/month-first guess at all, because there is nothing to guess.
//!
//! **Preamble.** Bank exports open with the account holder's name, the account
//! number and a date range before the table starts. So the header row is *found*
//! rather than assumed to be row 1, and everything above it is dropped.

use anyhow::{bail, Context, Result};
use calamine::{Data, Reader};

/// Extract a statement spreadsheet as CSV text.
///
/// Returns the CSV and how many rows were skipped as not being transactions.
pub fn to_csv(bytes: &[u8]) -> Result<(String, usize)> {
    let cursor = std::io::Cursor::new(bytes.to_vec());
    let mut wb = calamine::open_workbook_auto_from_rs(cursor)
        .context("that spreadsheet could not be opened")?;
    let Some(name) = wb.sheet_names().first().cloned() else {
        bail!("that spreadsheet has no sheets");
    };
    let range = wb.worksheet_range(&name).context("that sheet could not be read")?;

    let rows: Vec<Vec<String>> =
        range.rows().map(|r| r.iter().map(cell_text).collect()).collect();
    Ok(rows_to_csv(&rows))
}

/// One cell as the text a CSV would have carried.
///
/// `DateTime` is rendered ISO on purpose: it is the one format
/// [`super::dates::sniff`] never has to guess at, and a spreadsheet knows which of
/// its numbers are dates even when the file's own display format is `dd/mm`.
fn cell_text(c: &Data) -> String {
    match c {
        Data::Empty => String::new(),
        Data::String(s) => s.trim().to_string(),
        Data::Float(f) => {
            // Whole floats print as integers: a cell holding 4620 is "4620", not
            // "4620.0", which the amount parser would then read to two decimals.
            if f.fract() == 0.0 && f.abs() < 1e15 {
                format!("{}", *f as i64)
            } else {
                f.to_string()
            }
        }
        Data::Int(i) => i.to_string(),
        Data::Bool(b) => b.to_string(),
        Data::DateTime(d) => d
            .as_datetime()
            .map(|dt| dt.date().to_string())
            .unwrap_or_else(|| d.as_f64().to_string()),
        Data::DateTimeIso(s) | Data::DurationIso(s) => s.trim().to_string(),
        Data::Error(_) => String::new(),
    }
}

fn rows_to_csv(rows: &[Vec<String>]) -> (String, usize) {
    // Where the table starts. Bank exports put the account holder, the account
    // number and a date range above it, and treating row 1 as the header would map
    // every column from someone's name.
    let header_at = rows.iter().position(|r| is_header(r));
    let (header, body) = match header_at {
        Some(i) => (rows[i].clone(), &rows[i + 1..]),
        // No recognisable header: give the guesser the names it knows, and keep
        // every row — a sheet that is only data is a legitimate export.
        None => {
            let width = rows.iter().map(Vec::len).max().unwrap_or(0);
            (synthetic_header(width), rows)
        }
    };

    let mut out = String::new();
    write_csv_line(&mut out, &trim_trailing_empties(&header));
    // Everything above the header row was preamble, and is reported as such.
    let mut unreadable = header_at.unwrap_or(0);
    for r in body {
        // A row with nothing in it, or nothing but a total, is not a transaction.
        // `csv::parse` would reject these too; counting them here is what lets the
        // preview say how many lines of the file it did not use.
        if r.iter().all(|c| c.trim().is_empty()) || !looks_like_row(r) {
            unreadable += 1;
            continue;
        }
        write_csv_line(&mut out, &trim_trailing_empties(r));
    }
    (out, unreadable)
}

/// A transaction row has a date-shaped cell and at least one number in it.
///
/// Both, not either: a "Closing balance" line has the number, and a section heading
/// can have the date.
fn looks_like_row(r: &[String]) -> bool {
    let dated = r.iter().any(|c| super::pdf::looks_like_date(c));
    // The date cell is itself all digits and separators, so it has to be taken out of
    // the running before asking whether anything here is a number — otherwise a
    // "Statement period" line passes on the strength of its own date.
    let numeric = r
        .iter()
        .filter(|c| !c.trim().is_empty() && !super::pdf::looks_like_date(c))
        .any(|c| {
            c.chars().any(|ch| ch.is_ascii_digit())
                && c.chars()
                    .all(|ch| ch.is_ascii_digit() || matches!(ch, ',' | '.' | '-' | '+' | ' '))
        });
    dated && numeric
}

fn is_header(r: &[String]) -> bool {
    let joined = r.join(" ").to_lowercase();
    joined.contains("date")
        && ["description", "narration", "particulars", "details", "remarks", "transaction"]
            .iter()
            .any(|k| joined.contains(k))
}

fn synthetic_header(width: usize) -> Vec<String> {
    let names = ["Date", "Description", "Debit", "Credit", "Balance"];
    (0..width.max(3))
        .map(|i| names.get(i).map(|s| s.to_string()).unwrap_or_else(|| format!("Column {}", i + 1)))
        .collect()
}

/// Spreadsheets are ragged to the right — a sheet is often 16,384 columns wide with
/// four of them used. Trailing blanks would become trailing commas in every line.
fn trim_trailing_empties(r: &[String]) -> Vec<String> {
    let end = r.iter().rposition(|c| !c.trim().is_empty()).map_or(0, |i| i + 1);
    r[..end].to_vec()
}

fn write_csv_line(out: &mut String, fields: &[String]) {
    for (i, f) in fields.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        if f.contains([',', '"', '\n']) {
            out.push('"');
            out.push_str(&f.replace('"', "\"\""));
            out.push('"');
        } else {
            out.push_str(f);
        }
    }
    out.push('\n');
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(cells: &[&str]) -> Vec<String> {
        cells.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_preamble_above_the_table_is_dropped() {
        // What an ICICI or Axis export actually looks like: four lines of account
        // details, then the table. Mapping columns from row 1 would map them from
        // the account holder's name.
        let rows = vec![
            row(&["Detailed Statement", "", "", ""]),
            row(&["Account Name", "A K SHARMA", "", ""]),
            row(&["Account Number", "00123456789", "", ""]),
            row(&[]),
            row(&["Date", "Narration", "Withdrawal", "Deposit"]),
            row(&["2026-07-01", "BIGBASKET RETAIL", "4620", ""]),
            row(&["2026-07-02", "SALARY ACME CORP", "", "125000"]),
            row(&["", "Closing balance", "", "248456"]),
        ];
        let (csv, unreadable) = rows_to_csv(&rows);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Date,Narration,Withdrawal,Deposit");
        assert_eq!(lines[1], "2026-07-01,BIGBASKET RETAIL,4620");
        assert_eq!(lines[2], "2026-07-02,SALARY ACME CORP,,125000");
        assert_eq!(lines.len(), 3);
        // Four preamble rows and the closing-balance line.
        assert_eq!(unreadable, 5);
    }

    #[test]
    fn a_sheet_with_no_header_keeps_all_of_its_rows() {
        let rows = vec![
            row(&["2026-07-01", "SWIGGY", "680"]),
            row(&["2026-07-02", "UBER", "240"]),
        ];
        let (csv, unreadable) = rows_to_csv(&rows);
        assert_eq!(csv.lines().next().unwrap(), "Date,Description,Debit");
        assert_eq!(csv.lines().count(), 3);
        assert_eq!(unreadable, 0);
    }

    #[test]
    fn a_whole_amount_does_not_arrive_with_a_spurious_decimal() {
        // 4620.0 parsed at two decimals is ₹46.20. The cell holds a float because
        // every number in a spreadsheet does.
        assert_eq!(cell_text(&Data::Float(4620.0)), "4620");
        assert_eq!(cell_text(&Data::Float(4620.5)), "4620.5");
        assert_eq!(cell_text(&Data::Int(4620)), "4620");
        assert_eq!(cell_text(&Data::Empty), "");
    }

    #[test]
    fn the_thousands_of_empty_columns_a_sheet_carries_are_trimmed() {
        let r = row(&["2026-07-01", "SWIGGY", "680", "", "", "", ""]);
        assert_eq!(trim_trailing_empties(&r).len(), 3);
    }

    #[test]
    fn a_total_row_is_not_a_transaction() {
        assert!(!looks_like_row(&row(&["", "Total", "", "248456"])), "no date");
        assert!(!looks_like_row(&row(&["2026-07-01", "Statement period", ""])), "no amount");
        assert!(looks_like_row(&row(&["2026-07-01", "SWIGGY", "680"])));
    }
}
