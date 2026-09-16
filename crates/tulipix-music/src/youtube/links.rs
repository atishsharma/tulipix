//! What a pasted YouTube link points at.
//!
//! The search box takes links as well as words, so this answers "is this a
//! link, and to what" before anything goes to the network. Only the shapes
//! YouTube itself hands out are recognised; anything else is a search.

use reqwest::Url;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum YtLink {
    Video { id: String },
    Playlist { list: String },
    Channel { id: String },
    Handle { handle: String },
}

impl YtLink {
    /// `video` | `playlist` | `channel`: what the search box offers to open.
    /// A handle is a channel to anyone reading the row.
    pub fn kind(&self) -> &'static str {
        match self {
            YtLink::Video { .. } => "video",
            YtLink::Playlist { .. } => "playlist",
            YtLink::Channel { .. } | YtLink::Handle { .. } => "channel",
        }
    }
}

/// Eleven characters of `[A-Za-z0-9_-]`, which is every video id YouTube issues.
fn video_id(s: &str) -> Option<String> {
    (s.len() == 11 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'))
        .then(|| s.to_string())
}

fn handle(s: &str) -> Option<String> {
    let h = s.strip_prefix('@')?;
    (!h.is_empty() && h.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.')))
        .then(|| h.to_string())
}

pub fn parse_link(text: &str) -> Option<YtLink> {
    let text = text.trim();
    if text.contains(char::is_whitespace) {
        return None;
    }
    if let Some(h) = handle(text) {
        return Some(YtLink::Handle { handle: h });
    }
    // `youtu.be/…` and `www.youtube.com/…` get pasted without a scheme.
    let url = Url::parse(text)
        .or_else(|_| Url::parse(&format!("https://{text}")))
        .ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.trim_start_matches("www.").trim_start_matches("m.");
    let mut segments = url.path_segments()?.filter(|s| !s.is_empty());
    let first = segments.next();
    let query = |key: &str| url.query_pairs().find(|(k, _)| k == key).map(|(_, v)| v.into_owned());

    match host {
        "youtu.be" => video_id(first?).map(|id| YtLink::Video { id }),
        "youtube.com" | "music.youtube.com" | "youtube-nocookie.com" => match first? {
            "watch" => query("v")
                .and_then(|v| video_id(&v))
                .map(|id| YtLink::Video { id })
                // `watch?list=…` with no video is a playlist.
                .or_else(|| query("list").map(|list| YtLink::Playlist { list })),
            "shorts" | "live" | "embed" | "v" => {
                video_id(segments.next()?).map(|id| YtLink::Video { id })
            }
            "playlist" => query("list").map(|list| YtLink::Playlist { list }),
            "channel" => segments
                .next()
                .filter(|id| id.starts_with("UC"))
                .map(|id| YtLink::Channel { id: id.to_string() }),
            other => handle(other).map(|h| YtLink::Handle { handle: h }),
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn video(id: &str) -> Option<YtLink> {
        Some(YtLink::Video { id: id.into() })
    }

    #[test]
    fn every_video_shape() {
        assert_eq!(parse_link("https://www.youtube.com/watch?v=dQw4w9WgXcQ"), video("dQw4w9WgXcQ"));
        assert_eq!(parse_link("https://www.youtube.com/watch?v=dQw4w9WgXcQ&list=PL123&t=42s"), video("dQw4w9WgXcQ"));
        assert_eq!(parse_link("https://youtu.be/jNQXAC9IVRw?t=5"), video("jNQXAC9IVRw"));
        assert_eq!(parse_link("youtu.be/jNQXAC9IVRw"), video("jNQXAC9IVRw"));
        assert_eq!(parse_link("m.youtube.com/watch?v=dQw4w9WgXcQ"), video("dQw4w9WgXcQ"));
        assert_eq!(parse_link("https://music.youtube.com/watch?v=dQw4w9WgXcQ&si=x"), video("dQw4w9WgXcQ"));
        assert_eq!(parse_link("https://www.youtube.com/shorts/dQw4w9WgXcQ"), video("dQw4w9WgXcQ"));
        assert_eq!(parse_link("https://www.youtube.com/live/dQw4w9WgXcQ?feature=share"), video("dQw4w9WgXcQ"));
        assert_eq!(parse_link("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"), video("dQw4w9WgXcQ"));
    }

    #[test]
    fn playlists_channels_and_handles() {
        let pl = Some(YtLink::Playlist { list: "PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf".into() });
        assert_eq!(parse_link("https://www.youtube.com/playlist?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf"), pl);
        assert_eq!(parse_link("https://www.youtube.com/watch?list=PLrAXtmErZgOeiKm4sgNOknGvNjby9efdf"), pl);
        assert_eq!(
            parse_link("https://www.youtube.com/channel/UC4QobU6STFB0P71PMvOGN5A/videos"),
            Some(YtLink::Channel { id: "UC4QobU6STFB0P71PMvOGN5A".into() })
        );
        let jawed = Some(YtLink::Handle { handle: "jawed".into() });
        assert_eq!(parse_link("https://www.youtube.com/@jawed"), jawed);
        assert_eq!(parse_link("youtube.com/@jawed/videos"), jawed);
        assert_eq!(parse_link("@jawed"), jawed);
        assert_eq!(parse_link("@jawed").unwrap().kind(), "channel");
    }

    #[test]
    fn words_and_other_sites_are_searches() {
        assert_eq!(parse_link("lofi rain"), None);
        assert_eq!(parse_link("dQw4w9WgXcQ"), None);
        assert_eq!(parse_link("https://vimeo.com/123"), None);
        assert_eq!(parse_link("https://www.youtube.com/watch?v=short"), None);
        assert_eq!(parse_link("https://www.youtube.com/channel/notachannel"), None);
        assert_eq!(parse_link("https://www.youtube.com/feed/subscriptions"), None);
        assert_eq!(parse_link("ftp://youtu.be/dQw4w9WgXcQ"), None);
        assert_eq!(parse_link("@"), None);
        assert_eq!(parse_link(""), None);
    }
}
