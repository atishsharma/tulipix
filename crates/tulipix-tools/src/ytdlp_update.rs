//! `np.p4.tools.ytdlp-update` — in-app yt-dlp self-update.
//!
//! yt-dlp breaks often as sites change, so it updates on its own channel,
//! separate from app updates. This owns the `--update`/`--version` argv, the
//! "is a check due?" timer, and a semantic-ish version comparison so we only
//! flag a newer build.

/// argv to print the installed version.
pub fn version_args() -> Vec<String> { vec!["--version".into()] }

/// argv to self-update (optionally to a channel like "nightly").
pub fn update_args(channel: Option<&str>) -> Vec<String> {
    match channel {
        Some(c) => vec!["--update-to".into(), c.into()],
        None => vec!["-U".into()],
    }
}

pub const CHECK_INTERVAL_S: i64 = 7 * 86_400; // weekly

pub fn is_check_due(last_check_unix: i64, now_unix: i64) -> bool {
    now_unix >= last_check_unix + CHECK_INTERVAL_S
}

/// Is `remote` newer than `local`? yt-dlp versions are date-based
/// `YYYY.MM.DD[.N]`; compare component-wise numerically.
pub fn is_newer(local: &str, remote: &str) -> bool {
    fn parts(v: &str) -> Vec<u64> { v.split('.').map(|p| p.parse().unwrap_or(0)).collect() }
    let (l, r) = (parts(local), parts(remote));
    for i in 0..l.len().max(r.len()) {
        let a = l.get(i).copied().unwrap_or(0);
        let b = r.get(i).copied().unwrap_or(0);
        if b != a { return b > a; }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("2024.01.01", "2024.03.10"));
        assert!(is_newer("2024.03.10", "2024.03.10.1"));
        assert!(!is_newer("2024.03.10", "2024.03.10"));
        assert!(!is_newer("2024.04.01", "2024.03.10"));
    }

    #[test]
    fn timer_and_argv() {
        assert!(is_check_due(0, CHECK_INTERVAL_S));
        assert!(!is_check_due(0, CHECK_INTERVAL_S - 1));
        assert_eq!(update_args(None), vec!["-U".to_string()]);
        assert!(update_args(Some("nightly")).contains(&"nightly".to_string()));
    }
}
