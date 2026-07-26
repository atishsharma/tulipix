//! Statement PDFs, turned into the CSV text the rest of the importer already reads.
//!
//! Banks that will not give you a CSV will usually give you a PDF, and increasingly
//! that is the only machine-readable thing on offer. So this does not add a second
//! import pipeline: it extracts the text, works out where the columns are, and
//! writes CSV. Everything after that — column mapping, date sniffing, the preview,
//! the dedup index — is the code that was already there and already tested.
//!
//! # This is a heuristic, and it is presented as one
//!
//! A PDF has no columns. It has glyphs at coordinates, and a table is a visual
//! convention. What comes out of text extraction is lines of text where the columns
//! *were*, separated by runs of spaces — usually. So:
//!
//! 1. A line splits on runs of two or more spaces, which is what a laid-out table
//!    leaves behind.
//! 2. Failing that, a line with a date at the front and amounts at the back is
//!    split by those, with everything between them as the description.
//! 3. Anything that is not either of those is dropped, and counted as unreadable so
//!    the preview can say how many lines it could not read.
//!
//! The user sees every row before anything is posted and can correct the column
//! mapping, which is the safety net that makes a heuristic acceptable here. A silent
//! import of a misread PDF would not be.

use anyhow::{bail, Context, Result};

/// Extract a statement PDF as CSV text.
///
/// Returns the CSV — a header line plus one line per row that looked like a
/// transaction — and how many lines were skipped.
pub fn to_csv(bytes: &[u8]) -> Result<(String, usize)> {
    let text = extract_text(bytes)?;
    Ok(rows_to_csv(&text))
}

/// Every page's text, in order.
///
/// `lopdf` decompresses and decodes the content streams; page order is taken from
/// the page tree rather than from the object order, which is not the same thing in a
/// PDF assembled by a bank's report generator.
fn extract_text(bytes: &[u8]) -> Result<String> {
    let doc = lopdf::Document::load_mem(bytes).context("that PDF could not be opened")?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    if pages.is_empty() {
        bail!("that PDF has no pages");
    }
    let mut out = String::new();
    for p in pages {
        // One page at a time: a statement can be forty pages and one damaged page
        // must not lose the other thirty-nine.
        match doc.extract_text(&[p]) {
            Ok(t) => {
                out.push_str(&t);
                out.push('\n');
            }
            Err(e) => tracing::debug!("finances: page {p} of the PDF had no readable text: {e}"),
        }
    }
    if out.trim().is_empty() {
        bail!("that PDF has no text in it — it may be a scan, which the receipt scanner reads");
    }
    Ok(out)
}

/// The widest row wins the column count, because a statement's header line is
/// often *narrower* than its rows (two words over three columns).
fn rows_to_csv(text: &str) -> (String, usize) {
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut unreadable = 0;
    let mut header: Option<Vec<String>> = None;

    for line in text.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            continue;
        }
        let Some(fields) = split_row(line) else {
            unreadable += 1;
            continue;
        };
        // The header is the first line that names a date column and something that
        // could be a description. Taken from the file when it is there, because the
        // user's own bank's wording is what the column guesser is best at.
        if header.is_none() && is_header(&fields) {
            header = Some(fields);
            continue;
        }
        // A row has to have a date in it somewhere, or it is a page footer, a
        // balance summary or an address.
        if fields.iter().any(|f| looks_like_date(f)) {
            rows.push(fields);
        } else {
            unreadable += 1;
        }
    }

    let width = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let header = match header {
        Some(h) if h.len() >= width => h,
        // No header line, or one narrower than the rows: synthesise one. The guesser
        // needs *some* header to map columns from, and these are the names it knows.
        _ => synthetic_header(width),
    };

    let mut out = String::new();
    write_csv_line(&mut out, &header);
    for r in &rows {
        write_csv_line(&mut out, r);
    }
    (out, unreadable)
}

