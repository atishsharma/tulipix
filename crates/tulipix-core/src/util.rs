//! Tiny helpers shared across the section crates — single home for the
//! epoch/date/url-encoding snippets that used to be copy-pasted per file.

use std::time::{SystemTime, UNIX_EPOCH};

/// Seconds since the Unix epoch (0 if the clock reads before 1970).
pub fn unix_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `unix_secs` as `i64` — the sign the DB timestamp columns use.
pub fn unix_secs_i64() -> i64 {
    unix_secs() as i64
}

/// A byte count as a short label: `"842 B"`, `"4.20 KB"`, `"12.5 MB"`, `"1.5 TB"`.
///
/// Precision scales with magnitude — two decimals under 10, one under 100, none
/// above — so the string stays about the same width whatever the number.
///
/// This is the *short* form, for lists and rows. `tulipix_common::human_size`
/// is a different thing despite the similar name: it renders the long
/// properties-panel form, `"3.25 GB (3,489,660,928 bytes)"`.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if value >= 100.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else if value >= 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.2} {}", UNITS[unit])
    }
}

/// Howard Hinnant's days_from_civil — Unix-epoch days from y-m-d, valid for
/// the full Gregorian range. Returns negative for dates before 1970-01-01.
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y / 400 } else { (y - 399) / 400 };
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Percent-encode everything outside the RFC 3986 unreserved set (space → %20).
pub fn url_encode(s: &str) -> String {
    s.bytes().map(|b| match b {
        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
        b' ' => "%20".to_string(),
        _ => format!("%{b:02X}"),
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn days_from_civil_epoch_anchors() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 1, 1), 10957);
        assert_eq!(days_from_civil(2024, 2, 29), 19782); // leap year handled
    }

    #[test]
    fn url_encode_unreserved_and_space() {
        assert_eq!(url_encode("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(url_encode("a b/c"), "a%20b%2Fc");
    }
}
