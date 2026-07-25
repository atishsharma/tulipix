//! `Range: bytes=…` — what lets a phone resume a 4 GB download after the screen
//! locks. Returns an inclusive `(start, end)` pair, or `None` when the header is
//! malformed or cannot be satisfied, in which case the caller sends the whole
//! file with 200 rather than failing.

pub fn parse(header: &str, len: u64) -> Option<(u64, u64)> {
    if len == 0 {
        return None;
    }
    let spec = header.strip_prefix("bytes=")?.trim();
    let (from, to) = spec.split_once('-')?;

    let range = match (from.trim(), to.trim()) {
        // bytes=-N — the trailing N bytes.
        ("", n) => {
            let n: u64 = n.parse().ok()?;
            (len.saturating_sub(n.min(len)), len - 1)
        }
        // bytes=N- — from N to the end.
        (s, "") => {
            let start: u64 = s.parse().ok()?;
            (start, len - 1)
        }
        // bytes=N-M — inclusive both ends, clamped to the file.
        (s, e) => {
            let start: u64 = s.parse().ok()?;
            let end: u64 = e.parse().ok()?;
            (start, end.min(len - 1))
        }
    };

    (range.0 <= range.1 && range.0 < len).then_some(range)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_ended_range_runs_to_the_last_byte() {
        assert_eq!(parse("bytes=0-", 1000), Some((0, 999)));
        assert_eq!(parse("bytes=500-", 1000), Some((500, 999)));
    }

    #[test]
    fn closed_range_is_inclusive_at_both_ends() {
        assert_eq!(parse("bytes=100-199", 1000), Some((100, 199)));
    }

    #[test]
    fn suffix_range_counts_back_from_the_end() {
        assert_eq!(parse("bytes=-500", 1000), Some((500, 999)));
        // Asking for more than exists yields the whole file, not an error.
        assert_eq!(parse("bytes=-5000", 1000), Some((0, 999)));
    }

    #[test]
    fn a_range_past_the_end_is_rejected() {
        assert_eq!(parse("bytes=1000-", 1000), None);
        assert_eq!(parse("bytes=2000-3000", 1000), None);
    }

    #[test]
    fn malformed_headers_are_rejected_rather_than_guessed_at() {
        assert_eq!(parse("items=0-99", 1000), None);
        assert_eq!(parse("bytes=abc-def", 1000), None);
        assert_eq!(parse("bytes=", 1000), None);
        assert_eq!(parse("bytes=50-10", 1000), None); // end before start
    }

    #[test]
    fn an_empty_file_has_no_satisfiable_range() {
        assert_eq!(parse("bytes=0-", 0), None);
    }
}
