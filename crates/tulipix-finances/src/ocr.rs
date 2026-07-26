//! Receipt reading: a photo in, a half-filled add sheet out.
//!
//! Two halves, split so the interesting one is testable. [`read`] shells out to
//! `tesseract`; [`parse`] turns whatever text came back into a guess. Only
//! [`parse`] has tests, because a test that needs a working OCR binary and a
//! JPEG of a Blinkit receipt is a test that never runs in CI.
//!
//! **Why a subprocess and not a bundled model.** An embedded OCR model is
//! 15-40 MB of weights, a runtime, and a per-platform build matrix, for a
//! feature that fills in three fields the user can also type. `tesseract` is one
//! `pacman -S tesseract tesseract-data-eng` away, and where it is missing this
//! reports that plainly instead of failing mysteriously.
//!
//! **Nothing here posts anything.** The output is a suggestion that lands in the
//! add sheet for the user to correct. An OCR'd total read from a crumpled
//! receipt is exactly the kind of number that must not become a ledger fact
//! without a human looking at it — the same rule as an estimate.

use anyhow::{Context, Result, bail};
use std::path::Path;
use std::process::Command;

/// What could be read off the receipt. Every field is optional: a blurry photo
/// that yields only a total is still worth pre-filling.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Receipt {
    pub merchant: Option<String>,
    pub amount_minor: Option<i64>,
    pub occurred_on: Option<String>,
    /// The raw text, so the user can see what was actually read when the guess
    /// is wrong.
    pub text: String,
}

