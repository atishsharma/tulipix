//! SponsorBlock while playing: which stretches of a video to jump over.
//!
//! Downloads cut segments out with yt-dlp's own `--sponsorblock-remove`; a
//! stream cannot be cut, so the player seeks past them instead. The segments
//! come from the public SponsorBlock API.

use serde_json::Value;

pub const API: &str = "https://sponsor.ajay.app/api/skipSegments";

/// What playback skips: paid promotion, self-promotion, and "like and
/// subscribe" breaks. Intros and outros are left to the viewer.
pub const CATEGORIES: &str = r#"["sponsor","selfpromo","interaction"]"#;

/// `(start, end)` in seconds, from the API's answer. Anything unreadable is
/// no segments: a video nobody has tagged answers 404, which is the same.
pub fn parse_segments(body: &str) -> Vec<(f64, f64)> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(body) else { return Vec::new() };
    let mut out: Vec<(f64, f64)> = items
        .iter()
        .filter(|s| s["actionType"].as_str().is_none_or(|a| a == "skip"))
        .filter_map(|s| {
            let seg = s["segment"].as_array()?;
            let (a, b) = (seg.first()?.as_f64()?, seg.get(1)?.as_f64()?);
            (b - a >= 1.0).then_some((a, b))
        })
        .collect();
    out.sort_by(|x, y| x.0.total_cmp(&y.0));
    out
}

/// Where to jump to from `pos`, if `pos` is inside a segment. The last half
/// second of a segment is left alone: a seek that lands just short of the end
/// would otherwise be asked again on the next report.
pub fn skip_to(segments: &[(f64, f64)], pos: f64) -> Option<f64> {
    segments
        .iter()
        .find(|(a, b)| pos >= *a && pos < *b - 0.5)
        .map(|(_, b)| *b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segments_are_read_and_skipped() {
        let body = r#"[
            {"segment": [60.0, 90.5], "category": "sponsor", "actionType": "skip"},
            {"segment": [10.0, 20.0], "category": "selfpromo"},
            {"segment": [30.0, 30.4], "category": "sponsor", "actionType": "skip"},
            {"segment": [40.0, 50.0], "category": "sponsor", "actionType": "mute"}
        ]"#;
        let s = parse_segments(body);
        assert_eq!(s, [(10.0, 20.0), (60.0, 90.5)], "sorted, no slivers, no mutes");
        assert_eq!(skip_to(&s, 12.0), Some(20.0));
        assert_eq!(skip_to(&s, 19.8), None, "already at the end");
        assert_eq!(skip_to(&s, 25.0), None);
        assert!(parse_segments("Not Found").is_empty());
    }
}
