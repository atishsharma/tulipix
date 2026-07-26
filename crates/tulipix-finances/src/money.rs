//! Money as integers, start to finish.
//!
//! Every amount in this section is an `i64` count of minor units — paise for
//! INR, cents for USD. No `f64` reaches the database, and none is used to add
//! two amounts together, because a thousand additions of 0.33 in binary
//! floating point does not equal 330.
//!
//! Exchange rates are integers too, in millionths (`83.60` → `83_600_000`), so
//! converting a foreign amount to the base currency is integer arithmetic as
//! well and cannot drift between two runs.
//!
//! Parsing and formatting are the only places a decimal point exists, and both
//! are here rather than at the call sites.

use anyhow::{bail, Result};

/// Millionths. `1.0` as a rate is `1_000_000`.
pub const RATE_ONE: i64 = 1_000_000;

/// Minor units per major unit for `code`.
///
/// Almost every currency is two decimals; the exceptions are the ones where a
/// wrong answer here silently multiplies every amount by a hundred, so they are
/// named. Unknown codes get two, which is the safe guess.
pub fn decimals(code: &str) -> u32 {
    match code {
        // Zero-decimal currencies: the minor unit is the major unit.
        "JPY" | "KRW" | "VND" | "IDR" | "ISK" | "CLP" | "XOF" | "XAF" => 0,
        // Three-decimal currencies.
        "BHD" | "IQD" | "JOD" | "KWD" | "LYD" | "OMR" | "TND" => 3,
        _ => 2,
    }
}

fn symbol(code: &str) -> Option<&'static str> {
    match code {
        "INR" => Some("₹"),
        "USD" => Some("$"),
        "EUR" => Some("€"),
        "GBP" => Some("£"),
        "JPY" => Some("¥"),
        _ => None,
    }
}

/// Convert `amount_minor` in some currency to base-currency minor units at
/// `rate_micro`, truncating toward zero.
///
/// The multiply is done in `i128`: a lakh-scale amount times a rate near
/// 83_600_000 is around 8.4e18, which is inside `i64` only just, and a larger
/// amount or a weaker currency overflows it. Widening costs nothing here and
/// removes the ceiling entirely.
pub fn convert(amount_minor: i64, rate_micro: i64) -> i64 {
    let scaled = (amount_minor as i128) * (rate_micro as i128) / (RATE_ONE as i128);
    scaled as i64
}

/// The rate that leaves an amount unchanged. Stored on base-currency rows so
/// `rate_micro` is never null and `base_minor` is always `convert`ed the same
/// way, whatever the currency.
pub const fn identity_rate() -> i64 {
    RATE_ONE
}

/// Round `minor` to the nearest whole `step` minor units, half away from zero.
///
/// Used for the variable-bill estimate, which the design requires be rounded to
/// the nearest ₹10 so it reads as the guess it is rather than as a measurement.
pub fn round_to(minor: i64, step: i64) -> i64 {
    if step <= 1 {
        return minor;
    }
    let (sign, mag) = if minor < 0 { (-1i64, -(minor as i128)) } else { (1i64, minor as i128) };
    let step = step as i128;
    let rounded = (mag + step / 2) / step * step;
    sign * rounded as i64
}

