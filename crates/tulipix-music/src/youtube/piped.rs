//! Piped API client — URL builders + response parsers. Network calls are thin
//! wrappers over these pure functions, so the parsing is unit-tested offline.

use anyhow::Result;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub duration: i64, // seconds (0 = unknown)
    pub views: i64,
    pub uploaded: String, // "8 years ago"
    pub blurb: String,    // shortDescription
    pub thumbnail: String, // remote URL
    pub is_short: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelPage {
    pub name: String,
    pub avatar: String,
    pub subscribers: i64,
    pub videos: Vec<Video>,
    pub nextpage: Option<String>,
}

/// Minimal percent-encoding for a query path segment / value: encode anything
/// outside `A-Za-z0-9-_.~`. Keeps tests deterministic without pulling urlencoding.
fn enc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

fn trim_slash(base: &str) -> &str {
    base.trim_end_matches('/')
}

pub fn search_url(base: &str, query: &str) -> String {
    format!("{}/search?q={}&filter=videos", trim_slash(base), enc(query))
}

pub fn channel_url(base: &str, channel_id: &str) -> String {
    format!("{}/channel/{}", trim_slash(base), channel_id)
}

pub fn nextpage_channel_url(base: &str, channel_id: &str, token: &str) -> String {
    format!(
        "{}/nextpage/channel/{}?nextpage={}",
        trim_slash(base),
        channel_id,
        enc(token)
    )
}

// ---- raw serde shapes ----
#[derive(Deserialize)]
struct RawVideo {
    #[serde(default)]
    url: String,
    #[serde(default)]
    title: String,
    #[serde(default, rename = "uploaderName")]
    uploader_name: String,
    #[serde(default)]
    duration: i64,
    #[serde(default)]
    views: i64,
    #[serde(default, rename = "uploadedDate")]
    uploaded_date: String,
    #[serde(default, rename = "shortDescription")]
    short_description: String,
    #[serde(default)]
    thumbnail: String,
    #[serde(default, rename = "isShort")]
    is_short: bool,
}

#[derive(Deserialize)]
struct RawSearch {
    #[serde(default)]
    items: Vec<RawVideo>,
    #[serde(default)]
    nextpage: Option<String>,
}

#[derive(Deserialize)]
struct RawChannel {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "avatarUrl")]
    avatar_url: String,
    #[serde(default, rename = "subscriberCount")]
    subscriber_count: i64,
    #[serde(default, rename = "relatedStreams")]
    related_streams: Vec<RawVideo>,
    #[serde(default)]
    nextpage: Option<String>,
}

/// Extract the video id from a Piped `/watch?v=ID` url (or pass through).
fn id_from_url(url: &str) -> String {
    url.rsplit("v=")
        .next()
        .unwrap_or(url)
        .split('&')
        .next()
        .unwrap_or(url)
        .to_string()
}

fn to_video(r: RawVideo) -> Video {
    Video {
        id: id_from_url(&r.url),
        title: r.title,
        channel: r.uploader_name,
        duration: r.duration.max(0),
        views: r.views.max(0),
        uploaded: r.uploaded_date,
        blurb: r.short_description,
        thumbnail: r.thumbnail,
        is_short: r.is_short,
    }
}

pub fn parse_search(body: &str) -> Result<ChannelPage> {
    let raw: RawSearch = serde_json::from_str(body)?;
    Ok(ChannelPage {
        name: String::new(),
        avatar: String::new(),
        subscribers: 0,
        videos: raw.items.into_iter().map(to_video).collect(),
        nextpage: raw.nextpage,
    })
}

pub fn parse_channel(body: &str) -> Result<ChannelPage> {
    let raw: RawChannel = serde_json::from_str(body)?;
    Ok(ChannelPage {
        name: raw.name,
        avatar: raw.avatar_url,
        subscribers: raw.subscriber_count,
        videos: raw.related_streams.into_iter().map(to_video).collect(),
        nextpage: raw.nextpage,
    })
}

/// A video is a Short if Piped flags it, or its duration is a known value
/// in (0, 60]. Unknown duration (0) is kept — better to show than to hide.
pub fn is_video_short(v: &Video) -> bool {
    v.is_short || (v.duration > 0 && v.duration <= 60)
}

/// Drop Shorts from a video list.
pub fn without_shorts(videos: Vec<Video>) -> Vec<Video> {
    videos.into_iter().filter(|v| !is_video_short(v)).collect()
}

// ---- network wrappers ----

/// HTTP GET → body text, mapping non-2xx to an error.
async fn get_text(client: &reqwest::Client, url: &str) -> Result<String> {
    Ok(client.get(url).send().await?.error_for_status()?.text().await?)
}

/// Search via Piped; on any failure fall back to yt-dlp `ytsearch`. Shorts removed.
pub async fn search(client: &reqwest::Client, instance: &str, query: &str) -> Result<ChannelPage> {
    match get_text(client, &search_url(instance, query))
        .await
        .and_then(|b| parse_search(&b))
    {
        Ok(mut page) => {
            page.videos = without_shorts(page.videos);
            Ok(page)
        }
        Err(_) => yt_dlp_search_fallback(query).await,
    }
}