/// Whether OCR is available at all. The UI asks first so it can explain the
/// missing binary rather than showing a failure after the file picker.
pub fn available() -> bool {
    Command::new("tesseract")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Run `tesseract` over an image and parse what comes back.
///
/// `stdout` as the output target (`-`) rather than a temp file, so there is
/// nothing to clean up and no collision between two runs.
pub fn read(path: &Path, currency: &str) -> Result<Receipt> {
    if !path.is_file() {
        bail!("{} is not a file", path.display());
    }
    let out = Command::new("tesseract")
        .arg(path)
        .arg("-")
        .arg("--psm")
        // 6 = "assume a single uniform block of text", which is what a receipt
        // is. The default page-segmentation mode looks for columns and shuffles
        // a receipt's lines out of order.
        .arg("6")
        .output()
        .context("tesseract is not installed, or not on PATH")?;

    if !out.status.success() {
        bail!(
            "tesseract failed: {}",
            String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("no output").trim()
        );
    }
    Ok(parse(&String::from_utf8_lossy(&out.stdout), currency))
}

/// How much a line's wording suggests it carries the figure that matters.
/// Higher wins, so "Grand total" beats a plain "Total", which beats "Sub total"
/// on a receipt that prints all three.
///
/// Explicit branches rather than a word list scanned for the first hit: every
/// specific phrase here *contains* a generic one ("grand total" contains
/// "total"), so a list would always match the generic word first and the
/// specific entries would be unreachable.
fn total_rank(lower: &str) -> Option<usize> {
    if lower.contains("subtotal") || lower.contains("sub total") {
        return Some(0);
    }
    if lower.contains("grand total") || lower.contains("payable") || lower.contains("net total") {
        return Some(3);
    }
    if lower.contains("total") || lower.contains("amount") || lower.contains("paid") {
        return Some(2);
    }
    None
}

/// Turn OCR text into a guess.
pub fn parse(text: &str, currency: &str) -> Receipt {
    let mut r = Receipt { text: text.to_string(), ..Default::default() };

    // Merchant: the first line with letters in it. On nearly every receipt the
    // shop's name is the first thing printed, and the alternative — matching
    // against a list of known merchants — is a list that has to be maintained
    // forever.
    r.merchant = text
        .lines()
        .map(str::trim)
        .find(|l| l.chars().filter(|c| c.is_alphabetic()).count() >= 3)
        .map(tidy);

    r.amount_minor = amount(text, currency);
    r.occurred_on = date(text);
    r
}

/// Strip the OCR noise that turns "BLINKIT" into "BLINKIT ~ |".
fn tidy(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_alphanumeric() || " &'-.,".contains(*c))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// The best candidate for the amount.
///
/// Prefers a number on a line that says "total"; falls back to the largest
/// number on the receipt, which on a till slip is nearly always the total
/// anyway.
fn amount(text: &str, currency: &str) -> Option<i64> {
    let mut best_rank: Option<usize> = None;
    let mut best: Option<i64> = None;
    let mut largest: Option<i64> = None;

    for line in text.lines() {
        let rank = total_rank(&line.to_lowercase());
        for n in numbers(line, currency) {
            largest = Some(largest.map_or(n, |m: i64| m.max(n)));
            // `>=` so the last of two equally-ranked lines wins: a receipt that
            // prints the total twice prints the payable one second.
            if let Some(rank) = rank
                && best_rank.is_none_or(|b| rank >= b)
            {
                best_rank = Some(rank);
                best = Some(n);
            }
        }
    }
    best.or(largest)
}

/// Every money-shaped number on a line, in minor units.
///
/// A bare integer is treated as whole units — `Total 450` means ₹450, not ₹4.50
/// — because no receipt prints a total without a separator when it has paise.
fn numbers(line: &str, currency: &str) -> Vec<i64> {
    let mut out = Vec::new();
    let bytes: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < bytes.len() {
        if !bytes[i].is_ascii_digit() {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == ',' || bytes[i] == '.') {
            i += 1;
        }
        let raw: String = bytes[start..i].iter().collect();
        // A trailing separator belongs to the sentence, not the number.
        let raw = raw.trim_end_matches([',', '.']);
        if raw.chars().filter(|c| c.is_ascii_digit()).count() > 12 {
            continue; // a phone number, an invoice id, a GSTIN
        }
        if let Ok(v) = crate::money::parse_amount(raw, currency) {
            out.push(v);
        }
    }
    out
}

/// Formats a till slip actually prints, most specific first.
const DATE_FORMATS: &[&str] =
    &["%d/%m/%Y", "%d-%m-%Y", "%Y-%m-%d", "%d.%m.%Y", "%d %b %Y", "%d %B %Y", "%d/%m/%y", "%d-%m-%y"];

/// The first parseable date in the text.
///
/// Day-first only. This is a single receipt with no other rows to cross-check
/// against, so the ambiguity that [`crate::import::dates::sniff`] resolves by
/// looking at a whole statement cannot be resolved here — and day-first is what
/// the receipts in this app's home country print.
fn date(text: &str) -> Option<String> {
    let singles = text.split_whitespace().map(str::to_string);
    for token in singles.chain(runs_of_date_chars(text)) {
        let t = token.trim_matches(|c: char| !c.is_alphanumeric());
        for f in DATE_FORMATS {
            if let Ok(d) = chrono::NaiveDate::parse_from_str(t, f) {
                return Some(crate::date::iso(d));
            }
        }
    }
    None
}

/// `19 Jul 2026` is three whitespace-separated tokens, so the month-name formats
/// need the neighbours glued back together.
fn runs_of_date_chars(text: &str) -> Vec<String> {
    let words: Vec<&str> = text.split_whitespace().collect();
    words.windows(3).map(|w| w.join(" ")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SLIP: &str = "
        BLINKIT ~ |
        Bengaluru 560095
        GSTIN 29AABCU9603R1ZX
        19/07/2026  14:32
        Milk 1L            62.00
        Bread              45.00
        Sub total         107.00
        Delivery           25.00
        Grand Total       132.00
        Card **** 4412
    ";

    #[test]
    fn a_till_slip_yields_all_three_fields() {
        let r = parse(SLIP, "INR");
        assert_eq!(r.merchant.as_deref(), Some("BLINKIT"));
        assert_eq!(r.amount_minor, Some(13_200), "the grand total, not the sub total");
        assert_eq!(r.occurred_on.as_deref(), Some("2026-07-19"));
    }

    #[test]
    fn the_grand_total_beats_the_sub_total() {
        // Ranking, not position: "Sub total" appears second here.
        let t = "Grand Total 132.00\nSub total 107.00";
        assert_eq!(amount(t, "INR"), Some(13_200));
    }

    #[test]
    fn with_no_total_line_the_largest_number_wins() {
        let t = "Chai 20.00\nSamosa 15.00\n35.00";
        assert_eq!(amount(t, "INR"), Some(3_500));
    }

    #[test]
    fn long_digit_runs_are_not_money() {
        // A GSTIN, a phone number and a card number must not become the total.
        let t = "GSTIN 29AABCU9603R1ZX\nPh 9876543210\nTotal 450";
        assert_eq!(amount(t, "INR"), Some(45_000), "a bare 450 is ₹450, not ₹4.50");
    }

    #[test]
    fn a_month_name_date_parses() {
        assert_eq!(date("Issued 19 Jul 2026 at 14:32").as_deref(), Some("2026-07-19"));
    }

    #[test]
    fn nothing_readable_yields_nothing_rather_than_a_wrong_guess() {
        let r = parse("|| ~~ ..", "INR");
        assert_eq!(r.amount_minor, None);
        assert_eq!(r.occurred_on, None);
        assert_eq!(r.merchant, None);
    }

    #[test]
    fn the_raw_text_is_kept_so_a_bad_read_is_visible() {
        let r = parse("Total 99.00", "INR");
        assert_eq!(r.text, "Total 99.00");
    }
}
