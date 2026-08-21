//! Opening and ending timestamps, from AniSkip's public v2 API.
//!
//! The API is keyed on MyAnimeList ids. AniList hands `idMal` back on the same
//! query as everything else (`anilist::MEDIA_FIELDS`), so there is no id-mapping
//! layer here — a title either arrived with a MAL id or it did not, and without
//! one the feature is simply absent rather than approximated.

use anyhow::{Context, Result};
use serde_json::Value;

const BASE: &str = "https://api.aniskip.com/v2";

/// One span to skip, in seconds from the start of the episode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    pub start: f64,
    pub end: f64,
    /// "op" or "ed" — the UI says which one it skipped.
    pub kind: &'static str,
}

/// Opening and ending for one episode. Empty when AniSkip has no data, which is
/// common for anything recent.
pub async fn spans(mal_id: i64, episode: i64, length_secs: f64) -> Result<Vec<Span>> {
    if mal_id <= 0 || episode <= 0 {
        return Ok(Vec::new());
    }
    let url = format!(
        "{BASE}/skip-times/{mal_id}/{episode}?types=op&types=ed&episodeLength={}",
        length_secs.max(0.0).round() as i64
    );
    let resp = tulipix_core::net::http()
        .get(&url)
        .send()
        .await
        .context("aniskip: request failed")?;
    // 404 is the normal "nobody has timed this episode" answer, not a fault.
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(Vec::new());
    }
    if !resp.status().is_success() {
        anyhow::bail!("aniskip: HTTP {}", resp.status().as_u16());
    }
    let v: Value = resp.json().await.context("aniskip: bad JSON")?;
    Ok(parse(&v))
}

fn parse(v: &Value) -> Vec<Span> {
    if !v.get("found").and_then(Value::as_bool).unwrap_or(false) {
        return Vec::new();
    }
    let Some(results) = v.get("results").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for r in results {
        let kind = match r.get("skipType").and_then(Value::as_str) {
            Some("op") => "op",
            Some("ed") => "ed",
            _ => continue,
        };
        let Some(iv) = r.get("interval") else { continue };
        let start = iv.get("startTime").and_then(Value::as_f64).unwrap_or(-1.0);
        let end = iv.get("endTime").and_then(Value::as_f64).unwrap_or(-1.0);
        // A zero-length or reversed span would make mpv seek backwards forever.
        if start < 0.0 || end <= start {
            continue;
        }
        out.push(Span { start, end, kind });
    }
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// mpv arguments that skip the opening.
///
/// Only the opening is turned into a `--start`: skipping the ending by seeking
/// would end playback early, and "the credits rolled" is not a bug worth
/// introducing. The ending span is still returned above for the autoplay
/// countdown to start on.
pub fn mpv_args(spans: &[Span], resume: Option<f64>) -> Vec<String> {
    let already = resume.unwrap_or(0.0);
    spans
        .iter()
        .find(|s| s.kind == "op" && s.start <= already.max(1.0) + 1.0 && already < s.end)
        .map(|s| vec![format!("--start={}", s.end)])
        .unwrap_or_default()
}

/// Where the ending starts, for the autoplay countdown.
pub fn credits_at(spans: &[Span]) -> Option<f64> {
    spans.iter().find(|s| s.kind == "ed").map(|s| s.start)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(found: bool) -> Value {
        serde_json::json!({
            "found": found,
            "results": [
                { "skipType": "ed", "interval": { "startTime": 1320.0, "endTime": 1410.0 } },
                { "skipType": "op", "interval": { "startTime": 42.0, "endTime": 132.0 } }
            ]
        })
    }

    #[test]
    fn not_found_is_empty_not_an_error() {
        assert!(parse(&body(false)).is_empty());
    }

    #[test]
    fn spans_come_back_in_time_order() {
        let s = parse(&body(true));
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].kind, "op");
        assert_eq!(s[1].kind, "ed");
    }

    #[test]
    fn reversed_or_empty_intervals_are_dropped() {
        let v = serde_json::json!({"found": true, "results": [
            { "skipType": "op", "interval": { "startTime": 90.0, "endTime": 90.0 } },
            { "skipType": "op", "interval": { "startTime": 90.0, "endTime": 10.0 } }
        ]});
        assert!(parse(&v).is_empty());
    }

    #[test]
    fn opening_is_skipped_from_the_top_but_not_mid_episode() {
        let s = parse(&body(true));
        assert_eq!(mpv_args(&s, None), vec!["--start=132".to_string()]);
        // Resuming past the opening must not yank the viewer back to it.
        assert!(mpv_args(&s, Some(600.0)).is_empty());
    }

    #[test]
    fn credits_time_drives_the_countdown() {
        assert_eq!(credits_at(&parse(&body(true))), Some(1320.0));
        assert_eq!(credits_at(&[]), None);
    }
}