/// Column names the CSV guesser recognises, for a file that came without any.
fn synthetic_header(width: usize) -> Vec<String> {
    let names = ["Date", "Description", "Debit", "Credit", "Balance"];
    (0..width.max(3))
        .map(|i| names.get(i).map(|s| s.to_string()).unwrap_or_else(|| format!("Column {}", i + 1)))
        .collect()
}

fn is_header(fields: &[String]) -> bool {
    let joined = fields.join(" ").to_lowercase();
    joined.contains("date")
        && ["description", "narration", "particulars", "details", "remarks", "transaction"]
            .iter()
            .any(|k| joined.contains(k))
}

/// Split one extracted line into fields.
fn split_row(line: &str) -> Option<Vec<String>> {
    // A laid-out table leaves runs of spaces where the column gaps were.
    let wide: Vec<String> = line
        .split("  ")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if wide.len() >= 3 {
        return Some(split_trailing_amounts(wide));
    }

    // Otherwise: a date at the front and amounts at the back, description between.
    // This is the shape of a statement line whose columns collapsed to single
    // spaces, which happens with proportional fonts.
    let words: Vec<&str> = line.split_whitespace().collect();
    if words.len() < 3 {
        return None;
    }
    if !looks_like_date(words[0]) {
        return None;
    }
    // Amounts are taken from the right, and only while they keep being amounts: the
    // last two are typically the transaction and the running balance.
    let mut tail = Vec::new();
    let mut end = words.len();
    while end > 1 && looks_like_amount(words[end - 1]) && tail.len() < 3 {
        tail.push(words[end - 1].to_string());
        end -= 1;
    }
    if tail.is_empty() || end < 2 {
        return None;
    }
    tail.reverse();

    let mut out = vec![words[0].to_string(), words[1..end].join(" ")];
    out.extend(tail);
    Some(out)
}

/// Split a final field that is really two amounts with one space between them.
///
/// Common in the wide-gap case: the withdrawal column is empty, so the deposit and
/// the balance end up in the same field. Left merged, the balance would be read as
/// the amount of the transaction.
fn split_trailing_amounts(mut fields: Vec<String>) -> Vec<String> {
    let Some(last) = fields.pop() else { return fields };
    let parts: Vec<&str> = last.split_whitespace().collect();
    if parts.len() > 1 && parts.iter().all(|p| looks_like_amount(p)) {
        fields.extend(parts.into_iter().map(str::to_string));
    } else {
        fields.push(last);
    }
    fields
}

/// Digits and separators in a date-shaped arrangement. Deliberately loose about
/// which way round the day and month are — [`super::dates`] decides that, from the
/// whole file, which is the only place it can be decided correctly.
pub(super) fn looks_like_date(s: &str) -> bool {
    let t = s.trim();
    // A thousands separator is never part of a date, and counting digits alone would
    // read "4,620.00" as one: six digits either side of a dot.
    if t.contains(',') {
        return false;
    }
    let parts: Vec<&str> = t.split(['/', '-', '.', ' ']).filter(|p| !p.is_empty()).collect();
    if parts.len() != 3 {
        return false;
    }
    let digits = |p: &str| p.chars().all(|c| c.is_ascii_digit());
    // Day or month: one or two digits, or a three-letter month name.
    let small = |p: &str| (digits(p) && (1..=2).contains(&p.len())) || is_month_name(p);
    let year = |p: &str| digits(p) && (p.len() == 2 || p.len() == 4);
    // Year at either end, and nothing three digits wide anywhere — which is what
    // keeps "1.234.567" from reading as a date.
    (small(parts[0]) && small(parts[1]) && year(parts[2]))
        || (year(parts[0]) && small(parts[1]) && small(parts[2]))
}

fn is_month_name(p: &str) -> bool {
    p.len() == 3 && p.chars().all(|c| c.is_ascii_alphabetic())
}

