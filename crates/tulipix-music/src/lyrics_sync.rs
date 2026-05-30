//! `np.p4.music.lyrics-sync` — manual lyrics time-tagging editor.
//!
//! Backs the mini-editor: take plain lines, stamp each with the current
//! playhead as the user taps along, nudge a whole file's timing by an offset,
//! and serialize back to canonical `[mm:ss.xx]` LRC for the local `.lrc`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimedLine {
    pub ms: Option<i64>, // None = not yet stamped
    pub text: String,
}

/// Split plain lyrics into editable, unstamped lines.
pub fn from_plain(text: &str) -> Vec<TimedLine> {
    text.lines().map(|l| TimedLine { ms: None, text: l.trim().to_string() }).collect()
}

/// Stamp `idx` with `ms` (the current playhead). No-op if out of range.
pub fn stamp(lines: &mut [TimedLine], idx: usize, ms: i64) {
    if let Some(l) = lines.get_mut(idx) { l.ms = Some(ms.max(0)); }
}

/// Shift every stamped line by `delta_ms`, clamping at zero. Used to correct a
/// constant lead/lag against the audio.
pub fn shift_all(lines: &mut [TimedLine], delta_ms: i64) {
    for l in lines.iter_mut() {
        if let Some(ms) = l.ms { l.ms = Some((ms + delta_ms).max(0)); }
    }
}

/// Format milliseconds as `[mm:ss.xx]`.
pub fn fmt_ts(ms: i64) -> String {
    let total_cs = ms / 10; // centiseconds
    let cs = total_cs % 100;
    let secs = (total_cs / 100) % 60;
    let mins = total_cs / 6000;
    format!("[{mins:02}:{secs:02}.{cs:02}]")
}

/// Serialize stamped lines to LRC text (unstamped lines emitted without a tag).
pub fn to_lrc(lines: &[TimedLine]) -> String {
    lines.iter().map(|l| match l.ms {
        Some(ms) => format!("{}{}", fmt_ts(ms), l.text),
        None => l.text.clone(),
    }).collect::<Vec<_>>().join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamp_then_serialize() {
        let mut lines = from_plain("one\ntwo");
        stamp(&mut lines, 0, 1000);
        stamp(&mut lines, 1, 3450);
        assert_eq!(to_lrc(&lines), "[00:01.00]one\n[00:03.45]two");
    }

    #[test]
    fn shift_clamps_at_zero() {
        let mut lines = from_plain("a\nb");
        stamp(&mut lines, 0, 500);
        stamp(&mut lines, 1, 5000);
        shift_all(&mut lines, -1000);
        assert_eq!(lines[0].ms, Some(0));
        assert_eq!(lines[1].ms, Some(4000));
    }

    #[test]
    fn ts_format_minutes() {
        assert_eq!(fmt_ts(83_450), "[01:23.45]");
    }
}