/// First channel page via Piped (no fallback — channel browsing is Piped-only).
pub async fn channel(client: &reqwest::Client, instance: &str, channel_id: &str) -> Result<ChannelPage> {
    let mut c = parse_channel(&get_text(client, &channel_url(instance, channel_id)).await?)?;
    c.videos = without_shorts(c.videos);
    Ok(c)
}

/// Next channel page via Piped nextpage token.
pub async fn channel_next(
    client: &reqwest::Client,
    instance: &str,
    channel_id: &str,
    token: &str,
) -> Result<ChannelPage> {
    let mut c = parse_channel(&get_text(client, &nextpage_channel_url(instance, channel_id, token)).await?)?;
    c.videos = without_shorts(c.videos);
    Ok(c)
}

/// yt-dlp `ytsearch20:` fallback → adapt SearchHit into Video (no blurb/views).
async fn yt_dlp_search_fallback(query: &str) -> Result<ChannelPage> {
    let args = crate::yt_search::search_args(crate::yt_search::Source::YouTube, query, 20);
    let bin = tulipix_core::thumbs::tool_bin("yt-dlp");
    let out = tokio::process::Command::new(bin).args(&args).output().await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let hits = crate::yt_search::parse_dump_json(&stdout);
    let videos = hits
        .into_iter()
        .map(|h| Video {
            id: h.id,
            title: h.title,
            channel: String::new(),
            duration: h.duration.unwrap_or(0.0) as i64,
            views: 0,
            uploaded: String::new(),
            blurb: String::new(),
            thumbnail: String::new(),
            is_short: false,
        })
        .collect();
    Ok(ChannelPage {
        name: String::new(),
        avatar: String::new(),
        subscribers: 0,
        videos: without_shorts(videos),
        nextpage: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_search_url() {
        let u = search_url("https://pi.example", "miles davis");
        assert_eq!(u, "https://pi.example/search?q=miles%20davis&filter=videos");
    }

    #[test]
    fn builds_channel_urls() {
        assert_eq!(
            channel_url("https://pi.example", "UC123"),
            "https://pi.example/channel/UC123"
        );
        assert_eq!(
            nextpage_channel_url("https://pi.example", "UC123", "TOK%2B"),
            "https://pi.example/nextpage/channel/UC123?nextpage=TOK%252B"
        );
    }

    #[test]
    fn parses_search_videos() {
        let body = r#"{"items":[
            {"url":"/watch?v=abc","title":"One","uploaderName":"Chan","duration":252,
             "views":312000,"uploadedDate":"8 years ago","shortDescription":"desc",
             "thumbnail":"https://t/1.jpg","isShort":false},
            {"url":"/watch?v=sh","title":"Short","uploaderName":"Chan","duration":30,
             "views":1,"uploadedDate":"1 day ago","shortDescription":"",
             "thumbnail":"https://t/2.jpg","isShort":true}
        ],"nextpage":"NX"}"#;
        let page = parse_search(body).unwrap();
        assert_eq!(page.videos.len(), 2);
        assert_eq!(page.videos[0].id, "abc");
        assert_eq!(page.videos[0].channel, "Chan");
        assert_eq!(page.videos[0].duration, 252);
        assert!(page.videos[1].is_short);
        assert_eq!(page.nextpage.as_deref(), Some("NX"));
    }

    #[test]
    fn parses_channel_page() {
        let body = r#"{"name":"Chan","avatarUrl":"https://a/av.jpg","subscriberCount":4200000,
            "relatedStreams":[{"url":"/watch?v=x","title":"V","uploaderName":"Chan",
            "duration":100,"views":5,"uploadedDate":"now","shortDescription":"d",
            "thumbnail":"https://t/x.jpg","isShort":false}],"nextpage":"NX2"}"#;
        let c = parse_channel(body).unwrap();
        assert_eq!(c.name, "Chan");
        assert_eq!(c.avatar, "https://a/av.jpg");
        assert_eq!(c.subscribers, 4200000);
        assert_eq!(c.videos.len(), 1);
        assert_eq!(c.videos[0].id, "x");
        assert_eq!(c.nextpage.as_deref(), Some("NX2"));
    }

    #[test]
    fn filters_shorts() {
        let mk = |dur: i64, short: bool| Video {
            id: "x".into(),
            title: "t".into(),
            channel: "c".into(),
            duration: dur,
            views: 0,
            uploaded: String::new(),
            blurb: String::new(),
            thumbnail: String::new(),
            is_short: short,
        };
        assert!(!is_video_short(&mk(252, false)));
        assert!(is_video_short(&mk(30, true)));
        assert!(is_video_short(&mk(45, false)));
        assert!(!is_video_short(&mk(0, false)));
    }
}
