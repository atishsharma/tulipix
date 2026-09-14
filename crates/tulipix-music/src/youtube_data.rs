//! YouTube Data API v3 — the richer half of the YouTube tab.
//!
//! Without a key, the tab searches through yt-dlp (`yt_search`), which
//! answers with a title, a channel and a duration and nothing else, at the
//! cost of a subprocess per search. With a key, one HTTPS call comes back
//! with the view count, the published date and a real thumbnail, which is
//! what the cards want to draw.
//!
//! Two calls, not one: `search.list` costs 100 units of the 10,000-a-day
//! quota and does not return statistics, so the ids it gives are handed to
//! `videos.list` (1 unit) for the rest. That is ~101 units a search, near
//! enough 99 searches a day on the free quota.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const YT_BASE: &str = "https://www.googleapis.com/youtube/v3";
/// What one search costs against the 10,000-unit daily quota.
pub const YT_SEARCH_COST: u32 = 101;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct YtVideo {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    /// Seconds. 0 for a live stream, which has no duration yet.
    pub duration_s: i64,
    pub views: i64,
    /// "2024-11-01T12:00:00Z", as the API sends it.
    pub published: String,
    pub thumbnail: Option<String>,
    pub live: bool,
}

impl YtVideo {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

pub struct YoutubeDataClient {
    key: String,
    http: reqwest::Client,
}

impl YoutubeDataClient {
    pub fn new(key: impl Into<String>) -> Self {
        Self { key: key.into(), http: tulipix_core::net::http().clone() }
    }

    /// Search, then fill in what search does not return.
    pub async fn search(&self, query: &str, limit: u32) -> Result<Vec<YtVideo>> {
        if query.trim().is_empty() {
            return Ok(Vec::new());
        }
        let max = limit.clamp(1, 50).to_string();
        let v: serde_json::Value = self
            .http
            .get(format!("{YT_BASE}/search"))
            .query(&[
                ("part", "snippet"),
                ("type", "video"),
                ("maxResults", max.as_str()),
                ("q", query.trim()),
                ("key", self.key.as_str()),
            ])
            .send()
            .await?
            .error_for_status()
            .context("YouTube turned the key down, or the day's quota is spent")?
            .json()
            .await?;
        let ids = search_ids(&v);
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        self.videos(&ids).await
    }

