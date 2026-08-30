//! `np.p4.tools.ytdlp-update` — the Tools section's view of the yt-dlp
//! self-update.
//!
//! The version arithmetic and the weekly timer moved to
//! `tulipix_core::ytdlp`, because the updater that needs them lives in
//! `tulipix-core` and this crate sits above it. They are re-exported here so
//! the Tools call sites and their tests keep the names they had.
//!
//! What stays: the argv for yt-dlp's *own* `-U`, which is a different thing
//! from the app's updater and is kept for the one case it still suits — a
//! yt-dlp the user installed themselves, on PATH, that they would rather have
//! update itself in place.

pub use tulipix_core::ytdlp::{CHECK_INTERVAL_S, is_check_due, is_newer, version_args};

/// argv for yt-dlp's built-in self-update (optionally to a channel like
/// "nightly").
///
/// Not what the app's Tools button runs any more. `-U` overwrites the running
/// binary in place, which fails silently on a packaged install where
/// `resources/bin/` is not writable — and failing silently is exactly how the
/// bundled copy went eight weeks stale. `tulipix_core::updater::update_ytdlp_now`
/// downloads to a location it has checked it can write, so that is what the
/// button calls.
pub fn update_args(channel: Option<&str>) -> Vec<String> {
    match channel {
        Some(c) => vec!["--update-to".into(), c.into()],
        None => vec!["-U".into()],
    }
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
