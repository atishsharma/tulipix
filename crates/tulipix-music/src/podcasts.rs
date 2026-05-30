//! `np.p4.music.podcasts` — RSS subscription, episode list, download manager.
//!
//! A dependency-light RSS reader: extracts the channel title and per-episode
//! guid / enclosure URL / pubDate / duration without pulling an XML crate
//! (podcast feeds are flat enough). Persists subscriptions + episodes and
//! tracks downloaded paths for auto-cleanup.

use anyhow::Result;
use sqlx::SqlitePool;

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedEpisode {
    pub guid: String,
    pub title: String,
    pub audio_url: String,
    pub duration_s: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedFeed {
    pub title: Option<String>,
    pub episodes: Vec<ParsedEpisode>,
}

/// Text between the first `<tag ...>` and its closing `</tag>` within `hay`.
fn tag_text<'a>(hay: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}");
    let start = hay.find(&open)?;
    let after_open = hay[start..].find('>')? + start + 1;
    let close = hay[after_open..].find(&format!("</{tag}>"))? + after_open;
    Some(hay[after_open..close].trim())
}

/// Strip a leading `<![CDATA[ ... ]]>` wrapper.
fn uncdata(s: &str) -> String {
    s.trim().trim_start_matches("<![CDATA[").trim_end_matches("]]>").trim().to_string()
}

/// Value of `attr="..."` inside the first `<tag ...>` of `hay`.
fn tag_attr(hay: &str, tag: &str, attr: &str) -> Option<String> {
    let start = hay.find(&format!("<{tag}"))?;
    let slice = &hay[start..];
    let end = slice.find('>')?;
    let open = &slice[..end];
    let key = format!("{attr}=\"");
    let ai = open.find(&key)? + key.len();
    let rest = &open[ai..];
    let vend = rest.find('"')?;
    Some(rest[..vend].to_string())
}

/// Parse `HH:MM:SS`, `MM:SS`, or bare seconds → seconds.
pub fn parse_itunes_duration(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    let nums: Option<Vec<f64>> = parts.iter().map(|p| p.parse::<f64>().ok()).collect();
    let nums = nums?;
    Some(match nums.as_slice() {
        [s] => *s,
        [m, s] => m * 60.0 + s,
        [h, m, s] => h * 3600.0 + m * 60.0 + s,
        _ => return None,
    })
}

pub fn parse_feed(xml: &str) -> ParsedFeed {
    let title = tag_text(xml, "title").map(uncdata);
    let mut episodes = Vec::new();
    let mut rest = xml;
    while let Some(s) = rest.find("<item") {
        let after = &rest[s..];
        let Some(e) = after.find("</item>") else { break };
        let block = &after[..e + "</item>".len()];
        if let Some(url) = tag_attr(block, "enclosure", "url") {
            let guid = tag_text(block, "guid").map(uncdata).unwrap_or_else(|| url.clone());
            let etitle = tag_text(block, "title").map(uncdata).unwrap_or_default();
            let dur = tag_text(block, "itunes:duration").and_then(parse_itunes_duration);
            episodes.push(ParsedEpisode { guid, title: etitle, audio_url: url, duration_s: dur });
        }
        rest = &after[e + "</item>".len()..];
    }
    ParsedFeed { title, episodes }
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Subscribe (or refresh) a feed and upsert its episodes. Returns podcast id.
pub async fn subscribe(pool: &SqlitePool, feed_url: &str, feed: &ParsedFeed) -> Result<i64> {
    sqlx::query("INSERT INTO podcasts (feed_url, title, last_checked) VALUES (?,?,?) ON CONFLICT(feed_url) DO UPDATE SET title=excluded.title, last_checked=excluded.last_checked")
        .bind(feed_url).bind(&feed.title).bind(now()).execute(pool).await?;
    let pid: i64 = sqlx::query_scalar("SELECT id FROM podcasts WHERE feed_url = ?").bind(feed_url).fetch_one(pool).await?;
    for ep in &feed.episodes {
        sqlx::query("INSERT INTO podcast_episodes (podcast_id, guid, title, audio_url, duration_s) VALUES (?,?,?,?,?) ON CONFLICT(podcast_id, guid) DO NOTHING")
            .bind(pid).bind(&ep.guid).bind(&ep.title).bind(&ep.audio_url).bind(ep.duration_s).execute(pool).await?;
    }
    Ok(pid)
}

pub async fn mark_downloaded(pool: &SqlitePool, episode_id: i64, path: &str) -> Result<()> {
    sqlx::query("UPDATE podcast_episodes SET downloaded_path = ? WHERE id = ?").bind(path).bind(episode_id).execute(pool).await?;
    Ok(())
}

/// Played-and-downloaded episode ids older than `cutoff` plays — the
/// auto-cleanup candidates.
pub async fn cleanup_candidates(pool: &SqlitePool) -> Result<Vec<(i64, String)>> {
    Ok(sqlx::query_as("SELECT id, downloaded_path FROM podcast_episodes WHERE played = 1 AND downloaded_path IS NOT NULL")
        .fetch_all(pool).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::tests::open_pool;

    const FEED: &str = r#"<rss><channel><title>My Show</title>
      <item><title><![CDATA[Ep 1]]></title><guid>g1</guid>
        <enclosure url="https://x/1.mp3" type="audio/mpeg"/>
        <itunes:duration>01:02:03</itunes:duration></item>
      <item><title>Ep 2</title><guid>g2</guid>
        <enclosure url="https://x/2.mp3" type="audio/mpeg"/></item>
    </channel></rss>"#;

    #[test]
    fn parses_channel_and_items() {
        let f = parse_feed(FEED);
        assert_eq!(f.title.as_deref(), Some("My Show"));
        assert_eq!(f.episodes.len(), 2);
        assert_eq!(f.episodes[0].audio_url, "https://x/1.mp3");
        assert_eq!(f.episodes[0].duration_s, Some(3723.0));
        assert_eq!(f.episodes[1].guid, "g2");
    }

    #[test]
    fn duration_formats() {
        assert_eq!(parse_itunes_duration("90"), Some(90.0));
        assert_eq!(parse_itunes_duration("2:30"), Some(150.0));
        assert_eq!(parse_itunes_duration("1:00:00"), Some(3600.0));
    }

    #[tokio::test]
    async fn subscribe_upserts_episodes() {
        let (_t, pool) = open_pool().await;
        let f = parse_feed(FEED);
        let pid = subscribe(&pool, "https://x/feed.xml", &f).await.unwrap();
        // re-subscribe = no duplicates
        subscribe(&pool, "https://x/feed.xml", &f).await.unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM podcast_episodes WHERE podcast_id = ?").bind(pid).fetch_one(&pool).await.unwrap();
        assert_eq!(n, 2);
    }
}