    /// The full record for a list of ids, in the order asked for.
    pub async fn videos(&self, ids: &[String]) -> Result<Vec<YtVideo>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let joined = ids.join(",");
        let v: serde_json::Value = self
            .http
            .get(format!("{YT_BASE}/videos"))
            .query(&[
                ("part", "snippet,contentDetails,statistics"),
                ("id", joined.as_str()),
                ("key", self.key.as_str()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let mut found = parse_videos(&v);
        // `videos.list` answers in its own order; the search's ranking is the
        // one worth keeping.
        found.sort_by_key(|x| ids.iter().position(|i| *i == x.id).unwrap_or(usize::MAX));
        Ok(found)
    }

    /// One cheap call, for Settings' Test button. `videos.list` on a video
    /// that has existed since 2005 costs a single unit.
    pub async fn check(&self) -> Result<bool> {
        let resp = self
            .http
            .get(format!("{YT_BASE}/videos"))
            .query(&[("part", "id"), ("id", "jNQXAC9IVRw"), ("key", self.key.as_str())])
            .send()
            .await?;
        Ok(resp.status().is_success())
    }
}

fn search_ids(v: &serde_json::Value) -> Vec<String> {
    v["items"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|i| i["id"]["videoId"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn parse_videos(v: &serde_json::Value) -> Vec<YtVideo> {
    v["items"]
        .as_array()
        .map(|a| a.iter().map(parse_video).collect())
        .unwrap_or_default()
}

fn parse_video(i: &serde_json::Value) -> YtVideo {
    let snip = &i["snippet"];
    let live = snip["liveBroadcastContent"].as_str().unwrap_or("none") == "live";
    YtVideo {
        id: i["id"].as_str().unwrap_or_default().to_string(),
        title: snip["title"].as_str().unwrap_or_default().to_string(),
        channel: snip["channelTitle"].as_str().unwrap_or_default().to_string(),
        channel_id: snip["channelId"].as_str().unwrap_or_default().to_string(),
        duration_s: i["contentDetails"]["duration"]
            .as_str()
            .map(parse_iso_duration)
            .unwrap_or(0),
        views: i["statistics"]["viewCount"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        published: snip["publishedAt"].as_str().unwrap_or_default().to_string(),
        // "high" is 480x360, which is the size the cards draw. The map always
        // carries "default"; "high" is missing only on very old uploads.
        thumbnail: ["high", "medium", "default"]
            .iter()
            .find_map(|k| snip["thumbnails"][*k]["url"].as_str())
            .map(str::to_string),
        live,
    }
}

/// ISO-8601 durations, the shape YouTube sends: `PT4M13S`, `PT1H2M`, `P1DT2H`.
/// Anything it cannot read is 0, which the cards draw as "live" or blank
/// rather than as a wrong length.
pub fn parse_iso_duration(s: &str) -> i64 {
    let Some(rest) = s.strip_prefix('P') else { return 0 };
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total = 0i64;
    let mut num = String::new();
    for c in date.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: i64 = num.parse().unwrap_or(0);
        num.clear();
        total += match c {
            'D' => n * 86_400,
            'W' => n * 604_800,
            // Months and years have no fixed length; a video is never that
            // long, so they are ignored rather than guessed at.
            _ => 0,
        };
    }
    num.clear();
    for c in time.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            continue;
        }
        let n: i64 = num.parse().unwrap_or(0);
        num.clear();
        total += match c {
            'H' => n * 3600,
            'M' => n * 60,
            'S' => n,
            _ => 0,
        };
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_durations_in_every_shape_youtube_sends() {
        assert_eq!(parse_iso_duration("PT4M13S"), 253);
        assert_eq!(parse_iso_duration("PT1H2M"), 3720);
        assert_eq!(parse_iso_duration("PT45S"), 45);
        assert_eq!(parse_iso_duration("P1DT2H"), 93_600);
        // A live stream sends P0D, and anything unreadable is 0 rather than
        // a wrong number.
        assert_eq!(parse_iso_duration("P0D"), 0);
        assert_eq!(parse_iso_duration("nonsense"), 0);
    }

    #[test]
    fn search_gives_up_its_ids() {
        let v = serde_json::json!({"items":[
            {"id":{"videoId":"a1"}},
            {"id":{"channelId":"c1"}},
            {"id":{"videoId":"b2"}}
        ]});
        assert_eq!(search_ids(&v), vec!["a1".to_string(), "b2".to_string()]);
    }

    #[test]
    fn a_video_payload_reads_whole() {
        let v = serde_json::json!({"items":[{
            "id":"a1",
            "snippet":{
                "title":"Autobahn","channelTitle":"Kraftwerk","channelId":"UC1",
                "publishedAt":"2014-01-02T03:04:05Z","liveBroadcastContent":"none",
                "thumbnails":{"default":{"url":"https://t/d.jpg"},"high":{"url":"https://t/h.jpg"}}
            },
            "contentDetails":{"duration":"PT22M43S"},
            "statistics":{"viewCount":"1234567"}
        }]});
        let vids = parse_videos(&v);
        assert_eq!(vids.len(), 1);
        assert_eq!(vids[0].duration_s, 1363);
        assert_eq!(vids[0].views, 1_234_567);
        // "high" beats "default" when both are there.
        assert_eq!(vids[0].thumbnail.as_deref(), Some("https://t/h.jpg"));
        assert!(!vids[0].live);
        assert_eq!(vids[0].url(), "https://www.youtube.com/watch?v=a1");
    }
}