/// Parse a decimal string into `10^decimals` scaled integer units.
///
/// Tolerates what a person actually types or a bank actually exports: grouping
/// commas in either the Indian or Western position, a leading currency symbol,
/// surrounding space, a leading `+`, and a trailing bare `.`. Extra fractional
/// digits round half away from zero rather than being dropped, so `40.555`
/// at two decimals is `4056`, not `4055`.
pub fn parse_scaled(s: &str, decimals: u32) -> Result<i64> {
    let t = s.trim();
    if t.is_empty() {
        bail!("empty amount");
    }
    let mut neg = false;
    let mut int = String::new();
    let mut frac = String::new();
    let mut seen_dot = false;
    let mut seen_digit = false;

    for c in t.chars() {
        match c {
            // Matched on "before any digit" rather than "at index 0" so that
            // `₹-40` is negative. Keyed on position, the sign would be skipped
            // by the currency-symbol arm below and the amount would come out
            // positive.
            '-' if !seen_digit => neg = true,
            '+' if !seen_digit => {}
            // Parenthesised negatives: some statement exports write (1,234.00).
            '(' if !seen_digit => neg = true,
            ')' if neg => {}
            ',' | ' ' | '\u{a0}' | '\'' | '_' => {}
            '.' => {
                if seen_dot {
                    bail!("two decimal points in {s:?}");
                }
                seen_dot = true;
            }
            '0'..='9' => {
                seen_digit = true;
                if seen_dot { frac.push(c) } else { int.push(c) }
            }
            // A currency symbol or code is allowed only before any digit.
            _ if !seen_digit && !c.is_ascii_alphanumeric() => {}
            _ if !seen_digit && c.is_alphabetic() => {}
            _ => bail!("unexpected {c:?} in {s:?}"),
        }
    }
    if !seen_digit {
        bail!("no digits in {s:?}");
    }

    let want = decimals as usize;
    // One extra digit is all that is needed to decide the rounding.
    let round_up = frac.len() > want && frac.as_bytes()[want] >= b'5';
    frac.truncate(want);
    while frac.len() < want {
        frac.push('0');
    }

    let digits = format!("{int}{frac}");
    let mut v: i64 = digits.parse().map_err(|_| anyhow::anyhow!("amount out of range: {s:?}"))?;
    if round_up {
        v += 1;
    }
    Ok(if neg { -v } else { v })
}

/// Parse a user-entered amount into minor units of `code`.
pub fn parse_amount(s: &str, code: &str) -> Result<i64> {
    parse_scaled(s, decimals(code))
}

/// Parse a hand-edited exchange rate into millionths.
pub fn parse_rate(s: &str) -> Result<i64> {
    parse_scaled(s, 6)
}

/// Group an unsigned digit string the Indian way: last three, then pairs.
/// `12430000` → `1,24,30,000`.
fn group_indian(digits: &str) -> String {
    if digits.len() <= 3 {
        return digits.to_string();
    }
    let (head, tail) = digits.split_at(digits.len() - 3);
    let mut parts: Vec<String> = Vec::new();
    let bytes = head.as_bytes();
    let mut end = bytes.len();
    while end > 0 {
        let start = end.saturating_sub(2);
        parts.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
        end = start;
    }
    parts.reverse();
    format!("{},{tail}", parts.join(","))
}

/// Group an unsigned digit string in threes. `1234567` → `1,234,567`.
fn group_western(digits: &str) -> String {
    let bytes = digits.as_bytes();
    let mut parts: Vec<String> = Vec::new();
    let mut end = bytes.len();
    while end > 0 {
        let start = end.saturating_sub(3);
        parts.push(String::from_utf8_lossy(&bytes[start..end]).into_owned());
        end = start;
    }
    parts.reverse();
    parts.join(",")
}

/// Format minor units for display: `₹1,24,300`, `₹3,284.50`, `-$40.10`,
/// `CHF 12.00`.
///
/// The fractional part is omitted when it is zero. Most amounts in this section
/// are whole rupees, and printing `₹649.00` everywhere adds three characters of
/// noise to every row of every table for no information.
pub fn format_minor(minor: i64, code: &str) -> String {
    let dec = decimals(code);
    let scale = 10i64.pow(dec);
    let neg = minor < 0;
    let mag = (minor as i128).unsigned_abs();
    let whole = (mag / scale as u128).to_string();
    let frac = (mag % scale as u128) as i64;

    let grouped = if code == "INR" { group_indian(&whole) } else { group_western(&whole) };
    let mut out = String::new();
    if neg {
        out.push('-');
    }
    match symbol(code) {
        Some(sym) => out.push_str(sym),
        None => {
            out.push_str(code);
            out.push(' ');
        }
    }
    out.push_str(&grouped);
    if frac != 0 {
        out.push('.');
        out.push_str(&format!("{:0width$}", frac, width = dec as usize));
    }
    out
}