fn looks_like_amount(s: &str) -> bool {
    let t = s.trim().trim_end_matches(|c| c == 'r' || c == 'R' || c == 'D' || c == 'C');
    !t.is_empty()
        && t.chars().any(|c| c.is_ascii_digit())
        && t.chars().all(|c| c.is_ascii_digit() || matches!(c, ',' | '.' | '-' | '+' | '(' | ')'))
}

/// One CSV line, quoted properly.
///
/// Statement descriptions contain commas — "BIGBASKET RETAIL, BANGALORE" — and a
/// naive join would shift every column after it by one.
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

    #[test]
    fn a_column_aligned_statement_becomes_csv() {
        let text = "\
HDFC BANK LTD
Statement of account
Date        Narration                     Withdrawal    Deposit     Balance
01/07/2026  BIGBASKET RETAIL BANGALORE    4,620.00                  1,23,456.00
02/07/2026  SALARY ACME CORP                            1,25,000.00 2,48,456.00
Closing balance                                                     2,48,456.00
";
        let (csv, unreadable) = rows_to_csv(text);
        let lines: Vec<&str> = csv.lines().collect();
        assert_eq!(lines[0], "Date,Narration,Withdrawal,Deposit,Balance");
        assert_eq!(lines[1], "01/07/2026,BIGBASKET RETAIL BANGALORE,\"4,620.00\",\"1,23,456.00\"");
        assert_eq!(lines.len(), 3, "two transactions and a header");
        // The deposit and the balance were one space apart, so they arrived in one
        // field. Split, or the balance would be read as the amount of the deposit.
        assert_eq!(lines[2], "02/07/2026,SALARY ACME CORP,\"1,25,000.00\",\"2,48,456.00\"");
        // The bank name, the title and the closing-balance line.
        assert!(unreadable >= 2, "got {unreadable}");
    }

    #[test]
    fn a_single_spaced_statement_is_split_by_its_date_and_amounts() {
        // The harder shape: the column gaps came out as single spaces, so the only
        // structure left is "date, words, numbers".
        let row = split_row("01-Jul-2026 UPI PAYMENT TO SWIGGY 680.00 1,22,776.00").unwrap();
        assert_eq!(row[0], "01-Jul-2026");
        assert_eq!(row[1], "UPI PAYMENT TO SWIGGY");
        assert_eq!(row[2], "680.00");
        assert_eq!(row[3], "1,22,776.00");
    }

    #[test]
    fn lines_that_are_not_transactions_are_refused_rather_than_guessed() {
        // Page furniture. Turning any of these into a row would put an invented
        // amount in someone's ledger.
        assert!(split_row("Page 1 of 4").is_none());
        assert!(split_row("Registered office: Mumbai").is_none());
        assert!(split_row("Opening balance 1,00,000.00").is_none(), "no date");
        // Two fields is not a row.
        assert!(split_row("01/07/2026 4,620.00").is_none());
    }

    #[test]
    fn a_description_with_a_comma_does_not_shift_the_columns() {
        let mut out = String::new();
        write_csv_line(&mut out, &["01/07/2026".into(), "BIGBASKET, BANGALORE".into(), "10".into()]);
        assert_eq!(out, "01/07/2026,\"BIGBASKET, BANGALORE\",10\n");
    }

    #[test]
    fn a_statement_with_no_header_line_gets_one_it_can_be_mapped_from() {
        let text = "01/07/2026  SWIGGY  680.00  1,22,776.00\n";
        let (csv, _) = rows_to_csv(text);
        assert_eq!(csv.lines().next().unwrap(), "Date,Description,Debit,Credit");
    }

    #[test]
    fn dates_are_recognised_in_every_shape_a_bank_uses() {
        assert!(looks_like_date("01/07/2026"));
        assert!(looks_like_date("2026-07-01"));
        assert!(looks_like_date("01-Jul-2026"));
        assert!(looks_like_date("01.07.2026"));
        // Not dates: an amount, a reference number, a year on its own.
        assert!(!looks_like_date("4,620.00"));
        assert!(!looks_like_date("2026"));
        assert!(!looks_like_date("UPI"));
    }
}
