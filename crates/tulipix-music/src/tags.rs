//! `np.p4.music.tags` — metadata read.
//!
//! Production reads tags via `symphonia` (pure-Rust) or TagLib bindings; this
//! module owns the normalized [`TrackTags`] shape both paths produce, plus the
//! fiddly normalization that bites every tagger: `"3/12"` track-of-total
//! fields, whitespace, and empty-string-as-absent. Keeping it here means the
//! symphonia/TagLib backends stay thin and this logic stays unit-tested.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackTags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub year: Option<i64>,
    pub track_no: Option<i64>,
    pub disc_no: Option<i64>,
    pub duration_s: Option<f64>,
    pub bitrate: Option<i64>,
    pub sample_rate: Option<i64>,
    pub channels: Option<i64>,
    pub codec: Option<String>,
    pub container: Option<String>,
}

/// Trim, then treat empty as absent.
pub fn clean(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t.to_string()) }
}

/// Parse a `"track"` or `"track/total"` field → track number only.
pub fn parse_track_no(s: &str) -> Option<i64> {
    let head = s.split('/').next()?.trim();
    head.parse::<i64>().ok().filter(|n| *n >= 0)
}

/// Parse a year out of a date tag that may be `"1998"`, `"1998-05-02"` or
/// `"1998/05"`. Only a plausible 4-digit year is accepted.
pub fn parse_year(s: &str) -> Option<i64> {
    let head: String = s.trim().chars().take(4).collect();
    head.parse::<i64>().ok().filter(|y| (1000..=9999).contains(y))
}

/// Container/codec guess from extension when the demuxer can't say. The real
/// backend prefers probe data; this is the fallback for the audit table.
pub fn container_from_ext(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "flac" => "flac",
        "mp3"  => "mp3",
        "m4a" | "aac" | "alac" => "mp4",
        "ogg"  => "ogg",
        "opus" => "opus",
        "wav"  => "wav",
        "aiff" | "aif" => "aiff",
        "dsf" | "dff"  => "dsd",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn track_field_handles_slash_total() {
        assert_eq!(parse_track_no("3/12"), Some(3));
        assert_eq!(parse_track_no("7"), Some(7));
        assert_eq!(parse_track_no(" 4 "), Some(4));
        assert_eq!(parse_track_no("abc"), None);
    }

    #[test]
    fn year_from_various_dates() {
        assert_eq!(parse_year("1998"), Some(1998));
        assert_eq!(parse_year("1998-05-02"), Some(1998));
        assert_eq!(parse_year("not a year"), None);
        assert_eq!(parse_year("12"), None);
    }

    #[test]
    fn clean_drops_empty() {
        assert_eq!(clean("  "), None);
        assert_eq!(clean(" Hi "), Some("Hi".into()));
    }

    #[test]
    fn ext_maps_to_container() {
        assert_eq!(container_from_ext("FLAC"), Some("flac"));
        assert_eq!(container_from_ext("dsf"), Some("dsd"));
        assert_eq!(container_from_ext("xyz"), None);
    }
}