/// Format as an estimate, which the design requires be visually distinct from a
/// posted fact: `~₹3,190`.
pub fn format_estimate(minor: i64, code: &str) -> String {
    format!("~{}", format_minor(minor, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conversion_truncates_and_is_stable() {
        // 100.00 USD at 83.60 -> 8360.00 INR
        assert_eq!(convert(10_000, 83_600_000), 836_000);
        // 1 cent at 83.60 -> 83.6 paise, truncated to 83.
        assert_eq!(convert(1, 83_600_000), 83);
        assert_eq!(convert(12_345, identity_rate()), 12_345);
    }

    #[test]
    fn conversion_does_not_overflow_at_crore_scale() {
        // ₹10 crore worth of USD at a weak-rupee rate. In i64 the intermediate
        // product is 8.36e19 and wraps; in i128 it does not.
        let amount = 1_000_000_000_00i64; // 100 crore minor units
        assert_eq!(convert(amount, 83_600_000), 8_360_000_000_00);
    }

    #[test]
    fn a_thousand_additions_lose_nothing() {
        let sum: i64 = std::iter::repeat_n(33i64, 1_000).sum();
        assert_eq!(sum, 33_000, "integer paise cannot drift");
    }

    #[test]
    fn a_thousand_conversions_are_each_exact() {
        // Every row converts independently at post time, so the sum of the
        // converted rows must equal 1000x the converted row.
        let one = convert(199, 83_600_000);
        let total: i64 = std::iter::repeat_n(one, 1_000).sum();
        assert_eq!(total, one * 1_000);
    }

    #[test]
    fn round_to_nearest_ten_rupees() {
        assert_eq!(round_to(318_900, 1_000), 319_000);
        assert_eq!(round_to(318_400, 1_000), 318_000);
        assert_eq!(round_to(318_500, 1_000), 319_000);
        assert_eq!(round_to(-318_500, 1_000), -319_000);
        assert_eq!(round_to(4_242, 1), 4_242);
    }

    #[test]
    fn parses_what_people_and_banks_write() {
        assert_eq!(parse_amount("1,24,300", "INR").unwrap(), 12_430_000);
        assert_eq!(parse_amount("₹649", "INR").unwrap(), 64_900);
        assert_eq!(parse_amount(" 40.5 ", "INR").unwrap(), 4_050);
        assert_eq!(parse_amount("-40", "INR").unwrap(), -4_000);
        assert_eq!(parse_amount("(1,234.00)", "INR").unwrap(), -123_400);
        assert_eq!(parse_amount("₹-40", "INR").unwrap(), -4_000);
        assert_eq!(parse_amount("1,234,567.89", "USD").unwrap(), 123_456_789);
        assert_eq!(parse_amount("12.", "INR").unwrap(), 1_200);
    }

    #[test]
    fn extra_fractional_digits_round_rather_than_vanish() {
        assert_eq!(parse_amount("40.555", "INR").unwrap(), 4_056);
        assert_eq!(parse_amount("40.554", "INR").unwrap(), 4_055);
        assert_eq!(parse_amount("-40.555", "INR").unwrap(), -4_056);
    }

    #[test]
    fn zero_decimal_currencies_do_not_gain_two_digits() {
        assert_eq!(parse_amount("5000", "JPY").unwrap(), 5_000);
        assert_eq!(format_minor(5_000, "JPY"), "¥5,000");
        assert_eq!(parse_amount("5,000", "KWD").unwrap(), 5_000_000);
    }

    #[test]
    fn rejects_nonsense() {
        assert!(parse_amount("", "INR").is_err());
        assert!(parse_amount("abc", "INR").is_err());
        assert!(parse_amount("1.2.3", "INR").is_err());
    }

    #[test]
    fn rates_parse_to_millionths() {
        assert_eq!(parse_rate("83.60").unwrap(), 83_600_000);
        assert_eq!(parse_rate("1").unwrap(), RATE_ONE);
        assert_eq!(parse_rate("0.0123").unwrap(), 12_300);
    }

    #[test]
    fn inr_groups_the_indian_way() {
        assert_eq!(format_minor(12_430_000, "INR"), "₹1,24,300");
        assert_eq!(format_minor(354_800_000, "INR"), "₹35,48,000");
        assert_eq!(format_minor(64_900, "INR"), "₹649");
        assert_eq!(format_minor(328_450, "INR"), "₹3,284.50");
        assert_eq!(format_minor(-4_000, "INR"), "-₹40");
        assert_eq!(format_minor(0, "INR"), "₹0");
        assert_eq!(format_minor(1_000_000_000_00, "INR"), "₹1,00,00,00,000");
    }

    #[test]
    fn other_currencies_group_in_threes() {
        assert_eq!(format_minor(123_456_789, "USD"), "$1,234,567.89");
        assert_eq!(format_minor(1_200, "CHF"), "CHF 12");
    }

    #[test]
    fn estimates_are_marked_as_estimates() {
        assert_eq!(format_estimate(319_000, "INR"), "~₹3,190");
    }
}
